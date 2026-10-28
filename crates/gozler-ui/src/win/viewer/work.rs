//! One Viewer's file and notification lifetime stay on its reader worker.
//! The mailbox holds only the latest command and one bounded block.

use super::ViewerEvent;
use crate::core::clipboard::{Publication, RETRY_AFTER};
use crate::core::viewer::copy_text;
use crate::core::viewer_window::{BLOCK_BYTES, CopyText};
use crate::win::clipboard::{ClipboardBlock, CopyAttempt, CopyFailure};
use crate::win::folder::{Register, reason, unregister, verbatim_wide};
use crate::win::locale::ansi_text;
use crate::win::wake::OpenWaker;
use crate::win::work::OffThread;
use gozler_client::CancelToken;
use gozler_winfs::{Access, Entry, Reparse, WinError, open_entry};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use windows::Win32::Foundation::E_OUTOFMEMORY;

pub(crate) enum Command {
    Open(Vec<u16>),
    Read(u64),
    Copy(CopyText, Publication),
    Cancel,
    Release { keep_registration: bool },
}

#[derive(Default)]
struct Pending {
    generation: u64,
    command: Option<Command>,
    answer: Option<(u64, ViewerEvent)>,
    registration: Option<(u64, Register)>,
    closed: bool,
}

#[derive(Default)]
pub(crate) struct Mailbox {
    pending: Mutex<Pending>,
    changed: Condvar,
    notification: Arc<AtomicIsize>,
    active: AtomicBool,
}

impl Mailbox {
    pub(crate) fn post(&self, generation: u64, command: Command) {
        self.replace(generation, Some(command));
    }

    pub(crate) fn invalidate(&self, generation: u64) {
        self.replace(generation, None);
    }

    fn replace(&self, generation: u64, command: Option<Command>) {
        let (replaced, abandoned) = {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.closed {
                return;
            }
            pending.generation = generation;
            (
                std::mem::replace(&mut pending.command, command),
                pending.registration.take(),
            )
        };
        drop(replaced);
        drop(abandoned);
        self.changed.notify_one();
    }

    pub(crate) fn take(&self, generation: u64) -> Option<ViewerEvent> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending.closed || pending.answer.as_ref()?.0 != generation {
            return None;
        }
        pending.answer.take().map(|(_, event)| event)
    }

    pub(crate) fn is_registration(&self, notification: isize) -> bool {
        notification != 0 && self.notification.load(Ordering::Acquire) == notification
    }

    pub(crate) fn released(&self) -> bool {
        !self.active.load(Ordering::Acquire)
    }

    fn opening(&self, generation: u64) -> bool {
        let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending.closed || pending.generation != generation {
            return false;
        }
        // Query invalidation uses this same lock before checking release.
        self.active.store(true, Ordering::Release);
        true
    }

    pub(crate) fn take_registration(&self, generation: u64) -> Option<Register> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending.closed || pending.registration.as_ref()?.0 != generation {
            return None;
        }
        pending.registration.take().map(|(_, request)| request)
    }

    pub(crate) fn close(&self) {
        let abandoned = {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            pending.closed = true;
            pending.registration.take()
        };
        drop(abandoned);
        self.changed.notify_one();
    }

    fn register(&self, generation: u64, handle: isize, waker: &OpenWaker) {
        let (request, acknowledged) = Register::new(handle, Arc::clone(&self.notification));
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.closed || pending.generation != generation {
                return;
            }
            pending.registration = Some((generation, request));
        }
        waker.deliver(|| true);
        // A claimed UI request keeps the file valid even if close arrives meanwhile.
        let _ = acknowledged.recv();
    }

    fn next(&self) -> Option<(u64, Command)> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if pending.closed {
                return None;
            }
            if let Some(command) = pending.command.take() {
                return Some((pending.generation, command));
            }
            pending = self
                .changed
                .wait(pending)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn current(&self, generation: u64) -> bool {
        let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        !pending.closed && pending.generation == generation
    }

    fn deliver(&self, generation: u64, event: ViewerEvent, waker: &OpenWaker) {
        let replaced = {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.closed || pending.generation != generation {
                return;
            }
            pending.answer.replace((generation, event))
        };
        drop(replaced);
        waker.deliver(|| true);
    }
}

pub(crate) struct Session {
    pub(crate) mailbox: Arc<Mailbox>,
    pub(crate) waker: OpenWaker,
}

