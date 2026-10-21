//! The person's shortcuts (DESIGN.md: The Keyboard page): the shortcuts of the
//! window's commands, which the menus and the accelerator table read, and
//! the one shortcut that shows Gozler from any program. Both are saved in
//! `ui.json` by the names of [`Keystroke::name`], which are the same in
//! every language.

use super::commands::{CommandId, Entry, Key, Keystroke, MenuItem, menu_bar, row_menu};
use super::settings::Mode;
use super::text::{Strings, without_access_key};
use serde::de::Error as _;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// Every command that takes a shortcut, each with the catalogue key of the
/// menu that holds it: each command of either mode's menu bar, Cancel export
/// in Export results…' place, and the context menu's commands that no menu
/// of a bar holds (DESIGN.md: The Keyboard page). A submenu of choices, the
/// match controls, Clear filters and the tray's Show window take none.
pub(crate) fn listed_commands() -> Vec<(CommandId, &'static str)> {
    let mut listed: Vec<(CommandId, &'static str)> = Vec::new();
    for menu in Mode::ALL.into_iter().flat_map(menu_bar) {
        for entry in menu.groups.iter().flatten() {
            if let Entry::Item(MenuItem::Run(command)) = entry
                && !listed.iter().any(|(c, _)| c == command)
            {
                listed.push((*command, menu.label));
                if *command == CommandId::ExportResults {
                    listed.push((CommandId::CancelExport, menu.label));
                }
            }
        }
    }
    for item in row_menu().into_iter().flatten() {
        if let MenuItem::Run(command) = item
            && !listed.iter().any(|(c, _)| *c == command)
        {
            listed.push((command, "settings_keyboard.context_menu"));
        }
    }
    listed
}

/// Where `command` is, as the Keyboard page lists it and its messages name
/// it, such as "Edit > Find".
pub(crate) fn command_place(strings: &Strings, command: CommandId) -> String {
    let menu = listed_commands()
        .into_iter()
        .find(|(c, _)| *c == command)
        .map_or("", |(_, menu)| menu);
    format!(
        "{} > {}",
        without_access_key(strings.text(menu)),
        without_access_key(strings.text(command.row().label))
    )
}

/// One row of the Keyboard page's command list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandRow {
    pub(crate) command: CommandId,
    /// Such as "Edit > Find".
    pub(crate) place: String,
    /// Such as "Ctrl+F, Ctrl+E".
    pub(crate) keys: String,
}

/// The listed commands whose place or shortcuts contain `filter`, ignoring
/// case, so `ctrl+e` finds the command that has it (DESIGN.md: The Keyboard
/// page).
pub(crate) fn command_rows(
    strings: &Strings,
    shortcuts: &CommandShortcuts,
    filter: &str,
) -> Vec<CommandRow> {
    let filter = filter.trim().to_lowercase();
    listed_commands()
        .into_iter()
        .map(|(command, _)| CommandRow {
            command,
            place: command_place(strings, command),
            keys: shortcuts
                .of(command)
                .iter()
                .map(|k| k.text(strings))
                .collect::<Vec<_>>()
                .join(", "),
        })
        .filter(|row| {
            row.place.to_lowercase().contains(&filter) || row.keys.to_lowercase().contains(&filter)
        })
        .collect()
}

/// Why a command cannot have a shortcut (DESIGN.md: The Keyboard page).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The key belongs to the search box or the list, such as `Ctrl+V`.
    Control,
    /// The key types a character or opens a menu, such as `K` or `Alt+F`.
    Typing,
    /// Another command has it.
    Taken(CommandId),
}

impl Refusal {
    /// What the dialog says about `keystroke`.
    pub(crate) fn message(self, strings: &Strings, keystroke: Keystroke) -> String {
        let key = keystroke.text(strings);
        match self {
            Refusal::Control => strings.format("settings_keyboard.refused_control", &[&key]),
            Refusal::Typing => strings.format("settings_keyboard.refused_typing", &[&key]),
            Refusal::Taken(other) => strings.format(
                "settings_keyboard.refused_taken",
                &[&key, &command_place(strings, other)],
            ),
        }
    }
}

