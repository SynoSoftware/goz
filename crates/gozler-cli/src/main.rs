//! gozler: the goz CLI client.
//!
//! Parses arguments with the es.exe-compatible rules from
//! `gozler-core::escompat`, talks to the running daemon over `\\.\pipe\goz-v1`,
//! and writes results as an es-compatible CSV (`-export-csv`), JSON (`--json`),
//! or a plain path list (default).
//!
//! The exit codes are [`gozler_core::escompat::ExitCode`]; every failure is a
//! [`Failure`], which maps to its code in one place.

// Thread-caching allocator: decoding a broad result set allocates a String per
// path (millions of them), which the Windows default heap serializes.
#[cfg(windows)]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(windows)]
mod client;
mod failure;

use failure::{Failure, output};
use gozler_core::escompat::{Command, ExitCode, help, parse_argv, version};
use std::io::Write;

fn main() -> std::process::ExitCode {
    // A panic is an internal failure: exit 1 after the panic message, not
    // Rust's 101, which the exit-code table does not list.
    let report_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report_panic(info);
        std::process::exit(ExitCode::Internal.code().into());
    }));

    let code = match run() {
        Ok(code) => code,
        Err(failure) => {
            failure.report();
            failure.exit_code()
        }
    };
    std::process::ExitCode::from(code.code())
}

fn run() -> Result<ExitCode, Failure> {
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|arg| Failure::ArgumentNotUnicode(arg.to_string_lossy().into_owned()))
        })
        .collect::<Result<Vec<String>, Failure>>()?;
    match parse_argv(&args).map_err(Failure::Argv)? {
        Command::Help => print(&help()),
        Command::Version => print(&format!("{}\n", version())),
        Command::Run(plan) => execute(plan),
    }
}

/// Writes `text` to stdout.
fn print(text: &str) -> Result<ExitCode, Failure> {
    let mut out = std::io::stdout().lock();
    output(
        out.write_all(text.as_bytes()).and_then(|()| out.flush()),
        "stdout",
    )?;
    Ok(ExitCode::Complete)
}

#[cfg(not(windows))]
fn execute(_plan: gozler_core::escompat::EsPlan) -> Result<ExitCode, Failure> {
    // gozlerd runs only on Windows, so there is never a daemon to talk to.
    Err(Failure::DaemonNotRunning)
}

#[cfg(windows)]
use win::execute;

#[cfg(windows)]
mod win {
    use super::client::Client;
    use super::failure::{Failure, output};
    use gozler_core::escompat::{Answer, EsPlan, ExitCode};
    use gozler_core::output::{CsvOptions, CsvRow, filetime_to_unix_ms, write_csv};
    use gozler_core::proto::{DaemonStatus, QueryRequest, QueryResults, VolumeStatus};
    use gozler_core::scope::Scope;
    use gozler_core::types::EsColumn;
    use std::io::Write;
    use std::time::{Duration, Instant};

    /// How often `-timeout` asks gozlerd whether the volumes in scope are
    /// complete.
    const STATUS_POLL: Duration = Duration::from_millis(250);

    pub(crate) fn execute(plan: EsPlan) -> Result<ExitCode, Failure> {
        // Create the export file up front (es semantics): a later daemon-unreachable
        // failure then leaves the observable artifact, a 0-byte file + exit 8.
        let export_file = match &plan.export_csv {
            Some(path) => {
                Some(
                    std::fs::File::create(path).map_err(|error| Failure::ExportCreate {
                        path: path.clone(),
                        error,
                    })?,
                )
            }
            None => None,
        };

        let mut connection = Client::connect(plan.insecure_no_server_check)?;

        if plan.status {
            return status(&mut connection, plan.json);
        }

        let request = build_query(&plan);
        if let Some(timeout) = plan.timeout {
            wait_for_scope(&mut connection, request.scope.as_deref(), timeout)?;
        }

        let out: Box<dyn Write> = match export_file {
            Some(file) => Box::new(file),
            // StdoutLock is a LineWriter (it flushes on every newline), so wrap
            // it in a BufWriter to batch a large result set into a few writes.
            None => Box::new(std::io::BufWriter::new(std::io::stdout().lock())),
        };
        let mut writer = ResultWriter::new(&plan, jiff::tz::TimeZone::system(), out);
        let mut answer = Answer::default();
        connection.query(request, |page| {
            answer.add_page(&page);
            writer.page(page)
        })?;
        writer.finish()?;
        for warning in answer.warnings() {
            eprintln!("gozler: warning: {warning}");
        }
        Ok(answer.exit_code(plan.no_result_error))
    }

