# Spec: the file manager

**Status, 2026-10-20.** Stages 1 and 2 are built: the engine, Search mode's
file commands, dropping files on a folder row, the File operations window
and the speed settings. The Two panels mode is not built yet. This spec says exactly what it does, so that stages 3 to 8 build
it without asking. When the owner changes a rule, the rule changes here
first.

## Goal

Gozler gets a second mode, Two panels, which works like Norton Commander. A
person who knows Norton Commander or Total Commander presses the same keys
and gets the same result:

| Key | Result |
| --- | --- |
| `F3` | view the file, as text or as hex |
| `F4` | edit: shown in the key bar, not available in this version |
| `F5` | copy to the other panel |
| `F6` | move to the other panel, or rename |
| `F7` | new folder |
| `F8` | delete |
| `Alt+F7` | search with gozlerd, and show the results in the panel |
| `Alt`+a letter | quick search: go to the first name that starts with the letters |
| `Alt+F1`, `Alt+F2` | change the left or the right panel's drive or folder |

Each folder in a panel shows its size, which gozlerd already knows, without
a key press.

The disk work is done by the engine, `gozler-fileops`, which
[the engine spec](fast-copy-delete-spec.md) describes. This spec owns what
the person sees and does.

## Owner decisions

- **A Norton Commander style mode** (owner, 2026-10-19).
- **Norton Commander's keys,** as the table in "Goal" lists them (owner,
  2026-10-20). `F3` views text and binary files. `F4` is Edit, which this
  version does not implement.
- **gozlerd finds the files and gives the folder sizes** (owner,
  2026-10-20).
- **Copy, move and delete as fast as possible**, with the engine's methods:
  parallel deletion and the copy engine that measures faster.
- **Only the asked features** (owner, 2026-10-20): the keys of "Goal",
  with what makes them usable, such as selecting several entries for `F5`.
  "Not planned" lists what this removed.
- **Owner ruling: no Recycle Bin** (2026-10-19). Every delete is permanent
  and fast. A person who wants the Recycle Bin uses File Explorer.
- **Operations run in the background,** with a queue (owner, 2026-10-19).
  "Waits" says which operations wait for which.
- **The UI never waits for disk work** (owner, 2026-10-19): a command
  replies when the person's choice is recorded, the disk work runs on a
  worker, and its progress shows on the operation.

## Choices this spec makes

Each choice has its reason in its section. They are listed here, so that the
owner can change one before stage 3 starts.

1. **Two panels and Search share one window.** View > Two panels switches
   between them, and the window opens in the mode it closed in.
2. **A panel lists the disk, and gozlerd gives only the folder sizes,**
   because gozlerd does not index USB, network and FAT drives, and a file
   manager must browse them.
3. **In Two panels, `Alt`+a letter is quick search.** So there, the menu bar
   opens with `Alt` pressed and released alone, then the letter, or with
   `F9` or `F10`, as in Far Manager. `Alt+F` does not open the File menu
   while a panel's list has the focus. In the path box, and in Search,
   `Alt+F` opens it.
4. **A letter without `Alt` also starts quick search,** because the panels
   have no command line to type into, and File Explorer's lists jump the
   same way.
5. **`F4` Edit is in the key bar, always disabled,** so the key bar has
   Norton Commander's order and the person sees that Edit is planned.
6. **`F3` opens the Viewer, a window of its own.** It chooses Text or Hex
   from the file's first bytes, and `F4` in the Viewer switches the two, as
   in Norton Commander and Far Manager.
7. **`Alt+F7` searches the panel's folder and its subfolders,** as Far
   Manager's search does by default. All drives is one choice in a list
   beside the search box.
8. **`F5` and `F6` run Copy to folder… and Move to folder…, the commands
   Search already has,** and ask for the destination in an edit box, as
   Norton Commander does. With one entry selected, a new name copies or
   moves it under that name. A destination folder that does not exist is
   created, as Total Commander does.
9. **The panel keys are fixed.** Settings > Keyboard lists them and does
   not change them, because they are the keys people choose Norton
   Commander for.
10. **Each panel takes half the window,** with no splitter between them.

## Two panels

### The window

```text
 File  Edit  View  Help                                                 Language
┌─────────────────────────────────────────┐┌─────────────────────────────────────────┐
│[C:] C:\Users\me\Documents               ││[D:] D:\Backup                           │
├─────────────────────────────────────────┤├─────────────────────────────────────────┤
│ Name             Ext        Size  Date  ││ Name             Ext        Size  Date  │
│ ..                                      ││ ..                                      │
│ Invoices                  1,204 KB  ... ││ Photos               48,113,220 KB  ... │
│ Projects              4,404,019 KB  ... ││ notes            txt          4 KB  ... │
│ report           pdf        812 KB  ... ││                                         │
├─────────────────────────────────────────┤├─────────────────────────────────────────┤
│ 1 of 3 selected · 812 KB                ││ 2 items · 4 KB                          │
└─────────────────────────────────────────┘└─────────────────────────────────────────┘
 [ F3 View ][ F4 Edit ][ F5 Copy ][ F6 Move ][ F7 New folder ][ F8 Delete ]
 C:\ · 120 GB free           │                                │
```

| Area | Control |
| --- | --- |
| Top | the menu bar |
| Left half, right half | a panel: the drive button and the path box, the list, and the status line |
| Under the panels | the key bar |
| Bottom edge | the status bar |

The filter pane and the inspector belong to Search, and Two panels does not
show them.

- **The title** is "Gozler" in Two panels, because a title that names one
  panel's folder would be wrong for the other.
- **Switching modes keeps each mode's state:** the search text, its results
  and the panels' folders. While Search shows, the panels stop watching
  their folders and ask gozlerd nothing, and they read their folders again
  when Two panels shows, because a watched folder on a USB drive blocks its
  removal and nobody sees the changes.
- **Show window** (the tray, the shortcut from any program, a second start)
  in Two panels focuses the active panel's list. A start with
  `--location <folder>`, the first or a second, switches to Search, because
  Location is Search's filter.

### A panel

- **The drive button** shows the drive letter, such as `C:`, and opens the
  drive menu (see "The drive menu"), so the mouse reaches it too. A UNC
  folder shows `\\`.
- **The path box** is an edit box with the full path of the panel's folder.
  `Ctrl+L` or a click focuses it. The person types a path and presses
  `Enter` to go there. `%USERPROFILE%` and the other environment variables
  are expanded, as File Explorer does. A relative path, such as `..\Other`,
  is joined to the panel's folder. A file's path shows its folder with the
  file focused. `Esc` puts the folder's path back and returns the focus to
  the list. When the path box loses the focus without `Enter`, such as from
  a click in the list, it also puts the folder's path back.
  `Tab` goes to the other panel's list, as it does from the list.
  The path box decides nothing from the disk: the panel's reading thread
  opens the typed path, and the list keeps the old folder's rows until it
  has. A typed path that cannot be opened then leaves the panel in its
  folder, and a message box says why, such as "D:\Old does not exist." or "Windows could
  not open D:\Private: Access is denied." When it closes, the path box
  has the focus with the typed text selected, so the person corrects it.
- **The list** is a virtual list view in report mode, as Search's is, so a
  folder of a million entries shows at once. It has four columns:

  | Column | Content |
  | --- | --- |
  | Name | the full name, with the extension's icon, as in Search |
  | Ext | the extension without its dot; empty for a folder |
  | Size | a file's size, in the unit of Settings > General's "Show sizes in", as Search's (DESIGN.md: Results list); a folder's size from gozlerd, or "Folder" while gozlerd does not know it |
  | Date modified | as in Search |

  - The first row is `..` in every folder except a drive's root. It has no
    size and no date, and no command acts on it.
  - Folders come first, then files. Each group is sorted by the chosen
    column. Names compare by their folded form, as Search's name sort does,
    so the two modes order names the same.
  - A click on a column header sorts by it, and a second click reverses
    the order; View > Sort by does the same. The first sort by Size or Date
    modified puts the largest or the newest first, as in Search. The header
    shows the arrow.
  - Hidden and system entries are shown, in the gray text color, as File
    Explorer shows them when "Hidden items" is on. A copy of a folder
    copies them too, so the person must see them.
  - Name takes the width that the other three columns leave. Ext, Size and
    Date modified fit their content. The person can drag a column's
    divider; widths are not saved.
