//! The window's facts: the loaded language, the saved settings, what the
//! controls ask for, what the list shows, the selection, the export, the
//! service fix and what the service said last. Captions are built from
//! these facts when they are shown, never stored (DESIGN.md: window rule
//! 12). The fields are private to the core: every reply passes through
//! [`Model::accept`], and every change the person makes through a method of
//! its own, and each says what changed and which request that makes
//! necessary.

use super::fetch::{
    Ask, ChunkAsk, DaemonFailure, ExportAsk, Fetch, Fetched, Generation, Local, Outcome, Programs,
    Purpose, WriteFailure, inside,
};
use super::file_operations::StatusOperation;
use super::fixer::{FixEvent, FixWork, Fixer};
use super::layout::{Dip, Panes};
use super::open::{TOTAL_COMMANDER_SEARCH, newest_total_commander};
use super::panel::{Panels, Side, View};
use super::query::{self, Filters, LocationItem, Matching};
use super::results::{Fix, ListButton, ListMessage, Results, RowsChange, Shown};
use super::selection::{FolderContents, Selection, wide_path};
use super::settings::{Choices, Mode, Placement, SavedColumn, Settings};
use super::text::{self, LanguageChoice, Strings};
use super::update;
use gozler_core::escompat::EsPlan;
use gozler_core::include::IncludeSettings;
use gozler_core::output::Format;
use gozler_core::proto::{DaemonStatus, ResultItem, VolumeStatus};
use gozler_core::query::{ParsedQuery, QueryStart};
use gozler_core::scope::{Coverage, leaves_matches_out};
use gozler_core::search::Search;
use gozler_core::types::{EsColumn, SortDir, SortKey, SortSpec, VolumePhase};
use gozler_core::winpath::parent_of;
use gozler_fileops::{OperationId, Request, Taken};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) struct Model {
    pub(super) strings: Strings,
    /// Also the one copy of the match controls, the filters, the sort and
    /// whether the filter pane shows.
    pub(super) settings: Settings,
    /// The columns the list shows, in the list's column order: Name first,
    /// because only the first column carries the icon.
    pub(super) columns: Vec<Column>,
    /// The typed search text.
    pub(super) text: String,
    /// Whether the search for the typed text waits for the person to stop
    /// typing (`Then::SearchSoon`).
    pub(super) search_due: bool,
    /// Whether `Enter` in the search box asked to open the first row of
    /// the search in flight once it answers (DESIGN.md: window rule 8).
    pub(super) open_on_answer: bool,
    pub(super) results: Results,
    /// The latest status reply.
    pub(super) status: Option<DaemonStatus>,
    /// The gozlerd.exe files, as the latest status reply found them.
    programs: Programs,
    pub(super) selection: Selection,
    pub(super) export: Export,
    fix: Fixer,
    /// The file operation the middle group shows.
    operation: Option<StatusOperation>,
    /// The jobs in the order they last started, the newest last.
    started_jobs: Vec<Job>,
    /// How the latest request for the results or the status failed; `None`
    /// once the service answers.
    failure: Option<DaemonFailure>,
    /// The status request in flight.
    status_asked: Option<Generation>,
    /// The Use Total Commander link's search in flight.
    program_asked: Option<Generation>,
    /// Settings > Include's count in flight.
    include_count_asked: Option<Generation>,
    /// The notice that the Include settings are saved, in flight.
    settings_saved_asked: Option<Generation>,
    /// Whether Settings shows its Index page, which follows the service's
    /// state every 2 seconds (DESIGN.md: The Index page).
    index_page_shown: bool,
    /// The newer release that Check for updates found, such as `0.4.0`.
    newer_release: Option<String>,
    latest_release: LatestRelease,
    last_generation: u64,
    /// Whether this window offers the file operations: only the first
    /// window does, because its queue owns the saved operations
    /// (file-manager-spec.md: Interrupted operations).
    file_operations: bool,
    /// Whether the clipboard holds files, which Paste copies or moves.
    clipboard_files: bool,
    /// The entries that deletes and moves have taken, which the list shows
    /// blank and the selection leaves out (file-manager-spec.md: The person
    /// never waits for the disk).
    taken: Arc<Taken>,
    /// The ended deletes and moves whose rows the list still shows blank
    /// until its next search, because the rows it shows are of the moment
    /// its search ran: each one that took a fetched row when it ended.
    search_kept: HashSet<OperationId>,
    panels: Panels,
}

/// What the window knows of GitHub's latest release once the service is
/// older than the window: whether it is older than the window too, which
/// makes the service's fix a downgrade (DESIGN.md: Daemon failures).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LatestRelease {
    Unknown,
    Asking,
    /// A release that could not be read counts as not older.
    Known {
        older: bool,
    },
}

/// What a reply or an input changed, and what it makes necessary.
#[derive(Debug, PartialEq)]
pub(crate) struct Accepted {
    pub(crate) change: Change,
    pub(crate) then: Option<Then>,
}

/// What `Enter` in the search box does ([`Model::enter_in_search_box`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchBoxEnter {
    /// The search runs at once.
    Search,
    /// The selected rows open, or the first row when none is selected.
    Open,
    /// The search in flight opens its first row when it answers.
    Wait,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Then {
    Fetch(Fetch),
    /// The search again, with the clock of the moment it starts: the
    /// person changed a match control or the sort, the service answers
    /// again, or every volume in scope became complete.
    SearchAgain,
    /// The search, once the person has stopped typing for a moment.
    SearchSoon,
    /// [`Model::read_selection`], once the selection has stopped changing
    /// for a moment, so one request serves a run of Shift+Down presses. The
    /// live `Selection` request ends now, because it reads an older
    /// selection (DESIGN.md: T6).
    ReadSelectionSoon,
    /// Ends the live request of the purpose; its reply would be stale.
    Cancel(Purpose),
    /// Asks GitHub whether its latest release is older than the window.
    AskLatestRelease,
    /// Runs the work on the fix's thread.
    RunFix(FixWork),
    /// Asks the person whether to download the setup the fix found.
    ConfirmSetup,
    /// Tells the person that the latest release gives no setup to install.
    ExplainNoSetup,
    /// Ends the fix's thread.
    StopFix,
    /// Starts the checked setup, and closes the window for it.
    RunSetup(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Nothing,
    /// The typed text, which the title shows.
    Typed,
    /// A match control or the sort, which their controls show.
    Choice,
    /// A filter, which the filter pane shows.
    Filters,
    /// A pane was shown or hidden, or the inspector's height changed.
    Panes,
    /// Search or Two panels now shows, which every control, the menus and
    /// the accelerators follow.
    Mode,
    /// The column at `index` of [`Model::columns`] was added.
    ColumnShown {
        index: usize,
    },
    /// The column that was at `index` of [`Model::columns`] was removed.
    ColumnHidden {
        index: usize,
    },
    /// A new search runs; the list keeps its rows.
    Searching,
    /// A new search's first rows replace the old ones, and nothing is
    /// selected; `open_first` when `Enter` in the search box waited for
    /// them to open the first row.
    NewSearch {
        count: u64,
        open_first: bool,
    },
    /// Rows of the shown search arrived, and the count is `count`.
    Rows {
        rows: Range<u64>,
        count: u64,
    },
    /// The list is empty and shows a message.
    Message,
    /// The service's state changed, which the status bar's right group
    /// and the status poll follow.
    Status,
    /// The selection changed.
    Selection,
    /// The selection's files arrived, and the cached rows in `rows` are
    /// now the ones of their moment.
    SelectionRead {
        rows: Range<u64>,
    },
    /// The selected folder's contents were counted, which only the
    /// inspector shows.
    FolderCounted,
    /// The export started, wrote more rows, ended or was cancelled.
    Export,
    /// The fix started, moved to another step, ended or was cancelled,
    /// which the status bars and the fix buttons follow.
    Fix,
    /// The export to `path` failed and its partial file is deleted.
    ExportFailed {
        path: PathBuf,
        error: ExportError,
    },
    /// Settings applied new choices, which the menus, the accelerators and
    /// the tray icon follow.
    Settings,
    /// The Use Total Commander link's search answered: the path of the Total
    /// Commander it takes, none, or why the service could not search.
    TotalCommander(Result<Option<String>, DaemonFailure>),
    /// Settings > Include's count answered: the entries the index would
    /// keep, or why the service could not count them.
    IncludedCount(Result<u64, DaemonFailure>),
    /// gozlerd answered a panel's folder sizes.
    FolderSizes(Side),
}

/// Why an export failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExportError {
    Write(WriteFailure),
    Daemon(DaemonFailure),
}

/// The export, which the status bar's middle group shows (DESIGN.md:
/// Status bar).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Export {
    Idle,
    Running {
        generation: Generation,
        path: PathBuf,
        rows: u64,
    },
    /// The last export's result, which stays until the next export starts.
    Done {
        path: PathBuf,
        rows: u64,
    },
}

/// A job that the status bar's middle group shows ([`Model::job`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Job {
    Export,
    Fix,
    /// A file operation ([`Model::operation`]).
    Operation,
}

/// The formats Export results… offers, in the save dialog's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportFormat {
    Csv,
    Tsv,
    Txt,
}

impl ExportFormat {
    pub(crate) const ALL: [ExportFormat; 3] =
        [ExportFormat::Csv, ExportFormat::Tsv, ExportFormat::Txt];

    /// The extension, which the save dialog shows as `*.csv`.
    pub(crate) fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Txt => "txt",
        }
    }

    /// The save dialog's name for the format.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "export.csv",
            ExportFormat::Tsv => "export.tsv",
            ExportFormat::Txt => "export.txt",
        }
    }

    /// The format a chosen file's name asks for, or `chosen`, the type the
    /// save dialog shows, when its extension is none of the formats.
    pub(crate) fn of(path: &Path, chosen: ExportFormat) -> ExportFormat {
        let path = path.to_string_lossy();
        let extension =
            gozler_core::query::extension(text::name_of(&path).as_bytes(), false).unwrap_or(b"");
        ExportFormat::ALL
            .into_iter()
            .find(|f| f.extension().as_bytes().eq_ignore_ascii_case(extension))
            .unwrap_or(chosen)
    }

    fn table_format(self) -> Format {
        match self {
            ExportFormat::Csv => Format::Csv,
            ExportFormat::Tsv => Format::Tsv,
            ExportFormat::Txt => Format::Txt,
        }
    }
}

/// The most `totalcmd*.exe` rows the Use Total Commander link reads,
/// newest first; a PC with more copies than this keeps its newest among
/// them.
const PROGRAM_ROWS: u64 = 1_000;

impl Accepted {
    fn only(change: Change) -> Accepted {
        Accepted { change, then: None }
    }
}

/// Whether the Location folder exists, which `app` asks before a search,
/// because the core makes no filesystem call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeCheck {
    Exists,
    Missing,
}

