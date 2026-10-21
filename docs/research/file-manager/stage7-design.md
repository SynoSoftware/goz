# Stage 7: code design

This is the code design for stage 7 of `docs/plans/file-manager-spec.md`:
"One state for each list, then search in a panel, and the measured memory
of a search panel". The spec settles the behavior. This file settles the
code: the types, where each piece of state lives, which function changes,
and what proves each step.

It is written against `HEAD` 95035a8. Stage 6 (the Viewer) is wired into
the same tree before stage 7 starts. Section 0 lists every place where
stage 6's wiring (`.work/stage6-viewer-wiring.md`) touches code that this
design changes.

Names in this file follow `CONTEXT.md`: Search, Two panels, panel, active
panel, other panel, focused row, selected entries, quick search, drive
menu, key bar, results list. "Search list" is the spec's name for a list
that shows gozlerd's results: Search's results list, or a search panel's
list.

Line numbers drift, so callers are named by function.

## 0. Where stage 6 touches the same code

Stage 6 lands first. Apply stage 7 on top of it. These are the overlaps:

| File | Stage 6 adds | Stage 7 changes there |
| --- | --- | --- |
| `core/mod.rs` | `mod viewer; mod viewer_window;` | adds `mod search_list;` |
| `core/panel.rs` | `Panel::focused_entry()`, and `chosen` and `chosen_count` call it | replaces `Panel::chosen` and `focused_entry` with `Panels::chosen(side)` and `Panels::focused(side)` (2.12), because a search panel's rows are not `Entry` values |
| `core/commands.rs` | the View row's `enabled`: `focused_entry().is_some_and(\|e\| !e.is_folder())` | that closure calls `panels.focused(active)` instead (2.12) |
| `app/main_window.rs` | `run` → `view_focused`, which reads `panel.focused_entry()` and `panel.path_of` | `view_focused` reads `panels.focused(active)`, which carries the full path for both kinds of panel |
| `core/model.rs` | `viewer()` and `set_viewer()` after `placement()` | no conflict; stage 7 edits other methods |
| `core/layout.rs` | `image_button_width`, and `status_last_part` after `button_room` | no conflict; stage 7 edits `PanelLayout`, `CaptionWidths` and `compute_two_panels` |
| `resources/ids.h` | 3501 to 3504 | 101 to 108 (2.13) |
| catalogues | a `"viewer"` section after `"key_bar"` | new keys in `"panel"` and `"header"` (2.13) |
| `core/settings.rs` | `ViewerState` | nothing |

Where stage 6 made `chosen` and `chosen_count` call `focused_entry()`,
stage 7 replaces all three, so take stage 6's version as the base and
rewrite those methods as 2.12 says.

## 1. Step 1: one state for each list

Step 1 is a restructuring with its own proof, before any feature. After
step 1, Search behaves exactly as before, and the core can hold a second
search list, a search panel's, whose requests never cancel Search's.

Step 1 is the core of the search panel, without its window. It adds
`View::Search` (1.5), writes every exhaustive match on `View` with its
behavior from 2.5, and builds a panel list's request as 2.3 says. Step 2
adds the window: the controls, keys, captions, layout and texts, and the
`Panels` functions that only the window calls.

### 1.1 The list key

In `core/fetch.rs`, beside `Purpose`:

```rust
/// A list that shows gozlerd's results: Search's results list, or a
/// search panel's list (file-manager-spec.md: Search in a panel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum List {
    Search,
    Panel(Side),
}

impl List {
    pub(crate) const ALL: [List; 3] =
        [List::Search, List::Panel(Side::Left), List::Panel(Side::Right)];
}
```

### 1.2 `Purpose` and `Ask` carry the list

- `Purpose::Results` becomes `Purpose::Results(List)`.
- `Purpose::Selection` becomes `Purpose::Selection(List)`.
- `Purpose::ALL` becomes 13 entries: `Results` and `Selection` for each of
  `List::ALL`, then `Status`, `Export`, `Program`, `IncludeCount`,
  `SettingsSaved`, `FolderSizes(Left)`, `FolderSizes(Right)`. The array
  size is `[Purpose; 13]`.
- `Ask::Rows(ChunkAsk)` becomes `Ask::Rows(List, ChunkAsk)`.
- `Ask::Selection(SelectionAsk)` becomes `Ask::Selection(List, SelectionAsk)`.
- `Ask::FolderSize(Search)` stays. It is only Search's: the inspector
  counts the contents of the one selected folder, and a panel has no
  inspector. `Fetch::purpose` maps it to `Purpose::Selection(List::Search)`.
- `Fetch::purpose`: `Ask::Rows(list, _)` → `Purpose::Results(list)`,
  `Ask::Selection(list, _)` → `Purpose::Selection(list)`.

Update the doc comment of `Purpose` to say that each list has its own
`Results` and `Selection`, so a search in one list never cancels another
list's request (DESIGN.md: window rule 4).

### 1.3 `app/requests.rs`

`Requests` needs no new logic. `live` is sized by `Purpose::ALL.len()`
already, and `slot()` finds a purpose by its position in `Purpose::ALL`,
so 13 purposes give 13 slots. `start` cancels only the slot of the new
fetch's purpose, which now names the list.

`run()` changes only its patterns:

- `Ask::Rows(ask) | Ask::Program(ask) | Ask::FolderSizes(_, ask)` becomes
  `Ask::Rows(_, ask) | Ask::Program(ask) | Ask::FolderSizes(_, ask)`.
- `Ask::Selection(ask)` becomes `Ask::Selection(_, ask)`.

### 1.4 The state of one search list: `core/search_list.rs`

A new module holds what Search's list holds today. One `SearchList` exists
for Search, and one inside each search panel.

```rust
/// One list of gozlerd's results: its typed text, its rows, its selection
/// and the ended operations it still shows blank
/// (file-manager-spec.md: Taken entries).
#[derive(Debug)]
pub(crate) struct SearchList {
    text: String,
    results: Results,
    selection: Selection,
    /// The ended deletes and moves whose rows this list still shows blank
    /// until its next search: each one that took a fetched row when it
    /// ended.
    kept: HashSet<OperationId>,
    /// A search panel's selected size, which its status line shows (Q2);
    /// `None` while nothing is read, for Search's list, or when a counted
    /// folder's size is unknown.
    chosen_bytes: Option<u64>,
}
```

`#[derive(Debug)]` is needed because `Panel` derives `Debug` and will hold
a `SearchList`. `Results` has no `Debug` at `HEAD`, so `Results`,
`Shown`, `ShownRows`, `InFlight` and the chunk type in
`core/results.rs` get `#[derive(Debug)]`. `Selection` has it already.

Where each field of today's `Model` goes:

| `Model` field at `HEAD` | Goes to |
| --- | --- |
| `text` | `SearchList::text` |
| `search_due` | stays on `Model`; it is Search's only, because only Search has "search as you type" off. Its doc comment says so |
| `open_on_answer` | stays on `Model`; Search's only, because `Enter` in a search panel's box goes to the list (spec: Keys in the search box) |
| `results` | `SearchList::results` |
| `selection` | `SearchList::selection` |
| `search_kept` | `SearchList::kept` |
| `taken` | stays on `Model`, one for all lists, because it is the queue's view of the waiting, running and ended operations. Each list decides with its own `kept` |
| `failure`, `status`, `status_asked` | stay; they are the service's state, not a list's |
| `panels` | stays |

`SearchList` methods, moved from `Model` with the same bodies:

- `new() -> SearchList`, `text()`, `results()`, `selection()`.
- `shown_row(index, taken: &Taken) -> Option<&ResultItem>` and
  `row_taken(index, taken) -> bool`, from `Model::shown_row` and
  `Model::row_taken`.
