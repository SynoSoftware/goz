//! Where every control of the main window goes (design rule 7). Sizes are
//! designed in [`Dip`] and become [`Px`] only here, so a size can never mix
//! the two (rule 6). The spacing follows Microsoft's Windows desktop layout
//! guidelines at 96 DPI: 11 px margins and gaps between groups, 7 px between
//! related controls, 23 px tall text boxes and lists, 17 px check boxes.

use super::captions::WindowCaptions;
use super::model::Column;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::ops::{Add, Sub};

/// Device-independent pixels, as the layout is designed; 96 DPI pixels.
/// `ui.json` saves one as its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Dip(pub(crate) i32);

/// Physical pixels on the window's monitor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Px(pub(crate) i32);

/// The DPI of the monitor that shows the window; 96 is 100 %.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dpi(NonZeroU32);

impl Dpi {
    pub(crate) fn new(dpi: u32) -> Option<Dpi> {
        NonZeroU32::new(dpi).map(Dpi)
    }

    pub(crate) fn get(self) -> u32 {
        self.0.get()
    }
}

impl Dip {
    /// Rounded to the nearest pixel, as `MulDiv` rounds.
    fn to_px(self, dpi: Dpi) -> Px {
        let scaled = (i64::from(self.0) * i64::from(dpi.get()) + 48).div_euclid(96);
        Px(i32::try_from(scaled).expect("a layout size fits in i32 at any DPI"))
    }
}

impl Px {
    /// `self` at `to` instead of `from`, as a column width scales when the
    /// window moves to a monitor with another DPI.
    pub(crate) fn rescale(self, from: Dpi, to: Dpi) -> Px {
        let scaled = (i64::from(self.0) * i64::from(to.get()) + i64::from(from.get()) / 2)
            .div_euclid(i64::from(from.get()));
        Px(i32::try_from(scaled).expect("a scaled size fits in i32"))
    }
}

/// The width of `column` on a monitor of `dpi`: its `saved` width, or else
/// its default width.
pub(crate) fn column_width(column: Column, saved: Option<Dip>, dpi: Dpi) -> Px {
    saved.unwrap_or(column.default_width()).to_px(dpi)
}

/// A column `width` wide on a monitor of `dpi`, as it is saved: in DIP, so
/// the column keeps its size on a monitor with another DPI.
pub(crate) fn saved_width(width: Px, dpi: Dpi) -> Dip {
    Dip(width.rescale(dpi, Dpi::new(96).expect("96 is not zero")).0)
}

/// Where the list draws a cell's text after the left edge of the cell's
/// label, which a cell the window draws itself keeps, so the two line up.
/// Measured against the list's own drawing at 96 DPI.
pub(crate) fn cell_text_inset(first_column: bool, dpi: Dpi) -> Px {
    let inset = if first_column { Dip(2) } else { Dip(6) };
    inset.to_px(dpi)
}

impl Add for Px {
    type Output = Px;
    fn add(self, other: Px) -> Px {
        Px(self.0 + other.0)
    }
}

