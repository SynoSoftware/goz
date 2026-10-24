//! The main window and its message handlers.
//!
//! Window rule 1 (DESIGN.md): a handler changes the model in a statement of its
//! own, or binds a borrow with `let` only in a block that makes no Win32 call,
//! and calls Win32 after the borrow ends, because many calls send messages back
//! to this window before they return.

use super::dialogs::{About, CustomDate, CustomSize, SearchSyntax, Transfer, TransferStart};
use super::file_operations::{FileOperations, FilesSource};
use super::icons::Icons;
use super::include::{Elevated, NotSaved};
use super::settings::SettingsDialog;
use super::status_bar::{Entry as StatusEntry, Slot as StatusSlot, StatusBar};
use super::viewer::Viewers;
use super::work::{
    CopyFailure, CopyNotice, OpenTarget, Opened, PathAction, PathPurpose, PreparedPaths,
    QueueReady, RegistryAnswer, ReleaseAnswer, ReleasePurpose, ShellPurpose, ShellTarget,
};
use crate::core::captions::{
    self, ButtonIcon, DialogCaptions, DialogItem, FilterCaptions, IndexCaptions, Inspected,
    InspectorCaptions, JobCaption, ListCaption, MODIFIED_ROW, StatusCaptions, StatusWidest,
    ValuePart, WindowCaptions,
};
use crate::core::clipboard::ClipboardFormat;
use crate::core::commands::{
    self, CommandButton, CommandId, Focus, HIDE_DETAILS, INSPECTOR_BUTTONS, INSPECTOR_COPY_FOLDER,
    INSPECTOR_COPY_NAME, INSPECTOR_COPY_PATH, KeyAction, Keystroke, MenuItem, SHOW_DETAILS, State,
    TabStop,
};
use crate::core::dates::{DateFormat, Dates};
use crate::core::drop_target::{Dragged, DropEffect, DropKeys, DropMenu, SourceEffects};
use crate::core::fetch::{
    ChunkAsk, DaemonFailure, Fetch, List, LocalService, Purpose, WriteFailure,
};
use crate::core::file_operations::{QueueAction, delete_question, exit_question};
use crate::core::fixer::{FixEvent, SetupEvent, SetupResult};
use crate::core::glyph::Glyph;
use crate::core::ids::{ControlId, DialogId};
use crate::core::include::{
    IncludeDrive, SummaryRow, connected_drives, connected_volumes, include_drives, preview,
    summary_rows,
};
use crate::core::layout::column_sizing::{self, ColumnFit, ColumnSizing};
use crate::core::layout::{
    self, Body, CaptionWidths, ControlSizes, Dpi, FilterLayout, InspectorLayout, Layout,
    MessageArea, Px, Rect, ScrollRequest, Size, StatusLayout, ToolHeights,
};
use crate::core::model::{
    Accepted, Change, Column, ExportError, ExportFormat, Job, Model, SaveScope, ScopeCheck,
    SearchBoxEnter, Then,
};
use crate::core::open::{Commands, Entries, EntryAction, Selected};
use crate::core::operations::{
    OperationDialog, OperationProgress, OperationSteps, PreparedFailure, QueueStep, Reporting,
    SourceStep,
};
use crate::core::panel::Side;
use crate::core::query::{
    AttributeChoice, DateModified, Filters, Kind, MatchMode, Matching, RangeItem, SizeFilter,
    date_items, size_items,
};
use crate::core::results::{Fix, ListButton};
use crate::core::search_list::Deselect;
use crate::core::selection::{DRAG_LIMIT, RowSelection, SELECTION_LIMIT, Selection, wide_path};
use crate::core::settings::{Choices, Mode, ResultEnter, SettingsValues};
use crate::core::shortcuts::{CommandShortcuts, ShowShortcut};
use crate::core::text::{LanguageChoice, Numbers, Strings, without_access_key};
use crate::core::work::{Id, Retirement, Waiting};
use crate::win::clipboard;
use crate::win::controls::{
    Button, CheckBox, Divider, DropDownList, Edit, FileIconLabel, IconList, Label, ListClosing,
    ListColumn, ListView, ScrollEdge, Tooltip,
};
use crate::win::dialog::{DialogEnd, DialogProc};
use crate::win::drop_target::{self, DropData, DropHandler, DropRegistration, DropSource};
use crate::win::file_operations::QUEUE_CHANGED;
use crate::win::gdi::{Font, ImageList, TextMeter};
use crate::win::locale::WindowsNumbers;
use crate::win::menu::Accelerators;
use crate::win::notify::{
    Command, ControlEvent, CustomDraw, DispInfo, DrawAnswer, DrawStage, InfoTip, ListNotify,
};
use crate::win::shell::{self, FileType};
use crate::win::shortcut::{self, RegisterError, Registration};
use crate::win::toolbar::{Tool, Toolbar};
use crate::win::tray::TrayEvent;
use crate::win::wake::OpenWaker;
use crate::win::window::{DEFERRED, Message, WindowProc};
use crate::win::{self, Hwnd};
use gozler_core::include::IndexSettings;
use gozler_core::proto::ResultItem;
use gozler_core::query::QueryStart;
use gozler_core::types::{SortDir, SortSpec};
use gozler_fileops::{Destination, OperationId, Request, Taken};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io;
use std::ops::Range;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use windows::Win32::Foundation::LRESULT;

mod drops;
mod panels;
mod selection;
mod synchronize;

use drops::DragIn;
use panels::{KEY_BAR, KEY_TIPS, PanelControls};

mod timer {
    use crate::core::fetch::List;

    /// Search as you type waits this long after the last keystroke.
    pub(super) const SEARCH: usize = 1;
    pub(super) const STATUS: usize = 2;
    /// The selection is read this long after it last changed.
    pub(super) const SELECTION: usize = 3;
    /// The inspector's age of the modification time, while it shows one.
    pub(super) const AGE: usize = 4;
    /// Scrolls the list while dragged files rest at its top or bottom edge.
    pub(super) const DROP_SCROLL: usize = 6;
    /// `ui.json` is saved this long after the window's saved state last
    /// changed, so a change survives a window that does not exit, and a
    /// splitter drag saves once.
    pub(super) const SAVE: usize = 7;
    /// Each panel's folder sizes and free space are read again this long
    /// after the last change in its folder, by side.
    pub(super) const PANEL_SETTLE: [usize; 2] = [8, 9];
    /// Each panel asks for its folder sizes once more after an operation in
    /// it ended, by side.
    pub(super) const PANEL_AGAIN: [usize; 2] = [10, 11];
    /// Each search panel's search waits this long after the last keystroke,
    /// by side.
    pub(super) const PANEL_SEARCH: [usize; 2] = [12, 13];
    /// Each search panel's selection is read this long after it last changed,
    /// by side.
    pub(super) const PANEL_SELECTION: [usize; 2] = [14, 15];
    /// Reads a running file operation's counts, which change without a
    /// wake.
    pub(super) const OPERATIONS_POLL: usize = 16;
    /// Names Search's search as running in the status bar once it has run
    /// this long.
    pub(super) const SEARCHING: usize = 18;
    /// The same for each search panel's search, by side.
    pub(super) const PANEL_SEARCHING: [usize; 2] = [19, 20];
    pub(super) const DATES: usize = 21;
    pub(super) const CLIPBOARD: usize = 22;
    pub(super) const WORK: usize = 23;

    /// The timer of `list`'s search.
    pub(super) fn search(list: List) -> usize {
        match list {
            List::Search => SEARCH,
            List::Panel(side) => PANEL_SEARCH[side.index()],
        }
    }

    /// The timer that names `list`'s search as running.
    pub(super) fn searching(list: List) -> usize {
        match list {
            List::Search => SEARCHING,
            List::Panel(side) => PANEL_SEARCHING[side.index()],
        }
    }

    /// The timer that reads `list`'s selection.
    pub(super) fn selection(list: List) -> usize {
        match list {
            List::Search => SELECTION,
            List::Panel(side) => PANEL_SELECTION[side.index()],
        }
    }
}

const SEARCH_DELAY_MS: u32 = 150;
/// A search that answers within this time keeps the previous counts until
/// its own arrive, so the status bar changes once (DESIGN.md: Searching).
const SEARCHING_NAMED_MS: u32 = 300;
const STATUS_POLL_MS: u32 = 2_000;
const SELECTION_DELAY_MS: u32 = 150;
const AGE_REFRESH_MS: u32 = 60_000;
/// A row each tenth of a second, as the transfer queue
/// scrolls while a row is dragged past its edge.
const DROP_SCROLL_MS: u32 = 100;
const SAVE_DELAY_MS: u32 = 1_000;
/// Twice a second, as the engine's counters expect to be read.
const OPERATIONS_POLL_MS: u32 = 500;

/// The controls, created in `WM_CREATE` in Tab order; Windows destroys them
/// with the window.
struct Controls {
    search: Edit,
    /// Clear, Match case and Match path, after the search box.
    search_tools: Toolbar,
    match_label: Label,
    match_mode: DropDownList,
    filters: FilterPane,
    /// The list's message and its buttons, in the list's empty area. They
    /// are created before the list, so they lie above it and come before it
    /// in the Tab order.
    message: Label,
    /// In [`Model::list_buttons`]' order.
    message_buttons: [Button; 2],
    message_tips: Tooltip,
    list: ListView,
    inspector: Inspector,
    /// Two panels' panels, the left one's first, and the key bar.
    panels: [PanelControls; 2],
    key_bar: [Button; 6],
    /// The tooltips of `F4 Edit`, which is always disabled, and `F6 Move`,
    /// as areas of the window, because a disabled button gets no mouse
    /// messages.
    key_tips: Tooltip,
    status: StatusBar,
    /// As many as [`layout::DIVIDERS`]; those the layout leaves out hide.
    dividers: Vec<Divider>,
}