impl Model {
    pub(crate) fn new(strings: Strings, settings: Settings) -> Model {
        let window = &settings.window;
        let panels = Panels::new(&window.panels);
        let mut columns = vec![Column::Name];
        let saved: Vec<Column> = window.columns.iter().map(|c| c.column).collect();
        let shown = if saved.is_empty() {
            Column::ALL
                .into_iter()
                .filter(|c| c.shown_by_default())
                .collect()
        } else {
            saved
        };
        for column in shown {
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
        Model {
            strings,
            columns,
            settings,
            text: String::new(),
            search_due: false,
            open_on_answer: false,
            results: Results::new(),
            status: None,
            programs: Programs::UNKNOWN,
            selection: Selection::None,
            export: Export::Idle,
            fix: Fixer::Idle,
            operation: None,
            started_jobs: Vec::new(),
            failure: None,
            status_asked: None,
            program_asked: None,
            include_count_asked: None,
            settings_saved_asked: None,
            index_page_shown: false,
            newer_release: None,
            latest_release: LatestRelease::Unknown,
            last_generation: 0,
            file_operations: false,
            clipboard_files: false,
            taken: Arc::default(),
            search_kept: HashSet::new(),
            panels,
        }
    }

    /// Makes this window the one that offers the file operations.
    pub(crate) fn offer_file_operations(&mut self) {
        self.file_operations = true;
    }

    pub(crate) fn offers_file_operations(&self) -> bool {
        self.file_operations
    }

    /// Where the folder picker of Copy to folder… and Move to folder…
    /// opens.
    pub(crate) fn destination_folder(&self) -> Option<&str> {
        self.settings.window.destination_folder.as_deref()
    }

    pub(crate) fn set_destination_folder(&mut self, folder: String) {
        self.settings.window.destination_folder = Some(folder);
    }

    pub(crate) fn set_clipboard_holds_files(&mut self, holds: bool) {
        self.clipboard_files = holds;
    }

    pub(crate) fn clipboard_holds_files(&self) -> bool {
        self.clipboard_files
    }

    /// The row at `index` as the list shows it: `None` while its chunk has
    /// not arrived, or while a delete or a move has taken its entry.
    pub(crate) fn shown_row(&self, index: u64) -> Option<&ResultItem> {
        self.results
            .row(index)
            .filter(|row| !is_taken(&self.taken, &self.search_kept, row))
    }

    /// Whether a delete or a move has taken the entry of the row at `index`.
    pub(crate) fn row_taken(&self, index: u64) -> bool {
        self.results
            .row(index)
            .is_some_and(|row| is_taken(&self.taken, &self.search_kept, row))
    }

    /// The operations `ended`, which ended since the window last told the
    /// lists, and the taken entries `taken` that the lists show next: the
    /// search list keeps an ended delete or move that takes one of its
    /// fetched rows, until its next search (fast-copy-delete-spec.md:
    /// Taken entries), and the panels decide for themselves. Returns the
    /// panels that read their folders again or ask for their sizes, as
    /// [`Panels::operations_ended`] does.
    pub(crate) fn operations_ended(
        &mut self,
        ended: &[(OperationId, Arc<Request>)],
        taken: &Taken,
    ) -> Vec<(Side, bool, bool)> {
        for (id, request) in ended {
            if matches!(**request, Request::Delete { .. } | Request::Move { .. })
                && (self.results.fetched()).any(|row| taken.takes(&wide_path(row), *id))
            {
                self.search_kept.insert(*id);
            }
        }
        // `taken` may be older than `ended`, so an operation that ended in
        // this call stays even when `taken` does not have it as ended yet.
        let in_queue: HashSet<OperationId> = (taken.ended())
            .chain(ended.iter().map(|(id, _)| *id))
            .collect();
        self.search_kept.retain(|id| in_queue.contains(id));
        self.panels.operations_ended(ended)
    }

    /// Whether a list still shows the rows of the ended operation `id`
    /// blank, so it stays in the queue.
    pub(crate) fn keeps(&self, id: OperationId) -> bool {
        self.search_kept.contains(&id) || self.panels.keep(id)
    }

    /// Takes the entries of `taken` out of the selection; the list shows
    /// their rows blank once it draws them again.
    pub(crate) fn set_taken(&mut self, taken: Arc<Taken>) -> Accepted {
        self.taken = taken;
        let selected = self.selection.files().map_or(0, <[_]>::len);
        let files = match std::mem::replace(&mut self.selection, Selection::None) {
            Selection::Files(files) => files,
            Selection::Folder { folder, contents } => {
                if !is_taken(&self.taken, &self.search_kept, &folder) {
                    self.selection = Selection::Folder { folder, contents };
                }
                Vec::new()
            }
            other => {
                self.selection = other;
                Vec::new()
            }
        };
        if !files.is_empty() {
            let kept: Vec<ResultItem> = files
                .into_iter()
                .filter(|file| !is_taken(&self.taken, &self.search_kept, file))
                .collect();
            if !kept.is_empty() {
                self.selection = Selection::Files(kept);
            }
        }
        let change = if self.selection.files().map_or(0, <[_]>::len) == selected {
            Change::Nothing
        } else {
            Change::Selection
        };
        Accepted { change, then: None }
    }

    pub(crate) fn strings(&self) -> &Strings {
        &self.strings
    }

    pub(crate) fn choices(&self) -> &Choices {
        &self.settings.choices
    }

    /// The latest status reply, and how the latest status or results
    /// request failed, which Settings > Index shows.
    pub(crate) fn service(&self) -> (Option<&DaemonStatus>, Option<&DaemonFailure>) {
        (self.status.as_ref(), self.failure.as_ref())
    }

    /// Settings applied `choices`. Showing every file for an empty search
    /// changes what the empty search shows, so it runs again.
    pub(crate) fn set_choices(&mut self, choices: Choices) -> Accepted {
        let search_again = choices.show_all_when_empty != self.choices().show_all_when_empty;
        self.settings.choices = choices;
        Accepted {
            change: Change::Settings,
            then: search_again.then_some(Then::SearchAgain),
        }
    }

    /// Replaces the language with `strings` and records `choice`, which the
    /// settings file saves. It starts no request, because the results do
    /// not depend on the language (DESIGN.md: Language, `T12`).
    pub(crate) fn set_language(&mut self, choice: LanguageChoice, strings: Strings) {
        self.settings.choices.language = choice;
        self.strings = strings;
    }

    /// Settings shows or hides its Index page; while it shows, the window
    /// asks for status every 2 seconds ([`Model::polls_status`]).
    pub(crate) fn show_index_page(&mut self, shown: bool) {
        self.index_page_shown = shown;
    }

    /// Whether the window hides to the notification area for the first
    /// time, which shows a notification once; from then on, it does not.
    pub(crate) fn first_hide(&mut self) -> bool {
        !std::mem::replace(&mut self.settings.window.tray_notice_shown, true)
    }

    /// Shows the one-time notices again, as if Gozler had just been
    /// installed, which Settings' Reset all asks for.
    pub(crate) fn show_notices_again(&mut self) {
        self.settings.window.tray_notice_shown = false;
    }

    /// Whether Check for updates asks GitHub now, at `now` in seconds
    /// since 1970: it is on, and the last check was a day ago or more, or
    /// in the future of a clock that was set back. A check that is due
    /// counts as done.
    pub(crate) fn update_check_due(&mut self, now: i64) -> bool {
        let window = &mut self.settings.window;
        let due = self.settings.choices.check_for_updates
            && window.last_update_check.is_none_or(|last| {
                !(last..last.saturating_add(update::CHECK_INTERVAL_SECONDS)).contains(&now)
            });
        if due {
            window.last_update_check = Some(now);
        }
        due
    }

    /// Check for updates found `version`, which the status bar's right
    /// group names.
    pub(crate) fn set_newer_release(&mut self, version: String) -> Accepted {
        self.newer_release = Some(version);
        Accepted::only(Change::Status)
    }

    pub(crate) fn newer_release(&self) -> Option<&str> {
        self.newer_release.as_deref()
    }

    /// GitHub's latest release is or is not older than the window, which
    /// the service's fix button says.
    pub(crate) fn set_latest_release_older(&mut self, older: bool) -> Accepted {
        self.latest_release = LatestRelease::Known { older };
        Accepted::only(Change::Fix)
    }

    /// Asks once whether GitHub's latest release is older than the window,
    /// when the service is first found older than the window, so its fix
    /// reads Downgrade before the person clicks it. With Check for updates
    /// off, Gozler asks GitHub only after a click, so the button reads
    /// Update and the question after the click says which it is.
    fn ask_latest_release(&mut self, failure: &DaemonFailure) -> Option<Then> {
        let ask = matches!(failure, DaemonFailure::TooOld { .. })
            && self.settings.choices.check_for_updates
            && self.latest_release == LatestRelease::Unknown;
        ask.then(|| {
            self.latest_release = LatestRelease::Asking;
            Then::AskLatestRelease
        })
    }

    /// The catalogue key of `button`'s caption: Update reads Downgrade
    /// while GitHub's latest release is older than the window.
    pub(crate) fn button_label(&self, button: &ListButton) -> &'static str {
        match button {
            ListButton::Fix(Fix::Update)
                if self.latest_release == (LatestRelease::Known { older: true }) =>
            {
                Fix::DOWNGRADE_LABEL
            }
            _ => button.label(),
        }
    }

    /// The Use Total Commander link's search, which cancels an earlier
    /// one: every `totalcmd*.exe`, newest first (DESIGN.md: The Context
    /// menu page).
    pub(crate) fn find_total_commander(&mut self) -> Fetch {
        let search = query::search(
            TOTAL_COMMANDER_SEARCH,
            Matching::default(),
            &Filters::default(),
            &QueryStart {
                now: jiff::Timestamp::UNIX_EPOCH,
                time_zone: jiff::tz::TimeZone::UTC,
            },
        )
        .expect("the Total Commander search parses");
        let generation = self.generation();
        self.program_asked = Some(generation);
        Fetch {
            generation,
            ask: Ask::Program(ChunkAsk {
                search,
                sort: SortSpec {
                    key: SortKey::DateModified,
                    dir: SortDir::Desc,
                },
                start: 0,
                len: PROGRAM_ROWS,
            }),
        }
    }

    /// Settings closed, so the Use Total Commander link's search has nobody to
    /// answer; the window ends its request (rule 4).
    pub(crate) fn forget_total_commander(&mut self) -> Accepted {
        Accepted {
            change: Change::Nothing,
            then: self
                .program_asked
                .take()
                .map(|_| Then::Cancel(Purpose::Program)),
        }
    }

    /// Settings > Include's preview, which cancels an earlier one: how
    /// many entries the index keeps under `settings` (DESIGN.md: The
    /// Include page).
    pub(crate) fn count_included(&mut self, settings: IncludeSettings) -> Fetch {
        let generation = self.generation();
        self.include_count_asked = Some(generation);
        Fetch {
            generation,
            ask: Ask::CountIncluded(settings),
        }
    }

    /// An administrator saved the Include settings: the service reads and
    /// applies them.
    pub(crate) fn settings_saved(&mut self) -> Fetch {
        let generation = self.generation();
        self.settings_saved_asked = Some(generation);
        Fetch {
            generation,
            ask: Ask::SettingsSaved,
        }
    }

