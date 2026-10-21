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
pass failed or stopped.

**Folder size**:
The sum of the known sizes of every file below a folder, each hard link
counted; what `-get-folder-size` gives for that folder.

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
The app's main window: the search box, the panes and the results list.
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
A side area of the window: the filter pane at the left and the inspector
below the results list.
_Avoid_: panel, sidebar

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

**Color token**:
What a piece of text means, which decides its color wherever gozler shows it:
Strong, Muted, Primary for what the person types, Success, Warning and Danger.
_Avoid_: a color name, such as green, for a meaning
