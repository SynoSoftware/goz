//! Where every control of the main window goes (DESIGN.md: window rule 7).
//! Sizes are designed in [`Dip`] and become [`Px`] only here, so a size can
//! never mix the two (rule 6). The spacing follows Microsoft's Windows desktop
//! layout guidelines at 96 DPI: 11 px margins and gaps between groups, 7 px
//! between related controls, 23 px tall text boxes and lists, 17 px check
//! boxes.

use super::captions::WindowCaptions;
use super::model::Column;
use super::query::Kind;
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

    /// A size on a monitor of `dpi`, as it is saved: in DIP, so a column or
    /// the inspector keeps its size on a monitor with another DPI.
    pub(crate) fn to_dip(self, dpi: Dpi) -> Dip {
        Dip(self.rescale(dpi, Dpi::new(96).expect("96 is not zero")).0)
    }
}

/// The width of `column` on a monitor of `dpi`: its `saved` width, or else
/// its default width.
pub(crate) fn column_width(column: Column, saved: Option<Dip>, dpi: Dpi) -> Px {
    saved.unwrap_or(column.default_width()).to_px(dpi)
}

/// Where the list draws a cell's text after the left edge of the cell's
/// label, which a cell the window draws itself keeps, so the two line up.
/// Measured against the list's own drawing at 96 DPI.
pub(crate) fn cell_text_inset(first_column: bool, dpi: Dpi) -> Px {
    let inset = if first_column { Dip(2) } else { Dip(6) };
    inset.to_px(dpi)
}

/// Where a list box draws an item's text after the item's left edge,
/// which a list box the window draws itself keeps, so the filter pane's
/// lists line up. Measured against a list box's own drawing at 96 DPI.
pub(crate) fn list_box_text_inset(dpi: Dpi) -> Px {
    Dip(2).to_px(dpi)
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

    /// Whether `point`, in the rectangle's coordinates, is inside it.
    pub(crate) fn contains(self, (x, y): (i32, i32)) -> bool {
        (self.x.0..self.right().0).contains(&x) && (self.y.0..self.bottom().0).contains(&y)
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
/// The filter pane's narrowest width; longer captions widen it.
const FILTER_PANE_WIDTH: Dip = Dip(180);
/// Between a label and the control below it.
const LABEL_GAP: Dip = Dip(4);
/// A list box's borders and the inset of its text.
const LIST_BOX_CHROME: Dip = Dip(6);
/// The rows the Location list shows; more scroll in the list.
const LOCATION_ROWS: usize = 5;
/// A vertical scroll bar, as Windows draws it at 100 %.
const SCROLL_BAR: Dip = Dip(17);
/// The lines one notch of the mouse wheel scrolls, as Windows' default.
const WHEEL_LINES: i32 = 3;
/// One notch of the mouse wheel, `WHEEL_DELTA`.
const WHEEL_DELTA: i32 = 120;
const LIST_MIN_WIDTH: Dip = Dip(300);
/// A list row is this much taller than its font, as File Explorer's.
const ROW_PADDING: Dip = Dip(7);
/// The header and three rows stay visible at the minimum size.
const LIST_MIN_ROWS: i32 = 4;
const INSPECTOR_MIN_HEIGHT: Dip = Dip(90);
/// The inspector's icon, the shell's large icon at 100 %.
const ICON: Dip = Dip(32);
/// The inspector's heading keeps room for a short name when the window is
/// narrow; a longer name ends in an ellipsis.
const HEADING_MIN_WIDTH: Dip = Dip(100);
/// A read-only value is this much taller than its font, the room a
/// borderless edit control gives its text.
const VALUE_PADDING: Dip = Dip(2);
/// A status bar part's borders and text inset.
const STATUS_PART_PADDING: Dip = Dip(13);
/// Part 4 takes the rest, at least this much.
const STATUS_REST_MIN_WIDTH: Dip = Dip(100);

/// The largest result count the status bar is sized for; a larger one ends
/// in an ellipsis instead of moving the parts (DESIGN.md: Look).
pub(crate) const WIDEST_TOTAL: u64 = 9_999_999;

/// Which panes the window shows, and the inspector's height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Panes {
    pub(crate) filters: bool,
    pub(crate) inspector: bool,
    pub(crate) inspector_height: Dip,
}

impl Default for Panes {
    /// The first height fits the seven fields of one entry (DESIGN.md:
    /// Inspector pane).
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
    filters: FilterWidths,
    /// The inspector's Open, Open file location and Copy as path.
    inspector_buttons: [Px; 3],
    /// The widest label of an inspector field.
    inspector_labels: Px,
    /// The widest text of each fixed status bar part.
    status: [Px; 3],
}

/// The measured widths of the filter pane's captions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FilterWidths {
    clear: Px,
    choose_folder: Px,
    only_directly: Px,
    /// The widest group label.
    labels: Px,
    /// The widest Kind item.
    kinds: Px,
    /// The widest Date modified or Size item.
    ranges: Px,
    attribute_names: Px,
    attribute_choices: Px,
}

