//! Per-volume bootstrap: on NTFS, read the MFT through FILE_LAYOUT, and
//! enumerate it only when that pass stops; on ReFS, which has no MFT, walk the
//! folders. It wires the raw Win32 buffers from `gozler-winfs` through the
//! pure parsers in `gozler-core` into a `VolumeIndex`.
//!
//! This is the bridge the whole design turns on: `gozler-winfs` hands us bytes,
//! `gozler-core` interprets them, and neither knows about the other. The daemon
//! (here) owns the buffers and the loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use gozler_core::include::VolumeRules;
use gozler_core::index::VolumeIndex;
use gozler_core::index::settle::{self, FileLink, FileStat, VolumeReads};
use gozler_core::layout::{
    LayoutFile, RECOMMENDED_LAYOUT_FLAGS, walk_layout_buffer, walk_listing_buffer,
};
use gozler_core::types::Frn;
use gozler_core::usn::walk_enum_buffer;
use gozler_winfs::{
    ERROR_ACCESS_DENIED, JournalInfo, JournalQuery, JournalRecords, VolumeHandle, VolumeInfo,
    WinError, create_usn_journal, enum_usn_data, enumerate_volumes, file_links, open_folder,
    query_file_layout, query_usn_journal, root_folder_id, stat_file, volume_total_bytes,
};

/// Floor for the USN journal: large enough that routine churn does not wrap it
/// before we finish bootstrapping. [`journal_target_bytes`] may ask for more.
const JOURNAL_MIN_SIZE: u64 = 64 * 1024 * 1024;
/// Ceiling for [`journal_target_bytes`]: past 1 GiB the horizon is weeks and
/// the space stops being free.
const JOURNAL_MAX_SIZE: u64 = 1024 * 1024 * 1024;
/// 8 MiB trimmed from the front when the journal is full. Kept small on
/// purpose: NTFS trims slowly, and a large delta stalls the writer.
const JOURNAL_ALLOC_DELTA: u64 = 8 * 1024 * 1024;
/// 1 MiB ioctl buffer, which keeps the number of ENUM round-trips low.
const IO_BUFFER_BYTES: usize = 1024 * 1024;
/// 64 KiB listing buffer, as `u64`s because the entries are 8-byte aligned.
/// It holds several hundred names, so one read lists most folders.
const LISTING_BUFFER_WORDS: usize = 8 * 1024;

/// The filesystems gozler indexes. They build the index differently and ask
/// the journal for different records; the index, the journal tail and the
/// coherence rules are shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filesystem {
    Ntfs,
    Refs,
}

impl Filesystem {
    /// The journal records that hold this filesystem's file IDs whole.
    pub(crate) fn journal_records(self) -> JournalRecords {
        match self {
            Self::Ntfs => JournalRecords::V2,
            Self::Refs => JournalRecords::V3,
        }
    }
}

/// A freshly bootstrapped volume, as `gozlerd scan` reports it.
pub(crate) struct BootstrappedVolume {
    pub guid_path: String,
    pub mounts: Vec<String>,
    pub filesystem: Filesystem,
    pub read_secs: f64,
    pub enum_secs: Option<f64>,
    pub journal: JournalSetup,
    pub index: VolumeIndex,
}

impl BootstrappedVolume {
    pub(crate) fn entries(&self) -> usize {
        self.index.len()
    }

    /// Where the volume is mounted.
    pub(crate) fn mounted(&self) -> gozler_core::scope::VolumeMounts<'_> {
        gozler_core::scope::VolumeMounts {
            mounts: &self.mounts,
            guid: &self.guid_path,
        }
    }
}

/// Enables the volume privileges, then lists every volume on the machine.
pub(crate) fn enumerate_all() -> Result<Vec<VolumeInfo>> {
    // The raw volume and journal reads need these privileges; if the process is
    // not elevated the open itself fails with a clear message, so a
    // privilege-enable failure here is only a warning.
    if let Err(e) = gozler_winfs::enable_volume_privileges() {
        tracing::warn!(error = %e, "could not enable volume privileges; continuing");
    }
    enumerate_volumes().context("enumerating volumes")
}

