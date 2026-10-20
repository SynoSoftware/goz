//! Prepare's totals, from gozlerd's index, because the person never waits
//! for a walk that counts (D5). gozlerd answers for a folder only where its
//! index holds everything the disk holds below it: on a live volume, with
//! every size known, and with Include settings that keep every entry there.
//! Anywhere else the operation has no total.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gozler_core::proto::{DaemonStatus, VolumeStatus};

use crate::identity::Identity;
use crate::operation::Total;

/// Where prepare reads the totals of the selected folders.
pub(crate) trait Index: Send + Sync {
    /// The items and bytes below each of `folders`, in their order, or
    /// `None` when gozlerd cannot answer exactly for one of them. Stops
    /// waiting for gozlerd once `cancel` is set.
    fn below(&self, folders: &[&Identity], cancel: &Arc<AtomicBool>) -> Option<Vec<Total>>;
}

/// The search scope that names `folder` to gozlerd, or `None` when the
/// index may hold less than the disk below it: gozlerd does not index its
/// volume, or the Include settings leave entries out there.
fn scope(status: &DaemonStatus, folder: &Identity) -> Option<String> {
    let guid = String::from_utf16(folder.volume()).ok()?;
    let same_volume = |volume: &&VolumeStatus| {
        let path = volume.guid.trim_end_matches('\\');
        path.eq_ignore_ascii_case(&guid)
    };
    let volume = status.volumes.iter().find(same_volume)?;
    // A daemon before Include settings indexes every entry.
    if (status.include.as_ref())
        .is_some_and(|include| !include.rules_for(&volume.guid).keeps_everything())
    {
        return None;
    }
    let below = String::from_utf16(folder.below_volume()).ok()?;
    Some(format!("{}{below}", volume.mounted().display_prefix()))
}

#[cfg(windows)]
pub(crate) use gozlerd::{Counting, Gozlerd, totals};

#[cfg(windows)]
mod gozlerd {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use gozler_client::{CancelToken, Client};
    use gozler_core::proto::{PIPE_NAME, Returns, SearchRequest};
    use gozler_core::query::ParsedQuery;

    use super::{Index, scope};
    use crate::identity::Identity;
    use crate::listing::is_folder;
    use crate::operation::Total;
    use crate::receive::Selected;

    /// The installed gozlerd.
    pub(crate) struct Gozlerd;

    impl Index for Gozlerd {
        fn below(&self, folders: &[&Identity], cancel: &Arc<AtomicBool>) -> Option<Vec<Total>> {
            if folders.is_empty() {
                return Some(Vec::new());
            }
            let cancel = CancelToken::from(Arc::clone(cancel));
            let mut client = Client::connect(PIPE_NAME, true, cancel).ok()?;
            let status = client.status().ok()?;
            (folders.iter())
                .map(|folder| {
                    let request = SearchRequest {
                        query: ParsedQuery {
                            clauses: Vec::new(),
                            match_case: false,
                        },
                        scope: Some(scope(&status, folder)?),
                        returns: Returns::Totals,
                    };
                    let totals = client.totals(request).ok()?;
                    let exact = !totals.volumes_incomplete && !totals.metadata_pending;
                    exact.then_some(Total {
                        items: totals.count,
                        bytes: totals.size,
                    })
                })
                .collect()
        }
    }

    /// Whether the walks enter the entry, so gozlerd counts what is below it.
    fn entered(entry: &Selected) -> bool {
        is_folder(entry.tag.attributes, entry.tag.reparse_tag)
    }

    /// Each entry's total: the entry, and everything below a folder that
    /// the walks enter. `None` when `index` cannot answer for every folder.
    pub(crate) fn totals(
        entries: &[&Selected],
        index: &dyn Index,
        cancel: &Arc<AtomicBool>,
    ) -> Option<Vec<Total>> {
        let folders: Vec<&Identity> = (entries.iter())
            .filter(|entry| entered(entry))
            .map(|entry| &entry.identity)
            .collect();
        let mut below = index.below(&folders, cancel)?.into_iter();
        let totals = entries.iter().map(|&entry| {
            if entered(entry) {
                let below = below.next().expect("one total for each folder");
                Total {
                    items: 1 + below.items,
                    bytes: below.bytes,
                }
            } else {
                // A link is copied or deleted as a link, without its target's bytes.
                let bytes = if entry.folder() { 0 } else { entry.stat.size };
                Total { items: 1, bytes }
            }
        });
        Some(totals.collect())
    }

