//! The command table: one row for each command of the spec's Commands table,
//! and the menu bar built from the rows (design rule 9). Every surface reads
//! a command's label, shortcut, enabled state and check mark from here, so
//! the menus, the keys and the controls cannot disagree.

use super::model::{Column, Model};
use super::query::{SORT_DIRECTIONS, SORT_KEYS, sort_direction_label, sort_key_label};
use super::text::{LANGUAGES, LanguageChoice, Strings, menu_label};
use gozler_core::types::{SortDir, SortKey};

/// One command of the spec's Commands table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        let (label, shortcuts, enabled, checked): (_, &'static [Shortcut], Enabled, Checked) =
            match self {
                C::Open => ("command.open", &[], never, None),
                C::OpenFileLocation => (
                    "command.open_file_location",
                    const { &[Shortcut::ctrl(Key::Enter)] },
                    never,
                    None,
                ),
                C::Properties => (
                    "command.properties",
                    const { &[Shortcut::alt(Key::Enter)] },
                    never,
                    None,
                ),
                C::ExportResults => ("command.export_results", &[], never, None),
                C::CancelExport => ("command.cancel_export", &[], never, None),
                C::Settings => (
                    "command.settings",
                    const { &[Shortcut::ctrl(Key::Comma)] },
                    never,
                    None,
                ),
                C::Exit => ("command.exit", &[], always, None),
                C::Copy => (
                    "command.copy",
                    const { &[Shortcut::ctrl(Key::Letter(b'C'))] },
                    search_text_selected,
                    None,
                ),
                C::CopyAsPath => (
                    "command.copy_as_path",
                    const { &[Shortcut::ctrl_shift(Key::Letter(b'C'))] },
                    never,
                    None,
                ),
                C::CopyName => ("command.copy_name", &[], never, None),
                C::SelectAll => (
                    "command.select_all",
                    const { &[Shortcut::ctrl(Key::Letter(b'A'))] },
                    search_box_focused,
                    None,
                ),
                C::SearchInThisFolder => ("command.search_in_this_folder", &[], never, None),
                C::Filters => (
                    "command.filters",
                    &[],
                    never,
                    Some(|s| s.model.panes.filters),
                ),
                C::Inspector => (
                    "command.inspector",
                    &[],
                    never,
                    Some(|s| s.model.panes.inspector),
                ),
                C::Columns => ("command.columns", &[], always, None),
                C::Language => ("menu.language", &[], never, None),
                C::Refresh => (
                    "command.refresh",
                    const { &[Shortcut::plain(Key::Function(5))] },
                    searched,
                    None,
                ),
                C::Find => (
                    "command.find",
                    const {
                        &[
                            Shortcut::ctrl(Key::Letter(b'F')),
                            Shortcut::ctrl(Key::Letter(b'E')),
                        ]
                    },
                    always,
                    None,
                ),
                C::Match => ("match.label", &[], always, None),
                C::SortBy => ("command.sort_by", &[], always, None),
                C::ClearFilters => ("command.clear_filters", &[], never, None),
                C::SearchSyntax => (
                    "command.search_syntax",
                    const { &[Shortcut::plain(Key::Function(1))] },
                    always,
                    None,
                ),
                C::About => ("command.about", &[], always, None),
                C::ShowWindow => ("command.show_window", &[], never, None),
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
    pub(crate) shortcuts: &'static [Shortcut],
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
    SearchBox { text_selected: bool },
    Other,
}

fn always(_: &State) -> bool {
    true
}

/// A command the window cannot carry out yet. It stays in its menu,
/// disabled, so the menus never change shape (spec: Menus).
fn never(_: &State) -> bool {
    false
}

/// Refresh runs the shown search again (spec: C17).
fn searched(state: &State) -> bool {
    state.model.results.has_searched()
}

/// Copy and Select all act on the focused control (spec: C8, C11).
fn search_box_focused(state: &State) -> bool {
    matches!(state.focus, Focus::SearchBox { .. })
}

fn search_text_selected(state: &State) -> bool {
    state.focus
        == Focus::SearchBox {
            text_selected: true,
        }
}

/// A key combination that runs a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shortcut {
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) key: Key,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    /// An ASCII capital letter, which is also its virtual-key code.
    Letter(u8),
    /// F1 to F12.
    Function(u8),
    Enter,
    Comma,
}

impl Shortcut {
    const fn plain(key: Key) -> Shortcut {
        Shortcut {
            ctrl: false,
            alt: false,
            shift: false,
            key,
        }
    }

    const fn ctrl(key: Key) -> Shortcut {
        Shortcut {
            ctrl: true,
            ..Shortcut::plain(key)
        }
    }

