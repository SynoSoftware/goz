use super::{
    AttributeChoice, Button, Cell, CheckBox, CommandId, ControlEvent, ControlId, CustomDate,
    CustomSize, DateModified, DialogEnd, DialogId, DropDownList, FilterCaptions, FilterLayout,
    Filters, Glyph, Hwnd, IconList, InfoTip, Kind, LRESULT, Label, Layout, ListClosing, MainWindow,
    MenuItem, Numbers, Px, RangeItem, Rect, ScrollRequest, SizeFilter, Tool, Toolbar, captions,
    clear_glyph, date_items, size_items, win,
};

/// The filter pane's header, its window and its controls, in Tab order, and
/// Show filters. Each label comes right before its control, so its access
/// key focuses the control (DESIGN.md: Filter pane).
pub(super) struct FilterPane {
    pub(super) scroll: Cell<Px>,
    pub(super) range_list: Cell<RangeList>,
    /// In the search row while the pane is hidden.
    pub(super) show: Toolbar,
    /// The header, in the main window above the pane's window, so it stays
    /// while the pane scrolls: the title, then Clear and Hide filters.
    pub(super) title: Label,
    pub(super) tools: Toolbar,
    pub(super) pane: Hwnd,
    pub(super) kind_label: Label,
    pub(super) kind: IconList,
    pub(super) location_label: Label,
    pub(super) location: IconList,
    pub(super) choose_folder: Button,
    pub(super) only_directly: CheckBox,
    pub(super) date_label: Label,
    pub(super) date: DropDownList,
    pub(super) size_label: Label,
    pub(super) size: DropDownList,
    pub(super) attributes_label: Label,
    /// Each attribute's label and list, in the order of
    /// [`crate::core::query::ATTRIBUTES`].
    pub(super) attributes: Vec<(Label, DropDownList)>,
}

/// The controls of each attribute's label and list.
const ATTRIBUTE_CONTROLS: [(ControlId, ControlId); 6] = [
    (ControlId::HiddenLabel, ControlId::Hidden),
    (ControlId::SystemLabel, ControlId::System),
    (ControlId::ReadOnlyLabel, ControlId::ReadOnly),
    (ControlId::LinkLabel, ControlId::Link),
    (ControlId::CompressedLabel, ControlId::Compressed),
    (ControlId::EncryptedLabel, ControlId::Encrypted),
];

/// The Date modified and Size lists, whose Custom… opens a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RangeFilter {
    Date,
    Size,
}

impl FilterPane {
    pub(super) fn create(parent: Hwnd, show: Toolbar) -> windows::core::Result<FilterPane> {
        let title = Label::create_heading(parent, ControlId::FilterTitle.id())?;
        let tools = Toolbar::create(
            parent,
            ControlId::FilterTools.id(),
            &[
                Tool::IconOrText {
                    id: ControlId::ClearFilters.id(),
                    glyph: clear_glyph("filter.clear"),
                },
                Tool::Icon {
                    id: ControlId::HideFilters.id(),
                    glyph: Glyph::OpenPane,
                },
            ],
        )?;
        let pane = win::pane::create(parent, ControlId::FilterPane.id())?;
        let label = |id: ControlId| Label::create(pane, id.id());
        let drop_down = |id: ControlId| DropDownList::create(pane, id.id());
        let mut filters = FilterPane {
            scroll: Cell::new(Px(0)),
            range_list: Cell::new(RangeList::Idle),
            show,
            title,
            tools,
            pane,
            kind_label: label(ControlId::KindLabel)?,
            kind: IconList::create(pane, ControlId::Kind.id())?,
            location_label: label(ControlId::LocationLabel)?,
            location: {
                let list = IconList::create(pane, ControlId::Location.id())?;
                list.show_info_tips();
                list
            },
            choose_folder: Button::create(pane, ControlId::ChooseFolder.id())?,
            only_directly: CheckBox::create(pane, ControlId::OnlyDirectly.id())?,
            date_label: label(ControlId::DateLabel)?,
            date: drop_down(ControlId::Date)?,
            size_label: label(ControlId::SizeLabel)?,
            size: drop_down(ControlId::Size)?,
            attributes_label: label(ControlId::AttributesLabel)?,
            attributes: Vec::new(),
        };
        for (name, choice) in ATTRIBUTE_CONTROLS {
            filters.attributes.push((label(name)?, drop_down(choice)?));
        }
        Ok(filters)
    }

