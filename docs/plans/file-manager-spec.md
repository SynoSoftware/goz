# Spec: the file manager

**Sketch, 2026-10-19.** This file says what the file manager is, and how its
spec, design and plan get written and built. It is not the finished spec yet.
The rules here are defaults; the owner corrects them before stage 2.

## Goal

Gozler gets a second mode, Two panels, in the style of Norton Commander and
Total Commander. A person browses folders in two panels and copies, moves
and deletes between them with the keyboard, as fast as the disks allow.
Search stays Gozler's center: a panel can show a search instead of a folder,
so the person finds files anywhere and copies them to the other panel.

The disk work is done by the engine, `gozler-fileops`, which
[the engine spec](fast-copy-delete-spec.md) describes. This spec owns what
the person sees and does.

## Owner decisions

- **A Norton Commander style mode** (owner, 2026-10-19).
- **Copy, move and delete as fast as possible**, with the engine's methods:
  parallel deletion and the copy engine that measures faster.
- **Owner ruling: no Recycle Bin** (2026-10-19). Every delete is permanent
  and fast. A person who wants the Recycle Bin uses File Explorer.
- **Operations run in the background,** with a queue (owner, 2026-10-19).
  "Waits" says which operations wait for which.
- **The UI never waits for disk work** (owner, 2026-10-19): a command
  replies when the person's choice is recorded, the disk work runs on a
  worker, and its progress shows on the operation.

## Defaults to confirm

These are this sketch's choices. Each has a reason; the owner changes any of
them before stage 2.

1. **The two modes share one window.** View > Two panels switches between
   Search and Two panels, and the window opens in the mode it closed in.
   The filter pane and the inspector belong to Search.
2. **A panel lists the disk, and gozlerd gives the folder sizes.** The list
   comes from the folder itself, so USB, network and FAT drives work, which
   gozlerd does not index. On an indexed volume, one search with `-parent`
   for the shown folder gives every subfolder's folder size at once. Total
   Commander makes the person press Space for each one.
3. **A panel shows a folder or a search.** `Alt+F7`, as in Norton Commander
   and Total Commander, or `Ctrl+F`, opens a search box over the active
   panel. Its results fill that panel, with the same rows, order and limit
   as Search mode. `Esc` returns the panel to its folder. `F5` copies the
   selected results to the other panel's folder.
4. **Keys follow Norton Commander,** inside Two panels only; Search keeps
   its keys.
5. **Rename and New folder are in the first version,** because a file
   manager without them sends the person to File Explorer for the most
   common acts. View (`F3`) and Edit (`F4`) are not: Open does that.
6. **Operations wait only for overlapping paths, or for a copy on the same
   hard disk,** as the engine's queue decides (engine spec, D14). Deleting folder
   A never waits for a copy of folder B.
