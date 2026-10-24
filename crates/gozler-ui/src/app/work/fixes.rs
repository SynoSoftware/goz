//! Service fixes share one bounded work slot; the core fixer owns their steps.

use crate::core::fixer::{CommandEvent, CommandResult, FixEvent, FixWork, SetupEvent, SetupResult};
use crate::core::update::{self, Setup};
use crate::core::work::{INLINE_ITEMS, Replacement, Retirement};
use crate::win;
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

const SERVICE_WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);
const DOWNLOAD_PROGRESS: Duration = Duration::from_millis(250);
const EVENTS: usize = 8;

struct Run {
    work: FixWork,
    folder: PathBuf,
    events: SyncSender<Event>,
    waker: OpenWaker,
}

struct Event(Option<FixEvent>);

impl Drop for Event {
    fn drop(&mut self) {
        if let Some(FixEvent::Setup(SetupEvent::Ready(path))) = &self.0 {
            let _ = std::fs::remove_file(path);
            if let Some(folder) = path.parent() {
                let _ = std::fs::remove_dir(folder);
            }
        }
    }
}

pub(in crate::app) struct Fixes {
    task: Option<Task<Run, ()>>,
    events: Option<Receiver<Event>>,
    started: u64,
    waker: OpenWaker,
    commands: bool,
    retired: Retirement,
}

impl Fixes {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            task: Some(Task::shell(waker.clone(), Replacement::Cancel, run)),
            events: None,
            started: 0,
            waker,
            commands: false,
            retired: Retirement::default(),
        }
    }

    pub(in crate::app) fn start(&mut self, work: FixWork) {
        let Some(task) = &mut self.task else { return };
        self.started = self
            .started
            .checked_add(1)
            .expect("fix run ids cannot wrap");
        let (sender, events) = sync_channel(EVENTS);
        if let Some(earlier) = self.events.replace(events) {
            self.retired.push(earlier, INLINE_ITEMS + 1);
        }
        self.commands = matches!(work, FixWork::Commands(_));
        task.replace(Run {
            work,
            folder: std::env::temp_dir().join("Gozler").join(format!(
                "fix-{}-{}",
                std::process::id(),
                self.started
            )),
            events: sender,
            waker: self.waker.clone(),
        });
    }

    pub(in crate::app) fn stop(&mut self) {
        if let Some(events) = self.events.take() {
            self.retired.push(events, INLINE_ITEMS + 1);
        }
        if let Some(task) = &mut self.task {
            task.cancel();
        }
    }

    pub(in crate::app) fn drain(&mut self) -> Vec<FixEvent> {
        let mut events = self
            .events
            .as_ref()
            .map(|events| {
                events
                    .try_iter()
                    .take(EVENTS)
                    .filter_map(|mut event| event.0.take())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(task) = &mut self.task
            && task.take().is_some()
            && let Some(remaining) = self.events.take()
        {
            events.extend(
                remaining
                    .try_iter()
                    .take(EVENTS)
                    .filter_map(|mut event| event.0.take()),
            );
        }
        events
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        self.task
            .as_mut()
            .is_some_and(|task| task.tick(now).is_some())
    }

    pub(in crate::app) fn take_retired(&mut self) -> Retirement {
        let mut retired = std::mem::take(&mut self.retired);
        if let Some(task) = &mut self.task {
            retired.append(task.take_retired());
        }
        retired
    }

    pub(super) fn close(&mut self) -> impl FnOnce(&OffThread) + Send + 'static {
        let events = self.events.take();
        let task = self.task.take();
        let commands = self.commands;
        let retired = self.take_retired();
        move |worker| {
            // Disconnect before waiting, including a sender blocked on a step.
            drop(events);
            drop(retired);
            if let Some(mut task) = task {
                if commands {
                    // A command can be pending behind a cancelled download.
                    task.finish(worker);
                } else {
                    task.cancel();
                    task.finish_current(worker);
                }
            }
        }
    }
}

fn run(input: Run, session: &win::shell::Session, _worker: &OffThread, cancel: &CancelToken) {
    let report = |event| {
        let progress = matches!(&event, FixEvent::Setup(SetupEvent::Downloaded(_)));
        let event = Event(Some(event));
        let sent = if progress {
            input.events.try_send(event).is_ok()
        } else {
            input.events.send(event).is_ok()
        };
        if sent {
            input.waker.deliver(|| true);
        }
        sent
    };
    match input.work {
        FixWork::Commands(lines) => {
            run_commands(session, &lines, &input.folder, cancel, &|event| {
                report(FixEvent::Commands(event))
            })
        }
        FixWork::FindSetup => find(cancel, &|event| {
            report(FixEvent::Setup(event));
        }),
        FixWork::Download(found) => download(&found, &input.folder, cancel, &|event| {
            report(FixEvent::Setup(event));
        }),
    }
}

