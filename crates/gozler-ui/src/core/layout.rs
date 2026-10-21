//! Where every control of the main window goes (DESIGN.md: window rule 7).
//! Sizes are designed in [`Dip`] and become [`Px`] only here, so a size can
//! never mix the two (rule 6). The spacing follows Microsoft's Windows desktop
//! layout guidelines at 96 DPI: 11 px margins and gaps between groups, 7 px
//! between related controls, 23 px tall text boxes and lists, 17 px check
//! boxes.

use super::captions::{MODIFIED_ROW, MemoryWidest, StatusWidest, WindowCaptions};
use super::commands::INSPECTOR_BUTTONS;
use super::query::Kind;
use super::settings::Mode;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::ops::{Add, Sub};

pub(crate) mod column_sizing;

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

    /// A size on a monitor of `dpi`, as it is saved: in DIP, so the
    /// inspector keeps its size on a monitor with another DPI.
    pub(crate) fn to_dip(self, dpi: Dpi) -> Dip {
        Dip(self.rescale(dpi, Dpi::new(96).expect("96 is not zero")).0)
    }
}

/// Whether `a` and `b` share a row of pixels.
fn overlaps_rows(a: Rect, b: Rect) -> bool {
    a.y < b.bottom() && b.y < a.bottom()
}

/// The height of an icon glyph, such as the inspector's copy icon, on a
/// monitor of `dpi`: 16 px at 100 %.
pub(crate) fn icon_glyph_height(dpi: Dpi) -> Px {
    Dip(16).to_px(dpi)
}

/// Between an icon and the text after it.
pub(crate) fn glyph_gap(dpi: Dpi) -> Px {
    Dip(4).to_px(dpi)
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

    /// Whether `point`, in the rectangle's coordinates, is inside it.
    pub(crate) fn contains(self, (x, y): (i32, i32)) -> bool {
        (self.x.0..self.right().0).contains(&x) && (self.y.0..self.bottom().0).contains(&y)
    }
}

/// The window's edges and the gaps between groups.
const MARGIN: Dip = Dip(11);
/// Between related controls, such as the match controls.
const RELATED: Dip = Dip(7);
/// Between groups, such as the strip and the panes; a one-pixel divider
/// sits in the middle of it.
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
/// The inspector's icon, the shell's large icon at 100 %.
const ICON: Dip = Dip(32);
/// The inspector's heading keeps room for a short name when the window is
/// narrow; a longer name ends in an ellipsis.
const HEADING_MIN_WIDTH: Dip = Dip(100);
/// Between a text and its copy button.
const COPY_GAP: Dip = Dip(8);
/// A read-only value is this much taller than its font, the room a
/// borderless edit control gives its text.
const VALUE_PADDING: Dip = Dip(2);
/// A status bar part's borders and text inset.
const STATUS_PART_PADDING: Dip = Dip(13);
/// The right group takes the rest, at least this much.
const STATUS_REST_MIN_WIDTH: Dip = Dip(100);
/// The status bar's Cancel is this much taller than its font, less than
/// other buttons, so the status bar stays low.
const STATUS_BUTTON_PADDING: Dip = Dip(6);
/// The room between the status bar's edges, or a part's border, and the
/// controls in it.
const STATUS_INSET: Dip = Dip(2);
const PROGRESS_WIDTH: Dip = Dip(100);
const PROGRESS_HEIGHT: Dip = Dip(12);

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
            inspector_height: Dip(132),
        }
    }
}

/// What the window measures on its controls, which only Windows knows: the
/// widths of the toolbars that share a line with other controls, the Kind
/// list's rows, an icon and a text, and the Location list's rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ControlSizes {
    /// Clear, Match case and Match path, with their separators.
    pub(crate) search_tools: Px,
    /// The filter pane header's Clear and Hide filters.
    pub(crate) filter_tools: Px,
    /// The height of a Kind row and the width of the widest.
    pub(crate) kind_row: Size,
    /// The height of a Location row.
    pub(crate) location_row: Px,
}

/// The height of the toolbars' buttons, each as tall as the control beside
/// it: the search box's, also in the filter pane's header and beside the
/// inspector's heading; a field row's for the inspector's copy icons; a
/// status bar button's for Show details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ToolHeights {
    pub(crate) row: Px,
    pub(crate) copy: Px,
    pub(crate) status: Px,
}

impl ToolHeights {
    pub(crate) fn new(dpi: Dpi, font_height: Px) -> ToolHeights {
        ToolHeights {
            row: font_height + CONTROL_PADDING.to_px(dpi),
            copy: InspectorLayout::copy_side(dpi, font_height),
            status: font_height + STATUS_BUTTON_PADDING.to_px(dpi),
        }
    }
}

/// The measured width of each caption that the window places in code, in
/// the current language, font and DPI, and the sizes of the controls that
/// the window measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptionWidths {
    match_label: Px,
    match_modes: Px,
    search_tools: Px,
    filters: FilterWidths,
    /// The widest label of an inspector field.
    inspector_labels: Px,
    status: StatusWidths,
    memory: Option<MemoryWidths>,
    /// Whether icons show beside text, which puts one before each memory
    /// figure.
    icons: bool,
    /// Explore, Open location, Open and Copy path.
    inspector_buttons: ButtonWidths,
    /// The list message's buttons, every fix's included, and a panel's
    /// Retry.
    message_buttons: ButtonWidths,
    status_cancel: ButtonWidths,
    /// A drive button's widest text, such as `W:`.
    drive: Px,
    /// A search panel's Look in label, and its list's widest item.
    look_in_label: Px,
    look_in: Px,
    /// The widest free space of a panel's status line.
    free: Px,
    /// The widest key bar caption, its key and its word; each button also
    /// shows its command's icon while icons show.
    key_bar: Px,
}

/// The widest captions of push buttons that share one width, as a row of a
/// dialog's buttons does (DESIGN.md: Look).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ButtonWidths {
    plain: Px,
    /// The widest of those that show an image before the caption: an icon,
    /// or the administrator shield.
    with_image: Px,
}

impl ButtonWidths {
    /// Measures `buttons`, each a caption and whether it shows an image,
    /// with `measure`, as [`CaptionWidths::measure`] does.
    fn measure<'a>(
        buttons: impl IntoIterator<Item = (&'a String, bool)>,
        measure: &dyn Fn(&str) -> Px,
    ) -> ButtonWidths {
        let mut widths = ButtonWidths::default();
        for (caption, with_image) in buttons {
            let widest = if with_image {
                &mut widths.with_image
            } else {
                &mut widths.plain
            };
            *widest = (*widest).max(measure(caption));
        }
        widths
    }

    /// The buttons' one width.
    pub(crate) fn width(self, dpi: Dpi) -> Px {
        button_width(self.plain, dpi).max(button_width(
            button_content(self.with_image, true, dpi),
            dpi,
        ))
    }
}

/// The measured widths of the status bar's captions: the widest text of
/// each fixed group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StatusWidths {
    view: Px,
    running: Px,
    done: Px,
    panel_index: Px,
}

impl StatusWidths {
    /// Measures with `measure`, as [`CaptionWidths::measure`] does.
    pub(crate) fn measure(widest: &StatusWidest, measure: &dyn Fn(&str) -> Px) -> StatusWidths {
        let widest_of =
            |texts: &[String]| texts.iter().map(|t| measure(t)).max().unwrap_or_default();
        StatusWidths {
            view: widest_of(&widest.view),
            running: widest_of(&widest.running),
            done: widest_of(&widest.done),
            panel_index: widest_of(&widest.panel_index),
        }
    }
}

/// The measured widths of the main window's memory group (DESIGN.md: Status
/// bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemoryWidths {
    /// The service's widest figure.
    service: Px,
    /// The app's widest figure.
    app: Px,
}

impl MemoryWidths {
    /// Measures with `measure`, as [`CaptionWidths::measure`] does.
    fn measure(widest: &MemoryWidest, measure: &dyn Fn(&str) -> Px) -> MemoryWidths {
        let widest_of =
            |texts: &[String]| texts.iter().map(|t| measure(t)).max().unwrap_or_default();
        MemoryWidths {
            service: widest_of(&widest.service),
            app: widest_of(&widest.app),
        }
    }
}