/// Whether `keystroke` belongs to a control, whatever command is chosen:
/// `Tab`, `Enter`, `Esc`, `Shift+F10`, `Alt+F4`, `Ctrl+V`, `Ctrl+X`,
/// `Ctrl+Z`, and `Ctrl` with a key that moves the caret or deletes.
fn belongs_to_a_control(keystroke: Keystroke) -> bool {
    let Keystroke {
        ctrl,
        alt,
        shift,
        key,
    } = keystroke;
    match key {
        Key::Tab => true,
        Key::Enter | Key::Escape => !ctrl && !alt,
        Key::Function(10) => shift && !ctrl && !alt,
        Key::Function(4) => alt && !ctrl && !shift,
        Key::Letter(b'V' | b'X' | b'Z') => ctrl && !alt && !shift,
        Key::Left
        | Key::Up
        | Key::Right
        | Key::Down
        | Key::Home
        | Key::End
        | Key::PageUp
        | Key::PageDown
        | Key::Backspace
        | Key::Delete => ctrl && !alt,
        _ => false,
    }
}

/// A command shortcut needs `Ctrl`, `Alt` with a key that is not a letter
/// or a digit, or a function key, because the search box types the other
/// keys and `Alt` with a letter opens a menu.
fn leaves_typing_alone(keystroke: Keystroke) -> bool {
    let letter_or_digit = matches!(keystroke.key, Key::Letter(_) | Key::Digit(_));
    keystroke.ctrl || keystroke.alt && !letter_or_digit || matches!(keystroke.key, Key::Function(_))
}

/// The shortcuts of the window's commands: each command's default from
/// the command table, unless the person changed it on the Keyboard page.
/// Only changed commands are held, in [`CommandId::ALL`]'s order, so two
/// tables with the same shortcuts are equal, and a table without changes
/// saves nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CommandShortcuts {
    changed: Vec<(CommandId, Vec<Keystroke>)>,
}

impl CommandShortcuts {
    /// `command`'s shortcuts; the menus show the first.
    pub(crate) fn of(&self, command: CommandId) -> &[Keystroke] {
        self.changed
            .iter()
            .find(|(c, _)| *c == command)
            .map_or(command.row().shortcuts, |(_, keys)| keys)
    }

    /// The command that has `keystroke`, if one has.
    fn owner(&self, keystroke: Keystroke) -> Option<CommandId> {
        CommandId::ALL
            .into_iter()
            .find(|c| self.of(*c).contains(&keystroke))
    }

    /// Whether `command` may have `keystroke`, beside the shortcuts it has.
    pub(crate) fn check(&self, command: CommandId, keystroke: Keystroke) -> Result<(), Refusal> {
        if belongs_to_a_control(keystroke) {
            return Err(Refusal::Control);
        }
        if !leaves_typing_alone(keystroke) {
            return Err(Refusal::Typing);
        }
        match self.owner(keystroke) {
            Some(other) if other != command => Err(Refusal::Taken(other)),
            _ => Ok(()),
        }
    }

    /// Gives `command` `keystroke` too, or in place of `replaced`, after
    /// [`CommandShortcuts::check`] allows it.
    pub(crate) fn add(
        &mut self,
        command: CommandId,
        keystroke: Keystroke,
        replaced: Option<Keystroke>,
    ) -> Result<(), Refusal> {
        self.check(command, keystroke)?;
        let mut keys = self.of(command).to_vec();
        match replaced.and_then(|old| keys.iter().position(|k| *k == old)) {
            Some(index) => keys[index] = keystroke,
            None => keys.push(keystroke),
        }
        // Replacing a shortcut with one the command has would list it twice.
        let mut unique = Vec::with_capacity(keys.len());
        for key in keys {
            if !unique.contains(&key) {
                unique.push(key);
            }
        }
        self.set(command, unique);
        Ok(())
    }

    pub(crate) fn remove(&mut self, command: CommandId, keystroke: Keystroke) {
        let keys = self
            .of(command)
            .iter()
            .copied()
            .filter(|k| *k != keystroke)
            .collect();
        self.set(command, keys);
    }

    fn set(&mut self, command: CommandId, keys: Vec<Keystroke>) {
        self.changed.retain(|(c, _)| *c != command);
        if keys != command.row().shortcuts {
            self.changed.push((command, keys));
            self.changed.sort_by_key(|(c, _)| {
                CommandId::ALL
                    .iter()
                    .position(|all| all == c)
                    .expect("ALL holds every command")
            });
        }
    }
}

