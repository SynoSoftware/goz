//! The window's dialogs. Each copies the text it shows before it opens, so
//! no model borrow is held while its modal loop runs (DESIGN.md:
//! window rule 1). `MainWindow::open_dialog` runs each, and the dialog module
//! sets its fixed captions from `core::captions::CaptionTable`.

use super::main_window::MainWindow;
use super::work::REPOSITORY;
use crate::core::captions::DialogItem;
use crate::core::ids::ControlId;
use crate::core::query::{DateRange, DateRangeError, Day, SizeRange, SizeRangeError, SizeUnit};
use crate::core::text::Strings;
use crate::core::typed::{self, Refusal};
use crate::win::controls::{Balloon, DatePicker, DropDownList, Edit, Tooltip};
use crate::win::dialog::{self, DialogEnd, DialogMessage, DialogProc, Opened};
use crate::win::gdi::{Font, TextMeter};
use crate::win::{self, Hwnd};
use gozler_fileops::{Destination, Policy};
use std::cell::{Cell, OnceCell, RefCell};

/// The About Gozler dialog (DESIGN.md: C23).
pub(super) struct About<'a> {
    main: &'a MainWindow,
    version: String,
    service: String,
}

impl<'a> About<'a> {
    /// `service_version` is the version of the service the window is
    /// connected to, if any.
    pub(super) fn new(
        main: &'a MainWindow,
        strings: &Strings,
        service_version: Option<&str>,
    ) -> About<'a> {
        About {
            main,
            version: strings.format("about.version", &[gozler_core::escompat::version()]),
            service: match service_version {
                Some(version) => strings.format("about.service", &[version]),
                None => strings.text("about.service_unknown").to_owned(),
            },
        }
    }
}

impl DialogProc for About<'_> {
    fn init(&self, dialog: Hwnd) -> Opened {
        let mut fonts = Vec::new();
        if let Some(version) = dialog::item(dialog, ControlId::AboutVersion.id()) {
            version.set_text(&self.version);
            if let Ok(font) = Font::of(version).and_then(|font| font.semibold()) {
                version.set_font(&font);
                fonts.push(font);
            }
        }
        if let Some(service) = dialog::item(dialog, ControlId::AboutService.id()) {
            service.set_text(&self.service);
        }
        // The address is not translated, so it is the link's text.
        let address = REPOSITORY.trim_start_matches("https://");
        if let Some(link) = dialog::item(dialog, ControlId::AboutLink.id()) {
            link.set_text(&format!("<a>{address}</a>"));
        }
        // Close starts with the focus, so Enter closes the dialog instead
        // of opening the link.
        Opened {
            focus: DialogItem::Cancel.id(),
            fonts,
        }
    }

    fn message(&self, _dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::LinkActivated { id } if id == ControlId::AboutLink.id() => {
                self.main.settings.open_repository(self.main);
                true
            }
            DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Command { .. }
            | DialogMessage::Edited { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Tree { .. }
            | DialogMessage::Focus { .. }
            | DialogMessage::KeysCaptured { .. }
            | DialogMessage::Other => false,
        }
    }
}

/// The Search syntax dialog (DESIGN.md: C22): `gozler`'s search syntax, which
/// is English in every language, as the syntax itself is.
pub(super) struct SearchSyntax;

impl DialogProc for SearchSyntax {
    fn init(&self, dialog: Hwnd) -> Opened {
        let mut fonts = Vec::new();
        if let Some(text) = dialog::item(dialog, ControlId::SearchSyntaxText.id()) {
            if let Ok(font) = Font::monospace_of(text) {
                text.set_font(&font);
                fonts.push(font);
            }
            // An edit control breaks lines only at "\r\n".
            text.set_text(&gozler_core::escompat::search_syntax().replace('\n', "\r\n"));
        }
        // Close starts with the focus, so the text is not selected and
        // Enter closes the dialog.
        Opened {
            focus: DialogItem::Cancel.id(),
            fonts,
        }
    }

