//! Stage 0 item 1 of the visibility plan (`docs/plans/visibility-plan.md`):
//! on one NTFS volume, how many folders, folder `SecurityId`s and distinct
//! folder descriptors there are, what reading the descriptors costs with one
//! read per `SecurityId` and with one read per folder, whether a record's USN
//! matches the layout's, and which folders' reads fail, with their errors.
//!
//! Usage, elevated: `visibility_stage0 C:`. The owner runs it through
//! `target\probe\visibility-stage0.ps1` after a reboot, so the reads meet a
//! cache that only the boot has warmed. The reads run in the order a build
//! would make them: the layout pass, one read per `SecurityId`, then every
//! folder, which the earlier reads warm only slightly.
//!
//! Windows only; on other targets it is an empty stub, so
//! `cargo build --all-targets` stays green everywhere.

#[cfg(not(windows))]
fn main() {
    eprintln!("visibility_stage0 reads an NTFS volume and runs on Windows only.");
}

#[cfg(windows)]
use anyhow::{Context, Result, bail};
#[cfg(windows)]
use gozler_core::layout::{RECOMMENDED_LAYOUT_FLAGS, walk_layout_buffer};
#[cfg(windows)]
use gozler_core::types::Frn;
#[cfg(windows)]
use gozler_core::usn::record_usn;
#[cfg(windows)]
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
#[cfg(windows)]
use std::time::{Duration, Instant};

/// The most failed folders listed by path; the rest are only counted.
#[cfg(windows)]
const LISTED_FAILURES: usize = 200;

/// One folder from the layout pass.
#[cfg(windows)]
struct LayoutFolder {
    frn: Frn,
    security_id: Option<u32>,
    usn: Option<i64>,
    /// The first name that is not a DOS alias, and its parent, for the path
    /// of a folder whose read fails.
    parent: Frn,
    name: String,
}

/// The time of a set of reads.
#[cfg(windows)]
#[derive(Default)]
struct Timing {
    reads: u32,
    total: Duration,
    longest: Duration,
}

#[cfg(windows)]
impl Timing {
    fn add(&mut self, took: Duration) {
        self.reads += 1;
        self.total += took;
        self.longest = self.longest.max(took);
    }

    fn line(&self) -> String {
        let mean_ms = if self.reads == 0 {
            0.0
        } else {
            self.total.as_secs_f64() * 1e3 / f64::from(self.reads)
        };
        format!(
            "{} reads in {:.2} s, {mean_ms:.3} ms each on average, longest {:.1} ms",
            self.reads,
            self.total.as_secs_f64(),
            self.longest.as_secs_f64() * 1e3
        )
    }
}