    pub(crate) fn columns(&self) -> &[Column] {
        &self.columns
    }

    pub(crate) fn mode(&self) -> Mode {
        self.settings.window.mode
    }

    pub(crate) fn panes(&self) -> Panes {
        let window = &self.settings.window;
        Panes {
            filters: window.filter_pane,
            inspector: window.inspector,
            inspector_height: window.inspector_height,
        }
    }

    pub(crate) fn filters(&self) -> &Filters {
        &self.settings.window.filters
    }

    /// The folder the filters limit the search to.
    pub(crate) fn location(&self) -> Option<&str> {
        self.filters().location.as_deref()
    }

    /// The volumes of the latest status reply.
    fn volumes(&self) -> &[VolumeStatus] {
        self.status.as_ref().map_or(&[], |s| &s.volumes)
    }

    /// The Location list's items.
    pub(crate) fn location_items(&self) -> Vec<LocationItem<'_>> {
        query::location_items(self.filters(), self.volumes())
    }

    /// The buttons below the list's message: the failure's fix, or the ways
    /// out of a search that the filters leave without results (DESIGN.md: S5,
    /// S8).
    pub(crate) fn list_buttons(&self) -> Vec<ListButton> {
        let filters = self.filters();
        match self.results.shown() {
            Shown::Rows(_) if self.results.total() == 0 => [
                filters.is_set().then_some(ListButton::ClearFilters),
                filters
                    .location
                    .is_some()
                    .then_some(ListButton::SearchEverywhere),
            ]
            .into_iter()
            .flatten()
            .collect(),
            Shown::Message(ListMessage::MissingFolder(_)) => {
                vec![ListButton::ChooseFolder, ListButton::SearchEverywhere]
            }
            Shown::Message(message) => message.fixes().into_iter().map(ListButton::Fix).collect(),
            Shown::Nothing | Shown::Rows(_) => Vec::new(),
        }
    }

    /// Whether `button` can be clicked: a repair waits while a fix runs
    /// (DESIGN.md: Service fixes).
    pub(crate) fn list_button_enabled(&self, button: &ListButton) -> bool {
        !(self.fix.running()
            && matches!(
                button,
                ListButton::Fix(Fix::Repair(_) | Fix::UseLocal(_) | Fix::Update)
            ))
    }

    pub(crate) fn results(&self) -> &Results {
        &self.results
    }

    pub(crate) fn matching(&self) -> Matching {
        self.settings.window.matching
    }

    pub(crate) fn sort(&self) -> SortSpec {
        self.settings.window.sort
    }

    /// The sort that View > Sort by shows: Search's, or the active panel's.
    pub(crate) fn shown_sort(&self) -> SortSpec {
        match self.mode() {
            Mode::Search => self.sort(),
            Mode::TwoPanels => self.panels.panel(self.panels.active()).sort(),
        }
    }

    pub(crate) fn panels(&self) -> &Panels {
        &self.panels
    }

    pub(crate) fn panels_mut(&mut self) -> &mut Panels {
        &mut self.panels
    }

    /// The request for `side`'s folder sizes, when it shows a folder with
    /// subfolders (file-manager-spec.md: Folder sizes).
    pub(crate) fn ask_sizes(&mut self, side: Side) -> Option<Fetch> {
        let generation = self.generation();
        let ask = self.panels.ask_sizes(side, generation)?;
        Some(Fetch { generation, ask })
    }

    /// What the latest status says about the volume of the active panel's
    /// folder: none when no volume holds it, so gozlerd does not index it.
    pub(crate) fn panel_coverage(&self) -> Option<Coverage<'_>> {
        self.status.as_ref()?;
        let panel = self.panels.panel(self.panels.active());
        (*panel.view() == View::Folder)
            .then(|| Coverage::of(self.volumes(), Some(panel.folder()), true))
    }

    pub(crate) fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The saved window placement, which the window opens at.
    pub(crate) fn placement(&self) -> Option<Placement> {
        self.settings.window.placement
    }

    fn generation(&mut self) -> Generation {
        self.last_generation += 1;
        Generation(self.last_generation)
    }

    /// The person typed `text`; its search runs once they pause, or with
    /// search as you type off, when they press `Enter` (DESIGN.md: C6).
    pub(crate) fn set_typed_search(&mut self, text: String) -> Accepted {
        if text == self.text {
            return Accepted::only(Change::Nothing);
        }
        self.text = text;
        // An open that waits is for the text before this one.
        self.open_on_answer = false;
        self.search_due = self.choices().search_as_you_type;
        Accepted {
            change: Change::Typed,
            then: self.search_due.then_some(Then::SearchSoon),
        }
    }

    /// What `Enter` in the search box does (DESIGN.md: window rule 8). With
    /// search as you type off, the search runs. On, the selected row or the
    /// first opens; while the search for the text has not answered, it runs
    /// at once if it still waits for the pause, and its first row opens
    /// when it answers.
    pub(crate) fn enter_in_search_box(&mut self) -> SearchBoxEnter {
        if !self.choices().search_as_you_type {
            return SearchBoxEnter::Search;
        }
        if self.search_due || self.results.searching() {
            self.open_on_answer = true;
            return if self.search_due {
                SearchBoxEnter::Search
            } else {
                SearchBoxEnter::Wait
            };
        }
        SearchBoxEnter::Open
    }

    /// The person changed a match control; the search runs again.
    pub(crate) fn set_matching(&mut self, matching: Matching) -> Accepted {
        self.settings.window.matching = matching;
        Accepted {
            change: Change::Choice,
            then: Some(Then::SearchAgain),
        }
    }

    /// The person chose a sort; the search runs again (DESIGN.md: C20).
    pub(crate) fn sort_by(&mut self, sort: SortSpec) -> Accepted {
        self.settings.window.sort = sort;
        Accepted {
            change: Change::Choice,
            then: Some(Then::SearchAgain),
        }
    }

    /// The person changed the filters; the search runs again. Only directly
    /// in this folder needs a folder, so Everywhere clears it.
    pub(crate) fn set_filters(&mut self, mut filters: Filters) -> Accepted {
        if filters.location.is_none() {
            filters.only_directly = false;
        }
        if filters == self.settings.window.filters {
            return Accepted::only(Change::Nothing);
        }
        self.settings.window.filters = filters;
        Accepted {
            change: Change::Filters,
            then: Some(Then::SearchAgain),
        }
    }

    /// Every filter group at its first choice (DESIGN.md: C21).
    pub(crate) fn clear_filters(&mut self) -> Accepted {
        self.set_filters(self.filters().cleared())
    }

    /// Location Everywhere, from a button below the list's message.
    pub(crate) fn search_everywhere(&mut self) -> Accepted {
        self.set_filters(Filters {
            location: None,
            ..self.filters().clone()
        })
    }

    /// Location `folder`, which also becomes one of the Location list's
    /// chosen folders.
    pub(crate) fn choose_folder(&mut self, folder: String) -> Accepted {
        let mut filters = self.filters().clone();
        filters.choose_folder(folder);
        self.set_filters(filters)
    }

    /// Location the one selected folder, or the folder that holds the one
    /// selected file (DESIGN.md: C12).
    pub(crate) fn search_in_this_folder(&mut self) -> Accepted {
        let folder = match self.selected_files() {
            Some([entry]) if entry.is_dir => entry.path.clone(),
            Some([entry]) => parent_of(&entry.path),
            _ => return Accepted::only(Change::Nothing),
        };
        self.choose_folder(folder)
    }

    /// Shows `mode`. Each mode keeps its state while the other shows
    /// (file-manager-spec.md: The window).
    pub(crate) fn show_mode(&mut self, mode: Mode) -> Accepted {
        let window = &mut self.settings.window;
        if window.mode == mode {
            return Accepted::only(Change::Nothing);
        }
        window.mode = mode;
        Accepted::only(Change::Mode)
    }

    /// Shows or hides the filter pane (DESIGN.md: C13).
    pub(crate) fn toggle_filter_pane(&mut self) -> Accepted {
        let window = &mut self.settings.window;
        window.filter_pane = !window.filter_pane;
        Accepted::only(Change::Panes)
    }

    /// Shows or hides the inspector (DESIGN.md: C14).
    pub(crate) fn toggle_inspector(&mut self) -> Accepted {
        let window = &mut self.settings.window;
        window.inspector = !window.inspector;
        Accepted::only(Change::Panes)
    }

    /// The person dragged the splitter to `height`, which the layout
    /// clamped.
    pub(crate) fn set_inspector_height(&mut self, height: Dip) -> Accepted {
        let window = &mut self.settings.window;
        if window.inspector_height == height {
            return Accepted::only(Change::Nothing);
        }
        window.inspector_height = height;
        Accepted::only(Change::Panes)
    }

    /// Shows or hides `column` (DESIGN.md: C15). A shown column joins at the
    /// end; Name cannot be hidden.
    pub(crate) fn toggle_column(&mut self, column: Column) -> Accepted {
        if !column.can_hide() {
            return Accepted::only(Change::Nothing);
        }
        Accepted::only(match self.columns.iter().position(|c| *c == column) {
            Some(index) => {
                self.columns.remove(index);
                Change::ColumnHidden { index }
            }
            None => {
                self.columns.push(column);
                Change::ColumnShown {
                    index: self.columns.len() - 1,
                }
            }
        })
    }

    /// The search that the typed text and the controls ask for, or the
    /// message that says why it cannot run, which starts no request: the
    /// text does not parse, `scope` says the Location folder is missing, or
    /// the search is empty and the settings say to show nothing (DESIGN.md:
    /// S6). The shown rows and the selection stay until the new search's first
    /// rows arrive.
    pub(crate) fn start_search(&mut self, query_start: &QueryStart, scope: ScopeCheck) -> Accepted {
        self.search_due = false;
        let search = query::search(&self.text, self.matching(), self.filters(), query_start);
        let missing = self
            .location()
            .filter(|_| scope == ScopeCheck::Missing)
            .map(str::to_owned);
        let shows_nothing = !self.choices().show_all_when_empty
            && self.text.trim().is_empty()
            && !self.filters().is_set();
        let message = match (search, missing) {
            (Err(error), _) => ListMessage::Parse(error),
            (Ok(_), Some(folder)) => ListMessage::MissingFolder(folder),
            (Ok(_), None) if shows_nothing => ListMessage::TypeToSearch,
            (Ok(search), None) => {
                let generation = self.generation();
                let ask = self.results.start_search(generation, search, self.sort());
                return Accepted {
                    change: Change::Searching,
                    then: Some(Then::Fetch(Fetch {
                        generation,
                        ask: Ask::Rows(ask),
                    })),
                };
            }
        };
        self.results.show_message(message);
        self.selection = Selection::None;
        self.open_on_answer = false;
        Accepted::only(Change::Message)
    }

    /// The chunk that the visible rows still lack, if no request for rows
    /// is in flight.
    pub(crate) fn rows_needed(&mut self, visible: Range<u64>) -> Option<Fetch> {
        let generation = self.generation();
        let ask = self.results.next_chunk(visible, generation)?;
        Some(Fetch {
            generation,
            ask: Ask::Rows(ask),
        })
    }

    /// The status request, unless one is in flight.
    pub(crate) fn status_needed(&mut self) -> Option<Fetch> {
        if self.status_asked.is_some() {
            return None;
        }
        let generation = self.generation();
        self.status_asked = Some(generation);
        Some(Fetch {
            generation,
            ask: Ask::Status,
        })
    }

    /// The positions the list says are selected, sorted and apart, read once
    /// per user action. When the cache holds every one, the selection is
    /// those rows, the files the person sees; otherwise it is read
    /// (DESIGN.md: The selection).
    pub(crate) fn set_selection(&mut self, positions: Vec<Range<u64>>) -> Accepted {
        let older = self.selection.asked();
        let positions: Vec<Range<u64>> = positions.into_iter().filter(|r| !r.is_empty()).collect();
        let then = if positions.is_empty() || self.results.shown_ask().is_none() {
            self.selection = Selection::None;
            None
        } else {
            match self.results.cached_rows(&positions) {
                Some(files) => self.select(files),
                None => {
                    self.selection = Selection::Reading {
                        positions,
                        asked: None,
                    };
                    Some(Then::ReadSelectionSoon)
                }
            }
        };
        // The request for the older selection reads other files (DESIGN.md: T6,
        // T7); a new request of the purpose ends it too.
        let then = then.or_else(|| older.map(|_| Then::Cancel(Purpose::Selection)));
        Accepted {
            change: Change::Selection,
            then,
        }
    }

    /// Selects `files`, which are read; one folder alone also asks for its
    /// contents, which the inspector shows (DESIGN.md: Inspector pane).
    fn select(&mut self, mut files: Vec<ResultItem>) -> Option<Then> {
        files.retain(|file| !is_taken(&self.taken, &self.search_kept, file));
        if files.is_empty() {
            self.selection = Selection::None;
            return None;
        }
        let folder = match files.as_slice() {
            [entry] if entry.is_dir => files.pop().expect("the selection is one folder"),
            _ => {
                self.selection = Selection::Files(files);
                return None;
            }
        };
        let generation = self.generation();
        let search = Search {
            query: ParsedQuery {
                clauses: Vec::new(),
                match_case: false,
            },
            scope: Some(folder.path.clone()),
        };
        self.selection = Selection::Folder {
            folder,
            contents: FolderContents::Calculating(generation),
        };
        Some(Then::Fetch(Fetch {
            generation,
            ask: Ask::FolderSize(search),
        }))
    }

    /// The request for the selection's rows, if it waits for one.
    pub(crate) fn read_selection(&mut self) -> Option<Fetch> {
        let Selection::Reading {
            positions,
            asked: None,
        } = &self.selection
        else {
            return None;
        };
        let ask = self.results.selection_ask(positions.clone())?;
        let generation = self.generation();
        if let Selection::Reading { asked, .. } = &mut self.selection {
            *asked = Some(generation);
        }
        Some(Fetch {
            generation,
            ask: Ask::Selection(ask),
        })
    }

    /// The selected files, once they are read.
    pub(crate) fn selected_files(&self) -> Option<&[ResultItem]> {
        self.selection.files()
    }

    /// Whether Export results… can run: the list shows results and no
    /// export runs (DESIGN.md: C4).
    pub(crate) fn can_export(&self) -> bool {
        self.results.shown_ask().is_some() && self.results.total() > 0 && !self.exporting()
    }

    pub(crate) fn exporting(&self) -> bool {
        matches!(self.export, Export::Running { .. })
    }

    pub(crate) fn export(&self) -> &Export {
        &self.export
    }

    /// Exports every match of the shown search to `path` (DESIGN.md: Export).
    /// The file holds the full path and each shown Size, Date modified,
    /// Extension and Attributes column in `display_order`, the list's order
    /// of [`Model::columns`], so it holds the bytes of `gozler -export-csv`
    /// with those switches.
    pub(crate) fn start_export(
        &mut self,
        path: PathBuf,
        format: ExportFormat,
        display_order: &[usize],
    ) -> Accepted {
        let Some(shown) = self.results.shown_ask().filter(|_| self.can_export()) else {
            return Accepted::only(Change::Nothing);
        };
        let (search, sort) = (shown.search.clone(), shown.sort);
        let columns: Vec<EsColumn> = display_order
            .iter()
            .filter_map(|&index| self.columns.get(index)?.export_column())
            .collect();
        let plan = EsPlan {
            format: format.table_format(),
            columns,
            export: Some(path.display().to_string()),
            ..EsPlan::default()
        };
        let generation = self.generation();
        self.export = Export::Running {
            generation,
            path: path.clone(),
            rows: 0,
        };
        self.job_started(Job::Export);
        Accepted {
            change: Change::Export,
            then: Some(Then::Fetch(Fetch {
                generation,
                ask: Ask::Export(ExportAsk {
                    search,
                    sort,
                    path,
                    want_attributes: plan.columns.contains(&EsColumn::Attributes),
                    format: plan.result_format(),
                }),
            })),
        }
    }

    /// Ends the running export; its thread deletes the partial file (DESIGN.md:
    /// C5).
    pub(crate) fn cancel_export(&mut self) -> Accepted {
        if !self.exporting() {
            return Accepted::only(Change::Nothing);
        }
        self.export = Export::Idle;
        Accepted {
            change: Change::Export,
            then: Some(Then::Cancel(Purpose::Export)),
        }
    }

    /// Whether the window must ask for status every 2 seconds: the service
    /// is not running, not ready or lost, a volume in scope is indexing, or
    /// Settings shows its Index page. A refused request, a failed owner
    /// check, an older service and an offline or failed volume end only by
    /// the person's action, so they start no poll.
    pub(crate) fn polls_status(&self) -> bool {
        self.failure.as_ref().is_some_and(DaemonFailure::recovers)
            || self.indexing_in_scope()
            || self.index_page_shown
            || self.fix.waits_for_service()
    }

    /// The job the status bar's middle group shows: the newest running
    /// one, or the one started last when none runs (DESIGN.md: Status bar).
    pub(crate) fn job(&self) -> Job {
        let runs = |job: &Job| match job {
            Job::Export => self.exporting(),
            Job::Fix => self.fix.running(),
            Job::Operation => matches!(self.operation, Some(StatusOperation::Running { .. })),
        };
        let mut started = self.started_jobs.iter().rev();
        (started.clone().find(|job| runs(job)))
            .or_else(|| started.next())
            .copied()
            .unwrap_or(Job::Export)
    }

    fn job_started(&mut self, job: Job) {
        self.started_jobs.retain(|started| *started != job);
        self.started_jobs.push(job);
    }

    /// The file operation the File operations window last reported for the
    /// middle group ([`StatusOperation`]).
    pub(crate) fn operation(&self) -> Option<&StatusOperation> {
        self.operation.as_ref()
    }

    /// The File operations window reported `operation`: another
    /// operation than the one shown is the newest job, also one that ended
    /// before the window saw it run.
    pub(crate) fn set_operation(&mut self, operation: Option<StatusOperation>) {
        let id = |operation: &Option<StatusOperation>| operation.as_ref().map(StatusOperation::id);
        if id(&operation).is_some() && id(&operation) != id(&self.operation) {
            self.job_started(Job::Operation);
        }
        self.operation = operation;
    }

    pub(crate) fn fix(&self) -> &Fixer {
        &self.fix
    }

    /// The fix the service's failure offers, which the list's message and
    /// the Index page's Service group show (DESIGN.md: Daemon failures).
    pub(crate) fn service_fix(&self) -> Option<Fix> {
        self.failure.as_ref().and_then(Fix::of)
    }

    /// The Use local fix, which the Service group shows before the
    /// failure's other fix, and alone while a service of a different build
    /// answers. The list's message shows it only for a failure.
    pub(crate) fn use_local_fix(&self) -> Option<Fix> {
        match (&self.failure, &self.programs.local) {
            (Some(failure), _) => Fix::use_local(failure),
            (None, Local::Offered(local)) => Some(Fix::UseLocal(local.clone())),
            (None, Local::NotOffered | Local::Mismatched) => None,
        }
    }

    /// The gozlerd.exe files that the latest status reply, or the failure
    /// of a service older than the window, found; `None` after any other
    /// failure, which reads no files.
    pub(crate) fn programs(&self) -> Option<&Programs> {
        match &self.failure {
            Some(DaemonFailure::TooOld { programs }) => Some(programs),
            Some(_) => None,
            None => Some(&self.programs),
        }
    }

    /// Starts `fix`, unless a fix runs (DESIGN.md: Service fixes). Open
    /// Services is the window's own, and starts nothing here.
    pub(crate) fn start_fix(&mut self, fix: &Fix) -> Accepted {
        let Some(work) = self.fix.start(fix) else {
            return Accepted::only(Change::Nothing);
        };
        self.job_started(Job::Fix);
        Accepted {
            change: Change::Fix,
            then: Some(Then::RunFix(work)),
        }
    }

    /// The person answered whether to download the setup the fix found;
    /// a fix they declined ends without a word in the status bar.
    pub(crate) fn answer_setup(&mut self, download: bool) -> Accepted {
        let setup = self.fix.answer(download);
        Accepted {
            change: Change::Fix,
            then: setup.map(|setup| Then::RunFix(FixWork::Download(setup))),
        }
    }

    /// The fix's thread reported `event`. Once the commands end well, the
    /// window asks the service for its status until it answers; once the
    /// downloaded setup is checked, the window starts it.
    pub(crate) fn accept_fix(&mut self, event: FixEvent) -> Accepted {
        if !self.fix.accept(event) {
            return Accepted::only(Change::Nothing);
        }
        let then = if let Some(setup) = self.fix.starting_setup() {
            Some(Then::RunSetup(setup.to_owned()))
        } else if self.fix.confirming().is_some() {
            Some(Then::ConfirmSetup)
        } else if self.fix.found_no_setup() {
            Some(Then::ExplainNoSetup)
        } else if self.fix.waits_for_service() {
            self.status_needed().map(Then::Fetch)
        } else {
            None
        };
        Accepted {
            change: Change::Fix,
            then,
        }
    }

    /// Stops the fix, in a step that can stop.
    pub(crate) fn cancel_fix(&mut self) -> Accepted {
        if !self.fix.cancel() {
            return Accepted::only(Change::Nothing);
        }
        Accepted {
            change: Change::Fix,
            then: Some(Then::StopFix),
        }
    }

    /// What the latest status says about the volumes the shown search
    /// reaches.
    pub(crate) fn coverage(&self) -> Coverage<'_> {
        let volumes = self.volumes();
        let scope = self
            .results
            .shown_search()
            .and_then(|search| search.scope.as_deref());
        Coverage::of(volumes, scope, self.reads_size_or_date())
    }

    fn reads_size_or_date(&self) -> bool {
        self.results
            .shown_search()
            .is_some_and(|search| search.query.tests_size_or_date())
    }

    /// Whether a volume the shown search reaches completes by itself once
    /// it is read.
    fn indexing_in_scope(&self) -> bool {
        self.coverage().volumes().any(|v| {
            matches!(
                v.phase,
                VolumePhase::Bootstrapping | VolumePhase::Rescanning
            )
        })
    }

    /// Applies a reply; a reply of an older generation changes nothing.
    pub(crate) fn accept(&mut self, fetched: Fetched) -> Accepted {
        let generation = fetched.generation;
        match fetched.purpose {
            Purpose::Results if self.results.awaits(generation) => {
                self.accept_results(generation, fetched.outcome)
            }
            Purpose::Status if self.status_asked == Some(generation) => {
                self.accept_status(fetched.outcome)
            }
            Purpose::Selection if self.selection.asked() == Some(generation) => {
                self.accept_selection(fetched.outcome)
            }
            Purpose::Export if matches!(self.export, Export::Running { generation: g, .. } if g == generation) => {
                self.accept_export(fetched.outcome)
            }
            Purpose::Program if self.program_asked == Some(generation) => {
                self.program_asked = None;
                Accepted::only(Change::TotalCommander(match fetched.outcome {
                    Outcome::Rows(chunk) => {
                        Ok(newest_total_commander(&chunk.rows).map(str::to_owned))
                    }
                    Outcome::Failed(failure) => Err(failure),
                    other => {
                        unreachable!("a program search's reply is rows or a failure: {other:?}")
                    }
                }))
            }
            Purpose::IncludeCount if self.include_count_asked == Some(generation) => {
                self.include_count_asked = None;
                Accepted::only(Change::IncludedCount(match fetched.outcome {
                    Outcome::IncludedCount(entries) => Ok(entries),
                    Outcome::Failed(failure) => Err(failure),
                    other => unreachable!("a count's reply is a count or a failure: {other:?}"),
                }))
            }
            Purpose::SettingsSaved if self.settings_saved_asked == Some(generation) => {
                self.settings_saved_asked = None;
                // A service that is not running reads the settings when it
                // starts, so a failure needs nothing. Once applied, the
                // drives' states change, which status shows.
                Accepted {
                    change: Change::Nothing,
                    then: self.status_needed().map(Then::Fetch),
                }
            }
            Purpose::FolderSizes(side) => {
                if self.panels.accept_sizes(side, generation, fetched.outcome) {
                    Accepted::only(Change::FolderSizes(side))
                } else {
                    Accepted::only(Change::Nothing)
                }
            }
            Purpose::Results
            | Purpose::Status
            | Purpose::Selection
            | Purpose::Export
            | Purpose::Program
            | Purpose::IncludeCount
            | Purpose::SettingsSaved => Accepted::only(Change::Nothing),
        }
    }

    fn accept_results(&mut self, generation: Generation, outcome: Outcome) -> Accepted {
        match outcome {
            Outcome::Rows(chunk) => {
                self.failure = None;
                let flags = (chunk.volumes_incomplete, chunk.metadata_pending);
                let change = match self.results.accept(generation, chunk) {
                    RowsChange::Stale => Change::Nothing,
                    RowsChange::NewSearch { count } => {
                        // A new search's rows are other files (DESIGN.md: T2),
                        // and the disk as it is now.
                        self.selection = Selection::None;
                        self.search_kept.clear();
                        Change::NewSearch {
                            count,
                            open_first: std::mem::take(&mut self.open_on_answer) && count > 0,
                        }
                    }
                    RowsChange::Rows { rows, count } => Change::Rows { rows, count },
                };
                // A page's flags cover every volume in scope, and the status
                // names which. The scope is the shown search's, so it is read
                // after the rows are accepted.
                let coverage = self.coverage();
                let unexplained = flags.0 && coverage.incomplete().is_none()
                    || leaves_matches_out(flags.1, self.reads_size_or_date())
                        && coverage.sizes_or_dates_unknown().is_none();
                let then = if unexplained {
                    self.status_needed().map(Then::Fetch)
                } else {
                    None
                };
                Accepted { change, then }
            }
            Outcome::Failed(failure) => self.show_failure(failure),
            other => unreachable!("a results request's reply is rows or a failure: {other:?}"),
        }
    }

    /// The service failed while rows are shown: the list shows the failure
    /// instead of the rows, nothing is selected, and the status poll runs
    /// the search again once a failure that ends by itself has ended
    /// (DESIGN.md: T3, T4).
    fn show_failure(&mut self, failure: DaemonFailure) -> Accepted {
        let then = self.ask_latest_release(&failure);
        self.failure = Some(failure.clone());
        self.results.show_message(ListMessage::Failure(failure));
        self.selection = Selection::None;
        self.open_on_answer = false;
        Accepted {
            change: Change::Message,
            then,
        }
    }

    fn accept_status(&mut self, outcome: Outcome) -> Accepted {
        self.status_asked = None;
        match outcome {
            Outcome::Status(status, programs) => {
                let answers_again = self.failure.as_ref().is_some_and(DaemonFailure::recovers);
                let was_indexing = self.indexing_in_scope();
                self.failure = None;
                self.status = Some(status);
                self.programs = programs;
                // The fix's end shows with this status.
                self.fix.service_answered();
                let completed = was_indexing && !self.indexing_in_scope();
                let search_again =
                    answers_again || completed && self.results.shown_search().is_some();
                Accepted {
                    change: Change::Status,
                    then: search_again.then_some(Then::SearchAgain),
                }
            }
            Outcome::Failed(failure) => {
                // A failure the list shows follows the service, so its
                // message and button never offer the fix for a state the
                // service has left. A search in flight shows its own reply.
                let shows_another = matches!(
                    self.results.shown(),
                    Shown::Message(ListMessage::Failure(shown)) if *shown != failure
                );
                let change = if shows_another && !self.results.searching() {
                    self.results
                        .show_message(ListMessage::Failure(failure.clone()));
                    Change::Message
                } else {
                    Change::Status
                };
                let then = self.ask_latest_release(&failure);
                self.failure = Some(failure);
                Accepted { change, then }
            }
            other => unreachable!("a status request's reply is status or a failure: {other:?}"),
        }
    }

    fn accept_selection(&mut self, outcome: Outcome) -> Accepted {
        match (
            std::mem::replace(&mut self.selection, Selection::None),
            outcome,
        ) {
            (Selection::Reading { positions, .. }, Outcome::Selection(reply)) => {
                // Each row moves to the files or to the cache, and only a row
                // that is both is copied, so a selection of 100,000 rows is
                // held once (DESIGN.md: Memory budget).
                let selected = positions.iter().map(|r| r.end - r.start).sum::<u64>();
                let mut files = Vec::with_capacity(usize::try_from(selected).unwrap_or(0));
                let mut cached = Vec::new();
                for (position, row) in reply.rows {
                    match (
                        inside(&positions, position),
                        inside(&reply.cached, position),
                    ) {
                        (true, true) => {
                            files.push(row.clone());
                            cached.push((position, row));
                        }
                        (true, false) => files.push(row),
                        (false, _) => cached.push((position, row)),
                    }
                }
                let rows = self.results.replace_cached(&reply.cached, cached);
                let then = if files.is_empty() {
                    None
                } else {
                    self.select(files)
                };
                Accepted {
                    change: Change::SelectionRead { rows },
                    then,
                }
            }
            (Selection::Folder { folder, .. }, Outcome::Totals(totals)) => {
                self.selection = Selection::Folder {
                    folder,
                    contents: FolderContents::Counted(totals),
                };
                Accepted::only(Change::FolderCounted)
            }
            (_, Outcome::Failed(failure)) if !self.results.searching() => {
                self.show_failure(failure)
            }
            // A search in flight shows its own reply, which clears the
            // list's selection.
            (_, Outcome::Failed(_)) => Accepted::only(Change::Selection),
            (selection, other) => unreachable!(
                "a selection request's reply is its rows, its folder's totals or a failure: \
                 {selection:?}, {other:?}"
            ),
        }
    }

    fn accept_export(&mut self, outcome: Outcome) -> Accepted {
        let Export::Running { path, rows, .. } = &mut self.export else {
            unreachable!("accept checked that an export runs")
        };
        let failed = |path: &PathBuf, error| Change::ExportFailed {
            path: path.clone(),
            error,
        };
        let change = match outcome {
            Outcome::ExportProgress { rows: written } => {
                *rows = written;
                return Accepted::only(Change::Export);
            }
            Outcome::Exported { rows } => {
                let path = std::mem::take(path);
                self.export = Export::Done { path, rows };
                return Accepted::only(Change::Export);
            }
            Outcome::ExportFailed(failure) => failed(path, ExportError::Write(failure)),
            Outcome::Failed(failure) => failed(path, ExportError::Daemon(failure)),
            other => unreachable!("an export's reply is progress, its end or a failure: {other:?}"),
        };
        self.export = Export::Idle;
        Accepted::only(change)
    }

    /// The list's display order of [`Model::columns`], as indexes into it:
    /// the saved columns in their saved order, which the person sets by
    /// dragging a header, then any other shown column.
    pub(crate) fn column_order(&self) -> Vec<usize> {
        let saved = &self.settings.window.columns;
        let mut order: Vec<usize> = Vec::new();
        for index in saved
            .iter()
            .filter_map(|s| self.columns.iter().position(|c| *c == s.column))
            .chain(0..self.columns.len())
        {
            if !order.contains(&index) {
                order.push(index);
            }
        }
        order
    }

    /// The sort a click on `column`'s header asks for: the column's key,
    /// reversed on a second click, and largest or newest first on the first
    /// click on Size or Date modified, as gozler's sort keys do.
    pub(crate) fn header_sort(&self, column: Column) -> Option<SortSpec> {
        let key = column.sort_key()?;
        let sort = self.sort();
        Some(if sort.key == key {
            SortSpec {
                key,
                dir: match sort.dir {
                    SortDir::Asc => SortDir::Desc,
                    SortDir::Desc => SortDir::Asc,
                },
            }
        } else {
            SortSpec::default_for(key)
        })
    }

    /// The sort View > Sort by's `key` asks for: the shown direction for the
    /// shown key, else the key's own first direction.
    pub(crate) fn key_sort(&self, key: SortKey) -> SortSpec {
        let sort = self.sort();
        if sort.key == key {
            sort
        } else {
            SortSpec::default_for(key)
        }
    }

    /// Saves the settings with the window's `placement` and `columns`, when
    /// Windows gave them.
    pub(crate) fn save_settings(
        &mut self,
        path: &Path,
        placement: Option<Placement>,
        columns: Option<Vec<SavedColumn>>,
    ) -> io::Result<()> {
        let window = &mut self.settings.window;
        window.panels = self.panels.saved();
        if placement.is_some() {
            window.placement = placement;
        }
        if let Some(columns) = columns {
            window.columns = columns;
        }
        self.settings.save(path)
    }
}

