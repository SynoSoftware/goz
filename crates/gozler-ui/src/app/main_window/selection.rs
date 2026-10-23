//! Captures native selection in bounded turns, before commands or folder
//! candidates consume its indices. The control remains the selection owner.

use super::*;
use crate::core::selection::{CaptureKey, Capturing, RowsChange};
use crate::core::work::INLINE_ITEMS;
use std::collections::LinkedList;

#[derive(Default)]
pub(super) struct NativeSelection {
    capture: Capturing,
    reading: Option<NativeRead>,
    applying: bool,
    writing: bool,
    update: Option<NativeUpdate>,
    action: Option<SelectionAction>,
    keep_quick: bool,
    unhighlight: Option<Unhighlight>,
    unhighlighting: bool,
}

#[derive(Clone, Copy)]
struct Unhighlight {
    key: CaptureKey,
    after: Option<u64>,
}

struct NativeUpdate {
    key: CaptureKey,
    selected: RowSelection,
    range: usize,
    row: Option<u64>,
    focused: Option<u64>,
    to_focus: bool,
    initialized: bool,
    unchanged: bool,
}

#[derive(Clone, Copy)]
pub(super) struct CommandContext {
    pub(super) mode: Mode,
    pub(super) list: List,
    pub(super) focus: Focus,
    key: CaptureKey,
}

#[derive(Clone, Copy)]
enum SelectionAction {
    Command(MenuItem, CommandContext),
    SearchMenu(Option<(i32, i32)>, CommandContext),
    PanelMenu(Side, Option<(i32, i32)>, CommandContext),
}

impl SelectionAction {
    fn context(self) -> CommandContext {
        match self {
            Self::Command(_, context)
            | Self::SearchMenu(_, context)
            | Self::PanelMenu(_, _, context) => context,
        }
    }

    fn remapped(self, key: CaptureKey) -> Self {
        let mut context = self.context();
        context.key = key;
        match self {
            Self::Command(item, _) => Self::Command(item, context),
            Self::SearchMenu(point, _) => Self::SearchMenu(point, context),
            Self::PanelMenu(side, point, _) => Self::PanelMenu(side, point, context),
        }
    }
}

struct NativeRead {
    key: CaptureKey,
    count: u64,
    expected: u64,
    after: Option<u64>,
    chunks: LinkedList<Vec<Range<u64>>>,
    current: Vec<Range<u64>>,
    items: usize,
    read: u64,
}

impl NativeRead {
    fn push(&mut self, row: u64) {
        if self.current.len() == INLINE_ITEMS {
            self.chunks.push_back(std::mem::replace(
                &mut self.current,
                Vec::with_capacity(INLINE_ITEMS),
            ));
        }
        match self.current.last_mut() {
            Some(last) if last.end == row => last.end = row + 1,
            _ => {
                self.current.push(row..row + 1);
                self.items += 1;
            }
        }
        self.after = Some(row);
        self.read += 1;
    }
}

impl MainWindow {
    pub(super) fn command_context(&self) -> CommandContext {
        let (mode, list) = {
            let model = self.model.borrow();
            (model.mode(), model.active_list())
        };
        CommandContext {
            mode,
            list,
            focus: self.focus(),
            key: self.native_key(list),
        }
    }

    pub(super) fn selection_context_current(&self, context: CommandContext) -> bool {
        let mode = self.model.borrow().mode();
        context.mode == mode && context.key.same_list(self.native_key(context.list))
    }

    fn read_target_selection(&self, hwnd: Hwnd, list: List) -> bool {
        match list {
            List::Search => self.read_list_selection(hwnd),
            List::Panel(side) => self.read_selection(side, true),
        }
    }

    fn defer_selection_action(&self, action: SelectionAction) {
        let list = action.context().list;
        let accepted = {
            let mut native = self.native_selection(list).borrow_mut();
            if native.action.is_some() {
                false
            } else {
                native.action = Some(action);
                true
            }
        };
        if accepted {
            self.list_control(list).hwnd().set_enabled(false);
        }
    }

    pub(super) fn prepare_selection_command(
        &self,
        hwnd: Hwnd,
        item: MenuItem,
        context: CommandContext,
    ) -> bool {
        if self
            .native_selection(context.list)
            .borrow()
            .action
            .is_some()
        {
            return false;
        }
        if self.read_target_selection(hwnd, context.list) && !self.selection_waiting(context.list) {
            return true;
        }
        self.defer_selection_action(SelectionAction::Command(item, context));
        false
    }

