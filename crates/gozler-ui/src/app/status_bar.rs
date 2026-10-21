//! The status bar (DESIGN.md: Status bar): what the view holds on the
//! left, the running job or the last job's result in the middle, and the
//! service on the right. The main window and the Settings dialog each have
//! one. It owns the Win32 status bar, the progress bar and Cancel in its
//! middle group, and the main window's memory group and Show details at its
//! right end; `core::captions` gives it the texts and `core::layout` the
//! places.

use crate::core::captions::{ButtonIcon, JobCaption, JobProgress, MemoryCaptions, StatusCaptions};
use crate::core::glyph::Glyph;
use crate::core::ids::ControlId;
use crate::core::layout::{Px, StatusLayout};
use crate::win::controls::{self, Button, ProgressBar, Tooltip};
use crate::win::notify::{ControlEvent, Notification};
use crate::win::toolbar::{Tool, Toolbar};
use crate::win::{self, Hwnd};

const VIEW_GROUP: usize = 0;
const JOB_GROUP: usize = 1;
/// The right group, which the window draws, and where a click on a newer
/// release runs Update.
const SERVICE_GROUP: usize = 2;
/// The parts of the main window's memory figures, after the right group
/// while they show.
const SERVICE_MEMORY_PART: usize = 3;
const APP_MEMORY_PART: usize = 4;
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

    /// Places the groups and the controls, and shows Show details while
    /// `layout` has a place for it. `memory` is what the memory figures
    /// show, which their parts show again when the parts change.
    pub(super) fn place(&self, layout: &StatusLayout, memory: Option<&MemoryCaptions>) {
        let parts_changed = self.bar.set_parts(&layout.part_widths());
        if let Some(tips) = &self.memory_tips {
            // A hidden group's tips cover nothing.
            let (service, app) = layout
                .memory
                .map(|memory| (memory.service, memory.app))
                .unwrap_or_default();
            tips.place_area(SERVICE_MEMORY, service);
            tips.place_area(APP_MEMORY, app);
            if parts_changed {
                self.show_memory(memory.filter(|_| layout.memory.is_some()));
            }
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

    pub(super) fn hwnd(&self) -> Hwnd {
        self.bar.hwnd()
    }

    /// Shows every group of `captions` as the main window's, which draws
    /// the right group from `captions.right`, so its icons show. Returns
    /// what [`Self::show_work`] returns.
    pub(super) fn show(&self, captions: &StatusCaptions) -> bool {
        self.show_view(&captions.view);
        self.bar.redraw_owner_drawn(SERVICE_GROUP);
        if captions.memory.is_some() {
            self.show_memory(captions.memory.as_ref());
        }
        self.show_job(&captions.job)
    }

    /// Shows the memory figures, each after its icon, or for `None` empties
    /// their parts' indexes, which other parts take while the figures are
    /// hidden.
    fn show_memory(&self, memory: Option<&MemoryCaptions>) {
        let Some(tips) = &self.memory_tips else {
            return;
        };
        let figures = memory.map(|memory| {
            tips.set_area(SERVICE_MEMORY, &memory.service_tip);
            tips.set_area(APP_MEMORY, &memory.app_tip);
            [&memory.service, &memory.app]
        });
        for (index, part) in [SERVICE_MEMORY_PART, APP_MEMORY_PART]
            .into_iter()
            .enumerate()
        {
            let figure = figures.map(|figures| figures[index]);
            self.bar
                .set_text(part, figure.map_or("", |figure| figure.text.as_str()));
            self.bar
                .set_icon(part, figure.and_then(|figure| figure.glyph));
        }
    }

    pub(super) fn show_view(&self, text: &str) {
        self.bar.set_text(VIEW_GROUP, text);
    }

    /// Shows the job and the service as the Settings dialog does. Returns
    /// what [`Self::show_job`] returns.
    pub(super) fn show_work(&self, job: &JobCaption, service: &str) -> bool {
        self.bar.set_text(SERVICE_GROUP, service);
        self.show_job(job)
    }

    /// Shows the job, with its progress bar and Cancel. Returns whether
    /// Cancel had the focus when it was hidden, so the parent gives the
    /// focus to another control.
    fn show_job(&self, job: &JobCaption) -> bool {
        self.bar.set_text(JOB_GROUP, &job.text);
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

    /// Whether `notification` is a click on the right group.
    pub(super) fn service_clicked(&self, notification: &Notification) -> bool {
        self.bar.clicked_part(notification) == Some(SERVICE_GROUP)
    }
}
