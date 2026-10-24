//! gozlerd: the gozler indexing daemon.
//!
//! Bootstraps per-volume indexes from the NTFS MFT or a ReFS folder walk,
//! tails the USN change journal, and serves queries over
//! `\\.\pipe\gozler-v1`. `run` starts the live daemon (console by default;
//! `--service` under the SCM); `scan` does a one-shot bootstrap and prints
//! stats; `install`/`uninstall` register the auto-start Windows service.

// Thread-caching allocator; the workspace Cargo.toml note says why.
#[cfg(windows)]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(windows)]
mod bootstrap;
#[cfg(windows)]
mod include;
#[cfg(windows)]
mod pulse;
#[cfg(windows)]
mod result_snapshots;
#[cfg(windows)]
mod run;
#[cfg(windows)]
mod server;
#[cfg(windows)]
mod service;
#[cfg(windows)]
mod supervisor;
#[cfg(windows)]
mod tail;
#[cfg(windows)]
mod volume_state;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "gozlerd",
    version,
    about = "Keeps an index of every fixed NTFS and ReFS volume and answers gozler's searches"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index every fixed NTFS and ReFS volume once, print the statistics and exit.
    ///
    /// No daemon runs. With a query, also search each new index with it.
    Scan {
        /// Optional query to run against each volume after bootstrap.
        query: Option<String>,
    },
    /// Run the daemon in this console.
    Run {
        /// Run attached to the console with Ctrl+C handling (the default).
        #[arg(long)]
        console: bool,
        /// Run under the Windows Service Control Manager. Set automatically in
        /// the service's registered command line; not for interactive use.
        #[arg(long)]
        service: bool,
    },
    /// Register and start the gozler service (needs an elevated terminal).
    Install,
    /// Stop and remove the gozler service (needs an elevated terminal).
    Uninstall,
    /// Show whether the gozler service is installed and running.
    Status,
    /// Print the oldest and newest protocol versions this build serves, and
    /// its version, separated by spaces. The window asks the gozlerd.exe
    /// beside it before it offers Use local, without starting a service.
    #[command(hide = true)]
    Protocol,
}

/// Purge freed memory immediately, because delayed purging needs later
/// allocator activity that an idle daemon may never supply. Explicit
/// allocator settings keep their values.
#[cfg(windows)]
fn tune_allocator() {
    // Bundled mimalloc's mi_option_purge_delay; the sys crate omits its constant.
    const PURGE_DELAY: i32 = 15;
    // SAFETY: the option is valid and startup precedes the daemon's worker threads.
    unsafe {
        libmimalloc_sys::mi_option_set_default(PURGE_DELAY, 0);
    }
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    gozler_core::escompat::set_linked(gozler_winfs::program_linked());
    tune_allocator();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Scan { query } => run_scan(query),
        Command::Run { service: true, .. } => service::run(),
        Command::Run { .. } => run::run_console(),
        Command::Install => service::install(),
        Command::Uninstall => service::uninstall(),
        Command::Status => service::status(),
        Command::Protocol => {
            use gozler_core::proto::{OLDEST_PROTO_VERSION, PROTO_VERSION};
            println!(
                "{OLDEST_PROTO_VERSION} {PROTO_VERSION} {}",
                gozler_core::escompat::version()
            );
            Ok(())
        }
    };
    // One line with every cause, as gozler writes its errors, instead of
    // anyhow's multi-line `Error: ... Caused by:` report.
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gozlerd: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> anyhow::Result<()> {
    let _ = Cli::parse();
    anyhow::bail!(
        "gozlerd requires Windows (it reads NTFS and ReFS volumes and their USN journals)"
    )
}