    pub(super) fn resume_selection_actions(&self, hwnd: Hwnd) {
        for list in List::ALL {
            let action = self.native_selection(list).borrow().action;
            let Some(action) = action else {
                continue;
            };
            let context = action.context();
            let current = self.selection_context_current(context);
            if current {
                if !self.read_target_selection(hwnd, list) {
                    continue;
                }
                if self.selection_waiting(list) {
                    continue;
                }
            }
            self.native_selection(list).borrow_mut().action = None;
            if !self.native_update_active(list) && !self.native_deselection_pending(list) {
                self.list_control(list).hwnd().set_enabled(true);
            }
            if current {
                match action {
                    SelectionAction::Command(item, context) => {
                        self.run_command(hwnd, item, context)
                    }
                    SelectionAction::SearchMenu(point, context) => {
                        self.row_menu_ready(hwnd, point, context)
                    }
                    SelectionAction::PanelMenu(side, point, context) => {
                        self.panel_menu_ready(hwnd, side, point, context)
                    }
                }
            }
            if self.shutdown.borrow().is_some() {
                break;
            }
        }
    }

    fn selection_waiting(&self, list: List) -> bool {
        if self.native_deselection_pending(list) {
            return true;
        }
        let model = self.model.borrow();
        model.list(list).is_some_and(|shown| {
            shown.is_keeping()
                || matches!(
                    shown.selection(),
                    Selection::Reading { .. } | Selection::Filtering { .. }
                )
        }) || matches!(list, List::Panel(side) if model.panels().panel(side).is_calculating())
    }
    fn native_selection(&self, list: List) -> &RefCell<NativeSelection> {
        let index = List::ALL
            .iter()
            .position(|each| *each == list)
            .expect("ALL holds every list");
        &self.told[index]
    }

    fn retire_native_selection(&self, list: List) {
        let retired = self
            .native_selection(list)
            .borrow_mut()
            .capture
            .take_retired();
        self.created().work.borrow_mut().retire(retired);
    }

    fn native_key(&self, list: List) -> CaptureKey {
        let count = self.list_control(list).count();
        self.native_selection(list).borrow().capture.key(count)
    }

    pub(super) fn selection_changed(&self, list: List, change: RowsChange) -> bool {
        let mut native = self.native_selection(list).borrow_mut();
        if native.applying {
            return false;
        }
        native.keep_quick = false;
        if !native.unhighlighting
            && let Some(pending) = native.unhighlight.as_mut()
        {
            pending.after = None;
        }
        native.capture.changed(change);
        drop(native);
        self.retire_native_selection(list);
        true
    }

    pub(super) fn gate_changed_selections(&self) {
        let mut model = self.model.borrow_mut();
        for side in Side::BOTH {
            if self.panel_changed[side.index()].get() == panels::ListChanged::Selection {
                model.panels_mut().native_selection_pending(side, true);
            }
        }
    }

    pub(super) fn accept_told_selections(&self, hwnd: Hwnd) {
        let answers = self.created().work.borrow_mut().take_told();
        for (list, answer) in answers {
            self.native_selection(list)
                .borrow_mut()
                .capture
                .accept(answer);
            self.retire_native_selection(list);
            hwnd.post(DEFERRED);
        }
    }

    pub(super) fn seed_native_selection(&self, list: List, selected: RowSelection) {
        let count = self.list_control(list).count();
        let mut retired = Retirement::default();
        let input = {
            let mut native = self.native_selection(list).borrow_mut();
            retired.push(native.reading.take(), INLINE_ITEMS + 1);
            retired.push(native.update.take(), INLINE_ITEMS + 1);
            native.applying = false;
            native.action = None;
            native.unhighlight = None;
            native.keep_quick = false;
            native.capture.seed(count, selected)
        };
        self.created().work.borrow_mut().retire(retired);
        self.created().work.borrow_mut().tell(list, input);
        self.retire_native_selection(list);
        if let List::Panel(side) = list {
            self.model
                .borrow_mut()
                .panels_mut()
                .native_selection_pending(side, false);
            self.panel_changed[side.index()].set(panels::ListChanged::Nothing);
        } else {
            self.selection_pending.set(false);
        }
        self.list_control(list).hwnd().set_enabled(true);
    }

