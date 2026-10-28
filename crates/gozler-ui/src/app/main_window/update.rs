use super::*;
use crate::core::model::may_apply;

struct PaneFocus {
    maximized: bool,
    was_maximized: bool,
    hidden: bool,
    inspector: bool,
    focused: Option<Hwnd>,
}

impl MainWindow {
    /// Applies native structure before values, then the ordered actions. A
    /// nested accepted update owns later presentation; [`Message::Deferred`]
    /// repairs interrupted areas and delivers still-current work afterward.
    pub(super) fn apply(&self, hwnd: Hwnd, accepted: Accepted) {
        if accepted.touched == Areas::NONE && accepted.effects.is_empty() {
            self.retire_model();
            return;
        }
        let revision = self.update_revision.get().wrapping_add(1);
        self.update_revision.set(revision);
        self.apply_at(hwnd, revision, accepted);
    }

    fn apply_at(&self, hwnd: Hwnd, revision: u64, accepted: Accepted) {
        self.retire_model();
        let mut later = Vec::new();
        let mut pane_focus = None;
        let mut layout_pending = false;
        for effect in accepted.effects {
            if !effect.structural() {
                later.push(effect);
                continue;
            }
            if !self.update_live() {
                return;
            }
            if revision != self.update_revision.get() {
                layout_pending |= matches!(effect, Effect::Layout);
                self.post_refresh(hwnd, accepted.touched);
                self.pending_effects.borrow_mut().push_back(effect);
                continue;
            }
            if matches!(effect, Effect::Layout) {
                pane_focus = Some(self.pane_focus());
            }
            self.apply_effect(hwnd, revision, effect);
        }
        if !self.update_live() {
            return;
        }
        self.refresh_areas(hwnd, revision, accepted.touched);
        for effect in later {
            if !self.update_live() {
                return;
            }
            if matches!(effect, Effect::PaneFocus | Effect::HomeFocus) {
                if layout_pending {
                    self.post_refresh(hwnd, accepted.touched);
                    self.pending_effects.borrow_mut().push_back(effect);
                } else {
                    self.reconcile_focus(pane_focus.take(), matches!(effect, Effect::HomeFocus));
                }
                continue;
            }
            if !may_apply(revision, self.update_revision.get(), &effect) {
                self.post_refresh(hwnd, accepted.touched);
                self.pending_effects.borrow_mut().push_back(effect);
                continue;
            }
            self.apply_effect(hwnd, revision, effect);
        }
    }

    pub(super) fn update_live(&self) -> bool {
        self.created().waker.while_open(|| {})
    }

    fn post_refresh(&self, hwnd: Hwnd, touched: Areas) {
        let post =
            self.pending_refresh.get() == Areas::NONE && self.pending_effects.borrow().is_empty();
        self.pending_refresh
            .set(self.pending_refresh.get() | touched);
        if post && hwnd.post(DEFERRED) {
            win::present::extend_to_deferred();
        }
    }

    pub(super) fn apply_pending(&self, hwnd: Hwnd) {
        let touched = self.pending_refresh.replace(Areas::NONE);
        let effects = self.pending_effects.take();
        if touched == Areas::NONE && effects.is_empty() {
            return;
        }
        let effects = effects
            .into_iter()
            .filter_map(|effect| {
                if !self.effect_is_current(&effect) {
                    return None;
                }
                Some(match effect {
                    Effect::SetCount { list, .. } => {
                        let count = self
                            .model
                            .borrow()
                            .list(list)
                            .map_or(0, |shown| shown.results().count());
                        Effect::SetCount { list, count }
                    }
                    Effect::Rows { list, rows, .. } => {
                        let count = self
                            .model
                            .borrow()
                            .list(list)
                            .map_or(0, |shown| shown.results().count());
                        Effect::Rows { list, rows, count }
                    }
                    effect => effect,
                })
            })
            .collect();
        self.apply(hwnd, Accepted { touched, effects });
    }

