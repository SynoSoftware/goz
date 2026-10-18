//! Sets `GOZLER_VERSION`, the version gozler prints. A release build, which
//! the release workflow marks with `GOZLER_RELEASE=1`, prints the Cargo
//! version. Every other build appends the local date and time it was built,
//! as `0.2.1 (2026-10-18 11:51)`, so a build from a working tree cannot pass
//! for the release of the same version.
//!
//! The script reruns when the version, `src` or `GOZLER_RELEASE` changes, so
//! the time is when this crate last changed and was built. Each
//! `installer\Build.ps1` run raises the version, so each setup carries the
//! time of its own build.

fn main() {
    println!("cargo::rerun-if-changed=src");
    println!("cargo::rerun-if-env-changed=GOZLER_RELEASE");
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let release = std::env::var_os("GOZLER_RELEASE").is_some_and(|v| v == "1");
    let version = if release {
        version
    } else {
        format!(
            "{version} ({})",
            jiff::Zoned::now().strftime("%Y-%m-%d %H:%M")
        )
    };
    println!("cargo::rustc-env=GOZLER_VERSION={version}");
}