    pub(super) fn selected_rows(&self, list: List) -> Option<RowSelection> {
        if self.native_selection(list).borrow().applying {
            return None;
        }
        let control = self.list_control(list);
        let hwnd = control.hwnd().parent();
        for _ in 0..2 {
            let count = control.count();
            let key = self.native_selection(list).borrow().capture.key(count);
            let selected = self.native_selection(list).borrow().capture.selected(count);
            if let Some(selected) = selected {
                let first = control.next_selected(None);
                let confirmed = selected.count == control.selected_count()
                    && selected.positions.first().map(|range| range.start) == first
                    && selected
                        .positions
                        .last()
                        .is_none_or(|range| control.row_state(range.end - 1).1);
                if key != self.native_key(list) {
                    break;
                }
                if confirmed {
                    return Some(selected);
                }
                self.native_selection(list)
                    .borrow_mut()
                    .capture
                    .invalidate();
            }
            let input = self.native_selection(list).borrow_mut().capture.take(count);
            if let Some(input) = input {
                self.created().work.borrow_mut().tell(list, input);
                self.accept_told_selections(hwnd);
                continue;
            }
            if !self.native_selection(list).borrow().capture.needs_native() {
                break;
            }
            self.read_native_selection(list, count, key);
            let selected = self.native_selection(list).borrow().capture.selected(count);
            self.retire_native_selection(list);
            return selected;
        }
        self.retire_native_selection(list);
        None
    }

    fn read_native_selection(&self, list: List, count: u64, key: CaptureKey) {
        let control = self.list_control(list);
        let mut read = self.native_selection(list).borrow_mut().reading.take();
        if read.as_ref().is_some_and(|read| read.key != key) {
            let mut retired = Retirement::default();
            retired.push(read.take(), INLINE_ITEMS + 1);
            self.created().work.borrow_mut().retire(retired);
        }
        if read.is_none() {
            let expected = control.selected_count();
            let first = control.next_selected(None);
            if expected == 0
                || first.is_some_and(|first| first <= count && expected == count - first)
            {
                let positions = first
                    .filter(|_| expected != 0)
                    .into_iter()
                    .map(|first| first..count)
                    .collect();
                if key == self.native_key(list) {
                    let input = self
                        .native_selection(list)
                        .borrow_mut()
                        .capture
                        .known(count, RowSelection::new(positions, expected));
                    self.created().work.borrow_mut().tell(list, input);
                    self.accept_told_selections(control.hwnd().parent());
                }
                return;
            }
            read = Some(NativeRead {
                key,
                count,
                expected,
                after: None,
                chunks: LinkedList::new(),
                current: Vec::with_capacity(INLINE_ITEMS),
                items: 0,
                read: 0,
            });
        }
        let mut read = read.expect("a scattered selection has a native cursor");
        let mut finished = false;
        for _ in 0..INLINE_ITEMS {
            let next = control.next_selected(read.after);
            if self.native_key(list) != key {
                break;
            }
            match next {
                Some(row) if row < read.count => read.push(row),
                _ => {
                    finished = true;
                    break;
                }
            }
        }
        if finished && read.read == read.expected && control.selected_count() == read.expected {
            read.chunks.push_back(std::mem::take(&mut read.current));
            let input =
                self.native_selection(list)
                    .borrow_mut()
                    .capture
                    .read(key, read.chunks, read.items);
            if let Some(input) = input {
                self.created().work.borrow_mut().tell(list, input);
                self.accept_told_selections(control.hwnd().parent());
            }
        } else if !finished && self.native_key(list) == key {
            self.native_selection(list).borrow_mut().reading = Some(read);
            control.hwnd().parent().post(DEFERRED);
        } else {
            let mut retired = Retirement::default();
            retired.push(read, INLINE_ITEMS + 1);
            self.created().work.borrow_mut().retire(retired);
            control.hwnd().parent().post(DEFERRED);
        }
    }

    pub(super) fn selection_command_pending(&self, list: List) -> bool {
        self.native_selection(list).borrow().action.is_some()
    }

    pub(super) fn await_search_menu(
        &self,
        hwnd: Hwnd,
        point: Option<(i32, i32)>,
    ) -> Option<CommandContext> {
        let context = self.command_context();
        if self.selection_command_pending(context.list) {
            return None;
        }
        if self.read_target_selection(hwnd, context.list) && !self.selection_waiting(context.list) {
            return Some(context);
        }
        self.defer_selection_action(SelectionAction::SearchMenu(point, context));
        None
    }

