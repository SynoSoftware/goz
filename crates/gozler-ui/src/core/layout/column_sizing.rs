//! Column widths: how a column fits its header and cells, how the fits
//! fill the list's width, and when a fill of Search's list happens
//! (DESIGN.md: Results list, A panel). The rules are TableView's, so a
//! column's width comes only from what it shows. The window measures the
//! texts and sets the widths that [`ColumnSizing`] and [`fill_widths`]
//! answer with.

use super::{Dip, Dpi, Px, cell_text_inset};
use crate::core::model::Column;

/// The space a header leaves on each side of its text.
const HEADER_TEXT_MARGIN: Dip = Dip(6);

/// What a column's header and the cells a fit measured ask of a fill
/// (DESIGN.md: Results list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ColumnFit {
    /// The width that shows the header and every measured cell whole.
    pub(crate) widest: Px,
    /// No measured cell shows anything, so `widest` is the header's.
    pub(crate) empty: bool,
    /// Whether a fill may make the column narrower than `widest`.
    pub(crate) shrinks: bool,
    /// Whether a fill may make the column wider than `widest`.
    pub(crate) grows: bool,
}

impl ColumnFit {
    /// The fit of `column`, whose header text is `header` wide and whose
    /// measured cells that show anything have texts `texts` wide, in the
    /// list's first column when `first`, with each text starting `label`
    /// after the column's left edge.
    pub(crate) fn of(
        column: Column,
        header: Px,
        texts: impl IntoIterator<Item = Px>,
        label: Px,
        first: bool,
        dpi: Dpi,
    ) -> ColumnFit {
        let margin = HEADER_TEXT_MARGIN.to_px(dpi);
        let mut widest = header + margin + margin;
        let mut empty = true;
        for text in texts {
            widest = widest.max(cell_width(text, label, first, dpi));
            empty = false;
        }
        let row = column.row();
        ColumnFit {
            widest,
            empty,
            shrinks: row.can_shrink,
            grows: row.can_grow,
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
/// TableView's fill does (DESIGN.md: Results list). Spare width goes to
/// the columns with content, because an empty column has nothing to show
/// in more. A shortfall comes from the empty columns first, because a cut
/// header hides less than a cut value. When no column is empty, or every
/// column is, they all take part at once. A column that cannot grow keeps
/// its fit, so when no column that takes part can grow, the columns end
/// before the edge; when even the columns that cannot shrink do not fit,
/// they run past it.
pub(crate) fn fill_widths(fits: &[ColumnFit], width: Px) -> Vec<Px> {
    let mut widths: Vec<f64> = fits.iter().map(|fit| f64::from(fit.widest.0)).collect();
    let mut change = f64::from(width.0) - widths.iter().sum::<f64>();
    let all: Vec<usize> = (0..fits.len()).collect();
    let empty: Vec<usize> = all.iter().copied().filter(|&i| fits[i].empty).collect();
    let content: Vec<usize> = all.iter().copied().filter(|&i| !fits[i].empty).collect();
    let steps = if empty.is_empty() || content.is_empty() {
        vec![all]
    } else if change > 0.0 {
        vec![content]
    } else {
        vec![empty, content]
    };
    for step in steps {
        change = scale(&mut widths, fits, &step, change);
    }
    whole_pixels(&widths)
}

/// Widens Extension for the rows now visible. Name and Path give up the
/// needed room in their current proportions; the other columns stay put.
pub(crate) fn grow_extension(columns: &[Column], widths: &[Px], needed: Px) -> Option<Vec<Px>> {
    let extension = columns
        .iter()
        .position(|&column| column == Column::Extension)?;
    if needed <= widths[extension] {
        return None;
    }
    let fits: Vec<_> = columns
        .iter()
        .zip(widths)
        .map(|(&column, &width)| ColumnFit {
            widest: if column == Column::Extension {
                needed
            } else {
                width
            },
            empty: false,
            shrinks: matches!(column, Column::Name | Column::Path),
            grows: false,
        })
        .collect();
    Some(fill_widths(
        &fits,
        widths.iter().copied().fold(Px(0), |sum, width| sum + width),
    ))
}

/// Changes the widths of the columns at `step` by `change` in all, by one
/// factor, so the widest columns keep the most room and a resize makes no
/// column jump. A column that the factor would take below its fit when it
/// cannot shrink, below nothing, or past its fit when it cannot grow,
/// stays at that bound, and the others share what is left. Answers the
/// part of `change` that the columns could not take.
fn scale(widths: &mut [f64], fits: &[ColumnFit], step: &[usize], change: f64) -> f64 {
    let bounds = |i: usize| {
        let fit = f64::from(fits[i].widest.0);
        let least = if fits[i].shrinks { 0.0 } else { fit };
        let most = if fits[i].grows { f64::INFINITY } else { fit };
        (least, most)
    };
    let mut free: Vec<usize> = step.to_vec();
    let mut left = change;
    while !free.is_empty() && left != 0.0 {
        let weight: f64 = free.iter().map(|&i| widths[i]).sum();
        if weight <= 0.0 {
            break;
        }
        let factor = 1.0 + left / weight;
        let held: Vec<usize> = (free.iter().copied())
            .filter(|&i| {
                let (least, most) = bounds(i);
                !(least..=most).contains(&(widths[i] * factor))
            })
            .collect();
        if held.is_empty() {
            for &i in &free {
                widths[i] *= factor;
            }
            return 0.0;
        }
        for &i in &held {
            let (least, most) = bounds(i);
            let bound = (widths[i] * factor).clamp(least, most);
            left -= bound - widths[i];
            widths[i] = bound;
        }
        free.retain(|i| !held.contains(i));
    }
    left
}

/// `widths` in whole pixels that add up to their rounded sum, so the
/// columns end exactly at the edge whenever they reach it: each rounded
/// down, and the pixels that leaves going one each to the columns that
/// lost the most.
fn whole_pixels(widths: &[f64]) -> Vec<Px> {
    // Rounded down, so the sum is no wider than the widths it rounds.
    let mut pixels: Vec<Px> = widths.iter().map(|&w| Px(w as i32)).collect();
    let total = widths.iter().sum::<f64>().round() as i32;
    let left = total - pixels.iter().map(|px| px.0).sum::<i32>();
    let mut by_loss: Vec<usize> = (0..widths.len()).collect();
    by_loss.sort_by(|&a, &b| {
        (widths[b] - widths[b].floor()).total_cmp(&(widths[a] - widths[a].floor()))
    });
    for &i in by_loss.iter().take(usize::try_from(left).unwrap_or(0)) {
        pixels[i].0 += 1;
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
    /// they fit their headers.
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
    /// row, when the columns fit only their headers.
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
    /// had when hidden, as TableView's columns do, or `None` for a column
    /// not hidden before, which then fits its header and rows.
    pub(crate) fn column_shown(&mut self, column: Column, dpi: Dpi) -> Option<Px> {
        self.forget_drag_fits();
        (self.hidden.iter())
            .find(|(hidden, ..)| *hidden == column)
            .map(|(_, width, then)| width.rescale(*then, dpi))
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

    fn fit(widest: i32) -> ColumnFit {
        ColumnFit {
            widest: Px(widest),
            empty: false,
            shrinks: true,
            grows: true,
        }
    }

    #[test]
    fn newly_visible_extensions_grow_without_refitting_other_values() {
        let columns = [
            Column::Name,
            Column::Extension,
            Column::Size,
            Column::DateModified,
        ];
        let widths = [Px(300), Px(25), Px(70), Px(150)];
        let grown = grow_extension(&columns, &widths, Px(65)).unwrap();
        assert_eq!(grown, [Px(260), Px(65), Px(70), Px(150)]);
        assert_eq!(grow_extension(&columns, &grown, Px(25)), None);
    }

    /// A column that a fill neither shrinks nor grows, such as Size.
    fn whole(widest: i32) -> ColumnFit {
        ColumnFit {
            shrinks: false,
            grows: false,
            ..fit(widest)
        }
    }

    /// A column whose measured rows show nothing, so its fit is its header.
    fn empty(header: i32) -> ColumnFit {
        ColumnFit {
            empty: true,
            ..fit(header)
        }
    }

    /// A measure of two columns with content that counts its calls,
    /// because each call measures every shown cell's text.
    fn measure(calls: &Cell<u32>) -> impl Fn() -> Vec<ColumnFit> + '_ {
        || {
            calls.set(calls.get() + 1);
            vec![fit(100), fit(300)]
        }
    }

    /// In a list too narrow for the fits, the columns with content shrink
    /// by one factor, and Size, which cannot shrink, keeps its fit.
    #[test]
    fn a_shortfall_scales_the_columns_that_can_shrink() {
        let fits = [fit(300), fit(900), whole(70)];
        assert_eq!(fill_widths(&fits, Px(670)), [Px(150), Px(450), Px(70)]);
    }

    /// Spare width goes to the columns with content that can grow, by one
    /// factor; an empty column and Size keep their fits.
    #[test]
    fn spare_width_goes_to_the_columns_with_content() {
        let fits = [fit(200), empty(60), whole(70), fit(100)];
        assert_eq!(
            fill_widths(&fits, Px(730)),
            [Px(400), Px(60), Px(70), Px(200)]
        );
    }

    /// A shortfall comes from the empty columns first.
    #[test]
    fn a_shortfall_comes_from_the_empty_columns_first() {
        let fits = [fit(300), empty(100), fit(100)];
        assert_eq!(fill_widths(&fits, Px(450)), [Px(300), Px(50), Px(100)]);
    }

    /// When even the columns that cannot shrink do not fit, the columns
    /// run past the edge.
    #[test]
    fn columns_that_cannot_shrink_run_past_the_edge() {
        let fits = [fit(300), whole(70), whole(120)];
        assert_eq!(fill_widths(&fits, Px(100)), [Px(0), Px(70), Px(120)]);
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
    /// measures them rather than scaling fits of the headers alone.
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
        assert_eq!(sizing.column_shown(Column::Size, dpi(192)), Some(Px(180)));
        assert_eq!(sizing.column_shown(Column::Path, dpi(96)), None);
    }
}
