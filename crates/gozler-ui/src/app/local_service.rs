//! The gozlerd.exe beside the window, which Use local offers over a service
//! older than the window or of a different build (DESIGN.md: Service fixes),
//! and the service's program, which Settings > Index shows beside it.
//! The check reads both programs and runs them, so it runs on a thread of
//! its own and a status request never waits for it: closing the window
//! joins the status request's thread, and a gozlerd.exe that does not end
//! would otherwise hold the window open.

use crate::core::fetch::{Beside, Local, LocalService, ProgramFile, Programs};
use crate::win;
use gozler_core::proto::SERVICE_NAME;
use std::fs::File;
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

/// The files a check reads, with their builds, each when it can be read:
/// the gozlerd.exe beside the window and the program the service's
/// configuration names.
#[derive(Clone, PartialEq, Eq)]
struct Files {
    local: Option<(PathBuf, Build)>,
    registered: Option<(PathBuf, Build)>,
}

/// The check of the latest [`Files`].
#[expect(
    clippy::large_enum_variant,
    reason = "one value lives in `LATEST`, so boxing would save nothing"
)]
enum Check {
    Running,
    Done(Programs),
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
/// Neither command starts a service. In a paired window, a gozlerd.exe that
/// is not its pair is [`Local::Mismatched`], and it never runs. The same
/// check reads what Settings > Index shows of both files.
///
/// Returns at once: [`Programs::UNKNOWN`] while the check of these builds
/// runs, so the window learns its answer at a later status.
pub(super) fn local_service() -> Programs {
    let with_build = |path: PathBuf| {
        let build = build(&path)?;
        Some((path, build))
    };
    let files = Files {
        local: std::env::current_exe()
            .ok()
            .and_then(|exe| with_build(exe.with_file_name("gozlerd.exe"))),
        registered: gozler_winfs::service_program(SERVICE_NAME)
            .ok()
            .and_then(with_build),
    };
    if files.local.is_none() && files.registered.is_none() {
        return Programs::UNKNOWN;
    }
    let mut latest = LATEST.lock().unwrap_or_else(PoisonError::into_inner);
    match latest.as_ref() {
        Some((checked, Check::Done(found))) if *checked == files => return found.clone(),
        Some((checked, Check::Running)) if *checked == files => return Programs::UNKNOWN,
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
    Programs::UNKNOWN
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

/// What [`local_service`] answers for `files`.
fn check(files: &Files) -> Programs {
    let service = files
        .registered
        .as_ref()
        .map(|(registered, build)| ProgramFile {
            path: registered.clone(),
            version: version(registered),
            modified: filetime(build.0),
        });
    let mut programs = Programs {
        service,
        ..Programs::UNKNOWN
    };
    let Some((program, local_build)) = &files.local else {
        return programs;
    };
    if let Some((registered, registered_build)) = &files.registered
        && local_build.1 == registered_build.1
        && matches!(
            (std::fs::read(program), std::fs::read(registered)),
            (Ok(a), Ok(b)) if a == b
        )
    {
        programs.beside = Beside::SameAsService(program.clone());
        return programs;
    }
    let paired = is_paired(program);
    programs.beside = Beside::Other(ProgramFile {
        path: program.clone(),
        version: paired.then(|| version(program)).flatten(),
        modified: filetime(local_build.0),
    });
    if !paired {
        programs.local = Local::Mismatched;
        return programs;
    }
    let installed = programs
        .service
        .as_ref()
        .and_then(|service| service.version.clone());
    if let Some(local) = printed(program, "protocol")
        .and_then(|protocol| LocalService::from_protocol(program.clone(), &protocol, installed))
    {
        programs.local = Local::Offered(local);
    }
    programs
}

/// What `gozlerd.exe --version` prints after the program's name, such as
/// `0.3.0 (2026-10-20 09:14)`.
fn version(program: &Path) -> Option<String> {
    let printed = printed(program, "--version")?;
    printed.trim().strip_prefix("gozlerd ").map(str::to_owned)
}

/// The build of the file at `path`.
fn build(path: &Path) -> Option<Build> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// `time` as a FILETIME, which the window's date formats read.
fn filetime(time: SystemTime) -> i64 {
    let ms = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    };
    gozler_core::output::filetime::unix_ms_to_filetime(ms)
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