impl CaptionWidths {
    /// Measures `captions` with `measure`, which gives the width of a text
    /// as its control draws it, an access-key `&` included.
    pub(crate) fn measure(
        captions: &WindowCaptions,
        measure: &dyn Fn(&str) -> Px,
    ) -> CaptionWidths {
        let widest = |texts: &[String]| texts.iter().map(|t| measure(t)).max().unwrap_or_default();
        let filters = &captions.filters;
        CaptionWidths {
            match_label: measure(&captions.match_label),
            match_modes: widest(&captions.match_modes),
            match_case: measure(&captions.match_case),
            match_path: measure(&captions.match_path),
            filters: FilterWidths {
                clear: measure(&filters.clear),
                choose_folder: measure(&filters.choose_folder),
                only_directly: measure(&filters.only_directly),
                labels: widest(&[
                    filters.kind.clone(),
                    filters.location.clone(),
                    filters.date.clone(),
                    filters.size.clone(),
                    filters.attributes.clone(),
                ]),
                kinds: widest(&filters.kinds),
                ranges: widest(&filters.ranges_widest),
                attribute_names: widest(&filters.attribute_names),
                attribute_choices: widest(&filters.attribute_choices),
            },
            inspector_buttons: captions.inspector_buttons.each_ref().map(|b| measure(b)),
            inspector_labels: widest(&captions.inspector_labels),
            status: captions.status_widest.each_ref().map(|texts| widest(texts)),
        }
    }
}

/// A push button is this much wider than its caption.
const BUTTON_PADDING: Dip = Dip(24);
/// The narrowest push button, as Windows' guidelines size OK and Cancel.
const BUTTON_MIN_WIDTH: Dip = Dip(75);
/// The administrator shield and its gap, which a button that runs as
/// administrator draws before its caption.
const SHIELD_WIDTH: Dip = Dip(20);

/// The room a button needs for a caption `caption` wide, with the
/// administrator shield before it when `shielded`.
pub(crate) fn button_content(caption: Px, shielded: bool, dpi: Dpi) -> Px {
    if shielded {
        caption + SHIELD_WIDTH.to_px(dpi)
    } else {
        caption
    }
}

/// The width of a push button for a caption `caption` wide.
fn button_width(caption: Px, dpi: Dpi) -> Px {
    (caption + BUTTON_PADDING.to_px(dpi)).max(BUTTON_MIN_WIDTH.to_px(dpi))
}

