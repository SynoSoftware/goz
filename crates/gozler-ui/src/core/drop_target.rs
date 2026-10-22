//! What files dragged over the results list do when they are dropped
//! (DESIGN.md: Dragging): the engine copies or moves them into the folder row
//! under the pointer, and the effect follows File Explorer's rules.
//!
//! Paths are compared as text, ignoring case as Windows does, so the choice
//! reads no disk and runs on every `DragOver`.

use super::commands::{Mark, MenuNode};
use super::glyph::Glyph;
use super::text::Strings;
use gozler_core::proto::ResultItem;
use std::path::PathBuf;

/// What a drop does with the dragged files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DropEffect {
    Copy,
    Move,
}

/// The modifier keys held while the files are dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DropKeys {
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
}

impl DropKeys {
    const CTRL: DropKeys = DropKeys {
        ctrl: true,
        shift: false,
        alt: false,
    };
    const SHIFT: DropKeys = DropKeys {
        ctrl: false,
        shift: true,
        alt: false,
    };
}

/// The effects the drag's source allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceEffects {
    pub(crate) copy: bool,
    pub(crate) moves: bool,
}

/// The files of one drag, as read when it enters the window, which choose
/// the effect. The drop reads them again, because a source can make its
/// files only then.
pub(crate) struct Dragged {
    /// Each path in [`fold`]'s form.
    folded: Vec<String>,
}

impl Dragged {
    pub(crate) fn new(paths: Vec<PathBuf>) -> Dragged {
        let folded = paths
            .iter()
            .map(|path| fold(&path.to_string_lossy()))
            .collect();
        Dragged { folded }
    }

    /// What dropping the files on `row`, the row under the pointer, does:
    /// nothing over a file row or empty space, which is `None`, and nothing
    /// on a folder that is one of the files or inside one, which the engine
    /// would refuse. Otherwise a move when every file is on the folder's
    /// volume and a copy when not, unless `Ctrl` asks for a copy or `Shift`
    /// for a move; each only where the source allows it.
    pub(crate) fn effect(
        &self,
        row: Option<&ResultItem>,
        keys: DropKeys,
        allowed: SourceEffects,
    ) -> Option<DropEffect> {
        let folder = fold(&row.filter(|row| row.is_dir)?.path);
        if self.folded.is_empty() || self.folded.iter().any(|path| within(&folder, path)) {
            return None;
        }
        let allows = |effect: &DropEffect| match effect {
            DropEffect::Copy => allowed.copy,
            DropEffect::Move => allowed.moves,
        };
        let effect = match (keys.ctrl, keys.shift, keys.alt) {
            // Ctrl+Shift and Alt ask for shortcuts, which Gozler does not
            // make.
            (true, true, _) | (_, _, true) => None,
            (true, false, false) => Some(DropEffect::Copy).filter(allows),
            (false, true, false) => Some(DropEffect::Move).filter(allows),
            (false, false, false) => {
                let same_volume = self.folded.iter().all(|path| root(path) == root(&folder));
                let order = if same_volume {
                    [DropEffect::Move, DropEffect::Copy]
                } else {
                    [DropEffect::Copy, DropEffect::Move]
                };
                order.into_iter().find(allows)
            }
        }?;
        // A move into the folder that holds every file already does
        // nothing, so File Explorer refuses it.
        let moves_nothing = self
            .folded
            .iter()
            .all(|path| parent(path) == Some(folder.as_str()));
        (effect == DropEffect::Copy || !moves_nothing).then_some(effect)
    }

    /// The menu of a drop with the right button on `folder`, where a drop
    /// without the menu would do `default`: Copy here and Move here, each
    /// enabled where the key that forces it would give it, as in File
    /// Explorer.
    pub(crate) fn menu(
        &self,
        folder: &ResultItem,
        default: DropEffect,
        allowed: SourceEffects,
    ) -> DropMenu {
        let offers = |keys| self.effect(Some(folder), keys, allowed).is_some();
        DropMenu {
            default,
            copy: offers(DropKeys::CTRL),
            moves: offers(DropKeys::SHIFT),
        }
    }
}

/// The menu that a drop with the right button shows: Copy here, Move here
/// and Cancel, with the effect of a drop without the menu in bold. Gozler
/// makes no shortcuts, so it has no shortcut item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DropMenu {
    default: DropEffect,
    copy: bool,
    moves: bool,
}

const COPY_HERE: u16 = 1;
const MOVE_HERE: u16 = 2;
const CANCEL: u16 = 3;

fn id_of(effect: DropEffect) -> u16 {
    match effect {
        DropEffect::Copy => COPY_HERE,
        DropEffect::Move => MOVE_HERE,
    }
}

impl DropMenu {
    pub(crate) fn nodes(strings: &Strings) -> Vec<MenuNode> {
        let item = |id, key: &str, glyph| MenuNode::Item {
            id,
            text: strings.text(key).to_owned(),
            mark: Mark::None,
            glyph,
        };
        vec![
            item(
                COPY_HERE,
                "file_operations.drop_copy_here",
                Some(Glyph::Copy),
            ),
            item(
                MOVE_HERE,
                "file_operations.drop_move_here",
                Some(Glyph::MoveToFolder),
            ),
            MenuNode::Separator,
            item(CANCEL, "file_operations.drop_cancel", None),
        ]
    }

    /// Each item's ID, whether it is enabled, and that it is not checked.
    pub(crate) fn states(self) -> [(u16, bool, bool); 2] {
        [
            (COPY_HERE, self.copy, false),
            (MOVE_HERE, self.moves, false),
        ]
    }

