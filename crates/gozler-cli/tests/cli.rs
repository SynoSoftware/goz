//! End-to-end exit-code contract for the `gozler` binary.
//!
//! `gozler_core::escompat` already unit-tests the argv rules themselves; what no
//! unit test can observe is whether `main` actually maps an `EsFatal` onto the
//! process exit code. These spawn the real binary and check that wiring.
//!
//! Only cases that need no running daemon live here. Anything past
//! `Client::connect` depends on whether `\\.\pipe\gozler-v1` happens to be live
//! on the machine running the suite, which would pass on CI and flake on a
//! developer box with `gozlerd` running. The cases below all end before the
//! first connect attempt, so they are hermetic on any OS.

use std::process::{Command, Output};

fn gozler<const N: usize>(args: [&str; N]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gozler"))
        .args(args)
        .output()
        .expect("failed to spawn the gozler binary")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// es exit code 6. Parsing rejects the option before any daemon contact.
#[test]
fn unknown_option_exits_6() {
    let out = gozler(["-bogus-option", "foo"]);
    assert_eq!(out.status.code(), Some(6), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("unknown option: -bogus-option"),
        "stderr: {}",
        stderr(&out)
    );
}

/// es exit code 4. `-path` consumes a following value; there is none here.
#[test]
fn missing_option_value_exits_4() {
    let out = gozler(["-path"]);
    assert_eq!(out.status.code(), Some(4), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("-path needs DIR after it"),
        "stderr: {}",
        stderr(&out)
    );
}

/// es exit code 5. The export file is created up front, so an uncreatable path
/// fails before `Client::connect` and needs no daemon.
///
/// Windows-only: on other targets `execute` short-circuits to exit 8 without
/// ever reading `-export-csv`, so this would fail on the CI ubuntu leg.
#[cfg(windows)]
#[test]
fn unwritable_export_path_exits_5() {
    // A directory that cannot exist, made unique so a stale tree cannot make
    // this pass for the wrong reason (a creatable path would reach connect).
    let missing = std::env::temp_dir()
        .join(format!("gozler-absent-{}", std::process::id()))
        .join("out.csv");
    assert!(!missing.exists());

    let out = gozler(["-export-csv", &missing.to_string_lossy(), "foo"]);
    assert_eq!(out.status.code(), Some(5), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("cannot create"),
        "stderr: {}",
        stderr(&out)
    );
}

/// A missing `-path` folder exits 4 before any daemon contact, where es
/// answers nothing with exit 0 and an agent reads that as "no such file".
#[cfg(windows)]
#[test]
fn a_missing_path_folder_exits_4() {
    let missing = std::env::temp_dir().join(format!("gozler-absent-{}", std::process::id()));
    assert!(!missing.exists());
    let missing = missing.to_string_lossy();

    let out = gozler(["-path", &missing, "foo"]);
    assert_eq!(out.status.code(), Some(4), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("{missing} does not exist")),
        "stderr: {}",
        stderr(&out)
    );
}

/// Bug 7: `-help` prints the help to stdout and exits 0, before any daemon
/// contact.
#[test]
fn help_exits_0() {
    let out = gozler(["-help"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(!out.stdout.is_empty(), "the help goes to stdout");
}

/// An argument that is not valid Unicode, such as an unpaired surrogate from
/// a shell glob, exits 4 instead of panicking. Arguments are read before any
/// daemon contact.
///
/// Windows-only: a lone surrogate is a Windows argument; other targets
/// spell arguments as bytes.
#[cfg(windows)]
#[test]
fn an_argument_that_is_not_unicode_exits_4() {
    use std::os::windows::ffi::OsStringExt;
    let lone_surrogate = std::ffi::OsString::from_wide(&[u16::from(b'a'), 0xD800]);
    let out = Command::new(env!("CARGO_BIN_EXE_gozler"))
        .arg(lone_surrogate)
        .output()
        .expect("failed to spawn the gozler binary");
    assert_eq!(out.status.code(), Some(4), "stderr: {}", stderr(&out));
}