    /// Whether `control` is the pane's; the header holds nothing that takes
    /// the focus.
    pub(super) fn contains(&self, control: Hwnd) -> bool {
        self.pane.is_parent_of(control)
    }

    /// Every control in the pane.
    pub(super) fn controls(&self) -> Vec<Hwnd> {
        self.placed_with(None)
            .into_iter()
            .map(|(control, ..)| control)
            .collect()
    }

    /// Each control, its rectangle in `layout` before the pane scrolls, and
    /// whether it is a drop-down list, whose rectangle also holds its open
    /// list.
    pub(super) fn placed(&self, layout: &FilterLayout) -> Vec<(Hwnd, Rect, bool)> {
        self.placed_with(Some(layout))
            .into_iter()
            .map(|(control, rect, drop_down)| (control, rect.unwrap_or_default(), drop_down))
            .collect()
    }

    pub(super) fn placed_with(
        &self,
        layout: Option<&FilterLayout>,
    ) -> Vec<(Hwnd, Option<Rect>, bool)> {
        let at = |rect: fn(&FilterLayout) -> Rect| layout.map(rect);
        let mut placed = vec![
            (self.kind_label.hwnd(), at(|l| l.kind_label), false),
            (self.kind.hwnd(), at(|l| l.kind), false),
            (self.location_label.hwnd(), at(|l| l.location_label), false),
            (self.location.hwnd(), at(|l| l.location), false),
            (self.choose_folder.hwnd(), at(|l| l.choose_folder), false),
            (self.only_directly.hwnd(), at(|l| l.only_directly), false),
            (self.date_label.hwnd(), at(|l| l.date_label), false),
            (self.date.hwnd(), at(|l| l.date), true),
            (self.size_label.hwnd(), at(|l| l.size_label), false),
            (self.size.hwnd(), at(|l| l.size), true),
            (
                self.attributes_label.hwnd(),
                at(|l| l.attributes_label),
                false,
            ),
        ];
        for (index, (name, choice)) in self.attributes.iter().enumerate() {
            let rects = layout.map(|l| l.attributes[index]);
            placed.push((name.hwnd(), rects.map(|r| r.0), false));
            placed.push((choice.hwnd(), rects.map(|r| r.1), true));
        }
        placed
    }

    /// Sets the labels and the buttons' captions. An icon's caption is the
    /// name that screen readers say.
    pub(super) fn set_captions(&self, captions: &FilterCaptions) {
        self.title.hwnd().set_text(&captions.title);
        self.tools
            .set_text(ControlId::ClearFilters.id(), &captions.clear);
        self.tools
            .set_text(ControlId::HideFilters.id(), &captions.hide);
        self.show
            .set_text(ControlId::ShowFilters.id(), &captions.show);
        self.kind_label.hwnd().set_text(&captions.kind);
        self.location_label.hwnd().set_text(&captions.location);
        self.choose_folder.hwnd().set_text(&captions.choose_folder);
        self.only_directly.hwnd().set_text(&captions.only_directly);
        self.date_label.hwnd().set_text(&captions.date);
        self.size_label.hwnd().set_text(&captions.size);
        self.attributes_label.hwnd().set_text(&captions.attributes);
        for ((name, _), text) in self.attributes.iter().zip(&captions.attribute_names) {
            name.hwnd().set_text(text);
        }
    }

    /// The list `range` stands for.
    pub(super) fn range(&self, range: RangeFilter) -> DropDownList {
        match range {
            RangeFilter::Date => self.date,
            RangeFilter::Size => self.size,
        }
    }

