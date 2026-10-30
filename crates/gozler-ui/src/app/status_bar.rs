//! The status bar (DESIGN.md: Status bar): what the view holds on the
//! left, jobs and index builds in the middle, and the
//! service on the right. The main window and the Settings dialog each have
//! one; Settings shows only its changes and its own work. It owns the native
//! job controls and, in the main window, the transfer summary, memory group
//! and Show details. The core supplies the captions and layout; Windows draws
//! every control.

mod indexing;
mod work;

use self::indexing::Indexing;
use self::work::Work;
use crate::core::captions::{IndexingCaption, JobCaption, JobProgress, MemoryCaptions, ValuePart};
use crate::core::glyph::Glyph;
use crate::core::ids::ControlId;
use crate::core::layout::{Px, Rect, RightWidth, StatusLayout, TextForm};
use crate::core::status_transfer::Input;
use crate::win::controls::{self, Label, Tooltip};
use crate::win::gdi::{Font, TextMeter};
use crate::win::notify::{ControlEvent, Notification};
use crate::win::status_transfer::Transfer;
use crate::win::toolbar::{Tool, Toolbar};
use crate::win::{self, Hwnd};
use std::cell::{Cell, RefCell};

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
const ACTIVITY_TEXT: usize = 3;
const VIEW_TEXT: usize = 4;

