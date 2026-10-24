# The window: after install

The owner's checklist for the window, from the stages of the window's plan,
and for the copy, move and delete engine. Each item needs the installed
Gozler, the owner's own settings, registry or taskbar, a second monitor, or
for the engine other disks or an elevated run, so no agent runs it. Delete this file when the
owner has run it, and file an issue for each item that fails.

## Install

- The Start menu shortcut "Gozler" starts the window, and the window searches
  through the installed `gozlerd`.
- The setup's last page offers "Launch Gozler". The window it starts runs
  without administrator rights: Task Manager's Details page shows it with
  Elevated "No".
- Against the installed `gozlerd`, `gozler ext:cs -n 3`,
  `gozler -get-result-count ext:cs`, `gozler --status` and
  `gozler -path X:\no-such-folder x` exit with the codes they gave before, and
  their output has the same form: three full paths, one number, the same
  status headings, the same message.
- For a folder, `gozler <folder> -size` shows the same size as
  `gozler -get-folder-size <folder>`; after a file below it grows, both show
  the new sum.
- After uninstall, neither `gozler` value under the current user's `Run` and
  `Explorer\StartupApproved\Run` keys is left, and File Explorer's menu on a
  folder no longer shows "Search with Gozler".

## The service

- With the service stopped, the list says that the Gozler service is stopped
  and offers Start the service; after Windows' permission prompt, the service
  starts and the search runs by itself.
- Restart the service while the window shows results and scrolls: the list
  says that the connection was lost, then searches again by itself.
- After a restart of Windows, while C:\ indexes, the status bar's service
  group says so, and the results complete by themselves.
- With the Settings dialog open, stop the service: the dialog stays usable,
  its Index page shows the failure, and the main window shows it behind the
  dialog.
- Settings > Drives and service > Rebuild shows the administrator shield and
  its tooltip. After Windows' permission prompt, each drive reads Indexing,
  then Live again, and `gozler --status` shows each rebuilt; saying no to the
  prompt changes nothing. Clicking it again while the drives index rebuilds
  them once more, not twice.
- Settings > Drives and service > Dates created, then OK, shows the
  administrator shield on OK. After Windows' permission prompt, each drive
  reads Indexing, then Live; View > Columns > Date created then shows the
  dates File Explorer shows, and `gozler dc:today -dc` finds the files
  created today. Turned off again, the service's memory falls by about 8
  bytes an entry, and `gozler dc:today` exits 4 with a message that names
  Dates created.

## The selection and its actions

- Open, Open file location and Properties act on the selected entries. Open on
  a folder and on a file uses the open commands, or File Explorer's way when
  they are empty, and `Enter` on a found file opens it.
- Copy followed by a paste in File Explorer copies the selected files; Copy as
  path and Copy name followed by a paste in a text editor give the quoted paths
  and the names.
- Copy 100,000 selected rows: Task Manager shows the window's memory rise by
  about 22 MB at most, the window stays responsive, and afterwards shows
  whether the clipboard's memory still counts in the window's private bytes.
- Drag a marquee over rows in the list: it selects them.
- Drop files from File Explorer and an archiver onto a folder result, including
  a right-button drop. Leave and re-enter while preview is pending; old feedback
  must not return. Own-list drops use precisely the entries dragged.
- Cancel a receiving drop while its source is slow, then close during another
  intake. The UI remains responsive, cancellation cannot restart the transfer,
  and synchronous sources keep their temporary files until file use ends.
- In a large scattered selection, request a command or context menu in the
  left panel, then activate the right panel while capture finishes. The command
  still acts on the left. Select all and Clear selection during row remapping
  take effect after its native update, rather than being overwritten.
- In a large folder or search panel, type quick-search text rapidly, including
  a character that matches nothing, then use Up, Down and Backspace. Escape or
  a newer mouse selection must prevent an older answer from moving the focus
  or reopening quick search.
- The labels the window shares with File Explorer, such as "Open file
  location" and "Copy as path", match File Explorer's wording.

## Settings

- Each open command runs for its own case, and emptying a box brings back File
  Explorer's way.
- Total Commander fills
  `"C:\Program Files\totalcmd\TOTALCMD64.EXE" /O /A /P=L /L="%1"` into Open
  folders and Open files. Then Open on a folder and on a file opens it in
  Total Commander, and Open file location, left empty, opens File Explorer
  with the entry selected.
- After a restart of Windows with "Start Gozler when I sign in" on: with "Keep
  Gozler running in the notification area" off, the window starts minimized;
  with it on, Gozler starts in the notification area with no window, and the
  shortcut or the tray icon shows it. Turned off, the `gozler` value under the
  current user's `Run` key is gone. Turned off in Task Manager's Startup apps,
  the check box shows off; turned on again, Task Manager shows the entry on.
