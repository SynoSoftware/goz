use super::{
    Button, ButtonIcon, Cell, Column, ColumnFit, ColumnSizing, CommandId, ControlId, CustomDraw,
    DEFERRED, DispInfo, Dpi, DrawAnswer, DrawStage, DropDownList, Edit, Hwnd, LRESULT, Label,
    Layout, List, ListButton, ListCaption, ListNotify, ListView, MainWindow, MatchMode, Matching,
    MenuItem, Px, Range, Rect, RefCell, SELECTION_LIMIT, SortDir, TextMeter, Tool, Toolbar,
    Tooltip, captions, clear_glyph, layout, matched_width, win,
};

pub(super) struct Search {
    pub(super) row: SearchRow,
    pub(super) message: Label,
    pub(super) message_buttons: [Button; 2],
    message_shields: [Cell<bool>; 2],
    pub(super) message_tips: Tooltip,
    pub(super) list: ListView,
    pub(super) select_on_click: Cell<bool>,
    pub(super) column_dragged: Cell<bool>,
    pub(super) column_sizing: RefCell<ColumnSizing>,
    pub(super) selection_pending: Cell<bool>,
}

impl Search {
    pub(super) fn create(
        parent: Hwnd,
        row: SearchRow,
        message_buttons: [Button; 2],
    ) -> windows::core::Result<Self> {
        Ok(Self {
            row,
            message: Label::create_message(parent, ControlId::Message.id())?,
            message_buttons,
            message_shields: [Cell::new(false), Cell::new(false)],
            message_tips: Tooltip::create(parent, &message_buttons.map(Button::hwnd))?,
            list: ListView::create(parent, ControlId::List.id())?,
            select_on_click: Cell::new(false),
            column_dragged: Cell::new(false),
            column_sizing: RefCell::default(),
            selection_pending: Cell::new(false),
        })
    }
}

impl Search {
    pub(super) fn set_captions(&self, window: &MainWindow, captions: &super::WindowCaptions) {
        self.row.input.set_cue(&captions.search_cue);
        // The caption stays the name that screen readers say, and the
        // access key, while Clear shows only its icon.
        let tools = self.row.tools;
        tools.set_text(ControlId::ClearSearch.id(), &captions.clear_search);
        tools.set_text(ControlId::MatchCase.id(), &captions.match_case);
        tools.set_text(ControlId::MatchPath.id(), &captions.match_path);
        self.row.match_label.hwnd().set_text(&captions.match_label);
        let mode = self.match_mode_index(window);
        let modes = captions.match_modes.each_ref().map(String::as_str);
        self.row.match_mode.set_items(&modes, mode);
        for (index, header) in captions.headers.iter().enumerate() {
            self.list
                .set_header(index, self.list.fitting_text(index, header));
        }
    }
}

impl Search {
    pub(super) fn refresh_commands(&self, window: &MainWindow) {
        let (enabled, matching, mode) = {
            let model = window.model.borrow();
            let state = super::State {
                model: &model,
                operations_ready: window.file_operations.get().is_some(),
                focus: super::Focus::Other,
                list: model.active_list(),
            };
            (
                (CommandId::Match.row().enabled)(&state),
                model.matching(),
                self.match_mode_index(window),
            )
        };
        for control in [self.row.match_label.hwnd(), self.row.match_mode.hwnd()] {
            control.set_enabled(enabled);
        }
        for (id, checked) in [
            (ControlId::MatchCase.id(), matching.case),
            (ControlId::MatchPath.id(), matching.path),
        ] {
            self.row.tools.set_enabled(id, enabled);
            self.row.tools.set_checked(id, checked);
        }
        self.row.match_mode.select(mode);
        self.show_sort(window);
    }
}

impl Search {
    pub(super) fn match_mode_index(&self, window: &MainWindow) -> usize {
        let mode = window.model.borrow().matching().mode;
        MatchMode::ALL
            .iter()
            .position(|m| *m == mode)
            .expect("ALL holds every mode")
    }

    /// Shows the list's message and its buttons, or hides them.
    pub(super) fn show_message(&self, window: &MainWindow, message: Option<&ListCaption>) {
        let list = (window.created().layout.borrow().as_ref())
            .and_then(Layout::search)
            .map(|search| search.list);
        let mut moves = Vec::new();
        let mut shown = Vec::new();
        let focus_lost = self.prepare_message(window, message, list, &mut moves, &mut shown);
        win::place_children(&moves, &shown);
        if focus_lost {
            self.row.input.hwnd().focus();
        }
    }

