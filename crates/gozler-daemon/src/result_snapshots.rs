//! The result snapshots gozlerd keeps between requests (DESIGN.md: Result
//! snapshots).
//!
//! A connection serves one request and closes, so a result snapshot lives
//! in one daemon-wide table, behind one mutex that is never held while a
//! volume's lock is taken. Whoever holds an id reads its rows. The table
//! drops a result snapshot nobody read for [`EXPIRY`], keeps at most
//! [`PER_USER`] for each user, and keeps no more rows in all than its cap,
//! twice the entries of every index; each rule drops the one read least
//! recently. A dropped result snapshot costs only a new search.

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use gozler_core::proto::ResultSnapshotId;
use gozler_core::query::ResultSnapshot;

/// A result snapshot nobody read for this long is dropped, whenever the
/// table is used: a person who looks away for a few minutes keeps
/// scrolling without a new search, and no timer is needed.
const EXPIRY: Duration = Duration::from_secs(10 * 60);

/// The result snapshots one user keeps: the window's and one other
/// client's.
const PER_USER: usize = 2;

/// The result snapshots kept, each with the volumes it reads as `W`, which
/// gozlerd holds weakly, so that a removed volume's index is freed when its
/// last owner drops it.
pub(crate) struct ResultSnapshots<W> {
    kept: Mutex<Vec<Kept<W>>>,
}

/// A result snapshot the table keeps.
pub(crate) struct Kept<W> {
    pub(crate) id: ResultSnapshotId,
    /// The user SID of the client that made it, as bytes; empty when it
    /// could not be read.
    pub(crate) user: Vec<u8>,
    /// When it was made or last read.
    pub(crate) last_read: Instant,
    pub(crate) snapshot: Arc<ResultSnapshot>,
    pub(crate) volumes: Vec<W>,
}

impl<W: Clone> ResultSnapshots<W> {
    pub(crate) fn new() -> Self {
        Self {
            kept: Mutex::new(Vec::new()),
        }
    }

    /// Keeps `new`, made at its `last_read`, after it drops `replaces`, the
    /// client's earlier one, and whatever the rules drop to make room. `cap`
    /// is the most rows all result snapshots hold.
    pub(crate) fn keep(&self, new: Kept<W>, replaces: Option<ResultSnapshotId>, cap: u64) {
        let mut kept = self.kept.lock().expect("no table user panics");
        expire(&mut kept, new.last_read);
        kept.retain(|k| Some(k.id) != replaces);
        while kept.iter().filter(|k| k.user == new.user).count() >= PER_USER {
            drop_least_recent(&mut kept, |k| k.user == new.user);
        }
        let rows = |kept: &[Kept<W>]| kept.iter().map(|k| k.snapshot.total()).sum::<u64>();
        while !kept.is_empty() && rows(&kept) + new.snapshot.total() > cap {
            drop_least_recent(&mut kept, |_| true);
        }
        kept.push(new);
    }

    /// The result snapshot `id` and its volumes, which it marks as read
    /// `now`; `None` once it was dropped.
    pub(crate) fn get(
        &self,
        id: ResultSnapshotId,
        now: Instant,
    ) -> Option<(Arc<ResultSnapshot>, Vec<W>)> {
        let mut kept = self.kept.lock().expect("no table user panics");
        expire(&mut kept, now);
        let found = kept.iter_mut().find(|k| k.id == id)?;
        found.last_read = now;
        Some((found.snapshot.clone(), found.volumes.clone()))
    }
}

/// Drops each result snapshot nobody read for [`EXPIRY`].
fn expire<W>(kept: &mut Vec<Kept<W>>, now: Instant) {
    kept.retain(|k| now.saturating_duration_since(k.last_read) < EXPIRY);
}

/// Drops the result snapshot read least recently of those `of` picks.
fn drop_least_recent<W>(kept: &mut Vec<Kept<W>>, of: impl Fn(&Kept<W>) -> bool) {
    if let Some(at) = (0..kept.len())
        .filter(|&at| of(&kept[at]))
        .min_by_key(|&at| kept[at].last_read)
    {
        kept.remove(at);
    }
}

