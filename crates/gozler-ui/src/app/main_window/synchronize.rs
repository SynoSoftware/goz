//! Synchronization uses Queue for comparison and execution; this dialog only
//! reads its ready preview through a native virtual list.

use super::MainWindow;
use crate::app::file_operations::FileOperations;
use crate::core::captions::{DialogCaptions, DialogItem};
use crate::core::file_operations::{self, QueueAction};
use crate::core::ids::{ControlId as C, DialogId};
use crate::core::layout::{Dpi, Px, Rect, Size};
use crate::core::panel::Side;
use crate::core::settings::Mode;
use crate::core::synchronize::{self, Preview};
use crate::core::text::Strings;
use crate::win::controls::{CheckBox, Edit, ListColumn, ListView};
use crate::win::dialog::{self, DialogEnd, DialogMessage, DialogProc, Opened};
use crate::win::{self, Hwnd};
use gozler_fileops::{End, OperationId, Request, State};
use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;

const POLL: usize = 1;

impl MainWindow {
    pub(super) fn synchronize_folders(&self, hwnd: Hwnd, continuing: Option<OperationId>) {
        let Some(operations) = self.file_operations.get() else {
            return;
        };
        let recovered = continuing.and_then(|id| operations.operation(id));
        let (source, destination, mirror) = match recovered.as_ref() {
            Some(operation) if matches!(operation.state, State::Interrupted) => {
                let Request::Synchronize {
                    source,
                    destination,
                    mirror,
                } = &*operation.request
                else {
                    return;
                };
                (
                    source.to_string_lossy().into_owned(),
                    destination.to_string_lossy().into_owned(),
                    *mirror,
                )
            }
            Some(_) => return,
            None if continuing.is_some() => return,
            None => {
                let model = self.model.borrow();
                let panels = model.panels();
                let folder = |side: Side| {
                    if model.mode() == Mode::TwoPanels
                        && matches!(panels.panel(side).view(), crate::core::panel::View::Folder)
                    {
                        panels.panel(side).folder().to_owned()
                    } else {
                        String::new()
                    }
                };
                (
                    folder(panels.active()),
                    folder(panels.active().other()),
                    false,
                )
            }
        };
        let strings = self.model.borrow().strings().clone();
        let captions = DialogCaptions::new(DialogId::Synchronize, &strings);
        let state = Synchronize {
            main: self,
            operations,
            strings,
            source,
            destination,
            mirror,
            continuing,
            preview: RefCell::default(),
            list: OnceCell::new(),
            layout: OnceCell::new(),
            dpi: Cell::new(hwnd.dpi()),
        };
        let _ = dialog::run_modal(hwnd, DialogId::Synchronize, &captions, &state);
        state.clear();
    }
}

struct Synchronize<'a> {
    main: &'a MainWindow,
    operations: &'a FileOperations,
    strings: Strings,
    source: String,
    destination: String,
    mirror: bool,
    continuing: Option<OperationId>,
    preview: RefCell<Preview>,
    list: OnceCell<ListView>,
    layout: OnceCell<Layout>,
    dpi: Cell<Dpi>,
}

struct Layout {
    dpi: Dpi,
    client: Size,
    minimum: Size,
    controls: Vec<(u16, Hwnd, Rect)>,
}

