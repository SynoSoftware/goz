//! Native folder listing and watch resources. The work owner admits one
//! reading per panel; this module keeps its native handles on that worker.

use super::Hwnd;
use super::wake::OpenWaker;
use crate::core::panel::{FolderChange, ReadAsk, ReadEvent, Side};
use crate::core::work::{INLINE_ITEMS, Retirement};
use gozler_winfs::{Signal, WinError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::GetLogicalDrives;
use windows::Win32::UI::WindowsAndMessaging::{
    DBT_DEVTYP_HANDLE, DEV_BROADCAST_HANDLE, DEVICE_NOTIFY_WINDOW_HANDLE, HDEVNOTIFY,
    RegisterDeviceNotificationW, UnregisterDeviceNotification,
};

mod mailbox;
mod work;

pub(crate) use work::{free, label, read};

use mailbox::Delivery;
pub(crate) use mailbox::Mailbox;

/// What a disk thread found, for one panel.
pub(crate) enum PanelEvent {
    Register {
        side: Side,
        generation: u64,
        request: Register,
    },
    Read {
        side: Side,
        event: ReadEvent,
    },
    /// The changes of the panel's watched folder, by name, from the
    /// reading `reading`.
    Changes {
        side: Side,
        reading: u64,
        changes: Vec<FolderChange>,
    },
    /// Windows lost changes of the watched folder, or the folder can no
    /// longer be watched, as when it is deleted, so the panel reads it
    /// again.
    Lost {
        side: Side,
        reading: u64,
    },
    /// The free space of the drive that holds `folder`.
    Free {
        side: Side,
        folder: String,
        free: Option<u64>,
    },
    /// A drive's label; empty for a drive without one.
    Label {
        letter: char,
        label: String,
    },
}

/// The watch signal is independent of native synchronous-I/O cancellation.
pub(super) struct Stop {
    stopped: AtomicBool,
    signal: Option<Signal>,
}

impl Stop {
    pub(super) fn new() -> Self {
        Self {
            stopped: AtomicBool::new(false),
            signal: Signal::new().ok(),
        }
    }

    pub(super) fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(signal) = &self.signal {
            signal.set();
        }
    }
}

pub(crate) struct ReadBack {
    pub(crate) generation: u64,
    pub(crate) mailbox: Mailbox,
    stop: Arc<Stop>,
    notification: Arc<AtomicIsize>,
    registering: Option<Receiver<Register>>,
}

impl ReadBack {
    pub(crate) fn stop(&mut self) {
        self.stop.stop();
        self.registering.take();
    }

    pub(crate) fn names(&self, notification: isize) -> bool {
        notification != 0 && self.notification.load(Ordering::Acquire) == notification
    }

    pub(crate) fn take_registration(&self) -> Option<Register> {
        self.registering.as_ref()?.try_recv().ok()
    }
}

/// Carries native-resource ownership through the actual end of a reading,
/// including cancellation while a filesystem call is still blocked.
pub(crate) struct Reading {
    side: Side,
    ask: ReadAsk,
    stop: Arc<Stop>,
    notification: Arc<AtomicIsize>,
    delivery: Delivery,
    registering: SyncSender<Register>,
    waker: OpenWaker,
}

impl Reading {
    pub(crate) fn new(side: Side, ask: ReadAsk, waker: OpenWaker) -> (Self, ReadBack) {
        let stop = Arc::new(Stop::new());
        let notification = Arc::new(AtomicIsize::new(0));
        let (mailbox, delivery) = Mailbox::new(Arc::clone(&stop), waker.clone());
        let (registering, registered) = sync_channel(1);
        let back = ReadBack {
            generation: ask.generation,
            mailbox,
            stop: Arc::clone(&stop),
            notification: Arc::clone(&notification),
            registering: Some(registered),
        };
        (
            Self {
                side,
                ask,
                stop,
                notification,
                delivery,
                registering,
                waker,
            },
            back,
        )
    }
}

