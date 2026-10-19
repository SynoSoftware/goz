# Plan: gozler-ui

The order in which to build the window that [the spec](ui-spec.md)
describes and [the design](ui-design.md) designs, how each stage proves it is
done, the tests, and the provisional API sketches. It changes as the
implementation progresses.

## Schedule

**Scheduled:** started on 2026-10-18, the day the owner approved the
prototype (stage 0). The plan needs `gozlerd` installed and running on the
owner's machine, because the live checks search through it.

## How every stage works

- A stage is a milestone of one or more commits, each of which passes the
  full CI set from `AGENTS.md`. Each new rule in the core has the test that
  "Tests" below names for it, under `AGENTS.md`'s test rules.
- **Traceability.** Each item of a stage's **Done when** and **After
  install** ends with the spec IDs it proves ("Covers: C4, T9"). Every ID in
  the spec appears in at least one item. A stage that builds a behavior whose
  ID no item covers is not done.
- **Live checks** run the release `gozler-ui.exe` through
  [the probe](#the-probe), against the running `gozlerd`, which only reads.
  Exact comparisons search only the probe's fixture folder, where nothing else
  changes, and the expected answer is the fixture's own count or list and the
  answer of the `gozler.exe` of the same build. Stage 1 is the only stage
  that changes the daemon and the protocol, for word start, name start and
  folder sizes; the owner installs its build before stage 2 starts, so from
  then on the installed `gozlerd` serves both.
- **Screenshots.** The probe takes a screenshot of every state that the stage
  makes reachable, in English and in the pseudo-language, at the machine's
  DPI. Each passes [the screenshot standard](#the-screenshot-standard).
- **Memory.** Each stage measures the steady-state
  [memory budget](ui-spec.md#memory-budget) with the probe and reports the
  figure; stages 4 and 6 also measure the two allowances.
- **Text.** Every text that a stage adds is a key in `en.json` with its access
  key, shown through the function that rule 12 of the design names, so stage
  8 switches languages without revisiting earlier stages.
- **The owner's environment is never touched.** A check never ends a process
  the stage did not start, never opens a file or a folder on the owner's
  desktop, never uses the clipboard, never sends a key to a window that is not
  the probe's, and never reads or writes the owner's `ui.json`: the probe
  starts the window with `--data-dir` in its fixture folder. The clipboard can
  hold the owner's files, images or other formats that no script can save and
  put back, so the clipboard's content is proven by core tests of
  `ClipboardData`. What only a person can see, such as Open starting the right
  program or a paste, is in the stage's **After install**, which the owner
  checks.
- **The stage review** below ends every stage.
- Installing is the owner's step. Every stage leaves `gozler` and `gozlerd`
  working as before.

### The screenshot standard

A screenshot fails when any of these is true. A finding names the item it
fails and the control; "looks off" is not a finding.

1. Text is clipped, cut by an ellipsis where the spec does not allow one, or
   overlaps another control.
2. A margin or gap differs from the spacing constants in `core::layout`.
3. Controls in one column do not share their left edge, or controls in one
   row do not share their baseline.
4. Two different states look the same, such as no results and a failure.
5. A label breaks a rule of the spec's "Words": wording, capitalization, a
   missing access key, or a "…" missing on a command that opens a dialog.
6. A dialog breaks the spec's "Dialogs": button order, default button, Esc,
   Apply's state.
7. The list's row height, header and selection differ from File Explorer's
   list at the same DPI.
8. A control's place is not given by `Layout::compute` or a dialog template.

### The stage review

At the end of each stage, an independent reviewer (Opus) reads the stage's
diff, the probe's output and the screenshots against the spec, the design
and the owner's review standard: every option and grouping makes sense to the
user, every label is short, fluent and standard, the buttons are standard,
nothing is placed by accident, every feature of the stage works, each journey
the stage reaches makes sense, and each failure helps the user fix it. The
stage is done when:

1. no correctness defect is open;
2. nothing breaks an approved standard of the spec, the design or this plan;
3. every item of its **Done when** passes;
4. no complexity is left without a requirement that forces it.

A reviewer's idea that goes beyond the approved spec is not a defect. It goes
to the owner as a proposal and does not hold the stage.

## Stage 0: the prototype

A throwaway HTML page that draws the window as Windows 11 draws its common
controls, on synthetic data and with no daemon. It shows every screen of the
spec, so the owner approves the look, the words and the behavior before stage
1 builds anything. The es superset plan compared every stage with a golden
corpus of `es.exe` output; the approved prototype is that reference for the
window, and every later stage's screenshots are compared with its screens.

- It is `ui-prototype.html` beside this plan: one file that opens with a
  double-click. No code moves from it into `gozler-ui`, because it imitates
  Win32 controls and is not built from them. It is deleted with this plan.
- It showed the filters three ways; the owner chose variant A, the list
  boxes that the spec describes, and the other two are removed.
- A "Prototype" panel beside the window chooses the states and the daemon
  failures, shows the model's state after each action, and measures the
  window's minimum size.
- It covers:
  - the main window: the search box, the match controls, the filter pane, the
    results list, the inspector and the status bar, with their sizes and
    spacing;
  - every menu and context menu, with the commands, groups, separators,
    labels, shortcuts and enabled states of the spec;
  - every dialog: Settings with its five pages, Custom date, Custom
    size, Search syntax, About and the message boxes of "Feedback". The spec
    has no confirmations, because no command destroys anything;
  - the states S1 to S13 and every daemon failure;
  - synthetic data with long names, deep paths, Unicode names, more than
    100,000 matches and multiple selections, and replies that take as long
    as the daemon's: about 0.3 s for a search and for rows scrolled to;
  - focus, Tab order, access keys, keyboard navigation, resizing and the
    minimum size;
  - the Language menu and Settings > General > Language, with English and
    the pseudo-language;
  - 150 % scale and the high contrast colours, switched in the panel.
- It cannot answer what only Win32 decides, so
  [design: Not verified](ui-design.md#not-verified) stays as it is: UI
  Automation, `IsDialogMessageW` with access keys, the marquee, DPI scaling of
  dialog templates and of the shell's icons. Stage 2 and later stages check
  them in the real window.

**Done when:**
- Screenshots of every screen, in English and in the pseudo-language, at 100
  and 150 %, pass [the screenshot standard](#the-screenshot-standard).
- The owner has used the prototype and approved its screens, and each
  correction is in the spec.

## Stage 1: the shared code and folder sizes

Two milestones, each reviewed and committed on its own: the shared code,
then folder sizes. The owner installs the build after both, once.

### Milestone 1: the shared code

- Move the pipe client into `gozler-client`, with the service-state query,
  the scope-folder check, its own error type and the cancel check
  ([design: Shared code](ui-design.md#shared-code)). Its connect takes the
  pipe name and whether to check the server's owner. Its tests serve their
  own pipe with tokio's `named_pipe::ServerOptions` as a dev-dependency,
  because a pipe created with raw Win32 calls would put `unsafe` outside
  `gozler-winfs`; they never need the running daemon.
- In `gozler-core`: `regex_test` becomes public; the `-parent` rule becomes a
  public function that `build_search` calls; the size ranges move into a
  table that the parser reads; `QueryError::Malformed` carries a typed reason,
  with the same `Display`; the export date formatter moves into
  `gozler_core::output`.
- Word start and name start in `gozler-core` and the daemon: the
  `wordstart:` and `startwith:` search functions, `ParseOptions::term_match`
  in place of `whole_word`, the engine's two tests, and `PROTO_VERSION` 7
  ([spec: Search box](ui-spec.md#search-box),
  [design: Shared code](ui-design.md#shared-code)). `DESIGN.md`'s "Search",
  the help and `CONTEXT.md` describe them.
- The search-only help: `gozler_core::escompat::search_syntax()`, beside
  `help`, returns the search terms, the search functions and the attribute
  letters, rendered from the same tables and with the same section code as
  the full reference, and nothing about options, shell quoting or exit codes
  ([spec: Commands](ui-spec.md#commands), C22). It is a function, not a
  third `HelpScreen`, because `HelpScreen` holds what a command line asks
  for, and no switch asks for this screen. Its headings name no switch, so
  the attribute letters' heading drops "/a".
- `deny.toml` and the Ubuntu CI stub cover `gozler-client`.

**Done when:**
- Both golden corpora pass with no changed row, and every parser message is
  unchanged, so `gozler` behaves and writes as before. `gozler --help`
  changes only by the rows of `wordstart:` and `startwith:`, and by the
  `nopath:` and `regex:` rows, reworded so that they read correctly in the
  search-only help too.
- `search_syntax()` needs no new test: its rows are the full reference's
  rows, which `everything_the_help_lists_parses` already parses. Reading
  its text once shows the three sections and no option, quoting or exit
  code.
- An engine test over the names `Paracetamol 500mg.pdf`, `IMG_1234.jpg`,
  `comparator.txt` and `my report.txt` gives: `wordstart:para wordstart:50`
  the first; `wordstart:1234` the second; `wordstart:para` not the third;
  `startwith:para` the first and not the fourth; `wordstart:PARA` the first,
  and with `-case` none. With `-p`, `wordstart:` matches where a word of the
  full path starts. Each fails if the boundary check before the match is
  lost or the test reads the full path instead of the name.
- `failure.rs` has a test case for each client error, with the exit code and
  the first line of the message it gives today: each service state, the pipe
  not ready, `Communication`, the owner check, an older daemon, each
  `ProtoError` code of a refusal, a lost connection and a missing scope
  folder.
- A test with a local pipe server that accepts a connection and then neither
  reads nor answers proves that a cancelled request returns `Cancelled`
  within 100 ms while it waits for the reply. A test with a server that is
  busy proves the same for the connect retries. Without the token check, the
  wait lasts 30 seconds (`READ_DEADLINE`) and the retries about 500 ms, so
  both tests can fail.
- Before install, the new `gozler` against the installed, older `gozlerd`
  gives the version gate's refusal ("the running gozlerd is older than this
  client; install the matching gozlerd", exit 7), because `gozler` and
  `gozlerd` install together (DESIGN.md: "Wire protocol").

### Milestone 2: folder sizes

- Each volume's index keeps a size for each folder: the sum of the known
  sizes of every file below it ([spec: Results list](ui-spec.md#results-list),
  [design: Shared code](ui-design.md#shared-code)). The size and date pass
  computes the sums when it ends; every journal change after it keeps them
  exact. The daemon sends a folder's sum as its size; `size:` and the size
  sort read it; `-get-total-size` and `-get-folder-size` still add files
  only. It costs at most 8 bytes a folder, which `--status` shows in the
  index memory.
- `DESIGN.md`'s "Sizes and dates" replaces "A folder has no size, by design,
  so `size:`, `-get-total-size` and `-get-folder-size` skip folders" with the
  rule above, and the help and the golden corpora follow it.

**Done when:**
- A test drives one volume's index through creates, deletes, size changes,
  moves of files and of folders, and hard links, and after each step every
  folder's size equals the sum recomputed from its files. A `proptest` over
  random sequences of those changes fits `AGENTS.md`'s test rules. It fails
  if any one journal path forgets to update the folders above it.
- An engine test: `size:>N` matches a folder whose files add up to more than
  N; the size sort places a folder by its sum; `-get-total-size` over a
  folder and its files counts each file once.
- The golden corpora change only where a folder's row now shows its size,
  and the commit message names those rows.

**After install** (both milestones):
- Against the installed `gozlerd`, `gozler ext:cs -n 3`,
  `gozler -get-result-count ext:cs`, `gozler --status` and
  `gozler -path X:\no-such-folder x` exit with the codes they gave before the
  stage, and their output has the same form: three full paths, one number,
  the same status headings, the same message. The numbers are not compared,
  because the index changes between two runs.
- For a folder in the probe's fixture, `gozler <folder> -size` shows the
  same size as `gozler -get-folder-size <folder>`; after a file below it
  grows, both show the new sum.

## Stage 2: the window, the text and the probe

- The crate `gozler-ui` with its `core`, `win` and `app` modules,
  `#![forbid(unsafe_code)]` on `core` and `app`, the workspace lints, the
  Ubuntu `dead_code` allowance on `core`, and `gozler-ui` in `deny.toml`'s
  `wrappers` ([design: Where the unsafe lives](ui-design.md#where-the-unsafe-lives)).
- First check the `.rc` toolchain items, the spacing numbers and the
  `IsDialogMessageW` access-key item in
  [design: Not verified](ui-design.md#not-verified). Then the `.rc` file with
  the manifest, the icon, the version information and the About dialog
  ([design: The .rc file](ui-design.md#the-rc-file)).
- The command table with every row of the spec's Commands table, and the menu
  bar and the accelerators built from it. The commands of later stages are in
  their menus and disabled.
- The whole text system, before any caption exists
  ([design: Text and languages](ui-design.md#text-and-languages)): `en.json`
  with a key for every text this stage shows, the language table,
  `Strings::load` with its fallback to the parent language and to English,
  plural forms, `Numbers` from `win::locale`, and `apply_text`. No caption
  is written in the code or the `.rc` file. The `pseudo-language` feature and
  a `-Release -Pseudo` mode in `check.ps1` let every stage's screenshots run
  in it too. Later stages only add keys to `en.json`.
- At start, the window loads the `Strings` of the language that
  `settings.language` names, or of Windows default, as the "Start" execution
  path says. The probe's pseudo-language run writes `qps-ploc` there, because
  the Language menu arrives only in stage 8.
- The window procedure trampoline, the panic hook, the fonts and the DPI
  handling, the caption measurement, `Layout::compute` with every control in
  place and empty, the minimum size, the single-instance mutex, the saved
  window placement and the move of a placement that no monitor shows.
- `--data-dir`.
- A `-Release` mode in `check.ps1`, which builds the release executables,
  because the memory budget is measured on what ships.
- `probe.ps1` and its fixture folder. First check that UI Automation reads the
  status bar and the menus. The list has no rows until stage 3, which checks
  the cells.
- `AGENTS.md` names the `win` module in its `unsafe` rule; `DESIGN.md` lists
  `gozler-ui` and `gozler-client`.

**Done when:**
- The `Layout::compute`, placement, `Settings`, `Strings`, catalogue and
  command-table tests pass on Ubuntu.
- The probe finds the search box, the Match list with its five modes and
  Start of words chosen, the Match case and Match path check boxes, the
  list with its default columns, the four status bar parts and every
  top-level menu. Each menu holds the spec's items in the spec's groups and
  order, each item has its access key, and only this stage's commands are
  enabled.
- Help > About Gozler shows the version and Close closes it; File > Exit ends
  the process. Covers: C7, C23.
- The probe creates the fixture folder, waits until `gozler` lists all of it
  with the sizes, dates and attributes it built, and removes it after the
  run.
- A second start leaves one `gozler-ui` process, with the first window in
  front, for a first window that is normal, minimized and maximized; a
  minimized one is restored and a maximized one stays maximized.
- The window moved and closed opens again at the place it was closed.
- In the `-Release -Pseudo` build with `qps-ploc` in `settings.language`, the
  menu bar and the status bar open in pseudo text; with an unknown tag, they
  open in English.
- The probe reports the memory of the idle window: the real baseline.

## Stage 3: search and results

- Requests on their own threads, `Model`, `Results` with the 100,000-row
  limit, the virtual list, header sort, View > Columns, the four status bar
  parts, search as you type, the match controls, Find, Refresh, the Search
  syntax dialog, the first status request, the daemon failures with their
  messages and buttons, and the states S1 to S5, S7 and S9
  ([spec: Searching](ui-spec.md#searching),
  [Results list](ui-spec.md#results-list),
  [Status bar](ui-spec.md#status-bar),
  [States](ui-spec.md#states),
  [Daemon failures](ui-spec.md#daemon-failures);
  [design: Requests](ui-design.md#requests),
  [Rows on demand](ui-design.md#rows-on-demand)).
- The bold matched part of the Name and Path cells, from `gozler-core`'s
  match ranges ([spec: Results list](ui-spec.md#results-list)). DESIGN.md's
  "Not planned" list drops highlighting.
- The keys for what this stage shows: every `QueryError` with its typed
  reason and what to do, and every `VolumePhase`, each mapped with a `match`.
- First check the system image list item in
  [design: Not verified](ui-design.md#not-verified) on the owner's monitors.
- With the first rows on screen, and before the live checks below, check that
  UI Automation reads the cells of the list's visible rows. If it cannot, the
  probe reads them with `LVM_GETITEMTEXTW` through memory it allocates in the
  window's process (`VirtualAllocEx`, `WriteProcessMemory`,
  `ReadProcessMemory`), a documented technique. Screenshots never stand in
  for reading the rows.
- Choose `CHUNK_ROWS`: measure the wait for a chunk at 4,096 and at 16,384
  rows, at offset 0 and near 100,000, and record the figures in the commit
  message.

**Done when:**
- The `Model::accept`, `Model::polls_status` and `Results` tests from "Tests"
  pass on Ubuntu, with one test for each of the transition rows T1 to T5.
  Covers: T1, T2, T3, T4, T5, S9, S13.
- The probe reads the cells of the list's visible rows.

In the fixture folder, written `F\` below:
- For `F\ ext:cs`, status part 1's count equals the fixture's count of `.cs`
  files and `gozler -get-result-count F\ ext:cs`, and the first visible rows
  equal the first lines of `gozler F\ ext:cs -n 20`. Covers: S3, J1 (to the
  selected row).
- After `End` in `F\`, which holds more than two chunks, the last row equals
  the last line of `gozler F\ -n all`.
- After a click on the Size header, the first rows equal `gozler F\ ext:cs -n
  20 -sort size-descending`, and View > Sort by shows Size, descending.
  Covers: C20.
- Each match mode and check box gives the count of the same search on the
  command line, on the fixture's names: Start of words as `wordstart:`,
  Start of name as `startwith:`, Anywhere as a plain term, Whole words as
  `-ww`, Match case as `-case` and Match path as `-p`. With Regular
  expression, the whole text is the pattern, so the check uses a pattern
  that names the fixture folder with `\\` and matches its `.cs` files,
  against `gozler -r` with the same pattern. `Alt+A`, `Alt+C` and `Alt+P`
  reach the three controls. Covers: C19.
- Ten changes to the search text within one second end with the results of
  the last text, and part 1 said "Searching…" in between while the previous
  rows stayed. Covers: S2, T1, T2.
- For `para 50` on the fixture's `Paracetamol 500mg.pdf`, the core test of
  the match ranges gives "Para" and "50", and the screenshot shows them bold.
- A search for a name that no fixture file has shows "No files match." and
  "0 results". Covers: S5.
- View > Columns hides and shows Extension, and the header's right-click menu
  shows the same check marks. Covers: C15.
- Find focuses the search box and selects its text; Refresh runs the search
  again; Help > Search syntax opens and Close closes it. Covers: C17, C18,
  C22.
- The first screenshot is taken before the first reply arrives. Covers: S1.

Elsewhere:
- The empty search shows "Showing the first 100,000 of N results" with N
  above 100,000; `End` shows a row, and the memory budget holds. The count is
  not compared, because the machine's files change between two queries.
  Covers: S4.
- `dz:x` shows the parser's message and how to fix it, and `regex:(` shows the
  daemon's refusal; neither starts the status poll. Covers: S7, S9.
- `dm:yesterdy` shows what is wrong and how to fix it; corrected to
  `dm:yesterday`, the results appear. Covers: J7.
- Closing the window while the empty search runs ends the process within one
  second. This proves only that the join in `WM_DESTROY` does not deadlock:
  the daemon answers the first chunk in about 0.3 s, so the check passes even
  without the token check. Stage 4's export check proves the token. Covers:
  T14.

**The owner's checkpoint.** The owner looks at the stage 3 screenshots of every
state, in English and in the pseudo-language, and answers. Stage 4 starts
after that answer, so the layout and wording that later stages copy are the
ones the owner accepted.

**After install:**
- With the service stopped by the owner, the list shows "The Gozler service
  is stopped." and Start the service; after Windows' permission prompt the
  service starts and the search runs by itself. Covers: S9, T4, J8.
- The owner restarts the service while the window shows results and
  scrolls: the list shows "The connection to the Gozler service was lost…",
  then searches again by itself. Covers: S9, T4.
- After a restart of Windows, while C:\ indexes, part 4 says so, and the
  results complete by themselves. Covers: S13, J9.

## Stage 4: the selection and the actions

- The selection ([design: The selection](ui-design.md#the-selection)), Open,
  Open file location, Copy, Copy as path, Copy name, Select all, Properties,
  the context menu with Shift+F10 and the Menu key, the keys, Export with
  its save dialog, progress, result and Cancel export, and the spec's
  feedback message boxes ([spec: Commands](ui-spec.md#commands),
  [Feedback](ui-spec.md#feedback), [Export](ui-spec.md#export);
  [design: Copy](ui-design.md#copy), [Export](ui-design.md#export)). First
  check the marquee item in [design: Not verified](ui-design.md#not-verified).

**Done when:**
- The `Selection`, `ClipboardData` and command-table tests from "Tests" pass
  on Ubuntu. Covers: C8, C9, C10, J5, S12, T6.
- The probe selects the first fixture row, scrolls more than the cache's
  32,768 rows away, and adds a row there to the selection. Part 2 says
  "Reading the selection…" and the selection commands are disabled in the
  menus; then part 2 shows the two files and the commands are enabled.
  Covers: S12.
- With one fixture row selected, Open, Open file location and Properties are
  enabled; with two, Properties is disabled; with 16, Open and Open file
  location are disabled. The probe reads this from the menus and does not run
  the commands. Covers: C1, C2, C3.
- Edit > Select all with the search box focused selects its text, and with the
  list focused selects every row. Covers: C11.
- File > Export results… for `F\ ext:cs` as CSV, with Size and Date modified
  visible, writes the same bytes as `gozler F\ ext:cs -export-csv FILE -size
  -dm`; the same holds for TSV and TXT, and with Extension and Attributes
  shown. Part 3 then says how many rows were written. Covers: C4, J6.
- During an export of the empty search, part 3 shows its progress and File
  shows Cancel export; Cancel export ends it and its partial file is gone.
  Covers: C5, S11.
- A search of the empty search with another sort, started about 3.5 s into
  an export of the empty search, is still running when the export ends:
  part 3 shows the export's result while part 1 says Searching…, and the
  export went on through the search's start. Covers: T1, T8.
- The `Model::accept` tests for T8 and T9 pass. Covers: T8, T9.
- The core test for T11 passes: a command chosen from the context menu after
  a new search's first rows cleared the selection does nothing. It is here
  and not in stage 3, because the row's context menu and the selection
  arrive in this stage. Covers: T11.
- An export into the fixture folder whose ACL denies the current user file
  creation shows the spec's message box and leaves no file. Covers: T9.
- Closing the window 0.5 s after an export of the empty search starts ends
  the process within two seconds, and the partial file is gone. That export
  takes 3.6 to 3.9 seconds, so an export that ignores the token fails the
  check. Covers: T14.

**After install:**
- Open, Open file location and Properties act on the selected entries; Open
  on a folder and on a file use the open commands, or File Explorer's
  way when they are empty; `Enter` on a found file opens it.
  Covers: C1, C2, C3, J1, J2.
- Copy followed by a paste in File Explorer copies the selected files; Copy
  as path and Copy name followed by a paste in a text editor give the quoted
  paths and the names. Covers: C8, C9, C10, J5.
- A copy of 100,000 selected rows: the probe reads the window's peak private
  bytes (`PeakPagedMemorySize64`) and its longest wait on `WM_NULL` during
  the copy, and whether the clipboard memory still counts in its private
  bytes afterwards. The owner runs the copy, because it replaces the
  clipboard.
- The labels the window shares with File Explorer match File Explorer's
  wording on the owner's Windows.

## Stage 5: the filter pane

- Every filter group, the Choose folder… button, the Custom date and Custom
  size dialogs with their input rules, Clear filters, Search in this folder,
  the buttons of the no-results and missing-folder messages, "· Filtered",
  and the saved filter choices ([spec: Filter pane](ui-spec.md#filter-pane),
  [Dialogs](ui-spec.md#dialogs)).

**Done when:**
- The `Query` tests from "Tests" pass on Ubuntu, one per group.
- Search in this folder on a row of the fixture sets Location to the fixture
  folder `F`. Then each filter gives the fixture's own count for it and the
  count of the `gozler` query it stands for: Kind Documents and `-path F
  ext:<the Documents list>`; no filter and `-path F`; the check box and
  `-parent F`; Last 7 days and `-path F dm:7days`; Large and `-path F
  size:large`; Hidden Exclude and `-path F !attrib:h`. The fixture holds
  files for each case, with the dates, sizes and attributes the probe sets.
  Covers: C12, J3 (the filters), J4.
- With a filter set and no match, the message offers Clear filters and Search
  everywhere, and each does what it says; the pane's Clear filters button
  does the same. Covers: C21, S5.
- With Location set to a fixture subfolder that the probe then deletes, `F5`
  shows "The folder … does not exist." with Choose folder… and Search
  everywhere, and part 1 never says "Searching…". Covers: S8.
- View > Filters hides the pane; with a filter set, part 1 adds "· Filtered".
  Covers: C13, S10.
- Moving onto Custom… with the arrow keys on a closed combo box opens
  nothing; opening it, entering a Minimum above the Maximum and pressing OK
  shows the spec's balloon and keeps the dialog open; Cancel returns the
  previous choice. Choose folder… then Cancel keeps the previous Location.
- The filter choices are the same after a restart.

## Stage 6: the inspector pane

- The three inspector states, the folder size, the selection's sums from its
  files, "Reading the selection…", and the splitter
  ([spec: Inspector pane](ui-spec.md#inspector-pane)). Each attribute letter
  has the key of its word.

**Done when:**
- The `Model::accept` tests for T6 and T7 pass. Covers: T6, T7.
- For a selected fixture file, the inspector's fields match the size, date
  and attributes the probe gave it, and `gozler <its full path> -size -dm`.
  Covers: J3 (the size).
- View > Inspector hides and shows the pane. Covers: C14.
- For one selected fixture file, the inspector shows Open, Open file location
  and Copy as path, each enabled when its Edit or File menu item is, and the
  command table gives the button the same command as the menu item. The
  clipboard text is proven by the `ClipboardData` core test, because the
  probe never uses the clipboard. Covers: C9.
- For a selected fixture folder, the size equals the fixture's sum and
  `gozler -get-folder-size <folder>`; selecting another folder before the
  size arrives shows only the second folder's size. Covers: T7.
- For three selected fixture files, the sum equals the sum of their sizes.
- Select all in `F\`, which holds more rows than the cache, shows "Reading the
  selection…" with the selection commands disabled, then the fixture's total
  size. Covers: S12.
- Changing the selection again (`Shift+Up`) while part 2 says "Reading the
  selection…" ends with the count and sum of the final selection. Covers: T6.
- After Select all on the empty search, the selection's files hold 100,000
  rows: the window's private bytes stay within the budget plus the
  selection's allowance, no message waits more than 100 ms during the read,
  and after the selection is cleared the private bytes fall back.
- The splitter's height is the same after a restart.

## Stage 7: the Settings dialog

- The dialog with its General, Search and Keyboard pages, each page's
  heading, group boxes and Restore defaults, OK, Cancel and Apply, the
  dialog rules, the Command, Shortcut and command-shortcut input rules,
  Browse…, the `Run` and `StartupApproved` values, the File Explorer verb,
  the update check, the shortcut with its keyboard hook, the person's
  command shortcuts in the menus and the accelerator table, the tray and
  `ui.json`
  ([spec: Settings dialog](ui-spec.md#settings-dialog),
  [Window lifetime](ui-spec.md#window-lifetime),
  [Dialogs](ui-spec.md#dialogs); [design: Execution paths](ui-design.md#execution-paths)).
- The Index page as [spec: Window settings and service
  settings](ui-spec.md#window-settings-and-service-settings) says until the
  service settings plan is built: the drives, their states and entry
  counts, what the index holds, and the service's state and memory. The
  Include page, Dates created and Rebuild index wait for that plan, which
  is not written yet.
- First, before the dialog depends on it, the keyboard hook items in
  [design: Not verified](ui-design.md#not-verified): the owner sets `Win+S`
  in a first build and presses it. Windows Search and Start stay closed,
  Gozler comes to the front, and no restart or sign-out is needed. If
  Windows needs one, the dialog gets the spec's message for it before the
  stage goes on. Then check the child-dialog DPI item by reading the
  documentation; the owner checks it on a second monitor after install.

**Done when:**
- Apply is disabled until a value changes. Turning off search as you type,
  pressing Apply, then Cancel, closes the dialog with the setting applied:
  typing then starts no search until `Enter`. The setting is the same after a
  restart. Covers: C6.
- `Esc` closes the dialog and changes nothing that Apply did not apply.
- With `Ctrl+Alt+Shift+F12` chosen as the shortcut, pressing it while
  another of the probe's windows is in front brings Gozler forward with the
  search text selected. Covers: C6.
- A shortcut that another program holds shows the spec's message and keeps
  the dialog open. The probe holds the combination itself with
  `RegisterHotKey` first. Covers: C6.
- On the Keyboard page, `Ctrl+K` added to Edit > Find makes `Ctrl+K` focus
  the search box, and Edit > Find shows `Ctrl+F` as before plus the new key
  in the list; `Alt+F`, `Ctrl+V` and `K` alone are refused with the spec's
  messages; `Ctrl+E` on another command names Edit > Find; Restore defaults
  gives every command its default again. Covers: C6, C18.
- With "Keep Gozler running in the notification area" off, closing the
  window ends the process even with a shortcut set. Covers: C7.
- With "Select the search text when Gozler is activated" on, switching to
  the window from another of the probe's windows selects the search text;
  off, the focus stays on the list row it was on. Covers: C6.
- In the hotkey control, `Ctrl+F` becomes `Ctrl+Alt+F`; with Windows key on,
  `S` alone is accepted as `Win+S`, and `Win+L` shows the spec's message.
  Covers: C6.
- Browse… beside Open files with `C:\Windows\notepad.exe` chosen writes
  `"C:\Windows\notepad.exe" "%1"` into that box. Covers: C6.
- Use Total Commander, with the probe's fixture holding an older and a newer
  `TOTALCMD64.EXE`, fills Open folders and Open files with the newer one's
  path in the spec's command and leaves Open file location alone; with no
  `TOTALCMD*.EXE` in the index, and with the service stopped, it shows the
  spec's balloons and changes no box. Covers: C6.
- With "Keep Gozler running in the notification area" on, the default,
  closing the window hides it and the process keeps running, with or
  without a shortcut; Show window brings it back with the search text
  selected; File > Exit ends the process. The probe checks the window's
  visibility and the process, not the tray icon, because the notification
  area is the owner's taskbar. Covers: C7, C24.
- With the probe holding the window's shortcut before the window starts,
  the window shows the spec's "another program uses" message the first time
  it shows, and the tray's Show window still brings it back.
- With "Show every file and folder when the search is empty" off, the empty
  search shows "Type to search." Covers: S6.
- The Index page lists the volumes and phases of `gozler --status
  --json`. Entry counts are not compared, because they change between two
  reads.
- A damaged `ui.json` opens the window with the defaults.
- The `Model::accept` test for a failure while the dialog is open passes.
  Covers: T10.

**After install:**
- Each open command runs for its own case, emptying a box brings
  back File Explorer's way, and the dialog scales when it moves to a monitor
  with another DPI.
- After a restart of Windows with "Start Gozler when I sign in" on: with no
  shortcut, the window is running, minimized; with `Win+S` set, Gozler is in
  the notification area with no window, and `Win+S` shows it. Turned off,
  the `gozler` value under the current user's `Run` key is gone. Turned off
  in Task Manager's Startup apps, the check box shows off; turned on again,
  Task Manager shows the entry on. The owner checks this, because the `Run`
  key is the owner's, and a probe that wrote it could replace the owner's
  own value.
- With `Win+S` set, Windows Search does not open and Gozler comes forward;
  after choosing another shortcut, and after Exit, `Win+S` opens Windows
  Search again at once, with no restart. With an administrator window in
  front, `Win+S` opens Windows Search, as the spec says.
- The tray icon: a click and `Enter` show the window; its menu holds Show
  window and Exit; it comes back after Explorer restarts; the first hide
  shows the notification once, and later hides show none. Covers: C24.
- Use Total Commander on the owner's PC fills
  `"C:\Program Files\totalcmd\TOTALCMD64.EXE" /O /A /P=L /L="%1"` into
  Open folders and Open files. Then Open on a folder and on a file opens it
  in Total Commander, and Open file location, left empty, opens File
  Explorer with the entry selected.
- With "Search with Gozler" on, File Explorer's menu on a folder and on a
  folder's background shows it, and it opens Gozler with Location set to that
  folder, also when Gozler already runs; off, the item is gone. The owner
  checks this, because the verb is in the owner's registry.
- With "Check for updates" on and an older version installed, the status
  bar names the newer release and its link opens the release page; off,
  Gozler makes no request (Resource Monitor's Network page shows none).
- With the dialog open, the owner stops the service: the dialog stays usable,
  its Index page shows the failure, and the main window shows it behind
  the dialog. Covers: T10.

## Stage 8: languages

- The Language menu at the right of the menu bar, the Language list on
  Settings > General, `MainWindow::set_language` and saving the choice
  ([spec: Language](ui-spec.md#language);
  [design: Text and languages](ui-design.md#text-and-languages), and the
  "Switching the language" execution path). The keys and catalogues exist
  since stage 2; this stage adds only the switch.

**Done when:**
- The `Model::set_strings` test from "Tests" passes on Ubuntu.
- The Language menu is the last menu bar item, aligned right. With English
  alone, it and the Language list in Settings hold Windows default and
  English; in the build with the pseudo-language, they hold that language
  too. A choice in one shows in the other. Covers: C16.
- With `F\ ext:cs` typed, a row selected and the list focused, Language >
  the pseudo-language changes the menu bar, the column headers,
  the filter pane, the match controls, the inspector and the status bar to
  pseudo text at once, with no pause a person would notice. The typed text, the
  selected row, the focus and the top visible row are the same as before, and
  the window opens in the pseudo-language after a restart. Covers: T12, J10.
- `dm:yesterdy` shows its message in the pseudo-language, with the English
  detail where the spec allows it. Covers: S7, J7.
- The screenshots of every screen in the pseudo-language pass the screenshot
  standard.

## Stage 9: install and release

- `Setup.iss` installs `gozler-ui.exe` and adds a Start menu shortcut named
  "Gozler"; the release workflow packages `gozler-ui.exe` with the other two
  executables.

- Uninstalling removes the `gozler` values under the current user's `Run`
  and `StartupApproved\Run` keys, so Windows does not try to start a missing
  program at sign-in.

**Done when:** `installer\Build.ps1` run with `GOZLER_RELEASE=1`, so that it
does not raise the version in `Cargo.toml`, builds a setup that contains
`gozler-ui.exe`, and the release workflow's file list names it. The build
needs Inno Setup 7.

**After install:**
- The Start menu shortcut starts the window, and the window searches through
  the installed `gozlerd`.
- After uninstall, neither `Run` value is left.
- The window moves between monitors with different DPI, and the layout and
  fonts scale with nothing else changing. Covers: T13.
- At 150 % DPI, and in high contrast, every screen passes the screenshot
  standard.

## Tests

The core carries the tests, because it holds the decisions and runs on every
OS. Each pins a rule with one representative case and one case for each
boundary, under `AGENTS.md`'s test rules:

- `Query` to `SearchRequest` through the real parser: a typed `ext:rs` with
  Kind set to Documents gives two clauses that both must match; an unclosed
  `"` in the typed text leaves the filter's clause intact; the parent-folder
  check box gives the same request as `build_search` with `-parent`;
- `Model::accept` and `Model::start_search`: one test for each row of the
  spec's transitions table whose "Checked by" names a core test, named with
  its ID; a reply of an older generation
  changes nothing, for each purpose; a failure of each purpose changes what
  "Requests" in the design says and nothing else; a search that does not
  parse, an empty search with "show nothing", and a missing Location folder
  start no request;
- `Model::polls_status`: true after `NotRunning`, `NotReady` or `Lost` and
  while a volume in scope is bootstrapping or rescanning; false after
  `Refused` and for an offline volume;
- `Results`: at most one chunk is in flight, and none while a search is; the
  loaded rows stay until the new search's first chunk; a changed total
  changes the count; the count stops at 100,000; the cache drops the chunk
  used least recently;
- `Selection`: all positions cached builds the files from the cache; one
  position outside the cache gives `Reading` and one `Selection` fetch for
  the span; the reply builds the files and replaces the cached chunks inside
  the span; an older reply is stale; a new search clears the selection;
- the command table: Open and Open file location are enabled for 1 and 15
  read rows and disabled for 16 and while the selection is read; Properties
  only for one row; Export results… is disabled while an export runs;
- `ClipboardData`: the size it reports equals the bytes it writes, for 3 rows
  and for 100,000 rows; Copy as path gives each path in double quotes on its
  own line; Copy name gives the names; Copy gives the whole `CF_HDROP` block,
  the `DROPFILES` header with its `pFiles` offset and `fWide` set, then the
  double-NUL path list;
- `Layout::compute` at 96, 144 and 192 DPI, with English and pseudo-language
  caption widths: no control overlaps another or leaves the client area; the
  list keeps a minimum size; the status bar parts' widths do not depend on the
  message shown; the minimum window size fits every control;
- placement: a saved placement that no monitor shows moves to the nearest
  monitor;
- `Settings`: a damaged value falls back to its default;
- the catalogues: every shipped catalogue parses with no repeated key (the
  map visitor rejects one), has no key that `en.json` lacks, the same
  placeholders as English for each key, and both plural forms wherever it has
  one; every menu and every dialog has access keys that are unique within it;
  every letter of `Attributes::all_letters()` has its key; the test does not
  check the wording;
- `Strings`: a message missing in a language comes from its parent language,
  then from English, with all its plural forms; a translated message with
  other placeholders than English shows the English; a key that English
  lacks shows its dotted name; Windows default takes the first language of
  the Windows list that ships, then its parent language, then English;
- `Model::set_strings`: it returns no `Fetch` and keeps the results, the
  selection and the typed text.

## The probe

The `win` layer and the handlers are checked by a script that drives the built
window the way a person does, so an agent proves each stage without the owner.
`crates/gozler-ui/probe.ps1` uses UI Automation, which PowerShell 7 loads
(`Add-Type -AssemblyName UIAutomationClient`) and which every common control
supports:

- It starts the built `gozler-ui.exe` with `--data-dir` in its fixture
  folder, so the owner's settings and error log are never read or written,
  even when the probe crashes. For the pseudo-language run, it writes
  `qps-ploc` as `settings.language` in that folder's `ui.json` first.
- It drives the window through UI Automation patterns, which need no
  keyboard focus: Value for the search box, Invoke for buttons and menu
  items, ExpandCollapse for menus and combo boxes, SelectionItem and Scroll
  for the list. A check that needs a real key, such as the keyboard-alone
  journeys, sends it only after `GetForegroundWindow` shows the probe's
  window, because `SendInput` goes to whichever window is in front. When the
  window is not in front, the probe reports those checks as not run, and the
  stage is not done until a run with the window in front has passed them.
- It waits until status part 1 no longer says "Searching…".
- It prints, as JSON, the status bar's parts, the visible rows' cells, the
  inspector's fields, the menus' items with their state, and the process's
  private bytes, peak private bytes and working set. For a responsiveness
  check, it polls `SendMessageTimeout` with `WM_NULL` every 50 ms and reports
  the longest wait.
- It closes the window it started and nothing else. It stops with a message
  when another `gozler-ui` is running, because that window is the owner's and
  the single-instance rule would bring it forward instead.
- It holds a named mutex, `Local\gozler.probe`, for its whole run, and stops
  with a message when another probe holds it, because agents share the
  machine and two probes would use each other's windows and fixtures.
- It never uses the clipboard (see "How every stage works").

**The fixture folder.** The machine's files change all the time, so the window
and `gozler`, which query at different moments, can disagree about a broad
search without either being wrong. Exact comparisons therefore search only a
folder that the probe creates in `%TEMP%`, named `gozler-probe-` and a random
suffix:

- It holds 40,000 files, more than two chunks of the largest `CHUNK_ROWS` and
  more than the cache, so `End`, scrolling and Select all reach rows outside
  the cache; `.cs` files and documents; names that differ only in case, in
  whole words, in where a word starts, such as `Paracetamol 500mg.pdf` and
  `comparator.txt`, and in their folder; files with set sizes, modified
  dates, and the hidden and read-only attributes; subfolders, so folder sizes and
  `-parent` have something to count; one subfolder to delete for the missing
  Location check; and one folder whose ACL denies the current user file
  creation (`icacls <folder> /deny "%USERNAME%:(W)"`), for the failed export,
  because NTFS ignores the read-only attribute on a folder.
- The probe knows each expected count, list and sum from how it built the
  folder. Before a check starts, it waits until `gozler F\ -n all` with the
  size, date and attribute columns equals what it built. Counting the files
  is not enough: the daemon learns of new files from the journal, adds their
  names first, and sets sizes, dates and attributes in later steps.
- The probe deletes the folder after the run. While it holds its mutex, it
  also deletes any `gozler-probe-` folder that an interrupted run left, so
  it never deletes the folder of a probe that is running.

A check that compares with `gozler` uses the `gozler.exe` of the same build,
against the same running `gozlerd`.

## API sketches

These show the shape of the types that the design's rules and execution
paths name. They are provisional: a stage changes them when the code shows a
better shape, and the design changes only when a rule changes.

The core:

```rust
/// Device-independent pixels, as the layout is designed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Dip(pub(crate) i32);

/// Physical pixels on the window's monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Px(pub(crate) i32);

/// The DPI of the monitor that shows the window; 96 is 100 %.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dpi(NonZeroU32);

impl Dip {
    pub(crate) fn to_px(self, dpi: Dpi) -> Px;
}

/// The measured size of each caption that the window places in code.
pub(crate) struct Captions { /* … */ }

impl Layout {
    /// Every control's rectangle above the status bar, the status bar's part
    /// widths, and the window's minimum size.
    pub(crate) fn compute(client: Size, dpi: Dpi, font_height: Px, captions: &Captions, panes: &Panes) -> Layout;
}

/// What a request is for; at most one request of each purpose is live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose { Results, Selection, Status, Export }

/// Which request of its purpose a reply answers; a newer request has a
/// larger number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Generation(u64);

/// A request the model wants sent; `app::requests` runs it.
pub(crate) struct Fetch {
    pub(crate) purpose: Purpose,
    pub(crate) generation: Generation,
    pub(crate) ask: Ask,
}

pub(crate) enum Ask {
    /// One chunk of the list, gathered from all its pages.
    Rows { request: SearchRequest },
    /// A folder's entries and summed size.
    Totals { request: SearchRequest },
    /// The selected rows: one request for the span from the first selected
    /// position to the last. The thread keeps the rows at `positions`, and
    /// the rows of the cached chunks inside the span (`cached`), so the cache
    /// shows the same moment; it drops every other row.
    SelectionRows { request: SearchRequest, positions: Vec<Range<u64>>, cached: Vec<Range<u64>> },
    /// Every match, written page by page; no limit.
    Export { request: SearchRequest, path: PathBuf, format: ExportFormat, columns: ExportColumns },
    Status,
}

/// A reply, in the core's terms, so the model's handling of replies and
/// failures is tested without a daemon.
pub(crate) struct Fetched {
    pub(crate) purpose: Purpose,
    pub(crate) generation: Generation,
    pub(crate) outcome: Outcome,
}

pub(crate) enum Outcome {
    Rows { start: u64, rows: Vec<ResultRow>, total: u64 },
    Totals(QueryTotals),
    /// The selection's files, and the new rows of the cached chunks inside
    /// the span.
    SelectionRows { files: Vec<SelectedFile>, chunks: Vec<(u64, Vec<ResultRow>)>, total: u64 },
    Status(DaemonStatus),
    ExportProgress { rows: u64 },
    Exported { rows: u64, path: PathBuf },
    /// Creating or writing the file failed; the partial file is deleted.
    ExportFailed { path: PathBuf, error: io::ErrorKind, message: String },
    Failed(DaemonFailure),
}

/// The failures the window shows differently; `app` maps the client's error.
/// `Communication`, a pipe that cannot be opened or read, shows as `Lost`.
pub(crate) enum DaemonFailure {
    NotRunning(ServiceState),
    NotReady,
    NotGozler,
    TooOld,
    Refused { detail: String },
    Lost,
}

/// What a reply or an input changed, and the request it makes necessary, if
/// any. `then` is the search again when a status reply changes the answer:
/// the daemon answers after `NotRunning`, `NotReady` or `Lost`, or every
/// volume in scope has become complete. It is a `Status` fetch when a page
/// sets a flag that the latest status does not explain. Chunks come only from
/// `rows_needed`, which the handler calls with the visible range.
pub(crate) struct Accepted {
    pub(crate) change: Change,
    pub(crate) then: Option<Fetch>,
}

pub(crate) enum Change {
    Stale,
    NewSearch { count: u64 },
    Rows { rows: Range<u64>, count: u64 },
    /// The list is empty and shows a message.
    Message,
    Status,
    Selection,
    Inspector,
    Export,
}

/// What the list area shows. Rows and a message never show together.
pub(crate) enum Shown {
    Nothing,
    Rows { request: SearchRequest, count: u64, chunks: ChunkCache },
    Message(ListMessage),
}

/// Why the list shows a message instead of rows; each has its own text and
/// buttons (spec: States).
pub(crate) enum ListMessage { NoResults, TypeToSearch, Parse(QueryError), MissingFolder(String), Failure(DaemonFailure) }

/// The search the list shows, and the search or chunk asked for. The list
/// keeps the shown rows until the new search's first chunk arrives, so it
/// does not empty on every keystroke. Every request about the shown rows is
/// built from the shown search's request, so it names the rows the user sees
/// even while the typed text has already changed.
pub(crate) struct Results { shown: Shown, in_flight: Option<InFlight> }

impl Results {
    /// The row at `index`, or `None` while its chunk has not arrived.
    pub(crate) fn row(&self, index: u64) -> Option<&ResultRow>;
    /// The chunk the visible rows need next; `None` while a search or a
    /// chunk is in flight.
    pub(crate) fn next_chunk(&mut self, visible: Range<u64>) -> Option<Range<u64>>;
}

/// The selection: a set of files built at one moment (design rule 10).
pub(crate) enum Selection {
    None,
    /// Some selected rows are not cached; their one request is due or in flight.
    Reading { positions: Vec<Range<u64>> },
    Files(Vec<SelectedFile>),
}

pub(crate) struct SelectedFile { path: String, is_folder: bool, size: Option<u64>, modified: Option<Timestamp> }

pub(crate) struct SelectionSums { files: u64, folders: u64, bytes: u64, unknown_size: u64 }

/// One copy format's bytes, sized before they are written, so `win::shell`
/// allocates one block of the exact size and nothing is copied twice.
pub(crate) struct ClipboardData<'a> { format: ClipboardFormat, files: &'a [SelectedFile] }

impl ClipboardData<'_> {
    pub(crate) fn len(&self) -> usize;
    /// Writes exactly `len()` bytes.
    pub(crate) fn write_into(&self, block: &mut [u8]);
}

/// One row of the spec's Commands table.
pub(crate) struct Command {
    pub(crate) id: CommandId,
    pub(crate) label: &'static str,
    pub(crate) accelerator: Option<Accelerator>,
    pub(crate) menu: Option<(Menu, Group)>,
    pub(crate) enabled: fn(&Model) -> bool,
    pub(crate) checked: fn(&Model) -> Option<bool>,
}

impl Model {
    pub(crate) fn set_text(&mut self, text: String);
    /// The search the input asks for, or a message when it cannot run.
    /// `scope` is the result of `gozler-client`'s scope-folder check, which
    /// `app` runs first, because the core makes no filesystem call.
    pub(crate) fn start_search(&mut self, scope: ScopeCheck) -> Accepted;
    pub(crate) fn rows_needed(&mut self, visible: Range<u64>) -> Option<Fetch>;
    /// The positions the list says are selected, read once per user action.
    pub(crate) fn set_selection(&mut self, positions: Vec<Range<u64>>) -> Accepted;
    /// The selection's request, 150 ms after the last change.
    pub(crate) fn read_selection(&mut self) -> Option<Fetch>;
    /// Whether the window must ask for status every 2 seconds.
    pub(crate) fn polls_status(&self) -> bool;
    /// Applies a reply; a reply of an older generation changes nothing.
    pub(crate) fn accept(&mut self, fetched: Fetched) -> Accepted;
    /// Replaces the language. It starts no request, because the results do
    /// not depend on the language.
    pub(crate) fn set_strings(&mut self, strings: Strings);
}

/// Regional number formatting; `win::locale` implements it with
/// `GetNumberFormatEx`, the tests with English formatting.
pub(crate) trait Numbers {
    fn integer(&self, n: u64) -> String;
    fn decimal(&self, n: f64, digits: u8) -> String;
}

/// A shipped language: its tag, its own name and its embedded catalogue.
pub(crate) struct Language { tag: &'static str, name: &'static str, catalogue: &'static str }

/// Every shipped language, English first; the Language list shows them.
pub(crate) static LANGUAGES: &[Language];

/// The saved choice; `Windows` follows the Windows display language.
pub(crate) enum LanguageChoice { Windows, Tag(String) }

impl LanguageChoice {
    /// The shipped language to load: the tag, or the first of
    /// `windows_languages` that ships, then its parent language, then English.
    pub(crate) fn resolve(&self, windows_languages: &[String]) -> &'static Language;
}

/// One loaded language, with fallback to its parent language and English.
pub(crate) struct Strings { /* … */ }

impl Strings {
    pub(crate) fn load(language: &'static Language) -> Strings;
    /// The message, or the dotted key when English lacks it.
    pub(crate) fn text(&self, key: &'static str) -> &str;
    /// The message with `{0}`, `{1}`, … replaced.
    pub(crate) fn format(&self, key: &'static str, args: &[&str]) -> String;
    /// The `_one` or `_other` form for `count`, with `count` as `{0}`.
    pub(crate) fn count(&self, key: &'static str, count: u64, numbers: &dyn Numbers, args: &[&str]) -> String;
}
```

The `win` layer:

```rust
/// A window handle that does not own its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hwnd(HWND);

/// One window class's message handling. The trampoline holds an `Rc` clone
/// for the length of each call (rule 2), so a handler may destroy its window.
pub(crate) trait WindowProc: 'static {
    fn message(&self, hwnd: Hwnd, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT>;
}

/// Creates a top-level window owned by `state`. Messages that arrive before
/// `WM_NCCREATE`, such as `WM_GETMINMAXINFO`, go to `DefWindowProcW`.
pub(crate) fn create_window<W: WindowProc>(class: &WindowClass, state: Rc<W>, params: CreateParams) -> windows::core::Result<Hwnd>;

/// Runs a modal dialog from a .rc template. `state` is borrowed for the
/// dialog's whole life, because the call returns only after it closes.
pub(crate) fn run_modal<D: DialogProc>(owner: Hwnd, template: DialogId, state: &D) -> isize;

/// An owned GDI font, deleted on drop.
pub(crate) struct Font(HFONT);

/// The fonts the controls use. `replace` gives the controls the new fonts
/// before it deletes the old ones, because `WM_SETFONT` does not take
/// ownership and a control must never hold a deleted font.
pub(crate) struct Fonts { ui: Font, heading: Font }

/// A list notification decoded from `WM_NOTIFY`. The borrow ties it to the
/// message that carried it, so no handler can keep it.
pub(crate) enum ListNotify<'a> {
    GetDispInfo(DispInfo<'a>),
    CacheHint(Range<u64>),
    /// `LVN_ITEMCHANGED` with a change of `LVIS_SELECTED`, item -1 (every
    /// row) included, and `LVN_ODSTATECHANGED`, which a virtual list sends
    /// for a range such as a Shift+click. The handler reads the selection
    /// from the list once per user action (design: The selection).
    SelectionChanged,
    ColumnClick(Column),
}

impl DispInfo<'_> {
    pub(crate) fn index(&self) -> u64;
    pub(crate) fn column(&self) -> Column;
    /// Copies `text` into the list's buffer, cut to the buffer's length.
    pub(crate) fn set_text(&mut self, text: &[u16]);
    pub(crate) fn set_image(&mut self, image: SystemImage);
}

/// A `GMEM_MOVEABLE` block for one clipboard format. `set` gives it to the
/// open clipboard, which then owns it; dropping an unset block frees it.
pub(crate) struct ClipboardBlock(HGLOBAL);

/// Wakes the main window from a request thread; carries no data (rule 5).
pub(crate) struct Waker(isize);
// SAFETY: PostMessageW may be called from any thread. The window joins every
// request thread in WM_DESTROY, while its handle is valid, so the handle
// never names a destroyed or reused window.
unsafe impl Send for Waker {}
```

The `app` layer:

```rust
struct MainWindow {
    controls: OnceCell<Controls>, // filled in WM_CREATE; Windows destroys the controls
    fonts: RefCell<Fonts>,        // dropped after WM_NCDESTROY, when no control is left
    model: RefCell<Model>,        // never borrowed across a Win32 call (rule 1)
    icons: RefCell<Icons>,        // extension -> system image index; filled with no borrow held
    requests: Requests,
}

impl MainWindow {
    /// Runs one command of the command table; every surface calls this
    /// (rule 9). It asks the model whether the command is enabled now.
    fn run(&self, command: CommandId);
    /// Loads the language and calls `apply_text` (design: "Switching the
    /// language").
    fn set_language(&self, choice: LanguageChoice);
    /// Sets every caption from the model's facts and strings (rule 12).
    fn apply_text(&self);
}

/// The live request of each purpose, and the cancelled ones that have not yet
/// been joined (rule 5); finished ones are joined on each `start` and `drain`.
struct Requests { live: RefCell<[Option<Running>; 4]>, cancelled: RefCell<Vec<Running>>, replies: Receiver<Fetched>, waker: Waker }

impl Requests {
    /// Cancels the live request of `fetch.purpose`, then runs `fetch` on a new
    /// thread that connects, sends, maps the client's error to
    /// `DaemonFailure`, sends a `Fetched` on the channel and wakes the window.
    fn start(&self, fetch: Fetch);
    /// Every reply that has arrived, without waiting.
    fn drain(&self) -> impl Iterator<Item = Fetched>;
    /// Cancels every request and joins every thread, cancelled ones included.
    fn shut_down(&self);
}
```

## When the plan is done

The spec's [acceptance criteria](ui-spec.md#acceptance-criteria) hold, and
the full CI set from `AGENTS.md` passes. The Include page, Dates created
and Rebuild index wait for the service settings plan, so when this plan is
deleted, their rules move into that plan instead of `DESIGN.md`. `AGENTS.md` names the `win` module in
its `unsafe` rule, and `DESIGN.md` lists `gozler-ui` and `gozler-client` among
the crates. The rules of the spec and the design move into `docs/DESIGN.md`,
and the spec, the design and this plan are deleted.
