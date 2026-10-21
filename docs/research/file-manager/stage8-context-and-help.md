# Stage 8: proposed `CONTEXT.md` and help changes

For the later pass that closes `docs/plans/file-manager-spec.md`, after the
search-in-a-panel work (stage 7) is built. The first parts were written
against the committed code of stages 3 to 6 and `docs/DESIGN.md` as it now
stands ("Two panels" chapter). Part 1 changes `CONTEXT.md`, part 2 the help
and window texts, part 3 maps the spec's section names, which about 125 code
comments cite as `file-manager-spec.md: <section>`, to `docs/DESIGN.md`.

## 1. `CONTEXT.md`

`CONTEXT.md` already holds the vocabulary that the spec's "Vocabulary" lists
(Two panels, Panel, Active panel, Other panel, Focused row, Selected entries,
Quick search, Drive menu, Key bar, Viewer, File operation, Queue) and the
"Pane" entry that no longer lists "panel" under _Avoid_. These changes remain.

### 1.1 Replace the Window entry

Replace

```
**Window**:
`gozler-ui`, the desktop window that searches through the service.
```

with

```
**Window**:
`gozler-ui`, the desktop window that searches through the service and, in
Two panels, browses folders.
```

### 1.2 Add after the Two panels entry

```
**Mode**:
Search or Two panels, the two ways the window is laid out. Each keeps its
state while the other shows, and the window opens in the mode it closed in.
```

### 1.3 Add after the Panel entry

```
**Path box**:
The edit box above a panel's list. It holds the full path of the panel's
folder, and the person types a path there and presses `Enter` to go to it.

**Drive button**:
The button left of a panel's path box. It shows the drive letter and opens the
drive menu.

**Status line**:
The line under a panel's list: its counts at the left, and the free space of
its drive at the right.
```

### 1.4 Extend the Folder size entry

Replace

```
**Folder size**:
The sum of the known sizes of every file below a folder, each hard link
counted; what `-get-folder-size` gives for that folder.
```

with

```
**Folder size**:
The sum of the known sizes of every file below a folder, each hard link
counted; what `-get-folder-size` gives for that folder. A panel shows it from
the service, and shows "Folder" while the service does not know it.
```

### 1.5 Add after the Queue entry

```
**Taken entry**:
An entry that a waiting or running delete or move takes. The lists hide it, or
show its row blank, at once, before the disk work ends. A list keeps an ended
operation's taken entries hidden until it has read the disk again.

**File operations window**:
The window that shows the running operation and the queue. It opens when an
operation still runs 2 seconds after it starts.
```

### 1.6 Add after the Viewer entry

```
**Text and Hex**:
The two ways the Viewer shows a file: Text decodes it into lines, and Hex shows
each byte. The Viewer chooses from the file's first bytes, and `F4` switches.
```

### 1.7 Add when search in a panel is built (wording to check against stage 7)

The Panel entry already says "or the results of a search (a search panel)".
Add

```
**Search panel**:
A panel that shows the results of a search in place of a folder. `Alt+F7`
turns the active panel into one, and `Esc` or `Backspace` shows its folder
again.
```

and keep the Mode entry's sentence "Each keeps its state while the other
shows" only if stage 7 keeps it true. `DESIGN.md` uses the spec's words for
this part: "search panel", "Look in", "one state for each list".

## 2. Help and window texts

- **`gozler --help`, the quick help and the overview: no change.** They
  describe the command line, and Two panels is a feature of the window only.
  `gozler_core::escompat::help` holds no text about the window's modes.
- **Help > Search syntax** (`C22`, `gozler_core::escompat::search_syntax`) is
  a command of both modes with `F1` as its key. In Two panels it opens the
  same dialog, whose notes speak of "the search box" (for example, "the whole
  search box is one pattern"), although a panel has no search box until
  search in a panel is built. The later pass checks that wording against the
  search panel and changes it in `escompat::help` only if it reads wrong
  then.
- **Settings > Keyboard** should say which keys Two panels keeps. The spec
  asks for it ("The Keyboard page says which keys Two panels fixes, and that
  a shortcut on `Alt` with a character works only in Search"), and the page
  does not say it now. Proposed English text for a note under the command
  list, key `settings_keyboard.two_panels` in `en.json` and each catalogue:

  ```
  Two panels keeps F3 to F9, Alt+F1, Alt+F2, Ctrl+L and Ctrl+R, and Alt with a
  letter or a digit. A shortcut on one of these keys works in Search only.
  ```

  This is a code and catalogue change (the page's layout, `en.json`, `es.json`,
  `nl.json`, `tr.json`), so the later pass decides whether to make it or to
  change `DESIGN.md`'s "The Keyboard page" bullet to say the page is silent.
- **`DESIGN.md` "Commands" and "Menus"** hold the Search tables (`C1` to
  `C26`, the File, Edit and View groups). They do not yet list Copy to
  folder…, Move to folder…, Delete, Cut and Paste, which Search has since the
  file operations work, nor `FileOperations`. The later pass adds those rows,
  with the drop onto a folder row once it is built, and removes "No command
  destroys anything, so there are no confirmations", which `DESIGN.md` no
  longer holds anywhere.

## 3. Where a citation moves

A code comment that cites `file-manager-spec.md: X` cites `docs/DESIGN.md: Y`
afterwards:

| Spec section | `DESIGN.md` section |
| --- | --- |
| The window | Two panels' window |
| A panel | A panel |
| Moving around | Moving around |
| Selecting entries | Selecting entries |
| Quick search | Quick search |
| The drive menu | The drive menu |
| Folder sizes | Folder sizes |
| The key bar | The key bar |
| Commands in Two panels | Commands in Two panels |
| Keys | Keys |
| Copy and Move, step 1 | Copy and Move, item 1 |
| New folder | New folder |
| Delete | Delete |
| Menus in Two panels | Menus in Two panels |
| The status bar in Two panels | The status bar in Two panels |
| The Viewer | The Viewer |
| File operations, step N | File operations, the bullet that holds step N (2 Delete asks, 5 the File operations window, 6 when an operation ends, 7 a failure, 8 Exit) |
| The person never waits for the disk | The person never waits for the disk |
| Interrupted operations | Interrupted operations |
| Memory | Memory budget |
| Search in a panel | Search in a panel (placeholder until stage 7) |
| Search mode, Speed settings, Choices, Two panels | not moved yet |

Citations of `fast-copy-delete-spec.md: Taken entries` stay, because the engine
spec is not part of this plan.
