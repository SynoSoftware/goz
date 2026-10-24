//! Native listing and watch execution; all resource release stays off UI.

use super::{
    PanelEvent, ReadAsk, ReadEvent, Reading, Register, Registration, Side, Stop, reason, verbatim,
};
use crate::core::panel::{Entry, FolderChange, ReadFailure, give_sizes, join, sort_listing};
use crate::win::work::OffThread;
use gozler_client::CancelToken;
use gozler_core::layout::{NotifyAction, walk_full_listing_buffer, walk_notify_buffer};
use gozler_core::winpath::{is_root, parent_of};
use gozler_winfs::{
    Access, FileIdentity, FolderWatch, Reparse, Signal, WatchEvent, WinError, open_entry,
    watch_folder,
};
use std::sync::Arc;
use std::time::Duration;
use windows::Win32::Foundation::{
    ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
};

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

pub(crate) fn read(reading: Reading, _worker: &OffThread, cancel: &CancelToken) -> Registration {
    let registration = Registration(Arc::clone(&reading.notification));
    read_and_watch(reading, cancel);
    registration
}

fn read_and_watch(reading: Reading, cancel: &CancelToken) {
    let Reading {
        side,
        ask,
        stop,
        notification,
        delivery,
        registering,
        waker,
    } = reading;
    if stop.stopped() || cancel.is_cancelled() {
        return;
    }
    let generation = ask.generation;
    let deliver = |event| delivery.send(event);
    let armed = |watch: &FolderWatch| {
        if stop.stopped() {
            return false;
        }
        let Some(handle) = watch.handle_value() else {
            return false;
        };
        let (request, acknowledgment) = Register::new(handle, Arc::clone(&notification));
        let mut request = Some(request);
        let mut sent = false;
        waker.deliver(|| {
            if stop.stopped() {
                return false;
            }
            sent = registering
                .try_send(request.take().expect("registration is sent once"))
                .is_ok();
            sent
        });
        sent && acknowledgment.recv().is_ok() && !stop.stopped()
    };
    let Some((folder, identity, mut watch)) = read_folder(
        &ask,
        &stop,
        &|event| deliver(PanelEvent::Read { side, event }),
        &armed,
    ) else {
        return;
    };
    drop(ask);
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
}

pub(crate) fn free(
    (side, folder): (Side, String),
    _worker: &OffThread,
    cancel: &CancelToken,
) -> Option<PanelEvent> {
    if cancel.is_cancelled() {
        return None;
    }
    let free = gozler_winfs::free_space(&folder.encode_utf16().collect::<Vec<u16>>()).ok();
    Some(PanelEvent::Free { side, folder, free })
}

pub(crate) fn label(letter: char, worker: &OffThread, cancel: &CancelToken) -> Option<PanelEvent> {
    if cancel.is_cancelled() {
        return None;
    }
    let label = crate::win::shell::volume_label(&format!(r"{letter}:\"), worker)?;
    Some(PanelEvent::Label { letter, label })
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

/// Opens the folder of `ask`, or the folder of the file it names with the
/// file to focus, or the nearest folder above it that exists when `ask`
/// says so, opens its watch and gives it to `armed`, then lists it, sorted
/// by the panel's sort. Returns the folder, its identity and its watch,
/// which then follows it; `None` when the reading failed or stopped, which
/// `armed` answers with `false`, or the folder cannot be watched.
fn read_folder(
    ask: &ReadAsk,
    stop: &Stop,
    deliver: &dyn Fn(ReadEvent) -> bool,
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
    if !deliver(ReadEvent::Opened {
        generation,
        folder: path.clone(),
    }) {
        return None;
    }
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
        give_sizes(&mut entries, &ask.sizes.values());
    }
    sort_listing(&mut entries, ask.sort);
    if !deliver(ReadEvent::Listed {
        generation,
        folder: path.clone(),
        entries,
        focus,
    }) {
        return None;
    }
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
/// on its next admitted reading.
fn follow(
    folder: &str,
    identity: Option<FileIdentity>,
    watch: &mut FolderWatch,
    signal: &Signal,
    stop: &Stop,
    deliver: &dyn Fn(Followed) -> bool,
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
            Err(_) | Ok(WatchEvent::Changes([])) => {
                deliver(Followed::Lost);
                return;
            }
            Ok(WatchEvent::Changes(bytes)) => match changes(folder, bytes) {
                Some(changes) => {
                    if !deliver(Followed::Changes(changes)) {
                        return;
                    }
                }
                None => {
                    deliver(Followed::Lost);
                    return;
                }
            },
            Ok(WatchEvent::Quiet) => {}
        }
        if moved(folder, identity) {
            deliver(Followed::Lost);
            return;
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