- After an earlier version added "Search with Gozler", File Explorer's menu
  on a folder no longer shows it once Gozler has started.
- With "Check for updates" on and an older version installed, the status bar
  names the newer release and its link opens the release page; off, Gozler
  makes no request (Resource Monitor's Network page shows none).

## The shortcut and the tray

- With `Win+S` set, Windows Search does not open, Start stays closed and
  Gozler comes forward, with no restart or sign-out. After choosing another
  shortcut, and after Exit, `Win+S` opens Windows Search again at once. With a
  program that runs as administrator in front, `Win+S` opens Windows Search. If
  Windows needs a restart or a sign-out for the shortcut, the Keyboard page
  needs a message that says so.
- The tray icon: a click and `Enter` show the window; its menu holds Show
  window and Exit; it comes back after Explorer restarts; the first hide shows
  the notification once, and later hides show none.
- With Explorer unresponsive, show or hide the window, change the tray setting
  and Exit. The UI stays usable; tray cleanup does not delay settings writes.

## Background work and Exit

- Close during a search, an export, a folder read on an unresponsive share and
  a copy. The UI closes immediately; at next start its place and the interrupted
  operation are restored. Export cleanup may finish in the invisible process.
- Close while an administrator prompt is open for Include, Rebuild or a service
  fix. The UI closes immediately. Accepted changes finish if permission is given;
  settings persistence proceeds independently of that prompt.
- While Paste waits for another program, cancel it and navigate elsewhere.
  Late clipboard data cannot start the cancelled transfer or change its target.
- Open Indexed folders on a sleeping volume, close and reopen it, then expand
  folders on another volume. The dialog stays usable, the independent volume
  lists, and the earlier dialog's results never enter the new tree.
- With Viewer open on a removable drive, attempt removal and then cancel it.
  Viewer can reopen after failed removal; close during a pending read remains
  immediate.

## DPI and contrast

- The window moves between monitors with different DPI, and the layout, the
  fonts and the icons scale with nothing else changing. The Settings dialog and
  its pages scale too.
- At 150 % DPI, and in high contrast, every screen passes the screenshot
  standard: no text is clipped or overlaps, margins and gaps follow
  `core::layout`, controls in a column share their left edge, two states never
  look the same, and the list's rows, header and selection look like File
  Explorer's.

## The copy, move and delete engine

- Open a waiting transfer's context menu, including by right-clicking a row
  that was not selected. Move up and Move down update while the native menu
  stays open; choosing one moves that row only when Queue still allows it.
- With a large restored transfer queue, resize or change DPI during population,
  scroll after it finishes, and reorder a waiting operation. Rows retain their
  operation identity; only the queue list waits during a structural replacement,
  and card Cancel remains usable. A large nontransfer failure report opens
  without populating every native row; its visible paths and tooltips agree.
- Select a different queue row and immediately use Delete, Enter or the reorder
  keys while its card is still updating. List commands target the selected row;
  Cancel on the displayed card continues to target that card's operation.

These need a second disk, a share, a USB drive, OneDrive or an elevated run.
`crates/gozler-fileops/examples/measure.rs` runs the measurements, elevated,
and its rows go into `docs/BENCHMARKS.md`. The decisions named here are those
of [the engine spec](fast-copy-delete-spec.md).

- The measurement's copy cases between two disks: on a hard disk with two
  partitions, where the ring's hard-disk profile should help most, and to a
  USB drive; and a small-file copy to a share with 1, 2 and 16 workers,
  which also times the listing of each destination folder there. Their
  winners replace the Automatic preset's hard-disk, USB and share values
  (D7).
- Showing the current file costs one allocation and one lock for each file
  copied or deleted. The measurement's small-file copy and a delete of the
  small tree, compared with the "Stage E" rows in `docs/BENCHMARKS.md`,
  show whether that cost is visible.
- A move across volumes, for example to a USB drive: the sources go, the
  progress shows bytes, and a cancel leaves each file in one place.
- A copy from a share whose server is switched off during the copy: the
  operation shows Cancelling until the share times out, and Exit ends Gozler
  within 3 seconds, leaving the copy Interrupted.
- A delete of a tree with read-only files on a FAT32 USB drive, which takes
  D1's classic fallback.
- A copy of a folder symbolic link and of a file symbolic link by an
  elevated Gozler, which holds the right to create them, and by Gozler
  without elevation with Developer Mode off: each arrives as the same link,
  or is listed with `ERROR_PRIVILEGE_NOT_HELD` (the spec's "Not verified").
- One share folder reached through two drive mappings: a copy from one into
  the other is refused as a copy into itself (D12).
- A move to another drive of a OneDrive folder whose files are only in the
  cloud: every file arrives with its data, and the source folder goes only
  after its files (the spec's "What an operation receives", the second
  handle).
