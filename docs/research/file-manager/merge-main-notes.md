# Notes for the merge of `main` into `file-manager`

Basis: `git merge-tree --write-tree main file-manager` on the committed branch (9d46d3e, stages 3-6),
base 492175d. Nothing was built. "Merged tree" below means that in-memory result.
Merge-tree reports 17 conflicted files. Many more compile errors hide in files that merge
without a conflict (section 3A).

The working tree holds uncommitted stage 7 edits (20 files, among them `core/model.rs`,
`core/panel.rs`, `core/results.rs`, `core/captions.rs`, `core/commands.rs`, `core/layout.rs`,
`app/main_window.rs`, `docs/DESIGN.md`, and the new `core/search_list.rs`). They are not in
this analysis. Section 3E lists what stage 7 changes under main's new code.

## 1. What main's 7 commits change (oldest first)

**e2df95a "Drop files on folder rows, strike through taken rows and speed up Match path"** (47 files)
- Drop target for the Search list: new `core/drop_target.rs`, `win/drop_target.rs`
  (`DropHandler`, `AsyncDrop`, `DropRegistration`). `MainWindow` implements `DropHandler`
  (`drag_enter`, `drag_over`, `hold_still`, drop) with fields `drop_target`, `drag_in`
  (`DragIn`, `DropFolder`); timer `DROP_SCROLL`. `app/file_operations.rs` gets
  `FilesSource::{Paste(ClipboardFiles), Drop(AsyncDrop)}` and `Window::drops`. `start()` takes
  `Option<FilesSource>` instead of `Option<ClipboardFiles>`. `MainWindow::transfer()` is new.
  `DispInfo::wants_state`, `set_drop_target`; `ListView::create` sets `LVM_SETCALLBACKMASK`
  for `LVIS_DROPHILITED`; `drag_scroll_edge`, `scroll_row`; `ScrollEdge`.
- Taken rows are struck through: `DrawStage::Row` became `DrawStage::Row { row }`; new
  `DrawStage::RowPainted`, `DrawAnswer::NewFont`, `CustomDraw::draw_row_struck`,
  `restore_font`, `Font::struck_out`, `MainWindow::draw_row`.
- Settings > General: "Show sizes in" (`text::SizeFormat`; `text::cell(strings, numbers,
  sizes, column, row)` gained the `sizes` argument) and "Show memory use in the status bar"
  (`MemoryCaptions`, `MemoryWidths`, `MemoryLayout`, `WindowEnd`, `Model::app_working_set`,
  `service_working_set`, `polls_status(on_screen)`, `Tooltip::create_areas`, status parts 3
  and 4, `StatusBar::show_memory`). Settings > Reset all (`reset_and_start_again`).
- Keyboard: Tab goes only between the search box and the results. New `TabStop`,
  `KeyAction::MoveTab`, `tab_stop`. The Keyboard page takes the Windows key through a hook:
  new `DialogMessage::Focus`, `DialogMessage::KeysCaptured`, `HotkeyControl::take_any_key`,
  `ShowShortcut::pressed`; removed `ShowRefusal`, `ShowShortcut::two_modifiers`,
  `windows()`, `keystroke()`, `require_two_modifiers`. `command_rows` gained `show`.
- `CommandId::CancelExport` was renamed to `CommandId::Cancel` (`#[serde(alias =
  "cancel_export")]`, catalogue key `command.cancel`). File > Cancel is a normal item after
  Export results…. `Model::can_cancel_job`, `cancel_job` (in the next commit too).
  `export_slot` and `Menu::replace_item` are gone.
- Glyph set: removed `ChromeClose`, `ChromeMinimize`, `Clear`; added `Component`, `EraseTool`,
  `Favicon`, `Globe`. `Cancel` now serves Close, Exit and Cancel. `Remove` serves Hide.
  `gdi::draw_glyph` now copies a kept alpha bitmap (`AlphaBlend`) and glyphs are smoothed
  (4x oversample).