    pub(super) fn prepare_message(
        &self,
        window: &MainWindow,
        message: Option<&ListCaption>,
        list: Option<Rect>,
        moves: &mut Vec<(Hwnd, Rect)>,
        shown: &mut Vec<(Hwnd, bool)>,
    ) -> bool {
        let panes = window.model.borrow().panes();
        let maximized = panes.inspector
            && panes.maximized
            && panes.content == crate::core::layout::InspectorContent::Transfers;
        let message = message.filter(|_| !maximized);
        let controls = window.controls();
        let (text, buttons) = match message {
            Some(message) => (message.text.as_str(), message.buttons.as_slice()),
            None => ("", &[][..]),
        };
        // Read first, because Windows takes the focus from a control it
        // disables.
        let focused = Hwnd::focused();
        controls.search.message.hwnd().set_text(text);
        for (index, button) in controls.search.message_buttons.iter().enumerate() {
            let shown = buttons.get(index);
            button
                .hwnd()
                .set_text(shown.map_or("", |shown| shown.label.as_str()));
            let icon = shown.and_then(|shown| shown.icon);
            let shield = icon == Some(ButtonIcon::Shield);
            if self.message_shields[index].replace(shield) != shield {
                button.set_icon(icon);
            } else if !shield {
                button.set_glyph(match icon {
                    Some(ButtonIcon::Glyph(glyph)) => Some(glyph),
                    Some(ButtonIcon::Shield) | None => None,
                });
            }
            button
                .hwnd()
                .set_enabled(shown.is_none_or(|shown| shown.enabled));
            let tip = shown.and_then(|shown| shown.tooltip.as_deref());
            controls
                .search
                .message_tips
                .set(button.hwnd(), tip.unwrap_or_default());
        }
        // A hidden message is placed when it shows again.
        if message.is_some()
            && let Some(list) = list
        {
            let header = controls.search.list.header().screen_rect().height;
            let shown: Vec<Hwnd> = (controls.search.message_buttons.iter())
                .take(buttons.len())
                .map(|button| button.hwnd())
                .collect();
            moves.extend(window.message_moves(
                list,
                header,
                controls.search.message.hwnd(),
                text,
                &shown,
                false,
            ));
        }
        shown.push((controls.search.message.hwnd(), message.is_some()));
        let mut focus_lost = false;
        for (index, button) in controls.search.message_buttons.iter().enumerate() {
            let usable = buttons.get(index).is_some_and(|shown| shown.enabled);
            focus_lost |= !usable && focused == Some(button.hwnd());
            shown.push((button.hwnd(), index < buttons.len()));
        }
        // A hidden or disabled button would keep the focus but take no key.
        focus_lost
    }

    /// Fills the list's width with its columns when the width changed
    /// ([`ColumnSizing::resized`]).
    pub(super) fn fill_on_resize(&self, window: &MainWindow, width: Px) {
        let width = self.fill_width() + width - self.list.hwnd().screen_rect().width;
        let widths = (self.column_sizing.borrow_mut()).resized(width, || self.column_fits(window));
        if let Some(widths) = widths
            && widths
                .iter()
                .enumerate()
                .any(|(index, width)| self.list.column_width(index) != *width)
        {
            self.list.draw_once_after(|| self.set_column_widths(widths));
        }
    }

    /// The person ended a drag of the window's border or title bar
    /// ([`ColumnSizing::drag_ended`]).
    pub(super) fn end_sizing(&self, window: &MainWindow) {
        let width = self.fill_width();
        let widths =
            (self.column_sizing.borrow_mut()).drag_ended(width, || self.column_fits(window));
        if let Some(widths) = widths {
            let list = self.list;
            list.draw_once_after(|| self.set_column_widths(widths));
        }
    }

