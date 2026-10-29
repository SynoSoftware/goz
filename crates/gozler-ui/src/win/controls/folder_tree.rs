use super::{send, send_pointer};
use crate::win::present::{self, Repaint};
use crate::win::{Hwnd, wide};
use gozler_core::include::FolderCheck;
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Controls::{
    HTREEITEM, SetWindowTheme, TREE_VIEW_ITEM_STATE_FLAGS, TVE_EXPAND, TVGN_CARET, TVGN_CHILD,
    TVGN_FIRSTVISIBLE, TVGN_NEXT, TVGN_NEXTVISIBLE, TVGN_ROOT, TVI_LAST, TVI_ROOT, TVIF_CHILDREN,
    TVIF_PARAM, TVIF_STATE, TVIF_TEXT, TVINSERTSTRUCTW, TVINSERTSTRUCTW_0, TVIS_STATEIMAGEMASK,
    TVITEMEXW_CHILDREN, TVITEMW, TVM_DELETEITEM, TVM_ENSUREVISIBLE, TVM_EXPAND, TVM_GETITEMSTATE,
    TVM_GETITEMW, TVM_GETNEXTITEM, TVM_GETVISIBLECOUNT, TVM_INSERTITEMW, TVM_SELECTITEM,
    TVM_SETEXTENDEDSTYLE, TVM_SETITEMW, TVS_CHECKBOXES, TVS_EX_AUTOHSCROLL, TVS_EX_DOUBLEBUFFER,
    TVS_EX_PARTIALCHECKBOXES,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GWL_STYLE, GetWindowLongPtrW, HWND_MESSAGE, KillTimer, SetParent, SetTimer,
    SetWindowLongPtrW,
};
use windows::core::{PCWSTR, PWSTR, w};

/// An item of a [`FolderTree`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TreeItem(pub(crate) HTREEITEM);

/// A tree view with a check box on each item, which a dialog's template
/// declares, such as Indexed folders' folders. Each item carries a number,
/// which the dialog gives it, and a check box that shows a [`FolderCheck`].
///
/// Each item that the tree adds, removes or checks raises an accessibility
/// event, which costs up to a few tenths of a millisecond while another
/// program listens for them, so a folder such as `C:\Windows\WinSxS`, with
/// tens of thousands of subfolders, takes seconds.
#[derive(Clone, Copy)]
pub(crate) struct FolderTree(Hwnd);

impl FolderTree {
    /// The tree view `id` of a dialog.
    pub(crate) fn of_dialog(dialog: Hwnd, id: u16) -> Option<FolderTree> {
        crate::win::dialog::item(dialog, id).map(FolderTree)
    }

    /// Gives the tree check boxes that can show [`FolderCheck::Partial`],
    /// and File Explorer's look: no horizontal scroll bar, which the dialog
    /// template turns off. With `TVS_EX_AUTOHSCROLL` as well, opening
    /// `C:\Windows\WinSxS` took 2 ms instead of 22. Call it before the first
    /// item is added.
    pub(crate) fn show_check_boxes(self) {
        // Windows makes the check boxes' images only when the style is set
        // after the control exists and before it holds an item.
        // SAFETY: plain values on a window of this thread.
        unsafe {
            let style = GetWindowLongPtrW(self.0.0, GWL_STYLE);
            SetWindowLongPtrW(self.0.0, GWL_STYLE, style | TVS_CHECKBOXES as isize);
        }
        let extended = TVS_EX_PARTIALCHECKBOXES | TVS_EX_DOUBLEBUFFER | TVS_EX_AUTOHSCROLL;
        send(
            self.0,
            TVM_SETEXTENDEDSTYLE,
            extended as usize,
            extended as isize,
        );
        // SAFETY: plain values; a missing theme leaves the classic look.
        let _ = unsafe { SetWindowTheme(self.0.0, w!("Explorer"), PCWSTR::null()) };
    }