impl Serialize for CommandShortcuts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.changed.len()))?;
        for (command, keys) in &self.changed {
            let names: Vec<String> = keys.iter().map(|k| k.name()).collect();
            map.serialize_entry(command, &names)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for CommandShortcuts {
    /// A table that breaks a rule, such as two commands with one shortcut
    /// or a shortcut for a command that takes none, is refused whole, so
    /// the defaults apply; a person who edited the file by hand gets
    /// working keys rather than a guess at which entry they meant.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let saved = BTreeMap::<String, Vec<String>>::deserialize(deserializer)?;
        let listed = listed_commands();
        let mut table = CommandShortcuts::default();
        for (name, keys) in saved {
            let command: CommandId = serde_json::from_value(serde_json::Value::String(name))
                .map_err(D::Error::custom)?;
            if !listed.iter().any(|(c, _)| *c == command) {
                return Err(D::Error::custom("a command that takes no shortcut"));
            }
            let keys = keys
                .iter()
                .map(|key| Keystroke::from_name(key))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| D::Error::custom("a shortcut without a name"))?;
            table.set(command, keys);
        }
        // Checked once every command has its keys, because a key that moved
        // from one command to another is taken until both are read.
        let valid = listed.iter().all(|(command, _)| {
            let keys = table.of(*command);
            keys.iter().enumerate().all(|(index, key)| {
                !keys[..index].contains(key) && table.check(*command, *key).is_ok()
            })
        });
        if valid {
            Ok(table)
        } else {
            Err(D::Error::custom("a shortcut that the rules refuse"))
        }
    }
}

/// The shortcut that shows Gozler from any program (DESIGN.md: The Keyboard
/// page): a key with the Windows key, or with two of `Ctrl`, `Alt` and
/// `Shift`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShowShortcut {
    windows: bool,
    keystroke: Keystroke,
}

/// Why a shortcut cannot show Gozler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShowRefusal {
    /// `Win+L` always locks the computer.
    Lock,
    /// Without the Windows key, it has fewer than two of `Ctrl`, `Alt` and
    /// `Shift`, so it would take a key from every program.
    Modifiers,
}

impl ShowShortcut {
    pub(crate) fn new(windows: bool, keystroke: Keystroke) -> Result<ShowShortcut, ShowRefusal> {
        let held = [keystroke.ctrl, keystroke.alt, keystroke.shift]
            .into_iter()
            .filter(|held| *held)
            .count();
        if windows && keystroke == Keystroke::plain(Key::Letter(b'L')) {
            return Err(ShowRefusal::Lock);
        }
        if !windows && held < 2 {
            return Err(ShowRefusal::Modifiers);
        }
        Ok(ShowShortcut { windows, keystroke })
    }

    /// Whether it holds the Windows key.
    pub(crate) fn windows(self) -> bool {
        self.windows
    }

    /// The keys it holds besides the Windows key.
    pub(crate) fn keystroke(self) -> Keystroke {
        self.keystroke
    }

    /// `keystroke` as the shortcut to show Gozler without the Windows key:
    /// with fewer than two of `Ctrl`, `Alt` and `Shift`, it becomes
    /// `Ctrl+Alt` and the key, as the hotkey control turns what the person
    /// presses, because the Windows key check box can be cleared after.
    pub(crate) fn two_modifiers(keystroke: Keystroke) -> Keystroke {
        let held = [keystroke.ctrl, keystroke.alt, keystroke.shift]
            .into_iter()
            .filter(|held| *held)
            .count();
        if held >= 2 {
            keystroke
        } else {
            Keystroke {
                ctrl: true,
                alt: true,
                shift: false,
                key: keystroke.key,
            }
        }
    }

    /// As the dialog and the messages show it, such as "Win+S".
    pub(crate) fn text(self, strings: &Strings) -> String {
        show_text(strings, self.windows, self.keystroke)
    }

    /// Its name in `ui.json`, such as "Win+S".
    fn name(self) -> String {
        let keys = self.keystroke.name();
        if self.windows {
            format!("Win+{keys}")
        } else {
            keys
        }
    }

    fn from_name(name: &str) -> Option<ShowShortcut> {
        let (windows, keys) = match name.strip_prefix("Win+") {
            Some(keys) => (true, keys),
            None => (false, name),
        };
        ShowShortcut::new(windows, Keystroke::from_name(keys)?).ok()
    }
}

impl Serialize for ShowShortcut {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.name())
    }
}

impl<'de> Deserialize<'de> for ShowShortcut {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        ShowShortcut::from_name(&name).ok_or_else(|| D::Error::custom("not a shortcut"))
    }
}

/// `keystroke`, with the Windows key when `windows`, as the dialog shows a
/// shortcut to show Gozler.
fn show_text(strings: &Strings, windows: bool, keystroke: Keystroke) -> String {
    let keys = keystroke.text(strings);
    if windows {
        format!("{}+{keys}", strings.text("key.windows"))
    } else {
        keys
    }
}