    fn effect_is_current(&self, effect: &Effect) -> bool {
        if let Effect::FirstRow { list, generation } = effect {
            let current = self
                .model
                .borrow()
                .list(*list)
                .is_some_and(|shown| shown.results().generation() == Some(*generation));
            return current && self.list_control(*list).selected_count() == 0;
        }
        if let Effect::Cancel(purpose) = effect {
            let current = self.created().work.borrow().requests.current(*purpose);
            return current
                .is_some_and(|generation| !self.model.borrow().awaits(*purpose, generation));
        }
        let model = self.model.borrow();
        match effect {
            Effect::Fetch(fetch) => model.awaits(fetch.purpose(), fetch.generation),
            Effect::ReadSelectionSoon(list) => model.list(*list).is_some_and(|shown| {
                matches!(shown.selection(), Selection::Reading { asked: None, .. })
            }),
            Effect::NameSearchSoon(list) => model
                .list(*list)
                .is_some_and(|shown| shown.results().searching()),
            Effect::RunFix { generation, .. } => model.awaits_fix(*generation),
            Effect::StopFix => !model.fix().running(),
            Effect::RunSetup(path) => model.fix().starting_setup() == Some(path.as_path()),
            Effect::ShowColumn { index, column } => model.columns().get(*index) == Some(column),
            Effect::HideColumn { column, .. } => !model.columns().contains(column),
            // The newer update owns the native selection and its row focus.
            Effect::ClearSelection(_) | Effect::Deselect { .. } | Effect::Age(_) => false,
            _ => true,
        }
    }

    fn refresh_areas(&self, hwnd: Hwnd, revision: u64, touched: Areas) {
        for area in [
            Areas::TITLE,
            Areas::FILTERS,
            Areas::LOCATIONS,
            Areas::SEARCH,
            Areas::LEFT_PANEL,
            Areas::RIGHT_PANEL,
            Areas::INSPECTOR,
            Areas::STATUS,
            Areas::KEY_BAR,
        ] {
            if !touched.contains(area) {
                continue;
            }
            if !self.update_live() {
                return;
            }
            if revision != self.update_revision.get() {
                self.post_refresh(hwnd, touched);
                return;
            }
            match area {
                Areas::TITLE => {
                    let title = captions::title(&self.model.borrow());
                    hwnd.set_text(&title);
                }
                Areas::FILTERS => {
                    self.controls().filters.update_filters(self);
                    self.controls().filters.refresh_commands(self);
                }
                Areas::LOCATIONS if !touched.contains(Areas::FILTERS) => {
                    self.controls().filters.update_locations(self)
                }
                Areas::SEARCH => {
                    self.controls().search.refresh_commands(self);
                    self.update_message();
                }
                Areas::LEFT_PANEL => self.controls().panels.show_panel(self, Side::Left),
                Areas::RIGHT_PANEL => self.controls().panels.show_panel(self, Side::Right),
                Areas::INSPECTOR => {
                    self.controls().inspector.refresh_commands(self);
                    self.controls().inspector.update_inspector(self);
                }
                Areas::STATUS => self.update_status(),
                Areas::KEY_BAR => self.controls().key_bar.update_key_bar(self),
                _ => {}
            }
        }
    }