- `ended(&mut self, ended, taken)`: the work of the first loop and the
  `retain` of `Model::operations_ended` for this list, with its loop
  turned inside out. At `HEAD` the loop encodes every fetched row once
  for each ended delete or move. Here it returns at once when no ended
  operation is a delete or a move, or the list has no fetched rows. Else
  it walks the fetched rows once, encodes each row into one reused
  `Vec<u16>`, checks it against each ended delete or move that is not yet
  kept, and stops when all of them are kept. A new
  `selection::wide_path_into(file, &mut Vec<u16>)` does the encoding
  without an allocation for each row; `wide_path` calls it.
- `keeps(id) -> bool`.
- `drop_taken(&mut self, taken: &Taken) -> bool`: the body of
  `Model::set_taken` for this list's selection; returns whether the
  number of selected files changed. It returns `false` at once when
  `taken` is empty or the selection holds no files, so a list with no
  selection costs nothing, and Search's list does less than at `HEAD`,
  where it rebuilt its selection's `Vec` on every call.
- `fn is_taken(taken, kept, row)` moves here as a private function.

Methods that need the model's generation counter, settings or status stay
on `Model` and take a `List` (1.6). `SearchList` gets `pub(super)` fields
only where `Model` writes them, as `Model`'s own fields are `pub(super)`.

### 1.5 Where each list's state lives

- Search's list: `Model::search_list: SearchList`.
- A search panel's list: inside the panel's `View`, in `core/panel.rs`:

```rust
pub(crate) enum View {
    Reading,
    Folder,
    NotReady { drive: String },
    /// The panel shows the results of a search (file-manager-spec.md:
    /// Search in a panel).
    Search(Box<PanelSearch>),
}

#[derive(Debug)]
pub(crate) struct PanelSearch {
    pub(crate) list: SearchList,
    pub(crate) look_in: LookIn,
    /// The name of the row that was focused before the search, which
    /// `Esc` and `Backspace` focus again.
    back: Option<Box<[u16]>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LookIn {
    /// The panel's folder and its subfolders.
    #[default]
    ThisFolder,
    AllDrives,
}
```

Why the search lives in `View`: the representation then cannot hold
results for a panel that shows a folder. When the panel leaves its search,
the `PanelSearch` is dropped with its rows, its selection and its `kept`,
which is the spec's "drops its rows when it leaves the search". A reply
that arrives later finds no list (`Model::list` returns `None`) and
changes nothing, so no extra staleness check is needed.

`Box` keeps `View` small: `Panel` has many fields, and only a search
panel pays for `PanelSearch`.

`View` loses `Clone`, `PartialEq` and `Eq`, because `SearchList` holds
`Results`. Every `== View::Folder` and `!= View::Folder` becomes
`matches!(…, View::Folder)`. The sites at `HEAD`: `Panel::has_up`,
`Panel::row`, `Panel::shown_name`, `Panels::sort`, `Panels::accept_read`
(two), `Panels::apply_changes`, `Panels::ask_sizes`, `Panels::quick_char`,
`PanelCaptions::new`, the `NewFolder` row in `commands.rs`,
`Model::panel_coverage`, and `on_panel_char` in `app/main_window/panels.rs`.
The test `a_stopped_reading_starts_again` compares `&View::NotReady {..}`
with `assert_eq!`; it becomes `assert!(matches!(…))`.

Every exhaustive `match` on `View` gets its `View::Search` arm in step 1,
with the behavior that 2.5 gives it, so step 2 adds no arm.

`Panels` gets, in step 1:

- `search(side) -> Option<&PanelSearch>` and
  `search_mut(side) -> Option<&mut PanelSearch>`.
- `enter_search(side)`, `leave_search(side)`, `leave_on_escape(side)` and
  `row(side, index)`, with their full behavior (2.2, 2.5, 2.6), because
  the step's tests and 2.5's arms use them.

`Model` gets:

```rust
pub(crate) fn search_list(&self) -> &SearchList   // Search's
pub(crate) fn list(&self, list: List) -> Option<&SearchList>
fn list_mut(&mut self, list: List) -> Option<&mut SearchList>
```

`list(List::Search)` is always `Some`. `list(List::Panel(side))` is `Some`
while that panel is a search panel.

### 1.6 `Model` methods: each takes the list

`Model::results()`, `Model::selection()` and `Model::selected_files()` are
removed. Callers use `model.search_list().results()` and so on, or
`model.list(list)`.

| Method at `HEAD` | After step 1 |
| --- | --- |
| `shown_row(index)` | `shown_row(list, index)` |
| `row_taken(index)` | `row_taken(list, index)` |
| `operations_ended(ended, taken)` | same signature; calls `SearchList::ended` on every list that `list()` returns, then `panels.operations_ended`. A list with no fetched rows returns at once (1.4) |
| `keeps(id)` | any list's `keeps(id)`, or `panels.keep(id)` |
| `set_taken(taken)` | same signature; `drop_taken` on Search's list returns `Change::Selection(List::Search)` or `Nothing`; the panels' lists are handled by `Panels::set_taken` (2.7). Returns, besides, whether each list's selection lost files, for `show_taken` (2.7) |
| `set_typed_search(text)` | `set_typed_search(list, text)`. For `List::Search` as today. For a panel list: no `search_due`, always `Then::SearchSoon(list)`, because the panel always searches as the person types (spec: The search) |
| `start_search(start, scope)` | `start_search(list, start, scope)`; 2.3 says what a panel list asks |
| `rows_needed(visible)` | `rows_needed(list, visible)` |
| `set_selection(positions)` | `set_selection(list, positions)` |
| `select(files)` | `select(list, files)`; one folder alone becomes `Selection::Folder` and asks `Ask::FolderSize` only for `List::Search`. A panel list keeps it as `Selection::Files(vec![folder])`. The doc comment of `Selection::Files` changes to "never one folder alone in Search's list" |
| `read_selection()` | `read_selection(list)` |
| `coverage()` | `coverage(list) -> Coverage` over that list's shown search |
| `reads_size_or_date()` | `reads_size_or_date(list)` |
| `indexing_in_scope()` | `indexing_in_scope(list)`, for one list; `polls_status` asks it of every list with a shown search |
| `accept(fetched)` | routes `Purpose::Results(list)` and `Purpose::Selection(list)` to the list, if `list(list)` exists and awaits that generation |
| `accept_results(generation, outcome)` | `accept_results(list, …)`. `NewSearch` clears that list's selection and `kept`. `open_first` only for `List::Search`. Its coverage check uses `coverage(list)`. It also asks for the status when another list shows a failure (the rule below) |
| `show_failure(failure)` | `show_failure(list, failure)` |
| `accept_status(outcome)` | see below |
| `accept_selection(outcome)` | `accept_selection(list, outcome)` |

Search-only methods keep their signatures and use `self.search_list`:
`enter_in_search_box`, `set_matching`, `sort_by`, `set_filters`,
`clear_filters`, `search_everywhere`, `choose_folder`, `set_choices`,
`search_in_this_folder`, `list_buttons`, `can_export`, `start_export`,
`header_sort`, `key_sort`, `shown_sort`.

One rule decides which lists search again, and it lives in one place,
`accept_status` for `Outcome::Status`, because a status reply is the
service answering:

- Every list that shows `ListMessage::Failure` searches again. At `HEAD`
  this is `answers_again`, read from the shared `failure`; with several
  lists it is read from each list's own message, because
  `accept_results` clears the shared `failure` when any one list's rows
  arrive, which would leave the other lists showing a failure that has
  ended.
- Every list with a shown search whose scope finished indexing searches
  again: it was indexing before this status (`indexing_in_scope(list)`
  read before `self.status` is replaced) and is not now.
- No filter by mode: Search's list recovers while Two panels shows, and a
  search panel's list while Search shows. A search panel holds no shown
  search and no message while Search shows (2.14), so it asks nothing
  then.

The result is `Then::SearchAgain(lists)` when `lists` is not empty. The
not-indexed recheck of 2.8 adds its lists to the same `Vec`.

A list's rows are the other way the service answers. `accept_results`
for `Outcome::Rows` asks for the status (`status_needed`) when another
list shows a failure, as it already does for flags the status must
explain; that status reply then runs the rule above. So `Accepted` keeps
its one `then`, and the rule has one owner.

