//! The window's dialogs. Each copies the text it shows before it opens, so
//! no model borrow is held while its modal loop runs (design rule 1).

use crate::core::ids::{ControlId, DialogId};
use crate::core::model::Model;
use crate::win::dialog::{self, DialogMessage, DialogProc};
use crate::win::gdi::Font;
use crate::win::{self, Hwnd};
use std::cell::RefCell;
use windows::Win32::UI::WindowsAndMessaging::{IDCANCEL, IDOK};

/// The About Gozler dialog (spec: C23).
struct About {
    title: String,
    version: String,
    description: String,
    copyright: String,
    close: String,
    /// The version line's font, which the dialog's control uses until the
    /// dialog closes.
    version_font: RefCell<Option<Font>>,
}

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

pub(super) fn about(owner: Hwnd, model: &RefCell<Model>) {
    let dialog = {
        let model = model.borrow();
        let strings = &model.strings;
        About {
            title: strings.text("about.title").to_owned(),
            version: strings.format("about.version", &[gozler_core::escompat::version()]),
            description: strings.text("about.description").to_owned(),
            copyright: strings.text("about.copyright").to_owned(),
            close: strings.text("button.close").to_owned(),
            version_font: RefCell::new(None),
        }
    };
    dialog::run_modal(owner, DialogId::About.id(), &dialog);
}

impl DialogProc for About {
    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Init => {
                dialog.set_text(&self.title);
                let texts = [
                    (ControlId::AboutVersion.id(), self.version.as_str()),
                    (ControlId::AboutDescription.id(), &self.description),
                    (ControlId::AboutCopyright.id(), &self.copyright),
                    (IDCANCEL.0 as u16, &self.close),
                ];
                for (id, text) in texts {
                    if let Some(control) = dialog::item(dialog, id) {
                        control.set_text(text);
                    }
                }
                if let Some(version) = dialog::item(dialog, ControlId::AboutVersion.id())
                    && let Ok(font) = Font::semibold_of(version)
                {
                    version.set_font(&font);
                    self.version_font.replace(Some(font));
                }
                // The address is not translated, so it is the link's text.
                let address = REPOSITORY.trim_start_matches("https://");
                if let Some(link) = dialog::item(dialog, ControlId::AboutLink.id()) {
                    link.set_text(&format!("<a>{address}</a>"));
                }
                // Close starts with the focus, so Enter closes the dialog
                // instead of opening the link. WM_INITDIALOG answers false
                // when it has set the focus itself.
                if let Some(close) = dialog::item(dialog, IDCANCEL.0 as u16) {
                    close.focus();
                }
                false
            }
            DialogMessage::LinkActivated { id } if id == ControlId::AboutLink.id() => {
                win::shell::open(REPOSITORY);
                true
            }
            DialogMessage::Command { id } if id == IDOK.0 as u16 || id == IDCANCEL.0 as u16 => {
                dialog::end(dialog, 0);
                true
            }
            DialogMessage::LinkActivated { .. }
            | DialogMessage::Command { .. }
            | DialogMessage::Other => false,
        }
    }
}

/// The Search syntax dialog (spec: C22): `gozler`'s search syntax, which is
/// English in every language, as the syntax itself is.
struct SearchSyntax {
    title: String,
    close: String,
    /// The edit control's font, which it uses until the dialog closes.
    font: RefCell<Option<Font>>,
}

pub(super) fn search_syntax(owner: Hwnd, model: &RefCell<Model>) {
    let dialog = {
        let model = model.borrow();
        let strings = &model.strings;
        SearchSyntax {
            title: strings.text("search_syntax.title").to_owned(),
            close: strings.text("button.close").to_owned(),
            font: RefCell::new(None),
        }
    };
    dialog::run_modal(owner, DialogId::SearchSyntax.id(), &dialog);
}

impl DialogProc for SearchSyntax {
    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Init => {
                dialog.set_text(&self.title);
                if let Some(close) = dialog::item(dialog, IDCANCEL.0 as u16) {
                    close.set_text(&self.close);
                }
                if let Some(text) = dialog::item(dialog, ControlId::SearchSyntaxText.id()) {
                    if let Ok(font) = Font::monospace_of(text) {
                        text.set_font(&font);
                        self.font.replace(Some(font));
                    }
                    // An edit control breaks lines only at "\r\n".
                    text.set_text(&gozler_core::escompat::search_syntax().replace('\n', "\r\n"));
                }
                // Close starts with the focus, so the text is not selected
                // and Enter closes the dialog.
                if let Some(close) = dialog::item(dialog, IDCANCEL.0 as u16) {
                    close.focus();
                }
                false
            }
            DialogMessage::Command { id } if id == IDOK.0 as u16 || id == IDCANCEL.0 as u16 => {
                dialog::end(dialog, 0);
                true
            }
            DialogMessage::LinkActivated { .. }
            | DialogMessage::Command { .. }
            | DialogMessage::Other => false,
        }
    }
}