    /// Fills the list's width once the first rows on screen arrive
    /// ([`ColumnSizing::rows_arrived`]).
    pub(super) fn fill_around_first_rows(&self, window: &MainWindow) {
        let first = self.list.visible_rows().start;
        if window
            .model
            .borrow()
            .search_list()
            .results()
            .row(first)
            .is_none()
        {
            return;
        }
        let width = self.fill_width();
        let widths =
            (self.column_sizing.borrow_mut()).rows_arrived(width, || self.column_fits(window));
        if let Some(widths) = widths {
            // A list that draws itself paints every row it shows for each
            // width it sets, and its first paints shape every text afresh.
            let list = self.list;
            list.draw_once_after(|| self.set_column_widths(widths));
        }
    }

    /// The width the list's columns fill.
    pub(super) fn fill_width(&self) -> Px {
        self.list.fill_width()
    }

    /// Gives the columns `widths`, each only when it differs from the
    /// column's, because each width that the list takes draws it again.
    pub(super) fn set_column_widths(&self, widths: Vec<Px>) {
        let list = self.list;
        for (index, width) in widths.into_iter().enumerate() {
            if list.column_width(index) != width {
                list.set_column_width(index, width);
            }
        }
    }

    /// Fits the column at `index` to its header and the rows it shows, as
    /// a double-click on its divider and Fit column do; the others keep
    /// their widths.
    pub(super) fn fit_column(&self, window: &MainWindow, index: usize) {
        if let Some(fit) = self.fits_of(window, index..index + 1).first() {
            window
                .controls()
                .search
                .list
                .set_column_width(index, fit.widest);
        }
    }

    /// Every column's fit, as [`Self::fits_of`] measures it.
    pub(super) fn column_fits(&self, window: &MainWindow) -> Vec<ColumnFit> {
        self.fits_of(window, 0..window.model.borrow().columns().len())
    }

    /// The fit of each of the shown columns at `indexes`, from its header
    /// and the cells of the rows the list shows and one row above and below
    /// them, which are the rows the person reads and those a small scroll
    /// brings (DESIGN.md: Results list). A column without rows keeps its
    /// first width, except Size, whose samples cover later values too.
    pub(super) fn fits_of(&self, window: &MainWindow, indexes: Range<usize>) -> Vec<ColumnFit> {
        let created = window.created();
        let list = created.controls.search.list;
        let dpi = created.metrics.get().dpi;
        let shown = list.visible_rows();
        // The shown rows end with a part row, or with the row below them.
        let below = u64::from(list.shows_part_row());
        let rows = shown.start.saturating_sub(1)..(shown.end + below).min(list.count());
        let label = list.first_label_offset(rows.start).unwrap_or_default();
        // The model is borrowed while the texts are formatted, and the
        // measuring follows.
        let (columns, headers, cells) = {
            let model = window.model.borrow();
            let numbers = window.numbers.borrow();
            let strings = model.strings();
            let sizes = model.choices().size_format;
            let results = model.search_list().results();
            let columns = model.columns().get(indexes.clone()).unwrap_or_default();
            let items: Vec<_> = rows
                .filter_map(|row| Some((row, results.row(row)?)))
                .collect();
            let headers: Vec<_> = columns
                .iter()
                .map(|column| captions::header(strings, *column))
                .collect();
            let cells: Vec<Vec<(String, Vec<Range<usize>>)>> = columns
                .iter()
                .map(|&column| {
                    let mut cells: Vec<_> = items
                        .iter()
                        .map(|&(row, item)| {
                            (
                                crate::core::text::cell(
                                    strings,
                                    &*numbers,
                                    sizes,
                                    &window.dates.borrow(),
                                    column,
                                    item,
                                )
                                .full,
                                model.matched_in_cell(row, column),
                            )
                        })
                        .collect();
                    if column == Column::Size {
                        cells.extend(
                            sizes
                                .width_samples(strings, &*numbers)
                                .map(|text| (text, Vec::new())),
                        );
                    }
                    cells
                })
                .collect();
            (columns.to_vec(), headers, cells)
        };
        let fonts = created.fonts.borrow();
        let plain = TextMeter::new(&fonts.ui);
        let bold = TextMeter::new(&fonts.bold);
        headers
            .iter()
            .zip(cells)
            .zip(columns)
            .zip(indexes)
            .map(|(((header, cells), column), index)| {
                if cells.is_empty() {
                    return ColumnFit::first(column, dpi);
                }
                let first = index == 0;
                let label = if first { label } else { Px(0) };
                let texts = (cells.iter())
                    .filter(|(text, _)| !text.is_empty())
                    .map(|(text, parts)| matched_width(&plain, &bold, text, parts));
                ColumnFit::of(
                    plain.plain_width(header.compact()),
                    texts,
                    label,
                    first,
                    dpi,
                )
                .for_column(column)
            })
            .collect()
    }