    fn message(&self, _dialog: Hwnd, _message: DialogMessage) -> bool {
        false
    }
}

/// The service's report (DESIGN.md: The Drives and service page): the text
/// that `gozler --status` prints, in English, as `gozler` prints it, with
/// Copy and Close.
pub(super) struct ServiceReport {
    pub(super) text: String,
}

impl DialogProc for ServiceReport {
    fn init(&self, dialog: Hwnd) -> Opened {
        let mut fonts = Vec::new();
        if let Some(text) = dialog::item(dialog, ControlId::ServiceReportText.id()) {
            // The report aligns its values in a column.
            if let Ok(font) = Font::monospace_of(text) {
                text.set_font(&font);
                fonts.push(font);
            }
            // An edit control breaks lines only at "\r\n".
            text.set_text(&self.text.replace('\n', "\r\n"));
        }
        // Close starts with the focus, so the text is not selected and
        // Enter closes the dialog.
        Opened {
            focus: DialogItem::Cancel.id(),
            fonts,
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Command { id } if id == ControlId::ServiceReportCopy.id() => {
                // The whole report, as Windows' own Copy details buttons
                // copy, selected so the person sees what was copied.
                if let Some(text) = Edit::of_dialog(dialog, ControlId::ServiceReportText.id()) {
                    text.select_all();
                    text.copy();
                }
                true
            }
            DialogMessage::Command { .. }
            | DialogMessage::Edited { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Tree { .. }
            | DialogMessage::Focus { .. }
            | DialogMessage::KeysCaptured { .. }
            | DialogMessage::Other => false,
        }
    }
}

/// The Custom date dialog: From and To, either of which may be left open
/// (DESIGN.md: Filter pane, Dialogs).
pub(super) struct CustomDate {
    /// The range the dialog opens with.
    shown: Option<DateRange>,
    /// Where a new range starts: a week before today, because a range from
    /// today alone is what the Today choice already gives.
    week_ago: jiff::civil::Date,
    order: String,
    balloon: OnceCell<Balloon>,
    chosen: Cell<Option<DateRange>>,
}

impl CustomDate {
    pub(super) fn new(strings: &Strings, shown: Option<DateRange>) -> CustomDate {
        CustomDate {
            shown,
            week_ago: jiff::Zoned::now()
                .date()
                .saturating_sub(jiff::Span::new().days(7)),
            order: strings.text("custom_date.order").to_owned(),
            balloon: OnceCell::new(),
            chosen: Cell::new(None),
        }
    }

    /// The range OK chose.
    pub(super) fn chosen(&self) -> Option<DateRange> {
        self.chosen.get()
    }

    fn pickers(dialog: Hwnd) -> Option<(DatePicker, DatePicker)> {
        Some((
            DatePicker::of_dialog(dialog, ControlId::CustomDateFrom.id())?,
            DatePicker::of_dialog(dialog, ControlId::CustomDateTo.id())?,
        ))
    }

    fn read(dialog: Hwnd) -> Result<DateRange, DateRangeError> {
        let (from, to) = Self::pickers(dialog).ok_or(DateRangeError::Neither)?;
        DateRange::new(from.day().map(Day), to.day().map(Day))
    }

