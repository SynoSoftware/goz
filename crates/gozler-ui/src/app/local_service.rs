//! The gozlerd.exe beside the window, which Use local offers over a service
//! older than the window or of a different build (DESIGN.md: Service fixes),
//! and the service's program, which Settings > Index shows beside it.
//! The check reads both programs and runs them, so it runs on a thread of
//! its own and a status request never waits for it: closing the window
//! joins the status request's thread, and a gozlerd.exe that does not end
//! would otherwise hold the window open. `core::programs` decides what
//! runs; this module reads and runs the files.

use crate::core::fetch::Programs;
use crate::core::programs::{Cache, Files, Found, Printed, check};
use crate::win;
use gozler_core::proto::SERVICE_NAME;
use std::fs::File;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Keeps `gozlerd.exe` from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How long a program may take to print its answer before it is ended; a
/// first run can wait for an antivirus scan.
const PRINT_LIMIT: Duration = Duration::from_secs(10);

/// How often the check looks whether the program has ended.
const POLL: Duration = Duration::from_millis(20);

static LATEST: Mutex<Cache> = Mutex::new(Cache::new());

/// The gozlerd.exe beside the window, when it serves the window's protocol
/// and its bytes differ from the program the service runs, with that
/// program's version. The bytes decide, because a development build reports
/// the same version as the release it follows, and an installed Gozler,
/// whose window runs beside the service's own program, then never offers
/// it. An older gozlerd.exe has no hidden `protocol` command and fails, so
/// it is not offered; an older service's program still answers `--version`.
/// Neither command starts a service. In a paired window, a gozlerd.exe that
/// is not its pair is [`Local::Mismatched`], and it never runs. The same
/// check reads what Settings > Index shows of both files.
///
/// Returns at once: [`Programs::UNKNOWN`] while the check of these builds
/// runs, so the window learns its answer at a later status.
///
/// [`Local::Mismatched`]: crate::core::fetch::Local::Mismatched
pub(super) fn local_service() -> Programs {
    let found = |path: PathBuf| {
        let metadata = std::fs::metadata(&path).ok()?;
        Some(Found {
            modified: metadata.modified().ok()?,
            length: metadata.len(),
            path,
        })
    };
    let files = Files {
        local: std::env::current_exe()
            .ok()
            .and_then(|exe| found(exe.with_file_name("gozlerd.exe"))),
        registered: gozler_winfs::service_program(SERVICE_NAME)
            .ok()
            .and_then(found),
    };
    let asked = LATEST
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .ask(&files);
    if asked.start {
        // Not joined: the thread holds nothing of the window, and a program
        // that does not end is ended at PRINT_LIMIT.
        std::thread::spawn(move || {
            let checked = check(&files, &same_bytes, &is_paired, &printed);
            LATEST
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .finish(&files, checked);
        });
    }
    asked.programs
}

/// Whether `program` is the gozlerd.exe that the setup's build paired with
/// this window; any is, in a window that was never paired. Use local asks
/// again when the person confirms it, because a gozlerd.exe rebuilt or
/// replaced since the check would switch the service to a file nobody
/// checked.
pub(super) fn is_paired(program: &Path) -> bool {
    let Some(paired) = win::pairing::paired_gozlerd() else {
        return true;
    };
    File::open(program)
        .and_then(|mut file| win::update::sha256(&mut file))
        .is_ok_and(|sha256| sha256 == paired)
}

/// Whether both files can be read and hold the same bytes.
fn same_bytes(a: &Path, b: &Path) -> bool {
    matches!((std::fs::read(a), std::fs::read(b)), (Ok(a), Ok(b)) if a == b)
}

/// What `program argument` printed when it ended within [`PRINT_LIMIT`],
/// run without a console window. Its answer is one line, far smaller than
/// the pipe's buffer, so it is read after the program ends.
fn printed(program: &Path, argument: &str) -> Printed {
    let Ok(mut child) = Command::new(program)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    else {
        return Printed::Failed;
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < PRINT_LIMIT => std::thread::sleep(POLL),
            // The limit passed, or the wait failed.
            stopped => {
                let _ = child.kill();
                let _ = child.wait();
                return if stopped.is_ok() {
                    Printed::TimedOut
                } else {
                    Printed::Failed
                };
            }
        }
    };
    let mut output = Vec::new();
    let read = child
        .stdout
        .take()
        .is_some_and(|mut stdout| stdout.read_to_end(&mut output).is_ok());
    if status.success() && read {
        Printed::Text(String::from_utf8_lossy(&output).into_owned())
    } else {
        Printed::Failed
    }
}