    /// The range list that `control` is, if it is one.
    pub(super) fn range_of(&self, control: Hwnd) -> Option<RangeFilter> {
        [RangeFilter::Date, RangeFilter::Size]
            .into_iter()
            .find(|range| self.range(*range).hwnd() == control)
    }
}

#[derive(Clone, Copy)]
pub(super) enum RangeList {
    Idle,
    Closing(RangeFilter, ListClosing),
    Closed(RangeFilter),
}

impl FilterPane {
    /// Gives the filter pane's lists their items and selects the model's
    /// choices.
    pub(super) fn show_filters(&self, window: &MainWindow, captions: &FilterCaptions) {
        let pane = self;
        let (positions, located, only_directly) = {
            let model = window.model.borrow();
            let filters = model.filters();
            (
                filters.positions(&model.location_items()),
                filters.location.is_some(),
                filters.only_directly,
            )
        };
        // Each item shows the icon of its place in `Kind::ALL`.
        let kinds: Vec<(&str, usize)> =
            captions.kinds.iter().map(String::as_str).zip(0..).collect();
        pane.kind.set_items(&kinds, positions.kind);
        pane.location
            .set_paths(&captions.locations, positions.location);
        // Only directly in a folder needs a folder.
        pane.only_directly.hwnd().set_enabled(located);
        pane.only_directly.set_checked(only_directly);
        pane.date.set_items(&captions.dates, positions.date);
        pane.size.set_items(&captions.sizes, positions.size);
        for ((_, list), position) in pane.attributes.iter().zip(positions.attributes) {
            list.set_items(&captions.attribute_choices, position);
        }
    }

    pub(super) fn update_filters(&self, window: &MainWindow) {
        let captions = FilterCaptions::new(&window.model.borrow(), &*window.numbers.borrow());
        self.show_filters(window, &captions);
    }

    /// The Location list after a status reply, which can name other
    /// volumes; the other lists keep what the person moved to, such as
    /// Custom… with the arrow keys.
    pub(super) fn update_locations(&self, window: &MainWindow) {
        let (locations, position) = {
            let model = window.model.borrow();
            let items = model.location_items();
            (
                captions::location_items(&model),
                model.filters().positions(&items).location,
            )
        };
        window
            .controls()
            .filters
            .location
            .set_paths(&locations, position);
    }

    /// Places the pane's controls for the scroll position nearest
    /// `position` that `filters` allows; [`MainWindow::layout`] places the pane.
    pub(super) fn place_filters(&self, filters: &FilterLayout, position: Px) {
        let pane = self;
        let position = filters.scrolled(position, ScrollRequest::To(position));
        self.scroll.set(position);
        let moves: Vec<(Hwnd, Rect)> = pane
            .placed(filters)
            .into_iter()
            .map(|(control, rect, drop_down)| {
                // A drop-down list's height includes its open list: twelve
                // times the box's height fits Size's ten items, the longest
                // list (Date has eight).
                let height = if drop_down {
                    Px(rect.height.0 * 12)
                } else {
                    rect.height
                };
                let rect = Rect {
                    y: rect.y - position,
                    height,
                    ..rect
                };
                (control, rect)
            })
            .collect();
        win::move_children(&moves);
        pane.pane
            .set_vertical_scroll(filters.content_height, filters.pane.height, position);
    }

    /// Scrolls the pane to the position `to` gives for its layout and its
    /// current position.
    pub(super) fn scroll_filters(
        &self,
        window: &MainWindow,
        to: impl FnOnce(&FilterLayout, Px) -> Px,
    ) {
        let filters = window
            .created()
            .layout
            .borrow()
            .as_ref()
            .and_then(Layout::search)
            .and_then(|search| search.filters.clone());
        let Some(filters) = filters else { return };
        let position = to(&filters, self.scroll.get());
        if position != self.scroll.get() {
            self.place_filters(&filters, position);
        }
    }

