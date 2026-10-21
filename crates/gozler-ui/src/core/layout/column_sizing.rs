//! Search's column widths: how a column fits its header and cells, how
//! the fits fill the list's width, and when a fill happens (DESIGN.md:
//! Results list). The window measures the texts and sets the widths that
//! [`ColumnSizing`] answers with.

use super::{Dip, Dpi, Px, cell_text_inset};
use crate::core::model::Column;

/// The width of `column` on a monitor of `dpi` until a fill gives it one.
pub(crate) fn column_width(column: Column, dpi: Dpi) -> Px {
    column.row().width.to_px(dpi)
}

/// The narrowest a fill makes a column, so its header still shows a few
/// letters.
const COLUMN_MIN_WIDTH: Dip = Dip(40);

/// The space a header leaves on each side of its text.
const HEADER_TEXT_MARGIN: Dip = Dip(6);

/// What a column's header and the cells a fit measured ask of a fill
/// (DESIGN.md: Results list). Each width is at least the header's and at
/// least the narrowest a fill makes a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ColumnFit {
    /// The width that shows every measured cell whole.
    pub(crate) widest: Px,
    /// The mean width of the measured cells, the column's share when the
    /// list is too narrow for every widest width; the header's width when
    /// no measured cell shows anything.
    pub(crate) average: Px,
    /// The narrowest a fill makes the column.
    pub(crate) narrowest: Px,
}

impl ColumnFit {
    /// The fit of a column whose header text is `header` wide and whose
    /// measured cells that show anything have texts `texts` wide, in the
    /// column `first` when it is the list's first, with each text starting
    /// `label` after the column's left edge.
    pub(crate) fn of(
        header: Px,
        texts: impl IntoIterator<Item = Px>,
        label: Px,
        first: bool,
        dpi: Dpi,
    ) -> ColumnFit {
        let margin = HEADER_TEXT_MARGIN.to_px(dpi);
        let narrowest = COLUMN_MIN_WIDTH.to_px(dpi);
        let least = (header + margin + margin).max(narrowest);
        let (mut widest, mut sum, mut count) = (least, 0_i64, 0_i64);
        for text in texts {
            let cell = cell_width(text, label, first, dpi);
            widest = widest.max(cell);
            sum += i64::from(cell.0);
            count += 1;
        }
        let average = if count == 0 {
            least
        } else {
            Px(i32::try_from(sum / count).expect("a mean of i32 widths fits in i32")).max(least)
        };
        ColumnFit {
            widest,
            average,
            narrowest,
        }
    }

    /// The fit of `column` before any row is read: its first width, as
    /// though every column had content, so the first fill keeps the
    /// columns' proportions until the rows arrive.
    pub(crate) fn first(column: Column, dpi: Dpi) -> ColumnFit {
        let width = column_width(column, dpi);
        ColumnFit {
            widest: width,
            average: width,
            narrowest: COLUMN_MIN_WIDTH.to_px(dpi),
        }
        .for_column(column)
    }

    /// This fit, kept at its widest when `column`'s cells are never cut
    /// (`ColumnRow::whole`).
    pub(crate) fn for_column(self, column: Column) -> ColumnFit {
        if column.row().whole {
            ColumnFit {
                narrowest: self.widest,
                ..self
            }
        } else {
            self
        }
    }
}

/// The width of a cell whose text is `text` wide, in the column `first`
/// when it is the list's first, whose text starts `label` after the
/// column's left edge, past the row's icon: the text with the list's
/// space on each side.
pub(crate) fn cell_width(text: Px, label: Px, first: bool, dpi: Dpi) -> Px {
    let inset = cell_text_inset(first, dpi);
    label + inset + text + inset
}

