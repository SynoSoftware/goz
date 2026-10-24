//! The Indexed folders dialog, which Settings > Include's Modify… opens: a
//! tree of each drive's folders with a check box on each, and the summary
//! of the indexed folders under it (DESIGN.md: The Include page). It edits
//! its own copy of the Include settings, which OK gives back to the page.

use super::main_window::MainWindow;
use super::work::indexed::{Session, Subfolders};
use crate::core::captions::DialogItem;
use crate::core::ids::ControlId;
use crate::core::include::{IncludeDrive, SummaryRow};
use crate::core::text::Strings;
use crate::core::work::{self, INLINE_ITEMS, Retirement, Waiting};
use crate::win::Hwnd;
use crate::win::controls::{FolderTree, ReportList, TreeItem};
use crate::win::dialog::{self, DialogMessage, DialogProc, Opened, TreeChange};
use gozler_core::include::IndexSettings;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// The dialog's timer, which adds the folders listed on other threads and
/// sets the check boxes after a click, a slice at a time.
const WORK_TIMER: usize = 1;
const WORK_POLL_MS: u32 = 15;
/// The longest one slice of that work keeps the dialog from the person,
/// because each item costs the tree an accessibility event ([`FolderTree`]).
const SLICE: Duration = Duration::from_millis(15);

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
    /// The volume's listing slot accepted them.
    Pending,
    /// They are in the tree, or wait in [`IndexedFolders::jobs`].
    Listed,
}

/// Work that the dialog's timer does a slice at a time.
enum Job {
    Add(Adding),
    /// The items whose check boxes, and their children's, are still to be
    /// set from the settings after a click; the next one is last.
    Recheck(Vec<TreeItem>),
}

/// Listed subfolders of `item`, the folder `path` of `drive`, that the tree
/// does not show yet.
struct Adding {
    item: TreeItem,
    drive: usize,
    path: String,
    folders: std::vec::IntoIter<(String, bool)>,
    /// Whether the tree has opened `item`, which it does once the first of
    /// them show.
    opened: bool,
}

