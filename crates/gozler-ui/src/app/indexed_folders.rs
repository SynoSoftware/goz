//! The Indexed folders dialog, which Settings > Include's Modify… opens: a
//! tree of each drive's folders with a check box on each, and the summary
//! of the indexed folders under it (DESIGN.md: The Include page). It edits
//! its own copy of the Include settings, which OK gives back to the page.

use super::main_window::MainWindow;
use crate::core::ids::ControlId;
use crate::core::include::{IncludeDrive, SummaryRow};
use crate::core::text::Strings;
use crate::win::Hwnd;
use crate::win::controls::{FolderTree, ReportList, TreeItem};
use crate::win::dialog::{DialogMessage, DialogProc, Opened, TreeChange};
use gozler_core::include::IncludeSettings;
use std::cell::{Cell, RefCell};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// The dialog's timer, which adds the folders listed on other threads.
const LISTED_TIMER: usize = 1;
const LISTED_POLL_MS: u32 = 50;

/// One folder the tree shows: a drive's root, as an empty path, or a folder
/// below it.
struct Node {
    drive: usize,
    path: String,
    listing: Listing,
}

/// Whether a folder's subfolders are in the tree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    NotListed,
    /// A thread lists them.
    Pending,
    Listed,
}

pub(super) struct IndexedFolders<'a> {
    main: &'a MainWindow,
    strings: &'a Strings,
    drives: &'a [IncludeDrive],
    settings: RefCell<IncludeSettings>,
    /// The tree's folders; each item carries its index.
    nodes: RefCell<Vec<Node>>,
    /// The folders a thread lists, and where its subfolders arrive.
    pending: RefCell<Vec<(TreeItem, Receiver<Subfolders>)>>,
    /// The summary's rows.
    rows: RefCell<Vec<SummaryRow>>,
    /// Whether the dialog is setting check boxes or rows, whose
    /// notifications then change nothing.
    filling: Cell<bool>,
}