    fn build_query(plan: &EsPlan) -> QueryRequest {
        QueryRequest {
            query: plan.query.clone(),
            scope: plan.scope.as_ref().map(|s| absolutize(s)),
            sort: plan.sort,
            offset: 0,
            limit: plan.row_limit(),
            // Always fetch metadata; output decides what to show.
            want_size: true,
            want_mtime: true,
            match_case: plan.match_case,
        }
    }

    /// Resolves a `-path` value against the CWD without touching the filesystem, so
    /// the daemon receives an absolute `X:\...` path to match a mount prefix. On
    /// failure the original value is forwarded, but with a warning: a non-absolute
    /// scope will not match any mount prefix and yields no results, so a silent
    /// fallback would look like "the folder is empty".
    fn absolutize(path: &str) -> String {
        match std::path::absolute(path)
            .ok()
            .and_then(|p| p.to_str().map(str::to_owned))
        {
            Some(abs) => abs,
            None => {
                eprintln!(
                    "gozler: warning: could not resolve '{path}' to an absolute path; scope may not match"
                );
                path.to_owned()
            }
        }
    }

    /// Waits up to `timeout` for every volume that `scope` selects to become
    /// complete. A volume still incomplete then is no failure: the query runs
    /// anyway, and its results say so with exit 2.
    fn wait_for_scope(
        connection: &mut Client,
        scope: Option<&str>,
        timeout: Duration,
    ) -> Result<(), Failure> {
        let deadline = Instant::now() + timeout;
        loop {
            let volumes = connection.status()?.volumes;
            let complete = match Scope::of(scope, volumes.iter().map(VolumeStatus::mounted)) {
                Scope::Everywhere => volumes.iter().all(|v| v.phase.is_complete()),
                Scope::Folder { volume, .. } => volumes[volume].phase.is_complete(),
                Scope::Nowhere => true,
            };
            let left = deadline.saturating_duration_since(Instant::now());
            if complete || left.is_zero() {
                return Ok(());
            }
            std::thread::sleep(left.min(STATUS_POLL));
        }
    }