- **The status line** under the list counts the panel's entries, without
  `..`, and shows the free space of the panel's drive at its right end, so
  before `F5` the person compares the selection's size with the other
  panel's free space without leaving the panel:

  | Case | Left | Right |
  | --- | --- | --- |
  | Nothing selected | "14 items · 5.2 GB" | "120 GB free" |
  | Entries selected | "3 of 14 selected · 4.2 GB" | "120 GB free" |
  | A search panel | "1,204 results in C:\Users\me", or "3 of 1,204 selected · 4.2 GB" | the searched drive's free space; for All drives, the focused result's full folder, because the Folder column may shorten it. A folder that does not fit loses its middle, so the drive and the last folders stay |

  The size adds the sizes of the counted files and folders, in the short
  form of Search's status bar (DESIGN.md: Status bar), such as "4.2 GB",
  because the Size column's one unit does not compare at a glance with the
  free space. When a counted folder's size is unknown (see "Folder sizes"), the
  size is left out, because a sum that misses a folder would promise room
  that the copy does not have: "2 of 14 selected". The free space is read
  on a thread, and again after each change in the panel.
- **The active panel** has the focus. Its path box shows the system
  highlight colors, as Total Commander shows its active path, except while
  the path box has the focus: then it is a plain edit box, so selected text
  and the caret show. The other
  panel shows its focused row with the gray bar that Windows gives a list
  without the focus. Commands act on the active panel. The other panel's
  folder is the destination of `F5` and `F6`. `Tab` makes the other panel
  active, and so does a click anywhere in it.
- **Reading a folder** runs on the panel's own thread, so a slow network
  share never holds the window. The path box shows the new path at once.
  The list keeps the old folder's rows until the thread has opened the new
  folder, then says "Reading…" until the listing is complete, so a folder
  that cannot be opened leaves the panel where it was. Going to another
  folder cancels the reading. **Reading the shown folder again** (after
  lost changes, after an operation ends, on `Ctrl+R`) keeps its rows, the
  focus, the selection and the scroll position until the new listing is
  complete, then applies it by name, so the person never loses their
  place.
- **The list follows the disk.** The panel watches its folder, not the
  subfolders, with `ReadDirectoryChangesW`, and applies each change by
  name, so a file that another program creates appears at once. When
  Windows reports that it lost changes, the panel reads the folder again.
  The focus and the selection stay on the same names. When the focused
  entry goes, the focus moves to the next row.
- **A USB drive can always be removed.** When Windows asks to remove the
  drive that a panel shows, the panel closes its watch, and a Viewer closes
  its file, so Safely Remove never says the device is in use. When the drive
  is gone, the panel shows its "not ready" state with Retry. When another
  program stops the removal, the panel watches its folder again and reads
  it again, so the list does not silently stop following the disk. Stage 4
  chooses the Win32 notification and builds it for both. A request that
  arrives while the panel is reading the folder stops the reading, but the
  reading thread closes its handle a moment later, so Windows may refuse
  that one request; the next one succeeds. The window never waits for the
  thread, and never closes a handle that another thread may be using.
- **A folder that Windows cannot watch,** such as one on some network
  shares, is read again when an operation in it ends and on `Ctrl+R`.