#[cfg(windows)]
fn run_scan(query: Option<String>) -> anyhow::Result<()> {
    use std::time::Instant;

    if !gozler_winfs::is_elevated() {
        eprintln!("note: gozlerd is not elevated; raw volume access will likely fail.");
        eprintln!("      run from an elevated terminal for a real scan.\n");
    }

    let started = Instant::now();
    let scans = bootstrap::scan_all()?;
    let total_entries: usize = scans
        .iter()
        .map(bootstrap::BootstrappedVolume::entries)
        .sum();

    for s in &scans {
        println!("volume {}", s.mounted().display_prefix());
        println!("  guid          {}", s.guid_path);
        println!("  entries       {}", s.entries());
        println!("  {:<13} {:.2}s", s.filesystem.first_pass(), s.read_secs);
        if let Some(enum_secs) = s.enum_secs {
            println!("  enum          {enum_secs:.2}s (the file-layout pass stopped)");
        }
        match &s.journal {
            bootstrap::JournalSetup::Active(c) => println!(
                "  journal       id={:#x} next_usn={}",
                c.journal_id, c.next_usn
            ),
            bootstrap::JournalSetup::Refused | bootstrap::JournalSetup::Unavailable => {
                println!("  journal       (none: live updates unavailable)")
            }
        }
    }
    println!(
        "\nindexed {} entries across {} volume(s) in {:.2}s",
        total_entries,
        scans.len(),
        started.elapsed().as_secs_f64()
    );

    if let Some(q) = query {
        run_demo_query(&scans, &q)?;
    }
    Ok(())
}

/// Runs a query against each volume's freshly-built index and prints the first
/// results: the end-to-end smoke test (bootstrap → search).
#[cfg(windows)]
fn run_demo_query(scans: &[bootstrap::BootstrappedVolume], query: &str) -> anyhow::Result<()> {
    use gozler_core::query::{CompiledQuery, parse_query, run_query};
    use gozler_core::types::SortSpec;
    use std::time::Instant;

    let parsed = parse_query(query)
        .and_then(|parsed| CompiledQuery::new(&parsed))
        .map_err(|e| anyhow::anyhow!("bad query: {e}"))?;
    println!("\nquery: {query:?}");
    for s in scans {
        let mount = s.mounted().display_prefix();
        let t = Instant::now();
        let out = run_query(
            &s.index,
            mount,
            &parsed,
            None,
            SortSpec::default(),
            Some(20),
        );
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "  {mount}: {} match(es) in {:.1} ms (showing up to 20)",
            out.total, ms
        );
        for row in &out.rows {
            let path = gozler_core::wtf8::to_string_lossy(&row.path);
            let size = match row.size {
                Some(b) => format!("{b} B"),
                None => "?".to_string(),
            };
            let date = match row.mtime {
                Some(ft) => {
                    let ms = gozler_core::output::filetime_to_unix_ms(ft);
                    format!("{ms} ms")
                }
                None => "?".to_string(),
            };
            println!("    [{size:>12}] [{date:>15}] {path}");
        }
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn allocator_purges_by_default_and_preserves_an_explicit_delay() {
        const EXPECTED: &str = "GOZLER_TEST_PURGE_DELAY";
        if let Ok(expected) = std::env::var(EXPECTED) {
            super::tune_allocator();
            // SAFETY: mi_option_purge_delay is a valid option and reading it takes no pointers.
            let actual = unsafe { libmimalloc_sys::mi_option_get(15) };
            assert_eq!(actual, expected.parse::<i32>().unwrap());
            return;
        }

        // Allocator options are process-global and read the environment at startup.
        for (delay, expected) in [(None, "0"), (Some("1000"), "1000")] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "tests::allocator_purges_by_default_and_preserves_an_explicit_delay",
                "--nocapture",
            ]);
            command.env(EXPECTED, expected);
            command.env_remove("MIMALLOC_PURGE_DELAY");
            command.env_remove("MIMALLOC_RESET_DELAY");
            if let Some(delay) = delay {
                command.env("MIMALLOC_PURGE_DELAY", delay);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "delay {delay:?}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
}
