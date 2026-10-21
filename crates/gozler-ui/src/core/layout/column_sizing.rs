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

/// The width a column needs for its header and the cells a fit measured
/// (DESIGN.md: Results list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ColumnFit {
    pub(crate) width: Px,
    /// No measured row shows anything in the column, so `width` is its
    /// header's.
    pub(crate) empty: bool,
}

impl ColumnFit {
    /// The fit of a column whose header text is `header` wide and whose
    /// measured cells that show anything have texts `texts` wide, in the
    /// column `first` when it is the list's first, with each text starting
    /// `label` after the column's left edge; never narrower than a fill
    /// makes a column.
    pub(crate) fn of(
        header: Px,
        texts: impl IntoIterator<Item = Px>,
        label: Px,
        first: bool,
        dpi: Dpi,
    ) -> ColumnFit {
        let widest = (texts.into_iter())
            .map(|text| cell_width(text, label, first, dpi))
            .max();
        let margin = HEADER_TEXT_MARGIN.to_px(dpi);
        let header = header + margin + margin;
        ColumnFit {
            width: widest
                .map_or(header, |cell| cell.max(header))
                .max(COLUMN_MIN_WIDTH.to_px(dpi)),
            empty: widest.is_none(),
        }
    }

    /// The fit of `column` before any row is read: its first width, as
    /// though every column had content, so the first fill keeps the
    /// columns' proportions until the rows arrive.
    pub(crate) fn first(column: Column, dpi: Dpi) -> ColumnFit {
        ColumnFit {
            width: column_width(column, dpi),
            empty: false,
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

/// The widths that make columns with these `fits` end at `width`, as
/// TableView fills (DESIGN.md: Results list). Spare width goes to the
/// columns with content, because an empty column has nothing to show in
/// more. A shortfall comes from the empty columns first, because cutting a
/// header hides less than cutting a value, and from the others once the
/// empty ones are at their narrowest. With no empty column, or only empty
/// ones, every column scales. When even the narrowest widths do not fit,
/// the columns run past the edge.
pub(crate) fn fill_widths(fits: &[ColumnFit], width: Px, dpi: Dpi) -> Vec<Px> {
    let min = COLUMN_MIN_WIDTH.to_px(dpi);
    let mut widths: Vec<Px> = fits.iter().map(|fit| fit.width).collect();
    let (empty, filled): (Vec<usize>, Vec<usize>) = (0..fits.len()).partition(|&i| fits[i].empty);
    if empty.is_empty() || filled.is_empty() {
        scale_to(&mut widths, (0..fits.len()).collect(), width, min);
        return widths;
    }
    let shortfall = total(&widths) - width.0;
    if shortfall <= 0 {
        scale_to(&mut widths, filled, width, min);
        return widths;
    }
    let slack: i32 = empty.iter().map(|&i| (widths[i] - min).0).sum();
    scale_to(&mut widths, empty, width, min);
    if shortfall > slack {
        scale_to(&mut widths, filled, width, min);
    }
    widths
}

fn total(widths: &[Px]) -> i32 {
    widths.iter().map(|width| width.0).sum()
}

/// Scales the columns `scaled` of `widths` by one factor so that all of
/// `widths` end at `width`, while the others keep theirs. One factor keeps
/// the fitted proportions, and taking only the missing width keeps a
/// resize from making a column jump. A column the factor would take below
/// `min` stays at `min`, and the others share what is left. Rounding down
/// leaves a few pixels, which go one each to the first scaled columns, so
/// the columns end exactly at `width`.
fn scale_to(widths: &mut [Px], mut scaled: Vec<usize>, width: Px, min: Px) {
    let share = |widths: &[Px], scaled: &[usize]| scaled.iter().map(|&i| widths[i].0).sum::<i32>();
    let mut room = width.0 - total(widths) + share(widths, &scaled);
    while !scaled.is_empty() {
        let factor = f64::from(room.max(0)) / f64::from(share(widths, &scaled).max(1));
        let scaled_width = |width: Px| f64::from(width.0) * factor;
        let floored: Vec<usize> = scaled
            .iter()
            .copied()
            .filter(|&i| scaled_width(widths[i]) < f64::from(min.0))
            .collect();
        if floored.is_empty() {
            let mut left = room.max(0);
            for &i in &scaled {
                // Rounded down, so it is no wider than `room`.
                widths[i] = Px(scaled_width(widths[i]) as i32);
                left -= widths[i].0;
            }
            for &i in scaled.iter().take(usize::try_from(left).unwrap_or(0)) {
                widths[i].0 += 1;
            }
            return;
        }
        for i in floored {
            widths[i] = min;
            room -= min.0;
            scaled.retain(|&s| s != i);
        }
    }
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
        dpi: Dpi,
        measure: impl FnOnce() -> Vec<ColumnFit>,
    ) -> Option<Vec<Px>> {
        if self.filled_width.replace(width) == Some(width) {
            return None;
        }
        Some(match &mut self.drag {
            None => fill_widths(&measure(), width, dpi),
            Some(drag) => fill_widths(drag.fits.get_or_insert_with(measure), width, dpi),
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
        dpi: Dpi,
        measure: impl FnOnce() -> Vec<ColumnFit>,
    ) -> Option<Vec<Px>> {
        self.drag.take()?.fits?;
        Some(self.fill(width, dpi, measure))
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
        dpi: Dpi,
        measure: impl FnOnce() -> Vec<ColumnFit>,
    ) -> Option<Vec<Px>> {
        if std::mem::replace(&mut self.filled_around_rows, true) {
            return None;
        }
        Some(self.fill(width, dpi, measure))
    }

    /// The window shows `width` wide: the widths that fit the rows it
    /// shows, because a hidden window shows at the width it had, which no
    /// resize fills.
    pub(crate) fn window_shown(
        &mut self,
        width: Px,
        dpi: Dpi,
        measure: impl FnOnce() -> Vec<ColumnFit>,
    ) -> Vec<Px> {
        self.fill(width, dpi, measure)
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

    fn fill(&mut self, width: Px, dpi: Dpi, measure: impl FnOnce() -> Vec<ColumnFit>) -> Vec<Px> {
        // A drag that follows measures these rows, not the earlier ones.
        self.forget_drag_fits();
        self.filled_width = Some(width);
        fill_widths(&measure(), width, dpi)
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

    fn fit(width: i32, empty: bool) -> ColumnFit {
        ColumnFit {
            width: Px(width),
            empty,
        }
    }

    /// A measure of two columns with content that counts its calls,
    /// because each call measures every shown cell's text.
    fn measure(calls: &Cell<u32>) -> impl Fn() -> Vec<ColumnFit> + '_ {
        || {
            calls.set(calls.get() + 1);
            vec![fit(100, false), fit(300, false)]
        }
    }

    /// A shortfall comes from the empty columns while they can give it, and
    /// the columns with content keep their widths.
    #[test]
    fn fill_takes_a_shortfall_from_empty_columns_first() {
        let fits = [
            fit(300, false),
            fit(400, false),
            fit(100, true),
            fit(120, true),
        ];
        assert_eq!(
            fill_widths(&fits, Px(850), dpi(96)),
            [Px(300), Px(400), Px(69), Px(81)]
        );
    }

    /// Each fill measures every shown cell and sets every width, so a
    /// layout that keeps the width, such as a pane's text changing, fills
    /// nothing.
    #[test]
    fn a_layout_at_the_width_filled_last_fills_nothing() {
        let calls = Cell::new(0);
        let mut sizing = ColumnSizing::default();
        assert_eq!(
            sizing.resized(Px(800), dpi(96), measure(&calls)),
            Some(vec![Px(200), Px(600)])
        );
        assert_eq!(sizing.resized(Px(800), dpi(96), measure(&calls)), None);
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
        assert_eq!(sizing.drag_ended(Px(800), dpi(96), measure(&calls)), None);
        sizing.drag_started();
        sizing.resized(Px(800), dpi(96), measure(&calls));
        assert_eq!(
            sizing.resized(Px(400), dpi(96), measure(&calls)),
            Some(vec![Px(100), Px(300)])
        );
        assert_eq!(calls.get(), 1);
        assert!(
            sizing
                .drag_ended(Px(400), dpi(96), measure(&calls))
                .is_some()
        );
        assert_eq!(calls.get(), 2);
        assert!(!sizing.is_dragging());
    }

    /// The widths are in the old DPI's pixels, so the same width in pixels
    /// on the new monitor fills again.
    #[test]
    fn a_dpi_change_fills_the_same_width_again() {
        let calls = Cell::new(0);
        let mut sizing = ColumnSizing::default();
        sizing.resized(Px(800), dpi(96), measure(&calls));
        sizing.dpi_changed();
        assert!(sizing.resized(Px(800), dpi(144), measure(&calls)).is_some());
    }

    /// The first rows fill the columns once, and a drag that follows
    /// measures them rather than scaling fits of the first widths.
    #[test]
    fn the_first_rows_fill_once_and_a_drag_measures_them() {
        let calls = Cell::new(0);
        let mut sizing = ColumnSizing::default();
        sizing.drag_started();
        sizing.resized(Px(800), dpi(96), measure(&calls));
        assert!(
            sizing
                .rows_arrived(Px(800), dpi(96), measure(&calls))
                .is_some()
        );
        assert_eq!(sizing.rows_arrived(Px(800), dpi(96), measure(&calls)), None);
        sizing.resized(Px(700), dpi(96), measure(&calls));
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
