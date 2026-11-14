//! Runs the taskbar hider on a thread of its own.
//!
//! Hiding and restoring the taskbar waits on Explorer, which is slowest
//! exactly when the dock needs to redraw: while it changes the work area.
//! The launcher keeps receiving the shell's broadcasts, since only top-level
//! windows get them, and forwards them here as jobs. Results come back as
//! [`Outcome`]s, announced with [`WM_TASKBAR_RESULT`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HRESULT, Result};

use super::TaskbarHider;
pub use crate::messages::WM_TASKBAR_RESULT;

/// Wakes the worker's message loop to read its jobs.
const WM_JOB: u32 = WM_APP + 1;

enum Job {
    /// Hide the taskbar, or restore it.
    Configure(bool),
    /// Hide it again after Windows or the user showed it.
    Reapply,
    /// A shell broadcast or hook message the hider reacts to.
    Message(u32),
    KeepBottom(bool),
    Peek,
    PeekTick,
    /// Restore now and answer, for quitting.
    Restore(Sender<Result<()>>),
}

/// An error that can cross threads.
#[derive(Debug)]
pub struct Failure(HRESULT, String);

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Failure(error.code(), error.message())
    }
}

impl From<&Failure> for Error {
    fn from(failure: &Failure) -> Self {
        Error::new(failure.0, failure.1.as_str())
    }
}

pub enum Outcome {
    /// A configure or reapply finished. `message` is the message that asked
    /// for it, if any; `changed` whether the hider acted on it.
    Applied {
        message: Option<u32>,
        result: std::result::Result<bool, Failure>,
    },
    Peeked(std::result::Result<(), Failure>),
    /// True once the peek is over.
    PeekTick(std::result::Result<bool, Failure>),
}

pub struct TaskbarWorker {
    jobs: Option<Sender<Job>>,
    outcomes: Receiver<Outcome>,
    thread: u32,
    peeking: Arc<AtomicBool>,
}

