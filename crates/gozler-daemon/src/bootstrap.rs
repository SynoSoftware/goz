//! Per-volume bootstrap: enumerate the MFT and enrich from FILE_LAYOUT, wiring
//! the raw Win32 buffers from `gozler-winfs` through the pure parsers in
//! `gozler-core` into a `VolumeIndex`.
//!
//! This is the bridge the whole design turns on: `gozler-winfs` hands us bytes,
//! `gozler-core` interprets them, and neither knows about the other. The daemon
//! (here) owns the buffers and the loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use gozler_core::index::{FrnMap, NTFS_ROOT_FRN, VolumeIndex};
use gozler_core::layout::{RECOMMENDED_LAYOUT_FLAGS, walk_layout_buffer};
use gozler_core::usn::walk_enum_buffer;
use gozler_winfs::{
    JournalInfo, JournalQuery, VolumeHandle, VolumeInfo, WinError, create_usn_journal,
    enum_usn_data, enumerate_volumes, query_file_layout, query_usn_journal, volume_total_bytes,
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
/// 1 MiB ioctl buffer: ~100 ENUM round-trips per million files.
const IO_BUFFER_BYTES: usize = 1024 * 1024;

/// A freshly bootstrapped volume, as `gozlerd scan` reports it.
pub(crate) struct BootstrappedVolume {
    pub guid_path: String,
    pub mounts: Vec<String>,
    pub enum_secs: f64,
    pub layout_secs: f64,
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

/// Why gozler does not index `vol`, or `None` when it does: gozler indexes
/// fixed NTFS volumes.
pub(crate) fn skip_reason(vol: &VolumeInfo) -> Option<String> {
    if !vol.is_fixed {
        return Some("not a fixed disk".into());
    }
    match vol.filesystem.as_deref() {
        Some("NTFS") => None,
        Some(other) => Some(format!("{other}, not NTFS")),
        None => Some("its filesystem could not be read, as on a locked BitLocker volume".into()),
    }
}

/// Bootstraps every fixed NTFS volume once, for `gozlerd scan`. A volume that
/// fails is logged and left out rather than aborting the whole run.
pub(crate) fn scan_all() -> Result<Vec<BootstrappedVolume>> {
    let targets: Vec<VolumeInfo> = enumerate_all()?
        .into_iter()
        .filter(|v| skip_reason(v).is_none())
        .collect();
    if targets.is_empty() {
        bail!("no fixed NTFS volumes found");
    }

    // One thread per volume. The MFT scans are independent, so a USB disk
    // waking from spin-down or a second SSD no longer queues behind the
    // largest volume (measured: the non-system volumes were 21% of a 36 s
    // bootstrap, all of it now overlapped). Results are collected in
    // enumeration order so the volume set is deterministic whichever thread
    // finishes first. Two large partitions on one spinning disk would
    // interleave their MFT reads; accepted, since such layouts are rare and
    // the cost is bounded by the 1 MiB ioctl buffer per seek.
    let results: Vec<Result<BootstrappedVolume>> = std::thread::scope(|s| {
        let threads: Vec<_> = targets
            .iter()
            .map(|vol| {
                let label = gozler_core::scope::VolumeMounts {
                    mounts: &vol.mounts,
                    guid: &vol.guid_path,
                }
                .display_prefix();
                std::thread::Builder::new()
                    .name(format!("goz-bootstrap-{label}"))
                    .spawn_scoped(s, move || bootstrap_volume(vol))
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
    for (vol, result) in targets.iter().zip(results) {
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

/// Bootstraps a single volume: journal setup → MFT enumeration → FILE_LAYOUT
/// enrichment.
fn bootstrap_volume(vol: &VolumeInfo) -> Result<BootstrappedVolume> {
    let handle = gozler_winfs::open_volume(&vol.guid_path).with_context(|| {
        format!(
            "opening {} (must run elevated for raw volume access)",
            vol.guid_path
        )
    })?;

    // The journal is only needed for the live tail; a one-shot scan reads the
    // MFT directly, so an unavailable journal is not fatal here.
    let journal = ensure_journal(&handle, &vol.guid_path);
    // First-time bootstrap passes no stop flag, so the rebuild can never abort.
    let built = build_index(&handle, None)?.expect("build_index cannot abort without a stop flag");

    Ok(BootstrappedVolume {
        guid_path: vol.guid_path.clone(),
        mounts: vol.mounts.clone(),
        enum_secs: built.enum_secs,
        layout_secs: built.layout_secs,
        journal,
        index: built.index,
    })
}

/// A freshly built index and how its two passes went.
pub(crate) struct BuiltIndex {
    pub index: VolumeIndex,
    pub enum_secs: f64,
    pub layout_secs: f64,
    /// The size and date pass failed or stopped, so some files' sizes and
    /// dates are unknown. The volume stores it as
    /// [`VolumeState::metadata_pending`](crate::volume_state::VolumeState::metadata_pending).
    pub metadata_pending: bool,
}

/// How the size and date pass (FILE_LAYOUT enrichment) ended.
#[derive(Debug, PartialEq, Eq)]
enum LayoutPass {
    Complete,
    Stopped,
    Cancelled,
}

/// Builds a fresh [`VolumeIndex`] for an already-open volume handle: a full MFT
/// enumeration followed by FILE_LAYOUT enrichment, with its storage optimized
/// for publishing. Shared by first-time bootstrap and the live tail's rescan
/// (after a USN journal wrap/deletion), which rebuilds the index on the handle
/// it already owns. Returns `None` when `stop` aborts the build. The journal is
/// the caller's concern.
pub(crate) fn build_index(
    handle: &VolumeHandle,
    stop: Option<&AtomicBool>,
) -> Result<Option<BuiltIndex>> {
    let cancelled = || stop.is_some_and(|s| s.load(Ordering::Relaxed));
    let mut index = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
    let mut buf = vec![0u8; IO_BUFFER_BYTES];

    // --- MFT enumeration ------------------------------------------------
    let enum_start = Instant::now();
    let mut next_frn: u64 = 0;
    loop {
        if cancelled() {
            return Ok(None); // shutdown arrived mid-rebuild: abort cooperatively
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
        // The kernel guarantees forward progress; guard against a stuck cursor
        // anyway so a driver quirk can never spin forever.
        if walk.next_start_frn == next_frn && walk.records.is_empty() {
            break;
        }
        next_frn = walk.next_start_frn;
    }
    let enum_secs = enum_start.elapsed().as_secs_f64();

    let layout_start = Instant::now();
    let pass = enrich_from_layout(&mut index, &mut buf, cancelled, |restart, buf| {
        query_file_layout(handle, RECOMMENDED_LAYOUT_FLAGS, restart, buf)
    })?;
    let metadata_pending = match pass {
        LayoutPass::Complete => false,
        LayoutPass::Stopped => true,
        LayoutPass::Cancelled => return Ok(None),
    };
    let layout_secs = layout_start.elapsed().as_secs_f64();

    // Every record has been seen, so the FRN map can densify against its true
    // maximum, and the growth slack of the build goes back.
    index.optimize_storage();

    Ok(Some(BuiltIndex {
        index,
        enum_secs,
        layout_secs,
        metadata_pending,
    }))
}

/// The size and date pass: enriches `index` with sizes, dates and hard-link
/// names from each FILE_LAYOUT page `read_page` writes into `buf`, where
/// `read_page(restart, buf)` behaves as [`query_file_layout`].
///
/// Best-effort: a failed read stops the pass and leaves the remaining sizes
/// and dates unknown, which the query engine and CSV writer handle safely.
/// Degrading beats failing the whole bootstrap, since names alone still answer
/// most queries.
fn enrich_from_layout(
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
                    index.enrich(f);
                }
                restart = false;
            }
            Ok(None) => return Ok(LayoutPass::Complete),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "FILE_LAYOUT enrichment stopped; sizes and dates may be incomplete"
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
/// rescan, so its size sets the longest downtime goz can recover from without
/// a full MFT scan. Measured on a busy development C: (2 TB, ~4M entries):
/// about 4.5 MB of journal per hour, so Windows' default 32 MB holds roughly
/// nine hours and a cursor saved at night is purged by morning. The cost is
/// disk: the journal never shrinks, and its maximum is real space once the
/// volume has churned that much. Every other journal consumer (Windows Search,
/// backup tools) shares it and is only helped by a larger one.
///
/// Policy: 1/4096 of the volume, clamped to [[`JOURNAL_MIN_SIZE`], 1 GiB].
/// A 256 GB laptop disk gets 64 MiB, a 2 TB disk 512 MiB (about four days of
/// the churn above), anything from 4 TB up the 1 GiB cap. Everything leaves
/// the system's 32 MB in place and makes the user find the setting; its author
/// runs 128 MiB himself and suggests 256 MiB to 1 GiB for large busy volumes,
/// so this automates what he does by hand. `FSCTL_CREATE_USN_JOURNAL` treats a
/// value below the current maximum as a no-op, so no clamp against it is
/// needed.
fn journal_target_bytes(volume_bytes: Option<u64>) -> u64 {
    volume_bytes
        .map_or(JOURNAL_MIN_SIZE, |bytes| bytes / 4096)
        .clamp(JOURNAL_MIN_SIZE, JOURNAL_MAX_SIZE)
}

/// What [`ensure_journal`] got for a volume.
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
/// in place; a failure to grow is an error (not a warning: it went unread as a
/// warning for months while every restart fell back to a full scan) but the
/// journal is still used at its current size. An absent journal is created on
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

    #[test]
    fn journal_target_scales_with_the_volume_between_the_clamps() {
        assert_eq!(journal_target_bytes(None), JOURNAL_MIN_SIZE);
        assert_eq!(journal_target_bytes(Some(0)), JOURNAL_MIN_SIZE);
        assert_eq!(journal_target_bytes(Some(256 * GIB)), 64 * MIB);
        assert_eq!(journal_target_bytes(Some(2048 * GIB)), 512 * MIB);
        assert_eq!(journal_target_bytes(Some(4096 * GIB)), GIB);
        assert_eq!(journal_target_bytes(Some(16 * 1024 * GIB)), GIB);
    }

    /// A failed FILE_LAYOUT read must end the pass as stopped, not complete.
    /// When it ended as complete (issue #5), `--status` and every results page
    /// claimed sizes and dates were complete on a volume where they were not.
    #[test]
    fn layout_pass_reports_a_failed_read_as_stopped() {
        let pass = |result: Result<Option<usize>, WinError>| {
            let mut index = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
            enrich_from_layout(&mut index, &mut [0u8; 16], || false, |_, _| result.clone())
                .expect("no page is parsed")
        };
        let failed = Err(WinError {
            code: 1, // ERROR_INVALID_FUNCTION: the volume refuses the query
            context: "FSCTL_QUERY_FILE_LAYOUT",
        });
        assert_eq!(pass(failed), LayoutPass::Stopped);
        assert_eq!(pass(Ok(None)), LayoutPass::Complete);
    }
}