    /// Adds the item `text`, carrying `data` and showing `check`, as the
    /// last child of `parent`, or as the last top item. With
    /// `may_have_children`, it shows the button that expands it, before its
    /// children are added. The check box is set with the item, because
    /// setting it afterwards costs one more accessibility event.
    pub(crate) fn insert(
        self,
        parent: Option<TreeItem>,
        text: &str,
        data: usize,
        may_have_children: bool,
        check: FolderCheck,
    ) -> Option<TreeItem> {
        let mut text = wide(text);
        let insert = TVINSERTSTRUCTW {
            hParent: parent.map_or(TVI_ROOT, |p| p.0),
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                item: TVITEMW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN | TVIF_STATE,
                    pszText: PWSTR(text.as_mut_ptr()),
                    cChildren: TVITEMEXW_CHILDREN(i32::from(may_have_children)),
                    lParam: LPARAM(data as isize),
                    state: check_state(check),
                    stateMask: TVIS_STATEIMAGEMASK,
                    ..Default::default()
                },
            },
        };
        let item = self.hold_after(|| {
            // SAFETY: `insert` and its text outlive the call, which copies
            // them.
            unsafe { send_pointer(self.0, TVM_INSERTITEMW, 0, &raw const insert) }
        });
        (item != 0).then_some(TreeItem(HTREEITEM(item)))
    }

    /// Shows that `item` has no children, after a listing found none.
    pub(crate) fn set_childless(self, item: TreeItem) {
        let change = TVITEMW {
            mask: TVIF_CHILDREN,
            hItem: item.0,
            ..Default::default()
        };
        self.hold_after(|| {
            // SAFETY: `change` outlives the call, which reads it.
            unsafe { send_pointer(self.0, TVM_SETITEMW, 0, &raw const change) };
        });
    }

    /// Shows `check` on `item`'s check box, unless it shows it already,
    /// because each change raises an accessibility event.
    pub(crate) fn set_check(self, item: TreeItem, check: FolderCheck) {
        let shown = send(
            self.0,
            TVM_GETITEMSTATE,
            item.0.0 as usize,
            TVIS_STATEIMAGEMASK.0 as isize,
        );
        let state = check_state(check);
        if shown as u32 == state.0 {
            return;
        }
        let change = TVITEMW {
            mask: TVIF_STATE,
            hItem: item.0,
            state,
            stateMask: TVIS_STATEIMAGEMASK,
            ..Default::default()
        };
        self.hold_after(|| {
            // SAFETY: `change` outlives the call, which reads it.
            unsafe { send_pointer(self.0, TVM_SETITEMW, 0, &raw const change) };
        });
    }

    /// The number the item carries.
    pub(crate) fn data(self, item: TreeItem) -> usize {
        let mut read = TVITEMW {
            mask: TVIF_PARAM,
            hItem: item.0,
            ..Default::default()
        };
        // SAFETY: `read` outlives the call, which fills it in.
        unsafe { send_pointer(self.0, TVM_GETITEMW, 0, &raw mut read) };
        read.lParam.0 as usize
    }

    /// The children of `parent`, or the top items, in their order.
    pub(crate) fn children(self, parent: Option<TreeItem>) -> Vec<TreeItem> {
        let mut children = Vec::new();
        let mut next = match parent {
            Some(parent) => send(self.0, TVM_GETNEXTITEM, TVGN_CHILD as usize, parent.0.0),
            None => send(self.0, TVM_GETNEXTITEM, TVGN_ROOT as usize, 0),
        };
        while next != 0 {
            children.push(TreeItem(HTREEITEM(next)));
            next = send(self.0, TVM_GETNEXTITEM, TVGN_NEXT as usize, next);
        }
        children
    }

    /// The items in view, top to bottom, with the one that the bottom edge
    /// cuts.
    pub(crate) fn visible_items(self) -> Vec<TreeItem> {
        let count = send(self.0, TVM_GETVISIBLECOUNT, 0, 0) as usize + 1;
        let mut items = Vec::with_capacity(count);
        let mut item = send(self.0, TVM_GETNEXTITEM, TVGN_FIRSTVISIBLE as usize, 0);
        while item != 0 && items.len() < count {
            items.push(TreeItem(HTREEITEM(item)));
            item = send(self.0, TVM_GETNEXTITEM, TVGN_NEXTVISIBLE as usize, item);
        }
        items
    }

    pub(crate) fn expand(self, item: TreeItem) {
        self.hold_after(|| send(self.0, TVM_EXPAND, TVE_EXPAND.0 as usize, item.0.0));
    }

    /// Runs `change`, after which the tree draws itself and updates its
    /// scroll bar at once, so that a slice of many items draws the tree
    /// once ([`present::hold_after`]).
    fn hold_after<T>(self, change: impl FnOnce() -> T) -> T {
        present::hold_after(self.0, Repaint::Erased, change)
    }

    /// Selects `item` and scrolls it into view.
    pub(crate) fn select(self, item: TreeItem) {
        send(self.0, TVM_SELECTITEM, TVGN_CARET as usize, item.0.0);
        send(self.0, TVM_ENSUREVISIBLE, 0, item.0.0);
    }

    /// Takes the tree out of its dialog, which can then close at once, and
    /// removes its items a slice at a time on a timer of this thread before
    /// it destroys it, because removing them all at once would stop the
    /// thread for seconds.
    pub(crate) fn destroy_later(self) {
        // SAFETY: plain values on a window of this thread; a message-only
        // window is never shown.
        let _ = unsafe { SetParent(self.0.0, Some(HWND_MESSAGE)) };
        DYING_TREES.with_borrow_mut(|trees| trees.push(self));
        // SAFETY: plain values; one thread timer serves every tree, and an
        // id that names none makes a new one.
        let timer = unsafe { SetTimer(None, DYING_TIMER.get(), TREE_SLICE_MS, Some(remove_slice)) };
        DYING_TIMER.set(timer);
    }

    /// The first item that has no children, if the tree holds any item.
    fn first_leaf(self) -> Option<TreeItem> {
        let mut item = send(self.0, TVM_GETNEXTITEM, TVGN_ROOT as usize, 0);
        if item == 0 {
            return None;
        }
        loop {
            match send(self.0, TVM_GETNEXTITEM, TVGN_CHILD as usize, item) {
                0 => return Some(TreeItem(HTREEITEM(item))),
                child => item = child,
            }
        }
    }
}