    pub(crate) fn default_id(self) -> u16 {
        id_of(self.default)
    }

    /// What the item `chosen` does; Cancel, and a click outside the menu,
    /// which chooses none, drop nothing.
    pub(crate) fn effect_of(chosen: Option<u16>) -> Option<DropEffect> {
        [DropEffect::Copy, DropEffect::Move]
            .into_iter()
            .find(|effect| chosen == Some(id_of(*effect)))
    }
}

/// `path` as Windows compares it: in lower case, without a last backslash.
fn fold(path: &str) -> String {
    path.trim_end_matches('\\').to_lowercase()
}

/// Whether the folded `folder` is the folded `path` or inside it.
fn within(folder: &str, path: &str) -> bool {
    folder
        .strip_prefix(path)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

fn parent(path: &str) -> Option<&str> {
    path.rfind('\\').map(|end| &path[..end])
}

/// The part of a folded path that names its volume: the drive, such as
/// `c:`, or the share, such as `server\share`. A folder that another volume
/// is mounted on counts as its parent's volume, which only makes the
/// default a move where it would be a copy.
fn root(path: &str) -> &str {
    let (rest, parts) = match path.strip_prefix(r"\\") {
        Some(rest) => (rest, 2),
        None => (path, 1),
    };
    let end = rest
        .match_indices('\\')
        .nth(parts - 1)
        .map_or(rest.len(), |(end, _)| end);
    &rest[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, is_dir: bool) -> ResultItem {
        ResultItem {
            path: path.to_owned(),
            path_u16: None,
            size: None,
            mtime_ft: None,
            created_ft: None,
            is_dir,
            attributes: None,
        }
    }

    fn dragged(paths: &[&str]) -> Dragged {
        Dragged::new(paths.iter().map(PathBuf::from).collect())
    }

    const BOTH: SourceEffects = SourceEffects {
        copy: true,
        moves: true,
    };
    const NO_KEYS: DropKeys = DropKeys {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const CTRL: DropKeys = DropKeys::CTRL;
    const SHIFT: DropKeys = DropKeys::SHIFT;

    /// A move where File Explorer copies would take the originals from
    /// another disk, and a copy where it moves would leave them behind.
    #[test]
    fn the_folder_s_volume_and_the_keys_choose_copy_or_move() {
        let files = dragged(&[r"C:\Users\ana\report.pdf", r"C:\Users\ana\photos\"]);
        let same = row(r"c:\Archive", true);
        let other = row(r"D:\Backup", true);
        let share = dragged(&[r"\\server\share\report.pdf"]);
        let effect = |files: &Dragged, row: &ResultItem, keys| files.effect(Some(row), keys, BOTH);
        assert_eq!(effect(&files, &same, NO_KEYS), Some(DropEffect::Move));
        assert_eq!(effect(&files, &other, NO_KEYS), Some(DropEffect::Copy));
        assert_eq!(effect(&share, &same, NO_KEYS), Some(DropEffect::Copy));
        assert_eq!(effect(&files, &same, CTRL), Some(DropEffect::Copy));
        assert_eq!(effect(&files, &other, SHIFT), Some(DropEffect::Move));
        let alt = DropKeys {
            alt: true,
            ..NO_KEYS
        };
        assert_eq!(effect(&files, &same, alt), None, "Alt asks for shortcuts");
    }

    /// A source that only allows a copy, such as an archive, would lose
    /// files that the engine moved.
    #[test]
    fn neither_a_key_nor_the_menu_gives_an_effect_the_source_refuses() {
        let files = dragged(&[r"C:\Temp\7z\report.pdf"]);
        let folder = row(r"C:\Archive", true);
        let copy_only = SourceEffects {
            copy: true,
            moves: false,
        };
        assert_eq!(
            files.effect(Some(&folder), NO_KEYS, copy_only),
            Some(DropEffect::Copy)
        );
        assert_eq!(files.effect(Some(&folder), SHIFT, copy_only), None);
        let menu = files.menu(&folder, DropEffect::Copy, copy_only);
        assert_eq!(
            menu.states(),
            [(COPY_HERE, true, false), (MOVE_HERE, false, false)],
            "the right button's menu offers no Move here either"
        );
    }

    /// Only a folder row takes the files; the engine would refuse a folder
    /// inside a dragged one, and a move into the files' own folder does
    /// nothing.
    #[test]
    fn the_drop_is_refused_where_it_could_do_nothing() {
        let files = dragged(&[r"C:\Work\Photos", r"C:\Work\notes.txt"]);
        let effect = |row: Option<&ResultItem>, keys| files.effect(row, keys, BOTH);
        assert_eq!(effect(None, NO_KEYS), None, "empty space");
        assert_eq!(
            effect(Some(&row(r"C:\Archive\a.txt", false)), NO_KEYS),
            None
        );
        assert_eq!(effect(Some(&row(r"C:\work\photos", true)), CTRL), None);
        assert_eq!(effect(Some(&row(r"C:\Work\Photos\2024", true)), CTRL), None);
        assert_eq!(
            effect(Some(&row(r"C:\Work\Photos 2024", true)), NO_KEYS),
            Some(DropEffect::Move),
            "a folder whose name only starts with a dragged one's"
        );
        let own = row(r"C:\Work", true);
        assert_eq!(effect(Some(&own), NO_KEYS), None);
        assert_eq!(effect(Some(&own), CTRL), Some(DropEffect::Copy));
    }
}