/// The filesystem gozler indexes `vol` as, or why it does not index it:
/// gozler indexes fixed NTFS and ReFS volumes that have a drive letter or a
/// mount folder.
pub(crate) fn indexed_filesystem(vol: &VolumeInfo) -> Result<Filesystem, String> {
    if !vol.is_fixed {
        return Err("not a fixed disk".into());
    }
    // A volume without one, such as Windows' hidden recovery partition, has
    // no path that File Explorer or any program opens, so its files are no
    // result a person can use.
    if vol.mounts.is_empty() {
        return Err("no drive letter or mount folder".into());
    }
    match vol.filesystem.as_deref() {
        Some("NTFS") => Ok(Filesystem::Ntfs),
        Some("ReFS") => Ok(Filesystem::Refs),
        Some(other) => Err(format!("{other}, not NTFS or ReFS")),
        None => Err("its filesystem could not be read, as on a locked BitLocker volume".into()),
    }
}

/// Bootstraps every fixed NTFS and ReFS volume once, for `gozlerd scan`, as
/// the saved Include settings say. A volume that fails is logged and left out
/// rather than aborting the whole run.
pub(crate) fn scan_all() -> Result<Vec<BootstrappedVolume>> {
    let include = crate::include::read_saved();
    let targets: Vec<(VolumeInfo, Filesystem, Arc<VolumeRules>)> = enumerate_all()?
        .into_iter()
        .filter_map(|v| {
            let filesystem = indexed_filesystem(&v).ok()?;
            let rules = include.rules_for(&v.guid_path);
            (!rules.leaves_out_volume()).then(|| (v, filesystem, Arc::new(rules)))
        })
        .collect();
    if targets.is_empty() {
        bail!("no fixed NTFS or ReFS volumes found");
    }

    // One thread per volume. The MFT scans are independent, so a slow disk
    // (a USB disk waking from spin-down) does not queue the others. Results
    // are collected in enumeration order so the volume set is deterministic
    // whichever thread finishes first. Two large partitions on one spinning
    // disk would interleave their MFT reads; accepted, since such layouts are
    // rare and the cost is bounded by the 1 MiB ioctl buffer per seek.
    let results: Vec<Result<BootstrappedVolume>> = std::thread::scope(|s| {
        let threads: Vec<_> = targets
            .iter()
            .map(|(vol, filesystem, rules)| {
                let label = gozler_core::scope::VolumeMounts {
                    mounts: &vol.mounts,
                    guid: &vol.guid_path,
                }
                .display_prefix();
                std::thread::Builder::new()
                    .name(format!("gozler-bootstrap-{label}"))
                    .spawn_scoped(s, move || bootstrap_volume(vol, *filesystem, rules))
            })
            .collect();
        threads
            .into_iter()
            .map(|t| match t {
                Ok(t) => t
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("bootstrap thread panicked"))),
                Err(e) => Err(anyhow::Error::from(e).context("spawning bootstrap thread")),
            })
            .collect()
    });

    let mut ok = Vec::new();
    for ((vol, ..), result) in targets.iter().zip(results) {
        match result {
            Ok(scan) => ok.push(scan),
            Err(e) => tracing::error!(
                volume = %vol.guid_path,
                error = %format!("{e:#}"),
                "volume failed to bootstrap"
            ),
        }
    }
    if ok.is_empty() {
        bail!("no volume could be bootstrapped (see the log for per-volume errors)");
    }
    Ok(ok)
}

