//! Sets `cfg(gozler_release)` for a release build, which the release
//! workflow marks with `GOZLER_RELEASE=1`, so `escompat::version` leaves out
//! the time the program was linked.

fn main() {
    println!("cargo::rerun-if-env-changed=GOZLER_RELEASE");
    println!("cargo::rustc-check-cfg=cfg(gozler_release)");
    if std::env::var_os("GOZLER_RELEASE").is_some_and(|v| v == "1") {
        println!("cargo::rustc-cfg=gozler_release");
    }
}