- The window's work is named "App" in `CONTEXT.md` (new term **App**; **Window** now means the
  main window).
- `gozler-core`: Match path speed-up (`query/engine.rs`, `paths.rs`, bench). Branch has no change there.
- `Cargo.toml`: thin LTO release profile, `Win32_System_SystemServices`.

**763bee1 "divide the search row's groups with separators"**: `core/layout.rs` only. Obsolete
after 114d80c, which has the toolbar draw its own separators.

**13d3c3a installer**: `installer/Setup.iss`, a comment in `app/mod.rs`, DESIGN.md. Independent.

**6467f00 release 0.4.0 / 67f44ec README**: version bump (Cargo.toml, Cargo.lock, AGENTS.md
intro text, README). Independent.

**a2bd89a** `*.dll` match by name with `-p`: `gozler-core` only. Independent.

**114d80c "use native Windows controls instead of drawn ones"** (28 files, +2084 -1583)

Drawn control -> native control:

| Was | Is |
| --- | --- |
| Search row: Clear, Match case, Match path as `Button`/`CheckBox` | one `Toolbar` (`Controls::search_tools`, `IDC_SEARCH_TOOLS`), with `Tool::Icon/Check` buttons; Windows draws hover, pressed, checked, disabled and the group separators |
| Filter pane: `clear`, `hide` buttons | `FilterLayout::tools` + `IDC_FILTER_TOOLS` toolbar |
| Show filters, inspector copy icons, Hide details, status bar Show details: `Button` + `draw_flat` | `Toolbar` of one `Tool::Icon` each (`win/toolbar.rs`, new) |
| File operations window icon buttons: `Button::draw_flat` + `FlatGround` | push buttons with `Button::set_lone_icon` |
| Settings Clear | push button with an icon |
| Kind list and Settings page list: owner-drawn `ListBox` (`create_with_glyphs`, `set_glyphs`, `fit_glyph_rows`) | `IconList` (list view, `win/controls.rs`) |
| Inspector icon and File operations card icon: painted in `WM_PAINT` | static icon controls: `FileIconLabel`, `GlyphLabel` (`IDC_INSPECTOR_ICON`, `IDC_FILE_OPERATIONS_ICON`) |
| Inspector Attributes row: owner-drawn `Label` + `FieldValue::Parts` + `ATTRIBUTES_ROW` | read-only `Edit` like the other values |
| Status bar memory figures: owner-drawn | status bar parts 3 and 4 with `SB_SETICON` (`StatusBar::set_icon`) |
| File operations queue selection and drop line: custom draw (`ListDraw`, `draw`, `draw_selected`) | the list's own selection; `QueueList::set_insert_mark` |
| Dialog erase/paint of the card: `paint_card(window, card, corner, icon)` + `ERASE_BACKGROUND` | `Message::EraseBackground(EraseDc)`; `paint_card(window, dc, card, corner)` |

Removed (exact names): `FlatGround`, `Button::draw_flat`, `Button::draw_grey`,
`Label::create_owner_drawn`, `Edit::left_margin`, `ListBox::create_with_glyphs`,
`ListBox::set_glyphs`, `ListBox::fit_glyph_rows`, `layout::glyph_row_height`,
`layout::glyph_room`, `layout::glyph_list_item_width`, `FilterLayout`'s `clear_is_icon`,
`InspectorLayout::drawn_value`, `ERASE_BACKGROUND`, `ListDraw`, `QueueNotify::CustomDraw`,
`QueueList::draw`, `OwnerDraw::draw_parts`, `OwnerDraw::status_part`,
`OwnerDraw::draw_status_slots`, `Menu::replace_item`, `commands::export_slot`,
`captions::ATTRIBUTES_ROW`, `FILL_SETTLE_MS` and `timer::FILL`, `gdi::draw_lone_glyph`,
`Hwnd::placed_rect` (now `is_placed_at`), `cancel_shown` argument of `tab_stop`, the
`TabStop::Cancel` variant.

