# Service settings

**The plan itself is still to be written.** This file holds the rules that the
window's design settled for the settings that the service keeps, so the plan
starts from them. The window ships without these settings: Settings shows the
General, Search, Keyboard and Index pages, and the Index page shows the drives,
their states and entry counts, what the index holds, and the service's state
and memory ("Settings dialog" in `docs/DESIGN.md`).

The plan adds three things, because each needs a service request that does not
exist yet:

- the Include page, which decides what the service indexes;
- Dates created, a check box on the Index page, which adds the Date created
  column and the `dc:` search function;
- Rebuild index, a button on the Index page.

It owns how the service stores, checks and applies these settings, how it
counts the preview without changing anything, and how a request proves that
an administrator sent it.

The approved prototype of these screens, with their English words, is in git
history: `git show b43f935:docs/plans/ui-prototype.html`, a page that opens in
a browser. Its Settings dialog shows the Include page, the Indexed folders
dialog and the Index page with both controls.

## The service's settings

- **Index and Include are the service's settings.** One service indexes for
  every user of the PC, so a change there changes what every user finds. The
  service saves them for the whole machine, and only an administrator changes
  them. A change on either page puts the administrator shield on OK and Apply;
  pressing either asks Windows for permission, then the service saves the
  change and applies it without a restart. Anyone can open the pages and read
  them, and each says at its top: "These settings apply to every user of this
  PC. Changing them needs administrator permission."
- **A change acts at once.** An unchecked drive or folder loses its entries and
  the service frees their memory. A drive or folder checked again is read and
  shows "indexing" until it is complete. A name pattern removes the matching
  entries. Changes below an unchecked folder are ignored from then on. A drive
  with nothing checked also leaves Location. The command-line `gozler` sees
  the same index.
- **Dates created is a search feature,** so `gozler` gets `dc:` in the same
  change, because the window adds no search feature that `gozler` lacks.

## The Include page

The page follows Windows' own Indexing Options, which every Windows user can
meet: a list of the indexed folders with what is left out inside each, a
Modify button that opens a folder tree with check boxes, and a count of what is
indexed.

| Group | Setting | Control | Default |
| --- | --- | --- | --- |
| — | the preview line: entries indexed now and after OK | text at the top of the page | — |
| Folders | Folders Gozler indexes: each indexed folder and the folders left out inside it | list view, read-only | every drive, nothing left out |
| | Modify… | button that opens Indexed folders | — |
| | Include new fixed drives automatically | check box | on |
| Names | Names that are not indexed | edit box: name patterns separated by `;`, such as `*.tmp;node_modules;.git` | empty |
| | Exclude hidden files and folders | check box | off |
| | Exclude system files and folders | check box | off |

- **The preview line** says what OK would change before it is pressed:
  "Indexed now: 2,273,581 entries. After OK: about 1,871,204." It follows every
  change on the page. The service counts it from its index without changing
  anything. A folder that is left out today has no entries in the index, so
  checking it again adds "and the folders you checked again, counted once
  they are indexed". The window asks for the count 300 ms after the last
  change, through its worker, and a newer change cancels the older request
  (window rule 4 in `docs/DESIGN.md`).
