//! The command table: one row for each command of DESIGN.md's Commands table,
//! and the menu bar built from the rows (DESIGN.md: window rule 9). Every
//! surface reads a command's label, shortcut, enabled state and check mark from
//! here, so the menus, the keys and the controls cannot disagree.

use super::glyph::Glyph;
use super::ids::ControlId;
use super::model::{Column, Model};
use super::panel::{Side, Step, View};
use super::query::{SORT_DIRECTIONS, SORT_KEYS, sort_direction_label, sort_key_label};
use super::selection::OPEN_LIMIT;
use super::settings::{ContextItem, ContextMenu, Mode};
use super::shortcuts::CommandShortcuts;
use super::text::{LANGUAGES, LanguageChoice, Strings, menu_label};
use gozler_core::types::{SortDir, SortKey};
use serde::{Deserialize, Serialize};

/// One command of DESIGN.md's Commands table. Its names are `ui.json`'s,
/// where the person's command shortcuts are saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandId {
    Open,
    OpenFileLocation,
    Explore,
    Properties,
    ExportResults,
    /// Stops the job that the status bar shows. Its shortcuts were saved as
    /// `cancel_export` while it stopped only an export.
    #[serde(alias = "cancel_export")]
    Cancel,
    Settings,
    Exit,
    Copy,
    CopyAsPath,
    CopyName,
    CopyFolder,
    SelectAll,
    SearchInThisFolder,
    Filters,
    Inspector,
    Columns,
    Language,
    Refresh,
    Find,
    Match,
    SortBy,
    ClearFilters,
    SearchSyntax,
    About,
    ShowWindow,
    CopyToFolder,
    MoveToFolder,
    Delete,
    Cut,
    Paste,
    FileOperations,
    TwoPanels,
    ClearSelection,
    View,
    Edit,
    NewFolder,
}

impl CommandId {
    pub(crate) const ALL: [CommandId; 37] = [
        CommandId::Open,
        CommandId::OpenFileLocation,
        CommandId::Explore,
        CommandId::Properties,
        CommandId::ExportResults,
        CommandId::Cancel,
        CommandId::Settings,
        CommandId::Exit,
        CommandId::Copy,
        CommandId::CopyAsPath,
        CommandId::CopyName,
        CommandId::CopyFolder,
        CommandId::SelectAll,
        CommandId::SearchInThisFolder,
        CommandId::Filters,
        CommandId::Inspector,
        CommandId::Columns,
        CommandId::Language,
        CommandId::Refresh,
        CommandId::Find,
        CommandId::Match,
        CommandId::SortBy,
        CommandId::ClearFilters,
        CommandId::SearchSyntax,
        CommandId::About,
        CommandId::ShowWindow,
        CommandId::CopyToFolder,
        CommandId::MoveToFolder,
        CommandId::Delete,
        CommandId::Cut,
        CommandId::Paste,
        CommandId::FileOperations,
        CommandId::TwoPanels,
        CommandId::ClearSelection,
        CommandId::View,
        CommandId::Edit,
        CommandId::NewFolder,
    ];

    /// The command's row; a `match`, so every command has one.
    pub(crate) fn row(self) -> Command {
        use CommandId as C;
        let (label, shortcuts, enabled, checked): (_, &'static [Keystroke], Enabled, Checked) =
            match self {
                C::Open => ("command.open", &[], open, None),
                C::OpenFileLocation => (
                    "command.open_file_location",
                    const { &[Keystroke::ctrl(Key::Enter)] },
                    open,
                    None,
                ),
                C::Explore => ("command.explore", &[], open, None),
                C::Properties => (
                    "command.properties",
                    const { &[Keystroke::alt(Key::Enter)] },
                    one_file,
                    None,
                ),
                C::ExportResults => (
                    "command.export_results",
                    &[],
                    |s| s.model.can_export(),
                    None,
                ),
                C::Cancel => ("command.cancel", &[], |s| s.model.can_cancel_job(), None),
                C::Settings => (
                    "command.settings",
                    const { &[Keystroke::ctrl(Key::Comma)] },
                    always,
                    None,
                ),
                C::Exit => (
                    "command.exit",
                    const { &[Keystroke::ctrl(Key::Letter(b'Q'))] },
                    always,
                    None,
                ),
                C::Copy => (
                    "command.copy",
                    const { &[Keystroke::ctrl(Key::Letter(b'C'))] },
                    copy,
                    None,
                ),
                C::CopyAsPath => (
                    "command.copy_as_path",
                    const { &[Keystroke::ctrl_shift(Key::Letter(b'C'))] },
                    files,
                    None,
                ),
                C::CopyName => ("command.copy_name", &[], files, None),
                C::CopyFolder => ("command.copy_folder", &[], files, None),
                C::SelectAll => (
                    "command.select_all",
                    const { &[Keystroke::ctrl(Key::Letter(b'A'))] },
                    always,
                    None,
                ),
                C::SearchInThisFolder => ("command.search_in_this_folder", &[], one_file, None),
                C::Filters => (
                    "command.filters",
                    &[],
                    always,
                    Some(|s| s.model.panes().filters),
                ),
                C::Inspector => (
                    "command.inspector",
                    &[],
                    always,
                    Some(|s| s.model.panes().inspector),
                ),
                C::Columns => ("command.columns", &[], always, None),
                C::Language => ("menu.language", &[], always, None),
                C::Refresh => (
                    "command.refresh",
                    const { &[Keystroke::plain(Key::Function(5))] },
                    searched,
                    None,
                ),
                C::Find => (
                    "command.find",
                    const {
                        &[
                            Keystroke::ctrl(Key::Letter(b'F')),
                            Keystroke::ctrl(Key::Letter(b'E')),
                        ]
                    },
                    always,
                    None,
                ),
                C::Match => ("match.label", &[], always, None),
                C::SortBy => ("command.sort_by", &[], always, None),
                C::ClearFilters => (
                    "command.clear_filters",
                    &[],
                    |s| s.model.filters().is_set(),
                    None,
                ),
                C::SearchSyntax => (
                    "command.search_syntax",
                    const { &[Keystroke::plain(Key::Function(1))] },
                    always,
                    None,
                ),
                C::About => ("command.about", &[], always, None),
                C::ShowWindow => (
                    "command.show_window",
                    &[],
                    |s| s.model.choices().keep_running,
                    None,
                ),
                // Delete, Cut and Paste are keys of the list (control_key).
                C::CopyToFolder => ("command.copy_to_folder", &[], transfer, None),
                C::MoveToFolder => ("command.move_to_folder", &[], transfer, None),
                C::Delete => ("command.delete", &[], file_operation, None),
                C::Cut => ("command.cut", &[], cut, None),
                C::Paste => ("command.paste", &[], paste, None),
                C::FileOperations => (
                    "command.file_operations",
                    &[],
                    |s| s.model.offers_file_operations(),
                    None,
                ),
                C::TwoPanels => (
                    "command.two_panels",
                    &[],
                    always,
                    Some(|s| s.model.mode() == Mode::TwoPanels),
                ),
                // `Esc` in a panel's list (control_key).
                C::ClearSelection => (
                    "command.clear_selection",
                    &[],
                    |s| s.model.panels().has_selection(),
                    None,
                ),
                C::View => (
                    "command.view",
                    &[],
                    |s| {
                        let panels = s.model.panels();
                        (panels.focused(panels.active())).is_some_and(|entry| !entry.is_folder)
                    },
                    None,
                ),
                C::Edit => ("command.edit", &[], never, None),
                C::NewFolder => (
                    "command.new_folder",
                    &[],
                    |s| {
                        s.model.offers_file_operations()
                            && matches!(
                                s.model.panels().panel(s.model.panels().active()).view(),
                                View::Folder
                            )
                    },
                    None,
                ),
            };
        Command {
            label,
            shortcuts,
            enabled,
            checked,
        }
    }

    /// The icon before its label in a menu, if it has one (DESIGN.md: Look).
    /// A command with a check mark has none, because the mark takes the
    /// icon's place.
    pub(crate) fn glyph(self) -> Option<Glyph> {
        use CommandId as C;
        Some(match self {
            C::Open => Glyph::OpenFile,
            C::OpenFileLocation => Glyph::FolderOpen,
            C::Explore => Glyph::FileExplorer,
            C::Properties => Glyph::Info,
            C::ExportResults => Glyph::Export,
            C::Settings => Glyph::Settings,
            C::Exit => Glyph::Cancel,
            C::Copy => Glyph::Copy,
            C::CopyName => Glyph::Rename,
            C::CopyFolder => Glyph::Folder,
            C::CopyAsPath => Glyph::Link,
            C::SelectAll => Glyph::SelectAll,
            C::Find | C::SearchInThisFolder => Glyph::Search,
            C::Refresh => Glyph::Refresh,
            C::SearchSyntax => Glyph::Help,
            C::About => Glyph::Info,
            C::ShowWindow => Glyph::BackToWindow,
            C::CopyToFolder => Glyph::CopyTo,
            C::MoveToFolder => Glyph::MoveToFolder,
            C::Delete => Glyph::Delete,
            C::Cut => Glyph::Cut,
            C::Paste => Glyph::Paste,
            C::View => Glyph::View,
            C::Edit => Glyph::Edit,
            C::NewFolder => Glyph::NewFolder,
            C::Cancel
            | C::FileOperations
            | C::Filters
            | C::Inspector
            | C::Columns
            | C::Language
            | C::Match
            | C::SortBy
            | C::ClearFilters
            | C::TwoPanels
            | C::ClearSelection => return None,
        })
    }

    /// Whether the command belongs to `mode`: only such a command shows in
    /// the menu bar and takes its shortcuts while `mode` shows
    /// (file-manager-spec.md: Keys).
    pub(crate) fn belongs_to(self, mode: Mode) -> bool {
        use CommandId as C;
        match self {
            C::Settings
            | C::Exit
            | C::Language
            | C::SearchSyntax
            | C::About
            | C::ShowWindow
            | C::FileOperations
            | C::TwoPanels
            | C::SelectAll
            | C::Refresh
            | C::Find
            | C::SortBy
            | C::Cancel => true,
            C::Open | C::Properties | C::CopyToFolder | C::MoveToFolder | C::Delete => true,
            C::ClearSelection | C::View | C::Edit | C::NewFolder => mode == Mode::TwoPanels,
            C::OpenFileLocation
            | C::Explore
            | C::ExportResults
            | C::Copy
            | C::CopyAsPath
            | C::CopyName
            | C::CopyFolder
            | C::SearchInThisFolder
            | C::Filters
            | C::Inspector
            | C::Columns
            | C::Match
            | C::ClearFilters
            | C::Cut
            | C::Paste => mode == Mode::Search,
        }
    }
}

type Enabled = fn(&State) -> bool;
type Checked = Option<fn(&State) -> bool>;

/// One row of the command table.
#[derive(Clone, Copy)]
pub(crate) struct Command {
    /// The catalogue key of its menu label, with its access key.
    pub(crate) label: &'static str,
    /// The default shortcuts; the menus show the first.
    pub(crate) shortcuts: &'static [Keystroke],
    pub(crate) enabled: Enabled,
    /// `Some` for a command with a check mark.
    pub(crate) checked: Checked,
}

/// What a command's state depends on: the model, and which control has the
/// focus, which only Windows knows.
pub(crate) struct State<'a> {
    pub(crate) model: &'a Model,
    pub(crate) focus: Focus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    SearchBox {
        text_selected: bool,
    },
    List,
    /// The Date modified or Size list, closed, whose `Enter` opens the
    /// custom range's dialog when Custom… is selected (DESIGN.md: Filter pane).
    RangeFilter,
    /// One of the inspector's read-only values, whose text Copy and Select
    /// all act on (DESIGN.md: Inspector pane).
    Value {
        text_selected: bool,
    },
    /// A panel's list (file-manager-spec.md: Keys).
    Panel,
    /// A panel's path box, whose `Enter`, `Esc` and `Tab` the window
    /// takes.
    PathBox,
    /// A search panel's search box (file-manager-spec.md: Search in a
    /// panel).
    PanelSearch,
    /// A search panel's Look in list.
    LookIn,
    Other,
}

