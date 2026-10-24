//! The gozlerd.exe beside the window, which Use local offers over a service
//! older than the window or of a different build (DESIGN.md: Service fixes),
//! and the service's program, which Settings > Index shows beside it.
//! `programs` owns admission and the cache; these functions inspect and
//! run the files off the window thread without delaying a status request.

use crate::core::programs::{Checked, Files, Found, Printed, check};
use crate::win;
use crate::win::work::OffThread;
use gozler_client::CancelToken;
use gozler_core::proto::SERVICE_NAME;
use std::fs::File;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Keeps `gozlerd.exe` from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How long a program may take to print its answer before it is ended; a
/// first run can wait for an antivirus scan.
const PRINT_LIMIT: Duration = Duration::from_secs(10);

/// How often the check looks whether the program has ended.
const POLL: Duration = Duration::from_millis(20);

pub(super) fn files(_worker: &OffThread) -> Files {
    let found = |path: PathBuf| {
        let metadata = std::fs::metadata(&path).ok()?;
        Some(Found {
            modified: metadata.modified().ok()?,
            length: metadata.len(),
            path,
        })
    };
    Files {
        local: std::env::current_exe()
            .ok()
            .and_then(|exe| found(exe.with_file_name("gozlerd.exe"))),
        registered: gozler_winfs::service_program(SERVICE_NAME)
            .ok()
            .and_then(found),
    }
}

pub(super) fn check_files(files: &Files, _worker: &OffThread, cancel: &CancelToken) -> Checked {
    check(
        files,
        &|a, b| !cancel.is_cancelled() && same_bytes(a, b),
        &|program| !cancel.is_cancelled() && is_paired(program),
        &|program, argument| printed(program, argument, cancel),
    )
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
fn printed(program: &Path, argument: &str, cancel: &CancelToken) -> Printed {
    if cancel.is_cancelled() {
        return Printed::Failed;
    }
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
            Ok(None) if started.elapsed() < PRINT_LIMIT && !cancel.is_cancelled() => {
                std::thread::sleep(POLL);
            }
            stopped => {
                let _ = child.kill();
                let _ = child.wait();
                return if stopped.is_ok() && !cancel.is_cancelled() {
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