/// The widths that make columns with these `fits` end at `width`
/// (DESIGN.md: Results list): each column is
/// `grow * clamp(share * average, narrowest, widest)`, where `share` makes
/// the widths add up to `width` and `grow` is 1 until every column is at
/// its widest, then the factor that takes them all to `width`. Sharing by
/// the average keeps one long cell from taking the room of the other
/// columns, and one factor for every column keeps a resize from making a
/// column jump. When even the narrowest widths do not fit, the columns run
/// past the edge.
pub(crate) fn fill_widths(fits: &[ColumnFit], width: Px) -> Vec<Px> {
    let widest: f64 = fits.iter().map(|fit| f64::from(fit.widest.0)).sum();
    let room = f64::from(width.0);
    let grow = (room / widest.max(1.0)).max(1.0);
    let shared = share(fits, room.min(widest));
    let widths: Vec<f64> = shared.iter().map(|width| width * grow).collect();
    whole_pixels(&widths, width)
}

/// The widths `clamp(share * average, narrowest, widest)` that add up to
/// `room`, or the narrowest widths when even they do not fit. Each round
/// tries one factor for the columns not yet held at a bound. When the
/// clamped widths come out wider than the room, the columns that the
/// factor took below their narrowest hold there; when narrower, those it
/// took past their widest hold there; the others share what is left in
/// the next round, so the answer is the one factor that fits.
fn share(fits: &[ColumnFit], room: f64) -> Vec<f64> {
    let mut held: Vec<Option<f64>> = vec![None; fits.len()];
    loop {
        let free: Vec<usize> = (0..fits.len()).filter(|&i| held[i].is_none()).collect();
        if free.is_empty() {
            return held.into_iter().flatten().collect();
        }
        let left = room - held.iter().flatten().sum::<f64>();
        let weight: f64 = free.iter().map(|&i| f64::from(fits[i].average.0)).sum();
        let factor = left.max(0.0) / weight.max(1.0);
        let wanted = |i: usize| factor * f64::from(fits[i].average.0);
        let clamped =
            |i: usize| wanted(i).clamp(f64::from(fits[i].narrowest.0), f64::from(fits[i].widest.0));
        let excess: f64 = free.iter().map(|&i| clamped(i) - wanted(i)).sum();
        for &i in &free {
            let (bound, asked) = (clamped(i), wanted(i));
            let holds = if excess > 0.0 {
                bound > asked
            } else if excess < 0.0 {
                bound < asked
            } else {
                true
            };
            if holds {
                held[i] = Some(bound);
            }
        }
    }
}

/// `widths` rounded down to whole pixels, with the pixels that leaves going
/// one each to the first columns, so the columns end exactly at `width`
/// whenever they fit in it.
fn whole_pixels(widths: &[f64], width: Px) -> Vec<Px> {
    // Rounded down, so the sum is no wider than the widths it rounds.
    let mut pixels: Vec<Px> = widths.iter().map(|&w| Px(w as i32)).collect();
    let left = width.0 - pixels.iter().map(|px| px.0).sum::<i32>();
    for px in pixels.iter_mut().take(usize::try_from(left).unwrap_or(0)) {
        px.0 += 1;
    }
    pixels
}

/// When Search's columns fill the list's width, and the widths that
/// change between fills (DESIGN.md: Results list). Each event that fills
/// the columns measures their fits with the `measure` it is given and
/// answers with the widths.
#[derive(Debug, Default)]
pub(crate) struct ColumnSizing {
    /// The width the columns last filled, so a layout that keeps it fills
    /// nothing.
    filled_width: Option<Px>,
    /// A drag of the window's border or title bar, between
    /// `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`.
    drag: Option<Drag>,
    /// The width each column had when the person hid it, at the DPI then,
    /// which it gets again when shown, for the window's life.
    hidden: Vec<(Column, Px, Dpi)>,
    /// Whether the columns have filled the list around rows; until then
    /// they hold their first widths.
    filled_around_rows: bool,
}

#[derive(Debug, Default)]
struct Drag {
    /// The fits that each fill of the drag scales, once a fill of the
    /// drag measured them, so the widths follow the pointer without
    /// measuring every time.
    fits: Option<Vec<ColumnFit>>,
}