/// Where the list's message and its buttons go, inside the list below its
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
    /// buttons' in a row below it, for captions `buttons` wide.
    pub(crate) fn place(
        &self,
        text_height: Px,
        font_height: Px,
        buttons: &[Px],
    ) -> (Rect, Vec<Rect>) {
        let px = |dip: Dip| dip.to_px(self.dpi);
        let text = Rect {
            height: text_height.min(self.text.height),
            ..self.text
        };
        let mut x = text.x;
        let buttons = buttons
            .iter()
            .map(|caption| {
                let button = Rect {
                    x,
                    y: text.bottom() + px(GROUP),
                    width: button_width(*caption, self.dpi),
                    height: font_height + px(CONTROL_PADDING),
                };
                x = button.right() + px(RELATED);
                button
            })
            .collect();
        (text, buttons)
    }
}

/// How the person scrolls the filter pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollRequest {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
    /// The scroll box dragged to this position.
    To(Px),
    /// The mouse wheel turned by this `WM_MOUSEWHEEL` delta, positive away
    /// from the person.
    Wheel(i32),
}

/// The filter pane's window, and its controls in the pane's coordinates
/// before it scrolls. When the pane is shorter than its controls, it shows
/// a scroll bar and the part of them from its scroll position down (DESIGN.md:
/// Filter pane).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FilterLayout {
    /// The pane's window in the client area.
    pub(crate) pane: Rect,
    /// The height of the pane's controls, its scroll range.
    pub(crate) content_height: Px,
    pub(crate) clear: Rect,
    pub(crate) kind_label: Rect,
    pub(crate) kind: Rect,
    pub(crate) location_label: Rect,
    pub(crate) location: Rect,
    pub(crate) choose_folder: Rect,
    pub(crate) only_directly: Rect,
    pub(crate) date_label: Rect,
    pub(crate) date: Rect,
    pub(crate) size_label: Rect,
    pub(crate) size: Rect,
    pub(crate) attributes_label: Rect,
    /// Each attribute's label and list, in the order of
    /// [`super::query::ATTRIBUTES`].
    pub(crate) attributes: [(Rect, Rect); 6],
    /// What one line of scrolling moves.
    line: Px,
}

impl FilterLayout {
    /// The pane's controls, in a pane window `pane`; when they are taller
    /// than the pane, its scroll bar takes room from their width.
    fn compute(pane: Rect, dpi: Dpi, font_height: Px, widths: &FilterWidths) -> FilterLayout {
        let full = FilterLayout::place(pane, pane.width, dpi, font_height, widths);
        if full.content_height > pane.height {
            let width = pane.width - SCROLL_BAR.to_px(dpi);
            FilterLayout::place(pane, width, dpi, font_height, widths)
        } else {
            full
        }
    }

    /// The controls `width` wide, from the top down.
    fn place(
        pane: Rect,
        width: Px,
        dpi: Dpi,
        font_height: Px,
        widths: &FilterWidths,
    ) -> FilterLayout {
        let px = |dip: Dip| dip.to_px(dpi);
        let control_height = font_height + px(CONTROL_PADDING);
        let check_height = font_height + px(CHECK_BOX_PADDING);
        let list_height = |rows: usize| Px(font_height.0 * rows as i32) + px(LIST_BOX_CHROME);
        let mut y = Px(0);
        let mut next = |height: Px, gap: Dip| {
            let rect = Rect {
                x: Px(0),
                y,
                width,
                height,
            };
            y = rect.bottom() + px(gap);
            rect
        };
        let clear = Rect {
            width: button_width(widths.clear, dpi),
            ..next(control_height, GROUP)
        };
        let label = |rect: Rect| Rect {
            width: widths.labels.min(width),
            ..rect
        };
        let kind_label = label(next(font_height, LABEL_GAP));
        let kind = next(list_height(Kind::ALL.len()), GROUP);
        let location_label = label(next(font_height, LABEL_GAP));
        let location = next(list_height(LOCATION_ROWS), RELATED);
        let choose_folder = Rect {
            width: button_width(widths.choose_folder, dpi),
            ..next(control_height, RELATED)
        };
        let only_directly = next(check_height, GROUP);
        let date_label = label(next(font_height, LABEL_GAP));
        let date = next(control_height, GROUP);
        let size_label = label(next(font_height, LABEL_GAP));
        let size = next(control_height, GROUP);
        let attributes_label = label(next(font_height, LABEL_GAP));
        let attributes = [(); 6].map(|()| {
            let row = next(control_height, RELATED);
            let names = widths.attribute_names;
            let name = Rect {
                y: row.y + Px((control_height.0 - font_height.0) / 2),
                width: names,
                height: font_height,
                ..row
            };
            let choice_x = names + px(RELATED);
            let choice = Rect {
                x: choice_x,
                width: width - choice_x,
                ..row
            };
            (name, choice)
        });
        FilterLayout {
            pane,
            content_height: attributes[5].1.bottom(),
            clear,
            kind_label,
            kind,
            location_label,
            location,
            choose_folder,
            only_directly,
            date_label,
            date,
            size_label,
            size,
            attributes_label,
            attributes,
            line: font_height,
        }
    }

