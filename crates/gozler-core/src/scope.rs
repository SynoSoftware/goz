//! Which volumes a query searches, and the mount prefix each volume's full
//! paths print with.
//!
//! The daemon routes a query's `-path` scope with [`Scope::of`], and the
//! clients use the same rule, through [`Coverage`], to know which volumes
//! `-timeout` waits for and which volume the window names in its status bar,
//! so they cannot disagree about the volume a folder is on.

use crate::proto::VolumeStatus;

/// Where one volume is mounted: its mount points, such as `C:\` or
/// `C:\mnt\data\`, and its GUID path.
#[derive(Clone, Copy, Debug)]
pub struct VolumeMounts<'a> {
    pub mounts: &'a [String],
    pub guid: &'a str,
}

impl<'a> VolumeMounts<'a> {
    /// The mount prefix the volume's full paths print with: its first mount,
    /// or its GUID path when it has no mount.
    pub fn display_prefix(self) -> &'a str {
        self.mounts.first().map_or(self.guid, String::as_str)
    }

    /// Every prefix a scope or a link target can name the volume by.
    fn prefixes(self) -> impl Iterator<Item = &'a str> {
        self.mounts.iter().map(String::as_str).chain([self.guid])
    }
}

/// The volumes a query searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope<'p> {
    /// No scope: every volume.
    Everywhere,
    /// The scope is on the volume at position `volume` of the volumes given
    /// to [`Scope::of`]. `folder` is the rest of the scope below that
    /// volume's mount, without leading or trailing separators, so it equals
    /// the path the index rebuilds for that folder; it is empty for the
    /// volume's root.
    Folder { volume: usize, folder: &'p str },
    /// The scope is on no indexed volume, so nothing matches.
    Nowhere,
}

impl<'p> Scope<'p> {
    /// Includes the owning volume and volumes whose printed mount lies
    /// below the scope, including a volume with only a folder mount.
    pub fn includes(self, scope: Option<&str>, volume: usize, mounts: VolumeMounts<'_>) -> bool {
        matches!(self, Scope::Everywhere)
            || matches!(self, Scope::Folder { volume: owner, .. } if owner == volume)
            || scope.is_some_and(|scope| mount_len(mounts.display_prefix(), scope).is_some())
    }

    /// Routes `scope`, an absolute folder, to the volume with the longest
    /// mount that starts it. A mount matches ignoring ASCII case and only up
    /// to a separator, so a volume mounted at `C:\mnt\data\` owns
    /// `C:\mnt\data\proj`, and `C:\` owns `C:\mnt\database`.
    pub fn of<'a>(
        scope: Option<&'p str>,
        volumes: impl IntoIterator<Item = VolumeMounts<'a>>,
    ) -> Self {
        let Some(path) = scope else {
            return Scope::Everywhere;
        };
        let mut best: Option<(usize, usize)> = None;
        for (volume, mounts) in volumes.into_iter().enumerate() {
            for len in mounts
                .prefixes()
                .filter_map(|prefix| mount_len(path, prefix))
            {
                if best.is_none_or(|(_, longest)| len > longest) {
                    best = Some((volume, len));
                }
            }
        }
        match best {
            None => Scope::Nowhere,
            Some((volume, len)) => Scope::Folder {
                volume,
                folder: path[len..].trim_matches(['\\', '/']),
            },
        }
    }
}

/// What the index state of the volumes a search reaches means for its
/// answer. `gozler` and the window both read a status reply through it.
#[derive(Clone, Debug)]
pub struct Coverage<'s> {
    volumes: Vec<&'s VolumeStatus>,
    reads_size_or_date: bool,
}

impl<'s> Coverage<'s> {
    /// The volumes of a status reply that a search scoped to `scope`
    /// reaches, routed by [`Scope::of`]. `reads_size_or_date` says whether the
    /// search tests sizes or dates, or totals sizes.
    pub fn of(volumes: &'s [VolumeStatus], scope: Option<&str>, reads_size_or_date: bool) -> Self {
        let routed = Scope::of(scope, volumes.iter().map(VolumeStatus::mounted));
        let volumes = volumes
            .iter()
            .enumerate()
            .filter(|(number, v)| routed.includes(scope, *number, v.mounted()))
            .map(|(_, v)| v)
            .collect();
        Coverage {
            volumes,
            reads_size_or_date,
        }
    }

    /// The volumes the search reaches.
    pub fn volumes(&self) -> impl Iterator<Item = &'s VolumeStatus> + '_ {
        self.volumes.iter().copied()
    }

    /// The first volume the search reaches that is not complete, so matches
    /// on it may be missing.
    pub fn incomplete(&self) -> Option<&'s VolumeStatus> {
        self.volumes().find(|v| !v.phase.is_complete())
    }

    /// The first volume the search reaches whose unknown sizes or dates may
    /// leave matches out of the answer.
    pub fn sizes_or_dates_unknown(&self) -> Option<&'s VolumeStatus> {
        self.volumes()
            .find(|v| leaves_matches_out(v.metadata_pending, self.reads_size_or_date))
    }
}