impl Synchronize<'_> {
    fn text(dialog: Hwnd, id: C) -> String {
        dialog::item(dialog, id.id()).map_or_else(String::new, Hwnd::text)
    }

    fn set_text(dialog: Hwnd, id: C, text: &str) {
        if let Some(control) = dialog::item(dialog, id.id())
            && control.text() != text
        {
            control.set_text(text);
        }
    }

    fn detach(&self) -> Option<OperationId> {
        let preview = self.preview.take();
        let id = preview.id();
        if let Preview::Ready { comparison, .. } | Preview::Committing { comparison, .. } = preview
        {
            self.operations.release_comparison(comparison);
        }
        id
    }

    fn clear(&self) {
        if let Some(id) = self.detach() {
            self.operations.act(id, QueueAction::Cancel);
        }
    }

    fn invalidate(&self, dialog: Hwnd) {
        self.clear();
        dialog.kill_timer(POLL);
        if let Some(list) = self.list.get() {
            list.set_count(0);
        }
        Self::set_text(dialog, C::SyncSummary, "");
        Self::set_text(dialog, C::SyncHint, self.strings.text("sync.initial"));
        self.enable(dialog);
    }

    fn enable(&self, dialog: Hwnd) {
        let committing = matches!(*self.preview.borrow(), Preview::Committing { .. });
        for id in [
            C::SyncSource,
            C::SyncDestination,
            C::SyncSourceBrowse,
            C::SyncDestinationBrowse,
            C::SyncMirror,
        ] {
            if let Some(control) = dialog::item(dialog, id.id()) {
                control.set_enabled(!committing);
            }
        }
        let filled = !Self::text(dialog, C::SyncSource).trim().is_empty()
            && !Self::text(dialog, C::SyncDestination).trim().is_empty();
        if let Some(compare) = dialog::item(dialog, C::SyncCompare.id()) {
            compare.set_enabled(filled && !committing);
        }
        let ready = self.preview.borrow().can_commit();
        if let Some(commit) = dialog::item(dialog, DialogItem::Ok.id()) {
            commit.set_enabled(ready);
        }
    }

    fn compare(&self, dialog: Hwnd) {
        let source = Self::text(dialog, C::SyncSource);
        let destination = Self::text(dialog, C::SyncDestination);
        if source.trim().is_empty() || destination.trim().is_empty() {
            return;
        }
        self.invalidate(dialog);
        let mirror =
            CheckBox::of_dialog(dialog, C::SyncMirror.id()).is_some_and(CheckBox::is_checked);
        if let Some(id) = self.main.start_operation(
            Request::Synchronize {
                source: PathBuf::from(source.trim()),
                destination: PathBuf::from(destination.trim()),
                mirror,
            },
            None,
        ) {
            self.preview.replace(Preview::Comparing(id));
            dialog.set_timer(POLL, 200);
            self.poll(dialog);
        }
    }

    fn poll(&self, dialog: Hwnd) {
        let committing = match *self.preview.borrow() {
            Preview::Committing { id, .. } => Some(id),
            _ => None,
        };
        if let Some(id) = committing {
            match self.operations.take_synchronized(id) {
                Some(true) => {
                    self.detach();
                    dialog::end(dialog, DialogEnd::Ok);
                }
                Some(false) => self.invalidate(dialog),
                None => {}
            }
            return;
        }
        let id = match *self.preview.borrow() {
            Preview::Comparing(id) => id,
            _ => return,
        };
        self.operations.request_refresh();
        let state = self
            .operations
            .operation(id)
            .map(|operation| operation.state)
            .or_else(|| {
                self.main
                    .model
                    .borrow()
                    .operation_outcome(id)
                    .map(State::Ended)
            });
        let Some(state) = state else {
            if !self.operations.is_pending(id) {
                self.invalidate(dialog);
            }
            return;
        };
        match state {
            State::Compared(comparison) => {
                let Some(comparison) = comparison.upgrade() else {
                    self.invalidate(dialog);
                    return;
                };
                let count = comparison.len() as u64;
                let summary =
                    synchronize::summary(&self.strings, &*self.main.numbers.borrow(), &comparison);
                let hint = if comparison.summary().conflicts > 0 {
                    "sync.conflicts"
                } else if comparison.summary().deletions > 0 {
                    "sync.delete_hint"
                } else {
                    "sync.ready"
                };
                self.preview.replace(Preview::Ready { id, comparison });
                dialog.kill_timer(POLL);
                if let Some(list) = self.list.get() {
                    list.set_count(count);
                }
                Self::set_text(dialog, C::SyncSummary, &summary);
                Self::set_text(dialog, C::SyncHint, self.strings.text(hint));
                self.enable(dialog);
            }
            State::Ended(outcome) => {
                let reason = match &outcome.end {
                    End::Refused(refusal) => file_operations::refusal_text(
                        &self.strings,
                        &*self.main.numbers.borrow(),
                        *refusal,
                        &|code| win::error_text(&std::io::Error::from_raw_os_error(code as i32)),
                    ),
                    End::Cancelled => self.strings.text("file_operations.cancelled").to_owned(),
                    _ => self.strings.text("sync.initial").to_owned(),
                };
                self.preview.replace(Preview::Failed(id));
                dialog.kill_timer(POLL);
                Self::set_text(
                    dialog,
                    C::SyncSummary,
                    self.strings.text("sync.failed_short"),
                );
                Self::set_text(dialog, C::SyncHint, &reason);
                self.enable(dialog);
            }
            state => {
                let key = match state {
                    State::Waiting { .. } => "file_operations.waiting",
                    State::Running(ref running) if running.paused => "file_operations.paused",
                    State::Cancelling => "file_operations.cancelling",
                    _ => "sync.comparing",
                };
                Self::set_text(dialog, C::SyncSummary, self.strings.text(key));
                Self::set_text(dialog, C::SyncHint, self.strings.text("sync.initial"));
            }
        }
    }
}

