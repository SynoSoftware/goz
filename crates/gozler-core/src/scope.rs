//! Which volumes a query searches, and the mount prefix each volume's full
//! paths print with.
//!
//! The daemon routes a query's `-path` scope with [`Scope::of`], and the client
//! uses the same rule to know which volumes `-timeout` waits for, so the two
//! cannot disagree about the volume a folder is on.

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

    /// Every prefix a scope can name the volume by: each mount, or the GUID
    /// path when it has no mount.
    fn prefixes(self) -> impl Iterator<Item = &'a str> {
        let guid = self.mounts.is_empty().then_some(self.guid);
        self.mounts.iter().map(String::as_str).chain(guid)
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
}
