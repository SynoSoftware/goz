//! Preparation applies "What an operation receives", takes copy/move totals
//! from gozlerd without walking to count (D5), checks free space, and splits
//! transfers by source and destination volume with
//! their physical devices. Copy and move keep no file list: the run walks
//! the disk. Synchronization opens its roots here and compares after admission.

use std::collections::{BTreeMap, HashMap};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gozler_winfs::{Access, Reparse, cluster_size, free_space, open_entry, windows_build};

use crate::conflicts::Rule;
use crate::copy::{Source, TargetVolume};
use crate::identity::Identity;
use crate::listing::BACKSLASH;
use crate::names::fold;
use crate::operation::{
    Destination as Asked, End, Initialization, Listed, Pause, Prepared, Refusal, Request, Total,
};
use crate::place::Target;
use crate::receive::{Destination, Selected, receive, verbatim};
use crate::ring_plan::Layout;
use crate::settings::SpeedSettings;
use crate::survey::{Surveying, survey};
use crate::totals::Index;
use crate::volumes::{CopyPlan, read_volume};
use crate::waits::Plan;
use crate::win32::FILE_ATTRIBUTE_DIRECTORY;
use crate::win32::{
    COPY_FILE_22H2_BUILD, FILE_SUPPORTS_BLOCK_REFCOUNTING, FILE_SUPPORTS_SPARSE_FILES,
};

/// A prepared operation.
pub(crate) struct Ready {
    pub(crate) prepared: Prepared,
    pub(crate) work: Vec<(Plan, Work)>,
    /// Selected entries whose identity cannot be read.
    pub(crate) failures: Vec<Listed>,
    /// The request with prepare's decision for a [`Asked::Typed`]
    /// destination, which the queue keeps and saves instead of the asked
    /// one, so Continue after a crash writes where the run wrote.
    pub(crate) resolved: Option<Request>,
}

pub(crate) enum Work {
    Compare(Box<crate::sync::windows::Roots>),
    Synchronize(Box<crate::sync::windows::Review>),
    Delete {
        entries: Vec<Selected>,
        /// Selected entries that no longer exist, which count as deleted.
        missing: u64,
    },
    Transfer(Box<Transfer>),
    Rename {
        /// As the window gave it, for the result.
        path: Vec<u16>,
        from: Vec<u16>,
        to: Vec<u16>,
        name: Vec<u16>,
    },
    NewFolder {
        path: Vec<u16>,
        verbatim: Vec<u16>,
    },
}

/// A copy or a move.
pub(crate) struct Transfer {
    pub(crate) destination: Arc<TransferDestination>,
    /// The new name of the one entry of a [`Asked::Named`] destination.
    pub(crate) name: Option<Vec<u16>>,
    /// Selected entries that no longer exist, as the window gave them.
    pub(crate) missing: Vec<Vec<u16>>,
    /// A move's entries on the destination's volume, which are renamed.
    pub(crate) renamed: Vec<Source>,
    /// The entries copied; for a move, each is deleted after its copy.
    pub(crate) copied: Vec<Source>,
    pub(crate) moving: bool,
    pub(crate) plan: CopyPlan,
    pub(crate) target: TargetVolume,
}

pub(crate) struct TransferDestination {
    pub(crate) received: Destination,
    pub(crate) needed: Option<u64>,
    pub(crate) ready: Initialization<Result<Arc<Target>, End>>,
}

pub(crate) struct Preparation<'a> {
    pub(crate) settings: &'a SpeedSettings,
    pub(crate) rule: Rule,
    pub(crate) index: &'a dyn Index,
    pub(crate) cancel: &'a Arc<AtomicBool>,
    pub(crate) pause: Option<&'a Pause>,
}