impl<'a> IndexedFolders<'a> {
    pub(super) fn new(
        main: &'a MainWindow,
        strings: &'a Strings,
        drives: &'a [IncludeDrive],
        settings: IncludeSettings,
    ) -> IndexedFolders<'a> {
        IndexedFolders {
            main,
            strings,
            drives,
            settings: RefCell::new(settings),
            nodes: RefCell::new(Vec::new()),
            pending: RefCell::new(Vec::new()),
            rows: RefCell::new(Vec::new()),
            filling: Cell::new(false),
        }
    }

    /// The settings as the person left them; OK applies them to the page.
    pub(super) fn into_settings(self) -> IncludeSettings {
        self.settings.into_inner()
    }

    fn tree(dialog: Hwnd) -> Option<FolderTree> {
        FolderTree::of_dialog(dialog, ControlId::FoldersTree.id())
    }

    fn summary(dialog: Hwnd) -> Option<ReportList> {
        ReportList::of_dialog(dialog, ControlId::FoldersSummary.id())
    }

    fn filling<T>(&self, fill: impl FnOnce() -> T) -> T {
        let was = self.filling.replace(true);
        let filled = fill();
        self.filling.set(was);
        filled
    }

    /// Adds the folder `path` of `drive` below `parent`, with its check box.
    fn add(
        &self,
        tree: FolderTree,
        parent: Option<TreeItem>,
        drive: usize,
        path: String,
        text: &str,
        may_have_children: bool,
    ) -> Option<TreeItem> {
        let check = self
            .settings
            .borrow()
            .folder_check(&self.drives[drive].volume, &path);
        let data = {
            let mut nodes = self.nodes.borrow_mut();
            nodes.push(Node {
                drive,
                path,
                listing: Listing::NotListed,
            });
            nodes.len() - 1
        };
        // A new item gets the control's own check box state, which must not
        // read as a click.
        self.filling(|| {
            let item = tree.insert(parent, text, data, may_have_children)?;
            tree.set_check(item, check);
            Some(item)
        })
    }

    /// The drive and folder of `item`; `None` for an item the dialog did
    /// not add.
    fn node(&self, tree: FolderTree, item: TreeItem) -> Option<(usize, String)> {
        let nodes = self.nodes.borrow();
        let node = nodes.get(tree.data(item))?;
        Some((node.drive, node.path.clone()))
    }

    /// Marks `item` as being listed, and gives its folder; `None` once it
    /// was, or for a drive that is not connected.
    fn start_listing(&self, tree: FolderTree, item: TreeItem) -> Option<String> {
        let mut nodes = self.nodes.borrow_mut();
        let node = nodes.get_mut(tree.data(item))?;
        let root = self.drives[node.drive].root.as_ref()?;
        if node.listing != Listing::NotListed {
            return None;
        }
        node.listing = Listing::Pending;
        Some(format!("{root}{}", node.path))
    }

    /// Lists the subfolders of `item` the first time it opens, on a thread
    /// of its own, because a folder such as `C:\Windows\WinSxS` takes long
    /// to list; the dialog's timer adds them. They come from the drive, not
    /// from the index, so a folder that is left out can be checked again.
    fn list_later(&self, dialog: Hwnd, tree: FolderTree, item: TreeItem) {
        let Some(folder) = self.start_listing(tree, item) else {
            return;
        };
        let (sender, listed) = channel();
        // Not joined: once the dialog has closed, its send fails.
        std::thread::spawn(move || {
            let _ = sender.send(subfolders(&folder));
        });
        self.pending.borrow_mut().push((item, listed));
        dialog.set_timer(LISTED_TIMER, LISTED_POLL_MS);
    }

    /// Lists the subfolders of `item` now, unless they are in the tree,
    /// waiting for the thread that lists them.
    fn list_now(&self, tree: FolderTree, item: TreeItem) {
        let folders = match self.start_listing(tree, item) {
            Some(folder) => subfolders(&folder),
            None => {
                let position = self
                    .pending
                    .borrow()
                    .iter()
                    .position(|(pending, _)| *pending == item);
                let Some(position) = position else {
                    return;
                };
                let (_, listed) = self.pending.borrow_mut().remove(position);
                listed.recv().unwrap_or_default()
            }
        };
        self.add_folders(tree, item, folders);
    }

    /// Adds the folders that the threads have listed, and stops the timer
    /// once none lists any.
    fn add_listed(&self, dialog: Hwnd) {
        let mut listed = Vec::new();
        {
            let mut pending = self.pending.borrow_mut();
            pending.retain(|(item, folders)| match folders.try_recv() {
                Err(TryRecvError::Empty) => true,
                answer => {
                    listed.push((*item, answer.unwrap_or_default()));
                    false
                }
            });
            if pending.is_empty() {
                dialog.kill_timer(LISTED_TIMER);
            }
        }
        let Some(tree) = Self::tree(dialog) else {
            return;
        };
        for (item, folders) in listed {
            self.add_folders(tree, item, folders);
            // The tree opened the item while it had no folders yet.
            tree.expand(item);
        }
    }

    /// Adds `folders`, the subfolders of `item`, below it.
    fn add_folders(&self, tree: FolderTree, item: TreeItem, folders: Subfolders) {
        let (drive, path) = {
            let mut nodes = self.nodes.borrow_mut();
            let Some(node) = nodes.get_mut(tree.data(item)) else {
                return;
            };
            node.listing = Listing::Listed;
            (node.drive, node.path.clone())
        };
        if folders.is_empty() {
            tree.set_childless(item);
        }
        for (name, may_have_children) in folders {
            let below = if path.is_empty() {
                name.clone()
            } else {
                format!(r"{path}\{name}")
            };
            self.add(tree, Some(item), drive, below, &name, may_have_children);
        }
    }

    /// Sets every check box the tree shows from the settings.
    fn refresh_checks(&self, tree: FolderTree) {
        let settings = self.settings.borrow();
        let nodes = self.nodes.borrow();
        let mut items = tree.children(None);
        self.filling(|| {
            while let Some(item) = items.pop() {
                if let Some(node) = nodes.get(tree.data(item)) {
                    let check = settings.folder_check(&self.drives[node.drive].volume, &node.path);
                    tree.set_check(item, check);
                }
                items.extend(tree.children(Some(item)));
            }
        });
    }

    fn fill_summary(&self, dialog: Hwnd) {
        let rows = self
            .main
            .include_summary(self.drives, &self.settings.borrow());
        let cells: Vec<[String; 3]> = rows.iter().map(|row| row.cells.clone()).collect();
        self.rows.replace(rows);
        if let Some(list) = Self::summary(dialog) {
            self.filling(|| list.set_rows(&cells, None));
        }
    }

    /// Opens the tree down to the folder of the selected summary row and
    /// selects it.
    fn reveal(&self, dialog: Hwnd) {
        let (Some(tree), Some(index)) = (
            Self::tree(dialog),
            Self::summary(dialog).and_then(ReportList::selected),
        ) else {
            return;
        };
        let Some((drive, path)) = self
            .rows
            .borrow()
            .get(index)
            .map(|row| (row.drive, row.path.clone()))
        else {
            return;
        };
        let Some(mut item) = tree
            .children(None)
            .into_iter()
            .find(|&item| self.node(tree, item).is_some_and(|(d, _)| d == drive))
        else {
            return;
        };
        for name in path.split('\\').filter(|name| !name.is_empty()) {
            self.list_now(tree, item);
            tree.expand(item);
            let child = tree.children(Some(item)).into_iter().find(|&child| {
                self.node(tree, child).is_some_and(|(_, path)| {
                    let child_name = path.rsplit('\\').next().unwrap_or(&path);
                    child_name.eq_ignore_ascii_case(name)
                })
            });
            match child {
                Some(child) => item = child,
                // The folder is gone from the drive; show the nearest one.
                None => break,
            }
        }
        tree.select(item);
    }
}