impl Sub for Px {
    type Output = Px;
    fn sub(self, other: Px) -> Px {
        Px(self.0 - other.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Size {
    pub(crate) width: Px,
    pub(crate) height: Px,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: Px,
    pub(crate) y: Px,
    pub(crate) width: Px,
    pub(crate) height: Px,
}

impl Rect {
    pub(crate) fn right(self) -> Px {
        self.x + self.width
    }

    pub(crate) fn bottom(self) -> Px {
        self.y + self.height
    }
}

/// The window's edges and the gaps between groups.
const MARGIN: Dip = Dip(11);
/// Between related controls, such as the match controls.
const RELATED: Dip = Dip(7);
/// Between groups, such as the strip and the panes; a pane's one-pixel
/// divider sits in the middle of it.
const GROUP: Dip = Dip(11);
/// A text box or a drop-down list is this much taller than its font.
const CONTROL_PADDING: Dip = Dip(8);
/// A check box is this much taller than its font.
const CHECK_BOX_PADDING: Dip = Dip(2);
/// A check box's box and the gap before its caption.
const CHECK_BOX_CHROME: Dip = Dip(20);
/// A drop-down list's arrow button, borders and text inset.
const DROP_DOWN_CHROME: Dip = Dip(30);
/// The search box keeps room for a few words when the window is narrow.
const SEARCH_MIN_WIDTH: Dip = Dip(120);
const FILTER_PANE_WIDTH: Dip = Dip(180);
const LIST_MIN_WIDTH: Dip = Dip(300);
/// A list row is this much taller than its font, as File Explorer's.
const ROW_PADDING: Dip = Dip(7);
/// The header and three rows stay visible at the minimum size.
const LIST_MIN_ROWS: i32 = 4;
const INSPECTOR_MIN_HEIGHT: Dip = Dip(90);
/// A status bar part's borders and text inset.
const STATUS_PART_PADDING: Dip = Dip(13);
/// Part 4 takes the rest, at least this much.
const STATUS_REST_MIN_WIDTH: Dip = Dip(100);

/// The largest result count the status bar is sized for; a larger one ends
/// in an ellipsis instead of moving the parts (spec: Look).
pub(crate) const WIDEST_TOTAL: u64 = 9_999_999;

/// Which panes the window shows, and the inspector's height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Panes {
    pub(crate) filters: bool,
    pub(crate) inspector: bool,
    pub(crate) inspector_height: Dip,
}

impl Default for Panes {
    /// The first height fits the seven fields of one entry (spec: Inspector
    /// pane).
    fn default() -> Panes {
        Panes {
            filters: true,
            inspector: true,
            inspector_height: Dip(196),
        }
    }
}

/// The measured width of each caption that the window places in code, in
/// the current language, font and DPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptionWidths {
    match_label: Px,
    match_modes: Px,
    match_case: Px,
    match_path: Px,
    /// The widest text of each fixed status bar part.
    status: [Px; 3],
}

impl CaptionWidths {
    /// Measures `captions` with `measure`, which gives the width of a text
    /// as its control draws it, an access-key `&` included.
    pub(crate) fn measure(
        captions: &WindowCaptions,
        measure: &dyn Fn(&str) -> Px,
    ) -> CaptionWidths {
        let widest = |texts: &[String]| texts.iter().map(|t| measure(t)).max().unwrap_or_default();
        CaptionWidths {
            match_label: measure(&captions.match_label),
            match_modes: widest(&captions.match_modes),
            match_case: measure(&captions.match_case),
            match_path: measure(&captions.match_path),
            status: captions.status_widest.each_ref().map(|texts| widest(texts)),
        }
    }
}

/// A push button is this much wider than its caption.
const BUTTON_PADDING: Dip = Dip(24);
/// The narrowest push button, as Windows' guidelines size OK and Cancel.
const BUTTON_MIN_WIDTH: Dip = Dip(75);

/// Where the list's message and its button go, inside the list below its
/// header, so the message reads where the rows would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MessageArea {
    text: Rect,
    dpi: Dpi,
}

impl MessageArea {
    /// `header` is the height of the list's header.
    pub(crate) fn new(list: Rect, header: Px, dpi: Dpi) -> MessageArea {
        let margin = MARGIN.to_px(dpi);
        MessageArea {
            text: Rect {
                x: list.x + margin,
                y: list.y + header + margin,
                width: (list.width - margin - margin).max(Px(0)),
                height: (list.height - header - margin - margin).max(Px(0)),
            },
            dpi,
        }
    }

    /// The width the message wraps at.
    pub(crate) fn text_width(&self) -> Px {
        self.text.width
    }