pub(crate) fn prepare(request: &Request, preparing: &Preparation<'_>) -> Result<Ready, Refusal> {
    let Preparation { cancel, pause, .. } = *preparing;
    match request {
        Request::Synchronize {
            source,
            destination,
            mirror,
        } => {
            let (plan, roots) = crate::sync::windows::Roots::open(source, destination, *mirror)?;
            Ok(one(plan, Work::Compare(Box::new(roots))))
        }
        Request::Delete { sources } => {
            let (received, _) = receive(sources, None, cancel, pause)?;
            Ok(Ready {
                prepared: Prepared {
                    entries: received.entries.len(),
                    // The queue asks gozlerd once the delete runs (`Counting`).
                    total: None,
                },
                failures: received.failures,
                work: vec![(
                    Plan::default(),
                    Work::Delete {
                        entries: received.entries,
                        missing: received.missing.len() as u64,
                    },
                )],
                resolved: None,
            })
        }
        Request::Copy {
            sources,
            destination,
            policy,
        }
        | Request::Move {
            sources,
            destination,
            policy,
        } => {
            let moving = matches!(request, Request::Move { .. });
            let decided = match destination {
                Asked::Typed(path) => Some(decide_typed(sources, path)),
                Asked::Folder(_) | Asked::Named { .. } => None,
            };
            let destination = decided.as_ref().unwrap_or(destination);
            let renamed = moving.then(|| rename_in_place(sources, destination));
            let mut ready = match renamed.flatten() {
                Some(rename) => rename?,
                None => transfer(sources, destination, moving, preparing)?,
            };
            ready.resolved = decided.map(|destination| {
                let sources = sources.clone();
                if moving {
                    Request::Move {
                        policy: *policy,
                        sources,
                        destination,
                    }
                } else {
                    Request::Copy {
                        policy: *policy,
                        sources,
                        destination,
                    }
                }
            });
            Ok(ready)
        }
        Request::NewFolder { path } => {
            let path: Vec<u16> = path.as_os_str().encode_wide().collect();
            let verbatim = verbatim(&path);
            Ok(one(Plan::default(), Work::NewFolder { path, verbatim }))
        }
    }
}

pub(crate) fn synchronize(
    request: &Request,
    destination: &Asked,
    preparing: &Preparation<'_>,
) -> Result<Ready, Refusal> {
    let Request::Synchronize { source, mirror, .. } = request else {
        unreachable!("only explicit synchronization carries a destination choice");
    };
    let sources = std::slice::from_ref(source);
    let decided = match destination {
        Asked::Typed(path) => decide_typed(sources, path),
        destination => destination.clone(),
    };
    let target = match decided {
        Asked::Folder(folder) => folder.join(source.file_name().ok_or(Refusal::SourceUnavailable)?),
        Asked::Named { folder, name } => folder.join(name),
        Asked::Typed(_) => unreachable!("typed destinations were decided above"),
    };
    // Validate identities before creating a missing destination, so an
    // overlapping root cannot create folders inside its source.
    let (received, _) = receive(sources, Some(&target), preparing.cancel, preparing.pause)?;
    if received.entries.len() != 1 || !received.entries[0].folder() {
        return Err(Refusal::SourceUnavailable);
    }
    std::fs::create_dir_all(&target).map_err(|error| {
        Refusal::DestinationUnreadable(error.raw_os_error().unwrap_or(5) as u32)
    })?;
    let resolved = Request::Synchronize {
        source: source.clone(),
        destination: target,
        mirror: *mirror,
    };
    let mut ready = prepare(&resolved, preparing)?;
    ready.resolved = Some(resolved);
    Ok(ready)
}

fn one(plan: Plan, work: Work) -> Ready {
    Ready {
        prepared: Prepared {
            entries: 1,
            total: None,
        },
        work: vec![(plan, work)],
        failures: Vec::new(),
        resolved: None,
    }
}

/// Renames the entry `path`, as the window gave it, to `name` in its
/// folder.
fn rename(path: Vec<u16>, name: Vec<u16>) -> Result<Ready, Refusal> {
    let from = verbatim(&path);
    let folder = from.iter().rposition(|&unit| unit == BACKSLASH);
    let to = folder.map_or_else(Vec::new, |at| {
        let mut to = from[..=at].to_vec();
        to.extend_from_slice(&name);
        to
    });
    let identity = open_entry(&from, Access::Inspect, Reparse::Open)
        .and_then(|entry| entry.final_path())
        .map(Identity::new)
        .map_err(|_| Refusal::SourceUnavailable)?;
    Ok(one(
        Plan::new(read_volume(identity.volume()).disks),
        Work::Rename {
            path,
            from,
            to,
            name,
        },
    ))
}