`accept_status`, `Outcome::Failed`: each list that shows a different failure and is not
  searching takes the new failure. The change is `Change::Status` in every
  case; the app's `Change::Status` handler already updates the status bar,
  and it now also refreshes the message of Search's list and of each panel
  (1.8). At `HEAD` the shown list's update returned `Change::Message`,
  whose handler also clears the list's selection and count; a list that
  already shows a failure has neither, so `Change::Status` loses nothing.

### 1.7 `Change` and `Then`

Changes that describe one list carry it:

- `Change::Searching(List)`
- `Change::NewSearch { list, count, open_first }`
- `Change::Rows { list, rows, count }`
- `Change::Message(List)`
- `Change::Selection(List)`
- `Change::SelectionRead { list, rows }`

`Change::FolderCounted` stays Search's only.

- `Then::SearchAgain` becomes `Then::SearchAgain(Vec<List>)`.
- `Then::SearchSoon` becomes `Then::SearchSoon(List)`.
- `Then::ReadSelectionSoon` becomes `Then::ReadSelectionSoon(List)`.

Search-only methods return `Then::SearchAgain(vec![List::Search])`.

### 1.8 Every caller

`core/model.rs` tests: the helpers `search(model, text)` and
`rows(fetch, total)` build `Purpose::Results(List::Search)`; `rows`
matches `Ask::Rows(List::Search, ask)`. Each test that calls
`set_selection`, `shown_row`, `row_taken` or `start_search` passes
`List::Search`. `model.results.row(5)` in the tests becomes
`model.search_list.results.row(5)`. No test changes what it asserts.

`core/commands.rs`:

- `searched()`: `state.model.results.has_searched()` becomes
  `state.model.search_list().results().has_searched()`.
- `files()`, `chosen()` and the rows that read `selected_files()` in
  Search mode read `search_list().selection().files()`.
- The tests that call `model.set_selection(…)` pass `List::Search`.

`core/captions.rs`: every `model.results()` and `model.selection()` and
`model.selected_files()` becomes the Search list's. The functions:
`StatusCaptions::new` (its selection and results parts),
`InspectorCaptions` (selection, results), `totals_status`,
`results_status`, `selection_status`, `index_status`, `list_message`,
`title` (`model.text` becomes `search_list().text()`). In step 2,
`totals_status`, `index_status` and the counts take a `List` (2.10).

`app/main_window.rs`:

| Function | Change |
| --- | --- |
| `search(hwnd)` | `search(hwnd, list)`. The `folder_is_missing` scope check runs only for `List::Search` (2.3) |
| `fetch_visible()` | `fetch_visible(list)`: reads the visible rows of that list's control |
| `apply()` | each `Then` and `Change` that carries a list goes to Search's controls for `List::Search`, or to the panel's for `List::Panel(side)` (2.6) |
| `apply()`, `Then::ReadSelectionSoon(list)` | cancels `Purpose::Selection(list)` and starts that list's selection timer |
| `apply()`, `Change::Status` | also `update_message()` and, in Two panels, `show_panel(side)` for each side, for the failure update of 1.6 |
| `column_fits` | `model.results()` → `model.search_list().results()` |
| `row_menu` | `model.results().row(…)` → Search list's |
| `on_control`, search box `EN_CHANGE` | `set_typed_search(List::Search, text)` |
| `on_list_notify`, `CacheHint` | `rows_needed(List::Search, …)` through `fetch_visible(List::Search)` |
| `read_list_selection` | `set_selection(List::Search, …)` |
| `fill_cell`, `draw_cell` | `shown_row(List::Search, row)`; `matched_parts` from Search list's results |
| `on_key` | `FocusList` uses the Search list's count |
| `open_first_or_selected` | Search list's results and selection |
| `selected_entries`, `drag_selected`, `copy`, `present_entries` | Search list's `selection().files()` |
| `show_taken` | `row_taken(List::Search, row)`, and only when `set_taken` says the Search list's selection lost files or is read (2.7) |
| `message()`, `Timer(timer::SEARCH)` | `search(hwnd, List::Search)` |
| `message()`, `Timer(timer::SELECTION)` | `read_selection(List::Search)` |

Timers: `timer::SEARCH` (1) and `timer::SELECTION` (3) stay Search's. Step
2 adds each panel's, in the pattern of `PANEL_SETTLE`:

```rust
/// Each search panel's search waits this long after the last keystroke,
/// by side.
pub(super) const PANEL_SEARCH: [usize; 2] = [10, 11];
/// Each search panel's selection is read this long after it last changed,
/// by side.
pub(super) const PANEL_SELECTION: [usize; 2] = [12, 13];
```

and two functions `timer::search(list)` and `timer::selection(list)` that
return the ID for a list, so `apply()` sets the right one.

### 1.9 Proof of step 1

- `pwsh ./check.ps1`, the full script, green: every existing test passes
  unchanged in what it asserts (they pin Search's behavior and the
  folder panels'), and clippy and the formatting pass over the new
  `View::Search` arms.
- The step's new tests: T1 (a search in one list leaves another list's
  request alone), and the core tests of the search panel that need no
  window, T2 to T8 (section 4).

## 2. Step 2: search in a panel

### 2.1 State

`PanelSearch` and `LookIn` from 1.5. The search panel keeps these `Panel`
fields with their meaning:

- `folder`: the searched folder, and the folder that `Esc` and
  `Backspace` show again.
- `sort`: the panel's sort, for its folder and its search (spec: The
  sort).
- `free`: the free space of the searched folder's drive.
- `focused`, `selected`: the list's focused row and selected rows as the
  list last told them. In a search panel, `selected` holds only the
  ranges: no `Entry` carries the mark.
- `quick`: the quick search box's text.
- `history`, `drive_folders`, `saved`: unchanged.

These are emptied when the panel enters its search, because they describe
a folder's rows: `entries`, `order`, `totals`, `shown`, `kept`, `read`,
`sizes_asked`, `folder_sort`, `gone_focus`, `awaited`.

### 2.2 Entering a search

`Find` (`Alt+F7`, `Ctrl+F`) in Two panels runs `Model::find(side)`:

1. On a search panel: nothing changes in the model. The app focuses the
   panel's search box and selects its text.
2. On any other panel: `Panels::enter_search(side)`:
   - `back` = the name of the focused entry, when the view is `Folder`.
   - `folder` = the panel's `folder`. Before the first listing, `folder` is
     empty; then it is the path of the reading in progress.
   - Empties the fields listed in 2.1, so `kept` loses this panel's ended
     operations; the app then calls `release_operations()`.
   - `view = View::Search(Box::new(PanelSearch { list: SearchList::new(),
     look_in: LookIn::ThisFolder, back }))`.
3. Then `Model::start_search(List::Panel(side), …)` with the empty text,
   which shows "Type a name…" or the not-indexed message (2.8).

The app, for a panel that becomes a search panel:

- `panel_threads.cancel_read(side)`, which also stops watching the folder;
  `requests.cancel(Purpose::FolderSizes(side))`; kills
  `PANEL_SETTLE[side]` and `PANEL_AGAIN[side]`.
- swaps the controls (2.13) and focuses the search box.

`Find`'s row in `commands.rs` belongs to both modes. Its shortcuts are
`Ctrl+F` and `Ctrl+E` today; in Two panels, `TWO_PANEL_KEYS` adds
`Alt+F7` first, so the menu shows `Alt+F7`, and `Ctrl+F` (spec: Commands
table, "Find | Alt+F7, Ctrl+F | always"). `run(Find)` in Two panels calls
`Model::find(active)`. The test
`each_mode_takes_only_its_own_commands_and_keys` changes for `Find`; that
is an intended change, so the test is rewritten for the new rule.

### 2.3 The request

`Model::start_search(List::Panel(side), start, scope)` builds:

```rust
let filters = match look_in {
    LookIn::ThisFolder => Filters { location: Some(folder.clone()), ..Filters::default() },
    LookIn::AllDrives => Filters::default(),
};
query::search(text, Matching::default(), &filters, start)
```

