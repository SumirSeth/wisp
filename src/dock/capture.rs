//! Live window previews through Windows.Graphics.Capture.
//!
//! DWM thumbnails are black for windows with `WS_EX_NOREDIRECTIONBITMAP`
//! (Chromium/Electron apps). WGC captures the composed output instead. WGC
//! needs an MTA apartment with a pumped message loop, so it runs on a worker
//! thread and hands finished frames back to the dock thread.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::UI::WindowId;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, SelectObject,
};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PM_REMOVE, PeekMessageW};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, PW_RENDERFULLCONTENT};
use windows::core::{Interface, Ref};

use crate::indexer::IconPixels;

use super::thumbnails::Thumbnails;

enum Command {
    Start {
        hwnd: isize,
        popup: usize,
        width: u32,
        height: u32,
    },
    Stop(isize),
    Clear,
}

struct Frame {
    hwnd: isize,
    pixels: Arc<IconPixels>,
}

struct Stream {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: i64,
}

/// A WGC worker plus the frames it has produced, read by the dock thread.
pub struct Capture {
    commands: Sender<Command>,
    frames: Receiver<Frame>,
    latest: HashMap<isize, Arc<IconPixels>>,
}

impl Capture {
    pub fn new() -> Option<Self> {
        let (command_tx, command_rx) = channel::<Command>();
        let (frame_tx, frame_rx) = channel::<Frame>();
        std::thread::Builder::new()
            .name("wisp-wgc".into())
            .spawn(move || worker(command_rx, frame_tx))
            .ok()?;
        Some(Self {
            commands: command_tx,
            frames: frame_rx,
            latest: HashMap::new(),
        })
    }

    pub fn start(&mut self, hwnd: isize, popup: usize, width: u32, height: u32) {
        let _ = self.commands.send(Command::Start {
            hwnd,
            popup,
            width,
            height,
        });
    }

    #[allow(dead_code)]
    pub fn stop(&mut self, hwnd: isize) {
        let _ = self.commands.send(Command::Stop(hwnd));
    }

    pub fn clear(&mut self) {
        self.latest.clear();
        let _ = self.commands.send(Command::Clear);
    }

    /// Drains frames delivered since the last call.
    pub fn poll(&mut self) {
        while let Ok(frame) = self.frames.try_recv() {
            self.latest.insert(frame.hwnd, frame.pixels);
        }
    }

    pub fn frame(&self, hwnd: isize) -> Option<Arc<IconPixels>> {
        self.latest.get(&hwnd).cloned()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Clear);
    }
}