/// Whether unknown sizes or dates leave matches out of an answer: they do
/// when the search reads them, because a size or date test never matches a
/// file whose value is unknown, and a size total adds nothing for it.
pub fn leaves_matches_out(metadata_pending: bool, reads_size_or_date: bool) -> bool {
    metadata_pending && reads_size_or_date
}

/// The length of `prefix`, without its trailing separators, when it starts
/// `path` and ends there or at a separator of `path`.
fn mount_len(path: &str, prefix: &str) -> Option<usize> {
    let prefix = prefix.trim_end_matches(['\\', '/']);
    let head = path.get(..prefix.len())?;
    let rest = &path[prefix.len()..];
    (head.eq_ignore_ascii_case(prefix) && (rest.is_empty() || rest.starts_with(['\\', '/'])))
        .then_some(prefix.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::VolumePhase;

    fn mounts(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    /// A scope on a folder mount or on a volume's second mount reaches that
    /// volume, not the drive whose mount comes first in the list, whose index
    /// holds only an empty mount folder there.
    #[test]
    fn the_longest_mount_owns_the_scope() {
        let c = mounts(&[r"C:\"]);
        let d = mounts(&[r"D:\", r"C:\mnt\data\"]);
        let volumes = [
            VolumeMounts {
                mounts: &c,
                guid: r"\\?\Volume{0}\",
            },
            VolumeMounts {
                mounts: &d,
                guid: r"\\?\Volume{1}\",
            },
            VolumeMounts {
                mounts: &[],
                guid: r"\\?\Volume{2}\",
            },
        ];
        assert_eq!(Scope::of(None, volumes), Scope::Everywhere);
        let cases = [
            (r"C:\mnt\data\proj\", 1, "proj"),
            (r"c:\MNT\data", 1, ""),
            (r"D:\proj", 1, "proj"),
            (r"C:\mnt\database", 0, r"mnt\database"),
            (r"C:\", 0, ""),
            (r"\\?\Volume{2}\x", 2, "x"),
        ];
        for (scope, volume, folder) in cases {
            assert_eq!(
                Scope::of(Some(scope), volumes),
                Scope::Folder { volume, folder },
                "{scope}"
            );
        }
        assert_eq!(Scope::of(Some(r"E:\x"), volumes), Scope::Nowhere);
    }

    fn volume(mount: &str, phase: VolumePhase, metadata_pending: bool) -> VolumeStatus {
        VolumeStatus {
            guid: String::new(),
            mounts: mounts(&[mount]),
            phase,
            progress: None,
            entries: 0,
            generation: 0,
            metadata_pending,
            journal_max_bytes: None,
            placeholders_created: 0,
            delete_of_unknown: 0,
            stale_slots: 0,
            link_reconciles_dropped: 0,
            unread_folders: 0,
            memory: None,
        }
    }

    /// A search scoped to D:\ that counted C:\'s indexing would wait for it
    /// with `-timeout` and tell the window's reader that its results may be
    /// incomplete; unknown sizes matter only to a search that reads them.
    #[test]
    fn coverage_reads_only_the_volumes_the_search_reaches() {
        let volumes = [
            volume(r"C:\", VolumePhase::Rescanning, false),
            volume(r"D:\", VolumePhase::Live, true),
        ];
        let mount = |v: Option<&VolumeStatus>| v.map(|v| v.mounts[0].clone());
        let on_d = Coverage::of(&volumes, Some(r"D:\work"), true);
        assert_eq!(mount(on_d.incomplete()), None);
        assert_eq!(mount(on_d.sizes_or_dates_unknown()), Some(r"D:\".into()));
        let everywhere = Coverage::of(&volumes, None, false);
        assert_eq!(mount(everywhere.incomplete()), Some(r"C:\".into()));
        assert_eq!(mount(everywhere.sizes_or_dates_unknown()), None);
        let nowhere = Coverage::of(&volumes, Some(r"E:\x"), true);
        assert_eq!(nowhere.volumes().count(), 0);
    }

    #[test]
    fn coverage_through_links_includes_targets_and_descendant_primary_mounts() {
        let links = crate::links::Links::new(vec![crate::links::FolderLink {
            link: r"C:\alias".into(),
            target: r"D:\data".into(),
        }]);
        let volumes = [
            volume(r"C:\", VolumePhase::Live, false),
            volume(r"D:\", VolumePhase::Bootstrapping, false),
            volume(r"C:\repo\data\", VolumePhase::Rescanning, false),
        ];
        let effective = links.search_scope(Some(r"C:\alias"));
        let reached = Coverage::of(&volumes, effective.as_deref(), false);
        assert_eq!(reached.incomplete().unwrap().mounts, [r"D:\"]);
        let reached = Coverage::of(&volumes, Some(r"C:\repo"), false);
        assert_eq!(reached.incomplete().unwrap().mounts, [r"C:\repo\data\"]);
        assert_eq!(reached.volumes().count(), 2);
        assert_eq!(
            Coverage::of(&volumes[2..], Some(r"C:\repo"), false)
                .volumes()
                .count(),
            1
        );
    }
}