and asks `Results::start_search(generation, search, panel.sort())`.

- `Matching::default()` is Start of words, without Match case and Match
  path, confirmed in `gozler_core::query`. Search's `settings.window.matching`
  and `settings.window.filters` are never read, so the typed syntax gives
  the rest (spec: The search).
- `Filters::location` is "the folder and its subfolders", the same scope
  as Search's Location filter.
- The sort is the panel's. Search's sort is never read or written.

The order of the checks, each of which starts no request:

1. `LookIn::ThisFolder` and the latest status known and
   `Coverage::of(volumes, Some(folder), false).volumes().next().is_none()`:
   `ListMessage::NotIndexed { root }` (2.8). This is the test
   `panel_index_status` uses for "not indexed" today.
2. The text is empty after `trim`: `ListMessage::TypeToSearch`. A panel
   ignores Search's "show everything when empty" setting, because the
   spec gives the panel its own "Type a name…" message.
3. The text does not parse: `ListMessage::Parse`.

The panel's search makes no `folder_is_missing` check. That check reads
the disk on the window's thread, and a search panel's folder is the one
the panel just showed. A folder deleted since then gives no results.

Errors and the "Missing folder" message stay Search's: `MissingFolder` is
never shown by a panel.

### 2.4 The timer

`set_typed_search(List::Panel(side), text)` returns
`Then::SearchSoon(List::Panel(side))`; `apply()` sets
`PANEL_SEARCH[side]` to 150 ms. The timer runs `search(hwnd,
List::Panel(side))`. A new key restarts the timer. Changing Look in runs
the search at once.

### 2.5 Each match on the panel's view

| Code at `HEAD` | `View::Search` |
| --- | --- |
| `Panel::has_up` | `false`: a search panel has no `..` row |
| `Panel::count` | the search list's `results().count()` |
| `Panel::row`, `Panel::entry` | stay for folder rows. They become private. The app reads rows through `Panels::row(side, index)` (below) |
| `Panel::shown_name` | `None`: no Copy or New folder awaits a name in a search panel |
| `Panel::counts` | not called; the status line has its own text (2.10) |
| `Panels::start_reading` | returns `Option<ReadAsk>`; `None` for a search panel. The app then runs the panel's search again (2.14). `stop_reading` has already dropped any `Go` in flight, so there is none to restart |
| `Panels::read_again` | `None`: nothing to read |
| `Panels::restart_reading` | unchanged: a `Go` in flight (leaving the search) starts again with a new generation and the panel's sort, as after a drive's removal query failed; with none, `read_again`, which is `None` |
| `Panels::sort` | stores the sort. With a `Go` in flight it restarts it in the new sort, as today; else `None` for a search panel, and the app runs the search again, so gozlerd sorts (spec: The sort). So `sort_panel` searches again only when `sort` returned `None` and the panel is a search panel |
| `Panels::drive_removed` | unchanged. A search panel has no watch, so a removal reaches it only for a `Go` in flight to that drive; then it shows "not ready" for that folder, as a folder panel does, which drops the `PanelSearch` |
| `Panels::go`, `go_typed`, `go_up`, `go_drive` | start a `Go` reading as for any panel. The search stays shown until the reading opens (below) |
| `Panels::accept_read`, `Opened` of a `Go` | `view = View::Reading`, rows cleared: this drops the `PanelSearch` |
| `Panels::accept_read`, `Failed` of a `Go` | as for `View::Folder`: refused, the panel stays as it is, so a search panel stays a search panel and the message box says why |
| `Panels::apply_changes`, `accept_folder_order`, `ask_sizes`, `accept_sizes`, `take_folder_sort` | nothing: they act on a folder's entries only, as for `Reading` today |
| `Panels::operations_ended` | skips a search panel: its list decides in `Model::operations_ended` |
| `Panels::set_taken` | returns `[Option<Place>; 2]`; `None` for a search panel (2.7) |
| `Panels::quick_char`, `quick_step` | search the fetched, not taken rows only (2.11) |
| `Panels::enter` | `EnterAction::Read(go(side, parent, Some(name)))` for a result; a folder result goes to the folder itself (2.6) |
| `Panels::set_selected`, `has_selection` | the ranges only (2.1) |
| `Panels::chosen_count` | follows the list's `Selection`: the number of files read; 0 while the selection is read; with nothing selected, 1 when the focused row is fetched and not taken, else 0 |
| `Panels::selectable(side, row)` (new; replaces `move_focus`'s `panel.entry(row).is_some()`) | a folder panel: the row has an entry, so not `..`. A search panel: every row unless a delete or a move took it. A row not fetched yet is selectable, because its position is the selection; only a fetched row can be known as taken |
| `Panels::await_name` | does nothing for a search panel and returns, so F6 rename from a search panel awaits no name in a folder the panel does not show |
| `Panels::show_awaited` | `None`: nothing is awaited |
| `PanelCaptions::new` | the search branch (2.10) |
| `Model::panel_coverage` | `None` for a search panel; the search's coverage shows instead (2.10) |
| `commands.rs`, `NewFolder` row | disabled, because it checks `View::Folder` |
| `on_panel_char` | allowed for `View::Search` too |
| `transfer_start`, the other panel's view | `View::Search`: no folder text; F5 and F6 are disabled when both panels search (2.12) |

`Row` gets a third variant, and `Panels::row(side, index) -> Option<Row<'_>>`
replaces the app's calls of `Panel::row` and `Panel::entry`:

```rust
pub(crate) enum Row<'a> {
    Up,
    Entry(&'a Entry),
    /// A search panel's fetched row that no operation has taken.
    Result(&'a ResultItem),
}
```

For a search panel, `None` means a blank row: not fetched, or taken.
`Panels::row` uses `Panels.taken` and the list's `kept`, as
`SearchList::shown_row` does. The app's callers at `HEAD`:
`on_panel_key`, `move_focus`, `read_selection`, `panel_menu`,
`fill_panel_cell` and `draw_panel_row` in `app/main_window/panels.rs`.

### 2.6 Leaving a search

The spec's ways out: `Esc` with nothing selected, `Backspace`, `Enter` on a
result, and a choice in the drive menu.

In `core/panel.rs`:

- `Panels::leave_search(side) -> Option<ReadAsk>`: when the panel is a
  search panel, `Some(self.start(side, folder, ReadKind::Go { focus:
  back, typed: false }, true))`. `back` focuses the row that was focused
  before the search. `nearest` is `true`, so a searched folder deleted
  during the search opens the nearest folder above it that exists, as
  the saved folder does at start.
- `Panels::leave_on_escape(side) -> Option<ReadAsk>`: `leave_search` when
  nothing is selected, else `None`. The app first reads the list's
  selection (`list.selected()` and `Panels::set_selected`, window rule
  10), because the model's copy may be older than the list; it calls
  `leave_on_escape` only after the quick search box was closed. So `Esc`
  does the first of: close the quick search box, clear the selection,
  leave the search (spec: `Esc`).
- `Panels::enter(side, row)` on a result: the parent folder of its path,
  with its name focused. A folder result: its own path, with nothing
  focused. Both through `go`, with `nearest` false as for any row a
  person enters. The name comes from `wide_path(item)`, so a name with an
  unpaired surrogate keeps its exact units.

Keys, in `app/main_window/panels.rs::on_panel_key`:

| Key | Folder panel today | Search panel |
| --- | --- | --- |
| `Esc` in the list | closes quick search, else clears the selection | the same, then `leave_on_escape` |
| `Backspace` in the list | `go_up` | `leave_search` |
| `Enter` in the list | `enter_row` | `enter_row`, which reads the `Go` |
| `Esc` in the search box | (path box: `PathEscape`) | `PanelKey::LeaveSearch` → `leave_search` |

The drive menu's choices go through `go` and `go_drive` as today.

While the `Go` reads, the search panel shows its results, as a folder
panel keeps its rows until the next folder opens. When the `Opened` event
drops the `PanelSearch`, the app syncs its controls. Add
`fn follow_panel_kind(&self, side)` in `app/main_window/panels.rs`,
called from `show_panel(side)`. It compares the model's kind (search or
not) with what the controls show, and on a change:

