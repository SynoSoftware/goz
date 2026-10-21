//! The status bar (DESIGN.md: Status bar): what the view holds on the
//! left, the running job or the last job's result in the middle, and the
//! service on the right. The main window and the Settings dialog each have
//! one. It owns the Win32 status bar, the progress bar and Cancel in its
//! middle group, and the main window's memory group and Show details at its
//! right end; `core::captions` gives it the texts and `core::layout` the
//! places.

use crate::core::captions::{
    ButtonIcon, JobCaption, JobProgress, MemoryCaptions, StatusCaptions, ValuePart,
};
use crate::core::glyph::Glyph;
use crate::core::ids::ControlId;
use crate::core::layout::{Px, StatusLayout};
use crate::win::controls::{self, Button, ProgressBar, Tooltip};
use crate::win::notify::{ControlEvent, Notification};
use crate::win::toolbar::{Tool, Toolbar};
use crate::win::{self, Hwnd};

const VIEW_GROUP: usize = 0;
const JOB_GROUP: usize = 1;
/// The right group's first part, where a click on a newer release runs
/// Update. In the main window, the right group's other parts follow it,
/// then the memory figures' parts while they show.
const RIGHT_GROUP: usize = 2;
/// The memory figures, by their tooltip's area.
const SERVICE_MEMORY: usize = 0;
const APP_MEMORY: usize = 1;

#[derive(Clone, Copy)]
pub(super) struct StatusBar {
    bar: controls::StatusBar,
    progress: ProgressBar,
    cancel: Button,
    /// The main window's Show details, a toolbar of its one icon.
    show_details: Option<Toolbar>,
    /// The tooltips of the main window's memory figures.
    memory_tips: Option<Tooltip>,
}

impl StatusBar {
    /// Creates the main window's status bar at the bottom of `parent`, with
    /// the size grip, because the person resizes the window, the memory
    /// group and Show details.
    pub(super) fn for_window(parent: Hwnd) -> windows::core::Result<StatusBar> {
        let mut status = StatusBar::create(parent, true)?;
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
        status.memory_tips = Some(Tooltip::create_areas(status.bar.hwnd(), 2)?);
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
            progress: ProgressBar::create(bar.hwnd(), ControlId::StatusProgress.id())?,
            cancel: Button::create(bar.hwnd(), ControlId::StatusCancel.id())?,
            show_details: None,
            memory_tips: None,
        };
        status.progress.hwnd().set_visible(false);
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
    pub(super) fn text_controls(&self) -> [Hwnd; 2] {
        [self.bar.hwnd(), self.cancel.hwnd()]
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

    /// Places a dialog's groups and controls.
    pub(super) fn place(&self, layout: &StatusLayout) {
        self.bar.set_parts(&layout.part_widths(&[]));
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
        self.place_controls(layout);
    }

    fn place_controls(&self, layout: &StatusLayout) {
        if let Some(tips) = &self.memory_tips {
            // A hidden group's tips cover nothing.
            let (service, app) = layout
                .memory
                .map(|memory| (memory.service, memory.app))
                .unwrap_or_default();
            tips.place_area(SERVICE_MEMORY, service);
            tips.place_area(APP_MEMORY, app);
        }
        let mut moves = vec![
            (self.progress.hwnd(), layout.progress),
            (self.cancel.hwnd(), layout.cancel),
        ];
        if let Some(show) = self.show_details {
            if let Some(rect) = layout.show_details {
                moves.push((show.hwnd(), rect));
            }
            show.hwnd().set_visible(layout.show_details.is_some());
        }
        win::move_children(&moves);
    }

    /// Shows every group of `captions` as the main window's, the right
    /// group and the memory figures in `layout`'s parts, or none before the
    /// window has a layout, with the job's text shown as `job_text`.
    /// Returns what [`Self::show_work`] returns.
    pub(super) fn show(
        &self,
        captions: &StatusCaptions,
        job_text: &str,
        layout: Option<&StatusLayout>,
    ) -> bool {
        self.show_view(&captions.view);
        if let Some(layout) = layout {
            self.show_window_end(layout, &captions.right, captions.memory.as_ref());
        }
        self.show_job(&captions.job, job_text)
    }

    /// Makes a part of the main window's status bar for each item of
    /// `right` that `layout` has room for, then for each memory figure
    /// while they show, and shows them, each after its icon. A part keeps
    /// the text and icon of the part that had its index before, so every
    /// part after the middle group is shown again, or emptied.
    fn show_window_end(
        &self,
        layout: &StatusLayout,
        right: &[ValuePart],
        memory: Option<&MemoryCaptions>,
    ) {
        let widths: Vec<Px> = right
            .iter()
            .map(|item| self.bar.part_width(&item.text, item.glyph))
            .collect();
        let shown = layout.right_parts_shown(&widths);
        let part_widths = layout.part_widths(&widths[..shown]);
        self.bar.set_parts(&part_widths);
        let memory = memory.filter(|_| layout.memory.is_some());
        if let (Some(tips), Some(memory)) = (&self.memory_tips, memory) {
            tips.set_area(SERVICE_MEMORY, &memory.service_tip);
            tips.set_area(APP_MEMORY, &memory.app_tip);
        }
        let figures = memory.map(|memory| [&memory.service, &memory.app]);
        // The right group has a part also while it shows nothing.
        let items = right[..shown]
            .iter()
            .map(Some)
            .chain((shown == 0).then_some(None))
            .chain(figures.into_iter().flatten().map(Some));
        // The parts after them, Show details' or none, show nothing.
        let parts = (RIGHT_GROUP..=part_widths.len()).zip(items.chain(std::iter::repeat(None)));
        for (part, item) in parts {
            let text = item.map_or("", |item| item.text.as_str());
            self.bar.set_text(part, text);
            // A part's tip shows only while its text is cut off.
            self.bar.set_tip(part, text);
            self.bar.set_icon(part, item.and_then(|item| item.glyph));
        }
    }

    pub(super) fn show_view(&self, text: &str) {
        self.bar.set_text(VIEW_GROUP, text);
    }

    /// Shows the job and the service as the Settings dialog does. Returns
    /// what [`Self::show_job`] returns.
    pub(super) fn show_work(&self, job: &JobCaption, job_text: &str, service: &str) -> bool {
        self.bar.set_text(RIGHT_GROUP, service);
        // A long state is cut off, so the tooltip shows it whole.
        self.bar.set_tip(RIGHT_GROUP, service);
        self.show_job(job, job_text)
    }

    /// Shows the job as `text`, with its progress bar and Cancel. Returns
    /// whether Cancel had the focus when it was hidden, so the parent gives
    /// the focus to another control.
    fn show_job(&self, job: &JobCaption, text: &str) -> bool {
        self.bar.set_text(JOB_GROUP, text);
        // A long result ends in an ellipsis, so the tooltip shows it whole.
        self.bar.set_tip(JOB_GROUP, &job.text);
        match job.progress {
            JobProgress::None => self.progress.set_moving(false),
            JobProgress::Unmeasured => self.progress.set_moving(true),
            JobProgress::Measured { done, total } => self.progress.set_measured(done, total),
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

    /// Whether `notification` is a click on the right group's first part,
    /// which shows the index state.
    pub(super) fn service_clicked(&self, notification: &Notification) -> bool {
        self.bar.clicked_part(notification) == Some(RIGHT_GROUP)
    }
}