    /// OK stays disabled while neither check box is on (DESIGN.md: Dialogs).
    fn update_ok(dialog: Hwnd) {
        if let Some(ok) = dialog::item(dialog, DialogItem::Ok.id()) {
            ok.set_enabled(!matches!(Self::read(dialog), Err(DateRangeError::Neither)));
        }
    }
}

impl DialogProc for CustomDate {
    fn init(&self, dialog: Hwnd) -> Opened {
        let (from, to) = match self.shown {
            Some(range) => range.days(),
            None => (Some(Day(self.week_ago)), None),
        };
        if let Some((from_picker, to_picker)) = Self::pickers(dialog) {
            from_picker.set_day(from.map(|d| d.0));
            to_picker.set_day(to.map(|d| d.0));
        }
        if let Ok(balloon) = Balloon::create(dialog) {
            let _ = self.balloon.set(balloon);
        }
        Self::update_ok(dialog);
        Opened {
            focus: ControlId::CustomDateFrom.id(),
            fonts: Vec::new(),
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Edited { .. } => {
                if let Some(balloon) = self.balloon.get() {
                    balloon.hide();
                }
                Self::update_ok(dialog);
                true
            }
            DialogMessage::Command { id } if id == DialogItem::Ok.id() => {
                match Self::read(dialog) {
                    Ok(range) => {
                        self.chosen.set(Some(range));
                        dialog::end(dialog, DialogEnd::Ok);
                    }
                    Err(DateRangeError::FromAfterTo) => {
                        if let Some(to) = dialog::item(dialog, ControlId::CustomDateTo.id()) {
                            to.focus();
                            if let Some(balloon) = self.balloon.get() {
                                balloon.show(to, &self.order);
                            }
                        }
                    }
                    // OK is disabled then.
                    Err(DateRangeError::Neither) => {}
                }
                true
            }
            DialogMessage::Command { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Tree { .. }
            | DialogMessage::Focus { .. }
            | DialogMessage::KeysCaptured { .. }
            | DialogMessage::Other => false,
        }
    }
}

/// The Custom size dialog: Minimum and Maximum, either of which may stay
/// empty, in one unit (DESIGN.md: Filter pane, Dialogs).
pub(super) struct CustomSize {
    /// The range the dialog opens with.
    shown: Option<SizeRange>,
    separator: String,
    /// In [`SizeUnit::ALL`]'s order.
    units: [String; 3],
    not_a_number: String,
    order: String,
    balloon: OnceCell<Balloon>,
    chosen: Cell<Option<SizeRange>>,
}

impl CustomSize {
    pub(super) fn new(strings: &Strings, separator: &str, shown: Option<SizeRange>) -> CustomSize {
        CustomSize {
            shown,
            separator: separator.to_owned(),
            units: SizeUnit::ALL.map(|unit| strings.text(unit.label()).to_owned()),
            not_a_number: strings.format("custom_size.not_a_number", &[&format!("1{separator}5")]),
            order: strings.text("custom_size.order").to_owned(),
            balloon: OnceCell::new(),
            chosen: Cell::new(None),
        }
    }

    /// The range OK chose.
    pub(super) fn chosen(&self) -> Option<SizeRange> {
        self.chosen.get()
    }

    fn read(&self, dialog: Hwnd) -> Result<SizeRange, SizeRangeError> {
        let text = |id: ControlId| {
            dialog::item(dialog, id.id())
                .map(Hwnd::text)
                .unwrap_or_default()
        };
        let unit = DropDownList::of_dialog(dialog, ControlId::CustomSizeUnit.id())
            .and_then(DropDownList::selected)
            .and_then(|index| SizeUnit::ALL.get(index).copied())
            .unwrap_or_default();
        SizeRange::parse(
            &text(ControlId::CustomSizeMinimum),
            &text(ControlId::CustomSizeMaximum),
            unit,
            &self.separator,
        )
    }

    /// OK stays disabled while both fields are empty, as Custom date's
    /// while neither check box is on.
    fn update_ok(&self, dialog: Hwnd) {
        if let Some(ok) = dialog::item(dialog, DialogItem::Ok.id()) {
            ok.set_enabled(!matches!(self.read(dialog), Err(SizeRangeError::Neither)));
        }
    }