#[cfg(windows)]
fn main() -> Result<()> {
    let drive = std::env::args().nth(1).unwrap_or_else(|| "C:".to_owned());
    let mount = format!("{}\\", drive.trim_end_matches('\\'));
    gozler_winfs::enable_volume_privileges()
        .context("enabling the backup privilege; run this program elevated")?;
    let volume = gozler_winfs::enumerate_volumes()?
        .into_iter()
        .find(|v| v.mounts.iter().any(|m| m.eq_ignore_ascii_case(&mount)))
        .with_context(|| format!("no volume is mounted at {mount}"))?;
    if volume.filesystem.as_deref() != Some("NTFS") {
        bail!("{mount} is {:?}, not NTFS", volume.filesystem);
    }
    let handle = gozler_winfs::open_volume(&volume.guid_path)?;
    println!("volume {mount} ({})", volume.guid_path);

    let started = Instant::now();
    let (entries, folders) = layout_folders(&handle)?;
    println!(
        "layout pass: {entries} entries, {} folders, in {:.1} s",
        folders.len(),
        started.elapsed().as_secs_f64()
    );

    let mut by_security_id: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut without = 0usize;
    for (at, folder) in folders.iter().enumerate() {
        match folder.security_id {
            Some(id) => by_security_id.entry(id).or_default().push(at),
            None => without += 1,
        }
    }
    let own_descriptor = by_security_id.get(&0).map_or(0, Vec::len);
    println!(
        "folder SecurityIds: {} distinct; {own_descriptor} folders have SecurityId 0 \
         (their own descriptor); {without} folders have no SecurityId in the layout",
        by_security_id.len()
    );

    // One read per SecurityId, as a build that shares descriptors makes them.
    let mut shared = Timing::default();
    let (mut usn_equal, mut usn_differs, mut shared_gone) = (0u32, 0u32, 0u32);
    let mut shared_failed: BTreeMap<String, u32> = BTreeMap::new();
    for (&id, list) in &by_security_id {
        if id == 0 {
            continue;
        }
        let folder = &folders[list[0]];
        let start = Instant::now();
        let read = gozler_winfs::read_folder_security(&handle, folder.frn.0);
        shared.add(start.elapsed());
        match read {
            Ok(Some(security)) => match record_usn(&security.usn_record) {
                Ok(usn) if Some(usn) == folder.usn => usn_equal += 1,
                Ok(_) => usn_differs += 1,
                Err(error) => {
                    *shared_failed
                        .entry(format!("record_usn: {error}"))
                        .or_default() += 1
                }
            },
            Ok(None) => shared_gone += 1,
            Err(error) => *shared_failed.entry(error.to_string()).or_default() += 1,
        }
    }
    println!(
        "one read per SecurityId (read_folder_security): {}",
        shared.line()
    );
    println!(
        "  the record's USN equals the layout's for {usn_equal}, differs for {usn_differs}; \
         {shared_gone} folders gone"
    );
    for (error, count) in &shared_failed {
        println!("  failed {count}: {error}");
    }

    // One read per folder, as a build without sharing makes them.
    let mut every = Timing::default();
    let mut descriptors: HashSet<Vec<u8>> = HashSet::new();
    let mut descriptor_of_id: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut ids_with_several = BTreeSet::new();
    let mut gone = 0u32;
    let mut failed: Vec<(usize, gozler_winfs::WinError)> = Vec::new();
    for (at, folder) in folders.iter().enumerate() {
        let start = Instant::now();
        let read = gozler_winfs::read_folder_descriptor(&handle, folder.frn.0);
        every.add(start.elapsed());
        match read {
            Ok(Some(descriptor)) => {
                if let Some(id) = folder.security_id.filter(|&id| id != 0) {
                    let first = descriptor_of_id
                        .entry(id)
                        .or_insert_with(|| descriptor.clone());
                    if *first != descriptor {
                        ids_with_several.insert(id);
                    }
                }
                descriptors.insert(descriptor);
            }
            Ok(None) => gone += 1,
            Err(error) => failed.push((at, error)),
        }
    }
    println!("every folder (read_folder_descriptor): {}", every.line());
    println!("  {gone} folders gone, {} reads failed", failed.len());
    let total_bytes: usize = descriptors.iter().map(Vec::len).sum();
    println!(
        "distinct folder descriptors: {}, {} bytes in all, {:.0} bytes each on average",
        descriptors.len(),
        total_bytes,
        total_bytes as f64 / descriptors.len().max(1) as f64
    );
    println!(
        "SecurityIds whose folders read different descriptors: {}",
        ids_with_several.len()
    );

    if !failed.is_empty() {
        let mut by_error: BTreeMap<String, u32> = BTreeMap::new();
        for (_, error) in &failed {
            *by_error.entry(error.to_string()).or_default() += 1;
        }
        println!("failed reads by error:");
        for (error, count) in &by_error {
            println!("  {count}: {error}");
        }
        let index: HashMap<Frn, usize> = folders
            .iter()
            .enumerate()
            .map(|(at, f)| (f.frn, at))
            .collect();
        println!("failed folders:");
        for (at, error) in failed.iter().take(LISTED_FAILURES) {
            println!("  {}  ({error})", path(&mount, &folders, &index, *at));
        }
        if failed.len() > LISTED_FAILURES {
            println!("  and {} more", failed.len() - LISTED_FAILURES);
        }
    }
    Ok(())
}

/// Every folder of one FILE_LAYOUT pass over the volume, and the number of
/// entries the pass gave.
#[cfg(windows)]
fn layout_folders(handle: &gozler_winfs::VolumeHandle) -> Result<(usize, Vec<LayoutFolder>)> {
    let mut buf = vec![0u8; 1 << 20];
    let mut restart = true;
    let mut entries = 0;
    let mut folders = Vec::new();
    while let Some(bytes) =
        gozler_winfs::query_file_layout(handle, RECOMMENDED_LAYOUT_FLAGS, restart, &mut buf)?
    {
        restart = false;
        for file in walk_layout_buffer(&buf[..bytes]).context("parsing a FILE_LAYOUT page")? {
            entries += 1;
            if !file.is_dir() {
                continue;
            }
            let name = file.names.iter().find(|n| !n.dos_only);
            folders.push(LayoutFolder {
                frn: file.frn,
                security_id: file.security_id,
                usn: file.usn,
                parent: name.map_or(file.frn, |n| n.parent_frn),
                name: name.map_or_else(String::new, |n| {
                    String::from_utf8_lossy(&n.name).into_owned()
                }),
            });
        }
    }
    Ok((entries, folders))
}

/// The path of `folders[at]` below `mount`, as far as the layout names its
/// parents.
#[cfg(windows)]
fn path(mount: &str, folders: &[LayoutFolder], index: &HashMap<Frn, usize>, at: usize) -> String {
    let mut names = Vec::new();
    let mut current = at;
    // A depth cap, so a damaged parent chain cannot loop.
    for _ in 0..1024 {
        let folder = &folders[current];
        if folder.parent == folder.frn {
            break;
        }
        names.push(folder.name.as_str());
        match index.get(&folder.parent) {
            Some(&parent) => current = parent,
            None => {
                names.push("?");
                break;
            }
        }
    }
    names.reverse();
    format!("{mount}{}", names.join("\\"))
}
