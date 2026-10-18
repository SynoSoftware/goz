//! Shared, thread-safe per-volume state.
//!
//! The index lives behind a `parking_lot::RwLock`: the volume's tail thread
//! takes the write lock for micro-batches of journal updates, while pipe-server
//! query tasks take read locks. The raw volume handle is NOT stored here (it is
//! `Send` but not `Sync`): it is moved into the tail thread, so `VolumeState`
//! stays `Sync` and can be shared across the tokio runtime via `Arc`.

use std::sync::Arc;

use gozler_core::index::VolumeIndex;
use gozler_core::scope::VolumeMounts;
use gozler_core::types::VolumePhase;
use parking_lot::RwLock;

/// Live state for one indexed volume, shared between its tail thread and the
/// query server.
pub(crate) struct VolumeState {
    pub guid: String,
    pub mounts: Vec<String>,
    pub index: RwLock<VolumeIndex>,
    pub phase: RwLock<VolumePhase>,
    /// The size and date pass of the index's last build failed or stopped, so
    /// some sizes and dates are unknown. Set at bootstrap and by every rescan.
    pub metadata_pending: RwLock<bool>,
    /// Maximum size of the volume's USN journal as of the index's last build,
    /// `None` without a journal. Set at bootstrap and by every rescan.
    /// Surfaced by `--status` because the journal's size bounds how long a
    /// downtime can be replayed instead of rescanned, and a journal that could
    /// not be grown was invisible for months when only the log knew.
    pub journal_max_bytes: RwLock<Option<u64>>,
}

impl VolumeState {
    /// A volume that is not indexed yet: an empty index, phase `Bootstrapping`.
    pub(crate) fn new(guid: String, mounts: Vec<String>) -> Self {
        Self {
            guid,
            mounts,
            index: RwLock::new(VolumeIndex::new()),
            phase: RwLock::new(VolumePhase::Bootstrapping),
            metadata_pending: RwLock::new(false),
            journal_max_bytes: RwLock::new(None),
        }
    }

    /// Where the volume is mounted.
    pub(crate) fn mounted(&self) -> VolumeMounts<'_> {
        VolumeMounts {
            mounts: &self.mounts,
            guid: &self.guid,
        }
    }

    /// The prefix full paths on this volume print with
    /// ([`VolumeMounts::display_prefix`]).
    pub(crate) fn mount_prefix(&self) -> &str {
        self.mounted().display_prefix()
    }
}

/// The indexed volumes at one moment. A request works on one of these from
/// start to end, so a volume that appears or goes meanwhile cannot change its
/// answer halfway.
pub(crate) type VolumeSet = Arc<Vec<Arc<VolumeState>>>;

/// The indexed volumes as they change: the supervisor adds a volume when it
/// appears and removes it when it goes, and every reader takes a
/// [`VolumeSet`] from [`VolumeRegistry::current`].
#[derive(Default)]
pub(crate) struct VolumeRegistry(RwLock<VolumeSet>);

impl VolumeRegistry {
    pub(crate) fn current(&self) -> VolumeSet {
        self.0.read().clone()
    }

    pub(crate) fn add(&self, volume: Arc<VolumeState>) {
        let mut set = self.0.write();
        let mut volumes = Vec::clone(&set);
        volumes.push(volume);
        *set = Arc::new(volumes);
    }

    pub(crate) fn remove(&self, guid: &str) {
        let mut set = self.0.write();
        let volumes: Vec<_> = set.iter().filter(|v| v.guid != guid).cloned().collect();
        *set = Arc::new(volumes);
    }
}