fn always(_: &State) -> bool {
    true
}

fn never(_: &State) -> bool {
    false
}

/// Refresh runs the shown search again (DESIGN.md: C17); in Two panels it
/// reads the active panel's folder again.
fn searched(state: &State) -> bool {
    state.model.mode() == Mode::TwoPanels || state.model.search_list().results().has_searched()
}

/// The selected entries, once the selection is read (DESIGN.md: The selection).
fn files(state: &State) -> bool {
    state.model.search_list().selection().files().is_some()
}

/// How many entries a file command acts on: Search's selected files, once
/// read, or the active panel's selected entries, else its focused entry
/// (file-manager-spec.md: Commands in Two panels).
fn chosen(state: &State) -> usize {
    match state.model.mode() {
        Mode::Search => (state.model.search_list().selection().files()).map_or(0, <[_]>::len),
        Mode::TwoPanels => state.model.panels().chosen_count(),
    }
}

/// Open, Open file location and Explore (DESIGN.md: C1, C2, C26).
fn open(state: &State) -> bool {
    (1..=OPEN_LIMIT).contains(&chosen(state))
}

/// Properties shows one entry, and Search in this folder searches its
/// folder (DESIGN.md: C3, C12).
fn one_file(state: &State) -> bool {
    chosen(state) == 1
}

/// Copy to folder…, Move to folder…, Delete and Cut act on the chosen
/// entries, in the window that owns the file operations
/// (file-manager-spec.md: Search mode, Commands in Two panels).
fn file_operation(state: &State) -> bool {
    state.model.offers_file_operations() && chosen(state) > 0
}

/// Copy to folder… and Move to folder…, which in Two panels also need a
/// folder to go to, so not while both panels search
/// (file-manager-spec.md: Search in a panel).
fn transfer(state: &State) -> bool {
    let panels = state.model.panels();
    let both_search = Side::BOTH
        .into_iter()
        .all(|side| panels.search(side).is_some());
    file_operation(state) && !(state.model.mode() == Mode::TwoPanels && both_search)
}

/// Cut acts on the selected files while the list has the focus, as Copy
/// does (DESIGN.md: C8); the search box cuts its own text.
fn cut(state: &State) -> bool {
    state.focus == Focus::List && file_operation(state)
}

/// Paste copies or moves the clipboard's files into the one selected
/// folder while the list has the focus (file-manager-spec.md: Search
/// mode); the search box pastes text itself.
fn paste(state: &State) -> bool {
    state.focus == Focus::List
        && state.model.offers_file_operations()
        && state.model.clipboard_holds_files()
        && (state.model.search_list().selection().files())
            .is_some_and(|files| matches!(files, [folder] if folder.is_dir))
}

/// Copy acts on the focused control (DESIGN.md: C8).
fn copy(state: &State) -> bool {
    match state.focus {
        Focus::SearchBox { text_selected } | Focus::Value { text_selected } => text_selected,
        Focus::List => files(state),
        Focus::RangeFilter
        | Focus::Panel
        | Focus::PathBox
        | Focus::PanelSearch
        | Focus::LookIn
        | Focus::Other => false,
    }
}

/// A pressed key combination: a command's shortcut, or a key the focused
/// control gives a meaning (DESIGN.md: window rule 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Keystroke {
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) key: Key,
}

/// A key that a shortcut or a control's key can be. A key that Windows names
/// differently on each keyboard layout, such as `;`, is none of them,
/// because its name would differ between a menu and the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    /// An ASCII capital letter, which is also its virtual-key code.
    Letter(u8),
    /// An ASCII digit, which is also its virtual-key code.
    Digit(u8),
    /// F1 to F24.
    Function(u8),
    Enter,
    Escape,
    Tab,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Up,
    Right,
    Down,
    Comma,
    Period,
}

impl Key {
    /// Every key without a number, for reading a key's name back.
    const NAMED: [Key; 17] = [
        Key::Enter,
        Key::Escape,
        Key::Tab,
        Key::Space,
        Key::Backspace,
        Key::Delete,
        Key::Insert,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
        Key::Left,
        Key::Up,
        Key::Right,
        Key::Down,
        Key::Comma,
        Key::Period,
    ];