pub(crate) struct Registration(Arc<AtomicIsize>);

impl Drop for Registration {
    fn drop(&mut self) {
        let notification = self.0.swap(0, Ordering::AcqRel);
        unregister((notification != 0).then_some(notification));
    }
}

/// The worker retains the native handle until acknowledgment or abandonment.
/// Windows' device-notification example registers on the receiving UI thread;
/// this handoff also prevents a late worker call from targeting a reused HWND.
pub(crate) struct Register {
    handle: isize,
    notification: Arc<AtomicIsize>,
    acknowledgment: SyncSender<()>,
}

impl Register {
    pub(crate) fn new(handle: isize, notification: Arc<AtomicIsize>) -> (Self, Receiver<()>) {
        let (acknowledgment, received) = sync_channel(1);
        (
            Self {
                handle,
                notification,
                acknowledgment,
            },
            received,
        )
    }

    /// Call on the recipient's UI thread, outside model and work borrows.
    pub(crate) fn complete(self, window: Hwnd) -> Retirement {
        let notification = register(window.0.0 as isize, self.handle).unwrap_or(0);
        self.notification.store(notification, Ordering::Release);
        let mut retired = Retirement::default();
        if self.acknowledgment.try_send(()).is_err() {
            retired.push(Registration(self.notification), INLINE_ITEMS + 1);
        }
        retired
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Removal {
    Query,
    Failed,
    Complete,
}

/// The letters of the drives Windows has, which `GetLogicalDrives` reads
/// without touching any drive.
pub(crate) fn drives() -> Vec<char> {
    // SAFETY: no parameters.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| char::from(b'A' + bit))
        .collect()
}

/// `path` with the prefix that lets Windows take it longer than `MAX_PATH`.
pub(super) fn verbatim(path: &str) -> Vec<u16> {
    verbatim_wide(&path.encode_utf16().collect::<Vec<u16>>())
}

/// [`verbatim`] of a path in UTF-16, which holds every name a folder can
/// hold, also one that is not valid Unicode.
pub(super) fn verbatim_wide(path: &[u16]) -> Vec<u16> {
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().collect() };
    if path.starts_with(&wide(r"\\?\")) {
        path.to_vec()
    } else if let Some(share) = path.strip_prefix(&wide(r"\\")[..]) {
        [&wide(r"\\?\UNC\")[..], share].concat()
    } else {
        [&wide(r"\\?\")[..], path].concat()
    }
}

pub(super) fn reason(error: &WinError) -> String {
    super::error_text(&std::io::Error::from_raw_os_error(error.code as i32))
}

/// Registers `handle`, a watched folder's or a viewed file's, for its
/// drive's removal with `window`; `None` when Windows refuses, as for a
/// network share.
pub(super) fn register(window: isize, handle: isize) -> Option<isize> {
    let filter = DEV_BROADCAST_HANDLE {
        dbch_size: size_of::<DEV_BROADCAST_HANDLE>() as u32,
        dbch_devicetype: DBT_DEVTYP_HANDLE.0,
        dbch_handle: HANDLE(handle as *mut _),
        ..Default::default()
    };
    // SAFETY: `filter` outlives the call, which copies it. The caller runs on
    // the receiving window's UI thread while its handle is still valid.
    let notification = unsafe {
        RegisterDeviceNotificationW(
            HANDLE(window as *mut _),
            (&raw const filter).cast(),
            DEVICE_NOTIFY_WINDOW_HANDLE,
        )
    }
    .ok()?;
    Some(notification.0 as isize)
}

/// Ends a registration that [`register`] made, from any thread.
pub(super) fn unregister(notification: Option<isize>) {
    if let Some(notification) = notification {
        // SAFETY: a registration that `register` made and nothing ended,
        // because the owning reading or viewer ends it once.
        let _ = unsafe { UnregisterDeviceNotification(HDEVNOTIFY(notification as *mut _)) };
    }
}
