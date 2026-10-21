//! Saving the Include settings (DESIGN.md: The Include page) and asking for
//! a rebuild of the index (DESIGN.md: The Drives and service page). Only an
//! administrator can write either, so OK or Apply starts a copy of the
//! window as administrator with `--save-include <file>`, and Rebuild with
//! `--rebuild-index`, after Windows asks the person for permission. That
//! copy writes the registry and exits; the service reads it there, where
//! only an administrator can write, so Windows proves who asked. A thread of
//! its own waits for the person's answer and for that copy, so the window
//! keeps working.

use crate::win;
use crate::win::shell::PromptOwner;
use crate::win::wake::OpenWaker;
use gozler_core::include::{INCLUDE_VALUE, IncludeSettings, REBUILD_VALUE, SETTINGS_KEY};
use std::io;
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::SystemTime;

/// The argument that makes the window the copy that saves.
pub(super) const SAVE_ARGUMENT: &str = "--save-include";
/// The argument that makes the window the copy that asks for a rebuild.
pub(super) const REBUILD_ARGUMENT: &str = "--rebuild-index";

/// Windows' code for settings that do not parse.
const ERROR_INVALID_DATA: i32 = 13;

/// The copy that saves: writes the settings in `file` to the registry, and
/// gives the exit code, 0 or the Windows error code of the failure.
pub(super) fn save_as_administrator(file: &Path) -> i32 {
    let written = std::fs::read_to_string(file).and_then(|text| {
        let settings = IncludeSettings::from_json(&text)
            .map_err(|_| io::Error::from_raw_os_error(ERROR_INVALID_DATA))?;
        win::shell::set_machine_string(SETTINGS_KEY, INCLUDE_VALUE, &settings.to_json())
    });
    match written {
        Ok(()) => 0,
        Err(error) => error.raw_os_error().unwrap_or(ERROR_INVALID_DATA),
    }
}

/// The copy that asks for a rebuild: writes a new rebuild token, the time
/// in nanoseconds, to the registry, and gives the exit code, 0 or the
/// Windows error code of the failure.
pub(super) fn rebuild_as_administrator() -> i32 {
    let token = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();
    match win::shell::set_machine_string(SETTINGS_KEY, REBUILD_VALUE, &token) {
        Ok(()) => 0,
        Err(error) => error.raw_os_error().unwrap_or(ERROR_INVALID_DATA),
    }
}

/// What the copy that runs as administrator does.
pub(super) enum Elevated {
    /// Saves these Include settings.
    Include(IncludeSettings),
    /// Asks the service to rebuild the index.
    Rebuild,
}

/// Why an [`Elevated`] job did not run.
pub(super) enum NotSaved {
    /// The person said no to Windows' permission prompt.
    Declined,
    /// Windows' words for the failure.
    Failed(String),
}

/// An [`Elevated`] job that runs on its own thread.
pub(super) struct Saving {
    thread: JoinHandle<()>,
    answer: Receiver<Result<(), NotSaved>>,
}

impl Saving {
    /// Runs `job` on a new thread, which wakes the window with its answer.
    /// Windows places its permission prompt over `owner`, the window, which
    /// joins the thread before it is destroyed.
    pub(super) fn start(owner: PromptOwner, job: Elevated, waker: OpenWaker) -> Saving {
        let (sender, answer) = channel();
        let thread = std::thread::spawn(move || {
            let saved = run(owner, &job);
            waker.deliver(|| sender.send(saved).is_ok());
        });
        Saving { thread, answer }
    }

    /// The answer, once it has arrived; the thread has then ended, or ends
    /// at once.
    pub(super) fn answer(&self) -> Option<Result<(), NotSaved>> {
        self.answer.try_recv().ok()
    }

    /// Waits for the thread, which ends once the person answered the
    /// prompt and the copy that runs as administrator has ended.
    pub(super) fn join(self) {
        let _ = self.thread.join();
    }
}

/// Runs `job` in a copy of the window that runs as administrator, and waits
/// for it.
fn run(owner: PromptOwner, job: &Elevated) -> Result<(), NotSaved> {
    let failed = |error: &io::Error| NotSaved::Failed(win::error_text(error));
    let file = match job {
        Elevated::Include(settings) => {
            let file =
                std::env::temp_dir().join(format!("gozler-include-{}.json", std::process::id()));
            std::fs::write(&file, settings.to_json()).map_err(|e| failed(&e))?;
            Some(file)
        }
        Elevated::Rebuild => None,
    };
    let arguments = match &file {
        Some(file) => format!("{SAVE_ARGUMENT} \"{}\"", file.display()),
        None => REBUILD_ARGUMENT.to_owned(),
    };
    let program = std::env::current_exe().map_err(|e| failed(&e))?;
    let ran = win::shell::run_as_administrator_and_wait(
        owner,
        &program.display().to_string(),
        &arguments,
    );
    if let Some(file) = file {
        let _ = std::fs::remove_file(&file);
    }
    match ran {
        Ok(0) => Ok(()),
        Ok(code) => Err(failed(&io::Error::from_raw_os_error(code as i32))),
        Err(error) if win::shell::refused(&error) => Err(NotSaved::Declined),
        Err(error) => Err(failed(&error)),
    }
}