    pub(super) fn await_panel_menu(
        &self,
        hwnd: Hwnd,
        side: Side,
        point: Option<(i32, i32)>,
    ) -> Option<CommandContext> {
        let mut context = self.command_context();
        context.list = List::Panel(side);
        context.key = self.native_key(context.list);
        if self.selection_command_pending(context.list) {
            return None;
        }
        if self.read_target_selection(hwnd, context.list) && !self.selection_waiting(context.list) {
            return Some(context);
        }
        self.defer_selection_action(SelectionAction::PanelMenu(side, point, context));
        None
    }

    pub(super) fn native_update_active(&self, list: List) -> bool {
        self.native_selection(list).borrow().applying
    }

    pub(super) fn keep_quick_selection(&self, side: Side) {
        self.native_selection(List::Panel(side))
            .borrow_mut()
            .keep_quick = true;
    }

    pub(super) fn selection_closes_quick(&self, side: Side, closes: bool) -> bool {
        closes && !self.native_selection(List::Panel(side)).borrow().keep_quick
    }

    pub(super) fn native_deselection_pending(&self, list: List) -> bool {
        let native = self.native_selection(list).borrow();
        native.unhighlight.is_some() || native.unhighlighting
    }

    pub(super) fn request_taken_deselection(&self, list: List) {
        let key = self.native_key(list);
        self.native_selection(list).borrow_mut().unhighlight =
            Some(Unhighlight { key, after: None });
        if let List::Panel(side) = list {
            self.model
                .borrow_mut()
                .panels_mut()
                .native_selection_pending(side, true);
        }
        let control = self.list_control(list);
        control.hwnd().set_enabled(false);
        control.hwnd().parent().post(DEFERRED);
    }

    pub(super) fn apply_taken_deselections(&self, hwnd: Hwnd) {
        for list in List::ALL {
            if self.native_update_active(list) {
                continue;
            }
            let waiting = self.model.borrow().list(list).is_some_and(|shown| {
                shown.is_keeping() || matches!(shown.selection(), Selection::Filtering { .. })
            });
            if waiting {
                continue;
            }
            let pending = self.native_selection(list).borrow_mut().unhighlight.take();
            let Some(mut pending) = pending else {
                continue;
            };
            let current =
                || pending.key.same_list(self.native_key(list)) && self.shutdown.borrow().is_none();
            if !current() {
                continue;
            }
            self.native_selection(list).borrow_mut().unhighlighting = true;
            let control = self.list_control(list);
            let mut selected = Vec::with_capacity(INLINE_ITEMS);
            let mut finished = false;
            for _ in 0..INLINE_ITEMS {
                match control.next_selected(pending.after) {
                    Some(row) if current() => {
                        selected.push(row..row + 1);
                        pending.after = Some(row);
                    }
                    _ => {
                        finished = true;
                        break;
                    }
                }
            }
            let rows = if current() {
                self.model.borrow().taken_rows(list, &selected)
            } else {
                Vec::new()
            };
            for row in rows {
                if !current() {
                    break;
                }
                control.deselect(row);
            }
            let current = current();
            let complete = {
                let mut native = self.native_selection(list).borrow_mut();
                native.unhighlighting = false;
                if current && !finished && native.unhighlight.is_none() {
                    native.unhighlight = Some(pending);
                }
                native.unhighlight.is_none()
            };
            if complete && current {
                if !self.selection_command_pending(list) {
                    control.hwnd().set_enabled(true);
                }
                match list {
                    List::Search => self.selection_pending.set(true),
                    List::Panel(side) => {
                        self.panel_changed[side.index()].set(panels::ListChanged::Selection)
                    }
                }
            }
            hwnd.post(DEFERRED);
        }
    }

    pub(super) fn retire_native_selections(&self) {
        let mut retired = Retirement::default();
        for slot in &self.told {
            retired.push(std::mem::take(&mut *slot.borrow_mut()), INLINE_ITEMS + 1);
        }
        self.created().work.borrow_mut().retire(retired);
    }