/// Opens a volume for the journal and MFT reads. Only `ERROR_ACCESS_DENIED`
/// gets the elevation hint: a missing, offline or not-ready volume fails with
/// another code, and the hint would send the reader the wrong way.
pub(crate) fn open_volume(guid_path: &str) -> Result<VolumeHandle> {
    gozler_winfs::open_volume(guid_path).map_err(|e| {
        let hint = if e.code == ERROR_ACCESS_DENIED {
            " (must run elevated for raw volume access)"
        } else {
            ""
        };
        anyhow::Error::new(e).context(format!("opening the volume{hint}"))
    })
}

/// Bootstraps a single volume: journal setup, then [`build_index`].
fn bootstrap_volume(
    vol: &VolumeInfo,
    filesystem: Filesystem,
    rules: &Arc<VolumeRules>,
) -> Result<BootstrappedVolume> {
    let handle = open_volume(&vol.guid_path)?;

    // The journal is only needed for the live tail; a one-shot scan reads the
    // volume directly, so an unavailable journal is not fatal here.
    let journal = ensure_journal(&handle, &vol.guid_path);
    // First-time bootstrap passes no stop flag, so the rebuild can never abort.
    let built = build_index(&handle, &vol.guid_path, filesystem, rules, None)?
        .expect("build_index cannot abort without a stop flag");

    Ok(BootstrappedVolume {
        guid_path: vol.guid_path.clone(),
        mounts: vol.mounts.clone(),
        filesystem,
        read_secs: built.read_secs,
        enum_secs: built.enum_secs,
        journal,
        index: built.index,
    })
}

/// A freshly built index and how its passes went.
pub(crate) struct BuiltIndex {
    pub index: VolumeIndex,
    /// How long the first pass took: the FILE_LAYOUT pass on NTFS, the
    /// folder walk on ReFS.
    pub read_secs: f64,
    /// How long the enumeration took, or `None` when it did not run because
    /// the first pass completed.
    pub enum_secs: Option<f64>,
    /// The FILE_LAYOUT pass stopped, so some files' sizes and dates are
    /// unknown. The volume stores it as
    /// [`Indexed::metadata_pending`](crate::volume_state::Indexed::metadata_pending).
    pub metadata_pending: bool,
}

/// How the FILE_LAYOUT pass ended.
#[derive(Debug, PartialEq, Eq)]
enum LayoutPass {
    Complete,
    Stopped,
    Cancelled,
}