Added: `win::toolbar::{Tool, Toolbar, TipRequest, press_access_key}`, `controls::IconList`,
`GlyphLabel`, `FileIconLabel`, `StatusBar::set_icon`, `StatusBar::set_parts -> bool`,
`ListView::shows_part_row`, `gdi::EraseDc`, `gdi::glyph_list`, `gdi::glyph_icon`,
`gdi::lone_button_images`, `Icon::into_handle`, `ImageList::icon`, `Hwnd::is_minimized`,
`Message::{EnterSizeMove, ExitSizeMove, EraseBackground}`, `ControlEvent::id`,
`layout::{ControlSizes, ToolHeights}`, `CaptionWidths::measure(captions, measure,
ControlSizes)` (third argument), `CaptionWidths::{inspector_button_width,
message_button_width}` (replace `button_width`), `CaptionWidths` fields `inspector_buttons`,
`message_buttons`, `status_cancel` (replace one shared `buttons`), `Fixer::can_cancel`,
`Model::{can_cancel_job, cancel_job}`, `MainWindow::{toolbars, icon_tools, sizes, end_sizing,
fits_of, command_button_clicked, on_kind_selected, draw_row}`. `Metrics::measure` takes
`&Controls`. `fix_status` returns `(String, JobProgress)`; `JobCaption.cancel` comes from
`Model::can_cancel_job`.

Behavior: the window message loop presses a visible toolbar's access key
(`toolbar::press_access_key`) before `IsDialogMessageW`. The status bar's Cancel is no longer a
Tab stop. Esc in the results list runs Cancel. During a border drag the list is not filled at
each step; columns are fitted when the drag ends (`sizing`, `sizing_fits`, `EnterSizeMove`).
`File > Cancel` stops an export or a service fix.

## 2. The 17 conflicted files

"Lines" = the same lines edited, with no clash of meaning. "Meaning" = the two sides change
the same behavior.

