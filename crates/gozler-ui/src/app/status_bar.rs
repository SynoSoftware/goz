//! The status bar (DESIGN.md: Status bar): what the view holds on the
//! left, jobs and index builds in the middle, and the
//! service on the right. The main window and the Settings dialog each have
//! one; Settings shows only its changes and its own work. It owns the native
//! job controls and, in the main window, the transfer summary, memory group
//! and Show details. The core supplies the captions and layout; Windows draws
//! every control.

use crate::core::captions::{ButtonIcon, JobCaption, JobProgress, MemoryCaptions, ValuePart};
use crate::core::glyph::Glyph;
use crate::core::ids::ControlId;
use crate::core::layout::{Px, Rect, RightWidth, StatusLayout, TextForm};
use crate::core::status_transfer::Input;
use crate::win::controls::{self, Button, Label, ProgressBar, Tooltip};
use crate::win::gdi::{Font, TextMeter};
use crate::win::notify::{ControlEvent, Notification};
use crate::win::status_transfer::Transfer;
use crate::win::toolbar::{Tool, Toolbar};
use crate::win::{self, Hwnd};
use std::cell::Cell;

const VIEW_GROUP: usize = 0;
const JOB_GROUP: usize = 1;
/// The right group's first part, where a click on a newer release runs
/// Update. In the main window, the right group's other parts follow it,
/// then the memory figures' parts while they show.
const RIGHT_GROUP: usize = 2;
/// The tooltip areas of the main window's status bar: the memory figures,
/// then the files total.
const SERVICE_MEMORY: usize = 0;
const APP_MEMORY: usize = 1;
const FILES_TOTAL: usize = 2;
const JOB_TEXT: usize = 3;
const ACTIVITY_TEXT: usize = 4;

#[derive(Clone, Copy)]
pub(super) enum Slot {
    Left,
    Center,
    Right,
}

