use super::captions::JobProgress;
use super::file_operations::QueueAction;
use super::glyph::Glyph;
use super::layout::{Dpi, Px, Rect};
use gozler_fileops::OperationId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Action {
    pub(crate) text: String,
    pub(crate) glyph: Glyph,
    pub(crate) command: QueueAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Caption {
    pub(crate) id: OperationId,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) activity: String,
    pub(crate) glyph: Option<Glyph>,
    pub(crate) progress: JobProgress,
    pub(crate) paused: bool,
    pub(crate) percent: String,
    pub(crate) byte_rate: String,
    pub(crate) file_rate: String,
    pub(crate) pause: Option<Action>,
    pub(crate) cancel: Option<Action>,
    pub(crate) report: Option<Action>,
    pub(crate) summary_tip: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Input {
    Toggle,
    Action(OperationId, QueueAction),
}

pub(crate) struct Widths {
    pub(crate) name: Px,
    pub(crate) activity: Px,
    pub(crate) icon: Px,
    pub(crate) percent: Px,
    pub(crate) byte_rate: Px,
    pub(crate) file_rate: Px,
    pub(crate) separator: Px,
    pub(crate) actions: [Option<Px>; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) summary: Rect,
    pub(crate) icon: Option<Rect>,
    pub(crate) activity: Rect,
    pub(crate) progress: Option<Rect>,
    pub(crate) statistics: Rect,
    pub(crate) files: bool,
    pub(crate) actions: [Option<Rect>; 3],
}

impl Layout {
    pub(crate) fn compute(
        middle: Rect,
        widths: &Widths,
        measured: bool,
        font_height: Px,
        dpi: Dpi,
    ) -> Layout {
        let px = |value| Px(value).rescale(Dpi::new(96).expect("96 is not zero"), dpi);
        let gap = px(6);
        let mut remaining = middle.width.max(Px(0));
        let mut actions = [None; 3];
        // Cancel survives Pause when another job leaves room for one action.
        for (index, width) in widths.actions.iter().enumerate().rev() {
            if let Some(width) = width
                && *width + gap <= remaining
            {
                remaining = remaining - *width - gap;
                actions[index] = Some(*width);
            }
        }
        let available = remaining;
        let inset = px(8).min(Px(available.0 / 2));
        let icon_width = widths.icon.min((available - inset - inset).max(Px(0)));
        let button = (inset + inset + icon_width + widths.name).min(available);
        let activity_gap = gap.min((available - button).max(Px(0)));
        let bar = if measured && button + activity_gap + px(64) + gap <= available {
            px(64) + gap
        } else {
            Px(0)
        };
        let statistics = |files| {
            let mut width = widths.percent;
            for value in [
                widths.byte_rate,
                if files { widths.file_rate } else { Px(0) },
            ] {
                if value > Px(0) {
                    if width > Px(0) {
                        width = width + widths.separator;
                    }
                    width = width + value;
                }
            }
            width
        };
        let fixed = button + activity_gap + bar;
        let files = fixed + widths.activity + statistics(true) + gap <= available;
        let statistics_width = statistics(files).min((available - fixed).max(Px(0)));
        let activity = widths
            .activity
            .min((available - fixed - statistics_width - gap).max(Px(0)));
        let summary_width = (fixed + activity + gap + statistics_width).min(available);
        let mut x = middle.x;
        let summary = Rect {
            x,
            width: button,
            ..middle
        };
        let icon = (icon_width >= px(16)).then_some(Rect {
            x: inset,
            y: Px((middle.height - px(16)).0 / 2),
            width: px(16),
            height: px(16),
        });
        let progress = (bar > Px(0)).then_some(Rect {
            x: x + button + activity_gap + activity + gap,
            y: middle.y + Px((middle.height - px(10)).0 / 2),
            width: px(64),
            height: px(10),
        });
        let statistics = Rect {
            x: x + summary_width - statistics_width,
            y: middle.y + Px((middle.height - font_height).0 / 2),
            width: statistics_width,
            height: font_height,
        };
        x = x + summary_width;
        let actions = actions.map(|width| {
            width.map(|width| {
                x = x + gap;
                let rect = Rect { x, width, ..middle };
                x = x + width;
                rect
            })
        });
        Layout {
            summary,
            icon,
            activity: Rect {
                x: summary.right() + activity_gap,
                y: statistics.y,
                width: activity,
                height: font_height,
            },
            progress,
            statistics,
            files,
            actions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_summary_drops_files_before_clipping_activity_and_keeps_actions_separate() {
        let middle = Rect {
            x: Px(100),
            y: Px(0),
            width: Px(420),
            height: Px(24),
        };
        let widths = Widths {
            name: Px(30),
            activity: Px(110),
            icon: Px(20),
            percent: Px(24),
            byte_rate: Px(60),
            file_rate: Px(60),
            separator: Px(12),
            actions: [Some(Px(24)), Some(Px(24)), None],
        };
        let layout = Layout::compute(middle, &widths, true, Px(16), Dpi::new(96).unwrap());
        assert!(!layout.files);
        assert_eq!(layout.activity.width, Px(110));
        assert!(layout.summary.right() < layout.actions[0].unwrap().x);
        assert!(layout.actions[1].unwrap().right() <= middle.right());
    }

    #[test]
    fn shared_job_cannot_push_transfer_controls_outside_their_remaining_room() {
        let middle = Rect {
            x: Px(470),
            y: Px(0),
            width: Px(30),
            height: Px(24),
        };
        let widths = Widths {
            name: Px(30),
            activity: Px(110),
            icon: Px(20),
            percent: Px(24),
            byte_rate: Px(60),
            file_rate: Px(60),
            separator: Px(12),
            actions: [Some(Px(24)), Some(Px(24)), None],
        };
        let layout = Layout::compute(middle, &widths, true, Px(16), Dpi::new(96).unwrap());
        assert!(layout.progress.is_none());
        assert!(layout.actions[0].is_none());
        assert!(layout.actions[1].is_some());
        assert!(layout.summary.x >= middle.x);
        assert!(layout.summary.right() <= middle.right());
        for action in layout.actions.into_iter().flatten() {
            assert!(action.x >= layout.summary.right());
            assert!(action.right() <= middle.right());
        }
    }
}