impl ColumnSizing {
    pub(crate) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The list is `width` wide after a layout: when the window shows, is
    /// resized, or a pane beside the list shows, hides or moves. Answers
    /// the widths that fill it, or `None` when the columns filled that
    /// width last. A drag measures once and scales those fits at each
    /// width; [`Self::drag_ended`] measures again at the final width.
    pub(crate) fn resized(
        &mut self,
        width: Px,
        measure: impl FnOnce() -> Vec<ColumnFit>,
    ) -> Option<Vec<Px>> {
        if self.filled_width.replace(width) == Some(width) {
            return None;
        }
        Some(match &mut self.drag {
            None => fill_widths(&measure(), width),
            Some(drag) => fill_widths(drag.fits.get_or_insert_with(measure), width),
        })
    }

    pub(crate) fn drag_started(&mut self) {
        // Fits measured before this drag were measured on other rows.
        self.drag = Some(Drag::default());
    }

    /// The drag ended with the list `width` wide: when the drag resized
    /// the list, the widths that fit the rows it shows now.
    pub(crate) fn drag_ended(
        &mut self,
        width: Px,
        measure: impl FnOnce() -> Vec<ColumnFit>,
    ) -> Option<Vec<Px>> {
        self.drag.take()?.fits?;
        Some(self.fill(width, measure))
    }

    /// The window moved to a monitor of another DPI: the widths and the
    /// drag's fits are in the old DPI's pixels, so the next layout fills
    /// afresh.
    pub(crate) fn dpi_changed(&mut self) {
        self.forget_drag_fits();
        self.filled_width = None;
    }

    /// The rows the list shows first arrived, with the list `width` wide:
    /// the widths that fit them, once, because the window shows before any
    /// row, when only the columns' first widths stand for their fits.
    pub(crate) fn rows_arrived(
        &mut self,
        width: Px,
        measure: impl FnOnce() -> Vec<ColumnFit>,
    ) -> Option<Vec<Px>> {
        if std::mem::replace(&mut self.filled_around_rows, true) {
            return None;
        }
        Some(self.fill(width, measure))
    }

    /// The window shows `width` wide: the widths that fit the rows it
    /// shows, because a hidden window shows at the width it had, which no
    /// resize fills.
    pub(crate) fn window_shown(
        &mut self,
        width: Px,
        measure: impl FnOnce() -> Vec<ColumnFit>,
    ) -> Vec<Px> {
        self.fill(width, measure)
    }

    /// The person hid `column`, which was `width` wide on a monitor of
    /// `dpi`.
    pub(crate) fn column_hidden(&mut self, column: Column, width: Px, dpi: Dpi) {
        self.forget_drag_fits();
        self.hidden.retain(|(hidden, ..)| *hidden != column);
        self.hidden.push((column, width, dpi));
    }

    /// The width of `column`, shown on a monitor of `dpi`: the width it
    /// had when hidden, as TableView's columns do, or its first width.
    pub(crate) fn column_shown(&mut self, column: Column, dpi: Dpi) -> Px {
        self.forget_drag_fits();
        (self.hidden.iter())
            .find(|(hidden, ..)| *hidden == column)
            .map_or_else(
                || column_width(column, dpi),
                |(_, width, then)| width.rescale(*then, dpi),
            )
    }

    fn fill(&mut self, width: Px, measure: impl FnOnce() -> Vec<ColumnFit>) -> Vec<Px> {
        // A drag that follows measures these rows, not the earlier ones.
        self.forget_drag_fits();
        self.filled_width = Some(width);
        fill_widths(&measure(), width)
    }

