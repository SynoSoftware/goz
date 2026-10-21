# Hand-drawn controls after the merge

What I reviewed: `main` at 95ba03a merged with `file-manager` at 86bdfe4 and
the key bar commit cb2466d. I built that merge as a tree with
`git merge-tree` (main against 86bdfe4 + cb2466d, base 492175d) and read it
and the three sides. Nothing was checked out, built or run.

main has no `crates/gozler-ui` commit after 114d80c, so "what main added
after 114d80c" is empty.

The owner's ruling of 2026-10-21 applies to every verdict: no custom drawing
for little to show. Setting a font or a colour at item prepaint is allowed;
painting is not. "Keep" means the drawing shows something the person needs
that no native control can show at all.

## Summary

| # | What is drawn | Where (merged code) | Native that gives almost the same | Verdict |
| --- | --- | --- | --- | --- |
| 1 | Panel rows: focus bar, selected text, gray rows, cells | `win/notify.rs` `CustomDraw::draw_panel_row`; `app/main_window/panels.rs` `MainWindow::draw_panel_row` | the list view's own row (look B); font and colour at item prepaint | **Convert** (owner ruling) |
| 2 | `..` arrow over the folder icon | inside `CustomDraw::draw_panel_row` (`draw_lone_glyph`) | an icon of its own in the list's image list | **Convert** (owner ruling) |
| 3 | Search panel's Folder column, middle ellipsis | `CellFit::Path` in `CustomDraw::draw_panel_row`; `panels.rs` `fit_of` | the list's end ellipsis and label tip | **Convert** (owner ruling) |
| 4 | One-pixel dividers (Search: filter pane edge, inspector edge; Two panels: middle, above the key bar, above each status line) | `win/gdi.rs` `Hwnd::paint_window`; `app/main_window.rs` `Message::Paint`; `core/layout.rs` `dividers` | an empty static per divider, coloured by `WM_CTLCOLORSTATIC` | **Convert** (missing) |
| 5 | Settings group boxes with an icon | `win/controls.rs` `show_group_glyph`, `group_procedure`, `paint_group`; `app/settings.rs` | plain `BS_GROUPBOX` | **Convert** (missing) |
| 6 | Location list (filter pane) | `win/controls.rs` `ListBox::create_paths`, `draw_list_item`, `PathTip`, `path_list_procedure`; `win/pane.rs` | list view as the Kind list (`IconList`), with its label tip | **Convert** (missing) |
| 7 | File operations' Failed list, path column | `win/notify.rs` `CustomDraw::draw_path`; `win/dialog.rs` `NM_CUSTOMDRAW`; `app/file_operations.rs` `path_column` | the list's end ellipsis and label tip | **Convert** (missing) |
| 8 | File operations window's card: white rounded panel with a border | `win/file_operations.rs` `paint_card`; `app/file_operations.rs` `Message::EraseBackground` | no card, or a group box | **Convert** (missing) |
| 9 | Results list: matched parts in bold | `win/notify.rs` `CustomDraw::draw_cell`; `app/main_window.rs` `MainWindow::draw_cell` | none: a list view cell has one font | **Keep** (checked) |
| 10 | Viewer area | `win/viewer.rs` `paint_area`, `area_procedure`; `app/viewer.rs` `Viewer::paint` | none that is a view over a whole file | **Keep** (checked) |
| 11 | Status bar right group: index state and totals with icons, also a search panel's totals in Two panels | `win/gdi.rs` `OwnerDraw::draw_status_parts`; `app/main_window.rs` `Message::DrawItem` | status bar parts with `SB_SETICON` | **Covered** (#109, open) |
| 12 | Key bar `F3`–`F8` | was `Button::draw_key` | push buttons Windows draws | **Covered** (cb2466d) |
| 13–20 | The 8 controls 114d80c converted that the branch still drew: flat icon buttons (2 groups), Kind list, Settings page list, queue selection and drag line, inspector icon, card icon, Attributes | see the first report's "Done on main" | toolbars, lone-icon push buttons, `IconList`, `LVM_SETINSERTMARK`, `FileIconLabel`, `GlyphLabel`, read-only edit | **Covered** (114d80c), with 3 merge hunks to resolve to main's side (below) |