/// A column of the results list (DESIGN.md: Results list). Its names are
/// `ui.json`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Column {
    Name,
    Path,
    Size,
    DateModified,
    Extension,
    Attributes,
}

impl Column {
    /// In View > Columns' order.
    pub(crate) const ALL: [Column; 6] = [
        Column::Name,
        Column::Path,
        Column::Size,
        Column::DateModified,
        Column::Extension,
        Column::Attributes,
    ];

    pub(crate) fn header(self) -> &'static str {
        match self {
            Column::Name => "header.name",
            Column::Path => "header.path",
            Column::Size => "header.size",
            Column::DateModified => "header.date_modified",
            Column::Extension => "header.extension",
            Column::Attributes => "header.attributes",
        }
    }

    /// Its item in View > Columns.
    pub(crate) fn menu_label(self) -> &'static str {
        match self {
            Column::Name => "column.name",
            Column::Path => "column.path",
            Column::Size => "column.size",
            Column::DateModified => "column.date_modified",
            Column::Extension => "column.extension",
            Column::Attributes => "column.attributes",
        }
    }

    pub(crate) fn default_width(self) -> Dip {
        Dip(match self {
            Column::Name => 320,
            Column::Path => 380,
            Column::Size => 90,
            Column::DateModified => 130,
            Column::Extension | Column::Attributes => 80,
        })
    }

    fn shown_by_default(self) -> bool {
        !matches!(self, Column::Extension | Column::Attributes)
    }

    /// Name cannot be hidden, because the list would show rows that name
    /// nothing (DESIGN.md: C15).
    pub(crate) fn can_hide(self) -> bool {
        self != Column::Name
    }

    /// Sizes are right-aligned, so their digits line up.
    pub(crate) fn right_aligned(self) -> bool {
        self == Column::Size
    }

    /// What a click on its header sorts by; Attributes sorts by nothing,
    /// because gozler has no attribute sort.
    pub(crate) fn sort_key(self) -> Option<SortKey> {
        match self {
            Column::Name => Some(SortKey::Name),
            Column::Path => Some(SortKey::Path),
            Column::Size => Some(SortKey::Size),
            Column::DateModified => Some(SortKey::DateModified),
            Column::Extension => Some(SortKey::Extension),
            Column::Attributes => None,
        }
    }

    /// Its column in an export; Name and Path are parts of the full path,
    /// which every export holds (DESIGN.md: Export).
    fn export_column(self) -> Option<EsColumn> {
        match self {
            Column::Name | Column::Path => None,
            Column::Size => Some(EsColumn::Size),
            Column::DateModified => Some(EsColumn::DateModified),
            Column::Extension => Some(EsColumn::Extension),
            Column::Attributes => Some(EsColumn::Attributes),
        }
    }
}