    /// The arrow on the sorted column's header (DESIGN.md: Results list).
    pub(super) fn show_sort(&self, window: &MainWindow) {
        let (count, sorted) = {
            let model = window.model.borrow();
            let sort = model.sort();
            (
                model.columns().len(),
                model
                    .columns()
                    .iter()
                    .position(|c| c.row().sort_key == Some(sort.key))
                    .map(|index| (index, sort.dir == SortDir::Asc)),
            )
        };
        self.list.show_sort(count, sorted);
    }

    /// Reads the list's selection, if it changed since it was last read
    /// (DESIGN.md: The selection).
    pub(super) fn read_list_selection(&self, window: &MainWindow, hwnd: Hwnd) -> bool {
        if window.native_deselection_pending(List::Search) {
            return false;
        }
        if !self.selection_pending.replace(false) {
            return true;
        }
        let list = self.list;
        let count = list.selected_count();
        let accepted = if count > SELECTION_LIMIT {
            window
                .model
                .borrow_mut()
                .select_too_many(List::Search, count)
        } else {
            let Some(selected) = window.selected_rows(List::Search) else {
                self.selection_pending.set(true);
                return false;
            };
            window
                .model
                .borrow_mut()
                .set_selection(List::Search, selected)
        };
        window.apply(hwnd, accepted);
        true
    }

    pub(super) fn on_list_notify(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        notify: ListNotify,
    ) -> Option<LRESULT> {
        match notify {
            ListNotify::GetDispInfo(mut info) => {
                self.fill_cell(window, &mut info);
                Some(LRESULT(0))
            }
            ListNotify::CacheHint(rows) => {
                let fetch = window.model.borrow_mut().rows_needed(List::Search, rows);
                if let Some(fetch) = fetch {
                    window.start(fetch);
                }
                Some(LRESULT(0))
            }
            ListNotify::ColumnClick(index) => {
                let accepted = {
                    let mut model = window.model.borrow_mut();
                    let sort = model
                        .columns()
                        .get(index)
                        .and_then(|column| model.header_sort(*column));
                    sort.map(|sort| model.sort_by(sort))
                };
                if let Some(accepted) = accepted {
                    window.apply(hwnd, accepted);
                }
                Some(LRESULT(0))
            }
            ListNotify::CustomDraw(mut draw) => Some(CustomDraw::answer(match draw.stage() {
                DrawStage::Paint => DrawAnswer::EachRow,
                DrawStage::Row { row } => self.draw_row(window, &mut draw, row),
                DrawStage::Cell { row, column } => {
                    self.draw_cell(window, &draw, hwnd.dpi(), row, column)
                }
                DrawStage::RowPainted => {
                    draw.restore_font(&window.created().fonts.borrow().ui);
                    DrawAnswer::Default
                }
                DrawStage::Other => DrawAnswer::Default,
            })),
            ListNotify::SelectionChanged(change) => {
                if !window.selection_changed(List::Search, change) {
                    return None;
                }
                if !self.selection_pending.replace(true) {
                    hwnd.post(DEFERRED);
                }
                None
            }
            ListNotify::Activated => {
                self.enter_result(window, hwnd);
                None
            }
            ListNotify::BeginDrag => {
                window.drag_selected(hwnd);
                None
            }
            ListNotify::DividerDoubleClick(index) => {
                self.fit_column(window, index);
                None
            }
            ListNotify::ColumnDragEnded => {
                if !self.column_dragged.replace(true) {
                    hwnd.post(DEFERRED);
                }
                None
            }
            ListNotify::ColumnWidthChanged(index) => {
                let header = {
                    let model = window.model.borrow();
                    model
                        .columns()
                        .get(index)
                        .map(|column| captions::header(model.strings(), *column))
                };
                if let Some(header) = header {
                    self.list
                        .set_header(index, self.list.fitting_text(index, &header));
                }
                None
            }
            ListNotify::Focused
            | ListNotify::Unfocused
            | ListNotify::Clicked
            | ListNotify::FocusChanged
            | ListNotify::Other => None,
        }
    }