    /// Its name in `ui.json`, the same in every language, such as "PageUp".
    pub(crate) fn name(self) -> String {
        match self {
            Key::Letter(c) | Key::Digit(c) => char::from(c).to_string(),
            Key::Function(n) => format!("F{n}"),
            Key::Enter => "Enter".into(),
            Key::Escape => "Esc".into(),
            Key::Tab => "Tab".into(),
            Key::Space => "Space".into(),
            Key::Backspace => "Backspace".into(),
            Key::Delete => "Delete".into(),
            Key::Insert => "Insert".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "PageUp".into(),
            Key::PageDown => "PageDown".into(),
            Key::Left => "Left".into(),
            Key::Up => "Up".into(),
            Key::Right => "Right".into(),
            Key::Down => "Down".into(),
            Key::Comma => ",".into(),
            Key::Period => ".".into(),
        }
    }

    /// The key whose [`Key::name`] is `name`.
    pub(crate) fn from_name(name: &str) -> Option<Key> {
        match name.as_bytes() {
            [c @ b'A'..=b'Z'] => Some(Key::Letter(*c)),
            [c @ b'0'..=b'9'] => Some(Key::Digit(*c)),
            [b'F', number @ ..] if !number.is_empty() => std::str::from_utf8(number)
                .ok()?
                .parse()
                .ok()
                .filter(|n| (1..=24).contains(n))
                .map(Key::Function),
            _ => Key::NAMED.into_iter().find(|key| key.name() == name),
        }
    }

    /// As a menu shows it, in the language of `strings`.
    pub(crate) fn text(self, strings: &Strings) -> String {
        let named = |key: &'static str| strings.text(key).to_owned();
        match self {
            Key::Letter(_) | Key::Digit(_) | Key::Function(_) | Key::Comma | Key::Period => {
                self.name()
            }
            Key::Enter => named("key.enter"),
            Key::Escape => named("key.escape"),
            Key::Tab => named("key.tab"),
            Key::Space => named("key.space"),
            Key::Backspace => named("key.backspace"),
            Key::Delete => named("key.delete"),
            Key::Insert => named("key.insert"),
            Key::Home => named("key.home"),
            Key::End => named("key.end"),
            Key::PageUp => named("key.page_up"),
            Key::PageDown => named("key.page_down"),
            Key::Left => named("key.left"),
            Key::Up => named("key.up"),
            Key::Right => named("key.right"),
            Key::Down => named("key.down"),
        }
    }
}

impl Keystroke {
    pub(crate) const fn plain(key: Key) -> Keystroke {
        Keystroke {
            ctrl: false,
            alt: false,
            shift: false,
            key,
        }
    }

    pub(crate) const fn ctrl(key: Key) -> Keystroke {
        Keystroke {
            ctrl: true,
            ..Keystroke::plain(key)
        }
    }

    pub(crate) const fn alt(key: Key) -> Keystroke {
        Keystroke {
            alt: true,
            ..Keystroke::plain(key)
        }
    }

    pub(crate) const fn ctrl_shift(key: Key) -> Keystroke {
        Keystroke {
            ctrl: true,
            shift: true,
            ..Keystroke::plain(key)
        }
    }

    /// As a menu shows it, such as "Ctrl+Shift+C", with the modifier names
    /// of the language, because German Windows writes "Strg".
    pub(crate) fn text(self, strings: &Strings) -> String {
        let mut parts: Vec<String> = self
            .modifiers()
            .into_iter()
            .map(|(_, key)| strings.text(key).to_owned())
            .collect();
        parts.push(self.key.text(strings));
        parts.join("+")
    }

    /// Its name in `ui.json`, such as "Ctrl+Shift+F12", the same in every
    /// language.
    pub(crate) fn name(self) -> String {
        let mut parts: Vec<String> = self
            .modifiers()
            .into_iter()
            .map(|(name, _)| name.to_owned())
            .collect();
        parts.push(self.key.name());
        parts.join("+")
    }

    /// The keystroke whose [`Keystroke::name`] is `name`.
    pub(crate) fn from_name(name: &str) -> Option<Keystroke> {
        // The key comes last; no key's name holds a `+`.
        let (modifiers, key) = name.rsplit_once('+').unwrap_or(("", name));
        let mut keystroke = Keystroke::plain(Key::from_name(key)?);
        for modifier in modifiers.split('+').filter(|m| !m.is_empty()) {
            let held = match modifier {
                "Ctrl" => &mut keystroke.ctrl,
                "Alt" => &mut keystroke.alt,
                "Shift" => &mut keystroke.shift,
                _ => return None,
            };
            if std::mem::replace(held, true) {
                return None;
            }
        }
        Some(keystroke)
    }

    /// The `ui.json` name and the catalogue key of each modifier it holds,
    /// in the order Windows writes them.
    fn modifiers(self) -> Vec<(&'static str, &'static str)> {
        [
            (self.ctrl, "Ctrl", "key.ctrl"),
            (self.alt, "Alt", "key.alt"),
            (self.shift, "Shift", "key.shift"),
        ]
        .into_iter()
        .filter_map(|(held, name, key)| held.then_some((name, key)))
        .collect()
    }
}

/// A control that `Tab` and `Shift+Tab` move the focus between. Every
/// other control has an access key, a menu command or a shortcut, so a
/// person who typed a search reaches the results with one `Tab`
/// (DESIGN.md: Dialogs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabStop {
    SearchBox,
    Results,
}

/// Where `Tab`, or `Shift+Tab` when `backward`, moves the focus from
/// `from`, which is `None` for a control outside the cycle: from there
/// `Tab` goes to the results and `Shift+Tab` to the search box.
pub(crate) fn tab_stop(from: Option<TabStop>, backward: bool) -> TabStop {
    match from {
        Some(TabStop::SearchBox) => TabStop::Results,
        Some(TabStop::Results) => TabStop::SearchBox,
        None if backward => TabStop::SearchBox,
        None => TabStop::Results,
    }
}

/// What a key does in the focused control (DESIGN.md: window rule 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    Run(CommandId),
    /// `Tab`, or `Shift+Tab` when `backward`, in any control: the focus
    /// moves as [`tab_stop`] says.
    MoveTab {
        backward: bool,
    },
    /// `Esc` in the results list: File > Cancel, unless the job is a file
    /// operation, which one stray key must not stop
    /// (file-manager-spec.md: Keys).
    CancelJob,
    /// `Down` in the search box: the focus moves to the list's first row.
    FocusList,
    /// `Enter` in the search box: the search runs, or the result opens
    /// ([`Model::enter_in_search_box`]).
    SearchBoxEnter,
    /// `Esc` in the search box: the box is emptied.
    ClearSearch,
    /// `Enter` in a closed Date modified or Size list.
    ChooseCustom,
    /// A key of a panel's list or path box (file-manager-spec.md: Moving
    /// around, Selecting entries).
    Panel(PanelKey),
}

/// What a key does in a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelKey {
    /// `Enter` in the list: the focused row's folder shows, or its file
    /// opens.
    Enter,
    /// `Backspace`: the parent folder.
    Back,
    /// `Tab`: the other panel becomes active.
    OtherPanel,
    /// `Esc` in the list: the quick search box closes, else the selection
    /// is cleared.
    Escape,
    /// `Insert` and `Space`: the focused row is selected or cleared, and
    /// the focus moves down.
    Toggle,
    /// The focus moves, the selection stays.
    Move(Step),
    /// The focus moves and selects each row it passes.
    Select(Step),
    /// `Enter` in the path box: the panel goes to the typed path.
    PathEnter,
    /// `Esc` in the path box: the folder's path is back and the list has
    /// the focus.
    PathEscape,
    /// `Down` and `Enter` in a search panel's search box: the list takes
    /// the focus, and its first row the focus when no row has it.
    ToList,
    /// `Esc` in a search panel's search box: the searched folder shows
    /// again.
    LeaveSearch,
    /// `Enter` in a search panel's closed Look in list: the search box
    /// takes the focus.
    LookInEnter,
}

