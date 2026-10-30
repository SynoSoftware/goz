# Commander: what to add next

## Scope

This roadmap improves the existing Rust and native Win32 `gozler-ui`.
SynapseCommander is a separate project to be written from the ground up
in WinUI 3; it is not a rename or framework migration of this window.
The user research can inform both products, but the implementation owners,
native control choices and maintenance stages here belong to gozler.

## Schedule

**Scheduled:** the command bar is the next feature. The remaining entries
are a proposed backlog in priority order, not work already started. Do the
small command-metadata preparation in
[maintainability stage 5](maintainability.md#stage-5-one-row-for-each-command)
as its own change before adding the commands. The other maintenance stages
run when their affected owner needs them; the whole plan is not a gate.

This roadmap prioritizes everyday file-management tasks, with Total
Commander as one reference rather than a command checklist. It promises
neither that program's complete command set nor compatible numeric IDs.
The command reference is its
[list of internal commands](https://www.ghisler.ch/wiki/index.php?title=List_of_internal_commands),
consulted on 2026-10-30. Names in the reference column identify the source
ideas; product names and keyboard choices follow this app's vocabulary.
Windows' familiar baseline also matters: Microsoft documents
[copy/cut/paste and Rename shortcuts](https://support.microsoft.com/en-gb/accessibility/windows/keyboard-shortcuts-in-windows),
[pinned folders and tabs](https://www.microsoft.com/en-us/windows/tips/file-explorer),
and [ZIP creation and extraction](https://support.microsoft.com/en-us/windows/experience/storage-filemanagement/zip-and-unzip-files).

The command bar request changes the inherited decision in `DESIGN.md`'s
"Not planned" section. Other entries below remain proposals where they
extend that list. On delivery, update the corresponding design and help;
until then, the existing behavior sections describe the shipped app.
Shares, FTP and archives as folders remain outside this roadmap. A typed
command does not itself add browsing support for another kind of location.

## Existing behavior to build on

- Commander already browses folders, opens drive and recent-folder menus,
  searches names through the service, and has quick search that focuses a
  matching row. It does not yet have a command bar, per-panel Back/Forward
  history, favorites or tabs.
- Copy, Move, rename through Move, Delete, New folder, Synchronize, View,
  selection masks and ordinary list selection already have owners. Extend
  them rather than adding another file-operation engine or selection model.
- Folder changes already update live; header clicks already sort; panels
  already reopen their saved folders. File operations already have progress,
  pause/resume, cancellation, reports and recovery after interruption.
  These are existing capabilities, not new backlog entries.
- `F4 Edit` is present but disabled. It is a planned placeholder, not a
  working editor command.
- Search already has clipboard commands. Commander currently leaves
  `Ctrl+C`, `Ctrl+X` and `Ctrl+V` to text boxes; adding them for panel entries
  is an extension of the existing commands, not a second clipboard path.
- Commander can drag entries out but accepts no incoming drop. Its context
  menu has Open, View, Copy, Move, Delete and Properties, without a distinct
  Rename or Open with command. It has no built-in ZIP creation/extraction.

## Everyday-user assessment

Assume a regular Windows user organizing Downloads, moving documents into
project folders, opening files in another app and returning to recent work.
Some use keys, some use the mouse; neither should need to learn that Move
also means Rename. This is a review of documented behavior and source,
not an observed user study or a live visual/accessibility test.

The first roadmap overemphasized specialist commands. The strongest gaps
are completing familiar operations and reducing repeated navigation:

- **"Let me move this the usual way."** Panel clipboard commands, a named
  Rename action and incoming drag-and-drop remove daily detours. Reuse the
  working engines behind them.
- **"Let me get back to where I was."** Back/Forward, visible history,
  favorites and tabs serve repeated work better than another search command.
- **"Let me find the file in this folder."** A local quick filter complements
  quick search and indexed recursive search; its scope must be clear.
- **"Let me use the file."** Open with, Edit, New file, ZIP extraction and
  ZIP creation complete common handoffs to editors and other people.

The command bar remains first because the owner specifically requested it.
For a broad Windows audience without that requirement, clipboard, Rename
and navigation would lead. Tabs move ahead of bulk rename, checksums and
branch view. All orders after the command bar remain recommendations.

### Recovery is a product decision

Every delete is permanent by an explicit owner ruling in
[DESIGN.md](../DESIGN.md#owner-decisions). The existing confirmation already
names the entry or count and says it will not go to the Recycle Bin.
Adding another warning is not the missing feature.

For a broader audience, reconsider offering Delete to the Recycle Bin
where Windows supports it, with permanent deletion as a separate explicit
action. This conflicts with the settled speed-first ruling and is recorded
as a recommendation, not scheduled implementation or permission to change
the engine. Keep the current behavior until the owner revises that ruling.
An unavailable Recycle Bin must never silently turn a recycle request into
permanent deletion.

Interrupted-operation recovery already exists; it does not reverse a
completed delete, overwrite or move. Do not label it Undo. A general Undo
system would need its own integrity and lifetime design and is not a small
extension of this roadmap.

## Priorities

| Order | User task | First useful scope | Reference |
|---|---|---|---|
| 1 | Run a command here | Command bar, terminal launch, recall, completion and focused-name/path insertion | Owner request; TC command-line commands |
| 2 | Copy or move the familiar way | Panel Cut/Copy/Paste, Copy name/path, and Paste on the empty-area menu | Windows clipboard; `cm_CopyToClipboard`, `cm_CopyNamesToClip` |
| 3 | Rename this file | A visible Rename command and F2 in a panel, selecting the name without its extension; use the existing rename operation | Windows F2; `cm_RenameSingleFile` |
| 4 | Return to a folder | Per-panel Back/Forward, history menu and saved favorites; visible navigation commands as well as keys | `cm_DirectoryHotlist`, `cm_DirectoryHistory`, `cm_GotoPreviousDir` |
| 5 | Narrow this folder | Quick filter with visible scope, counts and Esc to restore the listing | `cm_QuickFilter` |
| 6 | Drop files here | Receive files from Explorer and the other panel, on a folder or the shown folder's empty area | TC drag-and-drop; existing Search drop owner |
| 7 | Keep several places open | Native folder tabs on either side, with each tab's navigation/view state | Windows tabs; TC folder tabs |
| 8 | Open or create a document | Open with, Show in File Explorer, working F4 Edit and New text file | Windows app chooser; `cm_Edit`, `cm_EditNewFile` |
| 9 | Unpack a download or send a bundle | Extract ZIP to a chosen folder and create a ZIP from selected entries | Windows ZIP workflow; TC pack/unpack commands |
| 10 | Arrange the two working folders | Swap panels and show the active folder in the other panel | `cm_Exchange`, `cm_MatchSrc` |
| 11 | Select a related group | Invert selection and select/unselect by the focused file's extension | `cm_ExchangeSelection`, `cm_SelectCurrentExtension` |
| 12 | Rename many files | Preview changes and collisions, then submit through the file-operation engine | `cm_MultiRenameFiles` |
| 13 | Compare two versions | Launch a chosen external comparison tool on two files | `cm_CompareFilesByContent` |
| 14 | Check a download or copied bundle | Create/verify checksum files and report mismatches or unreadable files | `cm_CRCcreate`, `cm_CRCcheck` |
| 15 | Find descendants in one list | Indexed branch view with truthful completeness | `cm_DirBranch` |

The first complete everyday-use milestone is clipboard, Rename,
navigation, filtering and incoming drops, alongside the requested command
bar. Tabs, document actions and ZIP workflows follow. The specialist
entries remain useful, but do not displace these basics.

## First delivery: the command bar

The command bar sits below the panels and above the key bar, as in the
owner's reference image. It shows the active folder beside an editable
command and history drop-down. Compose native static and editable combo-box
controls; keep native text editing, focus and accessibility behavior.

- Enter submits in the active panel's folder captured at that moment.
  Commands run through Windows' command prompt in a visible console so
  output remains available. The window never waits for the launched
  program to finish. Open terminal uses the same launch owner without
  a command; a launched console has its own lifetime after launch.
- A standalone `cd` or `chdir` navigates the active panel through its
  existing navigation owner. Resolve relative paths against that panel's
  folder. Quoted paths and `cd /d` are covered explicitly. Compound shell
  commands go to the shell unchanged; do not build another shell grammar.
- Give the bar a focus command, previous/next command recall, a history
  drop-down and a clear action. Start with bounded session history and
  preserve the current draft while browsing it. Whitespace-only input
  does nothing. A launch failure keeps the text for correction.
- Insert the focused file's name or full path at the caret, quoting it as
  required by the command prompt. Propose `Ctrl+Enter`, `Ctrl+Shift+Enter`
  and `Alt+F8` for insertion and history after checking existing key
  reservations. Menus and Keyboard use the same command definitions.
- Provide file/folder completion for the current token without blocking
  the window. Keep path and command text separate from search queries;
  stale completion cannot overwrite a newer draft or a changed folder.
- Typing in a panel retains quick search. Typing in the command bar edits
  commands. Focus routing decides which receives Enter, Esc and Tab;
  global shortcuts must not steal the edit control's keys. Esc returns to
  the panel without losing the command draft. In a search panel, use its
  underlying folder, not a selected result's parent.

Rules and input state belong in `gozler-ui::core`; `app` composes the bar
and routes commands, and `win` owns native controls and process launch.
Use the existing work owner for process launch, path resolution and
completion that can wait. This does not require a new scheduler, an
embedded terminal, a new crate or a service protocol change. Run with the
person's normal permissions.

**Done when:** a quoted command launched in a folder containing spaces
receives that working directory, its output remains readable, `cd`
changes the active panel, history and insertion work from the keyboard,
and a slow launch or completion leaves both panels usable. A delayed
completion cannot replace newer input, and changing focus cannot execute
a command intended for another control. Pure parsing/routing rules get
focused tests for those failures; use one bounded native check where
source review cannot establish launch or control behavior. Save screenshots
only of the new bar and its history state for later owner review.

## Boundaries for the following features

- **Navigation:** keep history with the panel, and later with its tab;
  favorite folders are shared saved choices. New navigation after Back
  replaces the forward branch. Going Back restores the prior location,
  focus and view where they still exist. Commander currently uses Alt+Left
  to show Search; introducing Back must update that reservation and keep
  Show Search available through its existing command.
- **Clipboard and Rename:** keep text editing separate from entry actions.
  Capture entry identities once, use the existing clipboard and operation
  owners, and expose the same commands in menus and keys. A one-entry Rename
  keeps its parent fixed, preserves focus on success and reports a collision
  without replacing another file. F6 retains its existing Move behavior.
- **Incoming drops:** extend Search's existing OLE intake and operation
  routing. The pointer chooses the destination panel; a folder row targets
  that folder and empty space targets the shown folder. Do not use whichever
  panel happened to be active before the drag. Keep source lifetime,
  copy/move feedback, cancellation and late hover results under their
  existing owners. A search panel's empty area is not an implicit destination.
- **Quick filter:** narrow the current folder's entries, rather than start
  a recursive service search. Keep quick search's focus-only behavior
  distinct. Preserve selection by entry identity across filter changes;
  commands target only visible selected entries, or the visible focused
  row when none are selected there. Counts distinguish visible selection
  from retained hidden marks, so Delete cannot include unseen entries.
  Large passes use the work owner and superseded results cannot change
  a newer view.
- **Document actions:** reuse launch and file-operation owners. Open with
  uses Windows' app chooser, while F4 uses the chosen editor. Show in File
  Explorer is an escape route for shell-specific tasks; it does not require
  hosting every shell extension in this process. New file creation must not
  overwrite an existing name. Keep each command's input and target explicit.
- **ZIP:** begin with create/extract commands, not archives as navigable
  panels or a plugin system. Select a supported platform or installed-tool
  integration during feature design. Extraction validates destinations,
  resolves collisions and never writes outside the chosen destination;
  malformed archives return errors. The operation reports progress and
  partial failure without holding the window. This new proposal does not
  change the existing decision about browsing archives as folders.
- **Panel utilities and selection:** reuse command dispatch, clipboard,
  selection and panel owners. Swapping moves each panel's complete view,
  not just its path text. Copying a name or path into a text box remains
  text editing; file commands act on the captured entry selection.
- **Bulk rename:** preview is not permission to overwrite. The engine owns
  validation, execution, partial failure and recovery, including a rename
  that crosses another selected name. Do not implement rename integrity
  in the dialog.
- **Tabs:** use the native tab control and existing panel state. A tab owns
  its view and pending-result identity; closing it invalidates late replies.
  Preserve the existing operation and clipboard lifetimes independently
  of tabs.
- **Comparison and checksums:** comparison launch reuses the process owner.
  Hashing uses background file reads with cancellation and reports partial
  failure. Metadata equality does not establish equal file contents;
  reuse synchronization's comparison rules where applicable.
- **Branch view:** reuse the indexed search/listing path and entry identity.
  Missing index coverage is shown as incomplete or unavailable. Do not
  introduce a synchronous directory walk to fill a gap.

## Usability follow-through

Check these within the area being changed rather than starting a separate
redesign. Commands need a discoverable menu entry as well as a shortcut;
disabled entries explain the prerequisite. Distinguish typing a path,
filtering a folder and searching recursively in the labels and focus route.
Retain selection, focus and scroll after refresh or returning to a folder.
Existing progress, pause/resume, cancellation and failure reports stay
reachable while another operation or dialog is open.

Long names are a practical reason to consider a native panel splitter and
remembered column widths later. Those are comfort proposals, not required
preparation for the command bar. Previewing images/PDFs and searching inside
the text/hex Viewer also have everyday value, but come after the basic
workflows; start with external viewers where they serve the task. Retain
keyboard access, accessible native control names, high contrast and DPI
behavior in every delivery. This pass did not test those qualities live.

Before implementing a later entry, turn its first scope into a bounded
change with its owners, current key reservations and caller-visible proof.
Keep the coding and naming rules in `AGENTS.md` authoritative. Complete
structural preparation separately, review the final change, then follow
the repository's `build.ps1` rules. Owner-only environment checks go in
After install and do not hold a stage open. When an entry ships, move its
rules into `DESIGN.md` and the help and remove it from this pending list.
