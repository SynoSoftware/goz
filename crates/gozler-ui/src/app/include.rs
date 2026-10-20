//! Saving the Include settings (DESIGN.md: The Include page). Only an
//! administrator can write them, so OK or Apply starts a copy of the window
//! as administrator with `--save-include <file>`, after Windows asks the
//! person for permission. That copy writes the settings to the registry and
//! exits; the service reads them there, where only an administrator can
//! write, so Windows proves who saved them.

use crate::win::{self, Hwnd};
use gozler_core::include::{INCLUDE_VALUE, IncludeSettings, SETTINGS_KEY};
use std::io;
use std::path::Path;

/// The argument that makes the window the copy that saves.
pub(super) const SAVE_ARGUMENT: &str = "--save-include";

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

/// Why the settings were not saved.
pub(super) enum NotSaved {
    /// The person said no to Windows' permission prompt.
    Declined,
    /// Windows' words for the failure.
    Failed(String),
}

/// Saves `settings` through a copy of the window that runs as
/// administrator, and waits for it.
pub(super) fn save(owner: Hwnd, settings: &IncludeSettings) -> Result<(), NotSaved> {
    let failed = |error: &io::Error| NotSaved::Failed(win::error_text(error));
    let file = std::env::temp_dir().join(format!("gozler-include-{}.json", std::process::id()));
    std::fs::write(&file, settings.to_json()).map_err(|e| failed(&e))?;
    let program = std::env::current_exe().map_err(|e| failed(&e))?;
    let ran = win::shell::run_as_administrator_and_wait(
        owner,
        &program.display().to_string(),
        &format!("{SAVE_ARGUMENT} \"{}\"", file.display()),
    );
    let _ = std::fs::remove_file(&file);
    match ran {
        Ok(0) => Ok(()),
        Ok(code) => Err(failed(&io::Error::from_raw_os_error(code as i32))),
        Err(error) if win::shell::refused(&error) => Err(NotSaved::Declined),
        Err(error) => Err(failed(&error)),
    }
}
