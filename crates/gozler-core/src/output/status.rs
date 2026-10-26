//! The `gozler --status` report.

use super::table::grouped;
use crate::proto::{DaemonStatus, MemPair};
use std::io::{self, Write};

/// Writes the `--status` report: whether every volume is ready first, then
/// each volume with its details under labels.
///
/// # Errors
///
/// The error of writing to `out`.
pub fn write_status(s: &DaemonStatus, out: &mut impl Write) -> io::Result<()> {
    let ready = s.volumes.iter().filter(|v| v.phase.is_complete()).count();
    let entries: u64 = s.volumes.iter().map(|v| v.entries).sum();
    writeln!(
        out,
        "gozlerd: {ready} of {} ready, {}",
        plural(s.volumes.len() as u64, "volume", "volumes"),
        plural(entries, "entry", "entries"),
    )?;
    if s.process_private_bytes > 0 {
        writeln!(
            out,
            "Memory: {} private, {} working set",
            bytes(s.process_private_bytes),
            bytes(s.process_working_set),
        )?;
    }
    for v in &s.volumes {
        writeln!(
            out,
            "\n{}  {}, {}",
            v.mounted().display_prefix(),
            v.phase,
            plural(v.entries, "entry", "entries"),
        )?;
        if v.metadata_pending {
            detail(
                out,
                "Sizes, dates",
                "some unknown: the size and date pass did not finish",
            )?;
        }
        // The journal's size is the replay horizon: everything older
        // than what it holds costs a full rescan on restart.
        if let Some(max) = v.journal_max_bytes {
            detail(out, "Journal", &format!("{} MiB", max / (1024 * 1024)))?;
        }
        // `allocated` above `used` is capacity slack.
        if let Some(m) = &v.memory {
            let total = m.total();
            detail(
                out,
                "Index memory",
                &format!("{} of {} allocated", bytes(total.used), bytes(total.alloc)),
            )?;
            if total.used > 0 {
                let part = |name: &str, p: &MemPair| format!("{name} {}", bytes(p.used));
                detail(
                    out,
                    "",
                    &format!(
                        "{}, {}, {}",
                        part("entries", &m.entries),
                        part("names", &m.arena_raw),
                        part("folded names", &m.arena_folded)
                    ),
                )?;
                detail(
                    out,
                    "",
                    &format!(
                        "{} ({}), {}, {}, {}",
                        part("file IDs", &m.frn_map),
                        m.frn_map_kind,
                        part("name tables", &m.name_tables),
                        part("folders", &m.dir_children),
                        part("permissions", &m.descriptors)
                    ),
                )?;
            }
        }
        // The drift counters are the only outward sign that the index and
        // the volume have diverged, so each shows whenever it is not zero.
        if v.link_reconciles_dropped > 0 {
            detail(
                out,
                "Hard links",
                &format!(
                    "{} not reconciled; some names may be stale",
                    plural(v.link_reconciles_dropped, "change", "changes")
                ),
            )?;
        }
        if v.descriptor_reads_failed > 0 {
            detail(
                out,
                "Permissions",
                &format!(
                    "{} failed since the last scan; such a folder's contents stay hidden from most users until the folder changes",
                    plural(
                        v.descriptor_reads_failed,
                        "folder permission read",
                        "folder permission reads"
                    )
                ),
            )?;
        }
        let drift: Vec<String> = [
            (v.stale_slots, "stale slot", "stale slots"),
            (v.delete_of_unknown, "unknown delete", "unknown deletes"),
            (v.placeholders_created, "placeholder", "placeholders"),
        ]
        .into_iter()
        .filter(|&(n, _, _)| n > 0)
        .map(|(n, one, many)| plural(n, one, many))
        .collect();
        if !drift.is_empty() {
            detail(out, "Drift", &drift.join(", "))?;
        }
    }
    Ok(())
}

/// One labelled line under a volume in the `--status` report.
fn detail(out: &mut impl Write, label: &str, value: &str) -> io::Result<()> {
    writeln!(out, "  {label:<14}{value}")
}

/// `n` and its noun, with thousands grouped: `1 entry`, `2,004,613 entries`.
fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { one } else { many })
}

/// A byte count in the largest decimal unit that keeps it at least 1:
/// `950 B`, `4.2 KB`, `192.0 MB`, `1.3 GB`.
pub fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} KB", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.1} GB", n as f64 / 1e9),
    }
}