pub(super) struct IndexedFolders<'a> {
    main: &'a MainWindow,
    strings: &'a Strings,
    drives: &'a [IncludeDrive],
    settings: RefCell<IndexSettings>,
    /// The tree's folders; each item carries its index.
    nodes: RefCell<Vec<Node>>,
    session: Cell<Option<Session>>,
    /// Native items remain here; worker replies name only their node indices.
    pending: RefCell<HashMap<usize, TreeItem>>,
    /// The work that waits, the newest last. The timer does the newest
    /// first, so a folder the person opens, or a click, does not wait
    /// behind a large folder that is still being added.
    jobs: RefCell<Vec<Job>>,
    /// The folder of a summary row that the tree opens down to once the
    /// folders on the way are in it: the deepest folder reached, and the
    /// names of the folders below it.
    revealing: RefCell<Option<(TreeItem, VecDeque<String>)>>,
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
        settings: IndexSettings,
    ) -> IndexedFolders<'a> {
        IndexedFolders {
            main,
            strings,
            drives,
            settings: RefCell::new(settings),
            nodes: RefCell::new(Vec::new()),
            session: Cell::new(None),
            pending: RefCell::new(HashMap::new()),
            jobs: RefCell::new(Vec::new()),
            revealing: RefCell::new(None),
            rows: RefCell::new(Vec::new()),
            filling: Cell::new(false),
        }
    }

    /// The settings as the person left them; OK applies them to the page.
    pub(super) fn into_settings(self) -> IndexSettings {
        self.settings.take()
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
        // A new item's check box must not read as a click.
        self.filling(|| tree.insert(parent, text, data, may_have_children, check))
    }

    /// Adds the folder `name` below `item`, the folder `path` of `drive`.
    fn add_below(&self, tree: FolderTree, item: TreeItem, adding: &Adding, folder: (String, bool)) {
        let (name, may_have_children) = folder;
        let below = if adding.path.is_empty() {
            name.clone()
        } else {
            format!(r"{}\{name}", adding.path)
        };
        self.add(
            tree,
            Some(item),
            adding.drive,
            below,
            &name,
            may_have_children,
        );
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
    fn start_listing(&self, tree: FolderTree, item: TreeItem) -> Option<(usize, usize, String)> {
        let index = tree.data(item);
        let mut nodes = self.nodes.borrow_mut();
        let node = nodes.get_mut(index)?;
        let root = self.drives[node.drive].root.as_ref()?;
        if node.listing != Listing::NotListed {
            return None;
        }
        node.listing = Listing::Pending;
        Some((index, node.drive, format!("{root}{}", node.path)))
    }

    /// Reads the disk rather than the index, so a folder left out can be
    /// checked again. The volume's work slot preserves every expansion.
    fn list_later(&self, dialog: Hwnd, tree: FolderTree, item: TreeItem) {
        let Some(session) = self.session.get() else {
            return;
        };
        let Some((node, drive, folder)) = self.start_listing(tree, item) else {
            return;
        };
        self.pending.borrow_mut().insert(node, item);
        self.main
            .indexed()
            .list(session, node, &self.drives[drive].volume, folder);
        dialog.set_timer(WORK_TIMER, WORK_POLL_MS);
    }

    /// Records that `item`'s subfolders, `folders`, were listed, and gives
    /// them to add, or shows that it has none.
    fn listed(&self, tree: FolderTree, item: TreeItem, folders: Subfolders) -> Option<Adding> {
        let (drive, path) = {
            let mut nodes = self.nodes.borrow_mut();
            let node = nodes.get_mut(tree.data(item))?;
            node.listing = Listing::Listed;
            (node.drive, node.path.clone())
        };
        if folders.is_empty() {
            tree.set_childless(item);
            return None;
        }
        Some(Adding {
            item,
            drive,
            path,
            folders: folders.into_iter(),
            opened: false,
        })
    }

    /// Takes the folders that the threads have listed, then does one slice
    /// of the work that waits, and stops the timer once nothing waits.
    fn work(&self, dialog: Hwnd) {
        let Some(tree) = Self::tree(dialog) else {
            return;
        };
        let Some(session) = self.session.get() else {
            return;
        };
        let listed = self.main.indexed().take(session);
        let mut retired = Retirement::default();
        for listed in listed {
            let item = self.pending.borrow_mut().remove(&listed.node);
            let current = self.nodes.borrow().get(listed.node).is_some_and(|node| {
                node.listing == Listing::Pending
                    && self.drives[node.drive]
                        .root
                        .as_ref()
                        .is_some_and(|root| listed.folder == format!("{root}{}", node.path))
            });
            if let Some(item) = item.filter(|_| current) {
                if let Some(adding) = self.listed(tree, item, listed.folders) {
                    self.jobs.borrow_mut().push(Job::Add(adding));
                }
            } else {
                retired.push(listed, INLINE_ITEMS + 1);
            }
        }
        self.main.indexed().retire(retired);
        if !self.jobs.borrow().is_empty() {
            let end = Instant::now() + SLICE;
            // A tree that draws itself updates its scroll bar for each item.
            tree.hwnd().draw_once_after(|| {
                self.recheck_visible(tree);
                self.work_until(tree, end);
            });
        }
        self.reveal_more(dialog, tree);
        let pending = self.main.indexed().pending(session);
        if !pending && self.jobs.borrow().is_empty() {
            dialog.kill_timer(WORK_TIMER);
        } else {
            dialog.set_timer(WORK_TIMER, WORK_POLL_MS);
        }
        self.show_waiting(dialog);
    }

    fn show_waiting(&self, dialog: Hwnd) {
        let waiting = self
            .session
            .get()
            .and_then(|session| self.main.indexed().waiting(session, Instant::now()));
        let text = match waiting {
            Some((Waiting::Slow, path)) => work::waiting(self.strings, Some(&path)),
            _ => self.strings.text("indexed_folders.hint").to_owned(),
        };
        if let Some(hint) = dialog::item(dialog, ControlId::FoldersHint.id())
            && hint.text() != text
        {
            hint.set_text(&text);
        }
    }

    /// Does the newest job until `end`, then the one before it, and opens
    /// each folder whose first subfolders it added, because the tree opened
    /// it while it had none.
    fn work_until(&self, tree: FolderTree, end: Instant) {
        let mut opened = Vec::new();
        while Instant::now() < end {
            let mut jobs = self.jobs.borrow_mut();
            match jobs.last_mut() {
                None => break,
                Some(Job::Add(adding)) => match adding.folders.next() {
                    Some(folder) => {
                        if !adding.opened {
                            adding.opened = true;
                            opened.push(adding.item);
                        }
                        self.add_below(tree, adding.item, adding, folder);
                    }
                    None => drop(jobs.pop()),
                },
                Some(Job::Recheck(items)) => match items.pop() {
                    Some(item) => {
                        self.recheck(tree, item);
                        // Reversed, so the walk goes down the tree as it
                        // shows.
                        items.extend(tree.children(Some(item)).into_iter().rev());
                    }
                    None => drop(jobs.pop()),
                },
            }
        }
        for item in opened {
            tree.expand(item);
        }
    }

    /// Sets the check boxes of the rows in view from the settings while a
    /// click's are still being set, so the person sees, and clicks, what
    /// the settings hold.
    fn recheck_visible(&self, tree: FolderTree) {
        let rechecking = (self.jobs.borrow().iter()).any(|job| matches!(job, Job::Recheck(_)));
        if rechecking {
            for item in tree.visible_items() {
                self.recheck(tree, item);
            }
        }
    }

    /// Sets `item`'s check box from the settings.
    fn recheck(&self, tree: FolderTree, item: TreeItem) {
        let nodes = self.nodes.borrow();
        if let Some(node) = nodes.get(tree.data(item)) {
            let check =
                (self.settings.borrow()).folder_check(&self.drives[node.drive].volume, &node.path);
            self.filling(|| tree.set_check(item, check));
        }
    }

    /// Sets every check box the tree shows from the settings, a slice at a
    /// time, because a click changes the folders above and below it as well.
    /// The walk starts again from the top, so it replaces an earlier click's.
    fn refresh_checks(&self, dialog: Hwnd, tree: FolderTree) {
        let mut tops = tree.children(None);
        tops.reverse();
        {
            let mut jobs = self.jobs.borrow_mut();
            jobs.retain(|job| !matches!(job, Job::Recheck(_)));
            jobs.push(Job::Recheck(tops));
        }
        self.work(dialog);
    }

    fn fill_summary(&self, dialog: Hwnd) {
        let rows = self
            .main
            .include_summary(self.strings, self.drives, &self.settings.borrow());
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
        let Some(item) = tree
            .children(None)
            .into_iter()
            .find(|&item| self.node(tree, item).is_some_and(|(d, _)| d == drive))
        else {
            return;
        };
        let names = path.split('\\').filter(|name| !name.is_empty());
        self.revealing
            .replace(Some((item, names.map(str::to_owned).collect())));
        self.reveal_more(dialog, tree);
    }

    /// Opens the tree toward the folder that [`Self::revealing`] names, as
    /// far as the folders on the way are in the tree, and selects it once
    /// there. A folder on the way that is still listed or added stops it
    /// until the timer calls it again.
    fn reveal_more(&self, dialog: Hwnd, tree: FolderTree) {
        let Some((mut item, mut names)) = self.revealing.take() else {
            return;
        };
        while let Some(name) = names.front() {
            self.list_later(dialog, tree, item);
            if self.adds_children(tree, item) {
                self.revealing.replace(Some((item, names)));
                return;
            }
            tree.expand(item);
            let child = tree.children(Some(item)).into_iter().find(|&child| {
                self.node(tree, child).is_some_and(|(_, path)| {
                    let child_name = path.rsplit('\\').next().unwrap_or(&path);
                    child_name.eq_ignore_ascii_case(name)
                })
            });
            match child {
                Some(child) => {
                    item = child;
                    names.pop_front();
                }
                // The folder is gone from the drive; show the nearest one.
                None => break,
            }
        }
        tree.select(item);
    }

    /// Whether a thread lists `item`'s subfolders or the timer still adds
    /// them.
    fn adds_children(&self, tree: FolderTree, item: TreeItem) -> bool {
        let listing = self
            .nodes
            .borrow()
            .get(tree.data(item))
            .map(|node| node.listing);
        listing == Some(Listing::Pending)
            || (self.jobs.borrow().iter())
                .any(|job| matches!(job, Job::Add(adding) if adding.item == item))
    }
}