/// Builds a fresh [`VolumeIndex`] for an already-open volume handle, with its
/// storage optimized for publishing and its names ranked. Shared by
/// first-time bootstrap and the live tail's rescan (after a USN journal
/// wrap/deletion), which rebuilds the index on the handle it already owns.
/// Returns `None` when `stop` aborts the build. The journal is the caller's
/// concern.
///
/// On NTFS, one FILE_LAYOUT pass gives every file its names, size and date,
/// so the MFT is read once. When that pass stops part way, the MFT enumeration
/// runs into the same index and names the files the pass did not reach, which
/// keeps their sizes and dates unknown. Without it a failed read would drop
/// every file after it from the index.
///
/// ReFS answers neither request, so its build walks every folder from the
/// root with [`settle::walk_folders`], which gives names, sizes and dates in
/// one pass.
///
/// Either build reads the whole volume and then removes what `rules` leave
/// out, because a folder's path is known only once every folder is read.
pub(crate) fn build_index(
    handle: &VolumeHandle,
    guid_path: &str,
    filesystem: Filesystem,
    rules: &Arc<VolumeRules>,
    stop: Option<&AtomicBool>,
) -> Result<Option<BuiltIndex>> {
    let cancelled = || stop.is_some_and(|s| s.load(Ordering::Relaxed));
    // Read from the volume on every filesystem: NTFS gives record 5, and
    // ReFS gives its own, so neither root is assumed.
    let root = Frn(root_folder_id(guid_path).context("reading the root folder's file ID")?);
    let mut index = VolumeIndex::new(root);

    let read_start = Instant::now();
    if filesystem == Filesystem::Refs {
        let walked = settle::walk_folders(
            &mut Disk::new(handle, guid_path),
            &[root],
            cancelled,
            |files| {
                for f in files {
                    index.insert_listed(f);
                }
            },
        )?;
        if !walked {
            return Ok(None);
        }
        finish_index(&mut index, rules);
        return Ok(Some(BuiltIndex {
            index,
            read_secs: read_start.elapsed().as_secs_f64(),
            enum_secs: None,
            metadata_pending: false,
        }));
    }

    let mut buf = vec![0u8; IO_BUFFER_BYTES];
    let pass = layout_pass(&mut index, &mut buf, cancelled, |restart, buf| {
        query_file_layout(handle, RECOMMENDED_LAYOUT_FLAGS, restart, buf)
    })?;
    let read_secs = read_start.elapsed().as_secs_f64();

    let enum_secs = match pass {
        LayoutPass::Complete => None,
        LayoutPass::Cancelled => return Ok(None),
        LayoutPass::Stopped => {
            let enum_start = Instant::now();
            let mut next_frn: u64 = 0;
            loop {
                if cancelled() {
                    return Ok(None);
                }
                let Some(bytes) =
                    enum_usn_data(handle, next_frn, &mut buf).context("FSCTL_ENUM_USN_DATA")?
                else {
                    break; // ERROR_HANDLE_EOF: enumeration complete
                };
                let walk = walk_enum_buffer(&buf[..bytes]).context("parsing ENUM buffer")?;
                for rec in &walk.records {
                    index.insert_enum(rec);
                }
                // The kernel guarantees forward progress; guard against a stuck
                // cursor anyway so a driver quirk can never spin forever.
                if walk.next_start_frn == next_frn && walk.records.is_empty() {
                    break;
                }
                next_frn = walk.next_start_frn;
            }
            Some(enum_start.elapsed().as_secs_f64())
        }
    };

    finish_index(&mut index, rules);
    if pass == LayoutPass::Stopped {
        index.record_size_pass_stopped();
    }

    Ok(Some(BuiltIndex {
        index,
        read_secs,
        enum_secs,
        metadata_pending: pass == LayoutPass::Stopped,
    }))
}

/// A volume's reads for [`settle`], on the handle the caller owns. Each error
/// is logged at debug level where it happens, with the volume and file ID.
pub(crate) struct Disk<'a> {
    handle: &'a VolumeHandle,
    /// The volume as log lines name it.
    volume: &'a str,
    buf: Vec<u64>,
}

impl<'a> Disk<'a> {
    pub(crate) fn new(handle: &'a VolumeHandle, volume: &'a str) -> Self {
        Self {
            handle,
            volume,
            buf: vec![0; LISTING_BUFFER_WORDS],
        }
    }

    fn list(&mut self, folder: Frn, page: &mut dyn FnMut(&[LayoutFile])) -> Result<bool> {
        let Some(listing) =
            open_folder(self.handle, folder.0).map_err(|e| anyhow!("opening it: {e}"))?
        else {
            return Ok(false);
        };
        while let Some(bytes) = listing
            .read_listing(&mut self.buf)
            .map_err(|e| anyhow!("reading its listing: {e}"))?
        {
            let files = walk_listing_buffer(folder, bytes)
                .map_err(|e| anyhow!("parsing its listing: {e}"))?;
            page(&files);
        }
        Ok(true)
    }
}