    /// The narrowest pane that shows every caption whole, with room for
    /// its scroll bar.
    fn min_width(dpi: Dpi, widths: &FilterWidths) -> Px {
        let px = |dip: Dip| dip.to_px(dpi);
        let content = [
            button_width(widths.clear, dpi),
            button_width(widths.choose_folder, dpi),
            widths.only_directly + px(CHECK_BOX_CHROME),
            widths.labels,
            widths.kinds + px(LIST_BOX_CHROME),
            widths.ranges + px(DROP_DOWN_CHROME),
            widths.attribute_names + px(RELATED) + widths.attribute_choices + px(DROP_DOWN_CHROME),
        ]
        .into_iter()
        .max()
        .unwrap_or_default();
        (content + px(SCROLL_BAR)).max(px(FILTER_PANE_WIDTH))
    }

    /// The largest scroll position, at which the last control shows.
    pub(crate) fn max_position(&self) -> Px {
        (self.content_height - self.pane.height).max(Px(0))
    }

    /// The scroll position after `request` from `position`.
    pub(crate) fn scrolled(&self, position: Px, request: ScrollRequest) -> Px {
        let page = self.pane.height.max(self.line);
        let to = match request {
            ScrollRequest::LineUp => position - self.line,
            ScrollRequest::LineDown => position + self.line,
            ScrollRequest::PageUp => position - page,
            ScrollRequest::PageDown => position + page,
            ScrollRequest::Top => Px(0),
            ScrollRequest::Bottom => self.max_position(),
            ScrollRequest::To(to) => to,
            ScrollRequest::Wheel(delta) => {
                position - Px(self.line.0 * WHEEL_LINES * delta / WHEEL_DELTA)
            }
        };
        to.clamp(Px(0), self.max_position())
    }

    /// The scroll position nearest `position` that shows `control`, a
    /// rectangle in the pane's coordinates, such as the control the focus
    /// moved to.
    pub(crate) fn revealing(&self, position: Px, control: Rect) -> Px {
        let to = if control.y < position {
            control.y
        } else if control.bottom() > position + self.pane.height {
            control.bottom() - self.pane.height
        } else {
            position
        };
        to.clamp(Px(0), self.max_position())
    }
}

/// The inspector's controls in the client area (DESIGN.md: Inspector pane). The
/// heading's row holds the icon, the heading and the buttons; the fields
/// follow, a label and a value in each row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InspectorLayout {
    /// The band between the list and the pane that the person drags.
    pub(crate) splitter: Rect,
    pub(crate) icon: Rect,
    pub(crate) heading: Rect,
    /// Open, Open file location and Copy as path.
    pub(crate) buttons: [Rect; 3],
    /// Each field row's label and value, as many as the pane's height
    /// shows.
    pub(crate) fields: Vec<(Rect, Rect)>,
    /// The hint under the heading, across the first field row, which is
    /// empty while the hint shows; `None` when the pane has no room for it.
    pub(crate) note: Option<Rect>,
    /// The pane's height, and the heights the splitter can give it.
    height: Px,
    min_height: Px,
    max_height: Px,
    dpi: Dpi,
}