#[derive(Clone, Copy)]
pub(super) enum Slot {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkId {
    Job(crate::core::model::Job),
    Waiting,
}

pub(super) enum Entry<'a> {
    Text(&'a str),
    Indexing(&'a IndexingCaption),
    Work(WorkId, &'a JobCaption),
    Transfer(&'a crate::core::status_transfer::Caption),
    Values {
        parts: &'a [ValuePart],
        memory: Option<&'a MemoryCaptions>,
    },
}

pub(super) struct StatusBar {
    bar: controls::StatusBar,
    view: Label,
    work: Vec<(WorkId, Work)>,
    indexing: RefCell<Vec<Indexing>>,
    /// The main window's Show details, a toolbar of its one icon.
    show_details: Option<Toolbar>,
    /// The tooltips of the main window's memory figures and files total.
    part_tips: Option<Tooltip>,
    transfer: Option<Transfer>,
    split: Cell<Option<Px>>,
    work_width: Cell<Px>,
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
            view: Label::create_heading(bar.hwnd(), ControlId::StatusView.id())?,
            work: [
                WorkId::Job(crate::core::model::Job::Export),
                WorkId::Job(crate::core::model::Job::Fix),
                WorkId::Job(crate::core::model::Job::Operation),
                WorkId::Waiting,
            ]
            .into_iter()
            .map(|id| Work::create(bar.hwnd()).map(|work| (id, work)))
            .collect::<windows::core::Result<_>>()?,
            indexing: RefCell::new(Vec::new()),
            show_details: None,
            part_tips: Some(Tooltip::create_areas(bar.hwnd(), 5)?),
            transfer: None,
            split: Cell::new(None),
            work_width: Cell::new(Px(0)),
        };
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
        [self.bar.hwnd(), self.view.hwnd()]
            .into_iter()
            .chain(self.work.iter().flat_map(|(_, work)| work.text_controls()))
            .chain(
                self.indexing
                    .borrow()
                    .iter()
                    .flat_map(Indexing::text_controls),
            )
            .chain(self.transfer.into_iter().flat_map(Transfer::text_controls))
            .collect()
    }

    pub(super) fn set_progress_captions(&self, captions: &[String; 2]) {
        for (_, work) in &self.work {
            work.set_progress_caption(&captions[1]);
        }
    }

    pub(super) fn set_cancel_caption(&self, caption: &str) {
        for (_, work) in &self.work {
            work.set_cancel_caption(caption);
        }
    }

    /// Gives Cancel its icon, again when icons are shown or hidden.
    pub(super) fn show_cancel_icon(&self) {
        for (_, work) in &self.work {
            work.show_cancel_icon();
        }
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
        let inset = self.content_inset();
        let view = Rect {
            x: inset,
            y: layout.middle.y,
            width: (layout.groups[0] - inset - inset).max(Px(0)),
            height: layout.middle.height,
        };
        win::move_children(&[(self.view.hwnd(), view)]);
        if let Some(tips) = self.part_tips {
            tips.place_area(VIEW_TEXT, view);
            tips.set_area(VIEW_TEXT, &self.view.hwnd().text());
        }
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
        let mut work = Vec::new();
        let mut transfer_caption = None;
        let mut right = Vec::new();
        let mut memory = None;
        for (slot, entry) in contents {
            match (slot, entry) {
                (Slot::Left, Entry::Text(text)) => view.push(*text),
                (Slot::Center, Entry::Indexing(caption)) => activity.push(*caption),
                (Slot::Center, Entry::Work(id, caption)) => work.push((*id, *caption)),
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
        let Some(layout) = layout else { return false };
        let dpi = self.bar.hwnd().dpi();
        let base = crate::core::layout::Dpi::new(96).expect("96 is not zero");
        let minimum = work.iter().fold(Px(0), |width, (_, job)| {
            width
                + Px(48).rescale(base, dpi)
                + if job.progress != JobProgress::None {
                    layout.progress.width + Px(6).rescale(base, dpi)
                } else {
                    Px(0)
                }
                + if job.cancel {
                    layout.cancel.width + Px(6).rescale(base, dpi)
                } else {
                    Px(0)
                }
        }) + Px(if transfer_caption.is_some() { 240 } else { 0 }).rescale(base, dpi)
            + Px(6 * work.len() as i32 + 12).rescale(base, dpi);
        self.work_width.set(minimum);
        let layout = if self.transfer.is_some() {
            self.show_window_end(layout, &right, memory)
        } else {
            self.place(layout);
            *layout
        };
        let (transfer_rect, mut lost) =
            self.place_activity(&layout, &work, &activity, transfer_caption);
        if let Some(transfer) = self.transfer {
            self.split.set(
                transfer_rect
                    .filter(|_| !work.is_empty() || !activity.is_empty())
                    .map(|rect| rect.x),
            );
            self.show_window_end(&layout, &right, memory);
            lost |= match (transfer_caption, transfer_rect) {
                (Some(caption), Some(rect)) => transfer.show(caption, rect),
                _ => transfer.hide(),
            };
        }
        lost
    }

    fn place_activity(
        &self,
        layout: &StatusLayout,
        jobs: &[(WorkId, &JobCaption)],
        activity: &[&IndexingCaption],
        transfer: Option<&crate::core::status_transfer::Caption>,
    ) -> (Option<Rect>, bool) {
        let mut content = *layout;
        let inset = self.content_inset();
        content.middle.x = content.middle.x + inset;
        content.middle.width = (content.middle.width - inset - inset).max(Px(0));
        let layout = &content;
        let Ok(font) = Font::of(self.bar.hwnd()) else {
            return (None, false);
        };
        let meter = TextMeter::new(&font);
        let dpi = self.bar.hwnd().dpi();
        let base = crate::core::layout::Dpi::new(96).expect("96 is not zero");
        let gap = Px(6).rescale(base, dpi);
        let padding = Px(4).rescale(base, dpi);
        let indexing_minimum = activity.first().map_or(Px(0), |caption| {
            meter.plain_width(&caption.compact) + padding
        });
        let mut indexing = self.indexing.borrow_mut();
        indexing.truncate(activity.len());
        while indexing.len() < activity.len() {
            let Ok(control) = Indexing::create(self.bar.hwnd()) else {
                return (None, false);
            };
            indexing.push(control);
        }
        let activity_widths: Vec<_> = activity
            .iter()
            .map(|caption| {
                meter.plain_width(&caption.label)
                    + meter.plain_width(&caption.value)
                    + layout.progress.width
                    + gap
                    + gap
            })
            .collect();
        let activity_wanted = activity_widths
            .iter()
            .fold(Px(0), |sum, width| sum + *width)
            + Px(gap.0 * activity.len().saturating_sub(1) as i32);
        let entries = jobs.len() + usize::from(transfer.is_some());
        let activity_reserved = activity_wanted
            .min(Px(150).rescale(base, dpi))
            .max(indexing_minimum)
            .min(
                (layout.middle.width - Px(150 * entries as i32).rescale(base, dpi) - gap)
                    .max(Px(0)),
            );
        let mut wanted = Vec::new();
        let mut minimum = Vec::new();
        for (_, job) in jobs {
            let controls = if job.progress != JobProgress::None {
                layout.progress.width + gap
            } else {
                Px(0)
            } + if job.cancel {
                layout.cancel.width + gap
            } else {
                Px(0)
            };
            wanted.push(meter.plain_width(&job.text).min(layout.running_text) + padding + controls);
            minimum.push(Px(48).rescale(base, dpi) + controls);
        }
        if let Some((caption, control)) = transfer.zip(self.transfer) {
            wanted.push(control.width(caption, layout.middle));
            minimum.push(Px(240).rescale(base, dpi));
        }
        let room = (layout.middle.width
            - activity_reserved
            - if activity_reserved > Px(0) && entries > 0 {
                gap
            } else {
                Px(0)
            })
        .max(Px(0));
        let widths = crate::core::status_bar::work_widths(room, &wanted, &minimum, gap);
        let used = widths.iter().fold(Px(0), |sum, width| sum + *width)
            + Px(gap.0 * widths.len().saturating_sub(1) as i32);
        let activity_available =
            (layout.middle.width - used - if used > Px(0) { gap } else { Px(0) }).max(Px(0));
        let compact = activity_available < activity_wanted;
        let activity_width = if compact {
            indexing_minimum.min(activity_available)
        } else {
            activity_wanted
        };
        let mut all_widths = widths;
        if activity_width > Px(0) {
            all_widths.push(activity_width);
        }
        let placed = layout.place_entries(&all_widths, dpi);
        let mut lost = false;
        for (id, control) in &self.work {
            if let Some(index) = jobs.iter().position(|(shown, _)| shown == id) {
                lost |= control.show(jobs[index].1, placed[index], layout, &meter, gap);
            } else {
                lost |= control.hide();
            }
        }
        let transfer_rect = transfer.and_then(|_| placed.get(jobs.len()).copied());
        let activity_rect = if activity_width > Px(0) {
            *placed.last().expect("visible indexing has an entry width")
        } else {
            Rect::default()
        };
        let mut area = activity_rect;
        for (index, ((control, caption), wanted)) in indexing
            .iter()
            .zip(activity)
            .zip(activity_widths)
            .enumerate()
        {
            let width = if compact {
                if index == 0 { activity_width } else { Px(0) }
            } else {
                wanted
            };
            control.show(
                caption,
                Rect { width, ..area },
                layout.progress,
                &meter,
                gap,
                compact,
            );
            area.x = area.x + width + gap;
            area.width = (area.width - width - gap).max(Px(0));
        }
        if let Some(tips) = self.part_tips {
            tips.place_area(ACTIVITY_TEXT, activity_rect);
            let text = activity
                .iter()
                .map(|caption| format!("{} {}", caption.label, caption.value))
                .collect::<Vec<_>>()
                .join(" ");
            tips.set_area(ACTIVITY_TEXT, &text);
        }
        (transfer_rect, lost)
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
            .map(|item| {
                let width = |text: &str| {
                    let connected = item
                        .connector
                        .as_ref()
                        .map(|connector| format!("{text} {connector}"));
                    self.bar
                        .part_width(connected.as_deref().unwrap_or(text), item.glyph)
                };
                RightWidth {
                    whole: width(&item.text),
                    short: item.narrow.as_ref().map(|narrow| width(&narrow.text)),
                }
            })
            .collect();
        let view = self.view.hwnd().text();
        let view = if view.is_empty() {
            Px(0)
        } else {
            self.bar.part_width(&view, None)
        };
        let dpi = self.bar.hwnd().dpi();
        let width = self.bar.hwnd().client_size().width;
        let wanted = widths.iter().fold(Px(0), |sum, width| sum + width.whole);
        let layout = layout
            .reserve_middle(self.work_width.get())
            .fit_groups(view, wanted, width, dpi);
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
            let connected = item.and_then(|(item, _)| {
                item.connector
                    .as_ref()
                    .filter(|_| part + 1 < right_group + shown)
                    .map(|connector| format!("{text} {connector}"))
            });
            let text = connected.as_deref().unwrap_or(text);
            let total = item
                .is_some_and(|(item, _)| matches!(item.glyph, Some(Glyph::Page | Glyph::Folder)));
            self.bar.set_text_with_border(part, text, !total);
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
        self.bar.set_text(VIEW_GROUP, "");
        self.view.hwnd().set_text(text);
        self.view.hwnd().set_visible(!text.is_empty());
    }

    fn content_inset(&self) -> Px {
        Px(6).rescale(
            crate::core::layout::Dpi::new(96).expect("96 is not zero"),
            self.bar.hwnd().dpi(),
        )
    }

    pub(super) fn cancelled(&self, event: ControlEvent) -> Option<WorkId> {
        self.work
            .iter()
            .find_map(|(id, work)| work.cancel_clicked(event).then_some(*id))
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
