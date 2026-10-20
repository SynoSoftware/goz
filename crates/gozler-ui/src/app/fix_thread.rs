//! Runs a service fix on its own thread (DESIGN.md: Service fixes): it runs
//! a repair's or Use local's commands as administrator and waits for them
//! and then for the service, finds the latest setup, or downloads the setup
//! the person accepted and checks it. It reports each step as a
//! [`FixEvent`] on a channel, as the requests do (DESIGN.md: window rule
//! 5); `core::fixer` decides what each event means.

use crate::core::fixer::{
    self, CommandEvent, CommandResult, FixEvent, FixWork, SetupEvent, SetupResult,
};
use crate::core::update::{self, Setup};
use crate::win;
use crate::win::shell::PromptOwner;
use crate::win::wake::OpenWaker;
use gozler_core::proto::SERVICE_NAME;
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the fix waits for the service to answer after its commands
/// ended well. `net start` returns once the service runs, so this covers
/// an install, whose service starts after the command.
const SERVICE_WAIT: Duration = Duration::from_secs(30);

/// How often the thread checks its stop flag while it waits.
const POLL: Duration = Duration::from_millis(100);

/// How often a download reports its bytes, which the status bar shows.
const DOWNLOAD_PROGRESS: Duration = Duration::from_millis(250);

/// The start of the name of the file in `%TEMP%` that a fix's commands
/// write to.
const SAID_PREFIX: &str = "gozler-fix-";

/// One fix's thread, with its own channel, so the events of a run that
/// was stopped never reach the next run.
struct Run {
    stop: Arc<AtomicBool>,
    /// A command run's thread, which [`FixThread::shut_down`] joins. A
    /// download's thread is not joined, because WinHTTP can hold it for
    /// about a minute, while the closing process keeps `Local\gozler.ui`
    /// and a new start would find no window to show.
    commands: Option<JoinHandle<()>>,
    events: Receiver<FixEvent>,
}

/// The fix's thread, which outlives the Settings dialog.
pub(super) struct FixThread {
    run: RefCell<Option<Run>>,
    /// The threads of stopped command runs, which end within [`POLL`].
    /// Only [`FixThread::shut_down`] joins them, so a new fix never waits.
    stopped: RefCell<Vec<JoinHandle<()>>>,
    /// The number of runs started, which names each download's folder,
    /// so a stopped download never deletes the next one's file.
    started: Cell<u32>,
    /// Wakes the window while it is open, because a download's thread is
    /// not joined.
    waker: OpenWaker,
}

impl FixThread {
    pub(super) fn new(waker: OpenWaker) -> FixThread {
        FixThread {
            run: RefCell::new(None),
            stopped: RefCell::new(Vec::new()),
            started: Cell::new(0),
            waker,
        }
    }

