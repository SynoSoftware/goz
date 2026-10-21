//! Shared, thread-safe per-volume state.
//!
//! The raw volume handle is NOT stored here (it is `Send` but not `Sync`): it
//! is moved into the tail thread, so `VolumeState` stays `Sync` and can be
//! shared across the tokio runtime via `Arc`.
//!
//! A volume has two `parking_lot::RwLock`s. One holds [`Indexed`], the index
//! and the facts of the build that made it: the tail takes its write lock for
//! micro-batches of journal updates, and pipe-server requests take read locks.
//! The other holds the phase. The rules:
//!
//! - The index lock is taken before the phase lock, and nothing holds the
//!   phase lock while it takes the index lock.
//! - A request that holds several volumes' index locks at once, as each page
//!   of a reply does, takes them in [`VolumeSet`] order. A `parking_lot`
//!   reader waits behind a waiting writer, so two requests that took them in
//!   different orders could each wait for the other's volume.
//! - A phase change never takes the index lock, because a `parking_lot` writer
//!   that waits behind a long export blocks every new reader. A build installs
//!   its index and releases the lock before it sets the phase, and
//!   [`VolumeState::begin_build`] reads the index the same way, before it
//!   sets the phase.
//! - Build facts change only with the index, in [`Building::finish`].
//! - [`VolumeState::compact_index`] holds the only upgradable read, and
//!   nothing else takes one, because an upgradable read queues behind another.
//!
//! [`VolumeState::read`] reads the phase while it holds the index, and that
//! phase describes that index: an index is swapped only while the phase is
//! `Bootstrapping` or `Rescanning`, and the phase after the build is set after
//! the swap.
//!
//! The phase changes only through the methods of [`VolumeState`], each an
//! event in one table, [`next`], so no caller can set a phase the table does
//! not lead to. Two writers hand the phase over: the tail writes it while it
//! runs, and the supervisor writes it only after the tail's pulse reads
//! `Retired`, or when no tail ever started, and never while a start is
//! pending. So the phase a writer last saw is the phase it moves from.

use std::sync::Arc;
use std::time::Instant;

use gozler_core::include::VolumeRules;
use gozler_core::index::settle::SharedIndex;
use gozler_core::index::{NTFS_ROOT_FRN, VolumeIndex};
use gozler_core::scope::VolumeMounts;
use gozler_core::types::VolumePhase;
use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, Mutex, RwLock, RwLockReadGuard,
    RwLockUpgradableReadGuard, RwLockWriteGuard,
};

use crate::bootstrap::{BuiltIndex, Filesystem, JournalSetup};

/// Live state for one indexed volume, shared between its tail thread and the
/// query server.
pub(crate) struct VolumeState {
    pub guid: String,
    pub mounts: Vec<String>,
    pub filesystem: Filesystem,
    indexed: RwLock<Indexed>,
    phase: RwLock<VolumePhase>,
    /// The Include rules a build of the volume applies, and whether they
    /// changed since the last build began or the tail last took them, so the
    /// index does not apply them yet. Never held with another lock.
    rules: Mutex<Rules>,
}

struct Rules {
    rules: Arc<VolumeRules>,
    changed: bool,
    /// A new tail was started to build the volume for changed rules, and no
    /// build has begun since, so the index follows older ones.
    unapplied: bool,
}

/// An index and the facts of the build that made it, installed together.
pub(crate) struct Indexed {
    pub index: VolumeIndex,
    /// The size and date pass of the build failed or stopped, so some sizes
    /// and dates are unknown.
    pub metadata_pending: bool,
    /// Maximum size of the volume's USN journal as of the build, `None`
    /// without a journal. Surfaced by `--status` because the journal's size
    /// bounds how long a downtime can be replayed instead of rescanned.
    pub journal_max_bytes: Option<u64>,
    /// A finished build made this index, so the volume's next build is a
    /// rebuild. Only the empty index of a volume never indexed lacks it.
    pub built: bool,
}

/// One volume as a request reads it: the index held, and the phase read while
/// it was held.
pub(crate) struct VolumeRead<'a> {
    indexed: RwLockReadGuard<'a, Indexed>,
    phase: VolumePhase,
}

