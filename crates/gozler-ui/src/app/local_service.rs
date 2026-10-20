//! The gozlerd.exe beside the window, which Use local offers over a service
//! older than the window or of a different build (DESIGN.md: Service fixes).
//! The check reads both programs and runs them, so it runs on a thread of
//! its own and a status request never waits for it: closing the window
//! joins the status request's thread, and a gozlerd.exe that does not end
//! would otherwise hold the window open.

use crate::core::fetch::LocalService;
use gozler_core::proto::SERVICE_NAME;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

/// Keeps `gozlerd.exe` from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How long a program may take to print its answer before it is ended and
/// its build is not offered; a first run can wait for an antivirus scan.
const PRINT_LIMIT: Duration = Duration::from_secs(10);

/// How often the check looks whether the program has ended.
const POLL: Duration = Duration::from_millis(20);

/// A file's last write time and length, which change with each build.
type Build = (SystemTime, u64);

/// The files a check reads, with their builds: the gozlerd.exe beside the
/// window and, when it can be read, the program the service's configuration
/// names.
#[derive(Clone, PartialEq, Eq)]
struct Files {
    local: (PathBuf, Build),
    registered: Option<(PathBuf, Build)>,
}

/// The check of the latest [`Files`].
enum Check {
    Running,
    Done(Option<LocalService>),
}

/// The latest files and their check. A check runs once for each build of
/// the files, because the status poll asks every two seconds, and a
/// developer who rebuilds gozlerd.exe expects the new build to count.
static LATEST: Mutex<Option<(Files, Check)>> = Mutex::new(None);

/// The gozlerd.exe beside the window, when it serves the window's protocol
/// and its bytes differ from the program the service runs, with that
/// program's version. The bytes decide, because a development build reports
/// the same version as the release it follows, and an installed Gozler,
/// whose window runs beside the service's own program, then never offers
/// it. An older gozlerd.exe has no hidden `protocol` command and fails, so
/// it is not offered; an older service's program still answers `--version`.
/// Neither command starts a service.
///
/// Returns at once: `None` while the check of these builds runs, so the
/// window learns its answer at a later status.
pub(super) fn local_service() -> Option<LocalService> {
    let program = std::env::current_exe().ok()?.with_file_name("gozlerd.exe");
    let local_build = build(&program)?;
    let files = Files {
        local: (program, local_build),
        registered: gozler_winfs::service_program(SERVICE_NAME)
            .ok()
            .and_then(|registered| {
                let registered_build = build(&registered)?;
                Some((registered, registered_build))
            }),
    };
    let mut latest = LATEST.lock().unwrap_or_else(PoisonError::into_inner);
    match latest.as_ref() {
        Some((checked, Check::Done(found))) if *checked == files => return found.clone(),
        Some((checked, Check::Running)) if *checked == files => return None,
        _ => {}
    }
    *latest = Some((files.clone(), Check::Running));
    drop(latest);
    // Not joined: the thread holds nothing of the window, and a program
    // that does not end is ended at PRINT_LIMIT.
    std::thread::spawn(move || {
        let found = check(&files);
        let mut latest = LATEST.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((checked, check)) = latest.as_mut()
            && *checked == files
        {
            *check = Check::Done(found);
        }
    });
    None
}

/// What [`local_service`] answers for `files`.
fn check(files: &Files) -> Option<LocalService> {
    let (program, local_build) = &files.local;
    if let Some((registered, registered_build)) = &files.registered
        && local_build.1 == registered_build.1
        && matches!(
            (std::fs::read(program), std::fs::read(registered)),
            (Ok(a), Ok(b)) if a == b
        )
    {
        return None;
    }
    let protocol = printed(program, "protocol")?;
    let installed = files
        .registered
        .as_ref()
        .and_then(|(registered, _)| printed(registered, "--version"))
        .and_then(|version| version.trim().strip_prefix("gozlerd ").map(str::to_owned));
    LocalService::from_protocol(program.clone(), &protocol, installed)
}

/// The build of the file at `path`.
fn build(path: &Path) -> Option<Build> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// What `program argument` printed when it ended well within
/// [`PRINT_LIMIT`], run without a console window. Its answer is one line,
/// far smaller than the pipe's buffer, so it is read after the program
/// ends.
fn printed(program: &Path, argument: &str) -> Option<String> {
    let mut child = Command::new(program)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < PRINT_LIMIT => std::thread::sleep(POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut output = Vec::new();
    child.stdout.take()?.read_to_end(&mut output).ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&output).into_owned())
}
