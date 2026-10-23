# gozler

gozler finds files and folders on NTFS and ReFS volumes by name, path, size and
date, from an index of each volume that a daemon keeps current. Its window
also copies, moves and deletes them.

**Gozler**:
**Owner ruling.** The product's name wherever a person sees it outside the
command line: the window, the setup, Windows' lists such as Services, and the
folders Gozler creates, such as `C:\Program Files\Gozler`. The command line
writes `gozler`: the command, its help and its messages, and every name a
person types there, such as `gozler.exe`, `gozlerd install` and
`net start gozler`. A person types the command line in lower case, so its text
matches what they type; elsewhere Gozler follows Windows, where a product name
is a proper noun.

## Index

**Indexed volume**:
A fixed NTFS or ReFS volume that is attached and that the Include settings do
not leave out. gozler indexes it from when it appears until it is removed;
removable drives, network drives and other filesystems are not indexed.

**Left out**:
A drive, folder or name that the Include settings keep out of the index. The
index holds no entry for it, so no search finds it.
_Avoid_: excluded, ignored

**Entry**:
One indexed name: a folder, or one name of a file.
_Avoid_: record, node

**Hard link**:
One of a file's names. Each hard link is its own entry; all of them share the
file's file ID, size, date and attributes.
_Avoid_: alias

**File ID**:
The number that identifies a file on its volume, shared by every hard link of
the file: 64 bits on NTFS, 128 bits on ReFS.
_Avoid_: FRN (outside code), inode

