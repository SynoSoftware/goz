//! gozler: the gozler CLI client.
//!
//! Parses arguments with the es.exe-compatible rules from
//! `gozler-core::escompat`, talks to the running daemon over `\\.\pipe\gozler-v1`
//! through `gozler-client`, and writes results through
//! `gozler_core::output::ResultWriter` as JSON (`--json`), plain text
//! (default), CSV, TSV or TXT.
//!
//! The exit codes are [`gozler_core::escompat::ExitCode`]; every failure is a
//! [`Failure`], which maps to its code in one place.

// Thread-caching allocator: decoding a broad result set allocates a String per
// path (millions of them), which the Windows default heap serializes.
#[cfg(windows)]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod failure;
#[cfg(windows)]
mod overview;

use failure::{Failure, output};
use gozler_core::escompat::{Command, ExitCode, help, parse_argv, version};
use std::io::Write;

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    gozler_core::escompat::set_linked(gozler_winfs::program_linked());
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
        Command::Overview => overview(),
        Command::Help(screen) => print(&help(screen, console_color())),
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
    Err(Failure::NotWindows)
}

/// Whether to color the help and the overview: stdout is a console that
/// draws ANSI colors, and `NO_COLOR` is not set.
#[cfg(windows)]
fn console_color() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && gozler_winfs::enable_console_color()
}

#[cfg(not(windows))]
fn console_color() -> bool {
    false
}

#[cfg(not(windows))]
fn overview() -> Result<ExitCode, Failure> {
    Err(Failure::NotWindows)
}

#[cfg(windows)]
use win::{execute, overview};

#[cfg(windows)]
mod win {
    use super::failure::{Failure, output};
    use super::overview::{Index, Service, render};
    use gozler_client::{CancelToken, Client, ClientError, folder_is_missing};
    use gozler_core::escompat::{Answer, Count, EsPlan, ExitCode, ScopeFolder, build_search};
    use gozler_core::output::{ResultWriter, write_status};
    use gozler_core::proto::{DaemonStatus, PIPE_NAME};
    use gozler_core::query::QueryStart;
    use gozler_core::scope::Coverage;
    use gozler_core::winpath::is_drive_relative;
    use std::io::Write;
    use std::time::{Duration, Instant};

    /// How often `-timeout` asks gozlerd whether the volumes in scope are
    /// complete.
    const STATUS_POLL: Duration = Duration::from_millis(250);

    /// Bare `gozler`: prints the overview, colored as [`super::console_color`]
    /// decides, and exits with the code of the failure that keeps gozlerd
    /// from answering, if any.
    pub(crate) fn overview() -> Result<ExitCode, Failure> {
        let error = match connect(false) {
            Ok(mut connection) => {
                let index = index(&mut connection)?;
                return super::print(&render(&Service::Running(index), super::console_color()));
            }
            Err(error) => error,
        };
        let service = match &error {
            ClientError::NotReady => Service::NotReady,
            ClientError::NotRunning(why) => Service::NotRunning(why),
            _ => return Err(error.into()),
        };
        super::print(&render(&service, super::console_color()))?;
        Ok(Failure::from(error).exit_code())
    }

    /// The size of the index, with the total size of every file.
    fn index(connection: &mut Client) -> Result<Index, Failure> {
        let status = connection.status()?;
        let plan = EsPlan {
            count: Some(Count::TotalSize),
            ..EsPlan::default()
        };
        let start = QueryStart {
            now: jiff::Timestamp::now(),
            time_zone: jiff::tz::TimeZone::system(),
        };
        let request = build_search(&plan, &working_dir()?, &start).map_err(Failure::Query)?;
        let totals = connection.totals(request)?;
        Ok(Index {
            volumes: status.volumes.len(),
            ready: status
                .volumes
                .iter()
                .filter(|v| v.phase.is_complete())
                .count(),
            entries: status.volumes.iter().map(|v| v.entries).sum(),
            size: totals.size,
            some_sizes_unknown: totals.metadata_pending,
        })
    }

    /// Connects to the running daemon, and checks that the elevated daemon
    /// owns the pipe unless `insecure` is set. gozler never cancels a request.
    fn connect(insecure: bool) -> Result<Client, ClientError> {
        Client::connect(PIPE_NAME, !insecure, CancelToken::default())
    }

    pub(crate) fn execute(plan: EsPlan) -> Result<ExitCode, Failure> {
        if plan.status {
            let mut connection = connect(plan.insecure_no_server_check)?;
            return status(&mut connection, plan.json);
        }
        search(plan)
    }