    fn forget_drag_fits(&mut self) {
        if let Some(drag) = &mut self.drag {
            drag.fits = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn dpi(dpi: u32) -> Dpi {
        Dpi::new(dpi).expect("not zero")
    }

    fn fit(widest: i32, average: i32) -> ColumnFit {
        ColumnFit {
            widest: Px(widest),
            average: Px(average),
            narrowest: Px(40),
        }
    }

    /// A column whose cells are never cut, such as Size.
    fn whole(widest: i32, average: i32) -> ColumnFit {
        ColumnFit {
            narrowest: Px(widest),
            ..fit(widest, average)
        }
    }

    /// A measure of two columns with content that counts its calls,
    /// because each call measures every shown cell's text.
    fn measure(calls: &Cell<u32>) -> impl Fn() -> Vec<ColumnFit> + '_ {
        || {
            calls.set(calls.get() + 1);
            vec![fit(100, 100), fit(300, 300)]
        }
    }

    /// Name, Path with one long path, Size and Date modified in a list too
    /// narrow for them. Shared by the widest cell, Path would take most of
    /// the room and cut Size and Date; shared by the average, Size stays
    /// whole, Date stops at its widest, and Name and Path share the rest.
    #[test]
    fn one_long_cell_does_not_take_the_room_of_the_others() {
        let fits = [fit(300, 200), fit(1200, 500), whole(70, 60), fit(120, 120)];
        assert_eq!(
            fill_widths(&fits, Px(1000)),
            [Px(232), Px(578), Px(70), Px(120)]
        );
    }

    /// When even the narrowest widths do not fit, every column is at its
    /// narrowest and the columns run past the edge.
    #[test]
    fn columns_below_their_narrowest_run_past_the_edge() {
        let fits = [fit(300, 200), fit(1200, 500), whole(70, 60)];
        assert_eq!(fill_widths(&fits, Px(100)), [Px(40), Px(40), Px(70)]);
    }

    /// Each fill measures every shown cell and sets every width, so a
    /// layout that keeps the width, such as a pane's text changing, fills
    /// nothing.
    #[test]
    fn a_layout_at_the_width_filled_last_fills_nothing() {
        let calls = Cell::new(0);
        let mut sizing = ColumnSizing::default();
        assert_eq!(
            sizing.resized(Px(800), measure(&calls)),
            Some(vec![Px(200), Px(600)])
        );
        assert_eq!(sizing.resized(Px(800), measure(&calls)), None);
        assert_eq!(calls.get(), 1);
    }

    /// A drag measures once, so the widths follow the pointer, and once
    /// more at its end, on the rows the list shows then; a drag that only
    /// moved the window fills nothing.
    #[test]
    fn a_drag_measures_once_and_again_at_its_end() {
        let calls = Cell::new(0);
        let mut sizing = ColumnSizing::default();
        sizing.drag_started();
        assert_eq!(sizing.drag_ended(Px(800), measure(&calls)), None);
        sizing.drag_started();
        sizing.resized(Px(800), measure(&calls));
        assert_eq!(
            sizing.resized(Px(400), measure(&calls)),
            Some(vec![Px(100), Px(300)])
        );
        assert_eq!(calls.get(), 1);
        assert!(sizing.drag_ended(Px(400), measure(&calls)).is_some());
        assert_eq!(calls.get(), 2);
        assert!(!sizing.is_dragging());
    }

    /// The widths are in the old DPI's pixels, so the same width in pixels
    /// on the new monitor fills again.
    #[test]
    fn a_dpi_change_fills_the_same_width_again() {
        let calls = Cell::new(0);
        let mut sizing = ColumnSizing::default();
        sizing.resized(Px(800), measure(&calls));
        sizing.dpi_changed();
        assert!(sizing.resized(Px(800), measure(&calls)).is_some());
    }

    /// The first rows fill the columns once, and a drag that follows
    /// measures them rather than scaling fits of the first widths.
    #[test]
    fn the_first_rows_fill_once_and_a_drag_measures_them() {
        let calls = Cell::new(0);
        let mut sizing = ColumnSizing::default();
        sizing.drag_started();
        sizing.resized(Px(800), measure(&calls));
        assert!(sizing.rows_arrived(Px(800), measure(&calls)).is_some());
        assert_eq!(sizing.rows_arrived(Px(800), measure(&calls)), None);
        sizing.resized(Px(700), measure(&calls));
        assert_eq!(calls.get(), 3);
    }

    /// A column shown again gets the width it had, at the DPI of now.
    #[test]
    fn a_column_shown_again_gets_its_width_back() {
        let mut sizing = ColumnSizing::default();
        sizing.column_hidden(Column::Size, Px(90), dpi(96));
        assert_eq!(sizing.column_shown(Column::Size, dpi(192)), Px(180));
        assert_eq!(
            sizing.column_shown(Column::Path, dpi(96)),
            column_width(Column::Path, dpi(96))
        );
    }
}