/// Decides a typed destination for the one entry `sources` holds: the
/// folder `path` when it is a folder and not the entry itself, otherwise
/// the entry's new full path. The attribute decides, because a folder's
/// open also opens a file.
fn decide_typed(sources: &[PathBuf], path: &Path) -> Asked {
    let identity = |path: &Path, reparse| {
        let units: Vec<u16> = path.as_os_str().encode_wide().collect();
        open_entry(&verbatim(&units), Access::Inspect, reparse)
            .and_then(|entry| Ok((entry.attribute_tag()?, entry.final_path()?)))
    };
    let entry = sources
        .first()
        .and_then(|source| identity(source, Reparse::Open).ok());
    if let Ok((tag, typed)) = identity(path, Reparse::Follow)
        && tag.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
        && entry.is_none_or(|(_, entry)| Identity::new(entry) != Identity::new(typed))
    {
        return Asked::Folder(path.to_path_buf());
    }
    match (path.parent(), path.file_name()) {
        (Some(folder), Some(name)) => Asked::Named {
            folder: folder.to_path_buf(),
            name: name.to_owned(),
        },
        // A root has no name to write the entry under.
        _ => Asked::Folder(path.to_path_buf()),
    }
}

/// A move of one entry to a new name in its own folder that differs from
/// its name only in case, which is a rename: as a move, the new name would
/// meet the entry itself in its folder's names.
fn rename_in_place(sources: &[PathBuf], destination: &Asked) -> Option<Result<Ready, Refusal>> {
    let (Asked::Named { folder, name }, [source]) = (destination, sources) else {
        return None;
    };
    let own = source.file_name()?;
    let name: Vec<u16> = name.encode_wide().collect();
    let own: Vec<u16> = own.encode_wide().collect();
    if name == own || fold(&name) != fold(&own) {
        return None;
    }
    let same_folder = |path: &Path| {
        let units: Vec<u16> = path.as_os_str().encode_wide().collect();
        open_entry(&verbatim(&units), Access::Inspect, Reparse::Follow)
            .and_then(|entry| entry.final_path())
            .map(Identity::new)
    };
    let parent = same_folder(source.parent()?).ok()?;
    if same_folder(folder).ok()? != parent {
        return None;
    }
    let path: Vec<u16> = source.as_os_str().encode_wide().collect();
    Some(rename(path, name))
}

