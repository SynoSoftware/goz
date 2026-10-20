//! Prepare: what an operation reads before the person answers. It applies
//! "What an operation receives", takes a copy's or a move's totals from
//! gozlerd and never walks to count (D5), counts its conflicts, checks the
//! free space, and records the paths and disks that the waits compare
//! (D14). It keeps no list of files (D11): the run walks the disk.

use std::collections::{BTreeMap, HashMap};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gozler_winfs::{Access, Reparse, cluster_size, free_space, open_entry, windows_build};

use crate::conflicts::Rule;
use crate::copy::{Source, TargetVolume};
use crate::identity::Identity;
use crate::listing::BACKSLASH;
use crate::names::fold;
use crate::operation::{Listed, Prepared, Refusal, Request, Total};
use crate::receive::{Destination, Selected, receive, verbatim};
use crate::ring_plan::Layout;
use crate::settings::SpeedSettings;
use crate::survey::{Surveying, survey};
use crate::totals::Index;
use crate::volumes::{CopyPlan, VolumeClass, read_volume};
use crate::waits::Plan;
use crate::win32::{
    COPY_FILE_22H2_BUILD, FILE_SUPPORTS_BLOCK_REFCOUNTING, FILE_SUPPORTS_SPARSE_FILES,
};

/// A prepared operation.
pub(crate) struct Ready {
    pub(crate) prepared: Prepared,
    pub(crate) plan: Plan,
    pub(crate) work: Work,
    /// Selected entries whose identity cannot be read.
    pub(crate) failures: Vec<Listed>,
}

pub(crate) enum Work {
    Delete {
        entries: Vec<Selected>,
        /// Selected entries that no longer exist, which count as deleted.
        missing: u64,
    },
    Transfer(Transfer),
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
    pub(crate) destination: Destination,
    /// Selected entries that no longer exist, as the window gave them.
    pub(crate) missing: Vec<Vec<u16>>,
    /// A move's entries on the destination's volume, which are renamed.
    pub(crate) renamed: Vec<Source>,
    /// The entries copied; for a move, each is deleted after its copy.
    pub(crate) copied: Vec<Source>,
    pub(crate) moving: bool,
    pub(crate) plan: CopyPlan,
    pub(crate) target: TargetVolume,
    /// The bytes the destination needs; `None` when prepare cannot know
    /// them, and within one volume that can clone blocks, where both
    /// engines clone (D20).
    pub(crate) needed: Option<u64>,
}

/// Prepares `request` for a run under `rule`: for Continue, the choice and
/// the start of the interrupted run. `index` gives the totals.
pub(crate) fn prepare(
    request: &Request,
    settings: &SpeedSettings,
    rule: Rule,
    index: &dyn Index,
    cancel: &Arc<AtomicBool>,
) -> Result<Ready, Refusal> {
    match request {
        Request::Delete { sources } => {
            let (received, _) = receive(sources, None, cancel)?;
            let mut plan = Plan::default();
            for entry in &received.entries {
                plan.writes.insert(&entry.identity);
            }
            Ok(Ready {
                prepared: Prepared {
                    entries: received.entries.len(),
                    // The queue asks gozlerd once the delete runs (`Counting`).
                    total: None,
                    conflicts: None,
                },
                plan,
                failures: received.failures,
                work: Work::Delete {
                    entries: received.entries,
                    missing: received.missing.len() as u64,
                },
            })
        }
        Request::Copy {
            sources,
            destination,
        } => transfer(sources, destination, false, settings, rule, index, cancel),
        Request::Move {
            sources,
            destination,
        } => transfer(sources, destination, true, settings, rule, index, cancel),
        Request::Rename { path, name } => {
            let path: Vec<u16> = path.as_os_str().encode_wide().collect();
            let name: Vec<u16> = name.encode_wide().collect();
            let from = verbatim(&path);
            let folder = from.iter().rposition(|&unit| unit == BACKSLASH);
            let to = folder.map_or_else(Vec::new, |at| {
                let mut to = from[..=at].to_vec();
                to.extend_from_slice(&name);
                to
            });
            // A rename reads and writes its old and new names.
            let mut plan = Plan::default();
            let identity = open_entry(&from, Access::Inspect, Reparse::Open)
                .and_then(|entry| entry.final_path())
                .map(Identity::new);
            if let Ok(identity) = identity {
                if let Some(parent) = identity.parent() {
                    plan.writes.insert(&parent.child(&name));
                }
                plan.writes.insert(&identity);
            }
            Ok(one(
                plan,
                Work::Rename {
                    path,
                    from,
                    to,
                    name,
                },
            ))
        }
        Request::NewFolder { path } => {
            let path: Vec<u16> = path.as_os_str().encode_wide().collect();
            let verbatim = verbatim(&path);
            let mut plan = Plan::default();
            if let Some(at) = verbatim.iter().rposition(|&unit| unit == BACKSLASH) {
                let parent = open_entry(&verbatim[..at], Access::Inspect, Reparse::Follow)
                    .and_then(|entry| entry.final_path())
                    .map(Identity::new);
                if let Ok(parent) = parent {
                    plan.writes.insert(&parent.child(&verbatim[at + 1..]));
                }
            }
            Ok(one(plan, Work::NewFolder { path, verbatim }))
        }
    }
}