7. **One File operations window shows every operation,** as File Explorer
   groups its copies in one window. It does not move the main window's
   controls, which a pane that appears by itself would (DESIGN.md: "Nothing
   moves by itself").
8. **A panel shows the disk as it is.** A copy writes each file under its
   final name (engine spec D10), so a file that an interrupted copy left
   unfinished looks like any other file, with the time it was last written.
   Continue in the File operations window finishes it.

## The window in Two panels

| Area | Control |
| --- | --- |
| Top | the menu bar |
| Left and right | two panels, each with a path box, a list and a status line |
| Bottom edge | the status bar |

- **A panel's path box** shows the folder's full path. The person types a
  path and presses `Enter` to go there. A path that does not exist shows the
  message in the panel's list, as Search shows S8.
- **A panel's list** is a list view in report mode, with Name, Extension,
  Size and Date modified, sortable by a header click. Its first row is `..`
  in every folder but a root. A folder sorts before the files, as in Total
  Commander.
- **A panel's status line** shows "3 of 120 selected · 12.4 MB of 4.8 GB",
  as Norton Commander's mini status does.
- **The active panel** has the focus and a highlighted path box. The other
  panel is the destination of Copy and Move.
- **A folder's size** shows when gozlerd knows it, otherwise "<Folder>", as
  Total Commander shows `<DIR>`. Space on a folder in a volume that gozlerd
  does not index counts its size with the engine's walk.
- **The list follows the disk.** A change notification on each shown folder
  (`ReadDirectoryChangesW`) updates the list, so a file another program
  creates appears without `Ctrl+R`.
- **Each panel remembers** its folder, sort and column widths in `ui.json`.

## Keys in Two panels

| Key | Command | As in |
| --- | --- | --- |
| `Tab` | switch the active panel | Norton Commander |
| `Enter` | open: a folder is entered, a file opens with Open | Norton Commander |
| `Backspace` | go to the parent folder | Total Commander |
| `Insert`, `Space` | select the focused row and move down | Norton Commander, Total Commander |
| `Num +`, `Num -`, `Num *` | select by pattern, clear by pattern, invert | Norton Commander |
| `F2` | rename in place | File Explorer, Total Commander |
| `F5` | copy to the other panel's folder | Norton Commander |
| `F6` | move to the other panel's folder | Norton Commander |
| `F7` | new folder | Norton Commander |
| `F8`, `Delete`, `Shift+Delete` | delete, after a question | Norton Commander, File Explorer |
| `Alt+F1`, `Alt+F2` | choose the left or right panel's drive | Norton Commander |
| `Alt+F7`, `Ctrl+F` | search in the active panel | Norton Commander, Gozler |
| `Ctrl+R` | read the folder again | Total Commander |
| `Ctrl+U` | swap the panels | Norton Commander |
| a letter or digit | jump to the next name that starts with it | File Explorer |

`F5` is Refresh in Search. In Two panels it is Copy, so Refresh there is
`Ctrl+R`. The Keyboard page shows the commands of both modes.
`Shift+Delete` deletes too, so a File Explorer habit gives the result the
person expects.

## Menus and status bar in Two panels

The menus follow Search's (DESIGN.md, "Menus"): File acts on the selected
files and the window, Edit on the clipboard and the selection, View on what
the window shows.

- **File:** Open, Properties | Copy, Move, Rename, New folder, Delete |
  Settings… | Exit
- **Edit:** Copy, Copy as path, Copy name | Select all, Select by pattern…,
  Clear by pattern…, Invert selection | Find
- **View:** Two panels | Swap panels | Refresh | File operations
- **Help** and **Language** as in Search.
- **The context menu on a row:** Open | Copy, Move, Rename, Delete | Copy as
  path | Properties. Copy and Move here mean to the other panel; the
  clipboard's Copy stays in Edit.
- **The status bar** has the same four parts as Search's. Part 1 shows the
  active panel's folder and its volume's free space, part 2 the selection as
  in Search, part 3 the running operations, "2 operations · 41 %", which
  opens the File operations window on a click; the percentage counts the
  operations with a total, and a delete, which has none (engine spec, "The
  interface"), shows its count of deleted items in its own row, and part 4 the index state of
  the shown volumes.

## File operations

`F5`, `F6` and `F8` act on the active panel's selection, or on the focused
row when nothing is selected, as in Norton Commander.

1. **Copy and Move ask first,** as Norton Commander does: "Copy 3 items to
   D:\Backup\" with the destination in an edit box that the person can
   change, and OK and Cancel. `Enter` starts the copy at once.
2. **Delete asks:** "Delete these 12 items? Gozler deletes them permanently;
   they do not go to the Recycle Bin." Delete and Cancel. Gozler never uses
   the Recycle Bin (owner ruling).
3. **Conflicts are asked once for the whole operation,** before any file is
   written: Replace the files in the destination, Skip these files, Keep
   both files, and Cancel. The engine spec's "Conflicts" says what each does,
   and what happens to a conflict that appears after the question.
4. **The operation then runs in the background.** The person goes on
   browsing, searching and starting other operations.
5. **The File operations window** opens when an operation starts. It is one
   window that never blocks Gozler, modelled on Total Commander's copy window
   with its background queue under it (owner, 2026-10-20). At the top, the
   running operation: its title (Copy, Move or Delete), From and To with the
   current file's paths shortened in the middle, a bar with a percentage for
   the current file and one for the whole operation, files and bytes done of
   the total, the speed in bytes and in files per second, and Pause, Cancel and Hide. Every operation runs in the
   background and never blocks Gozler, so Hide only hides the window; the
   work goes on. A delete deletes while it walks and never knows
   its total, so it shows a moving bar and the number of entries deleted so
   far and per second, with no percentage and no time left. A move on the
   same drive renames and moves no data, so it shows its entries done of
   the total and per second, with no bytes and no bar for the current file. Under it, the list of
   every operation in the queue: what it does, its progress in bytes or
   items, its speed, and Cancel. A waiting operation says what it waits for
   (see "Waits"). A speed limit is not planned. The window closes when
   the last operation ends, and the person can close it at any time; View >
   File operations shows it again. The taskbar button shows the progress of
   all operations together.
6. **When an operation ends,** both panels already show the change, through
   their change notifications. A search panel shows the rows that the
   operation took blank (see "The person never waits for the disk").
7. **A failure does not stop the operation.** At its end, a dialog lists
   each item that failed or was skipped, with Windows' reason. An operation
   that the engine refuses before it starts (a destination inside a source,
   a destination that is gone, too little free space) shows the same dialog
   with the one reason.
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
- **Prepare runs off the UI thread too.** Counting a large tree takes
  seconds, so the File operations window shows "Preparing…" and the
  conflict question appears when the count is ready.
- **An entry that a delete or a move takes leaves the panels and shows
  blank in the search results at once,** before the disk work ends. A row never carries a
  "being deleted" flag.
- **A panel's rows** are the folder listing minus the entries the engine's
  queue reports as taken (engine spec, "The interface", taken entries).
  When an operation fails for an entry, the entry is back in the next
  listing, because the disk still holds it.
- **A search list shows a taken row blank,** with empty cells, as it shows
  a row not fetched yet (DESIGN.md, "Rows on demand"), and the selection
  leaves taken rows out. The list keeps gozlerd's count and positions,
  because gozlerd renumbers its results when its journal tail applies the
  delete, and a list that removed rows itself would then map positions to
  the wrong files. The blank rows go when the person searches again, or when
  their chunk is fetched again after gozlerd has applied the delete. The
  prototype shows this.
- **A rename or a new folder that waits** shows in the File operations
  window; the panel shows the change when the disk has it.

### Waits

The engine's queue decides which operation waits (engine spec, D14): for an
earlier one whose paths overlap, or, for a copy, for an earlier copy on the
same hard disk or network server. Nothing else waits. The window only shows the result. A waiting
operation shows
what it waits for: "Waiting for the copy to D:\Backup". The person can
cancel that copy to let the waiting one start. The person is never asked to
wait or to try again.

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
  to folder… and Move to folder… disabled and accepts no drop, as it shows
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
  ("Handling Shell Data Transfer Scenarios", delete-on-paste). Checking the
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
  attachment, because the engine copies paths. Conflicts are asked as for Copy to folder….
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
    operation ends; the engine holds no COM object. Without that, a source such as an
    archiver's window deletes its temporary files when the drop returns,
    before the queued copy has read them. When the operation ends, Gozler
    sets `CFSTR_PERFORMEDDROPEFFECT` and calls `EndOperation` with the same
    effect: none for a move, so the source deletes nothing, also not the
    files the move skipped or could not move; copy for a copy. The result
    code says whether the operation succeeded. `EndOperation`'s effect has
    the meaning of `CFSTR_PERFORMEDDROPEFFECT`, so move there would tell
    the source to delete its originals.
  - **The UI thread calls `OleInitialize`,** which `RegisterDragDrop` and the
    OLE clipboard require; the window initializes COM today.
- **Copy to folder… and Move to folder… have no default key.** File
  Explorer has none, and `F5` is Refresh in Search.
- **The folder picker** has the title "Copy to folder" or "Move to folder",
  its OK button says Copy or Move, and it opens at the last folder chosen
  there.
- **File menu:** Open, Open file location, Properties | Copy to folder…,
  Move to folder…, Delete | Export results… or Cancel export | Settings… |
  Exit. **Edit menu:** Cut and Paste join Copy's group, in File Explorer's
  order: Cut, Copy, Paste, Copy as path, Copy name. **Context menu:** Cut,
  Copy to folder…, Move to folder… and Delete join the Copy group; Paste
  joins it on a folder row.
- **Select all reaches rows the person never saw,** up to 100,000, so the
  Delete question gives the count.
- `docs/DESIGN.md` loses "No command destroys anything, so there are no
  confirmations" from "Feedback", and "Not planned" keeps only "Rename and
  drag out of the window's list" for Search.

## Not planned

- A command line under the panels, as Norton Commander had: Windows
  Terminal does that.
- View (`F3`) and Edit (`F4`) inside Gozler: Open does that.
- Archives as folders, FTP, and compare or synchronize folders.
- Drag and drop between the panels and out of the window: a later plan.
  Dropping files from other programs onto a folder row in Search is in
  this plan.
- Thumbnails and other views than the report list.

## Vocabulary

These terms go into `CONTEXT.md` before stage 2:

- **Panel**: one of the two lists of Two panels; it shows a folder or a
  search.
- **Active panel**: the panel with the focus; its selection is what a
  command acts on.
- **Other panel**: the panel that is not active; its folder is the
  destination of Copy and Move.
- **File operation**: a copy, a move, a delete, a rename or a new folder,
  which the engine's queue runs. Every delete is permanent.
- **Queue**: the file operations that wait, each for an earlier operation
  whose paths overlap its own or whose copy uses the same hard disk.

## How the documents get written and built

The window's documents had 3,126 lines and took about 18.5 hours to build;
the es superset's had about 300 lines and took about 5.6 hours. The
difference was what the documents asked for around the code (`AGENTS.md`,
"Writing a plan"). These documents follow those lessons.

### The documents

| Document | Holds | Does not hold |
| --- | --- | --- |
| `file-manager-spec.md` (this file) | what the person sees and does: modes, keys, commands, dialogs, failures and their partial results | how the code does it |
| `file-manager-design.md` | the panel model in `gozler-ui`'s `core`, the list's data, change notifications, the folder-size request, how the window shows the queue's states (the queue itself is the engine spec's), the File operations window, and the restructuring of `Layout::compute` for two lists | anything the spec already says |
| `file-manager-plan.md` | the stages, the proof of each, and the After install list | the process of `AGENTS.md`, which it names instead of repeating |

- **Rules with their reasons, no IDs** except for the cases a test names.
  The window's spec gave IDs to every command, state, transition and
  journey, and each ID became something to prove.
- **One prototype**, an HTML page of the Two panels window, the File
  operations window, Search mode with its new commands, the Delete and
  conflict questions, and the File operations page of Settings, outside the
  repository. The owner approves it once,
  before stage 2, because a design change after that costs code. It shows
  only what this plan builds; a screen for later work is left out or
  labeled "Later" on the screen, because the window's prototype showed
  postponed pages that the owner later took for forgotten work. Its sample
  data looks like the owner's machine, or is labeled as samples: the
  window's prototype showed a drive label "Windows", so a drive without a
  label looked like a defect.
- **The prototype's text becomes `en.json` before stage 2.** Every hint,
  caption and state text on the approved screens gets its catalogue key.
  Stage 7, when every screen exists, adds a test that fails when a key is
  used by no screen; earlier, the keys of screens not yet built would fail
  it on every run. The built window lost
  six texts of its prototype, such as "Press the keys together. Backspace
  removes the shortcut.", because they existed only in the prototype and
  the reviews compared the code with the spec.
- **A departure from the approved prototype is the owner's decision.** It
  is written into this spec with its reason before it is built, never made
  silently: the window dropped its Folder sizes check box for a good reason,
  but nobody asked the owner.
- **Names first.** The vocabulary above and every key in `en.json` are
  settled in the spec, not during a stage.

### The stages

1. **The engine** of [the engine spec](fast-copy-delete-spec.md): delete,
   copy and move, and the measurement on the owner's machine.
2. **Search mode's commands,** straight after the engine, because Search's
   result list needs no panels: Delete, Copy to folder… and Move to folder…
   on the selected rows, Cut and Paste, dropping files onto a folder row,
   the dialogs, the
   File operations window, the taskbar progress and the speed settings
   page. The owner uses the engine on real files from here on, before any
   panel work starts.
3. **The restructuring:** two lists in `Layout::compute`, the mode switch,
   and keys that belong to a mode. It changes no behavior of Search, and is
   its own stage with its own proof, because restructuring inside a feature
   stage made the window's stages and reviews larger.
4. **The panels:** folders, navigation, selection, folder sizes, change
   notifications, Rename and New folder.
5. **The panels' operations:** `F5`, `F6`, `F7`, `F8` and the panel's
   menus, on the dialogs and the File operations window of stage 2.
6. **Search in a panel.**
7. **Close the plan:** the rules move into `docs/DESIGN.md`, and the plan
   files are deleted.

### How each stage runs

- One implementer keeps its context through every stage and fixes the
  review's findings. The reviewer reads and reports, and rechecks the fixes
  by reading. An adversarial review runs only in stage 2, where a wrong
  drop or paste result makes another program delete the person's files.
- A rule that `core` decides is proven by a unit test. The engine is proven
  by tests on folders that each test creates; no test touches the owner's
  files.
- A stage saves one screenshot set of the screens it adds, for the owner to
  review later. No end-to-end probe and no regression run of earlier
  stages.
- **The stage review compares the stage's screens with the prototype's.**
  Each difference is fixed, or written into this spec with its reason as an
  owner decision. The window's reviews compared only code with spec, and 14
  differences from its prototype went unnoticed.
- **A stage brief names the files that other sessions own,** because other
  sessions edit this repository at the same time: while the window's fixes
  ran, another session added catalogues and edited `core/text.rs` in the
  same crate.
- **A defect is reproduced before it is reported.** Paths are tested from
  PowerShell, because Git Bash rewrites `\\?\` paths, and a screen's filters
  are read before its result is diagnosed. Two of the window's alarms were
  false for these two reasons; file operations test paths far more than
  search did.
- **A setup built for a check uses `GOZLER_RELEASE=1`,** because each other
  local run of `installer\Build.ps1` raises the patch version in
  `Cargo.toml`.
- No check waits a fixed time or needs the window in front.
- Nothing waits for the owner. What only the owner can check goes into
  After install: moves between two disks, a hard disk, a network share.
- One `check.ps1` run when the stage is reviewed (`AGENTS.md`, "Before you
  finish").