**Mount prefix**:
Where a volume is mounted, as gozler prints it: a drive such as `C:\`, or a
mount folder. A volume can have several; its full paths print with the first,
or with its GUID path when it has none.
_Avoid_: drive (for a volume mounted in a folder)

**Full path**:
The mount prefix followed by every folder and the entry's name, as gozler prints
it: `C:\repo\docs\testing.md`.
_Avoid_: path within the volume, relative path (when the drive is included)

**Folded name**:
An entry's name with case folded away; what a case-insensitive search and the
result order compare.

**Attributes**:
The Windows attributes of a file or folder that gozler indexes, each written
as es writes it, with one letter: `H` hidden, `S` system, `D` folder. `N`
(normal) means that the file has no other attribute.
_Avoid_: flags, attribute bits (in prose)

**Unknown size or date**:
A file's size or date that gozler did not read, because the size and date pass
failed or stopped, or could not read that file. A folder's size is unknown
until its volume's size and date pass completes, and stays unknown when the
pass failed or stopped. For a filtered user, every folder's size is unknown.

**Folder size**:
The sum of the known sizes of every file below a folder, each hard link
counted; what `-get-folder-size` gives for that folder.

## Visibility

**Visible entry**:
An entry a user may see: one in a folder the user may list, or any entry for
a user who sees every file. A search shows only visible entries, and its
counts and totals add only them.
_Avoid_: allowed entry, permitted file

**Users and groups who see every file**:
The one list, kept for the whole PC, of the accounts and groups whose
members see every entry, whatever the folder permissions say. Adding
Everyone turns the filter off for every user. Only an administrator changes
it.
_Avoid_: whitelist, exception list, admin list

**Filtered user**:
A user who does not see every file: they see only visible entries, and no
folder sizes.
_Avoid_: restricted user, limited user

## Command line

**Option**:
A command-line argument that starts with `-`, `/` or `--` and changes how
gozler searches or prints, such as `-n 5`, `/ad` or `--json`. Some options
take a value, the next argument. Every other argument is a search term.
_Avoid_: switch, flag, parameter (for the value)

## Search

**Name term**:
A search term without a separator; it matches the entry's name.

**Path term**:
A search term with a `\` or `/`, or a `path:` value; it matches the full path,
as a substring.

**Whole word**:
A match of a term's text with no letter or digit right before or after it, so
`_` and punctuation end a word; what `ww:` and `-ww` ask for.

**Word start**:
A match of a term's text where a word of the name starts: at the start of
the name, or after a character that is not a letter or digit. `wordstart:`
asks for it, and it is the window's default, so `para 50` finds
`Paracetamol 500mg.pdf`.

**Name start**:
A match of a term's text at the start of the name; what `startwith:` asks
for.

**Match mode**:
How the window matches the name terms the person types: Start of words (word
start), Start of name (name start), Anywhere (a substring, as the command
line does), Whole words, or Regular expression, where the whole text is one
pattern.

**Regular expression**:
A `regex:` value or `-r` pattern, in Rust's `regex` syntax. It matches the
name, or the full path with `-p` or when it holds `\\`; gozler never rewrites
its `/`.

**Scope**:
The folder `-path` names. A search finds the entries below it, on the volume
whose mount prefix is the longest that starts it.

**Relative path**:
A path that starts at gozler's working directory instead of a root: a path
term that starts with `.\` or `..\`, or a folder for `-path` or `parent:`
without a drive or share.

**Path wildcard**:
A path term with `*` or `?`; it must match the whole full path, as `*\docs\*`
does.
_Avoid_: wildcard path term

**Query start**:
The instant a search starts, with the time zone of the program that parses
it. A `dm:` date counts from it: `today` is the calendar day that holds it, in
that time zone, and `7days` is the 7 times 24 hours that end at it.
_Avoid_: now (in prose)

**Result order**:
The order gozler prints matches in: by the sort key, then folded name, volume,
file ID, parent file ID and raw name, so the same query over the same index
prints the same list, and a limited list is the start of the full one.

## App

The app uses TinyTorrent2's words where both mean the same thing, so the
owner's programs read alike.

**App**:
`gozler-ui`, the desktop app that searches through the service and copies,
moves and deletes files: its window,
its dialogs and its notification area icon. The status bar's memory figure
calls it "UI", as TinyTorrent2 does (owner ruling).
_Avoid_: GUI

**Window**:
The app's main window, which shows one of two modes: Search or Two panels.
_Avoid_: UI, app (for the window)

**Service**:
`gozlerd` as the app names it, "the Gozler service", and the command line,
"the gozler service".
_Avoid_: daemon (in text the person reads)

**Settings**:
The saved choices for how the window behaves, changed in the Settings dialog.
_Avoid_: options, preferences; an option is a command-line argument

**Dialog**:
A surface over the window for one task, such as Settings or Custom date.
_Avoid_: form, window (for a dialog)

**Pane**:
A side area of Search: the filter pane at the left and the inspector below
the results list. Not a panel: a panel is one of the two lists of Two panels.
_Avoid_: sidebar

**Results list**:
The list of matches, one row per entry.
_Avoid_: table, grid

**Filter**:
A choice in the filter pane that narrows the results, such as Kind or Size.
Each filter adds one clause to the typed search.

**Include**:
The Settings page whose folder tree and file patterns decide what the
service indexes, for every user. Unlike a filter, it changes the index, not
one search.
_Avoid_: index filter, exclude list

**Inspector**:
The pane that shows the details of the selection.

**Selection**:
The entries the person selected in the results list, fixed at the moment the
window reads them.

**Command**:
An action the person chooses from a menu, a key or a button. Every place that
offers a command runs the same one.

**Shortcut**:
A key combination that runs a command, such as `Ctrl+F` for Edit > Find.
The person changes them on Settings > Keyboard, where one more shortcut
shows gozler from any program, such as `Win+S`.
_Avoid_: hotkey (in text the person reads)

**Tray**:
gozler's icon and menu in the Windows notification area, shown while
gozler keeps running there (Settings > General).

**Close**:
Closing the window: it hides to the tray while gozler keeps running there,
and exits otherwise.

**Exit**:
Ending gozler, from File > Exit or the tray menu. The service keeps running.
_Avoid_: quit

**Search**:
The mode of the window with the search box, the results list and the panes.
The other mode is Two panels.

**Two panels**:
The mode of the window with two panels, the key bar and Norton Commander's
keys, which View > Two panels switches on and off.

**Panel**:
One of the two lists of Two panels, with its path box and its status line. It
shows a folder, or the results of a search (a search panel).

**Active panel**:
The panel with the focus; commands act on it.

**Other panel**:
The panel that is not active; its folder is the destination of Copy to
folder… and Move to folder….

**Path box**:
The edit box above a panel's list. It shows the full path of the panel's
folder, and the person types a path there and presses `Enter` to go to it.

**Drive button**:
The button left of a panel's path box. It shows the drive, such as `C:`, and
opens the drive menu.

**Status line**:
The line under a panel's list: its counts at the left, and the free space of
its drive at the right.

**Status slot**:
One of the status bar's three positions: Left, Center or Right. Each holds
the status entries aligned at that position.
_Avoid_: socket, group (for the position)

**Status entry**:
One piece of status information, such as a transfer, an export or a memory
figure, within a status slot.

**Search panel**:
A panel that shows the results of a search in place of a folder. `Alt+F7` or
`Ctrl+F` turns the active panel into one, and `Esc` or `Backspace` shows its
folder again.

**Focused row**:
The row with the focus outline, which the arrow keys move together with the
selection.

**Selected entries**:
The entries the person selected in a panel with the arrow keys, `Insert`,
`Space`, `Shift` or `Ctrl`; commands act on them, or on the focused row when there are none.

**Quick search**:
Going to a name in a panel by typing its start, with or without `Alt`.

**Drive menu**:
The menu of drives and recent folders that `Alt+F1` and `Alt+F2` open.

**Key bar**:
The row of `F3` to `F8` buttons under the panels.

**Viewer**:
The window that `F3` opens, which shows a file as text or as hex.

**Text and Hex**:
The two ways the Viewer shows a file: Text decodes it into lines, and Hex
shows each byte. The Viewer chooses from the file's first bytes, and `F4`
switches.

**File operation**:
A person's command to copy, move, synchronize, delete, rename or create a
folder. An operation includes its whole selection; every delete is permanent.

**Transfer**:
The work of a copy, move or synchronization between one source volume and
one destination volume. An operation can contain several transfers.

**Copy policy**:
The rule chosen before a copy or move for files whose names already exist at
the destination: update changed files, update only newer files, keep both or
skip existing files. A metadata match does not verify equal contents.

**Synchronization**:
A one-way update of a destination folder tree from an authoritative source
folder tree, after comparing and reviewing their differences.

**Comparison**:
The differences found between synchronization's source and destination,
including new, changed, unchanged and destination-only entries.

**Mirror**:
Synchronization that also permanently deletes the reviewed destination-only
entries after the required additions and updates succeed.

**Device admission**:
Permission for a transfer to use all its physical storage devices together,
within each device's transfer limit.

**Queue**:
The accepted transfers waiting for device admission, in the person's chosen
priority order.

**Paused transfer**:
A transfer the person stopped until they choose Resume.

**Yielded transfer**:
A transfer temporarily stopped for higher-priority work, which returns
automatically when that work releases its devices.

**Taken entry**:
An entry that a delete or move takes. It leaves the folder panels, and shows
struck through in the search lists, at once, before the disk work ends. After
the operation ends, a list keeps treating the entry as taken until it replaces
its older view of the disk.

**Kept operation**:
An ended delete or move whose entries a list still treats as taken while it
shows its older view of the disk.

**Transfer pane**:
The pane that shows copies, moves, synchronizations, their queue and reports
in the window.
_Avoid_: File operations window

**Color token**:
What a piece of text means, which decides its color wherever gozler shows it:
Strong, Muted, Primary for what the person types, Success, Warning and Danger.
_Avoid_: a color name, such as green, for a meaning
