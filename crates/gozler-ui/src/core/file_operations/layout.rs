//! Positions native transfer controls within the shared inspector's client area.

use crate::core::layout::{Dpi, Px, Rect, Size};

/// `dip` at `dpi`, rounded as the main window's layout rounds.
fn px(dip: i32, dpi: Dpi) -> Px {
    Px(dip).rescale(Dpi::new(96).expect("96 is not zero"), dpi)
}

pub(crate) const CARD_ICON: i32 = 32;
pub(crate) const ROW_ICON: i32 = 32;

/// What the layout measures: the fonts' line heights and the widths of the
/// captions that a longer translation widens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Metrics {
    pub(crate) icons: bool,
    pub(crate) dpi: Dpi,
    /// A line of the message font.
    pub(crate) line: Px,
    /// A line of the card's title font.
    pub(crate) title_line: Px,
    pub(crate) heading_width: Px,
    /// The widest of "Copying" and its kind's words, "From" and "To".
    pub(crate) label_width: Px,
    /// The widest percentage, "100 %".
    pub(crate) percent_width: Px,
    /// Hide's width, which fits its icon and caption.
    pub(crate) hide_width: Px,
}

/// Native controls within the shared inspector's transfer content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) heading: Rect,
    pub(crate) count: Rect,
    pub(crate) all: Rect,
    pub(crate) flip: Rect,
    pub(crate) maximize: Rect,
    pub(crate) hide: Rect,
    pub(crate) queue: Rect,
    pub(crate) list: Rect,
    pub(crate) icon: Rect,
    pub(crate) title: Rect,
    pub(crate) status: Rect,
    pub(crate) rate: Rect,
    pub(crate) files_rate: Rect,
    pub(crate) doing: Rect,
    pub(crate) file: Rect,
    pub(crate) file_bar: Rect,
    pub(crate) file_percent: Rect,
    pub(crate) file_bytes: Rect,
    pub(crate) from_label: Rect,
    pub(crate) from: Rect,
    pub(crate) to_label: Rect,
    pub(crate) to: Rect,
    pub(crate) overall: Rect,
    pub(crate) total_bar: Rect,
    pub(crate) percent: Rect,
    pub(crate) items: Rect,
    pub(crate) bytes: Rect,
    pub(crate) pause: Rect,
    pub(crate) cancel: Rect,
    pub(crate) report: Rect,
    pub(crate) summary: Rect,
}

impl Layout {
    pub(crate) fn compute(
        size: Size,
        m: Metrics,
        multiple: bool,
        flipped: bool,
        selected: bool,
    ) -> Layout {
        let d = |dip| px(dip, m.dpi);
        let margin = d(12);
        let gap = d(8);
        let button = m.line + d(12);
        let utility = if m.icons { button } else { m.hide_width };
        let line = m.line + d(3);
        let title_line = m.title_line + d(3);
        let right = size.width - margin;
        let bottom = size.height - margin;
        let rect = |x: Px, y: Px, width: Px, height: Px| Rect {
            x,
            y,
            width: width.max(Px(0)),
            height: height.max(Px(0)),
        };
        let mut l = Layout::default();
        l.hide = rect(right - utility, margin, utility, button);
        l.maximize = rect(l.hide.x - gap - utility, margin, utility, button);
        l.flip = rect(l.maximize.x - gap - utility, margin, utility, button);
        l.all = rect(l.flip.x - gap - m.hide_width, margin, m.hide_width, button);
        l.heading = rect(margin, margin, m.heading_width, button);
        l.count = rect(
            l.heading.right() + gap,
            margin,
            l.all.x - l.heading.right() - gap - gap,
            button,
        );
        let y = margin + button + gap;
        let content = right - margin;
        let queue_width = if selected {
            Px(content.0 * 54 / 100)
        } else {
            content
        };
        let (x, width, queue_x) = if !multiple || !selected {
            (margin, content, margin)
        } else if flipped {
            (
                margin + queue_width + gap,
                content - queue_width - gap,
                margin,
            )
        } else {
            (margin, content - queue_width - gap, right - queue_width)
        };
        l.queue = rect(queue_x, y, queue_width, line);
        l.list = rect(
            queue_x,
            y + line + gap,
            queue_width,
            bottom - y - line - gap,
        );
        let end = x + width;
        let icon = d(CARD_ICON);
        l.icon = rect(x, y, icon, icon);
        let text_x = if m.icons { x + icon + gap } else { x };
        let speed_width = d(110).min(Px(width.0 / 3));
        l.rate = rect(end - speed_width, y, speed_width, title_line);
        l.files_rate = rect(l.rate.x, y + title_line, speed_width, line);
        l.title = rect(text_x, y, l.rate.x - text_x - gap, title_line);
        l.status = rect(text_x, y + title_line, l.title.width, line);
        let mut y = y + title_line + line + gap;
        let value = x + m.label_width + gap;
        let percent_x = end - m.percent_width;
        l.from_label = rect(x, y, m.label_width, line);
        l.from = rect(value, y, end - value, line);
        y = y + line;
        l.to_label = rect(x, y, m.label_width, line);
        l.to = rect(value, y, end - value, line);
        y = y + line + gap;
        l.overall = rect(x, y, m.label_width, line);
        l.total_bar = rect(value, y, percent_x - gap - value, d(18));
        l.percent = rect(percent_x, y, m.percent_width, line);
        y = y + d(18).max(line);
        l.items = rect(x, y, Px(width.0 / 2), line);
        l.bytes = rect(x + Px(width.0 / 2), y, width - Px(width.0 / 2), line);
        y = y + line + gap;
        l.doing = rect(x, y, m.label_width, line);
        l.file = rect(value, y, end - value, line);
        y = y + line;
        l.file_bar = rect(value, y, percent_x - gap - value, d(18));
        l.file_percent = rect(percent_x, y, m.percent_width, line);
        y = y + d(18).max(line);
        l.file_bytes = rect(value, y, end - value, line);
        l.cancel = rect(end - m.hide_width, bottom - button, m.hide_width, button);
        l.pause = rect(
            l.cancel.x - gap - m.hide_width,
            l.cancel.y,
            m.hide_width,
            button,
        );
        l.summary = rect(x, bottom - line, width, line);
        l.report = rect(
            x,
            l.status.bottom() + gap,
            width,
            l.cancel.y - gap - l.status.bottom() - gap,
        );
        l
    }

    /// Fits every progress row above the actions, also with a larger Windows font.
    pub(crate) fn min_height(m: Metrics) -> Px {
        let layout = Self::compute(Size::default(), m, false, false, true);
        (layout.file_bytes.bottom() + px(8, m.dpi) + m.line + px(24, m.dpi)).max(px(300, m.dpi))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_the_selected_report_leaves_the_remaining_queue_reachable() {
        let layout = Layout::compute(
            Size {
                width: Px(800),
                height: Px(340),
            },
            Metrics {
                icons: true,
                dpi: Dpi::new(96).expect("96 is not zero"),
                line: Px(16),
                title_line: Px(16),
                heading_width: Px(80),
                label_width: Px(60),
                percent_width: Px(80),
                hide_width: Px(75),
            },
            false,
            true,
            false,
        );
        assert_eq!(layout.list.x, Px(12));
        assert_eq!(layout.list.width, Px(776));
    }
}