impl VolumeRead<'_> {
    pub(crate) fn index(&self) -> &VolumeIndex {
        &self.indexed.index
    }

    pub(crate) fn phase(&self) -> &VolumePhase {
        &self.phase
    }

    pub(crate) fn metadata_pending(&self) -> bool {
        self.indexed.metadata_pending
    }

    pub(crate) fn journal_max_bytes(&self) -> Option<u64> {
        self.indexed.journal_max_bytes
    }
}

/// A build of a volume's index under way, from [`VolumeState::begin_build`].
/// Dropping it without [`Building::finish`] leaves the old index and the
/// build's phase in place, as a build that failed or was stopped does.
#[must_use = "a build that is not finished leaves the volume Bootstrapping or Rescanning"]
pub(crate) struct Building<'a> {
    state: &'a VolumeState,
    first: bool,
}

impl Building<'_> {
    /// Whether this is the volume's first build rather than a rebuild.
    pub(crate) fn is_first(&self) -> bool {
        self.first
    }

    /// Installs `built` with its build facts, then sets the phase that
    /// `journal` leads to.
    pub(crate) fn finish(self, built: BuiltIndex, journal: &JournalSetup) {
        *self.state.indexed.write() = Indexed {
            index: built.index,
            metadata_pending: built.metadata_pending,
            journal_max_bytes: match journal {
                JournalSetup::Active(c) => Some(c.maximum_size),
                JournalSetup::Refused | JournalSetup::Unavailable => None,
            },
            built: true,
        };
        self.state.apply(Event::Built(journal));
    }
}

impl VolumeState {
    /// A volume that is not indexed yet: an empty index, phase `Bootstrapping`.
    /// The first build replaces the index with one rooted at the volume's own
    /// root. Until then, `--status` reports the NTFS root's map kind, even on
    /// ReFS.
    pub(crate) fn new(
        guid: String,
        mounts: Vec<String>,
        filesystem: Filesystem,
        rules: Arc<VolumeRules>,
    ) -> Self {
        Self {
            guid,
            mounts,
            filesystem,
            indexed: RwLock::new(Indexed {
                index: VolumeIndex::new(NTFS_ROOT_FRN),
                metadata_pending: false,
                journal_max_bytes: None,
                built: false,
            }),
            phase: RwLock::new(VolumePhase::Bootstrapping),
            rules: Mutex::new(Rules {
                rules,
                changed: false,
                unapplied: false,
            }),
        }
    }

    /// The Include rules that a build of the volume applies.
    pub(crate) fn rules(&self) -> Arc<VolumeRules> {
        self.rules.lock().rules.clone()
    }

    /// Gives the volume new Include rules, which its tail applies at its next
    /// turn. Rules that keep and leave out the same entries as the current
    /// ones change nothing, so the tail does not check the index for them.
    pub(crate) fn set_rules(&self, rules: Arc<VolumeRules>) {
        let mut slot = self.rules.lock();
        if rules.adds_to(&slot.rules) || slot.rules.adds_to(&rules) {
            *slot = Rules {
                rules,
                changed: true,
                unapplied: slot.unapplied,
            };
        }
    }

    /// The rules set since the last call or the last build began, which the
    /// index does not apply yet.
    pub(crate) fn take_new_rules(&self) -> Option<Arc<VolumeRules>> {
        let mut slot = self.rules.lock();
        std::mem::take(&mut slot.changed).then(|| slot.rules.clone())
    }

    /// Takes the rules set since the last build began, for the build of a
    /// new tail, which a volume without a running tail needs to apply them:
    /// a snapshot, or one given up on. Until that build begins, the volume
    /// reads as not complete, because its index follows the old rules. Call
    /// it only when no tail runs.
    pub(crate) fn take_rules_for_rebuild(&self) -> bool {
        {
            let mut slot = self.rules.lock();
            if !std::mem::take(&mut slot.changed) {
                return false;
            }
            slot.unapplied = true;
        }
        self.apply(Event::RulesChanged);
        true
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

    /// The volume for a request: its index, build facts and phase, which
    /// describe each other (see the module doc).
    pub(crate) fn read(&self) -> VolumeRead<'_> {
        let indexed = self.indexed.read();
        let phase = self.phase();
        VolumeRead { indexed, phase }
    }

