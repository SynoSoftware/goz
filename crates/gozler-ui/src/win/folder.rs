//! The panels' disk threads (DESIGN.md: A panel): each panel's reading of its
//! folder, which then watches it, the free space and the drive labels. Each
//! runs on a thread of its own that the window never joins or waits for,
//! because a network share that stopped answering can hold a call for a minute:
//! a newer reading ends the older one with its stop signal and
//! `CancelSynchronousIo`, as the engine's threads are ended, and a thread wakes
//! the window only while the window is open (DESIGN.md: window rule 5). The
//! threads answer with the core's plain values, which `core::panel::Panels`
//! applies.
//!
//! A reading opens the watch before it lists the folder, so Windows keeps
//! every change from before the listing on, and the thread reports them
//! after the listing, which they then apply to. The watch's handle is also
//! registered for the removal of its drive, so Windows asks before Safely
//! Remove takes the drive, and the window then closes the handle at once
//! ([`PanelThreads::removal`]).

use super::Hwnd;
use super::wake::OpenWaker;
use crate::core::panel::{
    Entry, FolderChange, FolderSort, ReadAsk, ReadEvent, ReadFailure, Side, SortedFolders,
    give_sizes, join, sort_listing,
};
use gozler_core::layout::{NotifyAction, walk_full_listing_buffer, walk_notify_buffer};
use gozler_core::winpath::{is_root, parent_of};
use gozler_winfs::{
    Access, FileIdentity, FolderWatch, Reparse, Signal, ThreadIoHandle, WatchEvent, WatchRelease,
    WinError, cancel_synchronous_io, current_thread_io_handle, open_entry, watch_folder,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use windows::Win32::Foundation::{
    ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, HANDLE,
};
use windows::Win32::Storage::FileSystem::{GetLogicalDrives, GetVolumeInformationW};
use windows::Win32::UI::WindowsAndMessaging::{
    DBT_DEVTYP_HANDLE, DEV_BROADCAST_HANDLE, DEVICE_NOTIFY_WINDOW_HANDLE, HDEVNOTIFY,
    RegisterDeviceNotificationW, UnregisterDeviceNotification,
};
use windows::core::HSTRING;

/// One listing page, and one watch's buffer: 64 KiB, the most a network
/// share's watch takes.
const PAGE_WORDS: usize = 64 * 1024 / 8;

/// The errors with which a first wait on a watch says that Windows cannot
/// watch the folder, as on some network shares: the panel then does not
/// read it again in a loop. Any other error, as when the folder is deleted,
/// loses the watch, and the panel reads the folder again. A time does not
/// tell them apart, because a share can refuse late and a delete can come
/// at once.
const UNWATCHABLE: [u32; 3] = [
    ERROR_INVALID_FUNCTION.0,
    ERROR_NOT_SUPPORTED.0,
    ERROR_INVALID_PARAMETER.0,
];

/// How often a quiet watch checks that its folder's path still names the
/// folder, because a rename of the folder itself shows only in its parent.
const MOVED_CHECK: Duration = Duration::from_secs(2);

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_INVALID_DRIVE: u32 = 15;
const ERROR_NOT_READY: u32 = 21;
const ERROR_DEV_NOT_EXIST: u32 = 55;
const ERROR_NO_MEDIA_IN_DRIVE: u32 = 1112;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// What a disk thread found, for one panel.
pub(crate) enum PanelEvent {
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
    /// The panel's folders, sorted by gozlerd's sizes.
    FolderOrder {
        side: Side,
        sorted: SortedFolders,
    },
}

/// Asks a thread to stop: wakes its watch, and ends its wait in a
/// synchronous call.
pub(super) struct Stop {
    stopped: AtomicBool,
    thread: Mutex<Option<ThreadIoHandle>>,
    /// What a watch waits for beside its changes; `None` when Windows
    /// could not make it, and then the thread does not watch.
    signal: Option<Signal>,
}

impl Stop {
    pub(super) fn new() -> Stop {
        Stop {
            stopped: AtomicBool::new(false),
            thread: Mutex::new(None),
            signal: Signal::new().ok(),
        }
    }

    /// Names the calling thread as the one to cancel; whether it is to stop
    /// already.
    pub(super) fn enter(&self) -> bool {
        if let Ok(handle) = current_thread_io_handle() {
            *self.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        }
        self.stopped()
    }

    pub(super) fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(signal) = &self.signal {
            signal.set();
        }
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            let _ = cancel_synchronous_io(thread);
        }
    }
}

