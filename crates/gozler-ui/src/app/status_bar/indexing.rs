use crate::core::captions::{IndexingCaption, JobProgress};
use crate::core::layout::{Px, Rect};
use crate::win::controls::{Label, ProgressBar};
use crate::win::gdi::{Font, TextMeter};
use crate::win::{self, Hwnd};
use std::cell::Cell;

pub(super) struct Indexing {
    label: Label,
    bar: ProgressBar,
    value: Label,
    progress: Cell<JobProgress>,
    // The labels borrow this font until their parent replaces it.
    _font: Font,
}

impl Indexing {
    pub(super) fn create(parent: Hwnd) -> windows::core::Result<Self> {
        let font = Font::of(parent)?;
        let label = Label::create_line(parent, 0)?;
        let bar = ProgressBar::create(parent, 0).inspect_err(|_| label.hwnd().destroy())?;
        let value = Label::create_line(parent, 0).inspect_err(|_| {
            label.hwnd().destroy();
            bar.hwnd().destroy();
        })?;
        label.hwnd().set_font(&font);
        value.hwnd().set_font(&font);
        for hwnd in [label.hwnd(), bar.hwnd(), value.hwnd()] {
            hwnd.set_visible(false);
        }
        Ok(Self {
            label,
            bar,
            value,
            progress: Cell::new(JobProgress::None),
            _font: font,
        })
    }

    pub(super) fn text_controls(&self) -> [Hwnd; 2] {
        [self.label.hwnd(), self.value.hwnd()]
    }

    pub(super) fn show(
        &self,
        caption: &IndexingCaption,
        area: Rect,
        bar: Rect,
        meter: &TextMeter,
        gap: Px,
        compact: bool,
    ) {
        self.label.hwnd().set_text(if compact {
            &caption.compact
        } else {
            &caption.label
        });
        self.value.hwnd().set_text(&caption.value);
        self.bar
            .hwnd()
            .set_text(&format!("{} {}", caption.label, caption.value));
        let bar_width = if compact {
            Px(0)
        } else {
            bar.width.min(Px(area.width.0 / 2))
        };
        let available = (area.width - bar_width - gap - gap).max(Px(0));
        let label_width = if compact {
            area.width
        } else {
            meter.plain_width(&caption.label)
        };
        let value_width = (available - label_width).max(Px(0));
        let label = Rect {
            y: area.y + Px((area.height - meter.height()).0 / 2),
            height: meter.height(),
            width: label_width,
            ..area
        };
        let bar = Rect {
            x: label.right() + gap,
            width: bar_width,
            ..bar
        };
        let value = Rect {
            x: bar.right() + gap,
            width: value_width,
            ..label
        };
        win::move_children(&[
            (self.label.hwnd(), label),
            (self.bar.hwnd(), bar),
            (self.value.hwnd(), value),
        ]);
        let visible = area.width > Px(0);
        let progress = if visible && !compact {
            caption.progress
        } else {
            JobProgress::None
        };
        if self.progress.replace(progress) != progress {
            match progress {
                JobProgress::Measured { done, total } => self.bar.set_measured(done, total),
                JobProgress::Unmeasured => self.bar.set_moving(true),
                JobProgress::None => self.bar.set_moving(false),
            }
        }
        self.label.hwnd().set_visible(visible);
        self.bar.hwnd().set_visible(visible && !compact);
        self.value.hwnd().set_visible(visible && !compact);
    }
}

impl Drop for Indexing {
    fn drop(&mut self) {
        for hwnd in [self.label.hwnd(), self.bar.hwnd(), self.value.hwnd()] {
            hwnd.destroy();
        }
    }
}
