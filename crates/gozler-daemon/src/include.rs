//! The Include settings the service applies. An administrator saves them in
//! the registry, where only administrators and the system can write, so the
//! service trusts what it reads there and needs no check of its own on who
//! asked it to read them (`Request::SettingsSaved`).

use std::sync::Arc;

use gozler_core::include::{INCLUDE_VALUE, IncludeSettings, SETTINGS_KEY};
use gozler_core::proto::LeftOutVolume;
use parking_lot::Mutex;

/// The saved settings; the defaults, which index every drive, when none are
/// saved or they cannot be read.
pub(crate) fn read_saved() -> IncludeSettings {
    match gozler_winfs::machine_string(SETTINGS_KEY, INCLUDE_VALUE) {
        Ok(None) => IncludeSettings::default(),
        Ok(Some(json)) => IncludeSettings::from_json(&json).unwrap_or_else(|e| {
            tracing::error!(error = %e, "the saved Include settings do not parse; indexing every drive");
            IncludeSettings::default()
        }),
        Err(e) => {
            tracing::error!(error = %e, "could not read the saved Include settings; indexing every drive");
            IncludeSettings::default()
        }
    }
}

/// The settings the service applies and the volumes they leave out, shared
/// by the supervisor, which applies them, and the pipe server, which reports
/// them and reads them again.
pub(crate) struct Include {
    applied: Mutex<Arc<IncludeSettings>>,
    left_out: Mutex<Vec<LeftOutVolume>>,
    /// Tells the supervisor that [`Self::applied`] changed.
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Include {
    pub(crate) fn new(applied: IncludeSettings, wake: Box<dyn Fn() + Send + Sync>) -> Self {
        Include {
            applied: Mutex::new(Arc::new(applied)),
            left_out: Mutex::new(Vec::new()),
            wake,
        }
    }

    /// The settings the service applies.
    pub(crate) fn applied(&self) -> Arc<IncludeSettings> {
        self.applied.lock().clone()
    }

    /// The fixed NTFS and ReFS volumes the settings leave out.
    pub(crate) fn left_out(&self) -> Vec<LeftOutVolume> {
        self.left_out.lock().clone()
    }

    pub(crate) fn set_left_out(&self, volumes: Vec<LeftOutVolume>) {
        *self.left_out.lock() = volumes;
    }

    /// Reads the saved settings and, when they differ from the applied
    /// ones, has the supervisor apply them.
    pub(crate) fn reload(&self) {
        let saved = read_saved();
        {
            let mut applied = self.applied.lock();
            if **applied == saved {
                return;
            }
            tracing::info!("the Include settings changed; applying them");
            *applied = Arc::new(saved);
        }
        (self.wake)();
    }
}