/// What the window and a reading's thread share: the watch's release, once
/// the thread has opened it, and its registration for the drive's removal.
#[derive(Default)]
struct Watched {
    release: Option<WatchRelease>,
    notification: Option<isize>,
}

/// One panel's reading, which goes on as the watch of its folder.
struct Reading {
    /// The reading's generation, which its watch's events carry.
    generation: u64,
    stop: Arc<Stop>,
    shared: Arc<Mutex<Watched>>,
    /// The drive's removal was asked, so the handle is closed and only the
    /// registration waits for the answer.
    suspended: bool,
}

impl Reading {
    fn unregister(&self) {
        let notification = self
            .shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .notification
            .take();
        unregister(notification);
    }

    /// Stops the thread, which closes its handles as it ends.
    fn end(&self) {
        self.stop.stop();
        self.unregister();
    }
}

/// What the drive's removal asks of a panel's watch
/// ([`PanelThreads::removal`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Removal {
    /// Windows asks whether the drive may go.
    Query,
    /// Another program kept the drive.
    Failed,
    /// The drive is gone.
    Complete,
}

/// The panels' threads and what they found.
pub(crate) struct PanelThreads {
    window: Hwnd,
    waker: OpenWaker,
    sender: Sender<PanelEvent>,
    events: Receiver<PanelEvent>,
    readings: [RefCell<Option<Reading>>; 2],
    frees: [RefCell<Option<Arc<Stop>>>; 2],
    labels: RefCell<Vec<Arc<Stop>>>,
}

impl PanelThreads {
    pub(crate) fn new(window: Hwnd, waker: OpenWaker) -> PanelThreads {
        let (sender, events) = channel();
        PanelThreads {
            window,
            waker,
            sender,
            events,
            readings: Default::default(),
            frees: Default::default(),
            labels: RefCell::default(),
        }
    }

    /// Everything the threads found, without waiting.
    pub(crate) fn drain(&self) -> Vec<PanelEvent> {
        self.events.try_iter().collect()
    }

    /// Reads the folder of `ask` for `side`, then watches it, and ends the
    /// reading it replaces.
    pub(crate) fn read(&self, side: Side, ask: ReadAsk) {
        self.cancel_read(side);
        let stop = Arc::new(Stop::new());
        let shared = Arc::new(Mutex::new(Watched::default()));
        let generation = ask.generation;
        self.readings[side.index()].replace(Some(Reading {
            generation,
            stop: Arc::clone(&stop),
            shared: Arc::clone(&shared),
            suspended: false,
        }));
        let (sender, waker) = (self.sender.clone(), self.waker.clone());
        let window = self.window.0.0 as isize;
        std::thread::spawn(move || {
            if stop.enter() {
                return;
            }
            let deliver = |event: PanelEvent| {
                waker.deliver(|| !stop.stopped() && sender.send(event).is_ok());
            };
            let read = |event: ReadEvent| deliver(PanelEvent::Read { side, event });
            // Registered before the listing, so a removal of the drive
            // while the folder is listed finds the panel.
            // The registration is made outside the lock, because Windows may
            // hold it while it asks the window about a removal, and the
            // window takes the lock to answer.
            let armed = |watch: &FolderWatch| {
                {
                    let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
                    if stop.stopped() {
                        return false;
                    }
                    shared.release = Some(watch.releaser());
                }
                let notification = watch
                    .handle_value()
                    .and_then(|handle| register(window, handle));
                let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
                if stop.stopped() {
                    // The window ended the reading meanwhile and found no
                    // registration to end.
                    unregister(notification);
                    return false;
                }
                shared.notification = notification;
                true
            };
            let Some((folder, identity, mut watch)) = read_folder(&ask, &stop, &read, &armed)
            else {
                return;
            };
            let Some(signal) = &stop.signal else {
                return;
            };
            follow(
                &folder,
                identity,
                &mut watch,
                signal,
                &stop,
                &|event| match event {
                    Followed::Changes(changes) => deliver(PanelEvent::Changes {
                        side,
                        reading: generation,
                        changes,
                    }),
                    Followed::Lost => deliver(PanelEvent::Lost {
                        side,
                        reading: generation,
                    }),
                },
            );
        });
    }

    /// Ends the reading or the watch of `side`, whose answers nobody reads,
    /// as when Search shows.
    pub(crate) fn cancel_read(&self, side: Side) {
        if let Some(old) = self.readings[side.index()].take() {
            old.end();
        }
    }