    pub(super) fn show_native_place(
        &self,
        side: Side,
        before: &crate::core::panel::Place,
        place: &crate::core::panel::Place,
        to_focus: bool,
        keep_action: bool,
    ) {
        let list = List::Panel(side);
        let (count, selected_count) = {
            let model = self.model.borrow();
            (
                model.panels().panel(side).count(),
                model.panels().selected_count(side),
            )
        };
        let mut retired = Retirement::default();
        {
            let mut native = self.native_selection(list).borrow_mut();
            retired.push(native.reading.take(), INLINE_ITEMS + 1);
            retired.push(native.update.take(), INLINE_ITEMS + 1);
            native.capture.reset();
            native.unhighlight = None;
            if keep_action {
                let key = native.capture.key(count);
                native.action = native.action.map(|action| action.remapped(key));
            } else {
                native.action = None;
            }
            native.applying = true;
            native.update = Some(NativeUpdate {
                key: native.capture.key(count),
                selected: RowSelection {
                    positions: Arc::clone(&place.selected),
                    count: selected_count,
                },
                range: 0,
                row: None,
                focused: place.focused,
                to_focus,
                initialized: false,
                unchanged: Arc::ptr_eq(&before.selected, &place.selected),
            });
        }
        self.model
            .borrow_mut()
            .panels_mut()
            .native_selection_pending(side, true);
        self.created().work.borrow_mut().retire(retired);
        let control = self.list_control(list);
        control.hwnd().set_enabled(false);
        control.set_count(count);
        self.apply_native_place(side);
    }

    pub(super) fn apply_native_place(&self, side: Side) {
        let list = List::Panel(side);
        {
            let mut native = self.native_selection(list).borrow_mut();
            if native.writing {
                return;
            }
            native.writing = true;
        }
        self.write_native_place(side);
        let pending = {
            let mut native = self.native_selection(list).borrow_mut();
            native.writing = false;
            native.update.is_some()
        };
        if pending && self.shutdown.borrow().is_none() {
            self.list_control(list).hwnd().parent().post(DEFERRED);
        }
    }

    fn write_native_place(&self, side: Side) {
        let list = List::Panel(side);
        let update = self.native_selection(list).borrow_mut().update.take();
        let Some(mut update) = update else {
            return;
        };
        let key = update.key;
        let current = || self.native_key(list) == key && self.shutdown.borrow().is_none();
        let control = self.list_control(list);
        let mut remaining = INLINE_ITEMS;
        if !update.initialized && current() {
            update.initialized = true;
            if update.unchanged {
                update.range = update.selected.positions.len();
            } else if let [all] = update.selected.positions.as_slice()
                && all.end == control.count()
                && all.start < 2
            {
                control.select_all();
                if current() && all.start == 1 {
                    control.select(0, false);
                }
                update.range = update.selected.positions.len();
                remaining -= 2;
            } else {
                control.deselect_all();
                remaining -= 1;
            }
        }
        control.hwnd().draw_once_after(|| {
            while remaining > 0 && current() {
                let Some(range) = update.selected.positions.get(update.range) else {
                    break;
                };
                let row = update.row.unwrap_or(range.start);
                control.select(row, true);
                if row + 1 == range.end {
                    update.range += 1;
                    update.row = None;
                } else {
                    update.row = Some(row + 1);
                }
                remaining -= 1;
            }
        });
        if !current() {
            let mut retired = Retirement::default();
            retired.push(update, INLINE_ITEMS + 1);
            self.created().work.borrow_mut().retire(retired);
            return;
        }
        if update.range < update.selected.positions.len() {
            self.native_selection(list).borrow_mut().update = Some(update);
            control.hwnd().parent().post(DEFERRED);
            return;
        }
        if update.to_focus {
            control.set_focused(update.focused);
        } else {
            control.focus_in_place(update.focused);
        }
        if !current() {
            let mut retired = Retirement::default();
            retired.push(update, INLINE_ITEMS + 1);
            self.created().work.borrow_mut().retire(retired);
            return;
        }
        let focused = control.focused();
        self.model
            .borrow_mut()
            .panels_mut()
            .set_focused(side, focused);
        self.panel_changed[side.index()].set(panels::ListChanged::Nothing);
        let count = control.count();
        let input = {
            let mut native = self.native_selection(list).borrow_mut();
            native.applying = false;
            native.capture.known(count, update.selected.clone())
        };
        self.model
            .borrow_mut()
            .panels_mut()
            .native_selection_pending(side, false);
        self.created().work.borrow_mut().tell(list, input);
        self.retire_native_selection(list);
        if !self.selection_command_pending(list) && !self.native_deselection_pending(list) {
            control.hwnd().set_enabled(true);
        }
        let select_focus = update.to_focus
            && update.selected.positions.is_empty()
            && focused.is_some_and(|row| self.model.borrow().panels().selectable(side, row));
        if select_focus && let Some(row) = focused {
            control.select(row, true);
        }
        control.redraw_rows(control.visible_rows());
        self.show_panel(side);
        self.update_key_bar();
        self.resume_selection_actions(control.hwnd().parent());
        self.start_list_passes();
    }
}