/// The meaning `key` has in the focused control, which the window takes
/// before `IsDialogMessageW` turns `Enter` and `Esc` into `IDOK` and
/// `IDCANCEL`, and before the control itself sees the key. The message loop
/// translates a command's shortcut first, so no shortcut reaches this table.
pub(crate) fn control_key(focus: Focus, key: Keystroke) -> Option<KeyAction> {
    if key.key == Key::Tab && !key.ctrl && !key.alt {
        // In a panel, Tab switches panels (file-manager-spec.md: Keys).
        return Some(match focus {
            Focus::Panel | Focus::PathBox | Focus::PanelSearch | Focus::LookIn => {
                KeyAction::Panel(PanelKey::OtherPanel)
            }
            _ => KeyAction::MoveTab {
                backward: key.shift,
            },
        });
    }
    let table: &[(Keystroke, KeyAction)] = match focus {
        Focus::SearchBox { .. } => {
            const {
                &[
                    (Keystroke::plain(Key::Down), KeyAction::FocusList),
                    (Keystroke::plain(Key::Enter), KeyAction::SearchBoxEnter),
                    (Keystroke::plain(Key::Escape), KeyAction::ClearSearch),
                ]
            }
        }
        Focus::List => {
            const {
                &[
                    (
                        Keystroke::plain(Key::Enter),
                        KeyAction::Run(CommandId::Open),
                    ),
                    // Gozler never uses the Recycle Bin, so Shift+Delete,
                    // File Explorer's permanent delete, deletes as Delete
                    // does.
                    (
                        Keystroke::plain(Key::Delete),
                        KeyAction::Run(CommandId::Delete),
                    ),
                    (
                        Keystroke {
                            shift: true,
                            ..Keystroke::plain(Key::Delete)
                        },
                        KeyAction::Run(CommandId::Delete),
                    ),
                    (
                        Keystroke::ctrl(Key::Letter(b'X')),
                        KeyAction::Run(CommandId::Cut),
                    ),
                    (
                        Keystroke::ctrl(Key::Letter(b'V')),
                        KeyAction::Run(CommandId::Paste),
                    ),
                    // Stops the status bar's job, as Esc in the search box
                    // empties the box.
                    (Keystroke::plain(Key::Escape), KeyAction::CancelJob),
                ]
            }
        }
        Focus::RangeFilter => const { &[(Keystroke::plain(Key::Enter), KeyAction::ChooseCustom)] },
        Focus::Panel => PANEL_KEYS,
        Focus::PathBox => {
            const {
                &[
                    (
                        Keystroke::plain(Key::Enter),
                        KeyAction::Panel(PanelKey::PathEnter),
                    ),
                    (
                        Keystroke::plain(Key::Escape),
                        KeyAction::Panel(PanelKey::PathEscape),
                    ),
                ]
            }
        }
        Focus::PanelSearch => {
            const {
                &[
                    (
                        Keystroke::plain(Key::Down),
                        KeyAction::Panel(PanelKey::ToList),
                    ),
                    (
                        Keystroke::plain(Key::Enter),
                        KeyAction::Panel(PanelKey::ToList),
                    ),
                    (
                        Keystroke::plain(Key::Escape),
                        KeyAction::Panel(PanelKey::LeaveSearch),
                    ),
                ]
            }
        }
        Focus::LookIn => {
            const {
                &[(
                    Keystroke::plain(Key::Enter),
                    KeyAction::Panel(PanelKey::LookInEnter),
                )]
            }
        }
        Focus::Value { .. } | Focus::Other => &[],
    };
    table
        .iter()
        .find(|(stroke, _)| *stroke == key)
        .map(|(_, action)| *action)
}

/// The keys of a panel's list. The arrow keys are the window's here, not
/// the list's, because the list moves the selection with the focus.
const PANEL_KEYS: &[(Keystroke, KeyAction)] = {
    use KeyAction::Panel as P;
    const fn shift(key: Key) -> Keystroke {
        Keystroke {
            shift: true,
            ..Keystroke::plain(key)
        }
    }
    &[
        (Keystroke::plain(Key::Enter), P(PanelKey::Enter)),
        (Keystroke::plain(Key::Backspace), P(PanelKey::Back)),
        (Keystroke::plain(Key::Escape), P(PanelKey::Escape)),
        (Keystroke::plain(Key::Insert), P(PanelKey::Toggle)),
        (Keystroke::plain(Key::Space), P(PanelKey::Toggle)),
        // As in Search's list.
        (
            Keystroke::plain(Key::Delete),
            KeyAction::Run(CommandId::Delete),
        ),
        (shift(Key::Delete), KeyAction::Run(CommandId::Delete)),
        (Keystroke::plain(Key::Up), P(PanelKey::Move(Step::Up))),
        (Keystroke::plain(Key::Down), P(PanelKey::Move(Step::Down))),
        (
            Keystroke::plain(Key::PageUp),
            P(PanelKey::Move(Step::PageUp)),
        ),
        (
            Keystroke::plain(Key::PageDown),
            P(PanelKey::Move(Step::PageDown)),
        ),
        (Keystroke::plain(Key::Home), P(PanelKey::Move(Step::Home))),
        (Keystroke::plain(Key::End), P(PanelKey::Move(Step::End))),
        (shift(Key::Up), P(PanelKey::Select(Step::Up))),
        (shift(Key::Down), P(PanelKey::Select(Step::Down))),
        (shift(Key::PageUp), P(PanelKey::Select(Step::PageUp))),
        (shift(Key::PageDown), P(PanelKey::Select(Step::PageDown))),
        (shift(Key::Home), P(PanelKey::Select(Step::Home))),
        (shift(Key::End), P(PanelKey::Select(Step::End))),
    ]
};

/// Two panels' fixed keys that run no command (file-manager-spec.md:
/// Keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelAction {
    /// `Ctrl+L`: the active panel's path box, or a search panel's search
    /// box, takes the focus.
    PathBox,
    /// `Alt+F1`, `Alt+F2`: a panel's drive menu.
    Drives(Side),
    /// `F9`: the menu bar, because `Alt` with a letter is quick search.
    MenuBar,
}

impl PanelAction {
    const ALL: [PanelAction; 4] = [
        PanelAction::PathBox,
        PanelAction::Drives(Side::Left),
        PanelAction::Drives(Side::Right),
        PanelAction::MenuBar,
    ];
}

/// Two panels' fixed keys, which win over the person's shortcuts on the
/// same keys. `Alt+F7` comes first so that Find's menu item shows it.
const TWO_PANEL_KEYS: &[(Keystroke, MenuItem)] = &[
    (
        Keystroke::alt(Key::Function(7)),
        MenuItem::Run(CommandId::Find),
    ),
    (
        Keystroke::ctrl(Key::Letter(b'R')),
        MenuItem::Run(CommandId::Refresh),
    ),
    (
        Keystroke::ctrl(Key::Letter(b'L')),
        MenuItem::Panel(PanelAction::PathBox),
    ),
    (
        Keystroke::alt(Key::Function(1)),
        MenuItem::Panel(PanelAction::Drives(Side::Left)),
    ),
    (
        Keystroke::alt(Key::Function(2)),
        MenuItem::Panel(PanelAction::Drives(Side::Right)),
    ),
    (
        Keystroke::plain(Key::Function(9)),
        MenuItem::Panel(PanelAction::MenuBar),
    ),
    (
        Keystroke::plain(Key::Function(3)),
        MenuItem::Run(CommandId::View),
    ),
    (
        Keystroke::plain(Key::Function(4)),
        MenuItem::Run(CommandId::Edit),
    ),
    (
        Keystroke::plain(Key::Function(5)),
        MenuItem::Run(CommandId::CopyToFolder),
    ),
    (
        Keystroke::plain(Key::Function(6)),
        MenuItem::Run(CommandId::MoveToFolder),
    ),
    (
        Keystroke::plain(Key::Function(7)),
        MenuItem::Run(CommandId::NewFolder),
    ),
    (
        Keystroke::plain(Key::Function(8)),
        MenuItem::Run(CommandId::Delete),
    ),
];

/// The command of the key bar's button at `index` from the left, the one
/// its key runs: `F3` View first (file-manager-spec.md: The key bar).
pub(crate) fn key_bar_command(index: usize) -> CommandId {
    let key = u8::try_from(index).map(|index| Keystroke::plain(Key::Function(3 + index)));
    (TWO_PANEL_KEYS.iter())
        .find_map(|(stroke, item)| match item {
            MenuItem::Run(command) if Ok(*stroke) == key => Some(*command),
            _ => None,
        })
        .expect("F3 to F8 run the key bar's commands")
}

/// Whether Two panels keeps `keystroke` from the person's shortcuts: its
/// fixed keys, `F3` to `F8` among them, which the key bar's commands take;
/// and `Alt` with a character, which is quick search.
fn kept_by_two_panels(keystroke: Keystroke) -> bool {
    let character = matches!(
        keystroke.key,
        Key::Letter(_) | Key::Digit(_) | Key::Comma | Key::Period
    );
    TWO_PANEL_KEYS.iter().any(|(fixed, _)| *fixed == keystroke)
        || keystroke.alt && !keystroke.ctrl && character
}

