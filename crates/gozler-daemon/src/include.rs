//! The Include settings the service applies, and the rebuilds asked of it.
//! An administrator saves both in the registry, where only administrators
//! and the system can write, so the service trusts what it reads there and
//! needs no check of its own on who asked it to read them
//! (`Request::SettingsSaved`).

use std::sync::Arc;

use gozler_core::include::{INCLUDE_VALUE, IndexSettings, REBUILD_VALUE, SETTINGS_KEY};
use gozler_core::proto::LeftOutVolume;
use parking_lot::Mutex;

/// The saved settings; the defaults, which index every drive, when none are
/// saved or they cannot be read.
pub(crate) fn read_saved() -> IndexSettings {
    match gozler_winfs::machine_string(SETTINGS_KEY, INCLUDE_VALUE) {
        Ok(None) => IndexSettings::default(),
        Ok(Some(json)) => IndexSettings::from_json(&json).unwrap_or_else(|e| {
            tracing::error!(error = %e, "the saved Include settings do not parse; indexing every drive");
            IndexSettings::default()
        }),
        Err(e) => {
            tracing::error!(error = %e, "could not read the saved Include settings; indexing every drive");
            IndexSettings::default()
        }
    }
}

/// The token of the latest rebuild an administrator asked for, `None` when
/// none was asked or it cannot be read.
pub(crate) fn read_rebuild() -> Option<String> {
    gozler_winfs::machine_string(SETTINGS_KEY, REBUILD_VALUE).unwrap_or_else(|e| {
        tracing::error!(error = %e, "could not read whether a rebuild was asked for");
        None
    })
}

/// The settings the service applies and the volumes they leave out, shared
/// by the supervisor, which applies them, and the pipe server, which reports
/// them and reads them again.
pub(crate) struct Include {
    applied: Mutex<Arc<IndexSettings>>,
    left_out: Mutex<Vec<LeftOutVolume>>,
    rebuild: Mutex<Rebuild>,
    /// Tells the supervisor that [`Self::applied`] changed or a rebuild was
    /// asked for.
    wake: Box<dyn Fn() + Send + Sync>,
}

struct Rebuild {
    /// The token read last, so only a new one asks for a rebuild.
    seen: Option<String>,
    /// A new token was read and the supervisor has not taken it.
    asked: bool,
}

impl Include {
    /// `rebuild` is the token saved when the service started, which asks
    /// for nothing, because a start builds every volume anyway.
    pub(crate) fn new(
        applied: IndexSettings,
        rebuild: Option<String>,
        wake: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        Include {
            applied: Mutex::new(Arc::new(applied)),
            left_out: Mutex::new(Vec::new()),
            rebuild: Mutex::new(Rebuild {
                seen: rebuild,
                asked: false,
            }),
            wake,
        }
    }

    /// The settings the service applies.
    pub(crate) fn applied(&self) -> Arc<IndexSettings> {
        self.applied.lock().clone()
    }

    /// The fixed NTFS and ReFS volumes the settings leave out.
    pub(crate) fn left_out(&self) -> Vec<LeftOutVolume> {
        self.left_out.lock().clone()
    }

    pub(crate) fn set_left_out(&self, volumes: Vec<LeftOutVolume>) {
        *self.left_out.lock() = volumes;
    }

    /// Whether a rebuild was asked for since the last call.
    pub(crate) fn take_rebuild(&self) -> bool {
        std::mem::take(&mut self.rebuild.lock().asked)
    }

    /// Reads the saved settings and the rebuild token, and has the
    /// supervisor apply settings that differ from the applied ones and
    /// rebuild for a token it has not seen.
    pub(crate) fn reload(&self) {
        let saved = read_saved();
        let changed = {
            let mut applied = self.applied.lock();
            let changed = **applied != saved;
            if changed {
                tracing::info!("the Include settings changed; applying them");
                *applied = Arc::new(saved);
            }
            changed
        };
        let token = read_rebuild();
        let asked = {
            let mut rebuild = self.rebuild.lock();
            let asked = token.is_some() && token != rebuild.seen;
            if asked {
                tracing::info!("a rebuild of the index was asked for");
                rebuild.seen = token;
                rebuild.asked = true;
            }
            asked
        };
        if changed || asked {
            (self.wake)();
        }
    }
}