    /// The panel whose watch Windows names with the registration
    /// `notification` asks `removal` of it, and what the panel then does:
    /// for a query the window closes the watch's handle at once, without
    /// waiting for the thread, so Safely Remove never says the drive is in
    /// use. A listing that runs is cancelled, and its thread closes the
    /// listing's handle as it ends; the window does not close it, because
    /// the thread may be inside a call on it.
    pub(crate) fn removal(&self, notification: isize, removal: Removal) -> Option<Side> {
        let side = Side::BOTH.into_iter().find(|side| {
            self.readings[side.index()]
                .borrow()
                .as_ref()
                .is_some_and(|reading| {
                    reading
                        .shared
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .notification
                        == Some(notification)
                })
        })?;
        match removal {
            Removal::Query => {
                let mut slot = self.readings[side.index()].borrow_mut();
                let reading = slot.as_mut()?;
                reading.suspended = true;
                reading.stop.stop();
                let release = reading
                    .shared
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .release
                    .take();
                if let Some(release) = release {
                    release.release();
                }
            }
            Removal::Failed | Removal::Complete => self.cancel_read(side),
        }
        Some(side)
    }

    /// Whether `reading` is `side`'s reading and still watches, so its
    /// events are of the shown folder.
    pub(crate) fn is_reading(&self, side: Side, reading: u64) -> bool {
        self.readings[side.index()]
            .borrow()
            .as_ref()
            .is_some_and(|shown| shown.generation == reading && !shown.suspended)
    }

    /// Reads the free space of `folder`'s drive for `side`, and ends the
    /// reading it replaces.
    pub(crate) fn read_free(&self, side: Side, folder: &str) {
        let stop = Arc::new(Stop::new());
        if let Some(old) = self.frees[side.index()].replace(Some(Arc::clone(&stop))) {
            old.stop();
        }
        let (sender, waker) = (self.sender.clone(), self.waker.clone());
        let folder = folder.to_owned();
        std::thread::spawn(move || {
            if stop.enter() {
                return;
            }
            let free = gozler_winfs::free_space(&folder.encode_utf16().collect::<Vec<u16>>()).ok();
            waker.deliver(|| {
                !stop.stopped() && sender.send(PanelEvent::Free { side, folder, free }).is_ok()
            });
        });
    }

    /// Reads every drive's label, each on its own thread, because a
    /// sleeping network or optical drive can hold its call for long, and
    /// the drive menu shows the labels it has.
    pub(crate) fn read_labels(&self) {
        let mut labels = self.labels.borrow_mut();
        for old in labels.drain(..) {
            old.stop();
        }
        for letter in drives() {
            let stop = Arc::new(Stop::new());
            labels.push(Arc::clone(&stop));
            let (sender, waker) = (self.sender.clone(), self.waker.clone());
            std::thread::spawn(move || {
                if stop.enter() {
                    return;
                }
                let Some(label) = label(letter) else {
                    return;
                };
                waker.deliver(|| {
                    !stop.stopped() && sender.send(PanelEvent::Label { letter, label }).is_ok()
                });
            });
        }
    }

    /// Sorts `side`'s folders by their new sizes on a thread of its own,
    /// which touches no disk, so the window never waits for the sort.
    pub(crate) fn sort_folders(&self, side: Side, job: FolderSort) {
        let (sender, waker) = (self.sender.clone(), self.waker.clone());
        std::thread::spawn(move || {
            let sorted = job.sort();
            waker.deliver(|| {
                sender
                    .send(PanelEvent::FolderOrder { side, sorted })
                    .is_ok()
            });
        });
    }