    /// Formats a cell when the list asks for it, so a redraw shows the
    /// current language and regional format (DESIGN.md: window rule 12). This
    /// block borrows the model while it formats, which makes no Win32 call; an
    /// icon not asked for yet is looked up after the borrow ends.
    pub(super) fn fill_cell(&self, window: &MainWindow, info: &mut DispInfo) {
        let (text, icon) = {
            let model = window.model.borrow();
            let numbers = window.numbers.borrow();
            let column = model.columns().get(info.column()).copied();
            // A taken row keeps its text, which `draw_cell` strikes through.
            match (column, model.search_list().results().row(info.row())) {
                (Some(column), Some(row)) => (
                    info.wants_text().then(|| {
                        crate::core::text::cell(
                            model.strings(),
                            &*numbers,
                            model.choices().size_format,
                            &window.dates.borrow(),
                            column,
                            row,
                        )
                    }),
                    // A gone row names no file, so it has no icon.
                    crate::core::text::path_part(column, &row.path)
                        .filter(|_| column == Column::Name && !row.is_gone())
                        .map(|name| (row.path[name].to_owned(), row.is_dir)),
                ),
                _ => (None, None),
            }
        };
        if info.wants_text() {
            let text = text.unwrap_or_default();
            info.set_text(self.list.fitting_text(info.column(), &text));
        }
        if info.wants_image() {
            info.set_image(icon.map(|(name, is_folder)| window.icons.index(&name, is_folder)));
        }
        if info.wants_state() {
            // Rows that changed under the drag no longer show it.
            let target = {
                let model = window.model.borrow();
                let drag_in = window.drag_in.borrow();
                drag_in
                    .as_ref()
                    .and_then(|drag| drag.target.as_ref())
                    .is_some_and(|target| {
                        target.row == info.row()
                            && model
                                .shown_row(List::Search, target.row)
                                .is_some_and(|row| row.path == target.folder.path)
                    })
            };
            info.set_drop_target(target);
        }
    }

    /// Has the list draw a row whose entry a delete or a move took struck
    /// through in grey (DESIGN.md: Results list), and asks for every other
    /// row's cells, which [`Self::draw_cell`] draws.
    pub(super) fn draw_row(
        &self,
        window: &MainWindow,
        draw: &mut CustomDraw,
        row: u64,
    ) -> DrawAnswer {
        let taken = !(window.model.borrow())
            .taken_rows(List::Search, std::slice::from_ref(&(row..row + 1)))
            .is_empty();
        if taken {
            draw.draw_row_struck(&window.created().fonts.borrow().struck);
            DrawAnswer::NewFont
        } else {
            DrawAnswer::EachCell
        }
    }

    /// Draws a Name or Path cell whose text the search matched, with the
    /// matched parts in bold (DESIGN.md: Results list); the list draws every
    /// other cell.
    pub(super) fn draw_cell(
        &self,
        window: &MainWindow,
        draw: &CustomDraw,
        dpi: Dpi,
        row: u64,
        column: usize,
    ) -> DrawAnswer {
        let cell = {
            let model = window.model.borrow();
            let Some(column) = model.columns().get(column).copied() else {
                return DrawAnswer::Default;
            };
            let bold = model.matched_in_cell(row, column);
            if bold.is_empty() {
                return DrawAnswer::Default;
            }
            let item = (model.shown_row(List::Search, row)).expect("a matched cell's row shows");
            let span = crate::core::text::path_part(column, &item.path)
                .expect("only Name and Path have matched parts");
            let icon = (column == Column::Name).then_some(item.is_dir);
            (item.path[span].to_owned(), bold, icon)
        };
        let (text, bold, icon) = cell;
        let icon = icon.map(|is_folder| window.icons.index(&text, is_folder));
        let created = window.created();
        let fonts = created.fonts.borrow();
        draw.draw_cell(
            &text,
            &bold,
            (&fonts.ui, &fonts.bold),
            created.images.zip(icon),
            layout::cell_text_inset(column == 0, dpi),
        );
        DrawAnswer::Drawn
    }

    /// Gives the selected row `Enter`, as `Enter` in the list does, or the
    /// first row when none is selected (DESIGN.md: window rule 8).
    pub(super) fn enter_first_or_selected(&self, window: &MainWindow, hwnd: Hwnd) {
        let list = self.list;
        if list.selected_count() == 0 && window.model.borrow().search_list().results().count() > 0 {
            list.select_only(Some(0));
            list.ensure_visible(0);
        }
        self.enter_result(window, hwnd);
    }