/// The measured widths of the filter pane's captions and controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FilterWidths {
    /// Whether Choose folder… shows its icon.
    icons: bool,
    title: Px,
    /// The header's toolbar: Clear and Hide filters.
    tools: Px,
    choose_folder: Px,
    only_directly: Px,
    /// The widest group label.
    labels: Px,
    kind_row: Size,
    location_row: Px,
    /// The widest Date modified or Size item.
    ranges: Px,
    attribute_names: Px,
    attribute_choices: Px,
}

impl CaptionWidths {
    /// Measures `captions` with `measure`, which gives the width of a text
    /// as its control draws it, an access-key `&` included, beside the
    /// `controls` that the window measured.
    pub(crate) fn measure(
        captions: &WindowCaptions,
        measure: &dyn Fn(&str) -> Px,
        controls: ControlSizes,
    ) -> CaptionWidths {
        let widest = |texts: &[String]| texts.iter().map(|t| measure(t)).max().unwrap_or_default();
        let filters = &captions.filters;
        // Every button shows an icon while icons show, and a button that
        // needs administrator permission shows the shield also while they
        // are hidden.
        let inspector_buttons = captions
            .inspector_buttons
            .iter()
            .chain([&captions.copy_path])
            .map(|button| (&button.caption, captions.icons));
        let message_buttons = captions
            .list_buttons
            .iter()
            .map(|(caption, shielded)| (caption, *shielded || captions.icons))
            .chain([(&captions.retry, captions.icons)]);
        CaptionWidths {
            match_label: measure(&captions.match_label),
            match_modes: widest(&captions.match_modes),
            search_tools: controls.search_tools,
            filters: FilterWidths {
                icons: captions.icons,
                title: measure(&filters.title),
                tools: controls.filter_tools,
                choose_folder: measure(&filters.choose_folder),
                only_directly: measure(&filters.only_directly),
                labels: widest(&[
                    filters.kind.clone(),
                    filters.location.clone(),
                    filters.date.clone(),
                    filters.size.clone(),
                    filters.attributes.clone(),
                ]),
                kind_row: controls.kind_row,
                location_row: controls.location_row,
                ranges: widest(&filters.ranges_widest),
                attribute_names: widest(&filters.attribute_names),
                attribute_choices: widest(&filters.attribute_choices),
            },
            inspector_labels: widest(&captions.inspector_labels),
            status: StatusWidths::measure(&captions.status_widest, measure),
            memory: captions
                .memory_widest
                .as_ref()
                .map(|widest| MemoryWidths::measure(widest, measure)),
            icons: captions.icons,
            inspector_buttons: ButtonWidths::measure(inspector_buttons, measure),
            message_buttons: ButtonWidths::measure(message_buttons, measure),
            status_cancel: ButtonWidths::measure(
                [(&captions.status_cancel, captions.icons)],
                measure,
            ),
            drive: measure("W:").max(measure(r"\\")),
            look_in_label: measure(&captions.look_in_label),
            look_in: widest(&captions.look_in),
            free: measure(&captions.free_widest),
            key_bar: captions
                .key_bar
                .iter()
                .map(|(key, word, _)| measure(&format!("{key} {word}")))
                .max()
                .unwrap_or_default(),
        }
    }

    /// The width the inspector's buttons share.
    pub(crate) fn inspector_button_width(&self, dpi: Dpi) -> Px {
        self.inspector_buttons.width(dpi)
    }

    /// The width the list message's buttons share.
    pub(crate) fn message_button_width(&self, dpi: Dpi) -> Px {
        self.message_buttons.width(dpi)
    }
}

/// A push button is this much wider than its caption.
const BUTTON_PADDING: Dip = Dip(24);
/// The narrowest push button, as Windows' guidelines size OK and Cancel.
const BUTTON_MIN_WIDTH: Dip = Dip(75);

/// The space above a panel's message, under its list's header, as the
/// prototype draws it.
const MESSAGE_TOP: Dip = Dip(18);
/// The room a button needs for a caption `caption` wide, with an image
/// before it when `with_image`: an icon, or the administrator shield of a
/// button that runs as administrator, which is as large.
pub(crate) fn button_content(caption: Px, with_image: bool, dpi: Dpi) -> Px {
    if with_image {
        caption + icon_glyph_height(dpi) + glyph_gap(dpi)
    } else {
        caption
    }
}

/// The width of a push button for a caption `caption` wide.
fn button_width(caption: Px, dpi: Dpi) -> Px {
    button_room(caption, dpi).max(BUTTON_MIN_WIDTH.to_px(dpi))
}

/// The width a push button needs for content `content` wide, which
/// [`button_content`] gives, before any least width.
pub(crate) fn button_room(content: Px, dpi: Dpi) -> Px {
    content + BUTTON_PADDING.to_px(dpi)
}

/// The width of a push button whose caption is `caption` wide, with an icon
/// before it, as Retry under a message has.
pub(crate) fn image_button_width(caption: Px, dpi: Dpi) -> Px {
    button_width(button_content(caption, true, dpi), dpi)
}

/// The width of a status bar's last part, for a text `text` wide: the
/// part's borders and inset, and the size grip.
pub(crate) fn status_last_part(text: Px, dpi: Dpi) -> Px {
    text + STATUS_PART_PADDING.to_px(dpi) + SCROLL_BAR.to_px(dpi)
}

/// The space above the Viewer's message in the area that shows the file,
/// as the prototype draws it.
const VIEWER_MESSAGE_TOP: Dip = Dip(60);

/// The part of `area`, the area that shows the file, left of its vertical
/// scroll bar, which the area always shows.
fn viewer_page(area: Rect, dpi: Dpi) -> Rect {
    Rect {
        width: (area.width - SCROLL_BAR.to_px(dpi)).max(Px(0)),
        ..area
    }
}

/// The width the Viewer's message wraps at in `area`, the area that shows
/// the file.
pub(crate) fn viewer_message_width(area: Rect, dpi: Dpi) -> Px {
    (viewer_page(area, dpi).width - MARGIN.to_px(dpi) - MARGIN.to_px(dpi)).max(Px(0))
}

/// Where the Viewer's message, `text_height` tall, and Retry, `retry_width`
/// wide, go in `area`: both centred left of its scroll bar, the message
/// below the area's top as the prototype draws it, and Retry under it, as a
/// panel's Retry is.
pub(crate) fn viewer_message(
    area: Rect,
    text_height: Px,
    font_height: Px,
    retry_width: Px,
    dpi: Dpi,
) -> (Rect, Rect) {
    let px = |dip: Dip| dip.to_px(dpi);
    let area = viewer_page(area, dpi);
    let message = Rect {
        x: area.x + px(MARGIN),
        y: area.y + px(VIEWER_MESSAGE_TOP),
        width: viewer_message_width(area, dpi),
        height: text_height,
    };
    let retry = Rect {
        x: area.x + Px((area.width.0 - retry_width.0) / 2),
        y: message.bottom() + px(RELATED),
        width: retry_width,
        height: font_height + px(CONTROL_PADDING),
    };
    (message, retry)
}

/// The width of a Clear button `height` tall with a caption `caption` wide
/// (DESIGN.md: Look): square while `icons` show, because it then shows only
/// its icon, and otherwise its caption's room, with no least width, because
/// it stands beside the box it clears and shares no row with other buttons.
pub(crate) fn clear_width(caption: Px, icons: bool, height: Px, dpi: Dpi) -> Px {
    if icons {
        height
    } else {
        button_room(caption, dpi)
    }
}

