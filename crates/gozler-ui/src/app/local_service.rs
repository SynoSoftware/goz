//! The gozlerd.exe beside the window, which Use local offers over a service
//! older than the window (DESIGN.md: Service fixes).

use crate::core::fetch::LocalService;
use gozler_core::proto::SERVICE_NAME;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

/// Keeps `gozlerd.exe` from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The gozlerd.exe beside the window, when it serves the window's protocol,
/// with the version of the program the service runs. An older gozlerd.exe
/// has no hidden `protocol` command and fails, so it is not offered; an
/// older service's program still answers `--version`. Neither command
/// starts a service.
pub(super) fn local_service() -> Option<LocalService> {
    let program = std::env::current_exe().ok()?.with_file_name("gozlerd.exe");
    let protocol = printed(&program, "protocol")?;
    let installed = gozler_winfs::service_program(SERVICE_NAME)
        .ok()
        .and_then(|registered| printed(&registered, "--version"))
        .and_then(|version| version.trim().strip_prefix("gozlerd ").map(str::to_owned));
    LocalService::from_protocol(program, &protocol, installed)
}

/// What `program` printed for `argument` when it ended well. Each program
/// runs once for each build of its file, because the status poll meets an
/// older service every two seconds, and a developer who rebuilds
/// gozlerd.exe expects the new build to count.
fn printed(program: &Path, argument: &'static str) -> Option<String> {
    type Build = (SystemTime, u64);
    /// A program, the argument it was given, its build and what it printed.
    type Run = (PathBuf, &'static str, Build, Option<String>);
    static PRINTED: Mutex<Vec<Run>> = Mutex::new(Vec::new());
    let metadata = std::fs::metadata(program).ok()?;
    let build = (metadata.modified().ok()?, metadata.len());
    let mut printed = PRINTED.lock().unwrap_or_else(PoisonError::into_inner);
    let same = |path: &PathBuf, asked: &str| path == program && asked == argument;
    if let Some((.., output)) = printed
        .iter()
        .find(|(path, asked, built, _)| same(path, asked) && *built == build)
    {
        return output.clone();
    }
    let output = run(program, argument);
    printed.retain(|(path, asked, ..)| !same(path, asked));
    printed.push((program.to_owned(), argument, build, output.clone()));
    output
}

/// Runs `program argument` without a console window.
fn run(program: &Path, argument: &str) -> Option<String> {
    let output = Command::new(program)
        .arg(argument)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}