struct Reading {
    file: Option<Entry>,
    failed: Option<(bool, String)>,
    len: u64,
    mailbox: Arc<Mailbox>,
    /// The clipboard's sequence number after this Viewer's last copy.
    copied: AtomicU32,
}

impl Reading {
    fn release(&mut self, keep_registration: bool) {
        drop(self.file.take());
        self.mailbox.active.store(false, Ordering::Release);
        if !keep_registration {
            self.clear();
        }
    }

    fn clear(&mut self) {
        drop(self.file.take());
        self.mailbox.active.store(false, Ordering::Release);
        self.failed = None;
        let notification = self.mailbox.notification.swap(0, Ordering::AcqRel);
        unregister((notification != 0).then_some(notification));
    }

    fn open(
        &mut self,
        path: &[u16],
        generation: u64,
        current: impl Fn() -> bool,
        register: impl Fn(&Entry),
    ) -> Result<Option<ViewerEvent>, WinError> {
        self.clear();
        if !current() || !self.mailbox.opening(generation) {
            return Ok(None);
        }
        let file = open_entry(&verbatim_wide(path), Access::Read, Reparse::Follow)?;
        self.len = file.stat()?.size;
        if !current() {
            return Ok(None);
        }
        register(&file);
        self.file = Some(file);
        if !current() {
            return Ok(None);
        }
        let first = self.bytes(0)?;
        Ok(Some(ViewerEvent::Opened {
            len: self.len,
            first,
        }))
    }

    fn bytes(&self, index: u64) -> Result<Vec<u8>, WinError> {
        let file = self
            .file
            .as_ref()
            .expect("only an open reading requests bytes");
        let start = index.saturating_mul(BLOCK_BYTES as u64);
        let want = self.len.saturating_sub(start).min(BLOCK_BYTES as u64) as usize;
        let mut bytes = vec![0; want];
        let mut filled = 0;
        while filled < want {
            match file.read_at(start + filled as u64, &mut bytes[filled..])? {
                0 => break,
                read => filled += read,
            }
        }
        bytes.truncate(filled);
        Ok(bytes)
    }

    fn read(&self, index: u64) -> Result<ViewerEvent, WinError> {
        self.bytes(index)
            .map(|bytes| ViewerEvent::Block { index, bytes })
    }

    /// Reads the file whole and puts its text on the clipboard, retrying
    /// while another program holds the clipboard as the main window's Copy
    /// does; `None` when a newer command replaced it.
    fn copy(
        &self,
        copy: CopyText,
        mut publication: Publication,
        current: impl Fn() -> bool,
        cancel: &CancelToken,
        worker: &OffThread,
    ) -> Result<Option<ViewerEvent>, WinError> {
        let mut bytes = Vec::with_capacity(usize::try_from(copy.len).unwrap_or(0));
        for index in 0..copy.len.div_ceil(BLOCK_BYTES as u64) {
            if !current() {
                return Ok(None);
            }
            let block = self.bytes(index)?;
            let short = block.len() < BLOCK_BYTES;
            bytes.extend_from_slice(&block);
            if short {
                break;
            }
        }
        bytes.truncate(usize::try_from(copy.len).unwrap_or(usize::MAX));
        let text = copy_text(copy.mode, copy.detected, &bytes, &ansi_text);
        drop(bytes);
        let Some(mut block) = ClipboardBlock::text(&text) else {
            let error = windows::core::Error::from_hresult(E_OUTOFMEMORY);
            return Ok(Some(ViewerEvent::Copied(Err(CopyFailure::Native(error)))));
        };
        drop(text);
        let copied = loop {
            match block.set(&publication, &self.copied, cancel, worker) {
                CopyAttempt::Published | CopyAttempt::Superseded => break Ok(()),
                CopyAttempt::Busy(again) if publication.retry() => {
                    if !current() {
                        return Ok(None);
                    }
                    std::thread::sleep(RETRY_AFTER);
                    block = again;
                }
                CopyAttempt::Busy(_) => break Err(CopyFailure::Busy),
                CopyAttempt::Failed(error) => break Err(CopyFailure::Native(error)),
            }
        };
        Ok(Some(ViewerEvent::Copied(copied)))
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        self.clear();
    }
}