    /// The index alone, for the tail and the supervisor. A reply to a client
    /// reads through [`read`](Self::read), so it reports the phase of the
    /// index that answered it.
    pub(crate) fn index(&self) -> MappedRwLockReadGuard<'_, VolumeIndex> {
        RwLockReadGuard::map(self.indexed.read(), |i| &i.index)
    }

    /// The index for the tail's journal batches. A build's facts change only
    /// with its index, in [`Building::finish`].
    pub(crate) fn index_mut(&self) -> MappedRwLockWriteGuard<'_, VolumeIndex> {
        RwLockWriteGuard::map(self.indexed.write(), |i| &mut i.index)
    }

    pub(crate) fn phase(&self) -> VolumePhase {
        self.phase.read().clone()
    }

    /// Compacts the name arenas or merges the name ranks, blocking queries
    /// only for the install.
    ///
    /// The expensive half copies every live name into fresh arenas (hundreds
    /// of MB on a large volume), or sorts the unranked names and merges them
    /// into the rank order, so it runs while queries keep reading. The
    /// plan is valid only while the index does not change, and the upgradable
    /// read keeps every writer out until it upgrades, so the plan cannot go
    /// stale whoever else writes. The upgrade to exclusive covers only the
    /// column writes.
    pub(crate) fn compact_index(&self) {
        let started = Instant::now();
        let guard = self.indexed.upgradable_read();
        let Some(plan) = guard.index.plan_compaction() else {
            return; // raced with another compaction, or no longer worth it
        };
        let planned = started.elapsed();

        let install_started = Instant::now();
        let mut indexed = RwLockUpgradableReadGuard::upgrade(guard);
        indexed.index.apply_compaction(plan);
        drop(indexed);

        // `blocked_ms` is the only part that stops queries; `planned_ms` runs
        // alongside them.
        tracing::debug!(
            volume = %self.mount_prefix(),
            planned_ms = planned.as_millis() as u64,
            blocked_ms = install_started.elapsed().as_millis() as u64,
            "compacted the name store"
        );
    }

    /// The tail starts building the index. Read the rules for the build after
    /// this call: it marks the rules set so far as applied.
    pub(crate) fn begin_build(&self) -> Building<'_> {
        {
            let mut slot = self.rules.lock();
            slot.changed = false;
            slot.unapplied = false;
        }
        let first = !self.indexed.read().built;
        self.apply(Event::BuildStarted { first });
        Building { state: self, first }
    }

    /// The tail's journal reads have kept failing long enough that the index
    /// no longer tracks the volume.
    pub(crate) fn reads_keep_failing(&self) {
        self.apply(Event::ReadsKeepFailing);
    }

    /// The tail's journal reads answer again after
    /// [`reads_keep_failing`](Self::reads_keep_failing).
    pub(crate) fn reads_recovered(&self) {
        self.apply(Event::ReadsRecovered);
    }

    /// The tail stops for good, for `reason`.
    pub(crate) fn fail(&self, reason: String) {
        self.apply(Event::Failed(reason));
    }

    /// The supervisor is about to start a tail in place of one that retired
    /// or never started. Call it only after the tail's pulse reads `Retired`,
    /// or when no tail started.
    pub(crate) fn tail_retired(&self) {
        self.apply(Event::TailRetired);
    }

    /// The supervisor stops starting tails for this volume, after `attempts`
    /// revivals; `last_failure` is why the last start failed, if it did. Call
    /// it only after the tail's pulse reads `Retired`, or when no tail
    /// started.
    pub(crate) fn give_up(&self, last_failure: Option<&str>, attempts: u32) {
        let rules_unapplied = self.rules.lock().unapplied;
        self.apply(Event::GaveUp {
            last_failure,
            attempts,
            rules_unapplied,
        });
    }

    /// Moves the phase as [`next`] says. A pair the table refuses is a bug in
    /// a caller, so the phase stays and the refusal is logged.
    fn apply(&self, event: Event<'_>) {
        let mut phase = self.phase.write();
        let to = next(&phase, &event);
        if to.is_none() {
            tracing::error!(
                volume = %self.mount_prefix(),
                phase = ?*phase,
                ?event,
                "refused a phase change the volume's phase table does not allow"
            );
        }
        debug_assert!(to.is_some(), "{:?} refused {event:?}", *phase);
        if let Some(to) = to {
            *phase = to;
        }
    }

    #[cfg(test)]
    pub(crate) fn in_state(mount: &str, indexed: Indexed, phase: VolumePhase) -> Self {
        Self {
            guid: r"\\?\Volume{00000000-0000-0000-0000-000000000000}\".to_string(),
            mounts: vec![mount.to_string()],
            filesystem: Filesystem::Ntfs,
            indexed: RwLock::new(indexed),
            phase: RwLock::new(phase),
            rules: Mutex::new(Rules {
                rules: Arc::new(gozler_core::include::IncludeSettings::default().rules_for("")),
                changed: false,
                unapplied: false,
            }),
        }
    }
}