/// Folder names, each with whether it may hold folders.
type Subfolders = Vec<(String, bool)>;

/// The subfolders of `folder`, sorted by name, each with whether it may
/// hold folders: a junction or a symbolic link leads elsewhere, so the tree
/// does not open it.
fn subfolders(folder: &str) -> Subfolders {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut folders: Subfolders = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            let link = kind.is_symlink();
            (kind.is_dir() || link && entry.path().is_dir())
                .then(|| (entry.file_name().to_string_lossy().into_owned(), !link))
        })
        .collect();
    folders.sort_by_key(|(name, _)| name.to_lowercase());
    folders
}

impl DialogProc for IndexedFolders<'_> {
    fn init(&self, dialog: Hwnd) -> Opened {
        if let Some(tree) = Self::tree(dialog) {
            // Turning check boxes on sends a check box notice of its own.
            self.filling(|| tree.show_check_boxes());
            for (index, drive) in self.drives.iter().enumerate() {
                let item = self.add(
                    tree,
                    None,
                    index,
                    String::new(),
                    &drive.name,
                    drive.root.is_some(),
                );
                // Each drive opens one level deep, so the folders to
                // uncheck are in view.
                if let Some(item) = item
                    && drive.root.is_some()
                {
                    self.list_now(tree, item);
                    tree.expand(item);
                }
            }
        }
        if let Some(list) = Self::summary(dialog) {
            list.set_columns(&[
                (
                    self.strings.text("settings_include.folder_column"),
                    30,
                    false,
                ),
                (
                    self.strings.text("settings_include.left_out_column"),
                    18,
                    false,
                ),
                (
                    self.strings.text("settings_include.folders_column"),
                    52,
                    false,
                ),
            ]);
            list.show_info_tips();
        }
        self.fill_summary(dialog);
        Opened {
            focus: ControlId::FoldersTree.id(),
            fonts: Vec::new(),
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Tree {
                item,
                change: TreeChange::Expanding,
                ..
            } => {
                if let Some(tree) = Self::tree(dialog) {
                    self.list_later(dialog, tree, item);
                }
                true
            }
            DialogMessage::Timer { id: LISTED_TIMER } => {
                self.add_listed(dialog);
                true
            }
            DialogMessage::Tree {
                item,
                change: TreeChange::Checked,
                ..
            } => {
                if self.filling.get() {
                    return true;
                }
                if let Some(tree) = Self::tree(dialog)
                    && let Some((drive, path)) = self.node(tree, item)
                {
                    self.settings
                        .borrow_mut()
                        .toggle_folder(&self.drives[drive].volume, &path);
                    // The control changed the check box itself, and a click
                    // changes the folders above and below it as well.
                    self.refresh_checks(tree);
                    self.fill_summary(dialog);
                }
                true
            }
            DialogMessage::Edited { id } if id == ControlId::FoldersSummary.id() => {
                if !self.filling.get() {
                    self.reveal(dialog);
                }
                true
            }
            DialogMessage::Command { .. }
            | DialogMessage::Edited { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Focus { .. }
            | DialogMessage::KeysCaptured { .. }
            | DialogMessage::Other => false,
        }
    }

    fn info_tip(&self, id: u16, row: usize) -> Option<String> {
        let rows = self.rows.borrow();
        let row = rows
            .get(row)
            .filter(|_| id == ControlId::FoldersSummary.id())?;
        row.tip()
    }
}
