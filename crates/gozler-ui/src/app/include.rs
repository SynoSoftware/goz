//! Saving the Include settings (DESIGN.md: The Include page) and asking for
//! a rebuild of the index (DESIGN.md: The Drives and service page). Only an
//! administrator can write either, so OK or Apply starts a copy of the
//! window as administrator with `--save-include <file>`, and Rebuild with
//! `--rebuild-index`, after Windows asks the person for permission. That
//! copy writes the registry and exits; the service reads it there, where
//! only an administrator can write, so Windows proves who asked. The work
//! owner waits for that copy without retaining a window handle.

use crate::win;
use gozler_core::include::{INCLUDE_VALUE, IndexSettings, REBUILD_VALUE, SETTINGS_KEY};
use std::io;
use std::path::Path;
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
        let settings = IndexSettings::from_json(&text)
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
    Include(IndexSettings),
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
