//! The window's dialogs. Each copies the text it shows before it opens, so
//! no model borrow is held while its modal loop runs (DESIGN.md:
//! window rule 1). `MainWindow::open_dialog` runs each, and the dialog module
//! sets its fixed captions from `core::captions::CaptionTable`.

use crate::core::captions::DialogItem;
use crate::core::ids::ControlId;
use crate::core::query::{DateRange, DateRangeError, Day, SizeRange, SizeRangeError, SizeUnit};
use crate::core::text::Strings;
use crate::win::controls::{Balloon, DatePicker, DropDownList};
use crate::win::dialog::{self, DialogEnd, DialogMessage, DialogProc, Opened};
use crate::win::gdi::Font;
use crate::win::{self, Hwnd};
use std::cell::{Cell, OnceCell};

/// The About Gozler dialog (DESIGN.md: C23).
pub(super) struct About {
    version: String,
}

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

impl About {
    pub(super) fn new(strings: &Strings) -> About {
        About {
            version: strings.format("about.version", &[gozler_core::escompat::version()]),
        }
    }
}

impl DialogProc for About {
    fn init(&self, dialog: Hwnd) -> Opened {
        let mut fonts = Vec::new();
        if let Some(version) = dialog::item(dialog, ControlId::AboutVersion.id()) {
            version.set_text(&self.version);
            if let Ok(font) = Font::semibold_of(version) {
                version.set_font(&font);
                fonts.push(font);
            }
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
                win::shell::open(REPOSITORY);
                true
            }
            DialogMessage::LinkActivated { .. }
            | DialogMessage::Command { .. }
            | DialogMessage::Edited { .. }
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
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

/// The Custom date dialog: From and To, either of which may be left open
/// (DESIGN.md: Filter pane, Dialogs).
pub(super) struct CustomDate {
    /// The range the dialog opens with; a new range starts today.
    shown: Option<DateRange>,
    today: jiff::civil::Date,
    order: String,
    balloon: OnceCell<Balloon>,
    chosen: Cell<Option<DateRange>>,
}

impl CustomDate {
    pub(super) fn new(strings: &Strings, shown: Option<DateRange>) -> CustomDate {
        CustomDate {
            shown,
            today: jiff::Zoned::now().date(),
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
            None => (Some(Day(self.today)), None),
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
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
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
            | DialogMessage::Timer { .. }
            | DialogMessage::Reply
            | DialogMessage::Other => false,
        }
    }
}