impl Controls {
    fn create(parent: Hwnd) -> windows::core::Result<Controls> {
        let message_buttons = [
            Button::create(parent, ControlId::MessageButton1.id())?,
            Button::create(parent, ControlId::MessageButton2.id())?,
        ];
        // Show filters is created first, as it stands first.
        let show_filters = icon_tool(parent, ControlId::ShowFilters, Glyph::ClosePane)?;
        let search = Edit::create(parent, ControlId::Search.id())?;
        let clear = ControlId::ClearSearch.id();
        let search_tools = Toolbar::create(
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
        let controls = Controls {
            search,
            search_tools,
            // The label comes right before its list, so its access key
            // focuses the list (DESIGN.md: C19).
            match_label: Label::create(parent, ControlId::MatchLabel.id())?,
            match_mode: DropDownList::create(parent, ControlId::MatchMode.id())?,
            filters: FilterPane::create(parent, show_filters)?,
            message: Label::create_message(parent, ControlId::Message.id())?,
            message_buttons,
            message_tips: Tooltip::create(parent, &message_buttons.map(Button::hwnd))?,
            list: ListView::create(parent, ControlId::List.id())?,
            inspector: Inspector::create(parent)?,
            panels: [
                PanelControls::create(parent, Side::Left)?,
                PanelControls::create(parent, Side::Right)?,
            ],
            key_bar: [
                Button::create_key(parent, KEY_BAR[0].id())?,
                Button::create_key(parent, KEY_BAR[1].id())?,
                Button::create_key(parent, KEY_BAR[2].id())?,
                Button::create_key(parent, KEY_BAR[3].id())?,
                Button::create_key(parent, KEY_BAR[4].id())?,
                Button::create_key(parent, KEY_BAR[5].id())?,
            ],
            key_tips: Tooltip::create_areas(parent, KEY_TIPS.len())?,
            status: StatusBar::for_window(parent)?,
            dividers: (0..layout::DIVIDERS)
                .map(|_| Divider::create(parent))
                .collect::<windows::core::Result<_>>()?,
        };
        controls.message.hwnd().set_visible(false);
        // The search box starts empty; its changes enable Clear.
        controls.search_tools.set_enabled(clear, false);
        for button in controls.message_buttons {
            button.hwnd().set_visible(false);
        }
        Ok(controls)
    }

    /// The toolbars, whose tooltips the window gives.
    fn toolbars(&self) -> impl Iterator<Item = Toolbar> {
        let inspector = &self.inspector;
        [
            self.search_tools,
            self.filters.show,
            self.filters.tools,
            inspector.hide,
            inspector.copy_name,
            inspector.copy_folder,
        ]
        .into_iter()
        .chain(self.status.show_details())
    }

    /// Gives every control the window's font, at creation and on a monitor
    /// with another DPI.
    fn apply_fonts(&self, fonts: &Fonts) {
        let controls = [
            self.search.hwnd(),
            self.search_tools.hwnd(),
            self.match_label.hwnd(),
            self.match_mode.hwnd(),
            self.message.hwnd(),
            self.message_buttons[0].hwnd(),
            self.message_buttons[1].hwnd(),
            self.list.hwnd(),
        ];
        let pane = self.filters.controls();
        let inspector = self.inspector.controls();
        let status = self.status.text_controls();
        let panels = self.panels.iter().flat_map(PanelControls::hwnds);
        let key_bar = self.key_bar.map(Button::hwnd);
        for control in controls
            .into_iter()
            .chain(pane)
            .chain(inspector)
            .chain(status)
            .chain(panels)
            .chain(key_bar)
        {
            control.set_font(&fonts.ui);
        }
        self.inspector.heading.hwnd().set_font(&fonts.heading);
        self.filters.title.hwnd().set_font(&fonts.semibold);
        self.filters.tools.hwnd().set_font(&fonts.ui);
    }

    /// Gives the buttons and the Kind list their icons, and the toolbars'
    /// buttons the `heights` of the controls beside them, at creation, on a
    /// monitor with another DPI, when the system colours change and when
    /// icons are shown or hidden, after the fonts, which reset a toolbar's
    /// buttons. The list message's buttons get theirs with their captions.
    fn apply_glyphs(&self, heights: ToolHeights) {
        let inspector = &self.inspector;
        for toolbar in [
            self.search_tools,
            self.filters.show,
            self.filters.tools,
            inspector.hide,
        ] {
            toolbar.show_images(heights.row);
        }
        for copy in [inspector.copy_name, inspector.copy_folder] {
            copy.show_images(heights.copy);
        }
        if let Some(show) = self.status.show_details() {
            show.show_images(heights.status);
        }
        self.filters.kind.set_icons(&Kind::ALL.map(Kind::glyph));
        for (button, control) in inspector.text_buttons() {
            control.set_icon(button.command.glyph().map(ButtonIcon::Glyph));
        }
        for (index, control) in self.key_bar.iter().enumerate() {
            let command = commands::key_bar_command(index);
            control.set_icon(command.glyph().map(ButtonIcon::Glyph));
        }
        for (control, key) in [
            (self.filters.choose_folder, "filter.choose_folder"),
            (self.panels[0].retry, "panel.retry"),
            (self.panels[1].retry, "panel.retry"),
        ] {
            control.set_icon(ButtonIcon::of(key, false));
        }
        self.status.show_cancel_icon();
    }

    /// What the layout needs that only the controls know, once
    /// [`Self::apply_glyphs`] and the window's texts have set them up.
    fn sizes(&self) -> ControlSizes {
        ControlSizes {
            search_tools: self.search_tools.width(),
            filter_tools: self.filters.tools.width(),
            kind_row: self.filters.kind.row_size(),
            location_row: self.filters.location.row_size().height,
        }
    }
}

/// A toolbar of one icon-only button, such as Show filters, which takes
/// the button's ID.
fn icon_tool(parent: Hwnd, id: ControlId, glyph: Glyph) -> windows::core::Result<Toolbar> {
    let id = id.id();
    Toolbar::create(parent, id, &[Tool::Icon { id, glyph }])
}

/// The icon of a Clear captioned with the catalogue text `key`.
fn clear_glyph(key: &str) -> Glyph {
    captions::button_glyph(key).expect("every Clear has an icon")
}

/// The filter pane's header, its window and its controls, in Tab order, and
/// Show filters. Each label comes right before its control, so its access
/// key focuses the control (DESIGN.md: Filter pane).
struct FilterPane {
    /// In the search row while the pane is hidden.
    show: Toolbar,
    /// The header, in the main window above the pane's window, so it stays
    /// while the pane scrolls: the title, then Clear and Hide filters.
    title: Label,
    tools: Toolbar,
    pane: Hwnd,
    kind_label: Label,
    kind: IconList,
    location_label: Label,
    location: IconList,
    choose_folder: Button,
    only_directly: CheckBox,
    date_label: Label,
    date: DropDownList,
    size_label: Label,
    size: DropDownList,
    attributes_label: Label,
    /// Each attribute's label and list, in the order of
    /// [`crate::core::query::ATTRIBUTES`].
    attributes: Vec<(Label, DropDownList)>,
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
enum RangeFilter {
    Date,
    Size,
}

impl FilterPane {
    fn create(parent: Hwnd, show: Toolbar) -> windows::core::Result<FilterPane> {
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
    fn contains(&self, control: Hwnd) -> bool {
        self.pane.is_parent_of(control)
    }

    /// Every control in the pane.
    fn controls(&self) -> Vec<Hwnd> {
        self.placed_with(None)
            .into_iter()
            .map(|(control, ..)| control)
            .collect()
    }

    /// Each control, its rectangle in `layout` before the pane scrolls, and
    /// whether it is a drop-down list, whose rectangle also holds its open
    /// list.
    fn placed(&self, layout: &FilterLayout) -> Vec<(Hwnd, Rect, bool)> {
        self.placed_with(Some(layout))
            .into_iter()
            .map(|(control, rect, drop_down)| (control, rect.unwrap_or_default(), drop_down))
            .collect()
    }

    fn placed_with(&self, layout: Option<&FilterLayout>) -> Vec<(Hwnd, Option<Rect>, bool)> {
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
    fn set_captions(&self, captions: &FilterCaptions) {
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
    fn range(&self, range: RangeFilter) -> DropDownList {
        match range {
            RangeFilter::Date => self.date,
            RangeFilter::Size => self.size,
        }
    }

    /// The range list that `control` is, if it is one.
    fn range_of(&self, control: Hwnd) -> Option<RangeFilter> {
        [RangeFilter::Date, RangeFilter::Size]
            .into_iter()
            .find(|range| self.range(*range).hwnd() == control)
    }
}

/// The inspector's controls, in Tab order (DESIGN.md: Inspector pane).
struct Inspector {
    /// The selected entry's file icon.
    icon: FileIconLabel,
    heading: Label,
    /// [`INSPECTOR_COPY_NAME`], after the heading.
    copy_name: Toolbar,
    /// [`HIDE_DETAILS`], at the right end of the heading's row.
    hide: Toolbar,
    /// [`INSPECTOR_COPY_FOLDER`], after the folder's value.
    copy_folder: Toolbar,
    /// [`INSPECTOR_COPY_PATH`], at the end of the Date modified row.
    copy_path: Button,
    /// In [`INSPECTOR_BUTTONS`]' order.
    buttons: [Button; INSPECTOR_BUTTONS.len()],
    /// The text buttons' tips.
    tips: Tooltip,
    /// Each field row's label and its value, a read-only box, so the person
    /// can select and copy it.
    fields: Vec<(Label, Edit)>,
    /// The muted age after Date modified's value.
    age: Label,
    /// The gray hint while nothing is selected.
    note: Label,
}

/// The controls of each inspector field row's label and value.
const FIELD_CONTROLS: [(ControlId, ControlId); 4] = [
    (ControlId::InspectorLabel1, ControlId::InspectorValue1),
    (ControlId::InspectorLabel2, ControlId::InspectorValue2),
    (ControlId::InspectorLabel3, ControlId::InspectorValue3),
    (ControlId::InspectorLabel4, ControlId::InspectorValue4),
];

impl Inspector {
    /// Creates the controls in reading order.
    fn create(parent: Hwnd) -> windows::core::Result<Inspector> {
        let button = |button: CommandButton| Button::create(parent, button.control.id());
        let tool = |button: CommandButton, glyph| icon_tool(parent, button.control, glyph);
        let icon = FileIconLabel::create(parent, ControlId::InspectorIcon.id())?;
        let heading = Label::create_heading(parent, ControlId::InspectorHeading.id())?;
        let copy_name = tool(INSPECTOR_COPY_NAME, Glyph::Copy)?;
        let hide = tool(HIDE_DETAILS, Glyph::ChevronDown)?;
        let mut fields = Vec::new();
        let mut copy_folder = None;
        let mut copy_path = None;
        let mut age = None;
        for (index, (label, value)) in FIELD_CONTROLS.into_iter().enumerate() {
            let label = Label::create(parent, label.id())?;
            fields.push((label, Edit::create_read_only(parent, value.id())?));
            if index == 0 {
                copy_folder = Some(tool(INSPECTOR_COPY_FOLDER, Glyph::Copy)?);
            } else if index == MODIFIED_ROW {
                age = Some(Label::create_line(parent, ControlId::InspectorAge.id())?);
                copy_path = Some(button(INSPECTOR_COPY_PATH)?);
            }
        }
        let buttons = INSPECTOR_BUTTONS.map(button);
        let [explore, open_file_location, open] = buttons;
        let buttons = [explore?, open_file_location?, open?];
        let copy_folder = copy_folder.expect("the folder is the first field row");
        let copy_path = copy_path.expect("the inspector has a Date modified row");
        let tipped = buttons.map(Button::hwnd);
        Ok(Inspector {
            icon,
            heading,
            copy_name,
            hide,
            copy_folder,
            copy_path,
            buttons,
            tips: Tooltip::create(parent, &tipped)?,
            fields,
            age: age.expect("the inspector has a Date modified row"),
            note: Label::create_message(parent, ControlId::InspectorNote.id())?,
        })
    }

    /// The buttons with a caption, each with what it runs.
    fn text_buttons(&self) -> [(CommandButton, Button); INSPECTOR_BUTTONS.len() + 1] {
        let [explore, open_file_location, open] = INSPECTOR_BUTTONS;
        let [b0, b1, b2] = self.buttons;
        [
            (explore, b0),
            (open_file_location, b1),
            (open, b2),
            (INSPECTOR_COPY_PATH, self.copy_path),
        ]
    }

    /// The icons, each with what it runs.
    fn icon_tools(&self) -> [(CommandButton, Toolbar); 3] {
        [
            (INSPECTOR_COPY_NAME, self.copy_name),
            (INSPECTOR_COPY_FOLDER, self.copy_folder),
            (HIDE_DETAILS, self.hide),
        ]
    }

    /// Every control that shows text, but the heading, which has a font of
    /// its own.
    fn controls(&self) -> Vec<Hwnd> {
        let fields = self
            .fields
            .iter()
            .flat_map(|(label, value)| [label.hwnd(), value.hwnd()]);
        self.text_buttons()
            .into_iter()
            .map(|(_, b)| b.hwnd())
            .chain(fields)
            .chain([self.age.hwnd(), self.note.hwnd()])
            .collect()
    }

    fn contains(&self, control: Hwnd) -> bool {
        control == self.heading.hwnd() || self.controls().contains(&control)
    }

    /// The value that `control` is, if it is one.
    fn value(&self, control: Hwnd) -> Option<Edit> {
        self.fields
            .iter()
            .map(|(_, value)| *value)
            .find(|value| value.hwnd() == control)
    }
}

/// The controls' font, its bold form for the matched part of a cell, its
/// semibold form for the filter pane's title, and the inspector's heading
/// font.
struct Fonts {
    ui: Font,
    bold: Font,
    semibold: Font,
    heading: Font,
    struck: Font,
}

impl Fonts {
    fn new(dpi: Dpi) -> windows::core::Result<Fonts> {
        let ui = Font::message(dpi)?;
        let bold = ui.bold()?;
        let semibold = ui.semibold()?;
        let heading = ui.heading()?;
        let struck = ui.struck_out()?;
        Ok(Fonts {
            ui,
            bold,
            semibold,
            heading,
            struck,
        })
    }
}

/// What the layout needs that only measuring the text and the controls
/// gives.
#[derive(Clone, Copy)]
struct Metrics {
    dpi: Dpi,
    font_height: Px,
    captions: CaptionWidths,
}

impl Metrics {
    /// Measures after the controls have their fonts, icons and texts.
    fn measure(fonts: &Fonts, captions: &WindowCaptions, controls: &Controls, dpi: Dpi) -> Metrics {
        let meter = TextMeter::new(&fonts.ui);
        Metrics {
            dpi,
            font_height: meter.height(),
            captions: CaptionWidths::measure(captions, &|text| meter.width(text), controls.sizes()),
        }
    }
}

/// The inspector's texts fitted to their lines and the places of the
/// controls that show them ([`MainWindow::inspector_fit`]).
struct InspectorFit {
    moves: Vec<(Hwnd, Rect)>,
    texts: Vec<(Edit, String)>,
    one_entry: bool,
    folder_shown: bool,
}

/// What `WM_CREATE` makes, which every later handler uses: the window
/// procedure calls no other handler before it exists.
struct Created {
    controls: Controls,
    /// Replaced only after every control has the new fonts, because
    /// `WM_SETFONT` does not take ownership.
    fonts: RefCell<Fonts>,
    metrics: Cell<Metrics>,
    /// The client size the status bar last fitted itself to and the height
    /// it took, because each fit erases and draws the whole bar again
    /// (DESIGN.md: Painting). `None` after measuring, which may change its
    /// font or its minimum height.
    status_fit: Cell<Option<(Size, Px)>>,
    /// `None` while the window moves to a monitor with another DPI.
    layout: RefCell<Option<Layout>>,
    work: RefCell<super::work::Work>,
    /// Shared delivery lifetime, closed before this window releases its HWND.
    waker: OpenWaker,
    images: Option<ImageList>,
    /// The inspector's icons; their indexes are those of `images`.
    large_images: Option<ImageList>,
}

/// The open Settings dialog, which replies and the window's job go to.
#[derive(Clone, Copy)]
struct OpenSettings {
    dialog: Hwnd,
    follow_fix: bool,
}

/// Which Gozler of the session a window is (DESIGN.md: Window lifetime).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Instance {
    /// The first: it has the tray icon and the shortcut to show Gozler, and
    /// checks for updates.
    First,
    /// A later start while "Allow only one Gozler window" is off: Close
    /// ends it, and it saves only what Settings applies, so closing it
    /// never undoes a setting that another window saved.
    Another,
}

pub(crate) struct MainWindow {
    created: OnceCell<Created>,
    instance: Instance,
    model: RefCell<Model>,
    numbers: RefCell<WindowsNumbers>,
    dates: RefCell<Dates>,
    /// The control that had the focus when the window was deactivated.
    focus_on_activate: Cell<Option<Hwnd>>,
    settings_file: PathBuf,
    settings_failure_shown: Cell<bool>,
    ending_session: Cell<bool>,
    show_message: u32,
    icons: Icons,
    /// Whether the status timer runs.
    polling: Cell<bool>,
    /// Whether a [`Message::Deferred`] is posted to read the list's
    /// selection, so one user action reads it once (DESIGN.md: The selection).
    selection_pending: Cell<bool>,
    /// The rows each list's notifications told it selects, in the order of
    /// [`List::ALL`] ([`MainWindow::selected_rows`]).
    told: [RefCell<selection::NativeSelection>; 3],
    /// Whether a [`Message::Deferred`] is posted to read the header's order
    /// after the person dragged a column.
    column_dragged: Cell<bool>,
    /// The filter pane's scroll position.
    filter_scroll: Cell<Px>,
    /// How the open Date modified or Size list is closing, between its
    /// `CBN_SELENDOK` or `CBN_SELENDCANCEL` and its `CBN_CLOSEUP`.
    range_closing: Cell<Option<(RangeFilter, ListClosing)>>,
    /// The range list that closed on a choice, whose Custom… opens its
    /// dialog in [`Message::Deferred`], after the list has said which item
    /// it closed on, in whichever order it sends that.
    range_closed: Cell<Option<RangeFilter>>,
    /// Whether the downloaded setup started, so the window closes once its
    /// message loop has ended.
    closing_for_setup: Cell<bool>,
    /// Whether the window exited for Settings' Reset all.
    resetting: Cell<bool>,
    /// What the inspector shows, so a reply that changes none of it sets no
    /// text, and the index of its icon in the system image lists.
    inspector_shown: RefCell<(InspectorCaptions, Option<i32>)>,
    /// What the status bar's right group and memory parts show, which a
    /// new layout fits into parts again.
    status_right: RefCell<Vec<ValuePart>>,
    status_memory: RefCell<Option<captions::MemoryCaptions>>,
    /// The tooltip of each toolbar button, by its ID, which a toolbar asks
    /// for when the pointer rests on the button.
    tool_tips: RefCell<Vec<(u16, String)>>,
    /// While the person drags the splitter: the pointer's client `y` and the
    /// inspector's height when the drag started.
    splitter_drag: Cell<Option<(i32, Px)>>,
    inspector_focus: Cell<Option<Hwnd>>,
    column_sizing: RefCell<ColumnSizing>,
    /// The person's command shortcuts, which Settings replaces; the message
    /// loop translates with the table of the moment.
    accelerators: RefCell<Rc<Accelerators>>,
    /// `TaskbarCreated`, after which the tray icon is added again.
    taskbar_created: u32,
    /// `TaskbarButtonCreated`, after which the operations' progress is
    /// shown on the new button.
    taskbar_button_created: u32,
    /// A start of another program asks this window to exit
    /// ([`super::EXIT_MESSAGE`]).
    exit_message: u32,
    /// The shortcut to show Gozler, while Windows lets Gozler use it.
    shortcut: RefCell<Option<Registration>>,
    starting_shortcut: RefCell<Option<Registration>>,
    /// Why the saved shortcut could not be used at start, which the window
    /// says the first time it shows.
    shortcut_failure: RefCell<Option<RegisterError>>,
    /// Whether the window has not shown yet, because Gozler started in the
    /// notification area; its first show restores the saved placement.
    unshown: Cell<bool>,
    /// The open Settings dialog, from its WM_INITDIALOG until it closes.
    settings: Cell<Option<OpenSettings>>,
    total_commander: RefCell<Option<Result<Option<String>, DaemonFailure>>>,
    /// The acknowledged Include save or index rebuild for Settings.
    include_saved: RefCell<Option<Result<(), NotSaved>>>,
    sign_in_saved: RefCell<Option<(Id, io::Result<()>)>>,
    /// A Paste whose thread reads the clipboard.
    pasting: RefCell<HashMap<OperationId, PathBuf>>,
    /// Whether a click gave the search box the focus, so its text is
    /// selected once the click is handled (Settings > Search).
    select_search: Cell<bool>,
    /// The file operations' queue and the transfer pane, which
    /// only the first window has (DESIGN.md: Interrupted operations).
    file_operations: OnceCell<FileOperations>,
    /// The open Viewers (DESIGN.md: The Viewer).
    viewers: Viewers,
    /// What each panel's list changed since the panel last read it; a
    /// [`Message::Deferred`] is posted to read it.
    panel_changed: [Cell<panels::ListChanged>; 2],
    /// The text typed in each panel's path box, which it shows again when
    /// the typed path cannot be opened.
    panel_typed: RefCell<[Option<String>; 2]>,
    /// The window's registration as a drop target, which only a window with
    /// file operations has, so another window accepts no drop.
    drop_target: RefCell<Option<DropRegistration>>,
    /// While files are dragged over the window.
    drag_in: RefCell<Option<DragIn>>,
    /// The drops whose paths a thread still reads from their source.
    dropping: RefCell<HashMap<OperationId, drops::Dropping>>,
    /// The modal steps that wait for [`Message::Deferred`], in the order
    /// they came.
    modal_steps: RefCell<VecDeque<ModalStep>>,
    /// Whether a modal step runs, whose loop the later steps wait for.
    modal_running: Cell<bool>,
    operation_delivery: RefCell<Option<OperationDelivery>>,
    delivering_operations: Cell<bool>,
    shutdown: RefCell<Option<win::window::Shutdown>>,
}

struct OperationDelivery {
    queue: std::vec::IntoIter<QueueStep>,
    panels: std::vec::IntoIter<crate::core::panel::PanelFollow>,
    dialogs: std::vec::IntoIter<OperationDialog>,
    progress: Option<OperationProgress>,
    sources: std::vec::IntoIter<SourceStep>,
}

enum OperationEffect {
    Queue(QueueStep),
    Panel(crate::core::panel::PanelFollow),
    Dialog(OperationDialog),
    Progress(OperationProgress),
    Source(SourceStep),
}

impl OperationDelivery {
    fn new(steps: OperationSteps) -> Self {
        Self {
            queue: steps.queue.into_iter(),
            panels: steps.panels.into_iter(),
            dialogs: steps.dialogs.into_iter(),
            progress: Some(steps.progress),
            sources: steps.sources.into_iter(),
        }
    }

    fn next(&mut self) -> Option<OperationEffect> {
        self.queue
            .next()
            .map(OperationEffect::Queue)
            .or_else(|| self.panels.next().map(OperationEffect::Panel))
            .or_else(|| self.dialogs.next().map(OperationEffect::Dialog))
            .or_else(|| self.progress.take().map(OperationEffect::Progress))
            .or_else(|| self.sources.next().map(OperationEffect::Source))
    }

    fn is_empty(&self) -> bool {
        self.queue.len() + self.panels.len() + self.dialogs.len() + self.sources.len() == 0
            && self.progress.is_none()
    }
}

/// A dialog, or another call with its own message loop, that a reply, a
/// panel's disk event or a file operation starts. It runs from
/// [`Message::Deferred`], once everything that the same wake brought has
/// applied, because its loop dispatches the next wakes (DESIGN.md: window
/// rule 13).
enum ModalStep {
    /// Asks whether to download the setup that the fix found.
    ConfirmSetup,
    /// Says that the latest release has no setup to install.
    ExplainNoSetup,
    ExportFailed {
        path: PathBuf,
        error: ExportError,
    },
    SettingsNotSaved(std::io::Error),
    OpenSettings(Id, bool),
    CopyFailed(Id, CopyFailure),
    Opened(ShellPurpose, crate::core::work::Id, Opened),
    PreparedPaths(PathPurpose, Id, PreparedPaths),
    Paired(crate::core::work::Id, LocalService, bool),
    /// A panel's folder could not be opened.
    PanelRefused(panels::Refused),
    /// Gives `Enter` to the first row of `ask`'s search, which `Enter` in
    /// the search box waited for, while the list still shows it.
    EnterFirst(ChunkAsk),
    /// A file operation's retained failure dialog.
    Operation(OperationDialog),
    PreparedFailure(PreparedFailure),
    Synchronize(OperationId),
}

/// The folder row that dragged files would be dropped into. The drop goes
/// to this entry even when a results page replaces the rows before it
/// lands, because COM's wait loop can deliver one while `Drop` calls the
/// source.
struct DropFolder {
    row: u64,
    folder: ResultItem,
}

impl MainWindow {
    /// Creates the window, starts the first search and the first status
    /// request, and shows the window, or with `sign_in` starts as the `Run`
    /// value asks (DESIGN.md: Window lifetime). The window owns its state; the
    /// message loop reads its accelerators.
    pub(crate) fn open(
        class: &str,
        model: Model,
        settings_file: PathBuf,
        show_message: u32,
        instance: Instance,
        sign_in: bool,
    ) -> (Hwnd, Rc<MainWindow>) {
        let placement = model.placement();
        let accelerators = accelerator_table(&model.choices().shortcuts, model.mode())
            .expect("the accelerator table builds");
        let viewers = Viewers::new(model.viewer());
        let dates = Dates::new(model.choices().date_format, jiff::Zoned::now());
        let window = Rc::new(MainWindow {
            created: OnceCell::new(),
            instance,
            model: RefCell::new(model),
            numbers: RefCell::new(WindowsNumbers::new()),
            dates: RefCell::new(dates),
            focus_on_activate: Cell::new(None),
            settings_file,
            settings_failure_shown: Cell::new(false),
            ending_session: Cell::new(false),
            show_message,
            icons: Icons::default(),
            polling: Cell::new(false),
            selection_pending: Cell::new(false),
            told: Default::default(),
            column_dragged: Cell::new(false),
            filter_scroll: Cell::new(Px(0)),
            range_closing: Cell::new(None),
            range_closed: Cell::new(None),
            closing_for_setup: Cell::new(false),
            resetting: Cell::new(false),
            inspector_shown: RefCell::new((InspectorCaptions::default(), None)),
            status_right: RefCell::new(Vec::new()),
            status_memory: RefCell::new(None),
            tool_tips: RefCell::new(Vec::new()),
            splitter_drag: Cell::new(None),
            inspector_focus: Cell::new(None),
            column_sizing: RefCell::default(),
            accelerators: RefCell::new(Rc::new(accelerators)),
            taskbar_created: win::instance::register_message("TaskbarCreated"),
            taskbar_button_created: win::instance::register_message("TaskbarButtonCreated"),
            exit_message: win::instance::register_message(super::EXIT_MESSAGE),
            shortcut: RefCell::new(None),
            starting_shortcut: RefCell::new(None),
            shortcut_failure: RefCell::new(None),
            unshown: Cell::new(false),
            settings: Cell::new(None),
            total_commander: RefCell::new(None),
            include_saved: RefCell::new(None),
            sign_in_saved: RefCell::new(None),
            pasting: RefCell::new(HashMap::new()),
            shutdown: RefCell::new(None),
            select_search: Cell::new(false),
            file_operations: OnceCell::new(),
            viewers,
            panel_changed: Default::default(),
            panel_typed: RefCell::new([None, None]),
            drop_target: RefCell::new(None),
            drag_in: RefCell::new(None),
            dropping: RefCell::new(HashMap::new()),
            modal_steps: RefCell::new(VecDeque::new()),
            modal_running: Cell::new(false),
            operation_delivery: RefCell::new(None),
            delivering_operations: Cell::new(false),
        });
        let hwnd = win::window::create_window(class, Rc::clone(&window), placement)
            .expect("the main window opens");
        // Only a status reply names the volumes, so it is asked for beside
        // the first search (DESIGN.md: Requests).
        window.search(hwnd, List::Search);
        window.ask_status();
        if window.model.borrow().mode() == Mode::TwoPanels {
            window.start_panels(hwnd);
        }
        if instance == Instance::First {
            window.register_saved_shortcut(hwnd);
            window.check_for_updates();
            window.open_file_operations(hwnd);
            let registration =
                drop_target::register(hwnd, Rc::clone(&window) as Rc<dyn DropHandler>);
            window.drop_target.replace(registration);
        }
        window.update_tray(hwnd);
        let keeps_running = window.model.borrow().choices().keep_running;
        // Showing the window activates it, and WM_ACTIVATE focuses the
        // search box.
        match (sign_in, keeps_running) {
            (true, true) => window.unshown.set(true),
            (true, false) => win::window::show_minimized(hwnd, placement),
            (false, _) => win::window::show(hwnd, placement),
        }
        (hwnd, window)
    }

    /// The accelerator table of the moment, which the message loop holds
    /// until the command it sends returns.
    pub(crate) fn accelerators(&self) -> Rc<Accelerators> {
        Rc::clone(&self.accelerators.borrow())
    }

    fn created(&self) -> &Created {
        self.created
            .get()
            .expect("the window procedure runs a handler only after WM_CREATE made the controls")
    }

    fn on_create(&self, hwnd: Hwnd) -> bool {
        let dpi = hwnd.dpi();
        let (Ok(controls), Ok(fonts)) = (Controls::create(hwnd), Fonts::new(dpi)) else {
            return false;
        };
        controls.apply_fonts(&fonts);
        win::gdi::show_icons(self.model.borrow().choices().show_icons);
        controls.apply_glyphs(ToolHeights::new(dpi, TextMeter::new(&fonts.ui).height()));
        let (columns, order) = {
            let model = self.model.borrow();
            let columns: Vec<(Column, Px)> = model
                .columns()
                .iter()
                .map(|c| (*c, column_sizing::column_width(*c, dpi)))
                .collect();
            (columns, model.column_order())
        };
        for (index, (column, width)) in columns.into_iter().enumerate() {
            controls.list.insert_column(
                index,
                &ListColumn {
                    header: "",
                    width,
                    right_aligned: column.row().right_aligned,
                },
            );
        }
        controls.list.set_column_order(&order);
        let images = ImageList::system_small();
        if let Some(images) = &images {
            controls.list.set_small_images(images);
            for panel in &controls.panels {
                panel.list.set_small_images(images);
            }
        }
        let captions = self.captions();
        // Measured again once the controls have their texts.
        let metrics = Metrics::measure(&fonts, &captions, &controls, dpi);
        let waker = OpenWaker::new(hwnd);
        let _ = self.created.set(Created {
            controls,
            fonts: RefCell::new(fonts),
            metrics: Cell::new(metrics),
            status_fit: Cell::new(None),
            layout: RefCell::new(None),
            work: RefCell::new(super::work::Work::new(
                waker.clone(),
                self.settings_file.with_file_name("ui-error.log"),
            )),
            waker,
            images,
            large_images: ImageList::system_large(),
        });
        self.apply_text(hwnd, &captions);
        self.measure(&captions, dpi);
        self.layout(hwnd);
        true
    }

    /// Gives the controls and the memory figures their icons at `dpi`, in
    /// the system colours and with Show icons of now, after the fonts.
    fn apply_glyphs(&self, dpi: Dpi) {
        let created = self.created();
        let font_height = TextMeter::new(&created.fonts.borrow().ui).height();
        created
            .controls
            .apply_glyphs(ToolHeights::new(dpi, font_height));
        if let Some(operations) = self.file_operations.get() {
            operations.apply_glyphs();
        }
        self.update_status();
    }

    /// Measures the texts and the controls again at `dpi`, after their
    /// texts, their fonts or their icons changed.
    fn measure(&self, captions: &WindowCaptions, dpi: Dpi) {
        let created = self.created();
        let metrics = Metrics::measure(&created.fonts.borrow(), captions, &created.controls, dpi);
        created.metrics.set(metrics);
        created.status_fit.set(None);
    }

    /// Every caption, from the model's facts and the current strings
    /// (DESIGN.md: window rule 12).
    fn captions(&self) -> WindowCaptions {
        WindowCaptions::new(&self.model.borrow(), &*self.numbers.borrow())
    }

    /// Sets every caption, then measures them and lays out again. Runs
    /// after a change to the language or to the regional format, inside the
    /// caller's [`Hwnd::draw_once_after`], which does not nest, so a caller
    /// that refreshes several things draws the window once.
    fn refresh_text(&self, hwnd: Hwnd) {
        let captions = self.captions();
        self.apply_text(hwnd, &captions);
        self.measure(&captions, hwnd.dpi());
        self.layout(hwnd);
    }

    /// Loads the language of `choice`, gives it to the model and sets every
    /// caption in it (DESIGN.md: Language). The Language menu
    /// and Settings' OK and Apply call it; the typed text, the results, the
    /// selection and the focus stay as they are. Runs inside the caller's
    /// [`Hwnd::draw_once_after`].
    fn set_language(&self, hwnd: Hwnd, choice: LanguageChoice) {
        let language = choice.resolve(&win::locale::display_languages());
        let accepted = (self.model.borrow_mut()).set_language(choice, Strings::load(language));
        super::set_panic_report(|report| report.language = Some(language));
        self.apply(hwnd, accepted);
        self.refresh_text(hwnd);
        if let Some(operations) = self.file_operations.get() {
            operations.set_strings(Strings::load(language));
            self.refresh_operations();
        }
        self.viewers.set_strings(&Strings::load(language));
    }

    fn apply_text(&self, hwnd: Hwnd, captions: &WindowCaptions) {
        {
            let mut dates = self.dates.borrow_mut();
            dates.format = self.model.borrow().choices().date_format;
            dates.set_time(jiff::Zoned::now());
        }
        self.follow_dates(hwnd);
        let controls = &self.created().controls;
        hwnd.set_text(&captions.title);
        let _ = win::menu::replace_menu_bar(hwnd, &captions.menu_bar);
        controls.search.set_cue(&captions.search_cue);
        // The caption stays the name that screen readers say, and the
        // access key, while Clear shows only its icon.
        let tools = controls.search_tools;
        tools.set_text(ControlId::ClearSearch.id(), &captions.clear_search);
        tools.set_text(ControlId::MatchCase.id(), &captions.match_case);
        tools.set_text(ControlId::MatchPath.id(), &captions.match_path);
        controls.match_label.hwnd().set_text(&captions.match_label);
        let mode = self.match_mode_index();
        let modes = captions.match_modes.each_ref().map(String::as_str);
        controls.match_mode.set_items(&modes, mode);
        controls.filters.set_captions(&captions.filters);
        self.show_filters(&captions.filters);
        for (index, header) in captions.headers.iter().enumerate() {
            controls.list.set_header(index, header);
        }
        let inspector = &controls.inspector;
        let [explore, open_file_location, open] = &captions.inspector_buttons;
        for (button, caption) in [
            (inspector.buttons[0], explore),
            (inspector.buttons[1], open_file_location),
            (inspector.buttons[2], open),
            (inspector.copy_path, &captions.copy_path),
        ] {
            button.hwnd().set_text(&caption.caption);
            inspector.tips.set(button.hwnd(), &caption.tip);
        }
        for ((button, toolbar), caption) in inspector.icon_tools().into_iter().zip([
            &captions.copy_name,
            &captions.copy_folder,
            &captions.hide_details,
        ]) {
            toolbar.set_text(button.control.id(), &caption.caption);
        }
        self.show_inspector(hwnd, captions.inspector.clone());
        controls.status.set_cancel_caption(&captions.status_cancel);
        controls
            .status
            .set_progress_captions(&captions.status_progress);
        controls
            .status
            .set_show_details_caption(&captions.show_details.caption);
        let filters = &captions.filters;
        self.tool_tips.replace(vec![
            (
                ControlId::ClearSearch.id(),
                without_access_key(&captions.clear_search),
            ),
            (ControlId::ShowFilters.id(), filters.show.clone()),
            (ControlId::ClearFilters.id(), filters.clear_tip.clone()),
            (ControlId::HideFilters.id(), filters.hide.clone()),
            (
                INSPECTOR_COPY_NAME.control.id(),
                captions.copy_name.tip.clone(),
            ),
            (
                INSPECTOR_COPY_FOLDER.control.id(),
                captions.copy_folder.tip.clone(),
            ),
            (HIDE_DETAILS.control.id(), captions.hide_details.tip.clone()),
            (
                ControlId::StatusShowDetails.id(),
                captions.show_details.tip.clone(),
            ),
        ]);
        self.show_status(&captions.status);
        self.show_message(captions.message.as_ref());
        self.apply_panel_text(captions);
        self.update_bound_controls();
    }

    /// Gives the filter pane's lists their items and selects the model's
    /// choices.
    fn show_filters(&self, captions: &FilterCaptions) {
        let pane = &self.created().controls.filters;
        let (positions, located, only_directly) = {
            let model = self.model.borrow();
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

    fn update_filters(&self) {
        let captions = FilterCaptions::new(&self.model.borrow(), &*self.numbers.borrow());
        self.show_filters(&captions);
    }

    /// The Location list after a status reply, which can name other
    /// volumes; the other lists keep what the person moved to, such as
    /// Custom… with the arrow keys.
    fn update_locations(&self) {
        let (locations, position) = {
            let model = self.model.borrow();
            let items = model.location_items();
            (
                captions::location_items(&model),
                model.filters().positions(&items).location,
            )
        };
        self.created()
            .controls
            .filters
            .location
            .set_paths(&locations, position);
    }

    fn match_mode_index(&self) -> usize {
        let mode = self.model.borrow().matching().mode;
        MatchMode::ALL
            .iter()
            .position(|m| *m == mode)
            .expect("ALL holds every mode")
    }

    /// Gives the controls bound to a command the command's state and
    /// choice, after every change (DESIGN.md: window rule 9): the match
    /// controls, the pane's Clear filters, the inspector's buttons and the
    /// header's sort arrow.
    fn update_bound_controls(&self) {
        let controls = &self.created().controls;
        let (enabled, can_clear, buttons, icons, matching) = {
            let model = self.model.borrow();
            let state = State {
                model: &model,
                focus: Focus::Other,
                list: model.active_list(),
            };
            (
                (CommandId::Match.row().enabled)(&state),
                (CommandId::ClearFilters.row().enabled)(&state),
                controls
                    .inspector
                    .text_buttons()
                    .map(|(button, control)| (control, (button.command.row().enabled)(&state))),
                controls.inspector.icon_tools().map(|(button, toolbar)| {
                    (
                        toolbar,
                        button.control,
                        (button.command.row().enabled)(&state),
                    )
                }),
                model.matching(),
            )
        };
        controls
            .filters
            .tools
            .set_enabled(ControlId::ClearFilters.id(), can_clear);
        for (button, enabled) in buttons {
            button.hwnd().set_enabled(enabled);
        }
        for (toolbar, id, enabled) in icons {
            toolbar.set_enabled(id.id(), enabled);
        }
        for control in [controls.match_label.hwnd(), controls.match_mode.hwnd()] {
            control.set_enabled(enabled);
        }
        for (id, checked) in [
            (ControlId::MatchCase.id(), matching.case),
            (ControlId::MatchPath.id(), matching.path),
        ] {
            controls.search_tools.set_enabled(id, enabled);
            controls.search_tools.set_checked(id, checked);
        }
        controls.match_mode.select(self.match_mode_index());
        self.show_sort();
    }

    fn update_status(&self) {
        let status = StatusCaptions::new(&self.model.borrow(), &*self.numbers.borrow());
        self.show_status(&status);
    }

    /// Shows the selection in the inspector, if anything it shows changed.
    fn update_inspector(&self, hwnd: Hwnd) {
        let captions = InspectorCaptions::new(&self.model.borrow(), &*self.numbers.borrow());
        if self.inspector_shown.borrow().0 != captions {
            self.show_inspector(hwnd, captions);
        }
    }

    fn show_inspector(&self, hwnd: Hwnd, captions: InspectorCaptions) {
        if self.model.borrow().panes().content == crate::core::layout::InspectorContent::Transfers {
            return;
        }
        let inspector = &self.created().controls.inspector;
        inspector.heading.hwnd().set_text(&captions.heading);
        for (index, (label, value)) in inspector.fields.iter().enumerate() {
            let (label_text, value_text) = captions
                .fields
                .get(index)
                .map_or(("", ""), |(l, v)| (l.as_str(), v.as_str()));
            label.hwnd().set_text(label_text);
            value.hwnd().set_text(value_text);
        }
        inspector
            .note
            .hwnd()
            .set_text(captions.note().unwrap_or(""));
        if captions.modified().is_some() {
            hwnd.set_timer(timer::AGE, AGE_REFRESH_MS);
        } else {
            hwnd.kill_timer(timer::AGE);
        }
        // The shell call can dispatch messages, so no borrow is held
        // (DESIGN.md: window rule 1).
        let icon = match &captions.inspected {
            Inspected::One {
                name, is_folder, ..
            } => Some(self.icons.index(name, *is_folder)),
            _ => None,
        };
        let icon_changed = self.inspector_shown.borrow().1 != icon;
        self.inspector_shown.replace((captions, icon));
        self.show_age();
        self.show_fields();
        if icon_changed {
            inspector
                .icon
                .show_file_icon(self.created().large_images.zip(icon));
        }
    }

    /// Writes how long ago the shown entry was modified, from the current
    /// time, so a minute's refresh keeps it true (DESIGN.md: Inspector pane).
    fn show_age(&self) {
        let age = self.inspector_shown.borrow().0.modified().map(|modified| {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |since| {
                    i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
                });
            captions::age(
                self.model.borrow().strings(),
                &*self.numbers.borrow(),
                modified,
                gozler_core::output::filetime::unix_ms_to_filetime(now),
            )
        });
        let inspector = &self.created().controls.inspector;
        inspector.age.hwnd().set_text(age.as_deref().unwrap_or(""));
    }

    fn show_fields(&self) {
        self.show_field_rows();
        self.fit_inspector();
    }

    /// Shows each field row that has a field and room in the pane, and
    /// hides the others; the note and the age likewise.
    fn show_field_rows(&self) {
        let (rows, note, age) = {
            let layout = self.created().layout.borrow();
            let transfers = self.model.borrow().panes().content
                == crate::core::layout::InspectorContent::Transfers;
            let placed = layout
                .as_ref()
                .and_then(Layout::inspector)
                .filter(|_| !transfers);
            let room = placed.map_or(0, |i| i.fields.len());
            let shown = self.inspector_shown.borrow();
            let rows = room.min(shown.0.fields.len());
            (
                rows,
                placed.is_some_and(|i| i.note.is_some()) && shown.0.note().is_some(),
                MODIFIED_ROW < rows && shown.0.modified().is_some(),
            )
        };
        let inspector = &self.created().controls.inspector;
        for (index, (label, value)) in inspector.fields.iter().enumerate() {
            label.hwnd().set_visible(index < rows);
            value.hwnd().set_visible(index < rows);
        }
        inspector.note.hwnd().set_visible(note);
        inspector.age.hwnd().set_visible(age);
    }

    /// Fits the inspector's texts to their lines in the shown layout
    /// ([`Self::inspector_fit`]), when they change apart from a layout.
    fn fit_inspector(&self) {
        if self.model.borrow().panes().content == crate::core::layout::InspectorContent::Transfers {
            return;
        }
        let fit = (self.created().layout.borrow().as_ref())
            .and_then(Layout::inspector)
            .map(|placed| self.inspector_fit(placed));
        self.show_inspector_fit(fit);
    }

    /// Fits the inspector's texts to their lines in `placed`: a copy icon
    /// stands 8 px after the text it copies, for one entry only, whose name
    /// and folder it copies, the age a space after the modification time,
    /// and a value too long for its line ends in "…", a path losing its
    /// middle as Windows shortens one (DESIGN.md: Inspector pane). The name
    /// and the age end in an ellipsis themselves, and a value that has the
    /// focus keeps its whole text, so the person selects what is there.
    fn inspector_fit(&self, placed: &InspectorLayout) -> InspectorFit {
        let created = self.created();
        let inspector = &created.controls.inspector;
        let shown = self.inspector_shown.borrow();
        // The copy icons copy one entry's name and folder.
        let one_entry = matches!(shown.0.inspected, Inspected::One { .. });
        let fonts = created.fonts.borrow();
        let meter = TextMeter::new(&fonts.ui);
        let focused = Hwnd::focused();
        let mut moves = Vec::new();
        let mut heading = placed.heading;
        let mut folder_shown = false;
        if one_entry {
            let width = TextMeter::new(&fonts.heading).plain_width(&shown.0.heading);
            let (text, icon) = placed.copy_beside(heading, width);
            heading = text;
            moves.push((inspector.copy_name.hwnd(), icon));
        }
        moves.push((inspector.heading.hwnd(), heading));
        let mut texts: Vec<(Edit, String)> = Vec::new();
        for (index, ((_, line), &(_, value))) in
            placed.fields.iter().zip(&inspector.fields).enumerate()
        {
            let text = shown.0.fields.get(index).map_or("", |(_, v)| v.as_str());
            let margins = value.margins();
            let mut rect = *line;
            if index == 0 && one_entry {
                let (fitted, icon) = placed.copy_beside(rect, meter.plain_width(text) + margins);
                rect = fitted;
                moves.push((inspector.copy_folder.hwnd(), icon));
                folder_shown = true;
            } else if index == MODIFIED_ROW && shown.0.modified().is_some() {
                let (fitted, age) = placed.age_beside(
                    rect,
                    meter.plain_width(text) + margins,
                    meter.plain_width(" "),
                    meter.plain_width(&inspector.age.hwnd().text()),
                );
                rect = fitted;
                moves.push((inspector.age.hwnd(), age));
            }
            moves.push((value.hwnd(), rect));
            let room = rect.width - margins;
            let fitting = if focused == Some(value.hwnd()) {
                text.to_owned()
            } else {
                meter.compact_path(text, room)
            };
            texts.push((value, fitting));
        }
        InspectorFit {
            moves,
            texts,
            one_entry,
            folder_shown,
        }
    }

    /// Shows `fit`: sets its texts, moves the controls it still moves and
    /// shows the copy icons it places, or hides them for `None`, while the
    /// inspector is hidden.
    fn show_inspector_fit(&self, fit: Option<InspectorFit>) {
        let inspector = &self.created().controls.inspector;
        let Some(fit) = fit else {
            for copy in [inspector.copy_name, inspector.copy_folder] {
                copy.hwnd().set_visible(false);
            }
            return;
        };
        // A new text notifies the window, whose handlers may borrow what
        // `inspector_fit` borrowed, so no borrow is held here.
        for (value, text) in fit.texts {
            if value.text() != text {
                value.hwnd().set_text(&text);
            }
        }
        win::move_children(&fit.moves);
        inspector.copy_name.hwnd().set_visible(fit.one_entry);
        inspector.copy_folder.hwnd().set_visible(fit.folder_shown);
    }

    fn show_status(&self, status: &StatusCaptions) {
        let controls = &self.created().controls;
        self.status_right.replace(status.right.clone());
        let mut job = status.job.clone();
        let settings_fix = self.settings.get().is_some_and(|open| open.follow_fix)
            && self.model.borrow().job() == Job::Fix;
        if settings_fix {
            job = JobCaption {
                transfer: job.transfer,
                transfer_only: true,
                ..JobCaption::default()
            };
        } else {
            let now = Instant::now();
            let model = self.model.borrow();
            match model.job() {
                Job::Fix if self.fix_waiting(now) == Some(Waiting::Slow) => {
                    job.text = crate::core::work::waiting(model.strings(), None);
                }
                Job::Export
                    if self.request_waiting(Purpose::Export, now) == Some(Waiting::Slow) =>
                {
                    if let crate::core::model::Export::Running { path, .. } = model.export() {
                        job.text = crate::core::work::waiting(model.strings(), path.to_str());
                    }
                }
                _ => {}
            }
        }
        let cancel = self
            .model
            .borrow()
            .strings()
            .text("button.cancel")
            .to_owned();
        controls.status.set_cancel_caption(&cancel);
        self.status_memory.replace(status.memory.clone());
        let layout = self
            .created()
            .layout
            .borrow()
            .as_ref()
            .map(|layout| layout.status);
        // Cancel hides when the job ends, and a hidden control would keep
        // the focus but take no key.
        let mut entries = vec![
            (StatusSlot::Left, StatusEntry::Text(&status.view)),
            (
                StatusSlot::Right,
                StatusEntry::Values {
                    parts: &status.right,
                    memory: status.memory.as_ref(),
                },
            ),
        ];
        if !job.transfer_only {
            entries.push((StatusSlot::Center, StatusEntry::Work(&job)));
        }
        entries.extend(
            status
                .indexing
                .iter()
                .map(|text| (StatusSlot::Center, StatusEntry::Text(text))),
        );
        if let Some(transfer) = &job.transfer {
            entries.push((StatusSlot::Center, StatusEntry::Transfer(transfer)));
        }
        if controls.status.render(&entries, layout.as_ref()) {
            self.home().focus();
        }
    }

    /// A running service fix follows the modal dialog, where its controls
    /// stay reachable; an earlier fix's result does not enter a new dialog.
    pub(super) fn settings_opened(&self, dialog: Hwnd) {
        let follow_fix = self.model.borrow().fix().running();
        self.settings.set(Some(OpenSettings { dialog, follow_fix }));
        self.update_status();
    }

    pub(super) fn settings_job(&self, strings: &Strings) -> JobCaption {
        let follows = self.settings.get().is_some_and(|open| open.follow_fix);
        let mut job = captions::settings_job(
            &self.model.borrow(),
            strings,
            &*self.numbers.borrow(),
            follows,
        );
        if follows && self.fix_waiting(Instant::now()) == Some(Waiting::Slow) {
            job.text = crate::core::work::waiting(strings, None);
        }
        job
    }

    pub(super) fn request_waiting(&self, purpose: Purpose, now: Instant) -> Option<Waiting> {
        self.created().work.borrow().requests.waiting(purpose, now)
    }

    pub(super) fn sign_in_waiting(&self, id: Id, now: Instant) -> Option<Waiting> {
        self.created().work.borrow().sign_in_waiting(id, now)
    }

    pub(super) fn elevated_waiting(&self, now: Instant) -> Option<Waiting> {
        self.created().work.borrow().elevated.waiting(now)
    }

    pub(super) fn fix_waiting(&self, now: Instant) -> Option<Waiting> {
        self.created().work.borrow().fixes.waiting(now)
    }

    pub(super) fn cancel_settings_request(&self, hwnd: Hwnd, purpose: Purpose) {
        let accepted = match purpose {
            Purpose::Program => {
                self.total_commander.take();
                self.model.borrow_mut().forget_total_commander()
            }
            Purpose::IncludeCount => self.model.borrow_mut().forget_included_count(),
            _ => return,
        };
        self.apply(hwnd, accepted);
        self.post_settings_reply();
    }

    pub(super) fn cancel_settings_fix(&self, hwnd: Hwnd) {
        self.created().work.borrow_mut().cancel_pairing();
        let accepted = self.model.borrow_mut().cancel_fix();
        self.apply(hwnd, accepted);
    }

    /// Tells the open Settings dialog that a reply waits for it: Use Total
    /// Commander's, Include's count, or a fix's step, which the Drives and
    /// service page follows.
    fn post_settings_reply(&self) {
        if let Some(open) = self.settings.get() {
            win::dialog::post_reply(open.dialog);
        }
    }

    /// The Settings dialog's status bar's widest texts and Cancel's
    /// caption, which it measures in its own font.
    pub(super) fn settings_status_texts(&self) -> (StatusWidest, String) {
        let model = self.model.borrow();
        let strings = model.strings();
        (
            captions::settings_status_widest(strings),
            strings.text("button.cancel").to_owned(),
        )
    }

    /// The main window's status bar's Cancel: stops the job
    /// that the bar shows, as File > Cancel does.
    pub(super) fn cancel_job(&self, hwnd: Hwnd) {
        self.run(hwnd, MenuItem::Run(CommandId::Cancel));
    }

    /// Runs a failure's fix (DESIGN.md: Service fixes). The list's button
    /// and the Index page's call it, with the main window as `hwnd`, which
    /// owns Windows' permission prompt and outlives the fix's thread.
    pub(super) fn run_fix(&self, hwnd: Hwnd, fix: Fix) {
        self.created().work.borrow_mut().cancel_pairing();
        match &fix {
            Fix::OpenServices => {
                self.created().work.borrow_mut().open(ShellTarget::Services);
                return;
            }
            Fix::UseLocal(local) => {
                let question = captions::use_local_question(self.model.borrow().strings(), local);
                if !win::dialog::ask(self.fix_owner(hwnd), &question) {
                    return;
                }
                self.created()
                    .work
                    .borrow_mut()
                    .check_pairing(local.clone());
                return;
            }
            Fix::Repair(_) | Fix::Update => {}
        }
        self.start_fix(hwnd, &fix);
    }

    fn start_fix(&self, hwnd: Hwnd, fix: &Fix) {
        if let Some(mut open) = self.settings.get() {
            open.follow_fix = true;
            self.settings.set(Some(open));
        }
        let accepted = self.model.borrow_mut().start_fix(fix);
        self.apply(hwnd, accepted);
    }

    /// Runs the fix that `shown` gives, the one a button of the Index
    /// page's Service group shows, such as [`Model::service_fix`].
    pub(super) fn run_shown_fix(&self, hwnd: Hwnd, shown: fn(&Model) -> Option<Fix>) {
        let fix = shown(&self.model.borrow());
        if let Some(fix) = fix {
            self.run_fix(hwnd, fix);
        }
    }

    /// The window that owns a fix's question or message: the Settings
    /// dialog while it is open, because the main window under it takes no
    /// input, else the main window.
    fn fix_owner(&self, hwnd: Hwnd) -> Hwnd {
        self.settings.get().map_or(hwnd, |open| open.dialog)
    }

    /// Asks the person whether to download the setup the fix found, in a
    /// task dialog, and goes on as they answer (DESIGN.md: Service fixes).
    fn confirm_setup(&self, hwnd: Hwnd) {
        let question = captions::setup_question(&self.model.borrow(), &*self.numbers.borrow());
        let Some(question) = question else {
            return;
        };
        let download = win::dialog::ask(self.fix_owner(hwnd), &question);
        let accepted = self.model.borrow_mut().answer_setup(download);
        self.apply(hwnd, accepted);
    }

    /// Says why the fix found no setup to install, so it downloads nothing,
    /// unless another fix started since.
    fn explain_no_setup(&self, hwnd: Hwnd) {
        let text = {
            let model = self.model.borrow();
            (model.fix().found_no_setup())
                .then(|| model.strings().text("fixing.no_setup").to_owned())
        };
        if let Some(text) = text {
            self.error_box(self.fix_owner(hwnd), &text);
        }
    }

    fn run_setup(&self, setup: PathBuf) {
        self.created().work.borrow_mut().start_setup(setup);
    }

    /// Closes the window after its message loop has ended, when the setup
    /// asked for it in [`MainWindow::run_setup`].
    pub(super) fn close_for_setup(&self, hwnd: Hwnd) {
        if self.closing_for_setup.get() {
            self.exit(hwnd);
        }
    }

    /// Setup can end the message loop before destroying the window, so its
    /// caller also holds the process open for its essential writers.
    pub(super) fn finish_shutdown(&self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.finish();
        }
    }

    /// The service's report, which the Index page's Show report shows.
    pub(super) fn service_report(&self) -> Option<String> {
        captions::service_report(&self.model.borrow())
    }

    fn update_message(&self) {
        let slow = self.slow_request(List::Search);
        let message = if slow.is_some() && self.model.borrow().mode() == Mode::Search {
            let model = self.model.borrow();
            let text =
                crate::core::work::waiting(model.strings(), model.filters().location.as_deref());
            Some(ListCaption {
                text,
                buttons: vec![captions::MessageButton {
                    label: model.strings().text("button.cancel").to_owned(),
                    icon: ButtonIcon::of("button.cancel", false),
                    enabled: true,
                    tooltip: None,
                }],
            })
        } else {
            captions::list_message(&self.model.borrow())
        };
        self.show_message(message.as_ref());
    }

    pub(super) fn slow_request(&self, list: List) -> Option<Purpose> {
        let now = Instant::now();
        let work = self.created().work.borrow();
        [Purpose::Selection(list), Purpose::Results(list)]
            .into_iter()
            .find(|purpose| work.requests.waiting(*purpose, now) == Some(Waiting::Slow))
    }

    pub(super) fn cancel_list_read(&self, hwnd: Hwnd, purpose: Purpose) {
        let list = match purpose {
            Purpose::Selection(list) | Purpose::Results(list) => list,
            _ => return,
        };
        if self.slow_request(list) != Some(purpose) {
            return;
        }
        let accepted = match purpose {
            Purpose::Selection(_) => {
                let empty: RowSelection = Vec::new().into();
                self.forget_selection_action(list);
                self.model.borrow_mut().set_selection(list, empty)
            }
            Purpose::Results(_) => self.model.borrow_mut().cancel_results(list),
            _ => return,
        };
        self.apply(hwnd, accepted);
        if matches!(purpose, Purpose::Selection(_)) {
            self.list_control(list).select_only(None);
            self.seed_native_selection(list, Vec::new().into());
        }
        self.update_message();
        if let List::Panel(side) = list {
            self.show_panel_message(side);
        }
        self.poll_work(hwnd);
    }

    /// Shows the list's message and its buttons, or hides them.
    fn show_message(&self, message: Option<&ListCaption>) {
        let panes = self.model.borrow().panes();
        let maximized = panes.inspector
            && panes.maximized
            && panes.content == crate::core::layout::InspectorContent::Transfers;
        let message = message.filter(|_| !maximized);
        let controls = &self.created().controls;
        let (text, buttons) = match message {
            Some(message) => (message.text.as_str(), message.buttons.as_slice()),
            None => ("", &[][..]),
        };
        // Read first, because Windows takes the focus from a control it
        // disables.
        let focused = Hwnd::focused();
        // Setting a control's text takes the window manager's lock also
        // when the text is the same, and every resize comes through here.
        let set_text = |control: Hwnd, text: &str| {
            if control.text() != text {
                control.set_text(text);
            }
        };
        set_text(controls.message.hwnd(), text);
        for (index, button) in controls.message_buttons.iter().enumerate() {
            let shown = buttons.get(index);
            set_text(
                button.hwnd(),
                shown.map_or("", |shown| shown.label.as_str()),
            );
            button.set_icon(shown.and_then(|shown| shown.icon));
            button
                .hwnd()
                .set_enabled(shown.is_none_or(|shown| shown.enabled));
            let tip = shown.and_then(|shown| shown.tooltip.as_deref());
            controls
                .message_tips
                .set(button.hwnd(), tip.unwrap_or_default());
        }
        // A hidden message is placed when it shows again.
        let list = (self.created().layout.borrow().as_ref())
            .and_then(Layout::search)
            .map(|search| search.list);
        if message.is_some()
            && let Some(list) = list
        {
            let header = controls.list.header().screen_rect().height;
            let shown: Vec<Hwnd> = (controls.message_buttons.iter())
                .take(buttons.len())
                .map(|button| button.hwnd())
                .collect();
            self.place_message(list, header, controls.message.hwnd(), text, &shown, false);
        }
        controls.message.hwnd().set_visible(message.is_some());
        let mut focus_lost = false;
        for (index, button) in controls.message_buttons.iter().enumerate() {
            let usable = buttons.get(index).is_some_and(|shown| shown.enabled);
            focus_lost |= !usable && focused == Some(button.hwnd());
            button.hwnd().set_visible(index < buttons.len());
        }
        // A hidden or disabled button would keep the focus but take no key.
        if focus_lost {
            controls.search.hwnd().focus();
        }
    }

    /// Places a list's `message` control, which shows `text`, below the
    /// header of `list`, `header` tall, and `buttons` in a row below it,
    /// centred under a centred message, as a panel's.
    fn place_message(
        &self,
        list: Rect,
        header: Px,
        message: Hwnd,
        text: &str,
        buttons: &[Hwnd],
        centred: bool,
    ) {
        let created = self.created();
        let metrics = created.metrics.get();
        let area = MessageArea::new(list, header, metrics.dpi);
        let (text, rects) = {
            let fonts = created.fonts.borrow();
            let meter = TextMeter::new(&fonts.ui);
            area.place(
                meter.wrapped_height(text, area.text_width()),
                metrics.font_height,
                buttons.len(),
                metrics.captions.message_button_width(metrics.dpi),
                centred,
            )
        };
        let mut moves = vec![(message, text)];
        moves.extend(buttons.iter().copied().zip(rects));
        win::move_children(&moves);
    }

    /// Places every control where `Layout::compute` says (DESIGN.md:
    /// window rule 7), and shows only the controls of the shown mode.
    fn layout(&self, hwnd: Hwnd) {
        let created = self.created();
        let controls = &created.controls;
        let metrics = created.metrics.get();
        let client = hwnd.client_size();
        let status_height = match created.status_fit.get() {
            Some((fitted, height)) if fitted == client => height,
            _ => {
                let height = (controls.status)
                    .fit(StatusLayout::min_height(metrics.dpi, metrics.font_height));
                created.status_fit.set(Some((client, height)));
                height
            }
        };
        let (mode, panes) = {
            let model = self.model.borrow();
            (model.mode(), model.panes())
        };
        let layout = Layout::compute(
            client,
            metrics.dpi,
            metrics.font_height,
            status_height,
            &metrics.captions,
            mode,
            &panes,
        );
        let search = layout.search();
        let filters = search.and_then(|search| search.filters.as_ref());
        let show_filters = search.and_then(|search| search.show_filters);
        let transfers = panes.content == crate::core::layout::InspectorContent::Transfers;
        let maximized = panes.inspector && transfers && panes.maximized;
        let placed_inspector = layout.inspector();
        let selection_inspector = placed_inspector.filter(|_| !transfers);
        let strip = [
            controls.search.hwnd(),
            controls.search_tools.hwnd(),
            controls.match_label.hwnd(),
            controls.match_mode.hwnd(),
            controls.list.hwnd(),
        ];
        let mut moves = Vec::new();
        match &layout.body {
            Body::Search(placed) => moves.extend(strip.into_iter().zip([
                placed.search,
                placed.search_tools,
                placed.match_label,
                // A drop-down list's height includes its open list: ten
                // times the box's height fits Match's five modes.
                Rect {
                    height: Px(placed.match_mode.height.0 * 10),
                    ..placed.match_mode
                },
                placed.list,
            ])),
            Body::TwoPanels { panels, key_bar } => {
                for (panel, placed) in controls.panels.iter().zip(panels.iter()) {
                    moves.extend(panel.placed(placed));
                }
                moves.extend(controls.key_bar.map(Button::hwnd).into_iter().zip(*key_bar));
                for (area, index) in KEY_TIPS.into_iter().enumerate() {
                    controls.key_tips.place_area(area, key_bar[index]);
                }
            }
        }
        let pane = &controls.filters;
        if let Some(filters) = filters {
            moves.extend([
                (pane.title.hwnd(), filters.title),
                (pane.tools.hwnd(), filters.tools),
                (pane.pane, filters.pane),
            ]);
        }
        if let Some(show) = show_filters {
            moves.push((pane.show.hwnd(), show));
        }
        let inspector = &controls.inspector;
        let inspector_moved = created
            .layout
            .borrow()
            .as_ref()
            .is_none_or(|previous| previous.inspector() != placed_inspector);
        // The inspector's texts are fitted again only when it moved or its
        // size changed, because fitting them measures each text. Their
        // controls move in this batch with the others (DESIGN.md: Painting).
        let mut inspector_fit = selection_inspector
            .filter(|_| inspector_moved)
            .map(|placed| self.inspector_fit(placed));
        if let Some(fit) = &mut inspector_fit {
            moves.append(&mut fit.moves);
        }
        if let Some(placed) = selection_inspector {
            moves.push((inspector.icon.hwnd(), placed.icon));
            moves.push((inspector.hide.hwnd(), placed.hide));
            moves.push((inspector.copy_path.hwnd(), placed.copy_path));
            moves.extend(
                inspector
                    .buttons
                    .iter()
                    .map(|b| b.hwnd())
                    .zip(placed.buttons),
            );
            for ((label, _), (label_rect, _)) in inspector.fields.iter().zip(&placed.fields) {
                moves.push((label.hwnd(), *label_rect));
            }
            if let Some(note) = placed.note {
                moves.push((inspector.note.hwnd(), note));
            }
        }
        moves.extend(
            controls
                .dividers
                .iter()
                .map(|divider| divider.hwnd())
                .zip(layout.dividers.iter().copied()),
        );
        // The list draws once, after the fill has set its new widths, so a
        // narrower window does not flash its horizontal scroll bar.
        controls.list.hwnd().draw_once_after(|| {
            win::move_children(&moves);
            self.fill_on_resize();
        });
        if let Some(filters) = filters {
            self.place_filters(filters, self.filter_scroll.get());
            pane.kind.fill_column();
            // The paths are shortened to the list's new width.
            self.update_locations();
        }
        for control in strip {
            control.set_visible(search.is_some() && !maximized);
        }
        // A panel's path row and status line show as its kind says
        // (`show_panel`).
        for panel in &controls.panels {
            for control in [panel.drive.hwnd(), panel.list.hwnd()] {
                control.set_visible(search.is_none() && !maximized);
            }
        }
        for button in controls.key_bar {
            button.hwnd().set_visible(search.is_none() && !maximized);
        }
        let shown = filters.is_some() && !maximized;
        for control in [pane.title.hwnd(), pane.tools.hwnd(), pane.pane] {
            control.set_visible(shown);
        }
        pane.show
            .hwnd()
            .set_visible(show_filters.is_some() && !maximized);
        let shown = selection_inspector.is_some();
        inspector.icon.hwnd().set_visible(shown);
        inspector.heading.hwnd().set_visible(shown);
        inspector.hide.hwnd().set_visible(shown);
        inspector.copy_path.hwnd().set_visible(shown);
        for button in inspector.buttons {
            button.hwnd().set_visible(shown);
        }
        for (index, divider) in controls.dividers.iter().enumerate() {
            divider
                .hwnd()
                .set_visible(index < layout.dividers.len() && !maximized);
        }
        let right = self.status_right.borrow().clone();
        let memory = self.status_memory.borrow().clone();
        controls
            .status
            .place_in_window(&layout.status, &right, memory.as_ref());
        controls.status.set_visible(!maximized);
        if let Some(operations) = self.file_operations.get() {
            operations.place(
                placed_inspector.filter(|_| transfers).map(|i| i.area),
                panes.flipped,
                maximized,
            );
        }
        let two_panels = search.is_none();
        created.layout.replace(Some(layout));
        // Each panel's message is placed for its text in its list.
        for side in Side::BOTH {
            self.show_panel(side);
        }
        if maximized {
            for panel in &controls.panels {
                for control in panel.hwnds() {
                    control.set_visible(false);
                }
            }
            controls.message.hwnd().set_visible(false);
            for button in controls.message_buttons {
                button.hwnd().set_visible(false);
            }
        }
        if two_panels {
            // During a drag of the border only Name follows the width,
            // because fitting the other columns measures their texts, as
            // `fill_on_resize` keeps Search's fits.
            if self.column_sizing.borrow().is_dragging() {
                for side in Side::BOTH {
                    self.fill_name_column(side);
                }
            } else {
                self.fill_panel_columns();
            }
        }
        if inspector_moved {
            self.show_field_rows();
            self.show_inspector_fit(inspector_fit);
        }
        self.update_status();
        self.update_message();
        hwnd.invalidate();
    }

    /// Fills the list's width with its columns when the width changed
    /// ([`ColumnSizing::resized`]).
    fn fill_on_resize(&self) {
        let width = self.fill_width();
        let widths = (self.column_sizing.borrow_mut()).resized(width, || self.column_fits());
        if let Some(widths) = widths {
            self.set_column_widths(widths);
        }
    }

    /// The person ended a drag of the window's border or title bar
    /// ([`ColumnSizing::drag_ended`]).
    fn end_sizing(&self) {
        let width = self.fill_width();
        let widths = (self.column_sizing.borrow_mut()).drag_ended(width, || self.column_fits());
        if let Some(widths) = widths {
            let list = self.created().controls.list;
            list.hwnd()
                .draw_once_after(|| self.set_column_widths(widths));
        }
    }

    /// Fills the list's width once the first rows on screen arrive
    /// ([`ColumnSizing::rows_arrived`]).
    fn fill_around_first_rows(&self) {
        let first = self.created().controls.list.visible_rows().start;
        if self
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
        let widths = (self.column_sizing.borrow_mut()).rows_arrived(width, || self.column_fits());
        if let Some(widths) = widths {
            // A list that draws itself paints every row it shows for each
            // width it sets, and its first paints shape every text afresh.
            let list = self.created().controls.list;
            list.hwnd()
                .draw_once_after(|| self.set_column_widths(widths));
        }
    }

    /// The width the list's columns fill.
    fn fill_width(&self) -> Px {
        self.created().controls.list.fill_width()
    }

    /// Gives the columns `widths`, each only when it differs from the
    /// column's, because each width that the list takes draws it again.
    fn set_column_widths(&self, widths: Vec<Px>) {
        let list = self.created().controls.list;
        for (index, width) in widths.into_iter().enumerate() {
            if list.column_width(index) != width {
                list.set_column_width(index, width);
            }
        }
    }

    /// Fits the column at `index` to its header and the rows it shows, as
    /// a double-click on its divider and Fit column do; the others keep
    /// their widths.
    fn fit_column(&self, index: usize) {
        if let Some(fit) = self.fits_of(index..index + 1).first() {
            self.created()
                .controls
                .list
                .set_column_width(index, fit.widest);
        }
    }

    /// Every column's fit, as [`Self::fits_of`] measures it.
    fn column_fits(&self) -> Vec<ColumnFit> {
        self.fits_of(0..self.model.borrow().columns().len())
    }

    /// The fit of each of the shown columns at `indexes`, from its header
    /// and the cells of the rows the list shows and one row above and below
    /// them, which are the rows the person reads and those a small scroll
    /// brings (DESIGN.md: Results list). A column without rows keeps its
    /// first width, except Size, whose samples cover later values too.
    fn fits_of(&self, indexes: Range<usize>) -> Vec<ColumnFit> {
        let created = self.created();
        let list = created.controls.list;
        let dpi = created.metrics.get().dpi;
        let shown = list.visible_rows();
        // The shown rows end with a part row, or with the row below them.
        let below = u64::from(list.shows_part_row());
        let rows = shown.start.saturating_sub(1)..(shown.end + below).min(list.count());
        let label = list.first_label_offset(rows.start).unwrap_or_default();
        // The model is borrowed while the texts are formatted, and the
        // measuring follows.
        let (columns, headers, cells) = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            let strings = model.strings();
            let sizes = model.choices().size_format;
            let results = model.search_list().results();
            let columns = model.columns().get(indexes.clone()).unwrap_or_default();
            let items: Vec<_> = rows
                .filter_map(|row| Some((row, results.row(row)?)))
                .collect();
            let headers: Vec<String> = columns
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
                                    &self.dates.borrow(),
                                    column,
                                    item,
                                ),
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
                ColumnFit::of(plain.plain_width(header), texts, label, first, dpi)
                    .for_column(column)
            })
            .collect()
    }