- **No disk call holds the window or Exit.** The threads that read a folder,
  watch it, read the drive labels and the free space, and read for a Viewer
  can block for a minute on a network share that stopped answering. They
  are ended with `CancelSynchronousIo`, as the engine's are, are not joined
  at Exit, and wake the window only while it is open (DESIGN.md: window rule
  5, as for the update check's thread).
- **A panel remembers** its folder and its sort in `ui.json`, and the window
  remembers which panel was active. The first time, the left panel shows
  Documents and the right panel Downloads, the two folders a person copies
  between most. At start, a folder that no longer exists
  is replaced by the nearest folder above it that exists; the reading thread
  finds it, so a saved folder on a network share never delays the start.
- **Memory:** a panel holds about 100 bytes for each entry of its folder, so
  a folder of a million entries takes about 100 MB while a panel shows it.

What the list area shows:

| State | List area | Status line |
| --- | --- | --- |
| Reading | "Reading…" | empty |
| The folder | its rows | the counts |
| An empty folder | the `..` row only | "0 items" |
| The shown drive is removed, or stops answering | "Drive E:\ is not ready. Insert it again, then select Retry.", with Retry | empty |
| The shown folder is deleted | the nearest folder above it that exists, as File Explorer does | its counts |

A folder that cannot be opened, from `Enter`, the path box or the drive
menu, is not a state: the panel stays in its folder, and a message box says
why, as File Explorer and Total Commander do (DESIGN.md: Feedback, a failed
action that the person started). After `Enter` or the drive menu, the path
box shows the panel's folder again; after a typed path, it keeps the typed
text, as "A panel" says.

### Moving around

| Key | Does |
| --- | --- |
| `Up`, `Down`, `PgUp`, `PgDn`, `Home`, `End` | move the focus |
| `Enter`, a double-click | on a folder: show it in the panel. On `..`: show the parent folder. On a file: Open, with Settings' "Open files" command (DESIGN.md: The Context menu page) |
| `Backspace` | show the parent folder, with the folder the person came from focused |
| `Tab`, `Shift+Tab` | make the other panel active, from the list, the path box, the search box and Look in; from any other control, such as a drive button, return to the active panel. Search's controls are hidden, so `Tab` never reaches them |
| `Alt+F1`, `Alt+F2` | open the left or the right panel's drive menu |
| `Ctrl+L` | focus the path box |
| `Ctrl+R` | read the folder again, and ask gozlerd for the folder sizes again |
| `Esc` | close the quick search box; when it is closed, clear the selection; when nothing is selected, in a search panel, show the folder again. Each press does the first that applies, so the first `Esc` never drops a search whose results are selected |
| `Alt` pressed and released alone, `F9`, `F10` | open the menu bar, because `Alt`+a letter is quick search |

Going to a parent folder focuses the folder that the panel came from, as
Norton Commander and File Explorer do, so `Backspace` then `Enter` returns.

### Selecting entries

A panel has a focused row and selected entries, as Norton Commander has a
cursor and marked files. They are separate: the focus moves without
changing the selection.

- **The list view already keeps both:** the focused row is its
  `LVIS_FOCUSED` row, and the selected entries are its `LVIS_SELECTED` rows.
  So Narrator reads the selection, and `Ctrl`+click, `Shift`+click and
  `Ctrl+A` keep Windows' own behavior. The list owns which rows are
  selected, as in Search (DESIGN.md: The selection).
- **The arrow keys move only the focus.** `Up`, `Down`, `PgUp`, `PgDn`,
  `Home` and `End` are keys of the panel's list (DESIGN.md: window rule 8),
  which move `LVIS_FOCUSED` without changing the selection, as Norton
  Commander's cursor moves without changing its marks.
- **The window draws the rows** in `NM_CUSTOMDRAW` (owner, 2026-10-20,
  the prototype's look A), with system colors so that high contrast works:
  - the focused row of the active panel is a bar in `COLOR_HIGHLIGHT` with
    `COLOR_HIGHLIGHTTEXT`; the other panel's focused row is a gray bar;
  - a selected row's text is `COLOR_HIGHLIGHT` and semibold, as Total
    Commander draws selected files in red;
  - a selected row under the bar keeps the bar's text color and is bold, so
    it still shows as selected.
- **Commands act on the selected entries,** or on the focused row when
  nothing is selected, as in Norton Commander. No command acts on `..`, and
  `..` cannot be selected.

| Key or mouse | Does |
| --- | --- |
| `Insert`, `Space` | select the focused row, or clear it when it is selected, and move the focus down |
| `Shift+Up`, `Shift+Down`, `Shift+PgUp`, `Shift+PgDn`, `Shift+Home`, `Shift+End` | select each row the focus passes, as in Total Commander |
| `Ctrl+A` | select every entry |
| a click | select only that row and focus it, as in File Explorer |
| `Ctrl`+click | select the row, or clear it when it is selected |
| `Shift`+click | select the rows from the focused row to the clicked one |
| a right-click | on a row that is not selected: clear the selection and focus the row, as File Explorer does; on a selected row: focus it and keep the selection; then open the context menu |
| `Esc`, Edit > Clear selection | clear the whole selection (owner, 2026-10-21), because every delete is permanent, so a forgotten selection must take one key to drop |

- **The selection is cleared** when the panel shows another folder, and
  when the person submits a copy, a move or a delete of it, as in Total
  Commander, because the rows of a move or a delete leave the panel.
- **A search panel uses the same keys and the same list selection.** Its
  selection is read from gozlerd, as Search's is.

### Quick search

`Alt` and a letter, a digit or a punctuation key opens the quick search box,
as in Norton Commander. The same key without `Alt` opens it too. These are
keys of a panel's list (DESIGN.md: window rule 8): in the path box a letter
is typed, and `Alt`+a letter opens the menus as in Search. The message loop
takes the list's `WM_CHAR` and `WM_SYSCHAR` before the list sees them, and
lets the `WM_SYSKEYDOWN` through, so releasing `Alt` after a quick search
does not open the menu bar, while `Alt` pressed and released alone still
does. Stage 4 checks this once in the running window.

- **The box** takes the place of the active panel's status line while it is
  open, and shows the typed text, such as "Find: rep". It opens with the
  first character.
- **The focus goes to the first row whose name starts with the typed text,**
  from the top of the list, without regard to case. `..` never matches. The
  text is the start of a name, not a pattern, so `*` and `?` match
  themselves.
- **Each next character extends the text,** with or without `Alt`.
  `Space` is a character while the box is open, so a name with spaces is
  found. A character after which no name would match is not added, so the
  focus never leaves a matching row.
- **`Backspace`** removes the last character, and the focus goes to the
  first row that matches the shorter text.
- **`Up` and `Down`** go to the previous and the next row that matches.
- **`Enter`** closes the box and does what `Enter` does on the focused row.
- **`Esc`** closes the box. The focus stays.
- **Any other key, or a click,** closes the box and then does what it does
  without the box. So `F5` after a quick search copies the focused row.

### The drive menu

`Alt+F1` opens the drive menu of the left panel and `Alt+F2` that of the
right panel, as in Norton Commander, whichever panel is active. The drive
button opens it too. The panel becomes active.

- **The menu** opens under the panel's drive button. It lists every drive,
  "&C:  Windows", with the drive letter as its access key, so `Alt+F1` then
  `D` goes to drive D, as in Norton Commander. A separator follows, then the
  last 10 folders that this panel showed, newest first, with the access keys
  `1` to `9` and `0`, without the folder the panel shows, because choosing
  it would do nothing. The folder list is saved in `ui.json`. The panel's
  own drive has the radio mark, and the menu opens with it highlighted, as
  Norton Commander's drive menu opens on the current drive.
- **A drive** shows the folder that the panel last showed on that drive in
  this session, as Norton Commander remembers a folder for each drive, or
  the drive's root. The drive that the panel already shows goes to its root,
  because otherwise the choice would do nothing.
- **The drives** are read when the menu opens (`GetLogicalDrives`, which
  touches no drive). **The labels** are read on a thread and kept, so a
  sleeping network or optical drive never delays the menu. A label not read
  yet is left out.
- **A drive that is not ready,** such as a card reader with no card or a
  disconnected network drive, is listed. Choosing it leaves the panel in its
  folder, and a message box says "Drive E:\ is not ready." with Windows'
  reason.

### Folder sizes

A folder's size is the size of everything below it, as gozlerd keeps it
(DESIGN.md: Sizes and dates). The panels ask for it; they never count.

- **One request gives every size.** When a panel has read a folder, it asks
  gozlerd for the folder's direct subfolders: a search for `folder:` with
  the folder as its `-parent` scope. The reply gives each subfolder's name
  and size, and the panel matches them to its rows by name, without regard
  to case, as NTFS compares names.
- **Each panel has its own request purpose** (DESIGN.md: window rule 4), so
  one panel's request never cancels the other's, and a slow answer never
  delays the list. The rows show first, and the sizes fill in.
- **The cost is accepted.** gozlerd answers `-parent` by walking the
  folder's subtree, or the whole volume for a large tree or a drive's root.
  Measured with `gozler -parent <folder> folder: -size` against the
  owner's gozlerd (2.4 million entries): 142 ms for `C:\`, 122 to 149 ms for
  large folders and 38 ms for `Documents`, each with about 35 ms of process
  start. The request runs on its own thread, so the sizes appear about a
  tenth of a second after the rows.
- **A size that gozlerd does not know shows "Folder".** That happens when
  the drive is not indexed (USB, FAT, network), Include leaves the folder
  out, the volume's size pass has not completed, or the service does not
  run. The panel never counts a size by walking the folder, because the
  person never waits for a count (owner ruling, 2026-10-20). The status
  bar's right group says why, as Search's does: "C:\ is indexing — sizes
  may be missing." or "E:\ is not indexed, so folder sizes are not
  shown."
- **The panel asks again** after each reading of the folder, 500 ms after
  the last change in the folder, when a file operation whose source or
  destination is in or below the folder ends, and on `Ctrl+R`. The service
  applies a change from the volume's journal after the disk has it, so after
  an operation ends the panel asks once more 2 seconds later. A program that
  writes deeper in the tree changes a size without a change in the shown
  folder; that size is out of date until one of these events.

### Search in a panel

`Alt+F7` or `Ctrl+F` turns the active panel into a search panel, as Norton
Commander's Find File searches from the current folder.

- **The search box** takes the place of the path box, with a drop-down list
  beside it, "Look in": "This folder and subfolders" (the default) or "All
  drives". The drive button stays, so its menu leaves the search with the
  mouse too. The focus goes to the box. While the box is empty, the list
  area says "Type a name to search C:\Users\me and its subfolders.", or
  "Type a name to search all indexed drives." On a drive that gozlerd does
  not index, it shows the "not indexed" message below instead, so the
  person learns it before typing.
- **The search** runs as the person types, 150 ms after the last key, through
  gozlerd, as in Search mode. It uses Search's syntax with the default
  match: Start of words, without Match case and Match path. Search's match
  controls and filters do not apply, because the panel does not show them;
  the typed syntax, such as `case:` or `regex:`, gives the rest.
- **The list** shows Name, Ext, Folder, Size and Date modified. With "This
  folder and subfolders", Folder is the path below the searched folder,
  such as `Projects\Accounts`, or empty for the folder itself; with All
  drives, it is the full path. A path that does not fit loses its middle,
  as File Explorer shortens a path, because the part that tells two results
  apart is its end. Its rows
  come on demand, up to 100,000, as in Search (DESIGN.md: Rows on demand).
  It does not follow the disk; `Ctrl+R` runs the search again. A search
  that finds nothing says "No items match your search.", and with "This
  folder and subfolders" a second line says "Choose All drives in Look in
  to search every indexed drive." The panel
  drops its rows when it leaves the search, so a search panel costs memory
  only while it shows. It leaves the search by `Esc` with nothing selected, `Backspace`, `Enter`,
  or a choice in the drive menu.
- **The sort** is the panel's own, the one its folder uses, so the header
  works as in a folder panel; gozlerd sorts the results. It never changes Search's sort. The Folder header does not sort,
  because the panel's sort also orders its folder, which has no Folder
  column.
- **A folder on a drive that gozlerd does not index** cannot be searched
  with "This folder and subfolders". The list area says "E:\ is not indexed,
  so Gozler cannot search it. Choose All drives in Look in to search the
  indexed drives.", instead of an empty list that reads as "no such file".
- **Keys in the search box:** `Down` and `Enter` go to the list. `Esc` shows
  the folder again, with the row that was focused before the search.
  `Tab` goes to the other panel's list, as from the path box. `Alt+L`, the
  access key of "Look in", goes to the drop-down list, whose `Enter`
  returns to the search box.
- **Keys in the list:**

  | Key | Does |
  | --- | --- |
  | `Enter` | go to the focused result: show the folder that holds it, with it focused, and leave the search, as Far Manager's search and Norton Commander's Find File with Go to do. A folder result shows the folder itself. File > Open opens a file |
  | `Esc` | clear the selection; with nothing selected, show the folder again, as before the search |
  | `Backspace` | show the folder again, as before the search |
  | `Alt+F7`, `Ctrl+F` | go to the search box with its text selected |

- **Commands:** `F3` views the focused result, as in a folder panel. `F5`,
  `F6` and `F8` act on the selected results, or on the focused one (see
  "Selecting entries"). New folder is disabled, because the list shows
  gozlerd's results, not a folder. When the other
  panel is a search panel too, `F5` and `F6` are disabled, because there is
  no destination folder.
- **Quick search** in a search panel looks only at the rows already
  fetched, because the rest are not in the window.
- **A result that a delete or a move takes shows struck through, in
  grey** (see "The person never waits for the disk").
- **States:** the search panel's list area shows Search's messages for no
  results, a search that does not parse and a failure of the service
  (DESIGN.md: States, S5, S7 and S9), and the "not indexed" state above,
  without Search's buttons, because "Look in" is the panel's only filter.

### The key bar

A row of buttons under the panels: `F3 View`, `F4 Edit`, `F5 Copy`, `F6
Move`, `F7 New folder`, `F8 Delete`.

- **Each caption has the key and the word,** as Total Commander's bar does,
  because the bar is there to teach the keys. The word follows the owner
  ruling on button captions.
- **The buttons share the window's width equally.**
- **A click runs the command** on the active panel. The buttons take no
  focus, so the focus stays in the panel, and `Tab` does not reach them,
  because `Tab` switches panels.
- **A button is disabled when its command is,** never hidden. `F4 Edit` is
  always disabled, and its tooltip says "Editing is not available yet."
  `F6 Move`'s tooltip says "Move to the other panel, or rename.", because
  no other place tells a person who wants to rename that `F6` does it.

### Commands in Two panels

| Command | Keys | Enabled when | Acts on |
| --- | --- | --- | --- |
| View | `F3` | the focused row is a file | the focused file, in a Viewer |
| Edit | `F4` | never in this version | — |
| Copy to folder… | `F5` | an entry is selected or focused | the selected entries, or the focused row |
| Move to folder… | `F6` | as Copy to folder… | as Copy to folder… |
| New folder… | `F7` | the panel shows a folder | the panel's folder |
| Delete | `F8`, `Delete`, `Shift+Delete` | an entry is selected or focused | the selected entries, or the focused row |
| Find | `Alt+F7`, `Ctrl+F` | always | the active panel (see "Search in a panel") |
| Open, Properties | `Enter` on a file in a folder panel; none | as in Search | the focused row, or the selected entries |
| Select all | `Ctrl+A` | always | the panel's entries, or the focused box's text |
| Clear selection | `Esc` | an entry is selected | the panel's selection |
| Refresh | `Ctrl+R` | always | the panel's folder and its sizes, or its search |

Copy to folder… and Move to folder… are the commands Search already has
(DESIGN.md: window rule 9, one row per command). Only the question differs:
Search asks with the folder picker, Two panels with the edit box of "Copy
and Move". The key bar still reads `F5 Copy` and `F6 Move`.

The key bar, the menus and the context menus show each command disabled by
these rules, on the active panel's state, so the key bar never offers a key
that does nothing. A second Gozler window, which owns no file operations,
disables Copy to folder…, Move to folder…, New folder… and Delete (see
"Interrupted operations").

Two panels has no clipboard commands. `Ctrl+C`, `Ctrl+X` and `Ctrl+V` in
the path box and the search box edit their text, as in any edit box.

#### Keys

**Keys belong to a mode.** `F5` is Refresh in Search and Copy to folder… in
Two panels. Each mode has its own accelerator table, built when the mode
shows. A key has one of three owners:

- **Two panels' fixed keys** are accelerators of Two panels. They work
  wherever the focus is in Two panels, the path box included, as Norton
  Commander's function keys do.
- **Keys of the focused control** (DESIGN.md: window rule 8) act only in
  that control. A panel's list takes the keys marked "list" below; the path
  box takes `Enter`, `Esc` and `Tab`; a panel's search box takes `Down`,
  `Enter`, `Esc` and `Tab` (see "A panel" and "Search in a panel"). The path box
  and the search box keep every other key for their text.
- **The person's shortcuts** (Settings > Keyboard) for a command that both
  modes have, such as Settings…, work in both modes. In Two panels a fixed
  key wins over a shortcut on the same key, and the menus show the key that
  works in the shown mode, so a menu never shows a key that does something
  else. The Keyboard page says which keys Two panels fixes. Two panels'
  accelerator table also leaves out a person's shortcut on `Alt`+a
  character, such as `Alt+.`, because the message loop translates
  accelerators first and quick search needs those keys. The Keyboard page
  says that such a shortcut works only in Search, and stage 8 changes
  window rule 8's "The Keyboard page refuses every key that a focused
  control uses" to name this exception.

| Key | Two panels | Owner in Two panels | Search |
| --- | --- | --- | --- |
| `F3` | View | fixed | — |
| `F4` | Edit (disabled) | fixed | — |
| `F5` | Copy to folder… | fixed | Refresh |
| `F6` | Move to folder… | fixed | — |
| `F7` | New folder… | fixed | — |
| `F8` | Delete | fixed | — |
| `F9`, `F10` | open the menu bar | fixed; `F10` is Windows' own | `F10` opens the menu bar |
| `Alt+F1`, `Alt+F2` | the left or the right drive menu | fixed | — |
| `Alt+F7`, `Ctrl+F` | Find | fixed | `Ctrl+F`, `Ctrl+E` are Find |
| `Ctrl+R` | Refresh | fixed | — |
| `Ctrl+L` | focus the path box | fixed | — |
| `Delete`, `Shift+Delete` | Delete | list | the same, a key of the list |
| `Ctrl+C`, `Ctrl+X`, `Ctrl+V` | the path box's and the search box's own | the box | Copy, Cut and Paste |
| `Ctrl+A` | Select all | acts on the focused control, as in Search | the same |
| `Enter`, `Backspace`, `Esc`, arrows, `PgUp`, `PgDn`, `Home`, `End` | as "Moving around" says | list | the list's own |
| `Insert`, `Space`, `Shift`+arrows | as "Selecting entries" says | list | — |
| a character, `Alt`+a character | quick search | list | — |
| `Tab` | the other panel's list | list, path box and search box | the next control |

View > Two panels has no default shortcut, as Copy to folder… has none;
the person can give it one.

#### Copy and Move

1. **The question** is a dialog titled Copy to folder or Move to folder, as
   Search's folder picker is. It says "Copy 3 items to:" or "Copy
   report.pdf to:", then an edit box with the other panel's folder, ending
   in `\`, all selected. `F6` on one entry adds the entry's name, with
   the name selected without its extension, as Total Commander's, so the
   person sees that typing a name renames (owner, 2026-10-21). Left as it
   started, that box moves the entry into the other panel's folder, as the
   folder alone does, so a folder of the same name there is merged and not
   entered. When the other panel shows no folder, such as a
   search or a drive that is not ready, the box starts empty, so the person
   types the destination. Its buttons are Copy or Move, the default, and
   Cancel; Copy or Move is disabled while the box is empty. `Enter` starts the operation at once, as in Norton Commander.
2. **The dialog closes at once,** because it decides nothing from the disk.
   It stays open only for a path that is not valid as text, such as one
   with `*` or `?`, and then a message box gives the reason. Each name in
   the path follows "Names the person types" below. The engine's
   prepare, on its own thread, applies this table, so a slow network share
   never holds the dialog:

   | The edit box holds | Result |
   | --- | --- |
   | an existing folder | the entries go into it |
   | a path ending in `\`, or any path while several entries are selected | a folder; it is created, with its missing parents, when it does not exist, as Total Commander does |
   | any other path while one entry is selected | the entry's new full path: it is copied or moved under that name. `F6` with a new name in the same folder renames, as Norton Commander's RenMov does, and the focus and the selection go with the renamed entry |
   | a relative path | joined to the active panel's folder, so `report-old.pdf` copies the file beside itself |
   | a path that Windows refuses when the operation runs | the operation's failure dialog gives Windows' reason (step 7 of "File operations") |

3. **Then the operation goes on as every operation does,** with the
   conflict question and the File operations window, as "File operations"
   says.
4. **The engine needs two changes** for this, both in prepare, which today
   refuses a missing destination (`Refusal::DestinationMissing`) and counts
   conflicts under each entry's own name:
   - **A missing destination folder:** prepare opens the nearest folder
     above it that exists, and takes from it the identity, the volume, the
     disks and the free space that it checks today. The missing folders
     below it are new: they have no conflicts, and the run creates them
     before it writes into them.
   - **One entry to a full path:** prepare checks conflicts under the new
     name, and the run writes the entry under that name. The saved
     operation (engine spec, D19) records the new name, so Continue after a
     crash writes the same file; a saved file without it, from an earlier
     version, loads as before.

   The missing-folder change applies to every copy and move, so Search's
   Copy to folder… also creates a chosen folder that was deleted after it
   was chosen, as Total Commander does.

#### Names the person types

The engine opens paths with `\\?\`, which turns off Windows' own trimming
and its device names, so New folder and the Copy and Move box apply them
to each name they are given, as File Explorer does:

- Trailing dots and spaces are trimmed: `Photos. ` creates `Photos`.
- A device name, `CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9` or `LPT1` to
  `LPT9`, also with an extension such as `nul.txt`, is refused with a
  message box: "Windows uses the name NUL for a device. Choose another
  name."
- A character that no name holds, `< > : " | ? *` or a control character,
  is refused with a message box that says what the balloon of New folder
  says, also when it was pasted. In the Copy and Move box, `\` and `/`
  separate the names, and `:` may follow a drive letter.
- The Copy and Move box reads a trailing `\` or `/` after it expands the
  environment variables, so `%BACKUP%\2026\` names a folder.

#### New folder

`F7` opens a dialog titled New folder, with "Name:" and an empty edit box,
and Create and Cancel. Create is disabled while the box holds no name,
also while it holds only dots and spaces. The name follows "Names the
person types".

- A character that Windows does not allow in a name is refused as it is
  typed, with File Explorer's balloon: "A file name can't contain any of
  the following characters: \ / : * ? " < > |". So `\` is refused too,
  because the box names one folder.
- A name that a row of the panel shows, without regard to case: a message
  box says "Photos is already in this folder. Choose another name.", and
  the dialog stays open with the name selected. It names the row's entry as
  the row spells it, without its folder, because a long path breaks across
  the message box's lines. The check reads the panel's rows in memory, never
  the disk, and leaves out the rows that a delete or a move takes, because
  the person does not see them. A name that another program takes
  meanwhile is refused by the engine, which never replaces it, and the
  failure dialog says so.
- The engine creates the folder (`Request::NewFolder`). When it appears in
  the list, the focus goes to it.

#### Delete

`F8`, `Delete` and `Shift+Delete` ask the Delete question of "File
operations", step 2. After the delete is submitted, the focus moves to the
first row after the deleted ones.

### Menus in Two panels

The menus follow Search's (DESIGN.md: Menus): File acts on the selected
entries and the window, Edit on the selection, View on what the window
shows. Each item shows its key, so the menus teach the keys as the key bar
does.

- **File:** Open, View, Edit, Properties | Copy to folder…, Move to
  folder…, New folder…, Delete | Cancel | Settings… | Exit. Edit is
  disabled, as in the key bar. Cancel stops the job that the status bar
  shows, as its Cancel does (DESIGN.md: Menus).
- **Edit:** Select all, Clear selection | Find
- **View:** Two panels | Sort by ▸ | Refresh | File operations
- **Help** and **Language** as in Search.
- **The context menu on a row:** Open, View | Copy to folder…, Move to
  folder…, Delete | Properties. The context menu on the empty area of a
  list: New folder… | Refresh.

### The status bar in Two panels

It has Search's three groups (DESIGN.md: Status bar):

| Group | Shows |
| --- | --- |
| Left | none, because each panel's status line shows its drive's free space |
| Middle | the running job, as in Search, or what the last operation did ("File operations", step 6) |
| Right | in a folder panel, the index state when it leaves sizes unknown; in a search panel, the search's totals, as in Search |

The memory group follows the right group while Settings > General shows
it, as in Search. Show details does not show, because Two panels has no
inspector.

### The Viewer

`F3` on a file opens the Viewer, a window of its own, as Total Commander's
Lister is. Each `F3` opens a new Viewer, so the person can view two files
side by side. A Viewer is a top-level window that no window owns, so it has
its own taskbar button and stays reachable while the main window is hidden.
Its keys are its own: the message loop gives each message to the keys of
the window it belongs to (see stage 3), so `F3` or `F5` in a Viewer never
act on a panel. Exit in the main window closes every Viewer; closing the
main window to the notification area leaves them open.

```text
 File  View
┌──────────────────────────────────────────────────────────────────┐
│ [General]                                                        │
│ ShowHidden=1                                                     │
│ LastFolder=D:\Backup                                             │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
 settings.ini · UTF-8 · 2.4 KB                                 Top
```

```text
 00000000  4D 5A 90 00 03 00 00 00  04 00 00 00 FF FF 00 00  MZ..............
 00000010  B8 00 00 00 00 00 00 00  40 00 00 00 00 00 00 00  ........@.......
```

- **The title** is the file's name and the command: "settings.ini – View".
- **It opens at once, for a file of any size.** It reads only the part it
  shows, in blocks of 64 KB, on its own thread, and keeps the last 16
  blocks. Until the first block arrives, it says "Reading…". So a 50 GB file
  or a file on a slow network share never holds a window.
- **Removing a USB drive closes the file:** when Windows asks to remove the
  drive that holds the shown file, the Viewer closes its file, as a panel
  closes its watch (see "A panel"), and its area says "The drive was
  removed.", with Retry.
- **It opens the file so that other programs can still write, rename and
  delete it** (`FILE_SHARE_READ`, `FILE_SHARE_WRITE` and
  `FILE_SHARE_DELETE`), so a log file that a program writes opens, and the
  Viewer never blocks a delete.
- **Text or Hex** is chosen from the first 64 KB: Hex when it holds a zero
  byte and no UTF-16 byte order mark, otherwise Text. `F4` switches, as in
  Norton Commander and Far Manager. View > Text and View > Hex show the
  choice.
- **Text:**
  - The encoding comes from the byte order mark: UTF-8, UTF-16 LE or UTF-16
    BE. Without one, the text is UTF-8 when the first 64 KB is valid UTF-8,
    and Windows' ANSI code page otherwise, which the status bar names
    "ANSI".
  - A line ends at CR LF, LF or CR. A tab moves to the next multiple of 8
    columns. Bytes that the encoding cannot read show as �, as Notepad shows
    them.
  - Long lines wrap at the window's width. `F2` and View > Wrap long lines
    switch wrapping off, as in Norton Commander; then `Left` and `Right`
    scroll sideways. In Hex both are disabled, because a hex row is always
    16 bytes. A line longer than 10,000 characters is shown in pieces
    of 10,000, because the Viewer finds a line's start by reading back from
    the shown position.
- **Hex:** each row shows its offset, 8 hex digits, or 16 for a file of 4 GB
  or more; then 16 bytes in hex, with a gap after the eighth; then the same
  16 bytes as characters, with `.` for each byte outside the printable ASCII
  range.
- **Moving:** `Up`, `Down`, `PgUp`, `PgDn`, the mouse wheel and the scroll
  bar. `Home` or `Ctrl+Home` goes to the start, and `End` or `Ctrl+End` to the
  end. The scroll bar maps to the byte position in the file, so dragging it
  to the middle of a 50 GB file shows the middle at once. In Text, a jump
  shows the first line that starts at or after that byte. When the whole file
  shows, the scroll bar is disabled, as Notepad's is.
- **Closing:** `Esc` or `F3`, as in Norton Commander, or File > Close.
  Closing a Viewer that a panel opened activates the main window, with the
  same focused row, so `F3`, `Esc`, `Down`, `F3` walks through the files as
  in Norton Commander. Windows would otherwise activate whichever window
  was behind the Viewer.
- **The status bar:** the file's name, the encoding (Text) and the file's
  size, and at the right the position: "Top", "37 %" or "End" in Text, and
  the first row's offset in Hex.
- **Menus:** File: Close. View: Text, Hex | Wrap long lines.
- **A file that cannot be opened:** the Viewer's area says "Windows could
  not open settings.ini: Access is denied.", with Retry. A file that fails
  while the Viewer reads it says "Windows could not read settings.ini: The
  network name is no longer available.", with Retry.
- **The font** is Consolas, at the height of the window's message font,
  because columns of text and hex must line up.
- **The Viewer remembers** its size and position, and Wrap long lines, in
  `ui.json`.

## File operations

These rules apply to both modes. Search reaches them through its own
commands (see "Search mode"), Two panels through `F5`, `F6`, `F7` and
`F8`.

1. **Copy and Move ask first.** Two panels asks with the destination dialog
   of "Copy and Move"; Search asks with the folder picker of "Search mode".
2. **Delete asks:** "Permanently delete 12 items?" and below it "They won't
   go to the Recycle Bin." Delete and Cancel, with
   Cancel as the default. Gozler never uses the Recycle Bin (owner ruling).
3. **Conflicts are asked once for the whole operation,** before any file is
   written: Replace the files in the destination, Skip these files, Keep
   both files, and Cancel. The engine spec's "Conflicts" says what each does,
   and what happens to a conflict that appears after the question.
4. **The operation then runs in the background.** The person goes on
   browsing, searching and starting other operations.
5. **The File operations window** opens when an operation still runs 2
   seconds after it starts, in both modes, so a short operation shows only
   in the status bar's middle group (owner, 2026-10-21). The time its
   conflict question is open does not count, and an operation that ran when
   the person hid the window does not open it again; a later one does.
   While it runs, the middle group shows it as "Copying Projects to
   D:\Backup · 38 %", with a bar and Cancel; a long path loses its middle,
   so the line ends before the bar with its last folder and its percent,
   and the tooltip shows it whole. It opens without
   taking the keyboard focus, so the keys go on acting on the panel. It is
   one window that never blocks Gozler, modelled on Total Commander's copy window
   with its background queue under it (owner, 2026-10-20). At the top, the
   running operation: its title (Copy, Move or Delete), From and To with the
   current file's paths shortened in the middle, a bar with a percentage for
   the current file and one for the whole operation, files and bytes done of
   the total, the speed in bytes and in files per second, and Pause, Cancel
   and Hide. Every operation runs in the background and never blocks Gozler,
   so Hide only hides the window; the work goes on. A delete deletes while
   it walks and never knows its total, so it shows a moving bar and the
   number of entries deleted so far and per second, with no percentage and
   no time left. A move on the same drive renames and moves no data, so it
   shows its entries done of the total and per second, with no bytes and no
   bar for the current file. Under it, the list of every operation in the
   queue: what it does, its progress in bytes or items, its speed, and
   Cancel. A waiting operation says what it waits for (see "Waits"). A speed
   limit is not planned. The window closes when the last operation ends,
   and the person can close it at any time; View > File operations shows it
   again. The taskbar button shows the progress of all operations together.
6. **When an operation ends,** both panels show the change, through their
   change notifications and the reading that the operation's end starts.
   The status bar's middle group then says what was done, such as "Copied
   3,310 items to D:\Backup" or "Deleted 12 items", until another
   operation starts, in both modes, because after Hide it is the only sign
   that the operation finished (owner, 2026-10-21). The line counts the
   items the operation finished, and adds how many failed or were skipped:
   "Copied 3,310 items to D:\Backup · 1 failed or skipped". A cancelled
   operation says "Copy to D:\Backup cancelled after 120 items", a rename
   "Renamed report.pdf to report-old.pdf", and a new folder "Created
   D:\Backup\Photos", or "Couldn't create D:\Backup\Photos". After Keep
   both, a rename says "Renamed report.pdf", because the window does not
   know the numbered name. An operation that the engine refused, or that a
   full disk stopped, shows no line, because its message box already says
   why; nor does a cancelled rename or new folder, which did nothing. The
   line shows the operation that ended last, also when an older one stays
   in the queue while a list keeps its rows. A
   search list shows the rows that the operation took struck through, in
   grey. "The person
   never waits for the disk" says both.
7. **A failure does not stop the operation.** At its end, a dialog lists
   each item that failed or was skipped, with Windows' reason. An operation
   that the engine refuses before it starts (a destination inside a source,
   a destination with no existing folder above it, such as a drive that is
   gone, too little free space) shows the same dialog with the one reason.
8. **Exit while an operation runs** asks "2 operations are running. Stop them
   and exit?" Close, when Gozler keeps running in the notification area,
   only hides the window, and the operations go on.

### The person never waits for the disk

These rules come from TinyTorrent2's removal redesign
(SynoSoftware/TinyTorrent2#540, its `docs/plans/14-removed-torrents-plan.md`),
where a removed item kept in the list with a flag made 44 readers remember
to skip it, and one forgot.

- **A command replies when the person's choice is recorded.** The disk work
  runs on worker threads, and its progress shows in the File operations
  window. Listing a slow folder, such as a network share, also runs off the
  UI thread; the panel says "Reading…" until it arrives.
- **Prepare runs off the UI thread too.** The File operations window shows
  "Preparing…", and the conflict question appears when prepare is done.
- **An entry that a delete or a move takes leaves the panels and shows
  struck through in the search results at once,** before the disk work
  ends. A row never carries a "being deleted" flag.
- **A folder panel's rows** are the folder listing minus the sources of the
  deletes and moves that wait or run, and of those that ended until the
  panel has read its folder again (engine spec, "The interface", taken
  entries). When an operation ends whose destination is the panel's folder
  or a path in it, such as a new name or a created folder, or whose source
  is in the panel's folder, is that folder or holds it, the panel reads its
  folder once more, because the change notifications and
  the operation's end arrive in no fixed order, and a folder that cannot be
  watched gets none. The first complete listing that the panel started
  after the end, of this folder or of another one the person went to,
  releases the operation: it takes nothing more from the panel. So an entry that a delete
  could not remove, a cancelled move's leftovers and a file copied back to
  a path it was moved from all show, and a deleted row never shows again
  for a moment. Today
  `Queue::taken()` also holds ended operations, which the window releases
  only on a new search in Search (`release_ended`); stage 4 splits the
  queue's set into the waiting and running sources and the ended ones.
- **Each search list keeps the ended operations it shows struck through,**
  and releases them on its own next search, or when a search panel leaves its
  search in any way (see "Search in a panel") and drops its rows. So one list's new search never makes
  another list's struck-through rows show deleted files as present again. Search's list and
  each search panel are separate lists.
- **A search list shows a taken row struck through, in grey,** with the
  text and icon it had, so the person sees what the delete or move took,
  and the selection leaves taken rows out. The list keeps gozlerd's count
  and positions, because gozlerd renumbers its results when its journal
  tail applies the delete, and a list that removed rows itself would then
  map positions to the wrong files. The struck-through rows go when the
  person searches again, or when their chunk is fetched again after
  gozlerd has applied the delete.
- **A rename or a new folder that waits** shows in the File operations
  window; the panel shows the change when the disk has it.

### Waits

The engine's queue decides which operation waits (engine spec, D14): for an
earlier one whose paths overlap, or, for a copy, for an earlier copy on the
same hard disk or network server. Nothing else waits. The window only shows
the result. A waiting operation shows what it waits for: "Waiting for the
copy to D:\Backup". The person can cancel that copy to let the waiting one
start. The person is never asked to wait or to try again.

### Interrupted operations

A crash, a power loss or a forced exit does not mean the person changed
their mind, but the person must know what happened.

- **The engine saves each running operation** before it changes a file
  (engine spec, D19). The window gives it the file
  `%LOCALAPPDATA%\Gozler\operations.json`, or the same name in the
  `--data-dir` folder.
- **At the next start,** the File operations window shows each saved
  operation as "Interrupted", with Continue and Dismiss.
- **Waiting operations are not saved:** they have changed no file, and the
  Exit question says they will not run.
- **Another window offers no file operations.** When "Allow only one Gozler
  window at a time" is off, a second window shows Delete, Cut, Paste, Copy
  to folder…, Move to folder… and New folder… disabled and accepts no drop, as it shows
  no tray icon, because the first window's queue owns the saved file
  (engine spec, "One queue per user session").

### What a stopped operation leaves

The person must always know where each file is. The engine spec's table
of the same name says what each stopped operation leaves on disk. The
window adds two things: a crash also leaves the operation saved as
Interrupted, and the failure dialog and the File operations window say how
many items were done, so the person knows that a result is partial.

### Speed settings

Settings has a File operations page that sets the engine's speed settings
(engine spec, "Speed settings"):

- **"Speed settings"**, a drop-down list of the engine spec's presets, in
  its order: Automatic (the default), Parallel copy, Ring copy, the presets
  taken from other copiers, and Custom. One sentence under the list says
  what the chosen preset does, for example "Copies many files at once with
  Windows' copy function, unbuffered for large files" for Parallel copy,
  or "Reads and writes the data itself through one large buffer, so that a
  hard disk reads and writes in long runs" for Ring copy.
- **The values** of the chosen preset, below the list, one control for each
  setting in the engine spec's table: a drop-down list for each choice
  between the engines and a number box with its unit and range for each
  number. Changing a value selects Custom; choosing a preset sets every
  value. A number outside its range is refused with its range, as the other
  pages refuse a bad value.

No preset is File Explorer's copy, which copies one file at a time. The
values are saved in `ui.json`, and Custom keeps the person's values.

## Search mode

Search mode gets the same operations for its selection, with a folder
picker as the destination:

| Command | Keys | Enabled when |
| --- | --- | --- |
| Copy to folder… | none | rows selected, and the selection is read |
| Move to folder… | none | as Copy to folder… |
| Delete | `Delete`, `Shift+Delete` in the list | as Copy to folder… |
| Cut | `Ctrl+X` in the list | as Copy to folder… |
| Paste | `Ctrl+V` in the list | exactly one folder row selected, and the clipboard holds files |

- **`Delete` and `Shift+Delete` are keys of the list, not accelerators**
  (window rule 8). In the search box they keep deleting and cutting text.
  Both delete permanently after the Delete question of "File operations".
- **Cut and Paste move files with Gozler's engine.** Cut puts the selected
  files on the clipboard as File Explorer's Cut does (`CF_HDROP` with the
  move effect), and Copy already puts them there as File Explorer's Copy
  does, with `CFSTR_PREFERREDDROPEFFECT` set to move or copy. Paste on a
  selected folder row moves files that were cut, in Gozler or in File
  Explorer, and copies files that were copied, with the engine and its
  conflict question. When the move is submitted, and only if the clipboard
  still holds the data that Paste read (`GetClipboardSequenceNumber`), Paste
  sets `CFSTR_PASTESUCCEEDED` to move on that data and empties the
  clipboard, as File Explorer does after its own paste; then it releases
  the data. It never sets `CFSTR_PERFORMEDDROPEFFECT` to move, because that
  value tells the program that cut the files to delete them itself
  ("Handling Shell Data Transfer Scenarios", delete-on-paste). Paste reads
  the clipboard on a thread of its own, which keeps the data until the
  paste is submitted or dropped, because the program that holds the
  clipboard answers each call as slowly as it does. Checking the
  sequence number keeps anything the person copied since. Pasting Gozler's
  files in File Explorer works too, but then File Explorer's engine moves
  them.
  `Ctrl+X` and `Ctrl+V` are keys of the list (window rule 8); in the search
  box they keep cutting and pasting text. Cut rows are not dimmed, because
  the clipboard can change from any program at any time.
- **Files dropped on a folder row** from File Explorer or another program
  are copied or moved into that folder, with File Explorer's rules: a move
  when the files are on the folder's volume, a copy otherwise, `Ctrl`
  forces a copy and `Shift` a move, each only where the source allows that
  effect (`DragEnter`'s effect mask). The row under the cursor is
  highlighted, and the cursor shows "not allowed" over a file row, over
  empty space, and for data without `CF_HDROP`, such as an e-mail
  attachment, because the engine copies paths. Conflicts are asked as for
  Copy to folder….
  - **A move is reported as optimized.** Gozler moves the files later, in
    the background, so `Drop` returns copy, not move, and, unless the source
    works asynchronously (next point), sets `CFSTR_PERFORMEDDROPEFFECT` to
    none. Reporting move would make the
    source delete the originals as soon as the drop returns, before the
    queued move has read them ("Handling Shell Data Transfer Scenarios",
    optimized move).
  - **A source that extracts files for the drag keeps them.** When the data
    supports `IDataObjectAsyncCapability` with asynchronous mode on, `Drop`
    calls `StartOperation`, sets no `CFSTR_PERFORMEDDROPEFFECT`, and the
    window keeps the data object under the operation's id until the
    operation ends; the engine holds no COM object. Without that, a source
    such as an archiver's window deletes its temporary files when the drop
    returns, before the queued copy has read them. When the operation ends,
    Gozler sets `CFSTR_PERFORMEDDROPEFFECT` and calls `EndOperation` with the
    same effect: none for a move, so the source deletes nothing, also not
    the files the move skipped or could not move; copy for a copy. The
    result code says whether the operation succeeded. `EndOperation`'s
    effect has the meaning of `CFSTR_PERFORMEDDROPEFFECT`, so move there
    would tell the source to delete its originals.
  - **The UI thread calls `OleInitialize`,** which `RegisterDragDrop` and the
    OLE clipboard require; the window initializes COM today.
- **Copy to folder… and Move to folder… have no default key.** File
  Explorer has none, and `F5` is Refresh in Search.
- **The folder picker** has the title "Copy to folder" or "Move to folder",
  its OK button says Copy or Move, and it opens at the last folder chosen
  there.
- **File menu:** Open, Open file location, Properties | Copy to folder…,
  Move to folder…, Delete | Export results…, Cancel | Settings… | Exit.
  Cancel, and `Esc` in the results list, stop the job that the status bar
  shows (DESIGN.md: Menus), but `Esc` never stops a file operation, because
  one stray key would leave a copy half done; File > Cancel and the status
  bar's Cancel do. **Edit menu:** Cut and Paste join Copy's group, in File Explorer's
  order: Cut, Copy, Paste, Copy as path, Copy name. **Context menu:** Cut,
  Copy to folder…, Move to folder… and Delete join the Copy group; Paste
  joins it on a folder row.
- **Select all reaches rows the person never saw,** up to 100,000, so the
  Delete question gives the count.
- `docs/DESIGN.md` loses "No command destroys anything, so there are no
  confirmations" from "Feedback", and "Not planned" keeps only "Rename and
  drag out of the window's list" for Search.

## Not planned

- Edit (`F4`): a later plan. The key bar shows it disabled.
- In the Viewer: searching, selecting and copying text, images, and
  printing.
- A command line under the panels, as Norton Commander had: Windows
  Terminal does that.
- Counting a folder's size by walking it, on a drive that gozlerd does not
  index: the person never waits for a count.
- Searching inside files' contents: gozlerd indexes names.
- Archives as folders, FTP, and compare or synchronize folders.
- Drag and drop between the panels and out of them: a later plan. Dropping
  files from other programs onto a folder row in Search is in this plan;
  Two panels accepts no drop.
- Thumbnails and other views than the report list; choosing the panels'
  columns.
- A splitter between the panels.
- In Two panels, as beyond the owner's list (owner, 2026-10-20): rename in
  place (`F6` with a new name renames), the clipboard commands, selecting
  by pattern and inverting the selection (`Num +`, `Num -`, `Num *`),
  swapping the panels (`Ctrl+U`) and the sort keys (`Ctrl+F3` to
  `Ctrl+F6`). In the Viewer: choosing the encoding, and Reload.

## Vocabulary

These terms go into `CONTEXT.md` in stage 3, and "Pane" there stops listing
"panel" under _Avoid_:

- **Two panels**: the mode of the window with two panels, the key bar and
  Norton Commander's keys. The other mode is Search.
- **Panel**: one of the two lists of Two panels, with its path box and its
  status line. It shows a folder, or the results of a search (a search
  panel). Not a pane: a pane is a side area of Search.
- **Active panel**: the panel with the focus; commands act on it.
- **Other panel**: the panel that is not active; its folder is the
  destination of Copy to folder… and Move to folder….
- **Focused row**: the row with the bar, which the arrow keys move.
- **Selected entries**: the entries the person selected with `Insert`,
  `Space`, `Shift` or `Ctrl`; commands act on them, or on the
  focused row when there are none.
- **Quick search**: going to a name by typing its start, with or without
  `Alt`.
- **Drive menu**: the menu of drives and recent folders that `Alt+F1` and
  `Alt+F2` open.
- **Key bar**: the row of `F3` to `F8` buttons under the panels.
- **Viewer**: the window that `F3` opens, which shows a file as text or as
  hex.
- **File operation**: a copy, a move, a delete, a rename or a new folder,
  which the engine's queue runs. Every delete is permanent.
- **Queue**: the file operations that wait, each for an earlier operation
  whose paths overlap its own or whose copy uses the same hard disk.

## Stages

| Stage | Builds | State |
| --- | --- | --- |
| 1 | The engine of [the engine spec](fast-copy-delete-spec.md): delete, copy and move, and the measurement on the owner's machine | built |
| 2 | Search mode's commands: Delete, Copy to folder…, Move to folder…, Cut and Paste, dropping files onto a folder row, the questions, the File operations window, the taskbar progress and the speed settings page | built |
| 3 | The prototype, then the restructuring listed under this table: the Two panels shell, with two empty lists. It changes no behavior of Search | built |
| 4 | The taken entries split, then the panels: reading folders, the columns and sorting, moving around, selecting, quick search, the drive menu, folder sizes, change notifications, removing a USB drive, the status lines, the key bar and the status bar's left group | next |
| 5 | The panels' commands: Copy to folder… and Move to folder… with their dialog and the two engine changes, New folder…, Delete, the menus and the context menus; in both modes, the File operations window opening after 2 seconds and the status bar's line for an ended operation | |
| 6 | The Viewer | |
| 7 | One state for each list, then search in a panel, and the measured memory of a search panel | |
| 8 | Close the plan: the rules move into `docs/DESIGN.md` (its Memory budget included), `CONTEXT.md` and the help, and this file is deleted | |

**Stage 3's restructuring,** before any feature, as `AGENTS.md` ("Writing a
plan", item 4) asks, because each item changes code that every later stage
builds on:

- **Two lists in `Layout::compute`,** the mode switch, and an accelerator
  table for each mode (see "Keys"): each command row names the modes it
  belongs to, and the menus and the accelerator table hold only the shown
  mode's commands.
- **One message loop for several windows:** `win::window::run_message_loop`
  gives each message to the keys of its top-level window
  (`GetAncestor(msg.hwnd, GA_ROOT)`): the main window's accelerators and
  keys, the File operations window's dialog keys, or a Viewer's own keys.
  Today the loop has one extra window (`set_modeless`), and every other
  message goes through the main window's accelerators.
- **The documents:** the vocabulary goes into `CONTEXT.md`. In the engine
  spec, "Taken entries" splits the queue's set as stage 4's first step
  says, and its sentence that a panel releases an operation when it ends is
  replaced by the folder panel's rule. The "Measure" rule for `Space` on a
  folder that gozlerd does not index is removed, because the panels never
  count a size.

**A restructuring whose second user comes later is the first step of that
stage,** before its features and with its own proof, because a structure
built for one user is shaped by a guess (the `codebase-design` skill: one
adapter means a hypothetical seam):

- **Stage 4 starts with the taken entries split** into the sources of
  waiting and running operations and the ended operations that each list
  keeps: each search list until its next search or until it leaves the
  search, and each folder panel, for the operations whose end makes it read
  its folder again, until its first complete listing started after the end
  (see "The person never waits for the disk"). An ended operation leaves
  the queue when no list keeps it.
- **Stage 7 starts with one state for each list:** `core::model::Model` has
  one search text, one set of results, one selection and one taken set,
  and `core::fetch::Purpose` one `Results` and one `Selection`. They become
  one of each for every list (Search's list, the left panel, the right
  panel), so a search in one list never cancels another list's request
  (DESIGN.md: window rule 4). Each panel has its own sort, for its folder
  and its search, and Search keeps its saved sort; a search panel's request
  carries its panel's sort and the default match. Stage 4 adds each panel's
  folder-size purpose, which it needs first.

**The stages run in order, by one implementer.** The Viewer (stage 6) needs
stage 3's message loop and stage 4's handling of a removed drive, and it
edits the files that stages 4 and 5 edit: the View row in `core::commands`,
its settings in `core::settings`, its texts in `en.json` and Exit in the
main window.

- **The prototype comes first in stage 3.** It is one HTML page outside the
  repository, with: Two panels with a folder in each panel, the drive menu,
  quick search, a search panel, the Copy and the New folder dialogs, an
  operation running, the menus, the Viewer in Text and in Hex, and a
  panel's failure states. It does not draw the File operations window, the
  conflict question or the Delete question, because Two panels uses them as
  Search built them. The owner approves it once, because a design change after
  that costs code. It shows only what this plan builds, and its sample data
  looks like the owner's machine or is labeled as samples. Its texts become
  `en.json` keys before code uses them, because texts that existed only in
  the window's prototype were lost from the built window.
- **A departure from the approved prototype is the owner's decision.** It
  is written into this spec with its reason before it is built.
- **No separate design document.** This spec settles every behavior. The
  implementer designs the code in stage 3 with the `codebase-design` skill
  and records it in the crate's doc comments, and stage 8 moves the rules
  into `docs/DESIGN.md`. The code's design follows `docs/DESIGN.md`:
  decisions in `core`, `unsafe` in `win`, and the window rules.

### What each stage proves

| Stage | Unit tests in `core` | Screenshots |
| --- | --- | --- |
| 3 | which commands and keys each mode takes; the layout of two lists at the minimum size and at 150 % DPI | Search unchanged; Two panels empty |
| 4 | first, the taken entries: an ended delete still hiding its rows from a folder panel until a listing started after its end arrives, whichever of the change notifications and the end comes first, while a listing started before the end releases nothing, and the panel then showing an entry that the delete could not remove; then sorting with folders first; the selection rules (focus moves without changing the selection); quick search (first match, a refused character, `Up` and `Down`); applying a change notification by name (the focus moves to the next row when its entry goes, the selection keeps its names); the path box's relative paths, environment variables and a file's path; matching folder sizes to rows by name without regard to case; the status line's sum, left out when a folder's size is unknown | two panels with sizes; quick search; the drive menu; each failure state |
| 5 | the refused names of New folder; the enabled state of each command | the Copy to folder, Move to folder and New folder dialogs |
| 6 | the Text or Hex choice; encoding detection; finding a line's start from any byte; the hex row's text; the scroll position of a byte | a text file, a UTF-16 file and an executable in Hex |
| 7 | first, a search in one list leaving another list's request alone; then leaving a search with `Esc` and with `Backspace`; each list releasing only its own ended operations | a search panel beside a folder panel; the "not indexed" state |

The engine's two changes in stage 5 get tests on folders that each test
creates, one for each row of the Copy and Move table; no test touches the
owner's files. Two Win32 behaviors are checked in the running window,
because no unit test reaches them: that releasing `Alt` after a quick
search does not open the menu bar, which needs the window in front and so
is on the owner's after-install list (see "How each stage runs"); and in
stage 6, that closing a Viewer activates the main window with the same
focused row.

### How each stage runs

- One implementer keeps its context through stages 3 to 7 and fixes the
  review's findings. The reviewer reads and reports, and rechecks the fixes
  by reading.
- **The stage review compares the stage's screens with the prototype's.**
  Each difference is fixed, or written into this spec with its reason as an
  owner decision.
- A stage saves the screenshots of the screens it adds, for the owner to
  review later. No end-to-end probe and no rerun of earlier stages.
- **A stage brief names the files that other sessions own,** because other
  sessions edit this repository at the same time.
- **A defect is reproduced before it is reported.** Paths are tested from
  PowerShell, because Git Bash rewrites `\\?\` paths.
- **A setup built for a check uses `GOZLER_RELEASE=1`,** because each other
  local run of `installer\Build.ps1` raises the patch version in
  `Cargo.toml`.
- No check waits a fixed time or needs the window in front.
- Nothing waits for the owner. The owner checks these after install: a
  panel on a network share and on a USB drive (no folder sizes, "Folder"
  shown); Safely Remove of that USB drive while a panel and a Viewer show
  it (Windows removes it without saying it is in use, and the panel then
  shows "not ready" with Retry); releasing `Alt` after a quick search
  (the menu bar stays closed); a move between two disks; and the Viewer on
  a file larger than 4 GB.
- One `check.ps1` run when the stage is reviewed (`AGENTS.md`, "Before you
  finish").