/// Whether a delete or a move has taken `row`'s entry, by its exact name
/// when its UTF-8 path lost an unpaired surrogate. The window asks for every
/// cell it paints and every selected file, so an empty set answers before
/// the path is encoded.
fn is_taken(taken: &Taken, kept: &HashSet<OperationId>, row: &ResultItem) -> bool {
    !taken.is_empty() && taken.contains(&wide_path(row), |id| kept.contains(&id))
}

#[cfg(test)]
#[allow(
    clippy::single_range_in_vec_init,
    reason = "a selection is a list of row ranges"
)]
pub(crate) mod tests {
    use super::*;
    use crate::core::captions::{self, list_message};
    use crate::core::fetch::{SelectionRows, WriteFailure};
    use crate::core::fixer::CommandEvent;
    use crate::core::results::CHUNK_ROWS;
    use crate::core::results::tests::chunk;
    use crate::core::text::tests::{English, english};
    use gozler_client::NotRunning;
    use gozler_client::Repair;
    use gozler_core::escompat::{Command, parse_argv};
    use gozler_core::proto::VolumeStatus;

    /// The search list keeps an ended delete only when, at its end, the
    /// delete takes one of the list's fetched rows, and then until its next
    /// search; a delete elsewhere would otherwise stay in the queue until
    /// the next search (fast-copy-delete-spec.md: Taken entries). The
    /// snapshot of the taken entries may be older than the end, so it
    /// still has the delete as running.
    #[test]
    fn the_search_list_keeps_only_a_delete_of_its_rows() {
        let mut model = model();
        let fetch = search(&mut model, "row");
        model.accept(rows(&fetch, 3));
        let (ours, other) = (OperationId::for_tests(1), OperationId::for_tests(2));
        let mut taken = Taken::default();
        taken.insert(ours, std::path::Path::new(r"C:\row1"), false);
        taken.insert(other, std::path::Path::new(r"D:\elsewhere"), true);
        let delete = |path: &str| {
            Arc::new(Request::Delete {
                sources: vec![path.into()],
            })
        };
        model.operations_ended(
            &[(ours, delete(r"C:\row1")), (other, delete(r"D:\elsewhere"))],
            &taken,
        );
        assert!(model.keeps(ours));
        assert!(!model.keeps(other));
        let fetch = search(&mut model, "rows");
        model.accept(rows(&fetch, 3));
        assert!(!model.keeps(ours));
    }