    fn apply_effect(&self, hwnd: Hwnd, revision: u64, effect: Effect) {
        let created = self.created();
        let controls = &created.controls;
        match effect {
            Effect::Fetch(fetch) => {
                let current = self
                    .model
                    .borrow()
                    .awaits(fetch.purpose(), fetch.generation);
                if current {
                    self.start(fetch);
                }
            }
            Effect::SearchAgain(lists) => {
                let mut lists = lists.into_iter();
                while let Some(list) = lists.next() {
                    if !self.update_live() {
                        break;
                    }
                    if revision != self.update_revision.get() {
                        let remaining = std::iter::once(list).chain(lists).collect();
                        self.post_refresh(hwnd, Areas::NONE);
                        self.pending_effects
                            .borrow_mut()
                            .push_back(Effect::SearchAgain(remaining));
                        break;
                    }
                    let active =
                        list == List::Search || self.model.borrow().mode() == Mode::Commander;
                    if active {
                        let accepted = self.begin_search(hwnd, list);
                        self.apply_at(hwnd, revision, accepted);
                    }
                }
            }
            Effect::SearchSoon(list) => hwnd.set_timer(Timer::Search(list).id(), SEARCH_DELAY_MS),
            Effect::NameSearchSoon(list) => {
                hwnd.set_timer(Timer::Searching(list).id(), SEARCHING_NAMED_MS)
            }
            Effect::ReadSelectionSoon(list) => {
                let current = self.model.borrow().list(list).is_some_and(|shown| {
                    matches!(shown.selection(), Selection::Reading { asked: None, .. })
                });
                if current {
                    self.cancel_request(Purpose::Selection(list));
                    hwnd.set_timer(Timer::Selection(list).id(), SELECTION_DELAY_MS);
                }
            }
            Effect::Cancel(purpose) => self.cancel_request(purpose),
            Effect::AskLatestRelease => self.ask_latest_release(),
            Effect::RunFix {
                generation,
                work: input,
            } => {
                if !self.model.borrow().awaits_fix(generation) {
                    return;
                }
                let mut work = created.work.borrow_mut();
                work.fixes.start(generation, input);
                let retired = work.fixes.take_retired();
                work.retire(retired);
            }
            Effect::ConfirmSetup => self.defer(hwnd, ModalStep::ConfirmSetup),
            Effect::ExplainNoSetup => self.defer(hwnd, ModalStep::ExplainNoSetup),
            Effect::StopFix => {
                let mut work = created.work.borrow_mut();
                work.fixes.stop();
                let retired = work.fixes.take_retired();
                work.retire(retired);
            }
            Effect::CancelOperation(id) => self.act_operation(id, QueueAction::Cancel),
            Effect::RunSetup(setup) => self.run_setup(setup),
            Effect::ShowColumn { index, column } => {
                let header = captions::header(self.model.borrow().strings(), column);
                let width = controls
                    .search
                    .column_sizing
                    .borrow_mut()
                    .column_shown(column, hwnd.dpi());
                controls.search.list.insert_column(
                    index,
                    &ListColumn {
                        header: &header.full,
                        width,
                        right_aligned: column.row().right_aligned,
                    },
                );
                controls
                    .search
                    .list
                    .set_header(index, controls.search.list.fitting_text(index, &header));
            }
            Effect::HideColumn { index, column } => {
                let width = controls.search.list.column_width(index);
                controls
                    .search
                    .column_sizing
                    .borrow_mut()
                    .column_hidden(column, width, hwnd.dpi());
                controls.search.list.delete_column(index);
            }
            Effect::ClearSelection(list) => {
                self.list_control(list).select_only(None);
                self.seed_native_selection(list, Vec::new().into());
            }
            Effect::SetCount { list, count } => self.list_control(list).set_count(count),
            Effect::Rows { list, rows, count } => {
                let control = self.list_control(list);
                if control.count() == count {
                    control.redraw_rows(rows);
                } else {
                    control.set_count(count);
                }
            }
            Effect::Redraw { list, rows } => self.list_control(list).redraw_rows(rows),
            Effect::Deselect { list, rows } => self.show_list_taken(list, rows),
            Effect::PreparePanel(side) => self.controls().panels.show_panel_kind(self, side),
            Effect::Layout => self.layout(hwnd),
            Effect::FirstRow { list, generation } => {
                let current = self
                    .model
                    .borrow()
                    .list(list)
                    .is_some_and(|shown| shown.results().generation() == Some(generation));
                let control = self.list_control(list);
                if current && control.count() > 0 && control.selected_count() == 0 {
                    match list {
                        List::Search => control.ensure_visible(0),
                        List::Panel(_) => control.set_focused(Some(0)),
                    }
                }
            }
            Effect::ReleaseOperations => self.release_operations(),
            Effect::FetchVisible(list) => self.fetch_visible(list),
            Effect::FillFirstRows(list) => {
                if list == List::Search {
                    controls.search.fill_around_first_rows(self);
                }
            }
            Effect::EnterFirst(ask) => self.defer(hwnd, ModalStep::EnterFirst(ask)),
            Effect::ReadMemory => {
                let accepted = self.read_app_memory();
                self.apply_at(hwnd, revision, accepted);
            }
            Effect::PostSettings => self.settings.post_settings_reply(),
            Effect::TotalCommander(found) => {
                self.settings.total_commander.replace(Some(found));
                self.settings.post_settings_reply();
            }
            Effect::FolderSizes(side) => self.controls().panels.show_panel_sizes(self, side),
            Effect::ExportFailed { path, error } => {
                self.defer(hwnd, ModalStep::ExportFailed { path, error })
            }
            Effect::Menu => self.replace_menu_bar(hwnd),
            Effect::Tray => self.tray.update_tray(self, hwnd),
            Effect::SwitchPanels => {
                let mode = self.model.borrow().mode();
                let accepted = match mode {
                    Mode::Commander => self.controls().panels.start_panels(self, hwnd),
                    Mode::Search => {
                        // The entry the person left in Search is in view
                        // again (DESIGN.md: Commander mode and Search).
                        let list = self.controls().search.list;
                        if let Some(row) = list.focused().or_else(|| list.next_selected(None)) {
                            list.ensure_visible(row);
                        }
                        self.controls().panels.stop_panels(self, hwnd)
                    }
                };
                self.apply_at(hwnd, revision, accepted);
            }
            Effect::HomeFocus | Effect::PaneFocus => {}
            Effect::InspectAge => {
                let accepted = self.model.borrow_mut().inspect_age();
                self.apply_at(hwnd, revision, accepted);
            }
            Effect::Age(timer) => match timer {
                crate::core::model::AgeTimer::Keep => {}
                crate::core::model::AgeTimer::Restart => {
                    hwnd.set_timer(Timer::Age.id(), AGE_REFRESH_MS)
                }
                crate::core::model::AgeTimer::Stop => hwnd.kill_timer(Timer::Age.id()),
            },
            Effect::Polling => self.update_poll(hwnd),
            Effect::NoteSavedState => {
                let accepted = self.model.borrow_mut().observe_saved_state();
                self.apply_at(hwnd, revision, accepted);
            }
            Effect::SaveSoon => hwnd.set_timer(Timer::Save.id(), SAVE_DELAY_MS),
            Effect::ListPasses => self.start_list_passes(),
        }
    }