/// What the dialog says when `keystroke`, with the Windows key when
/// `windows`, cannot show Gozler.
pub(crate) fn show_refusal_message(
    strings: &Strings,
    refusal: ShowRefusal,
    windows: bool,
    keystroke: Keystroke,
) -> String {
    match refusal {
        ShowRefusal::Lock => strings.format(
            "settings_keyboard.lock",
            &[&show_text(strings, windows, keystroke)],
        ),
        ShowRefusal::Modifiers => strings.text("settings_keyboard.two_modifiers").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text::tests::english;

    fn key(name: &str) -> Keystroke {
        Keystroke::from_name(name).expect("a shortcut's name")
    }

    /// A default the rules refuse could not be added back after the person
    /// removed it, and Reset would give a table the dialog
    /// rejects.
    #[test]
    fn every_default_shortcut_passes_the_rules() {
        let defaults = CommandShortcuts::default();
        for (command, _) in listed_commands() {
            for keystroke in defaults.of(command) {
                assert_eq!(
                    defaults.check(command, *keystroke),
                    Ok(()),
                    "{command:?} {keystroke:?}"
                );
            }
        }
    }

    /// C18 and the Keyboard page's rules, with the messages the dialog
    /// shows.
    #[test]
    fn a_command_shortcut_follows_the_keyboard_page_rules() {
        let strings = english();
        let mut table = CommandShortcuts::default();
        table
            .add(CommandId::Find, key("Ctrl+K"), None)
            .expect("Ctrl+K is free");
        assert_eq!(
            table.of(CommandId::Find),
            [key("Ctrl+F"), key("Ctrl+E"), key("Ctrl+K")]
        );
        for (name, refusal, message) in [
            (
                "Alt+F",
                Refusal::Typing,
                "Alt+F is used for typing or the menus. Choose a shortcut with Ctrl, such as \
                 Ctrl+K, or a function key, such as F3.",
            ),
            (
                "K",
                Refusal::Typing,
                "K is used for typing or the menus. Choose a shortcut with Ctrl, such as Ctrl+K, \
                 or a function key, such as F3.",
            ),
            (
                "Ctrl+V",
                Refusal::Control,
                "Ctrl+V is used by the search box or the results list.",
            ),
            (
                "Ctrl+Home",
                Refusal::Control,
                "Ctrl+Home is used by the search box or the results list.",
            ),
        ] {
            assert_eq!(
                table.check(CommandId::Copy, key(name)),
                Err(refusal),
                "{name}"
            );
            assert_eq!(refusal.message(&strings, key(name)), message);
        }
        let taken = table.check(CommandId::Copy, key("Ctrl+E"));
        assert_eq!(taken, Err(Refusal::Taken(CommandId::Find)));
        assert_eq!(
            taken.unwrap_err().message(&strings, key("Ctrl+E")),
            "Ctrl+E is the shortcut for Edit > Find. Remove it there first."
        );
        // A function key alone and Alt with a key that is not a letter are
        // free of typing.
        assert_eq!(table.check(CommandId::Copy, key("F3")), Ok(()));
        assert_eq!(table.check(CommandId::Copy, key("Alt+Down")), Ok(()));
        // The list shows the new key after the command, and finds it.
        assert_eq!(
            command_rows(&strings, &table, " ctrl+k"),
            [CommandRow {
                command: CommandId::Find,
                place: "Edit > Find".into(),
                keys: "Ctrl+F, Ctrl+E, Ctrl+K".into(),
            }]
        );

        // Reset is the empty table, which saves nothing.
        table.remove(CommandId::Find, key("Ctrl+K"));
        assert_eq!(table, CommandShortcuts::default());
    }

    /// Without the Windows key a shortcut would take its key from every
    /// program, and Windows keeps Win+L however Gozler asks.
    #[test]
    fn the_shortcut_to_show_gozler_needs_the_windows_key_or_two_modifiers() {
        let strings = english();
        assert_eq!(
            ShowShortcut::two_modifiers(key("Ctrl+F")),
            key("Ctrl+Alt+F")
        );
        let win_s = ShowShortcut::new(true, key("S")).expect("Win+S is a shortcut");
        assert_eq!(win_s.text(&strings), "Win+S");
        assert_eq!(ShowShortcut::new(true, key("L")), Err(ShowRefusal::Lock));
        assert_eq!(
            show_refusal_message(&strings, ShowRefusal::Lock, true, key("L")),
            "Windows keeps Win+L for locking the computer. Choose another shortcut."
        );
        assert_eq!(
            ShowShortcut::new(false, key("Ctrl+F")),
            Err(ShowRefusal::Modifiers)
        );
    }
}