Counts: convert 8 (3 of them decided by the owner, 5 missing), keep 2,
covered 10.

Native already, checked: the search panel's search box (edit), Look in
(drop-down list), the focused result's folder (`Label::create_right_path`,
`SS_PATHELLIPSIS` static), the panel message (`Label::create_centred`) and
Retry, the status lines, the quick search box, the drive split button and
menu, the path box's highlight (`WM_CTLCOLOREDIT`, colour only), the Viewer's
status bar (two text parts), message and Retry, the scroll bars, the
toolbars, the dialogs, the tooltips. Results rows struck through
(`CustomDraw::draw_row_struck`) only set a font and a colour, which the ruling
allows. Menu icons are bitmaps in native menu items, owner-approved. The
glyph renderers in `win/gdi.rs` (`glyph_list`, `glyph_icon`, `button_images`,
`menu_bitmap`) make images for native controls and paint no control.

## Details

### 1. Panel rows: convert (owner ruling)

- **Now:** at `CDDS_ITEMPREPAINT` the window fills the row
  (`COLOR_HIGHLIGHT` bar, a gray bar from `inactive_bar_color`, or a frame in
  high contrast), draws the icon with `ImageList::draw`, and every cell with
  `DrawTextW`, then answers `CDRF_SKIPDEFAULT`.
- **Native:** the list draws the row: selection fill and focus outline in the
  Explorer theme (look B). At item prepaint the window may still set
  `clrText` (gray for hidden and system entries, as `draw_row_struck` does)
  and a font (semibold for selected rows, the struck font for taken results).
- **What goes:** `CustomDraw::draw_panel_row`, `PanelRow`, `RowBar`,
  `RowFonts`, `CellFit`, `gdi::inactive_bar_color`, `panels.rs`
  `draw_panel_row` and `fit_of`. The spec's "Selecting entries" bullet "The
  window draws the rows in `NM_CUSTOMDRAW` (… look A)" must change to look B.
- **What the person sees differ:** selected rows are filled, as in File
  Explorer, instead of blue semibold text; the focused row is an outline,
  not a bar.
- **The one run that settles the rest:** reading cannot say how look B shows
  the focused row, because the list draws it. Run the converted panels once,
  in a normal theme and in high contrast, and look at the focused row
  (a) in the active panel before any key is pressed, and (b) in the inactive
  panel. A list view draws its focus rectangle only while it has the focus,
  and Windows hides focus rectangles until the keyboard is used, so (b), and
  possibly (a), may show no cursor at all, while the prototype's look B shows
  a gray outline in the inactive panel. If (b) shows nothing, setting
  `clrTextBk` to a gray on that row at item prepaint is within the ruling;
  the same run shows whether the Explorer theme fills the whole row with it.
- **Merge note:** the merged spec (main's text) says a search panel's taken
  result "shows struck through, in grey"; the branch's panel shows taken rows
  blank and has no struck path. After the conversion, main's
  `draw_row_struck` gives the struck look natively.

### 2. `..` arrow: convert (owner ruling)

- **Now:** `draw_panel_row` paints `Glyph::Up` over the folder icon with
  `draw_lone_glyph`.
- **Merge fact:** 114d80c deleted `gdi::draw_lone_glyph`, so the merged code
  calls a function that no longer exists. The conversion removes the call;
  do not restore the function.
- **Native:** the panel lists use the shared system small image list
  (`ImageList::system_small`, `LVS_SHAREIMAGELISTS`), so the `..` icon needs
  an index in that list, or an overlay image. `gdi::glyph_icon` already makes
  an `HICON` from a glyph.

### 3. Folder column's middle ellipsis: convert (owner ruling)

- **Now:** `CellFit::Path` draws the Folder cell with `DT_PATH_ELLIPSIS`.
- **Native:** the list ends the text in "…"; `ListView::create` sets
  `LVS_EX_LABELTIP`, so a cut-off path shows whole in a tip.
- **For the owner, because it bears on the decision:** a native list can
  still lose the middle of a path without drawing: the window gives the
  shortened text in `LVN_GETDISPINFO`, as main already gives shortened paths
  to static controls with `TextMeter::compact_path` (`app/main_window.rs`,
  `app/file_operations.rs`). The cost: the label tip then never shows,
  because the text fits, and screen readers read the shortened path. The
  same choice applies to items 6 and 7.

### 4. Dividers: convert (missing)

- **Now:** `Hwnd::paint_window` fills each `Layout::dividers` rectangle with
  `COLOR_3DLIGHT` in the main window's `WM_PAINT`. Search has two (filter
  pane edge, inspector edge), Two panels four (between the panels, above the
  key bar, above each status line).