/// Hidden commands write Windows' reason to their private output file;
/// `net start` gives the same exit code for unrelated failures.
fn run_commands(
    session: &win::shell::Session,
    commands: &[String],
    folder: &Path,
    stop: &CancelToken,
    report: &dyn Fn(CommandEvent) -> bool,
) {
    if let Err(error) = std::fs::create_dir_all(folder) {
        report(failed(&error));
        return;
    }
    let said = folder.join("commands.txt");
    let parameters = commands
        .iter()
        .map(|command| format!("{command} >>\"{}\" 2>&1", said.display()))
        .collect::<Vec<_>>()
        .join(" && ");
    run_watched(session, &format!("/c {parameters}"), &said, stop, report);
    let _ = std::fs::remove_file(&said);
    let _ = std::fs::remove_dir(folder);
}

/// The end of a commands fix that Windows could not run or watch.
fn failed(error: &io::Error) -> CommandEvent {
    CommandEvent::End(CommandResult::Failed(win::error_text(error)))
}

fn run_watched(
    session: &win::shell::Session,
    parameters: &str,
    said: &Path,
    stop: &CancelToken,
    report: &dyn Fn(CommandEvent) -> bool,
) -> bool {
    let process = match session.start_as_administrator("cmd.exe", parameters) {
        Ok(process) => process,
        Err(error) if win::shell::refused(&error) => {
            return report(CommandEvent::End(CommandResult::Refused));
        }
        Err(error) => return report(failed(&error)),
    };
    report(CommandEvent::Allowed);
    let wait_ms = POLL.as_millis() as u32;
    let code = loop {
        // Once admitted, the commands finish before their output is deleted.
        match process.wait(wait_ms) {
            Ok(Some(code)) => break code,
            Ok(None) => {}
            Err(error) => return report(failed(&error)),
        }
    };
    if code != 0 {
        let said = std::fs::read(said)
            .ok()
            .and_then(|bytes| last_words(&bytes));
        return report(CommandEvent::End(CommandResult::CommandsFailed {
            code,
            said,
        }));
    }
    if !report(CommandEvent::Succeeded) {
        return false;
    }
    let started = Instant::now();
    while started.elapsed() < SERVICE_WAIT {
        if stop.is_cancelled() {
            return false;
        }
        std::thread::sleep(POLL);
    }
    report(CommandEvent::End(CommandResult::NoAnswer))
}

/// The last line of the commands' output that says something: Windows'
/// reason, such as "Access is denied.". `net` ends a failure with a line
/// that points to `NET HELPMSG`, in every language, which says nothing
/// itself.
///
/// `net` writes in the console's code page and `gozlerd` in UTF-8, so the
/// output mixes the two, and the line is decoded on its own: as UTF-8 when
/// it is, else in the console's code page. Line breaks and the pointer are
/// ASCII in both, so the bytes are split before they are decoded.
fn last_words(output: &[u8]) -> Option<String> {
    let line = output
        .split(|&byte| byte == b'\n')
        .map(<[u8]>::trim_ascii)
        .rfind(|line| !line.is_empty() && !line.windows(11).any(|part| part == b"NET HELPMSG"))?;
    Some(match std::str::from_utf8(line) {
        Ok(line) => line.to_owned(),
        Err(_) => win::locale::console_text(line),
    })
}

/// Asks GitHub for the latest release's setup. A stopped search reports
/// nothing, because the model has ended the fix.
fn find(stop: &CancelToken, report: &dyn Fn(SetupEvent)) {
    let reply = win::update::latest_release();
    if stop.is_cancelled() {
        return;
    }
    let Some(reply) = reply else {
        return report(SetupEvent::End(SetupResult::DownloadFailed));
    };
    report(
        match update::setup_to_install(&reply, gozler_core::escompat::version()) {
            Some(setup) => SetupEvent::Found(setup),
            None => SetupEvent::End(SetupResult::NoSetup),
        },
    );
}

/// Downloads `setup` to `folder`, which is this run's own, and checks it.
/// A stopped download deletes its file and reports nothing, because the
/// model has ended the fix.
fn download(setup: &Setup, folder: &Path, stop: &CancelToken, report: &dyn Fn(SetupEvent)) {
    let path = folder.join(&setup.name);
    let discard = || {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(folder);
    };
    let end = |result| report(SetupEvent::End(result));
    let not_saved = |error: io::Error| SetupResult::NotSaved {
        folder: folder.to_owned(),
        why: win::error_text(&error),
    };
    let saved = save(setup, folder, &path, stop, report);
    if stop.is_cancelled() {
        return discard();
    }
    match saved {
        Ok(true) => {}
        Ok(false) => {
            discard();
            return end(SetupResult::DownloadFailed);
        }
        Err(error) => {
            discard();
            return end(not_saved(error));
        }
    }
    report(SetupEvent::Checking);
    match check(&path, &setup.sha256) {
        Ok(true) => {}
        Ok(false) => {
            discard();
            return end(SetupResult::Damaged);
        }
        // Such as antivirus software that took the file away.
        Err(error) => {
            discard();
            return end(not_saved(error));
        }
    }
    if stop.is_cancelled() {
        return discard();
    }
    report(SetupEvent::Ready(path));
}