fn one(plan: Plan, work: Work) -> Ready {
    Ready {
        prepared: Prepared {
            entries: 1,
            total: None,
            conflicts: None,
        },
        plan,
        work,
        failures: Vec::new(),
    }
}

fn transfer(
    sources: &[std::path::PathBuf],
    destination: &Path,
    moving: bool,
    settings: &SpeedSettings,
    rule: Rule,
    index: &dyn Index,
    cancel: &Arc<AtomicBool>,
) -> Result<Ready, Refusal> {
    let (received, destination) = receive(sources, Some(destination), cancel)?;
    let destination = destination.expect("a destination was given");
    let mut plan = Plan::default();
    let mut renamed = Vec::new();
    let mut copied = Vec::new();
    let mut named: HashMap<Vec<u16>, usize> = HashMap::new();
    for selected in &received.entries {
        *named.entry(fold(selected.identity.name())).or_default() += 1;
    }
    for selected in received.entries {
        let name = selected.identity.name().to_vec();
        plan.writes
            .insert_named(&destination.identity, &name, selected.folder());
        if moving {
            plan.writes.insert(&selected.identity);
        } else {
            plan.reads.insert(&selected.identity);
        }
        let own_folder = selected.identity.parent().as_ref() == Some(&destination.identity);
        let same_volume = selected.identity.volume() == destination.identity.volume();
        let source = Source {
            selected,
            own_folder,
            shared: named[&fold(&name)] > 1,
        };
        if moving && same_volume {
            renamed.push(source);
        } else {
            copied.push(source);
        }
    }

    // The volumes it moves bytes on decide the method and the disk waits.
    let mut volumes = BTreeMap::new();
    if !copied.is_empty() {
        for volume in (copied
            .iter()
            .map(|source| source.selected.identity.volume()))
        .chain(Some(destination.identity.volume()))
        {
            volumes
                .entry(volume.to_vec())
                .or_insert_with(|| read_volume(volume));
        }
    }
    let pair = (volumes.values().map(|volume| volume.class))
        .max()
        .unwrap_or(VolumeClass::Ssd);
    plan.moves_bytes = !copied.is_empty();
    for volume in volumes.values() {
        for disk in &volume.disks {
            plan.disks.push((disk.clone(), volume.class.seeks()));
        }
    }
    let target_volume = destination.identity.volume();
    let target_disks = volumes.get(target_volume).map(|volume| &volume.disks);
    let one_disk = (copied.iter())
        .filter_map(|source| volumes.get(source.selected.identity.volume()))
        .any(|volume| {
            (volume.disks.iter()).any(|disk| target_disks.is_some_and(|disks| disks.contains(disk)))
        });
    let layout = if one_disk {
        Layout::OneDisk
    } else {
        Layout::TwoDisks
    };
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
        clone_cluster: clones.then(|| cluster_size(&root).ok()).flatten(),
    };

    let surveying = Surveying {
        rule,
        sparse_stays: sparse,
        index,
        cancel,
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

    Ok(Ready {
        prepared: Prepared {
            entries: renamed.len() + copied.len(),
            // The run counts each selected entry that no longer exists as
            // an item done.
            total: surveyed.total.map(|total| Total {
                items: total.items + received.missing.len() as u64,
                ..total
            }),
            conflicts: Some(surveyed.conflicts),
        },
        plan,
        failures: received.failures,
        work: Work::Transfer(Transfer {
            destination,
            missing: received.missing,
            renamed,
            copied,
            moving,
            plan: CopyPlan::new(pair, settings, layout),
            target,
            needed,
        }),
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