    /// Places the pane's controls for the scroll position nearest
    /// `position` that `filters` allows; [`Self::layout`] places the pane.
    fn place_filters(&self, filters: &FilterLayout, position: Px) {
        let pane = &self.created().controls.filters;
        let position = filters.scrolled(position, ScrollRequest::To(position));
        self.filter_scroll.set(position);
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
    fn scroll_filters(&self, to: impl FnOnce(&FilterLayout, Px) -> Px) {
        let filters = self
            .created()
            .layout
            .borrow()
            .as_ref()
            .and_then(Layout::search)
            .and_then(|search| search.filters.clone());
        let Some(filters) = filters else { return };
        let position = to(&filters, self.filter_scroll.get());
        if position != self.filter_scroll.get() {
            self.place_filters(&filters, position);
        }
    }

    /// Scrolls the pane so that `control`, which a key focused, shows.
    fn reveal(&self, control: Hwnd) {
        let filters = self
            .created()
            .layout
            .borrow()
            .as_ref()
            .and_then(Layout::search)
            .and_then(|search| search.filters.clone());
        let Some(filters) = filters else { return };
        let placed = self.created().controls.filters.placed(&filters);
        if let Some((_, rect, _)) = placed.into_iter().find(|(c, ..)| *c == control) {
            self.scroll_filters(|filters, position| filters.revealing(position, rect));
        }
    }

    fn on_dpi_changed(&self, hwnd: Hwnd, dpi: Dpi, suggested: Rect) {
        let created = self.created();
        let controls = &created.controls;
        let Ok(fonts) = Fonts::new(dpi) else { return };
        controls.apply_fonts(&fonts);
        drop(created.fonts.replace(fonts));
        self.apply_glyphs(dpi);
        self.column_sizing.borrow_mut().dpi_changed();
        let captions = self.captions();
        let _ = win::menu::replace_menu_bar(hwnd, &captions.menu_bar);
        self.measure(&captions, dpi);
        // The minimum size is in the old DPI's pixels until the layout
        // runs again, so it does not hold the window to the old size.
        created.layout.replace(None);
        hwnd.set_rect(suggested);
        self.layout(hwnd);
    }

    /// Which control of the window `control` is, as the command table asks.
    fn focus_of(&self, control: Option<Hwnd>) -> Focus {
        let controls = &self.created().controls;
        if let Some(value) = control.and_then(|c| controls.inspector.value(c)) {
            return Focus::Value {
                text_selected: value.has_selected_text(),
            };
        }
        match control {
            Some(control) if control == controls.search.hwnd() => Focus::SearchBox {
                text_selected: controls.search.has_selected_text(),
            },
            Some(control) if control == controls.list.hwnd() => Focus::List,
            Some(control) if self.panel_list(control).is_some() => Focus::Panel,
            Some(control) if self.panel_path(control).is_some() => Focus::PathBox,
            Some(control) if self.panel_search(control).is_some() => Focus::PanelSearch,
            // An open list takes Enter itself, to close.
            Some(control)
                if self
                    .panel_look_in(control)
                    .is_some_and(|side| !self.panel(side).look_in.is_open()) =>
            {
                Focus::LookIn
            }
            // An open list takes Enter itself, to close.
            Some(control)
                if controls
                    .filters
                    .range_of(control)
                    .is_some_and(|range| !controls.filters.range(range).is_open()) =>
            {
                Focus::RangeFilter
            }
            _ => Focus::Other,
        }
    }

    fn focus(&self) -> Focus {
        self.focus_of(Hwnd::focused())
    }

    /// The inspector value that has the focus, if one has it.
    fn focused_value(&self) -> Option<Edit> {
        Hwnd::focused().and_then(|focused| self.created().controls.inspector.value(focused))
    }

    /// Each item's ID, whether it is enabled and whether it is checked, from
    /// the command table.
    fn item_states(&self, items: &[MenuItem]) -> Vec<(u16, bool, bool)> {
        self.item_states_for(items, self.command_context())
    }

    fn item_states_for(
        &self,
        items: &[MenuItem],
        context: selection::CommandContext,
    ) -> Vec<(u16, bool, bool)> {
        let focus = context.focus;
        let model = self.model.borrow();
        let pending = self.selection_command_pending(context.list);
        let state = State {
            model: &model,
            focus,
            list: context.list,
        };
        items
            .iter()
            .map(|item| {
                (
                    item.id(),
                    item.enabled(&state)
                        && !(pending
                            && (item.reads_selection(focus) || item.writes_selection(focus))),
                    item.checked(&state),
                )
            })
            .collect()
    }

    /// Sets every menu item's enabled state and mark from the command
    /// table, after the selection of the action that opened the menu is
    /// read.
    fn update_menu(&self, hwnd: Hwnd) {
        self.read_list_selection(hwnd);
        self.read_panel_selection();
        let Some(bar) = win::menu::Menu::of(hwnd) else {
            return;
        };
        let mode = self.model.borrow().mode();
        let items = commands::items(&commands::menu_bar(mode));
        for (id, enabled, checked) in self.item_states(&items) {
            bar.set_state(id, enabled, checked);
        }
    }

    /// Runs one menu item or shortcut; every surface calls this (DESIGN.md:
    /// window rule 9). It asks the command table again whether the item is
    /// enabled, because the state can change while a menu is open (DESIGN.md:
    /// T11).
    fn run(&self, hwnd: Hwnd, item: MenuItem) {
        let context = self.command_context();
        self.run_in_context(hwnd, item, context);
    }

    fn run_in_context(&self, hwnd: Hwnd, item: MenuItem, context: selection::CommandContext) {
        if !self.selection_context_current(context) {
            return;
        }
        let writing = item.writes_selection(context.focus)
            && (self.native_update_active(context.list)
                || self.native_deselection_pending(context.list)
                || self.selection_command_pending(context.list));
        if (item.reads_selection(context.focus) || writing)
            && !self.prepare_selection_command(hwnd, item, context)
        {
            return;
        }
        self.run_command(hwnd, item, context);
    }

    fn run_command(&self, hwnd: Hwnd, item: MenuItem, context: selection::CommandContext) {
        if !self.selection_context_current(context) {
            return;
        }
        let selection::CommandContext {
            mode, list, focus, ..
        } = context;
        let enabled = {
            let model = self.model.borrow();
            item.enabled(&State {
                model: &model,
                focus,
                list,
            })
        };
        if !enabled {
            return;
        }
        if mode == Mode::TwoPanels {
            // Any key or command closes the box first, so F5 after a
            // quick search copies the focused row.
            self.close_quick_search();
        }
        let active = match list {
            List::Panel(side) => side,
            List::Search => self.model.borrow().panels().active(),
        };
        let controls = &self.created().controls;
        let accepted = match item {
            MenuItem::Run(command) => match command {
                CommandId::Exit => {
                    self.exit(hwnd);
                    return;
                }
                CommandId::About => {
                    let about = {
                        let model = self.model.borrow();
                        About::new(self, model.strings(), model.service_version())
                    };
                    self.open_dialog(hwnd, DialogId::About, &about);
                    return;
                }
                CommandId::Open if mode == Mode::TwoPanels => {
                    return self.open_panel_entries(active);
                }
                CommandId::Open => return self.act_on_selected(list, EntryAction::Open),
                CommandId::OpenFileLocation => {
                    return self.act_on_selected(list, EntryAction::OpenFileLocation);
                }
                CommandId::Explore => return self.act_on_selected(list, EntryAction::Explore),
                CommandId::Properties => {
                    return self.act_on_selected(list, EntryAction::Properties);
                }
                CommandId::ExportResults => return self.export(hwnd),
                CommandId::Cancel => {
                    self.created().work.borrow_mut().cancel_pairing();
                    self.model.borrow_mut().cancel_job()
                }
                CommandId::Copy => match focus {
                    Focus::SearchBox { .. } => {
                        self.cancel_copy(hwnd);
                        return controls.search.copy();
                    }
                    Focus::Value { .. } => {
                        self.cancel_copy(hwnd);
                        if let Some(value) = self.focused_value() {
                            value.copy();
                        }
                        return;
                    }
                    Focus::List
                    | Focus::RangeFilter
                    | Focus::Panel
                    | Focus::PathBox
                    | Focus::PanelSearch
                    | Focus::LookIn
                    | Focus::Other => {
                        return self.copy(hwnd, ClipboardFormat::Files);
                    }
                },
                CommandId::CopyAsPath => return self.copy(hwnd, ClipboardFormat::Paths),
                CommandId::CopyName => return self.copy(hwnd, ClipboardFormat::Names),
                CommandId::CopyFolder => return self.copy(hwnd, ClipboardFormat::Folders),
                CommandId::SelectAll => match focus {
                    Focus::SearchBox { .. } => return controls.search.select_all(),
                    Focus::Value { .. } => {
                        if let Some(value) = self.focused_value() {
                            value.select_all();
                        }
                        return;
                    }
                    Focus::PathBox => return self.panel(active).path.select_all(),
                    Focus::PanelSearch => return self.panel(active).search.select_all(),
                    Focus::List
                    | Focus::RangeFilter
                    | Focus::Panel
                    | Focus::LookIn
                    | Focus::Other => {
                        return match mode {
                            Mode::Search => controls.list.select_all(),
                            // `..` is let go when the selection is read.
                            Mode::TwoPanels => self.panel(active).list.select_all(),
                        };
                    }
                },
                CommandId::ClearSelection => return self.panel(active).list.deselect_all(),
                CommandId::Find => {
                    if mode == Mode::TwoPanels {
                        return self.find_in_panel(hwnd, active);
                    }
                    controls.search.hwnd().focus();
                    return controls.search.select_all();
                }
                CommandId::Refresh => {
                    return match mode {
                        Mode::Search => {
                            self.created().work.borrow_mut().refresh_location();
                            self.search(hwnd, List::Search);
                        }
                        Mode::TwoPanels => self.refresh_panel(hwnd, active),
                    };
                }
                CommandId::SearchSyntax => {
                    self.open_dialog(hwnd, DialogId::SearchSyntax, &SearchSyntax);
                    return;
                }
                CommandId::SearchInThisFolder => self.model.borrow_mut().search_in_this_folder(),
                CommandId::Filters => self.model.borrow_mut().toggle_filter_pane(),
                CommandId::Inspector => self.model.borrow_mut().toggle_inspector(),
                CommandId::ClearFilters => self.model.borrow_mut().clear_filters(),
                CommandId::Settings => return self.open_settings(hwnd),
                CommandId::ShowWindow => return self.show_window(hwnd),
                CommandId::CopyToFolder | CommandId::MoveToFolder if mode == Mode::TwoPanels => {
                    return self.transfer(hwnd, command == CommandId::MoveToFolder, active);
                }
                CommandId::CopyToFolder => return self.copy_to_folder(hwnd, false, list),
                CommandId::MoveToFolder => return self.copy_to_folder(hwnd, true, list),
                CommandId::Synchronize => return self.synchronize_folders(hwnd, None),
                CommandId::Delete => return self.delete_selected(hwnd, list),
                CommandId::NewFolder => return self.new_folder(hwnd, active),
                CommandId::View => return self.view_focused(hwnd, active),
                // Not available in this version (DESIGN.md: Commands in Two
                // panels).
                CommandId::Edit => return,
                CommandId::Cut => return self.copy(hwnd, ClipboardFormat::CutFiles),
                CommandId::Paste => return self.paste(),
                CommandId::FileOperations => return self.show_operations(None),
                CommandId::TwoPanels => {
                    let mut model = self.model.borrow_mut();
                    let other = match model.mode() {
                        Mode::Search => Mode::TwoPanels,
                        Mode::TwoPanels => Mode::Search,
                    };
                    model.show_mode(other)
                }
                CommandId::Columns | CommandId::Language | CommandId::Match | CommandId::SortBy => {
                    return;
                }
            },
            MenuItem::Column(column) => self.model.borrow_mut().toggle_column(column),
            MenuItem::FitColumn(column) => {
                let index = self
                    .model
                    .borrow()
                    .columns()
                    .iter()
                    .position(|c| *c == column);
                if let Some(index) = index {
                    self.fit_column(index);
                }
                return;
            }
            MenuItem::FitColumns => {
                for (index, fit) in self.column_fits().into_iter().enumerate() {
                    self.created()
                        .controls
                        .list
                        .set_column_width(index, fit.widest);
                }
                return;
            }
            MenuItem::SortKey(key) if mode == Mode::TwoPanels => {
                return self.change_panel_sort(hwnd, active, Some(key), None);
            }
            MenuItem::SortDirection(dir) if mode == Mode::TwoPanels => {
                return self.change_panel_sort(hwnd, active, None, Some(dir));
            }
            MenuItem::SortKey(key) => {
                let mut model = self.model.borrow_mut();
                let sort = model.key_sort(key);
                model.sort_by(sort)
            }
            MenuItem::SortDirection(dir) => {
                let mut model = self.model.borrow_mut();
                let key = model.sort().key;
                model.sort_by(SortSpec { key, dir })
            }
            MenuItem::Panel(action) => return self.run_panel_action(hwnd, action),
            MenuItem::Language(place) => {
                hwnd.draw_once_after(|| self.set_language(hwnd, LanguageChoice::at(place)));
                self.save_settings(true);
                return;
            }
        };
        self.apply(hwnd, accepted);
    }

    /// Runs the search that `list`'s text and controls ask for now. Only
    /// Search's Location uses its last background observation; a search panel's
    /// folder is the one the panel just showed.
    fn search(&self, hwnd: Hwnd, list: List) {
        hwnd.kill_timer(timer::search(list));
        let start = QueryStart {
            now: jiff::Timestamp::now(),
            time_zone: jiff::tz::TimeZone::system(),
        };
        let location = match list {
            List::Search => self.model.borrow().location().map(str::to_owned),
            List::Panel(_) => None,
        };
        let scope = match list {
            List::Search => self.created().work.borrow_mut().scope(location.as_deref()),
            List::Panel(_) => ScopeCheck::Exists,
        };
        let accepted = self.model.borrow_mut().start_search(list, &start, scope);
        if accepted.change == Change::Searching(list) {
            hwnd.set_timer(timer::searching(list), SEARCHING_NAMED_MS);
        }
        self.apply(hwnd, accepted);
    }

    /// Names `list`'s search as running, once it has run
    /// [`SEARCHING_NAMED_MS`] without an answer.
    fn name_search(&self, hwnd: Hwnd, list: List) {
        hwnd.kill_timer(timer::searching(list));
        let accepted = self.model.borrow_mut().name_search(list);
        self.apply(hwnd, accepted);
    }

    /// Runs the dialog `dialog` with `state`. A dialog that Windows cannot
    /// open leaves a line in `ui-error.log` and the window as it was.
    fn open_dialog<D: DialogProc>(
        &self,
        hwnd: Hwnd,
        dialog: DialogId,
        state: &D,
    ) -> Option<DialogEnd> {
        let captions = DialogCaptions::new(dialog, self.model.borrow().strings());
        match win::dialog::run_modal(hwnd, dialog, &captions, state) {
            Ok(end) => Some(end),
            Err(error) => {
                super::write_log(
                    &self.settings_file.with_file_name("ui-error.log"),
                    &format!("The {dialog:?} dialog did not open: {error}"),
                );
                None
            }
        }
    }

    /// Changes the filters to what `change` makes of them.
    fn change_filters(&self, hwnd: Hwnd, change: impl FnOnce(&mut Filters)) {
        let mut filters = self.model.borrow().filters().clone();
        change(&mut filters);
        let accepted = self.model.borrow_mut().set_filters(filters);
        self.apply(hwnd, accepted);
    }

    /// Asks for a folder with the Windows folder picker and sets Location
    /// to it; Cancel keeps the Location (DESIGN.md: Filter pane).
    fn choose_folder(&self, hwnd: Hwnd) {
        let start = self.model.borrow().location().map(str::to_owned);
        let Some(folder) = win::shell::pick_folder(hwnd, start.as_deref()) else {
            return;
        };
        let accepted = self.model.borrow_mut().choose_folder(folder);
        self.apply(hwnd, accepted);
    }

    /// Opens the custom range dialog of `range` when its list shows
    /// Custom…; OK sets the range, and Cancel shows the previous choice
    /// again (DESIGN.md: Filter pane).
    fn choose_custom(&self, hwnd: Hwnd, range: RangeFilter) {
        let custom = {
            let model = self.model.borrow();
            let filters = model.filters();
            match range {
                RangeFilter::Date => date_items(filters.date).len(),
                RangeFilter::Size => size_items(filters.size).len(),
            }
        };
        let list = self.created().controls.filters.range(range);
        if list.selected() != Some(custom - 1) {
            return;
        }
        match range {
            RangeFilter::Date => {
                let dialog = {
                    let model = self.model.borrow();
                    let shown = match model.filters().date {
                        DateModified::Custom(range) => Some(range),
                        _ => None,
                    };
                    CustomDate::new(model.strings(), shown)
                };
                if self.open_dialog(hwnd, DialogId::CustomDate, &dialog) == Some(DialogEnd::Ok)
                    && let Some(chosen) = dialog.chosen()
                {
                    self.change_filters(hwnd, |f| f.date = DateModified::Custom(chosen));
                }
            }
            RangeFilter::Size => {
                let dialog = {
                    let model = self.model.borrow();
                    let shown = match model.filters().size {
                        SizeFilter::Custom(range) => Some(range),
                        _ => None,
                    };
                    let numbers = self.numbers.borrow();
                    CustomSize::new(model.strings(), numbers.decimal_separator(), shown)
                };
                if self.open_dialog(hwnd, DialogId::CustomSize, &dialog) == Some(DialogEnd::Ok)
                    && let Some(chosen) = dialog.chosen()
                {
                    self.change_filters(hwnd, |f| f.size = SizeFilter::Custom(chosen));
                }
            }
        }
        // After Cancel, the list shows Custom… and the filter is the one
        // before it.
        self.update_filters();
    }

    /// Asks for status unless a request is in flight.
    fn ask_status(&self) {
        let fetch = self.model.borrow_mut().status_needed();
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    /// While the status bar shows memory, reads the app's own working set,
    /// one cheap call, to show beside the service's from the same status
    /// (DESIGN.md: Status bar).
    fn read_app_memory(&self) {
        let mut model = self.model.borrow_mut();
        if model.choices().show_memory {
            let working_set = gozler_winfs::self_memory().ok().map(|own| own.working_set);
            model.set_app_working_set(working_set);
        }
    }

    fn start(&self, fetch: Fetch) {
        let mut work = self.created().work.borrow_mut();
        if fetch.purpose() == Purpose::Status {
            work.programs.refresh();
        }
        work.requests.start(fetch);
        let retired = work.requests.take_retired();
        work.retire(retired);
    }

    fn cancel_request(&self, purpose: Purpose) {
        let mut work = self.created().work.borrow_mut();
        work.requests.cancel(purpose);
        let retired = work.requests.take_retired();
        work.retire(retired);
    }

    /// Asks for the chunk that `list`'s rows on screen still lack, so every
    /// chunk request comes from the rows visible now (DESIGN.md: Rows on
    /// demand).
    fn fetch_visible(&self, list: List) {
        let visible = self.list_control(list).visible_rows();
        let fetch = self.model.borrow_mut().rows_needed(list, visible);
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    /// The control that shows `list`.
    fn list_control(&self, list: List) -> ListView {
        match list {
            List::Search => self.created().controls.list,
            List::Panel(side) => self.panel(side).list,
        }
    }

    /// The selection timer of `list` ran out: its selected rows are asked
    /// for.
    fn fetch_selection(&self, hwnd: Hwnd, list: List) {
        hwnd.kill_timer(timer::selection(list));
        let fetch = self.model.borrow_mut().read_selection(list);
        if let Some(fetch) = fetch {
            self.start(fetch);
        }
    }

    /// Updates the controls for what a reply or an input changed, and starts
    /// the request it makes necessary.
    fn apply(&self, hwnd: Hwnd, accepted: Accepted) {
        self.retire_model();
        self.start_list_passes();
        let created = self.created();
        let controls = &created.controls;
        match accepted.then {
            Some(Then::Fetch(fetch)) => self.start(fetch),
            Some(Then::SearchAgain(lists)) => {
                for list in lists {
                    self.search(hwnd, list);
                }
            }
            Some(Then::SearchSoon(list)) => hwnd.set_timer(timer::search(list), SEARCH_DELAY_MS),
            Some(Then::ReadSelectionSoon(list)) => {
                self.cancel_request(Purpose::Selection(list));
                hwnd.set_timer(timer::selection(list), SELECTION_DELAY_MS);
            }
            Some(Then::Cancel(purpose)) => self.cancel_request(purpose),
            Some(Then::AskLatestRelease) => self.ask_latest_release(),
            Some(Then::RunFix(input)) => {
                let mut work = created.work.borrow_mut();
                work.fixes.start(input);
                let retired = work.fixes.take_retired();
                work.retire(retired);
            }
            Some(Then::ConfirmSetup) => self.defer(hwnd, ModalStep::ConfirmSetup),
            Some(Then::ExplainNoSetup) => self.defer(hwnd, ModalStep::ExplainNoSetup),
            Some(Then::StopFix) => {
                let mut work = created.work.borrow_mut();
                work.fixes.stop();
                let retired = work.fixes.take_retired();
                work.retire(retired);
            }
            Some(Then::CancelOperation(id)) => self.act_operation(id, QueueAction::Cancel),
            Some(Then::RunSetup(setup)) => self.run_setup(setup),
            None => {}
        }
        match accepted.change {
            Change::Nothing | Change::Choice | Change::FolderCounted => {}
            Change::Filters => self.update_filters(),
            Change::Panes => {
                let panes = self.model.borrow().panes();
                let maximized = panes.inspector
                    && panes.maximized
                    && panes.content == crate::core::layout::InspectorContent::Transfers;
                let was_maximized = created
                    .layout
                    .borrow()
                    .as_ref()
                    .and_then(Layout::inspector)
                    .is_some_and(|placed| placed.splitter.height == Px(0));
                let hidden_focus = Hwnd::focused().is_some_and(|focused| {
                    (!panes.filters && controls.filters.contains(focused))
                        || ((!panes.inspector
                            || panes.content != crate::core::layout::InspectorContent::Selection)
                            && controls.inspector.contains(focused))
                        || ((!panes.inspector
                            || panes.content != crate::core::layout::InspectorContent::Transfers)
                            && self
                                .file_operations
                                .get()
                                .is_some_and(|ops| ops.contains(focused)))
                });
                self.layout(hwnd);
                if maximized {
                    if Hwnd::focused().is_none_or(|focus| {
                        !self
                            .file_operations
                            .get()
                            .is_some_and(|ops| ops.contains(focus))
                    }) && let Some(operations) = self.file_operations.get()
                    {
                        operations.focus();
                    }
                } else if !panes.inspector || was_maximized || hidden_focus {
                    let previous = self.inspector_focus.take();
                    if hidden_focus || was_maximized {
                        previous
                            .filter(|focus| focus.is_visible())
                            .unwrap_or_else(|| self.home())
                            .focus();
                    }
                }
                self.update_status();
            }
            Change::Mode => {
                self.replace_menu_bar(hwnd);
                hwnd.set_text(&captions::title(&self.model.borrow()));
                let mode = self.model.borrow().mode();
                match mode {
                    Mode::TwoPanels => self.start_panels(hwnd),
                    Mode::Search => self.stop_panels(hwnd),
                }
                self.layout(hwnd);
                // The focused control may have hidden, and a hidden control
                // keeps the focus but takes no key.
                self.home().focus();
                self.update_status();
            }
            Change::Typed => hwnd.set_text(&captions::title(&self.model.borrow())),
            Change::ColumnShown { index } => {
                let (column, header) = {
                    let model = self.model.borrow();
                    let column = model.columns()[index];
                    (column, captions::header(model.strings(), column))
                };
                let width = (self.column_sizing.borrow_mut()).column_shown(column, hwnd.dpi());
                controls.list.insert_column(
                    index,
                    &ListColumn {
                        header: &header,
                        width,
                        right_aligned: column.row().right_aligned,
                    },
                );
            }
            Change::ColumnHidden { index, column } => {
                let width = controls.list.column_width(index);
                (self.column_sizing.borrow_mut()).column_hidden(column, width, hwnd.dpi());
                controls.list.delete_column(index);
            }
            // A status also changes the failure that each list shows.
            Change::Status => {
                self.read_app_memory();
                self.update_status();
                self.update_locations();
                self.update_message();
                if self.model.borrow().mode() == Mode::TwoPanels {
                    for side in Side::BOTH {
                        self.show_panel(side);
                    }
                }
            }
            Change::Searching(List::Search) | Change::Selection(List::Search) | Change::Export => {
                self.update_status();
            }
            Change::Searching(List::Panel(side)) | Change::Selection(List::Panel(side)) => {
                self.show_panel(side);
                self.update_status();
            }
            Change::Fix => {
                self.update_status();
                self.update_message();
                self.post_settings_reply();
            }
            Change::NewSearch {
                list: List::Search,
                count,
                enter_first,
            } => {
                // A new search shows its results from the top, with nothing
                // selected (DESIGN.md: T2).
                controls.list.select_only(None);
                controls.list.set_count(count);
                self.seed_native_selection(List::Search, Vec::new().into());
                if count > 0 {
                    controls.list.ensure_visible(0);
                }
                self.cancel_request(Purpose::Selection(List::Search));
                // The new rows no longer hold what the ended deletes and
                // moves took, and show again what they failed on.
                self.release_operations();
                self.update_status();
                self.update_message();
                self.fetch_visible(List::Search);
                self.fill_around_first_rows();
                if enter_first {
                    let shown = self
                        .model
                        .borrow()
                        .search_list()
                        .results()
                        .shown_ask()
                        .cloned();
                    if let Some(ask) = shown {
                        self.defer(hwnd, ModalStep::EnterFirst(ask));
                    }
                }
            }
            // A search panel's new results show from the top, with the
            // focus on the first row and nothing selected.
            Change::NewSearch {
                list: list @ List::Panel(side),
                count,
                ..
            } => {
                let control = self.panel(side).list;
                control.select_only(None);
                control.set_count(count);
                self.seed_native_selection(list, Vec::new().into());
                if count > 0 {
                    control.set_focused(Some(0));
                }
                self.cancel_request(Purpose::Selection(list));
                self.release_operations();
                self.show_panel(side);
                self.update_status();
                self.fetch_visible(list);
            }
            Change::Rows { list, rows, count } => {
                let control = self.list_control(list);
                if control.count() == count {
                    control.redraw_rows(rows);
                } else {
                    control.set_count(count);
                }
                match list {
                    List::Search => self.update_message(),
                    List::Panel(side) => self.show_panel(side),
                }
                self.update_status();
                self.fetch_visible(list);
                if list == List::Search {
                    self.fill_around_first_rows();
                }
            }
            Change::Message(list) => {
                let control = self.list_control(list);
                control.select_only(None);
                control.set_count(0);
                self.seed_native_selection(list, Vec::new().into());
                self.cancel_request(Purpose::Selection(list));
                match list {
                    List::Search => self.update_message(),
                    List::Panel(side) => self.show_panel(side),
                }
                self.update_status();
            }
            Change::SelectionRead { list, rows } => {
                self.list_control(list).redraw_rows(rows);
                if let List::Panel(side) = list {
                    self.show_panel(side);
                }
                self.update_status();
                // A result snapshot the read made emptied the cache.
                self.fetch_visible(list);
            }
            Change::SelectionFiltered { list, deselect } => {
                self.show_list_taken(list, deselect);
                if let List::Panel(side) = list {
                    self.show_panel(side);
                }
                self.update_status();
            }
            Change::ExportFailed { path, error } => {
                self.update_status();
                self.defer(hwnd, ModalStep::ExportFailed { path, error });
            }
            Change::Settings => {
                self.replace_menu_bar(hwnd);
                self.update_tray(hwnd);
            }
            Change::TotalCommander(found) => {
                self.total_commander.replace(Some(found));
                self.post_settings_reply();
            }
            Change::IncludedCount => self.post_settings_reply(),
            Change::FolderSizes(side) => self.show_panel_sizes(side),
        }
        self.update_bound_controls();
        self.update_inspector(hwnd);
        self.update_poll(hwnd);
        self.note_saved_state(hwnd);
    }

    fn retire_model(&self) {
        let mut retired = self.model.borrow_mut().take_retired();
        let mut work = self.created().work.borrow_mut();
        retired.append(work.folders.take_retired());
        retired.append(work.indexed.take_retired());
        work.retire(retired);
    }

    pub(super) fn indexed(&self) -> std::cell::RefMut<'_, super::work::indexed::Work> {
        std::cell::RefMut::map(self.created().work.borrow_mut(), |work| &mut work.indexed)
    }

    /// Starts the save timer when the state that `ui.json` saves changed
    /// (DESIGN.md: Where settings are saved). It runs after each change
    /// that can touch that state: the end of [`Self::apply`] and of the
    /// panels' events, each panel command that changes a folder, a sort or
    /// the active panel, a move of the window, and [`Message::Deferred`],
    /// which reads a header drag and what a Viewer changed.
    pub(super) fn note_saved_state(&self, hwnd: Hwnd) {
        let changed = {
            let mut model = self.model.borrow_mut();
            model.set_viewer(self.viewers.state());
            model.saved_state_changed()
        };
        if changed {
            hwnd.set_timer(timer::SAVE, SAVE_DELAY_MS);
        }
    }

    /// Runs `step` from [`Message::Deferred`], after every reply and panel
    /// event of this wake has applied (DESIGN.md: window rule 13).
    fn defer(&self, hwnd: Hwnd, step: ModalStep) {
        self.modal_steps.borrow_mut().push_back(step);
        hwnd.post(DEFERRED);
    }

    /// Runs the deferred modal steps one at a time. A step's own loop
    /// dispatches wakes, whose steps wait here for it to end, so no dialog
    /// opens over another.
    fn run_modal_steps(&self, hwnd: Hwnd) {
        if self.delivering_operations.get() || self.operation_delivery.borrow().is_some() {
            return;
        }
        if self.created().work.borrow().preparing_report() {
            return;
        }
        if self.modal_running.replace(true) {
            return;
        }
        loop {
            let step = self.modal_steps.borrow_mut().pop_front();
            let Some(step) = step else { break };
            match step {
                ModalStep::ConfirmSetup => self.confirm_setup(hwnd),
                ModalStep::ExplainNoSetup => self.explain_no_setup(hwnd),
                ModalStep::ExportFailed { path, error } => {
                    self.show_export_failure(hwnd, &path, &error);
                }
                ModalStep::SettingsNotSaved(error) if !self.ending_session.get() => {
                    let text = self.model.borrow().strings().format(
                        "message.settings_not_saved",
                        &[
                            &self.settings_file.display().to_string(),
                            &win::error_text(&error),
                        ],
                    );
                    self.error_box(hwnd, &text);
                }
                ModalStep::SettingsNotSaved(_) => {}
                ModalStep::CopyFailed(id, failure) => {
                    if !self.created().work.borrow().copy_is_current(id) {
                        continue;
                    }
                    let text = match failure {
                        CopyFailure::Busy => self
                            .model
                            .borrow()
                            .strings()
                            .text("message.clipboard_busy")
                            .to_owned(),
                        CopyFailure::Native(error) => error.message(),
                    };
                    self.error_box(hwnd, &text);
                }
                ModalStep::PreparedPaths(purpose, id, prepared) => {
                    if !self.created().work.borrow().paths_are_current(purpose, id) {
                        self.retire_paths(prepared.paths);
                        continue;
                    }
                    match prepared.action {
                        PathAction::Transfer { moves } => {
                            self.copy_paths_to_folder(hwnd, moves, prepared.paths);
                        }
                        PathAction::PanelTransfer {
                            moves,
                            place,
                            selected,
                            start,
                        } => {
                            self.transfer_paths(
                                hwnd,
                                moves,
                                place,
                                selected,
                                *start,
                                prepared.paths,
                            );
                        }
                        PathAction::Delete { place } => {
                            self.delete_paths(hwnd, place, prepared.paths)
                        }
                    }
                }
                ModalStep::OpenSettings(id, sign_in) => {
                    if self.created().work.borrow().registry_is_current(id)
                        && self.settings.get().is_none()
                    {
                        self.show_settings(hwnd, sign_in);
                    }
                }
                ModalStep::Opened(purpose, id, opened) => {
                    if !self.created().work.borrow().opening_is_current(purpose, id) {
                        continue;
                    }
                    if purpose == ShellPurpose::Setup {
                        match opened.failure {
                            Some(failure) => {
                                let failed = SetupResult::Failed(win::error_text(&failure.error));
                                let event = FixEvent::Setup(SetupEvent::End(failed));
                                let accepted = self.model.borrow_mut().accept_fix(event);
                                self.apply(hwnd, accepted);
                            }
                            None => {
                                // Setup waits for Gozler to close. Native modal
                                // loops must unwind before its HWND is destroyed.
                                self.closing_for_setup.set(true);
                                win::window::quit_modal_loops();
                            }
                        }
                        continue;
                    }
                    if let Some(failure) = opened.failure {
                        let reason = win::error_text(&failure.error);
                        let text = {
                            let model = self.model.borrow();
                            match failure.target {
                                OpenTarget::Command(program) => model
                                    .strings()
                                    .format("message.open_failed", &[&program, &reason]),
                                OpenTarget::Path(path) => model.strings().format(
                                    "panel.cannot_open",
                                    &[&String::from_utf16_lossy(&path), &reason],
                                ),
                            }
                        };
                        self.error_box(self.fix_owner(hwnd), &text);
                    }
                    if self.created().work.borrow().opening_is_current(purpose, id) {
                        self.report_missing(hwnd, opened.missing);
                    }
                }
                ModalStep::Paired(id, _, _)
                    if !self.created().work.borrow().pairing_is_current(id) => {}
                ModalStep::Paired(_, local, true) => self.start_fix(hwnd, &Fix::UseLocal(local)),
                ModalStep::Paired(_, _, false) => {
                    let (text, title) = {
                        let model = self.model.borrow();
                        let text = |key: &str| model.strings().text(key).to_owned();
                        (
                            text("fixing.use_local_not_paired"),
                            text("fixing.use_local_caption"),
                        )
                    };
                    win::dialog::error_box(Some(self.fix_owner(hwnd)), &text, &title);
                }
                ModalStep::PanelRefused(refused) => self.refuse(hwnd, refused),
                ModalStep::EnterFirst(ask) => {
                    let shown =
                        self.model.borrow().search_list().results().shown_ask() == Some(&ask);
                    if shown {
                        self.enter_first_or_selected(hwnd);
                    }
                }
                ModalStep::Operation(dialog) => {
                    if self.operation_dialog(hwnd, dialog) {
                        break;
                    }
                }
                ModalStep::PreparedFailure(prepared) => self.show_prepared_failure(hwnd, prepared),
                ModalStep::Synchronize(id) => self.synchronize_folders(hwnd, Some(id)),
            }
        }
        self.modal_running.set(false);
    }

    /// Builds the menu bar and the accelerator table again, for the shown
    /// mode and the person's shortcuts.
    fn replace_menu_bar(&self, hwnd: Hwnd) {
        let (menu_bar, shortcuts, mode) = {
            let model = self.model.borrow();
            let shortcuts = &model.choices().shortcuts;
            let menu_bar = commands::render(
                &commands::menu_bar(model.mode()),
                model.strings(),
                shortcuts,
                model.mode(),
            );
            (menu_bar, shortcuts.clone(), model.mode())
        };
        let _ = win::menu::replace_menu_bar(hwnd, &menu_bar);
        if let Ok(table) = accelerator_table(&shortcuts, mode) {
            self.accelerators.replace(Rc::new(table));
        }
    }

    /// Starts or stops the one status timer as the model says (DESIGN.md:
    /// Requests).
    fn update_poll(&self, hwnd: Hwnd) {
        let on_screen = hwnd.is_on_screen();
        let polls = self.model.borrow().polls_status(on_screen);
        if polls != self.polling.replace(polls) {
            if polls {
                hwnd.set_timer(timer::STATUS, STATUS_POLL_MS);
            } else {
                hwnd.kill_timer(timer::STATUS);
            }
        }
    }

    /// The window was shown, hidden, minimized or restored, which starts or
    /// stops the memory group's figures; coming on screen asks for fresh
    /// ones at once.
    fn follow_visibility(&self, hwnd: Hwnd) {
        self.follow_dates(hwnd);
        let polled = self.polling.get();
        self.update_poll(hwnd);
        if self.polling.get() && !polled {
            self.ask_status();
        }
    }

    fn follow_dates(&self, hwnd: Hwnd) {
        if self.dates.borrow().format == DateFormat::Relative && hwnd.is_on_screen() {
            self.refresh_dates();
            hwnd.set_timer(timer::DATES, AGE_REFRESH_MS);
        } else {
            hwnd.kill_timer(timer::DATES);
        }
    }

    fn refresh_dates(&self) {
        self.dates.borrow_mut().set_time(jiff::Zoned::now());
        let mode = self.model.borrow().mode();
        match mode {
            Mode::Search => {
                let list = self.created().controls.list;
                list.redraw_rows(list.visible_rows());
            }
            Mode::TwoPanels => {
                for side in Side::BOTH {
                    let list = self.panel(side).list;
                    list.redraw_rows(list.visible_rows());
                }
            }
        }
    }

    /// Accepts every reply that has arrived, and the newer release that the
    /// update check found.
    fn on_wake(&self, hwnd: Hwnd) {
        self.finish_shortcut_start(hwnd);
        self.post_settings_reply();
        {
            let mut work = self.created().work.borrow_mut();
            work.indexed.poll();
            let retired = work.indexed.take_retired();
            work.retire(retired);
        }
        self.accept_told_selections(hwnd);
        self.apply_taken_deselections(hwnd);
        self.read_list_selection(hwnd);
        self.read_panel_selection();
        let report = self.created().work.borrow_mut().take_report();
        if let Some(report) = report {
            self.modal_steps
                .borrow_mut()
                .push_front(ModalStep::PreparedFailure(report));
            hwnd.post(DEFERRED);
        }
        let followed = self.created().work.borrow_mut().take_followed();
        if let Some(answer) = followed {
            let steps = self.model.borrow_mut().accept_followed(answer);
            self.apply_operation_steps(hwnd, steps);
        }
        let panels = self.created().work.borrow_mut().take_panels();
        for (side, answer) in panels {
            let update = self
                .model
                .borrow_mut()
                .panels_mut()
                .accept_calculation(side, answer);
            if let Some(update) = update {
                self.apply_panel_update(hwnd, side, update);
            }
            self.retire_model();
        }
        let counted = self.created().work.borrow_mut().take_panel_counts();
        for (side, answer) in counted {
            let current = self
                .model
                .borrow_mut()
                .panels_mut()
                .accept_count(side, answer);
            if current && self.model.borrow().mode() == Mode::TwoPanels {
                self.show_panel(side);
            }
            self.retire_model();
        }
        let found = self.created().work.borrow_mut().take_quick();
        for (side, answer) in found {
            let row = self
                .model
                .borrow_mut()
                .panels_mut()
                .accept_quick(side, answer);
            if let Some(row) = row {
                self.quick_go(side, row);
            }
            self.show_panel(side);
            self.retire_model();
        }
        self.start_list_passes();
        let kept = self.created().work.borrow_mut().take_kept();
        if !kept.is_empty() {
            for (list, answer) in kept {
                let accepted = self.model.borrow_mut().accept_kept(list, answer);
                self.apply(hwnd, accepted);
            }
            self.release_operations();
        }
        let filtered = self.created().work.borrow_mut().take_filtered();
        for (list, answer) in filtered {
            let accepted = self.model.borrow_mut().accept_filtered(list, answer);
            self.apply(hwnd, accepted);
        }
        let restored = self.created().work.borrow_mut().take_queue();
        if let Some(restored) = restored {
            self.adopt_queue(hwnd, restored);
        }
        let location_changed = self.created().work.borrow_mut().take_location();
        if location_changed {
            self.search(hwnd, List::Search);
        }
        let labels_changed = self.created().work.borrow_mut().take_labels();
        if labels_changed {
            self.post_settings_reply();
        }
        let copied = self.created().work.borrow_mut().take_copied();
        match copied {
            Some(CopyNotice::Retry) => hwnd.set_timer(
                timer::CLIPBOARD,
                crate::core::clipboard::RETRY_AFTER.as_millis() as u32,
            ),
            Some(CopyNotice::Failed(id, error)) => {
                self.defer(hwnd, ModalStep::CopyFailed(id, error))
            }
            Some(CopyNotice::Finished) | None => {}
        }
        loop {
            let answer = self.created().work.borrow_mut().take_registry();
            let Some((id, answer)) = answer else { break };
            match answer {
                RegistryAnswer::Read(sign_in) => {
                    self.defer(hwnd, ModalStep::OpenSettings(id, sign_in))
                }
                RegistryAnswer::SignIn(result) => {
                    self.sign_in_saved.replace(Some((id, result)));
                    self.post_settings_reply();
                }
            }
        }
        let opened = self.created().work.borrow_mut().take_opened();
        for (purpose, id, opened) in opened {
            self.defer(hwnd, ModalStep::Opened(purpose, id, opened));
        }
        let prepared = self.created().work.borrow_mut().take_paths();
        for (purpose, id, prepared) in prepared {
            self.defer(hwnd, ModalStep::PreparedPaths(purpose, id, prepared));
        }
        let paired = self.created().work.borrow_mut().take_pairing();
        if let Some((id, local, paired)) = paired {
            self.defer(hwnd, ModalStep::Paired(id, local, paired));
        }
        loop {
            let saved = self.created().work.borrow_mut().take_saved();
            let Some(saved) = saved else { break };
            if let Err(error) = saved.result
                && saved.report_failure
                && !self.ending_session.get()
                && !self.settings_failure_shown.replace(true)
            {
                self.defer(hwnd, ModalStep::SettingsNotSaved(error));
            }
        }
        let fetched = {
            let mut work = self.created().work.borrow_mut();
            let programs = work.programs.drain();
            work.requests.set_programs(programs);
            let retired = work.programs.take_retired();
            work.retire(retired);
            let fetched = work.requests.drain();
            let retired = work.requests.take_retired();
            work.retire(retired);
            fetched
        };
        for fetched in fetched {
            let accepted = self.model.borrow_mut().accept(fetched);
            self.apply(hwnd, accepted);
        }
        let fixes = {
            let mut work = self.created().work.borrow_mut();
            let fixes = work.fixes.drain();
            let retired = work.fixes.take_retired();
            work.retire(retired);
            fixes
        };
        for event in fixes {
            let accepted = self.model.borrow_mut().accept_fix(event);
            self.apply(hwnd, accepted);
        }
        self.finish_include_save();
        self.finish_paste();
        self.finish_drops();
        self.on_panel_events(hwnd);
        let mut tray_events = Vec::new();
        {
            let mut work = self.created().work.borrow_mut();
            if let Some(tray) = &mut work.tray {
                tray.drain();
                while let Some(event) = tray.take_event() {
                    tray_events.push(event);
                }
                let retired = tray.take_retired();
                work.retire(retired);
            }
        }
        for event in tray_events {
            self.on_tray(hwnd, event);
            if self.shutdown.borrow().is_some() {
                return;
            }
        }
        let releases = self.created().work.borrow_mut().take_releases();
        for release in releases {
            let accepted = match release {
                ReleaseAnswer::Newer(Some(version)) => {
                    self.model.borrow_mut().set_newer_release(version)
                }
                ReleaseAnswer::Older(older) => {
                    self.model.borrow_mut().set_latest_release_older(older)
                }
                ReleaseAnswer::Newer(None) => continue,
            };
            self.apply(hwnd, accepted);
        }
        self.resume_selection_actions(hwnd);
        self.poll_work(hwnd);
    }

    fn poll_work(&self, hwnd: Hwnd) {
        if self.shutdown.borrow().is_some() {
            return;
        }
        let (active, expired) = self.created().work.borrow_mut().tick(Instant::now());
        for (purpose, generation) in expired {
            let accepted = self.model.borrow_mut().expire_request(purpose, generation);
            self.apply(hwnd, accepted);
        }
        self.update_message();
        self.update_status();
        if self.model.borrow().mode() == Mode::TwoPanels {
            for side in Side::BOTH {
                self.show_panel_message(side);
            }
        }
        if active {
            hwnd.set_timer(timer::WORK, 250);
        } else {
            hwnd.kill_timer(timer::WORK);
        }
    }

    /// A control's notification in `WM_COMMAND`.
    fn on_control(&self, hwnd: Hwnd, event: ControlEvent) {
        let controls = &self.created().controls;
        if let Some(input) = controls.status.transfer_input(event) {
            match input {
                crate::core::status_transfer::Input::Toggle => {
                    controls.status.restore_transfer_focus();
                    self.remember_inspector_focus();
                    if self.file_operations.get().is_some() {
                        self.model.borrow_mut().open_transfer_selection(None);
                    }
                    let accepted = self.model.borrow_mut().toggle_transfers();
                    self.apply(hwnd, accepted);
                    self.refresh_operations();
                }
                crate::core::status_transfer::Input::Action(id, QueueAction::Report) => {
                    self.show_operations(Some(id));
                    self.model
                        .borrow_mut()
                        .select_operation(crate::core::file_operations::Detail::Report(id));
                    self.refresh_operations();
                }
                crate::core::status_transfer::Input::Action(id, action) => {
                    self.act_operation(id, action);
                }
            }
            return;
        }
        if controls
            .inspector
            .fields
            .iter()
            .any(|(_, value)| value.focused(event) || value.unfocused(event))
        {
            // A value shows its whole text while it has the focus.
            self.fit_inspector();
            return;
        }
        if controls.search.focused(event) {
            // The click places the caret after the focus arrives, so the
            // text is selected once the click is handled.
            let select = self.model.borrow().choices().select_on_click;
            if select && win::window::left_button_down() && !self.select_search.replace(true) {
                hwnd.post(DEFERRED);
            }
            return;
        }
        let matching = || self.model.borrow().matching();
        let search_tool = controls.search_tools.clicked(event);
        let tools = controls.search_tools;
        let accepted = if controls.search.changed(event) {
            let text = controls.search.text();
            tools.set_enabled(ControlId::ClearSearch.id(), !text.is_empty());
            self.model.borrow_mut().set_typed_search(List::Search, text)
        } else if search_tool == Some(ControlId::ClearSearch.id()) {
            // As Esc in the search box does; the box's change runs the search.
            controls.search.hwnd().set_text("");
            controls.search.hwnd().focus();
            return;
        } else if controls.match_mode.selection_changed(event) {
            let Some(mode) = controls
                .match_mode
                .selected()
                .and_then(|i| MatchMode::ALL.get(i).copied())
            else {
                return;
            };
            let choice = Matching { mode, ..matching() };
            self.model.borrow_mut().set_matching(choice)
        } else if search_tool == Some(ControlId::MatchCase.id()) {
            let choice = Matching {
                case: tools.is_checked(ControlId::MatchCase.id()),
                ..matching()
            };
            self.model.borrow_mut().set_matching(choice)
        } else if search_tool == Some(ControlId::MatchPath.id()) {
            let choice = Matching {
                path: tools.is_checked(ControlId::MatchPath.id()),
                ..matching()
            };
            self.model.borrow_mut().set_matching(choice)
        } else if let Some(index) = controls
            .message_buttons
            .iter()
            .position(|button| button.clicked(event))
        {
            if index == 0
                && let Some(purpose) = self.slow_request(List::Search)
            {
                self.cancel_list_read(hwnd, purpose);
                return;
            }
            let button = self.model.borrow().list_buttons().get(index).cloned();
            match button {
                Some(ListButton::Fix(fix)) => self.run_fix(hwnd, fix),
                Some(ListButton::ClearFilters) => {
                    self.run(hwnd, MenuItem::Run(CommandId::ClearFilters));
                }
                Some(ListButton::SearchEverywhere) => {
                    let accepted = self.model.borrow_mut().search_everywhere();
                    self.apply(hwnd, accepted);
                }
                Some(ListButton::ChooseFolder) => self.choose_folder(hwnd),
                None => {}
            }
            return;
        } else if let Some(button) = self.command_button_clicked(event) {
            // The button runs its menu item's command (DESIGN.md:
            // window rule 9).
            return self.run(hwnd, MenuItem::Run(button.command));
        } else if controls.status.cancel_clicked(event) {
            return self.cancel_job(hwnd);
        } else if let Some(side) = self.panel_search(event.sender()) {
            let search = self.panel(side).search;
            if search.changed(event) {
                self.on_panel_search_text(hwnd, side);
            } else if search.focused(event) {
                self.activate_panel(side, false);
            }
            return;
        } else if let Some(side) = self.panel_look_in(event.sender()) {
            let look_in = self.panel(side).look_in;
            if look_in.selection_changed(event) {
                self.on_look_in(hwnd, side);
            }
            return;
        } else if let Some(side) = self.panel_path(event.sender()) {
            // The active path box shows the highlight colors only while it
            // does not have the focus; a path left without Enter goes back.
            let path = self.panel(side).path;
            if path.focused(event) {
                self.activate_panel(side, false);
                path.hwnd().invalidate();
            } else if path.unfocused(event) {
                path.hwnd().invalidate();
                self.show_panel(side);
            }
            return;
        } else if let Some(side) = Side::BOTH
            .into_iter()
            .find(|side| self.panel(*side).retry.clicked(event))
        {
            self.activate_panel(side, true);
            return self.panel_message_clicked(hwnd, side);
        } else if let Some(side) = Side::BOTH
            .into_iter()
            .find(|side| self.panel(*side).drive.clicked(event))
        {
            return self.drive_menu(hwnd, side);
        } else if let Some(index) = controls.key_bar.iter().position(|b| b.clicked(event)) {
            // The buttons take no focus, so it goes back to the panel.
            let active = self.model.borrow().panels().active();
            self.panel(active).list.hwnd().focus();
            return self.run(hwnd, MenuItem::Run(commands::key_bar_command(index)));
        } else {
            return self.on_filter(hwnd, event);
        };
        self.apply(hwnd, accepted);
    }

    /// The inspector's button or Show details that `event` says the person
    /// clicked, with what it runs.
    fn command_button_clicked(&self, event: ControlEvent) -> Option<CommandButton> {
        let controls = &self.created().controls;
        let inspector = &controls.inspector;
        let text = inspector
            .text_buttons()
            .into_iter()
            .find(|(_, control)| control.clicked(event))
            .map(|(button, _)| button);
        let icon = || {
            inspector
                .icon_tools()
                .into_iter()
                .chain(
                    controls
                        .status
                        .show_details()
                        .map(|show| (SHOW_DETAILS, show)),
                )
                .find(|(button, toolbar)| toolbar.clicked(event) == Some(button.control.id()))
                .map(|(button, _)| button)
        };
        text.or_else(icon)
    }

    /// The Kind list selected another item. A list view also tells of a
    /// selection that the window made to show the model's kind, which
    /// [`Model::set_filters`] takes as no change.
    fn on_kind_selected(&self, hwnd: Hwnd) {
        let selected = self.created().controls.filters.kind.selected();
        if let Some(kind) = selected.and_then(|index| Kind::ALL.get(index).copied()) {
            self.change_filters(hwnd, |f| f.kind = kind);
        }
    }

    /// The Location list selected another item, which the person or the
    /// window chose, as [`Self::on_kind_selected`] says.
    fn on_location_selected(&self, hwnd: Hwnd) {
        let location = {
            let model = self.model.borrow();
            let items = model.location_items();
            self.created()
                .controls
                .filters
                .location
                .selected()
                .and_then(|i| items.get(i))
                .map(|item| item.folder().map(str::to_owned))
                .filter(|location| *location != model.filters().location)
        };
        if let Some(location) = location {
            self.change_filters(hwnd, |f| f.location = location);
        }
    }

    /// Answers the Location list's tooltip request with the row's whole
    /// path when the row shows it shortened, and with no tip otherwise.
    fn location_tip(&self, tip: InfoTip) -> LRESULT {
        let list = self.created().controls.filters.location;
        let whole = tip.row().and_then(|row| {
            let path = captions::location_items(&self.model.borrow())
                .into_iter()
                .nth(row)?;
            (path != list.text(row)).then_some(path)
        });
        tip.set(whole.as_deref().unwrap_or_default())
    }

    /// A notification of a control in the filter pane.
    fn on_filter(&self, hwnd: Hwnd, event: ControlEvent) {
        let pane = &self.created().controls.filters;
        let tool = pane.tools.clicked(event).or(pane.show.clicked(event));
        if tool == Some(ControlId::ClearFilters.id()) {
            self.run(hwnd, MenuItem::Run(CommandId::ClearFilters));
        } else if tool.is_some() {
            // Hide filters, or Show filters in the search row.
            self.run(hwnd, MenuItem::Run(CommandId::Filters));
        } else if pane.choose_folder.clicked(event) {
            self.choose_folder(hwnd);
        } else if pane.only_directly.clicked(event) {
            let checked = pane.only_directly.is_checked();
            self.change_filters(hwnd, |f| f.only_directly = checked);
        } else if pane.date.selection_changed(event) {
            let date = self.model.borrow().filters().date;
            // Custom… opens its dialog only when the list closes on it, so
            // the arrow keys pass it by and keep the previous filter.
            if let Some(RangeItem::Choice(date)) = pane
                .date
                .selected()
                .and_then(|i| date_items(date).get(i).copied())
            {
                self.change_filters(hwnd, |f| f.date = date);
            }
        } else if pane.size.selection_changed(event) {
            let size = self.model.borrow().filters().size;
            if let Some(RangeItem::Choice(size)) = pane
                .size
                .selected()
                .and_then(|i| size_items(size).get(i).copied())
            {
                self.change_filters(hwnd, |f| f.size = size);
            }
        } else if let Some(range) = pane.range_of(event.sender()) {
            let list = pane.range(range);
            if let Some(closing) = list.closing(event) {
                self.range_closing.set(Some((range, closing)));
            } else if list.closed(event)
                && self.range_closing.take() == Some((range, ListClosing::Chosen))
            {
                self.range_closed.set(Some(range));
                hwnd.post(DEFERRED);
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
                self.change_filters(hwnd, |f| f.attributes[index] = choice);
            }
        }
    }

    /// The arrow on the sorted column's header (DESIGN.md: Results list).
    fn show_sort(&self) {
        let (count, sorted) = {
            let model = self.model.borrow();
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
        self.created().controls.list.show_sort(count, sorted);
    }

    /// A context menu of `groups` at `point`; the chosen item runs after
    /// the menu closes (DESIGN.md: window rule 9).
    fn context_menu(
        &self,
        hwnd: Hwnd,
        groups: &[Vec<MenuItem>],
        point: (i32, i32),
        default: Option<MenuItem>,
        context: selection::CommandContext,
    ) {
        let items: Vec<MenuItem> = groups.iter().flatten().copied().collect();
        let states = self.item_states_for(&items, context);
        let nodes = commands::render_popup(groups, self.model.borrow().strings());
        let default = default.map(MenuItem::id);
        let chosen = win::menu::Menu::track_popup(hwnd, &nodes, point, &states, default);
        if let Some(item) = chosen.and_then(MenuItem::from_id) {
            self.run_in_context(hwnd, item, context);
        }
    }

    /// The context menu of a row, at `point`, or at the focused row for the
    /// Menu key and Shift+F10. A right-click below the last row shows none
    /// (DESIGN.md: Results list).
    fn row_menu(&self, hwnd: Hwnd, point: Option<(i32, i32)>) {
        if let Some(context) = self.await_search_menu(hwnd, point) {
            self.row_menu_ready(hwnd, point, context);
        }
    }

    fn row_menu_ready(
        &self,
        hwnd: Hwnd,
        point: Option<(i32, i32)>,
        context: selection::CommandContext,
    ) {
        let list = self.created().controls.list;
        let point = match point {
            Some(point) => list.row_at(point).map(|_| point),
            None => list.focused_row_point(),
        };
        let Some(point) = point else { return };
        // The right-click selected its row a moment ago.
        // The first selected row's kind decides which Open shows, and Shift
        // the items shown only with it (DESIGN.md: The Context menu page).
        let first = list.next_selected(None);
        let groups = {
            let model = self.model.borrow();
            let folder = first
                .and_then(|row| model.search_list().results().row(row))
                .is_some_and(|row| row.is_dir);
            commands::shown_row_menu(
                model.choices().context_menu,
                folder,
                win::window::shift_held(),
            )
        };
        if groups.is_empty() {
            return;
        }
        let open = MenuItem::Run(CommandId::Open);
        let default = groups
            .iter()
            .flatten()
            .any(|item| *item == open)
            .then_some(open);
        self.context_menu(hwnd, &groups, point, default, context);
    }

    /// Reads the list's selection, if it changed since it was last read
    /// (DESIGN.md: The selection).
    fn read_list_selection(&self, hwnd: Hwnd) -> bool {
        if self.native_deselection_pending(List::Search) {
            return false;
        }
        if !self.selection_pending.replace(false) {
            return true;
        }
        let list = self.created().controls.list;
        let count = list.selected_count();
        let accepted = if count > SELECTION_LIMIT {
            self.model.borrow_mut().select_too_many(List::Search, count)
        } else {
            let Some(selected) = self.selected_rows(List::Search) else {
                self.selection_pending.set(true);
                return false;
            };
            self.model
                .borrow_mut()
                .set_selection(List::Search, selected)
        };
        self.apply(hwnd, accepted);
        true
    }

    fn on_list_notify(&self, hwnd: Hwnd, notify: ListNotify) -> Option<LRESULT> {
        match notify {
            ListNotify::GetDispInfo(mut info) => {
                self.fill_cell(&mut info);
                Some(LRESULT(0))
            }
            ListNotify::CacheHint(rows) => {
                let fetch = self.model.borrow_mut().rows_needed(List::Search, rows);
                if let Some(fetch) = fetch {
                    self.start(fetch);
                }
                Some(LRESULT(0))
            }
            ListNotify::ColumnClick(index) => {
                let accepted = {
                    let mut model = self.model.borrow_mut();
                    let sort = model
                        .columns()
                        .get(index)
                        .and_then(|column| model.header_sort(*column));
                    sort.map(|sort| model.sort_by(sort))
                };
                if let Some(accepted) = accepted {
                    self.apply(hwnd, accepted);
                }
                Some(LRESULT(0))
            }
            ListNotify::CustomDraw(mut draw) => Some(CustomDraw::answer(match draw.stage() {
                DrawStage::Paint => DrawAnswer::EachRow,
                DrawStage::Row { row } => self.draw_row(&mut draw, row),
                DrawStage::Cell { row, column } => self.draw_cell(&draw, hwnd.dpi(), row, column),
                DrawStage::RowPainted => {
                    draw.restore_font(&self.created().fonts.borrow().ui);
                    DrawAnswer::Default
                }
                DrawStage::Other => DrawAnswer::Default,
            })),
            ListNotify::SelectionChanged(change) => {
                if !self.selection_changed(List::Search, change) {
                    return None;
                }
                if !self.selection_pending.replace(true) {
                    hwnd.post(DEFERRED);
                }
                None
            }
            ListNotify::Activated => {
                self.enter_result(hwnd);
                None
            }
            ListNotify::BeginDrag => {
                self.drag_selected(hwnd);
                None
            }
            ListNotify::DividerDoubleClick(index) => {
                self.fit_column(index);
                None
            }
            ListNotify::ColumnDragEnded => {
                self.column_dragged.set(true);
                hwnd.post(DEFERRED);
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
    fn fill_cell(&self, info: &mut DispInfo) {
        let (text, icon) = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            let column = model.columns().get(info.column()).copied();
            // A taken row keeps its text, which `draw_cell` strikes through.
            match (column, model.search_list().results().row(info.row())) {
                (Some(column), Some(row)) => (
                    info.wants_text().then(|| {
                        crate::core::text::cell(
                            model.strings(),
                            &*numbers,
                            model.choices().size_format,
                            &self.dates.borrow(),
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
            info.set_text(text.as_deref().unwrap_or_default());
        }
        if info.wants_image() {
            info.set_image(icon.map(|(name, is_folder)| self.icons.index(&name, is_folder)));
        }
        if info.wants_state() {
            // Rows that changed under the drag no longer show it.
            let target = {
                let model = self.model.borrow();
                let drag_in = self.drag_in.borrow();
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
    fn draw_row(&self, draw: &mut CustomDraw, row: u64) -> DrawAnswer {
        let taken = !(self.model.borrow())
            .taken_rows(List::Search, std::slice::from_ref(&(row..row + 1)))
            .is_empty();
        if taken {
            draw.draw_row_struck(&self.created().fonts.borrow().struck);
            DrawAnswer::NewFont
        } else {
            DrawAnswer::EachCell
        }
    }

    /// Draws a Name or Path cell whose text the search matched, with the
    /// matched parts in bold (DESIGN.md: Results list); the list draws every
    /// other cell.
    fn draw_cell(&self, draw: &CustomDraw, dpi: Dpi, row: u64, column: usize) -> DrawAnswer {
        let cell = {
            let model = self.model.borrow();
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
        let icon = icon.map(|is_folder| self.icons.index(&text, is_folder));
        let created = self.created();
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

    /// A key that the control `target` gives a meaning (DESIGN.md:
    /// window rule 8). Returns whether the window took it.
    fn on_key(&self, hwnd: Hwnd, target: Hwnd, mut keystroke: Keystroke) -> bool {
        if keystroke == Keystroke::plain(commands::Key::Escape) {
            let list = if target == self.created().controls.list.hwnd() {
                Some(List::Search)
            } else {
                self.panel_list(target).map(List::Panel)
            };
            if let Some(list) = list {
                if let List::Panel(side) = list
                    && self
                        .created()
                        .work
                        .borrow()
                        .folders
                        .waiting(side, Instant::now())
                        == Some(Waiting::Slow)
                {
                    self.panel_message_clicked(hwnd, side);
                    return true;
                }
                if let Some(purpose) = self.slow_request(list) {
                    self.cancel_list_read(hwnd, purpose);
                    return true;
                }
            }
        }
        if self
            .file_operations
            .get()
            .is_some_and(|ops| ops.key(target, keystroke))
        {
            return true;
        }
        if keystroke.key == commands::Key::Tab
            && (self
                .file_operations
                .get()
                .is_some_and(|ops| ops.contains(target))
                || self.created().controls.status.contains(target))
        {
            return false;
        }
        // Shift+Space types a space in the quick search box, as Space does,
        // instead of selecting rows in the list.
        let space = Keystroke::plain(commands::Key::Space);
        if keystroke
            == (Keystroke {
                shift: true,
                ..space
            })
            && self.panel_list(target).is_some_and(|side| {
                (self.model.borrow().panels().panel(side))
                    .quick_text()
                    .is_some()
            })
        {
            keystroke = space;
        }
        let Some(action) = commands::control_key(self.focus_of(Some(target)), keystroke) else {
            return false;
        };
        let controls = &self.created().controls;
        match action {
            KeyAction::Run(command) => self.run(hwnd, MenuItem::Run(command)),
            KeyAction::FocusList => {
                let count = self.model.borrow().search_list().results().count();
                controls.list.focus_left_row(count);
            }
            KeyAction::SearchBoxEnter => {
                let enter = self.model.borrow_mut().enter_in_search_box();
                match enter {
                    SearchBoxEnter::Search => self.search(hwnd, List::Search),
                    SearchBoxEnter::Enter => self.enter_first_or_selected(hwnd),
                    SearchBoxEnter::Wait => {}
                }
            }
            KeyAction::ResultEnter => self.enter_result(hwnd),
            KeyAction::ClearSearch => controls.search.hwnd().set_text(""),
            KeyAction::MoveTab { backward } => self.move_tab_focus(hwnd, target, backward),
            KeyAction::CancelJob => {
                if self.model.borrow().job() != Job::Operation {
                    self.run(hwnd, MenuItem::Run(CommandId::Cancel));
                }
            }
            KeyAction::ChooseCustom => {
                if let Some(range) = controls.filters.range_of(target) {
                    self.choose_custom(hwnd, range);
                }
            }
            KeyAction::Panel(key) => return self.on_panel_key(hwnd, target, keystroke, key),
        }
        true
    }

    /// Moves the focus from `target` as `Tab`, or `Shift+Tab` when
    /// `backward`, does ([`commands::tab_stop`]), and shows the focus
    /// rectangles, which the window takes from `IsDialogMessageW` with the
    /// key. Entering the search box selects its text, as `Tab` into a
    /// dialog's edit box does.
    fn move_tab_focus(&self, hwnd: Hwnd, target: Hwnd, backward: bool) {
        hwnd.show_focus_cues();
        if self.model.borrow().mode() == Mode::TwoPanels {
            // From a control outside the panels' lists and boxes, such as
            // a drive button, Tab returns to the active panel, because
            // Search's controls are hidden.
            let active = self.model.borrow().panels().active();
            self.activate_panel(active, true);
            return;
        }
        let controls = &self.created().controls;
        let from = if target == controls.search.hwnd() {
            Some(TabStop::SearchBox)
        } else if target == controls.list.hwnd() {
            Some(TabStop::Results)
        } else {
            None
        };
        match commands::tab_stop(from, backward) {
            TabStop::SearchBox => {
                controls.search.hwnd().focus();
                controls.search.select_all();
            }
            TabStop::Results => controls.list.hwnd().focus(),
        }
    }

    /// Gives the selected row `Enter`, as `Enter` in the list does, or the
    /// first row when none is selected (DESIGN.md: window rule 8).
    fn enter_first_or_selected(&self, hwnd: Hwnd) {
        let list = self.created().controls.list;
        if list.selected_count() == 0 && self.model.borrow().search_list().results().count() > 0 {
            list.select_only(Some(0));
            list.ensure_visible(0);
        }
        self.enter_result(hwnd);
    }

    /// `Enter` or a double-click on Search's focused result: as Settings
    /// chooses, Two panels shows it, or Open opens the selection (DESIGN.md:
    /// Searching).
    fn enter_result(&self, hwnd: Hwnd) {
        let row = self.created().controls.list.focused_or_first_selected();
        let choice = {
            let model = self.model.borrow();
            let Some(result) = row.and_then(|row| model.search_list().results().row(row)) else {
                return;
            };
            model.choices().enter_result(result.is_dir)
        };
        match choice {
            ResultEnter::ShowInPanels => {
                let row = self.created().controls.list.focused_or_first_selected();
                if let Some(row) = row {
                    self.show_result_in_panels(hwnd, row);
                }
            }
            ResultEnter::Open => self.run(hwnd, MenuItem::Run(CommandId::Open)),
        }
    }

    /// Whether `point`, in client coordinates, is on the splitter above the
    /// inspector.
    fn on_splitter(&self, point: (i32, i32)) -> bool {
        self.created()
            .layout
            .borrow()
            .as_ref()
            .and_then(Layout::inspector)
            .is_some_and(|inspector| inspector.splitter.contains(point))
    }

    /// Whether `point`, in screen coordinates, is on the list's header.
    fn on_header(&self, point: (i32, i32)) -> bool {
        // A hidden list keeps its header's rectangle, which a panel covers
        // in Two panels.
        let list = self.created().controls.list;
        list.hwnd().is_visible() && list.header().screen_rect().contains(point)
    }

    fn selected_source(&self, list: List) -> Option<Entries> {
        let model = self.model.borrow();
        match list {
            List::Panel(side) => model.panels().chosen_source(side),
            List::Search => model
                .search_list()
                .selection()
                .shared_files()
                .map(Entries::Results),
        }
    }

    fn act_on_selected(&self, list: List, action: EntryAction) {
        let Some(entries) = self.selected_source(list) else {
            return;
        };
        self.open_entries(list, action, entries);
    }

    fn act_on_entries(&self, list: List, action: EntryAction, entries: Vec<Selected>) {
        self.open_entries(list, action, Entries::Ready(entries));
    }

    fn open_entries(&self, list: List, action: EntryAction, entries: Entries) {
        let commands = {
            let model = self.model.borrow();
            let choices = model.choices();
            Commands {
                folders: choices.open_folders.clone(),
                files: choices.open_files.clone(),
                location: choices.open_file_location.clone(),
            }
        };
        self.created()
            .work
            .borrow_mut()
            .open_entries(list, action, entries, commands);
    }

    /// Drags the selected entries out of the window, as File Explorer drags
    /// its own, once they are read (DESIGN.md: Dragging).
    fn drag_selected(&self, hwnd: Hwnd) {
        if !self.read_list_selection(hwnd) {
            return;
        }
        let selected =
            (self.model.borrow().search_list().selection().files()).map_or(0, <[_]>::len);
        if selected > DRAG_LIMIT {
            let text = {
                let numbers = self.numbers.borrow();
                let count = |n: usize| numbers.integer(n as u64);
                self.model.borrow().strings().format(
                    "message.drag_limit",
                    &[&count(DRAG_LIMIT), &count(selected)],
                )
            };
            self.error_box(hwnd, &text);
            return;
        }
        if selected == 0 {
            return;
        }
        // The drag runs its own message loop, so it gets its own copy of the
        // files and no borrow is held (DESIGN.md: window rule 1).
        let files = (self.model.borrow().search_list().selection().files())
            .unwrap_or_default()
            .to_vec();
        let missing = shell::drag(&files);
        self.report_missing(hwnd, missing);
    }

    /// Redraws the rows that a drop's highlight left or reached.
    fn redraw_drop_rows(&self, rows: impl IntoIterator<Item = u64>) {
        let list = self.created().controls.list;
        for row in rows {
            list.redraw_rows(row..row + 1);
        }
    }

    /// Takes away what a drag that left or was dropped showed: the row's
    /// highlight and the scroll timer.
    fn end_drag_in(&self, drag: Option<&DragIn>) {
        let Some(drag) = drag else { return };
        self.redraw_drop_rows(drag.target.as_ref().map(|target| target.row));
        if drag.scrolling.is_some() {
            let window = self.created().controls.list.hwnd().parent();
            window.kill_timer(timer::DROP_SCROLL);
        }
    }

    /// Scrolls the list a row while dragged files rest at its edge, and
    /// moves the highlight to the folder row the scroll brings under the
    /// pointer.
    fn drop_scroll(&self, hwnd: Hwnd) {
        // A tick posted before the timer stopped, such as during a drop,
        // finds no scrolling and changes nothing.
        let state = self
            .drag_in
            .borrow()
            .as_ref()
            .and_then(|drag| Some((drag.scrolling?, drag.over?)));
        let Some((edge, (point, keys, allowed))) = state else {
            hwnd.kill_timer(timer::DROP_SCROLL);
            return;
        };
        // A source that ended without a drop sends no DragLeave.
        if !win::window::mouse_button_down() {
            self.drag_leave();
            return;
        }
        self.created().controls.list.scroll_row(edge);
        self.drag_over(point, keys, allowed);
    }

    /// Says that `missing`, which the person chose, no longer exists
    /// (DESIGN.md: Feedback).
    fn report_missing(&self, hwnd: Hwnd, missing: Option<Vec<u16>>) {
        let Some(path) = missing else { return };
        let path = String::from_utf16_lossy(&path);
        let text = self
            .model
            .borrow()
            .strings()
            .format("message.no_longer_exists", &[&path]);
        self.error_box(hwnd, &text);
    }

    /// Puts the selected files on the clipboard in `format` (DESIGN.md: C8, C9,
    /// C10). The block holds this command's selection through background retries.
    fn copy(&self, hwnd: Hwnd, format: ClipboardFormat) {
        let files = {
            let model = self.model.borrow();
            model.search_list().selection().shared_files()
        };
        let Some(files) = files else { return };
        hwnd.kill_timer(timer::CLIPBOARD);
        self.created().work.borrow_mut().copy(files, format);
    }

    fn cancel_copy(&self, hwnd: Hwnd) {
        hwnd.kill_timer(timer::CLIPBOARD);
        self.created().work.borrow_mut().cancel_copy();
    }

    /// `F3`: opens a Viewer of the active panel's focused file
    /// (DESIGN.md: The Viewer).
    fn view_focused(&self, hwnd: Hwnd, side: Side) {
        let (path, strings) = {
            let model = self.model.borrow();
            let panels = model.panels();
            let Some(entry) = panels.focused(side).filter(|entry| !entry.is_folder) else {
                return;
            };
            (entry.path, model.strings().clone())
        };
        if let Err(error) = self.viewers.open(path, strings, hwnd) {
            super::write_log(
                &self.settings_file.with_file_name("ui-error.log"),
                &format!("The Viewer did not open: {error}"),
            );
        }
    }

    /// Admission stays disabled until the complete recovery read is adopted.
    fn open_file_operations(&self, hwnd: Hwnd) {
        let saved = self.settings_file.with_file_name("operations.json");
        self.created().work.borrow_mut().restore_queue(saved);
        clipboard::listen(hwnd);
        self.model
            .borrow_mut()
            .set_clipboard_holds_files(clipboard::holds_files());
    }

    fn adopt_queue(&self, hwnd: Hwnd, restored: QueueReady) {
        let strings = self.model.borrow().strings().clone();
        let error_log = self.settings_file.with_file_name("ui-error.log");
        let queue = self.created().work.borrow_mut().open_queue(restored);
        let operations = FileOperations::new(hwnd, queue, strings, error_log);
        operations.set_transfer_limit(self.model.borrow().choices().transfers_per_device);
        let _ = self.file_operations.set(operations);
        self.model.borrow_mut().offer_file_operations();
        self.update_key_bar();
        // Enrichment may have woken before adoption; explicitly read the restored state.
        hwnd.post(QUEUE_CHANGED);
    }

    /// Prepares `request` in the queue, with the speed settings; `None` in
    /// a window that owns no file operations.
    fn start_operation(
        &self,
        request: Request,
        source: Option<FilesSource>,
    ) -> Option<OperationId> {
        let operations = self.file_operations.get()?;
        let settings = self.model.borrow().choices().speed.settings();
        let kind = source.as_ref().map(FilesSource::kind);
        let id = operations.start(request, settings, source);
        self.model.borrow_mut().file_operation_started(id, kind);
        Some(id)
    }

    /// Reads the queue after it changed, and runs what the model decides:
    /// the queue's steps first, then the lists', the dialogs, which wait
    /// for [`Message::Deferred`], the transfer pane's, and the
    /// sources' last, because a drop's source can deliver messages while
    /// it is told.
    fn on_operations(&self, hwnd: Hwnd) {
        self.continue_operation_steps(hwnd);
        if self.delivering_operations.get() || self.operation_delivery.borrow().is_some() {
            return;
        }
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        if self.model.borrow().can_receive_operations() {
            for reply in operations.update() {
                match reply {
                    super::work::QueueAccepted::Resolved(id, accepted) => {
                        let source = operations.resolved(id, accepted);
                        if accepted {
                            self.model.borrow_mut().file_operation_started(id, source);
                        }
                    }
                    super::work::QueueAccepted::Submitted(id, true) => {
                        self.model.borrow_mut().operation_submitted(id);
                    }
                    super::work::QueueAccepted::Submitted(_, false)
                    | super::work::QueueAccepted::Moved(_, _) => {}
                }
            }
            let taken = operations.taken();
            self.model.borrow_mut().observe_operations(
                operations.snapshot(),
                Arc::clone(&taken),
                Instant::now(),
            );
            if !self.model.borrow().shows_taken(&taken) {
                self.show_taken(hwnd, taken);
            }
        }
        self.created()
            .work
            .borrow_mut()
            .retire(operations.take_retired());
        use super::file_operations::PaneAction;
        for action in operations.take_actions() {
            match action {
                PaneAction::Select(detail) => self.model.borrow_mut().select_operation(detail),
                PaneAction::Command(id, QueueAction::Report) => self
                    .model
                    .borrow_mut()
                    .select_operation(crate::core::file_operations::Detail::Report(id)),
                PaneAction::Command(id, QueueAction::Clear) => {
                    self.model.borrow_mut().operation_reported(id);
                }
                PaneAction::Command(id, QueueAction::Continue)
                    if operations.operation(id).is_some_and(|operation| {
                        matches!(*operation.request, Request::Synchronize { .. })
                    }) =>
                {
                    self.defer(hwnd, ModalStep::Synchronize(id));
                }
                PaneAction::Command(id, action) => self.act_operation(id, action),
                PaneAction::Move(id, before) => operations.move_before(id, before),
                PaneAction::CancelAll => {
                    self.dropping.borrow_mut().clear();
                    self.cancel_paste(None);
                    operations.cancel_all();
                }
                PaneAction::CloseReport => self.model.borrow_mut().close_operation_report(),
                PaneAction::Collapse | PaneAction::Maximize | PaneAction::Flip => {
                    let accepted = {
                        let mut model = self.model.borrow_mut();
                        match action {
                            PaneAction::Collapse => model.collapse_inspector(),
                            PaneAction::Maximize => model.maximize_inspector(),
                            PaneAction::Flip => model.flip_transfers(),
                            _ => unreachable!(),
                        }
                    };
                    self.apply(hwnd, accepted);
                }
            }
        }
        let released = self.model.borrow_mut().next_operation_release();
        self.run_queue_steps(released);
        self.retire_model();
        self.start_list_passes();
        if self.model.borrow().has_operation_release() {
            hwnd.post(QUEUE_CHANGED);
        }
    }

    fn apply_operation_steps(&self, hwnd: Hwnd, steps: OperationSteps) {
        assert!(
            self.operation_delivery.borrow().is_none(),
            "the previous receipt finishes delivery before the next pass"
        );
        self.operation_delivery
            .replace(Some(OperationDelivery::new(steps)));
        self.retire_model();
        for list in List::ALL {
            let pending = self
                .model
                .borrow()
                .list(list)
                .is_some_and(|shown| matches!(shown.selection(), Selection::Filtering { .. }));
            if pending {
                self.apply(
                    hwnd,
                    Accepted {
                        change: Change::Selection(list),
                        then: None,
                    },
                );
            }
        }
        self.continue_operation_steps(hwnd);
    }

    fn continue_operation_steps(&self, hwnd: Hwnd) {
        if self.delivering_operations.replace(true) {
            return;
        }
        for _ in 0..crate::core::work::INLINE_ITEMS {
            let next = self
                .operation_delivery
                .borrow_mut()
                .as_mut()
                .and_then(OperationDelivery::next);
            let Some(next) = next else {
                break;
            };
            match next {
                OperationEffect::Queue(step) => {
                    if let Some(operations) = self.file_operations.get() {
                        operations.run(step);
                    }
                }
                OperationEffect::Panel(step) => self.follow_operations(hwnd, &[step]),
                OperationEffect::Dialog(dialog) => self.defer(hwnd, ModalStep::Operation(dialog)),
                OperationEffect::Progress(progress) => {
                    self.refresh_operations();
                    self.show_progress(hwnd, progress);
                }
                OperationEffect::Source(step) => {
                    if let Some(operations) = self.file_operations.get() {
                        operations.tell_source(step);
                    }
                }
            }
            if self.shutdown.borrow().is_some() {
                break;
            }
        }
        self.delivering_operations.set(false);
        if self.shutdown.borrow().is_some() {
            return;
        }
        let finished = self
            .operation_delivery
            .borrow()
            .as_ref()
            .is_some_and(OperationDelivery::is_empty);
        if finished {
            let delivery = self.operation_delivery.take();
            let mut retired = Retirement::default();
            retired.push(delivery, crate::core::work::INLINE_ITEMS + 1);
            self.created().work.borrow_mut().retire(retired);
            self.start_list_passes();
            hwnd.post(DEFERRED);
        }
        if finished || self.operation_delivery.borrow().is_some() {
            hwnd.post(QUEUE_CHANGED);
        }
    }

    /// Runs `steps` on the queue. A removal wakes the window once the queue
    /// has taken the removed operation's sources out of the taken entries.
    fn run_queue_steps(&self, steps: Vec<QueueStep>) {
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        for step in steps {
            operations.run(step);
        }
    }

    fn act_operation(&self, id: OperationId, action: QueueAction) {
        if action == QueueAction::Cancel {
            self.abandon_drop(id);
            self.cancel_paste(Some(id));
        }
        if let Some(operations) = self.file_operations.get() {
            operations.act(id, action);
        }
    }

    /// Lets the queue remove the ended deletes and moves that no list keeps
    /// any more, after a list let go of them.
    fn release_operations(&self) {
        let steps = self.model.borrow_mut().release_operations();
        self.run_queue_steps(steps);
        self.start_list_passes();
    }

    fn start_list_passes(&self) {
        self.gate_changed_selections();
        let input =
            if self.delivering_operations.get() || self.operation_delivery.borrow().is_some() {
                None
            } else {
                self.model.borrow_mut().take_following()
            };
        if let Some(input) = input {
            self.created()
                .work
                .borrow_mut()
                .follow(input, self.numbers.borrow().clone());
        }
        for side in Side::BOTH {
            let input = self.model.borrow_mut().panels_mut().take_calculation(side);
            if let Some(input) = input {
                self.created()
                    .work
                    .borrow_mut()
                    .calculate_panel(side, input);
            }
            let input = self.model.borrow_mut().panels_mut().take_count(side);
            if let Some(input) = input {
                self.created().work.borrow_mut().count_panel(side, input);
            }
            let input = self.model.borrow_mut().panels_mut().take_quick(side);
            if let Some(input) = input {
                self.created().work.borrow_mut().find_quick(side, input);
            }
        }
        for list in List::ALL {
            let keep = self.model.borrow_mut().take_keep(list);
            if let Some(input) = keep {
                self.created().work.borrow_mut().keep(list, input);
            }
            let filter = self.model.borrow_mut().take_selection_filter(list);
            if let Some(input) = filter {
                self.created().work.borrow_mut().filter(list, input);
            }
        }
    }

    /// Opens File operations explicitly, with `select` selected.
    fn show_operations(&self, select: Option<OperationId>) {
        self.remember_inspector_focus();
        let accepted = {
            let mut model = self.model.borrow_mut();
            if self.file_operations.get().is_some() {
                model.open_transfer_selection(select);
            }
            model.show_transfers()
        };
        self.apply(self.created().controls.search.hwnd().parent(), accepted);
        if let Some(operations) = self.file_operations.get() {
            let view = self.model.borrow().operation_view();
            if let Some(view) = view {
                operations.show(view);
            }
            self.created()
                .work
                .borrow_mut()
                .retire(operations.take_retired());
        }
    }

    /// Refreshes the transfer pane without opening it.
    fn refresh_operations(&self) {
        self.start_list_passes();
        if let Some(operations) = self.file_operations.get() {
            let view = self.model.borrow().operation_view();
            if let Some(view) = view {
                operations.refresh(view);
            }
            self.created()
                .work
                .borrow_mut()
                .retire(operations.take_retired());
        }
    }

    fn remember_inspector_focus(&self) {
        if self.inspector_focus.get().is_none()
            && let Some(focus) = Hwnd::focused().filter(|focus| {
                !self
                    .file_operations
                    .get()
                    .is_some_and(|ops| ops.contains(*focus))
            })
        {
            self.inspector_focus.set(Some(focus));
        }
    }

    /// The status bar's operation, the taskbar button and the poll of a
    /// running operation's counts follow `progress`.
    fn show_progress(&self, hwnd: Hwnd, progress: OperationProgress) {
        if progress.status_changed {
            self.update_status();
        }
        if let Some(operations) = self.file_operations.get() {
            operations.show_taskbar(progress.taskbar);
        }
        if progress.polls {
            hwnd.set_timer(timer::OPERATIONS_POLL, OPERATIONS_POLL_MS);
        } else {
            hwnd.kill_timer(timer::OPERATIONS_POLL);
        }
    }

    /// Reads a running operation's counts, which change without a wake.
    fn poll_operations(&self, _hwnd: Hwnd) {
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        operations.request_refresh();
        self.refresh_operations();
    }

    /// Shows the retained failure without holding model borrows across its message loop.
    fn operation_dialog(&self, _hwnd: Hwnd, dialog: OperationDialog) -> bool {
        match dialog {
            OperationDialog::Report(id) => {
                let transfer = self.model.borrow().operation_is_transfer(id);
                if transfer {
                    self.show_operations(Some(id));
                    self.model
                        .borrow_mut()
                        .select_operation(crate::core::file_operations::Detail::Report(id));
                    self.refresh_operations();
                    return false;
                }
                let (operation, strings) = {
                    let model = self.model.borrow();
                    (model.operation_report(id), model.strings().clone())
                };
                let Some(operation) = operation else {
                    return false;
                };
                self.created().work.borrow_mut().prepare_report(
                    Reporting { operation, strings },
                    self.numbers.borrow().clone(),
                );
            }
        }
        true
    }

    fn show_prepared_failure(&self, hwnd: Hwnd, mut prepared: PreparedFailure) {
        let current = self
            .model
            .borrow()
            .operation_outcome(prepared.id)
            .is_some_and(|outcome| Arc::ptr_eq(&outcome, &prepared.outcome));
        if current {
            if let Some(failure) = prepared.failure.take() {
                prepared.failure = Some(super::file_operations::show_failure(
                    hwnd,
                    &prepared.strings,
                    failure,
                ));
            }
            if self.shutdown.borrow().is_none() {
                self.model.borrow_mut().operation_reported(prepared.id);
                self.start_list_passes();
                hwnd.post(QUEUE_CHANGED);
            }
        }
        let mut retired = Retirement::default();
        retired.push(prepared, crate::core::work::INLINE_ITEMS + 1);
        self.created().work.borrow_mut().retire(retired);
    }

    fn transfer_request(
        sources: Vec<PathBuf>,
        destination: PathBuf,
        moves: bool,
        policy: gozler_fileops::Policy,
    ) -> Request {
        let destination = Destination::Folder(destination);
        if moves {
            Request::Move {
                policy,
                sources,
                destination,
            }
        } else {
            Request::Copy {
                policy,
                sources,
                destination,
            }
        }
    }

    fn accept_transfer(&self, destination: PathBuf, moves: bool) -> Option<OperationId> {
        let operations = self.file_operations.get()?;
        let settings = self.model.borrow().choices().speed.settings();
        Some(operations.accept(
            Self::transfer_request(
                Vec::new(),
                destination,
                moves,
                self.model.borrow().choices().copy_policy,
            ),
            settings,
        ))
    }

    fn resolve_transfer(
        &self,
        id: OperationId,
        sources: Vec<PathBuf>,
        destination: PathBuf,
        moves: bool,
        from: Option<FilesSource>,
    ) {
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        if sources.is_empty() {
            operations.fail_intake(id);
            return;
        }
        let policy = self.model.borrow().choices().copy_policy;
        operations.resolve(
            id,
            Self::transfer_request(sources, destination, moves, policy),
            from,
        );
    }

    fn remember_copy_policy(&self, hwnd: Hwnd, policy: gozler_fileops::Policy) {
        self.model.borrow_mut().set_copy_policy(policy);
        self.note_saved_state(hwnd);
    }

    fn prepare_selected(&self, list: List, action: PathAction) {
        let Some(entries) = self.selected_source(list) else {
            return;
        };
        self.created()
            .work
            .borrow_mut()
            .prepare_paths(action, entries);
    }

    fn retire_paths(&self, paths: Vec<PathBuf>) {
        let items = paths.len();
        let mut retired = Retirement::default();
        retired.push(paths, items);
        self.created().work.borrow_mut().retire(retired);
    }

    fn ask_transfer(
        &self,
        hwnd: Hwnd,
        start: TransferStart,
        moves: bool,
    ) -> Option<(Destination, gozler_fileops::Policy)> {
        let (dialog, captions) = {
            let model = self.model.borrow();
            let strings = model.strings();
            let mut captions = DialogCaptions::new(DialogId::Transfer, strings);
            captions.title = start.title.clone();
            if moves {
                let ok = captions
                    .items
                    .iter_mut()
                    .find(|(id, _)| *id == DialogItem::Ok.id());
                if let Some((_, text)) = ok {
                    *text = strings.text("file_operations.move").to_owned();
                }
            }
            (Transfer::new(strings, start), captions)
        };
        win::dialog::run_modal(hwnd, DialogId::Transfer, &captions, &dialog)
            .ok()
            .and_then(|_| dialog.chosen())
    }

    fn copy_to_folder(&self, _hwnd: Hwnd, moves: bool, list: List) {
        self.prepare_selected(list, PathAction::Transfer { moves });
    }

    fn copy_paths_to_folder(&self, hwnd: Hwnd, moves: bool, sources: Vec<PathBuf>) {
        let Some(first) = sources.first() else {
            return;
        };
        let start = {
            let model = self.model.borrow();
            let strings = model.strings();
            let (title, name_line, items_line) = if moves {
                (
                    "file_operations.move_to_title",
                    "transfer.move_name",
                    "transfer.move_items",
                )
            } else {
                (
                    "file_operations.copy_to_title",
                    "transfer.copy_name",
                    "transfer.copy_items",
                )
            };
            let one = (sources.len() == 1).then(|| {
                first
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
            let line = match &one {
                Some(name) => strings.format(name_line, &[name]),
                None => strings.count(
                    items_line,
                    sources.len() as u64,
                    &*self.numbers.borrow(),
                    &[],
                ),
            };
            let folder = first
                .parent()
                .unwrap_or(first)
                .to_string_lossy()
                .into_owned();
            let text = crate::core::panel::join(model.destination_folder().unwrap_or(&folder), "");
            TransferStart {
                policy: model.choices().copy_policy,
                title: strings.text(title).to_owned(),
                line,
                selected: (0, text.encode_utf16().count()),
                other: text.clone(),
                text,
                folder,
                one,
            }
        };
        let Some((destination, policy)) = self.ask_transfer(hwnd, start, moves) else {
            self.retire_paths(sources);
            return;
        };
        if let Destination::Folder(folder) = &destination {
            let accepted = self
                .model
                .borrow_mut()
                .set_destination_folder(folder.to_string_lossy().into_owned());
            self.apply(hwnd, accepted);
        }
        self.remember_copy_policy(hwnd, policy);
        let request = if moves {
            Request::Move {
                policy,
                sources,
                destination,
            }
        } else {
            Request::Copy {
                policy,
                sources,
                destination,
            }
        };
        self.start_operation(request, None);
    }

    /// Delete, and the list's `Delete` and `Shift+Delete`: the Delete
    /// question, then the permanent delete (DESIGN.md: File operations).
    fn delete_selected(&self, _hwnd: Hwnd, list: List) {
        let place = {
            let model = self.model.borrow();
            match list {
                List::Panel(side) => Some(model.panels().command_place(side)),
                List::Search => None,
            }
        };
        self.prepare_selected(list, PathAction::Delete { place });
    }

    fn delete_paths(
        &self,
        hwnd: Hwnd,
        place: Option<crate::core::panel::CommandPlace>,
        sources: Vec<PathBuf>,
    ) {
        if sources.is_empty() {
            return;
        }
        let question = {
            let model = self.model.borrow();
            delete_question(model.strings(), &*self.numbers.borrow(), &sources)
        };
        // The question runs its own message loop, so nothing is borrowed
        // here.
        if win::dialog::ask(hwnd, &question) {
            self.start_operation(Request::Delete { sources }, None);
            let follows = place.is_some_and(|place| {
                let model = self.model.borrow();
                model.mode() == Mode::TwoPanels
                    && model.panels().active() == place.side
                    && model.panels().place_is_current(place)
            });
            if follows {
                self.focus_after_chosen();
            }
        } else {
            self.retire_paths(sources);
        }
    }

    /// Paste on one selected folder: copies the files on the clipboard into
    /// it, or moves them when they were cut (DESIGN.md: Copy), once a
    /// thread has read them.
    fn paste(&self) {
        let Some(entry) = self
            .selected_source(List::Search)
            .and_then(|entries| entries.one())
        else {
            return;
        };
        let destination = PathBuf::from(OsString::from_wide(&entry.path));
        let sequence = clipboard::sequence();
        let Some(id) = self.accept_transfer(destination.clone(), false) else {
            return;
        };
        self.pasting.borrow_mut().insert(id, destination);
        self.created().work.borrow_mut().paste.read(id, sequence);
    }

    /// Paste's files, once their thread has read them from the clipboard.
    fn finish_paste(&self) {
        let answer = {
            let mut work = self.created().work.borrow_mut();
            let answer = work.paste.take();
            let retired = work.paste.take_retired();
            work.retire(retired);
            answer
        };
        let Some((id, mut files)) = answer else {
            return;
        };
        let destination = self.pasting.borrow_mut().remove(&id);
        let Some(destination) = destination else {
            if let Some(files) = &mut files {
                files.release_source();
            }
            let mut retired = Retirement::default();
            retired.push(files, crate::core::work::INLINE_ITEMS + 1);
            self.created().work.borrow_mut().retire(retired);
            return;
        };
        let (sources, moves) = files.as_mut().map_or_else(
            || (Vec::new(), false),
            |files| (files.take_paths(), files.cut),
        );
        self.resolve_transfer(
            id,
            sources,
            destination,
            moves,
            files.map(FilesSource::Paste),
        );
    }

    fn cancel_paste(&self, id: Option<OperationId>) {
        let mut work = self.created().work.borrow_mut();
        match id {
            Some(id) => {
                self.pasting.borrow_mut().remove(&id);
                work.paste.cancel(id);
            }
            None => {
                let mut retired = Retirement::default();
                retired.push(self.pasting.take(), crate::core::work::INLINE_ITEMS + 1);
                work.retire(retired);
                work.paste.cancel_all();
            }
        }
        let retired = work.paste.take_retired();
        work.retire(retired);
    }

    /// Shows the rows that deletes and moves have taken, `taken`, struck
    /// through in Search's list and blank in a search panel, and takes them
    /// out of the selections (fast-copy-delete-spec.md: Taken entries).
    fn show_taken(&self, hwnd: Hwnd, taken: Arc<Taken>) {
        let replaced = Arc::clone(self.model.borrow().panels().taken());
        let shown = self.show_panels_taken(taken);
        self.created().work.borrow_mut().retire_taken(replaced);
        if shown.search_lost {
            let lost = Accepted {
                change: Change::Selection(List::Search),
                then: None,
            };
            self.apply(hwnd, lost);
        }
        self.show_list_taken(List::Search, shown.search_deselect);
        self.start_list_passes();
    }

    /// Shows `list`'s taken rows blank, and unhighlights the rows that
    /// `deselect` says, because the list would still draw a taken row it
    /// selects highlighted.
    fn show_list_taken(&self, list: List, deselect: Deselect) {
        let control = self.list_control(list);
        match deselect {
            Deselect::Nothing => {}
            Deselect::All => control.deselect_all(),
            Deselect::Taken => self.request_taken_deselection(list),
        }
        control.redraw_rows(control.visible_rows());
    }

    /// Asks where to export the shown search, and starts the export (DESIGN.md:
    /// Export).
    fn export(&self, hwnd: Hwnd) {
        let (title, file_name, names) = {
            let model = self.model.borrow();
            let strings = model.strings();
            (
                strings.text("export.title").to_owned(),
                strings.text("export.file_name").to_owned(),
                ExportFormat::ALL
                    .map(|f| format!("{} (*.{})", strings.text(f.label()), f.extension())),
            )
        };
        let types: Vec<FileType> = ExportFormat::ALL
            .iter()
            .zip(&names)
            .map(|(format, name)| FileType {
                name,
                extension: format.extension(),
            })
            .collect();
        let Some((path, index)) = shell::save_dialog(hwnd, &title, &file_name, &types) else {
            return;
        };
        let chosen = ExportFormat::ALL
            .get(index)
            .copied()
            .unwrap_or(ExportFormat::Csv);
        let columns = self.model.borrow().columns().len();
        let order = self.created().controls.list.column_order(columns);
        let format = ExportFormat::of(&path, chosen);
        let accepted = self.model.borrow_mut().start_export(path, format, &order);
        self.apply(hwnd, accepted);
    }

    /// The message box for an export that failed (DESIGN.md: T9).
    fn show_export_failure(&self, hwnd: Hwnd, path: &Path, error: &ExportError) {
        let text = {
            let model = self.model.borrow();
            let strings = model.strings();
            let path = path.display().to_string();
            match error {
                ExportError::Write(failure) => {
                    strings.format("message.export_failed", &[&path, &windows_reason(*failure)])
                }
                ExportError::Daemon(failure) => strings.format(
                    "message.export_stopped",
                    &[&path, &captions::failure_text(strings, failure)],
                ),
            }
        };
        self.error_box(hwnd, &text);
    }

    fn error_box(&self, hwnd: Hwnd, text: &str) {
        let title = self
            .model
            .borrow()
            .strings()
            .text("window.title")
            .to_owned();
        win::dialog::error_box(Some(hwnd), text, &title);
    }

    /// Brings the window forward with the search box focused and its text
    /// selected, so the person types the next search at once (DESIGN.md: Window
    /// lifetime, C24), or in Two panels with the left panel's list focused
    /// (DESIGN.md: Two panels' window). While a dialog is open, the dialog
    /// keeps the focus.
    fn show_window(&self, hwnd: Hwnd) {
        // The columns fill when the window shows (DESIGN.md: Results list),
        // and a hidden window shows at the width it had, which no resize
        // fills. Filling before it shows draws it once.
        if !hwnd.is_visible() {
            let width = self.fill_width();
            let widths =
                (self.column_sizing.borrow_mut()).window_shown(width, || self.column_fits());
            self.set_column_widths(widths);
        }
        if self.unshown.replace(false) {
            let placement = self.model.borrow().placement();
            win::window::show(hwnd, placement);
        }
        hwnd.bring_to_front();
        self.follow_visibility(hwnd);
        if !hwnd.is_enabled() {
            return;
        }
        self.home().focus();
        let mode = self.model.borrow().mode();
        if mode == Mode::Search {
            self.created().controls.search.select_all();
        }
    }

    /// The control that the shown mode focuses first: the search box, or the
    /// active panel's list.
    fn home(&self) -> Hwnd {
        let controls = &self.created().controls;
        let (mode, active) = {
            let model = self.model.borrow();
            (model.mode(), model.panels().active())
        };
        match mode {
            Mode::Search => controls.search.hwnd(),
            Mode::TwoPanels => self.panel(active).list.hwnd(),
        }
    }

    /// Closing hides the window while Gozler keeps running in the
    /// notification area, the first time with a notification that says so,
    /// and exits otherwise (DESIGN.md: Window lifetime).
    fn close(&self, hwnd: Hwnd) {
        if self.instance == Instance::Another || !self.model.borrow().choices().keep_running {
            self.exit(hwnd);
            return;
        }
        hwnd.hide();
        self.follow_visibility(hwnd);
        // Gozler keeps running hidden, so this is when the window's
        // placement is saved, with the first hide's notice.
        let first_hide = self.model.borrow_mut().first_hide();
        self.save_settings(true);
        if !first_hide {
            return;
        }
        let (title, text) = {
            let model = self.model.borrow();
            let strings = model.strings();
            let text = match model.choices().show_shortcut {
                Some(shortcut) => {
                    strings.format("tray.notice_shortcut", &[&shortcut.text(strings)])
                }
                None => strings.text("tray.notice").to_owned(),
            };
            (strings.text("window.title").to_owned(), text)
        };
        if let Some(tray) = &mut self.created().work.borrow_mut().tray {
            tray.notify(&title, &text);
        }
    }

    /// Saves the settings and ends Gozler (DESIGN.md: C7, T14). While file
    /// operations run or wait, it asks first, and stops them
    /// (DESIGN.md: File operations), over the window, which comes forward from
    /// the notification area or another start's request. Returns whether Gozler
    /// ends; `false` when the person kept the operations.
    fn exit(&self, hwnd: Hwnd) -> bool {
        let counts = self
            .file_operations
            .get()
            .map_or_else(Default::default, FileOperations::exit_counts);
        let question = {
            let model = self.model.borrow();
            exit_question(model.strings(), &*self.numbers.borrow(), counts)
        };
        if let Some(question) = question {
            self.show_window(hwnd);
            // The question runs its own message loop, so nothing is
            // borrowed here. `WM_DESTROY` stops the operations.
            if !win::dialog::ask(hwnd, &question) {
                return false;
            }
        }
        // Each Viewer leaves its size, place and Wrap long lines as it
        // closes, so the save holds them.
        self.viewers.close_all();
        self.save_settings(true);
        hwnd.destroy();
        true
    }

    /// Adds the tray icon while Gozler keeps running in the notification
    /// area, and removes it otherwise; another window has none (DESIGN.md:
    /// Window lifetime).
    fn update_tray(&self, _hwnd: Hwnd) {
        let keeps_running =
            self.instance == Instance::First && self.model.borrow().choices().keep_running;
        let tip = self
            .model
            .borrow()
            .strings()
            .text("window.title")
            .to_owned();
        if let Some(tray) = &mut self.created().work.borrow_mut().tray {
            tray.update(keeps_running, &tip);
        }
    }

    /// A click on the tray icon, or its menu. While a dialog is open, the
    /// menu shows the window and its dialog instead, because Exit would end
    /// the window under the dialog.
    fn on_tray(&self, hwnd: Hwnd, event: TrayEvent) {
        match event {
            TrayEvent::Menu { point } if hwnd.is_enabled() => {
                let nodes =
                    commands::render_popup(&commands::tray_menu(), self.model.borrow().strings());
                let show = MenuItem::Run(CommandId::ShowWindow);
                let chosen = win::menu::Menu::track_popup_in_front(
                    hwnd,
                    &nodes,
                    point,
                    &[],
                    Some(show.id()),
                );
                if let Some(item) = chosen.and_then(MenuItem::from_id) {
                    self.run(hwnd, item);
                }
            }
            TrayEvent::Show | TrayEvent::Menu { .. } => self.show_window(hwnd),
        }
    }

    /// Registers the saved shortcut to show Gozler. One that Windows
    /// refuses is said the first time the window shows, because a message
    /// at sign-in would come before the person looks.
    fn register_saved_shortcut(&self, hwnd: Hwnd) {
        let Some(saved) = self.model.borrow().choices().show_shortcut else {
            return;
        };
        match shortcut::register(hwnd, saved, self.show_message, self.created().waker.clone()) {
            Ok(registration) => {
                self.starting_shortcut.replace(Some(registration));
                self.finish_shortcut_start(hwnd);
            }
            Err(error) => {
                self.shortcut_failure.replace(Some(error));
            }
        }
    }

    fn finish_shortcut_start(&self, hwnd: Hwnd) {
        let Some(mut registration) = self.starting_shortcut.take() else {
            return;
        };
        match registration.poll() {
            None => {
                self.starting_shortcut.replace(Some(registration));
            }
            Some(Ok(())) => {
                registration.activate();
                self.shortcut.replace(Some(registration));
            }
            Some(Err(error)) => {
                self.shortcut_failure.replace(Some(error));
                hwnd.post(DEFERRED);
            }
        }
    }

    /// What the window says when Windows refused `shortcut`: in Settings,
    /// or at start, when the saved shortcut no longer works.
    pub(super) fn shortcut_message(
        &self,
        shortcut: ShowShortcut,
        error: &RegisterError,
        at_start: bool,
    ) -> String {
        let model = self.model.borrow();
        let strings = model.strings();
        let keys = shortcut.text(strings);
        match error {
            RegisterError::Taken if at_start => strings.format("message.shortcut_taken", &[&keys]),
            RegisterError::Taken => strings.format("settings_keyboard.shortcut_taken", &[&keys]),
            RegisterError::Failed(error) => strings.format(
                "settings_keyboard.shortcut_failed",
                &[&keys, error.message().trim()],
            ),
        }
    }

    /// The shortcut that shows Gozler from any program, while Windows lets
    /// Gozler use it.
    pub(super) fn registered_shortcut(&self) -> Option<ShowShortcut> {
        self.shortcut.borrow().as_ref().map(Registration::shortcut)
    }

    /// Says why the saved shortcut does not work, once the window shows.
    fn show_shortcut_failure(&self, hwnd: Hwnd) {
        if !hwnd.is_visible() {
            return;
        }
        let Some(error) = self.shortcut_failure.take() else {
            return;
        };
        let saved = self.model.borrow().choices().show_shortcut;
        if let Some(saved) = saved {
            let text = self.shortcut_message(saved, &error, true);
            self.error_box(hwnd, &text);
        }
    }

    /// Asks GitHub for the latest release when
    /// Check for updates is on and the last check is a day old (DESIGN.md: The
    /// General page).
    fn check_for_updates(&self) {
        let now = jiff::Timestamp::now().as_second();
        if !self.model.borrow_mut().update_check_due(now) {
            return;
        }
        self.created()
            .work
            .borrow_mut()
            .check_release(ReleasePurpose::Update);
    }

    /// Asks GitHub whether its latest release is
    /// older than the window, which the fix of an older service says
    /// (DESIGN.md: Daemon failures). A reply that is not a release counts
    /// as not older.
    fn ask_latest_release(&self) {
        self.created()
            .work
            .borrow_mut()
            .check_release(ReleasePurpose::ServiceVersion);
    }

    /// A click on the status bar's right group updates Gozler while the
    /// group names a newer release (DESIGN.md: The General page).
    fn on_status_click(&self, hwnd: Hwnd) {
        let shows_update = captions::shows_update(&self.model.borrow());
        if shows_update {
            self.run_fix(hwnd, Fix::Update);
        }
    }

    /// File > Settings…: the dialog edits a copy of the settings, which OK
    /// and Apply give back through [`MainWindow::apply_choices`] (DESIGN.md:
    /// Settings dialog).
    fn open_settings(&self, _hwnd: Hwnd) {
        if self.settings.get().is_some() {
            return;
        }
        self.created().work.borrow_mut().read_registry();
    }

    fn show_settings(&self, hwnd: Hwnd, start_at_sign_in: bool) {
        self.refresh_volume_labels();
        let (choices, strings) = {
            let model = self.model.borrow();
            (model.choices().clone(), model.strings().clone())
        };
        let applied = {
            let model = self.model.borrow();
            model.service().0.and_then(|status| status.include.clone())
        };
        let include = applied.map(|applied| {
            let drives = self.include_drives(&applied);
            applied.with_drives(&connected_drives(&drives))
        });
        let values = SettingsValues {
            choices,
            start_at_sign_in,
            include,
        };
        let dialog = SettingsDialog::new(self, hwnd, strings, values);
        self.open_dialog(hwnd, DialogId::Settings, &dialog);
        self.settings.set(None);
        self.update_status();
        self.total_commander.replace(None);
        let accepted = self.model.borrow_mut().forget_total_commander();
        self.apply(hwnd, accepted);
        self.forget_included_count(hwnd);
        self.show_index_page(hwnd, false);
        if dialog.resets() {
            self.reset_all(hwnd);
        }
    }

    pub(super) fn open_repository(&self) {
        self.created()
            .work
            .borrow_mut()
            .open(ShellTarget::Repository);
    }

    /// Settings' Reset all, which the person confirmed: Gozler exits as
    /// File > Exit does, and [`super::run`] then deletes `ui.json` and
    /// opens the window again, as a new installation (DESIGN.md: Settings
    /// dialog).
    fn reset_all(&self, hwnd: Hwnd) {
        self.resetting.set(true);
        if !self.exit(hwnd) {
            self.resetting.set(false);
        }
    }

    /// Whether the window exited for Settings' Reset all.
    pub(super) fn resets_settings(&self) -> bool {
        self.resetting.get()
    }

    pub(super) fn prepare_shortcut(
        &self,
        hwnd: Hwnd,
        edited: &Choices,
        last: &Choices,
    ) -> Result<Option<Registration>, String> {
        // Another window saves the shortcut for the first window's next
        // start, because the first window holds it.
        if self.instance != Instance::First || edited.show_shortcut == last.show_shortcut {
            return Ok(None);
        }
        edited
            .show_shortcut
            .map(|shortcut| {
                shortcut::register(
                    hwnd,
                    shortcut,
                    self.show_message,
                    self.created().waker.clone(),
                )
                .map_err(|error| self.shortcut_message(shortcut, &error, false))
            })
            .transpose()
    }

    /// Applies the captured choices after their shortcut preparation succeeds;
    /// registry and Include changes are acknowledged separately.
    pub(super) fn apply_choices(
        &self,
        hwnd: Hwnd,
        edited: &Choices,
        last: &Choices,
        registration: Option<Registration>,
    ) {
        if self.instance == Instance::First && edited.show_shortcut != last.show_shortcut {
            self.starting_shortcut.take();
            if let Some(registration) = &registration {
                registration.activate();
            }
            self.shortcut.replace(registration);
            self.shortcut_failure.replace(None);
        }
        // The window draws once after every refresh below.
        hwnd.draw_once_after(|| {
            let accepted = self.model.borrow_mut().set_choices(edited.clone());
            self.apply(hwnd, accepted);
            if edited.language != last.language {
                self.set_language(hwnd, edited.language.clone());
            }
            if edited.show_icons != last.show_icons {
                win::gdi::show_icons(edited.show_icons);
                self.apply_glyphs(hwnd.dpi());
                self.viewers.apply_glyphs();
                // The menus, the button widths and the Kind list follow.
                self.refresh_text(hwnd);
            }
            if edited.show_memory != last.show_memory {
                // The group first shows the app's figure, read now, and the
                // service's from the latest status; the reply to this request
                // brings both current ones.
                if edited.show_memory {
                    self.read_app_memory();
                    self.ask_status();
                }
                self.refresh_text(hwnd);
            }
            if edited.size_format != last.size_format || edited.date_format != last.date_format {
                self.dates.borrow_mut().format = edited.date_format;
                self.follow_dates(hwnd);
                if edited.size_format != last.size_format {
                    let width = self.fill_width();
                    let widths = (self.column_sizing.borrow_mut())
                        .window_shown(width, || self.column_fits());
                    self.created()
                        .controls
                        .list
                        .hwnd()
                        .draw_once_after(|| self.set_column_widths(widths));
                }
                self.fill_panel_columns();
                let list = self.created().controls.list;
                list.redraw_rows(list.visible_rows());
                for side in Side::BOTH {
                    let list = self.panel(side).list;
                    list.redraw_rows(list.visible_rows());
                }
            }
        });
        self.save_settings(true);
        if edited.transfers_per_device != last.transfers_per_device
            && let Some(operations) = self.file_operations.get()
        {
            operations.set_transfer_limit(edited.transfers_per_device);
        }
    }

    pub(super) fn settings_closed(&self, dialog: Hwnd) {
        if self
            .settings
            .get()
            .is_some_and(|open| open.dialog == dialog)
        {
            self.settings.set(None);
        }
    }

    pub(super) fn set_sign_in(&self, on: bool) -> Id {
        self.created()
            .work
            .borrow_mut()
            .set_sign_in(on, self.settings_file.with_file_name("ui-error.log"))
    }

    pub(super) fn take_sign_in_saved(&self, id: Id) -> Option<io::Result<()>> {
        let mut answer = self.sign_in_saved.borrow_mut();
        if answer.as_ref().is_some_and(|(attempt, _)| *attempt == id) {
            answer.take().map(|(_, result)| result)
        } else {
            None
        }
    }

    pub(super) fn sign_in_is_current(&self, id: Id) -> bool {
        self.created().work.borrow().registry_is_current(id)
    }

    /// Settings shows or hides its Index page, which follows the service's
    /// state while it shows (DESIGN.md: The Drives and service page).
    pub(super) fn show_index_page(&self, hwnd: Hwnd, shown: bool) {
        self.model.borrow_mut().show_index_page(shown);
        self.update_poll(hwnd);
        if shown {
            self.ask_status();
        }
    }

    /// What the Index page shows of the service's latest state.
    pub(super) fn index_captions(&self) -> IndexCaptions {
        let labels = self.volume_labels();
        let label = |prefix: &str| label_of(&labels, prefix);
        IndexCaptions::new(&self.model.borrow(), &*self.numbers.borrow(), &label)
    }

    /// The last observed labels, including mounted folders, without reading a
    /// volume while Settings opens or refreshes.
    fn volume_labels(&self) -> Vec<(String, Option<String>)> {
        let prefixes: Vec<String> = {
            let model = self.model.borrow();
            let (status, _) = model.service();
            status
                .map(|status| {
                    let indexed = status.volumes.iter().map(|v| v.mounted());
                    let left_out = status.left_out.iter().map(|v| v.mounted());
                    indexed
                        .chain(left_out)
                        .map(|mounted| mounted.display_prefix().to_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        self.created().work.borrow_mut().labels(prefixes)
    }

    pub(super) fn refresh_volume_labels(&self) {
        self.volume_labels();
        self.created().work.borrow_mut().refresh_labels();
    }

    /// The drives Settings > Include shows: the service's, and those
    /// `settings` names that are not connected.
    pub(super) fn include_drives(&self, settings: &IndexSettings) -> Vec<IncludeDrive> {
        let labels = self.volume_labels();
        let label = |prefix: &str| label_of(&labels, prefix);
        let model = self.model.borrow();
        model
            .service()
            .0
            .map_or_else(Vec::new, |status| include_drives(status, settings, &label))
    }

    /// The GUID paths of the drives the service lists, which Settings >
    /// Include's preview reads on every edit without asking Windows for a
    /// label.
    pub(super) fn connected_volumes(&self) -> Vec<String> {
        let model = self.model.borrow();
        model.service().0.map_or_else(Vec::new, connected_volumes)
    }

    /// The entries the index holds now.
    pub(super) fn indexed_entries(&self) -> u64 {
        let model = self.model.borrow();
        model
            .service()
            .0
            .map_or(0, |status| status.volumes.iter().map(|v| v.entries).sum())
    }

    /// The rows of the summary of the indexed folders, which Settings >
    /// Include and Indexed folders show, in the person's numbers and in the
    /// language of the dialog, which keeps its own until it closes.
    pub(super) fn include_summary(
        &self,
        strings: &Strings,
        drives: &[IncludeDrive],
        settings: &IndexSettings,
    ) -> Vec<SummaryRow> {
        summary_rows(drives, settings, strings, &*self.numbers.borrow())
    }

    /// Settings > Include's preview line, in the person's numbers and the
    /// dialog's language.
    pub(super) fn include_preview(
        &self,
        strings: &Strings,
        now: u64,
        after: Option<u64>,
        adds: bool,
    ) -> String {
        preview(strings, &*self.numbers.borrow(), now, after, adds)
    }

    /// Settings > Include's preview: counts the entries the index keeps
    /// under `settings`, whose answer goes to the Settings dialog.
    pub(super) fn count_included(&self, settings: IndexSettings) {
        let fetch = self.model.borrow_mut().count_included(settings);
        self.start(fetch);
    }

    /// Include's count of the settings the dialog shows, once it arrived.
    pub(super) fn included_count(&self) -> Option<Result<u64, DaemonFailure>> {
        self.model.borrow().included_count().cloned()
    }

    /// The Include settings changed, or Settings closed: the count in
    /// flight ends and the one that arrived is forgotten. `hwnd` is the
    /// main window.
    pub(super) fn forget_included_count(&self, hwnd: Hwnd) {
        let accepted = self.model.borrow_mut().forget_included_count();
        self.apply(hwnd, accepted);
    }

    /// Starts saving the Include settings, or asking for a rebuild of the
    /// index, as administrator, after Windows asks the person for
    /// permission (DESIGN.md: The Include page, The
    /// Drives and service page). The Settings dialog starts no second job
    /// while one runs.
    pub(super) fn run_as_administrator(&self, _hwnd: Hwnd, job: Elevated) {
        let mut work = self.created().work.borrow_mut();
        work.elevated.start(job);
        let retired = work.elevated.take_retired();
        work.retire(retired);
    }

    /// Takes the job's answer once it arrived: tells the service, which
    /// applies saved settings and rebuilds when asked, and the Settings
    /// dialog.
    fn finish_include_save(&self) {
        let answer = self.created().work.borrow_mut().elevated.take();
        let Some(answer) = answer else {
            return;
        };
        if answer.is_ok() {
            let fetch = self.model.borrow_mut().settings_saved();
            self.start(fetch);
        }
        self.include_saved.replace(Some(answer));
        self.post_settings_reply();
    }

    /// The Include save's answer, once it arrived.
    pub(super) fn take_include_saved(&self) -> Option<Result<(), NotSaved>> {
        self.include_saved.take()
    }

    /// The Use Total Commander link: searches the index for Total
    /// Commander, whose answer goes to the Settings dialog (DESIGN.md: The
    /// Context menu page).
    pub(super) fn find_total_commander(&self) {
        let fetch = self.model.borrow_mut().find_total_commander();
        self.start(fetch);
    }

    /// The Use Total Commander link's answer, once it arrived.
    pub(super) fn take_total_commander(&self) -> Option<Result<Option<String>, DaemonFailure>> {
        self.total_commander.take()
    }

    /// Saves what changed since the last save: from another window, only
    /// what its Settings and its Language menu apply. A failure is shown
    /// once per session, and not while Windows ends the session.
    fn save_settings(&self, report_failure: bool) {
        let scope = match self.instance {
            Instance::First => SaveScope::Everything,
            Instance::Another => SaveScope::Choices,
        };
        let changes = {
            let mut model = self.model.borrow_mut();
            model.set_viewer(self.viewers.state());
            model.settings_changes(scope)
        };
        if let Some(changes) = changes {
            self.created()
                .work
                .borrow_mut()
                .save(&self.settings_file, changes, report_failure);
        }
    }

    fn min_window_size(&self, hwnd: Hwnd) -> Option<Size> {
        let created = self.created();
        let min_client = created.layout.borrow().as_ref()?.min_client;
        Some(win::window::window_size(
            hwnd,
            min_client,
            created.metrics.get().dpi,
        ))
    }
}

/// The width of `text` with its parts `bold` in the bold font, as
/// `CustomDraw::draw_cell` draws a matched cell.
fn matched_width(plain: &TextMeter, bold: &TextMeter, text: &str, parts: &[Range<usize>]) -> Px {
    let mut width = Px(0);
    let mut at = 0;
    for part in parts {
        width = width
            + plain.plain_width(&text[at..part.start])
            + bold.plain_width(&text[part.clone()]);
        at = part.end;
    }
    width + plain.plain_width(&text[at..])
}

/// The accelerator table of `mode`, from the command shortcuts `shortcuts`.
fn accelerator_table(
    shortcuts: &CommandShortcuts,
    mode: Mode,
) -> windows::core::Result<Accelerators> {
    let table: Vec<_> = commands::accelerators(shortcuts, mode)
        .into_iter()
        .map(|(shortcut, item)| (shortcut, item.id()))
        .collect();
    Accelerators::new(&table)
}

/// Windows' own words for why a file could not be written.
fn windows_reason(failure: WriteFailure) -> String {
    let error = match failure.os_code {
        Some(code) => std::io::Error::from_raw_os_error(code),
        None => std::io::Error::from(failure.kind),
    };
    win::error_text(&error)
}

impl WindowProc for MainWindow {
    fn message(&self, hwnd: Hwnd, message: Message<'_>) -> Option<LRESULT> {
        match message {
            Message::Create => Some(LRESULT(if self.on_create(hwnd) { 0 } else { -1 })),
            // Every other handler does nothing until WM_CREATE has made the
            // controls, because notifications arrive while it runs.
            _ if self.created.get().is_none() => None,
            // Before the window first shows, it has the placement it was
            // made with, not the saved one.
            Message::WindowPosChanged => {
                if hwnd.is_visible()
                    && let Some(placement) = hwnd.placement()
                {
                    self.model.borrow_mut().set_placement(placement);
                    self.note_saved_state(hwnd);
                }
                None
            }
            Message::Size => {
                // A minimized window has no room for its controls, and laying
                // them out would fill the columns into it, so restoring it
                // keeps the widths the person dragged.
                if !hwnd.is_minimized() {
                    self.layout(hwnd);
                }
                // Minimizing and restoring send it too.
                self.follow_visibility(hwnd);
                Some(LRESULT(0))
            }
            Message::EnterSizeMove => {
                self.column_sizing.borrow_mut().drag_started();
                None
            }
            Message::ExitSizeMove => {
                self.end_sizing();
                None
            }
            Message::DpiChanged { dpi, suggested } => {
                self.on_dpi_changed(hwnd, dpi, suggested);
                Some(LRESULT(0))
            }
            Message::GetMinMaxInfo(mut info) => {
                info.set_min_track_size(self.min_window_size(hwnd)?);
                Some(LRESULT(0))
            }
            // A window that is not a dialog keeps no focus of its own across
            // deactivation, so it gives it back to the control that had it.
            Message::Deactivate => {
                self.focus_on_activate.set(Hwnd::focused());
                None
            }
            // "Select the search text when the window is activated" is about
            // switching from another program; closing one of Gozler's own
            // dialogs gives the focus back to the control that opened it.
            Message::Activate {
                from_another_program,
            } => {
                let search = self.created().controls.search;
                let saved = self.focus_on_activate.take();
                let selects = {
                    let model = self.model.borrow();
                    model.mode() == Mode::Search && model.choices().select_on_activate
                };
                if from_another_program && selects {
                    search.hwnd().focus();
                    search.select_all();
                } else {
                    // A control that hid or turned off while the window was
                    // inactive, such as Cancel after its export, would keep
                    // the focus but take no key.
                    saved
                        .filter(|control| control.is_visible() && control.is_enabled())
                        .unwrap_or(self.home())
                        .focus();
                }
                if self.shortcut_failure.borrow().is_some() {
                    hwnd.post(DEFERRED);
                }
                Some(LRESULT(0))
            }
            Message::Command(Command::Menu(id)) => {
                self.run(hwnd, MenuItem::from_id(id)?);
                Some(LRESULT(0))
            }
            Message::Command(Command::Control(event)) => {
                self.on_control(hwnd, event);
                None
            }
            Message::Notify(mut notification) => {
                let controls = &self.created().controls;
                if let Some(request) = controls
                    .toolbars()
                    .find_map(|toolbar| toolbar.tip_request(&notification))
                {
                    let tip = request.id().and_then(|id| {
                        let tips = self.tool_tips.borrow();
                        tips.iter()
                            .find(|(tipped, _)| *tipped == id)
                            .map(|(_, tip)| tip.clone())
                    });
                    if let Some(tip) = tip {
                        request.set(&tip);
                    }
                    return Some(LRESULT(0));
                }
                if controls.filters.kind.selection_changed(&notification) {
                    self.on_kind_selected(hwnd);
                    return Some(LRESULT(0));
                }
                if controls.filters.location.selection_changed(&notification) {
                    self.on_location_selected(hwnd);
                    return Some(LRESULT(0));
                }
                if let Some(tip) = controls.filters.location.info_tip(&mut notification) {
                    return Some(self.location_tip(tip));
                }
                if controls.status.service_clicked(&notification) {
                    self.on_status_click(hwnd);
                    return Some(LRESULT(0));
                }
                if let Some(side) = Side::BOTH
                    .into_iter()
                    .find(|side| self.panel(*side).drive.dropped_down(&notification))
                {
                    self.drive_menu(hwnd, side);
                    return Some(LRESULT(0));
                }
                if let Some(side) = self.panel_list(notification.sender()) {
                    let list = self.panel(side).list.notification(notification)?;
                    return self.on_panel_notify(hwnd, side, list);
                }
                let list = controls.list.notification(notification)?;
                self.on_list_notify(hwnd, list)
            }
            Message::Timer(id) if timer::PANEL_SETTLE.contains(&id) => {
                let side = Side::BOTH[usize::from(id == timer::PANEL_SETTLE[1])];
                self.on_panel_settled(hwnd, side);
                Some(LRESULT(0))
            }
            Message::Timer(id) if timer::PANEL_AGAIN.contains(&id) => {
                hwnd.kill_timer(id);
                let side = Side::BOTH[usize::from(id == timer::PANEL_AGAIN[1])];
                self.ask_panel_sizes(side);
                Some(LRESULT(0))
            }
            Message::Timer(id) if timer::PANEL_SEARCH.contains(&id) => {
                let side = Side::BOTH[usize::from(id == timer::PANEL_SEARCH[1])];
                self.search(hwnd, List::Panel(side));
                Some(LRESULT(0))
            }
            Message::Timer(id) if timer::PANEL_SELECTION.contains(&id) => {
                let side = Side::BOTH[usize::from(id == timer::PANEL_SELECTION[1])];
                self.fetch_selection(hwnd, List::Panel(side));
                Some(LRESULT(0))
            }
            Message::Timer(timer::SEARCH) => {
                self.search(hwnd, List::Search);
                Some(LRESULT(0))
            }
            Message::Timer(timer::SEARCHING) => {
                self.name_search(hwnd, List::Search);
                Some(LRESULT(0))
            }
            Message::Timer(id) if timer::PANEL_SEARCHING.contains(&id) => {
                let side = Side::BOTH[usize::from(id == timer::PANEL_SEARCHING[1])];
                self.name_search(hwnd, List::Panel(side));
                Some(LRESULT(0))
            }
            Message::Timer(timer::STATUS) => {
                self.ask_status();
                Some(LRESULT(0))
            }
            Message::Timer(timer::AGE) => {
                self.show_age();
                self.fit_inspector();
                Some(LRESULT(0))
            }
            Message::Timer(timer::DATES) => {
                self.refresh_dates();
                Some(LRESULT(0))
            }
            Message::Timer(timer::CLIPBOARD) => {
                hwnd.kill_timer(timer::CLIPBOARD);
                let left = self.created().work.borrow_mut().retry_copy();
                if let Some(left) = left {
                    hwnd.set_timer(timer::CLIPBOARD, left.as_millis().max(1) as u32);
                }
                Some(LRESULT(0))
            }
            Message::Timer(timer::DROP_SCROLL) => {
                self.drop_scroll(hwnd);
                Some(LRESULT(0))
            }
            Message::Timer(timer::SAVE) => {
                hwnd.kill_timer(timer::SAVE);
                self.save_settings(true);
                Some(LRESULT(0))
            }
            Message::Timer(timer::WORK) => {
                self.poll_work(hwnd);
                Some(LRESULT(0))
            }
            Message::Timer(timer::SELECTION) => {
                self.fetch_selection(hwnd, List::Search);
                Some(LRESULT(0))
            }
            Message::Timer(timer::OPERATIONS_POLL) => {
                self.poll_operations(hwnd);
                Some(LRESULT(0))
            }
            Message::Timer(_) => None,
            Message::Wake => {
                self.on_wake(hwnd);
                Some(LRESULT(0))
            }
            Message::Deferred => {
                for side in Side::BOTH {
                    self.apply_native_place(side);
                }
                self.apply_taken_deselections(hwnd);
                self.read_list_selection(hwnd);
                self.read_panel_selection();
                self.resume_selection_actions(hwnd);
                if self.column_dragged.replace(false) {
                    let list = self.created().controls.list;
                    let count = self.model.borrow().columns().len();
                    let order = list.column_order(count);
                    self.model.borrow_mut().set_column_order(&order);
                }
                self.note_saved_state(hwnd);
                if self.select_search.replace(false) {
                    self.created().controls.search.select_all();
                }
                if let Some(range) = self.range_closed.take() {
                    self.choose_custom(hwnd, range);
                }
                self.show_shortcut_failure(hwnd);
                self.run_modal_steps(hwnd);
                Some(LRESULT(0))
            }
            Message::VerticalScroll { from, request }
                if from == self.created().controls.filters.pane =>
            {
                self.scroll_filters(|filters, position| filters.scrolled(position, request));
                Some(LRESULT(0))
            }
            Message::VerticalScroll { .. } => None,
            Message::MouseWheel { delta, point }
                if self.model.borrow().panes().filters
                    && self
                        .created()
                        .controls
                        .filters
                        .pane
                        .screen_rect()
                        .contains(point) =>
            {
                self.scroll_filters(|filters, position| {
                    filters.scrolled(position, ScrollRequest::Wheel(delta))
                });
                Some(LRESULT(0))
            }
            Message::MouseWheel { .. } => None,
            Message::FocusMoved { to } => {
                self.reveal(to);
                Some(LRESULT(0))
            }
            Message::ContextMenu {
                point: Some(point), ..
            } if self.on_header(point) => {
                let column = self.created().controls.list.column_at(point);
                let column =
                    column.and_then(|index| self.model.borrow().columns().get(index).copied());
                self.context_menu(
                    hwnd,
                    &commands::header_menu(column),
                    point,
                    None,
                    self.command_context(),
                );
                Some(LRESULT(0))
            }
            Message::ContextMenu { from, point } if from == self.created().controls.list.hwnd() => {
                self.row_menu(hwnd, point);
                Some(LRESULT(0))
            }
            Message::ContextMenu { from, point } if self.panel_list(from).is_some() => {
                let side = self.panel_list(from).expect("the guard found the panel");
                self.panel_menu(hwnd, side, point);
                Some(LRESULT(0))
            }
            Message::ContextMenu { .. } => None,
            Message::KeyDown {
                target,
                keystroke,
                repeat,
            } => Some(LRESULT(isize::from(
                (repeat && keystroke == Keystroke::plain(commands::Key::Escape))
                    || self.on_key(hwnd, target, keystroke),
            ))),
            Message::Char {
                target,
                character,
                alt,
            } => Some(LRESULT(isize::from(
                self.on_panel_char(target, character, alt),
            ))),
            Message::CtlColorEdit { colors, control } => self.path_box_colors(colors, control),
            // TRUE lets Windows remove the drive.
            Message::DeviceRemoval {
                removal,
                notification,
            } => {
                self.on_device_removal(hwnd, removal, notification);
                Some(LRESULT(1))
            }
            Message::InitMenuPopup => {
                self.update_menu(hwnd);
                Some(LRESULT(0))
            }
            Message::SetCursor { over, client: true }
                if over == hwnd
                    && hwnd
                        .cursor_point()
                        .is_some_and(|point| self.on_splitter(point)) =>
            {
                win::window::show_resize_cursor();
                Some(LRESULT(1))
            }
            Message::SetCursor { .. } => None,
            Message::LeftButtonDown { point } if self.on_splitter(point) => {
                let height = self
                    .created()
                    .layout
                    .borrow()
                    .as_ref()
                    .and_then(Layout::inspector)
                    .map(InspectorLayout::height);
                if let Some(height) = height {
                    self.splitter_drag.set(Some((point.1, height)));
                    hwnd.capture_mouse();
                }
                Some(LRESULT(0))
            }
            Message::LeftButtonDown { .. } => None,
            Message::MouseMove { point } => {
                let (start, height) = self.splitter_drag.get()?;
                let dragged = self
                    .created()
                    .layout
                    .borrow()
                    .as_ref()
                    .and_then(Layout::inspector)
                    .map(|inspector| inspector.dragged(height, Px(point.1 - start)));
                if let Some(dragged) = dragged {
                    let accepted = self.model.borrow_mut().set_inspector_height(dragged);
                    self.apply(hwnd, accepted);
                }
                Some(LRESULT(0))
            }
            // A drag ends when the button goes up, also when the window had
            // no capture to release.
            Message::LeftButtonUp if self.splitter_drag.take().is_some() => {
                win::window::release_mouse();
                Some(LRESULT(0))
            }
            Message::LeftButtonUp => None,
            Message::CaptureChanged => {
                self.splitter_drag.set(None);
                None
            }
            Message::CtlColorStatic { colors, control } => {
                let controls = &self.created().controls;
                let inspector = &controls.inspector;
                Some(if control == inspector.note.hwnd() {
                    colors.gray_text()
                } else if control == inspector.age.hwnd() {
                    colors.muted_text()
                } else if controls.dividers.iter().any(|d| d.hwnd() == control) {
                    colors.divider()
                } else {
                    colors.window_background()
                })
            }
            Message::SettingChange {
                regional_format: true,
            } => {
                self.numbers.replace(WindowsNumbers::new());
                self.model.borrow_mut().operation_text_changed();
                self.start_list_passes();
                hwnd.draw_once_after(|| self.refresh_text(hwnd));
                None
            }
            Message::SettingChange { .. } => None,
            Message::SysColorChange => {
                self.apply_glyphs(hwnd.dpi());
                None
            }
            Message::EndSession { ending: true } => {
                if self.ending_session.replace(true) {
                    return Some(LRESULT(0));
                }
                self.viewers.close_all();
                hwnd.hide();
                self.save_settings(false);
                hwnd.destroy();
                let shutdown = self.shutdown.borrow().clone();
                if let Some(shutdown) = shutdown {
                    shutdown.finish_session();
                }
                Some(LRESULT(0))
            }
            Message::EndSession { .. } => {
                self.ending_session.set(false);
                None
            }
            Message::Close => {
                self.close(hwnd);
                Some(LRESULT(0))
            }
            // Close delivery before releasing native endpoints; persistence and
            // accepted-mutation cleanup retain their own off-UI lifetimes.
            Message::Destroy => {
                // A window gone before the loop ended leaves nothing for
                // close_for_setup to close.
                self.closing_for_setup.set(false);
                // Exit can come inside a modal step's loop, and the steps
                // that wait for it would run against the destroyed window.
                let steps = std::mem::take(&mut *self.modal_steps.borrow_mut());
                let mut retired = Retirement::default();
                retired.push(steps, crate::core::work::INLINE_ITEMS + 1);
                retired.push(
                    self.operation_delivery.take(),
                    crate::core::work::INLINE_ITEMS + 1,
                );
                self.created().work.borrow_mut().retire(retired);
                self.retire_native_selections();
                self.drag_leave();
                self.created().waker.close();
                self.drop_target.replace(None);
                self.dropping.borrow_mut().clear();
                self.cancel_paste(None);
                self.model.borrow_mut().retire_following();
                self.retire_model();
                let recovery_finished = self.file_operations.get().map(|operations| {
                    let finished = operations.close();
                    self.created()
                        .work
                        .borrow_mut()
                        .retire(operations.take_retired());
                    finished
                });
                let (settings_finished, cleanup) = self.created().work.borrow_mut().close();
                let cleanup = win::work::spawn(cleanup);
                self.shortcut.replace(None);
                self.starting_shortcut.replace(None);
                // No Viewer outlives the message loop.
                self.viewers.close_all();
                let error_log = self.settings_file.with_file_name("ui-error.log");
                let shutdown = win::window::quit_after(move |worker| {
                    super::work::finish_shutdown(
                        worker,
                        settings_finished,
                        recovery_finished,
                        cleanup,
                        &error_log,
                    );
                });
                self.shutdown.replace(Some(shutdown));
                Some(LRESULT(0))
            }
            Message::Hotkey => {
                let accepted = self.model.borrow_mut().show_mode(Mode::Search);
                self.apply(hwnd, accepted);
                self.show_window(hwnd);
                Some(LRESULT(0))
            }
            // A second start with `--location`, such as from Total Commander
            // or a script (DESIGN.md: Window lifetime), shows Search, whose
            // filter Location is (DESIGN.md: Two panels' window).
            Message::CopyData { tag, text } if tag == self.show_message as usize => {
                let accepted = self.model.borrow_mut().show_mode(Mode::Search);
                self.apply(hwnd, accepted);
                let accepted = self.model.borrow_mut().choose_folder(text);
                self.apply(hwnd, accepted);
                self.show_window(hwnd);
                Some(LRESULT(1))
            }
            Message::CopyData { .. } => None,
            Message::Other(msg) if msg == self.show_message => {
                self.show_window(hwnd);
                Some(LRESULT(0))
            }
            // A start of another program (DESIGN.md: Window lifetime).
            // A window with a dialog open comes forward instead, because
            // the dialog's loop runs on the window.
            Message::Other(msg) if msg == self.exit_message => {
                let closes = if hwnd.is_enabled() {
                    self.exit(hwnd)
                } else {
                    self.show_window(hwnd);
                    false
                };
                let answer = if closes {
                    super::EXIT_CLOSES
                } else {
                    super::EXIT_STAYS
                };
                Some(LRESULT(answer))
            }
            Message::Other(clipboard::CLIPBOARD_CHANGED) => {
                self.model
                    .borrow_mut()
                    .set_clipboard_holds_files(clipboard::holds_files());
                Some(LRESULT(0))
            }
            Message::Other(QUEUE_CHANGED) => {
                self.on_operations(hwnd);
                Some(LRESULT(0))
            }
            Message::Other(msg) if msg == self.taskbar_button_created => {
                if let Some(operations) = self.file_operations.get() {
                    operations.forget_taskbar();
                }
                Some(LRESULT(0))
            }
            Message::Other(msg) if msg == self.taskbar_created => {
                if let Some(tray) = &mut self.created().work.borrow_mut().tray {
                    tray.restart();
                }
                Some(LRESULT(0))
            }
            Message::Other(_) => None,
        }
    }
}

/// The label of the mount prefix `prefix` in `labels`.
fn label_of(labels: &[(String, Option<String>)], prefix: &str) -> Option<String> {
    labels
        .iter()
        .find(|(p, _)| p == prefix)
        .and_then(|(_, label)| label.clone())
}