impl VolumeReads for Disk<'_> {
    type Error = anyhow::Error;

    fn list_folder(&mut self, folder: Frn, page: &mut dyn FnMut(&[LayoutFile])) -> Result<bool> {
        let listed = self.list(folder, page);
        if let Err(e) = &listed {
            tracing::debug!(volume = %self.volume, ?folder, error = %e, "folder listing failed");
        }
        listed
    }

    fn stat(&mut self, frn: Frn) -> Result<Option<FileStat>> {
        match stat_file(self.handle, frn.0) {
            Ok(stat) => Ok(stat.map(|s| FileStat {
                size: s.size,
                mtime: s.mtime_ft,
            })),
            Err(e) => {
                tracing::debug!(volume = %self.volume, ?frn, error = %e, "live stat failed");
                Err(e.into())
            }
        }
    }

    fn links(&mut self, frn: Frn) -> Result<Option<Vec<FileLink>>> {
        match file_links(self.handle, frn.0) {
            Ok(links) => Ok(links.map(|links| {
                links
                    .into_iter()
                    .map(|link| {
                        let mut name = Vec::new();
                        gozler_core::wtf8::from_utf16(&link.name, &mut name);
                        FileLink {
                            folder: Frn(link.folder),
                            name,
                        }
                    })
                    .collect()
            })),
            Err(e) => {
                tracing::debug!(volume = %self.volume, ?frn, error = %e, "link walk failed");
                Err(e.into())
            }
        }
    }
}

/// Ends a build that read the whole volume: removes what `rules` leave out,
/// sums each folder's size, and readies the index to publish. A pass that
/// stopped left some files' sizes unknown, and a folder's size then sums the
/// sizes that were read, because the space found tells more than no size.
fn finish_index(index: &mut VolumeIndex, rules: &Arc<VolumeRules>) {
    index.apply_rules(rules.clone());
    index.compute_folder_sizes();
    // Every record has been seen, so the FRN map can densify against its true
    // maximum, and the growth slack of the build goes back.
    index.optimize_storage();
    index.rank_names();
}

/// The FILE_LAYOUT pass: hands each file of each page that `read_page` writes
/// into `buf` to [`VolumeIndex::insert_layout`], where `read_page(restart,
/// buf)` behaves as [`query_file_layout`]. A failed read stops the pass rather
/// than failing the build; [`build_index`] then names the remaining files.
fn layout_pass(
    index: &mut VolumeIndex,
    buf: &mut [u8],
    cancelled: impl Fn() -> bool,
    mut read_page: impl FnMut(bool, &mut [u8]) -> Result<Option<usize>, WinError>,
) -> Result<LayoutPass> {
    let mut restart = true;
    loop {
        if cancelled() {
            return Ok(LayoutPass::Cancelled);
        }
        match read_page(restart, buf) {
            Ok(Some(bytes)) => {
                let files =
                    walk_layout_buffer(&buf[..bytes]).context("parsing FILE_LAYOUT buffer")?;
                for f in &files {
                    index.insert_layout(f);
                }
                restart = false;
            }
            Ok(None) => return Ok(LayoutPass::Complete),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "FILE_LAYOUT pass stopped; enumerating the remaining names, whose sizes and dates stay unknown"
                );
                return Ok(LayoutPass::Stopped);
            }
        }
    }
}

/// How large the USN journal on a volume should be, given the volume's total
/// size (`None` when it could not be read).
///
/// The journal is the only thing that lets a restart replay instead of
/// rescan, so its size sets the longest downtime gozler can recover from without
/// a full MFT scan. On a busy volume Windows' default 32 MB holds only hours of
/// changes, so a cursor saved at night is purged by morning. The cost is disk:
/// the journal never shrinks, and its maximum is real space once the volume has
/// churned that much. Every other journal consumer (Windows Search, backup
/// tools) shares it and is only helped by a larger one.
///
/// Policy: 1/4096 of the volume, clamped to [[`JOURNAL_MIN_SIZE`], 1 GiB].
/// A 256 GB laptop disk gets 64 MiB, a 2 TB disk 512 MiB, anything from 4 TB
/// up the 1 GiB cap. `FSCTL_CREATE_USN_JOURNAL` treats a value below the
/// current maximum as a no-op, so no clamp against it is needed.
fn journal_target_bytes(volume_bytes: Option<u64>) -> u64 {
    volume_bytes
        .map_or(JOURNAL_MIN_SIZE, |bytes| bytes / 4096)
        .clamp(JOURNAL_MIN_SIZE, JOURNAL_MAX_SIZE)
}