    /// Scrolls the pane so that `control`, which a key focused, shows.
    pub(super) fn reveal(&self, window: &MainWindow, control: Hwnd) {
        let filters = window
            .created()
            .layout
            .borrow()
            .as_ref()
            .and_then(Layout::search)
            .and_then(|search| search.filters.clone());
        let Some(filters) = filters else { return };
        let placed = self.placed(&filters);
        if let Some((_, rect, _)) = placed.into_iter().find(|(c, ..)| *c == control) {
            self.scroll_filters(window, |filters, position| {
                filters.revealing(position, rect)
            });
        }
    }

    /// Changes the filters to what `change` makes of them.
    pub(super) fn change_filters(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        change: impl FnOnce(&mut Filters),
    ) {
        let mut filters = window.model.borrow().filters().clone();
        change(&mut filters);
        let accepted = window.model.borrow_mut().set_filters(filters);
        window.apply(hwnd, accepted);
    }

    /// Asks for a folder with the Windows folder picker and sets Location
    /// to it; Cancel keeps the Location (DESIGN.md: Filter pane).
    pub(super) fn choose_folder(&self, window: &MainWindow, hwnd: Hwnd) {
        let start = window.model.borrow().location().map(str::to_owned);
        let Some(folder) = win::shell::pick_folder(hwnd, start.as_deref()) else {
            return;
        };
        let accepted = window.model.borrow_mut().choose_folder(folder);
        window.apply(hwnd, accepted);
    }

    /// Opens the custom range dialog of `range` when its list shows
    /// Custom…; OK sets the range, and Cancel shows the previous choice
    /// again (DESIGN.md: Filter pane).
    pub(super) fn choose_custom(&self, window: &MainWindow, hwnd: Hwnd, range: RangeFilter) {
        let custom = {
            let model = window.model.borrow();
            let filters = model.filters();
            match range {
                RangeFilter::Date => date_items(filters.date).len(),
                RangeFilter::Size => size_items(filters.size).len(),
            }
        };
        let list = self.range(range);
        if list.selected() != Some(custom - 1) {
            return;
        }
        match range {
            RangeFilter::Date => {
                let dialog = {
                    let model = window.model.borrow();
                    let shown = match model.filters().date {
                        DateModified::Custom(range) => Some(range),
                        _ => None,
                    };
                    CustomDate::new(model.strings(), shown)
                };
                if window.open_dialog(hwnd, DialogId::CustomDate, &dialog) == Some(DialogEnd::Ok)
                    && let Some(chosen) = dialog.chosen()
                {
                    self.change_filters(window, hwnd, |f| f.date = DateModified::Custom(chosen));
                }
            }
            RangeFilter::Size => {
                let dialog = {
                    let model = window.model.borrow();
                    let shown = match model.filters().size {
                        SizeFilter::Custom(range) => Some(range),
                        _ => None,
                    };
                    let numbers = window.numbers.borrow();
                    CustomSize::new(model.strings(), numbers.decimal_separator(), shown)
                };
                if window.open_dialog(hwnd, DialogId::CustomSize, &dialog) == Some(DialogEnd::Ok)
                    && let Some(chosen) = dialog.chosen()
                {
                    self.change_filters(window, hwnd, |f| f.size = SizeFilter::Custom(chosen));
                }
            }
        }
        // After Cancel, the list shows Custom… and the filter is the one
        // before it.
        self.update_filters(window);
    }

    /// The Kind list selected another item. A list view also tells of a
    /// selection that the window made to show the model's kind, which
    /// [`crate::core::model::Model::set_filters`] takes as no change.
    pub(super) fn on_kind_selected(&self, window: &MainWindow, hwnd: Hwnd) {
        let selected = self.kind.selected();
        if let Some(kind) = selected.and_then(|index| Kind::ALL.get(index).copied()) {
            self.change_filters(window, hwnd, |f| f.kind = kind);
        }
    }

