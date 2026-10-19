//! The command table: one row for each command of DESIGN.md's Commands table,
//! and the menu bar built from the rows (DESIGN.md: window rule 9). Every
//! surface reads a command's label, shortcut, enabled state and check mark from
//! here, so the menus, the keys and the controls cannot disagree.

use super::model::{Column, Model};
use super::query::{SORT_DIRECTIONS, SORT_KEYS, sort_direction_label, sort_key_label};
use super::selection::OPEN_LIMIT;
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
    Properties,
    ExportResults,
    CancelExport,
    Settings,
    Exit,
    Copy,
    CopyAsPath,
    CopyName,
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
}

impl CommandId {
    pub(crate) const ALL: [CommandId; 24] = [
        CommandId::Open,
        CommandId::OpenFileLocation,
        CommandId::Properties,
        CommandId::ExportResults,
        CommandId::CancelExport,
        CommandId::Settings,
        CommandId::Exit,
        CommandId::Copy,
        CommandId::CopyAsPath,
        CommandId::CopyName,
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
                C::CancelExport => ("command.cancel_export", &[], |s| s.model.exporting(), None),
                C::Settings => (
                    "command.settings",
                    const { &[Keystroke::ctrl(Key::Comma)] },
                    always,
                    None,
                ),
                C::Exit => ("command.exit", &[], always, None),
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
            };
        Command {
            label,
            shortcuts,
            enabled,
            checked,
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
    Other,
}

fn always(_: &State) -> bool {
    true
}

/// Refresh runs the shown search again (DESIGN.md: C17).
fn searched(state: &State) -> bool {
    state.model.results.has_searched()
}

/// The selected entries, once the selection is read (DESIGN.md: The selection).
fn files(state: &State) -> bool {
    state.model.selected_files().is_some()
}

/// Open and Open file location (DESIGN.md: C1, C2).
fn open(state: &State) -> bool {
    state
        .model
        .selected_files()
        .is_some_and(|files| files.len() <= OPEN_LIMIT)
}

/// Properties shows one entry, and Search in this folder searches its
/// folder (DESIGN.md: C3, C12).
fn one_file(state: &State) -> bool {
    state
        .model
        .selected_files()
        .is_some_and(|files| files.len() == 1)
}

/// Copy acts on the focused control (DESIGN.md: C8).
fn copy(state: &State) -> bool {
    match state.focus {
        Focus::SearchBox { text_selected } | Focus::Value { text_selected } => text_selected,
        Focus::List => files(state),
        Focus::RangeFilter | Focus::Other => false,
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

/// What a key does in the focused control (DESIGN.md: window rule 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    Run(CommandId),
    /// `Down` in the search box: the focus moves to the list's first row.
    FocusList,
    /// `Enter` in the search box: the search runs at once.
    SearchNow,
    /// `Esc` in the search box: the box is emptied.
    ClearSearch,
    /// `Enter` in a closed Date modified or Size list.
    ChooseCustom,
}

/// The meaning `key` has in the focused control, which the window takes
/// before `IsDialogMessageW` turns `Enter` and `Esc` into `IDOK` and
/// `IDCANCEL`, and before the control itself sees the key. The message loop
/// translates a command's shortcut first, so no shortcut reaches this table.
pub(crate) fn control_key(focus: Focus, key: Keystroke) -> Option<KeyAction> {
    let table: &[(Keystroke, KeyAction)] = match focus {
        Focus::SearchBox { .. } => {
            const {
                &[
                    (Keystroke::plain(Key::Down), KeyAction::FocusList),
                    (Keystroke::plain(Key::Enter), KeyAction::SearchNow),
                    (Keystroke::plain(Key::Escape), KeyAction::ClearSearch),
                ]
            }
        }
        Focus::List => {
            const {
                &[(
                    Keystroke::plain(Key::Enter),
                    KeyAction::Run(CommandId::Open),
                )]
            }
        }
        Focus::RangeFilter => const { &[(Keystroke::plain(Key::Enter), KeyAction::ChooseCustom)] },
        Focus::Value { .. } | Focus::Other => &[],
    };
    table
        .iter()
        .find(|(stroke, _)| *stroke == key)
        .map(|(_, action)| *action)
}

/// One clickable menu item: a command, or one choice of a command that has
/// choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuItem {
    Run(CommandId),
    Column(Column),
    SortKey(SortKey),
    SortDirection(SortDir),
    /// `None` is Windows default, else an index into [`LANGUAGES`].
    Language(Option<usize>),
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
            _ => None,
        }
    }

    /// Its label, with its access key.
    pub(crate) fn label(self, strings: &Strings) -> &str {
        match self {
            MenuItem::Run(command) => strings.text(command.row().label),
            MenuItem::Column(column) => strings.text(column.menu_label()),
            MenuItem::SortKey(key) => strings.text(sort_key_label(key)),
            MenuItem::SortDirection(dir) => strings.text(sort_direction_label(dir)),
            MenuItem::Language(None) => strings.text("command.windows_default"),
            MenuItem::Language(Some(language)) => LANGUAGES[language].name,
        }
    }

    /// The shortcut the menu shows after the label: the first of the
    /// command's shortcuts in `shortcuts`, the table its accelerators come
    /// from.
    pub(crate) fn shortcut(self, shortcuts: &CommandShortcuts) -> Option<Keystroke> {
        match self {
            MenuItem::Run(command) => shortcuts.of(command).first().copied(),
            _ => None,
        }
    }

    /// The command whose row decides whether this item is enabled.
    fn command(self) -> CommandId {
        match self {
            MenuItem::Run(command) => command,
            MenuItem::Column(_) => CommandId::Columns,
            MenuItem::SortKey(_) | MenuItem::SortDirection(_) => CommandId::SortBy,
            MenuItem::Language(_) => CommandId::Language,
        }
    }

    pub(crate) fn enabled(self, state: &State) -> bool {
        let shown = match self {
            MenuItem::Column(column) => column.can_hide(),
            _ => true,
        };
        (self.command().row().enabled)(state) && shown
    }

    pub(crate) fn mark(self) -> Mark {
        match self {
            MenuItem::Run(command) => match command.row().checked {
                Some(_) => Mark::Check,
                None => Mark::None,
            },
            MenuItem::Column(_) => Mark::Check,
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
            MenuItem::SortKey(key) => model.sort().key == key,
            MenuItem::SortDirection(dir) => model.sort().dir == dir,
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

/// A menu of the menu bar; a separator divides its groups.
pub(crate) struct Menu {
    pub(crate) label: &'static str,
    pub(crate) groups: Vec<Vec<Entry>>,
    /// The Language menu sits at the right end of the menu bar.
    pub(crate) aligned_right: bool,
}

/// The menu bar, in its order and groups (DESIGN.md: Menus).
pub(crate) fn menu_bar() -> Vec<Menu> {
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
                run(&[C::Open, C::OpenFileLocation, C::Properties]),
                run(&[C::ExportResults]),
                run(&[C::Settings]),
                run(&[C::Exit]),
            ],
        ),
        menu(
            "menu.edit",
            vec![
                run(&[C::Copy, C::CopyAsPath, C::CopyName]),
                run(&[C::SelectAll]),
                run(&[C::Find]),
            ],
        ),
        menu(
            "menu.view",
            vec![
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
    },
    Separator,
    Submenu {
        text: String,
        children: Vec<MenuNode>,
        aligned_right: bool,
    },
}

/// The header's context menu: View > Columns' items (DESIGN.md: C15).
pub(crate) fn header_menu() -> Vec<Vec<MenuItem>> {
    vec![Column::ALL.into_iter().map(MenuItem::Column).collect()]
}

/// A row's context menu (DESIGN.md: Menus). Its first item, Open, is the
/// default, which a double-click and `Enter` also run.
pub(crate) fn row_menu() -> Vec<Vec<MenuItem>> {
    use CommandId as C;
    let run = |commands: &[CommandId]| commands.iter().map(|c| MenuItem::Run(*c)).collect();
    vec![
        run(&[C::Open, C::OpenFileLocation]),
        run(&[C::Copy, C::CopyAsPath, C::CopyName]),
        run(&[C::SearchInThisFolder]),
        run(&[C::Properties]),
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

/// The inspector's buttons: each runs its command, and its caption differs
/// from the command's menu label only in an access key that the main
/// window's other controls leave free (DESIGN.md: Inspector pane).
pub(crate) const INSPECTOR_BUTTONS: [(CommandId, &str); 3] = [
    (CommandId::Open, "inspector.open"),
    (CommandId::OpenFileLocation, "inspector.open_file_location"),
    (CommandId::CopyAsPath, "inspector.copy_as_path"),
];

/// The item that File shows in Export results…' place: Cancel export while
/// an export runs (DESIGN.md: Menus).
pub(crate) fn export_slot(model: &Model) -> MenuItem {
    MenuItem::Run(if model.exporting() {
        CommandId::CancelExport
    } else {
        CommandId::ExportResults
    })
}

/// `groups` as a context menu in the language of `strings`. A context menu
/// shows no shortcuts, as File Explorer's does (DESIGN.md: Menus).
pub(crate) fn render_popup(groups: &[Vec<MenuItem>], strings: &Strings) -> Vec<MenuNode> {
    separated(groups, |item| MenuNode::Item {
        id: item.id(),
        text: item.label(strings).to_owned(),
        mark: item.mark(),
    })
}

/// A menu bar item, with its first shortcut in `shortcuts` after its label.
pub(crate) fn node(item: &MenuItem, strings: &Strings, shortcuts: &CommandShortcuts) -> MenuNode {
    MenuNode::Item {
        id: item.id(),
        text: menu_label(
            item.label(strings),
            item.shortcut(shortcuts).map(|s| s.text(strings)),
        ),
        mark: item.mark(),
    }
}

/// The menu bar `menus` in the language of `strings`, with the shortcuts of
/// `shortcuts`.
pub(crate) fn render(
    menus: &[Menu],
    strings: &Strings,
    shortcuts: &CommandShortcuts,
) -> Vec<MenuNode> {
    let item = |item: &MenuItem| node(item, strings, shortcuts);
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

/// Every command shortcut of `shortcuts`, with the item it runs.
pub(crate) fn accelerators(shortcuts: &CommandShortcuts) -> Vec<(Keystroke, MenuItem)> {
    CommandId::ALL
        .iter()
        .flat_map(|c| shortcuts.of(*c).iter().map(|s| (*s, MenuItem::Run(*c))))
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::single_range_in_vec_init,
    reason = "a selection is a list of row ranges"
)]
mod tests {
    use super::*;
    use crate::core::model::ExportFormat;
    use crate::core::model::tests::{rows, search, showing};
    use crate::core::results::{Fix, ListButton};
    use crate::core::text::{Strings, access_key};
    use gozler_client::Repair;
    use std::collections::HashSet;

    /// `WM_COMMAND` carries only the ID, so two items with one ID, or an ID
    /// that decodes to another item, would run the wrong command.
    #[test]
    fn every_menu_item_id_names_its_item() {
        let mut seen = std::collections::HashMap::new();
        let all: Vec<MenuItem> = items(&menu_bar())
            .into_iter()
            .chain(
                accelerators(&CommandShortcuts::default())
                    .into_iter()
                    .map(|(_, item)| item),
            )
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
        let mut seen = HashSet::new();
        for (shortcut, item) in accelerators(&CommandShortcuts::default()) {
            assert!(
                seen.insert((shortcut.ctrl, shortcut.alt, shortcut.shift, shortcut.key)),
                "{item:?} repeats {shortcut:?}"
            );
        }
    }

    /// An access key that two items of one menu share opens the first only,
    /// and a main-window letter that a menu also uses never opens that menu
    /// (DESIGN.md: Words). Every shipped language chooses its own letters.
    #[test]
    fn access_keys_are_unique_in_every_menu_and_language() {
        fn assert_unique<'a>(
            tag: &str,
            labels: impl IntoIterator<Item = &'a str>,
            keys: &mut HashSet<char>,
        ) {
            for label in labels {
                assert!(
                    access_key(label).is_some_and(|k| keys.insert(k)),
                    "{tag}: {label} has no access key, or one its menu already uses"
                );
            }
        }
        for language in LANGUAGES {
            let strings = Strings::load(language);
            let bar = menu_bar();
            let mut bar_keys = HashSet::new();
            assert_unique(
                language.tag,
                bar.iter().map(|m| strings.text(m.label)),
                &mut bar_keys,
            );
            for menu in &bar {
                // A submenu's label shares its menu with the items beside it,
                // and Cancel export shares File with them in Export
                // results…' place.
                let labels = menu.groups.iter().flatten().map(|entry| match entry {
                    Entry::Item(item) => item.label(&strings),
                    Entry::Submenu { label, .. } => strings.text(label),
                });
                let cancel = (menu.label == "menu.file")
                    .then(|| MenuItem::Run(CommandId::CancelExport).label(&strings));
                assert_unique(language.tag, labels.chain(cancel), &mut HashSet::new());
                for entry in menu.groups.iter().flatten() {
                    if let Entry::Submenu { groups, .. } = entry {
                        let labels = groups.iter().flatten().map(|item| item.label(&strings));
                        assert_unique(language.tag, labels, &mut HashSet::new());
                    }
                }
            }
            for menu in [row_menu(), tray_menu()] {
                let labels = menu.into_iter().flatten().map(|item| item.label(&strings));
                assert_unique(language.tag, labels, &mut HashSet::new());
            }
            let window = [
                "match.label",
                "match.case",
                "match.path",
                "command.clear_filters",
                "filter.kind",
                "filter.location",
                "filter.choose_folder",
                "filter.only_directly",
                "filter.date",
                "filter.size",
                "filter.attributes",
            ]
            .into_iter()
            .chain(INSPECTOR_BUTTONS.map(|(_, key)| key))
            .map(|key| strings.text(key));
            assert_unique(language.tag, window, &mut bar_keys);
            // The list's message offers Clear filters and Choose folder…
            // with the pane's captions, which do what the pane's buttons do.
            assert_unique(
                language.tag,
                [strings.text(ListButton::SearchEverywhere.label())],
                &mut bar_keys,
            );
            // The list shows one message, with at most one of these buttons.
            let fixes = [
                Fix::Repair(Repair::StartService),
                Fix::Repair(Repair::TurnOnService),
                Fix::Repair(Repair::InstallService {
                    folder: Default::default(),
                }),
                Fix::Repair(Repair::UpdateService {
                    folder: Default::default(),
                }),
                Fix::Repair(Repair::Download),
                Fix::OpenServices,
            ];
            for fix in fixes {
                let button = [strings.text(fix.label())];
                assert_unique(language.tag, button, &mut bar_keys.clone());
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
            model.set_selection(vec![0..selected]);
            assert_eq!(enabled(&model, CommandId::Open), opens, "{selected} rows");
            assert_eq!(enabled(&model, CommandId::OpenFileLocation), opens);
            assert_eq!(enabled(&model, CommandId::Properties), shows_properties);
            assert!(enabled(&model, CommandId::CopyAsPath));
        }
        model.set_selection(vec![0..1, 40_000..40_001]);
        for command in [CommandId::Open, CommandId::Copy, CommandId::Properties] {
            assert!(
                !enabled(&model, command),
                "{command:?} while the selection is read"
            );
        }
    }

    /// A second export would write the same file twice at once.
    #[test]
    fn export_results_is_disabled_while_an_export_runs() {
        let mut model = showing(10);
        assert!(enabled(&model, CommandId::ExportResults));
        let order = model.column_order();
        model.start_export(r"D:\results.csv".into(), ExportFormat::Csv, &order);
        assert!(!enabled(&model, CommandId::ExportResults));
        assert!(enabled(&model, CommandId::CancelExport));
        assert_eq!(export_slot(&model), MenuItem::Run(CommandId::CancelExport));
    }

    /// T11: a context menu stays open while new results arrive, and its
    /// command must not act on rows of the new search that nobody selected.
    #[test]
    fn t11_a_command_chosen_after_new_rows_cleared_the_selection_does_nothing() {
        let mut model = showing(10);
        model.set_selection(vec![2..3]);
        assert!(
            enabled(&model, CommandId::Open),
            "the menu opens with it on"
        );
        let fetch = search(&mut model, "repo");
        model.accept(rows(&fetch, 3));
        assert!(!enabled(&model, CommandId::Open));
    }
}