/// One clickable menu item: a command, or one choice of a command that has
/// choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuItem {
    Run(CommandId),
    Column(Column),
    /// Fits the column, of the header's menu (DESIGN.md: Results list).
    FitColumn(Column),
    /// Fits every shown column, of the header's menu.
    FitColumns,
    SortKey(SortKey),
    SortDirection(SortDir),
    /// `None` is Windows default, else an index into [`LANGUAGES`].
    Language(Option<usize>),
    /// A fixed key of Two panels, which no menu shows.
    Panel(PanelAction),
}

/// How a menu item shows that it is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    None,
    Check,
    Radio,
}

impl MenuItem {
    /// The item's `WM_COMMAND` ID. Each kind has its own range, so no two
    /// items share an ID.
    pub(crate) fn id(self) -> u16 {
        let index = |found: Option<usize>| {
            u16::try_from(found.expect("the item is in its table")).expect("a table is short")
        };
        match self {
            MenuItem::Run(command) => {
                0x100 + index(CommandId::ALL.iter().position(|c| *c == command))
            }
            MenuItem::Column(column) => {
                0x200 + index(Column::ALL.iter().position(|c| *c == column))
            }
            MenuItem::SortKey(key) => 0x300 + index(SORT_KEYS.iter().position(|k| *k == key)),
            MenuItem::SortDirection(dir) => {
                0x400 + index(SORT_DIRECTIONS.iter().position(|d| *d == dir))
            }
            MenuItem::Language(None) => 0x500,
            MenuItem::Language(Some(language)) => 0x501 + index(Some(language)),
            MenuItem::FitColumn(column) => {
                0x600 + index(Column::ALL.iter().position(|c| *c == column))
            }
            MenuItem::FitColumns => 0x700,
            MenuItem::Panel(action) => {
                0x800 + index(PanelAction::ALL.iter().position(|a| *a == action))
            }
        }
    }

    /// The item whose [`id`](Self::id) is `id`.
    pub(crate) fn from_id(id: u16) -> Option<MenuItem> {
        let n = usize::from(id & 0xff);
        match id & 0xff00 {
            0x100 => CommandId::ALL.get(n).copied().map(MenuItem::Run),
            0x200 => Column::ALL.get(n).copied().map(MenuItem::Column),
            0x300 => SORT_KEYS.get(n).copied().map(MenuItem::SortKey),
            0x400 => SORT_DIRECTIONS.get(n).copied().map(MenuItem::SortDirection),
            0x500 if n == 0 => Some(MenuItem::Language(None)),
            0x500 if n <= LANGUAGES.len() => Some(MenuItem::Language(Some(n - 1))),
            0x600 => Column::ALL.get(n).copied().map(MenuItem::FitColumn),
            0x700 if n == 0 => Some(MenuItem::FitColumns),
            0x800 => PanelAction::ALL.get(n).copied().map(MenuItem::Panel),
            _ => None,
        }
    }

    /// Its label, with its access key.
    pub(crate) fn label(self, strings: &Strings) -> String {
        let key = match self {
            MenuItem::Run(command) => command.row().label,
            MenuItem::Column(column) => column.menu_label(),
            MenuItem::SortKey(key) => sort_key_label(key),
            MenuItem::SortDirection(dir) => sort_direction_label(dir),
            MenuItem::Language(None) => "command.windows_default",
            MenuItem::Language(Some(language)) => return LANGUAGES[language].label(strings),
            MenuItem::FitColumn(column) => {
                return strings.format("column.fit", &[strings.text(column.header())]);
            }
            MenuItem::FitColumns => "column.fit_columns",
            MenuItem::Panel(_) => return String::new(),
        };
        strings.text(key).to_owned()
    }

    /// The shortcut the menu shows after the label: the first key that
    /// runs it while `mode` shows, from the table its accelerators come
    /// from. In Two panels, Clear selection shows `Esc`, the key of a
    /// panel's list, because the menus there teach the keys
    /// (file-manager-spec.md: Menus in Two panels).
    pub(crate) fn shortcut(self, shortcuts: &CommandShortcuts, mode: Mode) -> Option<Keystroke> {
        let MenuItem::Run(command) = self else {
            return None;
        };
        accelerators(shortcuts, mode)
            .into_iter()
            .find(|(_, item)| *item == self)
            .map(|(key, _)| key)
            .or_else(|| {
                (mode == Mode::TwoPanels && command == CommandId::ClearSelection)
                    .then_some(Keystroke::plain(Key::Escape))
            })
    }

    /// The command whose row decides whether this item is enabled.
    fn command(self) -> CommandId {
        match self {
            MenuItem::Run(command) => command,
            MenuItem::Column(_) | MenuItem::FitColumn(_) | MenuItem::FitColumns => {
                CommandId::Columns
            }
            MenuItem::SortKey(_) | MenuItem::SortDirection(_) => CommandId::SortBy,
            MenuItem::Language(_) => CommandId::Language,
            MenuItem::Panel(_) => CommandId::TwoPanels,
        }
    }

    /// Whether the item shows while `mode` shows: Sort by > Path has no
    /// column in Two panels.
    fn belongs_to(self, mode: Mode) -> bool {
        match self {
            MenuItem::SortKey(SortKey::Path) => mode == Mode::Search,
            _ => self.command().belongs_to(mode),
        }
    }

    pub(crate) fn enabled(self, state: &State) -> bool {
        let shown = match self {
            MenuItem::Column(column) => column.can_hide(),
            _ => true,
        };
        (self.command().row().enabled)(state) && shown
    }

    /// The icon before its label, which only a command without a mark has.
    pub(crate) fn glyph(self) -> Option<Glyph> {
        match self {
            MenuItem::Run(command) => command.glyph(),
            MenuItem::Column(_)
            | MenuItem::FitColumn(_)
            | MenuItem::FitColumns
            | MenuItem::SortKey(_)
            | MenuItem::SortDirection(_)
            | MenuItem::Language(_)
            | MenuItem::Panel(_) => None,
        }
    }

    pub(crate) fn mark(self) -> Mark {
        match self {
            MenuItem::Run(command) => match command.row().checked {
                Some(_) => Mark::Check,
                None => Mark::None,
            },
            MenuItem::Column(_) => Mark::Check,
            MenuItem::FitColumn(_) | MenuItem::FitColumns | MenuItem::Panel(_) => Mark::None,
            MenuItem::SortKey(_) | MenuItem::SortDirection(_) | MenuItem::Language(_) => {
                Mark::Radio
            }
        }
    }

    pub(crate) fn checked(self, state: &State) -> bool {
        let model = state.model;
        match self {
            MenuItem::Run(command) => command.row().checked.is_some_and(|checked| checked(state)),
            MenuItem::Column(column) => model.columns.contains(&column),
            MenuItem::FitColumn(_) | MenuItem::FitColumns | MenuItem::Panel(_) => false,
            MenuItem::SortKey(key) => model.shown_sort().key == key,
            MenuItem::SortDirection(dir) => model.shown_sort().dir == dir,
            MenuItem::Language(None) => model.settings.choices.language == LanguageChoice::Windows,
            MenuItem::Language(Some(language)) => {
                model.settings.choices.language.shipped() == Some(language)
            }
        }
    }
}

/// An entry of a menu: an item, or a submenu of choices.
pub(crate) enum Entry {
    Item(MenuItem),
    Submenu {
        label: &'static str,
        groups: Vec<Vec<MenuItem>>,
    },
}

impl Entry {
    /// The entry with only the items whose command belongs to `mode`, or
    /// `None` when none does.
    fn of_mode(self, mode: Mode) -> Option<Entry> {
        let belongs = |item: &MenuItem| item.belongs_to(mode);
        match self {
            Entry::Item(item) => belongs(&item).then_some(Entry::Item(item)),
            Entry::Submenu { label, groups } => {
                let groups: Vec<Vec<MenuItem>> = groups
                    .into_iter()
                    .map(|group| group.into_iter().filter(belongs).collect::<Vec<_>>())
                    .filter(|group| !group.is_empty())
                    .collect();
                (!groups.is_empty()).then_some(Entry::Submenu { label, groups })
            }
        }
    }
}

/// A menu of the menu bar; a separator divides its groups.
pub(crate) struct Menu {
    pub(crate) label: &'static str,
    pub(crate) groups: Vec<Vec<Entry>>,
    /// The Language menu sits at the right end of the menu bar.
    pub(crate) aligned_right: bool,
}