impl InspectorLayout {
    /// The rows of one entry, the most that any selection shows.
    const FIELD_ROWS: usize = 7;

    /// The controls of a pane `area`, with the splitter above it.
    fn compute(
        area: Rect,
        max_height: Px,
        dpi: Dpi,
        font_height: Px,
        captions: &CaptionWidths,
    ) -> InspectorLayout {
        let px = |dip: Dip| dip.to_px(dpi);
        let related = px(RELATED);
        let row = Rect {
            height: px(ICON).max(font_height + px(CONTROL_PADDING)),
            ..area
        };
        let icon = Rect {
            y: row.y + Px((row.height.0 - px(ICON).0) / 2),
            width: px(ICON),
            height: px(ICON),
            ..row
        };
        // The buttons from the right edge, so they keep their place.
        let mut right = area.right();
        let mut buttons = [Rect::default(); 3];
        for (button, caption) in buttons.iter_mut().zip(captions.inspector_buttons).rev() {
            let height = font_height + px(CONTROL_PADDING);
            let width = button_width(caption, dpi);
            *button = Rect {
                x: right - width,
                y: row.y + Px((row.height.0 - height.0) / 2),
                width,
                height,
            };
            right = button.x - related;
        }
        let heading_x = icon.right() + related;
        let heading = Rect {
            x: heading_x,
            width: (right - heading_x).max(px(HEADING_MIN_WIDTH)),
            ..row
        };
        let value_height = font_height + px(VALUE_PADDING);
        let value_x = area.x + captions.inspector_labels + px(GROUP);
        let mut fields = Vec::new();
        let mut y = row.bottom() + related;
        while fields.len() < Self::FIELD_ROWS && y + value_height <= area.bottom() {
            let label = Rect {
                x: area.x,
                y,
                width: captions.inspector_labels,
                height: value_height,
            };
            let value = Rect {
                x: value_x,
                width: (area.right() - value_x).max(Px(0)),
                ..label
            };
            fields.push((label, value));
            y = label.bottom() + px(LABEL_GAP);
        }
        let note = fields.first().map(|(label, _)| Rect {
            width: area.width,
            ..*label
        });
        InspectorLayout {
            splitter: Rect {
                y: area.y - px(GROUP),
                height: px(GROUP),
                ..area
            },
            icon,
            heading,
            buttons,
            fields,
            note,
            height: area.height,
            min_height: px(INSPECTOR_MIN_HEIGHT),
            max_height: max_height.max(px(INSPECTOR_MIN_HEIGHT)),
            dpi,
        }
    }

    /// The narrowest pane whose heading's row fits.
    fn min_width(dpi: Dpi, captions: &CaptionWidths) -> Px {
        let px = |dip: Dip| dip.to_px(dpi);
        captions
            .inspector_buttons
            .iter()
            .fold(px(ICON) + px(HEADING_MIN_WIDTH), |sum, caption| {
                sum + px(RELATED) + button_width(*caption, dpi)
            })
            + px(RELATED)
    }

    /// The pane's height after the person dragged the splitter `moved`
    /// down from where the drag started at `start`, the height then, kept
    /// between the pane's and the list's minimum heights.
    pub(crate) fn dragged(&self, start: Px, moved: Px) -> Dip {
        (start - moved)
            .clamp(self.min_height, self.max_height)
            .to_dip(self.dpi)
    }