- **Folders Gozler indexes** is Windows' "Index these locations" list: one row
  for each indexed folder, a checked drive or a checked folder below an
  unchecked one, with the folders left out inside it, such as `C:\` with
  `C:\Windows; C:\ProgramData`. It shows the include and exclude choices in
  one place, so no choice is hidden deep in a tree. It is read-only, as in
  Windows; Modify… changes it.
- **Modify… opens Indexed folders,** Windows' "Indexed Locations" dialog:
  - a tree of each fixed NTFS or ReFS drive and its folders, opened one level
    deep, so the folders to uncheck are in view. A checked folder is indexed
    with everything below it, and an unchecked one is left out. A partial
    check box means that some folders below it differ;
  - the text "Uncheck a folder to leave it out of the index. To index only
    some folders, uncheck the drive and check those folders.";
  - the same summary under the tree, where selecting a row shows that folder
    in the tree;
  - OK, which puts the choices on the page for the Settings dialog's OK or
    Apply, and Cancel.
- **The nearest check box above an entry decides.** To index only
  `C:\Users\ana`, the person unchecks `C:\` and checks `C:\Users\ana`. To index
  all of `C:\` except `C:\Windows`, they uncheck `C:\Windows`. There is no
  include-only switch to understand.
- **Clicking a check box sets the folder and everything below it,** and drops
  the choices made below it, as Windows' folder trees do. The person then
  changes single folders below.
- **The tree lists the folders on the drive, not in the index,** so a folder
  that is left out can be checked again. It is a tree view with
  `TVS_CHECKBOXES` and `TVS_EX_PARTIALCHECKBOXES`; it lists a folder's
  subfolders with `FindFirstFileExW` when the folder is first expanded, and
  derives each check box from the nearest saved choice above it. Whether a
  click can be kept from reaching the partial state is not verified; if it
  cannot, the window handles the click itself and sets the state.
- **The service saves only the folders whose check box differs from the folder
  above,** so a new folder is indexed when the folder above it is checked. A
  folder that is renamed or deleted loses its own choice and follows the
  folder above it.
- **Include new fixed drives automatically** off makes a drive that appears
  later unchecked, so the person decides about it in Indexed folders.
- **A drive that is offline keeps its row** on the Index page and in Indexed
  folders, so a choice for a docking drive is not lost.
- **Names that are not indexed** apply everywhere a folder is indexed, with `*`
  and `?`, matched against each file and folder name. A file whose name
  matches is left out; a folder that matches is left out with everything in
  it, so `node_modules;.git` leaves out those folders on every drive. A
  pattern that matches nothing excludes nothing, so the field shows no error.
  Exclude hidden and Exclude system leave out a matching folder with
  everything in it in the same way.

## The Index page

- **Dates created** is off by default, because it adds memory per entry to the
  service. The page shows the service's memory, so the person sees the cost
  after they turn it on.
- **Rebuild index** makes the service read every included drive again, as
  after a restart. It acts at once, so the button itself has the
  administrator shield and asks Windows for permission. It is for an index
  the person no longer trusts; the service keeps itself current without it.
  While it runs, the drives show "indexing" and searches give incomplete
  results, as status part 4 says.
- The drive list adds the state "not indexed", for a drive with nothing
  checked.

## Everything's options

The Include page takes over these options of Everything, as the owner's
screenshots of Everything show them:

| Everything page | Option | Gozler |
| --- | --- | --- |
| NTFS | Volume list, include in database | Include: the drives at the top of the tree; Index: their state |
| NTFS | Include new fixed volumes | Include new fixed drives automatically |
| NTFS | Include only (per volume) | uncheck the drive, check the folders |
| Exclude | Exclude hidden, exclude system | the Names group |
| Exclude | Exclude list: folders and filters | the folder tree and the name patterns |
| Exclude | Include only folders | the folder tree |
| Exclude | Include only files | no: the Kind filter narrows a search to file types |
| Indexes | Force Rebuild | Rebuild index |
| Indexes | Index date created | Dates created |

## The Date created column

The change that adds the Date created column also gives each column one row,
as `CommandId::row` does for a command. Three of `Column`'s facts in
`core/model.rs` (`shown_by_default`, `can_hide`, `right_aligned`) are
`matches!` or `==`, so a new column silently gets "shown by default, can be
hidden, left-aligned". Date created must choose its default on purpose,
because it costs service memory and its index setting is off by default.
`Column::row(self) -> ColumnRow` is one exhaustive match, and
`ColumnRow { header, menu_label, width: Dip, shown_by_default: bool, can_hide:
bool, right_aligned: bool, sort_key: Option<SortKey>, export_column:
Option<EsColumn> }` replaces the per-column functions. `text::cell` stays
apart, because it formats data. The names that `ui.json` saves do not change,
so saved settings still load. No new test: the compiler forces each row.