    /// Points the balloon `text` at the field `field` and focuses it.
    fn refuse(&self, dialog: Hwnd, field: ControlId, text: &str) {
        if let Some(field) = dialog::item(dialog, field.id()) {
            field.focus();
            if let Some(balloon) = self.balloon.get() {
                balloon.show(field, text);
            }
        }
    }
}

impl DialogProc for CustomSize {
    fn init(&self, dialog: Hwnd) -> Opened {
        let unit = self.shown.map(|range| range.unit).unwrap_or_default();
        if let Some(range) = &self.shown {
            let (minimum, maximum) = range.fields(&self.separator);
            for (id, text) in [
                (ControlId::CustomSizeMinimum, minimum),
                (ControlId::CustomSizeMaximum, maximum),
            ] {
                if let Some(field) = dialog::item(dialog, id.id()) {
                    field.set_text(&text);
                }
            }
        }
        if let Some(units) = DropDownList::of_dialog(dialog, ControlId::CustomSizeUnit.id()) {
            let index = SizeUnit::ALL
                .iter()
                .position(|u| *u == unit)
                .expect("ALL holds every unit");
            units.set_items(&self.units, index);
        }
        if let Ok(balloon) = Balloon::create(dialog) {
            let _ = self.balloon.set(balloon);
        }
        self.update_ok(dialog);
        Opened {
            focus: ControlId::CustomSizeMinimum.id(),
            fonts: Vec::new(),
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Edited { .. } => {
                if let Some(balloon) = self.balloon.get() {
                    balloon.hide();
                }
                self.update_ok(dialog);
                true
            }
            DialogMessage::Command { id } if id == DialogItem::Ok.id() => {
                match self.read(dialog) {
                    Ok(range) => {
                        self.chosen.set(Some(range));
                        dialog::end(dialog, DialogEnd::Ok);
                    }
                    Err(SizeRangeError::MinimumNotANumber) => {
                        self.refuse(dialog, ControlId::CustomSizeMinimum, &self.not_a_number);
                    }
                    Err(SizeRangeError::MaximumNotANumber) => {
                        self.refuse(dialog, ControlId::CustomSizeMaximum, &self.not_a_number);
                    }
                    Err(SizeRangeError::MinimumAboveMaximum) => {
                        self.refuse(dialog, ControlId::CustomSizeMinimum, &self.order);
                    }
                    // OK is disabled then.
                    Err(SizeRangeError::Neither) => {}
                }
                true
            }
            DialogMessage::Command { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Tree { .. }
            | DialogMessage::Focus { .. }
            | DialogMessage::KeysCaptured { .. }
            | DialogMessage::Other => false,
        }
    }
}

/// Copy to folder… and Move to folder…: the destination as
/// text, which only the text decides, so the dialog closes at once
/// (DESIGN.md: Copy and Move).
pub(super) struct Transfer {
    line: String,
    /// The box's text when it opens, and the part of it selected.
    start: String,
    selected: (usize, usize),
    /// The active panel's folder, which a relative path is in.
    folder: String,
    /// The known folder prefix, which [`typed::destination`] keeps unexpanded.
    other: RefCell<String>,
    /// The name of the one entry it acts on; `None` for several.
    one: Option<String>,
    title: String,
    characters: String,
    reserved: String,
    policy: Policy,
    policies: [String; 4],
    mode: TransferMode,
    sync_hint: String,
    mirror_hint: String,
    chosen: RefCell<Option<(Destination, TransferAction)>>,
}

pub(super) enum TransferAction {
    Transfer(Policy),
    Synchronize { mirror: bool },
}

pub(super) enum TransferMode {
    Copy { single_folder: bool },
    Move,
}

/// What [`Transfer`] opens with.
pub(super) struct TransferStart {
    pub(super) policy: Policy,
    pub(super) mode: TransferMode,
    /// The dialog's title, which names its command.
    pub(super) title: String,
    /// What it acts on, such as "Copy report.pdf to:".
    pub(super) line: String,
    pub(super) text: String,
    pub(super) selected: (usize, usize),
    pub(super) folder: String,
    pub(super) other: String,
    pub(super) one: Option<String>,
}

impl Transfer {
    pub(super) fn new(strings: &Strings, start: TransferStart) -> Transfer {
        Transfer {
            policy: start.policy,
            policies: crate::core::file_operations::policy_names(strings).map(str::to_owned),
            line: start.line,
            start: start.text,
            selected: start.selected,
            folder: start.folder,
            other: RefCell::new(start.other),
            one: start.one,
            title: start.title,
            characters: strings.text("name.characters").to_owned(),
            reserved: strings.text("name.reserved").to_owned(),
            mode: start.mode,
            sync_hint: strings.text("transfer.sync_hint").to_owned(),
            mirror_hint: strings.text("transfer.mirror_hint").to_owned(),
            chosen: RefCell::new(None),
        }
    }