    /// Enter and double-click run the same Open command as the menus.
    pub(super) fn enter_result(&self, window: &MainWindow, hwnd: Hwnd) {
        window.run(hwnd, MenuItem::Run(CommandId::Open));
    }
}

impl Search {
    pub(super) fn on_control(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        event: super::ControlEvent,
    ) -> bool {
        if self.row.input.focused(event) {
            // The click places the caret after the focus arrives, so the
            // text is selected once the click is handled.
            let select = window.model.borrow().choices().select_on_click;
            if select && win::window::left_button_down() && !self.select_on_click.replace(true) {
                hwnd.post(DEFERRED);
            }
            return true;
        }
        let matching = || window.model.borrow().matching();
        let search_tool = self.row.tools.clicked(event);
        let tools = self.row.tools;
        let accepted = if self.row.input.changed(event) {
            let text = self.row.input.text();
            tools.set_enabled(ControlId::ClearSearch.id(), !text.is_empty());
            window
                .model
                .borrow_mut()
                .set_typed_search(List::Search, text)
        } else if search_tool == Some(ControlId::ClearSearch.id()) {
            // As Esc in the search box does; the box's change runs the search.
            self.row.input.hwnd().set_text("");
            self.row.input.hwnd().focus();
            return true;
        } else if self.row.match_mode.selection_changed(event) {
            let Some(mode) = self
                .row
                .match_mode
                .selected()
                .and_then(|i| MatchMode::ALL.get(i).copied())
            else {
                return true;
            };
            let choice = Matching { mode, ..matching() };
            window.model.borrow_mut().set_matching(choice)
        } else if search_tool == Some(ControlId::MatchCase.id()) {
            let choice = Matching {
                case: tools.is_checked(ControlId::MatchCase.id()),
                ..matching()
            };
            window.model.borrow_mut().set_matching(choice)
        } else if search_tool == Some(ControlId::MatchPath.id()) {
            let choice = Matching {
                path: tools.is_checked(ControlId::MatchPath.id()),
                ..matching()
            };
            window.model.borrow_mut().set_matching(choice)
        } else if let Some(index) = self
            .message_buttons
            .iter()
            .position(|button| button.clicked(event))
        {
            if index == 0
                && let Some(awaited) = window.slow_list_work(List::Search)
            {
                window.cancel_list_work(hwnd, List::Search, awaited);
                return true;
            }
            let button = window.model.borrow().list_buttons().get(index).cloned();
            match button {
                Some(ListButton::Fix(fix)) => window.run_fix(hwnd, fix),
                Some(ListButton::ClearFilters) => {
                    window.run(hwnd, MenuItem::Run(CommandId::ClearFilters));
                }
                Some(ListButton::SearchEverywhere) => {
                    let accepted = window.model.borrow_mut().search_everywhere();
                    window.apply(hwnd, accepted);
                }
                Some(ListButton::ChooseFolder) => {
                    window.controls().filters.choose_folder(window, hwnd)
                }
                None => {}
            }
            return true;
        } else {
            return false;
        };
        window.apply(hwnd, accepted);
        true
    }
}

pub(super) struct SearchRow {
    pub(super) input: Edit,
    pub(super) tools: Toolbar,
    pub(super) match_label: Label,
    pub(super) match_mode: DropDownList,
}

impl SearchRow {
    pub(super) fn create(parent: Hwnd) -> windows::core::Result<Self> {
        let input = Edit::create(parent, ControlId::Search.id())?;
        let clear = ControlId::ClearSearch.id();
        let tools = Toolbar::create(
            parent,
            ControlId::SearchTools.id(),
            &[
                Tool::IconOrText {
                    id: clear,
                    glyph: clear_glyph("button.clear"),
                },
                Tool::Separator,
                Tool::Check {
                    id: ControlId::MatchCase.id(),
                },
                Tool::Check {
                    id: ControlId::MatchPath.id(),
                },
                Tool::Separator,
            ],
        )?;
        Ok(Self {
            input,
            tools,
            match_label: Label::create(parent, ControlId::MatchLabel.id())?,
            match_mode: DropDownList::create(parent, ControlId::MatchMode.id())?,
        })
    }
}