    /// A delete's total, which gozlerd gives while the delete already runs,
    /// because a total that is only shown never delays the work.
    pub(crate) struct Counting {
        folders: Vec<Identity>,
        /// Each selected entry, and each one that no longer exists, which
        /// the run counts as deleted.
        selected: u64,
    }

    impl Counting {
        pub(crate) fn of(entries: &[Selected], missing: u64) -> Counting {
            Counting {
                folders: (entries.iter())
                    .filter(|entry| entered(entry))
                    .map(|entry| entry.identity.clone())
                    .collect(),
                selected: entries.len() as u64 + missing,
            }
        }

        /// The entries the delete covers. A delete copies no bytes, so its
        /// progress counts entries.
        pub(crate) fn total(&self, index: &dyn Index, cancel: &Arc<AtomicBool>) -> Option<Total> {
            let folders: Vec<&Identity> = self.folders.iter().collect();
            let below = index.below(&folders, cancel)?;
            Some(Total {
                items: self.selected + below.iter().map(|total| total.items).sum::<u64>(),
                bytes: 0,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use gozler_core::include::IncludeSettings;
    use gozler_core::proto::{DaemonStatus, VolumeStatus};
    use gozler_core::types::VolumePhase;

    use super::scope;
    use crate::identity::Identity;

    fn identity(path: &str) -> Identity {
        Identity::new(path.encode_utf16().collect())
    }

    fn volume(guid: &str, mounts: &[&str]) -> VolumeStatus {
        VolumeStatus {
            guid: guid.to_string(),
            mounts: mounts.iter().map(ToString::to_string).collect(),
            phase: VolumePhase::Live,
            entries: 0,
            generation: 0,
            metadata_pending: false,
            journal_max_bytes: None,
            placeholders_created: 0,
            delete_of_unknown: 0,
            stale_slots: 0,
            link_reconciles_dropped: 0,
            memory: None,
        }
    }

    fn status(include: Option<IncludeSettings>) -> DaemonStatus {
        DaemonStatus {
            volumes: vec![
                volume(r"\\?\Volume{c}\", &[r"C:\"]),
                volume(r"\\?\Volume{d}\", &[]),
            ],
            volumes_incomplete: false,
            process_working_set: 0,
            process_private_bytes: 0,
            include,
            left_out: Vec::new(),
        }
    }

    /// The scope names a folder by its volume's mount, or by the GUID path
    /// of a volume without one, because gozlerd routes a scope by those
    /// prefixes; a wrong prefix finds no folder, and the total reads 0.
    #[test]
    fn a_folder_is_named_by_its_volume_mount() {
        let status = status(None);
        let scope = |path| scope(&status, &identity(path)).unwrap_or_default();
        assert_eq!(scope(r"\\?\Volume{c}\Users\ana"), r"C:\Users\ana");
        assert_eq!(scope(r"\\?\Volume{c}\"), r"C:\");
        assert_eq!(scope(r"\\?\Volume{d}\x"), r"\\?\Volume{d}\x");
        assert_eq!(scope(r"\\?\Volume{e}\x"), "");
    }

    /// Include settings that leave any entry out of a volume give no total
    /// there, because the index would count less than the folder holds.
    #[test]
    fn include_settings_that_leave_entries_out_give_no_total() {
        let users = identity(r"\\?\Volume{c}\Users");
        let hidden_left_out = IncludeSettings {
            exclude_hidden: true,
            ..IncludeSettings::default()
        };
        assert_eq!(scope(&status(Some(hidden_left_out)), &users), None);
        let every_entry = status(Some(IncludeSettings::default()));
        assert_eq!(scope(&every_entry, &users).as_deref(), Some(r"C:\Users"));
    }
}