    pub(super) fn chosen(&self) -> Option<(Destination, TransferAction)> {
        self.chosen.take()
    }

    fn path(dialog: Hwnd) -> Option<Edit> {
        Edit::of_dialog(dialog, ControlId::TransferPath.id())
    }

    /// Copy or Move stays disabled while the box is empty.
    fn update_ok(&self, dialog: Hwnd) {
        let empty = Transfer::path(dialog).is_none_or(|path| path.text().trim().is_empty());
        if let Some(ok) = dialog::item(dialog, DialogItem::Ok.id()) {
            ok.set_enabled(!empty);
        }
        for id in [ControlId::TransferMirror, ControlId::TransferSync] {
            if let Some(button) = dialog::item(dialog, id.id()) {
                button.set_enabled(
                    !empty
                        && matches!(
                            self.mode,
                            TransferMode::Copy {
                                single_folder: true
                            }
                        ),
                );
            }
        }
    }
}

impl DialogProc for Transfer {
    fn init(&self, dialog: Hwnd) -> Opened {
        if let Some(hint) = dialog::item(dialog, ControlId::TransferPolicyHint.id())
            && let Ok(font) = Font::of(hint)
        {
            let rect = hint.rect_in(dialog);
            let extra = TextMeter::new(&font).width(&hint.text()) - rect.width;
            if extra > crate::core::layout::Px(0) {
                dialog::widen(dialog, rect.x + crate::core::layout::Px(1), extra);
            }
        }
        if let Some(line) = dialog::item(dialog, ControlId::TransferLine.id()) {
            line.set_text(&self.line);
        }
        if let Some(path) = Transfer::path(dialog) {
            path.hwnd().set_text(&self.start);
            path.select(self.selected.0, self.selected.1);
        }
        if let Some(list) = DropDownList::of_dialog(dialog, ControlId::TransferPolicy.id()) {
            let selected = Policy::ALL
                .iter()
                .position(|policy| *policy == self.policy)
                .unwrap_or(0);
            list.set_items(&self.policies, selected);
        }
        let buttons: Vec<_> = [ControlId::TransferMirror, ControlId::TransferSync]
            .into_iter()
            .filter_map(|id| dialog::item(dialog, id.id()))
            .collect();
        for button in &buttons {
            button.set_visible(matches!(self.mode, TransferMode::Copy { .. }));
            if matches!(self.mode, TransferMode::Move) {
                button.set_text("");
            }
        }
        if let Ok(tips) = Tooltip::create(dialog, &buttons) {
            for (button, hint) in buttons.iter().zip([&self.mirror_hint, &self.sync_hint]) {
                tips.set(*button, hint);
            }
        }
        self.update_ok(dialog);
        Opened {
            focus: ControlId::TransferPath.id(),
            fonts: Vec::new(),
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Edited { .. } => {
                self.update_ok(dialog);
                true
            }
            DialogMessage::Command { id } if id == ControlId::TransferBrowse.id() => {
                if let Some(path) = Transfer::path(dialog) {
                    let start = path.text();
                    if let Some(folder) = win::shell::pick_folder(dialog, Some(&start)) {
                        let text = crate::core::panel::join(&folder, "");
                        self.other.replace(text.clone());
                        path.hwnd().set_text(&text);
                        path.hwnd().focus();
                    }
                }
                true
            }
            DialogMessage::Command { id }
                if id == DialogItem::Ok.id()
                    || id == ControlId::TransferSync.id()
                    || id == ControlId::TransferMirror.id() =>
            {
                let synchronize = id != DialogItem::Ok.id();
                if synchronize
                    && !matches!(
                        self.mode,
                        TransferMode::Copy {
                            single_folder: true
                        }
                    )
                {
                    return true;
                }
                let Some(path) = Transfer::path(dialog) else {
                    return true;
                };
                let resolved = typed::destination(
                    &path.text(),
                    &self.folder,
                    &self.other.borrow(),
                    self.one.as_deref(),
                    |name| std::env::var(name).ok(),
                );
                match resolved {
                    Ok(Some(destination)) => {
                        let policy =
                            DropDownList::of_dialog(dialog, ControlId::TransferPolicy.id())
                                .and_then(DropDownList::selected)
                                .and_then(|i| Policy::ALL.get(i).copied())
                                .unwrap_or(self.policy);
                        let action = if synchronize {
                            TransferAction::Synchronize {
                                mirror: id == ControlId::TransferMirror.id(),
                            }
                        } else {
                            TransferAction::Transfer(policy)
                        };
                        self.chosen.replace(Some((destination, action)));
                        dialog::end(dialog, DialogEnd::Ok);
                    }
                    // Copy or Move is disabled then.
                    Ok(None) => {}
                    Err(refusal) => {
                        let text = refusal_text(&refusal, &self.characters, &self.reserved);
                        dialog::error_box(Some(dialog), &text, &self.title);
                        path.hwnd().focus();
                    }
                }
                true
            }
            DialogMessage::Command { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Tree { .. }
            | DialogMessage::Focus { .. }
            | DialogMessage::KeysCaptured { .. }
            | DialogMessage::Other => false,
        }
    }

    fn buttons_sized(&self, dialog: Hwnd, width: crate::core::layout::Px) {
        use crate::core::layout::{Px, Rect};
        let ids = [
            ControlId::TransferMirror.id(),
            ControlId::TransferSync.id(),
            DialogItem::Ok.id(),
            DialogItem::Cancel.id(),
        ];
        let buttons: Vec<_> = ids
            .into_iter()
            .filter_map(|id| dialog::item(dialog, id))
            .collect();
        if buttons.len() != 4 || matches!(self.mode, TransferMode::Move) {
            return;
        }
        let first = buttons[0].rect_in(dialog);
        let margin = first.x;
        let gap = buttons[1].rect_in(dialog).x - first.right();
        let required = margin + margin + Px(width.0 * 4) + Px(gap.0 * 4);
        let extra = required - dialog.client_size().width;
        if extra > Px(0) {
            dialog::widen(dialog, margin + Px(1), extra);
        }
        let right = dialog.client_size().width - margin;
        let positions = [
            margin,
            margin + width + gap,
            right - width - width - gap,
            right - width,
        ];
        for (button, x) in buttons.into_iter().zip(positions) {
            button.set_rect(Rect { x, width, ..first });
        }
    }
}

/// What a message box says about a typed name that Windows refuses.
fn refusal_text(refusal: &Refusal, characters: &str, reserved: &str) -> String {
    match refusal {
        Refusal::Characters => characters.to_owned(),
        Refusal::Reserved(name) => reserved.replace("{0}", name),
    }
}

/// Two panels' New folder…: the new folder's name (DESIGN.md: New folder).
pub(super) struct NewFolder<'a> {
    /// The name of the panel's row that shows a name, without regard to
    /// case.
    shown_name: &'a dyn Fn(&str) -> Option<String>,
    title: String,
    characters: String,
    reserved: String,
    exists: String,
    balloon: OnceCell<Balloon>,
    chosen: RefCell<Option<String>>,
}

impl<'a> NewFolder<'a> {
    pub(super) fn new(
        strings: &Strings,
        shown_name: &'a dyn Fn(&str) -> Option<String>,
    ) -> NewFolder<'a> {
        NewFolder {
            shown_name,
            title: strings.text("file_operations.new_folder").to_owned(),
            characters: strings.text("name.characters").to_owned(),
            reserved: strings.text("name.reserved").to_owned(),
            exists: strings.text("new_folder.exists").to_owned(),
            balloon: OnceCell::new(),
            chosen: RefCell::new(None),
        }
    }

