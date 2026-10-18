//! gozler's exit codes: one code per outcome a caller handles differently.

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
    /// A volume in scope is indexing, rescanning or offline, so the answer may
    /// be incomplete. The results that exist are printed.
    Incomplete = 2,
    /// The pipe server is not the trusted daemon.
    Untrusted = 3,
    /// A missing or bad switch value, or a query the parser refuses.
    InvalidInput = 4,
    /// Writing the output or the export file failed.
    OutputFailed = 5,
    /// A switch gozler does not know.
    UnknownSwitch = 6,
    /// The daemon's reply was malformed, late or refused.
    Communication = 7,
    /// The daemon is not running.
    DaemonNotRunning = 8,
}

impl ExitCode {
    /// Every code, in numeric order.
    pub const ALL: [ExitCode; 9] = [
        ExitCode::Complete,
        ExitCode::Internal,
        ExitCode::Incomplete,
        ExitCode::Untrusted,
        ExitCode::InvalidInput,
        ExitCode::OutputFailed,
        ExitCode::UnknownSwitch,
        ExitCode::Communication,
        ExitCode::DaemonNotRunning,
    ];

    /// The process exit code.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// What the code tells a script, as the help prints it.
    pub fn meaning(self) -> &'static str {
        match self {
            ExitCode::Complete => "complete answer, including no matches",
            ExitCode::Internal => "internal failure",
            ExitCode::Incomplete => {
                "incomplete: a volume in scope is indexing, rescanning or offline; \
                 the results that exist are printed"
            }
            ExitCode::Untrusted => {
                "the pipe server is not the trusted gozlerd \
                 (--insecure-no-server-check skips the check)"
            }
            ExitCode::InvalidInput => {
                "invalid input: a missing or bad switch value, or a bad query"
            }
            ExitCode::OutputFailed => "writing the output or the export file failed",
            ExitCode::UnknownSwitch => "unknown switch",
            ExitCode::Communication => "communication with gozlerd failed, or no reply in time",
            ExitCode::DaemonNotRunning => "gozlerd is not running",
        }
    }

    /// The code a run exits with when both `self` and `other` apply to it: a
    /// failure (1 and 3 to 8) before an incomplete answer (2), and that before
    /// a complete one (0). Of two failures, `self` wins: the first one stops
    /// the run.
    pub fn most_serious(self, other: ExitCode) -> ExitCode {
        if other.severity() > self.severity() {
            other
        } else {
            self
        }
    }

    fn severity(self) -> u8 {
        match self {
            ExitCode::Complete => 0,
            ExitCode::Incomplete => 2,
            ExitCode::Internal
            | ExitCode::Untrusted
            | ExitCode::InvalidInput
            | ExitCode::OutputFailed
            | ExitCode::UnknownSwitch
            | ExitCode::Communication
            | ExitCode::DaemonNotRunning => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExitCode;

    /// The precedence rule of the exit-code table: a failure wins over an
    /// incomplete answer, which wins over a complete one, whichever comes
    /// first.
    #[test]
    fn the_more_serious_outcome_wins() {
        let cases = [
            (ExitCode::Complete, ExitCode::Incomplete, 2),
            (ExitCode::Incomplete, ExitCode::Complete, 2),
            (ExitCode::Incomplete, ExitCode::OutputFailed, 5),
            (ExitCode::OutputFailed, ExitCode::Incomplete, 5),
            (ExitCode::Internal, ExitCode::Incomplete, 1),
            (ExitCode::Complete, ExitCode::Untrusted, 3),
            (ExitCode::Communication, ExitCode::OutputFailed, 7),
        ];
        for (first, second, wins) in cases {
            assert_eq!(
                first.most_serious(second).code(),
                wins,
                "{first:?} then {second:?}"
            );
        }
    }

    /// The help lists the codes in [`ExitCode::ALL`], which the compiler does
    /// not check against the enum, so a code missing from it would be missing
    /// from the help.
    #[test]
    fn all_holds_every_code_in_order() {
        let codes: Vec<u8> = ExitCode::ALL.iter().map(|code| code.code()).collect();
        assert_eq!(codes, (0..=8).collect::<Vec<u8>>());
    }
}