    fn pane_focus(&self) -> PaneFocus {
        let panes = self.model.borrow().panes();
        let maximized = panes.inspector
            && panes.maximized
            && panes.content == crate::core::layout::InspectorContent::Transfers;
        let was_maximized = self
            .created()
            .layout
            .borrow()
            .as_ref()
            .and_then(Layout::inspector)
            .is_some_and(|placed| placed.splitter.height == Px(0));
        let focused = Hwnd::focused();
        let hidden_focus = focused.is_some_and(|focused| {
            !focused.is_visible()
                || (!panes.filters && self.controls().filters.contains(focused))
                || ((!panes.inspector
                    || panes.content != crate::core::layout::InspectorContent::Selection)
                    && self.controls().inspector.contains(focused))
                || ((!panes.inspector
                    || panes.content != crate::core::layout::InspectorContent::Transfers)
                    && self
                        .file_operations
                        .get()
                        .is_some_and(|ops| ops.contains(focused)))
        });

        PaneFocus {
            maximized,
            was_maximized,
            hidden: hidden_focus,
            inspector: panes.inspector,
            focused,
        }
    }

    fn reconcile_focus(&self, before: Option<PaneFocus>, home: bool) {
        let mut current = self.pane_focus();
        let visible = current.focused.is_some_and(Hwnd::is_visible);
        let unchanged = before
            .as_ref()
            .is_some_and(|before| before.focused == current.focused);
        // Unmaximizing restores saved focus when focus has not moved; a newer
        // visible focus belongs to the person.
        let unmaximized =
            before.is_some_and(|before| before.was_maximized && !current.maximized && unchanged);
        if visible && !unmaximized && !(home && unchanged) {
            return;
        }
        if home && !current.maximized {
            self.home().focus();
        } else {
            current.hidden = !visible;
            current.was_maximized = unmaximized;
            self.restore_pane_focus(current);
        }
    }

    fn restore_pane_focus(&self, focus: PaneFocus) {
        if focus.maximized {
            if Hwnd::focused().is_none_or(|focus| {
                !self
                    .file_operations
                    .get()
                    .is_some_and(|ops| ops.contains(focus))
            }) && let Some(operations) = self.file_operations.get()
            {
                operations.focus();
            }
        } else if !focus.inspector || focus.was_maximized || focus.hidden {
            let previous = self.controls().inspector.focus.take();
            if focus.hidden || focus.was_maximized {
                previous
                    .filter(|focus| focus.is_visible())
                    .unwrap_or_else(|| self.home())
                    .focus();
            }
        }
    }
}