/// The menu bar of `mode`, in its order and groups (DESIGN.md: Menus): the
/// entries of the commands that belong to `mode`, without the groups and the
/// menus that are left empty.
pub(crate) fn menu_bar(mode: Mode) -> Vec<Menu> {
    every_menu()
        .into_iter()
        .filter_map(|menu| {
            let groups: Vec<Vec<Entry>> = menu
                .groups
                .into_iter()
                .map(|group| {
                    group
                        .into_iter()
                        .filter_map(|entry| entry.of_mode(mode))
                        .collect::<Vec<_>>()
                })
                .filter(|group| !group.is_empty())
                .collect();
            (!groups.is_empty()).then_some(Menu { groups, ..menu })
        })
        .collect()
}

/// The menus of both modes, each command in its place.
fn every_menu() -> Vec<Menu> {
    use CommandId as C;
    let run = |commands: &[CommandId]| {
        commands
            .iter()
            .map(|c| Entry::Item(MenuItem::Run(*c)))
            .collect()
    };
    let menu = |label, groups| Menu {
        label,
        groups,
        aligned_right: false,
    };
    vec![
        menu(
            "menu.file",
            vec![
                run(&[
                    C::Open,
                    C::OpenFileLocation,
                    C::View,
                    C::Edit,
                    C::Properties,
                ]),
                run(&[C::CopyToFolder, C::MoveToFolder, C::NewFolder, C::Delete]),
                run(&[C::ExportResults, C::Cancel]),
                run(&[C::Settings]),
                run(&[C::Exit]),
            ],
        ),
        menu(
            "menu.edit",
            vec![
                run(&[
                    C::Cut,
                    C::Copy,
                    C::Paste,
                    C::CopyAsPath,
                    C::CopyName,
                    C::CopyFolder,
                ]),
                run(&[C::SelectAll, C::ClearSelection]),
                run(&[C::Find]),
            ],
        ),
        menu(
            "menu.view",
            vec![
                run(&[C::TwoPanels]),
                vec![
                    Entry::Item(MenuItem::Run(C::Filters)),
                    Entry::Item(MenuItem::Run(C::Inspector)),
                    Entry::Submenu {
                        label: C::Columns.row().label,
                        groups: vec![Column::ALL.into_iter().map(MenuItem::Column).collect()],
                    },
                ],
                vec![Entry::Submenu {
                    label: C::SortBy.row().label,
                    groups: vec![
                        SORT_KEYS.into_iter().map(MenuItem::SortKey).collect(),
                        SORT_DIRECTIONS
                            .into_iter()
                            .map(MenuItem::SortDirection)
                            .collect(),
                    ],
                }],
                run(&[C::Refresh]),
                run(&[C::FileOperations]),
            ],
        ),
        menu("menu.help", vec![run(&[C::SearchSyntax]), run(&[C::About])]),
        Menu {
            label: C::Language.row().label,
            groups: vec![
                vec![Entry::Item(MenuItem::Language(None))],
                (0..LANGUAGES.len())
                    .map(|i| Entry::Item(MenuItem::Language(Some(i))))
                    .collect(),
            ],
            aligned_right: true,
        },
    ]
}

/// A menu as Windows builds it: each item with its ID, its text with the
/// shortcut after a tab, and its mark, and a separator between groups.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MenuNode {
    Item {
        id: u16,
        text: String,
        mark: Mark,
        glyph: Option<Glyph>,
    },
    Separator,
    Submenu {
        text: String,
        children: Vec<MenuNode>,
        aligned_right: bool,
    },
}

/// The header's context menu, opened on the header of `column` or on
/// none (DESIGN.md: Results list): Fit column for that column, then Fit
/// columns, then View > Columns' items (C15), as TableView's header menu
/// orders them.
pub(crate) fn header_menu(column: Option<Column>) -> Vec<Vec<MenuItem>> {
    let active: Vec<MenuItem> = column.map(MenuItem::FitColumn).into_iter().collect();
    [
        active,
        vec![MenuItem::FitColumns],
        Column::ALL.into_iter().map(MenuItem::Column).collect(),
    ]
    .into_iter()
    .filter(|group| !group.is_empty())
    .collect()
}

/// A row's context menu (DESIGN.md: Menus). Its first item, Open, is the
/// default, which a double-click and `Enter` also run.
pub(crate) fn row_menu() -> Vec<Vec<MenuItem>> {
    use CommandId as C;
    let run = |commands: &[CommandId]| commands.iter().map(|c| MenuItem::Run(*c)).collect();
    vec![
        run(&[C::Open, C::OpenFileLocation, C::Explore]),
        run(&[
            C::Cut,
            C::Copy,
            C::Paste,
            C::CopyAsPath,
            C::CopyName,
            C::CopyFolder,
            C::CopyToFolder,
            C::MoveToFolder,
            C::Delete,
        ]),
        run(&[C::SearchInThisFolder]),
        run(&[C::Properties]),
    ]
}

/// The row menu with the items that `menu` shows for a row that is a
/// folder when `folder`, with Shift down when `shift`, and without the
/// groups that are left empty (DESIGN.md: The Context menu page). Paste
/// shows only on a folder, which it pastes into.
pub(crate) fn shown_row_menu(menu: ContextMenu, folder: bool, shift: bool) -> Vec<Vec<MenuItem>> {
    row_menu()
        .into_iter()
        .map(|group| {
            group
                .into_iter()
                .filter(|item| match item {
                    MenuItem::Run(CommandId::Paste) => folder,
                    MenuItem::Run(command) => ContextItem::of(*command, folder)
                        .is_none_or(|item| menu.visibility(item).shows(shift)),
                    _ => true,
                })
                .collect::<Vec<_>>()
        })
        .filter(|group| !group.is_empty())
        .collect()
}

/// A panel row's context menu (file-manager-spec.md: Menus in Two panels).
/// Its first item, Open, is the default, which `Enter` also runs.
pub(crate) fn panel_row_menu() -> Vec<Vec<MenuItem>> {
    use CommandId as C;
    let run = |commands: &[CommandId]| commands.iter().map(|c| MenuItem::Run(*c)).collect();
    vec![
        run(&[C::Open, C::View]),
        run(&[C::CopyToFolder, C::MoveToFolder, C::Delete]),
        run(&[C::Properties]),
    ]
}

/// The context menu on the empty area of a panel's list.
pub(crate) fn panel_menu() -> Vec<Vec<MenuItem>> {
    vec![
        vec![MenuItem::Run(CommandId::NewFolder)],
        vec![MenuItem::Run(CommandId::Refresh)],
    ]
}

/// The tray icon's menu (DESIGN.md: Window lifetime). Its first item, Show
/// window, is the default, which a click on the icon also runs.
pub(crate) fn tray_menu() -> Vec<Vec<MenuItem>> {
    vec![
        vec![MenuItem::Run(CommandId::ShowWindow)],
        vec![MenuItem::Run(CommandId::Exit)],
    ]
}

/// A button that runs a command: a text button, or an icon-only one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandButton {
    pub(crate) control: ControlId,
    pub(crate) command: CommandId,
    /// The key of a text button's caption, of one or two words whose access
    /// key the main window's other controls leave free; `None` for an icon.
    pub(crate) caption: Option<&'static str>,
    /// The key of its tooltip, where the caption alone cannot say what it
    /// does; an icon without one has its command's name.
    pub(crate) tip: Option<&'static str>,
}

/// The inspector's buttons from the left, at the right end of the last
/// field row, with Open, the one used most, at the right edge (DESIGN.md:
/// Inspector pane).
pub(crate) const INSPECTOR_BUTTONS: [CommandButton; 3] = [
    CommandButton {
        control: ControlId::InspectorExplore,
        command: CommandId::Explore,
        caption: Some("inspector.explore"),
        tip: Some("inspector.explore_tooltip"),
    },
    CommandButton {
        control: ControlId::InspectorOpenFileLocation,
        command: CommandId::OpenFileLocation,
        caption: Some("inspector.open_file_location"),
        tip: Some("inspector.open_file_location_tooltip"),
    },
    CommandButton {
        control: ControlId::InspectorOpen,
        command: CommandId::Open,
        caption: Some("inspector.open"),
        tip: None,
    },
];