/// Writes the setup to `path` in `folder`, reporting its bytes as they
/// arrive. Returns whether every byte the release names arrived, and no
/// more, or the error that kept the file from being written.
fn save(
    setup: &Setup,
    folder: &Path,
    path: &Path,
    stop: &CancelToken,
    report: &dyn Fn(SetupEvent),
) -> io::Result<bool> {
    std::fs::create_dir_all(folder)?;
    let mut file = BufWriter::new(File::create(path)?);
    let mut written = 0u64;
    let mut reported = Instant::now();
    let mut failed = None;
    let read = win::update::read(&setup.host, &setup.path, &mut |piece| {
        written += piece.len() as u64;
        if reported.elapsed() >= DOWNLOAD_PROGRESS {
            reported = Instant::now();
            report(SetupEvent::Downloaded(written));
        }
        if written > setup.size || stop.is_cancelled() {
            return false;
        }
        match file.write_all(piece) {
            Ok(()) => true,
            Err(error) => {
                failed = Some(error);
                false
            }
        }
    });
    if let Some(error) = failed {
        return Err(error);
    }
    let whole = read.is_some() && written == setup.size;
    if whole {
        file.flush()?;
    }
    Ok(whole)
}

/// Whether the file at `path` has the SHA-256 `expected`, or the error
/// that kept it from being read. The caller deletes a file that is not
/// checked, so it is never run.
fn check(path: &Path, expected: &[u8; 32]) -> io::Result<bool> {
    let digest = win::update::sha256(&mut File::open(path)?)?;
    Ok(digest == *expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_ready_setup_is_removed_only_by_retirement() {
        let folder =
            std::env::temp_dir().join(format!("gozler-retired-setup-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a temporary folder");
        let path = folder.join("setup.exe");
        std::fs::write(&path, "checked setup").expect("a setup file");
        let mut fixes = Fixes::new(OpenWaker::closed());
        let (sender, events) = sync_channel(EVENTS);
        sender
            .send(Event(Some(FixEvent::Setup(SetupEvent::Ready(
                path.clone(),
            )))))
            .unwrap_or_else(|_| panic!("the event receiver remains open"));
        fixes.events = Some(events);
        fixes.stop();
        assert!(
            path.exists(),
            "Cancel does no filesystem work on its caller"
        );
        let retired = fixes.take_retired();
        win::work::spawn(move |_| drop(retired))
            .join()
            .expect("retirement ends");
        assert!(!path.exists(), "a discarded setup must not remain runnable");
        assert!(!folder.exists());
    }

    /// `net` ends a failure with a pointer to its help, which would replace
    /// Windows' reason in the status bar.
    #[test]
    fn the_reason_is_the_last_line_that_says_something() {
        let net = b"The Gozler service is starting.\r\nThe Gozler service could not be \
                    started.\r\n\r\nA service specific error occurred: 1.\r\n\r\nMore help \
                    is available by typing NET HELPMSG 3547.\r\n\r\n";
        assert_eq!(
            last_words(net).as_deref(),
            Some("A service specific error occurred: 1.")
        );
        assert_eq!(last_words(b" \r\n"), None);
    }

    /// Decoding the whole output in the console's code page garbles the
    /// UTF-8 reason that `gozlerd` writes after `net`'s text.
    #[test]
    fn a_utf8_reason_after_console_text_reads_whole() {
        let mut output = b"Le service Gozler d\x82marre.\r\n".to_vec();
        output.extend_from_slice("Accès refusé.\r\n".as_bytes());
        assert_eq!(last_words(&output).as_deref(), Some("Accès refusé."));
    }

    /// A setup that differs from the one GitHub published must fail the
    /// check, never start, and one that cannot be read must not pass as
    /// damaged (DESIGN.md: Service fixes).
    #[test]
    fn a_setup_with_another_digest_fails_the_check() {
        let folder = std::env::temp_dir().join(format!("gozler-check-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a temporary folder");
        let path = folder.join("setup.exe");
        // SHA-256 of "abc", FIPS 180-2's first example.
        let abc: [u8; 32] = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        std::fs::write(&path, "abc").expect("a written file");
        assert!(check(&path, &abc).expect("a readable file"));
        let mut other = abc;
        other[0] ^= 1;
        assert!(!check(&path, &other).expect("a readable file"));
        std::fs::remove_file(&path).expect("the file is deleted");
        assert!(check(&path, &abc).is_err(), "a missing file is not damaged");
        let _ = std::fs::remove_dir(&folder);
    }
}