/// The time a slice of a dying tree's removal takes at most, and its
/// timer's period.
const TREE_SLICE_MS: u32 = 15;

thread_local! {
    /// The trees that [`FolderTree::destroy_later`] removes.
    static DYING_TREES: RefCell<Vec<FolderTree>> = const { RefCell::new(Vec::new()) };
    /// The id of the thread timer that removes them, which the first
    /// `SetTimer` gives.
    static DYING_TIMER: Cell<usize> = const { Cell::new(0) };
}

/// The dying trees' timer: removes their items for one slice, leaves first,
/// so each removal is one item, and destroys each tree once it is empty.
unsafe extern "system" fn remove_slice(_: HWND, _: u32, timer: usize, _: u32) {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(TREE_SLICE_MS.into());
    DYING_TREES.with_borrow_mut(|trees| {
        while let Some(&tree) = trees.last() {
            while let Some(leaf) = tree.first_leaf() {
                send(tree.0, TVM_DELETEITEM, 0, leaf.0.0);
                if std::time::Instant::now() >= end {
                    return;
                }
            }
            // SAFETY: a window of this thread that no dialog holds any more.
            let _ = unsafe { DestroyWindow(tree.0.0) };
            trees.pop();
        }
        // SAFETY: the thread timer this call runs for.
        let _ = unsafe { KillTimer(None, timer) };
        DYING_TIMER.set(0);
    });
}

/// The state image of `check`, of those of `TVS_EX_PARTIALCHECKBOXES`.
fn check_state(check: FolderCheck) -> TREE_VIEW_ITEM_STATE_FLAGS {
    let image: u32 = match check {
        FolderCheck::Unchecked => 1,
        FolderCheck::Checked => 2,
        FolderCheck::Partial => 3,
    };
    TREE_VIEW_ITEM_STATE_FLAGS(image << 12)
}