    fn search(mut plan: EsPlan) -> Result<ExitCode, Failure> {
        resolve_drive_relative(&mut plan)?;
        let time_zone = jiff::tz::TimeZone::system();
        let start = QueryStart {
            now: jiff::Timestamp::now(),
            time_zone: time_zone.clone(),
        };
        let request = build_search(&plan, &working_dir()?, &start).map_err(Failure::Query)?;
        if let Some(folder) = request.scope.as_deref().filter(|f| folder_is_missing(f)) {
            return Err(Failure::MissingFolder(folder.to_string()));
        }

        // Create the export file up front (es semantics): a later daemon-unreachable
        // failure then leaves the observable artifact, a 0-byte file + exit 8.
        // A count prints the count, never results, so it writes none.
        let export_file = match &plan.export {
            Some(path) if plan.count.is_none() => Some(std::fs::File::create(path).map_err(
                |error| Failure::ExportCreate {
                    path: path.clone(),
                    error,
                },
            )?),
            _ => None,
        };

        let mut connection = connect(plan.insecure_no_server_check)?;
        if let Some(timeout) = plan.timeout {
            wait_for_scope(&mut connection, request.scope.as_deref(), timeout)?;
        }

        let mut answer = Answer::new(&plan, &request.query);
        if let Some(count) = plan.count {
            let totals = connection.totals(request)?;
            answer.add_totals(&totals);
            let value = match count {
                Count::Results => totals.count,
                Count::TotalSize => totals.size,
            };
            let mut out = std::io::stdout().lock();
            let written = writeln!(out, "{value}").and_then(|()| out.flush());
            output(written, "stdout")?;
        } else {
            let out: Box<dyn Write> = match export_file {
                Some(file) => Box::new(file),
                // StdoutLock is a LineWriter (it flushes on every newline), so
                // wrap it in a BufWriter to batch a large result set into a few
                // writes.
                None => Box::new(std::io::BufWriter::new(std::io::stdout().lock())),
            };
            let destination = plan.export.as_deref().unwrap_or("stdout");
            let mut writer = ResultWriter::new(plan.result_format(), time_zone, out);
            connection.query(request, |page| {
                answer.add_page(&page);
                output(writer.page(page), destination)
            })?;
            output(writer.finish(), destination)?;
        }
        for warning in answer.warnings() {
            eprintln!("gozler: warning: {warning}");
        }
        Ok(answer.exit_code(plan.no_result_error))
    }

    /// The working directory, which a relative `-path` or path term is below.
    /// A folder name that is not valid Unicode is replaced lossily, so a
    /// relative path below it names no indexed folder; other searches are
    /// unaffected.
    fn working_dir() -> Result<String, Failure> {
        let dir = std::env::current_dir()
            .map_err(|e| Failure::Internal(format!("cannot read the working directory: {e}")))?;
        Ok(dir.to_string_lossy().into_owned())
    }

    /// Resolves a drive-relative folder switch such as `-path D:x` against
    /// the current folder of that drive, which only Windows knows;
    /// [`build_search`] resolves every other form.
    fn resolve_drive_relative(plan: &mut EsPlan) -> Result<(), Failure> {
        let folders = plan
            .scope
            .as_mut()
            .map(ScopeFolder::path_mut)
            .into_iter()
            .chain(plan.parent.as_mut());
        for folder in folders.filter(|folder| is_drive_relative(folder)) {
            let absolute = std::path::absolute(&*folder)
                .map_err(|e| Failure::Internal(format!("cannot resolve {folder}: {e}")))?;
            *folder = absolute.to_string_lossy().into_owned();
        }
        Ok(())
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
            let complete = Coverage::of(&volumes, scope, false).incomplete().is_none();
            let left = deadline.saturating_duration_since(Instant::now());
            if complete || left.is_zero() {
                return Ok(());
            }
            std::thread::sleep(left.min(STATUS_POLL));
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
        // Every volume, and unknown sizes and dates whatever a search reads,
        // because the report is about the index, not one search.
        let every_volume = Coverage::of(&status.volumes, None, true);
        let mut warnings = Vec::new();
        if every_volume.incomplete().is_some() {
            warnings.push("some volumes are not ready; the phase above shows why");
        }
        if every_volume.sizes_or_dates_unknown().is_some() {
            warnings.push("gozler could not read the sizes and dates on some volumes");
        }
        warnings
    }

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

        /// `--status` warns, and so exits 2, when a volume is not complete (a
        /// snapshot is) or its size and date pass did not finish, so a script
        /// can tell that results or sizes and dates are missing.
        #[test]
        fn status_warns_for_each_way_a_volume_is_incomplete() {
            let warnings = |volumes| {
                status_warnings(&DaemonStatus {
                    volumes,
                    volumes_incomplete: false,
                    process_working_set: 0,
                    process_private_bytes: 0,
                    include: None,
                    left_out: Vec::new(),
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