| File | Main | Branch | Overlap |
| --- | --- | --- | --- |
| `resources/ids.h` | adds `IDC_FILE_OPERATIONS_ICON 3026` at the end | adds `IDC_VIEWER_*` 3501-3504 at the end | Lines. Keep both. No new number clash (`IDD_ABOUT` 100 and `IDC_KEY_F8` 100 are in different ID spaces and are already on the branch). |
| `app/file_operations.rs` (6 hunks) | `FilesSource`, `drops`, `dropped.end(...)` in `report`, `End` import, `GlyphLabel` icon, `kept` unchanged in design | `Destination`, `reported`/`ended`/`told`/`last_ended`, `tell_ended`, `take_ended`, `start()` returns `OperationId`, `report(&Arc<Request>, &Arc<Outcome>)`, `OPERATION_CHANGED`, `Choice` import | Meaning: `kept` (main, unchanged from base) and `reported` (branch) are the same idea; keep the branch's. `start()` must take `Option<FilesSource>` and return `OperationId`. `report` must keep both `drops` handling and the branch's `Arc` signature. Imports: drop `ERASE_BACKGROUND`, keep `OPERATION_CHANGED`; union of `End` and `Choice`. The branch's `FlatGround`/`draw_flat` lines (unchanged base) go away with main's deletion. |
| `app/main_window.rs` (20 hunks) | see below | see below | see below |
| `app/status_bar.rs` | `show_memory`, `memory_tips`, `place(layout, memory)`, `Toolbar` Show details, `show_details()` returns `Option<Toolbar>` | `show(captions, job_text)`, `show_work(job, job_text, service)`, `show_job(job, text)` | Lines. Merge: call `show_memory` then `show_job(&captions.job, job_text)`. |
| `core/captions.rs` | `fix_status` returns `(String, JobProgress)`; `JobCaption.cancel = model.can_cancel_job()` | `Job::Operation => operation_status(...)`; `view` empty in Two panels; `fix_status` returns `JobCaption` with its own `cancel` | Meaning: two ways to decide "Cancel shown for a fix". Main's `Fixer::can_cancel` and the branch's per-step flag do the same. Pick one. `operation_status` must set `cancel` the same way as `Model::can_cancel_job`. |
| `core/commands.rs` (3 hunks) | `C::Cancel` arm; File group `[ExportResults, Cancel]`; test without Cancel special case | glyph arms `View`, `Edit`, `NewFolder`; Two panels File group; test chains `menu_bar(Mode::TwoPanels)` | Meaning for `Cancel`. Use `Cancel` in the arm list (the branch's `C::CancelExport` is gone). Add `C::Cancel` after `ExportResults` in the File group of both modes. Test: iterate both bars, no special case. Also the merged tree still has `C::CancelExport` at `commands.rs` ~345 (not in a hunk). |
| `core/glyph.rs` | removes 3 glyphs, adds 4 | adds `NewFolder`, `Up`, `View` | Lines. `Glyph::ALL` must become `[Glyph; 60]` (57 + 3). |
| `core/layout.rs` (6 hunks) | `CaptionWidths` fields `memory`, `icons`, `inspector_buttons`, `message_buttons`, `status_cancel`; `clear_width`; `SearchLayout`-less `Layout` with `search_tools`; `filters.tools` | `Layout { body: Body, dividers, status, min_client }`, `SearchLayout`, `PanelLayout`, `compute(…, mode, …)`; `CaptionWidths` fields `buttons` (with Retry), `drive`, `free`, `key_bar`; `image_button_width`, `status_last_part`, viewer helpers; `StatusLayout::running_text`, `drop_view_group`, `give_middle_the_rest` | Meaning. `SearchLayout` must hold `search_tools: Rect` (main) in place of `clear_search`, `match_case`, `match_path` (branch); `FilterLayout` keeps `tools` (not `clear`, `hide`). Branch code uses `captions.button_width(dpi)` in `compute_two_panels` (status Cancel and Retry): replace by `status_cancel` / `message_button_width`. The branch's `buttons` field with Retry needs its own group in main's three-way split. `StatusLayout::compute` now takes `Option<&WindowEnd>`; the branch passes `None` (see 3B.7). |
| `core/settings.rs` | `matching` default function and `matching_or_default` | `mode: Mode` field, `Mode::Search` default | Lines. Keep both fields. |
| `core/shortcuts.rs` | doc comment (Cancel special case removed) | doc comment ("either mode's menu bar") | Lines only. The code already merged correctly (the `CancelExport` special case is gone, the branch's dedupe stays). |
| `win/controls.rs` (6 hunks) | imports (`Size`, `LVIS_DROPHILITED`, `LVM_SETCALLBACKMASK`, `SB_SETICON`, ...); `Label::create_owner_drawn` removed; `Tooltip::create_areas/place_area/set_area` with `area_tool` | `Rect` import, `EM_SCROLLCARET`, `BCM_SETSPLITINFO`, `HKCOMB_*`, `LVM_GETITEMSTATE`, `LVM_SETSELECTIONMARK`; `Label::create_centred`; `Tooltip::add_area/move_area/set_area_text` with `area` | Meaning for the tooltip areas: two APIs for the same thing (main numbers areas from 0 and creates them up front; branch adds by id). Keep one. Imports: both sets. Branch's `create_owner_drawn` is not needed (Attributes is now an edit box); keep `create_centred`. Import `glyph_row_height` is dead. |
| `win/gdi.rs` | imports (`AlphaBlend`, `BLENDFUNCTION`, `AC_SRC_*`) | imports (`COLOR_HIGHLIGHT`, `COLOR_HIGHLIGHTTEXT`, `RestoreDC`, `SaveDC`) | Lines. `SaveDC`/`RestoreDC` were used by `draw_lone_glyph`, which main removed: if nothing else uses them, drop them. |
| `win/menu.rs` (3 hunks) | `track_popup_in_front` (foreground, then `WM_NULL`) | `track_popup_highlighted` (posts `VK_DOWN` keys) | Lines. Two different functions in the same place. Keep both and both import sets (`PostMessageW`, `WM_KEYDOWN`, `WM_NULL`). |
| `win/notify.rs` (3 hunks) | removes drawn-button code (`FlatGround`, `draw_flat`); `IconList::selection_changed`; `DrawStage::Row { row }`; `RowPainted`; `NewFont` | `RowBar`, `PanelRow`, `RowFonts`, `CustomDraw::draw_panel_row`, `row()`, `Button::dropped_down`, `Button::draw_key`, `ListNotify::{Focused, Unfocused, Clicked, FocusChanged}` | Lines for the imports and for `CLR_DEFAULT` against `RowBar`. Branch code uses what main removed: `draw_lone_glyph` for the `..` arrow (line ~600 of the merged file), `disabled_color` (still exists). `draw_key` uses `DrawThemeBackground` on a push button, the kind of drawing 114d80c removed (see 4). |
| `win/window.rs` (4 hunks) | `EraseBackground`, `EnterSizeMove`, `ExitSizeMove`, `press_access_key` call in the loop | `CtlColorEdit`, `DeviceRemoval`, `Char`, `own_keys`, `offer_key`, root-window dispatch in the loop | Lines. The merged loop body is right: `Char` offer, then `press_access_key`, then `IsDialogMessageW`. Only the loop's doc comment must name both. Import lists: both. |
| `docs/plans/fast-copy-delete-spec.md` | "struck through, in grey" | "Taken entries" rewritten (kept ended operations) | Meaning: wording. Branch text wins; replace "blank" by "struck through, in grey". |
| `docs/plans/file-manager-spec.md` | "struck through, in grey" in step 6 | step 6 gains the status-bar line text | Meaning in one sentence. Take the branch's long text and change its last sentence to "struck through, in grey". |

### `app/main_window.rs` hunk by hunk

1. Imports (3 hunks): main drops `ATTRIBUTES_ROW`, adds `ControlSizes`, `ToolHeights`, `TabStop`, `Menu`; branch adds `JobCaption`, `JobProgress`, `Body`, drops `Menu`. Union, but `Body` and `Menu` need checking (the branch removed `menus: Vec<Menu>`).
2. `drop_target` imports vs `OPERATION_CHANGED`, `PanelThreads`: union.
3. **`mod timer`**: main `DROP_SCROLL = 6`, `SAVE = 7`, `FILL` removed; branch `PANEL_SETTLE = [6, 7]`, `PANEL_AGAIN = [8, 9]`. Meaning: ids 6 and 7 collide. The branch's timer arms come first in the `match`, so they would swallow `SAVE` and `DROP_SCROLL`. Renumber the panel timers (8..11).
4. Icons in `apply_glyphs`: main `filters.choose_folder.set_icon(...)` only; branch loops over `clear_search`, `filters.clear`, `choose_folder`, `panels[i].retry`. `clear_search` and `filters.clear` are toolbar buttons now (no `Button::set_icon`); only `choose_folder` and the two `retry` buttons stay push buttons.
5. `MainWindow` fields: main `drop_target`, `drag_in`, `DragIn`, `DropFolder`; branch `viewers`, `panel_changed`, `panel_typed`. Keep both.
6. `MainWindow::new`: initialisers, same.
7. `show_status`: main `status_memory.replace(...)`; branch `job_text = self.job_text(&status.job)`. Keep both.
8. `cancel_job`: main `run(CommandId::Cancel)`; branch matches `Job::Export/Fix/Operation`. Meaning. Main routes everything through `Model::cancel_job`; the branch needs `Job::Operation` (which has no model-side cancel, the window calls `operations.cancel(id)`). Decide where the operation case lives (see 3A, 3B.5).
9. `layout()` (hunks at the `moves` vector and visibility): main moves `search_tools` and uses `layout.search`, `layout.list`, `layout.filters`, `layout.inspector` as fields; branch matches `layout.body` and `layout.search()`. `strip` must list `controls.search_tools` instead of the three controls. Main's `filters.tools` replaces `pane.clear`, `pane.hide`. Main's inspector `icon` is a static control: `inspector.icon.hwnd()` moves and shows with the inspector. Branch's visibility loop sets `pane.show` etc. from `show_filters`.
10. After `place`: main `controls.status.place(&layout.status, memory.as_ref())` and "fit the inspector only when it moved" (`inspector_moved`); branch `fill_panel_columns()`, `fill_on_resize(hwnd)`, `show_fields()`. `fill_on_resize` has no argument in main; and during a drag it must not fit the panels' columns at each step (3B.6).
11. `update_menu`: main `commands::items(&self.menus)`; branch builds `slot`/`node`/`replace_item`. Take the branch's `commands::items(&commands::menu_bar(mode))`, delete `slot`, `export_slot`, `replace_item`.
12. `start_operation`/`transfer`: main `start_operation(request, source: Option<FilesSource>)` and `transfer(sources, destination, moves, from)`; branch `start_operation(...) -> Option<OperationId>` and `Destination::Folder(...)`. Merge: `start_operation(request, Option<FilesSource>) -> Option<OperationId>`; `transfer` builds `Destination::Folder(destination)`; main's `copy_to_folder` keeps the `SAVE` timer after `set_destination_folder`; `paste` passes `Some(FilesSource::Paste(files))`; drop passes `Some(FilesSource::Drop(...))`.
13. `show_taken` doc: "struck through" plus the branch's "panels follow the operations".
14. `WM_PAINT`: main `paint_window(&dividers)`; branch computes `(dividers, icon)`. Take main's call (the inspector icon is a static control); drop `icon`.

## 3. Risks that merge without a textual conflict

### 3A. Compile errors in the merged tree

- `core/model.rs`: `can_cancel_job` and `cancel_job` match `Job::{Export, Fix}` only; the branch added `Job::Operation`.
- `core/commands.rs` ~345: `C::CancelExport` in the "not in Search" or "no shortcut" list; renamed to `C::Cancel`.
- `core/layout.rs`: `captions.button_width(dpi)` (2 places: status cancel, Retry); the layout test calls `captions(dpi, stretch)` and main's helper takes `(dpi, stretch, show_memory)` and `CaptionWidths::measure` takes `ControlSizes`.
- `core/viewer_window.rs` ~938: `Glyph::ChromeClose`; use `Glyph::Cancel`.
- `win/notify.rs` ~600: `draw_lone_glyph` for the `..` arrow. Main has `draw_glyph` (bitmap, `AlphaBlend`), which draws nothing while icons are off (`icons_shown()`); `draw_lone_glyph` drew always.
- `app/main_window/panels.rs` ~1041: `match` on `DrawStage` has the unit variant `DrawStage::Row` and no `RowPainted`. Main's variant is `Row { row }`; the branch's `CustomDraw::row()` becomes redundant.
- `app/dialogs.rs`: the branch's `Transfer` and `NewFolder` `DialogProc` impls list `DialogMessage` variants without `Focus { .. }` and `KeysCaptured { .. }`.
- `app/main_window.rs`: branch code calls `controls.clear_search`, `match_case`, `match_path` only in lines main also rewrote, so main's versions win; new branch lines in `layout()` and `apply_glyphs` are the ones to fix (hunks 4, 9).
- `win/controls.rs`: `Tooltip::add_area` and `create_areas` are two APIs for one need (the branch's key bar uses `Tooltip::create(parent, &[])` then `add_area`).

### 3B. Behavior wrong with a clean build

1. **Tab in Two panels.** Main's `control_key` returns `KeyAction::MoveTab` for every Tab/Shift+Tab before the focus tables. The branch binds `Tab` to `PanelKey::OtherPanel` inside its panel table. After a textual merge Tab in a panel runs `move_tab_focus`, which focuses the hidden Search box or list. It needs a mode or `Focus` check. The branch's drive buttons carry `WS_TABSTOP` and `Button::create_drop_down`; main's rule is "Tab moves only between the search box and the results".
2. **Drop in Two panels.** Main's `drag_over` tests `list.hwnd().screen_rect().contains(point)` on the Search list. A hidden window keeps its rectangle, and the list still holds the last results, so a file dragged over a panel can be dropped into a folder of stale Search rows. The spec says Two panels accepts no drop. The drop handler must refuse while `mode == Mode::TwoPanels`.
3. **Timer ids 6 and 7** (hunk 3 above): `SAVE` (ui.json) would never fire; the right panel's settle timer would fire for it.
4. **Save debounce.** Main saves `ui.json` 1 s after `Change::{Choice, Filters, Panes, ColumnShown, ColumnHidden}`. The branch's new persistent state (`Change::Mode`, panel folders and sorts, the Viewer's size) is not in that list, so it is saved only by the paths the branch already has.
5. **Esc cancels the operation.** Main binds Esc in the results list to `Cancel`. If `can_cancel_job()` is made true for `Job::Operation` (to enable File > Cancel in both modes), one Esc in Search cancels a running copy or delete. Decide whether `Job::Operation` is cancellable by File > Cancel and Esc, or only by the status bar's Cancel.
6. **Column fit during a border drag.** Main stopped fitting the Search list at each drag step (`sizing`, `sizing_fits`, `EnterSizeMove`). `fill_on_resize` has no argument and fits only the Search list. The branch's `fill_panel_columns()` runs in every `layout()` and measures texts; it needs the same `sizing` guard (owner ruling: the window never lags).
7. **Status bar in Two panels.** The branch passes `None` as the window end to `StatusLayout::compute` and calls `drop_view_group` and `give_middle_the_rest`. So Show details and the memory figures (Settings > "Show memory use") never show in Two panels, and `status_memory` is stale there. Main's status parts 3 and 4 assume the right group is part 2. Decide whether Two panels shows the memory figures.
8. **Sizes.** Panels format the Size column with `captions::kilobytes` (always KB). Main's `SizeFormat` (Bytes, KB, MB) only reaches `text::cell` for the Search list. Decide whether the panels follow "Show sizes in".
9. **Taken rows.** Main strikes through taken Search rows with `draw_row_struck` and the branch's `Taken::contains(path, keeps)` needs the list's kept set. The call in main's `draw_row` is `model.row_taken(row)`; it works only when the model's `kept` set is what the branch's `FileOperations::release` leaves in the queue. Main's `FileOperations::kept` and `release_ended` are replaced by the branch's `reported`/`release`/`take_ended`; check that nothing of main's remains (`release_ended`, `kept`).
10. **Folder-row drop and the branch's `Taken` split.** A drop starts an ordinary `Request::Copy/Move` (now with `Destination::Folder`), so `report`'s `last_ended` line ("Copied 3 items to …") and `operations_ended` for the panels also run for it. `AsyncDrop::end(...)` must still be called when the branch's `report` takes the `Arc` forms. The branch's `Request::Rename` removal does not touch the drop.
11. **Key bar and panel rows are drawn.** `Button::draw_key` paints a push button with `DrawThemeBackground` and two fonts; `draw_panel_row` fills and draws the whole row; the path box colors come through `WM_CTLCOLOREDIT`. Main's commit set a rule against drawn imitations (DESIGN.md "plain Windows controls", AGENTS.md owner rulings). Not a conflict; a rule the lead must apply or waive.

### 3C. Settings, menus and shortcuts

- `CommandId::Cancel` alias keeps saved shortcuts of `cancel_export`. The access-key uniqueness test iterates both modes' bars; Cancel is now an ordinary File item in both and needs a unique letter in all four languages (es, nl, tr catalogues merged without a clash of keys).
- `command.cancel_export` is gone from the catalogues. The branch's catalogue keys all remain.
- `Mode::ALL.flat_map(menu_bar)` in `shortcuts::listed_commands` merged correctly with main's "Show window" row (`command_rows(strings, shortcuts, show, filter)` takes a new argument; its callers are in `app/settings.rs`, which only main changed).

### 3D. Resource and catalogue merges

`en.json`, `es.json`, `nl.json`, `tr.json`, `gozler-ui.rc`, `Cargo.lock`, `CONTEXT.md` merged
without conflict. `en.json` and the other three parse and have no duplicate keys in the merged
tree. Main changed these texts the branch relies on: `file_operations.delete_body_*` (now
"They won't go to the Recycle Bin."), `merges_*`, `same_name_*`, `exit_waiting_body`.

### 3E. What stage 7 (uncommitted) changes under main's new code

Stage 7 gives `Model::shown_row`, `row_taken`, `set_typed_search`, `start_search`,
`rows_needed`, `set_selection`, `read_selection` and `coverage` a `List` argument, and replaces
`results()` by `search_list()` and removes `selection()`/`selected_files()`. Main's new call
sites to fix after the merge, all in `app/main_window.rs`: `model.results().row(...)` (3 places:
`fill_around_first_rows`, `draw_cell`, `draw_row`), `model.shown_row(row)` (`drag_over` and
`DropFolder`), `model.row_taken(row)` (`draw_row`, `show_taken`). DESIGN.md will conflict
(main edited base lines ~5, 12, 198, 231, 268-319, 358-401, 429, 652-667, 702; stage 7 edits
base lines ~5, 12, 154, 194-197, 357-371, 658, 705, 725-751).

## 4. Spec and CONTEXT.md differences to reconcile

- **Taken rows**: main says "struck through, in grey" everywhere; the branch's rewritten
  "Taken entries" text and step 6 still say "blank" in the two conflicted hunks (and the stage
  list says "the drop is being built"; main built it).
- **Stage 2** in the branch's stage table: "built; the drop is being built" -> "built".
- **Delete question**: main's text drops "and you can't undo this" (catalogue and spec step 2);
  the branch's `dialogs.rs`, tests or spec text elsewhere must not quote the old sentence.
- **Window / App / Search / Two panels** (`CONTEXT.md`, merged without conflict but
  inconsistent): main defines **Window** as "the search box, the panes and the results list" and
  **App** as `gozler-ui`; the branch defines **Search** and **Two panels** as modes "of the
  window" and **Pane** as "a side area of Search". Window should be the app's main window, in
  one of two modes. Main's "Avoid: UI, app (for the window)" also applies to the branch's spec
  text, which says "the window" for the app in places.
- **Drop in Two panels**: spec says a second window "accepts no drop"; it says nothing about
  Two panels. Add "Two panels accepts no drop from other programs" (the "later plan" line
  already says so for drag between panels).
- **Status bar in Two panels** (spec table): no memory group, no Show details; main's DESIGN
  describes the memory group as part of the right end. State the Two panels rule.
- **Keys**: the spec's Tab (switch panels) and Esc rules and main's DESIGN rule "Tab moves only
  between the search box and the results" (and Esc in the list = Cancel) need one statement
  each, per mode.
- **Drawn key bar and rows** against main's "plain Windows controls" rule (see 3B.11).
- **Size column**: spec's panel sample shows KB; main's "Show sizes in" is in DESIGN. State
  whether panels follow it.
- **Cancel**: the spec's File menu for Two panels, the status bar's Cancel and "Cancel export"
  wording must use `Cancel` as main names it.