/// The places that give the push buttons of one dialog or page, `buttons`,
/// one width `width` (DESIGN.md: Look). A row is the buttons with one top,
/// and its buttons farther apart than `margin` are groups of their own,
/// such as Settings' Reset all and its OK, Cancel and Apply. A group at the
/// left margin keeps its left edge; any other keeps its right edge, and the
/// control of `others` to its left on its line shrinks to keep its gap.
/// Returns the buttons' places in `buttons`' order, and the index and new
/// place of each control of `others` that shrinks.
pub(crate) fn sized_buttons(
    buttons: &[Rect],
    others: &[Rect],
    width: Px,
    margin: Px,
) -> (Vec<Rect>, Vec<(usize, Rect)>) {
    let mut placed = buttons.to_vec();
    let mut shrunk: Vec<(usize, Rect)> = Vec::new();
    let mut order: Vec<usize> = (0..buttons.len()).collect();
    order.sort_by_key(|&i| (buttons[i].y, buttons[i].x));
    for row in order.chunk_by(|&a, &b| buttons[a].y == buttons[b].y) {
        for group in row.chunk_by(|&a, &b| buttons[b].x - buttons[a].right() <= margin) {
            let first = buttons[group[0]];
            if first.x <= margin {
                let mut x = first.x;
                for pair in group.windows(2) {
                    placed[pair[0]] = Rect {
                        x,
                        width,
                        ..buttons[pair[0]]
                    };
                    x = x + width + (buttons[pair[1]].x - buttons[pair[0]].right());
                }
                let last = group[group.len() - 1];
                placed[last] = Rect {
                    x,
                    width,
                    ..buttons[last]
                };
                continue;
            }
            let mut right = buttons[group[group.len() - 1]].right();
            for pair in group.windows(2).rev() {
                placed[pair[1]] = Rect {
                    x: right - width,
                    width,
                    ..buttons[pair[1]]
                };
                right = right - width - (buttons[pair[1]].x - buttons[pair[0]].right());
            }
            placed[group[0]] = Rect {
                x: right - width,
                width,
                ..first
            };
            let shift = first.x - (right - width);
            let beside = others
                .iter()
                .enumerate()
                .filter(|(_, rect)| rect.right() <= first.x && overlaps_rows(**rect, first))
                .max_by_key(|(_, rect)| rect.right());
            if let Some((index, rect)) = beside {
                let width = rect.width - shift;
                match shrunk.iter_mut().find(|(shrunk, _)| *shrunk == index) {
                    Some((_, place)) => place.width = place.width.min(width),
                    None => shrunk.push((index, Rect { width, ..*rect })),
                }
            }
        }
    }
    (placed, shrunk)
}

