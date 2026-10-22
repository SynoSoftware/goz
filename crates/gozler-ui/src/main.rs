//! gozler-ui: the gozler window, a native Win32 search window over gozlerd
//! (DESIGN.md: The window).
//!
//! Three modules divide the work. [`core`] decides everything and holds no
//! Win32 type, so its tests run on every OS. `win` wraps the Win32 calls and
//! holds every `unsafe` block. `app` turns window messages into calls on
//! both, in safe Rust.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod core;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod win;

#[cfg(all(test, windows))]
mod timing;

#[cfg(windows)]
fn main() {
    gozler_core::escompat::set_linked(gozler_winfs::program_linked());
    app::run();
}

/// The window exists only on Windows; elsewhere the crate builds for the
/// tests of [`core`].
#[cfg(not(windows))]
fn main() {
    std::process::exit(1);
}
