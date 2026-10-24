use super::{
    Button, Cell, CommandButton, ControlId, Edit, FileIconLabel, Glyph, HIDE_DETAILS, Hwnd,
    INSPECTOR_BUTTONS, INSPECTOR_COPY_FOLDER, INSPECTOR_COPY_NAME, INSPECTOR_COPY_PATH, ImageList,
    Inspected, InspectorCaptions, InspectorLayout, Label, Layout, MODIFIED_ROW, MainWindow, Rect,
    SystemTime, TextMeter, Toolbar, Tooltip, captions, icon_tool, win,
};

/// The inspector's controls, in Tab order (DESIGN.md: Inspector pane).
pub(super) struct Inspector {
    pub(super) shown_icon: Cell<Option<i32>>,
    pub(super) large_images: Option<ImageList>,
    pub(super) focus: Cell<Option<Hwnd>>,
    /// The selected entry's file icon.
    pub(super) icon: FileIconLabel,
    pub(super) heading: Label,
    /// [`INSPECTOR_COPY_NAME`], after the heading.
    pub(super) copy_name: Toolbar,
    /// [`HIDE_DETAILS`], at the right end of the heading's row.
    pub(super) hide: Toolbar,
    /// [`INSPECTOR_COPY_FOLDER`], after the folder's value.
    pub(super) copy_folder: Toolbar,
    /// [`INSPECTOR_COPY_PATH`], at the end of the Date modified row.
    pub(super) copy_path: Button,
    /// In [`INSPECTOR_BUTTONS`]' order.
    pub(super) buttons: [Button; INSPECTOR_BUTTONS.len()],
    /// The text buttons' tips.
    pub(super) tips: Tooltip,
    /// Each field row's label and its value, a read-only box, so the person
    /// can select and copy it.
    pub(super) fields: Vec<(Label, Edit)>,
    /// The muted age after Date modified's value.
    pub(super) age: Label,
    /// The gray hint while nothing is selected.
    pub(super) note: Label,
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
    pub(super) fn create(parent: Hwnd) -> windows::core::Result<Inspector> {
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
            shown_icon: Cell::new(None),
            large_images: ImageList::system_large(),
            focus: Cell::new(None),
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
    pub(super) fn text_buttons(&self) -> [(CommandButton, Button); INSPECTOR_BUTTONS.len() + 1] {
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
    pub(super) fn icon_tools(&self) -> [(CommandButton, Toolbar); 3] {
        [
            (INSPECTOR_COPY_NAME, self.copy_name),
            (INSPECTOR_COPY_FOLDER, self.copy_folder),
            (HIDE_DETAILS, self.hide),
        ]
    }

    /// Every control that shows text, but the heading, which has a font of
    /// its own.
    pub(super) fn controls(&self) -> Vec<Hwnd> {
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

    pub(super) fn contains(&self, control: Hwnd) -> bool {
        control == self.heading.hwnd() || self.controls().contains(&control)
    }

    /// The value that `control` is, if it is one.
    pub(super) fn value(&self, control: Hwnd) -> Option<Edit> {
        self.fields
            .iter()
            .map(|(_, value)| *value)
            .find(|value| value.hwnd() == control)
    }
}

/// The inspector's texts fitted to their lines and the places of the
/// controls that show them ([`Inspector::inspector_fit`]).
pub(super) struct InspectorFit {
    pub(super) moves: Vec<(Hwnd, Rect)>,
    pub(super) texts: Vec<(Edit, String)>,
    pub(super) one_entry: bool,
    pub(super) folder_shown: bool,
}

impl Inspector {
    /// Shows the selection in the inspector, if anything it shows changed.
    pub(super) fn update_inspector(&self, window: &MainWindow) {
        let captions = InspectorCaptions::new(&window.model.borrow(), &*window.numbers.borrow());
        self.show_inspector(window, captions);
    }

    pub(super) fn show_inspector(&self, window: &MainWindow, captions: InspectorCaptions) {
        if window.model.borrow().panes().content == crate::core::layout::InspectorContent::Transfers
        {
            return;
        }
        let inspector = self;
        inspector.heading.hwnd().set_text(&captions.heading);
        for (index, (label, _)) in inspector.fields.iter().enumerate() {
            let label_text = captions
                .fields
                .get(index)
                .map_or("", |(label, _)| label.as_str());
            label.hwnd().set_text(label_text);
        }
        inspector
            .note
            .hwnd()
            .set_text(captions.note().unwrap_or(""));
        // The shell call can dispatch messages, so no borrow is held
        // (DESIGN.md: window rule 1).
        let icon = match &captions.inspected {
            Inspected::One {
                name, is_folder, ..
            } => Some(window.icons.index(name, *is_folder)),
            _ => None,
        };
        if !window.created().waker.while_open(|| {}) {
            return;
        }
        let current = {
            let model = window.model.borrow();
            model.panes().content == crate::core::layout::InspectorContent::Selection
                && InspectorCaptions::new(&model, &*window.numbers.borrow()) == captions
        };
        if !current {
            return;
        }
        let icon_changed = self.shown_icon.replace(icon) != icon;
        self.show_age(window);
        self.show_fields(window);
        if icon_changed {
            inspector.icon.show_file_icon(self.large_images.zip(icon));
        }
    }

    /// Writes how long ago the shown entry was modified, from the current
    /// time, so a minute's refresh keeps it true (DESIGN.md: Inspector pane).
    pub(super) fn show_age(&self, window: &MainWindow) {
        let age = window.model.borrow().inspected_modified().map(|modified| {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |since| {
                    i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
                });
            captions::age(
                window.model.borrow().strings(),
                &*window.numbers.borrow(),
                modified,
                gozler_core::output::filetime::unix_ms_to_filetime(now),
            )
        });
        let inspector = self;
        inspector.age.hwnd().set_text(age.as_deref().unwrap_or(""));
    }

    pub(super) fn show_fields(&self, window: &MainWindow) {
        self.show_field_rows(window);
        self.fit_inspector(window);
    }

    /// Shows each field row that has a field and room in the pane, and
    /// hides the others; the note and the age likewise.
    pub(super) fn show_field_rows(&self, window: &MainWindow) {
        let (rows, note, age) = {
            let layout = window.created().layout.borrow();
            let transfers = window.model.borrow().panes().content
                == crate::core::layout::InspectorContent::Transfers;
            let placed = layout
                .as_ref()
                .and_then(Layout::inspector)
                .filter(|_| !transfers);
            let room = placed.map_or(0, |i| i.fields.len());
            let shown = InspectorCaptions::new(&window.model.borrow(), &*window.numbers.borrow());
            let rows = room.min(shown.fields.len());
            (
                rows,
                placed.is_some_and(|i| i.note.is_some()) && shown.note().is_some(),
                MODIFIED_ROW < rows && shown.modified().is_some(),
            )
        };
        let inspector = self;
        for (index, (label, value)) in inspector.fields.iter().enumerate() {
            label.hwnd().set_visible(index < rows);
            value.hwnd().set_visible(index < rows);
        }
        inspector.note.hwnd().set_visible(note);
        inspector.age.hwnd().set_visible(age);
    }

    /// Fits the inspector's texts to their lines in the shown layout
    /// ([`Self::inspector_fit`]), when they change apart from a layout.
    pub(super) fn fit_inspector(&self, window: &MainWindow) {
        if window.model.borrow().panes().content == crate::core::layout::InspectorContent::Transfers
        {
            return;
        }
        let fit = (window.created().layout.borrow().as_ref())
            .and_then(Layout::inspector)
            .map(|placed| self.inspector_fit(window, placed));
        self.show_inspector_fit(fit);
    }

    /// Fits the inspector's texts to their lines in `placed`: a copy icon
    /// stands 8 px after the text it copies, for one entry only, whose name
    /// and folder it copies, the age a space after the modification time,
    /// and a value too long for its line ends in "…", a path losing its
    /// middle as Windows shortens one (DESIGN.md: Inspector pane). The name
    /// and the age end in an ellipsis themselves, and a value that has the
    /// focus keeps its whole text, so the person selects what is there.
    pub(super) fn inspector_fit(
        &self,
        window: &MainWindow,
        placed: &InspectorLayout,
    ) -> InspectorFit {
        let created = window.created();
        let inspector = &created.controls.inspector;
        let shown = InspectorCaptions::new(&window.model.borrow(), &*window.numbers.borrow());
        // The copy icons copy one entry's name and folder.
        let one_entry = matches!(shown.inspected, Inspected::One { .. });
        let fonts = created.fonts.borrow();
        let meter = TextMeter::new(&fonts.ui);
        let focused = Hwnd::focused();
        let mut moves = Vec::new();
        let mut heading = placed.heading;
        let mut folder_shown = false;
        if one_entry {
            let width = TextMeter::new(&fonts.heading).plain_width(&shown.heading);
            let (text, icon) = placed.copy_beside(heading, width);
            heading = text;
            moves.push((inspector.copy_name.hwnd(), icon));
        }
        moves.push((inspector.heading.hwnd(), heading));
        let mut texts: Vec<(Edit, String)> = Vec::new();
        for (index, ((_, line), &(_, value))) in
            placed.fields.iter().zip(&inspector.fields).enumerate()
        {
            let text = shown.fields.get(index).map_or("", |(_, v)| v.as_str());
            let margins = value.margins();
            let mut rect = *line;
            if index == 0 && one_entry {
                let (fitted, icon) = placed.copy_beside(rect, meter.plain_width(text) + margins);
                rect = fitted;
                moves.push((inspector.copy_folder.hwnd(), icon));
                folder_shown = true;
            } else if index == MODIFIED_ROW && shown.modified().is_some() {
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
    pub(super) fn show_inspector_fit(&self, fit: Option<InspectorFit>) {
        let inspector = self;
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

    pub(super) fn remember_inspector_focus(&self, window: &MainWindow) {
        if self.focus.get().is_none()
            && let Some(focus) = Hwnd::focused().filter(|focus| {
                !window
                    .file_operations
                    .get()
                    .is_some_and(|ops| ops.contains(*focus))
            })
        {
            self.focus.set(Some(focus));
        }
    }
}

impl Inspector {
    /// The inspector's button that `event` says the person
    /// clicked, with what it runs.
    fn command_button_clicked(&self, event: super::ControlEvent) -> Option<CommandButton> {
        let inspector = self;
        let text = inspector
            .text_buttons()
            .into_iter()
            .find(|(_, control)| control.clicked(event))
            .map(|(button, _)| button);
        let icon = || {
            inspector
                .icon_tools()
                .into_iter()
                .find(|(button, toolbar)| toolbar.clicked(event) == Some(button.control.id()))
                .map(|(button, _)| button)
        };
        text.or_else(icon)
    }

    pub(super) fn on_control(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        event: super::ControlEvent,
    ) -> bool {
        if self
            .fields
            .iter()
            .any(|(_, value)| value.focused(event) || value.unfocused(event))
        {
            self.fit_inspector(window);
            return true;
        }
        let Some(button) = self.command_button_clicked(event) else {
            return false;
        };
        window.run(hwnd, super::MenuItem::Run(button.command));
        true
    }
}

impl Inspector {
    pub(super) fn set_captions(&self, window: &MainWindow, captions: &super::WindowCaptions) {
        let inspector = self;
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
        self.show_inspector(window, captions.inspector.clone());
    }
}

impl Inspector {
    pub(super) fn refresh_commands(&self, window: &MainWindow) {
        let (buttons, icons) = {
            let model = window.model.borrow();
            let state = super::State {
                model: &model,
                operations_ready: window.file_operations.get().is_some(),
                focus: super::Focus::Other,
                list: model.active_list(),
            };
            (
                self.text_buttons()
                    .map(|(button, control)| (control, (button.command.row().enabled)(&state))),
                self.icon_tools().map(|(button, toolbar)| {
                    (
                        toolbar,
                        button.control,
                        (button.command.row().enabled)(&state),
                    )
                }),
            )
        };
        for (button, enabled) in buttons {
            button.hwnd().set_enabled(enabled);
        }
        for (toolbar, id, enabled) in icons {
            toolbar.set_enabled(id.id(), enabled);
        }
    }
}