fn worker(commands: Receiver<Command>, frames: Sender<Frame>) {
    if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        Thumbnails::debug("WGC RoInitialize", &error);
    }
    let mut streams: HashMap<isize, Stream> = HashMap::new();
    let mut device = None;
    if let Err(error) = unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            Default::default(),
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    } {
        Thumbnails::debug("D3D11CreateDevice", &error);
        return;
    }
    let Some(device) = device else { return };
    let context = match unsafe { device.GetImmediateContext() } {
        Ok(context) => context,
        Err(error) => {
            Thumbnails::debug("D3D11 context", &error);
            return;
        }
    };
    let dxgi: IDXGIDevice = match device.cast() {
        Ok(dxgi) => dxgi,
        Err(error) => {
            Thumbnails::debug("DXGI device", &error);
            return;
        }
    };
    let d3d: IDirect3DDevice =
        match unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.and_then(|value| value.cast()) {
            Ok(d3d) => d3d,
            Err(error) => {
                Thumbnails::debug("WGC device", &error);
                return;
            }
        };

    loop {
        match commands.recv_timeout(Duration::from_millis(16)) {
            Ok(Command::Start {
                hwnd,
                popup,
                width,
                height,
            }) => {
                if streams.contains_key(&hwnd) {
                    continue;
                }
                start_stream(
                    &mut streams,
                    &d3d,
                    &device,
                    &context,
                    &frames,
                    hwnd,
                    popup,
                    width,
                    height,
                );
            }
            Ok(Command::Stop(hwnd)) => stop_stream(&mut streams, hwnd),
            Ok(Command::Clear) => {
                let keys: Vec<isize> = streams.keys().copied().collect();
                for hwnd in keys {
                    stop_stream(&mut streams, hwnd);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let mut msg = Default::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            let _ = unsafe { DispatchMessageW(&msg) };
        }
    }
    let keys: Vec<isize> = streams.keys().copied().collect();
    for hwnd in keys {
        stop_stream(&mut streams, hwnd);
    }
}

#[allow(clippy::too_many_arguments)]
fn start_stream(
    streams: &mut HashMap<isize, Stream>,
    d3d: &IDirect3DDevice,
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    frames: &Sender<Frame>,
    hwnd: isize,
    popup: usize,
    width: u32,
    height: u32,
) {
    let item = match GraphicsCaptureItem::TryCreateFromWindowId(WindowId { Value: hwnd as u64 }) {
        Ok(item) => item,
        Err(error) => return Thumbnails::debug("WGC item", &error),
    };
    let full = match item.Size() {
        Ok(size) => size,
        Err(error) => return Thumbnails::debug("WGC item size", &error),
    };
    let pool = match Direct3D11CaptureFramePool::Create(d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, full) {
        Ok(pool) => pool,
        Err(error) => return Thumbnails::debug("WGC pool", &error),
    };
    let device = device.clone();
    let context = context.clone();
    let sink = frames.clone();
    let clock = Arc::new(Mutex::new(std::time::Instant::now()));
    let handler = TypedEventHandler::new(
        move |sender: Ref<Direct3D11CaptureFramePool>, _: Ref<windows::core::IInspectable>| {
            let Some(pool) = (*sender).as_ref() else { return Ok(()) };
            let frame = match pool.TryGetNextFrame() {
                Ok(frame) => frame,
                Err(error) => {
                    Thumbnails::debug("WGC TryGetNextFrame", &error);
                    return Ok(());
                }
            };
            let now = std::time::Instant::now();
            let mut previous = clock.lock().unwrap();
            if now.duration_since(*previous) < Duration::from_millis(80) {
                return Ok(());
            }
            *previous = now;
            drop(previous);
            if let Some(pixels) = copy_frame(&device, &context, &frame, width, height) {
                let _ = sink.send(Frame {
                    hwnd,
                    pixels: Arc::new(pixels),
                });
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(
                        Some(HWND(popup as *mut core::ffi::c_void)),
                        None,
                        false,
                    );
                }
            }
            Ok(())
        },
    );
    let token = match pool.FrameArrived(&handler) {
        Ok(token) => token,
        Err(error) => return Thumbnails::debug("WGC FrameArrived", &error),
    };
    let session = match pool.CreateCaptureSession(&item) {
        Ok(session) => session,
        Err(error) => {
            let _ = pool.RemoveFrameArrived(token);
            return Thumbnails::debug("WGC session", &error);
        }
    };
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(false);
    if let Err(error) = session.StartCapture() {
        let _ = pool.RemoveFrameArrived(token);
        return Thumbnails::debug("WGC StartCapture", &error);
    }
    streams.insert(hwnd, Stream { pool, session, token });
}

fn stop_stream(streams: &mut HashMap<isize, Stream>, hwnd: isize) {
    if let Some(stream) = streams.remove(&hwnd) {
        let _ = stream.session.Close();
        let _ = stream.pool.RemoveFrameArrived(stream.token);
        let _ = stream.pool.Close();
    }
}