impl SharedIndex for VolumeState {
    fn read<R>(&self, f: impl FnOnce(&VolumeIndex) -> R) -> R {
        f(&self.index())
    }

    fn write<R>(&self, f: impl FnOnce(&mut VolumeIndex) -> R) -> R {
        f(&mut self.index_mut())
    }
}

/// What happened to a volume, as [`next`] reads it.
#[derive(Debug)]
enum Event<'a> {
    BuildStarted {
        first: bool,
    },
    Built(&'a JournalSetup),
    ReadsKeepFailing,
    ReadsRecovered,
    Failed(String),
    TailRetired,
    /// A new tail starts to build a volume that no tail ran, for changed
    /// Include rules.
    RulesChanged,
    GaveUp {
        last_failure: Option<&'a str>,
        attempts: u32,
        /// The tails started for changed Include rules never began a build.
        rules_unapplied: bool,
    },
}

/// The phase `event` moves a volume in `phase` to, or `None` for a pair the
/// volume can never meet.
fn next(phase: &VolumePhase, event: &Event<'_>) -> Option<VolumePhase> {
    use VolumePhase::{Bootstrapping, Failed, Live, Offline, Rescanning, Snapshot};
    let to = match (phase, event) {
        (Offline, Event::ReadsRecovered) => Live,
        (Live, Event::ReadsKeepFailing) => Offline,
        // A volume's first build keeps the phase `Bootstrapping`, so
        // `--status` tells a volume that was never indexed from one being
        // rebuilt. A volume whose first build failed is still never indexed.
        (Bootstrapping | Failed { .. }, Event::BuildStarted { first: true }) => Bootstrapping,
        (Live | Offline | Failed { .. } | Snapshot, Event::BuildStarted { first: false }) => {
            Rescanning
        }
        (Bootstrapping | Rescanning, Event::Built(journal)) => match journal {
            JournalSetup::Active(_) => Live,
            // Snapshot, not Offline: nothing is wrong and nothing will change,
            // so the volume must not flag every result incomplete.
            JournalSetup::Refused => Snapshot,
            // Built, but with nothing to tail, so the volume is not tracking
            // and its results are flagged incomplete.
            JournalSetup::Unavailable => Offline,
        },
        (Bootstrapping | Rescanning | Live | Offline, Event::Failed(reason)) => Failed {
            reason: reason.clone(),
        },
        // Every deliberate exit of a tail sets Failed or Offline first, so a
        // phase still claiming to track is the panic signature. It is
        // corrected before the revival starts, so a revival blocked on sick
        // hardware cannot extend a false Live indefinitely.
        (Live | Rescanning, Event::TailRetired) => Offline,
        (Bootstrapping | Offline | Failed { .. } | Snapshot, Event::TailRetired) => phase.clone(),
        // The index follows the old Include rules until the new tail's build
        // begins, so the results are flagged incomplete until then, as a
        // built volume that nothing tails. A volume never indexed or given
        // up on already reads as incomplete.
        (Snapshot, Event::RulesChanged) => Offline,
        (Bootstrapping | Failed { .. }, Event::RulesChanged) => phase.clone(),
        // The tail never started or never finished the first build. Without
        // Failed the volume would claim to be indexing forever.
        (Bootstrapping, Event::GaveUp { last_failure, .. }) => Failed {
            reason: format!(
                "never indexed: {}",
                last_failure.unwrap_or("its tail kept stopping")
            ),
        },
        // The last revived tail died while tracking, the panic signature, and
        // no revival follows to correct it.
        (Live | Rescanning, Event::GaveUp { .. }) => Failed {
            reason: "tail thread kept dying; live updates stopped".into(),
        },
        // The tails started to apply changed Include settings never built
        // the volume, so its index still follows the old ones.
        (
            Offline,
            Event::GaveUp {
                last_failure,
                rules_unapplied: true,
                ..
            },
        )
        | (Snapshot, Event::GaveUp { last_failure, .. }) => Failed {
            reason: format!(
                "the changed Include settings were not applied: {}",
                last_failure.unwrap_or("its tail kept stopping")
            ),
        },
        // Retrying has stopped, and a volume that is no longer retried is
        // Failed, not Offline.
        (Offline, Event::GaveUp { attempts, .. }) => Failed {
            reason: format!("live updates stopped and {attempts} revivals did not restart them"),
        },
        // The tail's own exit already set this reason, and logged it.
        (Failed { .. }, Event::GaveUp { .. }) => phase.clone(),
        _ => return None,
    };
    Some(to)
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

#[cfg(test)]
mod tests {
    use super::{Indexed, VolumeState};
    use crate::bootstrap::{BuiltIndex, Filesystem, JournalSetup};
    use gozler_core::index::{NTFS_ROOT_FRN, VolumeIndex};
    use gozler_core::types::{Frn, VolumePhase};
    use gozler_core::usn::record::{ParsedUsnRecord, USN_REASON_FILE_CREATE};
    use gozler_winfs::JournalInfo;
    use std::sync::Arc;

    /// A finished build of an index holding the root and `files` files.
    fn built(files: u128, metadata_pending: bool) -> BuiltIndex {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        for frn in 100..100 + files {
            index.insert_enum(&ParsedUsnRecord {
                frn: Frn(frn),
                parent_frn: NTFS_ROOT_FRN,
                reason: USN_REASON_FILE_CREATE,
                attributes: 0,
                name: format!("{frn}.txt").into_bytes(),
            });
        }
        BuiltIndex {
            index,
            read_secs: 0.0,
            enum_secs: None,
            metadata_pending,
        }
    }

    /// A rebuild serves the old index with its own facts, flagged incomplete,
    /// until the new index installs with the new facts; a first build is not
    /// reported as a rebuild, and a build without a journal does not report
    /// complete.
    #[test]
    fn a_rebuild_serves_its_old_index_as_rescanning_until_it_installs() {
        let volume = VolumeState::new(
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\".into(),
            vec![r"C:\".into()],
            Filesystem::Ntfs,
            Arc::new(gozler_core::include::IncludeSettings::default().rules_for("")),
        );
        let first = volume.begin_build();
        assert!(first.is_first());
        assert_eq!(volume.read().phase(), &VolumePhase::Bootstrapping);
        let a = built(1, false);
        let a_len = a.index.len();
        let journal = JournalSetup::Active(JournalInfo {
            journal_id: 1,
            first_usn: 0,
            next_usn: 0,
            lowest_valid_usn: 0,
            max_usn: i64::MAX,
            maximum_size: 32 << 20,
        });
        first.finish(a, &journal);
        {
            let read = volume.read();
            assert_eq!(read.phase(), &VolumePhase::Live);
            assert_eq!(read.index().len(), a_len);
            assert_eq!(read.journal_max_bytes(), Some(32 << 20));
        }

        let rebuild = volume.begin_build();
        assert!(!rebuild.is_first());
        {
            let read = volume.read();
            assert_eq!(read.phase(), &VolumePhase::Rescanning);
            assert_eq!(read.index().len(), a_len);
            assert!(!read.metadata_pending());
            assert_eq!(read.journal_max_bytes(), Some(32 << 20));
        }
        let b = built(2, true);
        let b_len = b.index.len();
        assert_ne!(a_len, b_len);
        rebuild.finish(b, &JournalSetup::Unavailable);
        let read = volume.read();
        assert_eq!(read.phase(), &VolumePhase::Offline);
        assert_eq!(read.index().len(), b_len);
        assert!(read.metadata_pending());
        assert_eq!(read.journal_max_bytes(), None);
    }

    /// A tail that died, by panic or otherwise, leaves no volume reading as
    /// tracking: while its revival is blocked on sick hardware, a volume still
    /// Live would report complete results it no longer keeps current. Once
    /// revivals stop, every volume is Failed: one never indexed must not claim
    /// to be indexing forever.
    #[test]
    fn a_dead_tail_leaves_no_volume_claiming_to_track() {
        for phase in [
            VolumePhase::Bootstrapping,
            VolumePhase::Live,
            VolumePhase::Rescanning,
            VolumePhase::Offline,
            VolumePhase::Failed { reason: "x".into() },
            VolumePhase::Snapshot,
        ] {
            let in_phase = || {
                let indexed = Indexed {
                    index: VolumeIndex::new(NTFS_ROOT_FRN),
                    metadata_pending: false,
                    journal_max_bytes: None,
                    built: phase != VolumePhase::Bootstrapping,
                };
                VolumeState::in_state(r"C:\", indexed, phase.clone())
            };
            let volume = in_phase();
            volume.tail_retired();
            assert!(
                !matches!(volume.phase(), VolumePhase::Live | VolumePhase::Rescanning),
                "retired from {phase:?}: {:?}",
                volume.phase()
            );

            let volume = in_phase();
            volume.give_up(Some("open failed"), 3);
            let after = volume.phase();
            assert!(
                matches!(after, VolumePhase::Failed { .. }),
                "gave up from {phase:?}: {after:?}"
            );
            if phase == VolumePhase::Bootstrapping {
                assert!(
                    matches!(&after, VolumePhase::Failed { reason } if reason.starts_with("never indexed: open failed")),
                    "{after:?}"
                );
            }
        }
    }

    /// A snapshot volume given changed Include rules has no tail to apply
    /// them, and a build by a new tail must. Until that build begins, its
    /// index follows the old rules, so it must not read as complete, also
    /// while the new tail fails to start; giving up then says the settings
    /// were not applied.
    #[test]
    fn a_snapshot_with_unapplied_rules_reads_incomplete_until_its_build() {
        let snapshot = || {
            let indexed = Indexed {
                index: VolumeIndex::new(NTFS_ROOT_FRN),
                metadata_pending: false,
                journal_max_bytes: None,
                built: true,
            };
            let volume = VolumeState::in_state(r"D:\", indexed, VolumePhase::Snapshot);
            let settings = gozler_core::include::IncludeSettings {
                exclude_hidden: true,
                ..Default::default()
            };
            volume.set_rules(Arc::new(settings.rules_for("")));
            volume
        };
        let volume = snapshot();
        assert!(volume.take_rules_for_rebuild());
        assert!(!volume.read().phase().is_complete());
        volume.tail_retired();
        assert!(!volume.phase().is_complete(), "a start that failed");
        volume.give_up(Some("open failed"), 3);
        assert_eq!(
            volume.phase(),
            VolumePhase::Failed {
                reason: "the changed Include settings were not applied: open failed".into()
            }
        );

        let volume = snapshot();
        assert!(volume.take_rules_for_rebuild());
        let rebuild = volume.begin_build();
        rebuild.finish(built(1, false), &JournalSetup::Refused);
        assert_eq!(volume.phase(), VolumePhase::Snapshot);
        assert!(!volume.take_rules_for_rebuild(), "the build applied them");
    }

    /// A volume whose first build failed was still never indexed, so the
    /// build its revived tail starts is a first build too, and `--status`
    /// calls the volume bootstrapping, not rescanning.
    #[test]
    fn a_revival_after_a_failed_first_build_still_bootstraps() {
        let volume = VolumeState::new(
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\".into(),
            vec![r"C:\".into()],
            Filesystem::Ntfs,
            Arc::new(gozler_core::include::IncludeSettings::default().rules_for("")),
        );
        let _ = volume.begin_build();
        volume.fail("x".into());
        volume.tail_retired();
        let revived = volume.begin_build();
        assert!(revived.is_first());
        assert_eq!(volume.phase(), VolumePhase::Bootstrapping);
    }
}
