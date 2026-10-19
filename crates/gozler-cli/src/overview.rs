//! The overview that bare `gozler` prints: the version, what gozler does, the
//! state of the gozler service with what starts it, and the commands to begin
//! with. Each row starts with a label in one column, so a reader finds a row
//! without reading the others.

use gozler_core::escompat::version;
use gozler_core::output::style::{ColorToken, paint};
use gozler_core::output::table::grouped;

use crate::failure::{problem, repair_lines, state};
use crate::win::bytes;
use gozler_client::NotRunning;

/// What gozlerd reports about the index.
pub(crate) struct Index {
    pub(crate) volumes: usize,
    pub(crate) ready: usize,
    pub(crate) entries: u64,
    /// The total size of the files, or `None` when sizes on a volume could
    /// not be read, because a short total would read as the real one.
    pub(crate) size: Option<u64>,
}

/// The state of the gozler service.
pub(crate) enum Service<'a> {
    Running(Index),
    /// Running or changing state, without its pipe yet.
    NotReady,
    NotRunning(&'a NotRunning),
}

const TAGLINE: &str = "Find any file on your NTFS and ReFS drives by name, in milliseconds.";

/// The width of the label column, so every value starts at one column.
const LABEL: usize = 11;

/// The overview text, with ANSI colors when `color` is set.
pub(crate) fn render(service: &Service, color: bool) -> String {
    let mut out = Overview {
        text: String::new(),
        color,
    };
    out.paint(ColorToken::Strong, &format!("gozler {}", version()));
    out.text.push('\n');
    out.text.push_str(TAGLINE);
    out.text.push_str("\n\n");

    out.label("Service");
    match service {
        Service::Running(index) => {
            let found = format!("{} files and folders", grouped(index.entries));
            if index.ready < index.volumes {
                out.paint(ColorToken::Warning, "Indexing");
                let volumes = if index.volumes == 1 {
                    "volume"
                } else {
                    "volumes"
                };
                out.text.push_str(&format!(
                    " · {} of {} {volumes} · {found} so far\n",
                    index.ready, index.volumes
                ));
            } else {
                out.paint(ColorToken::Success, "Running");
                let size = match index.size {
                    Some(size) => bytes(size),
                    None => "sizes not read yet".to_string(),
                };
                out.text.push_str(&format!(" · {found} · {size}\n"));
            }
        }
        Service::NotReady => {
            out.paint(ColorToken::Warning, "Not ready");
            out.text.push('\n');
            out.label("Fix");
            out.text.push_str("Run gozler again in a few seconds.\n");
        }
        Service::NotRunning(why) => {
            out.paint(ColorToken::Danger, state(why));
            out.text.push('\n');
            match why.repair() {
                Some(repair) => {
                    let (how, lines) = repair_lines(&repair);
                    out.label("Fix");
                    out.text.push_str(how);
                    out.text.push('\n');
                    for line in &lines {
                        out.command(line);
                    }
                }
                None => {
                    out.label("");
                    out.text.push_str(&problem(why));
                    out.text.push('\n');
                }
            }
        }
    }

    out.text.push('\n');
    out.label("Search");
    out.paint(
        ColorToken::Primary,
        &format!("{:<21}", "gozler readme*.txt"),
    );
    out.text
        .push_str("Find .txt files that start with readme\n");
    out.label("Help");
    out.paint(ColorToken::Primary, "gozler -h");
    out.text.push('\n');
    out.label("More");
    out.paint(ColorToken::Primary, "gozler --help");
    // A blank line before the shell's next prompt.
    out.text.push_str("\n\n");
    out.text
}

struct Overview {
    text: String,
    color: bool,
}

impl Overview {
    fn paint(&mut self, token: ColorToken, text: &str) {
        paint(&mut self.text, token, text, self.color);
    }

    fn label(&mut self, label: &str) {
        self.paint(
            ColorToken::Muted,
            &format!("{:<LABEL$}", format!("  {label}")),
        );
    }

    /// A command alone on its line under the value column, so that it can
    /// be copied.
    fn command(&mut self, line: &str) {
        self.text.push_str(&" ".repeat(LABEL));
        self.paint(ColorToken::Primary, line);
        self.text.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The overview must fit an 80 by 25 console in every state, so that it
    /// never scrolls or wraps. Only an error text from Windows, which can be
    /// any length, may be wider.
    #[test]
    fn every_state_fits_an_80_by_25_console() {
        let index = |ready, size| Index {
            volumes: 12,
            ready,
            entries: 99_999_999,
            size,
        };
        let not_running = [
            NotRunning::Stopped,
            NotRunning::Disabled,
            NotRunning::NotInstalled {
                folder: PathBuf::from(r"C:\Program Files\gozler"),
                gozlerd_beside: true,
            },
            NotRunning::NotInstalled {
                folder: PathBuf::from(r"C:\Program Files\gozler"),
                gozlerd_beside: false,
            },
        ];
        let mut services = vec![
            Service::Running(index(12, Some(999_999_999_999_999))),
            Service::Running(index(12, None)),
            Service::Running(index(11, None)),
            Service::NotReady,
        ];
        services.extend(not_running.iter().map(Service::NotRunning));
        for service in &services {
            let text = render(service, false);
            assert!(text.lines().count() <= 25, "{text}");
            for line in text.lines() {
                assert!(line.chars().count() <= 80, "{line}");
            }
        }
    }
}
