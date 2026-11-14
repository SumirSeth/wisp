//! Shell artwork stays on one COM worker; both successes and failures are cached by size.
use crate::indexer::IconPixels;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

type Key = (String, i32);
type Reply = (Key, Option<Arc<IconPixels>>);
type Worker = (mpsc::Sender<(Key, String)>, mpsc::Receiver<Reply>);
#[derive(Default)]
pub struct Artwork {
    cache: HashMap<Key, Option<Arc<IconPixels>>>,
    pending: HashSet<Key>,
    worker: Option<Worker>,
    /// Set while a WM_ARTWORK is on its way, so a batch of icons costs one relayout.
    posted: Arc<AtomicBool>,
}
impl Artwork {
    pub fn request(&mut self, hwnd: HWND, id: &str, path: &str, size: i32) -> Option<Arc<IconPixels>> {
        let key = (id.to_owned(), size);
        if let Some(pixels) = self.cache.get(&key) {
            return pixels.clone();
        }
        if self.pending.contains(&key) {
            return None;
        }
        if self.worker.is_none() {
            let (send, requests) = mpsc::channel::<(Key, String)>();
            let (replies, receive) = mpsc::channel();
            let window = hwnd.0 as usize;
            let posted = self.posted.clone();
            if std::thread::Builder::new()
                .name("dock icons".into())
                .spawn(move || {
                    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
                    for (key, path) in requests {
                        let pixels = initialized
                            .then(|| super::tray::app_pixels(&path, key.1))
                            .flatten()
                            .map(Arc::new);
                        if replies.send((key, pixels)).is_err() {
                            break;
                        }
                        if !posted.swap(true, Ordering::AcqRel) {
                            unsafe {
                                let _ = PostMessageW(Some(HWND(window as _)), super::WM_ARTWORK, WPARAM(0), LPARAM(0));
                            }
                        }
                    }
                    if initialized {
                        unsafe {
                            CoUninitialize();
                        }
                    }
                })
                .is_err()
            {
                self.cache.insert(key, None);
                return None;
            }
            self.worker = Some((send, receive));
        }
        let send = &self.worker.as_ref().unwrap().0;
        if send.send((key.clone(), path.to_owned())).is_ok() {
            self.pending.insert(key);
        } else {
            self.cache.insert(key, None);
        }
        None
    }
    pub fn drain(&mut self) {
        if let Some((_, replies)) = &self.worker {
            // Clear first: a reply landing after the loop posts a new message.
            self.posted.store(false, Ordering::Release);
            while let Ok((key, pixels)) = replies.try_recv() {
                self.pending.remove(&key);
                // Keep a bounded set of sizes when repeatedly switching monitors.
                if self.cache.len() >= 4096 {
                    self.cache.clear();
                }
                self.cache.insert(key, pixels);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_sizes_are_cached_separately() {
        let mut artwork = Artwork::default();
        artwork.cache.insert(("app".into(), 32), None);
        assert!(artwork.cache.contains_key(&("app".into(), 32)));
        assert!(!artwork.cache.contains_key(&("app".into(), 48)));
        assert!(artwork.request(HWND::default(), "app", "missing", 32).is_none());
        assert!(artwork.worker.is_none());
    }
}
