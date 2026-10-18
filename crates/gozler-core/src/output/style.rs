//! Color tokens for the help and the overview. A token names what a piece of
//! text means, and the token decides its color, so one meaning has one color
//! everywhere. The colors are the standard console colors, so the text
//! follows the terminal's color scheme. The caller decides whether the console
//! draws them.

use std::fmt::Write;

/// What a piece of text means, which decides its color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorToken {
    /// The program name and version, and section titles: bold.
    Strong,
    /// A row label, such as `Service`: gray.
    Muted,
    /// Text the reader types: a command, an option or a search term: cyan.
    Primary,
    /// gozlerd runs, or the answer is complete: green.
    Success,
    /// gozlerd is not ready, or the answer may be incomplete or cut: yellow.
    Warning,
    /// gozlerd does not run, or the command failed: red.
    Danger,
}

/// Appends `text` to `out`, in the color of `token` when `color` is set.
pub fn paint(out: &mut String, token: ColorToken, text: &str, color: bool) {
    if !color {
        out.push_str(text);
        return;
    }
    let code = match token {
        ColorToken::Strong => "1",
        ColorToken::Muted => "90",
        ColorToken::Primary => "36",
        ColorToken::Success => "32",
        ColorToken::Warning => "33",
        ColorToken::Danger => "31",
    };
    write!(out, "\x1b[{code}m{text}\x1b[0m").expect("writing to a String succeeds");
}
