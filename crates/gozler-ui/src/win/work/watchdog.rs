use super::OffThread;
use crate::core::work::watchdog::{HEARTBEAT, LOG_BYTES, Watchdog, retain_log};
use crate::win::watchdog::Delivery;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::Instant;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows::Win32::System::Threading::{OpenThread, THREAD_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;
use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, IsIconic, IsWindowVisible};
use windows::core::BOOL;

/// The work owner retains this worker and drops it at close, without joining.
/// [`crate::win::watchdog::Endpoint`] closes delivery before destroying its HWND.
pub(crate) struct Thread {
    stop: Option<SyncSender<()>>,
    _thread: JoinHandle<()>,
}

impl Thread {
    pub(crate) fn start(delivery: Delivery, path: PathBuf) -> Self {
        let (stop, stopped) = sync_channel(1);
        let thread = super::spawn(move |worker| run(delivery, path, stopped, worker));
        Self {
            stop: Some(stop),
            _thread: thread,
        }
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        drop(self.stop.take());
    }
}

struct UiThread(HANDLE);

impl UiThread {
    fn open(id: u32, _worker: &OffThread) -> Option<Self> {
        // SAFETY: only querying a thread's cycle counter; this owned handle
        // remains valid even after its original numeric thread ID is reused.
        unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, id) }
            .ok()
            .map(Self)
    }

    fn cycles(&self, _worker: &OffThread) -> Option<u64> {
        let mut cycles = 0;
        // SAFETY: the owned thread handle and writable counter outlive the call.
        unsafe { QueryThreadCycleTime(self.0, &mut cycles) }
            .ok()
            .map(|()| cycles)
    }
}

impl Drop for UiThread {
    fn drop(&mut self) {
        // SAFETY: this worker is the only owner of the query handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn run(delivery: Delivery, path: PathBuf, stopped: Receiver<()>, worker: &OffThread) {
    let thread = UiThread::open(delivery.thread_id, worker);
    let origin = Instant::now();
    let mut watchdog = Watchdog::default();
    let mut log = Log { path, bytes: None };
    loop {
        if !matches!(
            stopped.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ) {
            break;
        }
        if any_visible(delivery.thread_id, worker) {
            watchdog.acknowledge(delivery.acknowledged());
            let context = delivery.context();
            let cycles = thread.as_ref().and_then(|thread| thread.cycles(worker));
            let now = origin.elapsed();
            let sequence = watchdog.heartbeat(now, cycles);
            if !delivery.notify(sequence, worker) {
                watchdog.reset();
            } else {
                watchdog.acknowledge(delivery.acknowledged());
                if let Some(stall) = watchdog.overdue(now, cycles, context) {
                    let _ = log.append(stall.line().as_bytes(), worker);
                }
            }
        } else {
            watchdog.reset();
        }
        match stopped.recv_timeout(HEARTBEAT) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => break,
        }
    }
}

fn any_visible(thread_id: u32, _worker: &OffThread) -> bool {
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: synchronous enumeration retains the caller's local; this
        // callback never exposes its borrow outside this call.
        let visible = unsafe { &mut *(data.0 as *mut bool) };
        // SAFETY: only native flags are queried; no Rust pointer or control
        // message crosses the UI-thread boundary.
        *visible = unsafe { IsWindowVisible(window).as_bool() && !IsIconic(window).as_bool() };
        BOOL::from(!*visible)
    }
    let mut visible = false;
    // SAFETY: the local outlives this synchronous enumeration of the captured
    // UI thread's nonchild windows, including native system dialogs and menus.
    let _ = unsafe {
        EnumThreadWindows(
            thread_id,
            Some(visit),
            LPARAM(std::ptr::from_mut(&mut visible) as isize),
        )
    };
    visible
}

struct Log {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

impl Log {
    fn append(&mut self, line: &[u8], _worker: &OffThread) -> std::io::Result<()> {
        if self.bytes.is_none() {
            let mut bytes = Vec::with_capacity(LOG_BYTES);
            match File::open(&self.path) {
                Ok(mut file) => {
                    let length = file.seek(SeekFrom::End(0))?;
                    file.seek(SeekFrom::Start(length.saturating_sub(LOG_BYTES as u64)))?;
                    file.take(LOG_BYTES as u64).read_to_end(&mut bytes)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            self.bytes = Some(bytes);
        }
        let bytes = self
            .bytes
            .as_mut()
            .expect("the first append initializes retained bytes");
        retain_log(bytes, line);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?
            .write_all(bytes)
    }
}