    /// Runs `work`: a repair's commands in one `cmd /c`, so Windows asks
    /// for permission once; the search for the latest setup; or the
    /// download of the setup the person accepted. The model starts work
    /// only when no thread does any, so an earlier run has ended, waits for
    /// the service or was cancelled, and its thread stops at its next
    /// check.
    pub(super) fn start(&self, owner: PromptOwner, work: FixWork) {
        let mut stopped = self.stopped.borrow_mut();
        if let Some(earlier) = self.run.take() {
            earlier.stop.store(true, Ordering::Relaxed);
            stopped.extend(earlier.commands);
        }
        stopped.retain(|thread| !thread.is_finished());
        self.started.set(self.started.get() + 1);
        let folder = std::env::temp_dir().join("Gozler").join(format!(
            "{}-{}",
            std::process::id(),
            self.started.get()
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, events) = channel();
        let runs_commands = matches!(work, FixWork::Commands(_) | FixWork::UseLocal(_));
        let thread = {
            let (stop, waker) = (Arc::clone(&stop), self.waker.clone());
            std::thread::spawn(move || {
                // The send fails once a new run has replaced this one, which
                // is how its late events are dropped.
                let report = |event: FixEvent| waker.deliver(|| sender.send(event).is_ok());
                let commands = |event| report(FixEvent::Commands(event));
                let setup = |event| report(FixEvent::Setup(event));
                match work {
                    FixWork::Commands(lines) => run_commands(owner, &lines, &stop, &commands),
                    FixWork::UseLocal(local) => use_local(owner, &local, &stop, &commands),
                    FixWork::FindSetup => find(&stop, &setup),
                    FixWork::Download(found) => download(&found, &folder, &stop, &setup),
                }
            })
        };
        self.run.replace(Some(Run {
            stop,
            commands: runs_commands.then_some(thread),
            events,
        }));
    }

    /// Stops the fix's thread at its next check.
    pub(super) fn stop(&self) {
        if let Some(run) = &*self.run.borrow() {
            run.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Every event of the latest run that has arrived, without waiting.
    pub(super) fn drain(&self) -> Vec<FixEvent> {
        self.run
            .borrow()
            .as_ref()
            .map(|run| run.events.try_iter().collect())
            .unwrap_or_default()
    }

    /// Stops every thread and joins the command runs, after the window
    /// closed its [`OpenWaker`], so none wakes the window after it is gone
    /// (DESIGN.md: window rule 5). A command run that waits for the
    /// permission prompt ends once the person answers it. A download is
    /// not joined; the process ends with it.
    pub(super) fn shut_down(&self) {
        let stopped = self.stopped.take();
        let latest = self.run.take().and_then(|run| {
            run.stop.store(true, Ordering::Relaxed);
            run.commands
        });
        for thread in stopped.into_iter().chain(latest) {
            let _ = thread.join();
        }
    }
}

/// Runs `commands` as administrator, each writing what it says to a file
/// in the person's `%TEMP%`, which the window reads when they fail: their
/// output is hidden, and `net start` ends with 2 for any failure, so only
/// their words give Windows' reason. The file is deleted afterwards, and
/// so are the files of earlier fixes, which commands that still ran when
/// their window closed may have written after it deleted them.
fn run_commands(
    owner: PromptOwner,
    commands: &[String],
    stop: &AtomicBool,
    report: &dyn Fn(CommandEvent),
) {
    win::shell::initialize_com();
    let temp = std::env::temp_dir();
    for entry in std::fs::read_dir(&temp).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(SAID_PREFIX) && name.ends_with(".txt") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let said = temp.join(format!("{SAID_PREFIX}{}.txt", std::process::id()));
    let parameters = commands
        .iter()
        .map(|command| format!("{command} >>\"{}\" 2>&1", said.display()))
        .collect::<Vec<_>>()
        .join(" && ");
    run_watched(owner, &format!("/c {parameters}"), &said, stop, report);
    let _ = std::fs::remove_file(&said);
}

/// Copies the gozlerd.exe at `local` over the program the service runs,
/// with the service stopped, so the service keeps its path.
fn use_local(owner: PromptOwner, local: &Path, stop: &AtomicBool, report: &dyn Fn(CommandEvent)) {
    let registered = match gozler_winfs::service_program(SERVICE_NAME) {
        Ok(registered) => registered,
        Err(error) => {
            let error = io::Error::from_raw_os_error(error.code as i32);
            return report(failed(&error));
        }
    };
    let same = match (
        std::fs::canonicalize(local),
        std::fs::canonicalize(&registered),
    ) {
        (Ok(local), Ok(registered)) => local == registered,
        _ => false,
    };
    let commands = fixer::use_local_commands(local, (!same).then_some(registered.as_path()));
    run_commands(owner, &commands, stop, report);
}

/// The end of a commands fix that Windows could not run or watch.
fn failed(error: &io::Error) -> CommandEvent {
    CommandEvent::End(CommandResult::Failed(win::error_text(error)))
}

fn run_watched(
    owner: PromptOwner,
    parameters: &str,
    said: &Path,
    stop: &AtomicBool,
    report: &dyn Fn(CommandEvent),
) {
    let process = match win::shell::start_as_administrator(owner, "cmd.exe", parameters) {
        Ok(process) => process,
        Err(error) if win::shell::refused(&error) => {
            return report(CommandEvent::End(CommandResult::Refused));
        }
        Err(error) => return report(failed(&error)),
    };
    report(CommandEvent::Allowed);
    let wait_ms = POLL.as_millis() as u32;
    let code = loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
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
    report(CommandEvent::Succeeded);
    let started = Instant::now();
    while started.elapsed() < SERVICE_WAIT {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(POLL);
    }
    report(CommandEvent::End(CommandResult::NoAnswer));
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
fn find(stop: &AtomicBool, report: &dyn Fn(SetupEvent)) {
    let reply = win::update::latest_release();
    if stop.load(Ordering::Relaxed) {
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
fn download(setup: &Setup, folder: &Path, stop: &AtomicBool, report: &dyn Fn(SetupEvent)) {
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
    if stop.load(Ordering::Relaxed) {
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
    report(SetupEvent::Ready(path));
}

/// Writes the setup to `path` in `folder`, reporting its bytes as they
/// arrive. Returns whether every byte the release names arrived, and no
/// more, or the error that kept the file from being written.
fn save(
    setup: &Setup,
    folder: &Path,
    path: &Path,
    stop: &AtomicBool,
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
        if written > setup.size || stop.load(Ordering::Relaxed) {
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