    /// The pane's height, where a drag starts.
    pub(crate) fn height(&self) -> Px {
        self.height
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
    /// `None` while the pane is hidden.
    pub(crate) filters: Option<FilterLayout>,
    pub(crate) list: Rect,
    /// `None` while the pane is hidden.
    pub(crate) inspector: Option<InspectorLayout>,
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
        let filters = panes.filters.then(|| {
            let pane = Rect {
                x: margin,
                y: top,
                width: FilterLayout::min_width(dpi, &captions.filters),
                height: (bottom - top).max(Px(0)),
            };
            left = pane.right() + group;
            dividers.push(Rect {
                x: pane.right() + Px((group.0 - 1) / 2),
                width: Px(1),
                ..pane
            });
            FilterLayout::compute(pane, dpi, font_height, &captions.filters)
        });
        let list_min_height = Px((font_height + px(ROW_PADDING)).0 * LIST_MIN_ROWS);
        let inspector_height = panes.inspector.then(|| {
            let room = bottom - top - list_min_height - group;
            (
                px(panes.inspector_height)
                    .min(room)
                    .max(px(INSPECTOR_MIN_HEIGHT)),
                room,
            )
        });
        let list = Rect {
            x: left,
            y: top,
            width: (client.width - margin - left).max(px(LIST_MIN_WIDTH)),
            height: (bottom - top - inspector_height.map_or(Px(0), |(height, _)| height + group))
                .max(list_min_height),
        };
        let inspector = inspector_height.map(|(height, room)| {
            dividers.push(Rect {
                y: list.bottom() + Px((group.0 - 1) / 2),
                height: Px(1),
                ..list
            });
            let area = Rect {
                y: list.bottom() + group,
                height,
                ..list
            };
            InspectorLayout::compute(area, room, dpi, font_height, captions)
        });

        let status_parts = captions.status.map(|width| width + px(STATUS_PART_PADDING));
        let body_width =
            left + if panes.inspector {
                px(LIST_MIN_WIDTH).max(InspectorLayout::min_width(dpi, captions))
            } else {
                px(LIST_MIN_WIDTH)
            } + margin;
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
            filters,
            list,
            inspector,
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

    /// The filter pane's controls, in the pane's coordinates.
    fn pane_controls(filters: &FilterLayout) -> Vec<Rect> {
        let mut controls = vec![
            filters.clear,
            filters.kind_label,
            filters.kind,
            filters.location_label,
            filters.location,
            filters.choose_folder,
            filters.only_directly,
            filters.date_label,
            filters.date,
            filters.size_label,
            filters.size,
            filters.attributes_label,
        ];
        for (name, choice) in filters.attributes {
            controls.extend([name, choice]);
        }
        controls
    }

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
                        if let Some(inspector) = &layout.inspector {
                            controls.extend([inspector.icon, inspector.heading]);
                            controls.extend(inspector.buttons);
                            for (label, value) in &inspector.fields {
                                controls.extend([*label, *value]);
                            }
                            // The first height shows one entry's every field
                            // (DESIGN.md: Inspector pane).
                            assert!(
                                client == min
                                    || inspector.fields.len() == InspectorLayout::FIELD_ROWS,
                                "{} fields show at {dpi:?}",
                                inspector.fields.len()
                            );
                        }
                        if let Some(filters) = &layout.filters {
                            controls.push(filters.pane);
                            // The pane scrolls instead of setting the
                            // window's minimum height.
                            assert!(client != min || filters.max_position() > Px(0));
                            let bar = if filters.max_position() > Px(0) {
                                Dip(17).to_px(dpi)
                            } else {
                                Px(0)
                            };
                            let inside = pane_controls(filters);
                            for (i, a) in inside.iter().enumerate() {
                                assert!(
                                    a.x >= Px(0)
                                        && a.y >= Px(0)
                                        && a.right() <= filters.pane.width - bar
                                        && a.bottom() <= filters.content_height,
                                    "{a:?} leaves the pane {filters:?} at {dpi:?}, \
                                     stretch {stretch}"
                                );
                                for b in &inside[i + 1..] {
                                    assert!(
                                        !overlaps(*a, *b),
                                        "{a:?} overlaps {b:?} in the pane at {dpi:?}, \
                                         stretch {stretch}"
                                    );
                                }
                            }
                        }
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