pub(crate) fn run(session: Session, worker: &OffThread, cancel: &CancelToken) {
    let mut reading = Reading {
        file: None,
        failed: None,
        len: 0,
        mailbox: Arc::clone(&session.mailbox),
        copied: AtomicU32::new(0),
    };
    while let Some((generation, command)) = session.mailbox.next() {
        if cancel.is_cancelled() {
            break;
        }
        let current = || !cancel.is_cancelled() && session.mailbox.current(generation);
        if !current() {
            continue;
        }
        let result = match command {
            Command::Open(path) => reading.open(&path, generation, current, |file| {
                session
                    .mailbox
                    .register(generation, file.handle_value(), &session.waker);
            }),
            Command::Read(index) if reading.file.is_some() => reading.read(index).map(Some),
            Command::Copy(copy, publication) if reading.file.is_some() => {
                reading.copy(copy, publication, current, cancel, worker)
            }
            Command::Read(_) | Command::Copy(..) => {
                Ok(reading
                    .failed
                    .as_ref()
                    .map(|(opened, reason)| ViewerEvent::Failed {
                        opened: *opened,
                        reason: reason.clone(),
                    }))
            }
            Command::Cancel => continue,
            Command::Release { keep_registration } => {
                // Keep removal notifications through our own veto's failure event.
                reading.release(keep_registration);
                continue;
            }
        };
        if reading.file.is_none() {
            session.mailbox.active.store(false, Ordering::Release);
        }
        if !session.mailbox.current(generation) || cancel.is_cancelled() {
            continue;
        }
        let event = result.unwrap_or_else(|error| {
            let opened = reading.file.is_some();
            let reason = reason(&error);
            // A removal query can race the preceding generation check.
            // Keep its notification until replacement, completion or close.
            drop(reading.file.take());
            session.mailbox.active.store(false, Ordering::Release);
            // Cancel can discard this answer while the document stays visible.
            reading.failed = Some((opened, reason.clone()));
            Some(ViewerEvent::Failed { opened, reason })
        });
        if let Some(event) = event {
            session.mailbox.deliver(generation, event, &session.waker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_release_does_not_acknowledge_an_open_file() {
        let path = std::env::temp_dir().join(format!("gozler-release-{}.bin", std::process::id()));
        std::fs::write(&path, "contents").expect("a test file");
        let wide: Vec<_> = path
            .to_str()
            .expect("a Unicode temp folder")
            .encode_utf16()
            .collect();
        crate::win::work::spawn(move |_| {
            let mailbox = Arc::new(Mailbox::default());
            let mut reading = Reading {
                file: None,
                failed: None,
                len: 0,
                mailbox: Arc::clone(&mailbox),
                copied: AtomicU32::new(0),
            };
            assert!(
                reading
                    .open(&wide, 0, || true, |_| {})
                    .expect("the file opens")
                    .is_some()
            );
            mailbox.post(
                1,
                Command::Release {
                    keep_registration: true,
                },
            );
            assert!(
                !mailbox.released(),
                "posting cancellation cannot consent to removal"
            );
            assert!(
                !mailbox.opening(0),
                "a superseded open cannot race release consent"
            );
            let (_, Command::Release { keep_registration }) =
                mailbox.next().expect("a release is queued")
            else {
                panic!("the queued command releases the file");
            };
            reading.release(keep_registration);
            assert!(mailbox.released());
            assert!(
                reading.file.is_none(),
                "consent follows actual handle ownership release"
            );
        })
        .join()
        .expect("the reader ends");
        std::fs::remove_file(path).expect("the file was released");
    }

    /// A later block 0 must not reset the page or leave its awaited block unresolved.
    #[test]
    fn only_the_opening_reads_block_0_as_opened() {
        let path = std::env::temp_dir().join(format!("gozler-viewer-{}.bin", std::process::id()));
        std::fs::write(&path, vec![b'a'; BLOCK_BYTES + 1]).expect("the temp folder is writable");
        let wide: Vec<u16> = path
            .to_str()
            .expect("the temp path is Unicode")
            .encode_utf16()
            .collect();
        crate::win::work::spawn(move |_| {
            let mut reading = Reading {
                file: None,
                failed: None,
                len: 0,
                mailbox: Arc::default(),
                copied: AtomicU32::new(0),
            };
            assert!(
                matches!(reading.open(&wide, 0, || true, |_| {}), Ok(Some(ViewerEvent::Opened { len, .. }))
                if len == BLOCK_BYTES as u64 + 1)
            );
            for index in [1, 0] {
                let event = reading.read(index);
                assert!(matches!(event, Ok(ViewerEvent::Block { index: got, .. }) if got == index));
            }
        })
        .join()
        .expect("the reader finishes");
        let _ = std::fs::remove_file(path);
    }
}