fn transfer(
    sources: &[PathBuf],
    asked: &Asked,
    moving: bool,
    preparing: &Preparation<'_>,
) -> Result<Ready, Refusal> {
    let Preparation {
        settings,
        rule,
        index,
        cancel,
        pause,
    } = *preparing;
    let (received, destination) = receive(sources, Some(asked.folder()), cancel, pause)?;
    let destination = destination.expect("a destination was given");
    let new_name: Option<Vec<u16>> = match asked {
        Asked::Named { name, .. } => Some(name.encode_wide().collect()),
        Asked::Folder(_) | Asked::Typed(_) => None,
    };
    let name_of = |selected: &Selected| {
        (new_name.clone()).unwrap_or_else(|| selected.identity.name().to_vec())
    };
    let mut renamed = Vec::new();
    let mut copied = Vec::new();
    let mut named: HashMap<Vec<u16>, usize> = HashMap::new();
    for selected in &received.entries {
        *named.entry(fold(&name_of(selected))).or_default() += 1;
    }
    for selected in received.entries {
        if let Some(pause) = pause {
            pause.wait(cancel);
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let name = name_of(&selected);
        // Left alone only under its own name; a new name in its own folder
        // is a copy or a rename beside it.
        let own_folder = selected.identity.parent().as_ref() == Some(&destination.identity)
            && fold(&name) == fold(selected.identity.name());
        let same_volume = selected.identity.volume() == destination.identity.volume();
        let source = Source {
            shared: named[&fold(&name)] > 1,
            selected,
            name,
            own_folder,
        };
        if moving && same_volume {
            renamed.push(source);
        } else {
            copied.push(source);
        }
    }

    let mut volumes = BTreeMap::new();
    for volume in copied
        .iter()
        .chain(&renamed)
        .map(|source| source.selected.identity.volume())
        .chain(Some(destination.identity.volume()))
    {
        volumes
            .entry(volume.to_vec())
            .or_insert_with(|| read_volume(volume));
    }
    let target_volume = destination.identity.volume();
    let target_storage = &volumes[target_volume];
    let flags = destination.entry.volume_flags().unwrap_or(0);
    let one_volume =
        (copied.iter()).all(|source| source.selected.identity.volume() == target_volume);
    let clones = one_volume && flags & FILE_SUPPORTS_BLOCK_REFCOUNTING != 0;
    let sparse_files = flags & FILE_SUPPORTS_SPARSE_FILES != 0;
    let sparse = sparse_files && windows_build() >= COPY_FILE_22H2_BUILD;
    let file_system = destination.entry.file_system_name().unwrap_or_default();
    let root: Vec<u16> = target_volume
        .iter()
        .copied()
        .chain(Some(BACKSLASH))
        .collect();
    let target = TargetVolume {
        sparse_copy: sparse,
        sparse_files,
        fat: file_system.to_ascii_uppercase().contains("FAT"),
        // A cluster size that cannot be read leaves the ring's copy.
        clone_cluster: (flags & FILE_SUPPORTS_BLOCK_REFCOUNTING != 0)
            .then(|| cluster_size(&root).ok())
            .flatten(),
    };

    let surveying = Surveying {
        rule,
        sparse_stays: sparse,
        index,
        cancel,
        pause,
    };
    let sources: Vec<(&Source, bool)> = (renamed.iter().map(|source| (source, false)))
        .chain(copied.iter().map(|source| (source, true)))
        .collect();
    let surveyed =
        survey(&sources, &destination, &surveying).map_err(Refusal::DestinationUnreadable)?;
    let needed = surveyed.needed.filter(|_| !clones);
    if let Some(needed) = needed {
        check_space(&destination, needed)?;
    }

    let entries = renamed.len() + copied.len();
    let mut pairs: BTreeMap<Vec<u16>, (Vec<Source>, Vec<Source>)> = BTreeMap::new();
    for source in renamed {
        pairs
            .entry(source.selected.identity.volume().to_vec())
            .or_default()
            .0
            .push(source);
    }
    for source in copied {
        pairs
            .entry(source.selected.identity.volume().to_vec())
            .or_default()
            .1
            .push(source);
    }
    if pairs.is_empty() {
        pairs.insert(target_volume.to_vec(), (Vec::new(), Vec::new()));
    }
    let missing_count = received.missing.len() as u64;
    let mut missing = received.missing;
    let work = pairs
        .into_iter()
        .map(|(volume, (renamed, copied))| {
            let source_storage = &volumes[&volume];
            let layout = if source_storage
                .disks
                .iter()
                .any(|disk| target_storage.disks.contains(disk))
            {
                Layout::OneDisk
            } else {
                Layout::TwoDisks
            };
            let devices = Plan::new(
                source_storage
                    .disks
                    .iter()
                    .chain(&target_storage.disks)
                    .cloned()
                    .collect(),
            );
            let mut target = target;
            if volume != target_volume {
                target.clone_cluster = None;
            }
            (
                devices,
                renamed,
                copied,
                CopyPlan::new(
                    source_storage.class.max(target_storage.class),
                    settings,
                    layout,
                ),
                target,
                std::mem::take(&mut missing),
            )
        })
        .collect::<Vec<_>>();
    let destination = Arc::new(TransferDestination {
        received: destination,
        needed,
        ready: Initialization::default(),
    });
    let work = work
        .into_iter()
        .map(|(devices, renamed, copied, plan, target, missing)| {
            (
                devices,
                Work::Transfer(Box::new(Transfer {
                    destination: Arc::clone(&destination),
                    name: new_name.clone(),
                    missing,
                    renamed,
                    copied,
                    moving,
                    plan,
                    target,
                })),
            )
        })
        .collect();
    Ok(Ready {
        prepared: Prepared {
            entries,
            // The run counts each selected entry that no longer exists as
            // an item done.
            total: surveyed.total.map(|total| Total {
                items: total.items + missing_count,
                ..total
            }),
        },
        failures: received.failures,
        resolved: None,
        work,
    })
}

/// Refuses a copy that needs more bytes than the destination has free, as
/// File Explorer refuses it. A free space that cannot be read refuses
/// nothing; a disk that fills stops the copy.
pub(crate) fn check_space(destination: &Destination, needed: u64) -> Result<(), Refusal> {
    match free_space(&destination.verbatim) {
        Ok(free) if needed > free => Err(Refusal::TooLittleSpace { needed, free }),
        _ => Ok(()),
    }
}