    /// Stops every thread's wait, when Search shows and at Exit; none is
    /// joined.
    pub(crate) fn stop_all(&self) {
        for side in Side::BOTH {
            self.cancel_read(side);
            if let Some(free) = self.frees[side.index()].take() {
                free.stop();
            }
        }
        for label in self.labels.borrow_mut().drain(..) {
            label.stop();
        }
    }
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

fn label(letter: char) -> Option<String> {
    let mut name = [0u16; 261];
    // SAFETY: the root is a NUL-terminated string the call reads; the name
    // buffer outlives the call, which writes at most its length.
    unsafe {
        GetVolumeInformationW(
            &HSTRING::from(format!(r"{letter}:\")),
            Some(&mut name),
            None,
            None,
            None,
            None,
        )
    }
    .ok()?;
    let length = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..length]))
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

fn failure_of(error: &WinError) -> ReadFailure {
    match error.code {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_DRIVE => ReadFailure::Missing,
        ERROR_NOT_READY
        | ERROR_DEV_NOT_EXIST
        | ERROR_NO_MEDIA_IN_DRIVE
        | ERROR_DEVICE_NOT_CONNECTED => ReadFailure::NotReady,
        _ => ReadFailure::Other,
    }
}

pub(super) fn reason(error: &WinError) -> String {
    super::error_text(&std::io::Error::from_raw_os_error(error.code as i32))
}

/// Opens the folder of `ask`, or the folder of the file it names with the
/// file to focus, or the nearest folder above it that exists when `ask`
/// says so, opens its watch and gives it to `armed`, then lists it, sorted
/// by the panel's sort. Returns the folder, its identity and its watch,
/// which then follows it; `None` when the reading failed or stopped, which
/// `armed` answers with `false`, or the folder cannot be watched.
fn read_folder(
    ask: &ReadAsk,
    stop: &Stop,
    deliver: &dyn Fn(ReadEvent),
    armed: &dyn Fn(&FolderWatch) -> bool,
) -> Option<(String, Option<FileIdentity>, FolderWatch)> {
    let generation = ask.generation;
    let failed = |error: &WinError, path: &str| {
        deliver(ReadEvent::Failed {
            generation,
            path: path.to_owned(),
            failure: failure_of(error),
            reason: reason(error),
        });
        None
    };
    let mut path = ask.path.clone();
    let mut focus: Option<Box<[u16]>> = None;
    let folder = loop {
        match open_entry(&verbatim(&path), Access::Inspect, Reparse::Follow)
            .and_then(|entry| entry.attribute_tag())
        {
            Ok(tag) if tag.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 => {
                match open_entry(&verbatim(&path), Access::List, Reparse::Follow) {
                    Ok(folder) => break folder,
                    Err(error) => return failed(&error, &path),
                }
            }
            Ok(_) if focus.is_none() && !is_root(&path) => {
                let name = path
                    .trim_end_matches('\\')
                    .rsplit('\\')
                    .next()
                    .unwrap_or("");
                focus = Some(name.encode_utf16().collect());
                path = parent_of(&path);
            }
            Ok(_) => {
                let error = WinError {
                    code: ERROR_PATH_NOT_FOUND,
                    context: "a file's folder",
                };
                return failed(&error, &path);
            }
            Err(error)
                if ask.nearest && failure_of(&error) == ReadFailure::Missing && !is_root(&path) =>
            {
                path = parent_of(&path);
            }
            Err(error) => return failed(&error, &path),
        }
        if stop.stopped() {
            return None;
        }
    };
    // Before the listing, so a change while it runs is kept.
    let watch = watch_folder(&verbatim(&path), PAGE_WORDS).ok();
    if watch.as_ref().is_some_and(|watch| !armed(watch)) {
        return None;
    }
    deliver(ReadEvent::Opened {
        generation,
        folder: path.clone(),
    });
    let mut page = vec![0u64; PAGE_WORDS];
    let mut entries = Vec::new();
    loop {
        if stop.stopped() {
            return None;
        }
        match folder.read_listing(&mut page) {
            Ok(Some(bytes)) => match walk_full_listing_buffer(bytes) {
                Ok(listed) => entries.extend(
                    listed
                        .into_iter()
                        .filter(|e| e.name != [u16::from(b'.')] && e.name != [u16::from(b'.'); 2])
                        .map(|e| {
                            Entry::new(
                                e.name.into_boxed_slice(),
                                e.attributes,
                                e.size,
                                e.last_write,
                            )
                        }),
                ),
                Err(_) => {
                    let error = WinError {
                        code: 13,
                        context: "FILE_FULL_DIR_INFO",
                    };
                    return failed(&error, &path);
                }
            },
            Ok(None) => break,
            Err(error) => return failed(&error, &path),
        }
    }
    let identity = folder.identity().ok();
    drop(folder);
    if path == ask.path {
        give_sizes(&mut entries, &ask.sizes);
    }
    sort_listing(&mut entries, ask.sort);
    deliver(ReadEvent::Listed {
        generation,
        folder: path.clone(),
        entries,
        focus,
    });
    Some((path, identity, watch?))
}

/// The entry `name` of `folder` as it is now, under its long name when a
/// watch named it by its short (8.3) one; `None` when it is gone.
fn stat(folder: &str, name: &[u16]) -> Option<Entry> {
    let path = join(folder, &String::from_utf16_lossy(name));
    let entry = open_entry(&verbatim(&path), Access::Inspect, Reparse::Open).ok()?;
    let tag = entry.attribute_tag().ok()?;
    let stat = entry.stat().ok()?;
    // The long name when Windows gives the entry's path, else the name the
    // watch reported, so the entry never goes for want of its long name.
    let long = || {
        let full = entry.final_path().ok()?;
        let at = full.iter().rposition(|&unit| unit == u16::from(b'\\'))?;
        Some(full[at + 1..].into())
    };
    let name: Box<[u16]> = name
        .contains(&u16::from(b'~'))
        .then(long)
        .flatten()
        .unwrap_or_else(|| name.into());
    let is_folder = tag.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let size = if is_folder { 0 } else { stat.size };
    Some(Entry::new(name, tag.attributes, size, stat.last_write))
}

/// The changes a watch's buffer holds, read again from the disk by name.
fn changes(folder: &str, bytes: &[u8]) -> Option<Vec<FolderChange>> {
    let mut changes = Vec::new();
    let mut renamed: Option<Box<[u16]>> = None;
    for notified in walk_notify_buffer(bytes).ok()? {
        let name: Box<[u16]> = notified.name.into_boxed_slice();
        let now = |present: fn(Entry) -> FolderChange| match stat(folder, &name) {
            Some(entry) => present(entry),
            None => FolderChange::Gone(name.clone()),
        };
        match notified.action {
            NotifyAction::Removed => changes.push(FolderChange::Gone(name)),
            NotifyAction::RenamedFrom => {
                changes.extend(renamed.replace(name).map(FolderChange::Gone));
            }
            NotifyAction::RenamedTo => match (renamed.take(), stat(folder, &name)) {
                (Some(from), Some(to)) => changes.push(FolderChange::Renamed { from, to }),
                (Some(from), None) => changes.push(FolderChange::Gone(from)),
                (None, _) => changes.push(now(FolderChange::Added)),
            },
            NotifyAction::Added => changes.push(now(FolderChange::Added)),
            NotifyAction::Modified => changes.push(now(FolderChange::Modified)),
        }
    }
    changes.extend(renamed.map(FolderChange::Gone));
    Some(changes)
}

/// What a watch reports.
enum Followed {
    Changes(Vec<FolderChange>),
    Lost,
}

/// Reports each change of `folder` that `watch` reads, until `signal`
/// stops it, or until it loses changes or `folder` names another folder
/// than the one of `identity`, after which the panel reads the folder again
/// on a new thread.
fn follow(
    folder: &str,
    identity: Option<FileIdentity>,
    watch: &mut FolderWatch,
    signal: &Signal,
    stop: &Stop,
    deliver: &dyn Fn(Followed),
) {
    let mut watched = false;
    loop {
        let read = watch.next(signal, MOVED_CHECK);
        if stop.stopped() {
            return;
        }
        match read {
            Ok(WatchEvent::Stopped) => return,
            Err(error) if !watched && UNWATCHABLE.contains(&error.code) => return,
            Err(_) | Ok(WatchEvent::Changes([])) => return deliver(Followed::Lost),
            Ok(WatchEvent::Changes(bytes)) => match changes(folder, bytes) {
                Some(changes) => deliver(Followed::Changes(changes)),
                None => return deliver(Followed::Lost),
            },
            Ok(WatchEvent::Quiet) => {}
        }
        if moved(folder, identity) {
            return deliver(Followed::Lost);
        }
        watched = true;
    }
}

/// Whether `folder` no longer opens the folder of `identity`, as when
/// another program renamed it, so the panel's rows are another folder's.
fn moved(folder: &str, identity: Option<FileIdentity>) -> bool {
    let Some(identity) = identity else {
        return false;
    };
    open_entry(&verbatim(folder), Access::Inspect, Reparse::Follow)
        .and_then(|entry| entry.identity())
        .map_or(true, |now| now != identity)
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
    // SAFETY: `filter` outlives the call, which copies it; the window's
    // handle is valid while the window is open, and the thread runs only
    // then, because the window stops it before it is destroyed.
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
        // because whoever takes it from `Watched` ends it once.
        let _ = unsafe { UnregisterDeviceNotification(HDEVNOTIFY(notification as *mut _)) };
    }
}