    /// The list's message and its buttons' captions.
    fn message(model: &Model) -> Option<(String, Vec<String>)> {
        list_message(model).map(|message| {
            let buttons = message.buttons.into_iter();
            (message.text, buttons.map(|button| button.label).collect())
        })
    }

    pub(crate) fn model() -> Model {
        Model::new(english(), Settings::default())
    }

    /// Types `text` and starts its search; returns the first chunk's fetch.
    pub(crate) fn search(model: &mut Model, text: &str) -> Fetch {
        model.set_typed_search(text.to_owned());
        let start = QueryStart {
            now: "2026-03-04T12:00:00Z".parse().expect("an instant"),
            time_zone: jiff::tz::TimeZone::UTC,
        };
        match model.start_search(&start, ScopeCheck::Exists) {
            Accepted {
                change: Change::Searching,
                then: Some(Then::Fetch(fetch)),
            } => fetch,
            other => panic!("the search starts a request: {other:?}"),
        }
    }

    /// The reply to `fetch`: its first rows of a search with `total` matches.
    pub(crate) fn rows(fetch: &Fetch, total: u64) -> Fetched {
        let start = match &fetch.ask {
            Ask::Rows(ask) => ask.start,
            other => panic!("a rows fetch: {other:?}"),
        };
        Fetched {
            purpose: Purpose::Results,
            generation: fetch.generation,
            outcome: Outcome::Rows(chunk(start, CHUNK_ROWS.min(total - start), total)),
        }
    }

    fn failed(fetch: &Fetch, failure: DaemonFailure) -> Fetched {
        Fetched {
            purpose: fetch.purpose(),
            generation: fetch.generation,
            outcome: Outcome::Failed(failure),
        }
    }

