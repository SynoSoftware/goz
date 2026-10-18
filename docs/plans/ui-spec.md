# Spec: gozler-ui

A Windows desktop window for gozler: the crate `gozler-ui`, which builds
`gozler-ui.exe`.

This spec says what the window does and why. It changes only when a
requirement changes. [The design](ui-design.md) says how it is built, and
[the plan](ui-plan.md) says in what order and how each stage proves it is
done.

Every command, state, transition and journey below has an ID, such as `C4`
or `T2`, and a "Checked by" entry that names the check that proves it. Each
ID appears in at least one check in the plan. An entry with no check is a
defect in this spec, and it stops the stage that adds the behavior.

## Goal

A person finds a file faster in a window than in a console: they type, see the
matches at once, narrow them with a few clicks, and open the file. The window
gives the search that `gozler` already does a Windows desktop face.

**It uses little memory.** The window stays open all day beside other work, so
it holds no index and no result set. The daemon already holds the index; the
window holds only a cache of at most 32,768 rows around the ones on screen,
and, while the user holds a selection, the files of that selection (see
"Memory budget").

**It adds no search feature.** Every filter, column and sort is something
`gozler` already does today; "Not planned" lists what is left out and why. A
new search feature goes to `gozler-core` first, then the window shows it.

**It answers as `gozler` does.** For the same query, the window shows the same
count, the same rows in the same order, and exports the same bytes as
`gozler`, because both use `gozler-core`'s parser, the same request rules and
the same output code.

**It helps.** When something fails, the window says what to do next and,
where it can, offers a button that does it (see "Feedback").

## Priorities

The owner set these, in this order. Each choice below names the one it serves.

1. **Low memory.** Essential. The window stays open all day beside other work.
2. **Usability and correct Windows behavior.** Essential.
3. **Little code to write and maintain.** Essential.
4. **A polished look.** Desirable. It never justifies substantial code.
5. **Dark mode.** Optional. It is dropped when it costs more than it gives.

## Owner decisions