impl TaskbarWorker {
    fn spawn(owner: HWND) -> Result<Self> {
        let (jobs, inbox) = channel();
        let (send, outcomes) = channel();
        let (started, thread) = channel();
        let peeking = Arc::new(AtomicBool::new(false));
        let worker_peeking = peeking.clone();
        let owner = owner.0 as usize;
        std::thread::Builder::new()
            .name("taskbar".into())
            .spawn(move || run(HWND(owner as _), inbox, send, started, worker_peeking))
            .map_err(|e| Error::new(windows::Win32::Foundation::E_FAIL, e.to_string()))?;
        let thread = thread
            .recv()
            .map_err(|_| Error::new(windows::Win32::Foundation::E_FAIL, "The taskbar thread did not start"))?;
        Ok(Self {
            jobs: Some(jobs),
            outcomes,
            thread,
            peeking,
        })
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs
            && jobs.send(job).is_ok()
        {
            unsafe {
                let _ = PostThreadMessageW(self.thread, WM_JOB, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// Starts hiding the taskbar, or restores it and lets the thread end.
    pub fn configure(slot: &mut Option<Self>, owner: HWND, enabled: bool) -> Result<()> {
        if enabled {
            if slot.is_none() {
                let worker = Self::spawn(owner)?;
                worker.send(Job::Configure(true));
                *slot = Some(worker);
            }
        } else if let Some(worker) = slot.take() {
            // The thread restores the taskbar, then ends once this is dropped.
            worker.send(Job::Configure(false));
        }
        Ok(())
    }

    pub fn reapply(&self) {
        self.send(Job::Reapply);
    }

    pub fn on_message(&self, msg: u32) {
        self.send(Job::Message(msg));
    }

    pub fn keep_bottom(&self, keep: bool) {
        self.send(Job::KeepBottom(keep));
    }

    pub fn peek(&self) {
        if self.is_active() {
            self.peeking.store(true, Ordering::Release);
            self.send(Job::Peek);
        }
    }

    pub fn peek_tick(&self) {
        self.send(Job::PeekTick);
    }

    /// Whether the taskbar is hidden, as far as the hider knows.
    pub fn is_active(&self) -> bool {
        super::ACTIVE.load(Ordering::SeqCst)
    }

    pub fn is_peeking(&self) -> bool {
        self.peeking.load(Ordering::Acquire)
    }

    /// Results of the jobs done so far.
    pub fn drain(&self) -> Vec<Outcome> {
        self.outcomes.try_iter().collect()
    }

    /// Restores the taskbar before Wisp quits, waiting a few seconds at most.
    pub fn shutdown(self) -> Result<()> {
        let (reply, answer) = channel();
        self.send(Job::Restore(reply));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match answer.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => return result,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            // Restoring the work area broadcasts WM_SETTINGCHANGE and waits for
            // every window, ours too: answer sent messages while waiting.
            unsafe {
                let mut msg = MSG::default();
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            }
        }
        // The watchdog restores the taskbar if Wisp ends first.
        Err(Error::new(
            windows::Win32::Foundation::E_FAIL,
            "The taskbar thread did not answer",
        ))
    }
}

impl Drop for TaskbarWorker {
    fn drop(&mut self) {
        // Disconnect first, so the woken thread sees there are no more jobs.
        self.jobs = None;
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_JOB, WPARAM(0), LPARAM(0));
        }
    }
}

fn run(owner: HWND, inbox: Receiver<Job>, outcomes: Sender<Outcome>, started: Sender<u32>, peeking: Arc<AtomicBool>) {
    // Create the message queue before anyone posts to it.
    let mut msg = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
    }
    let _ = started.send(unsafe { GetCurrentThreadId() });
    let mut hider: Option<TaskbarHider> = None;
    let report = |outcome: Outcome| {
        if outcomes.send(outcome).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(owner), WM_TASKBAR_RESULT, WPARAM(0), LPARAM(0));
            }
        }
    };
    loop {
        loop {
            match inbox.try_recv() {
                Ok(job) => handle(job, &mut hider, owner, &report, &peeking),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // Dropping the hider restores the taskbar.
                    drop(hider.take());
                    return;
                }
            }
        }
        // The show hook delivers its events through this loop.
        if unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 <= 0 {
            drop(hider.take());
            return;
        }
        if msg.hwnd.is_invalid() && msg.message == WM_JOB {
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn handle(job: Job, hider: &mut Option<TaskbarHider>, owner: HWND, report: &impl Fn(Outcome), peeking: &AtomicBool) {
    match job {
        Job::Configure(enabled) => report(Outcome::Applied {
            message: None,
            result: TaskbarHider::configure(hider, owner, enabled)
                .map(|()| true)
                .map_err(Failure::from),
        }),
        Job::Reapply => {
            let result = hider.as_mut().map_or(Ok(()), TaskbarHider::reapply);
            report(Outcome::Applied {
                message: None,
                result: result.map(|()| true).map_err(Failure::from),
            });
        }
        Job::Message(msg) => {
            let result = hider.as_mut().map_or(Ok(false), |h| h.on_message(msg));
            report(Outcome::Applied {
                message: Some(msg),
                result: result.map_err(Failure::from),
            });
        }
        Job::KeepBottom(keep) => {
            if let Some(hider) = hider {
                hider.keep_bottom(keep);
            }
        }
        Job::Peek => {
            let result = hider.as_mut().map_or(Ok(()), TaskbarHider::peek);
            if result.is_err() || !hider.as_ref().is_some_and(TaskbarHider::is_peeking) {
                peeking.store(false, Ordering::Release);
            }
            report(Outcome::Peeked(result.map_err(Failure::from)));
        }
        Job::PeekTick => {
            let result = hider.as_mut().map_or(Ok(true), TaskbarHider::peek_tick);
            if !matches!(result, Ok(false)) {
                peeking.store(false, Ordering::Release);
            }
            report(Outcome::PeekTick(result.map_err(Failure::from)));
        }
        Job::Restore(reply) => {
            let result = TaskbarHider::configure(hider, owner, false);
            let _ = reply.send(result);
        }
    }
}