    /// A status reply with one volume, C:\, in `phase`.
    fn status(fetch: &Fetch, phase: VolumePhase) -> Fetched {
        Fetched {
            purpose: Purpose::Status,
            generation: fetch.generation,
            outcome: Outcome::Status(
                DaemonStatus {
                    volumes: vec![VolumeStatus {
                        guid: r"\\?\Volume{c}\".into(),
                        mounts: vec![r"C:\".into()],
                        phase,
                        entries: 0,
                        generation: 0,
                        metadata_pending: false,
                        journal_max_bytes: None,
                        placeholders_created: 0,
                        delete_of_unknown: 0,
                        stale_slots: 0,
                        link_reconciles_dropped: 0,
                        memory: None,
                    }],
                    volumes_incomplete: false,
                    process_working_set: 0,
                    process_private_bytes: 0,
                    include: None,
                    left_out: Vec::new(),
                },
                Programs::UNKNOWN,
            ),
        }
    }

    /// A model that shows the first rows of a search with `total` matches.
    pub(crate) fn showing(total: u64) -> Model {
        let mut model = model();
        let fetch = search(&mut model, "report");
        model.accept(rows(&fetch, total));
        model
    }

    /// Emptying the list on every keystroke makes the rows flicker, and a
    /// reply to a search the person has typed past would show rows of text
    /// that is no longer in the box.
    #[test]
    fn t1_a_new_search_keeps_the_rows_and_drops_the_older_search() {
        let mut model = showing(10);
        let case = Matching {
            case: true,
            ..model.matching()
        };
        assert_eq!(model.set_matching(case).then, Some(Then::SearchAgain));
        let older = search(&mut model, "repo");
        assert_eq!(model.results.count(), 10, "the previous rows stay");
        assert_eq!(captions::results_status(&model, &English), "Searching…");
        let _newest = search(&mut model, "report f");
        assert_eq!(model.accept(rows(&older, 3)).change, Change::Nothing);
        assert_eq!(model.results.count(), 10);
    }

    #[test]
    fn t2_the_new_search_first_rows_replace_the_old_ones() {
        let mut model = showing(10);
        let fetch = search(&mut model, "repo");
        assert_eq!(
            model.accept(rows(&fetch, 3)).change,
            Change::NewSearch {
                count: 3,
                open_first: false
            }
        );
        assert_eq!(captions::results_status(&model, &English), "3 results");
    }

    #[test]
    fn t3_a_failure_while_rows_are_shown_removes_them() {
        let mut model = showing(50_000);
        let chunk = model
            .rows_needed(CHUNK_ROWS..CHUNK_ROWS + 40)
            .expect("the rows are not cached");
        assert_eq!(
            model.accept(failed(&chunk, DaemonFailure::Lost)).change,
            Change::Message
        );
        assert_eq!(model.results.count(), 0);
        let strings = english();
        assert_eq!(
            message(&model),
            Some((strings.text("failure.lost").to_owned(), vec![]))
        );
    }

    /// Without it, the person must search again by hand after the service
    /// starts or a volume finishes indexing (DESIGN.md: Requests).
    #[test]
    fn t4_the_search_runs_again_when_the_service_answers_or_the_index_completes() {
        let mut model = model();
        let fetch = search(&mut model, "report");
        model.accept(failed(
            &fetch,
            DaemonFailure::NotRunning(NotRunning::Stopped),
        ));
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Live)).then,
            Some(Then::SearchAgain)
        );

        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Bootstrapping)).then,
            None
        );
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Live)).then,
            Some(Then::SearchAgain)
        );
        assert!(!model.polls_status(), "nothing needs the poll any more");
    }

    #[test]
    fn t5_a_volume_that_starts_indexing_changes_part_4_and_keeps_the_rows() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        assert_eq!(captions::index_status(&model), "");
        assert!(!model.polls_status());

        let ask = model.status_needed().expect("no status request runs");
        let accepted = model.accept(status(&ask, VolumePhase::Rescanning));
        assert_eq!(accepted, Accepted::only(Change::Status));
        assert_eq!(model.results.count(), 10);
        // S13.
        assert_eq!(
            captions::index_status(&model),
            r"C:\ is being indexed. Results might be incomplete."
        );
        assert!(model.polls_status());
    }

    /// A results page carries only combined flags, so the status bar would
    /// never name a volume that became incomplete after the last status reply.
    #[test]
    fn a_page_flag_that_the_status_does_not_explain_asks_for_status() {
        let mut model = showing(50_000);
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        let fetch = model
            .rows_needed(CHUNK_ROWS..CHUNK_ROWS + 40)
            .expect("the rows are not cached");
        let mut reply = rows(&fetch, 50_000);
        if let Outcome::Rows(chunk) = &mut reply.outcome {
            chunk.volumes_incomplete = true;
        }
        assert!(matches!(
            model.accept(reply).then,
            Some(Then::Fetch(Fetch {
                ask: Ask::Status,
                ..
            }))
        ));
    }

    /// t1 shows the same for the results, t6 for the selection.
    #[test]
    fn a_status_reply_of_an_older_generation_changes_nothing() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        let older = Fetch {
            generation: Generation(ask.generation.0 - 1),
            ask: Ask::Status,
        };
        assert_eq!(
            model.accept(status(&older, VolumePhase::Rescanning)).change,
            Change::Nothing
        );
        assert_eq!(model.status, None);
    }

    /// A status failure that emptied the list would hide rows that are
    /// still right; a results failure must replace them (DESIGN.md: Requests).
    #[test]
    fn a_status_failure_leaves_the_rows_and_a_results_failure_replaces_them() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(failed(&ask, DaemonFailure::Lost)).change,
            Change::Status
        );
        assert_eq!(model.results.count(), 10);
        assert_eq!(message(&model), None);

        let fetch = search(&mut model, "repo");
        let refused = DaemonFailure::Refused {
            detail: "regex parse error".into(),
        };
        assert_eq!(
            model.accept(failed(&fetch, refused)).change,
            Change::Message
        );
        assert_eq!(model.results.count(), 0);
    }

    /// The list would otherwise keep offering Start the service for a
    /// service that has since been turned off, and that command fails.
    #[test]
    fn a_status_failure_with_another_reason_replaces_the_shown_failure() {
        let mut model = model();
        let fetch = search(&mut model, "report");
        model.accept(failed(
            &fetch,
            DaemonFailure::NotRunning(NotRunning::Stopped),
        ));
        let ask = model.status_needed().expect("no status request runs");
        let disabled = DaemonFailure::NotRunning(NotRunning::Disabled);
        assert_eq!(model.accept(failed(&ask, disabled)).change, Change::Message);
        let strings = english();
        assert_eq!(
            message(&model),
            Some((
                strings.text("failure.disabled").to_owned(),
                vec![strings.text("fix.turn_on_service").to_owned()]
            ))
        );
    }

    /// S7: the parser's message, and no request, so no poll either.
    #[test]
    fn a_search_that_does_not_parse_starts_no_request() {
        let mut model = model();
        model.set_typed_search("dz:x".into());
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        assert_eq!(
            model.start_search(&start, ScopeCheck::Exists),
            Accepted::only(Change::Message)
        );
        let strings = english();
        assert_eq!(
            message(&model),
            Some((strings.format("query.unknown_function", &["dz:"]), vec![]))
        );
        assert!(!model.polls_status());
    }

    /// S8: searching a folder that does not exist would say "No results"
    /// instead of naming the folder, and `gozler` refuses the same search.
    #[test]
    fn a_missing_location_folder_starts_no_request() {
        let mut model = showing(10);
        model.choose_folder(r"D:\Old".into());
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        assert_eq!(
            model.start_search(&start, ScopeCheck::Missing),
            Accepted::only(Change::Message)
        );
        let strings = english();
        assert_eq!(
            message(&model),
            Some((
                strings.format("list.missing_folder", &[r"D:\Old"]),
                vec![
                    strings.text("filter.choose_folder").to_owned(),
                    strings.text("list.search_everywhere").to_owned(),
                ]
            ))
        );
        assert!(!model.polls_status());
    }

    /// A failure that ends by itself is polled until the service answers; a
    /// poll after one that the same request repeats would ask forever.
    #[test]
    fn the_status_poll_runs_only_while_waiting_can_change_the_answer() {
        for (failure, polls) in [
            (DaemonFailure::NotRunning(NotRunning::Stopped), true),
            (DaemonFailure::NotReady, true),
            (DaemonFailure::Lost, true),
            (
                DaemonFailure::Refused {
                    detail: String::new(),
                },
                false,
            ),
        ] {
            let mut model = model();
            let fetch = search(&mut model, "report");
            model.accept(failed(&fetch, failure.clone()));
            assert_eq!(model.polls_status(), polls, "{failure:?}");
        }
        for (phase, polls) in [
            (VolumePhase::Bootstrapping, true),
            (VolumePhase::Rescanning, true),
            (VolumePhase::Offline, false),
        ] {
            let mut model = showing(10);
            let ask = model.status_needed().expect("no status request runs");
            model.accept(status(&ask, phase.clone()));
            assert_eq!(model.polls_status(), polls, "{phase:?}");
        }
    }

    fn paths(model: &Model) -> Vec<&str> {
        model
            .selected_files()
            .unwrap_or_default()
            .iter()
            .map(|f| f.path.as_str())
            .collect()
    }

    /// The cached rows are the files the person sees, so reading them again
    /// from the service could name other files (DESIGN.md: window rule 10).
    #[test]
    fn a_selection_of_cached_rows_is_built_from_the_cache() {
        let mut model = showing(50_000);
        let accepted = model.set_selection(vec![3..5, 9..10]);
        assert_eq!(accepted, Accepted::only(Change::Selection));
        assert_eq!(paths(&model), [r"C:\row3", r"C:\row4", r"C:\row9"]);
        assert_eq!(model.read_selection(), None);
    }

    /// The reply to the selection's one request: the rows it keeps, under
    /// the folder `folder`, so the test sees which rows came from it.
    fn selection_reply(fetch: &Fetch, folder: &str) -> Fetched {
        let Ask::Selection(ask) = &fetch.ask else {
            panic!("a selection fetch: {fetch:?}")
        };
        let span = ask.span.start..ask.span.start + ask.span.len;
        let rows = span
            .filter(|p| ask.keeps(*p))
            .map(|p| {
                let mut row = chunk(p, 1, 50_000).rows.remove(0);
                row.path = format!(r"{folder}\row{p}");
                (p, row)
            })
            .collect();
        Fetched {
            purpose: Purpose::Selection,
            generation: fetch.generation,
            outcome: Outcome::Selection(SelectionRows {
                rows,
                cached: ask.cached.clone(),
            }),
        }
    }

    /// S12: a selected row outside the cache is read in one request, whose
    /// rows also replace the cached ones inside its span, so the list shows
    /// the files the selection holds, of one moment.
    #[test]
    fn a_selected_row_outside_the_cache_is_read_with_the_cached_rows_beside_it() {
        let mut model = showing(50_000);
        let accepted = model.set_selection(vec![0..1, 40_000..40_001]);
        assert_eq!(accepted.then, Some(Then::ReadSelectionSoon));
        assert_eq!(
            captions::selection_status(&model, &English),
            "Reading the selection…"
        );
        let fetch = model.read_selection().expect("the selection is read");
        assert_eq!(model.read_selection(), None, "one request");
        let Ask::Selection(ask) = &fetch.ask else {
            panic!("a selection fetch")
        };
        assert_eq!((ask.span.start, ask.span.len), (0, 40_001));
        assert_eq!(ask.cached, [0..CHUNK_ROWS]);

        assert_eq!(
            model.accept(selection_reply(&fetch, "D:")).change,
            Change::SelectionRead {
                rows: 0..CHUNK_ROWS
            }
        );
        assert_eq!(paths(&model), [r"D:\row0", r"D:\row40000"]);
        let shown = model.results.row(5).map(|r| r.path.as_str());
        assert_eq!(shown, Some(r"D:\row5"), "the cached rows are of its moment");
    }

    /// T6: a reply for a selection the person has changed since names other
    /// files.
    #[test]
    fn t6_a_selection_changed_while_it_is_read_drops_the_older_reply() {
        let mut model = showing(50_000);
        model.set_selection(vec![40_000..40_001]);
        let older = model.read_selection().expect("the selection is read");
        // The window ends the older read when it schedules the new one.
        assert_eq!(
            model.set_selection(vec![40_000..40_002]).then,
            Some(Then::ReadSelectionSoon)
        );
        assert_eq!(
            model.accept(selection_reply(&older, "D:")).change,
            Change::Nothing
        );
        assert_eq!(paths(&model), Vec::<&str>::new());
        let newer = model.read_selection().expect("the new selection is read");
        assert!(newer.generation > older.generation);
    }

    /// The request for one selected folder's contents, which `accepted`
    /// starts.
    fn folder_size(accepted: Accepted) -> Fetch {
        match accepted.then {
            Some(Then::Fetch(
                fetch @ Fetch {
                    ask: Ask::FolderSize(_),
                    ..
                },
            )) => fetch,
            other => panic!("a folder size request: {other:?}"),
        }
    }

    fn totals(fetch: &Fetch, count: u64) -> Fetched {
        Fetched {
            purpose: Purpose::Selection,
            generation: fetch.generation,
            outcome: Outcome::Totals(gozler_core::proto::QueryTotals {
                count,
                size: 2048,
                volumes_incomplete: false,
                metadata_pending: false,
            }),
        }
    }

    /// T7: the size of a folder the person has left names another folder,
    /// and a selection that needs no count must still end the older one.
    #[test]
    fn t7_a_selection_changed_while_a_folder_is_counted_drops_the_older_count() {
        let mut model = model();
        let fetch = search(&mut model, "report");
        let mut reply = rows(&fetch, 10);
        if let Outcome::Rows(chunk) = &mut reply.outcome {
            chunk.rows[0].is_dir = true;
            chunk.rows[1].is_dir = true;
        }
        model.accept(reply);
        let size = |model: &Model| {
            captions::InspectorCaptions::new(model, &English).fields[1]
                .1
                .clone()
        };

        let first = folder_size(model.set_selection(vec![0..1]));
        assert_eq!(size(&model), "Calculating…");
        let second = folder_size(model.set_selection(vec![1..2]));
        let Ask::FolderSize(search) = &second.ask else {
            unreachable!("folder_size returns a folder size request")
        };
        assert_eq!(search.scope.as_deref(), Some(r"C:\row1"));
        assert_eq!(model.accept(totals(&first, 5)).change, Change::Nothing);
        assert_eq!(size(&model), "Calculating…");
        assert_eq!(
            model.accept(totals(&second, 7)),
            Accepted::only(Change::FolderCounted)
        );
        assert_eq!(size(&model), "2.00 KB (2,048 bytes) in 7 items");

        let _third = folder_size(model.set_selection(vec![0..1]));
        assert_eq!(
            model.set_selection(vec![2..3]).then,
            Some(Then::Cancel(Purpose::Selection))
        );
        assert_eq!(
            captions::InspectorCaptions::new(&model, &English).heading,
            "row2"
        );
    }

    /// A failed read left unread would say "Reading the selection…", with
    /// its commands disabled, until the person selected again, and nothing
    /// would say why (DESIGN.md: T3, T4); showing the failure over a search in
    /// flight would drop that search's reply.
    #[test]
    fn a_selection_read_that_fails_shows_the_failure_and_polls() {
        let mut model = showing(50_000);
        model.set_selection(vec![40_000..40_001]);
        let fetch = model.read_selection().expect("the selection is read");
        assert_eq!(
            model.accept(failed(&fetch, DaemonFailure::Lost)).change,
            Change::Message
        );
        assert_eq!(model.selection, Selection::None);
        let strings = english();
        assert_eq!(
            message(&model),
            Some((strings.text("failure.lost").to_owned(), vec![]))
        );
        assert!(model.polls_status());

        let mut model = showing(50_000);
        model.set_selection(vec![40_000..40_001]);
        let read = model.read_selection().expect("the selection is read");
        let newer = search(&mut model, "repo");
        model.accept(failed(&read, DaemonFailure::Lost));
        assert_eq!(
            model.accept(rows(&newer, 3)).change,
            Change::NewSearch {
                count: 3,
                open_first: false
            },
            "a search in flight shows its own reply"
        );
    }

    /// T12: a language switch that searched again or reset the results
    /// would lose the person's place, and one that forgot the choice would
    /// open in the old language after a restart.
    #[test]
    fn t12_a_language_switch_keeps_the_results_the_selection_and_the_text() {
        let mut model = showing(50_000);
        model.set_selection(vec![3..5]);
        model.set_typed_search(r"F:\ ext:cs".into());
        model.set_language(LanguageChoice::Tag("en".into()), english());
        assert_eq!(model.results().count(), 50_000);
        assert_eq!(paths(&model), [r"C:\row3", r"C:\row4"]);
        assert_eq!(model.text, r"F:\ ext:cs");
        assert_eq!(model.choices().language, LanguageChoice::Tag("en".into()));
    }

    /// T2: the rows of a new search are other files.
    #[test]
    fn a_new_search_clears_the_selection() {
        let mut model = showing(10);
        model.set_selection(vec![1..2]);
        let fetch = search(&mut model, "repo");
        assert_eq!(paths(&model), [r"C:\row1"], "the selection stays until");
        model.accept(rows(&fetch, 3));
        assert_eq!(model.selection, Selection::None);
    }

    /// Starts an export of the shown search as CSV; returns its fetch.
    fn export(model: &mut Model) -> Fetch {
        let order = model.column_order();
        match model.start_export(r"D:\results.csv".into(), ExportFormat::Csv, &order) {
            Accepted {
                change: Change::Export,
                then: Some(Then::Fetch(fetch)),
            } => fetch,
            other => panic!("the export starts a request: {other:?}"),
        }
    }

    fn export_reply(fetch: &Fetch, outcome: Outcome) -> Fetched {
        Fetched {
            purpose: Purpose::Export,
            generation: fetch.generation,
            outcome,
        }
    }

    /// The export must write what `gozler -export-csv` writes with the
    /// switches of the shown columns, or the two disagree for one search
    /// (DESIGN.md: Export).
    #[test]
    fn an_export_writes_as_gozler_with_the_shown_columns() {
        let mut model = showing(10);
        let Ask::Export(ask) = export(&mut model).ask else {
            panic!("an export fetch")
        };
        let args = ["report", "-export-csv", r"D:\results.csv", "-size", "-dm"];
        let Ok(Command::Run(plan)) = parse_argv(&args.map(String::from)) else {
            panic!("the command line is a search")
        };
        assert_eq!(ask.format, plan.result_format());
        assert!(!ask.want_attributes);
    }

    /// T8: the export is not the search's, so a new search neither stops it
    /// nor hides its result.
    #[test]
    fn t8_an_export_that_ends_while_a_search_runs_shows_its_result() {
        let mut model = showing(10);
        let fetch = export(&mut model);
        assert!(!model.can_export(), "one export at a time");
        let _search = search(&mut model, "repo");
        let progress = Outcome::ExportProgress { rows: 4 };
        model.accept(export_reply(&fetch, progress));
        assert_eq!(
            captions::export_status(&model, &English),
            "Exporting… 4 rows"
        );
        let ended = model.accept(export_reply(&fetch, Outcome::Exported { rows: 10 }));
        assert_eq!(ended, Accepted::only(Change::Export));
        assert_eq!(
            captions::export_status(&model, &English),
            "Exported 10 rows to results.csv"
        );
        assert_eq!(captions::results_status(&model, &English), "Searching…");
        assert!(model.can_export());
    }

    /// T10: the Index page must show that the service stopped while it is
    /// open, so the poll it starts goes on through the failure; the list
    /// keeps its rows, as a failed status request always does (DESIGN.md:
    /// Requests).
    #[test]
    fn t10_a_failure_while_settings_shows_the_index_page_keeps_the_poll() {
        let mut model = showing(10);
        assert!(!model.polls_status());
        model.show_index_page(true);
        assert!(model.polls_status(), "the page follows the service");
        let ask = model.status_needed().expect("no status request runs");
        let stopped = DaemonFailure::NotRunning(NotRunning::Stopped);
        assert_eq!(model.accept(failed(&ask, stopped)).change, Change::Status);
        assert_eq!(
            captions::IndexCaptions::new(&model, &English, &|_| None).service,
            english().text("failure.stopped")
        );
        assert_eq!(
            captions::index_status(&model),
            english().text("failure.stopped"),
            "the main window shows the failure behind the dialog"
        );
        assert!(model.polls_status());
        assert_eq!(model.results.count(), 10);
        // The service answers again: the page shows its state, and the
        // poll ends with the page.
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        model.show_index_page(false);
        assert!(!model.polls_status());
    }

    /// A service that answers again before its commands end stops the
    /// poll, so a fix that waited for the poll would end as "did not
    /// answer" with the service running (DESIGN.md: Service fixes).
    #[test]
    fn a_fix_asks_the_service_once_its_commands_end() {
        let mut model = showing(10);
        let started = model.start_fix(&Fix::Repair(Repair::StartService));
        assert_eq!(
            started.then,
            Some(Then::RunFix(FixWork::Commands(vec![
                "net start gozler".into()
            ])))
        );
        let other = ListButton::Fix(Fix::Repair(Repair::TurnOnService));
        assert!(!model.list_button_enabled(&other), "one fix runs at a time");
        model.accept_fix(FixEvent::Commands(CommandEvent::Allowed));
        assert!(!model.polls_status());
        let ended = model.accept_fix(FixEvent::Commands(CommandEvent::Succeeded));
        let Some(Then::Fetch(ask)) = ended.then else {
            panic!("the fix asks the service: {ended:?}");
        };
        assert!(
            model.polls_status(),
            "the poll runs until the service answers"
        );
        model.accept(status(&ask, VolumePhase::Live));
        assert!(!model.polls_status());
        assert_eq!(
            captions::StatusCaptions::new(&model, &English).job.text,
            english().text("fixing.done")
        );
    }

    /// S6, and C6's search as you type: with them off, the empty search and
    /// typing start no request, and `Enter` still searches.
    #[test]
    fn s6_with_both_search_settings_off_only_a_typed_search_runs_on_enter() {
        let mut model = model();
        let accepted = model.set_choices(Choices {
            search_as_you_type: false,
            show_all_when_empty: false,
            ..Choices::default()
        });
        assert_eq!(accepted.then, Some(Then::SearchAgain));
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        assert_eq!(
            model.start_search(&start, ScopeCheck::Exists),
            Accepted::only(Change::Message)
        );
        assert_eq!(
            message(&model),
            Some(("Type to search.".to_owned(), vec![]))
        );
        assert_eq!(model.set_typed_search("report".into()).then, None);
        assert!(matches!(
            model.start_search(&start, ScopeCheck::Exists).then,
            Some(Then::Fetch(_))
        ));
    }

    /// The Use Total Commander link's reply goes to the dialog alone; a
    /// reply after the dialog closed, or a failure, changes nothing the
    /// window shows.
    #[test]
    fn use_total_commander_answers_the_dialog_only() {
        let mut model = showing(10);
        let fetch = model.find_total_commander();
        let mut reply = rows(&fetch_rows(&fetch), 1);
        reply.purpose = Purpose::Program;
        if let Outcome::Rows(chunk) = &mut reply.outcome {
            chunk.rows[0].path = r"C:\TC\TOTALCMD64.EXE".into();
        }
        assert_eq!(
            model.accept(reply.clone()).change,
            Change::TotalCommander(Ok(Some(r"C:\TC\TOTALCMD64.EXE".into())))
        );
        assert_eq!(model.accept(reply).change, Change::Nothing, "answered once");
        let fetch = model.find_total_commander();
        assert_eq!(
            model.accept(failed(&fetch, DaemonFailure::Lost)).change,
            Change::TotalCommander(Err(DaemonFailure::Lost))
        );
        assert!(!model.polls_status());
        assert_eq!(model.results.count(), 10);
        let fetch = model.find_total_commander();
        assert_eq!(
            model.forget_total_commander().then,
            Some(Then::Cancel(Purpose::Program))
        );
        assert_eq!(
            model.accept(failed(&fetch, DaemonFailure::Lost)).change,
            Change::Nothing
        );
    }

    /// The rows fetch that `fetch`'s chunk ask stands for, so [`rows`]
    /// answers a program search.
    fn fetch_rows(fetch: &Fetch) -> Fetch {
        let Ask::Program(ask) = &fetch.ask else {
            panic!("a program search: {fetch:?}")
        };
        Fetch {
            generation: fetch.generation,
            ask: Ask::Rows(ask.clone()),
        }
    }

    /// The middle group shows one job: the newest of the running ones, so
    /// an export started during a copy shows, and the copy shows again
    /// when the export ends only if it started later (DESIGN.md: Status
    /// bar; file-manager-spec.md: File operations, step 5).
    #[test]
    fn the_newest_running_job_shows() {
        let mut model = showing(10);
        let running = |id| {
            Some(StatusOperation::Running {
                id: OperationId::for_tests(id),
                request: Arc::new(Request::Delete {
                    sources: vec![r"C:\f\a.txt".into()],
                }),
                running: gozler_fileops::Running {
                    items: 0,
                    bytes: 0,
                    passed: 0,
                    total: None,
                    renames: false,
                    paused: false,
                    current: None,
                },
            })
        };
        model.set_operation(running(1));
        assert_eq!(model.job(), Job::Operation);
        export(&mut model);
        assert_eq!(model.job(), Job::Export);
        model.set_operation(running(2));
        assert_eq!(model.job(), Job::Operation);
    }

    /// T9: the person must learn that the file was not written, and may
    /// export again at once.
    #[test]
    fn t9_a_failed_export_reports_the_file_and_clears_part_3() {
        let mut model = showing(10);
        let fetch = export(&mut model);
        let denied = WriteFailure {
            kind: io::ErrorKind::PermissionDenied,
            os_code: Some(5),
        };
        assert_eq!(
            model.accept(export_reply(&fetch, Outcome::ExportFailed(denied))),
            Accepted::only(Change::ExportFailed {
                path: r"D:\results.csv".into(),
                error: ExportError::Write(denied),
            })
        );
        assert_eq!(captions::export_status(&model, &English), "");
        assert!(model.can_export());
    }
}
