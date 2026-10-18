//! gozler's exit codes: one code per outcome a caller handles differently.

use crate::output::style::ColorToken;

/// The outcome of one gozler run, as its process exit code.
///
/// Codes 4 to 8 keep their goz v0.2.0 meanings. The help lists every code with
/// its [`ExitCode::meaning`], in the order of [`ExitCode::ALL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    /// A complete answer, including no matches.
    Complete = 0,
    /// A failure no other code describes.
    Internal = 1,
    /// A volume in scope is not ready (indexing, rescanning, offline or
    /// failed), or the query tests or totals sizes or dates on a volume whose
    /// size and date pass failed or stopped, so the answer may be incomplete;
    /// the results that exist are printed. For `--status`, any volume that is
    /// not ready or whose size and date pass failed or stopped.
    Incomplete = 2,
    /// The pipe server is not the trusted daemon.
    Untrusted = 3,
    /// A missing or bad option value, or a query the parser refuses.
    InvalidInput = 4,
    /// Writing the output or the export file failed.
    OutputFailed = 5,
    /// An option gozler does not know.
    UnknownSwitch = 6,
    /// The daemon's service runs but its pipe is not open yet, the daemon's
    /// reply was malformed, late or refused, or the daemon is older than the
    /// client.
    Communication = 7,
    /// The daemon is not running, and its service is stopped, disabled or not
    /// installed.
    DaemonNotRunning = 8,
    /// Nothing matched, and `-no-result-error` asked for this code.
    NoMatches = 9,
    /// The output is correct, but the limit left out more matches.
    Truncated = 10,
}

impl ExitCode {
    /// Every code, in numeric order.
    pub const ALL: [ExitCode; 11] = [
        ExitCode::Complete,
        ExitCode::Internal,
        ExitCode::Incomplete,
        ExitCode::Untrusted,
        ExitCode::InvalidInput,
        ExitCode::OutputFailed,
        ExitCode::UnknownSwitch,
        ExitCode::Communication,
        ExitCode::DaemonNotRunning,
        ExitCode::NoMatches,
        ExitCode::Truncated,
    ];

    /// The process exit code.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// The color the help shows the code in: an answer that is complete, an
    /// answer with a caveat, or a failure.
    pub fn token(self) -> ColorToken {
        match self {
            ExitCode::Complete => ColorToken::Success,
            ExitCode::Incomplete | ExitCode::NoMatches | ExitCode::Truncated => ColorToken::Warning,
            ExitCode::Internal
            | ExitCode::Untrusted
            | ExitCode::InvalidInput
            | ExitCode::OutputFailed
            | ExitCode::UnknownSwitch
            | ExitCode::Communication
            | ExitCode::DaemonNotRunning => ColorToken::Danger,
        }
    }

    /// What the code tells a script, as the help prints it.
    pub fn meaning(self) -> &'static str {
        match self {
            ExitCode::Complete => "Complete results, including no matches",
            ExitCode::Internal => "Internal error",
            ExitCode::Incomplete => {
                "Results may be incomplete: a volume is not ready or lacks sizes and dates"
            }
            ExitCode::Untrusted => "The pipe server is not the trusted gozlerd",
            ExitCode::InvalidInput => "Invalid input: a missing or bad value, or a bad query",
            ExitCode::OutputFailed => "Could not write the output or the export file",
            ExitCode::UnknownSwitch => "Unknown option",
            ExitCode::Communication => {
                "gozlerd is not ready, did not answer correctly, or is older than gozler"
            }
            ExitCode::DaemonNotRunning => "gozlerd is not running",
            ExitCode::NoMatches => "No matches, with `-no-result-error`",
            ExitCode::Truncated => "The result limit left out some matches",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExitCode;

    /// The help lists the codes in [`ExitCode::ALL`], which the compiler does
    /// not check against the enum, so a code missing from it would be missing
    /// from the help.
    #[test]
    fn all_holds_every_code_in_order() {
        let codes: Vec<u8> = ExitCode::ALL.iter().map(|code| code.code()).collect();
        assert_eq!(codes, (0..=10).collect::<Vec<u8>>());
    }
}