    const fn alt(key: Key) -> Shortcut {
        Shortcut {
            alt: true,
            ..Shortcut::plain(key)
        }
    }

    const fn ctrl_shift(key: Key) -> Shortcut {
        Shortcut {
            ctrl: true,
            shift: true,
            ..Shortcut::plain(key)
        }
    }

    /// As a menu shows it, such as "Ctrl+Shift+C", with the modifier names
    /// of the language, because German Windows writes "Strg".
    pub(crate) fn text(self, strings: &Strings) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (on, key) in [
            (self.ctrl, "key.ctrl"),
            (self.alt, "key.alt"),
            (self.shift, "key.shift"),
        ] {
            if on {
                parts.push(strings.text(key).to_owned());
            }
        }
        parts.push(match self.key {
            Key::Letter(letter) => char::from(letter).to_string(),
            Key::Function(n) => format!("F{n}"),
            Key::Enter => strings.text("key.enter").to_owned(),
            Key::Comma => ",".to_owned(),
        });
        parts.join("+")
    }
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

    /// The shortcut the menu shows after the label.
    pub(crate) fn shortcut(self) -> Option<Shortcut> {
        match self {
            MenuItem::Run(command) => command.row().shortcuts.first().copied(),
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
        // Name cannot be hidden (spec: C15).
        (self.command().row().enabled)(state) && self != MenuItem::Column(Column::Name)
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
            MenuItem::SortKey(key) => model.sort.key == key,
            MenuItem::SortDirection(dir) => model.sort.dir == dir,
            MenuItem::Language(None) => model.settings.choices.language == LanguageChoice::Windows,
            MenuItem::Language(Some(language)) => matches!(
                &model.settings.choices.language,
                LanguageChoice::Tag(tag) if tag.eq_ignore_ascii_case(LANGUAGES[language].tag)
            ),
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

/// The menu bar, in the spec's order and groups (spec: Menus).
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

/// The header's context menu: View > Columns' items (spec: C15). A context
/// menu shows no shortcuts, and these have none.
pub(crate) fn header_menu() -> Vec<MenuItem> {
    Column::ALL.into_iter().map(MenuItem::Column).collect()
}

/// `items` as a popup menu in the language of `strings`.
pub(crate) fn render_popup(items: &[MenuItem], strings: &Strings) -> Vec<MenuNode> {
    items.iter().map(|item| node(item, strings)).collect()
}

fn node(item: &MenuItem, strings: &Strings) -> MenuNode {
    MenuNode::Item {
        id: item.id(),
        text: menu_label(
            item.label(strings),
            item.shortcut().map(|s| s.text(strings)),
        ),
        mark: item.mark(),
    }
}

/// The menu bar `menus` in the language of `strings`.
pub(crate) fn render(menus: &[Menu], strings: &Strings) -> Vec<MenuNode> {
    let item = |item: &MenuItem| node(item, strings);
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

/// Every command shortcut, with the item it runs.
pub(crate) fn accelerators() -> Vec<(Shortcut, MenuItem)> {
    CommandId::ALL
        .iter()
        .flat_map(|c| c.row().shortcuts.iter().map(|s| (*s, MenuItem::Run(*c))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::results::Fix;
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
            .chain(accelerators().into_iter().map(|(_, item)| item))
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
        for (shortcut, item) in accelerators() {
            assert!(
                seen.insert((shortcut.ctrl, shortcut.alt, shortcut.shift, shortcut.key)),
                "{item:?} repeats {shortcut:?}"
            );
        }
    }

    /// An access key that two items of one menu share opens the first only,
    /// and a main-window letter that a menu also uses never opens that menu
    /// (spec: Words). Every shipped language chooses its own letters.
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
                // A submenu's label shares its menu with the items beside it.
                let labels = menu.groups.iter().flatten().map(|entry| match entry {
                    Entry::Item(item) => item.label(&strings),
                    Entry::Submenu { label, .. } => strings.text(label),
                });
                assert_unique(language.tag, labels, &mut HashSet::new());
                for entry in menu.groups.iter().flatten() {
                    if let Entry::Submenu { groups, .. } = entry {
                        let labels = groups.iter().flatten().map(|item| item.label(&strings));
                        assert_unique(language.tag, labels, &mut HashSet::new());
                    }
                }
            }
            let window = ["match.label", "match.case", "match.path"].map(|key| strings.text(key));
            assert_unique(language.tag, window, &mut bar_keys);
            // The list shows one message, with at most one of these buttons.
            let fixes = [
                Fix::Repair(Repair::StartService),
                Fix::Repair(Repair::TurnOnService),
                Fix::Repair(Repair::InstallService {
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
}
