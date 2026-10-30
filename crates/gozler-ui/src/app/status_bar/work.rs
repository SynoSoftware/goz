use crate::core::captions::{ButtonIcon, JobCaption, JobProgress};
use crate::core::ids::ControlId;
use crate::core::layout::{Px, Rect, StatusLayout};
use crate::win::controls::{Button, Label, ProgressBar, Tooltip};
use crate::win::gdi::TextMeter;
use crate::win::notify::ControlEvent;
use crate::win::{self, Hwnd};
use std::cell::Cell;

pub(super) struct Work {
    text: Label,
    progress: ProgressBar,
    cancel: Button,
    tips: Tooltip,
    shown: Cell<JobProgress>,
}

impl Work {
    pub(super) fn create(parent: Hwnd) -> windows::core::Result<Self> {
        let text = Label::create_line(parent, ControlId::StatusText.id())?;
        let progress = ProgressBar::create(parent, ControlId::StatusProgress.id())?;
        let cancel = Button::create(parent, ControlId::StatusCancel.id())?;
        let tips = Tooltip::create(parent, &[text.hwnd(), cancel.hwnd()])?;
        let work = Self {
            text,
            progress,
            cancel,
            tips,
            shown: Cell::new(JobProgress::None),
        };
        work.hide();
        Ok(work)
    }

    pub(super) fn text_controls(&self) -> [Hwnd; 2] {
        [self.text.hwnd(), self.cancel.hwnd()]
    }

    pub(super) fn set_cancel_caption(&self, caption: &str) {
        self.cancel.hwnd().set_text(caption);
        self.show_cancel_icon();
    }

    pub(super) fn show_cancel_icon(&self) {
        self.cancel.set_icon(ButtonIcon::of("button.cancel", false));
    }

    pub(super) fn set_progress_caption(&self, caption: &str) {
        self.progress.hwnd().set_text(caption);
    }

    pub(super) fn show(
        &self,
        job: &JobCaption,
        area: Rect,
        layout: &StatusLayout,
        meter: &TextMeter,
        gap: Px,
    ) -> bool {
        let progress = job.progress != JobProgress::None;
        let cancel_width = if job.cancel {
            layout.cancel.width + gap
        } else {
            Px(0)
        };
        let progress_width = if progress {
            layout
                .progress
                .width
                .min(Px((area.width - cancel_width).0.max(0) / 3))
                + gap
        } else {
            Px(0)
        };
        let text_width = (area.width - cancel_width - progress_width).max(Px(0));
        let text = Rect {
            width: text_width,
            y: area.y + Px((area.height - meter.height()).0 / 2),
            height: meter.height(),
            ..area
        };
        let bar = Rect {
            x: text.right() + gap,
            width: (progress_width - gap).max(Px(0)),
            ..layout.progress
        };
        let cancel = Rect {
            x: area.right() - layout.cancel.width,
            ..layout.cancel
        };
        win::move_children(&[
            (self.text.hwnd(), text),
            (self.progress.hwnd(), bar),
            (self.cancel.hwnd(), cancel),
        ]);
        let caption = meter.compact_path(&job.text, text_width);
        self.text.hwnd().set_text(&caption);
        self.tips.set(self.text.hwnd(), &job.text);
        self.tips.set(
            self.cancel.hwnd(),
            &format!("{}: {}", self.cancel.hwnd().text(), job.text),
        );
        self.progress.hwnd().set_text(&job.text);
        if self.shown.replace(job.progress) != job.progress {
            match job.progress {
                JobProgress::None => self.progress.set_moving(false),
                JobProgress::Unmeasured => self.progress.set_moving(true),
                JobProgress::Measured { done, total } => self.progress.set_measured(done, total),
            }
        }
        let lost = !job.cancel && Hwnd::focused() == Some(self.cancel.hwnd());
        self.text.hwnd().set_visible(text_width > Px(0));
        self.progress
            .hwnd()
            .set_visible(progress && bar.width > Px(0));
        self.cancel.hwnd().set_visible(job.cancel);
        lost
    }

    pub(super) fn hide(&self) -> bool {
        let lost = Hwnd::focused() == Some(self.cancel.hwnd());
        self.text.hwnd().set_visible(false);
        self.progress.hwnd().set_visible(false);
        self.cancel.hwnd().set_visible(false);
        if self.shown.replace(JobProgress::None) != JobProgress::None {
            self.progress.set_moving(false);
        }
        lost
    }

    pub(super) fn cancel_clicked(&self, event: ControlEvent) -> bool {
        self.cancel.hwnd().is_visible() && self.cancel.clicked(event)
    }
}