impl DialogProc for Synchronize<'_> {
    fn init(&self, dialog: Hwnd) -> Opened {
        Self::set_text(dialog, C::SyncSource, &self.source);
        Self::set_text(dialog, C::SyncDestination, &self.destination);
        if let Some(mirror) = CheckBox::of_dialog(dialog, C::SyncMirror.id()) {
            mirror.set_checked(self.mirror);
        }
        if let Some(list) = ListView::of_dialog(dialog, C::SyncList.id()) {
            let action = dialog.client_size().width.0 / 5;
            list.insert_column(
                0,
                &ListColumn {
                    header: self.strings.text("sync.action"),
                    width: Px(action),
                    right_aligned: false,
                },
            );
            list.insert_column(
                1,
                &ListColumn {
                    header: self.strings.text("header.path"),
                    width: Px(action * 4),
                    right_aligned: false,
                },
            );
            let _ = self.list.set(list);
        }
        self.enable(dialog);
        if let Some(id) = self.continuing {
            self.preview.replace(Preview::Comparing(id));
            self.operations.act(id, QueueAction::Continue);
            dialog.set_timer(POLL, 200);
            self.poll(dialog);
        }
        Opened {
            focus: C::SyncSource.id(),
            fonts: Vec::new(),
        }
    }

    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool {
        match message {
            DialogMessage::Edited { id }
                if id == C::SyncSource.id() || id == C::SyncDestination.id() =>
            {
                self.invalidate(dialog);
                true
            }
            DialogMessage::Command { id } if id == C::SyncMirror.id() => {
                self.invalidate(dialog);
                true
            }
            DialogMessage::Command { id } if id == C::SyncCompare.id() => {
                self.compare(dialog);
                true
            }
            DialogMessage::Command { id }
                if id == C::SyncSourceBrowse.id() || id == C::SyncDestinationBrowse.id() =>
            {
                let target = if id == C::SyncSourceBrowse.id() {
                    C::SyncSource
                } else {
                    C::SyncDestination
                };
                if let Some(folder) = win::shell::pick_folder(dialog, None)
                    && let Some(edit) = Edit::of_dialog(dialog, target.id())
                {
                    edit.hwnd().set_text(&folder);
                }
                true
            }
            DialogMessage::Command { id } if id == DialogItem::Ok.id() => {
                let ready = self.preview.borrow().can_commit();
                if ready && let Preview::Ready { id, comparison } = self.preview.take() {
                    self.preview.replace(Preview::Committing { id, comparison });
                    self.operations.synchronize(id);
                    self.enable(dialog);
                    dialog.set_timer(POLL, 200);
                }
                true
            }
            DialogMessage::Timer { id: POLL } => {
                self.poll(dialog);
                true
            }
            _ => false,
        }
    }

    fn list_cell(&self, id: u16, row: u64, column: usize) -> Option<String> {
        if id != C::SyncList.id() {
            return None;
        }
        let preview = self.preview.borrow();
        let entry = preview.comparison()?.entry(usize::try_from(row).ok()?)?;
        match column {
            0 => Some(synchronize::action(&self.strings, entry.action)),
            1 => Some(entry.path.clone()),
            _ => None,
        }
    }

    fn buttons_sized(&self, dialog: Hwnd, _width: Px) {
        let controls = [
            C::SyncSourceLabel,
            C::SyncSource,
            C::SyncSourceBrowse,
            C::SyncDestinationLabel,
            C::SyncDestination,
            C::SyncDestinationBrowse,
            C::SyncDirection,
            C::SyncMirror,
            C::SyncCompare,
            C::SyncList,
            C::SyncSummary,
            C::SyncHint,
        ]
        .into_iter()
        .map(C::id)
        .chain([DialogItem::Ok.id(), DialogItem::Cancel.id()])
        .filter_map(|id| {
            dialog::item(dialog, id).map(|control| (id, control, control.rect_in(dialog)))
        })
        .collect();
        let rect = dialog.rect_in(dialog.parent());
        let _ = self.layout.set(Layout {
            dpi: dialog.dpi(),
            client: dialog.client_size(),
            minimum: Size {
                width: rect.width,
                height: rect.height,
            },
            controls,
        });
        self.resized(dialog);
    }

    fn minimum_size(&self) -> Option<Size> {
        let layout = self.layout.get()?;
        Some(Size {
            width: layout.minimum.width.rescale(layout.dpi, self.dpi.get()),
            height: layout.minimum.height.rescale(layout.dpi, self.dpi.get()),
        })
    }

    fn resized(&self, dialog: Hwnd) {
        let Some(layout) = self.layout.get() else {
            return;
        };
        let dpi = dialog.dpi();
        self.dpi.set(dpi);
        let scale = |px: Px| px.rescale(layout.dpi, dpi);
        let size = dialog.client_size();
        let dx = size.width - scale(layout.client.width);
        let dy = size.height - scale(layout.client.height);
        let moves = layout
            .controls
            .iter()
            .map(|&(id, control, rect)| {
                let mut rect = Rect {
                    x: scale(rect.x),
                    y: scale(rect.y),
                    width: scale(rect.width),
                    height: scale(rect.height),
                };
                if [
                    C::SyncSourceBrowse.id(),
                    C::SyncDestinationBrowse.id(),
                    DialogItem::Ok.id(),
                    DialogItem::Cancel.id(),
                ]
                .contains(&id)
                {
                    rect.x = rect.x + dx;
                } else if id != C::SyncCompare.id() {
                    rect.width = rect.width + dx;
                }
                if id == C::SyncList.id() {
                    rect.height = rect.height + dy;
                }
                if [
                    C::SyncSummary.id(),
                    C::SyncHint.id(),
                    DialogItem::Ok.id(),
                    DialogItem::Cancel.id(),
                ]
                .contains(&id)
                {
                    rect.y = rect.y + dy;
                }
                (control, rect)
            })
            .collect::<Vec<_>>();
        win::move_children(&moves);
        if let Some(list) = self.list.get() {
            let width = Px((list.hwnd().client_size().width.0 - list.column_width(0).0).max(0));
            if list.column_width(1) != width {
                list.set_column_width(1, width);
            }
        }
    }
}