    /// The name Create chose, trimmed as Windows trims it.
    pub(super) fn chosen(&self) -> Option<String> {
        self.chosen.take()
    }

    fn name(dialog: Hwnd) -> Option<Edit> {
        Edit::of_dialog(dialog, ControlId::NewFolderName.id())
    }

    /// Create stays disabled while the box holds no name.
    fn update_ok(dialog: Hwnd) {
        let empty =
            NewFolder::name(dialog).is_none_or(|name| typed::trimmed(&name.text()).is_empty());
        if let Some(ok) = dialog::item(dialog, DialogItem::Ok.id()) {
            ok.set_enabled(!empty);
        }
    }

    /// Takes out the characters that no name holds as they are typed, and
    /// says why in the balloon, as File Explorer does.
    fn refuse_characters(&self, name: Edit) {
        let text = name.text();
        if text.chars().all(typed::allowed_in_name) {
            return;
        }
        // The caret stays after the characters before it that are kept.
        let caret = name.selection().1;
        let units: Vec<u16> = text.encode_utf16().collect();
        let refused_before = char::decode_utf16(units[..caret.min(units.len())].iter().copied())
            .filter(|c| c.as_ref().is_ok_and(|c| !typed::allowed_in_name(*c)))
            .count();
        let kept: String = text
            .chars()
            .filter(|c| typed::allowed_in_name(*c))
            .collect();
        name.hwnd().set_text(&kept);
        let caret = caret - refused_before;
        name.select(caret, caret);
        if let Some(balloon) = self.balloon.get() {
            balloon.show(name.hwnd(), &self.characters);
        }
    }
}

impl DialogProc for NewFolder<'_> {
    fn init(&self, dialog: Hwnd) -> Opened {
        if let Ok(balloon) = Balloon::create(dialog) {
            let _ = self.balloon.set(balloon);
        }
        NewFolder::update_ok(dialog);
        Opened {
            focus: ControlId::NewFolderName.id(),
            fonts: Vec::new(),
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Edited { .. } => {
                if let Some(name) = NewFolder::name(dialog) {
                    self.refuse_characters(name);
                }
                NewFolder::update_ok(dialog);
                true
            }
            DialogMessage::Command { id } if id == DialogItem::Ok.id() => {
                let Some(name) = NewFolder::name(dialog) else {
                    return true;
                };
                // A pasted name reaches here with any character.
                let text = match typed::folder_name(&name.text()) {
                    Ok(chosen) => match (self.shown_name)(&chosen) {
                        Some(shown) => self.exists.replace("{0}", &shown),
                        None => {
                            self.chosen.replace(Some(chosen));
                            dialog::end(dialog, DialogEnd::Ok);
                            return true;
                        }
                    },
                    Err(refusal) => refusal_text(&refusal, &self.characters, &self.reserved),
                };
                dialog::error_box(Some(dialog), &text, &self.title);
                name.hwnd().focus();
                name.select_all();
                true
            }
            DialogMessage::Command { .. }
            | DialogMessage::LinkActivated { .. }
            | DialogMessage::DoubleClicked { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Tree { .. }
            | DialogMessage::Focus { .. }
            | DialogMessage::KeysCaptured { .. }
            | DialogMessage::Other => false,
        }
    }
}