impl Drop for IndexedFolders<'_> {
    fn drop(&mut self) {
        let mut retired = Retirement::default();
        retired.push(self.nodes.take(), INLINE_ITEMS + 1);
        retired.push(self.rows.take(), INLINE_ITEMS + 1);
        if let Some((_, names)) = self.revealing.take() {
            retired.push(names, INLINE_ITEMS + 1);
        }
        retired.push(self.settings.take(), INLINE_ITEMS + 1);
        for job in self.jobs.take() {
            if let Job::Add(adding) = job {
                retired.push(adding.folders, INLINE_ITEMS + 1);
            }
        }
        let mut work = self.main.indexed();
        if let Some(session) = self.session.take() {
            work.end(session);
        }
        work.retire(retired);
    }
}

impl DialogProc for IndexedFolders<'_> {
    fn closed(&self, dialog: Hwnd) {
        dialog.kill_timer(WORK_TIMER);
        if let Some(session) = self.session.take() {
            self.main.indexed().end(session);
        }
    }

    fn init(&self, dialog: Hwnd) -> Opened {
        self.session.set(Some(self.main.indexed().begin()));
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
                // Each drive opens one level deep once listed, so the
                // folders to uncheck are in view; each volume has its own
                // listing slot, so a sleeping disk holds back no other drive.
                if let Some(item) = item {
                    self.list_later(dialog, tree, item);
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
            DialogMessage::Timer { id: WORK_TIMER } => {
                self.work(dialog);
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
                    // The control changed the check box itself.
                    self.refresh_checks(dialog, tree);
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
            // The dialog ends without waiting for the tree's items to go.
            DialogMessage::Command { id }
                if id == DialogItem::Ok.id() || id == DialogItem::Cancel.id() =>
            {
                if let Some(tree) = Self::tree(dialog) {
                    tree.destroy_later();
                }
                false
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