/// The inspector's Copy path, at the right end of the field row above the
/// buttons, because the full path has no line of its own.
pub(crate) const INSPECTOR_COPY_PATH: CommandButton = CommandButton {
    control: ControlId::InspectorCopyAsPath,
    command: CommandId::CopyAsPath,
    caption: Some("inspector.copy_as_path"),
    tip: None,
};

/// The copy icon after one entry's name.
pub(crate) const INSPECTOR_COPY_NAME: CommandButton = CommandButton {
    control: ControlId::InspectorCopyName,
    command: CommandId::CopyName,
    caption: None,
    tip: None,
};

/// The copy icon after one entry's folder.
pub(crate) const INSPECTOR_COPY_FOLDER: CommandButton = CommandButton {
    control: ControlId::InspectorCopyFolder,
    command: CommandId::CopyFolder,
    caption: None,
    tip: None,
};

/// Hide details, the icon at the right end of the inspector's heading row.
pub(crate) const HIDE_DETAILS: CommandButton = CommandButton {
    control: ControlId::InspectorHide,
    command: CommandId::Inspector,
    caption: None,
    tip: Some("inspector.hide"),
};

/// Show details, the icon at the right end of the status bar while the
/// inspector is hidden, where the inspector comes up from.
pub(crate) const SHOW_DETAILS: CommandButton = CommandButton {
    control: ControlId::StatusShowDetails,
    command: CommandId::Inspector,
    caption: None,
    tip: Some("inspector.show"),
};

/// `groups` as a context menu in the language of `strings`. A context menu
/// shows no shortcuts, as File Explorer's does (DESIGN.md: Menus).
pub(crate) fn render_popup(groups: &[Vec<MenuItem>], strings: &Strings) -> Vec<MenuNode> {
    separated(groups, |item| MenuNode::Item {
        id: item.id(),
        text: item.label(strings),
        mark: item.mark(),
        glyph: item.glyph(),
    })
}

/// A menu bar item, with the first key that runs it in `mode` after its
/// label.
pub(crate) fn node(
    item: &MenuItem,
    strings: &Strings,
    shortcuts: &CommandShortcuts,
    mode: Mode,
) -> MenuNode {
    MenuNode::Item {
        id: item.id(),
        text: menu_label(
            &item.label(strings),
            item.shortcut(shortcuts, mode).map(|s| s.text(strings)),
        ),
        mark: item.mark(),
        glyph: item.glyph(),
    }
}

/// The menu bar `menus` of `mode` in the language of `strings`, with the
/// shortcuts of `shortcuts`.
pub(crate) fn render(
    menus: &[Menu],
    strings: &Strings,
    shortcuts: &CommandShortcuts,
    mode: Mode,
) -> Vec<MenuNode> {
    let item = |item: &MenuItem| node(item, strings, shortcuts, mode);
    menus
        .iter()
        .map(|menu| MenuNode::Submenu {
            text: strings.text(menu.label).to_owned(),
            children: separated(&menu.groups, |entry| match entry {
                Entry::Item(i) => item(i),
                Entry::Submenu { label, groups } => MenuNode::Submenu {
                    text: strings.text(label).to_owned(),
                    children: separated(groups, item),
                    aligned_right: false,
                },
            }),
            aligned_right: menu.aligned_right,
        })
        .collect()
}

fn separated<T>(groups: &[Vec<T>], node: impl Fn(&T) -> MenuNode) -> Vec<MenuNode> {
    let mut nodes = Vec::new();
    for (i, group) in groups.iter().enumerate() {
        if i > 0 {
            nodes.push(MenuNode::Separator);
        }
        nodes.extend(group.iter().map(&node));
    }
    nodes
}

/// Every item of `menus`, submenus included, for setting their states.
pub(crate) fn items(menus: &[Menu]) -> Vec<MenuItem> {
    menus
        .iter()
        .flat_map(|menu| menu.groups.iter().flatten())
        .flat_map(|entry| match entry {
            Entry::Item(item) => vec![*item],
            Entry::Submenu { groups, .. } => groups.iter().flatten().copied().collect(),
        })
        .collect()
}

/// The accelerator table of `mode`: in Two panels its fixed keys first,
/// then every command shortcut of `shortcuts` whose command belongs to
/// `mode` and whose key the mode leaves free, with the item it runs.
pub(crate) fn accelerators(shortcuts: &CommandShortcuts, mode: Mode) -> Vec<(Keystroke, MenuItem)> {
    let fixed = match mode {
        Mode::Search => &[],
        Mode::TwoPanels => TWO_PANEL_KEYS,
    };
    let free = |key: &Keystroke| mode == Mode::Search || !kept_by_two_panels(*key);
    let shortcuts = CommandId::ALL
        .iter()
        .filter(|c| c.belongs_to(mode))
        .flat_map(|c| {
            shortcuts
                .of(*c)
                .iter()
                .filter(|key| free(key))
                .map(|s| (*s, MenuItem::Run(*c)))
        });
    fixed.iter().copied().chain(shortcuts).collect()
}

#[cfg(test)]
#[allow(
    clippy::single_range_in_vec_init,
    reason = "a selection is a list of row ranges"
)]
mod tests {
    use super::*;
    use crate::core::fetch::List;
    use crate::core::model::ExportFormat;
    use crate::core::model::tests::{rows, search, showing};
    use crate::core::results::{Fix, ListButton};
    use crate::core::text::{Strings, access_key};
    use std::collections::HashSet;

    /// `WM_COMMAND` carries only the ID, so two items with one ID, or an ID
    /// that decodes to another item, would run the wrong command.
    #[test]
    fn every_menu_item_id_names_its_item() {
        let mut seen = std::collections::HashMap::new();
        let all: Vec<MenuItem> = Mode::ALL
            .into_iter()
            .flat_map(|mode| {
                items(&menu_bar(mode)).into_iter().chain(
                    accelerators(&CommandShortcuts::default(), mode)
                        .into_iter()
                        .map(|(_, item)| item),
                )
            })
            .chain(CommandId::ALL.into_iter().map(MenuItem::Run))
            .collect();
        for item in all {
            assert_eq!(MenuItem::from_id(item.id()), Some(item));
            let earlier = seen.insert(item.id(), item);
            assert!(
                earlier.is_none_or(|earlier| earlier == item),
                "{item:?} has the ID of {earlier:?}"
            );
        }
    }

    /// The accelerator table runs the first command that has a key, so a
    /// second command with the same key would never run from it.
    #[test]
    fn no_two_commands_share_a_shortcut() {
        for mode in Mode::ALL {
            let mut seen = HashSet::new();
            for (shortcut, item) in accelerators(&CommandShortcuts::default(), mode) {
                assert!(
                    seen.insert((shortcut.ctrl, shortcut.alt, shortcut.shift, shortcut.key)),
                    "{item:?} repeats {shortcut:?} in {mode:?}"
                );
            }
        }
    }

    /// Two panels hides Search's list, so a Search command that ran there,
    /// such as `F5` Refresh or `Ctrl+A` Select all, would act on results the
    /// person cannot see; `F5` there is Copy to folder… (file-manager-spec.md:
    /// Keys).
    #[test]
    fn each_mode_takes_only_its_own_commands_and_keys() {
        let shortcuts = CommandShortcuts::default();
        let takes = |mode, key| {
            accelerators(&shortcuts, mode)
                .into_iter()
                .find(|(stroke, _)| *stroke == key)
                .map(|(_, item)| item)
        };
        let refresh = Keystroke::plain(Key::Function(5));
        let settings = Keystroke::ctrl(Key::Comma);
        assert_eq!(
            takes(Mode::Search, refresh),
            Some(MenuItem::Run(CommandId::Refresh))
        );
        assert_eq!(
            takes(Mode::TwoPanels, refresh),
            Some(MenuItem::Run(CommandId::CopyToFolder))
        );
        for mode in Mode::ALL {
            assert_eq!(
                takes(mode, settings),
                Some(MenuItem::Run(CommandId::Settings))
            );
        }
        // Find starts a search in a panel (file-manager-spec.md: Search in
        // a panel), with Total Commander's `Alt+F7` besides `Ctrl+F`.
        let find = Some(MenuItem::Run(CommandId::Find));
        assert_eq!(
            takes(Mode::TwoPanels, Keystroke::alt(Key::Function(7))),
            find
        );
        for mode in Mode::ALL {
            assert_eq!(takes(mode, Keystroke::ctrl(Key::Letter(b'F'))), find);
        }
        let shown = |mode, command| items(&menu_bar(mode)).contains(&MenuItem::Run(command));
        assert!(shown(Mode::Search, CommandId::Refresh));
        assert!(!shown(Mode::TwoPanels, CommandId::Copy));
        for command in [CommandId::TwoPanels, CommandId::Find] {
            assert!(shown(Mode::Search, command) && shown(Mode::TwoPanels, command));
        }
    }