/// Prints a window on demand, downsampled to the tile. This keeps previews
/// fresh when WGC stops delivering frames for an occluded window.
pub fn print_window(hwnd: isize, target_w: u32, target_h: u32) -> Option<IconPixels> {
    let source = HWND(hwnd as *mut core::ffi::c_void);
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(source, &mut rect) }.is_err() {
        return None;
    }
    let (sw, sh) = ((rect.right - rect.left) as u32, (rect.bottom - rect.top) as u32);
    if sw == 0 || sh == 0 {
        return None;
    }
    unsafe {
        let dc = CreateCompatibleDC(None);
        let screen = windows::Win32::Graphics::Gdi::GetDC(None);
        let bitmap = CreateCompatibleBitmap(screen, sw as i32, sh as i32);
        let _ = windows::Win32::Graphics::Gdi::ReleaseDC(None, screen);
        if dc.is_invalid() || bitmap.is_invalid() {
            if !bitmap.is_invalid() {
                let _ = DeleteObject(bitmap.into());
            }
            if !dc.is_invalid() {
                let _ = DeleteDC(dc);
            }
            return None;
        }
        let old = SelectObject(dc, bitmap.into());
        let printed = windows::Win32::Storage::Xps::PrintWindow(
            source,
            dc,
            windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT),
        )
        .as_bool();
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: sw as i32,
                biHeight: -(sh as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (sw * sh * 4) as usize];
        let lines = if printed {
            GetDIBits(
                dc,
                bitmap,
                0,
                sh,
                Some(bgra.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        if !old.is_invalid() {
            let _ = SelectObject(dc, old);
        }
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        if lines == 0 {
            return None;
        }
        let (dw, dh) = (target_w as usize, target_h as usize);
        let mut out = vec![0u8; dw * dh * 4];
        for y in 0..dh {
            let sy = (y * sh as usize / dh).min(sh as usize - 1);
            for x in 0..dw {
                let sx = (x * sw as usize / dw).min(sw as usize - 1);
                let src = (sy * sw as usize + sx) * 4;
                let dst = (y * dw + x) * 4;
                out[dst..dst + 4].copy_from_slice(&bgra[src..src + 4]);
            }
        }
        Some(IconPixels {
            width: target_w,
            height: target_h,
            bgra: out,
        })
    }
}

/// Copies a WGC frame into CPU-readable BGRA pixels, downsampled to the tile.
fn copy_frame(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    target_w: u32,
    target_h: u32,
) -> Option<IconPixels> {
    unsafe {
        let surface: windows::Graphics::DirectX::Direct3D11::IDirect3DSurface = frame
            .Surface()
            .inspect_err(|e| Thumbnails::debug("WGC frame surface", e))
            .ok()?;
        let access: IDirect3DDxgiInterfaceAccess = surface
            .cast()
            .inspect_err(|e| Thumbnails::debug("WGC surface access", e))
            .ok()?;
        let texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D = access
            .GetInterface()
            .inspect_err(|e| Thumbnails::debug("WGC texture access", e))
            .ok()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let mut staging_desc = desc;
        staging_desc.Usage = D3D11_USAGE_STAGING;
        staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        staging_desc.BindFlags = Default::default();
        staging_desc.MiscFlags = Default::default();
        let mut staging = None;
        device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .inspect_err(|e| Thumbnails::debug("WGC staging texture", e))
            .ok()?;
        let staging = staging?;
        context.CopyResource(&staging, &texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .inspect_err(|e| Thumbnails::debug("WGC Map", e))
            .ok()?;
        let (sw, sh) = (desc.Width as usize, desc.Height as usize);
        let (dw, dh) = (target_w as usize, target_h as usize);
        let mut bgra = vec![0u8; dw * dh * 4];
        for y in 0..dh {
            let sy = (y * sh / dh).min(sh - 1);
            let src_row = (mapped.pData as *const u8).add(sy * mapped.RowPitch as usize);
            for x in 0..dw {
                let sx = (x * sw / dw).min(sw - 1);
                let src = src_row.add(sx * 4);
                let dst = &mut bgra[(y * dw + x) * 4..(y * dw + x) * 4 + 4];
                std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), 4);
            }
        }
        context.Unmap(&staging, 0);
        Some(IconPixels {
            width: target_w,
            height: target_h,
            bgra,
        })
    }
}