    /// The Location list selected another item, which the person or the
    /// window chose, as [`Self::on_kind_selected`] says.
    pub(super) fn on_location_selected(&self, window: &MainWindow, hwnd: Hwnd) {
        let location = {
            let model = window.model.borrow();
            let items = model.location_items();
            window
                .controls()
                .filters
                .location
                .selected()
                .and_then(|i| items.get(i))
                .map(|item| item.folder().map(str::to_owned))
                .filter(|location| *location != model.filters().location)
        };
        if let Some(location) = location {
            self.change_filters(window, hwnd, |f| f.location = location);
        }
    }

    /// Answers the Location list's tooltip request with the row's whole
    /// path when the row shows it shortened, and with no tip otherwise.
    pub(super) fn location_tip(&self, window: &MainWindow, tip: InfoTip) -> LRESULT {
        let list = self.location;
        let whole = tip.row().and_then(|row| {
            let path = captions::location_items(&window.model.borrow())
                .into_iter()
                .nth(row)?;
            (path != list.text(row)).then_some(path)
        });
        tip.set(whole.as_deref().unwrap_or_default())
    }

    /// A notification of a control in the filter pane.
    pub(super) fn on_filter(&self, window: &MainWindow, hwnd: Hwnd, event: ControlEvent) {
        let pane = self;
        let tool = pane.tools.clicked(event).or(pane.show.clicked(event));
        if tool == Some(ControlId::ClearFilters.id()) {
            window.run(hwnd, MenuItem::Run(CommandId::ClearFilters));
        } else if tool.is_some() {
            // Hide filters, or Show filters in the search row.
            window.run(hwnd, MenuItem::Run(CommandId::Filters));
        } else if pane.choose_folder.clicked(event) {
            self.choose_folder(window, hwnd);
        } else if pane.only_directly.clicked(event) {
            let checked = pane.only_directly.is_checked();
            self.change_filters(window, hwnd, |f| f.only_directly = checked);
        } else if pane.date.selection_changed(event) {
            let date = window.model.borrow().filters().date;
            // Custom… opens its dialog only when the list closes on it, so
            // the arrow keys pass it by and keep the previous filter.
            if let Some(RangeItem::Choice(date)) = pane
                .date
                .selected()
                .and_then(|i| date_items(date).get(i).copied())
            {
                self.change_filters(window, hwnd, |f| f.date = date);
            }
        } else if pane.size.selection_changed(event) {
            let size = window.model.borrow().filters().size;
            if let Some(RangeItem::Choice(size)) = pane
                .size
                .selected()
                .and_then(|i| size_items(size).get(i).copied())
            {
                self.change_filters(window, hwnd, |f| f.size = size);
            }
        } else if let Some(range) = pane.range_of(event.sender()) {
            let list = pane.range(range);
            if let Some(closing) = list.closing(event) {
                self.range_list.set(RangeList::Closing(range, closing));
            } else if list.closed(event) {
                let closing = self.range_list.replace(RangeList::Idle);
                if matches!(closing, RangeList::Closing(each, ListClosing::Chosen) if each == range)
                {
                    self.range_list.set(RangeList::Closed(range));
                    hwnd.post_deferred();
                }
            }
        } else if let Some(index) = pane
            .attributes
            .iter()
            .position(|(_, list)| list.selection_changed(event))
        {
            let choice = pane.attributes[index]
                .1
                .selected()
                .and_then(|i| AttributeChoice::ALL.get(i).copied());
            if let Some(choice) = choice {
                self.change_filters(window, hwnd, |f| f.attributes[index] = choice);
            }
        }
    }
}

impl FilterPane {
    pub(super) fn refresh_commands(&self, window: &MainWindow) {
        let enabled = {
            let model = window.model.borrow();
            let state = super::State {
                model: &model,
                operations_ready: window.file_operations.get().is_some(),
                focus: super::Focus::Other,
                list: model.active_list(),
            };
            (CommandId::ClearFilters.row().enabled)(&state)
        };
        self.tools
            .set_enabled(ControlId::ClearFilters.id(), enabled);
    }
}