These are settled. The design and the plan build on them and do not reopen
them. The toolkit and where its `unsafe` code lives are in
[the design](ui-design.md#decisions).

1. **No dark mode in the first version.** It needs undocumented uxtheme
   exports, and the menu bar stays white without undocumented
   `WM_UAHDRAWMENU` painting.
2. **The top of the window** is the classic menu bar, then the search box with
   its match controls ("Search box"). A command bar of icon buttons was
   prototyped and not chosen.
3. **The list reaches the first 100,000 rows** in the chosen order (see
   "Searching"). A result snapshot kept by the daemon would remove the limit;
   it is new daemon work.
4. **The name** is `gozler-ui`, which builds `gozler-ui.exe`. Like
   `gozler-cli`, it names what the crate does, not what it is built with.
5. **Only the window is translated.** `gozler` and `gozlerd` stay in English,
   because their users are scripts, agents and developers. The window's
   catalogues are JSON files, as TinyTorrent2's localisation contract
   defines them, so the owner's projects share one catalogue style. Fluent
   and other translation crates are not used.
6. **Two places choose the language:** a Language menu at the right end of
   the menu bar, for instant access, and Settings > General > Language, where
   Everything and TinyTorrent2 keep it. Both change the same saved choice at
   once, with no restart.
7. **The filter pane uses list boxes** for Kind and Location, variant A of the
   prototype ("Filter pane").
8. **Start of words is the default match mode**, the rule of the owner's
   SearchHelper: `para 50` finds `Paracetamol 500mg.pdf` ("Search box").
9. **The window uses TinyTorrent2's words** where both programs mean the same
   thing, such as Settings, not Options. `CONTEXT.md` holds them.

## Look

The window is made of plain Windows controls, with only the tweaks that cost
one call or a few lines. Every control stays a real Win32 control, so Windows
keeps its keyboard behavior, focus, accessibility and high contrast.

- `SetWindowTheme(list, "Explorer")` on the list view: File Explorer's row
  hover, selection fill and header.
- `LVS_EX_DOUBLEBUFFER`, `LVS_EX_FULLROWSELECT` and `LVS_EX_HEADERDRAGDROP`:
  no flicker while scrolling, whole-row selection, column reorder.
- Layout in device-independent pixels, scaled with `GetDpiForWindow` and laid
  out again on `WM_DPICHANGED`.
- The message font from `SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS)`,
  and one larger semibold font for the inspector's heading.
- `WM_CTLCOLORSTATIC` returning the window colour, so labels and check boxes
  have no grey dialog background. Every colour the window paints comes from
  `GetSysColor`, so high contrast works.
- One-pixel dividers between the panes instead of 3D edges.
- **Spacing** follows Microsoft's Windows desktop layout guidelines: 11 px
  margins at the window's edges, 7 px between related controls and 11 px
  between groups, at 96 DPI. The design holds them as named constants; the
  plan checks the numbers against the guidelines before stage 2's layout.
- **Nothing moves by itself.** Only the window size, the DPI, the panes shown,
  the splitter and the language move a control. A count, a path, a message or
  a state never does. The status bar's first three parts have fixed widths,
  each sized for its longest message with the largest numbers in the current
  language and DPI; a longer value ends in an ellipsis.
- **Text fits.** A control that the window places in code takes its width
  from its measured caption, so a longer translation widens it instead of
  clipping it. Dialog templates leave room for twice the English length on
  captions of up to 12 characters and 50 % more on longer ones.

## Words

- Labels use sentence-style capitalization, as current Windows and the
  Microsoft Writing Style Guide do: "Open file location", "Copy as path".
- Where File Explorer has a word for the same act, the window uses it. Stage 4
  checks each such label against File Explorer on the owner's Windows.
- A label is a verb phrase for a command and a noun phrase for a setting. It
  names what happens, not how: "Copy name", not "Copy file name to clipboard".
- A command that opens a dialog ends in "…".
- Every menu item, dialog label and button has an access key, the underlined
  letter that Alt or the menu uses, except OK and Cancel, which `Enter` and
  `Esc` press, as in Windows' own dialogs. Each catalogue chooses its own
  letters, unique within each menu and each dialog. The main window's letters
  also differ from the menu bar's F, E, V and H, so an access key never opens
  a menu by accident; the six attribute combo boxes share one letter, on the
  Attributes label, which focuses the first. In the commands table, `&` marks
  the English access key.
- A message says what happened in one sentence, then what to do (see
  "Feedback"). It never blames the user and never shows a code alone.

## Main window

| Area | Control |
| --- | --- |
| Top | Menu bar, then a strip with the search box and its match controls |
| Left | Filter pane |
| Center | Results list: a list view in report and virtual mode |
| Bottom | Inspector pane, under a splitter that the user drags |
| Bottom edge | Status bar control (`STATUSCLASSNAME`) with four parts |

The window is per-monitor DPI aware (manifest `PerMonitorV2`) and uses the
Windows message font. It opens at its saved placement; a saved placement that
no monitor shows any more opens on the nearest monitor. Its minimum size is
the smallest one at which every control still fits, except the filter pane,
which scrolls ("Filter pane"). Its title is the typed search and the program's
name, "report – gozler", as Windows titles a document window, or "gozler"
alone while the search is empty.

### Search box

- An edit control with a cue banner ("Find files and folders", as
  TinyTorrent2's boxes say "Find a torrent") that stays visible while the box
  is focused and empty (`EM_SETCUEBANNER` with `wParam` TRUE), because the
  window opens with the box focused. It fills the strip's width.
- `Ctrl+F` and `Ctrl+E` focus it. `Escape` clears it. `Down` moves focus to
  the first row.
- Beside it, the match controls, each with an access key and none with a
  `Ctrl` shortcut:

  | Control | Kind | Choices | Command line |
  | --- | --- | --- | --- |
  | M&atch: | drop-down list | Start of words (default), Start of name, Anywhere, Whole words, Regular expression | `wordstart:`, `startwith:`, a plain term, `-ww`, `-r` |
  | Match &case | check box | on, off (default) | `-case` |
  | Match &path | check box | on, off (default) | `-p` |

- **The match mode is one choice, so it is one drop-down list.** Its five
  modes exclude each other: a regular expression reads the whole text as one
  pattern, so no other mode applies to it. Separate check boxes would allow
  combinations that mean nothing, such as Whole words with a regular
  expression.
- **Match case and Match path are check boxes,** because each is one
  independent on or off choice, and check boxes show it at a glance, as the
  Find dialogs of Windows do. A push-like toggle shows its state only as a
  slightly darker button.
- **The modes apply to name terms.** A path term, a wildcard and a search
  function keep their own meaning, so `ext:pdf para` finds PDF files with a
  word that starts with `para`, and in Start of name `docs\agents` still
  finds the files below that folder. Whole words is `-ww`, which `gozler`
  already applies to path terms too, so it keeps that rule and the window
  and `gozler -ww` agree. With Match path, a mode applies to the full path
  instead of the name.
- **Start of words** is the rule of the owner's SearchHelper: each term must
  match where a word of the name starts, in any order, so `para 50` finds
  `Paracetamol 500mg.pdf`. A word starts at the start of the name or after a
  character that is not a letter or digit, so `1234` finds `IMG_1234.jpg`. Two
  differences from SearchHelper keep gozler's own rules:
  - Accents count, as everywhere in gozler (`gozler_core::fold`): `resume`
    does not find `résumé`. Ignoring accents would need a second folded copy
    of every name in the service's index.
  - A typed term is matched whole: `report.txt` finds `my report.txt`, where
    SearchHelper would split it into `report` and `txt`.
- `wordstart:` and `startwith:` are new search functions that stage 1 adds to
  `gozler-core` and the service, so `gozler` gives the same answer for the same
  query ("Goal").

### Searching

- Search as you type starts a search 150 ms after the last keystroke. With it
  off (see "Settings dialog"), `Enter` runs the search.
- While a search runs, the list keeps the previous results and the status bar
  says "Searching…". The new results replace them at once, scrolled to the
  first row (`T1`, `T2`).
- The list reaches the first 100,000 rows of a search. When more match, the
  status bar says "Showing the first 100,000 of 3,204,000 results". The order
  still covers every match: the first 100,000 by name are the first 100,000 of
  all matches by name.
- **What a request costs the daemon.** Every request for rows scans every
  match, then sorts and builds the paths of the rows up to the last one asked
  for. On the owner's machine, with 2 million entries, the scan alone takes
  about 0.3 seconds for the empty search. Rows further down cost more: rows
  near 100,000 cost about 0.4 seconds. So rows that the user scrolls to show
  empty cells for that long before they fill, and the limit keeps the worst
  request bounded ([the design](ui-design.md#rows-on-demand) explains it).
- When the Location folder does not exist, the window does not search, as
  `gozler` refuses the same search (`S8`).
- `F5` runs the current search again. Results do not update by themselves when
  files change, because the protocol has no change notification.

### Results list

| Column | Default | Header click sorts by | Content |
| --- | --- | --- | --- |
| Name | shown, cannot be hidden | `name` | the name, with the icon of its extension |
| Path | shown | `path` | the folder of the full path |
| Size | shown | `size` | the size in KB, rounded up, as File Explorer shows it ("1,234 KB"), so sizes line up for comparison; for a folder, the size of every file below it (see "Folder sizes"); "Unknown" for an unknown size |
| Date modified | shown | `dm` | local date and time, in the Windows short date and time format |
| Extension | hidden | `extension` | the extension; empty for a folder |
| Attributes | hidden | (not sortable) | the attribute letters, as `gozler` prints them |

- **Folder sizes.** A folder's size is the sum of the known sizes of every
  file below it, the number `gozler -get-folder-size` gives, as in
  Everything with "Index folder size" on. The service keeps it for every
  folder, always: about 8 bytes a folder, 2.2 MB for the 270,137 folders on
  the owner's machine, against 196 MB for the whole index, so it needs no
  setting. Because the window answers as `gozler` does ("Goal"), the service
  and `gozler` change with it: the Size column, `-size` and exports show it,
  `size:` tests it and the size sort orders folders by it. `-get-total-size`
  and `-get-folder-size` still add files only, because a folder's size is
  already the sum of its files. Stage 1 of the plan builds it.
- **The matched part of a name or path is bold,** as in Everything, so the
  person sees why a row matched: `para 50` makes "Para" and "50" bold in
  `Paracetamol 500mg.pdf`. `gozler-core` gives the matched ranges for the
  shown search, so the bold part is always what the service matched. A
  regular expression shows its match; filter clauses show none.
- A cell whose text is cut off shows the whole text in a tooltip.
- A header click runs the search again in that order; a second click reverses
  it. The first click on Size or Date modified sorts largest or newest first,
  as gozler's sort keys do. The header shows the arrow (`HDF_SORTUP`,
  `HDF_SORTDOWN`), and View > Sort by shows the same choice and gives the
  keyboard a way to sort.
- View > Columns and a right-click on the header show and hide columns.
  Column order, widths and visibility are saved.
- Icons come from `SHGetFileInfoW` with `SHGFI_USEFILEATTRIBUTES` and
  `SHGFI_SYSICONINDEX`, so Windows gives the icon for the extension without
  reading the file. The window asks once for each extension. A per-file icon,
  such as an `.exe`'s own, would read every file from disk.
- A right-click on a row that is not selected selects it first, as in File
  Explorer. A right-click below the last row shows no menu.

### The selection

The selection is a set of files, fixed at the moment the user makes it. Every
action on the selection acts on those files, however long the user waits and
whatever changes in the index meanwhile. No action refuses.

- When every selected row is in the window's cache, the selection is those
  rows: the files the user sees.
- Otherwise the window reads the selected rows from the daemon in one
  request, 150 ms after the selection last changed. Until they arrive, the
  status bar says "Reading the selection…" and the commands that act on the
  selection are disabled; a key for one of them does nothing (`S12`, `T6`).
  The same reply refreshes the visible rows inside the selection, so the
  highlighted rows always show the files that the selection holds.
- **The protocol's one limit.** A selected row that the user never saw is the
  file that was at that position when the selection was made, because the
  daemon keeps no result set and results carry no file ID. A result snapshot
  in the daemon (owner decision 3) would remove this limit.
- A new search, sort or refresh clears the selection.
- The selection holds at most 100,000 rows, because the list reaches no more.

### Status bar

| Part | Shows | Width |
| --- | --- | --- |
| 1. Results | "1,204 results", "Showing the first 100,000 of 3,204,000 results", or "Searching…"; "· Filtered" is added while a filter is set and the filter pane is hidden | fixed |
| 2. Selection | "3 selected · 12.4 MB" (files only), or "Reading the selection…" | fixed |
| 3. Activity | an export's progress and result: "Exporting… 120,000 rows", "Exported 2,011,814 rows to results.csv". Cancel export clears it; the result stays until the next export starts | fixed |
| 4. Index | the index state, only when it affects the answer | the rest |

Part 4:
- A volume in scope is not complete (`VolumePhase::is_complete`): "C:\ is
  indexing — results may be incomplete.", or the word for an offline or
  failed volume. While a volume in scope is bootstrapping or rescanning, the
  window asks for status every 2 seconds and runs the search again once it is
  complete (`T4`, `T5`). An offline or failed volume is not polled, because
  waiting does not complete it.
- `metadata_pending` and the query tests a size or date: "Some sizes and
  dates on D:\ are unknown."

### Window lifetime

- One window per user session. A second start brings the first window
  forward, restored when it was minimized and still maximized when it was
  maximized; a named mutex (`Local\gozler.ui`) decides which process is
  first. The `Local\` namespace is already one per session.
- A second start and the shortcut from Settings > Keyboard both bring the
  window forward with the search box focused and its text selected, so the
  person types the next search at once. A second start with
  `--location <folder>`, from File Explorer's "Search with gozler", also sets
  Location to that folder.
- **"Keep gozler running in the notification area when I close its window"**
  (Settings > General) is Everything's "Run in background":

  | | Off | On (the default) |
  | --- | --- | --- |
  | Close (`Alt+F4`, the title bar's X) | exits | hides the window; gozler keeps running in the notification area |
  | Tray icon | none | shown while gozler runs |
  | The shortcut from any program | works while the window is open | always works |
  | At sign-in, with "Start gozler when I sign in" on | the window starts minimized | gozler starts in the notification area, with no window |
  | Exit (File > Exit, the tray menu) | exits | exits |

- **The tray icon:** a click, or `Enter` on it, shows the window; its menu
  holds **Show window** (bold, the default) and **Exit**, the words of
  TinyTorrent2's tray. Its tooltip is "gozler". Windows Explorer restarts
  drop every tray icon, so gozler adds its icon again when the taskbar
  comes back (`TaskbarCreated`).
- **The first time** the window hides to the notification area, a Windows
  notification says: "gozler is still running. Press {shortcut} or click its
  icon in the notification area to show it. Choose Exit from its menu to stop
  it." It shows once, because the person knows it after that.
- Hiding the window changes nothing else: a running export goes on, and
  status part 3 shows its result when the window shows again. Exit ends
  every request and the process (`T14`).

## Commands

One row per command. "Where" names every place the command appears; all of
them call the same implementation. Menus are grouped by what the user is
doing, with a separator between groups (see "Menus"). "Key" is each
command's default shortcut; the person changes them on Settings > Keyboard,
and the menus show the shortcuts in use.

| ID | Command | Where | Key | Enabled when | Acts on | Checked by |
| --- | --- | --- | --- | --- | --- | --- |
| C1 | &Open | File; context menu (default, bold); inspector button; double-click | `Enter` in the list | 1 to 15 rows selected and the selection is read | each selected entry: a folder with the "Open folders" command, a file with the "Open files" command; an empty command does what File Explorer does | stage 4; after install |
| C2 | Open file &location | File; context menu; inspector button | `Ctrl+Enter` | 1 to 15 rows selected and the selection is read | the folder of each entry, with the entry selected, or the "Open file location" command | stage 4; after install |
| C3 | Propert&ies | File; context menu | `Alt+Enter` | exactly 1 row selected and the selection is read | the shell's Properties dialog for the entry | stage 4; after install |
| C4 | &Export results… | File | — | the list shows results and no export runs | every match of the shown search, not only the rows the list reaches | stage 4 |
| C5 | &Cancel export | File, in place of C4 | — | an export runs | the running export; its partial file is deleted | stage 4 |
| C6 | &Settings… | File | `Ctrl+,` | always | — | stage 7 |
| C7 | E&xit | File; tray menu | — (`Alt+F4` closes the window, see "Window lifetime") | always | ends gozler | stage 2; stage 7 for the tray menu |
| C8 | &Copy | Edit; context menu | `Ctrl+C` | list focused: rows selected and the selection is read; search box focused: text selected | list: the files, as `CF_HDROP`, as File Explorer copies them; search box: the selected text | core test; after install |
| C9 | Copy as &path | Edit; context menu; inspector button | `Ctrl+Shift+C` | rows selected and the selection is read | each full path in double quotes on its own line, as File Explorer's Copy as path | core test; after install |
| C10 | Copy &name | Edit; context menu | — | rows selected and the selection is read | each name on its own line | core test; after install |
| C11 | Select &all | Edit | `Ctrl+A` | always | the focused control: the search box's text, or every row of the list | stage 4 |
| C12 | &Search in this folder | context menu | — | exactly 1 row selected and the selection is read | sets Location to the entry's folder, or to the folder itself | stage 5 |
| C13 | &Filters | View (check mark) | — | always | shows or hides the filter pane | stage 5 |
| C14 | &Inspector | View (check mark) | — | always | shows or hides the inspector pane | stage 6 |
| C15 | &Columns ▸ one item per column | View; right-click on the header | — | always; Name cannot be hidden | shows or hides the column | stage 3 |
| C16 | Windows default, then each language by its own name | La&nguage, the last menu bar item, aligned right (radio marks); `L` belongs to Clear filters | — | always | the window's language, at once (see "Language") | stage 8 |
| C17 | &Refresh | View | `F5` | a search has been run | runs the shown search again | stage 3 |
| C18 | &Find | Edit | `Ctrl+F`, `Ctrl+E` | always | focuses the search box and selects its text | stage 3 |
| C19 | M&atch: Start of words, Start of name, Anywhere, Whole words, Regular expression; Match &case; Match &path | the strip beside the search box | the access keys `Alt+A`, `Alt+C`, `Alt+P` | always | the match mode or option; runs the search again | stage 3 |
| C20 | Sort &by ▸ Name, Path, Size, Date modified, Extension; Ascending, Descending | View (radio marks); header click | — | always | the sort; runs the search again | stage 3 |
| C21 | C&lear filters | filter pane button; a button in a zero-results message | — | a filter is set | sets every filter group to its first choice | stage 5 |
| C22 | &Search syntax | Help | `F1` | always | a dialog with the search syntax: the search terms, the search functions and the attribute letters, from the tables that `gozler --help` lists; no command-line options, shell quoting or exit codes, because none of them applies in the search box | stage 3 |
| C23 | &About gozler | Help | — | always | a dialog with the version | stage 2 |
| C24 | &Show window | tray menu (default, bold); a click or `Enter` on the tray icon | the shortcut from Settings > Keyboard | gozler runs in the notification area | shows the window and focuses the search box with its text selected | stage 7 |

"The selection is read" means it holds its files ("The selection"). Open and
Open file location stop at 15 entries, because File Explorer asks before it
opens more than 15 items, and the window would rather not open many windows by
accident.

### Menus

Groups are separated by a separator, in this order:

- **File:** Open, Open file location, Properties | Export results… or Cancel
  export | Settings… | Exit
- **Edit:** Copy, Copy as path, Copy name | Select all | Find
- **View:** Filters, Inspector, Columns | Sort by | Refresh
- **Help:** Search syntax | About gozler
- **Language,** aligned right on the menu bar (`MFT_RIGHTJUSTIFY`): Windows
  default | each shipped language by its own name
- **Context menu on a row:** Open, Open file location | Copy, Copy as path,
  Copy name | Search in this folder | Properties
- **Tray menu:** Show window | Exit

The menu bar has four menus, as TinyTorrent2's has, and each holds what its
name says: File acts on the selected files and on the window, Edit on the
clipboard and the selection, View on what the window shows, Help on how to
use it.

- **No Search menu.** It held Find, the match controls and Sort by. A menu
  closes after each click, so setting two of them took two trips, and
  the strip beside the search box shows and changes them in one place, with
  access keys. Find moves to Edit, where Notepad has it, and Sort by to View,
  where File Explorer has it.
- **A context menu shows no shortcuts,** as File Explorer's does. The menu bar
  shows them.
- A disabled command stays in its menu, greyed, so the menus never change
  shape.

## States

What the window shows in each state. The inspector's own states are in
"Inspector pane". Each failure's message is in "Daemon failures".

| ID | State | List area | Status part 1 | Checked by |
| --- | --- | --- | --- | --- |
| S1 | Starting, before the first reply | empty, no message | Searching… | stage 3 |
| S2 | A new search runs | the previous rows | Searching… | stage 3 |
| S3 | Results | the rows | "1,204 results" | stage 3 |
| S4 | More than 100,000 results | the first 100,000 rows | "Showing the first 100,000 of N results" | stage 3 |
| S5 | No results | "No files match." with Clear filters when a filter is set, and Search everywhere when Location is set | "0 results" | stage 3, stage 5 |
| S6 | Empty search, with "Show every file and folder when the search is empty" off | "Type to search." | empty | stage 7 |
| S7 | The search text does not parse | the parser's message and how to fix it, such as "dm: needs a date, such as dm:today or dm:2026-01-31" | empty | stage 3, stage 8 |
| S8 | The Location folder does not exist | "The folder D:\Old does not exist." with Choose folder… and Search everywhere | empty | stage 5 |
| S9 | A daemon failure | the failure's message and its button ("Daemon failures") | empty | core test; stage 3; after install |
| S10 | A filter is set and the filter pane is hidden | the rows | "· Filtered" added | stage 5 |
| S11 | An export runs | unchanged | unchanged; part 3 shows its progress | stage 4 |
| S12 | The selection is being read | unchanged | unchanged; part 2 says "Reading the selection…" | core test; stage 4; stage 6 |
| S13 | A volume in scope is not complete | the rows | unchanged; part 4 says so | core test; after install |

## Transitions

What changes, and what does not, when an event arrives in another state. A
row's last column names its check; the design's model tests use the same IDs.

| ID | Event | What the user sees | Commands | Work in progress | Checked by |
| --- | --- | --- | --- | --- | --- |
| T1 | The search text, a match control, a filter or the sort changes | the previous rows and selection stay; part 1 says Searching… | unchanged | the running search is cancelled; an export and a selection read go on | core test; stage 3 |
| T2 | A new search's first rows arrive | the new rows replace the old ones, scrolled to the top; the selection is cleared | the selection commands are disabled until a new selection | a selection read for the old search is cancelled | core test; stage 3 |
| T3 | A search fails while rows are shown | the rows are removed and the failure's message shows | the selection commands are disabled | a selection read is cancelled; an export goes on and fails or ends by itself | core test; stage 3 |
| T4 | Status shows that the daemon answers again, or that every volume in scope is complete | the search runs again (T1, T2) | unchanged | the status poll stops when nothing needs it | core test; after install |
| T5 | A volume in scope starts or ends indexing | part 4 changes; the rows stay | unchanged | the status poll starts or stops | core test |
| T6 | The selection changes while it is being read | part 2 says Reading the selection… | the selection commands stay disabled | the old read is cancelled; one new read starts 150 ms after the last change | core test; stage 4; stage 6 |
| T7 | The selection changes while a folder's size is calculated | the inspector shows the new selection | unchanged | the old size request is cancelled | core test; stage 6 |
| T8 | An export ends while a search runs | part 3 shows the export's result; part 1 still says Searching… | Export results… returns | none | core test; stage 4 |
| T9 | An export fails | a message box names the file and Windows' reason; part 3 is cleared | Export results… returns | the partial file is deleted | core test; stage 4 |
| T10 | The service stops while the Settings dialog is open | the dialog stays open and usable; its Index page shows the failure; the main window shows the failure behind it | unchanged | the status poll runs | core test; after install |
| T11 | New results arrive while the context menu is open | the menu stays open | a command chosen from it acts on the selection when it is chosen; if the selection was cleared meanwhile, it does nothing | none | core test |
| T12 | The language changes | every caption changes; nothing else does | unchanged | none; no search runs | stage 8 |
| T13 | The window moves to a monitor with another DPI | the layout and fonts scale; nothing else changes | unchanged | none | after install |
| T14 | gozler exits while work runs: Exit, or Close with "Keep gozler running in the notification area" off | the window closes at once | — | every request is cancelled; a partial export is deleted; the settings are saved | stage 3, stage 4 |

## Journeys

The tasks a person comes to the window for, walked from start to finish. Each
journey must work with the keyboard alone and with the mouse alone.

| ID | Journey | Steps | Checked by |
| --- | --- | --- | --- |
| J1 | Find a file by part of its name and open it | type part of the name; the matches appear as they type; `Down`, then `Enter` | stage 3 (to the selected row); after install (the file opens) |
| J2 | See where a file is | find it; `Ctrl+Enter`; File Explorer opens its folder with the file selected | after install |
| J3 | Find the large files in a folder | Location > Choose folder…; Size > Huge; click the Size header; the inspector shows the selected file's size | stage 5, stage 6 |
| J4 | Find the documents changed this week | Kind > Documents; Date modified > Last 7 days; Sort by > Date modified | stage 5 |
| J5 | Copy the paths of a few files | Ctrl+click three rows; `Ctrl+Shift+C`; paste in an editor | core test; after install |
| J6 | Save a list of matches | File > Export results…; choose a name and a format; part 3 says how many rows were written | stage 4 |
| J7 | Mistype a search and fix it | type `dm:yesterdy`; the message names the mistake and the fix; correct it; the results appear | stage 3, stage 8 |
| J8 | Search when the service is not running | the list says the service is stopped and offers Start the service; after Windows asks for permission and the service starts, the search runs by itself | after install |
| J9 | Search right after Windows starts | while C:\ is still indexing, part 4 says so; the results complete by themselves | after install |
| J10 | Change the language | Language > Deutsch on the menu bar; the window is in German at once, with the same search, results and selection | stage 8 |

## Feedback

Each kind of message has one place, so the user always knows where to look:

- **A lasting fact** about the results or the index is in the status bar.
- **The result of a search** that has no rows, or failed, is in the list's
  empty area, with a button for the fix when there is one.
- **The progress and result of an export** are in status part 3.
- **A failed action that the user started** is a message box owned by the
  window. It names the file and Windows' reason and says what to do, as
  Notepad does for a failed save.
- Nothing shows success for an action whose result the user can see, such as
  Open or Copy.

| Event | Message |
| --- | --- |
| An export cannot write its file | "gozler could not write D:\results.csv. {Windows' reason} Choose another folder or free some space, then export again." |
| The clipboard stays in use by another program | "The clipboard is in use by another program. Try again in a moment." |
| A file to open or show no longer exists | "D:\report.docx no longer exists. Press F5 to search again." |
| An open command from Settings > General cannot start | "gozler could not start {command}. {Windows' reason} Check the command in File > Settings > General." |
| The settings cannot be saved | "gozler could not save its settings to {file}. {Windows' reason} Your changes apply until gozler exits." Shown once per session. |
| At start, another program holds the shortcut | "Another program uses Ctrl+Alt+G, so it no longer shows gozler. Choose another shortcut in File > Settings > Keyboard." Shown the first time the window shows in that session, because at sign-in gozler starts without a window. The tray icon still shows the window. |

### Daemon failures

A failure empties the list, and its empty area shows the message and its
button. The window then recovers by itself: after a daemon restart or a late
service start it searches again without any action from the user (`T4`). A
refused request, a failed owner check and a daemon older than the window stay
on screen until the user changes the search, because the same request would
fail the same way.

| Failure | Message | Button |
| --- | --- | --- |
| The service is stopped | The gozler service is stopped, so gozler cannot search. Start it, and gozler searches again by itself. | Start the service: runs `net start gozler` as administrator, after Windows asks for permission |
| The service is disabled | The gozler service is turned off, so gozler cannot search. Turn it on, and gozler searches again by itself. | Turn on the service: sets it to start automatically and starts it, as administrator |
| The service is not installed, and `gozlerd.exe` is beside the window | The gozler service is not installed, so gozler cannot search. Install it, and gozler searches again by itself. | Install the service: runs `gozlerd.exe install` as administrator |
| The service is not installed, and `gozlerd.exe` is missing | gozler is not fully installed: the gozler service is missing. Download gozler and run its setup. | Download gozler: opens the releases page |
| The service runs but does not answer yet | The gozler service is starting. Your search runs when it is ready. | — (the window polls) |
| The service state cannot be read | The gozler service is not running, and gozler could not find out why: {detail} Check the gozler service in Services. | Open Services |
| Owner check fails | The search service is not the gozler service, so gozler did not connect. Restart Windows, so that the gozler service starts first, then search again. | — (a security check; the fix is not the window's) |
| Daemon older than the window | The installed gozler service is older than this window. Download gozler and run its setup to update the service. | Download gozler |
| The daemon refuses the request | "The gozler service refused this search. Change the search, then try again." then the daemon's English detail, such as a scope on no indexed volume or a regular expression that does not compile | — |
| Connection lost, or the pipe cannot be opened or read | The connection to the gozler service was lost. gozler reconnects when the service answers. | — (the window polls) |

The service states are the ones `gozler` already tells apart for its fix-it
messages, and the buttons run the same commands that `gozler` prints.

## Filter pane

A fixed-width pane on the left, shown and hidden with View > Filters. Its
width follows its widest measured caption. When the window is shorter than the
pane, the pane scrolls, because its list boxes need about 630 px at 100 %, and
at 150 % a 1080p screen cannot hold them with the rest of the window. A "Clear filters" button is at the
top, then one group per filter. Groups combine with AND, and with the typed
text. A group left at its first choice adds nothing.

Each group except Location produces one clause of query text, such as
`ext:mp3;wav`. The window parses that text on its own, with gozler's parser,
and adds its clauses to those of the typed text, as `gozler` does for `-r`
and `-parent`. The parser therefore stays the only owner of the syntax, and
nothing the user types changes what a filter means: an unclosed `"`, a
trailing `|` or a typed `ext:` stays inside the typed text. Location sets the
request's scope, as `-path` does, or with the check box its parent folder, as
`-parent` does.

| Group | Control | Choices | Adds |
| --- | --- | --- | --- |
| Kind | list box | All, Folders, Files, Audio, Archives, Documents, Executables, Pictures, Videos | `folder:`, `file:`, or `ext:` with the group's list |
| Location | list box, and a Choose folder… button below it | Everywhere, each indexed volume by mount prefix, and the chosen folder | the scope, as `-path` |
| | check box | Only directly in this folder | the parent folder, as `-parent`: a `parent:` test that is also the scope |
| Date modified | combo box | Any time, Today, Yesterday, Last 7 days, Last 30 days, Last 365 days, Custom… | `dm:today`, `dm:yesterday`, `dm:7days`, …, `dm:2026-01-01..2026-03-31` |
| Size | combo box | Any size, Empty, Tiny, Small, Medium, Large, Huge, Gigantic, Custom… | `size:empty`, …, `size:1mb..10mb` |
| Attributes | one combo box each: Any, Only, Exclude | Hidden, System, Read-only, Link (reparse point), Compressed, Encrypted | `attrib:h` or `!attrib:h` |

- **Extension groups** live in one table in `gozler-ui`. Starting lists:
  - Audio: `mp3;wav;flac;aac;ogg;opus;wma;m4a`
  - Archives: `zip;7z;rar;tar;gz;bz2;xz;zst;cab;iso`
  - Documents: `pdf;doc;docx;xls;xlsx;ppt;pptx;odt;ods;odp;rtf;txt;md;csv`
  - Executables: `exe;msi;bat;cmd;ps1;com;scr`
  - Pictures: `jpg;jpeg;png;gif;bmp;webp;tif;tiff;heic;svg;ico`
  - Videos: `mp4;mkv;avi;mov;wmv;webm;m4v;mpg;mpeg`
- **Location** lists the volumes from Status. A volume that is not complete
  shows its phase after its name ("D:\ (indexing)"). Choose folder… is a
  button, not a list item, so moving through the list with the arrow keys
  never opens a dialog. It opens the Windows folder picker (`IFileOpenDialog`
  with `FOS_PICKFOLDERS`); the chosen folder becomes a list item and is
  selected. Cancel keeps the previous choice.
- **Custom…** in Date modified and Size opens its dialog when the drop-down
  list closes on it, or when `Enter` is pressed on it. Moving onto it with
  the arrow keys while the list is closed opens nothing and keeps the
  previous clause. Cancel in the dialog returns the combo box to the previous
  choice.
- **Custom date** has two date pickers, From and To, each with a check box.
  From alone adds `dm:>=FROM`, To alone adds `dm:<=TO`.
- **Custom size** has Minimum and Maximum and a unit (KB, MB, GB). Either may
  be empty.
- **A custom range** shows as its own item just above Custom…, such as
  "10/11/2026 to 10/18/2026" or "At least 1 MB", so the combo box always
  shows the filter that is set. Choosing that item again keeps the range.
- Each size word shows its range in the combo box ("Tiny (up to 10 KB)"). The
  ranges are a `match` inside the parser today; `gozler-core` moves them into
  a table that the parser and the window both read, so they cannot drift.
- **No counts beside the choices.** A count is one totals search per choice,
  which multiplies the cost of every keystroke.
- **Other attribute letters** are left to `attrib:` in the search box. Sixteen
  combo boxes would bury the six that people use.

## Inspector pane

A pane under the list, behind a splitter, shown and hidden with View >
Inspector. Its height is saved; the first height fits all seven fields of one
entry. Values are read-only edit controls, so the
user can select and copy any part of them, as in File Explorer's Properties.
Its sums come from the selection's files, so they never need a request of
their own.

### One entry selected

The icon, the name, and the buttons Open, Open file location and Copy as
path. In English the last one reads "Copy a&s path", because the main
window's `P` belongs to Match path ("Words"). The fields follow:

| Field | Value |
| --- | --- |
| Folder | the folder |
| Full path | the full path |
| Type | File or Folder, and the extension |
| Size | the formatted size and the exact bytes; "Unknown" for an unknown size |
| Date modified | local date and time, to the second |
| Attributes | each attribute as a word: Hidden, System, Read-only, … |
| Volume | its mount prefix and phase, such as "C:\ (live)" |

For a **folder**, Size shows its contents: the number of entries below it and
the summed size of its files. The window gets them with a totals search scoped
to the folder, as `gozler -get-folder-size` does. A new selection cancels it
(`T7`). While it runs, the field shows "Calculating…".

### Several entries selected

- How many files and how many folders.
- The summed size of the selected files. Files with an unknown size
  are counted apart: "12.4 MB, 2 files unknown". Folder contents are not
  added, because a selected folder and a selected file below it would be
  counted twice.
- While the selection is read, both say "Reading the selection…".

### Nothing selected

The number of matches.

## Export

File > Export results… opens the Windows save dialog (`IFileSaveDialog`)
with the formats CSV, TSV and TXT. The export writes every match of the
shown search, not only the rows the list reaches. A CSV or TSV file holds
the full path and each visible Size, Date modified, Extension and Attributes
column; it holds the same bytes as `gozler -export-csv FILE` with `-size`,
`-dm`, `-ext` and `-attributes` for the visible columns. A TXT file holds the
full paths, as `gozler -export-txt` writes them. Name and Path are parts of the
full path, so the full path stands for both.

## Settings dialog

File > Settings… opens a dialog with a page list on the left and the page on
the right, as Everything's Options dialog does. Each page has its name as a
heading, its controls in group boxes, and Restore defaults at its bottom
right, which sets that page's controls to their defaults without applying
them. OK, Cancel and Apply follow "Dialogs".

Each group box holds more than one control, as Windows' guidelines ask,
so a page reads as a few groups instead of a column of boxes.

The settings change what gozler does, never how it looks: the look follows
File Explorer and Windows ("Look"), because the owner does not want
appearance settings.

| Page | Group | Setting | Control | Default |
| --- | --- | --- | --- | --- |
| General | — | Language: Windows default, then each shipped language by its own name | drop-down list at the top of the page | Windows default |
| | Starting and closing | Start gozler when I sign in to Windows | check box | off |
| | | Check for updates when gozler starts | check box | off |
| | | Keep gozler running in the notification area when I close its window | check box | on, as in Everything |
| | File Explorer | Show "Search with gozler" on folders | check box | off |
| | | Open folders | edit box with the cue "File Explorer", and Browse… | empty |
| | | Open files | edit box with the cue "The file's default program", and Browse… | empty |
| | | Open file location | edit box with the cue "File Explorer", and Browse… | empty |
| | | Use Total &Commander | button that fills Open folders and Open files | — |
| Search | Search box | Search as you type | check box | on; off: `Enter` runs the search |
| | | Show every file and folder when the search is empty | check box | on, as in Everything; off: an empty search shows nothing (`S6`) |
| | | Select the search text when I click the search box | check box | on |
| | | Select the search text when gozler is activated | check box | off; on: switching to gozler puts the focus in the search box with its text selected |
| Keyboard | Show gozler | Shortcut to show gozler from any program | hotkey control and a "Windows key" check box | none |
| | Commands | Show commands containing | edit box that filters the list | empty |
| | | every command of "Commands" that is one item of the menu bar or the context menu, as "Menu > Command" | list | — |
| | | Shortcuts for the selected command | list, Add…, Edit…, Remove | the "Key" column of "Commands" |
| Index | Drives | one row per fixed NTFS or ReFS drive, with its state and its entry count | list view | — |
| | What the index holds | names, sizes, folder sizes, dates modified and attributes | text: always held, because the search, the filters and the Size column need them | — |
| | | Dates created (service settings plan) | check box; adds the Date created column and the `dc:` search function | off, because it costs service memory |
| | Service | the service's state and memory, and `gozler --status` for the full report | text | — |
| | | Rebuild index (service settings plan) | button with the administrator shield | — |
| Include | — | the preview line: entries indexed now and after OK | text at the top of the page | — |
| | Folders | Folders gozler indexes: each indexed folder and the folders left out inside it | list view, read-only | every drive, nothing left out |
| | | Modify… | button: opens Indexed folders, the folder tree with check boxes | — |
| | | Include new fixed drives automatically | check box | on |
| | Names | Names that are not indexed | edit box: name patterns separated by `;`, such as `*.tmp;node_modules;.git` | empty |
| | | Exclude hidden files and folders | check box | off |
| | | Exclude system files and folders | check box | off |

### Window settings and service settings

- **General, Search and Keyboard** are the window's settings. They belong to the
  person and are saved in their `ui.json`.
- **Index and Include** are the service's settings. One service
  indexes for every user of the PC, so a change there changes what every
  user finds. They are saved by the service for the whole machine, and only
  an administrator changes them. A change to one of these pages puts the
  administrator shield on OK and Apply; pressing either asks Windows for
  permission, then the service saves the change and applies it without a
  restart. Anyone can open the pages and read them; each says at its top:
  "These settings apply to every user of this PC. Changing them needs
  administrator permission." The service settings
  plan owns how the service stores, checks and applies them; it is not
  written yet.
- **What waits for the service settings plan:** the Include page, and the
  Index page's Dates created and Rebuild index, because each needs a
  service request that does not exist yet, and Dates created adds a search
  function that `gozler` does not have ("Goal"). Until that plan is built,
  Settings shows General, Search, Keyboard and the Index page without those
  two controls: the drives, their states and entry counts, what the index
  holds, and the service's state and memory. Anyone can read it, so it
  needs no administrator permission.
- **What a service change does at once:** an unchecked drive or folder
  loses its entries and the service frees their memory; a drive or folder
  checked again is read and shows "indexing" until it is complete; a name
  pattern removes the matching entries; changes below an unchecked folder
  are ignored from then on; a drive with nothing checked also leaves
  Location. The
  command-line `gozler` sees the same index.

### The Include page

The page follows Windows' own Indexing Options, which every Windows user can
meet: a list of the indexed folders with what is left out inside each, a
Modify button that opens a folder tree with check boxes, and a count of what
is indexed.

- **The preview line** at the top of the page says what OK would change
  before it is pressed: "Indexed now: 2,273,581 entries. After OK: about
  1,871,204." It follows every change on the page. The service counts it
  from its index without changing anything, a dry-run count that the
  service settings plan specifies. A folder that is left out today has no
  entries in the index, so checking it again adds "and the folders you
  checked again, counted once they are indexed".
- **Folders gozler indexes** is Windows' "Index these locations" list: one
  row for each indexed folder, a checked drive or a checked folder below an
  unchecked one, with the folders left out inside it, such as `C:\` with
  `C:\Windows; C:\ProgramData`. It shows the include and exclude choices in
  one place, so no choice is hidden deep in a tree. It is read-only, as in
  Windows; Modify… changes it.
- **Modify… opens Indexed folders,** Windows' "Indexed Locations" dialog:
  - a tree of each fixed NTFS or ReFS drive and its folders, opened one level deep
    so the folders to uncheck are in view. A checked folder is indexed with
    everything below it; an unchecked one is left out. A partial check box
    means that some folders below it differ;
  - the text "Uncheck a folder to leave it out of the index. To index only
    some folders, uncheck the drive and check those folders.";
  - the same summary under the tree; selecting a row shows that folder in
    the tree;
  - OK, which puts the choices on the page for the Settings dialog's OK or
    Apply, and Cancel.
- **The nearest check box above an entry decides.** To index only
  `C:\Users\ana`, the person unchecks `C:\` and checks `C:\Users\ana`. To
  index all of `C:\` except `C:\Windows`, they uncheck `C:\Windows`. There is
  no include-only switch to understand.
- **Clicking a check box sets the folder and everything below it,** and
  drops the choices made below it, as Windows' folder trees do. The person
  then changes single folders below.
- The tree lists the folders on the drive, not in the index, so a folder
  that is left out can be checked again.
- The service saves only the folders whose check box differs from the folder
  above, so a new folder is indexed when the folder above it is checked. A
  folder that is renamed or deleted loses its own choice and follows the
  folder above it.
- **Include new fixed drives automatically** off makes a drive that appears
  later unchecked, so the person decides about it in Indexed folders.
- **Names that are not indexed** apply everywhere a folder is indexed. A
  file whose name matches a pattern is left out; a folder that matches is
  left out with everything in it, so `node_modules;.git` leaves out those
  folders on every drive. Exclude hidden and Exclude system leave out a
  matching folder with everything in it in the same way.

### The Keyboard page

The page has two parts, as Everything's Keyboard page has: the one shortcut
that works from any program, and the shortcuts of the window's commands.

**Shortcut to show gozler**

- **A shortcut needs the Windows key, or two of `Ctrl`, `Alt` and `Shift`.**
  A shortcut with one of them, such as `Ctrl+F` or `Shift+A`, would take that
  key from every program. The hotkey control turns such a combination into
  `Ctrl+Alt` and the key, as the Shortcut key field of a Windows shortcut
  does. On keyboards with AltGr, `Ctrl+Alt` with some keys types a
  character, such as `@` with `Ctrl+Alt+Q` on a German keyboard; a person
  whose character stops working chooses another shortcut.
- **The Windows key** is a check box beside the hotkey control, because the
  Windows hotkey control cannot enter it. With it on, the key alone is
  enough: `Win+S`.
- **gozler replaces Windows' own use of a Windows-key shortcut,** such as
  `Win+S` for Windows Search, while gozler runs with that shortcut. It
  changes no Windows setting to do so: it catches the keys before Windows
  does. When the person chooses another shortcut or none, or gozler exits,
  Windows' own `Win+S` works again at once, so there is nothing to restore.
  It also takes effect at once, with no restart and no sign-out, because
  gozler changes nothing that Windows reads only at start. If a shortcut
  ever needs a restart, a sign-out or an Explorer restart to take effect,
  the dialog says so when the person applies it, before they rely on it:
  "Windows uses Win+S until you sign out and sign in again." Stage 7 of the
  plan checks this on Windows 11.
  Windows keeps two exceptions: `Win+L` always locks the computer, so the
  dialog refuses it; and while a program that runs as administrator is in
  front, the keys reach Windows, because Windows does not pass that
  program's keys to gozler.
- **A shortcut with `Ctrl`, `Alt` and `Shift` only** is registered with
  Windows. When another program holds it, the dialog says so; gozler never
  takes such a shortcut from another program.
- **After the window closes,** the shortcut works only while gozler keeps
  running in the notification area (Settings > General, on by default). With
  that off, the page says so under the shortcut, so the person knows before
  they close the window.

**Command shortcuts**

- The list shows every command of "Commands" that is one item of the menu
  bar or the context menu, as its menu and its name, such as "Edit > Find",
  with its shortcuts after it. A submenu of choices, such as Columns, Sort
  by or Language, the match controls, Clear filters and the tray's Show
  window take no command shortcut; the shortcut to show gozler is above. Typing in "Show commands
  containing" keeps only the commands whose name or shortcut contains the
  text, so `ctrl+e` finds the command that has it.
- **Add…** and **Edit…** open a small dialog with one hotkey control. A
  command can have several shortcuts, as Find has `Ctrl+F` and `Ctrl+E`.
- **A command shortcut needs `Ctrl`, `Alt` or a function key,** such as
  `Ctrl+E`, `Alt+Enter` or `F3`. `Alt` with a letter or a digit is refused,
  because those keys open the menus and reach the window's controls
  ("Words"). A key with neither `Ctrl` nor `Alt` is refused unless it is a
  function key, because the search box needs it for typing. Both say "{key}
  is for typing or for the menus. Choose a shortcut with Ctrl, such as
  Ctrl+K, or a function key, such as F3."
- **A shortcut that another command has** is refused with its name, so two
  commands never share one: "Ctrl+E is the shortcut for Edit > Find. Remove
  it there first."
- **The menus and the context menu show the person's shortcuts,** so what a
  menu says is always what the key does. Restore defaults gives every
  command its "Key" from "Commands" again.
- Keys that belong to a control, not to a command, keep their meaning and
  are refused with "{key} belongs to the search box or the list.": `Tab`,
  `Enter`, `Esc`, `Shift+F10`, `Alt+F4`, `Ctrl+V`, `Ctrl+X`, `Ctrl+Z`, and
  `Ctrl` with an arrow, `Home`, `End`, `Page Up`, `Page Down`, `Backspace`
  or `Delete`.

### The General page

- **Keep gozler running in the notification area** is Everything's "Run in
  background": closing the window hides it, the tray icon shows it again,
  and the Keyboard page's shortcut works from any program ("Window
  lifetime"). With it off, closing the window ends gozler, and the shortcut
  works only while the window is open.
- **Check for updates** asks GitHub for gozler's latest release, at most once
  a day, when gozler starts. When a newer one exists, the status bar's last
  part says "gozler 0.4.0 is available" with a link that opens the release
  page; gozler never downloads or installs anything itself. Off, gozler
  makes no network request at all.
- **"Search with gozler" on folders** adds that item to File Explorer's menu
  for folders and for a folder's background. It opens gozler with Location
  set to that folder, through `gozler-ui.exe --location <folder>`; a running
  gozler takes the folder over (second start, "Window lifetime"). Off, the
  item is removed. Uninstalling gozler removes it too.
- **Start at sign-in** shows on only when the `Run` value exists and Windows
  has not turned it off. Task Manager's Startup apps page and Windows
  Settings can turn the entry off without removing it; turning the check box
  on removes that mark, so the check box and Task Manager never disagree.
  The `Run` value names the `gozler-ui.exe` that wrote it, and uninstalling
  gozler removes it.

- **The File Explorer group has three open commands, as Everything's
  Context Menu page has:** Open folders runs for Open (`C1`) on a folder, Open files for Open
  on a file, and Open file location for `C2`. A person who browses with
  another file manager, such as Total Commander, sets them so gozler opens
  everything there.
- **An empty command does what File Explorer does:** a folder opens in File
  Explorer, a file starts its default program, and Open file location opens
  the folder with the entry selected. Each empty box shows that as its cue,
  so the person sees what happens without a separate choice.
- **In a command, `%1` is the entry's full path,** the folder or the file,
  as in Everything and in Windows' own commands. A person moving from
  Everything copies the command from inside `$exec(...)`, such as
  `"C:\Program Files\totalcmd\TOTALCMD64.EXE" /O /A /P=L /L="%1"`.
  Everything's `$openpath("%1")` is the empty Open file location box. With
  several entries selected, the command runs once for each.
- **Browse…** beside each box opens the Windows file dialog for programs and
  writes the chosen program in quotes, followed by ` "%1"`, so the person
  need not type the path to the program. The three buttons have no access
  key, because one letter cannot name three buttons; Tab reaches them.
- **Use Total Commander** fills Open folders and Open files with
  `"<path>" /O /A /P=L /L="%1"`, the owner's Total Commander command from
  Everything, because nobody remembers Total Commander's options. Open
  file location stays as it is, as in the owner's Everything.
  - **gozler finds Total Commander in its own index:** `TOTALCMD64.EXE`, or
    `TOTALCMD.EXE` when there is no 64-bit one. When it finds several, it
    takes the one with the newest date modified, because an older copy is
    usually a backup. The box shows the path, so the person sees which one
    it took and can change it with Browse….
  - **It finds none:** a balloon on the button says "gozler found no Total
    Commander (TOTALCMD64.EXE or TOTALCMD.EXE). Choose it with Browse…." and
    the boxes keep their text. While the service cannot search, the balloon
    gives the service's message instead ("Daemon failures").
  - Like every change on the page, the filled text applies with OK or Apply,
    and Cancel drops it.

### The Index page

- The drive list shows each fixed NTFS or ReFS drive by its mount prefix and label,
  such as "C:\ Windows", with its state (live, indexing, offline, not
  indexed, when nothing on it is checked) and its entry count. What is
  indexed is decided on the Include page. A drive that is offline keeps its
  row here and in Indexed folders, so a choice for a docking drive is not
  lost.
- The page refreshes its states and counts every 2 seconds while it is
  shown.
- **Dates created** is off by default, because it adds memory per entry to
  the service. The page shows the service's memory, so the person sees the
  cost after they turn it on. Folder sizes cost about 8 bytes a folder, so
  they are always held ("Results list").
- **Rebuild index** makes the service read every included drive again, as
  after a restart. It acts at once, so the button itself has the shield and
  asks Windows for permission. It is for an index the person no longer trusts; the
  service keeps itself current without it. While it runs, the drives show
  "indexing" and searches give incomplete results, as status part 4 says.
- **Removable drives, network drives and other filesystems are not in the
  list,** because the service does not index them (DESIGN.md).

### Everything's options, one by one

Every option of Everything's Options dialog, as the owner's screenshots show
it, with what gozler does instead. An option comes in only when it changes
what gozler finds or does, and gozler does not already decide it.

| Everything page | Option | gozler |
| --- | --- | --- |
| General | Language, Download languages | General: Language, and the Language menu (owner decision 6); languages ship with gozler |
| General | Store settings in `%APPDATA%` | no: the window's settings are per user in `%LOCALAPPDATA%`, the service's are per machine |
| General | Check for updates on startup | General: Check for updates when gozler starts |
| General | Start on system startup | General: Start gozler when I sign in |
| General | Run as administrator | no: the window needs no administrator rights; service pages ask when applied |
| General | Everything Service | the setup installs the service; Index shows its state |
| General | Show Search Everything folder context menu item | General: Show "Search with gozler" on folders |
| General | Start menu, desktop and Quick Launch shortcuts, URL protocol, EFU association | no: the setup adds the Start menu shortcut; gozler has no file lists or URLs |
| UI | Run in background, show tray icon | General: Keep gozler running in the notification area when I close its window; the tray icon shows while it runs there |
| UI | Single click tray | always: a click on the tray icon shows the window |
| UI | New window from the tray, new window when running | no: one window |
| UI | Search as you type | Search |
| UI | Select search on mouse click | Search: Select the search text when I click the search box |
| UI | Focus search on activate | Search: Select the search text when gozler is activated |
| UI | Full row select, single click open, underline icon titles | no: appearance, as File Explorer does it |
| Home | Start value of match case, whole word, path, regex, filter, sort and view | no: gozler starts with the last ones, as the owner's Everything does |
| Home | Match diacritics | see "Search box": accents count today |
| Home | Start search, index source, file list, ETP server, link type | no: gozler starts with an empty search on its own index |
| Search | Fast ASCII search | no: always fast |
| Search | Match path when a term has a separator, whole name with wildcards | always, as the shared query syntax says |
| Search | Literal operators, bracket grouping, environment variables, forward slashes, operator precedence | no: the query syntax is one rule shared with `gozler` (DESIGN.md: "Search") |
| Results | Hide results when the search is empty | Search: Show every file and folder when the search is empty |
| Results | Clear selection on search, sort date and size descending first | always ("The selection", "Results list") |
| Results | Close window on execute, open path with a double click in the Path column, automatic scrolling, result list focus | no: the window stays open; Open file location (`C2`) opens the folder |
| Results | Double quote copy as path | no: Copy as path quotes, as File Explorer's does (`C9`) |
| Results | Do not select extension when renaming | no: gozler does not rename |
| Results | Icon, thumbnail and extended information priorities | no: icons come from the shell's image list as rows are shown; no thumbnails |
| View | Double buffer, alternate row colour, row mouseover, update after scrolling, selection rectangle | no: appearance, as File Explorer does it |
| View | Show highlighted search terms | always: the matched part of each name and path is bold ("Results list") |
| View | Show tooltips | always: a cell whose text is cut off shows it whole in a tooltip |
| View | Selected item, result count and size in the status bar | always: the status bar's fixed parts ("Status bar") |
| View | Size format | no: KB, as File Explorer shows sizes |
| Context Menu | Open (Folders), Open (Files), Open Path commands | General: Open folders, Open files and Open file location, with `%1` |
| Context Menu | Other menu items and their visibility | no: the context menu has fixed items |
| Fonts and Colors | Font, size, colours per item state | no: Windows' font and colours, and high contrast |
| Keyboard | Show window hotkey | Keyboard: Shortcut to show gozler |
| Keyboard | Commands, show commands containing, shortcuts with Add, Edit and Remove | Keyboard |
| Keyboard | New window hotkey, toggle window hotkey | no: one window, which the shortcut shows |
| History | Search history and suggestions, run history | no: the owner's Everything has search history off; run history waits for a concrete need |
| Indexes | Database location, compression, multi-user file name | no: the index lives in the service's memory |
| Indexes | Force Rebuild | Index: Rebuild index |
| Indexes | Index recent changes | no |
| Indexes | Index file size, date modified, attributes | always, because the search and the filters need them |
| Indexes | Index folder size | always: a folder's size is the sum of its files ("Results list") |
| Indexes | Index date created | Index: Dates created |
| Indexes | Index date accessed | no: Windows can turn its updates off (`NtfsDisableLastAccessUpdate`), so it can mislead |
| Indexes | Fast size, date, attribute, path and extension sorts | no: gozler sorts each search's results by any column |
| NTFS | Volume list, include in database | Include: the drives at the top of the tree; Index: their state |
| NTFS | Include new fixed volumes | Include: Include new fixed drives automatically |
| NTFS | Include new removable volumes | no: removable drives are not indexed (DESIGN.md) |
| NTFS | Remove offline volumes | always: an offline drive keeps its row and shows "offline" |
| NTFS | Include only (per volume) | Include: uncheck the drive, check the folders |
| NTFS | USN journal enable, size and allocation, load recent changes, monitor changes | no: the service uses each drive's journal as Windows keeps it |
| ReFS | ReFS volumes | always: the service indexes fixed ReFS drives as it indexes NTFS ones |
| Folders | Indexed folders and their rescans | no: owner decision |
| File Lists | File lists and their editor | no: owner decision |
| Exclude | Exclude hidden, exclude system | Include: Names |
| Exclude | Exclude list: folders and filters | Include: the folder tree and the file patterns |
| Exclude | Include only folders | Include: the folder tree |
| Exclude | Include only files | no: the Kind filter narrows a search to file types |
| ETP/FTP Server, HTTP Server | servers and remote connections | no: owner decision |
| (every page) | Restore Defaults | each page |

## Dialogs

Every dialog follows the same rules, so the user never has to look for a
button:

- The buttons are at the bottom right, in the Windows order: OK, Cancel, then
  Apply where there is one. About and Search syntax have one button, Close.
- OK is the default button and `Enter` presses it; `Esc` is Cancel.
- Apply is disabled until a value changes. OK applies and closes. Cancel
  closes and changes nothing that Apply has not already applied.
- Focus starts on the first field. Tab moves through the fields in reading
  order, top to bottom, left to right.
- A dialog checks its input when OK or Apply is pressed. An invalid field
  shows a balloon tip (`EM_SHOWBALLOONTIP` or the control's own) that says
  what to enter, focus moves to it, and the dialog stays open.

| Field | Accepts | When the input is invalid |
| --- | --- | --- |
| Custom date: From, To | a date from the date picker; at least one check box on | From later than To: "From must be on or before To." on To. Neither on: OK stays disabled |
| Custom size: Minimum, Maximum | a number with the Windows decimal separator, and the unit; at least one filled in | not a number: "Enter a number, such as 1.5." Minimum above Maximum: "Minimum must not be larger than Maximum." |
| Settings: Open folders, Open files, Open file location | any text; empty does what File Explorer does | none in the dialog; a command that cannot start shows its message when it is used ("Feedback") |
| Settings: Shortcut | none; or a key with the Windows key; or a key with two of `Ctrl`, `Alt` and `Shift`, which the hotkey control enforces | another program holds it: "Another program uses Ctrl+Alt+G. Choose another shortcut." `Win+L`: "Windows keeps Win+L for locking the computer. Choose another shortcut." |
| Settings: a command's shortcut | a key with `Ctrl`, a key with `Alt` other than a letter or digit, or a function key ("The Keyboard page") | a key a control uses, a key for typing, or another command's shortcut, each with the Keyboard page's message |
| Settings: Names that are not indexed | name patterns with `*` and `?`, separated by `;`, matched against each file and folder name | none: a pattern that matches nothing excludes nothing |

## Windows conventions

The window does what a Windows user expects without being told:

- Every menu item, dialog label and button has an access key ("Words").
- Shift+F10 and the Menu key open the context menu at the focused row.
- `Alt+Enter` opens Properties, as in File Explorer.
- Tab and Shift+Tab move between the search box, the match controls, the
  filter pane, the list and the inspector, in that order.
- The window has a minimum size, and a placement that no monitor shows any
  more opens on the nearest monitor.
- Every colour comes from `GetSysColor`, so high contrast works.
- Typing in the list does nothing ("Not planned"); `Ctrl+F` goes to the
  search box.

## Where settings are saved

`%LOCALAPPDATA%\gozler\ui.json`, found through the Windows known-folder API,
written by the window alone: to a temporary file, then renamed over the old
one. A value that is missing or damaged falls back to its default. A failed
save shows its message ("Feedback"). `gozler-ui --data-dir <folder>` keeps
`ui.json` and `ui-error.log` in another folder instead; the plan's probe uses
it, so a test never touches the owner's settings. It holds two groups:

- `settings`: the settings above, and the language as a language tag, such as
  `de`, or absent for Windows default.
- `window`: placement, column order, widths and visibility, sort, filter pane
  shown and its choices, inspector shown and its height, the match controls, and whether the tray
  notification has been shown.

The keys are the file format, so renaming a key needs a migration.

## Language

The window's text is translated, and the user changes the language while the
window runs (owner decisions 5 and 6).

- **Choosing.** Two places list "Windows default", then each shipped
  language by its own name, such as "Deutsch", so a user who picked a
  language they cannot read can still find their own:
  - the Language menu, the last item of the menu bar, aligned right, with a
    radio mark on the current choice. It is one click from the window and
    stays in the same place in every language;
  - the Language list at the top of Settings > General, beside the other
    settings, as in Everything and TinyTorrent2.

  Both show even when only English ships, so the window has one layout.
- **At once.** A click in the Language menu changes the language
  immediately, with no restart, and saves the choice. In Settings, OK or
  Apply does the same, like every other setting. The menu bar, the
  column headers, the filter pane, the inspector, the status bar and the
  list's message all show the new language. After Apply, the open Settings
  dialog keeps its text and shows the new language the next time it opens,
  because changing the text of five open pages would need a second caption
  pass for one moment.
  The typed search, the results, the selection, the scroll position and the
  focus stay as they were, and no search runs again, because the results do
  not depend on the language (`T12`).
- **Windows default** is the choice until the user makes another. It takes
  the first language in the Windows display language list that the window
  ships, or its parent language (`pt` for `pt-BR`), and English when none
  matches. Numbers, dates and times follow the Windows regional format in
  every language, because the region is a separate Windows choice.
- **What stays in English:**
  - the search syntax, such as `ext:`, `dm:` and the size words, because a
    query must mean the same in every language; Help > Search syntax shows
    it in English, from the same tables as `gozler --help`;
  - the detail that the daemon or a library gives, such as why a regular
    expression does not compile, which follows a translated sentence,
    because the daemon sends English text and no code;
  - paths, names, exported files and copied text, which are data;
  - Windows' own dialogs, such as the folder picker and Properties, which
    follow the Windows display language.
- **The parser's messages** are translated in full, with what to do, because
  a mistyped search is the failure a user meets most.
- **Shipped languages.** English ships. Another language ships when the owner
  adds its catalogue, which changes data only.

## Not planned

| Feature | Why not |
| --- | --- |
| Removable and network volumes | gozler indexes fixed NTFS and ReFS volumes |
| Database location, compression | the index lives in the daemon's memory |
| Date accessed | Windows can turn its updates off, so it can mislead; Date created is an Index setting |
| Find duplicate files, content search, plugins | gozler searches names, paths, sizes, dates and attributes |
| File lists, ETP/FTP and HTTP servers | not planned in DESIGN.md |
| Search and run history | not a gozler feature; it waits for a concrete need |
| Editable context menu items, fonts and colors, size and date format options, and every other appearance setting | the owner wants no appearance settings; Windows and File Explorer decide the look |
| Acrylic or Mica background, and a custom title bar | the common controls paint opaque backgrounds; see "Look" |
| Custom-drawn buttons, two surface colours, taller list rows | polish that needs a paint routine for every state (priority 4); a search list shows more results at Windows' compact height |
| Right-to-left languages, such as Arabic and Hebrew | no such catalogue is planned; the first one adds a mirrored layout ([design: How normal changes land](ui-design.md#how-normal-changes-land)) |
| Translated `gozler` and `gozlerd` output | owner decision 5 |
| Live update of results while files change | the protocol has no change notification; `F5` runs the search again |
| The other hard links of a file | results carry no file ID |
| Rename, delete and drag out of the list | file operations are not search; Open file location gives File Explorer for them |
| Type-ahead in the list | a virtual list must find the row itself (`LVN_ODFINDITEM`), and most rows are not in the window; typing in the list does nothing, and `Ctrl+F` or `Ctrl+E` goes to the search box |
| Scrolling past the first 100,000 rows | owner decision 3 ([design: Rows on demand](ui-design.md#rows-on-demand)) |

## Memory budget

The release `gozler-ui.exe` stays under **16 MB of private bytes** in steady
state: after the empty search, which lists every entry, has been scrolled to
its last reachable row, with no selection held. The full cache of 32,768 rows
takes about 6.5 MB at about 200 bytes a row, because the average full path on
the owner's machine is about 110 characters. The prototype window used 4.6 MB
without the shell's system image list and COM; stage 2 measures the real
baseline. When a commit raises the measured figure by more than 1 MB, its
message says why.

Two actions that the user starts use more, only while they last:

- **A large selection** holds its files: about 20 MB for 100,000 rows, freed
  when the selection changes.
- **A large copy** builds its clipboard data once: about 22 MB for 100,000
  paths. Windows owns that memory after the copy. Whether it still counts in
  the window's private bytes is measured after install.

The window stays responsive during both: no message waits more than 100 ms.

## Acceptance criteria

The window is done when:

- For the same query, its count, its rows in their order, its export bytes and
  its folder sizes equal `gozler`'s, through the same running `gozlerd`.
- Each filter gives the same matches as the `gozler` query it stands for.
- Every command, state, transition and journey in this spec has passed the
  check that its row names.
- The memory budget and its two allowances hold.
- Every key, menu item and button does what this spec says, with the keyboard
  alone as well as with the mouse.
- The window looks right and stays usable at 100 % and 150 % DPI, in English
  and in the pseudo-language, when it moves between monitors with different
  DPI, and in high contrast.