/// A link control placed at `link` narrowed to its text, `text_width`
/// wide, keeping its right edge, and the control `beside` on its line
/// widened or narrowed to end `gap` before it.
pub(crate) fn link_beside(link: Rect, text_width: Px, beside: Rect, gap: Px) -> (Rect, Rect) {
    let link = Rect {
        x: link.right() - text_width,
        width: text_width,
        ..link
    };
    let beside = Rect {
        width: link.x - gap - beside.x,
        ..beside
    };
    (link, beside)
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
    /// `buttons` buttons' in a row below it, each `width` wide: from the
    /// text's left edge, or `centred` under a centred text, as a panel's.
    pub(crate) fn place(
        &self,
        text_height: Px,
        font_height: Px,
        buttons: usize,
        width: Px,
        centred: bool,
    ) -> (Rect, Vec<Rect>) {
        let px = |dip: Dip| dip.to_px(self.dpi);
        let text = Rect {
            height: text_height.min(self.text.height),
            ..self.text
        };
        let row = Px(width.0 * buttons as i32 + px(RELATED).0 * (buttons as i32 - 1).max(0));
        let mut x = match centred {
            true => text.x + Px((text.width.0 - row.0) / 2),
            false => text.x,
        };
        let buttons = (0..buttons)
            .map(|_| {
                let button = Rect {
                    x,
                    y: text.bottom() + px(GROUP),
                    width,
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

/// The filter pane's header in the client area, and under it the pane's
/// window, with its controls in the pane's coordinates before it scrolls.
/// When the pane is shorter than its controls, it shows a scroll bar and the
/// part of them from its scroll position down, and the header stays
/// (DESIGN.md: Filter pane).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FilterLayout {
    /// The header: the title at the left, then the toolbar of Clear and
    /// the icon that hides the pane at the right.
    pub(crate) title: Rect,
    pub(crate) tools: Rect,
    /// The pane's window in the client area.
    pub(crate) pane: Rect,
    /// The height of the pane's controls, its scroll range.
    pub(crate) content_height: Px,
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
    /// The header and the pane's controls in `area`; when the controls are
    /// taller than the pane under the header, its scroll bar takes room from
    /// their width.
    fn compute(area: Rect, dpi: Dpi, font_height: Px, widths: &FilterWidths) -> FilterLayout {
        let px = |dip: Dip| dip.to_px(dpi);
        let related = px(RELATED);
        let side = font_height + px(CONTROL_PADDING);
        let tools = Rect {
            x: area.right() - widths.tools,
            y: area.y,
            width: widths.tools,
            height: side,
        };
        let title = Rect {
            width: (tools.x - related - area.x).max(Px(0)),
            height: side,
            ..area
        };
        let top = tools.bottom() + related;
        let pane = Rect {
            y: top,
            height: (area.bottom() - top).max(Px(0)),
            ..area
        };
        let header = |placed: FilterLayout| FilterLayout {
            title,
            tools,
            ..placed
        };
        let full = FilterLayout::place(pane, pane.width, dpi, font_height, widths);
        if full.content_height > pane.height {
            let width = pane.width - SCROLL_BAR.to_px(dpi);
            header(FilterLayout::place(pane, width, dpi, font_height, widths))
        } else {
            header(full)
        }
    }

    /// The controls `width` wide, from the top down; the header is
    /// [`Self::compute`]'s.
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
        // Choose folder… spans the pane, as its lists and boxes do
        // (DESIGN.md: Filter pane).
        let label = |rect: Rect| Rect {
            width: widths.labels.min(width),
            ..rect
        };
        let kind_label = label(next(font_height, LABEL_GAP));
        let kind = next(
            Px(widths.kind_row.height.0 * Kind::ALL.len() as i32) + px(LIST_BOX_CHROME),
            GROUP,
        );
        let location_label = label(next(font_height, LABEL_GAP));
        let location = next(
            Px(widths.location_row.0 * LOCATION_ROWS as i32) + px(LIST_BOX_CHROME),
            RELATED,
        );
        let choose_folder = next(control_height, RELATED);
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
            title: Rect::default(),
            tools: Rect::default(),
            pane,
            content_height: attributes[5].1.bottom(),
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
            widths.title + px(RELATED) + widths.tools,
            button_width(button_content(widths.choose_folder, widths.icons, dpi), dpi),
            widths.only_directly + px(CHECK_BOX_CHROME),
            widths.labels,
            widths.kind_row.width + px(LIST_BOX_CHROME),
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

/// The inspector's controls in the client area (DESIGN.md: Inspector pane).
/// The heading's row holds the icon, the heading and Hide details; the fields
/// follow, a label and a value in each row. Copy path stands at the right end
/// of the Date modified row, and the buttons at the right end of one entry's
/// last row, the bottom right of the content, in one column. No control
/// overlaps a button, because the window draws siblings in no set order where
/// they overlap. [`InspectorLayout::copy_beside`] and
/// [`InspectorLayout::age_beside`] place what follows a text, which only the
/// window can measure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InspectorLayout {
    /// The band between the list and the pane that the person drags.
    pub(crate) splitter: Rect,
    pub(crate) icon: Rect,
    pub(crate) heading: Rect,
    /// Hide details, a square icon at the right end of the heading's row.
    pub(crate) hide: Rect,
    /// At the right end of the Date modified row, above the buttons and as
    /// wide.
    pub(crate) copy_path: Rect,
    /// In [`INSPECTOR_BUTTONS`]' order, from the left.
    pub(crate) buttons: [Rect; INSPECTOR_BUTTONS.len()],
    /// Each field row's label and value, as many as the pane's height
    /// shows; a row beside a button ends a gap before it.
    pub(crate) fields: Vec<(Rect, Rect)>,
    /// The hint under the heading, across the first field row, which is
    /// empty while the hint shows; `None` when the pane has no room for it.
    pub(crate) note: Option<Rect>,
    /// [`Self::copy_side`] at the layout's DPI and font.
    copy_side: Px,
    /// The pane's height, and the heights the splitter can give it.
    height: Px,
    min_height: Px,
    max_height: Px,
    dpi: Dpi,
}

impl InspectorLayout {
    /// The rows of one entry, the most that any selection shows.
    const FIELD_ROWS: usize = 4;

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
        let width = captions.inspector_button_width(dpi);
        let button_height = font_height + px(CONTROL_PADDING);
        let row = Rect {
            height: px(ICON).max(button_height),
            ..area
        };
        let icon = Rect {
            y: row.y + Px((row.height.0 - px(ICON).0) / 2),
            width: px(ICON),
            height: px(ICON),
            ..row
        };
        let hide = Rect {
            x: area.right() - button_height,
            y: row.y + Px((row.height.0 - button_height.0) / 2),
            width: button_height,
            height: button_height,
        };
        let heading_x = icon.right() + related;
        let heading = Rect {
            x: heading_x,
            width: (hide.x - related - heading_x).max(px(HEADING_MIN_WIDTH)),
            ..row
        };
        // The buttons and Copy path from the right edge, centred on their
        // rows, so they end the content whatever the pane's height.
        let on_row = |index: usize, x: Px| Rect {
            x,
            y: area.y + Self::button_line(dpi, font_height, index).y,
            width,
            height: button_height,
        };
        let mut right = area.right();
        let mut buttons = [Rect::default(); INSPECTOR_BUTTONS.len()];
        for button in buttons.iter_mut().rev() {
            *button = on_row(Self::FIELD_ROWS - 1, right - width);
            right = button.x - related;
        }
        let copy_path = on_row(MODIFIED_ROW, area.right() - width);
        // A row beside a button ends a gap before it.
        let end = |line: Rect| {
            [buttons[0], copy_path]
                .into_iter()
                .filter(|button| overlaps_rows(line, *button))
                .map(|button| button.x - related)
                .min()
                .unwrap_or(area.right())
        };
        let value_height = font_height + px(VALUE_PADDING);
        let value_x = area.x + captions.inspector_labels + px(GROUP);
        let mut fields = Vec::new();
        for index in 0..Self::FIELD_ROWS {
            let row = Self::row_height(dpi, font_height, index);
            let line = Rect {
                x: area.x,
                y: area.y + Self::row_top(dpi, font_height, index) + Px((row - value_height).0 / 2),
                width: area.width,
                height: value_height,
            };
            if line.bottom() > area.bottom() {
                break;
            }
            let label = Rect {
                width: captions.inspector_labels.min(end(line) - area.x),
                ..line
            };
            let value = Rect {
                x: value_x,
                width: (end(line) - value_x).max(Px(0)),
                ..line
            };
            fields.push((label, value));
        }
        let note = fields.first().map(|(label, _)| Rect {
            width: end(*label) - area.x,
            ..*label
        });
        let min_height = Self::min_height(dpi, font_height);
        InspectorLayout {
            splitter: Rect {
                y: area.y - px(GROUP),
                height: px(GROUP),
                ..area
            },
            icon,
            heading,
            hide,
            copy_path,
            buttons,
            fields,
            note,
            copy_side: Self::copy_side(dpi, font_height),
            height: area.height,
            min_height,
            max_height: max_height.max(min_height),
            dpi,
        }
    }

    /// A copy icon's side: a value's line and the gap after it, so the
    /// icons of two rows would meet without overlapping.
    fn copy_side(dpi: Dpi, font_height: Px) -> Px {
        font_height + VALUE_PADDING.to_px(dpi) + LABEL_GAP.to_px(dpi)
    }

    /// The top of field row `index`, from the pane's top: the heading's
    /// row, then a gap, then the rows one under the other.
    fn row_top(dpi: Dpi, font_height: Px, index: usize) -> Px {
        let px = |dip: Dip| dip.to_px(dpi);
        let heading = px(ICON).max(font_height + px(CONTROL_PADDING));
        (0..index).fold(heading + px(RELATED), |top, row| {
            top + Self::row_height(dpi, font_height, row) + px(LABEL_GAP)
        })
    }

    /// The height of field row `index`: a button's on the rows that hold
    /// one, Copy path's and the buttons', so the buttons of two rows never
    /// touch, and a value's on the others.
    fn row_height(dpi: Dpi, font_height: Px, index: usize) -> Px {
        let padding = if index == MODIFIED_ROW || index == Self::FIELD_ROWS - 1 {
            CONTROL_PADDING
        } else {
            VALUE_PADDING
        };
        font_height + padding.to_px(dpi)
    }

    /// A button's place on field row `index`, from the pane's top, which
    /// fills the row's height.
    fn button_line(dpi: Dpi, font_height: Px, index: usize) -> Rect {
        Rect {
            y: Self::row_top(dpi, font_height, index),
            height: Self::row_height(dpi, font_height, index),
            ..Rect::default()
        }
    }

    /// The lowest pane, which shows every row and the buttons on the last.
    fn min_height(dpi: Dpi, font_height: Px) -> Px {
        Self::button_line(dpi, font_height, Self::FIELD_ROWS - 1).bottom()
    }

    /// The narrowest pane whose heading's row fits with Hide details, and
    /// whose labels and buttons fit side by side.
    fn min_width(dpi: Dpi, font_height: Px, captions: &CaptionWidths) -> Px {
        let px = |dip: Dip| dip.to_px(dpi);
        let button = captions.inspector_button_width(dpi);
        let hide = font_height + px(CONTROL_PADDING);
        let heading = px(ICON) + px(RELATED) + px(HEADING_MIN_WIDTH) + px(RELATED) + hide;
        let buttons =
            (1..INSPECTOR_BUTTONS.len()).fold(button, |sum, _| sum + px(RELATED) + button);
        heading.max(captions.inspector_labels + px(GROUP) + px(RELATED) + buttons)
    }

    /// Where a text `width` wide on `line` shows with an item `item` wide
    /// `gap` after it: the item keeps its room within the line, so a long
    /// text is cut and the item stays visible. Returns the text's rectangle
    /// and the item's, both as high as the line.
    fn after_text(line: Rect, width: Px, gap: Px, item: Px) -> (Rect, Rect) {
        let text = Rect {
            width: width.min(line.width - gap - item).max(Px(0)),
            ..line
        };
        let x = text.right() + gap;
        let item = Rect {
            x,
            width: item.min(line.right() - x).max(Px(0)),
            ..line
        };
        (text, item)
    }

    /// Where a text `width` wide on `line`, the heading's or a value's,
    /// shows with its copy icon 8 px after it, as [`Self::after_text`]
    /// places an item; the icon is square, centred on the line.
    pub(crate) fn copy_beside(&self, line: Rect, width: Px) -> (Rect, Rect) {
        let side = self.copy_side;
        let (text, slot) = Self::after_text(line, width, COPY_GAP.to_px(self.dpi), side);
        let icon = Rect {
            y: line.y + Px((line.height.0 - side.0) / 2),
            height: side,
            ..slot
        };
        (text, icon)
    }

    /// Where Date modified's value `width` wide on `line` shows with its
    /// age `age` wide a space `space` after it, as [`Self::after_text`]
    /// places an item.
    pub(crate) fn age_beside(&self, line: Rect, width: Px, space: Px, age: Px) -> (Rect, Rect) {
        Self::after_text(line, width, space, age)
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

/// The most dividers a layout has: in Two panels, the line between the
/// panels, the line above the key bar and the line above each panel's
/// status line. The window makes a control for each.
pub(crate) const DIVIDERS: usize = 4;

/// Every control's rectangle in the client area, the status bar's part
/// widths, and the smallest client size at which every control fits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) body: Body,
    /// The one-pixel lines between the panes or the panels, at most
    /// [`DIVIDERS`].
    pub(crate) dividers: Vec<Rect>,
    pub(crate) status: StatusLayout,
    pub(crate) min_client: Size,
}

/// The controls of the mode that shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Body {
    /// Boxed, because Search's controls take far more room than two lists.
    Search(Box<SearchLayout>),
    /// The left and the right panel, each half of the window, and the key
    /// bar under them (DESIGN.md: Two panels' window). The panels are boxed, as
    /// Search is, so the two variants take about the same room.
    TwoPanels {
        panels: Box<[PanelLayout; 2]>,
        key_bar: [Rect; 6],
    },
}

/// One panel's controls, from the top: the drive button and the path box,
/// or a search panel's search box and Look in, the list, and the status
/// line (DESIGN.md: A panel, Search in a panel). The list's message lies over
/// the list, as Search's does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PanelLayout {
    pub(crate) drive: Rect,
    pub(crate) path: Rect,
    pub(crate) search: Rect,
    pub(crate) look_in_label: Rect,
    pub(crate) look_in: Rect,
    pub(crate) list: Rect,
    /// The status line's counts, which the quick search box replaces.
    pub(crate) counts: Rect,
    /// The free space, at the status line's right end.
    pub(crate) free: Rect,
    /// A search panel's counts while it searches all drives, in the status
    /// line's left half, and the focused result's folder in its right half.
    pub(crate) counts_half: Rect,
    pub(crate) folder: Rect,
}

/// Search's controls between the menu bar and the status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchLayout {
    /// Show filters, before the search box while the filter pane is hidden.
    pub(crate) show_filters: Option<Rect>,
    pub(crate) search: Rect,
    /// The toolbar after the search box: Clear, Match case and Match path.
    pub(crate) search_tools: Rect,
    pub(crate) match_label: Rect,
    pub(crate) match_mode: Rect,
    /// `None` while the pane is hidden.
    pub(crate) filters: Option<FilterLayout>,
    pub(crate) list: Rect,
    /// `None` while the pane is hidden.
    pub(crate) inspector: Option<InspectorLayout>,
}

impl Layout {
    /// The layout of `mode`, where Search shows `panes`. `status_height` is
    /// the height the status bar gives itself for its font. When the client
    /// is smaller than `min_client`, the controls keep their minimum sizes
    /// and the window's minimum size stops it there.
    pub(crate) fn compute(
        client: Size,
        dpi: Dpi,
        font_height: Px,
        status_height: Px,
        captions: &CaptionWidths,
        mode: Mode,
        panes: &Panes,
    ) -> Layout {
        match mode {
            Mode::Search => {
                Layout::compute_search(client, dpi, font_height, status_height, captions, panes)
            }
            Mode::TwoPanels => {
                Layout::compute_two_panels(client, dpi, font_height, status_height, captions)
            }
        }
    }

    /// Search's controls, or `None` while Two panels shows.
    pub(crate) fn search(&self) -> Option<&SearchLayout> {
        match &self.body {
            Body::Search(search) => Some(search.as_ref()),
            Body::TwoPanels { .. } => None,
        }
    }

    /// The inspector's controls, or `None` while it or Two panels hides it.
    pub(crate) fn inspector(&self) -> Option<&InspectorLayout> {
        self.search()?.inspector.as_ref()
    }

    /// The two panels side by side from the menu bar to the key bar, with a
    /// divider between them and no splitter (DESIGN.md: Two panels' window),
    /// and the key bar's buttons sharing its width. The status bar has no left
    /// group, because each panel's status line shows its drive's free space.
    fn compute_two_panels(
        client: Size,
        dpi: Dpi,
        font_height: Px,
        status_height: Px,
        captions: &CaptionWidths,
    ) -> Layout {
        let px = |dip: Dip| dip.to_px(dpi);
        let (margin, related) = (px(MARGIN), px(RELATED));
        let control_height = font_height + px(CONTROL_PADDING);
        let line_height = font_height + px(ROW_PADDING);
        let list_min_width = LIST_MIN_WIDTH.to_px(dpi);
        // Tall enough for the header, the message and Retry.
        let list_min_height = Px(line_height.0 * LIST_MIN_ROWS).max(
            line_height
                + px(MESSAGE_TOP)
                + Px(font_height.0 * 2)
                + related
                + control_height
                + related,
        );
        // The memory group shows as in Search; Show details does not,
        // because Two panels has no inspector.
        let mut status = StatusLayout::compute(
            &captions.status,
            captions.status_cancel.width(dpi),
            dpi,
            font_height,
            status_height,
            Some(&WindowEnd {
                width: client.width,
                memory: captions.memory,
                icons: captions.icons,
                show_details: false,
            }),
        );
        status.drop_view_group();
        let key_button = button_room(button_content(captions.key_bar, captions.icons, dpi), dpi);
        let key_min = Px(key_button.0 * 6 + related.0 * 5 + margin.0 * 2);
        let path_row = related + control_height + related;
        let status_line = Px(1) + line_height;
        let key_row = Px(1) + related + control_height + related;
        let drive_width = captions.drive + px(DROP_DOWN_CHROME);
        let look_in_width = captions.look_in + px(DROP_DOWN_CHROME);
        // Either panel may search at any moment, so each is as wide as the
        // right panel's search row, which has a margin on both sides.
        let search_row = margin
            + drive_width
            + related
            + px(SEARCH_MIN_WIDTH)
            + related
            + captions.look_in_label
            + related
            + look_in_width
            + margin;
        let panel_min_width = list_min_width.max(search_row);
        let min_client = Size {
            width: (panel_min_width + Px(1) + panel_min_width)
                .max(status.min_width(dpi))
                .max(key_min),
            height: path_row + list_min_height + status_line + key_row + status_height,
        };
        let width = client.width.max(min_client.width);
        let height = client.height.max(min_client.height);
        // The right group keeps the room for its text and an active search
        // panel's two totals glyphs.
        status.give_middle_the_rest(
            captions.status.panel_index
                + Px((icon_glyph_height(dpi) + glyph_gap(dpi)).0 * 2)
                + px(STATUS_PART_PADDING),
            width,
            dpi,
        );
        let key_top = height - status_height - key_row;
        let half = Px((width.0 - 1) / 2);
        let mut dividers = vec![
            Rect {
                x: half,
                y: Px(0),
                width: Px(1),
                height: key_top,
            },
            Rect {
                x: Px(0),
                y: key_top,
                width,
                height: Px(1),
            },
        ];
        let panel = |x: Px, panel_width: Px, inner: Px, dividers: &mut Vec<Rect>| {
            let drive = Rect {
                x: x + margin,
                y: related,
                width: drive_width,
                height: control_height,
            };
            let path_x = drive.right() + related;
            let row_right = x + panel_width - inner;
            let path = Rect {
                x: path_x,
                y: related,
                width: (row_right - path_x).max(Px(0)),
                height: control_height,
            };
            // As Search's Match label and list stand at its row's end.
            let look_in = Rect {
                x: row_right - look_in_width,
                width: look_in_width,
                ..path
            };
            let look_in_label = Rect {
                x: look_in.x - related - captions.look_in_label,
                y: related + Px((control_height.0 - font_height.0) / 2),
                width: captions.look_in_label,
                height: font_height,
            };
            let search = Rect {
                width: (look_in_label.x - related - path_x).max(Px(0)),
                ..path
            };
            let line_top = key_top - status_line;
            let list = Rect {
                x,
                y: path_row,
                width: panel_width,
                height: line_top - path_row,
            };
            dividers.push(Rect {
                x,
                y: line_top,
                width: panel_width,
                height: Px(1),
            });
            let line = Rect {
                x: x + margin,
                y: line_top + Px(1),
                width: (panel_width - margin - inner).max(Px(0)),
                height: line_height,
            };
            let free_width = captions.free.min(Px(line.width.0 / 2));
            let free = Rect {
                x: line.right() - free_width,
                width: free_width,
                ..line
            };
            let counts = Rect {
                width: (free.x - related - line.x).max(Px(0)),
                ..line
            };
            let half = Px((line.width.0 - related.0) / 2);
            let counts_half = Rect {
                width: half.max(Px(0)),
                ..line
            };
            let folder = Rect {
                x: line.right() - half,
                width: half.max(Px(0)),
                ..line
            };
            PanelLayout {
                drive,
                path,
                search,
                look_in_label,
                look_in,
                list,
                counts,
                free,
                counts_half,
                folder,
            }
        };
        let left = panel(Px(0), half, related, &mut dividers);
        let right = panel(half + Px(1), width - half - Px(1), margin, &mut dividers);
        let buttons_width = Px(width.0 - margin.0 * 2 - related.0 * 5);
        let key_y = key_top + Px(1) + related;
        let key_bar = std::array::from_fn(|i| {
            let i = i as i32;
            // Each button's edges at even shares, so the widths differ by
            // at most a pixel and the row ends at the margin.
            let edge = |n: i32| Px(margin.0 + related.0 * n + buttons_width.0 * n / 6);
            Rect {
                x: edge(i),
                y: key_y,
                width: edge(i + 1) - related - edge(i),
                height: control_height,
            }
        });
        Layout {
            body: Body::TwoPanels {
                panels: Box::new([left, right]),
                key_bar,
            },
            dividers,
            status,
            min_client,
        }
    }

    fn compute_search(
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
        let centred = |height: Px| margin + Px((control_height.0 - height.0) / 2);

        // The strip, from the right edge: the match mode, its label and the
        // toolbar keep their measured widths and the search box takes the
        // rest. The toolbar's separators divide its Clear, its check buttons
        // and the match mode into three groups.
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
        let match_mode = place_left(
            captions.match_modes + px(DROP_DOWN_CHROME),
            margin,
            control_height,
        );
        let match_label = place_left(captions.match_label, centred(font_height), font_height);
        let search_tools = place_left(captions.search_tools, margin, control_height);
        let show_filters = (!panes.filters).then_some(Rect {
            x: margin,
            y: margin,
            width: control_height,
            height: control_height,
        });
        let search_x = show_filters.map_or(margin, |show| show.right() + related);
        let search = Rect {
            x: search_x,
            y: margin,
            width: (right - search_x).max(px(SEARCH_MIN_WIDTH)),
            height: control_height,
        };
        let strip_width =
            search_x + px(SEARCH_MIN_WIDTH) + related + (client.width - search_tools.x);

        // The body, between the strip and the status bar.
        let top = search.bottom() + group;
        let bottom = client.height - status_height - margin;
        let mut dividers = Vec::new();
        let mut left = margin;
        let filters = panes.filters.then(|| {
            let area = Rect {
                x: margin,
                y: top,
                width: FilterLayout::min_width(dpi, &captions.filters),
                height: (bottom - top).max(Px(0)),
            };
            left = area.right() + group;
            dividers.push(Rect {
                x: area.right() + Px((group.0 - 1) / 2),
                width: Px(1),
                ..area
            });
            FilterLayout::compute(area, dpi, font_height, &captions.filters)
        });
        let list_min_height = Px((font_height + px(ROW_PADDING)).0 * LIST_MIN_ROWS);
        let inspector_height = panes.inspector.then(|| {
            let room = bottom - top - list_min_height - group;
            (
                px(panes.inspector_height)
                    .min(room)
                    .max(InspectorLayout::min_height(dpi, font_height)),
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

        let status = StatusLayout::compute(
            &captions.status,
            captions.status_cancel.width(dpi),
            dpi,
            font_height,
            status_height,
            Some(&WindowEnd {
                width: client.width,
                memory: captions.memory,
                icons: captions.icons,
                show_details: !panes.inspector,
            }),
        );
        let body_width =
            left + if panes.inspector {
                px(LIST_MIN_WIDTH).max(InspectorLayout::min_width(dpi, font_height, captions))
            } else {
                px(LIST_MIN_WIDTH)
            } + margin;
        let inspector_min = if panes.inspector {
            group + InspectorLayout::min_height(dpi, font_height)
        } else {
            Px(0)
        };
        let min_client = Size {
            width: strip_width.max(body_width).max(status.min_width(dpi)),
            height: margin
                + control_height
                + group
                + list_min_height
                + inspector_min
                + margin
                + status_height,
        };
        Layout {
            body: Body::Search(Box::new(SearchLayout {
                show_filters,
                search,
                search_tools,
                match_label,
                match_mode,
                filters,
                list,
                inspector,
            })),
            dividers,
            status,
            min_client,
        }
    }
}

/// The status bar's groups, and the controls of its middle group in the
/// status bar's own coordinates (DESIGN.md: Status bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StatusLayout {
    /// The widths of the left and middle groups; the right group takes the
    /// rest.
    pub(crate) groups: [Px; 2],
    /// The width of a running job's text, which ends before its progress
    /// bar.
    pub(crate) running_text: Px,
    /// A running job's progress bar and Cancel, at the middle group's
    /// right end.
    pub(crate) progress: Rect,
    pub(crate) cancel: Rect,
    /// Show details, a square icon at the right end before the size grip,
    /// in a part of its own, while the inspector is hidden.
    pub(crate) show_details: Option<Rect>,
    /// The main window's memory group, in parts of its own before Show
    /// details and the size grip, while Settings > General shows it.
    pub(crate) memory: Option<MemoryLayout>,
    /// The main window's right group's width, which its parts share, to
    /// the memory group, Show details or the size grip; `None` in the
    /// Settings dialog, whose right group is one part.
    right: Option<Px>,
    /// What the right end takes from the bar's width: the memory group,
    /// Show details and the size grip.
    end: Px,
}

/// The main window's status bar's right end, which the Settings dialog's
/// lacks: the memory group while Settings > General shows it, then Show
/// details while the inspector is hidden, then the size grip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowEnd {
    /// The status bar's width.
    pub(crate) width: Px,
    pub(crate) memory: Option<MemoryWidths>,
    /// Whether an icon stands before each memory figure.
    pub(crate) icons: bool,
    pub(crate) show_details: bool,
}

/// The status bar parts of the memory group's figures, each an icon and its
/// text, in the status bar's coordinates. Each part has the width of its
/// figure's widest form, so a changing figure never moves the next
/// (DESIGN.md: Look).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemoryLayout {
    pub(crate) service: Rect,
    pub(crate) app: Rect,
}

impl MemoryLayout {
    /// The parts, the app's ending at `right`, as tall as the bar's
    /// `height`, with room for the icon that the bar shows before each
    /// figure while `icons` show.
    fn compute(
        widths: &MemoryWidths,
        icons: bool,
        right: Px,
        height: Px,
        dpi: Dpi,
    ) -> MemoryLayout {
        let part_width = |text: Px| status_part_width(text, icons, dpi);
        let part = |right: Px, width: Px| Rect {
            x: right - width,
            y: Px(0),
            width,
            height,
        };
        let app = part(right, part_width(widths.app));
        let service = part(app.x, part_width(widths.service));
        MemoryLayout { service, app }
    }
}

/// The width of a status bar part that shows a `text` wide text whole,
/// after an icon when `icon` is true.
pub(crate) fn status_part_width(text: Px, icon: bool, dpi: Dpi) -> Px {
    let icon = if icon {
        icon_glyph_height(dpi) + glyph_gap(dpi)
    } else {
        Px(0)
    };
    icon + text + STATUS_PART_PADDING.to_px(dpi)
}

impl StatusLayout {
    /// The height of the status bar's drawing area, which fits Cancel;
    /// Windows adds the bar's borders.
    pub(crate) fn min_height(dpi: Dpi, font_height: Px) -> Px {
        let inset = STATUS_INSET.to_px(dpi);
        font_height + STATUS_BUTTON_PADDING.to_px(dpi) + inset + inset
    }

    /// `cancel_width` is Cancel's width, which the window or the dialog
    /// that holds the bar decides; `height` is the status bar's height;
    /// `window_end` is the main window's right end.
    pub(crate) fn compute(
        widths: &StatusWidths,
        cancel_width: Px,
        dpi: Dpi,
        font_height: Px,
        height: Px,
        window_end: Option<&WindowEnd>,
    ) -> StatusLayout {
        let px = |dip: Dip| dip.to_px(dpi);
        let related = px(RELATED);
        let running = widths.running + related + px(PROGRESS_WIDTH) + related + cancel_width;
        let groups = [
            widths.view + px(STATUS_PART_PADDING),
            running.max(widths.done) + px(STATUS_PART_PADDING),
        ];
        let centred = |control: Px| Px((height.0 - control.0) / 2);
        let button_height = font_height + px(STATUS_BUTTON_PADDING);
        let cancel = Rect {
            x: groups[0] + groups[1] - px(STATUS_INSET) - cancel_width,
            y: centred(button_height),
            width: cancel_width,
            height: button_height,
        };
        let progress = Rect {
            x: cancel.x - related - px(PROGRESS_WIDTH),
            y: centred(px(PROGRESS_HEIGHT)),
            width: px(PROGRESS_WIDTH),
            height: px(PROGRESS_HEIGHT),
        };
        // Left of the size grip, as wide as a scroll bar.
        let grip = |end: &WindowEnd| end.width - px(SCROLL_BAR);
        let show_details = window_end.filter(|end| end.show_details).map(|end| Rect {
            x: grip(end) - px(STATUS_INSET) - button_height,
            y: centred(button_height),
            width: button_height,
            height: button_height,
        });
        let memory = window_end.and_then(|end| {
            let right = show_details.map_or(grip(end), |show| show.x - px(STATUS_INSET));
            let widths = end.memory.as_ref()?;
            Some(MemoryLayout::compute(widths, end.icons, right, height, dpi))
        });
        // Where the right group ends while a part follows it.
        let right_end = memory
            .map(|memory| memory.service.x)
            .or(show_details.map(|show| show.x - px(STATUS_INSET)));
        StatusLayout {
            groups,
            running_text: widths.running,
            progress,
            cancel,
            show_details,
            memory,
            right: window_end
                .map(|end| (right_end.unwrap_or(grip(end)) - groups[0] - groups[1]).max(Px(0))),
            end: window_end
                .zip(right_end)
                .map_or(Px(0), |(end, right_end)| end.width - right_end),
        }
    }

    /// Leaves out the left group, which Two panels does not have, by giving
    /// it no width.
    fn drop_view_group(&mut self) {
        let width = self.groups[0];
        self.groups[0] = Px(0);
        self.cancel.x = self.cancel.x - width;
        self.progress.x = self.progress.x - width;
    }

    /// Widens the middle group of a bar `width` wide so the right group
    /// keeps only `right`, as Two panels draws it. The right group ends
    /// where the memory group starts, or before the size grip when it is
    /// the last part. The running job's progress bar and Cancel stay beside
    /// its text.
    fn give_middle_the_rest(&mut self, right: Px, width: Px, dpi: Dpi) {
        let end = if self.followed() {
            self.end
        } else {
            SCROLL_BAR.to_px(dpi)
        };
        let start = self.groups[0] + self.groups[1];
        self.groups[1] = self.groups[1].max(width - end - right - self.groups[0]);
        if let Some(rest) = &mut self.right {
            *rest = (*rest - (self.groups[0] + self.groups[1] - start)).max(Px(0));
        }
    }

    /// Whether a part follows the right group: the memory group's or Show
    /// details'.
    fn followed(&self) -> bool {
        self.memory.is_some() || self.show_details.is_some()
    }

    /// How many of the right group's parts show, of parts that need
    /// `widths` to show their texts whole: the first always, cut off where
    /// it does not fit, and each other only while it fits whole, so a total
    /// never shows in part (DESIGN.md: Status bar).
    pub(crate) fn right_parts_shown(&self, widths: &[Px]) -> usize {
        let Some(room) = self.right else {
            return widths.len();
        };
        let mut used = Px(0);
        let fitting = widths
            .iter()
            .take_while(|width| {
                used = used + **width;
                used <= room
            })
            .count();
        fitting.max(1).min(widths.len())
    }

    /// The widths of the status bar's parts but the last, which takes the
    /// rest: the left and middle groups, the right group's shown parts,
    /// which need `right`, the last taking the rest of the group, and the
    /// memory figures' parts, the app's only when Show details has the last
    /// part.
    pub(crate) fn part_widths(&self, right: &[Px]) -> Vec<Px> {
        let before_last = right.split_last().map_or(&[][..], |(_, before)| before);
        // The right group's last part ends where the next part starts.
        let last = self.right.filter(|_| self.followed()).map(|room| {
            let used = before_last.iter().fold(Px(0), |used, width| used + *width);
            (room - used).max(Px(0))
        });
        let memory = self.memory.map(|memory| {
            let app = self.show_details.map(|_| memory.app.width);
            std::iter::once(memory.service.width).chain(app)
        });
        self.groups
            .iter()
            .chain(before_last)
            .copied()
            .chain(last)
            .chain(memory.into_iter().flatten())
            .collect()
    }

    fn min_width(&self, dpi: Dpi) -> Px {
        self.groups[0] + self.groups[1] + STATUS_REST_MIN_WIDTH.to_px(dpi) + self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::tests::model;
    use crate::core::text::tests::English;

    /// A cut-off total would show a partial sum (DESIGN.md: Status bar), so
    /// a right part after the first shows only while it fits whole, and the
    /// first shows even when it does not fit.
    #[test]
    fn right_group_shows_only_parts_that_fit_whole() {
        let dpi = Dpi::new(96).expect("96 is not zero");
        let widths = StatusWidths {
            view: Px(100),
            running: Px(100),
            done: Px(100),
            panel_index: Px(100),
        };
        let end = WindowEnd {
            width: Px(800),
            memory: None,
            icons: true,
            show_details: false,
        };
        let layout = StatusLayout::compute(&widths, Px(60), dpi, Px(15), Px(24), Some(&end));
        let room = layout
            .right
            .expect("the main window's right group has a width");
        assert_eq!(layout.right_parts_shown(&[room - Px(50), Px(50), Px(1)]), 2);
        assert_eq!(layout.right_parts_shown(&[room + Px(1), Px(1)]), 1);
    }

    /// A row of buttons keeps its right edge and its gap, and the edit box
    /// to its left shrinks to keep its own; a button at the left margin
    /// keeps its left edge.
    #[test]
    fn sized_buttons_keep_their_edges_and_gaps() {
        let rect = |x: i32, y: i32, width: i32| Rect {
            x: Px(x),
            y: Px(y),
            width: Px(width),
            height: Px(20),
        };
        let buttons = [rect(276, 0, 60), rect(210, 0, 60), rect(11, 40, 60)];
        let others = [rect(10, 0, 190), rect(10, 40, 30)];
        assert_eq!(
            sized_buttons(&buttons, &others, Px(80), Px(11)),
            (
                vec![rect(256, 0, 80), rect(170, 0, 80), rect(11, 40, 80)],
                vec![(0, rect(10, 0, 150))],
            )
        );
    }

    /// The filter pane's controls, in the pane's coordinates.
    fn pane_controls(filters: &FilterLayout) -> Vec<Rect> {
        let mut controls = vec![
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
    /// give them; `stretch` stands for a longer language. The toolbars and
    /// the Kind list are as wide as their texts with room for the buttons'
    /// padding, the separators and the icons.
    fn captions(dpi: Dpi, stretch: f64, show_memory: bool) -> CaptionWidths {
        let char_width = f64::from(Dip(7).to_px(dpi).0);
        let mut model = model();
        model.settings.choices.show_memory = show_memory;
        let captions = WindowCaptions::new(&model, &English);
        let measure =
            |text: &str| Px((text.chars().count() as f64 * char_width * stretch).ceil() as i32);
        let widest = |texts: &[String]| texts.iter().map(|t| measure(t)).max();
        let controls = ControlSizes {
            search_tools: measure(&captions.match_case)
                + measure(&captions.match_path)
                + Dip(90).to_px(dpi),
            filter_tools: Dip(46).to_px(dpi),
            kind_row: Size {
                width: widest(&captions.filters.kinds).unwrap_or_default() + Dip(30).to_px(dpi),
                height: Dip(20).to_px(dpi),
            },
            location_row: Dip(19).to_px(dpi),
        };
        CaptionWidths::measure(&captions, &measure, controls)
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
    }

    /// Asserts that every control of `placed` lies in `client` above the
    /// status bar and overlaps no other.
    fn fits(placed: &[Rect], client: Size, status_height: Px, dpi: Dpi, stretch: f64) {
        for (i, a) in placed.iter().enumerate() {
            assert!(
                a.x >= Px(0)
                    && a.right() <= client.width
                    && a.bottom() <= client.height - status_height
                    && a.width > Px(0),
                "{a:?} in {client:?} at {dpi:?}, stretch {stretch}"
            );
            for b in &placed[i + 1..] {
                assert!(!overlaps(*a, *b), "{a:?} overlaps {b:?} at {dpi:?}");
            }
        }
    }

    /// A control that overlaps another or leaves the client area is cut off
    /// or hidden, and no compiler or other test sees it before a person
    /// does. English and the pseudo-language's longer captions, at 100, 150
    /// and 200 %, at the minimum size and at a large one.
    #[test]
    fn every_control_fits_without_overlap() {
        for dpi in [96, 144, 192].map(|d| Dpi::new(d).expect("a DPI is not zero")) {
            for (stretch, show_memory) in [(1.0, false), (2.0, false), (1.0, true), (2.0, true)] {
                let captions = captions(dpi, stretch, show_memory);
                let font_height = Dip(15).to_px(dpi);
                // The drawing area and the borders Windows adds to it.
                let status_height = StatusLayout::min_height(dpi, font_height) + Dip(4).to_px(dpi);
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
                        Mode::Search,
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
                            Mode::Search,
                            &panes,
                        );
                        let search = layout.search().expect("Search shows");
                        let mut controls = vec![
                            search.search,
                            search.search_tools,
                            search.match_label,
                            search.match_mode,
                            search.list,
                        ];
                        assert!(layout.dividers.len() <= DIVIDERS);
                        controls.extend(&layout.dividers);
                        if let Some(inspector) = &search.inspector {
                            controls.extend([
                                inspector.icon,
                                inspector.heading,
                                inspector.hide,
                                inspector.copy_path,
                            ]);
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
                        controls.extend(search.show_filters);
                        if let Some(filters) = &search.filters {
                            controls.extend([filters.title, filters.tools]);
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
                            search.list.height >= list_min
                                && search.list.width >= Dip(300).to_px(dpi)
                        );
                        let status = layout.status;
                        let [view, middle] = status.groups;
                        assert!(
                            view + middle < client.width,
                            "the status bar's fixed groups leave the right group room"
                        );
                        assert_eq!(status.memory.is_some(), show_memory);
                        if let Some(memory) = status.memory {
                            assert!(
                                view + middle + STATUS_REST_MIN_WIDTH.to_px(dpi)
                                    <= memory.service.x
                                    && memory.app.right() <= client.width - Dip(17).to_px(dpi),
                                "the memory group leaves the right group its room and stays \
                                 left of the size grip: {status:?} at {dpi:?}, stretch {stretch}"
                            );
                        }
                        // A running job's text, which the status bar draws
                        // about 5 px after the part's edge, its progress bar
                        // and Cancel fit the middle group side by side,
                        // inside the status bar.
                        let text_end = view + Dip(5).to_px(dpi) + captions.status.running;
                        assert!(
                            text_end <= status.progress.x
                                && status.progress.right() <= status.cancel.x
                                && status.cancel.right() <= view + middle,
                            "{status:?} at {dpi:?}, stretch {stretch}"
                        );
                        for control in [status.progress, status.cancel] {
                            assert!(
                                control.y >= Px(0) && control.bottom() <= status_height,
                                "{control:?} leaves the status bar at {dpi:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Two panels' controls split the window between them, so a control
    /// that left the client area or overlapped another, a list that took
    /// more than its half, or a key bar button too narrow for its caption
    /// and icon would show only when a person looks: at the minimum size, at
    /// a size with an odd width, at 150 %, and with the pseudo-language's
    /// longer captions.
    #[test]
    fn the_two_panels_share_the_window() {
        for (dpi, stretch, show_memory) in [(96, 1.0, false), (144, 1.0, true), (96, 2.0, true)] {
            let dpi = Dpi::new(dpi).expect("a DPI is not zero");
            // Key bar captions wide enough to set the minimum width, as
            // Spanish's do.
            let captions = captions(dpi, stretch, show_memory);
            let captions = CaptionWidths {
                key_bar: captions.key_bar.max(Dip(140).to_px(dpi)),
                ..captions
            };
            let font_height = Dip(15).to_px(dpi);
            let status_height = StatusLayout::min_height(dpi, font_height) + Dip(4).to_px(dpi);
            let compute = |client| {
                Layout::compute(
                    client,
                    dpi,
                    font_height,
                    status_height,
                    &captions,
                    Mode::TwoPanels,
                    &Panes::default(),
                )
            };
            let min = compute(Size::default()).min_client;
            for client in [
                min,
                Size {
                    width: Px(min.width.0 * 2 + 1),
                    height: Px(min.height.0 * 2),
                },
            ] {
                let layout = compute(client);
                let Body::TwoPanels { panels, key_bar } = layout.body else {
                    panic!("Two panels shows");
                };
                let panels = *panels;
                let [left, right] = panels.map(|panel| panel.list);
                // Each key bar button shows its caption whole after its
                // icon, also at the minimum width.
                let key_button =
                    button_room(button_content(captions.key_bar, captions.icons, dpi), dpi);
                assert!(
                    key_bar.iter().all(|button| button.width >= key_button),
                    "{key_bar:?} narrower than {key_button:?} at {dpi:?}, stretch {stretch}"
                );
                let mut controls: Vec<Rect> = key_bar.to_vec();
                controls.extend(layout.dividers.iter().copied());
                for panel in panels {
                    // The path row shows the path box or the search row,
                    // and the status line the counts and the free space or
                    // the halves of all drives, so each set is checked
                    // apart.
                    let sets = [
                        [panel.path, panel.counts, panel.free],
                        [panel.search, panel.counts_half, panel.folder],
                    ];
                    for set in sets {
                        let mut placed = vec![panel.drive, panel.list];
                        placed.extend(set);
                        if set[0] == panel.search {
                            placed.extend([panel.look_in_label, panel.look_in]);
                        }
                        fits(&placed, client, status_height, dpi, stretch);
                    }
                    // The message and Retry lie over the list, which is
                    // empty while they show: a "not ready" message of two
                    // lines and Retry under it fit in the list.
                    let area = MessageArea::new(panel.list, font_height + Dip(7).to_px(dpi), dpi);
                    let button = captions.message_button_width(dpi);
                    let (_, retry) =
                        area.place(Px(font_height.0 * 2), font_height, 1, button, true);
                    assert!(
                        retry[0].bottom() <= panel.list.bottom(),
                        "{retry:?} at {dpi:?}"
                    );
                    controls.extend([
                        panel.drive,
                        panel.path,
                        panel.list,
                        panel.counts,
                        panel.free,
                    ]);
                }
                for (i, a) in controls.iter().enumerate() {
                    assert!(
                        a.x >= Px(0)
                            && a.right() <= client.width
                            && a.bottom() <= client.height - status_height
                            && a.width > Px(0),
                        "{a:?} in {client:?} at {dpi:?}, stretch {stretch}"
                    );
                    for b in &controls[i + 1..] {
                        assert!(!overlaps(*a, *b), "{a:?} overlaps {b:?} at {dpi:?}");
                    }
                }
                for list in [left, right] {
                    assert!(list.width >= Dip(300).to_px(dpi), "{list:?} at {dpi:?}");
                }
                for panel in panels {
                    assert!(
                        panel.search.width >= Dip(120).to_px(dpi),
                        "{panel:?} at {dpi:?}"
                    );
                }
                assert_eq!(right.right(), client.width);
                assert!((right.width.0 - left.width.0).abs() <= 1);
                assert_eq!(layout.status.groups[0], Px(0));
                // The memory group shows as in Search, after the right
                // group, and Show details does not.
                assert_eq!(layout.status.memory.is_some(), show_memory);
                assert_eq!(layout.status.show_details, None);
                let parts = (layout.status.part_widths(&[]).into_iter())
                    .fold(Px(0), |sum, width| sum + width);
                assert!(parts <= client.width, "{parts:?} in {client:?}");
            }
        }
    }
}
