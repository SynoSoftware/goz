//! Check for updates (DESIGN.md: The General page): which release GitHub names
//! as the latest, and whether it is newer than this window.

use serde::Deserialize;

/// Where the check asks, and the page a newer release's status text opens.
pub(crate) const LATEST_RELEASE_HOST: &str = "api.github.com";
pub(crate) const LATEST_RELEASE_PATH: &str = "/repos/SynoSoftware/goz/releases/latest";
pub(crate) const RELEASES_PAGE: &str = "https://github.com/SynoSoftware/goz/releases/latest";

/// How long after a check the next start checks again.
pub(crate) const CHECK_INTERVAL_SECONDS: i64 = 24 * 60 * 60;

/// The version GitHub's latest release names, such as `0.4.0` for the tag
/// `v0.4.0`, when it is newer than `current`; `None` for a reply that is not
/// a release, because no update is worth a message (DESIGN.md: The General
/// page).
pub(crate) fn newer_release(reply: &str, current: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }
    let release: Release = serde_json::from_str(reply).ok()?;
    let version = release.tag_name.trim_start_matches('v');
    (numbers(version)? > numbers(current)?).then(|| version.to_owned())
}

/// A version's numbers, so 0.10.0 is newer than 0.9.0; a pre-release
/// suffix such as `-rc1` counts as its release.
fn numbers(version: &str) -> Option<Vec<u64>> {
    version
        .split(['-', '+'])
        .next()?
        .split('.')
        .map(|n| n.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Comparing the tags as text would call 0.9.0 newer than 0.10.0, and
    /// the status bar would offer an older release.
    #[test]
    fn a_release_is_newer_by_its_numbers() {
        let reply = |tag: &str| format!(r#"{{"tag_name": "{tag}", "name": "Gozler"}}"#);
        assert_eq!(
            newer_release(&reply("v0.10.0"), "0.9.0"),
            Some("0.10.0".into())
        );
        assert_eq!(newer_release(&reply("v0.3.3"), "0.3.3"), None);
        assert_eq!(newer_release(&reply("v0.3.2"), "0.3.3"), None);
        assert_eq!(newer_release(r#"{"message": "Not Found"}"#, "0.3.3"), None);
    }
}