- **Native:** an empty `STATIC` per divider, placed at the same rectangle,
  whose `WM_CTLCOLORSTATIC` answer is a `COLOR_3DLIGHT` brush; Windows
  documents that the static paints its background with that brush. This is
  a colour, which the ruling allows. `SS_ETCHEDHORZ`/`SS_ETCHEDVERT` would
  draw a 2-pixel 3D line, against DESIGN.md "Look" ("One-pixel dividers …
  instead of 3D edges").
- **What the person sees differ:** nothing with the coloured static.
- **What goes:** `Hwnd::paint_window` and the main window's `Message::Paint`
  branch. The old reason (DESIGN.md "Look") is about the look, which the
  static keeps; it does not require painting.

### 5. Settings group boxes with an icon: convert (missing)

- **Now:** `show_group_glyph` subclasses each Settings group box, and
  `paint_group` paints the whole box in `WM_PAINT`: the themed frame
  (`DrawThemeBackground`, `BP_GROUPBOX`) or an etched edge, the icon
  (`draw_glyph`) and the caption.
- **What the drawing shows:** an icon before the caption. The caption
  already names the group, so the person needs nothing that a plain group
  box cannot show.
- **Native:** `BS_GROUPBOX` as the template declares it; the icon goes.
- **Earlier ruling:** on 2026-10-20 the owner said icons go only where a
  control supports them, and also that the icons already built stay. The
  2026-10-21 ruling is later and is about drawing; this verdict follows it.
  DESIGN.md "Look" ("before each Settings group box's caption") and "Show
  icons" ("plain group captions") change with it.

### 6. Location list: convert (missing)

- **Now:** an `LBS_OWNERDRAWFIXED` list box. `draw_list_item` fills each row
  (`COLOR_HIGHLIGHT` when selected), draws the path with `DT_PATH_ELLIPSIS`
  and a `DrawFocusRect`. A hand-built tooltip (`PathTip`, the
  `path_list_procedure` subclass, `shortened`) shows a cut-off path whole.
- **The first report's keep reason** ("neither a list box nor a list view has
  a path ellipsis") does not pass the ruling: the owner gave up the middle
  ellipsis for the Folder column, and item 3's compacted text keeps it
  natively if wanted.
- **Native:** a list view built as the Kind list above it (`IconList`, P3)
  with `LVS_EX_LABELTIP`, whose label tip replaces `PathTip`.
- **What the person sees differ:** the selected location shows the Explorer
  theme's selection, as the Kind list in the same pane already does, instead
  of a solid `COLOR_HIGHLIGHT` row; a long path ends in "…" (or keeps its
  middle ellipsis by compacted text).
- **What goes:** `draw_list_item`, `path_text_rect`, `PathTip`,
  `path_list_procedure`, `shortened`, `ListBox::set_item_height` if nothing
  else uses it, and `win/pane.rs`'s `WM_DRAWITEM` branch.

### 7. Failed list, path column: convert (missing)

- **Correction to the first report:** its item 9 says "Settings lists". The
  only list with a path column is the File operations window's Failed list:
  `app/file_operations.rs` `path_column` returns column 0 for
  `ControlId::FailedList`, and `DialogProc::path_column` returns `None` for
  every other dialog.
- **Now:** `win/dialog.rs` answers that list's `NM_CUSTOMDRAW` and
  `CustomDraw::draw_path` draws the cell's text with `DT_PATH_ELLIPSIS`.
- **Native:** the list's end ellipsis; `ReportList` sets `LVS_EX_LABELTIP`,
  so a cut-off path shows whole in a tip.
- **What goes:** `CustomDraw::draw_path`, `CustomDraw::cell_text`,
  `DialogProc::path_column` and the `NM_CUSTOMDRAW` branch in `win/dialog.rs`.

### 8. File operations card: convert (missing)

- **Now:** `paint_card`, in `WM_ERASEBKGND`, fills the window in
  `COLOR_BTNFACE` and draws a `RoundRect` in `COLOR_WINDOW` with a one-pixel
  `COLOR_3DLIGHT` border (`COLOR_WINDOWTEXT` in high contrast). The card's
  labels then ask for the window colour in `WM_CTLCOLORSTATIC`.
- **What the drawing shows:** a white panel behind the running operation.
  It is decoration; no spec or DESIGN.md text asks for it.
- **Native:** the running operation's controls on the window's own
  background, or inside a group box when a frame is wanted.
- **What the person sees differ:** no white rounded card.
- **What goes:** `paint_card`, `CARD_CORNER`, the `EraseBackground` branch,
  and the card labels' window-colour case in `CtlColorStatic`.

### 9. Results list, matched parts in bold: keep

- **What the person needs from it:** which part of each name or path matched
  the typed text (DESIGN.md "Shared code": "draws them bold with
  `NM_CUSTOMDRAW`").
- **Why no native control shows it:** a list view cell has one font and one
  colour; item prepaint sets them for the whole row. Only cells with a match
  are drawn; the list draws all other cells.

### 10. Viewer area: keep

- **What the person needs from it:** any part of a file of any size, at
  once, with the arrow keys, the wheel and the scroll bar acting on the whole
  file (spec "The Viewer": reads only the part it shows; the scroll bar maps
  to the byte position).
- **Why no native control shows it:** an edit box or a rich edit box holds
  its whole text. Fed only the shown rows, its own caret, selection,
  `Ctrl+A`, `Ctrl+End` and arrow keys would act on those rows, not on the
  file, so the window would have to intercept them all. An owner-data list
  view counts rows in an `i32`, which a hex view of a file over 32 GB passes.
  The scroll bar is already the window's standard one.

### 11–12. Covered

- **Status bar right group (#109, open):** in Two panels the same drawn part
  shows the index state or a search panel's totals, through the same
  `draw_status_parts` call, so #109's fix covers both modes. After #109 and
  item 5, nothing calls `gdi::draw_glyph`.
- **Key bar (cb2466d):** see the next section.

## The key bar commit cb2466d

It removed all of the key bar's drawing:

- `Button::draw_key` is gone, and with it the key bar's imports
  (`BP_PUSHBUTTON`, `PBS_*`, `DrawFrameControl`, `DFC_*`/`DFCS_*`,
  `DrawThemeParentBackground`, `COLOR_BTNTEXT`).
- The `NM_CUSTOMDRAW` loop over `controls.key_bar` in `main_window.rs` is
  gone. No other code answers a key bar button's notifications.
- The comments "the window draws the button" (`panels.rs`) and "Its parent
  draws it" (`Button::create_key`) are gone. The spec never asked for a bold
  key.
- Nothing is left over: `fonts.bold` is still used by the results' bold
  matches and the panel rows; `captions::key_bar` gives the captions and
  tips; `CaptionWidths` measures "F3 View" in the plain font, as Windows now
  draws it.
- The imports still in cb2466d's `notify.rs` that look like button drawing
  (`DrawThemeBackground`, `OpenThemeData`, `TP_BUTTON`, `TS_HOT`,
  `TS_PRESSED`, `CDIS_*`, `COLOR_BTNFACE`, `DrawFocusRect`, `InflateRect`,
  `disabled_color`) are each used only by `Button::draw_flat`, the branch's
  flat icon buttons, which 114d80c removed on main.

## Merge hunks that would bring drawn code back

Each is a conflict whose branch side carries drawing that 114d80c removed.
Take main's side for the drawing.

- **`win/notify.rs`, the imports:** the branch side lists `Button`'s
  `draw_flat` imports (above) and `draw_lone_glyph`, which no longer exists.
- **`win/controls.rs`, after `Label::create_heading`:** the branch side adds
  `create_right_path` and `create_centred`, which stay, and
  `create_owner_drawn` (`SS_OWNERDRAW`), which only the old Attributes value
  used; drop it.
- **`app/main_window.rs`, `Message::Paint`:** the branch side paints the
  inspector's icon in `paint_window`; main shows it in a `FileIconLabel`.
  Take main's side (and then item 4 removes the handler).