    /// Writes one result set to stdout or the export file, in the format the
    /// plan asks for, as its pages arrive, so a large export never waits for
    /// or holds the whole set. CSV dates are local time in `tz`.
    struct ResultWriter<'p, W: Write> {
        plan: &'p EsPlan,
        tz: jiff::tz::TimeZone,
        out: W,
        /// Whether a CSV page was written, so the byte order mark and the
        /// header come once.
        csv_started: bool,
        /// The pages so far with `--json`, which prints one object at the end.
        json: Option<QueryResults>,
    }

    impl<'p, W: Write> ResultWriter<'p, W> {
        fn new(plan: &'p EsPlan, tz: jiff::tz::TimeZone, out: W) -> Self {
            Self {
                plan,
                tz,
                out,
                csv_started: false,
                json: None,
            }
        }

        fn page(&mut self, page: QueryResults) -> Result<(), Failure> {
            let written = if self.plan.export_csv.is_some() {
                let csv = render_csv(self.plan, &page, &self.tz, !self.csv_started);
                self.csv_started = true;
                self.out.write_all(&csv)
            } else if self.plan.json {
                match &mut self.json {
                    None => self.json = Some(page),
                    Some(all) => {
                        all.items.extend(page.items);
                        all.more = page.more;
                    }
                }
                Ok(())
            } else {
                // Default: one path per line.
                page.items
                    .iter()
                    .try_for_each(|item| writeln!(self.out, "{}", item.path))
            };
            output(written, self.destination())
        }

        /// Writes what the pages left to write, and flushes.
        fn finish(mut self) -> Result<(), Failure> {
            let written = match self.json.take() {
                Some(all) => {
                    let json = serde_json::to_string_pretty(&all)
                        .map_err(|e| Failure::Internal(format!("JSON encoding failed: {e}")))?;
                    writeln!(self.out, "{json}")
                }
                None => Ok(()),
            };
            output(written.and_then(|()| self.out.flush()), self.destination())
        }

        fn destination(&self) -> &'p str {
            self.plan.export_csv.as_deref().unwrap_or("stdout")
        }
    }

    /// Renders one page of results as es-compatible CSV, with dates in `tz`;
    /// only the `first` page starts with the byte order mark and the header.
    fn render_csv(
        plan: &EsPlan,
        results: &QueryResults,
        tz: &jiff::tz::TimeZone,
        first: bool,
    ) -> Vec<u8> {
        let opts = CsvOptions {
            bom: plan.bom && first,
            header: plan.header && first,
            double_quote: plan.double_quote,
            columns: plan.columns.clone(),
        };

        // Pre-format dates (CsvRow borrows them). Local time, ISO-8601, so the
        // JavaScript consumer's `new Date()` parses it regardless of locale.
        let want_date = plan.columns.contains(&EsColumn::DateModified);
        let dates: Vec<Option<String>> = results
            .items
            .iter()
            .map(|it| {
                it.mtime_ft
                    .filter(|_| want_date)
                    .map(|ft| format_date_local(ft, tz))
            })
            .collect();

        let rows = results.items.iter().zip(&dates).map(|(it, date)| CsvRow {
            path: &it.path,
            size: it.size,
            mtime: date.as_deref(),
            is_dir: it.is_dir,
        });

        let mut out = Vec::new();
        write_csv(&opts, rows, &mut out);
        out
    }

    /// Formats a Windows FILETIME as local `YYYY-MM-DDTHH:MM:SS` in `tz` (resolved
    /// once by the caller). The `TimeZone` handle is cheap (Arc-backed) to clone.
    fn format_date_local(filetime: i64, tz: &jiff::tz::TimeZone) -> String {
        let ms = filetime_to_unix_ms(filetime);
        match jiff::Timestamp::from_millisecond(ms) {
            Ok(ts) => ts
                .to_zoned(tz.clone())
                .strftime("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            Err(_) => String::new(),
        }
    }

    /// `--status`: prints the daemon's state, as JSON with `--json`, and exits
    /// 2 with a warning for each way a volume is not complete.
    fn status(connection: &mut Client, json: bool) -> Result<ExitCode, Failure> {
        let status = connection.status()?;
        let mut out = std::io::stdout().lock();
        let written = if json {
            let text = serde_json::to_string_pretty(&status)
                .map_err(|e| Failure::Internal(format!("JSON encoding failed: {e}")))?;
            writeln!(out, "{text}")
        } else {
            write_status(&status, &mut out)
        };
        output(written.and_then(|()| out.flush()), "stdout")?;
        let warnings = status_warnings(&status);
        for warning in &warnings {
            eprintln!("gozler: warning: {warning}");
        }
        Ok(if warnings.is_empty() {
            ExitCode::Complete
        } else {
            ExitCode::Incomplete
        })
    }

    /// The ways a volume of `status` is not complete, each as the warning
    /// `--status` writes for it.
    fn status_warnings(status: &DaemonStatus) -> Vec<&'static str> {
        let mut warnings = Vec::new();
        if status.volumes.iter().any(|v| !v.phase.is_complete()) {
            warnings.push("some volumes are not fully live");
        }
        if status.volumes.iter().any(|v| v.metadata_pending) {
            warnings.push(
                "the size and date pass failed or stopped on some volumes, \
                 so some sizes and dates are unknown",
            );
        }
        warnings
    }

    /// Writes the `--status` report.
    fn write_status(s: &DaemonStatus, out: &mut impl Write) -> std::io::Result<()> {
        writeln!(out, "gozlerd status: {} volume(s)", s.volumes.len())?;
        for v in &s.volumes {
            let mount = v.mounted().display_prefix();
            writeln!(
                out,
                "  {mount:<8} {:>12} entries  phase={}{}",
                v.entries,
                v.phase, // user-facing Display, not Debug
                if v.metadata_pending {
                    "  (some sizes and dates unknown)"
                } else {
                    ""
                }
            )?;
            // The journal's size is the replay horizon: everything
            // older than what it holds costs a full rescan on restart.
            if let Some(bytes) = v.journal_max_bytes {
                writeln!(out, "           journal max {} MiB", bytes / (1024 * 1024))?;
            }
            // Drift counters, shown only when non-zero. These are the
            // only outward sign that the index and the volume have
            // diverged, so reporting them nowhere made a drifting index
            // indistinguishable from a healthy one.
            if v.link_reconciles_dropped > 0 {
                writeln!(
                    out,
                    "           {} hard-link change(s) not reconciled: some names may be stale",
                    v.link_reconciles_dropped
                )?;
            }
            if v.stale_slots > 0 || v.delete_of_unknown > 0 || v.placeholders_created > 0 {
                writeln!(
                    out,
                    "           drift: {} stale slot(s), {} unknown delete(s), {} placeholder(s)",
                    v.stale_slots, v.delete_of_unknown, v.placeholders_created
                )?;
            }
            // Index memory, component by component. `alloc` differing
            // from `used` is capacity slack held by that component.
            if let Some(m) = &v.memory {
                let mb = |p: &gozler_core::proto::MemPair| {
                    format!("{:.1}/{:.1}", p.used as f64 / 1e6, p.alloc as f64 / 1e6)
                };
                writeln!(
                    out,
                    "           mem MB used/alloc: total {}  entries {}  names {}  folded {}  frn[{}] {}  ntables {}  dirs {}",
                    mb(&m.total()),
                    mb(&m.entries),
                    mb(&m.arena_raw),
                    mb(&m.arena_folded),
                    m.frn_map_kind,
                    mb(&m.frn_map),
                    mb(&m.name_tables),
                    mb(&m.dir_children),
                )?;
            }
        }
        if s.process_private_bytes > 0 {
            writeln!(
                out,
                "  daemon: {:.1} MB private, {:.1} MB working set",
                s.process_private_bytes as f64 / 1e6,
                s.process_working_set as f64 / 1e6,
            )?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod corpus;

    #[cfg(test)]
    mod tests {
        use super::status_warnings;
        use gozler_core::proto::{DaemonStatus, VolumeStatus};
        use gozler_core::types::VolumePhase;

        fn volume(phase: VolumePhase, metadata_pending: bool) -> VolumeStatus {
            VolumeStatus {
                guid: String::new(),
                mounts: vec!["C:\\".to_string()],
                phase,
                entries: 0,
                generation: 0,
                metadata_pending,
                journal_max_bytes: None,
                placeholders_created: 0,
                delete_of_unknown: 0,
                stale_slots: 0,
                link_reconciles_dropped: 0,
                memory: None,
            }
        }

        /// `--status` exits 2, with a warning, when a volume is not live or its
        /// size and date pass failed or stopped, so a script can tell that
        /// sizes and dates are missing; a warning means exit 2.
        #[test]
        fn status_warns_for_each_way_a_volume_is_incomplete() {
            let warnings = |volumes| {
                status_warnings(&DaemonStatus {
                    volumes,
                    volumes_incomplete: false,
                    process_working_set: 0,
                    process_private_bytes: 0,
                })
                .len()
            };
            let live = || volume(VolumePhase::Live, false);
            assert_eq!(
                warnings(vec![live(), volume(VolumePhase::Snapshot, false)]),
                0
            );
            assert_eq!(
                warnings(vec![live(), volume(VolumePhase::Rescanning, false)]),
                1
            );
            assert_eq!(warnings(vec![live(), volume(VolumePhase::Live, true)]), 1);
        }
    }
}
