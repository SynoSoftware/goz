# First review of docs/plans/native-controls.md (Opus)

Verdict: Needs revision.

## Blocking

1. Access keys. The message loop sends only WM_KEYDOWN to control_key (win/window.rs run_message_loop); Alt+letter arrives as WM_SYSKEYDOWN/WM_SYSCHAR and IsDialogMessageW dispatches it. Letters are per catalogue (nl.json: Identiek hoo&fdlettergebruik, Zoeken in &pad, Overeenko&mst:), so hard-coded Alt+C/P/A breaks other languages. Filter pane Clear "C&lear" loses Alt+L. Removing "M&atch:" leaves no text carrying A. Fix: in the loop before IsDialogMessageW, take WM_SYSCHAR and ask each visible toolbar with TB_MAPACCELERATOR; take the mode's letter from captions.match_label or keep the label.
2. Keyboard reach. File operations window runs IsDialogMessageW (set_modeless); Pause, Cancel, queue toggle, Run, Move up, Move down, queue Cancel are WS_TABSTOP push buttons with no access keys; toolbars are not Tab stops, so they lose keyboard access. Settings' two Clears are template PUSHBUTTONs (gozler-ui.rc lines 116, 203), Tab stops. Main window moves focus to Show/Hide filters and Show/Hide details when panes change (Change::Panes); toolbar buttons cannot hold focus. Fix: in File operations and Settings keep push buttons, drop only draw_flat, give icons with Button::set_icon (BCM_SETIMAGELIST); in the main window fall back to the search box and change two DESIGN sentences.
3. File operations card's Pause/Resume and Cancel are also draw_flat and missing from the plan's table. That window answers WM_ERASEBKGND with 1 and paints in WM_PAINT (paint_card), no WM_PRINTCLIENT, so a transparent TBSTYLE_FLAT toolbar's background would be wrong. Push buttons avoid it.
4. button_images returns None while Show icons is off, and builds a six-state BUTTON_IMAGELIST list. Toolbars need one image per button plus TB_SETDISABLEDIMAGELIST; disabled_color matters (COLOR_GRAYTEXT does not read as unavailable on a toolbar). Icon-only buttons must always draw. TB_SETIMAGELIST does not take ownership: say who frees.
5. Toolbar styles: default CCS_TOP moves the toolbar to the top, full width, with a divider. Add CCS_NORESIZE | CCS_NOPARENTALIGN | CCS_NODIVIDER.
6. Match mode width: TB_GETIDEALSIZE measures the current mode; BTNS_AUTOSIZE changes width per mode and moves the search box (DESIGN: nothing moves by itself); without it every button takes the widest width. Fix: fixed width via TB_SETBUTTONINFO/TBIF_SIZE from CaptionWidths.match_modes plus the arrow; others BTNS_AUTOSIZE.

## Non-blocking

- Attributes: other values are read-only edit boxes; Attributes becomes a static (SS_NOPREFIX | SS_ENDELLIPSIS).
- draw_icon stays (draw_cell uses it); cell_text stays (draw_path).
- Tooltips: filter Clear's tooltip differs from its caption; with MIXEDBUTTONS the toolbar shows text as tooltip only without BTNS_SHOWTEXT; handle TBN_GETINFOTIP.
- Heights: set TB_SETBUTTONSIZE/TB_SETPADDING so Clear is as tall as the search box and copy icons as a field row.
- Measuring order: update toolbars (font, text, images) before TB_GETIDEALSIZE on DPI, language and Show icons changes; toolbar widths are a new layout input.
- Inspector icon: unconfirmed whether SS_ICON resizes on STM_SETIMAGE without SS_REALSIZECONTROL; ImageList_GetIcon HICON per selection must be destroyed.
- Memory parts: keep each HICON alive (Icon type); widths must cover the bar's icon gap and padding (unchecked); SBARS_TOOLTIPS may show its own tooltip beside the area tooltip when text is cut.
- Removing "Match:": a screen reader says only "Start of words".
- Settings Clear, if a toolbar: declare ToolbarWindow32 in the template.
- Dividers already committed (763bee1).
- DESIGN "Search box" rejects push-like toggles ("shows its state only as a slightly darker button"); the plan reverses it and does not give the new Owner decisions text.
- The match controls are Windows controls, not drawn; converting them causes issues 1 and 6 and the missing label.

## Unverified

Toolbar buttons take no focus on click; focused toolbar keyboard; CDRF_NEWFONT restore; clrText at row stage for selected rows in high contrast; disabled 32-bit images without a disabled list; TB_SETIMAGELIST at new DPI; PMv2 dialog scaling of code-created children; screen-reader roles of toolbar check and drop-down buttons; TB_MAPACCELERATOR with hidden text; toolbar text colour COLOR_BTNTEXT vs COLOR_WINDOWTEXT in custom high contrast.

## Missed: drawn things that have a native control

| What | Where | Native replacement | Loss |
| Card Pause/Resume, Cancel | app/file_operations.rs draw_flat | push buttons with icons | flat look; gain a border |
| Status bar right group totals with icons | draw_status_parts, SBT_OWNERDRAW | one plain part, or three parts with SB_SETICON | icons; or fixed widths with gaps |
| Group box caption icons | show_group_glyph subclass | plain group box | the icon |
| Settings page list with icons | IDC_SETTINGS_PAGES LBS_OWNERDRAWFIXED | list view with glyph image list, or plain list box | Explorer-like selection; or no icons |
| Kind list | ListBox::create_with_glyphs | same | same; conflicts with "filter pane uses list boxes" |
| Location list path ellipsis and hand-made tooltip | ListBox::create_paths, PathTip subclass | one-column list view with compact_path text and LVN_GETINFOTIP | Explorer-like selection; same conflict |
| Queue list selection kept while unfocused | QueueList::draw | LVS_SHOWSELALWAYS | selection grey while unfocused |
| Queue list drag insertion line | QueueList::draw FillRect | LVM_SETINSERTMARK | unverified in one-column tile view |
| File operations card icon | paint_card draw_lone_glyph | static icon (Icon::colored) | nothing |

## The seven kept, nearest native

| Pane dividers | SS_ETCHED statics or client edges | 2px etched or sunken; DESIGN chose 1px "instead of 3D edges" |
| Bold matched text | none | keep |
| Path ellipsis in failed-items list | list's end ellipsis + label tips; or PathCompactPathW + LVN_GETINFOTIP; or Name and Folder columns | name cut instead of folder middle; or two columns |
| File operations card | SS_ETCHEDFRAME or group box; or no card | square etched frame; or plain face |
| Coloured status texts | already Windows statics via WM_CTLCOLORSTATIC | keep |

## Reimplementations to keep (no native control)

splitter drag; filter pane scrolling container; inspector values' middle ellipsis; shortcut box with keyboard hook; tracked balloon tooltip. Menu icons are native and settled.
