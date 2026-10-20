# The window: after install

The owner's checklist for the window, from the stages of the window's plan.
Each item needs the installed Gozler, the owner's own settings, registry or
taskbar, or a second monitor, so no agent runs it. Delete this file when the
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

## DPI and contrast

- The window moves between monitors with different DPI, and the layout, the
  fonts and the icons scale with nothing else changing. The Settings dialog and
  its pages scale too.
- At 150 % DPI, and in high contrast, every screen passes the screenshot
  standard: no text is clipped or overlaps, margins and gaps follow
  `core::layout`, controls in a column share their left edge, two states never
  look the same, and the list's rows, header and selection look like File
  Explorer's.
