# Hand-drawn controls in Gozler's window

Branch `file-manager` at 9d46d3e, compared with main at 114d80c ("feat(ui):
use native Windows controls instead of drawn ones").

A hand-drawn control here is a control whose look our code paints: `WM_PAINT`,
owner draw (`WM_DRAWITEM`), or `NM_CUSTOMDRAW` that paints the control itself.
I read the committed code only. Nothing was built or run.

## Summary

| # | Control | Where (9d46d3e) | Native replacement | Mirrors 114d80c | Verdict |
| --- | --- | --- | --- | --- | --- |
| 1 | Key bar buttons `F3`–`F8` (branch) | `win/notify.rs` `Button::draw_key`, called from `app/main_window.rs` `Message::Notify` | the same `BS_PUSHBUTTON`, drawn by Windows, caption "F3 View", icon by image list | P2 | Switch |
| 2 | Status bar right group: index state and the file and folder totals with icons | `win/gdi.rs` `OwnerDraw::draw_status_parts` and `draw_line`; `SBT_OWNERDRAW` from `StatusBar::redraw_owner_drawn` | one status bar part per item, `SB_SETTEXTW` and `SB_SETICON` | P7 | Switch |
| 3 | Panel rows: focus bar, selected text, gray rows, `..` arrow (branch) | `win/notify.rs` `CustomDraw::draw_panel_row`; `app/main_window/panels.rs` `draw_panel_row` | the list view draws the row; the window sets font and colours at item prepaint (`CDRF_NEWFONT`) | P5 | Unsure |
| 4 | Viewer area (branch) | `win/viewer.rs` `paint_area` | none that fits | none | Keep |
| 5 | One-pixel dividers, in Search and in Two panels | `win/gdi.rs` `Hwnd::paint_window`; `core/layout.rs` `dividers` | `SS_ETCHEDHORZ` / `SS_ETCHEDVERT` static | P9 applies only inside a toolbar | Keep |
| 6 | Settings group boxes with an icon | `win/controls.rs` `show_group_glyph`, `paint_group` | plain `BS_GROUPBOX`, without the icon | P6 only by a trick | Keep |
| 7 | Location list | `win/controls.rs` `ListBox::create_paths`, `draw_list_item` | list box or list view, without a path ellipsis | P3 | Keep |
| 8 | Results list: matched parts in bold | `win/notify.rs` `CustomDraw::draw_cell`; `app/main_window.rs` `draw_cell` | none (one font per cell) | none | Keep |
| 9 | Settings lists: path columns | `win/notify.rs` `CustomDraw::draw_path`; `win/dialog.rs` | list view's end ellipsis | none | Keep |
| 10 | File operations window: the card's background | `win/file_operations.rs` `paint_card` | `SS_WHITERECT` plus a captionless group box | none | Keep |

Count: Switch 2, Unsure 1, Keep 7. Items 2 and 5 to 10 are drawn on main too.

Separately, 8 controls that 114d80c converted on main are still drawn on this
branch (see "Done on main").

## The pattern book: what 114d80c corrected

Each line is a hand-drawn thing on main before 114d80c, and the native control
that replaced it.

| Pattern | Hand-drawn before | Native after | How |
| --- | --- | --- | --- |
| P1 | Flat icon buttons in the main window, painted in `NM_CUSTOMDRAW` by `Button::draw_flat`: background, the theme's Toolbar hot and pressed look, the glyph, a focus rectangle | a toolbar (`win::toolbar::Toolbar`) | `TBSTYLE_FLAT \| TBSTYLE_LIST \| TBSTYLE_TOOLTIPS`; a glyph image list and a disabled image list; tooltips from `TBN_GETINFOTIPW`; access keys through `TB_MAPACCELERATORW`. A toolbar button takes no focus and is no Tab stop. |
| P2 | The same flat buttons in the File operations window and Settings' Clear | push buttons with an icon | `BCM_SETIMAGELIST` with `BUTTON_IMAGELIST_ALIGN_CENTER` (`Button::set_lone_icon`); they keep their Tab stops, as buttons in a dialog do. |
| P3 | Owner-drawn list boxes with an icon per item (`LBS_OWNERDRAWFIXED`, `draw_list_item`): the Kind list, Settings' page list | a list view (`IconList`) | `LVS_REPORT \| LVS_NOCOLUMNHEADER \| LVS_SINGLESEL`, the Explorer theme, a glyph image list as `LVSIL_SMALL`. |
| P4 | The File operations queue: selection painted with `DrawThemeBackground(LVP_LISTITEM)`, drag line painted with `FillRect` | the list view's own selection and insertion mark | `LVM_SETINSERTMARK`; the custom draw was removed. |
| P5 | A cell's text painted again in gray (`CustomDraw::draw_grey`) | the list draws the row in a font and colour that the window sets | at `CDDS_ITEMPREPAINT` set `clrText` and select the font, answer `CDRF_NEWFONT \| CDRF_NOTIFYPOSTPAINT`, restore the list's font at post-paint (`draw_row_struck`, `restore_font`). The list draws the icon, the alignment and the ellipsis. |
| P6 | Icons painted on a window's background: the inspector's file icon (`paint_window`), the File operations card's kind icon (`paint_card`) | static icon controls | `SS_ICON \| SS_CENTERIMAGE` with `STM_SETIMAGE` (`FileIconLabel`, `GlyphLabel`). |
| P7 | The memory group, an owner-drawn status bar part with icons | one status bar part per figure | `SB_SETTEXTW` and `SB_SETICON` (`StatusBar::set_icon`, `gdi::glyph_icon`), a tooltip area per figure. |
| P8 | Attributes, an owner-drawn static in two colours (`SS_OWNERDRAW`, `OwnerDraw::draw_parts`) | a read-only edit box, as the other values | The gray colour of Hidden and System was dropped. |
| P9 | One-pixel separators painted inside the search row | the toolbar's own separators | `BTNS_SEP` buttons. |

114d80c also turned Match case and Match path from check boxes into
`BTNS_CHECK` toggles. That is native to native, so it is no drawing pattern.
It moved `paint_card` from `WM_PAINT` to `WM_ERASEBKGND` and kept the card
drawn.

This branch does not have the memory group (P7), the search row's separators
(P9) or the struck rows (P5); they came to main after the branch point
(492175d). They are listed because they are patterns.

## Details

### 1. Key bar buttons: Switch

- **Where:** `win/notify.rs` `Button::draw_key` (about 87 lines). `app/main_window.rs`
  `MainWindow` `Message::Notify` sends each key bar button's `NM_CUSTOMDRAW` to
  it. The buttons are made by `Button::create_key` as plain `BS_PUSHBUTTON`.
- **What it does:** paints each button as a push button: `DrawThemeBackground`
  with `BP_PUSHBUTTON` and its states, or `DrawFrameControl` in high contrast.
  Then it writes the key in the bold font and the word in the normal font,
  centred, such as **F3** View.
- **Native:** stop answering `NM_CUSTOMDRAW`, so Windows draws the same push
  button. Its caption is already "F3 View" (`apply_panel_text` sets it for
  screen readers). This is P2.
- **What differs:**
  - The key is no longer bold. The bold key comes from the uncommitted
    prototype (`two-panels.prototype.html`, `.btn b { font-weight: 600 }`).
    The spec's "The key bar" only says "Each caption has the key and the word,
    as Total Commander's bar does".
  - Windows draws the hover, pressed, disabled and focus looks in every theme
    and in high contrast, instead of our copy of them.
  - DESIGN.md "Look" says an icon stands "before the caption of every push
    button, in the main window and in the dialogs ... through the button's own
    image list". The drawn key bar shows none. A native button gets it through
    the existing `Button::set_icon`. The key bar's width measure
    (`CaptionWidths::key_bar`) must then add the icon.
- **Written reason for drawing:** only the doc comment ("as Total Commander's
  key bar shows them"). DESIGN.md "Not planned" lists "custom-drawn buttons":
  "polish that needs a paint routine for every state costs more code than it
  gives".
- **Toolbar (P1) instead?** A toolbar takes no focus by itself, so the
  focus-return code in `on_control` could go. But a flat toolbar draws no
  border until the mouse is on a button. The spec and the prototype show
  bordered buttons. Push buttons are closer and are the smaller change.
- **Size:** about 100 lines removed (`draw_key`, the dispatch loop, the
  `fonts.bold` use there), about 15 added for icons and the width. 3 to 4 files:
  `win/notify.rs`, `app/main_window.rs`, `app/main_window/panels.rs`,
  `core/layout.rs`.

### 2. Status bar right group: Switch

- **Where:** `win/gdi.rs` `OwnerDraw::draw_status_parts` and `draw_line`.
  `app/status_bar.rs` `StatusBar::show` calls `redraw_owner_drawn(SERVICE_GROUP)`.
  `app/main_window.rs` answers `Message::DrawItem` for the status bar.
- **What it does:** draws one part with up to three items: the index state,
  then "1,192 files (3.4 GB)" and "12 folders (18.2 GB)", each after its icon,
  with " · " between them. The first item that does not fit ends in "…". In
  Two panels it shows the index state, or a search panel's totals.
- **Native:** P7, as 114d80c did for the memory group: one status bar part per
  item, text by `SB_SETTEXTW`, icon by `SB_SETICON`. 114d80c's
  `StatusBar::set_icon` and `gdi::glyph_icon` are the code to bring over.
- **What differs:**
  - The part edges replace the " · " separators.
  - The part widths must be measured and set (`SB_SETPARTS`) when the texts
    change. 114d80c's `StatusBar::set_parts` already returns whether the parts
    changed.
  - A click on a newer release must hit the index-state part only
    (`service_clicked`).
  - Not verified: whether a status bar part ends a long text in "…" as
    `draw_line` does.
- **Written reason for drawing:** DESIGN.md "Status bar": "The window draws this
  group, so its icons show." `SB_SETICON` shows the icons, so that reason no
  longer holds. 114d80c converted the memory group and kept this group drawn.
  No comment or DESIGN text on main says why.
- **Size:** about 150 to 250 lines touched in 5 files: `win/controls.rs`
  (`set_icon`), `win/gdi.rs` (`glyph_icon` in, `draw_status_parts` out),
  `app/status_bar.rs`, `core/layout.rs` (`StatusLayout` part widths),
  `app/main_window.rs` (the `DrawItem` branch out).

### 3. Panel rows: Unsure

- **Where:** `win/notify.rs` `CustomDraw::draw_panel_row` (about 72 lines) with
  `PanelRow`, `RowBar` and `RowFonts`; `win/gdi.rs` `inactive_bar_color`;
  `app/main_window/panels.rs` `draw_panel_row`, answered `DrawAnswer::Drawn`
  (`CDRF_SKIPDEFAULT`).
- **What it does:** paints the whole row. The active panel's focused row is a
  `COLOR_HIGHLIGHT` bar. The other panel's focused row is a gray bar, or a
  frame in high contrast. A selected row has `COLOR_HIGHLIGHT` text in
  semibold, or bold under the bar. Hidden and system entries are gray. The
  `..` row's folder icon gets an up arrow painted over it.
- **Owner ruling:** spec "Selecting entries": "The window draws the rows in
  `NM_CUSTOMDRAW` (owner, 2026-10-20, the prototype's look A)". The list
  view's own drawing is the prototype's look B (the selection filled, the focus
  a thin outline), which the owner did not choose. So a fully native row is
  out.
- **Native that keeps look A:** P5. At item prepaint, set `clrText`, set
  `clrTextBk` for the bar, select the semibold or bold font, and answer
  `CDRF_NEWFONT \| CDRF_NOTIFYPOSTPAINT`. The list then draws the icon, the
  cells, the right alignment, the ellipsis and the label tips itself.
- **Why unsure:** look A needs the list not to paint its own selection fill on
  `LVIS_SELECTED` rows, nor its focus rectangle. The panel lists use the
  Explorer theme (`ListView::create`). I do not know whether clearing
  `CDIS_SELECTED` and `CDIS_FOCUS` from `nmcd.uItemState` at item prepaint
  stops that. Clearing `LVIS_SELECTED` around each paint instead would send the
  selection notifications that the panel acts on (`ListNotify::SelectionChanged`
  calls `list_changed`).
- **What settles it:** one run. In an owner-data report list with the Explorer
  theme and `LVS_EX_FULLROWSELECT`, clear `CDIS_SELECTED | CDIS_FOCUS` at
  `CDDS_ITEMPREPAINT`, set `clrTextBk`, and look at a selected row and the
  focused row in a normal theme and in high contrast. If the selected row
  shows no fill and the focused row is filled across the row: switch. If not:
  keep the drawing.
- **What still needs painting after a switch:** the `..` arrow (a post-paint
  glyph, or an icon of its own) and the high-contrast frame of the inactive
  bar, because `clrTextBk` fills and cannot frame.
- **Size if it switches:** about 110 lines out of `win/notify.rs`, about 40 in;
  about 30 lines in `app/main_window/panels.rs`.

### 4. Viewer area: Keep

- **Where:** `win/viewer.rs` `paint_area`, `area_procedure`, `create_area`.
- **What it does:** paints the shown rows of a file in Consolas, the hex offset
  in the muted colour, on the window colour. The scroll bar is the window's
  standard one (`WS_VSCROLL`, `set_area_scroll`), which is native already.
- **Why no native control fits:** the spec ("The Viewer") requires that it
  "reads only the part it shows, in blocks of 64 KB" and that "The scroll bar
  maps to the byte position in the file".
  - An edit box or a rich edit box holds all its text. The window would replace
    the text at each scroll step. A plain edit box also has one text colour, so
    the muted offset goes.
  - An owner-data list view counts rows in an `i32`. A 50 GB file has about 3.3
    billion hex rows, and the number of text rows is unknown until the file is
    read.
- It is a content view, not a copy of a Windows control.

### 5. One-pixel dividers: Keep

- **Where:** `win/gdi.rs` `Hwnd::paint_window` fills `Layout::dividers` with
  `COLOR_3DLIGHT`. The branch adds four in Two panels
  (`Layout::compute_two_panels`): between the panels, above the key bar, and
  above each panel's status line.
- **Native:** `SS_ETCHEDHORZ` and `SS_ETCHEDVERT` draw a 2-pixel 3D line.
  `SS_GRAYRECT` uses the desktop colour. P9 works only where a toolbar already
  stands.
- **Written reason:** DESIGN.md "Look": "One-pixel dividers separate the panes
  instead of 3D edges." main keeps them too.

### 6. Settings group boxes with an icon: Keep

- **Where:** `win/controls.rs` `show_group_glyph` subclasses the group box, and
  `paint_group` paints the whole box: the frame by the theme (or etched), the
  icon, and the caption.
- **Written reason:** its doc comment: "Windows draws a group box's caption
  itself, so the whole box is drawn here instead". The owner ruled on
  2026-10-20 that icons go only where a control supports them, and also that
  "The icons that are already built stay".
- **Native:** a plain group box has no icon slot, so the icon goes, which is the
  owner's decision. The P6 way to keep it puts a static icon over the frame and
  pads the caption with spaces, which depends on the font and the DPI.
- main keeps it drawn.

### 7. Location list: Keep

- **Where:** `win/controls.rs` `ListBox::create_paths` (`LBS_OWNERDRAWFIXED`) and
  `draw_list_item`, called from `win/pane.rs`.
- **Written reason:** "A path wider than its row loses its middle
  (`DT_PATH_ELLIPSIS`), so the drive and the last folder stay visible, and a
  tooltip shows it whole."
- **Native:** neither a list box nor a list view (P3) has a path ellipsis. main
  keeps it drawn. When P3 is brought over for the Kind and Settings page lists,
  `draw_list_item` serves only this list, and its icon branch is no longer
  used.

### 8. Results list, matched parts in bold: Keep

- **Where:** `win/notify.rs` `CustomDraw::draw_cell`; `app/main_window.rs`
  `draw_cell`. Only cells with a match are drawn; the list draws the others.
- **Written reason:** DESIGN.md "Shared code": "the window asks only for the
  visible rows and draws them bold with `NM_CUSTOMDRAW`".
- **Native:** a list view cell has one font, so P5 cannot do it. main keeps it.

### 9. Settings lists, path columns: Keep

- **Where:** `win/notify.rs` `CustomDraw::draw_path`; `win/dialog.rs`
  `NM_CUSTOMDRAW` for the columns `path_column` names.
- **Written reason:** "its text, a path, loses its middle where it does not fit
  (`DT_PATH_ELLIPSIS`), so the drive and the name stay. The cell keeps its
  whole text, which the list's label tip shows."
- **Native:** the list view only ends a text in "…". A shortened text given by
  `LVN_GETDISPINFO` would also shorten the label tip. main keeps it.

### 10. File operations card: Keep

- **Where:** `win/file_operations.rs` `paint_card`.
- **What it does:** fills the window in the dialog colour and the card in the
  window colour, with a 1-pixel border and rounded corners.
- **Native:** `SS_WHITERECT` fills with the window colour but has no border. A
  captionless group box draws a frame but no fill. Two controls stacked to copy
  one rectangle are not simpler. 114d80c kept it drawn and only moved it to
  `WM_ERASEBKGND`, so that themed buttons paint their corners on it. (The card's
  icon is item 7 of "Done on main".)

## Checked and native already

The branch parts that the owner named, other than items 1, 3 and 4:

- **Panels' status lines:** static controls (`Label::create_line`,
  `Label::create_right_line`, `SS_ENDELLIPSIS`). Only the line above them is
  painted (item 5).
- **Quick search box:** a read-only edit box (`Edit::create_echo`).
- **Drive button and drive menu:** `BS_SPLITBUTTON` with `BCSS_NOSPLIT`, and a
  popup menu (`Menu::track_popup_highlighted`, which posts `Down` keys and
  draws nothing).
- **Path box:** an edit box. The active panel's highlight colours come from
  `WM_CTLCOLOREDIT` (`StaticColors::highlight`), the standard colour message.
- **A panel's message and Retry:** a static control and a push button.
- **Viewer status bar and scroll bar:** a status bar control with text parts,
  and the window's standard scroll bar.
- **Copy, Move and New folder dialogs** (`app/dialogs.rs`): dialog templates;
  no drawing.
- **Key bar tooltips:** a tooltip control with rectangle tools
  (`Tooltip::add_area`).

## Done on main

114d80c converted these on main. They are still drawn on this branch, because
main is not merged here.

| Control on this branch | Where (9d46d3e) | Native on main (114d80c) | Pattern |
| --- | --- | --- | --- |
| Inspector's Copy name, Copy folder and Hide details; filter pane's Hide filters and Show filters; status bar's Show details | `Button::draw_flat`, called from `app/main_window.rs` `Message::Notify` | toolbars (`win::toolbar`) | P1 |
| File operations window's Pause, Cancel, queue toggle, Run, Up, Down and queue Cancel | `Button::draw_flat`, called from `app/file_operations.rs` `Message::Notify` | push buttons with a centred icon (`Button::set_lone_icon`) | P2 |
| Kind list | `ListBox::create_with_glyphs`, `draw_list_item` | list view (`IconList`) | P3 |
| Settings page list | `ListBox::of_dialog`, `set_glyphs`, `draw_list_item` | list view (`IconList`) | P3 |
| File operations queue's selection fill and drag line | `win/file_operations.rs` `QueueList::draw`, `draw_selected` | the list's own selection and `LVM_SETINSERTMARK` | P4 |
| Inspector's file icon | `Hwnd::paint_window` | static icon (`FileIconLabel`) | P6 |
| File operations card's kind icon | `paint_card` | static icon (`GlyphLabel`) | P6 |
| Inspector's Attributes in two colours | `Label::create_owner_drawn`, `OwnerDraw::draw_parts` | read-only edit box | P8 |

114d80c changes 28 files (+2084, −1583). It also changes code that this
branch does not have: the memory group, the search row's separators, the
struck rows and File > Cancel.