/// The volumes a result snapshot reads, `kept` weakly, as the strong ones
/// of `current`, the volumes a request works on; `None` when one is gone.
/// A volume that the upgrade finds but `current` does not hold is gone too:
/// a tail that the supervisor abandoned still owns its removed volume, and
/// a volume attached again is a new one.
pub(crate) fn live_volumes<T>(kept: &[Weak<T>], current: &[Arc<T>]) -> Option<Vec<Arc<T>>> {
    kept.iter()
        .map(|weak| {
            let volume = weak.upgrade()?;
            current
                .iter()
                .any(|c| Arc::ptr_eq(c, &volume))
                .then_some(volume)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gozler_core::index::{NTFS_ROOT_FRN, VolumeIndex};
    use gozler_core::query::{
        CompiledQuery, HeldVolume, IndexedVolume, ResultRows, RowsSpec, Viewer, parse_query,
    };
    use gozler_core::scope::VolumeMounts;
    use gozler_core::types::{Frn, SortKey, SortSpec};
    use gozler_core::usn::record::{ParsedUsnRecord, USN_REASON_FILE_CREATE};

    /// A volume of `files` files at its root.
    struct Files {
        index: VolumeIndex,
        mounts: [String; 1],
    }

    impl IndexedVolume for Files {
        type Index<'a> = &'a VolumeIndex;

        fn read(&self) -> HeldVolume<&VolumeIndex> {
            HeldVolume {
                index: &self.index,
                complete: true,
                metadata_pending: false,
            }
        }

        fn mounts(&self) -> VolumeMounts<'_> {
            VolumeMounts {
                mounts: &self.mounts,
                guid: "",
            }
        }
    }

    /// A result snapshot of `rows` rows.
    fn snapshot(rows: u128) -> Arc<ResultSnapshot> {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
        for n in 0..rows {
            index.insert_enum(&ParsedUsnRecord {
                frn: Frn(100 + n),
                parent_frn: NTFS_ROOT_FRN,
                reason: USN_REASON_FILE_CREATE,
                attributes: 0,
                name: format!("{n}.txt").into_bytes(),
            });
        }
        let volume = Arc::new(Files {
            index,
            mounts: [r"C:\".to_string()],
        });
        let query = CompiledQuery::new(&parse_query("txt").expect("parses")).expect("compiles");
        let spec = RowsSpec {
            sort: SortSpec::default_for(SortKey::Name),
            offset: 0,
            limit: None,
            sums: false,
        };
        let found = ResultRows::search(vec![volume], &query, None, spec, Viewer::Everything)
            .expect("no scope");
        let snapshot = found.into_snapshot().expect("one volume").0;
        assert_eq!(snapshot.total(), rows as u64);
        Arc::new(snapshot)
    }

    fn kept(n: u128, user: &[u8], rows: u128, now: Instant) -> Kept<()> {
        Kept {
            id: ResultSnapshotId(n),
            user: user.to_vec(),
            last_read: now,
            snapshot: snapshot(rows),
            volumes: Vec::new(),
        }
    }

    /// Each rule drops the result snapshot read least recently, so a window
    /// scrolling its list keeps its own while another client's search makes
    /// room: a third one of a user drops that user's older one, `replaces`
    /// drops the one named, the rows of all of them stay under the cap, and
    /// nobody reads one past the expiry.
    #[test]
    fn the_table_drops_the_least_recently_read_and_the_expired() {
        let table = ResultSnapshots::<()>::new();
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let has = |n: u128, secs: u64| table.get(ResultSnapshotId(n), at(secs)).is_some();
        table.keep(kept(1, b"ana", 1, at(0)), None, 10);
        table.keep(kept(2, b"ana", 1, at(1)), None, 10);
        assert!(has(1, 2), "read again, so 2 is older");
        table.keep(kept(3, b"ana", 1, at(3)), None, 10);
        assert!(!has(2, 4), "ana's least recently read");
        assert!(has(1, 4));
        table.keep(kept(4, b"bo", 1, at(5)), None, 10);
        assert!(has(3, 6), "another user's search drops none");
        table.keep(kept(5, b"ana", 1, at(7)), Some(ResultSnapshotId(3)), 10);
        assert!(!has(3, 8), "the one replaced");
        assert!(has(1, 8));
        // 1, 4 and 5 hold 3 rows; 8 more pass the cap of 10.
        table.keep(kept(6, b"cy", 8, at(9)), None, 10);
        assert!(!has(4, 10), "read least recently of all");
        assert!(has(6, 10) && has(5, 10) && has(1, 10));
        let later = 10 + EXPIRY.as_secs();
        assert!(!has(1, later), "nobody read it for the expiry");
    }

    /// A result snapshot whose volume was removed reads as gone, also while
    /// something else still owns the volume.
    #[test]
    fn a_removed_volume_is_gone_though_still_owned() {
        let (c, d) = (Arc::new(()), Arc::new(()));
        let kept = [Arc::downgrade(&c), Arc::downgrade(&d)];
        assert!(live_volumes(&kept, &[c.clone(), d.clone()]).is_some());
        assert!(
            live_volumes(&kept, std::slice::from_ref(&c)).is_none(),
            "d was removed"
        );
        let attached_again = Arc::new(());
        assert!(live_volumes(&kept, &[c, attached_again]).is_none());
    }
}