    /// Tab in a panel's controls switches panels; otherwise Search's Tab
    /// rule would move the focus to Search's hidden search box or list
    /// (file-manager-spec.md: Keys).
    #[test]
    fn tab_in_a_panel_switches_panels() {
        let tab = Keystroke::plain(Key::Tab);
        let back = Keystroke { shift: true, ..tab };
        for focus in [
            Focus::Panel,
            Focus::PathBox,
            Focus::PanelSearch,
            Focus::LookIn,
        ] {
            for key in [tab, back] {
                assert_eq!(
                    control_key(focus, key),
                    Some(KeyAction::Panel(PanelKey::OtherPanel))
                );
            }
        }
        assert_eq!(
            control_key(Focus::List, back),
            Some(KeyAction::MoveTab { backward: true })
        );
    }

    /// An access key that two items of one menu share opens the first only,
    /// and a main-window letter that a menu also uses never opens that menu
    /// (DESIGN.md: Words). Every shipped language chooses its own letters.
    #[test]
    fn access_keys_are_unique_in_every_menu_and_language() {
        fn assert_unique(
            tag: &str,
            labels: impl IntoIterator<Item = impl AsRef<str>>,
            keys: &mut HashSet<char>,
        ) {
            for label in labels {
                let label = label.as_ref();
                assert!(
                    access_key(label).is_some_and(|k| keys.insert(k)),
                    "{tag}: {label} has no access key, or one its menu already uses"
                );
            }
        }
        for language in LANGUAGES {
            let strings = Strings::load(language);
            let mut bar_keys = HashSet::new();
            assert_unique(
                language.tag,
                menu_bar(Mode::TwoPanels)
                    .iter()
                    .map(|m| strings.text(m.label)),
                &mut HashSet::new(),
            );
            let bar = menu_bar(Mode::Search);
            assert_unique(
                language.tag,
                bar.iter().map(|m| strings.text(m.label)),
                &mut bar_keys,
            );
            for menu in bar.iter().chain(&menu_bar(Mode::TwoPanels)) {
                // A submenu's label shares its menu with the items beside it.
                let labels = menu.groups.iter().flatten().map(|entry| match entry {
                    Entry::Item(item) => item.label(&strings),
                    Entry::Submenu { label, .. } => strings.text(label).to_owned(),
                });
                assert_unique(language.tag, labels, &mut HashSet::new());
                for entry in menu.groups.iter().flatten() {
                    if let Entry::Submenu { groups, .. } = entry {
                        let labels = groups.iter().flatten().map(|item| item.label(&strings));
                        assert_unique(language.tag, labels, &mut HashSet::new());
                    }
                }
            }
            for menu in [row_menu(), tray_menu(), panel_row_menu(), panel_menu()] {
                let labels = menu.into_iter().flatten().map(|item| item.label(&strings));
                assert_unique(language.tag, labels, &mut HashSet::new());
            }
            let window = [
                "match.label",
                "match.case",
                "match.path",
                "filter.clear",
                "filter.kind",
                "filter.location",
                "filter.choose_folder",
                "filter.only_directly",
                "filter.date",
                "filter.size",
                "filter.attributes",
            ]
            .into_iter()
            .chain(
                INSPECTOR_BUTTONS
                    .iter()
                    .chain([&INSPECTOR_COPY_PATH])
                    .filter_map(|button| button.caption),
            )
            .map(|key| strings.text(key));
            assert_unique(language.tag, window, &mut bar_keys);
            // The list's message offers Clear filters and Choose folder…
            // with the pane's access keys, which do what the pane's buttons
            // do.
            assert_unique(
                language.tag,
                [strings.text(ListButton::SearchEverywhere.label())],
                &mut bar_keys,
            );
            // The list shows one message with the fix buttons that show
            // together.
            for shown in Fix::label_groups() {
                let buttons = shown.iter().map(|(key, _)| strings.text(key));
                assert_unique(language.tag, buttons, &mut bar_keys.clone());
            }
        }
    }

    fn enabled(model: &Model, command: CommandId) -> bool {
        MenuItem::Run(command).enabled(&State {
            model,
            focus: Focus::List,
        })
    }

    /// File Explorer asks before it opens more than 15 items, and a command
    /// that acts on rows not read yet would act on files nobody chose
    /// (DESIGN.md: C1, C2, C3, S12).
    #[test]
    fn the_selection_commands_follow_the_number_of_files_read() {
        let mut model = showing(50_000);
        for (selected, opens, shows_properties) in
            [(1, true, true), (15, true, false), (16, false, false)]
        {
            model.set_selection(List::Search, vec![0..selected]);
            assert_eq!(enabled(&model, CommandId::Open), opens, "{selected} rows");
            assert_eq!(enabled(&model, CommandId::OpenFileLocation), opens);
            assert_eq!(enabled(&model, CommandId::Properties), shows_properties);
            assert!(enabled(&model, CommandId::CopyAsPath));
        }
        model.set_selection(List::Search, vec![0..1, 40_000..40_001]);
        for command in [CommandId::Open, CommandId::Copy, CommandId::Properties] {
            assert!(
                !enabled(&model, command),
                "{command:?} while the selection is read"
            );
        }
    }

    /// The key bar and the menus offer a command only when it acts on
    /// something: Copy, Move and Delete on the selected entries, or the
    /// focused row but never `..`; New folder on a folder that shows; none
    /// of them in a second window, which owns no file operations
    /// (file-manager-spec.md: Commands in Two panels).
    #[test]
    fn two_panels_file_commands_follow_the_chosen_entries() {
        use crate::core::model::tests::model;
        use crate::core::panel::tests::{file, list};
        let mut model = model();
        model.show_mode(Mode::TwoPanels);
        model.offer_file_operations();
        list(
            model.panels_mut(),
            r"C:\f",
            vec![file("a.txt", 1, 0), file("b.txt", 1, 0)],
        );
        let file_commands = [
            CommandId::CopyToFolder,
            CommandId::MoveToFolder,
            CommandId::Delete,
        ];
        let offered = |model: &Model| file_commands.map(|command| enabled(model, command));
        // `..` alone.
        model.panels_mut().set_focused(Side::Left, Some(0));
        assert_eq!(offered(&model), [false; 3]);
        model.panels_mut().set_focused(Side::Left, Some(1));
        assert_eq!(offered(&model), [true; 3]);
        model.panels_mut().set_focused(Side::Left, Some(0));
        model.panels_mut().set_selected(Side::Left, vec![2..3]);
        assert_eq!(offered(&model), [true; 3]);
        assert!(enabled(&model, CommandId::NewFolder));
        model.panels_mut().drive_removed(Side::Left);
        assert!(!enabled(&model, CommandId::NewFolder));

        let mut second = crate::core::model::tests::model();
        second.show_mode(Mode::TwoPanels);
        list(second.panels_mut(), r"C:\f", vec![file("a.txt", 1, 0)]);
        second.panels_mut().set_focused(Side::Left, Some(1));
        assert_eq!(offered(&second), [false; 3]);
        assert!(!enabled(&second, CommandId::NewFolder));
    }

    /// A second export would write the same file twice at once.
    #[test]
    fn export_results_is_disabled_while_an_export_runs() {
        let mut model = showing(10);
        assert!(enabled(&model, CommandId::ExportResults));
        let order = model.column_order();
        model.start_export(r"D:\results.csv".into(), ExportFormat::Csv, &order);
        assert!(!enabled(&model, CommandId::ExportResults));
        assert!(enabled(&model, CommandId::Cancel));
    }

    /// T11: a context menu stays open while new results arrive, and its
    /// command must not act on rows of the new search that nobody selected.
    #[test]
    fn t11_a_command_chosen_after_new_rows_cleared_the_selection_does_nothing() {
        let mut model = showing(10);
        model.set_selection(List::Search, vec![2..3]);
        assert!(
            enabled(&model, CommandId::Open),
            "the menu opens with it on"
        );
        let fetch = search(&mut model, "repo");
        model.accept(rows(&fetch, 3));
        assert!(!enabled(&model, CommandId::Open));
    }
}
