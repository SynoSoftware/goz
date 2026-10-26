//! Who is asking, and what they see: for each request, gozlerd reads the
//! pipe client's token and decides whether to refuse the request, show every
//! entry, or show only the entries of the folders the caller may list.
//!
//! The decision is made again for each frame, because the pipe gives the
//! token of the client that sent the last message, and because a change to
//! the stored list must apply at the next search without a restart.

use std::os::windows::io::BorrowedHandle;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use gozler_core::include::{SEE_EVERY_FILE_VALUE, SETTINGS_KEY};
use gozler_core::query::Viewer;
use gozler_winfs::{
    ClientToken, Sid, WinError, machine_strings, pipe_client_token, set_machine_strings,
};

/// What the daemon decided for one request, before it runs.
pub(crate) enum Access {
    /// Searches and Include counts are refused; requests that show no name
    /// still answer.
    Refused(Refusal),
    Sees(Visibility),
}

/// Why gozlerd cannot tell who sent a request.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The client opened the pipe anonymously.
    Anonymous,
    /// Windows gave no token for the client, or the token did not read.
    Unidentified(WinError),
}

impl Refusal {
    /// The message a refused request gets.
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Refusal::Anonymous => {
                "gozlerd could not tell which user is searching; \
                 open the pipe at the identification level"
            }
            Refusal::Unidentified(_) => "gozlerd could not tell which user is searching",
        }
    }
}

/// Whether the caller may list a folder, given its descriptor bytes. Named
/// because clippy's `type_complexity` refuses the boxed form inline.
pub(crate) type MayList = dyn Fn(&[u8]) -> bool + Send;

/// What an identified caller sees.
pub(crate) enum Visibility {
    /// Every entry, with folder sizes.
    Everything,
    /// The entries of the folders this check lets the caller list.
    Filtered(Box<MayList>),
}

impl Visibility {
    /// The caller's own token decides which folders they may list. A check
    /// that fails hides the folder, because hiding is the safe answer.
    fn of_token(token: ClientToken) -> Visibility {
        Visibility::Filtered(Box::new(move |descriptor: &[u8]| {
            token.may_list(descriptor).unwrap_or_else(|error| {
                tracing::debug!(%error, "folder access check failed; its entries stay hidden");
                false
            })
        }))
    }

    /// What a search reads with this visibility.
    pub(crate) fn viewer(&self) -> Viewer<'_> {
        match self {
            Visibility::Everything => Viewer::Everything,
            Visibility::Filtered(may_list) => Viewer::Filtered(&**may_list),
        }
    }
}

/// What the client of `pipe` may see, for the last message read from it.
pub(crate) fn of_pipe_client(pipe: BorrowedHandle<'_>) -> Access {
    let token = match pipe_client_token(pipe) {
        Ok(Some(token)) => token,
        Ok(None) => return Access::Refused(Refusal::Anonymous),
        Err(error) => return Access::Refused(Refusal::Unidentified(error)),
    };
    match token.enabled_sids() {
        Err(error) => Access::Refused(Refusal::Unidentified(error)),
        Ok(sids) if sees_everything(&sids, &stored_sids()) => Access::Sees(Visibility::Everything),
        // A caller who is neither SYSTEM nor listed, or a restricted token,
        // which has no enabled SIDs.
        Ok(_) => Access::Sees(Visibility::of_token(token)),
    }
}

/// Whether the caller's enabled SIDs, user first, hold SYSTEM or meet the
/// stored list of users and groups who see every file.
fn sees_everything(caller: &[Sid], list: &[Sid]) -> bool {
    caller
        .iter()
        .any(|sid| sid.as_bytes() == Sid::SYSTEM || list.contains(sid))
}

/// The lines of the stored list of users and groups who see every file, one
/// SID string per line of the `SeeEveryFile` value, which only administrators
/// and the system can write; `None` when no list is stored.
pub(crate) fn stored_lines() -> anyhow::Result<Option<Vec<String>>> {
    machine_strings(SETTINGS_KEY, SEE_EVERY_FILE_VALUE)
        .context("reading the list of users and groups who see every file")
}

/// Stores `lines` as the list of users and groups who see every file.
pub(crate) fn store_lines(lines: &[String]) -> anyhow::Result<()> {
    set_machine_strings(SETTINGS_KEY, SEE_EVERY_FILE_VALUE, lines)
        .context("storing the list of users and groups who see every file")
}

/// The SIDs on the stored list of users and groups who see every file, as it
/// is stored now.
///
/// A missing value is an empty list, and so is one that does not read,
/// because then only SYSTEM sees every file and every other caller is
/// filtered by their own token. The read is made for each request, so its
/// failure is logged once per process, not for each request. A line that is
/// not a SID string is skipped without a log line, for the same reason;
/// `gozlerd see-every-file` shows it.
fn stored_sids() -> Vec<Sid> {
    static UNREADABLE_LOGGED: AtomicBool = AtomicBool::new(false);
    let lines = stored_lines().unwrap_or_else(|error| {
        if !UNREADABLE_LOGGED.swap(true, Ordering::Relaxed) {
            tracing::warn!(error = %format!("{error:#}"), "the list counts as empty");
        }
        None
    });
    (lines.unwrap_or_default().iter())
        .filter_map(|line| line.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(s: &str) -> Sid {
        s.parse().expect("a SID string")
    }

    /// Who sees every file. Without it, a wrong match turns the filter
    /// off for users the administrator did not list, or leaves it on for one
    /// they did.
    #[test]
    fn sees_everything_takes_system_and_the_listed_sids() {
        let user = sid("S-1-5-21-1-2-3-1001");
        let users = sid("S-1-5-32-545");
        let administrators = sid("S-1-5-32-544");
        let everyone = sid("S-1-1-0");
        let caller = [user.clone(), everyone.clone(), users.clone()];

        assert!(sees_everything(&[sid("S-1-5-18")], &[]), "SYSTEM");
        assert!(
            sees_everything(&caller, std::slice::from_ref(&user)),
            "the user listed"
        );
        assert!(sees_everything(&caller, &[users]), "a group listed");
        assert!(sees_everything(&caller, &[everyone]), "Everyone listed");

        let admin = [user, administrators];
        assert!(
            !sees_everything(&admin, &[]),
            "an administrator, empty list"
        );
        assert!(
            !sees_everything(&admin, &[sid("S-1-5-32-551")]),
            "a group the caller does not hold"
        );
    }
}
