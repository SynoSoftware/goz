//! Check for updates (DESIGN.md: The General page): which release GitHub names
//! as the latest, and whether it is newer or older than this window; and the
//! setup that Update, Downgrade and Install run (DESIGN.md: Service fixes).

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use serde::Deserialize;

/// Where the check asks.
pub(crate) const LATEST_RELEASE_HOST: &str = "api.github.com";
pub(crate) const LATEST_RELEASE_PATH: &str = "/repos/SynoSoftware/goz/releases/latest";

/// How long after a check the next start checks again.
pub(crate) const CHECK_INTERVAL_SECONDS: i64 = 24 * 60 * 60;

/// The version GitHub's latest release names, such as `0.4.0` for the tag
/// `v0.4.0`, when it is newer than `current`; `None` for a reply that is not
/// a release, because no update is worth a message (DESIGN.md: The General
/// page).
pub(crate) fn newer_release(reply: &str, current: &str) -> Option<String> {
    let release: Release = serde_json::from_str(reply).ok()?;
    let version = release.version();
    (numbers(version)? > numbers(current)?).then(|| version.to_owned())
}

/// The most bytes a setup may have, far more than Gozler's, so a wrong reply
/// cannot fill the disk.
const MOST_SETUP_BYTES: u64 = 1 << 30;

/// The latest release's setup, `gozler-<version>-x64-Setup.exe`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Setup {
    pub(crate) version: String,
    /// The release is older than this window, so the setup downgrades it.
    pub(crate) older: bool,
    pub(crate) name: String,
    /// Where the setup downloads from, always over HTTPS.
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) sha256: [u8; 32],
}

/// A release in GitHub's reply.
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    /// When GitHub published it, such as `2026-10-18T09:12:44Z`.
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

impl Release {
    fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }

    /// Whether the release is older than this window of version `current`:
    /// its numbers are lower, or they are the same and it was published
    /// before this local build was built, so its setup would put the window
    /// and the service back (DESIGN.md: Service fixes).
    fn is_older(&self, current: &str) -> bool {
        let (Some(release), Some(window)) = (numbers(self.version()), numbers(current)) else {
            return false;
        };
        release < window
            || release == window
                && self
                    .published_at
                    .as_deref()
                    .and_then(|published| published.parse::<Timestamp>().ok())
                    .zip(built(current))
                    .is_some_and(|(published, built)| published < built)
    }
}

/// Whether GitHub's latest release in `reply` is older than this window of
/// version `current`; `None` for a reply that is not a release.
pub(crate) fn latest_is_older(reply: &str, current: &str) -> Option<bool> {
    let release: Release = serde_json::from_str(reply).ok()?;
    Some(release.is_older(current))
}

/// The setup of the latest release in GitHub's `reply`, which says whether
/// it is older than this window of version `current`. `None` when the reply
/// is not a release with a setup whose SHA-256 GitHub gives, because a
/// setup that cannot be checked is not run.
pub(crate) fn setup_to_install(reply: &str, current: &str) -> Option<Setup> {
    let release: Release = serde_json::from_str(reply).ok()?;
    let version = release.version().to_owned();
    let older = release.is_older(current);
    let name = format!("gozler-{version}-x64-Setup.exe");
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == name)
        .filter(|asset| asset.size <= MOST_SETUP_BYTES)?;
    let (host, path) = asset
        .browser_download_url
        .strip_prefix("https://")
        .and_then(|address| address.split_once('/'))?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .and_then(hex_bytes)?;
    Some(Setup {
        version,
        older,
        name,
        host: host.to_owned(),
        path: format!("/{path}"),
        size: asset.size,
        sha256,
    })
}

/// The 32 bytes that 64 hexadecimal digits write.
fn hex_bytes(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}

/// A version's numbers, so 0.10.0 is newer than 0.9.0; a pre-release
/// suffix such as `-rc1` counts as its release, and a local build's time,
/// as in `0.3.0 (2026-10-19 21:10)`, is left out.
fn numbers(version: &str) -> Option<Vec<u64>> {
    version
        .split(['-', '+', ' '])
        .next()?
        .split('.')
        .map(|n| n.parse().ok())
        .collect()
}

/// When the local build of version `current`, such as
/// `0.3.0 (2026-10-19 21:10)`, was built, in this computer's time zone, as
/// `gozler_core::escompat::version` writes it; `None` for a release build.
fn built(current: &str) -> Option<Timestamp> {
    let time = current.split_once(" (")?.1.strip_suffix(')')?;
    let time = DateTime::strptime("%Y-%m-%d %H:%M", time).ok()?;
    Some(time.to_zoned(TimeZone::system()).ok()?.timestamp())
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

    /// A setup that cannot be checked must never run, and one older than
    /// this window must be offered as a downgrade, not as an update.
    #[test]
    fn only_a_checked_setup_installs_and_an_older_one_downgrades() {
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let reply = |tag: &str, digest: &str| {
            format!(
                r#"{{"tag_name": "{tag}", "assets": [{{"name": "gozler-0.4.0-x64-Setup.exe",
                "size": 3, "digest": {digest},
                "browser_download_url": "https://github.com/s/gozler-0.4.0-x64-Setup.exe"}}]}}"#
            )
        };
        let setup = setup_to_install(&reply("v0.4.0", &format!(r#""sha256:{digest}""#)), "0.4.0")
            .expect("a setup");
        assert!(!setup.older, "a release of the window's version repairs it");
        assert_eq!(setup.sha256[..3], [0xba, 0x78, 0x16]);
        assert_eq!(
            (setup.host.as_str(), setup.path.as_str()),
            ("github.com", "/s/gozler-0.4.0-x64-Setup.exe")
        );
        assert_eq!(setup_to_install(&reply("v0.4.0", "null"), "0.3.3"), None);
        assert_eq!(
            setup_to_install(&reply("v0.4.0", r#""sha256:00""#), "0.3.3"),
            None
        );
        let older = setup_to_install(&reply("v0.4.0", &format!(r#""sha256:{digest}""#)), "0.5.0");
        assert!(older.expect("an older release's setup").older);
    }

    /// A release of the window's version that was published before this
    /// local build would put the build's changes back, so it is older; a
    /// release build has no build time and is never older than its own
    /// release.
    #[test]
    fn a_release_published_before_this_local_build_is_older() {
        let reply = |tag: &str, published: &str| {
            format!(r#"{{"tag_name": "{tag}", "published_at": "{published}"}}"#)
        };
        let local = "0.3.0 (2026-10-19 21:10)";
        let before = reply("v0.3.0", "2026-01-01T00:00:00Z");
        assert_eq!(latest_is_older(&before, local), Some(true));
        let after = reply("v0.3.0", "2030-01-01T00:00:00Z");
        assert_eq!(latest_is_older(&after, local), Some(false));
        assert_eq!(latest_is_older(&before, "0.3.0"), Some(false));
        let lower = reply("v0.2.9", "2030-01-01T00:00:00Z");
        assert_eq!(latest_is_older(&lower, "0.3.0"), Some(true));
    }
}
