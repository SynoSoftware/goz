//! gozlerd: the gozler indexing daemon.
//!
//! Bootstraps per-volume indexes from the NTFS MFT or a ReFS folder walk,
//! tails the USN change journal, and serves queries over
//! `\\.\pipe\gozler-v1`. `run` starts the live daemon (console by default;
//! `--service` under the SCM); `scan` does a one-shot bootstrap and prints
//! stats; `install`/`uninstall` register the auto-start Windows service;
//! `see-every-file` shows and changes the list of users and groups who see
//! every file.

// Thread-caching allocator; the workspace Cargo.toml note says why.
#[cfg(windows)]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(windows)]
mod access;
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

#[cfg(windows)]
use anyhow::Context;
use clap::{Parser, Subcommand};
#[cfg(windows)]
use gozler_winfs::Sid;

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
    Install {
        /// When no list of users and groups who see every file is stored
        /// yet, store one with this user or group, or an empty one with
        /// `none`. A stored list is kept.
        #[arg(long, value_name = "ACCOUNT")]
        see_every_file: Option<String>,
    },
    /// Stop and remove the gozler service (needs an elevated terminal).
    Uninstall,
    /// Show whether the gozler service is installed and running.
    Status,
    /// Show the users and groups who see every file, or add or remove one.
    ///
    /// Every other user sees only the files in folders they can open.
    /// Adding or removing needs an elevated terminal.
    SeeEveryFile {
        #[command(subcommand)]
        change: Option<ListChange>,
    },
    /// Print the oldest and newest protocol versions this build serves, and
    /// its version, separated by spaces. The window asks the gozlerd.exe
    /// beside it before it offers Use local, without starting a service.
    #[command(hide = true)]
    Protocol,
}

/// A change to the list of users and groups who see every file.
#[derive(Subcommand)]
enum ListChange {
    /// Let a user or group see every file.
    Add {
        /// A user or group, such as PC\ana or Everyone, or a SID, such as
        /// S-1-5-32-545.
        account: String,
    },
    /// Take a user or group off the list.
    Remove {
        /// A user or group, such as PC\ana or Everyone, or a SID, such as
        /// S-1-5-32-545.
        account: String,
    },
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
        Command::Install { see_every_file } => install(see_every_file.as_deref()),
        Command::Uninstall => service::uninstall(),
        Command::Status => service::status(),
        Command::SeeEveryFile { change: None } => show_list(),
        Command::SeeEveryFile {
            change: Some(change),
        } => change_list(&change),
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

/// `install`, storing the list the setup chose. An account that does not
/// resolve stores no list, so the next setup asks again, and the service is
/// installed all the same, because every search works without the list.
#[cfg(windows)]
fn install(see_every_file: Option<&str>) -> anyhow::Result<()> {
    let first_list = match see_every_file {
        None => None,
        Some(none) if none.eq_ignore_ascii_case("none") => Some(Vec::new()),
        Some(account) => match resolve(account) {
            Ok(sid) => Some(vec![sid]),
            Err(error) => {
                eprintln!("gozlerd: {error:#}, so no list of users who see every file is stored");
                None
            }
        },
    };
    service::install(first_list.as_deref())
}

/// The SID of the user or group `account`.
#[cfg(windows)]
fn resolve(account: &str) -> anyhow::Result<Sid> {
    gozler_winfs::account_sid(account)
        .with_context(|| format!("looking up {account}"))?
        .with_context(|| format!("no user or group is named {account}"))
}

/// The account name of `sid`, or its SID string when it has none.
#[cfg(windows)]
fn display_name(sid: &Sid) -> String {
    gozler_winfs::account_name(sid).unwrap_or_else(|| sid.to_string())
}

/// `see-every-file` alone: prints the list, a line the daemon ignores marked
/// as such, and how to show every file to every user. It needs no elevation,
/// because every user may read the list.
#[cfg(windows)]
fn show_list() -> anyhow::Result<()> {
    let lines = access::stored_lines()?.unwrap_or_default();
    let mut everyone = false;
    if lines.is_empty() {
        println!("The list is empty: each user sees the files in folders they can open.");
    } else {
        println!("Users and groups who see every file:");
        for line in &lines {
            match line.parse::<Sid>() {
                Ok(sid) => {
                    everyone |= sid.as_bytes() == Sid::EVERYONE;
                    println!("  {}", display_name(&sid));
                }
                Err(_) => println!("  {line} (ignored: not a SID)"),
            }
        }
    }
    if !everyone {
        println!("To show every file to every user, open a terminal as administrator and run:");
        println!("  gozlerd see-every-file add Everyone");
    }
    Ok(())
}

/// `see-every-file add` and `remove`. Elevation is checked before the name
/// resolves, so a standard user learns at once that the change is not
/// theirs. A line that is not a SID stays as it is.
#[cfg(windows)]
fn change_list(change: &ListChange) -> anyhow::Result<()> {
    if !gozler_winfs::is_elevated() {
        anyhow::bail!("changing who sees every file needs an elevated terminal");
    }
    let (ListChange::Add { account } | ListChange::Remove { account }) = change;
    let sid = resolve(account)?;
    let name = display_name(&sid);
    let mut lines = access::stored_lines()?.unwrap_or_default();
    let on_list = |line: &String| line.parse::<Sid>().is_ok_and(|listed| listed == sid);
    let done = match change {
        ListChange::Add { .. } if lines.iter().any(on_list) => {
            println!("{name} is already on the list; nothing changed.");
            return Ok(());
        }
        ListChange::Add { .. } => {
            lines.push(sid.to_string());
            format!("Added {name}.")
        }
        ListChange::Remove { .. } if !lines.iter().any(on_list) => {
            println!("{name} is not on the list; nothing changed.");
            return Ok(());
        }
        ListChange::Remove { .. } => {
            lines.retain(|line| !on_list(line));
            format!("Removed {name}.")
        }
    };
    access::store_lines(&lines)?;
    println!("{done} The change applies from the next search.");
    Ok(())
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