pub(super) enum Entry<'a> {
    Text(&'a str),
    Work(&'a JobCaption),
    Transfer(&'a crate::core::status_transfer::Caption),
    Values {
        parts: &'a [ValuePart],
        memory: Option<&'a MemoryCaptions>,
    },
}

pub(super) struct StatusBar {
    bar: controls::StatusBar,
    text: Label,
    activity: Label,
    progress: ProgressBar,
    progress_moving: Cell<bool>,
    cancel: Button,
    /// The main window's Show details, a toolbar of its one icon.
    show_details: Option<Toolbar>,
    /// The tooltips of the main window's memory figures and files total.
    part_tips: Option<Tooltip>,
    transfer: Option<Transfer>,
    split: Cell<Option<Px>>,
}

impl StatusBar {
    /// Creates the main window's status bar at the bottom of `parent`, with
    /// the size grip, because the person resizes the window, the memory
    /// group and Show details.
    pub(super) fn for_window(parent: Hwnd) -> windows::core::Result<StatusBar> {
        let mut status = StatusBar::create(parent, true)?;
        status.transfer = Some(Transfer::create(status.bar.hwnd())?);
        let id = ControlId::StatusShowDetails.id();
        let show = Toolbar::create(
            status.bar.hwnd(),
            id,
            &[Tool::Icon {
                id,
                glyph: Glyph::ChevronUp,
            }],
        )?;
        show.hwnd().set_visible(false);
        status.show_details = Some(show);
        Ok(status)
    }

    /// Creates a dialog's status bar at the bottom of `dialog`.
    pub(super) fn for_dialog(dialog: Hwnd) -> windows::core::Result<StatusBar> {
        StatusBar::create(dialog, false)
    }

    fn create(parent: Hwnd, size_grip: bool) -> windows::core::Result<StatusBar> {
        let bar = controls::StatusBar::create(parent, ControlId::Status.id(), size_grip)?;
        let status = StatusBar {
            bar,
            text: Label::create_line(bar.hwnd(), ControlId::StatusText.id())?,
            activity: Label::create_line(bar.hwnd(), ControlId::StatusActivity.id())?,
            progress: ProgressBar::create(bar.hwnd(), ControlId::StatusProgress.id())?,
            progress_moving: Cell::new(false),
            cancel: Button::create(bar.hwnd(), ControlId::StatusCancel.id())?,
            show_details: None,
            part_tips: Some(Tooltip::create_areas(bar.hwnd(), 5)?),
            transfer: None,
            split: Cell::new(None),
        };
        status.progress.hwnd().set_visible(false);
        status.activity.hwnd().set_visible(false);
        status.cancel.hwnd().set_visible(false);
        Ok(status)
    }

    /// Show details, whose tooltip the main window gives.
    pub(super) fn show_details(&self) -> Option<Toolbar> {
        self.show_details
    }

    /// Gives Show details the name that screen readers say.
    pub(super) fn set_show_details_caption(&self, caption: &str) {
        if let Some(show) = self.show_details {
            show.set_text(ControlId::StatusShowDetails.id(), caption);
        }
    }

    /// The controls that show text in their parent's font.
    pub(super) fn text_controls(&self) -> Vec<Hwnd> {
        [
            self.bar.hwnd(),
            self.text.hwnd(),
            self.activity.hwnd(),
            self.cancel.hwnd(),
        ]
        .into_iter()
        .chain(self.transfer.into_iter().flat_map(Transfer::text_controls))
        .collect()
    }

    pub(super) fn set_progress_captions(&self, captions: &[String; 2]) {
        self.progress.hwnd().set_text(&captions[1]);
    }

    pub(super) fn set_cancel_caption(&self, caption: &str) {
        self.cancel.hwnd().set_text(caption);
        self.show_cancel_icon();
    }

    /// Gives Cancel its icon, again when icons are shown or hidden.
    pub(super) fn show_cancel_icon(&self) {
        self.cancel.set_icon(ButtonIcon::of("button.cancel", false));
    }

    /// Fits the status bar to its parent's new size, with a drawing area
    /// at least `min_height` tall, and returns its height.
    pub(super) fn fit(&self, min_height: Px) -> Px {
        self.bar.set_min_height(min_height);
        self.bar.fit()
    }

    pub(super) fn set_visible(&self, visible: bool) {
        self.bar.hwnd().set_visible(visible);
    }

    pub(super) fn contains(&self, control: Hwnd) -> bool {
        self.bar.hwnd().is_parent_of(control)
    }

    pub(super) fn restore_transfer_focus(&self) {
        if let Some(transfer) = self.transfer {
            transfer.restore_focus();
        }
    }

    /// Places a dialog's groups and controls.
    pub(super) fn place(&self, layout: &StatusLayout) {
        self.bar.set_parts(&[layout.middle.x]);
        self.place_controls(layout);
    }

    /// Places the main window's groups and controls, and shows Show details
    /// while `layout` has a place for it, `right` in the right group and
    /// `memory` in the memory figures' parts.
    pub(super) fn place_in_window(
        &self,
        layout: &StatusLayout,
        right: &[ValuePart],
        memory: Option<&MemoryCaptions>,
    ) {
        self.show_window_end(layout, right, memory);
    }

    fn place_controls(&self, layout: &StatusLayout) {
        if let Some(tips) = &self.part_tips {
            // A hidden group's tips cover nothing.
            let (service, app) = layout
                .memory
                .map(|memory| (memory.service, memory.app))
                .unwrap_or_default();
            tips.place_area(SERVICE_MEMORY, service);
            tips.place_area(APP_MEMORY, app);
        }
        let mut moves = Vec::new();
        if let Some(show) = self.show_details {
            if let Some(rect) = layout.show_details {
                moves.push((show.hwnd(), rect));
            }
            show.hwnd().set_visible(layout.show_details.is_some());
        }
        win::move_children(&moves);
    }

    /// Renders one complete frame of entries grouped by slot. Returns
    /// whether an action lost focus when its work disappeared.
    pub(super) fn render(
        &self,
        contents: &[(Slot, Entry<'_>)],
        layout: Option<&StatusLayout>,
    ) -> bool {
        let mut view = Vec::new();
        let mut activity = Vec::new();
        let mut work = None;
        let mut transfer_caption = None;
        let mut right = Vec::new();
        let mut memory = None;
        for (slot, entry) in contents {
            match (slot, entry) {
                (Slot::Left, Entry::Text(text)) => view.push(*text),
                (Slot::Center, Entry::Text(text)) => activity.push(*text),
                (Slot::Center, Entry::Work(caption)) => {
                    assert!(
                        work.replace(*caption).is_none(),
                        "the generic job has one owner"
                    );
                }
                (Slot::Center, Entry::Transfer(caption)) => {
                    assert!(
                        transfer_caption.replace(*caption).is_none(),
                        "the transfer queue has one owner"
                    );
                }
                (
                    Slot::Right,
                    Entry::Values {
                        parts,
                        memory: figures,
                    },
                ) => {
                    right.extend_from_slice(parts);
                    if let Some(figures) = figures {
                        assert!(
                            memory.replace(*figures).is_none(),
                            "memory has one producer"
                        );
                    }
                }
                _ => unreachable!("each entry declares the slot of its presentation"),
            }
        }
        self.show_view(&view.join(" · "));
        let activity = activity.join(" · ");
        self.activity.hwnd().set_text(&activity);
        self.activity.hwnd().set_visible(!activity.is_empty());
        let empty = JobCaption::default();
        let job = work.unwrap_or(&empty);
        let job_text = if job.progress != JobProgress::None {
            layout
                .and_then(|layout| {
                    crate::win::gdi::Font::of(self.bar.hwnd()).ok().map(|font| {
                        crate::win::gdi::TextMeter::new(&font)
                            .compact_path(&job.text, layout.running_text)
                    })
                })
                .unwrap_or_else(|| job.text.clone())
        } else {
            job.text.clone()
        };
        let job_text = job_text.as_str();
        if let Some(transfer) = self.transfer {
            if let (Some(caption), Some(layout)) = (transfer_caption, layout) {
                let layout = self.show_window_end(layout, &right, memory);
                let transfer_rect =
                    self.place_center(&layout, job, job_text, &activity, Some(caption));
                let only = activity.is_empty()
                    && job_text.is_empty()
                    && job.progress == JobProgress::None
                    && !job.cancel;
                let lost = self.show_job(job, job_text);
                let start = transfer_rect.map_or(layout.middle.right(), |rect| rect.x);
                self.split.set((!only).then_some(start));
                self.show_window_end(&layout, &right, memory);
                return transfer.show(caption, transfer_rect.unwrap_or_default()) || lost;
            }
            self.split.set(None);
            let lost = transfer.hide();
            if let Some(layout) = layout {
                let layout = self.show_window_end(layout, &right, memory);
                self.place_center(&layout, job, job_text, &activity, None);
            }
            return self.show_job(job, job_text) || lost;
        }
        if let Some(layout) = layout {
            self.place(layout);
            self.place_center(layout, job, job_text, &activity, None);
        }
        self.show_job(job, job_text)
    }

    fn place_center(
        &self,
        layout: &StatusLayout,
        job: &JobCaption,
        text: &str,
        activity: &str,
        transfer: Option<&crate::core::status_transfer::Caption>,
    ) -> Option<Rect> {
        let font = Font::of(self.bar.hwnd()).ok()?;
        let meter = TextMeter::new(&font);
        let dpi = self.bar.hwnd().dpi();
        let base = crate::core::layout::Dpi::new(96).expect("96 is not zero");
        let gap = Px(6).rescale(base, dpi);
        let padding = Px(4).rescale(base, dpi);
        let text_width = if text.is_empty() {
            Px(0)
        } else {
            meter.plain_width(text).min(layout.running_text) + padding
        };
        let progress = job.progress != JobProgress::None;
        let job_width = text_width
            + if progress {
                gap + layout.progress.width
            } else {
                Px(0)
            }
            + if job.cancel {
                gap + layout.cancel.width
            } else {
                Px(0)
            };
        let activity_wanted = if activity.is_empty() {
            Px(0)
        } else {
            meter.plain_width(activity) + padding
        };
        let activity_reserved = activity_wanted
            .min(Px(150).rescale(base, dpi))
            .min((layout.middle.width - job_width - gap).max(Px(0)));
        let transfer_width = transfer
            .zip(self.transfer)
            .map_or(Px(0), |(caption, control)| {
                control.width(
                    caption,
                    Rect {
                        width: (layout.middle.width
                            - job_width
                            - activity_reserved
                            - if activity_reserved > Px(0) {
                                gap
                            } else {
                                Px(0)
                            }
                            - if job_width > Px(0) { gap } else { Px(0) })
                        .max(Px(0)),
                        ..layout.middle
                    },
                )
            });
        let gaps = usize::from(job_width > Px(0)) + usize::from(transfer_width > Px(0));
        let activity_width = activity_wanted.min(
            (layout.middle.width - job_width - transfer_width - Px(gap.0 * gaps as i32)).max(Px(0)),
        );
        let widths: Vec<_> = [activity_width, job_width, transfer_width]
            .into_iter()
            .filter(|width| *width > Px(0))
            .collect();
        let placed = layout.center_entries(&widths, self.bar.hwnd().client_size().width, dpi);
        let mut placed = placed.into_iter();
        let activity_rect = if activity_width > Px(0) {
            let area = placed
                .next()
                .expect("the visible activity has an entry width");
            Rect {
                y: area.y + Px((area.height - meter.height()).0 / 2),
                height: meter.height(),
                ..area
            }
        } else {
            Rect::default()
        };
        win::move_children(&[(self.activity.hwnd(), activity_rect)]);
        self.activity.hwnd().set_visible(activity_width > Px(0));
        if let Some(tips) = self.part_tips {
            tips.place_area(ACTIVITY_TEXT, activity_rect);
            tips.set_area(ACTIVITY_TEXT, activity);
        }
        if job_width == Px(0)
            && let Some(tips) = self.part_tips
        {
            tips.place_area(JOB_TEXT, Rect::default());
            tips.set_area(JOB_TEXT, "");
        }
        if job_width > Px(0) {
            let area = placed.next().expect("the visible job has an entry width");
            let text_rect = Rect {
                x: area.x,
                y: area.y + Px((area.height - meter.height()).0 / 2),
                width: text_width,
                height: meter.height(),
            };
            let mut x = text_rect.right() + gap;
            let bar = Rect {
                x,
                ..layout.progress
            };
            if progress {
                x = x + layout.progress.width + gap;
            }
            let cancel = Rect { x, ..layout.cancel };
            win::move_children(&[
                (self.text.hwnd(), text_rect),
                (self.progress.hwnd(), bar),
                (self.cancel.hwnd(), cancel),
            ]);
            if let Some(tips) = self.part_tips {
                tips.place_area(JOB_TEXT, text_rect);
                tips.set_area(JOB_TEXT, &job.text);
            }
        }
        if transfer_width > Px(0) {
            placed.next()
        } else {
            None
        }
    }

    /// Makes a part of the main window's status bar for each item of
    /// `right` that `layout` has room for, then for each memory figure
    /// while they show, and shows them, each after its icon. A part keeps
    /// the text and icon of the part that had its index before, so every
    /// part after the middle group is shown again, or emptied.
    ///
    /// The files total shows without its size where it does not fit whole.
    /// While that happens, or a total after it is left out, its tooltip
    /// shows both totals whole.
    fn show_window_end(
        &self,
        layout: &StatusLayout,
        right: &[ValuePart],
        memory: Option<&MemoryCaptions>,
    ) -> StatusLayout {
        let widths: Vec<RightWidth> = right
            .iter()
            .map(|item| RightWidth {
                whole: self.bar.part_width(&item.text, item.glyph),
                short: (item.narrow.as_ref())
                    .map(|narrow| self.bar.part_width(&narrow.text, item.glyph)),
            })
            .collect();
        let view = self.bar.text(VIEW_GROUP);
        let view = if view.is_empty() {
            Px(0)
        } else {
            self.bar.part_width(&view, None)
        };
        let dpi = self.bar.hwnd().dpi();
        let width = self.bar.hwnd().client_size().width;
        let wanted = widths.iter().fold(Px(0), |sum, width| sum + width.whole);
        let layout = layout.fit_groups(view, wanted, width, dpi);
        let forms = layout.right_parts_shown(&widths);
        let shown = forms.len();
        let shown_widths: Vec<Px> = (widths.iter().zip(&forms))
            .map(|(width, form)| width.of(*form))
            .collect();
        let layout = layout.fit_groups(
            view,
            shown_widths.iter().fold(Px(0), |sum, width| sum + *width),
            width,
            dpi,
        );
        let mut part_widths = layout.part_widths(&shown_widths);
        let right_group = self.right_group();
        if let Some(start) = self.split.get() {
            let before = (start - layout.middle.x)
                .max(Px(0))
                .min(layout.middle.width);
            part_widths[JOB_GROUP] = before;
            part_widths.insert(JOB_GROUP + 1, layout.middle.width - before);
        }
        self.bar.set_parts(&part_widths);
        if right_group > RIGHT_GROUP {
            self.bar.set_text(JOB_GROUP + 1, "");
            self.bar.set_icon(JOB_GROUP + 1, None);
            self.bar.set_tip(JOB_GROUP + 1, "");
        }
        let memory = memory.filter(|_| layout.memory.is_some());
        if let (Some(tips), Some(memory)) = (&self.part_tips, memory) {
            tips.set_area(SERVICE_MEMORY, &memory.service_tip);
            tips.set_area(APP_MEMORY, &memory.app_tip);
        }
        let figures = memory.map(|memory| [&memory.service, &memory.app]);
        // The text of a right part is the one its form says.
        let right_items = (right.iter().zip(&forms)).map(|(item, form)| {
            let text = match (&item.narrow, form) {
                (Some(narrow), TextForm::Short) => narrow.text.as_str(),
                _ => item.text.as_str(),
            };
            Some((item, text))
        });
        // The right group has a part also while it shows nothing.
        let items = right_items.chain((shown == 0).then_some(None)).chain(
            figures
                .into_iter()
                .flatten()
                .map(|figure| Some((figure, figure.text.as_str()))),
        );
        // The parts after them, Show details' or none, show nothing.
        let parts = (right_group..=part_widths.len()).zip(items.chain(std::iter::repeat(None)));
        for (part, item) in parts {
            let text = item.map_or("", |(_, text)| text);
            self.bar.set_text(part, text);
            // A part's tip shows only while its text is cut off. The files
            // total has the tooltip of its own, which also shows while its
            // text is whole but shortened.
            let own_tip = item.is_some_and(|(item, _)| item.narrow.is_some());
            self.bar.set_tip(part, if own_tip { "" } else { text });
            self.bar
                .set_icon(part, item.and_then(|(item, _)| item.glyph));
        }
        self.show_files_tip(right, &forms);
        self.place_controls(&layout);
        layout
    }

    /// Gives the files total's part the tooltip of both totals while its
    /// text is shortened or a total after it is left out, and no tooltip
    /// otherwise.
    fn show_files_tip(&self, right: &[ValuePart], forms: &[TextForm]) {
        let Some(tips) = &self.part_tips else {
            return;
        };
        let tip = (right[..forms.len()].iter())
            .position(|item| item.narrow.is_some())
            .and_then(|index| {
                let narrow = right[index].narrow.as_ref()?;
                let cut = forms[index] == TextForm::Short || forms.len() < right.len();
                cut.then_some((index, narrow.tip.as_str()))
            });
        match tip {
            Some((index, tip)) => {
                tips.place_area(FILES_TOTAL, self.bar.part_rect(self.right_group() + index));
                tips.set_area(FILES_TOTAL, tip);
            }
            None => {
                tips.place_area(FILES_TOTAL, Rect::default());
                tips.set_area(FILES_TOTAL, "");
            }
        }
    }

    fn show_view(&self, text: &str) {
        self.bar.set_text(VIEW_GROUP, text);
    }

    /// Shows the job as `text`, with its progress bar and Cancel. Returns
    /// whether Cancel had the focus when it was hidden, so the parent gives
    /// the focus to another control.
    fn show_job(&self, job: &JobCaption, text: &str) -> bool {
        self.bar.set_text(JOB_GROUP, "");
        self.text.hwnd().set_text(text);
        self.text.hwnd().set_visible(!text.is_empty());
        // A long result ends in an ellipsis, so the tooltip shows it whole.
        self.bar.set_tip(JOB_GROUP, &job.text);
        match job.progress {
            JobProgress::None | JobProgress::Unmeasured => {
                let moving = job.progress == JobProgress::Unmeasured;
                if self.progress_moving.replace(moving) != moving {
                    self.progress.set_moving(moving);
                }
            }
            JobProgress::Measured { done, total } => {
                self.progress_moving.set(false);
                self.progress.set_measured(done, total);
            }
        }
        self.progress
            .hwnd()
            .set_visible(job.progress != JobProgress::None);
        let focus_lost = !job.cancel && Hwnd::focused() == Some(self.cancel.hwnd());
        self.cancel.hwnd().set_visible(job.cancel);
        focus_lost
    }

    pub(super) fn cancel_clicked(&self, event: ControlEvent) -> bool {
        self.cancel.clicked(event)
    }

    pub(super) fn transfer_input(&self, event: ControlEvent) -> Option<Input> {
        self.transfer.and_then(|transfer| transfer.input(event))
    }

    fn right_group(&self) -> usize {
        RIGHT_GROUP + usize::from(self.split.get().is_some())
    }

    /// Whether `notification` is a click on the right group's first part,
    /// which shows the index state.
    pub(super) fn service_clicked(&self, notification: &Notification) -> bool {
        self.bar.clicked_part(notification) == Some(self.right_group())
    }
}