- to search: shows the search box and Look in, hides the path box,
  inserts the Folder column (2.9);
- to folder: the reverse; cancels `Purpose::Results(List::Panel(side))`
  and `Purpose::Selection(List::Panel(side))`; kills
  `PANEL_SEARCH[side]` and `PANEL_SELECTION[side]`; clears the search
  box's text; calls `release_operations()`.

The controls' kind is a `Cell<bool>` per panel in `PanelControls`, so the
check is one comparison.

### 2.7 Each list releases only its own ended operations

- `Model::operations_ended` calls `SearchList::ended` for Search's list
  and for each search panel's list. Each list keeps the ended deletes and
  moves that take one of its fetched rows.
- `Model::keeps(id)` is true while any list keeps `id`, or a folder panel
  does.
- A list releases its kept operations on its own new search
  (`NewSearch` clears its `kept`), and a search panel releases them when
  it leaves the search, because its `PanelSearch` is dropped. In both
  cases the app calls `release_operations()`.
- `Panels::set_taken` calls `drop_taken` on each search panel's list. A
  taken row shows blank (spec: "A result that a delete or a move takes
  shows blank").

The pass that deselects taken rows in the list control. At `HEAD`,
`show_taken` reads the Search list's selected rows from the control
(`ListView::selected`, one `LVM_GETNEXTITEM` per selected row unless the
selection runs to the last row) and calls `row_taken` on each, which
encodes the row's path, on every call, that is, whenever an operation
starts or ends. Stage 7 runs that pass for a list only when it can find
something:

- its `drop_taken` returned `true` (a selected file was taken), or
- its selection is `Selection::Reading`, because the rows being read may
  be taken already, and `select` leaves them out of the files but not
  out of the control's highlight.

A list with no selection, or whose selected files were not taken, costs
no Win32 call and no encoding. Each list that holds rows redraws its
visible rows, one `LVM_REDRAWITEMS`. So Search's list does less work than
at `HEAD`, and each search panel does the same work as Search's list.

### 2.8 The not-indexed state and the list area's messages

`ListMessage` gets `NotIndexed { root: String }`. It is shown only by a
search panel with `LookIn::ThisFolder`. It is checked first (2.3), so the
person sees it before typing (spec: Search in a panel).

The latest status may be unknown when the panel enters its search, or a
drive may become indexed or stop being indexed. So when a status reply
arrives, `accept_status` runs check 1 of 2.3 again for each search panel
with This folder and subfolders that shows `TypeToSearch`,
`NotIndexed`, or no results. It adds the list to `Then::SearchAgain` only
when the check's answer differs from what the list shows: `NotIndexed`
shown and the folder now indexed, or the other states and the folder now
not indexed. So a status poll every 2 seconds starts no request while
nothing changed.

The panel's message label shows, by state:

| State | Text |
| --- | --- |
| Empty text, This folder | "Type a name to search C:\Users\me and its subfolders." |
| Empty text, All drives | "Type a name to search all indexed drives." |
| Not indexed | "E:\ is not indexed, so Gozler cannot search it.\nChoose All drives in Look in to search the indexed drives." |
| No results, This folder | "No items match your search.\nChoose All drives in Look in to search every indexed drive." |
| No results, All drives | "No items match your search." |
| Does not parse | `query_error`, as Search |
| Service failure | `failure_text`, as Search, without buttons |

`captions::list_message` stays Search's. Add
`captions::panel_list_message(model, side) -> Option<String>`, which
`PanelCaptions::new` uses for `message` in the search branch. Retry never
shows in a search panel.

The message's height comes from its wrapped text, as Search's does. At
`HEAD`, `compute_two_panels` gives the panel's message a fixed
`font_height * 2`, which cuts a third line, and a search panel's
two-sentence messages wrap to three or more lines in a narrow panel.
`place_message` in `main_window.rs` already sizes Search's message with
`MessageArea::new(list, header, dpi)` and
`TextMeter::wrapped_height(text, area.text_width())`, and places its
buttons under it. Make it take the list's rect, the header's height, the
message control and its buttons, and call it for a panel too, with Retry
as the one button when it shows. `PanelLayout` then drops its `message`
and `retry` rects; `list_min_height` stays as the minimum the list
needs.

### 2.9 Columns and the Folder column

In `core/panel.rs`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelColumn { Name, Ext, Folder, Size, DateModified }

impl PanelColumn {
    pub(crate) const FOLDER: [PanelColumn; 4] = [Name, Ext, Size, DateModified];
    pub(crate) const SEARCH: [PanelColumn; 5] = [Name, Ext, Folder, Size, DateModified];
    /// `None` for Folder, whose header does not sort (spec: The sort).
    pub(crate) fn sort_key(self) -> Option<SortKey>;
    pub(crate) fn right_aligned(self) -> bool;   // Size
}
```

`COLUMNS` in `app/main_window/panels.rs` is replaced by these arrays and
methods. `panel_cell` takes a `PanelColumn`, not an index.
`panel_headers` returns the headers for an array; Folder's header is the
new key `header.folder`. (`panel.folder` already exists: it is the Size
cell of a folder whose size is unknown.)

The Folder cell, in `captions::panel_cell` for `Row::Result(item)`:

- This folder: the parent of `item.path` below the searched folder, such
  as `Projects\Accounts`, or empty for a result directly in it.
- All drives: the full parent path.

The function `relative_folder(searched: &str, path: &str) -> &str` lives
in `core/panel.rs` beside `is_within`, and returns a slice, so no cell
allocates more than its text. It counts the searched folder's
components (`C:\` is one, `C:\Users\me` three) and skips that many
components of the result's parent, without comparing their text, so no
folding is needed: gozlerd answers a scoped search only with paths below
the scope. When the parent has fewer components than the searched
folder, which a correct reply never gives, it returns the full parent.

Drawing: `PanelRow::right_aligned: &[bool]` becomes
`fits: &[CellFit]` with `enum CellFit { End, Right, Path }` in
`win/notify.rs`. `Path` draws with `DT_PATH_ELLIPSIS`, as
`win/notify.rs` already does for Search's path cell, so a long folder
loses its middle (spec: The list). Folder rows draw as `End`; Size as
`Right`.

The Name column's icon for a result comes from the same source as
Search's Name column (by extension and `is_dir`), so `icon_of` gets a
`Row::Result` arm.

Matched parts are not drawn bold in a panel: the prototype shows plain
names.

Widths: Name and Folder share the room left by Ext, Size and Date
modified, 1.3 to 1, as the prototype's grid does. The Folder column is
inserted at index 2 when the panel becomes a search panel and deleted
when it leaves (`ListView::insert_column`, `delete_column`, which Search
uses for its columns).

### 2.10 The status line and the status bar

The status line's left part, in `PanelCaptions::new` for a search panel:

| State | Text |
| --- | --- |
| First answer not yet in | "Searching…" (`status.searching`) |
| Results, nothing selected, This folder | "1,204 results in C:\Users\me" |
| Results, nothing selected, All drives | "7 results on all indexed drives" |
| Selection read | "3 of 1,204 selected · 4.2 GB" (`panel.selected` and the size) |
| Selection being read | "3 of 1,204 selected" |
| Any message state | empty |
| Quick search open | "Find: …", as a folder panel |

The count is gozlerd's total, as Search's "N results"; a list capped at
100,000 rows still says the total.

The size is `SearchList::chosen_bytes` (1.4): the sizes of the selected
files and folders, without the size when a counted folder's size is
unknown, as `Counts::bytes` for a folder panel. An entry below another
selected folder is left out, so it is not counted twice (Q2).

It is computed once, in `Model::select(list, files)` for a panel list,
when the selection's files are read (window rule 10); every status line
draw reads the stored value. The computation: put the paths of the
selected folders into a `HashSet<&str>`; for each selected entry, walk
its parent paths as slices of its own path and leave it out when one is
in the set; add the rest. It allocates only the set, and its cost is the
number of selected entries times their depth, once per selection.

The status line's right part:

- This folder: the free space of the searched drive, in the `free`
  label, as a folder panel.
- All drives: the focused result's full folder, in a new label per panel
  created with `SS_RIGHT | SS_NOPREFIX | SS_PATHELLIPSIS`, so a long
  folder loses its middle (spec: the status line table). The free label is
  hidden. The app updates this label when the focus moves
  (`list_changed`, which already reads the focused row).

The status bar's right group in Two panels:

- Active folder panel: `panel_index_status`, as today.
- Active search panel: the search's index state and totals, as in Search
  (spec: The status bar in Two panels). `index_status` and
  `totals_status` take a `List`; in Two panels with an active search
  panel they use `List::Panel(active)`. `status_widest` adds the totals
  and the volume texts to the panel samples, so `captions.status.panel_index`
  reserves the room.

### 2.11 Keys and focus

`Focus` in `commands.rs` gets `PanelSearch` (a panel's search box) and
`LookIn` (a panel's Look in drop-down). `focus_of` in `main_window.rs`
returns them for the new controls.

`control_key` tables:

| Focus | Key | Action |
| --- | --- | --- |
| `PanelSearch` | `Down`, `Enter` | `PanelKey::ToList`: the list gets the focus, and row 0 the focus when no row has it |
| `PanelSearch` | `Esc` | `PanelKey::LeaveSearch` |
| `PanelSearch` | `Tab` | `PanelKey::OtherPanel`, as from the path box |
| `LookIn` | `Enter` | `PanelKey::LookInEnter`: the search box gets the focus |

`Alt+L` goes to Look in through the label's access key: the label
"&Look in:" is created just before the drop-down, so `IsDialogMessageW`
moves the focus to the drop-down, as Search's "Match" label does for its
drop-down. Creation order per panel: drive button, search box, Look in
label, Look in drop-down.

In a search panel's list:

- `Ctrl+R` runs `refresh_panel`, which for a search panel runs the
  search again (spec: `Ctrl+R`) instead of `read_again`. `Ctrl+R` is in
  `TWO_PANEL_KEYS`, an accelerator, so it works in the search box too.
- `Ctrl+L` focuses the search box (Q4).
- `Alt+F7`, `Ctrl+F` focus the search box with its text selected (2.2).
- `CacheHint` (`on_panel_notify`) calls `fetch_visible(List::Panel(side))`.
  A folder panel ignores it, as today.
- A selection change calls `Model::set_selection(List::Panel(side), …)`
  besides `Panels::set_selected`, so the selection is read from gozlerd
  (spec: "Its selection is read from gozlerd, as Search's is").

Quick search in a search panel looks only at fetched rows that no
operation has taken (spec: Quick search). It walks the cached chunks, not
the positions: a new `Results::fetched_from(from, down) -> impl
Iterator<Item = (u64, &ResultItem)>` gives the fetched rows in position
order from `from`, down or up, chunk by chunk, so the walk visits at most
the 32,768 cached rows and never a position whose row is not in the
window. `first_match` uses it for a search panel and skips taken rows.

### 2.12 Commands

`Panels::chosen(side) -> Vec<ChosenEntry>` and
`Panels::focused(side) -> Option<ChosenEntry>` replace `Panel::chosen` and
stage 6's `Panel::focused_entry`:

```rust
pub(crate) struct ChosenEntry {
    /// The full path as Windows names it.
    pub(crate) path: Vec<u16>,
    pub(crate) is_folder: bool,
}
```

- A folder panel: built from `Entry` and `wide_path_of`, as
  `selected_entries` does now.
- A search panel: from the list's selection files when read; while the
  selection is read, `chosen` is empty and the commands that need it are
  disabled, as Search's are while `Selection::Reading`.

Callers: `selected_entries` in `main_window.rs`; `panel_menu`,
`open_panel_entries`, `transfer` and `transfer_start` in
`main_window/panels.rs`; stage 6's `view_focused`; and the View row in
`commands.rs`.

Each command in a search panel:

| Command | Rule |
| --- | --- |
| F3 View | the focused result, when it is a file |
| F4 Edit | disabled, as in a folder panel |
| F5 Copy, F6 Move | the selected results, or the focused one; disabled when the other panel is a search panel too, because there is no destination folder |
| F6 rename | when one result is chosen, the rename dialog's folder is that result's parent; Q5 |
| F7 New folder | disabled |
| F8 Delete | the selected results, or the focused one; the rows show blank (2.7). `focus_after_chosen` is skipped, because the rows stay in place |
| File > Open | opens a file result with Settings' "Open files" command; a folder result goes there, as `Enter` |

### 2.13 Controls, layout, identifiers and texts

Controls per panel, in `PanelControls`:

- `search: Edit` (the search box), with the cue "Find files and folders"
  of Search's box (`search.cue`).
- `look_in_label: Label` "&Look in:".
- `look_in: DropDownList` with "This folder and subfolders" and "All
  drives".
- `folder: Label`, the All drives status-line label (2.10).

`ids.h`, in the main window's block, below every menu item ID (menu
items start at `0x100`):

```c
// Each search panel's search box, Look in and the status line's folder.
#define IDC_LEFT_SEARCH 101
#define IDC_LEFT_LOOK_IN_LABEL 102
#define IDC_LEFT_LOOK_IN 103
#define IDC_LEFT_FOLDER 104
#define IDC_RIGHT_SEARCH 105
#define IDC_RIGHT_LOOK_IN_LABEL 106
#define IDC_RIGHT_LOOK_IN 107
#define IDC_RIGHT_FOLDER 108
```

101 to 108 are free among `IDC_` values at `HEAD`; the `IDD_` values 101
to 117 are another kind, which `build.rs` keeps apart. Check again after
stage 6.

`Layout::compute_two_panels`:

- `PanelLayout` gets `search`, `look_in_label`, `look_in` rects in the
  path row: the drive button, then the search box, then the label and the
  drop-down at the row's right end. Both rows are computed for every panel;
  the app shows one.
- `PanelLayout` gets `counts_half` and `folder` rects for All drives: the
  status line splits in half, counts left, the folder right.
- `CaptionWidths` gets `look_in_label` and `look_in` (the widest item plus
  `DROP_DOWN_CHROME`).
- The panel's minimum width becomes the larger of `LIST_MIN_WIDTH` and
  the search row. It applies whether or not a panel searches, because
  `Layout::compute` gives one minimum size for the mode, and a panel can
  become a search panel at any moment. With the constants of
  `core/layout.rs`, the search row of the right panel, which has `MARGIN`
  on both sides (the left panel has `RELATED` on its inner side, so it is
  narrower), needs:

  `MARGIN 11 + (drive + DROP_DOWN_CHROME 30) + RELATED 7 +
  SEARCH_MIN_WIDTH 120 + RELATED 7 + look_in_label + LABEL_GAP 4 +
  (look_in + DROP_DOWN_CHROME 30) + MARGIN 11`

  that is, 220 DIP plus three measured widths: the widest drive text
  (`CaptionWidths::drive`), "Look in:" and "This folder and subfolders".
  `LABEL_GAP` sits between a label and its control, as Search's "Match:"
  label and its drop-down. With Segoe UI 9 pt at 96 DPI these measure
  about 15, 42 and 140 DIP, so the search row is about 417 DIP against
  `LIST_MIN_WIDTH` 300. The two-list part of the minimum client width
  becomes `2 × panel_min + 1`, about 835 DIP instead of 601; the
  `status.min_width` and `key_min` terms still apply through `max`. The
  exact figure comes from `Layout::compute` with the measured widths; the
  implementer states it in the stage's report.

Catalogue keys, `en.json` (and the same keys in `es`, `nl`, `tr`):

```json
"panel": {
  "look_in": "&Look in:",
  "this_folder": "This folder and subfolders",
  "all_drives": "All drives",
  "type_here": "Type a name to search {0} and its subfolders.",
  "type_everywhere": "Type a name to search all indexed drives.",
  "search_not_indexed": "{0} is not indexed, so Gozler cannot search it.\nChoose All drives in Look in to search the indexed drives.",
  "try_all_drives": "Choose All drives in Look in to search every indexed drive.",
  "results_in_one": "{0} result in {1}",
  "results_in_other": "{0} results in {1}",
  "results_everywhere_one": "{0} result on all indexed drives",
  "results_everywhere_other": "{0} results on all indexed drives"
},
"header": {
  "folder": "Folder"
}
```

The "No results" text joins `list.no_results` and `panel.try_all_drives`
with a line break. The access key of "Look in" must not clash with the
menu bar's in each language: L is free in English (F, E, V, H, G). The
translations choose their own free letter: Spanish menus use A, E, V, U,
I; Dutch B, W, D, H, T; Turkish D, Z, G, Y, I.

### 2.14 The mode switch

- `stop_panels` (Two panels → Search) also, for each search panel:
  cancels its `Results` and `Selection` purposes, kills its two timers,
  and drops its rows: the list's `Results` becomes `Results::new()` and
  its selection `None` and its `kept` empty, while the text and Look in
  stay. Then `release_operations()`. The spec says a search panel "costs
  memory only while it shows"; the rows would be replaced on the return
  anyway, so holding them while Search shows only costs memory.
- `start_panels` (Search → Two panels): `start_reading` returns `None`
  for a search panel (2.5), and the app runs `search(hwnd,
  List::Panel(side))`, which asks again with the kept text, Look in and
  sort. The status line says "Searching…" until the first chunk.

### 2.16 Work on the window's thread

Owner ruling: a wait in the window is a defect. Stage 7 multiplies the
per-list work by three lists. The design keeps two promises: a list that
holds no rows and no selection costs nothing, and no list does per-row
work that Search's list did not do at `HEAD`.

| Work | Stage 7 |
| --- | --- |
| Quick search | walks the at most 32,768 cached rows, never the 100,000 positions (2.11) |
| `operations_ended` | returns at once for a list without fetched rows or without an ended delete or move; else one pass over the cached rows, one encoding each into a reused buffer, instead of one encoding per row per ended operation at `HEAD` (1.4) |
| `Model::set_taken` | `drop_taken` returns at once when `taken` is empty or the list holds no selected files (1.4) |
| `show_taken`'s deselect pass | runs for a list only when its selection lost files or is being read (2.7) |
| `select` in a panel list | `chosen_bytes` once per selection (2.10) |

Costs that grow with the results and were already there at `HEAD`, which
stage 7 makes reachable from search panels too. These are for the lead
to file as issues; their cost is suspected from the code, not measured:

1. `SearchList::ended` (at `HEAD` the loop in `Model::operations_ended`)
   still encodes every cached row, up to 32,768, of each list that holds
   rows, whenever a delete or a move ends. It reads the rows' paths; the
   operation's own sources would decide it with no per-row work if
   `Taken` could answer "does this operation take anything below these
   rows' scope".
2. `drop_taken` (at `HEAD` `Model::set_taken`) encodes each selected file
   to test it against `Taken`, up to 100,000 for a full selection, on
   each start or end of an operation while files are selected.
3. `show_taken`'s deselect pass, when it runs, reads the selection from
   the control with one `LVM_GETNEXTITEM` per selected row (unless the
   selection runs to the last row) and encodes each selected fetched row.
4. `Shift+End`, `Shift+Home` and `Shift` with the arrow keys in a panel
   (`move_focus`) select the rows passed one `LVM_SETITEMSTATE` at a
   time, and each write notifies the window (`LVN_ITEMCHANGED`). From the
   top of a 100,000-row search panel, `Shift+End` is 100,000 calls; in a
   folder panel of a million entries, a million.

   Whether a range state removes it: `LVM_SETITEMSTATE` with item -1
   sets the state of every row, so it serves only a range that ends at
   the last row or starts at row 0. `ListView::change_selected` already
   uses it so: select all, then clear the rows before the range. For
   `Shift+End` that reduces the calls to the number of rows outside the
   range that are not selected, the smaller part only when the focus is
   near the top. `LVN_ODSTATECHANGED` is a notification that an
   owner-data list sends to its parent when it changes a range itself;
   the window cannot send it to set a range. So no single message selects
   an arbitrary range, and a range in the middle still costs one call per
   row. Stage 7 does not change `move_focus` beyond `selectable` (2.5);
   the issue is the per-row loop for long ranges.

### 2.15 Screenshots

Saved for the owner, not waited for (AGENTS.md: Writing a plan, item 5).
Each from the release exe with `--data-dir`, driven by posted messages as
`.work/stage4/shoot.ps1` does, so no window comes to the front:

1. A search panel (This folder and subfolders, with results) beside a
   folder panel.
2. The "not indexed" state.
3. A search panel with All drives, which shows the Folder column's full
   paths and the status line's folder.

## 3. The memory of a search panel

What is measured: the private bytes (`Process.PrivateMemorySize64`, as
`probe.ps1`'s `Memory-Of` reads them) of the release `gozler-ui.exe`.

How: one run of a script in `.work/stage7/`, not a probe that reruns. The
exe comes from `pwsh ./check.ps1 -Release`. It runs with `--data-dir` set
to a new folder, so no saved state of the owner's applies. The script
drives the window with posted and sent messages, as
`.work/stage4/shoot.ps1` does: `WM_COMMAND` for View > Two panels and
Find, `WM_SETTEXT` on the search box, `CB_SETCURSEL` with
`CBN_SELCHANGE` on Look in, `LVM_ENSUREVISIBLE` on the panel's list. It
waits for states, never a fixed time: it polls the status line's text and
the row count (`LVM_GETITEMCOUNT`), and a row's first cell, as
`probe.ps1`'s `Wait-LastRow` does, with a give-up time only for a failure.

The search: All drives, with a query that matches more than 100,000
entries, such as `*`; the script checks that the status line names more
than 100,000 results. The first answer fetches the chunk at the top
(rows 0 to 16,383). The script then brings row 20,000 into view and waits
for it, which fetches the second chunk (16,384 to 32,767), so the cache
holds its maximum, two full chunks of 32,768 rows.

The figures:

| Point | State | Budget |
| --- | --- | --- |
| B0 | Two panels, both panels listed their folders and show their free space | the baseline |
| B1 | the left panel a search panel, its cache full | B1 − B0 ≤ 7.5 MB: DESIGN.md's 6.5 MB for a full cache, plus the 1 MB that needs no reason |
| B2 | both panels search panels, both caches full | B2 − B0 ≤ 15 MB |
| B3 | both panels left their searches, then searched again, both caches full | B3 − B2 < 1 MB: the freed rows' memory is reused, nothing leaks |
| B4 | View > Search, the empty search scrolled to its last row, as DESIGN.md's budget measures | recorded beside DESIGN.md's 16 MB |

A figure over its budget is a defect to find (what holds the bytes), not
a number to document. Stage 8 writes the figures into DESIGN.md's Memory
budget.

If `check.ps1 -Release` cannot replace the exe because the owner's copy
runs, the script uses the debug exe and reports only the differences
(B1 − B0, B2 − B0, B3 − B2), with that note.

## 4. Tests

Each test is in `core`, driven by fixture values, with no daemon. Names
are proposals.

| Test | File | Failure it watches |
| --- | --- | --- |
| T1 `a_search_in_one_list_leaves_another_lists_request_alone` | `model.rs` | Search starts a search, then the left panel starts one. The two fetches have different purposes, and Search's reply is still accepted as a new search of Search's list. Without per-list purposes, `Requests::start` cancels Search's request and Search shows "Searching…" for ever; the compiler and the Search-only tests do not see it |
| T2 `leaving_a_search_shows_the_folder_with_its_row_focused` | `panel.rs` | A panel lists `C:\a` with `b.txt` focused, enters a search, and `leave_search` (what `Esc` and `Backspace` call) asks to read `C:\a`. After `Opened` and the listing, the view is the folder and `b.txt` is focused. A wrong `back` or a search left in place breaks `Backspace` then `Enter`, the way back that the spec promises |
| T3 `the_first_esc_clears_the_selection_and_keeps_the_search` | `panel.rs` | With rows selected, `leave_on_escape` returns `None`; with none, it returns the read. The spec says the first `Esc` never drops a search whose results are selected; a simplification to "Esc leaves" loses the person's results |
| T4 `each_list_releases_only_its_own_ended_operations` | `model.rs` | Search's list and a search panel both show `C:\x.txt`; a delete of it ends; both keep it. Search's new search releases it from Search only: `keeps` is still true. The panel leaves its search: `keeps` is false. Watches the spec's rule that one list's new search never shows another list's deleted rows again, and that an ended operation still leaves the queue |
| T5 `a_folder_that_is_not_indexed_says_so_before_typing` | `model.rs` | Status indexes `C:` only; the left panel shows `E:\x` and enters its search with no text: the list shows `NotIndexed { root: "E:\\" }`. With All drives it shows `TypeToSearch`. An empty list here reads as "no such file" |
| T6 `a_panel_search_asks_with_the_default_match_and_the_panel_sort` | `model.rs` | With Search's Match case and Match path on and Search sorted by Size, the panel sorted by Date modified, the panel's fetch is `Ask::Rows(List::Panel(Left), ask)` with `ask.sort` Date modified, the scope `C:\Users\me`, and `match_case` false; Search's sort is still Size. Watches a panel search that takes Search's controls or writes Search's sort, which the spec forbids |
| T7 `the_folder_cell_is_the_path_below_the_searched_folder` | `panel.rs` | `relative_folder("C:\\Users\\me", "C:\\Users\\me\\Projects\\Accounts\\a.txt")` is `Projects\Accounts`; a result directly in the folder gives an empty text; the root `C:\` with `C:\a\b.txt` gives `a`; a path with fewer components than the folder gives its full parent. Watches the root's trailing separator, which shifts the component count by one, and a slice past the end, which would panic on a malformed reply |
| T8 `a_file_below_a_selected_folder_counts_once` | `model.rs` | A panel list selects `C:\a` (a folder of 10 bytes) and `C:\a\b.txt` (4 bytes) and `C:\c.txt` (1 byte): `chosen_bytes` is 11. With `C:\a`'s size unknown it is `None`. Watches the double count of Q2 and the rule that an unknown folder size hides the sum, which a simplification to `Sums::of` would break |

No test for: the timer IDs, the captions' wording, the columns' order, or
the layout's new rects beyond the existing layout tests at the minimum
size and 150 % DPI, which already check that every control fits and now
include the search row. Those tests must still pass; if the minimum size
changes, they pass with the new minimum, which is an intended change.

## 5. Risks and open questions

### Load-bearing assumptions, and how each was confirmed

| Assumption | Confirmed in |
| --- | --- |
| `Requests::start` cancels only the slot of the new fetch's purpose, and the slots follow `Purpose::ALL` | `app/requests.rs`, `start` and `slot` |
| Generations come from one counter on `Model`, so a reply to a dropped panel list can never match a newer list's request | `Model::generation`, `last_generation` |
| `Results::accept` ignores a generation it does not await | `Results::awaits` and `Results::accept` |
| `Matching::default()` is Start of words, no Match case, no Match path | `gozler_core::query::Matching` |
| `Filters { location: Some(folder) }` searches the folder and its subfolders | `query::search` with `Filters::location`, as Search's Location filter |
| "Not indexed" is a coverage with no volume | `captions::panel_index_status` uses `coverage.volumes().next()` |
| A folder panel keeps its rows until a `Go` opens, and a refused `Go` keeps the view | `Panels::accept_read`, `Opened` and `Failed` arms |
| Main-window control IDs must stay below `0x100` | `MenuItem::id` and the comment in `ids.h` |
| The list view can draw a cell with `DT_PATH_ELLIPSIS` | `win/notify.rs` already draws Search's path cell so |
| `IsDialogMessageW` moves the focus to the control after a label's access key | Search's "M&atch:" label and its drop-down work so today |
| `Alt+L` in a panel's list stays quick search, and only the search box's `Alt+L` reaches Look in | `win::window::run_message_loop` sends `WM_CHAR` and `WM_SYSCHAR` to the window's own keys before `IsDialogMessageW` |
| Folder panels keep their entries while Search shows; `stop_reading` only drops `read` | `Panels::stop_reading`, `start_panels` |
| `Panels::sort` and `restart_reading` restart a `Go` in flight with a new generation and the new sort | `Panels::restart_reading` reuses `read.path`, `read.kind` and `read.nearest` |
| A selection that runs to the last row can be set with `LVM_SETITEMSTATE` on -1 and then cleared before its start | `ListView::change_selected` does so |
| A message's height can follow its wrapped text | `place_message` with `MessageArea` and `TextMeter::wrapped_height` |
### Assumptions not confirmed

- A static control with `SS_PATHELLIPSIS | SS_RIGHT` draws a right-aligned
  path with its middle cut. The design uses a separate label for it, so
  no style changes at run time. If it draws wrongly, the label can be
  drawn with `DrawTextW` and `DT_PATH_ELLIPSIS | DT_RIGHT` in `WM_PAINT`.
  One run of the screenshot of 2.15 item 3 settles it.
- A scoped search answers with paths that start with the scope's own
  components, so `relative_folder` may count components without comparing
  them. I did not check what gozlerd answers for a scope reached through
  a junction or a mount prefix (CONTEXT.md: Mount prefix), where the
  result's path could name another route to the folder. The first
  screenshot of 2.15 on a normal folder confirms the common case; a
  wrong count shows a wrong Folder cell, never a panic, because of the
  full-parent fallback.
- `*` matches every entry in gozler's syntax. If not, the memory script
  uses another query that the status line shows matching more than
  100,000 entries.

### Spec gaps, with proposed resolutions

- **Q1: the status line while the first answer is out, and in a message
  state.** The spec gives only the result and selection forms. Proposed:
  "Searching…" until the first answer, empty in every message state, as
  the prototype's "before typing" and "not indexed" screens show an empty
  line.
- **Q2: the selection's size when a file below a selected folder is
  selected too.** A folder panel's entries are siblings, so they never
  overlap. A search panel's results can: a folder and a file inside it.
  The spec's rule, "adds the sizes of the counted files and folders",
  then counts the file twice. Resolved: leave out an entry below another
  selected folder. The result is kept in `SearchList::chosen_bytes` and
  computed once in `Model::select(list, files)` for a panel list (2.10,
  T8). Search's own sum avoids the overlap another way: it leaves out
  folders.
- **Q3: the minimum width of Two panels grows.** Accepted. 2.13 gives the
  formula from `core/layout.rs`'s constants: 220 DIP plus three measured
  widths, about 417 DIP per panel against 300, so about 835 DIP for the
  two lists. It applies whether or not a panel searches.
- **Q4: `Ctrl+L` in a search panel.** The spec says "focus the path box",
  which a search panel hides. Proposed: it focuses the search box, the
  control in the path box's place.
- **Q5: F6 rename in a search panel.** The rename's folder for one chosen
  result is that result's parent, not the panel's folder. Proposed as
  written in 2.12.
- **Q6: the mode switch.** The spec does not say what a search panel does
  when Search shows. Proposed in 2.14: it drops its rows and searches
  again on the return, which matches "costs memory only while it shows".
- **Q7: a search panel before the first status.** No "not indexed" can
  show until the service answers. Proposed in 2.8: each status reply runs
  the check again for a This folder and subfolders panel that shows
  "Type a name…", "not indexed" or no results, and searches again only
  when the answer changed.
- **Q8: translations.** The four catalogues need the keys of 2.13. The
  Spanish, Dutch and Turkish texts and their access keys are the
  translator's; the English texts are the spec's and the prototype's.