    /// The message's rectangle for a text `text_height` tall, and its
    /// button's below it, for a caption `button` wide, when it has one.
    pub(crate) fn place(
        &self,
        text_height: Px,
        font_height: Px,
        button: Option<Px>,
    ) -> (Rect, Option<Rect>) {
        let px = |dip: Dip| dip.to_px(self.dpi);
        let text = Rect {
            height: text_height.min(self.text.height),
            ..self.text
        };
        let button = button.map(|caption| Rect {
            x: text.x,
            y: text.bottom() + px(GROUP),
            width: (caption + px(BUTTON_PADDING)).max(px(BUTTON_MIN_WIDTH)),
            height: font_height + px(CONTROL_PADDING),
        });
        (text, button)
    }
}

/// Every control's rectangle in the client area, the status bar's part
/// widths, and the smallest client size at which every control fits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) search: Rect,
    pub(crate) match_label: Rect,
    pub(crate) match_mode: Rect,
    pub(crate) match_case: Rect,
    pub(crate) match_path: Rect,
    pub(crate) list: Rect,
    /// The one-pixel lines between the panes.
    pub(crate) dividers: Vec<Rect>,
    /// The widths of the first three parts; the fourth takes the rest.
    pub(crate) status_parts: [Px; 3],
    pub(crate) min_client: Size,
}