/// What [`ensure_journal`] got for a volume.
#[derive(Debug)]
pub(crate) enum JournalSetup {
    /// A usable journal, with the cursor to tail it from.
    Active(JournalInfo),
    /// The volume has no journal and refused to create one, so it never will.
    Refused,
    /// No usable journal right now (a query that failed, a deletion in
    /// progress, or a journal created but not yet active); a later attempt
    /// may get one.
    Unavailable,
}

/// Gets the volume's USN journal, creating or growing it as needed.
///
/// Never fatal: a one-shot scan reads the MFT directly and does not need the
/// journal. An active journal smaller than [`journal_target_bytes`] is grown
/// in place; a failure to grow is logged as an error, because every restart
/// then falls back to a full scan, but the journal is still used at its
/// current size. An absent journal is created on
/// the spot and used if creation succeeds. Every other outcome logs a note and
/// leaves live updates unavailable.
pub(crate) fn ensure_journal(handle: &VolumeHandle, guid_path: &str) -> JournalSetup {
    let target = journal_target_bytes(volume_total_bytes(guid_path).ok());
    match query_usn_journal(handle) {
        Ok(JournalQuery::Active(info)) => {
            if info.maximum_size >= target {
                return JournalSetup::Active(info);
            }
            let wanted_mib = target / (1024 * 1024);
            let actual_mib = info.maximum_size / (1024 * 1024);
            if let Err(e) = create_usn_journal(handle, target, JOURNAL_ALLOC_DELTA) {
                tracing::error!(
                    error = %e,
                    wanted_mib,
                    actual_mib,
                    "could not grow the USN journal; at this size a restart after more than a few hours of changes must rescan the whole volume"
                );
                return JournalSetup::Active(info);
            }
            // Growing keeps the journal ID and every record, so the pre-grow
            // cursor stays valid; re-query only so the reported size is the
            // new one, and keep the old info if the re-query misbehaves.
            match query_usn_journal(handle) {
                Ok(JournalQuery::Active(grown)) => {
                    let to_mib = grown.maximum_size / (1024 * 1024);
                    if grown.maximum_size > info.maximum_size {
                        tracing::info!(from_mib = actual_mib, to_mib, "grew the USN journal");
                    } else {
                        // NTFS accepted the call but kept its own ceiling: a
                        // tiny volume (recovery partitions sit at 1 MiB) or
                        // a size that only applies at the next trim.
                        tracing::debug!(
                            wanted_mib,
                            actual_mib,
                            "USN journal size unchanged; the filesystem capped it"
                        );
                    }
                    JournalSetup::Active(grown)
                }
                other => {
                    tracing::error!(
                        state = ?other,
                        "journal state changed right after growing it; keeping the earlier cursor"
                    );
                    JournalSetup::Active(info)
                }
            }
        }
        Ok(JournalQuery::NotActive) => {
            match create_usn_journal(handle, target, JOURNAL_ALLOC_DELTA) {
                Ok(()) => match query_usn_journal(handle) {
                    Ok(JournalQuery::Active(info)) => JournalSetup::Active(info),
                    // We just created a journal and it is not active. Rare, but
                    // naming it matters: a volume that should have tailed live
                    // silently will not.
                    Ok(other) => {
                        tracing::warn!(
                            state = ?other,
                            "created a USN journal but it is not active; live updates unavailable"
                        );
                        JournalSetup::Unavailable
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "created a USN journal but could not re-query it; live updates unavailable"
                        );
                        JournalSetup::Unavailable
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "no USN journal and could not create one; live updates unavailable"
                    );
                    JournalSetup::Refused
                }
            }
        }
        Ok(JournalQuery::DeleteInProgress) => {
            tracing::warn!("USN journal deletion in progress; live updates unavailable");
            JournalSetup::Unavailable
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not query the USN journal; live updates unavailable");
            JournalSetup::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    /// Windows' hidden recovery partition is a fixed NTFS volume with no
    /// drive letter; indexed, it showed in the window's Location list and
    /// in results as `\\?\Volume{…}\` paths that nothing can open.
    #[test]
    fn a_volume_without_a_mount_is_not_indexed() {
        let volume = |mounts: &[&str]| VolumeInfo {
            guid_path: r"\\?\Volume{d0b34515-aad9-46e5-bb8c-eaeabd515c12}\".into(),
            mounts: mounts.iter().map(|m| m.to_string()).collect(),
            is_fixed: true,
            filesystem: Some("NTFS".into()),
        };
        assert!(indexed_filesystem(&volume(&[])).is_err());
        assert!(matches!(
            indexed_filesystem(&volume(&[r"C:\mnt\data\"])),
            Ok(Filesystem::Ntfs)
        ));
    }

    #[test]
    fn journal_target_scales_with_the_volume_between_the_clamps() {
        assert_eq!(journal_target_bytes(None), JOURNAL_MIN_SIZE);
        assert_eq!(journal_target_bytes(Some(0)), JOURNAL_MIN_SIZE);
        assert_eq!(journal_target_bytes(Some(256 * GIB)), 64 * MIB);
        assert_eq!(journal_target_bytes(Some(2048 * GIB)), 512 * MIB);
        assert_eq!(journal_target_bytes(Some(4096 * GIB)), GIB);
        assert_eq!(journal_target_bytes(Some(16 * 1024 * GIB)), GIB);
    }

    /// A failed FILE_LAYOUT read must end the pass as stopped, not complete;
    /// otherwise `--status` and every results page claim sizes and dates are
    /// complete on a volume where they are not.
    #[test]
    fn layout_pass_reports_a_failed_read_as_stopped() {
        let pass = |result: Result<Option<usize>, WinError>| {
            let mut index = VolumeIndex::new(gozler_core::index::NTFS_ROOT_FRN);
            layout_pass(&mut index, &mut [0u8; 16], || false, |_, _| result.clone())
                .expect("no page is parsed")
        };
        let failed = Err(WinError {
            code: 1, // ERROR_INVALID_FUNCTION: the volume refuses the query
            context: "FSCTL_QUERY_FILE_LAYOUT",
        });
        assert_eq!(pass(failed), LayoutPass::Stopped);
        assert_eq!(pass(Ok(None)), LayoutPass::Complete);
    }

    /// After a pass that stopped, the enumeration names the files the pass
    /// did not reach, without a size. Their folder still sums the sizes the
    /// pass read; if it stayed unknown, the window and `-size` would show no
    /// folder size on that volume until a rescan.
    #[test]
    fn a_stopped_pass_still_gives_folder_sizes() {
        use gozler_core::include::IncludeSettings;
        use gozler_core::usn::ParsedUsnRecord;
        use gozler_core::usn::record::FILE_ATTRIBUTE_DIRECTORY;

        let root = gozler_core::index::NTFS_ROOT_FRN;
        let mut index = VolumeIndex::new(root);
        let frn = |n: u128| Frn(n | 1 << 48);
        let (docs, read, unread) = (frn(100), frn(101), frn(102));
        for (frn, parent, name, attributes) in [
            (docs, root, "docs", FILE_ATTRIBUTE_DIRECTORY),
            (read, docs, "read.txt", 0),
            (unread, docs, "unread.txt", 0),
        ] {
            index.insert_enum(&ParsedUsnRecord {
                frn,
                parent_frn: parent,
                reason: 0,
                attributes,
                name: name.as_bytes().to_vec(),
            });
        }
        index.set_stat(read, 5, 0);
        finish_index(
            &mut index,
            &Arc::new(IncludeSettings::default().rules_for("V")),
        );
        let docs = index.head_of(docs).expect("docs is indexed");
        assert_eq!(index.entry(docs).size(), Some(5));
    }
}