impl Layout {
    /// `status_height` is the height the status bar gives itself for its
    /// font. When the client is smaller than `min_client`, the controls keep
    /// their minimum sizes and the window's minimum size stops it there.
    pub(crate) fn compute(
        client: Size,
        dpi: Dpi,
        font_height: Px,
        status_height: Px,
        captions: &CaptionWidths,
        panes: &Panes,
    ) -> Layout {
        let px = |dip: Dip| dip.to_px(dpi);
        let margin = px(MARGIN);
        let related = px(RELATED);
        let group = px(GROUP);
        let control_height = font_height + px(CONTROL_PADDING);
        let check_height = font_height + px(CHECK_BOX_PADDING);
        let centred = |height: Px| margin + Px((control_height.0 - height.0) / 2);

        // The strip, from the right edge: the match controls keep their
        // measured widths and the search box takes the rest.
        let mut right = client.width - margin;
        let mut place_left = |width: Px, y: Px, height: Px| {
            let rect = Rect {
                x: right - width,
                y,
                width,
                height,
            };
            right = rect.x - related;
            rect
        };
        let match_path = place_left(
            captions.match_path + px(CHECK_BOX_CHROME),
            centred(check_height),
            check_height,
        );
        let match_case = place_left(
            captions.match_case + px(CHECK_BOX_CHROME),
            centred(check_height),
            check_height,
        );
        let match_mode = place_left(
            captions.match_modes + px(DROP_DOWN_CHROME),
            margin,
            control_height,
        );
        let match_label = place_left(captions.match_label, centred(font_height), font_height);
        let search = Rect {
            x: margin,
            y: margin,
            width: (right - margin).max(px(SEARCH_MIN_WIDTH)),
            height: control_height,
        };
        let strip_width = margin
            + px(SEARCH_MIN_WIDTH)
            + [match_label, match_mode, match_case, match_path]
                .iter()
                .fold(Px(0), |sum, r| sum + related + r.width)
            + margin;

        // The body, between the strip and the status bar.
        let top = search.bottom() + group;
        let bottom = client.height - status_height - margin;
        let mut dividers = Vec::new();
        let mut left = margin;
        if panes.filters {
            left = left + px(FILTER_PANE_WIDTH) + group;
            dividers.push(Rect {
                x: left - group + Px((group.0 - 1) / 2),
                y: top,
                width: Px(1),
                height: (bottom - top).max(Px(0)),
            });
        }
        let list_min_height = Px((font_height + px(ROW_PADDING)).0 * LIST_MIN_ROWS);
        let mut list_bottom = bottom;
        if panes.inspector {
            let room = bottom - top - list_min_height - group;
            let inspector = px(panes.inspector_height)
                .min(room)
                .max(px(INSPECTOR_MIN_HEIGHT));
            list_bottom = bottom - inspector - group;
            dividers.push(Rect {
                x: left,
                y: list_bottom + Px((group.0 - 1) / 2),
                width: (client.width - margin - left).max(Px(0)),
                height: Px(1),
            });
        }
        let list = Rect {
            x: left,
            y: top,
            width: (client.width - margin - left).max(px(LIST_MIN_WIDTH)),
            height: (list_bottom - top).max(list_min_height),
        };

        let status_parts = captions.status.map(|width| width + px(STATUS_PART_PADDING));
        let body_width = left + px(LIST_MIN_WIDTH) + margin;
        let status_width = status_parts
            .iter()
            .fold(px(STATUS_REST_MIN_WIDTH), |sum, w| sum + *w);
        let inspector_min = if panes.inspector {
            group + px(INSPECTOR_MIN_HEIGHT)
        } else {
            Px(0)
        };
        let min_client = Size {
            width: strip_width.max(body_width).max(status_width),
            height: margin
                + control_height
                + group
                + list_min_height
                + inspector_min
                + margin
                + status_height,
        };
        Layout {
            search,
            match_label,
            match_mode,
            match_case,
            match_path,
            list,
            dividers,
            status_parts,
            min_client,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::tests::model;
    use crate::core::text::tests::English;

    /// Caption widths as a font of `char_width` pixels per character would
    /// give them; `stretch` stands for a longer language.
    fn captions(dpi: Dpi, stretch: f64) -> CaptionWidths {
        let char_width = f64::from(Dip(7).to_px(dpi).0);
        let captions = WindowCaptions::new(&model(), &English);
        CaptionWidths::measure(&captions, &|text: &str| {
            Px((text.chars().count() as f64 * char_width * stretch).ceil() as i32)
        })
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
    }

    /// A control that overlaps another or leaves the client area is cut off
    /// or hidden, and no compiler or other test sees it before a person
    /// does. English and the pseudo-language's longer captions, at 100, 150
    /// and 200 %, at the minimum size and at a large one.
    #[test]
    fn every_control_fits_without_overlap() {
        for dpi in [96, 144, 192].map(|d| Dpi::new(d).expect("a DPI is not zero")) {
            for stretch in [1.0, 2.0] {
                let captions = captions(dpi, stretch);
                let font_height = Dip(15).to_px(dpi);
                let status_height = Dip(22).to_px(dpi);
                for panes in [
                    Panes::default(),
                    Panes {
                        filters: false,
                        inspector: false,
                        ..Panes::default()
                    },
                ] {
                    let min = Layout::compute(
                        Size::default(),
                        dpi,
                        font_height,
                        status_height,
                        &captions,
                        &panes,
                    )
                    .min_client;
                    for client in [
                        min,
                        Size {
                            width: Px(min.width.0 * 2),
                            height: Px(min.height.0 * 2),
                        },
                    ] {
                        let layout = Layout::compute(
                            client,
                            dpi,
                            font_height,
                            status_height,
                            &captions,
                            &panes,
                        );
                        let mut controls = vec![
                            layout.search,
                            layout.match_label,
                            layout.match_mode,
                            layout.match_case,
                            layout.match_path,
                            layout.list,
                        ];
                        controls.extend(&layout.dividers);
                        for (i, a) in controls.iter().enumerate() {
                            assert!(
                                a.x >= Px(0)
                                    && a.y >= Px(0)
                                    && a.right() <= client.width
                                    && a.bottom() <= client.height - status_height,
                                "{a:?} leaves the client {client:?} at {dpi:?}, stretch {stretch}"
                            );
                            for b in &controls[i + 1..] {
                                assert!(
                                    !overlaps(*a, *b),
                                    "{a:?} overlaps {b:?} at {dpi:?}, stretch {stretch}"
                                );
                            }
                        }
                        let list_min = Px((font_height + Dip(7).to_px(dpi)).0 * 4);
                        assert!(
                            layout.list.height >= list_min
                                && layout.list.width >= Dip(300).to_px(dpi)
                        );
                        let parts: Px = layout.status_parts.iter().fold(Px(0), |s, w| s + *w);
                        assert!(
                            parts < client.width,
                            "the status bar's fixed parts leave part 4 room"
                        );
                    }
                }
            }
        }
    }
}
