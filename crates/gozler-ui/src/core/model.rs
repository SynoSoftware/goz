//! The window's facts: the loaded language, the saved settings, what the
//! controls ask for, what the list shows, the selection, the export, the
//! service fix and what the service said last. Captions derive from these
//! facts; an operation pass publishes an immutable presentation for UI readers
//! while the next pass runs. The fields are private to the core: service replies
//! pass through [`Model::accept`], operation answers through
//! [`Model::accept_followed`], and user changes through their own methods.
//! Each transition returns the areas whose values changed and ordered effects.
//! Native structure precedes those values; tasks, focus and modal intents
//! follow them. The app delivers effects without holding a model borrow.

use super::fetch::{
    Ask, ChunkAsk, DaemonFailure, ExportAsk, Fetch, Fetched, FromSnapshot, Generation, List, Local,
    Outcome, Programs, Purpose, WriteFailure,
};
use super::file_operations::{self, StatusOperation};
use super::fixer::{FixEvent, FixWork, Fixer};
pub(crate) use super::inspector::AgeTimer;
use super::layout::{Dip, Panes};
use super::open::{TOTAL_COMMANDER_SEARCH, newest_total_commander};
use super::operations::{OperationSteps, QueueStep, Source};
use super::panel::{LookIn, PanelFollow, Panels, Side, TakenRows, View, drive_root};
use super::query::{self, Filters, LocationItem, Matching};
use super::results::{Fix, ListButton, ListMessage, RowsChange, Shown};
use super::search::SearchArea;
use super::search_list::Deselect;
use super::search_list::{Ended, Keep, Kept, SearchList, SelectedSize};
use super::selection::{
    Filter, Filtered, FolderContents, RowSelection, SELECTION_LIMIT, Selection, SelectionSource,
    Sums, wide_path,
};
use super::settings::{
    Changes, Choices, Mode, Placement, SavedColumn, Settings, SettingsPage, ViewerState,
};
use super::text::{self, LanguageChoice, Strings};
use super::update;
use super::work::Retirement;
use gozler_core::escompat::EsPlan;
use gozler_core::include::IndexSettings;
use gozler_core::links::Links;
use gozler_core::output::Format;
use gozler_core::proto::{DaemonStatus, ResultItem, VolumeStatus};
use gozler_core::query::{ParsedQuery, QueryStart};
use gozler_core::scope::{Coverage, leaves_matches_out};
use gozler_core::search::Search;
use gozler_core::types::{EsColumn, SortDir, SortKey, SortSpec, VolumePhase};
use gozler_core::winpath::parent_of;
use gozler_fileops::{Operation, OperationId, Taken};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

mod accepted;
mod following;
pub(crate) use accepted::{Accepted, Areas, Effect, may_apply};

/// Which Gozler of the session a window is (DESIGN.md: Window lifetime).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Instance {
    /// The first: it has the tray icon and the shortcut to show Gozler, and
    /// checks for updates.
    First,
    /// A later start while "Allow only one Gozler window" is off: Close
    /// ends it, and it saves only what Settings applies, so closing it
    /// never undoes a setting that another window saved.
    Another,
}

pub(crate) struct Model {
    instance: Instance,
    pub(super) strings: Strings,
    /// Also the one copy of the match controls, the filters, the sort and
    /// whether the filter pane shows.
    pub(super) settings: Settings,
    /// `settings` when [`Model::saved_state_changed`] last ran, so the save
    /// timer starts once for each change.
    seen: Settings,
    /// The settings as this window saved them last, or loaded them, which
    /// a save compares with, so it writes only what this window changed.
    saved: Settings,
    /// The columns the list shows, in the list's column order: Name first,
    /// because only the first column carries the icon.
    pub(super) columns: Vec<Column>,
    pub(super) search: SearchArea,
    /// The latest status reply.
    pub(super) status: Option<DaemonStatus>,
    /// The folder links the service last sent, whose paths the Search
    /// list's matched parts follow.
    links: Arc<Links>,
    panel_folders: [Option<(u64, String, String)>; 2],
    /// The service's version, from the connection of the latest status
    /// reply.
    service_version: Option<String>,
    /// The gozlerd.exe files, as the latest status reply found them.
    programs: Programs,
    /// The app's own working set, read when it last asked for status;
    /// `None` when Windows could not say.
    app_working_set: Option<u64>,
    pub(super) export: Export,
    fix: Fixer,
    /// Work and replies retain the accepted fix's identity before its launch.
    fix_generation: Option<Generation>,
    following: following::OperationFollow,
    inspector: super::inspector::Inspector,
    location_observed: Option<(String, ScopeCheck)>,
    inspector_content: super::layout::InspectorContent,
    /// The file operation the middle group shows.
    operation: Option<StatusOperation>,
    /// The jobs in the order they last started, the newest last.
    started_jobs: Vec<(Job, u64)>,
    job_moment: u64,
    /// How the latest request for the results or the status failed; `None`
    /// once the service answers.
    failure: Option<DaemonFailure>,
    /// The status request in flight.
    status_asked: Option<Generation>,
    /// The Use Total Commander link's search in flight.
    program_asked: Option<Generation>,
    /// Settings > Include's count in flight.
    include_count_asked: Option<Generation>,
    /// Settings > Include's count for the current edit, once it arrived:
    /// the entries the index would keep, or why the service could not
    /// count them.
    included_count: Option<Result<u64, DaemonFailure>>,
    /// The notice that the Include settings are saved, in flight.
    settings_saved_asked: Option<Generation>,
    settings_page: Option<SettingsPage>,
    /// The newer release that Check for updates found, such as `0.4.0`.
    newer_release: Option<String>,
    latest_release: LatestRelease,
    last_generation: u64,
    /// Whether the clipboard holds files, which Paste copies or moves.
    clipboard_files: bool,
    panels: Panels,
    retired: Retirement,
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

/// What new taken entries change in the lists ([`Model::set_taken`]).
#[derive(Debug, PartialEq)]
pub(crate) struct TakenShown {
    pub(crate) panels: [TakenRows; 2],
    /// Search's selection lost files, which its status line counts.
    pub(crate) search_lost: bool,
    /// The rows Search's list unhighlights.
    pub(crate) search_deselect: Deselect,
}

/// What `Enter` in the search box does ([`Model::enter_in_search_box`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchBoxEnter {
    /// The search runs at once.
    Search,
    /// The selected row takes `Enter`, as in the list, or the first row
    /// when none is selected.
    Enter,
    /// The search in flight gives its first row `Enter` when it answers.
    Wait,
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

/// The latest background observation of Location; the core never reads a disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeCheck {
    Exists,
    Missing,
}

impl Model {
    pub(crate) fn new(strings: Strings, settings: Settings, instance: Instance) -> Model {
        let window = &settings.window;
        let panels = Panels::new(&window.panels);
        let mut columns = vec![Column::Name];
        let saved: Vec<Column> = window.columns.iter().map(|c| c.column).collect();
        let shown = if saved.is_empty() {
            Column::ALL
                .into_iter()
                .filter(|c| c.row().shown_by_default)
                .collect()
        } else {
            saved
        };
        for column in shown {
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
        let mut model = Model {
            instance,
            strings,
            columns,
            seen: settings.clone(),
            saved: settings.clone(),
            settings,
            search: SearchArea::new(),
            status: None,
            links: Arc::default(),
            panel_folders: [None, None],
            service_version: None,
            programs: Programs::UNKNOWN,
            app_working_set: None,
            export: Export::Idle,
            fix: Fixer::Idle,
            fix_generation: None,
            following: following::OperationFollow::new(),
            inspector: super::inspector::Inspector::default(),
            location_observed: None,
            inspector_content: super::layout::InspectorContent::Selection,
            operation: None,
            started_jobs: Vec::new(),
            job_moment: 0,
            failure: None,
            status_asked: None,
            program_asked: None,
            include_count_asked: None,
            included_count: None,
            settings_saved_asked: None,
            settings_page: None,
            newer_release: None,
            latest_release: LatestRelease::Unknown,
            last_generation: 0,
            clipboard_files: false,
            panels,
            retired: Retirement::default(),
        };
        // The saved columns stay current in display order from now on,
        // also when the file had none.
        model.settings.window.columns = (model.column_order().into_iter())
            .map(|index| SavedColumn {
                column: model.columns[index],
            })
            .collect();
        model.seen.clone_from(&model.settings);
        model
    }

    pub(crate) fn instance(&self) -> Instance {
        self.instance
    }

    pub(crate) fn inspect_age(&mut self) -> Accepted {
        let timer = self.inspector.refresh(&self.search.list);
        Accepted {
            touched: Areas::NONE,
            effects: vec![Effect::Age(timer)],
        }
    }

    pub(crate) fn awaits(&self, purpose: Purpose, generation: Generation) -> bool {
        match purpose {
            Purpose::Results(list) => self
                .list(list)
                .is_some_and(|shown| shown.results.awaits(generation)),
            Purpose::Selection(list) => self
                .list(list)
                .is_some_and(|shown| shown.selection.asked() == Some(generation)),
            Purpose::Status => self.status_asked == Some(generation),
            Purpose::Program => self.program_asked == Some(generation),
            Purpose::IncludeCount => self.include_count_asked == Some(generation),
            Purpose::SettingsSaved => self.settings_saved_asked == Some(generation),
            Purpose::Export => {
                matches!(self.export, Export::Running { generation: asked, .. } if asked == generation)
            }
            Purpose::FolderSizes(_) => true,
        }
    }

    pub(crate) fn awaits_fix(&self, generation: Generation) -> bool {
        self.fix_generation == Some(generation) && self.fix.running()
    }

    pub(crate) fn inspected_modified(&self) -> Option<i64> {
        match self.search.list.selection().files()? {
            [entry] => entry.mtime_ft,
            _ => None,
        }
    }

    pub(crate) fn location_check(&self) -> ScopeCheck {
        self.location_observed
            .as_ref()
            .filter(|(folder, _)| self.location() == Some(folder.as_str()))
            .map_or(ScopeCheck::Exists, |(_, check)| *check)
    }

    pub(crate) fn observe_location(&mut self, folder: String, check: ScopeCheck) -> bool {
        if self.location() != Some(folder.as_str()) {
            return false;
        }
        let changed = self.location_check() != check;
        self.location_observed = Some((folder, check));
        changed
    }

    pub(crate) fn take_retired(&mut self) -> Retirement {
        let mut retired = std::mem::take(&mut self.retired);
        retired.append(self.search.list.take_retired());
        retired.append(self.panels.take_retired());
        retired
    }

    pub(crate) fn offers_file_operations(&self) -> bool {
        self.instance == Instance::First
    }

    /// Where the folder picker of Copy to folder… and Move to folder…
    /// opens.
    pub(crate) fn destination_folder(&self) -> Option<&str> {
        self.settings.window.destination_folder.as_deref()
    }

    pub(crate) fn set_destination_folder(&mut self, folder: String) -> Accepted {
        self.settings.window.destination_folder = Some(folder);
        Accepted {
            touched: Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
            effects: vec![Effect::InspectAge, Effect::Polling, Effect::NoteSavedState],
        }
    }

    pub(crate) fn set_clipboard_holds_files(&mut self, holds: bool) {
        self.clipboard_files = holds;
    }

    pub(crate) fn clipboard_holds_files(&self) -> bool {
        self.clipboard_files
    }

    /// The row at `index` as `list` shows it: `None` while its chunk has
    /// not arrived, or while a delete or a move has taken its entry.
    pub(crate) fn shown_row(&self, list: List, index: u64) -> Option<&ResultItem> {
        self.list(list)?.shown_row(index, self.panels.taken())
    }

    /// The parts of the cell in `column` of Search's row at `index` that
    /// the search matched, from the cell's start, which the list draws in
    /// bold (DESIGN.md: Results list). None outside Name and Path, and none
    /// for a row whose entry a delete or a move took, which draws plain and
    /// struck through.
    pub(crate) fn matched_in_cell(&self, index: u64, column: Column) -> Vec<Range<usize>> {
        let Some(row) = self.shown_row(List::Search, index) else {
            return Vec::new();
        };
        let Some(span) = text::path_part(column, &row.path) else {
            return Vec::new();
        };
        within(&self.search.list.results().matched_parts(row), &span)
    }

    /// Whether a selection key can select `row` of `list`: not `..`, and
    /// not an entry that a delete or a move has taken.
    pub(crate) fn selectable(&self, list: List, row: u64) -> bool {
        match list {
            List::Panel(side) => self.panels.selectable(side, row),
            List::Search => self.list(list).is_some_and(|shown| {
                row < shown.results().count() && !shown.row_taken(row, self.panels.taken())
            }),
        }
    }

    /// The rows of `selected` in `list` whose entries a delete or a move
    /// has taken, for native deselection. Pending filtering/keeping preserves
    /// highlights even though painting may conservatively hide taken entries.
    pub(crate) fn taken_rows(&self, list: List, selected: &[Range<u64>]) -> Vec<u64> {
        let taken = self.panels.taken();
        match self.list(list) {
            Some(shown)
                if !taken.is_empty()
                    && !shown.is_keeping()
                    && !matches!(shown.selection(), Selection::Filtering { .. }) =>
            {
                (selected.iter().cloned().flatten())
                    .filter(|&row| shown.row_taken(row, taken))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn collapse_inspector(&mut self) -> Accepted {
        self.settings.window.inspector = false;
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::PaneFocus,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    pub(crate) fn take_keep(&mut self, list: List) -> Option<Keep> {
        self.list_mut(list)?.take_keep()
    }

    pub(crate) fn accept_kept(&mut self, list: List, answer: Kept) -> Accepted {
        if let Some(shown) = self.list_mut(list) {
            if shown.accept_kept(answer) {
                return Accepted {
                    touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                    effects: vec![
                        Effect::Deselect {
                            list,
                            rows: Deselect::Nothing,
                        },
                        Effect::ListPasses,
                        Effect::InspectAge,
                        Effect::Polling,
                        Effect::NoteSavedState,
                    ],
                };
            }
        } else {
            let items = answer.kept.len();
            self.retired.push(answer, items);
        }
        Accepted::default()
    }

    /// Whether a list still shows the rows of the ended operation `id`
    /// blank, so it stays in the queue.
    #[cfg(test)]
    pub(crate) fn keeps(&self, id: OperationId) -> bool {
        List::ALL
            .into_iter()
            .any(|list| self.list(list).is_some_and(|shown| shown.keeps(id)))
            || self.panels.keep(id)
    }

    /// Whether every list shows the taken entries `taken` already.
    pub(crate) fn shows_taken(&self, taken: &Arc<Taken>) -> bool {
        Arc::ptr_eq(self.panels.taken(), taken)
    }

    /// Gives every list the taken entries `taken` at once, so no list's
    /// deselect pass reads the ones before: the panels as
    /// [`Panels::set_taken`] says, with `focused` their lists' focused rows,
    /// and Search's selection loses them, its list showing their rows blank
    /// once it draws them again.
    pub(crate) fn set_taken(&mut self, taken: Arc<Taken>, focused: [Option<u64>; 2]) -> TakenShown {
        let mut panels = self.panels.set_taken(taken, focused);
        let mut search_changed = false;
        let mut search_deselect = self.search.list.deselect();
        for list in List::ALL {
            let changed = self
                .list(list)
                .is_some_and(|shown| shown.selection.files().is_some());
            let filtered = if self.refilter_selection(list) {
                self.inline_filter(list)
            } else {
                None
            };
            let deselect = match filtered {
                Some(accepted) => accepted
                    .effects
                    .into_iter()
                    .find_map(|effect| match effect {
                        Effect::Deselect { rows, .. } => Some(rows),
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        self.list(list)
                            .map_or(Deselect::Nothing, SearchList::deselect)
                    }),
                _ => self
                    .list(list)
                    .map_or(Deselect::Nothing, SearchList::deselect),
            };
            if list == List::Search {
                search_changed = changed;
                search_deselect = deselect;
            }
            if let List::Panel(side) = list
                && self.panels.search(side).is_some()
            {
                panels[side.index()] = TakenRows::Search { deselect };
            }
        }
        TakenShown {
            panels,
            search_lost: search_changed,
            search_deselect,
        }
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

    /// The service's working set, which the status bar and the Drives and
    /// service page show; `None` while it does not answer, because the
    /// latest status can be from before it stopped, and when its status
    /// says 0, because it could not read its own.
    pub(crate) fn service_working_set(&self) -> Option<u64> {
        match (&self.status, &self.failure) {
            (Some(status), None) => Some(status.process_working_set).filter(|&bytes| bytes != 0),
            _ => None,
        }
    }

    /// The version of the service the window is connected to, which About
    /// shows; `None` while the service does not answer.
    pub(crate) fn service_version(&self) -> Option<&str> {
        match (&self.service_version, &self.failure) {
            (Some(version), None) => Some(version),
            _ => None,
        }
    }

    /// The app's own working set, which the status bar shows.
    pub(crate) fn app_working_set(&self) -> Option<u64> {
        self.app_working_set
    }

    /// Records the app's own working set, read as it asks for status.
    pub(crate) fn set_app_working_set(&mut self, bytes: Option<u64>) -> Accepted {
        self.app_working_set = bytes;
        Accepted {
            touched: Areas::STATUS,
            effects: Vec::new(),
        }
    }

    /// Settings applied `choices`. Showing every file for an empty search
    /// changes what the empty search shows, so it runs again.
    pub(crate) fn set_choices(&mut self, choices: Choices) -> Accepted {
        let search_again = choices.show_all_when_empty != self.choices().show_all_when_empty;
        self.settings.choices = choices;
        {
            let mut accepted = Accepted {
                touched: Areas::TITLE
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::STATUS
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![Effect::Menu, Effect::Tray, Effect::InspectAge],
            };

            accepted
                .effects
                .extend(search_again.then(|| Effect::SearchAgain(vec![List::Search])));
            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    pub(crate) fn set_copy_policy(&mut self, policy: gozler_fileops::Policy) {
        self.settings.choices.copy_policy = policy;
    }

    /// Replaces the language with `strings` and records `choice`, which the
    /// settings file saves. It starts no request, because the results do
    /// not depend on the language (DESIGN.md: Language, `T12`).
    pub(crate) fn set_language(&mut self, choice: LanguageChoice, strings: Strings) -> Accepted {
        self.settings.choices.language = choice;
        self.strings = strings;
        self.operation_text_changed();
        Accepted {
            touched: Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
            effects: vec![
                Effect::InspectAge,
                Effect::ListPasses,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    pub(crate) fn show_settings_page(&mut self, page: Option<SettingsPage>) {
        self.settings_page = page;
    }

    /// Whether the window hides to the notification area for the first
    /// time, which shows a notification once; from then on, it does not.
    pub(crate) fn first_hide(&mut self) -> bool {
        !std::mem::replace(&mut self.settings.window.tray_notice_shown, true)
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
        Accepted {
            touched: Areas::STATUS
                | Areas::LOCATIONS
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::PreparePanel(Side::Left),
                Effect::PreparePanel(Side::Right),
                Effect::ReadMemory,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    pub(crate) fn newer_release(&self) -> Option<&str> {
        self.newer_release.as_deref()
    }

    /// GitHub's latest release is or is not older than the window, which
    /// the service's fix button says.
    pub(crate) fn set_latest_release_older(&mut self, older: bool) -> Accepted {
        self.latest_release = LatestRelease::Known { older };
        Accepted {
            touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
            effects: vec![
                Effect::PostSettings,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    /// Asks once whether GitHub's latest release is older than the window,
    /// when the service is first found older than the window, so its fix
    /// reads Downgrade before the person clicks it. With Check for updates
    /// off, Gozler asks GitHub only after a click, so the button reads
    /// Update and the question after the click says which it is.
    fn ask_latest_release(&mut self, failure: &DaemonFailure) -> Option<Effect> {
        let ask = matches!(failure, DaemonFailure::TooOld { .. })
            && self.settings.choices.check_for_updates
            && self.latest_release == LatestRelease::Unknown;
        ask.then(|| {
            self.latest_release = LatestRelease::Asking;
            Effect::AskLatestRelease
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
                snapshot: FromSnapshot::No,
            }),
        }
    }

    /// Settings closed, so the Use Total Commander link's search has nobody to
    /// answer; the window ends its request (rule 4).
    pub(crate) fn forget_total_commander(&mut self) -> Accepted {
        {
            let mut accepted = Accepted::default();
            accepted.effects.extend(
                self.program_asked
                    .take()
                    .map(|_| Effect::Cancel(Purpose::Program)),
            );
            accepted
        }
    }

    /// Settings > Include's preview, which cancels an earlier one: how
    /// many entries the index keeps under `settings` (DESIGN.md: The
    /// Include page).
    pub(crate) fn count_included(&mut self, settings: IndexSettings) -> Fetch {
        let generation = self.generation();
        self.include_count_asked = Some(generation);
        self.included_count = None;
        Fetch {
            generation,
            ask: Ask::CountIncluded(settings),
        }
    }

    /// The Include settings changed, or Settings closed: the count in
    /// flight and the one that arrived are of other settings, so the
    /// preview shows no count until the next one arrives (DESIGN.md: The
    /// Include page).
    pub(crate) fn forget_included_count(&mut self) -> Accepted {
        self.included_count = None;
        {
            let mut accepted = Accepted::default();
            accepted.effects.extend(
                self.include_count_asked
                    .take()
                    .map(|_| Effect::Cancel(Purpose::IncludeCount)),
            );
            accepted
        }
    }

    /// Settings > Include's count of the settings it shows, once it
    /// arrived.
    pub(crate) fn included_count(&self) -> Option<&Result<u64, DaemonFailure>> {
        self.included_count.as_ref()
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
            inspector: window.inspector
                && (self.mode() == Mode::Search
                    || self.inspector_content == super::layout::InspectorContent::Transfers),
            inspector_height: window.inspector_height,
            content: self.inspector_content,
            maximized: window.inspector_maximized,
            flipped: window.transfers_flipped,
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
        let results = self.search.list.results();
        match results.shown() {
            Shown::Rows(_) if results.total() == 0 => [
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

    /// Search's results list.
    pub(crate) fn search_list(&self) -> &SearchList {
        &self.search.list
    }

    pub(crate) fn active_list(&self) -> List {
        match self.mode() {
            Mode::Search => List::Search,
            Mode::Commander => List::Panel(self.panels.active()),
        }
    }

    /// The state of `list`: always Search's, and a panel's while it is a
    /// search panel.
    pub(crate) fn list(&self, list: List) -> Option<&SearchList> {
        match list {
            List::Search => Some(&self.search.list),
            List::Panel(side) => self.panels.search(side).map(|search| &search.list),
        }
    }

    /// How many entries a file command or a drag of `list` acts on: the
    /// selected entries, else the focused one (DESIGN.md: Selecting
    /// entries). A list of results counts them once they are read.
    pub(crate) fn chosen_count(&self, list: List) -> usize {
        match list {
            List::Search => (self.search.list.selection().files()).map_or(0, <[_]>::len),
            List::Panel(side) => self.panels.chosen_count(side),
        }
    }

    /// The file that View shows from `list`: Search's one selected file,
    /// once the selection is read, or a panel's focused file
    /// (DESIGN.md: The Viewer).
    pub(crate) fn file_to_view(&self, list: List) -> Option<Vec<u16>> {
        match list {
            List::Search => match self.search.list.selection().files() {
                Some([file]) if !file.is_dir => Some(wide_path(file)),
                _ => None,
            },
            List::Panel(side) => (self.panels.focused(side))
                .filter(|entry| !entry.is_folder)
                .map(|entry| entry.path),
        }
    }

    fn list_mut(&mut self, list: List) -> Option<&mut SearchList> {
        match list {
            List::Search => Some(&mut self.search.list),
            List::Panel(side) => self.panels.search_mut(side).map(|search| &mut search.list),
        }
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
            Mode::Commander => self.panels.panel(self.panels.active()).sort(),
        }
    }

    pub(crate) fn panels(&self) -> &Panels {
        &self.panels
    }

    pub(crate) fn panels_mut(&mut self) -> &mut Panels {
        &mut self.panels
    }

    /// The request for `side`'s folder sizes, when it shows a folder with
    /// subfolders (DESIGN.md: Folder sizes).
    pub(crate) fn ask_sizes(&mut self, side: Side) -> Option<Fetch> {
        let generation = self.generation();
        let ask = self.panels.ask_sizes(side, generation)?;
        Some(Fetch { generation, ask })
    }

    /// What the latest status says about the volume of the active panel's
    /// folder: none when no volume holds it, so gozlerd does not index it.
    /// `None` for a search panel, whose search's coverage shows instead.
    pub(crate) fn panel_coverage(&self) -> Option<Coverage<'_>> {
        self.status.as_ref()?;
        let side = self.panels.active();
        let panel = self.panels.panel(side);
        matches!(panel.view(), View::Folder)
            .then(|| Coverage::of(self.volumes(), self.panel_folder(side), true))
    }

    fn panel_folder(&self, side: Side) -> Option<&str> {
        let folder = self.panels.panel(side).folder();
        if self.links.is_empty() {
            return Some(folder);
        }
        self.panel_folders[side.index()]
            .as_ref()
            .filter(|(links, held, _)| *links == self.links.fingerprint() && held == folder)
            .map(|(_, _, real)| real.as_str())
    }

    /// The saved window placement, which the window opens at.
    pub(crate) fn placement(&self) -> Option<Placement> {
        self.settings.window.placement
    }

    /// What the Viewer remembers, which the next Viewer opens with.
    pub(crate) fn viewer(&self) -> ViewerState {
        self.settings.window.viewer
    }

    pub(crate) fn set_viewer(&mut self, state: ViewerState) {
        self.settings.window.viewer = state;
    }

    /// The window's placement changed after it showed.
    pub(crate) fn set_placement(&mut self, placement: Placement) {
        self.settings.window.placement = Some(placement);
    }

    fn generation(&mut self) -> Generation {
        self.last_generation += 1;
        Generation(self.last_generation)
    }

    /// The person typed `text` in `list`'s search box; its search runs once
    /// they pause, or in Search with search as you type off, when they
    /// press `Enter` (DESIGN.md: C6). A search panel always searches as the
    /// person types (DESIGN.md: Search in a panel).
    pub(crate) fn set_typed_search(&mut self, list: List, text: String) -> Accepted {
        let Some(shown) = self.list_mut(list) else {
            return Accepted::default();
        };
        if text == shown.text {
            return Accepted::default();
        }
        shown.text = text;
        let soon = match list {
            List::Search => {
                // An open that waits is for the text before this one.
                self.search.enter_on_answer = false;
                self.search.due = self.choices().search_as_you_type;
                self.search.due
            }
            List::Panel(_) => true,
        };
        {
            let mut accepted = Accepted {
                touched: Areas::TITLE | Areas::SEARCH,
                effects: vec![],
            };
            accepted
                .effects
                .extend(soon.then_some(Effect::SearchSoon(list)));
            accepted
        }
    }

    /// What `Enter` in the search box does (DESIGN.md: window rule 8). With
    /// search as you type off, the search runs. On, the selected row or the
    /// first takes `Enter`; while the search for the text has not answered,
    /// it runs at once if it still waits for the pause, and its first row
    /// takes `Enter` when it answers.
    pub(crate) fn enter_in_search_box(&mut self) -> SearchBoxEnter {
        if !self.choices().search_as_you_type {
            return SearchBoxEnter::Search;
        }
        if self.search.due || self.search.list.results.searching() {
            self.search.enter_on_answer = true;
            return if self.search.due {
                SearchBoxEnter::Search
            } else {
                SearchBoxEnter::Wait
            };
        }
        SearchBoxEnter::Enter
    }

    /// The person changed a match control; the search runs again. The
    /// window's own selection of the shown mode in the Match list changes
    /// nothing.
    pub(crate) fn set_matching(&mut self, matching: Matching) -> Accepted {
        if matching == self.settings.window.matching {
            return Accepted::default();
        }
        self.settings.window.matching = matching;
        {
            let mut accepted = Accepted {
                touched: Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge],
            };

            accepted
                .effects
                .extend(Some(Effect::SearchAgain(vec![List::Search])));
            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// The person chose a sort; the search runs again (DESIGN.md: C20).
    pub(crate) fn sort_by(&mut self, sort: SortSpec) -> Accepted {
        self.settings.window.sort = sort;
        {
            let mut accepted = Accepted {
                touched: Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge],
            };

            accepted
                .effects
                .extend(Some(Effect::SearchAgain(vec![List::Search])));
            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// The person changed the filters; the search runs again. Only directly
    /// in this folder needs a folder, so Everywhere clears it.
    pub(crate) fn set_filters(&mut self, mut filters: Filters) -> Accepted {
        if filters.location.is_none() {
            filters.only_directly = false;
        }
        if filters == self.settings.window.filters {
            return Accepted::default();
        }
        if filters.location != self.settings.window.filters.location {
            self.location_observed = None;
        }
        self.settings.window.filters = filters;
        {
            let mut accepted = Accepted {
                touched: Areas::FILTERS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::STATUS
                    | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge],
            };

            accepted
                .effects
                .extend(Some(Effect::SearchAgain(vec![List::Search])));
            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
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
        let folder = match self.search.list.selection.files() {
            Some([entry]) if entry.is_dir => entry.path.clone(),
            Some([entry]) => parent_of(&entry.path),
            _ => {
                return Accepted::default();
            }
        };
        self.choose_folder(folder)
    }

    /// Shows `mode`. Each mode keeps its state while the other shows
    /// (DESIGN.md: Commander mode's window).
    pub(crate) fn show_mode(&mut self, mode: Mode) -> Accepted {
        let window = &mut self.settings.window;
        if window.mode == mode {
            return Accepted::default();
        }
        window.mode = mode;
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::Menu,
                Effect::SwitchPanels,
                Effect::HomeFocus,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    /// Shows or hides the filter pane (DESIGN.md: C13).
    pub(crate) fn toggle_filter_pane(&mut self) -> Accepted {
        let window = &mut self.settings.window;
        window.filter_pane = !window.filter_pane;
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::PaneFocus,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    /// Shows or hides the inspector (DESIGN.md: C14).
    pub(crate) fn toggle_inspector(&mut self) -> Accepted {
        let window = &mut self.settings.window;
        window.inspector = self.inspector_content != super::layout::InspectorContent::Selection
            || !window.inspector;
        self.inspector_content = super::layout::InspectorContent::Selection;
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::PaneFocus,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    pub(crate) fn show_transfers(&mut self) -> Accepted {
        self.inspector_content = super::layout::InspectorContent::Transfers;
        self.settings.window.inspector = true;
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::PaneFocus,
                Effect::ListPasses,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    pub(crate) fn toggle_transfers(&mut self) -> Accepted {
        if self.inspector_content == super::layout::InspectorContent::Transfers
            && self.settings.window.inspector
        {
            self.settings.window.inspector = false;
        } else {
            self.inspector_content = super::layout::InspectorContent::Transfers;
            self.settings.window.inspector = true;
        }
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::PaneFocus,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    pub(crate) fn maximize_inspector(&mut self) -> Accepted {
        self.settings.window.inspector_maximized = !self.settings.window.inspector_maximized;
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::PaneFocus,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    pub(crate) fn flip_transfers(&mut self) -> Accepted {
        self.settings.window.transfers_flipped = !self.settings.window.transfers_flipped;
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::PaneFocus,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    /// The person dragged the splitter to `height`, which the layout
    /// clamped.
    pub(crate) fn set_inspector_height(&mut self, height: Dip) -> Accepted {
        let window = &mut self.settings.window;
        if window.inspector_height == height {
            return Accepted::default();
        }
        window.inspector_height = height;
        Accepted {
            touched: Areas::TITLE
                | Areas::SEARCH
                | Areas::INSPECTOR
                | Areas::STATUS
                | Areas::LEFT_PANEL
                | Areas::RIGHT_PANEL
                | Areas::KEY_BAR,
            effects: vec![
                Effect::Layout,
                Effect::PaneFocus,
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    /// Shows or hides `column` (DESIGN.md: C15). A shown column joins at the
    /// end; Name cannot be hidden.
    pub(crate) fn toggle_column(&mut self, column: Column) -> Accepted {
        if !column.row().can_hide {
            return Accepted::default();
        }
        match self.columns.iter().position(|c| *c == column) {
            Some(index) => {
                self.columns.remove(index);
                (self.settings.window.columns).retain(|saved| saved.column != column);
                Accepted {
                    touched: Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                    effects: vec![
                        Effect::HideColumn { index, column },
                        Effect::InspectAge,
                        Effect::Polling,
                        Effect::NoteSavedState,
                    ],
                }
            }
            None => {
                self.columns.push(column);
                // The list shows a column it inserts last at its end.
                (self.settings.window.columns).push(SavedColumn { column });
                Accepted {
                    touched: Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                    effects: vec![
                        Effect::ShowColumn {
                            index: self.columns.len() - 1,
                            column,
                        },
                        Effect::InspectAge,
                        Effect::Polling,
                        Effect::NoteSavedState,
                    ],
                }
            }
        }
    }

    /// The search that `list`'s typed text and controls ask for, or the
    /// message that says why it cannot run, which starts no request. In
    /// Search: the text does not parse, `scope` says the Location folder is
    /// missing, or the search is empty and the settings say to show nothing
    /// (DESIGN.md: S6). The shown rows and the selection stay until the new
    /// search's first rows arrive. A search panel's search is in
    /// [`Model::start_panel_search`].
    pub(crate) fn start_search(
        &mut self,
        list: List,
        query_start: &QueryStart,
        scope: ScopeCheck,
    ) -> Accepted {
        let List::Panel(side) = list else {
            return self.start_search_list_search(query_start, scope);
        };
        self.start_panel_search(side, query_start)
    }

    fn start_search_list_search(
        &mut self,
        query_start: &QueryStart,
        scope: ScopeCheck,
    ) -> Accepted {
        self.search.due = false;
        let text = &self.search.list.text;
        let search = query::search(text, self.matching(), self.filters(), query_start);
        let missing = self
            .location()
            .filter(|_| scope == ScopeCheck::Missing)
            .map(str::to_owned);
        let shows_nothing = !self.choices().show_all_when_empty
            && text.trim().is_empty()
            && !self.filters().is_set();
        let message = match (search, missing) {
            (Err(error), _) => ListMessage::Parse(error),
            (Ok(_), Some(folder)) => ListMessage::MissingFolder(folder),
            (Ok(_), None) if shows_nothing => ListMessage::TypeToSearch,
            (Ok(search), None) => {
                let generation = self.generation();
                let sort = self.sort();
                let ask = self
                    .search
                    .list
                    .results
                    .start_search(generation, search, sort);
                return {
                    let mut accepted = Accepted {
                        touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                        effects: vec![Effect::InspectAge],
                    };
                    accepted.effects.extend(Some(Effect::Fetch(Fetch {
                        generation,
                        ask: Ask::Rows(List::Search, ask),
                    })));
                    accepted.effects.push(Effect::NameSearchSoon(List::Search));

                    accepted
                        .effects
                        .extend([Effect::Polling, Effect::NoteSavedState]);
                    accepted
                };
            }
        };
        self.search.enter_on_answer = false;
        self.show_message(List::Search, message)
    }

    /// Names `list`'s search as running in the status bar, once it has run
    /// long enough ([`super::results::Results::name_search`]).
    pub(crate) fn name_search(&mut self, list: List) -> Accepted {
        let named = (self.list_mut(list)).is_some_and(|shown| shown.results.name_search());
        if named {
            Accepted {
                touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge, Effect::Polling, Effect::NoteSavedState],
            }
        } else {
            Accepted::default()
        }
    }

    /// A search panel's search (DESIGN.md: Search in a panel): its typed text
    /// in Start of words, without Search's match controls, filters or sort, in
    /// the panel's folder and its subfolders or on all drives, in the panel's
    /// sort. A folder that gozlerd does not index says so before anything is
    /// typed, and an empty text asks the person to type.
    fn start_panel_search(&mut self, side: Side, query_start: &QueryStart) -> Accepted {
        let list = List::Panel(side);
        let panel = self.panels.panel(side);
        let Some(search) = panel.search() else {
            return Accepted::default();
        };
        let folder = panel.folder();
        let filters = match search.look_in {
            LookIn::ThisFolder => Filters {
                location: Some(folder.to_owned()),
                ..Filters::default()
            },
            LookIn::AllDrives => Filters::default(),
        };
        let text = search.list.text();
        let message = if self.not_indexed(side) {
            ListMessage::NotIndexed {
                root: drive_root(folder),
            }
        } else if text.trim().is_empty() {
            ListMessage::TypeToSearch
        } else {
            match query::search(text, Matching::default(), &filters, query_start) {
                Err(error) => ListMessage::Parse(error),
                Ok(search) => {
                    let sort = panel.sort();
                    let generation = self.generation();
                    let shown = self.list_mut(list).expect("the panel searches");
                    let ask = shown.results.start_search(generation, search, sort);
                    return {
                        let mut accepted = Accepted {
                            touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                            effects: vec![Effect::InspectAge],
                        };
                        accepted.effects.extend(Some(Effect::Fetch(Fetch {
                            generation,
                            ask: Ask::Rows(list, ask),
                        })));
                        accepted.effects.push(Effect::NameSearchSoon(list));

                        accepted
                            .effects
                            .extend([Effect::Polling, Effect::NoteSavedState]);
                        accepted
                    };
                }
            }
        };
        self.show_message(list, message)
    }

    /// Whether `side` is a search panel of This folder and subfolders whose
    /// folder no volume of the latest status holds, so gozlerd cannot
    /// search it; `false` while no status is known.
    fn not_indexed(&self, side: Side) -> bool {
        let panel = self.panels.panel(side);
        self.status.is_some()
            && panel
                .search()
                .is_some_and(|search| search.look_in == LookIn::ThisFolder)
            && self.panel_folder(side).is_some_and(|folder| {
                Coverage::of(self.volumes(), Some(folder), false)
                    .volumes()
                    .next()
                    .is_none()
            })
    }

    /// Shows `message` in `list` instead of its rows, with nothing
    /// selected.
    fn show_message(&mut self, list: List, message: ListMessage) -> Accepted {
        let Some(shown) = self.list_mut(list) else {
            return Accepted::default();
        };
        shown.results.show_message(message);
        shown.abandon_keeping();
        shown.set_selection(Selection::None);
        shown.chosen_size = None;
        Accepted {
            touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
            effects: vec![
                Effect::ClearSelection(list),
                Effect::SetCount { list, count: 0 },
                Effect::Cancel(Purpose::Selection(list)),
                Effect::InspectAge,
                Effect::Polling,
                Effect::NoteSavedState,
            ],
        }
    }

    /// The chunk that `list`'s visible rows still lack, unless a request
    /// for rows in view is in flight ([`super::results::Results::next_chunk`]).
    pub(crate) fn rows_needed(&mut self, list: List, visible: Range<u64>) -> Option<Fetch> {
        let generation = self.generation();
        let ask = self
            .list_mut(list)?
            .results
            .next_chunk(visible, generation)?;
        Some(Fetch {
            generation,
            ask: Ask::Rows(list, ask),
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
            ask: Ask::Status {
                links_held: self.links.fingerprint(),
            },
        })
    }

    pub(crate) fn expire_request(&mut self, purpose: Purpose, generation: Generation) -> Accepted {
        match purpose {
            Purpose::Status if self.status_asked == Some(generation) => {
                self.status_asked = None;
                Accepted::default()
            }
            Purpose::FolderSizes(side) if self.panels.expire_sizes(side, generation) => Accepted {
                touched: Areas::panel(side) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![Effect::PreparePanel(side), Effect::FolderSizes(side)],
            },
            _ => Accepted::default(),
        }
    }

    pub(crate) fn cancel_results(&mut self, list: List) -> Accepted {
        if list == List::Search {
            self.search.enter_on_answer = false;
        }
        if let Some(shown) = self.list_mut(list) {
            shown.results.cancel();
        }
        {
            let mut accepted = Accepted {
                touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge],
            };

            accepted
                .effects
                .extend(Some(Effect::Cancel(Purpose::Results(list))));
            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// The positions `list` says are selected, sorted and apart, read once
    /// per user action, and its `focused` row. When the cache holds every
    /// one, the selection is those rows, the files the person sees;
    /// otherwise it is read (DESIGN.md: The selection). With no row
    /// selected, it is the focused row, which commands then act on
    /// (DESIGN.md: Selecting entries). More than [`SELECTION_LIMIT`] rows
    /// are not read, and no command acts on them.
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "a selection is a list of row ranges"
    )]
    pub(crate) fn set_selection(
        &mut self,
        list: List,
        selection: RowSelection,
        focused: Option<u64>,
    ) -> Accepted {
        let RowSelection {
            mut positions,
            mut count,
        } = selection;
        if count > SELECTION_LIMIT {
            return self.select_too_many(list, count);
        }
        let by_focus = match focused {
            Some(row) if positions.is_empty() && self.selectable(list, row) => {
                positions = Arc::new(vec![row..row + 1]);
                count = 1;
                true
            }
            _ => false,
        };
        let Some(shown) = self.list_mut(list) else {
            return Accepted::default();
        };
        let older = shown.selection.asked();
        shown.chosen_by_focus = by_focus;
        let effect = if positions.is_empty() || shown.results.shown_ask().is_none() {
            shown.set_selection(Selection::None);
            shown.chosen_size = None;
            None
        } else {
            let source = SelectionSource::Cached {
                chunks: shown.results.cached(),
                positions,
                count: count as usize,
            };
            self.filter_selection(list, source)
        };
        // The request for the older selection reads other files (DESIGN.md: T6,
        // T7); a new request of the purpose ends it too.
        let effect = effect.or_else(|| older.map(|_| Effect::Cancel(Purpose::Selection(list))));
        {
            let mut accepted = Accepted {
                touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge, Effect::ListPasses],
            };
            accepted.effects.extend(effect);

            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// `count` rows of `list` are selected, more than [`SELECTION_LIMIT`],
    /// which the window counts before it reads their positions, because
    /// reading a scattered selection of millions of rows takes seconds.
    pub(crate) fn select_too_many(&mut self, list: List, count: u64) -> Accepted {
        let Some(shown) = self.list_mut(list) else {
            return Accepted::default();
        };
        let older = shown.selection.asked();
        shown.chosen_by_focus = false;
        shown.set_selection(Selection::TooMany(count));
        shown.chosen_size = None;
        {
            let mut accepted = Accepted {
                touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge, Effect::ListPasses],
            };

            accepted
                .effects
                .extend(older.map(|_| Effect::Cancel(Purpose::Selection(list))));
            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// Selects `files` of `list`, which are read. In Search's list, one
    /// folder alone also asks for its contents, which the inspector shows
    /// (DESIGN.md: Inspector pane); a search panel adds up the selection's
    /// size for its status line instead.
    fn select(&mut self, list: List, files: Arc<[ResultItem]>) -> Option<Effect> {
        self.filter_selection(list, SelectionSource::Files(files))
    }

    fn filter_selection(&mut self, list: List, source: SelectionSource) -> Option<Effect> {
        let generation = self.generation();
        let shown = self.list_mut(list)?;
        shown.set_selection(Selection::Filtering {
            generation,
            source,
            running: false,
        });
        shown.chosen_size = None;
        self.inline_filter(list).and_then(|accepted| {
            accepted.effects.into_iter().find(|effect| {
                matches!(
                    effect,
                    Effect::Fetch(_) | Effect::ReadSelectionSoon(_) | Effect::Cancel(_)
                )
            })
        })
    }

    fn inline_filter(&mut self, list: List) -> Option<Accepted> {
        let shown = self.list(list)?;
        let Selection::Filtering { source, .. } = &shown.selection else {
            return None;
        };
        if shown.is_keeping() {
            return None;
        }
        match super::work::pass(source.items(), list, |list| {
            let input = self.take_selection_filter(list)?;
            Some(self.accept_filtered(list, input.run()))
        }) {
            super::work::Pass::Ready(accepted) => accepted,
            super::work::Pass::Deferred(_) => None,
        }
    }

    pub(crate) fn take_selection_filter(&mut self, list: List) -> Option<Filter> {
        let taken = Arc::clone(self.panels.taken());
        let shown = self.list_mut(list)?;
        if shown.is_keeping() {
            return None;
        }
        let Selection::Filtering {
            generation,
            source,
            running: false,
        } = &shown.selection
        else {
            return None;
        };
        let input = Filter {
            generation: *generation,
            source: source.clone(),
            taken,
            kept: Arc::clone(&shown.kept),
            count_bytes: matches!(list, List::Panel(_)),
        };
        if let Selection::Filtering { running, .. } = &mut shown.selection {
            *running = true;
        }
        Some(input)
    }

    pub(crate) fn accept_filtered(&mut self, list: List, answer: Filtered) -> Accepted {
        let current = self.list(list).is_some_and(|shown| {
            matches!(
                shown.selection,
                Selection::Filtering { generation, .. } if generation == answer.input.generation
            )
        });
        let valid = current
            && self.list(list).is_some_and(|shown| {
                !shown.is_keeping()
                    && Arc::ptr_eq(&shown.kept, &answer.input.kept)
                    && Arc::ptr_eq(self.panels.taken(), &answer.input.taken)
            });
        if !valid {
            let items = answer.input.retained_items();
            self.retired.push(answer, items);
            if current {
                if let Some(shown) = self.list_mut(list)
                    && let Selection::Filtering { running, .. } = &mut shown.selection
                {
                    *running = false;
                }
                return Accepted {
                    touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                    effects: vec![
                        Effect::InspectAge,
                        Effect::Polling,
                        Effect::NoteSavedState,
                        Effect::ListPasses,
                    ],
                };
            }
            return Accepted::default();
        }
        let Filtered {
            input,
            files,
            chosen_size,
            sums,
        } = answer;
        let count = input.source.items();
        let positions = match &input.source {
            SelectionSource::Cached { positions, .. } => Some(Arc::clone(positions)),
            SelectionSource::Files(_) => None,
        };
        let items = input.retained_items();
        self.retired.push(input, items);
        let Some(files) = files else {
            let shown = self.list_mut(list).expect("the filter is current");
            shown.set_selection(Selection::Reading {
                positions: positions.expect("only cached input can lack rows"),
                asked: None,
            });
            return {
                let mut accepted = Accepted {
                    touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                    effects: vec![Effect::InspectAge, Effect::ListPasses],
                };

                accepted
                    .effects
                    .extend(Some(Effect::ReadSelectionSoon(list)));
                accepted
                    .effects
                    .extend([Effect::Polling, Effect::NoteSavedState]);
                accepted
            };
        };
        let deselect = if files.len() == count {
            Deselect::Nothing
        } else if files.is_empty() {
            Deselect::All
        } else {
            Deselect::Taken
        };
        let effect = self.adopt_selection(list, files, chosen_size, sums);
        {
            let mut accepted = Accepted {
                touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::Deselect {
                        list,
                        rows: deselect,
                    },
                    Effect::ListPasses,
                    Effect::InspectAge,
                ],
            };
            accepted.effects.extend(effect);

            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    fn refilter_selection(&mut self, list: List) -> bool {
        let Some(shown) = self.list(list) else {
            return false;
        };
        let source = match &shown.selection {
            Selection::Files(_) if !shown.is_keeping() && self.panels.taken().is_empty() => {
                return false;
            }
            Selection::Files(files) => SelectionSource::Files(Arc::clone(files)),
            Selection::Folder { folder, .. }
                if !shown.is_keeping() && !shown.takes(self.panels.taken(), folder) =>
            {
                return false;
            }
            Selection::Folder { folder, .. } => SelectionSource::Files(Arc::from([folder.clone()])),
            Selection::Filtering { source, .. } => source.clone(),
            _ => return false,
        };
        let generation = self.generation();
        let shown = self.list_mut(list).expect("the selection's list exists");
        shown.set_selection(Selection::Filtering {
            generation,
            source,
            running: false,
        });
        shown.chosen_size = None;
        true
    }

    fn adopt_selection(
        &mut self,
        list: List,
        files: Arc<[ResultItem]>,
        chosen_size: Option<SelectedSize>,
        sums: Sums,
    ) -> Option<Effect> {
        let shown = self.list_mut(list)?;
        if files.is_empty() {
            shown.set_selection(Selection::None);
            shown.chosen_size = None;
            return None;
        }
        let folder = match (list, files.as_ref()) {
            (List::Search, [entry]) if entry.is_dir => entry.clone(),
            (List::Search, _) => {
                shown.set_selection(Selection::Files(files));
                shown.selection_sums = sums;
                return None;
            }
            (List::Panel(_), _) => {
                shown.set_selection(Selection::Files(files));
                shown.selection_sums = sums;
                shown.chosen_size = chosen_size;
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
        self.search.list.set_selection(Selection::Folder {
            folder,
            contents: FolderContents::Calculating(generation),
        });
        self.search.list.selection_sums = sums;
        Some(Effect::Fetch(Fetch {
            generation,
            ask: Ask::FolderSize(search),
        }))
    }

    /// The request for `list`'s selected rows, if it waits for one.
    pub(crate) fn read_selection(&mut self, list: List) -> Option<Fetch> {
        let shown = self.list(list)?;
        let Selection::Reading {
            positions,
            asked: None,
        } = &shown.selection
        else {
            return None;
        };
        let ask = shown.results.selection_ask(positions.clone())?;
        let generation = self.generation();
        if let Some(shown) = self.list_mut(list)
            && let Selection::Reading { asked, .. } = &mut shown.selection
        {
            *asked = Some(generation);
        }
        Some(Fetch {
            generation,
            ask: Ask::Selection(list, ask),
        })
    }

    /// Whether Export results… can run: Search's list shows results and no
    /// export runs (DESIGN.md: C4).
    pub(crate) fn can_export(&self) -> bool {
        let results = self.search.list.results();
        results.shown_ask().is_some() && results.total() > 0 && !self.exporting()
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
        let Some(shown) = (self.search.list.results.shown_ask()).filter(|_| self.can_export())
        else {
            return Accepted::default();
        };
        let (search, sort) = (shown.search.clone(), shown.sort);
        let columns: Vec<EsColumn> = display_order
            .iter()
            .filter_map(|&index| self.columns.get(index)?.row().export_column)
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
        {
            let mut accepted = Accepted {
                touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge],
            };
            accepted.effects.extend(Some(Effect::Fetch(Fetch {
                generation,
                ask: Ask::Export(ExportAsk {
                    search,
                    sort,
                    path,
                    want_attributes: plan.columns.contains(&EsColumn::Attributes),
                    want_created: plan.columns.contains(&EsColumn::DateCreated),
                    format: plan.result_format(),
                }),
            })));

            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// Ends the running export; its thread deletes the partial file (DESIGN.md:
    /// C5).
    pub(crate) fn cancel_export(&mut self) -> Accepted {
        if !self.exporting() {
            return Accepted::default();
        }
        self.export = Export::Idle;
        {
            let mut accepted = Accepted {
                touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge],
            };

            accepted
                .effects
                .extend(Some(Effect::Cancel(Purpose::Export)));
            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// Whether the window must ask for status every 2 seconds: the service
    /// is not running, not ready or lost, a volume in scope is indexing,
    /// Settings shows its Index page, or the window shows indexing or memory while
    /// the window is `on_screen`, not minimized or hidden. A refused
    /// request, a failed owner check, an older service and an offline or
    /// failed volume end only by the person's action, so they start no
    /// poll.
    pub(crate) fn polls_status(&self, on_screen: bool) -> bool {
        self.failure.as_ref().is_some_and(DaemonFailure::recovers)
            || (on_screen
                && self.failure.is_none()
                && self.volumes().iter().any(|volume| {
                    matches!(
                        volume.phase,
                        VolumePhase::Bootstrapping | VolumePhase::Rescanning
                    )
                }))
            || List::ALL
                .into_iter()
                .any(|list| self.indexing_in_scope(list))
            || self.settings_page == Some(SettingsPage::Index)
            || self.fix.waits_for_service()
            || (self.settings.choices.show_memory && on_screen)
    }

    /// The job the status bar's middle group shows: the newest running
    /// one, or the one started last when none runs (DESIGN.md: Status bar).
    pub(crate) fn job(&self) -> Job {
        let runs = |job: &Job| match job {
            Job::Export => self.exporting(),
            Job::Fix => self.fix.running(),
            Job::Operation => self
                .operation
                .as_ref()
                .is_some_and(StatusOperation::can_cancel),
        };
        let mut started = self.started_jobs.iter().rev();
        (started.clone().find(|(job, _)| runs(job)))
            .or_else(|| started.next())
            .copied()
            .map(|(job, _)| job)
            .unwrap_or(Job::Export)
    }

    fn job_started(&mut self, job: Job) {
        let moment = self.next_job_moment();
        self.job_started_at(job, moment);
    }

    fn next_job_moment(&mut self) -> u64 {
        self.job_moment += 1;
        self.job_moment
    }

    fn job_started_at(&mut self, job: Job, moment: u64) {
        self.started_jobs.retain(|(started, _)| *started != job);
        self.started_jobs.push((job, moment));
        self.started_jobs
            .sort_unstable_by_key(|(_, moment)| *moment);
    }

    /// The file operation the middle group shows ([`StatusOperation`]).
    pub(crate) fn operation(&self) -> Option<&StatusOperation> {
        self.operation.as_ref()
    }

    #[cfg(test)]
    fn set_operation(&mut self, operation: Option<StatusOperation>) {
        let moment = self.next_job_moment();
        self.set_operation_at(operation, moment);
    }

    // Order by receipt, so a delayed calculation cannot outrank a newer job.
    fn set_operation_at(&mut self, operation: Option<StatusOperation>, moment: u64) {
        let id = |operation: &Option<StatusOperation>| operation.as_ref().map(StatusOperation::id);
        if id(&operation).is_some() && id(&operation) != id(&self.operation) {
            self.job_started_at(Job::Operation, moment);
        }
        self.operation = operation;
    }

    /// Whether File > Cancel, and the status bar's Cancel, can stop the job
    /// that [`Self::job`] says the status bar shows.
    pub(crate) fn can_cancel_job(&self) -> bool {
        match self.job() {
            Job::Export => self.exporting(),
            Job::Fix => self.fix.can_cancel(),
            Job::Operation => self
                .operation
                .as_ref()
                .is_some_and(StatusOperation::can_cancel),
        }
    }

    /// Stops the job that the status bar shows, if it can stop.
    pub(crate) fn cancel_job(&mut self) -> Accepted {
        match self.job() {
            Job::Export => self.cancel_export(),
            Job::Fix => self.cancel_fix(),
            Job::Operation => match &self.operation {
                Some(operation) if operation.can_cancel() => {
                    let mut accepted = Accepted::default();
                    accepted
                        .effects
                        .extend(Some(Effect::CancelOperation(operation.id())));
                    accepted
                }
                _ => Accepted::default(),
            },
        }
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
            return Accepted::default();
        };
        self.job_started(Job::Fix);
        let generation = self.generation();
        self.fix_generation = Some(generation);
        {
            let mut accepted = Accepted {
                touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::PostSettings, Effect::InspectAge],
            };
            accepted
                .effects
                .extend(Some(Effect::RunFix { generation, work }));

            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// The person answered whether to download the setup the fix found;
    /// a fix they declined ends without a word in the status bar.
    pub(crate) fn answer_setup(&mut self, download: bool) -> Accepted {
        let setup = self.fix.answer(download);
        let effect = setup.map(|setup| {
            let generation = self.generation();
            self.fix_generation = Some(generation);
            Effect::RunFix {
                generation,
                work: FixWork::Download(setup),
            }
        });
        {
            let mut accepted = Accepted {
                touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::PostSettings, Effect::InspectAge],
            };

            accepted.effects.extend(effect);

            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// The fix's thread reported `event`. Once the commands end well, the
    /// window asks the service for its status until it answers; once the
    /// downloaded setup is checked, the window starts it.
    pub(crate) fn accept_fix(&mut self, generation: Generation, event: FixEvent) -> Accepted {
        if !self.awaits_fix(generation) {
            self.retired.push(event, 1);
            return Accepted::default();
        }
        if !self.fix.accept(event) {
            return Accepted::default();
        }
        let effect = if let Some(setup) = self.fix.starting_setup() {
            Some(Effect::RunSetup(setup.to_owned()))
        } else if self.fix.confirming().is_some() {
            Some(Effect::ConfirmSetup)
        } else if self.fix.found_no_setup() {
            Some(Effect::ExplainNoSetup)
        } else if self.fix.waits_for_service() {
            self.status_needed().map(Effect::Fetch)
        } else {
            None
        };
        {
            let mut accepted = Accepted {
                touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::PostSettings, Effect::InspectAge],
            };
            accepted.effects.extend(effect);

            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    pub(crate) fn setup_failed(&mut self, error: String) -> Accepted {
        let Some(generation) = self
            .fix_generation
            .filter(|_| self.fix.starting_setup().is_some())
        else {
            return Accepted::default();
        };
        self.accept_fix(
            generation,
            FixEvent::Setup(super::fixer::SetupEvent::End(
                super::fixer::SetupResult::Failed(error),
            )),
        )
    }

    /// Stops the fix, in a step that can stop.
    pub(crate) fn cancel_fix(&mut self) -> Accepted {
        if !self.fix.cancel() {
            return Accepted::default();
        }
        {
            let mut accepted = Accepted {
                touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::PostSettings, Effect::InspectAge],
            };
            accepted.effects.extend(Some(Effect::StopFix));

            accepted
                .effects
                .extend([Effect::Polling, Effect::NoteSavedState]);
            accepted
        }
    }

    /// What the latest status says about the volumes that `list`'s shown
    /// search reaches.
    pub(crate) fn coverage(&self, list: List) -> Coverage<'_> {
        let volumes = self.volumes();
        let shown = self
            .list(list)
            .and_then(|shown| shown.results.shown_search());
        let scope = shown.and_then(|search| search.scope.as_deref());
        let scope = if self.links.is_empty() {
            scope
        } else {
            self.list(list)
                .and_then(|shown| shown.results.coverage_scope(self.links.fingerprint()))
        };
        Coverage::of(volumes, scope, self.reads_size_or_date(list))
    }

    fn reads_size_or_date(&self, list: List) -> bool {
        (self
            .list(list)
            .and_then(|shown| shown.results.shown_search()))
        .is_some_and(|search| search.query.tests_size_or_date())
    }

    /// Whether a volume that `list`'s shown search reaches completes by
    /// itself once it is read.
    fn indexing_in_scope(&self, list: List) -> bool {
        let searched = self
            .list(list)
            .and_then(|shown| shown.results.shown_search());
        searched.is_some()
            && self.coverage(list).volumes().any(|v| {
                matches!(
                    v.phase,
                    VolumePhase::Bootstrapping | VolumePhase::Rescanning
                )
            })
    }

    /// Whether `list` shows a failure that ends by itself and is not
    /// searching again yet, so it searches again once the service answers.
    fn shows_recovering_failure(&self, list: List) -> bool {
        self.list(list).is_some_and(|shown| {
            !shown.results.searching()
                && matches!(
                    shown.results.shown(),
                    Shown::Message(ListMessage::Failure(failure)) if failure.recovers()
                )
        })
    }

    /// Applies a reply; a reply of an older generation changes nothing.
    pub(crate) fn accept(&mut self, fetched: Fetched) -> Accepted {
        let generation = fetched.generation;
        if !self.awaits(fetched.purpose, generation) {
            let items = fetched.outcome.items();
            self.retired.push(fetched, items);
            return Accepted::default();
        }
        match fetched.purpose {
            Purpose::Results(list) => self.accept_results(list, generation, fetched.outcome),
            Purpose::Status => self.accept_status(fetched.outcome),
            Purpose::Selection(list) => self.accept_selection(list, fetched.outcome),
            Purpose::Export => self.accept_export(fetched.outcome),
            Purpose::Program => {
                self.program_asked = None;
                Accepted {
                    touched: Areas::NONE,
                    effects: vec![Effect::TotalCommander(match fetched.outcome {
                        Outcome::Rows(chunk) => {
                            let found = newest_total_commander(&chunk.rows).map(str::to_owned);
                            let items = chunk.rows.len();
                            self.retired.push(chunk, items);
                            Ok(found)
                        }
                        Outcome::Failed(failure) => Err(failure),
                        other => {
                            unreachable!("a program search's reply is rows or a failure: {other:?}")
                        }
                    })],
                }
            }
            Purpose::IncludeCount => {
                self.include_count_asked = None;
                self.included_count = Some(match fetched.outcome {
                    Outcome::IncludedCount(entries) => Ok(entries),
                    Outcome::Failed(failure) => Err(failure),
                    other => unreachable!("a count's reply is a count or a failure: {other:?}"),
                });
                Accepted {
                    touched: Areas::NONE,
                    effects: vec![Effect::PostSettings],
                }
            }
            Purpose::SettingsSaved => {
                self.settings_saved_asked = None;
                // A service that is not running reads the settings when it
                // starts, so a failure needs nothing. Once applied, the
                // drives' states change, which status shows.
                {
                    let mut accepted = Accepted::default();
                    accepted
                        .effects
                        .extend(self.status_needed().map(Effect::Fetch));
                    accepted
                }
            }
            Purpose::FolderSizes(side) => {
                if self.panels.accept_sizes(side, generation, fetched.outcome) {
                    Accepted {
                        touched: Areas::panel(side) | Areas::STATUS | Areas::KEY_BAR,
                        effects: vec![Effect::PreparePanel(side), Effect::FolderSizes(side)],
                    }
                } else {
                    Accepted::default()
                }
            }
        }
    }

    fn accept_results(&mut self, list: List, generation: Generation, outcome: Outcome) -> Accepted {
        match outcome {
            Outcome::Rows(chunk) => {
                self.failure = None;
                let flags = (chunk.volumes_incomplete, chunk.metadata_pending);
                let shown = self
                    .list_mut(list)
                    .expect("accept checked that the list waits");
                let mut accepted = match shown.results.accept(generation, chunk) {
                    RowsChange::Stale => Accepted::default(),
                    RowsChange::NewSearch { count } => {
                        // A new search's rows are other files (DESIGN.md: T2),
                        // and the disk as it is now.
                        shown.set_selection(Selection::None);
                        shown.chosen_size = None;
                        shown.reset_kept();
                        let waits = list == List::Search
                            && std::mem::take(&mut self.search.enter_on_answer);
                        {
                            let mut accepted = Accepted {
                                touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                                effects: vec![
                                    Effect::ClearSelection(list),
                                    Effect::SetCount { list, count },
                                    Effect::FirstRow { list, generation },
                                    Effect::Cancel(Purpose::Selection(list)),
                                    Effect::ReleaseOperations,
                                    Effect::FetchVisible(list),
                                    Effect::FillFirstRows(list),
                                    Effect::InspectAge,
                                ],
                            };
                            accepted.effects.push(Effect::MarkMatches(list));
                            if waits
                                && count > 0
                                && let Some(ask) = self
                                    .list(list)
                                    .and_then(|shown| shown.results().shown_ask())
                                    .cloned()
                            {
                                accepted.effects.push(Effect::EnterFirst(ask));
                            }

                            accepted
                        }
                    }
                    RowsChange::Rows { rows, count } => Accepted {
                        touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                        effects: vec![
                            Effect::Rows { list, rows, count },
                            Effect::FetchVisible(list),
                            Effect::FillFirstRows(list),
                            Effect::InspectAge,
                        ],
                    },
                };
                // A page's flags cover every volume in scope, and the status
                // names which. The scope is the shown search's, so it is read
                // after the rows are accepted. Another list that shows a
                // failure that ends by itself searches again when the status
                // says the service answers ([`Model::accept_status`]).
                let coverage = self.coverage(list);
                let unexplained = flags.0 && coverage.incomplete().is_none()
                    || leaves_matches_out(flags.1, self.reads_size_or_date(list))
                        && coverage.sizes_or_dates_unknown().is_none();
                let another_failed = (List::ALL.into_iter())
                    .any(|other| other != list && self.shows_recovering_failure(other));
                let effect = if unexplained || another_failed {
                    self.status_needed().map(Effect::Fetch)
                } else {
                    None
                };
                accepted.effects.extend(effect);
                if accepted.touched != Areas::NONE {
                    accepted
                        .effects
                        .extend([Effect::Polling, Effect::NoteSavedState]);
                }
                accepted
            }
            Outcome::ResultSnapshotGone => self.snapshot_gone(list),
            Outcome::Failed(failure) => self.show_failure(list, failure),
            other => unreachable!("a results request's reply is rows or a failure: {other:?}"),
        }
    }

    /// The service no longer keeps the result snapshot that `list` reads,
    /// so the list searches again, as `F5` does.
    fn snapshot_gone(&mut self, list: List) -> Accepted {
        let Some(shown) = self.list_mut(list) else {
            return Accepted::default();
        };
        shown.results.forget_snapshot();
        {
            let mut accepted = Accepted::default();
            accepted
                .effects
                .extend(Some(Effect::SearchAgain(vec![list])));
            accepted
        }
    }

    /// The service failed while `list` shows rows: the list shows the
    /// failure instead of the rows, nothing is selected, and the status
    /// poll runs the search again once a failure that ends by itself has
    /// ended (DESIGN.md: T3, T4).
    fn show_failure(&mut self, list: List, failure: DaemonFailure) -> Accepted {
        let effect = self.ask_latest_release(&failure);
        self.failure = Some(failure.clone());
        if list == List::Search {
            self.search.enter_on_answer = false;
        }
        {
            let mut accepted = self.show_message(list, ListMessage::Failure(failure));
            accepted.effects.extend(effect);
            accepted
        }
    }

    pub(crate) fn mark_matches(&self, list: List) -> Option<super::results::Mark> {
        let mark = self.list(list).and_then(|shown| {
            shown
                .results
                .mark(Arc::clone(&self.links), list == List::Search)
        });
        match list {
            List::Search => mark,
            List::Panel(side) => {
                let folder = self.panels.panel(side).folder().to_owned();
                Some(match mark {
                    Some(mark) => mark.with_folder(folder),
                    None => super::results::Mark::folder(Arc::clone(&self.links), folder),
                })
            }
        }
    }

    pub(crate) fn accept_marked(
        &mut self,
        list: List,
        mut answer: super::results::Marked,
    ) -> Accepted {
        let current = self.list(list).and_then(|shown| shown.results.generation());
        let stale_folder = answer.folder.as_ref().is_some_and(|(folder, _)| {
            !matches!(list, List::Panel(side) if self.panels.panel(side).folder() == folder)
        });
        if answer.links != self.links.fingerprint()
            || answer
                .search
                .as_ref()
                .is_some_and(|search| current != Some(search.generation))
            || stale_folder
        {
            self.retired.push(answer, super::work::INLINE_ITEMS + 1);
            return Accepted::default();
        }
        let mut accepted = Accepted {
            touched: Areas::STATUS | Areas::LOCATIONS,
            effects: vec![],
        };
        if let (List::Panel(side), Some((folder, real))) = (list, answer.folder.take()) {
            let was = self.not_indexed(side);
            self.panel_folders[side.index()] = Some((answer.links, folder, real));
            if self.awaits_index(side) && was != self.not_indexed(side) {
                accepted.effects.push(Effect::SearchAgain(vec![list]));
            }
        }
        if let Some(search) = answer.search.take() {
            let results = &mut self
                .list_mut(list)
                .expect("a shown list accepts its marks")
                .results;
            results.accept_mark(answer.links, search);
            accepted.effects.push(Effect::Redraw {
                list,
                rows: 0..results.count(),
            });
        }
        accepted.effects.push(Effect::Polling);
        accepted
    }

    fn accept_status(&mut self, outcome: Outcome) -> Accepted {
        self.status_asked = None;
        match outcome {
            Outcome::Status(mut status, version, programs) => {
                let redraw = status.links.take().map(|links| {
                    let previous = std::mem::replace(&mut self.links, Arc::new(links));
                    let items = previous.as_slice().len();
                    self.retired.push(previous, items);
                    List::ALL.map(Effect::MarkMatches)
                });
                // Each list's own message says whether it failed, because a
                // list whose rows arrived cleared the shared failure while
                // another list still shows one.
                let failed: Vec<List> = (List::ALL.into_iter())
                    .filter(|list| self.shows_recovering_failure(*list))
                    .collect();
                let was_indexing: Vec<List> = (List::ALL.into_iter())
                    .filter(|list| self.indexing_in_scope(*list))
                    .collect();
                let not_indexed: Vec<(Side, bool)> = (Side::BOTH.into_iter())
                    .map(|side| (side, self.not_indexed(side)))
                    .collect();
                self.failure = None;
                self.status = Some(status);
                self.service_version = Some(version);
                self.programs = programs;
                // The fix's end shows with this status.
                self.fix.service_answered();
                let completed =
                    (was_indexing.into_iter()).filter(|list| !self.indexing_in_scope(*list));
                let indexed_now = (not_indexed.into_iter())
                    .filter(|&(side, was)| self.awaits_index(side) && was != self.not_indexed(side))
                    .map(|(side, _)| List::Panel(side));
                let mut lists: Vec<List> = Vec::new();
                for list in failed.into_iter().chain(completed).chain(indexed_now) {
                    if !lists.contains(&list) {
                        lists.push(list);
                    }
                }
                {
                    let mut accepted = Accepted {
                        touched: Areas::STATUS
                            | Areas::LOCATIONS
                            | Areas::SEARCH
                            | Areas::INSPECTOR
                            | Areas::LEFT_PANEL
                            | Areas::RIGHT_PANEL
                            | Areas::KEY_BAR,
                        effects: vec![
                            Effect::PreparePanel(Side::Left),
                            Effect::PreparePanel(Side::Right),
                            Effect::ReadMemory,
                            Effect::InspectAge,
                        ],
                    };

                    accepted
                        .effects
                        .extend((!lists.is_empty()).then_some(Effect::SearchAgain(lists)));
                    accepted.effects.extend(redraw.into_iter().flatten());
                    accepted
                        .effects
                        .extend([Effect::Polling, Effect::NoteSavedState]);
                    accepted
                }
            }
            Outcome::Failed(failure) => {
                // A failure a list shows follows the service, so its
                // message and button never offer the fix for a state the
                // service has left. A search in flight shows its own reply.
                for list in List::ALL {
                    let Some(shown) = self.list_mut(list) else {
                        continue;
                    };
                    let shows_another = matches!(
                        shown.results.shown(),
                        Shown::Message(ListMessage::Failure(shown)) if *shown != failure
                    );
                    if shows_another && !shown.results.searching() {
                        (shown.results).show_message(ListMessage::Failure(failure.clone()));
                    }
                }
                let effect = self.ask_latest_release(&failure);
                self.failure = Some(failure);
                {
                    let mut accepted = Accepted {
                        touched: Areas::STATUS
                            | Areas::LOCATIONS
                            | Areas::SEARCH
                            | Areas::INSPECTOR
                            | Areas::LEFT_PANEL
                            | Areas::RIGHT_PANEL
                            | Areas::KEY_BAR,
                        effects: vec![
                            Effect::PreparePanel(Side::Left),
                            Effect::PreparePanel(Side::Right),
                            Effect::ReadMemory,
                            Effect::InspectAge,
                        ],
                    };
                    accepted.effects.extend(effect);

                    accepted
                        .effects
                        .extend([Effect::Polling, Effect::NoteSavedState]);
                    accepted
                }
            }
            other => unreachable!("a status request's reply is status or a failure: {other:?}"),
        }
    }

    /// Whether `side` is a search panel of This folder and subfolders that
    /// shows a state a change of the index can change: "Type a name…",
    /// "not indexed", or no results. A status reply checks it again, and
    /// searches again only when the folder's index state changed, so a
    /// poll every 2 seconds starts no request while nothing changes.
    fn awaits_index(&self, side: Side) -> bool {
        let Some(search) = self.panels.search(side) else {
            return false;
        };
        let results = search.list.results();
        search.look_in == LookIn::ThisFolder
            && match results.shown() {
                Shown::Message(ListMessage::TypeToSearch | ListMessage::NotIndexed { .. }) => true,
                Shown::Rows(_) => results.total() == 0,
                Shown::Nothing | Shown::Message(_) => false,
            }
    }

    fn accept_selection(&mut self, list: List, outcome: Outcome) -> Accepted {
        let shown = self
            .list_mut(list)
            .expect("accept checked that the list waits");
        match outcome {
            Outcome::Selection(reply) if matches!(shown.selection, Selection::Reading { .. }) => {
                shown.set_selection(Selection::None);
                let adopted = reply
                    .made
                    .is_some_and(|id| shown.results.adopt_snapshot(id));
                let rows = shown.results.replace_cached(reply.cached);
                // The rows cached before are gone, so every row draws again.
                let rows = if adopted {
                    0..shown.results.count()
                } else {
                    rows
                };
                let effect = if reply.files.is_empty() {
                    None
                } else {
                    self.select(list, reply.files)
                };
                {
                    let mut accepted = Accepted {
                        touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                        effects: vec![
                            Effect::Redraw { list, rows },
                            Effect::FetchVisible(list),
                            Effect::ListPasses,
                            Effect::InspectAge,
                        ],
                    };
                    accepted.effects.extend(effect);

                    accepted
                        .effects
                        .extend([Effect::Polling, Effect::NoteSavedState]);
                    accepted
                }
            }
            Outcome::Totals(totals) if matches!(shown.selection, Selection::Folder { .. }) => {
                if let Selection::Folder { contents, .. } = &mut shown.selection {
                    *contents = FolderContents::Counted(totals);
                }
                Accepted {
                    touched: Areas::INSPECTOR,
                    effects: vec![Effect::InspectAge],
                }
            }
            Outcome::ResultSnapshotGone => {
                shown.set_selection(Selection::None);
                self.snapshot_gone(list)
            }
            Outcome::Failed(failure) if !shown.results.searching() => {
                shown.set_selection(Selection::None);
                self.show_failure(list, failure)
            }
            // A search in flight shows its own reply, which clears the
            // list's selection.
            Outcome::Failed(_) => {
                shown.set_selection(Selection::None);
                Accepted {
                    touched: Areas::list(list) | Areas::STATUS | Areas::KEY_BAR,
                    effects: vec![
                        Effect::InspectAge,
                        Effect::Polling,
                        Effect::NoteSavedState,
                        Effect::ListPasses,
                    ],
                }
            }
            other => unreachable!(
                "a selection request's reply is its rows, its folder's totals or a failure: \
                 {:?}, {other:?}",
                shown.selection
            ),
        }
    }

    fn accept_export(&mut self, outcome: Outcome) -> Accepted {
        let Export::Running { path, rows, .. } = &mut self.export else {
            unreachable!("accept checked that an export runs")
        };
        let failed = |path: &PathBuf, error| Accepted {
            touched: Areas::STATUS,
            effects: vec![Effect::ExportFailed {
                path: path.clone(),
                error,
            }],
        };
        let accepted = match outcome {
            Outcome::ExportProgress { rows: written } => {
                *rows = written;
                return Accepted {
                    touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                    effects: vec![Effect::InspectAge, Effect::Polling, Effect::NoteSavedState],
                };
            }
            Outcome::Exported { rows } => {
                let path = std::mem::take(path);
                self.export = Export::Done { path, rows };
                return Accepted {
                    touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                    effects: vec![Effect::InspectAge, Effect::Polling, Effect::NoteSavedState],
                };
            }
            Outcome::ExportFailed(failure) => failed(path, ExportError::Write(failure)),
            Outcome::Failed(failure) => failed(path, ExportError::Daemon(failure)),
            other => unreachable!("an export's reply is progress, its end or a failure: {other:?}"),
        };
        self.export = Export::Idle;
        accepted
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
        let key = column.row().sort_key?;
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

    /// The person dragged a header: `order` is the list's display order,
    /// as indexes into [`Model::columns`].
    pub(crate) fn set_column_order(&mut self, order: &[usize]) {
        self.settings.window.columns = (order.iter())
            .filter_map(|&index| self.columns.get(index))
            .map(|&column| SavedColumn { column })
            .collect();
    }

    pub(crate) fn observe_saved_state(&mut self) -> Accepted {
        Accepted {
            touched: Areas::NONE,
            effects: if self.saved_state_changed() {
                vec![Effect::SaveSoon]
            } else {
                Vec::new()
            },
        }
    }

    /// Records the state that `ui.json` saves and answers whether it
    /// changed since the last call, so the window starts its save timer
    /// once for each change (DESIGN.md: Where settings are saved). The
    /// panels are compared in place, so nothing is copied unless they
    /// changed.
    pub(crate) fn saved_state_changed(&mut self) -> bool {
        if !self.panels.saved_as(&self.settings.window.panels) {
            self.settings.window.panels = self.panels.saved();
        }
        if self.settings == self.seen {
            return false;
        }
        self.seen.clone_from(&self.settings);
        true
    }

    /// Advances the attempted baseline when changes are queued, including on a
    /// failed save, so unrelated refreshes never retry a failing disk.
    pub(crate) fn settings_changes(&mut self, scope: SaveScope) -> Option<Changes> {
        self.saved_state_changed();
        let settings = match scope {
            SaveScope::Everything => self.settings.clone(),
            SaveScope::Choices => Settings {
                choices: self.settings.choices.clone(),
                window: self.saved.window.clone(),
            },
        };
        if settings == self.saved {
            return None;
        }
        let changes = settings.changes(&self.saved);
        self.saved = settings;
        Some(changes)
    }
}

/// The parts of `ranges` inside `span`, from `span`'s start.
fn within(ranges: &[Range<usize>], span: &Range<usize>) -> Vec<Range<usize>> {
    ranges
        .iter()
        .filter_map(|range| {
            let start = range.start.max(span.start);
            let end = range.end.min(span.end);
            (start < end).then(|| start - span.start..end - span.start)
        })
        .collect()
}

/// What a save writes: everything this window changed, or, from another
/// window, only what its Settings and its Language menu apply, so closing
/// it never undoes a setting that another window saved (DESIGN.md: Window
/// lifetime).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveScope {
    Everything,
    Choices,
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
    DateCreated,
    Extension,
    Attributes,
}

impl Column {
    /// In View > Columns' order.
    pub(crate) const ALL: [Column; 7] = [
        Column::Name,
        Column::Path,
        Column::Size,
        Column::DateModified,
        Column::DateCreated,
        Column::Extension,
        Column::Attributes,
    ];

    /// Its facts, in one match, so a new column chooses each on purpose.
    pub(crate) fn row(self) -> ColumnRow {
        match self {
            Column::Name => ColumnRow {
                header: "header.name",
                short_header: None,
                menu_label: "column.name",
                shown_by_default: true,
                // The list would show rows that name nothing (DESIGN.md:
                // C15).
                can_hide: false,
                right_aligned: false,
                can_shrink: true,
                can_grow: true,
                sort_key: Some(SortKey::Name),
                export_column: None,
            },
            Column::Path => ColumnRow {
                header: "header.path",
                short_header: None,
                menu_label: "column.path",
                shown_by_default: true,
                can_hide: true,
                right_aligned: false,
                can_shrink: true,
                can_grow: true,
                sort_key: Some(SortKey::Path),
                export_column: None,
            },
            Column::Size => ColumnRow {
                header: "header.size",
                short_header: None,
                menu_label: "column.size",
                shown_by_default: true,
                can_hide: true,
                // So the digits line up.
                right_aligned: true,
                can_shrink: false,
                can_grow: false,
                sort_key: Some(SortKey::Size),
                export_column: Some(EsColumn::Size),
            },
            Column::DateModified => ColumnRow {
                header: "header.date_modified",
                short_header: Some("header.modified"),
                menu_label: "column.date_modified",
                shown_by_default: true,
                can_hide: true,
                right_aligned: false,
                can_shrink: false,
                can_grow: false,
                sort_key: Some(SortKey::DateModified),
                export_column: Some(EsColumn::DateModified),
            },
            Column::DateCreated => ColumnRow {
                header: "header.date_created",
                short_header: Some("header.created"),
                menu_label: "column.date_created",
                // Its index setting, Dates created, is off by default.
                shown_by_default: false,
                can_hide: true,
                right_aligned: false,
                can_shrink: false,
                can_grow: false,
                sort_key: Some(SortKey::DateCreated),
                export_column: Some(EsColumn::DateCreated),
            },
            Column::Extension => ColumnRow {
                header: "header.extension",
                short_header: Some("panel.ext"),
                menu_label: "column.extension",
                shown_by_default: false,
                can_hide: true,
                right_aligned: false,
                can_shrink: true,
                can_grow: true,
                sort_key: Some(SortKey::Extension),
                export_column: Some(EsColumn::Extension),
            },
            Column::Attributes => ColumnRow {
                header: "header.attributes",
                short_header: None,
                menu_label: "column.attributes",
                shown_by_default: false,
                can_hide: true,
                right_aligned: false,
                can_shrink: true,
                can_grow: true,
                // gozler has no attribute sort.
                sort_key: None,
                export_column: Some(EsColumn::Attributes),
            },
        }
    }
}

/// What the window knows of a [`Column`].
pub(crate) struct ColumnRow {
    pub(crate) header: &'static str,
    pub(crate) short_header: Option<&'static str>,
    /// Its item in View > Columns.
    pub(crate) menu_label: &'static str,
    pub(crate) shown_by_default: bool,
    pub(crate) can_hide: bool,
    pub(crate) right_aligned: bool,
    /// A fill may make it narrower than its widest cell. Size and the dates
    /// say no, because part of the value is a wrong value (DESIGN.md:
    /// Results list).
    pub(crate) can_shrink: bool,
    /// A fill may make it wider than its widest cell. Size and the dates
    /// say no, so the room the list has left goes to the columns whose text
    /// can use it.
    pub(crate) can_grow: bool,
    /// What a click on its header sorts by.
    pub(crate) sort_key: Option<SortKey>,
    /// Its column in an export; Name and Path are parts of the full path,
    /// which every export holds (DESIGN.md: Export).
    pub(crate) export_column: Option<EsColumn>,
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
    use crate::core::panel::ReadEvent;
    use crate::core::results::CHUNK_ROWS;
    use crate::core::results::tests::chunk;
    use crate::core::text::tests::{English, english};
    use gozler_client::NotRunning;
    use gozler_client::Repair;
    use gozler_core::escompat::{Command, parse_argv};
    use gozler_core::proto::{CallerSees, ResultSnapshotId, VolumeStatus};
    use gozler_fileops::{Request, State};
    use std::collections::HashSet;
    use std::io;

    /// The search list keeps an ended delete only when, at its end, the
    /// delete takes one of the list's fetched rows, and then until its next
    /// search; a delete elsewhere would otherwise stay in the queue until
    /// the next search (DESIGN.md: Taken entries). The
    /// snapshot of the taken entries may be older than the end, so it
    /// still has the delete as running, and a later end must not let go of
    /// the delete while the queue still holds it, or its rows show again.
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
        model.operations_ended(Arc::new(Ended {
            operations: vec![(ours, delete(r"C:\row1")), (other, delete(r"D:\elsewhere"))],
            in_queue: HashSet::from([ours, other]),
            taken: Arc::new(taken),
        }));
        assert!(model.keeps(ours));
        // Taken entries older than the queue, which do not have it as
        // ended yet, while another operation ends.
        let later = OperationId::for_tests(3);
        model.operations_ended(Arc::new(Ended {
            operations: vec![(later, delete(r"E:\later"))],
            in_queue: HashSet::from([ours, other, later]),
            taken: Arc::default(),
        }));
        assert!(model.keeps(ours));
        assert!(!model.keeps(other));
        let fetch = search(&mut model, "rows");
        model.accept(rows(&fetch, 3));
        assert!(!model.keeps(ours));
    }

    /// A row whose entry a delete took draws plain and struck through, so a
    /// fill that measured its matched part in bold would make the column a
    /// little too wide. It keeps its text, which the list strikes through.
    #[test]
    fn a_taken_row_keeps_its_text_without_bold_parts() {
        let mut model = model();
        let fetch = search(&mut model, "row");
        model.accept(rows(&fetch, 3));
        let marked = model.mark_matches(List::Search).unwrap().run();
        model.accept_marked(List::Search, marked);
        assert_eq!(model.matched_in_cell(1, Column::Name), [0..3]);
        let mut taken = Taken::default();
        taken.insert(OperationId::for_tests(1), Path::new(r"C:\row1"), false);
        model.set_taken(Arc::new(taken), [None, None]);
        assert!(model.matched_in_cell(1, Column::Name).is_empty());
        assert_eq!(model.taken_rows(List::Search, &[1..2]), [1]);
        let row = model.search_list().results().row(1).expect("a fetched row");
        assert_eq!(
            text::cell(
                model.strings(),
                &English,
                text::SizeFormat::Kb,
                &crate::core::dates::Dates::new(
                    crate::core::dates::DateFormat::Full,
                    jiff::Timestamp::UNIX_EPOCH.to_zoned(jiff::tz::TimeZone::UTC),
                ),
                Column::Name,
                row
            )
            .full,
            "row1"
        );
    }

    /// A matched part that crosses the end of a cell's part of the path is
    /// cut at it, and one that only touches it is left out.
    #[test]
    fn a_cell_keeps_only_the_matched_parts_inside_it() {
        assert_eq!(within(&[0..4, 5..9], &(3..7)), [0..1, 2..4]);
        assert!(within(&[0..3, 7..9], &(3..7)).is_empty());
    }

    /// A change that no `Accepted` reports, such as the active panel, was
    /// lost when the window ended without Exit; each change starts the save
    /// timer once, so a burst of changes saves a second after the last.
    #[test]
    fn each_change_of_the_saved_state_starts_one_save() {
        let mut model = model();
        assert!(!model.saved_state_changed());
        model.panels_mut().activate(Side::Right);
        assert!(model.saved_state_changed());
        assert!(!model.saved_state_changed());
        model.toggle_column(Column::Extension);
        assert!(model.saved_state_changed());
        let order = [3, 0, 1, 2, 4];
        model.set_column_order(&order);
        let saved: Vec<Column> = (model.settings.window.columns.iter())
            .map(|saved| saved.column)
            .collect();
        assert_eq!(
            saved,
            [order[0], order[1], order[2], order[3], order[4]].map(|i| model.columns[i])
        );
    }

    /// Another window saves only what its Settings apply, so closing it
    /// never undoes the placement or the panels the first window saved
    /// (DESIGN.md: Window lifetime).
    #[test]
    fn another_window_saves_only_its_choices() {
        let mut model = model();
        model.set_placement(Placement {
            left: 0,
            top: 0,
            right: 640,
            bottom: 480,
            maximized: true,
        });
        let choices = Choices {
            show_memory: true,
            ..model.choices().clone()
        };
        model.set_choices(choices);
        let changes = model
            .settings_changes(SaveScope::Choices)
            .expect("choices changed");
        let saved = Settings::parse(&changes.apply(None));
        assert!(saved.choices.show_memory);
        assert_eq!(saved.window.placement, None);
        assert!(model.settings_changes(SaveScope::Choices).is_none());
    }

    /// The list's message and its buttons' captions.
    fn message(model: &Model) -> Option<(String, Vec<String>)> {
        list_message(model).map(|message| {
            let buttons = message.buttons.into_iter();
            (message.text, buttons.map(|button| button.label).collect())
        })
    }

    pub(crate) fn model() -> Model {
        model_for(Instance::Another)
    }

    pub(crate) fn model_for(instance: Instance) -> Model {
        Model::new(english(), Settings::default(), instance)
    }

    #[test]
    fn an_expired_status_can_retry_without_abandoning_a_newer_request() {
        let mut model = model();
        let first = model.status_needed().unwrap();
        assert!(model.status_needed().is_none());
        model.expire_request(Purpose::Status, first.generation);
        let second = model
            .status_needed()
            .expect("expiry permits the next natural poll");
        model.expire_request(Purpose::Status, first.generation);
        assert!(model.status_needed().is_none());
        model.accept(status(&first, VolumePhase::Live));
        assert!(model.status_needed().is_none());
        model.accept(status(&second, VolumePhase::Live));
        assert!(model.status_needed().is_some());
    }

    #[test]
    fn stale_bold_from_another_search_or_links_is_rejected() {
        let mut model = showing(1);
        let old = model.mark_matches(List::Search).unwrap().run();
        let read = search(&mut model, "new");
        model.accept(rows(&read, 1));
        assert_eq!(model.accept_marked(List::Search, old), Accepted::default());

        let old = model.mark_matches(List::Search).unwrap().run();
        model.links = Arc::new(Links::new(vec![gozler_core::links::FolderLink {
            link: r"C:\alias".into(),
            target: r"D:\folder".into(),
        }]));
        assert_eq!(model.accept_marked(List::Search, old), Accepted::default());
        let current = model.mark_matches(List::Search).unwrap().run();
        assert_eq!(
            model.accept_marked(List::Search, current).effects,
            [
                Effect::Redraw {
                    list: List::Search,
                    rows: 0..1
                },
                Effect::Polling
            ]
        );
    }

    #[test]
    fn folder_coverage_through_links_reports_the_target_instead_of_the_source() {
        let mut model = model();
        let ask = model.status_needed().unwrap();
        let mut reply = status(&ask, VolumePhase::Live);
        if let Outcome::Status(status, _, _) = &mut reply.outcome {
            let mut target = status.volumes[0].clone();
            target.mounts = vec![r"D:\".into()];
            target.phase = VolumePhase::Offline;
            status.volumes.push(target);
            status.links = Some(Links::new(vec![gozler_core::links::FolderLink {
                link: r"C:\alias".into(),
                target: r"D:\data".into(),
            }]));
        }
        model.accept(reply);
        let side = Side::Left;
        crate::core::panel::tests::list(model.panels_mut(), r"C:\alias", Vec::new());
        let marked = model.mark_matches(List::Panel(side)).unwrap().run();
        model.accept_marked(List::Panel(side), marked);
        assert_eq!(
            model.panel_coverage().unwrap().incomplete().unwrap().mounts,
            [r"D:\"]
        );
        let target = &mut model.status.as_mut().unwrap().volumes[1];
        target.phase = VolumePhase::Live;
        target.metadata_pending = true;
        assert_eq!(
            model
                .panel_coverage()
                .unwrap()
                .sizes_or_dates_unknown()
                .unwrap()
                .mounts,
            [r"D:\"]
        );
        model.status.as_mut().unwrap().volumes.pop();
        panel_search(&mut model, r"C:\alias", "");
        assert!(model.not_indexed(side));
    }

    #[test]
    fn coverage_through_links_researches_when_the_target_finishes_indexing() {
        let mut model = model();
        let ask = model.status_needed().unwrap();
        let mut first = status(&ask, VolumePhase::Live);
        if let Outcome::Status(status, _, _) = &mut first.outcome {
            let mut target = status.volumes[0].clone();
            target.mounts = vec![r"D:\".into()];
            target.phase = VolumePhase::Bootstrapping;
            status.volumes.push(target);
            status.links = Some(Links::new(vec![gozler_core::links::FolderLink {
                link: r"C:\alias".into(),
                target: r"D:\data".into(),
            }]));
        }
        model.accept(first);
        let list = List::Panel(Side::Left);
        let read = fetch_of(panel_search(&mut model, r"C:\alias", "report"));
        model.accept(rows(&read, 1));
        let marked = model.mark_matches(list).unwrap().run();
        model.accept_marked(list, marked);
        assert_eq!(model.coverage(list).incomplete().unwrap().mounts, [r"D:\"]);
        let ask = model.status_needed().unwrap();
        let mut ready = status(&ask, VolumePhase::Live);
        if let Outcome::Status(status, _, _) = &mut ready.outcome {
            let mut target = status.volumes[0].clone();
            target.mounts = vec![r"D:\".into()];
            status.volumes.push(target);
        }
        assert!(
            model
                .accept(ready)
                .effects
                .contains(&Effect::SearchAgain(vec![list]))
        );
    }

    #[test]
    fn cancelling_a_search_rejects_its_late_answer_and_keeps_the_export() {
        let mut model = showing(10);
        let _export = export(&mut model);
        let read = search(&mut model, "pending");
        let accepted = model.cancel_results(List::Search);
        assert_eq!(
            accepted,
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::InspectAge,
                    Effect::Cancel(Purpose::Results(List::Search)),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(
            model.accept(rows(&read, 3)),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
        assert_eq!(model.search_list().results().total(), 10);
        assert!(model.exporting());
    }

    #[test]
    fn indexing_progress_keeps_polling_outside_search_scope_and_clears_when_ready() {
        use gozler_core::types::IndexProgress;
        let mut model = model();
        model.settings.choices.show_memory = false;
        let ask = model.status_needed().expect("no status request runs");
        let mut reply = status(&ask, VolumePhase::Bootstrapping);
        if let Outcome::Status(status, ..) = &mut reply.outcome {
            status.volumes[0].progress = Some(IndexProgress::Scanning {
                entries: 12000,
                percent: Some(62),
            });
            let mut second = status.volumes[0].clone();
            second.guid = "second".into();
            second.mounts = vec![r"D:\".into()];
            second.progress = Some(IndexProgress::Scanning {
                entries: 4300,
                percent: None,
            });
            status.volumes.push(second);
        }
        model.accept(reply);
        assert!(
            model.polls_status(true),
            "indexing is visible before any search"
        );
        assert_eq!(
            captions::StatusCaptions::new(&model, &English).indexing,
            [
                captions::IndexingCaption {
                    label: "Indexing C:".into(),
                    compact: "Indexing".into(),
                    value: "62%".into(),
                    progress: captions::JobProgress::Measured {
                        done: 62,
                        total: 100
                    },
                },
                captions::IndexingCaption {
                    label: "· D:".into(),
                    compact: "Indexing".into(),
                    value: "4,300 entries".into(),
                    progress: captions::JobProgress::Unmeasured,
                },
            ]
        );
        let status = model.status.as_mut().expect("the reply supplied status");
        status.volumes[0].progress = Some(IndexProgress::Preparing { entries: 15000 });
        status.volumes[1].phase = VolumePhase::Live;
        assert_eq!(
            captions::StatusCaptions::new(&model, &English).indexing,
            [captions::IndexingCaption {
                label: "Indexing C:".into(),
                compact: "Indexing".into(),
                value: "15,000 entries".into(),
                progress: captions::JobProgress::Unmeasured,
            }]
        );
        model.failure = Some(DaemonFailure::Refused {
            detail: "refused".into(),
        });
        assert!(
            captions::StatusCaptions::new(&model, &English)
                .indexing
                .is_empty()
        );
        assert!(
            !model.polls_status(true),
            "stale indexing status cannot restart a refused request"
        );
        model.failure = None;
        model.status.as_mut().expect("status remains").volumes[0].phase = VolumePhase::Live;
        assert!(
            captions::StatusCaptions::new(&model, &English)
                .indexing
                .is_empty()
        );
        assert!(!model.polls_status(true));
    }

    /// Types `text` and starts its search; returns the first chunk's fetch.
    pub(crate) fn search(model: &mut Model, text: &str) -> Fetch {
        model.set_typed_search(List::Search, text.to_owned());
        let start = QueryStart {
            now: "2026-03-04T12:00:00Z".parse().expect("an instant"),
            time_zone: jiff::tz::TimeZone::UTC,
        };
        fetch_of(model.start_search(List::Search, &start, ScopeCheck::Exists))
    }

    /// The reply to `fetch`: its first rows of a search with `total` matches.
    pub(crate) fn rows(fetch: &Fetch, total: u64) -> Fetched {
        let start = match &fetch.ask {
            Ask::Rows(_, ask) => ask.start,
            other => panic!("a rows fetch: {other:?}"),
        };
        Fetched {
            purpose: fetch.purpose(),
            generation: fetch.generation,
            outcome: Outcome::Rows(chunk(start, CHUNK_ROWS.min(total - start), total)),
        }
    }

    /// The left panel shows `folder` and searches it for `text`.
    fn panel_search(model: &mut Model, folder: &str, text: &str) -> Accepted {
        crate::core::panel::tests::list(model.panels_mut(), folder, Vec::new());
        model.panels_mut().enter_search(Side::Left);
        model.set_typed_search(List::Panel(Side::Left), text.to_owned());
        let start = QueryStart {
            now: "2026-03-04T12:00:00Z".parse().expect("an instant"),
            time_zone: jiff::tz::TimeZone::UTC,
        };
        model.start_search(List::Panel(Side::Left), &start, ScopeCheck::Exists)
    }

    fn fetch_of(accepted: Accepted) -> Fetch {
        match accepted
            .effects
            .into_iter()
            .find(|effect| matches!(effect, Effect::Fetch(_)))
        {
            Some(Effect::Fetch(fetch)) => fetch,
            other => panic!("a request starts: {other:?}"),
        }
    }

    /// T1: a request of one purpose ends the older request of that purpose,
    /// so a panel's search that shared Search's purpose would leave Search
    /// showing "Searching…" for ever.
    #[test]
    fn a_search_in_one_list_leaves_another_lists_request_alone() {
        let mut model = model();
        let fetch = search(&mut model, "report");
        let panel = fetch_of(panel_search(&mut model, r"C:\Users\me", "report"));
        assert_ne!(panel.purpose(), fetch.purpose());
        assert_eq!(
            model.accept(rows(&fetch, 3)),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 3
                    },
                    Effect::FirstRow {
                        list: List::Search,
                        generation: fetch.generation
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::ReleaseOperations,
                    Effect::FetchVisible(List::Search),
                    Effect::FillFirstRows(List::Search),
                    Effect::InspectAge,
                    Effect::MarkMatches(List::Search),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(
            model.accept(rows(&panel, 2)),
            Accepted {
                touched: Areas::list(List::Panel(Side::Left)) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Panel(Side::Left)),
                    Effect::SetCount {
                        list: List::Panel(Side::Left),
                        count: 2
                    },
                    Effect::FirstRow {
                        list: List::Panel(Side::Left),
                        generation: panel.generation
                    },
                    Effect::Cancel(Purpose::Selection(List::Panel(Side::Left))),
                    Effect::ReleaseOperations,
                    Effect::FetchVisible(List::Panel(Side::Left)),
                    Effect::FillFirstRows(List::Panel(Side::Left)),
                    Effect::InspectAge,
                    Effect::MarkMatches(List::Panel(Side::Left)),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(model.search_list().results().count(), 3);
    }

    /// T4: one list's new search must not show another list's deleted rows
    /// again, and an ended delete must still leave the queue once no list
    /// shows it.
    #[test]
    fn each_list_releases_only_its_own_ended_operations() {
        let mut model = model();
        let fetch = search(&mut model, "row");
        model.accept(rows(&fetch, 3));
        let panel = fetch_of(panel_search(&mut model, r"C:\", "row"));
        model.accept(rows(&panel, 3));
        let id = OperationId::for_tests(1);
        let mut taken = Taken::default();
        taken.insert(id, std::path::Path::new(r"C:\row1"), false);
        let delete = Arc::new(Request::Delete {
            sources: vec![r"C:\row1".into()],
        });
        model.operations_ended(Arc::new(Ended {
            operations: vec![(id, delete)],
            in_queue: HashSet::from([id]),
            taken: Arc::new(taken),
        }));
        let fetch = search(&mut model, "rows");
        model.accept(rows(&fetch, 3));
        assert!(
            model.keeps(id),
            "the panel's list still shows the row blank"
        );
        let ask = (model.panels_mut().leave_search(Side::Left)).expect("the panel searches");
        let listed = ReadEvent::Listed {
            generation: ask.generation,
            folder: ask.path,
            entries: Vec::new(),
            focus: None,
        };
        model.panels_mut().accept_read(Side::Left, listed, None);
        assert!(!model.keeps(id));
    }

    /// T5: an empty list in a folder that gozlerd does not index reads as
    /// "no such file".
    #[test]
    fn a_folder_that_is_not_indexed_says_so_before_typing() {
        let mut model = model();
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        panel_search(&mut model, r"E:\x", "");
        let message = |model: &Model| {
            let list = model.list(List::Panel(Side::Left)).expect("a search panel");
            match list.results().shown() {
                Shown::Message(message) => Some(message.clone()),
                Shown::Nothing | Shown::Rows(_) => None,
            }
        };
        assert_eq!(
            message(&model),
            Some(ListMessage::NotIndexed {
                root: r"E:\".to_owned()
            })
        );
        (model.panels_mut().search_mut(Side::Left))
            .expect("a search panel")
            .look_in = LookIn::AllDrives;
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        model.start_search(List::Panel(Side::Left), &start, ScopeCheck::Exists);
        assert_eq!(message(&model), Some(ListMessage::TypeToSearch));
    }

    /// T6: a panel's search takes none of Search's controls and leaves
    /// Search's sort alone (DESIGN.md: Search in a panel).
    #[test]
    fn a_panel_search_asks_with_the_default_match_and_the_panel_sort() {
        let mut model = model();
        model.set_matching(Matching {
            case: true,
            path: true,
            ..model.matching()
        });
        model.sort_by(SortSpec::default_for(SortKey::Size));
        (model.panels_mut()).sort(Side::Left, SortSpec::default_for(SortKey::DateModified));
        let fetch = fetch_of(panel_search(&mut model, r"C:\Users\me", "report"));
        let Ask::Rows(List::Panel(Side::Left), ask) = &fetch.ask else {
            panic!("the left panel's rows: {fetch:?}")
        };
        assert_eq!(ask.sort.key, SortKey::DateModified);
        assert_eq!(ask.search.scope.as_deref(), Some(r"C:\Users\me"));
        assert!(!ask.search.query.match_case);
        assert_eq!(model.sort().key, SortKey::Size);
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
                        progress: None,
                        entries: 0,
                        generation: 0,
                        metadata_pending: false,
                        journal_max_bytes: None,
                        placeholders_created: 0,
                        delete_of_unknown: 0,
                        stale_slots: 0,
                        link_reconciles_dropped: 0,
                        unread_folders: 0,
                        memory: None,
                    }],
                    volumes_incomplete: false,
                    process_working_set: 0,
                    process_private_bytes: 0,
                    include: None,
                    left_out: Vec::new(),
                    caller_sees: CallerSees::EveryFile,
                    links: None,
                },
                "0.4.0".to_owned(),
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
        assert_eq!(
            model.set_matching(case),
            Accepted {
                touched: Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![
                    Effect::InspectAge,
                    Effect::SearchAgain(vec![List::Search]),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        let older = search(&mut model, "repo");
        assert_eq!(
            model.search.list.results.count(),
            10,
            "the previous rows stay"
        );
        assert_eq!(
            captions::results_status(&model, &English),
            "10 results",
            "a search that answers at once changes the status bar once"
        );
        assert_eq!(
            model.name_search(List::Search),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge, Effect::Polling, Effect::NoteSavedState]
            }
        );
        assert_eq!(captions::results_status(&model, &English), "Searching…");
        let _newest = search(&mut model, "report f");
        assert_eq!(
            model.accept(rows(&older, 3)),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
        assert_eq!(model.search.list.results.count(), 10);
    }

    #[test]
    fn t2_the_new_search_first_rows_replace_the_old_ones() {
        let mut model = showing(10);
        let fetch = search(&mut model, "repo");
        assert_eq!(
            model.accept(rows(&fetch, 3)),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 3
                    },
                    Effect::FirstRow {
                        list: List::Search,
                        generation: fetch.generation
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::ReleaseOperations,
                    Effect::FetchVisible(List::Search),
                    Effect::FillFirstRows(List::Search),
                    Effect::InspectAge,
                    Effect::MarkMatches(List::Search),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(captions::results_status(&model, &English), "3 results");
    }

    #[test]
    fn t3_a_failure_while_rows_are_shown_removes_them() {
        let mut model = showing(50_000);
        let chunk = model
            .rows_needed(List::Search, CHUNK_ROWS..CHUNK_ROWS + 40)
            .expect("the rows are not cached");
        assert_eq!(
            model.accept(failed(&chunk, DaemonFailure::Lost)),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 0
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(model.search.list.results.count(), 0);
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
            model.accept(status(&ask, VolumePhase::Live)),
            Accepted {
                touched: Areas::STATUS
                    | Areas::LOCATIONS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::PreparePanel(Side::Left),
                    Effect::PreparePanel(Side::Right),
                    Effect::ReadMemory,
                    Effect::InspectAge,
                    Effect::SearchAgain(vec![List::Search]),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        // A search that takes longer than the poll would otherwise start
        // again on every status reply, and never finish.
        let _again = search(&mut model, "report");
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Live)),
            Accepted {
                touched: Areas::STATUS
                    | Areas::LOCATIONS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::PreparePanel(Side::Left),
                    Effect::PreparePanel(Side::Right),
                    Effect::ReadMemory,
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );

        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Bootstrapping)),
            Accepted {
                touched: Areas::STATUS
                    | Areas::LOCATIONS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::PreparePanel(Side::Left),
                    Effect::PreparePanel(Side::Right),
                    Effect::ReadMemory,
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        let ask = model.status_needed().expect("no status request runs");
        assert_eq!(
            model.accept(status(&ask, VolumePhase::Live)),
            Accepted {
                touched: Areas::STATUS
                    | Areas::LOCATIONS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::PreparePanel(Side::Left),
                    Effect::PreparePanel(Side::Right),
                    Effect::ReadMemory,
                    Effect::InspectAge,
                    Effect::SearchAgain(vec![List::Search]),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert!(
            !model.polls_status(false),
            "nothing needs the poll any more"
        );
    }

    #[test]
    fn t5_a_volume_that_starts_indexing_shows_progress_and_keeps_the_rows() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        assert_eq!(captions::index_status(&model), "");
        assert!(!model.polls_status(false));

        let ask = model.status_needed().expect("no status request runs");
        let accepted = model.accept(status(&ask, VolumePhase::Rescanning));
        assert_eq!(
            accepted,
            Accepted {
                touched: Areas::STATUS
                    | Areas::LOCATIONS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::PreparePanel(Side::Left),
                    Effect::PreparePanel(Side::Right),
                    Effect::ReadMemory,
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(model.search.list.results.count(), 10);
        assert_eq!(captions::index_status(&model), "");
        assert_eq!(
            captions::StatusCaptions::new(&model, &English)
                .indexing
                .len(),
            1
        );
        assert!(model.polls_status(false));
    }

    #[test]
    fn status_totals_keep_known_file_bytes_without_folder_bytes_or_unknown_counts() {
        let mut model = model();
        let fetch = search(&mut model, "report");
        let mut reply = rows(&fetch, 5);
        if let Outcome::Rows(chunk) = &mut reply.outcome {
            chunk.sums = Some(gozler_core::query::MatchSums {
                files: 3,
                file_bytes: 1024,
                files_without_size: 2,
                folders: 2,
                folder_bytes: 8192,
                folder_bytes_at_least: true,
            });
        }
        model.accept(reply);
        let status = captions::StatusCaptions::new(&model, &English);
        let texts: Vec<_> = status.right.iter().map(|part| part.text.as_str()).collect();
        assert_eq!(texts, ["3 files (1 KB)", "2 folders"]);
        assert_eq!(status.right[0].connector.as_deref(), Some("and"));
    }

    /// The model keeps the latest status while the service does not
    /// answer, and a service that could not read its memory says 0, so the
    /// status bar would show a stopped service's old memory, or 0 bytes.
    #[test]
    fn the_service_memory_is_unknown_while_the_service_does_not_answer() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        assert_eq!(model.service_working_set(), None, "0 is unknown");

        let ask = model.status_needed().expect("no status request runs");
        let mut answered = status(&ask, VolumePhase::Live);
        if let Outcome::Status(status, _, _) = &mut answered.outcome {
            status.process_working_set = 400_000_000;
        }
        model.accept(answered);
        assert_eq!(model.service_working_set(), Some(400_000_000));

        let ask = model.status_needed().expect("no status request runs");
        model.accept(failed(&ask, DaemonFailure::Lost));
        assert_eq!(model.service_working_set(), None);
    }

    /// The memory figures cost a status request every 2 seconds, which
    /// only a person who turned them on and can see them pays for.
    #[test]
    fn the_memory_figures_poll_only_while_shown_on_screen() {
        let mut model = showing(10);
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        assert!(!model.polls_status(true), "off by default");
        model.settings.choices.show_memory = true;
        assert!(model.polls_status(true));
        assert!(!model.polls_status(false), "minimized or hidden");
    }

    /// A results page carries only combined flags, so the status bar would
    /// never name a volume that became incomplete after the last status reply.
    #[test]
    fn a_page_flag_that_the_status_does_not_explain_asks_for_status() {
        let mut model = showing(50_000);
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        let fetch = model
            .rows_needed(List::Search, CHUNK_ROWS..CHUNK_ROWS + 40)
            .expect("the rows are not cached");
        let mut reply = rows(&fetch, 50_000);
        if let Outcome::Rows(chunk) = &mut reply.outcome {
            chunk.volumes_incomplete = true;
        }
        assert!(matches!(
            model
                .accept(reply)
                .effects
                .into_iter()
                .find(|effect| matches!(effect, Effect::Fetch(_))),
            Some(Effect::Fetch(Fetch {
                ask: Ask::Status { .. },
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
            ask: ask.ask.clone(),
        };
        assert_eq!(
            model.accept(status(&older, VolumePhase::Rescanning)),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
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
            model.accept(failed(&ask, DaemonFailure::Lost)),
            Accepted {
                touched: Areas::STATUS
                    | Areas::LOCATIONS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::PreparePanel(Side::Left),
                    Effect::PreparePanel(Side::Right),
                    Effect::ReadMemory,
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(model.search.list.results.count(), 10);
        assert_eq!(message(&model), None);

        let fetch = search(&mut model, "repo");
        let refused = DaemonFailure::Refused {
            detail: "regex parse error".into(),
        };
        assert_eq!(
            model.accept(failed(&fetch, refused)),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 0
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(model.search.list.results.count(), 0);
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
        assert_eq!(
            model.accept(failed(&ask, disabled)),
            Accepted {
                touched: Areas::STATUS
                    | Areas::LOCATIONS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::PreparePanel(Side::Left),
                    Effect::PreparePanel(Side::Right),
                    Effect::ReadMemory,
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
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
        model.set_typed_search(List::Search, "dz:x".into());
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        assert_eq!(
            model.start_search(List::Search, &start, ScopeCheck::Exists),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 0
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        let strings = english();
        assert_eq!(
            message(&model),
            Some((strings.format("query.unknown_function", &["dz:"]), vec![]))
        );
        assert!(!model.polls_status(false));
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
            model.start_search(List::Search, &start, ScopeCheck::Missing),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 0
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
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
        assert!(!model.polls_status(false));
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
            assert_eq!(model.polls_status(false), polls, "{failure:?}");
        }
        for (phase, polls) in [
            (VolumePhase::Bootstrapping, true),
            (VolumePhase::Rescanning, true),
            (VolumePhase::Offline, false),
        ] {
            let mut model = showing(10);
            let ask = model.status_needed().expect("no status request runs");
            model.accept(status(&ask, phase.clone()));
            assert_eq!(model.polls_status(false), polls, "{phase:?}");
        }
    }

    fn paths(model: &Model) -> Vec<&str> {
        model
            .search_list()
            .selection()
            .files()
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
        let accepted = model.set_selection(List::Search, vec![3..5, 9..10].into(), None);
        assert_eq!(
            accepted,
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::InspectAge,
                    Effect::ListPasses,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(paths(&model), [r"C:\row3", r"C:\row4", r"C:\row9"]);
        assert_eq!(model.read_selection(List::Search), None);
    }

    #[test]
    fn a_delayed_large_filter_cannot_restore_an_old_selection_or_taken_file() {
        let mut model = showing(5_000);
        model.set_selection(List::Search, vec![0..2_001].into(), None);
        let old = model.take_selection_filter(List::Search).unwrap().run();
        assert!(model.search.list.selection.files().is_none());
        model.set_selection(List::Search, vec![3..4].into(), None);
        assert_eq!(
            model.accept_filtered(List::Search, old),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
        assert_eq!(paths(&model), [r"C:\row3"]);

        model.set_selection(List::Search, vec![0..2_001].into(), None);
        let old = model.take_selection_filter(List::Search).unwrap().run();
        let mut taken = Taken::default();
        taken.insert(OperationId::for_tests(1), Path::new(r"C:\row1"), false);
        model.set_taken(Arc::new(taken), [None, None]);
        assert!(model.taken_rows(List::Search, &[0..2_001]).is_empty());
        model.accept_filtered(List::Search, old);
        assert!(model.search.list.selection.files().is_none());
        let current = model.take_selection_filter(List::Search).unwrap().run();
        model.accept_filtered(List::Search, current);
        let selected = paths(&model);
        assert_eq!(selected.len(), 2_000);
        assert!(!selected.contains(&r"C:\row1"));
    }

    #[test]
    fn operation_receipts_leave_folder_filters_dispatchable() {
        let mut model = model();
        let fetch = search(&mut model, "report");
        let mut reply = rows(&fetch, 3);
        let Outcome::Rows(chunk) = &mut reply.outcome else {
            panic!("the rows fixture returns rows")
        };
        Arc::make_mut(&mut chunk.rows)[0].is_dir = true;
        model.accept(reply);
        let counted = folder_size(model.set_selection(List::Search, vec![0..1].into(), None));
        model.accept(totals(&counted, 7));
        let id = OperationId::for_tests(1);
        let mut taken = Taken::default();
        taken.insert(id, Path::new(r"D:\unrelated"), false);
        let taken = Arc::new(taken);
        model.observe_operations(
            Arc::from([Operation {
                id,
                request: Arc::new(Request::Delete {
                    sources: vec![r"D:\unrelated".into()],
                }),
                folder: gozler_fileops::SourcesFolder::Pending,
                state: State::Preparing,
            }]),
            Arc::clone(&taken),
            Instant::now(),
        );
        model.set_taken(taken, [None; 2]);
        let pending = model.take_following().expect("the receipt needs following");
        model.accept_followed(pending.run(&English));
        assert_eq!(model.search_list().selection().asked(), None);
        let filter = model
            .take_selection_filter(List::Search)
            .expect("receipt filtering remains available to the normal dispatcher");
        let accepted = model.accept_filtered(List::Search, filter.run());
        let fetched = folder_size(accepted);
        let Ask::FolderSize(search) = fetched.ask else {
            panic!("folder_size returns a folder query")
        };
        assert_eq!(search.scope.as_deref(), Some(r"C:\row0"));
    }

    #[test]
    fn pending_ends_keep_their_original_cache_evidence_across_eviction() {
        let mut model = showing(50_000);
        let (first, second) = (OperationId::for_tests(1), OperationId::for_tests(2));
        let delete = |path: &str| {
            Arc::new(Request::Delete {
                sources: vec![path.into()],
            })
        };
        let ended = |id, path| Operation {
            id,
            request: delete(path),
            folder: gozler_fileops::SourcesFolder::Pending,
            state: State::Ended(Arc::new(gozler_fileops::Outcome::new(
                gozler_fileops::End::Complete,
                1,
                0,
                Vec::new(),
                gozler_fileops::RingCounts::default(),
            ))),
        };
        let mut snapshot = vec![ended(first, r"C:\row1")];
        let mut taken = Taken::default();
        taken.insert(first, Path::new(r"C:\row1"), true);
        let taken = Arc::new(taken);
        model.set_taken(Arc::clone(&taken), [None, None]);
        model.observe_operations(
            Arc::from(snapshot.clone()),
            Arc::clone(&taken),
            Instant::now(),
        );
        let pending = model.take_following().unwrap();
        assert!(
            model
                .search
                .list
                .results
                .adopt_snapshot(ResultSnapshotId(1))
        );
        let steps = model.accept_followed(pending.run(&English));
        assert!(!steps.queue.contains(&QueueStep::Remove(first)));
        assert!(model.taken_rows(List::Search, &[1..2]).is_empty());
        let first_pass = model.take_keep(List::Search).unwrap();
        snapshot.push(ended(second, r"D:\elsewhere"));
        model.observe_operations(snapshot.into(), Arc::default(), Instant::now());
        let pending = model.take_following().unwrap();
        let steps = model.accept_followed(pending.run(&English));
        assert!(
            steps.queue.is_empty(),
            "no operation leaves before every list settles"
        );
        assert!(
            model.take_keep(List::Search).is_none(),
            "accept the predecessor before dispatch"
        );
        model.accept_kept(List::Search, first_pass.run());
        let second_pass = model.take_keep(List::Search).unwrap();
        model.accept_kept(List::Search, second_pass.run());
        assert!(model.keeps(first));
        assert!(!model.keeps(second));
        assert!(model.release_operations().is_empty());
        let release = model.take_following().unwrap();
        assert_eq!(
            model.accept_followed(release.run(&English)).queue,
            [QueueStep::Remove(second)]
        );
    }

    /// The reply to the selection's one request: the rows it keeps, under
    /// the folder `folder`, so the test sees which rows came from it.
    fn selection_reply(fetch: &Fetch, folder: &str) -> Fetched {
        let Ask::Selection(_, ask) = &fetch.ask else {
            panic!("a selection fetch: {fetch:?}")
        };
        let span = ask.span.start..ask.span.start + ask.span.len;
        let rows = span
            .filter(|p| ask.keeps(*p))
            .map(|p| {
                let mut row = chunk(p, 1, 50_000).rows[0].clone();
                row.path = format!(r"{folder}\row{p}");
                (p, row)
            })
            .collect();
        Fetched {
            purpose: Purpose::Selection(List::Search),
            generation: fetch.generation,
            outcome: Outcome::Selection(SelectionRows::new(
                &ask.positions,
                rows,
                ask.cached.clone(),
                None,
            )),
        }
    }

    /// S12: a selected row outside the cache is read in one request, whose
    /// rows also replace the cached ones inside its span, so the list shows
    /// the files the selection holds, of one moment.
    #[test]
    fn a_selected_row_outside_the_cache_is_read_with_the_cached_rows_beside_it() {
        let mut model = showing(50_000);
        let accepted = model.set_selection(List::Search, vec![0..1, 40_000..40_001].into(), None);
        assert_eq!(
            accepted,
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::InspectAge,
                    Effect::ListPasses,
                    Effect::ReadSelectionSoon(List::Search),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(
            captions::selection_status(&model, &English),
            "Reading the selection…"
        );
        let fetch = model
            .read_selection(List::Search)
            .expect("the selection is read");
        assert_eq!(model.read_selection(List::Search), None, "one request");
        let Ask::Selection(_, ask) = &fetch.ask else {
            panic!("a selection fetch")
        };
        assert_eq!((ask.span.start, ask.span.len), (0, 40_001));
        assert_eq!(ask.cached, [0..CHUNK_ROWS]);

        assert_eq!(
            model.accept(selection_reply(&fetch, "D:")),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::Redraw {
                        list: List::Search,
                        rows: 0..CHUNK_ROWS
                    },
                    Effect::FetchVisible(List::Search),
                    Effect::ListPasses,
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(paths(&model), [r"D:\row0", r"D:\row40000"]);
        let shown = model.search.list.results.row(5).map(|r| r.path.as_str());
        assert_eq!(shown, Some(r"D:\row5"), "the cached rows are of its moment");
    }

    /// A list reaches every match, so Select all over millions of rows would
    /// read about 20 MB of files for each 100,000; past the limit the
    /// selection is not read, and no command acts on it.
    #[test]
    fn a_selection_past_the_limit_is_not_read() {
        let mut model = showing(200_000);
        let accepted = model.set_selection(List::Search, vec![0..150_000].into(), None);
        assert_eq!(
            accepted,
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::InspectAge,
                    Effect::ListPasses,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(model.read_selection(List::Search), None);
        assert_eq!(model.search_list().selection().files(), None);
        assert_eq!(
            captions::selection_status(&model, &English),
            "150,000 selected · Select up to 100,000 to use a command"
        );
    }

    /// A selection that reaches past the first chunk makes the list's
    /// result snapshot, whose rows replace the cache, and a row whose file
    /// is gone is no file to act on. Once the service drops the result
    /// snapshot, the list searches again, as F5 does.
    #[test]
    fn a_selection_read_makes_the_result_snapshot_that_rows_are_read_from() {
        use gozler_core::proto::{ResultItem, ResultSnapshotId};
        let mut model = showing(200_000);
        model.set_selection(List::Search, vec![150_000..150_002].into(), None);
        let fetch = model
            .read_selection(List::Search)
            .expect("the selection is read");
        let Ask::Selection(_, ask) = &fetch.ask else {
            panic!("a selection fetch")
        };
        assert_eq!(ask.span.snapshot, FromSnapshot::Make { replaces: None });
        let id = ResultSnapshotId(9);
        let mut reply = selection_reply(&fetch, "D:");
        if let Outcome::Selection(rows) = &mut reply.outcome {
            rows.made = Some(id);
            Arc::make_mut(&mut rows.files)[1] = ResultItem::gone();
        }
        assert_eq!(
            model.accept(reply),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::Redraw {
                        list: List::Search,
                        rows: 0..200_000
                    },
                    Effect::FetchVisible(List::Search),
                    Effect::ListPasses,
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(paths(&model), [r"D:\row150000"]);

        let fetch = (model.rows_needed(List::Search, 0..40)).expect("the cache was dropped");
        let Ask::Rows(_, ask) = &fetch.ask else {
            panic!("a rows fetch")
        };
        assert_eq!(ask.snapshot, FromSnapshot::Read(id));
        let gone = Fetched {
            purpose: Purpose::Results(List::Search),
            generation: fetch.generation,
            outcome: Outcome::ResultSnapshotGone,
        };
        assert_eq!(
            model.accept(gone),
            Accepted {
                touched: Areas::NONE,
                effects: vec![Effect::SearchAgain(vec![List::Search])]
            }
        );
    }

    /// T6: a reply for a selection the person has changed since names other
    /// files.
    #[test]
    fn t6_a_selection_changed_while_it_is_read_drops_the_older_reply() {
        let mut model = showing(50_000);
        model.set_selection(List::Search, vec![40_000..40_001].into(), None);
        let older = model
            .read_selection(List::Search)
            .expect("the selection is read");
        // The window ends the older read when it schedules the new one.
        assert_eq!(
            model.set_selection(List::Search, vec![40_000..40_002].into(), None),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::InspectAge,
                    Effect::ListPasses,
                    Effect::ReadSelectionSoon(List::Search),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(
            model.accept(selection_reply(&older, "D:")),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
        assert_eq!(paths(&model), Vec::<&str>::new());
        let newer = model
            .read_selection(List::Search)
            .expect("the new selection is read");
        assert!(newer.generation > older.generation);
    }

    /// The request for one selected folder's contents, which `accepted`
    /// starts.
    fn folder_size(accepted: Accepted) -> Fetch {
        match accepted
            .effects
            .into_iter()
            .find(|effect| matches!(effect, Effect::Fetch(_)))
        {
            Some(Effect::Fetch(
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
            purpose: Purpose::Selection(List::Search),
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
            Arc::make_mut(&mut chunk.rows)[0].is_dir = true;
            Arc::make_mut(&mut chunk.rows)[1].is_dir = true;
        }
        model.accept(reply);
        let size = |model: &Model| {
            captions::InspectorCaptions::new(model, &English).fields[1]
                .1
                .clone()
        };

        let first = folder_size(model.set_selection(List::Search, vec![0..1].into(), None));
        assert_eq!(size(&model), "Calculating…");
        let second = folder_size(model.set_selection(List::Search, vec![1..2].into(), None));
        let Ask::FolderSize(search) = &second.ask else {
            unreachable!("folder_size returns a folder size request")
        };
        assert_eq!(search.scope.as_deref(), Some(r"C:\row1"));
        assert_eq!(
            model.accept(totals(&first, 5)),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
        assert_eq!(size(&model), "Calculating…");
        assert_eq!(
            model.accept(totals(&second, 7)),
            Accepted {
                touched: Areas::INSPECTOR,
                effects: vec![Effect::InspectAge]
            }
        );
        assert_eq!(size(&model), "2 KB (2,048 bytes) in 7 items");

        let _third = folder_size(model.set_selection(List::Search, vec![0..1].into(), None));
        assert_eq!(
            model.set_selection(List::Search, vec![2..3].into(), None),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::InspectAge,
                    Effect::ListPasses,
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
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
        model.set_selection(List::Search, vec![40_000..40_001].into(), None);
        let fetch = model
            .read_selection(List::Search)
            .expect("the selection is read");
        assert_eq!(
            model.accept(failed(&fetch, DaemonFailure::Lost)),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 0
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(model.search.list.selection, Selection::None);
        let strings = english();
        assert_eq!(
            message(&model),
            Some((strings.text("failure.lost").to_owned(), vec![]))
        );
        assert!(model.polls_status(false));

        let mut model = showing(50_000);
        model.set_selection(List::Search, vec![40_000..40_001].into(), None);
        let read = model
            .read_selection(List::Search)
            .expect("the selection is read");
        let newer = search(&mut model, "repo");
        model.accept(failed(&read, DaemonFailure::Lost));
        assert_eq!(
            model.accept(rows(&newer, 3)),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 3
                    },
                    Effect::FirstRow {
                        list: List::Search,
                        generation: newer.generation
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::ReleaseOperations,
                    Effect::FetchVisible(List::Search),
                    Effect::FillFirstRows(List::Search),
                    Effect::InspectAge,
                    Effect::MarkMatches(List::Search),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
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
        model.set_selection(List::Search, vec![3..5].into(), None);
        model.set_typed_search(List::Search, r"F:\ ext:cs".into());
        model.set_language(LanguageChoice::Tag("en".into()), english());
        assert_eq!(model.search_list().results().count(), 50_000);
        assert_eq!(paths(&model), [r"C:\row3", r"C:\row4"]);
        assert_eq!(model.search.list.text, r"F:\ ext:cs");
        assert_eq!(model.choices().language, LanguageChoice::Tag("en".into()));
    }

    /// A delete of the whole selection unhighlights every row in one
    /// message; one that takes part of it unhighlights only the taken rows,
    /// because the list would otherwise drop the rest of the person's
    /// selection, which the model still holds.
    #[test]
    fn only_a_selection_that_lost_every_file_deselects_every_row() {
        let mut model = showing(10);
        model.set_selection(List::Search, vec![1..3].into(), None);
        let mut part = Taken::default();
        part.insert(OperationId::for_tests(1), Path::new(r"C:\row1"), false);
        let shown = model.set_taken(Arc::new(part), [None, None]);
        assert_eq!(shown.search_deselect, Deselect::Taken);
        assert_eq!(paths(&model), [r"C:\row2"]);
        let mut all = Taken::default();
        all.insert(OperationId::for_tests(1), Path::new(r"C:\row1"), false);
        all.insert(OperationId::for_tests(2), Path::new(r"C:\row2"), false);
        let shown = model.set_taken(Arc::new(all), [None, None]);
        assert_eq!(shown.search_deselect, Deselect::All);
    }

    /// T2: the rows of a new search are other files.
    #[test]
    fn a_new_search_clears_the_selection() {
        let mut model = showing(10);
        model.set_selection(List::Search, vec![1..2].into(), None);
        let fetch = search(&mut model, "repo");
        assert_eq!(paths(&model), [r"C:\row1"], "the selection stays until");
        model.accept(rows(&fetch, 3));
        assert_eq!(model.search.list.selection, Selection::None);
    }

    /// Starts an export of the shown search as CSV; returns its fetch.
    fn export(model: &mut Model) -> Fetch {
        let order = model.column_order();
        fetch_of(model.start_export(r"D:\results.csv".into(), ExportFormat::Csv, &order))
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
        assert_eq!(
            ended,
            Accepted {
                touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![Effect::InspectAge, Effect::Polling, Effect::NoteSavedState]
            }
        );
        assert_eq!(
            captions::export_status(&model, &English),
            "Exported 10 rows to results.csv"
        );
        assert_eq!(captions::results_status(&model, &English), "10 results");
        assert!(model.can_export());
    }

    /// T10: the Index page must show that the service stopped while it is
    /// open, so the poll it starts goes on through the failure; the list
    /// keeps its rows, as a failed status request always does (DESIGN.md:
    /// Requests).
    #[test]
    fn t10_a_failure_while_settings_shows_the_index_page_keeps_the_poll() {
        let mut model = showing(10);
        assert!(!model.polls_status(false));
        model.show_settings_page(Some(SettingsPage::Index));
        assert!(model.polls_status(false), "the page follows the service");
        let ask = model.status_needed().expect("no status request runs");
        let stopped = DaemonFailure::NotRunning(NotRunning::Stopped);
        assert_eq!(
            model.accept(failed(&ask, stopped)),
            Accepted {
                touched: Areas::STATUS
                    | Areas::LOCATIONS
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::PreparePanel(Side::Left),
                    Effect::PreparePanel(Side::Right),
                    Effect::ReadMemory,
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(
            captions::IndexCaptions::new(&model, &English, &|_| None).service,
            english().text("failure.stopped")
        );
        assert_eq!(
            captions::index_status(&model),
            english().text("failure.stopped"),
            "the main window shows the failure behind the dialog"
        );
        assert!(model.polls_status(false));
        assert_eq!(model.search.list.results.count(), 10);
        // The service answers again: the page shows its state, and the
        // poll ends with the page.
        let ask = model.status_needed().expect("no status request runs");
        model.accept(status(&ask, VolumePhase::Live));
        model.show_settings_page(None);
        assert!(!model.polls_status(false));
    }

    #[test]
    fn a_cancelled_fix_reply_cannot_advance_the_next_fix() {
        use crate::core::fixer::{FixState, SetupEvent, SetupFix, SetupResult, SetupStep};

        let mut model = model();
        model.start_fix(&Fix::Update);
        model.cancel_fix();
        model.start_fix(&Fix::Update);
        let accepted = model.accept_fix(
            Generation(1),
            FixEvent::Setup(SetupEvent::End(SetupResult::NoSetup)),
        );
        assert_eq!(
            accepted,
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
        assert_eq!(
            model.fix(),
            &Fixer::Setup {
                fix: SetupFix::Update,
                state: FixState::Running(SetupStep::FindingRelease),
            }
        );
        assert!(model.awaits_fix(Generation(2)));
    }

    /// A service that answers again before its commands end stops the
    /// poll, so a fix that waited for the poll would end as "did not
    /// answer" with the service running (DESIGN.md: Service fixes).
    #[test]
    fn a_fix_asks_the_service_once_its_commands_end() {
        let mut model = showing(10);
        let started = model.start_fix(&Fix::Repair(Repair::StartService));
        assert_eq!(
            started,
            Accepted {
                touched: Areas::STATUS | Areas::SEARCH | Areas::INSPECTOR | Areas::KEY_BAR,
                effects: vec![
                    Effect::PostSettings,
                    Effect::InspectAge,
                    Effect::RunFix {
                        generation: Generation(2),
                        work: FixWork::Commands(vec!["net start gozler".into()])
                    },
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        let other = ListButton::Fix(Fix::Repair(Repair::TurnOnService));
        assert!(!model.list_button_enabled(&other), "one fix runs at a time");
        model.accept_fix(Generation(2), FixEvent::Commands(CommandEvent::Allowed));
        assert!(!model.polls_status(false));
        let ended = model.accept_fix(Generation(2), FixEvent::Commands(CommandEvent::Succeeded));
        let ask = fetch_of(ended);
        assert!(
            model.polls_status(false),
            "the poll runs until the service answers"
        );
        model.accept(status(&ask, VolumePhase::Live));
        assert!(!model.polls_status(false));
        assert_eq!(captions::StatusCaptions::new(&model, &English).job.text, "");
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
        assert_eq!(
            accepted,
            Accepted {
                touched: Areas::TITLE
                    | Areas::SEARCH
                    | Areas::INSPECTOR
                    | Areas::STATUS
                    | Areas::LEFT_PANEL
                    | Areas::RIGHT_PANEL
                    | Areas::KEY_BAR,
                effects: vec![
                    Effect::Menu,
                    Effect::Tray,
                    Effect::InspectAge,
                    Effect::SearchAgain(vec![List::Search]),
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        assert_eq!(
            model.start_search(List::Search, &start, ScopeCheck::Exists),
            Accepted {
                touched: Areas::list(List::Search) | Areas::STATUS | Areas::KEY_BAR,
                effects: vec![
                    Effect::ClearSelection(List::Search),
                    Effect::SetCount {
                        list: List::Search,
                        count: 0
                    },
                    Effect::Cancel(Purpose::Selection(List::Search)),
                    Effect::InspectAge,
                    Effect::Polling,
                    Effect::NoteSavedState
                ]
            }
        );
        assert_eq!(
            message(&model),
            Some(("Type to search.".to_owned(), vec![]))
        );
        assert_eq!(
            model.set_typed_search(List::Search, "report".into()),
            Accepted {
                touched: Areas::TITLE | Areas::SEARCH,
                effects: vec![]
            }
        );
        assert!(
            model
                .start_search(List::Search, &start, ScopeCheck::Exists)
                .effects
                .into_iter()
                .any(|effect| matches!(effect, Effect::Fetch(_)))
        );
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
            Arc::make_mut(&mut chunk.rows)[0].path = r"C:\TC\TOTALCMD64.EXE".into();
        }
        assert_eq!(
            model.accept(reply.clone()),
            Accepted {
                touched: Areas::NONE,
                effects: vec![Effect::TotalCommander(Ok(Some(
                    r"C:\TC\TOTALCMD64.EXE".into()
                )))]
            }
        );
        assert_eq!(
            model.accept(reply),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            },
            "answered once"
        );
        let fetch = model.find_total_commander();
        assert_eq!(
            model.accept(failed(&fetch, DaemonFailure::Lost)),
            Accepted {
                touched: Areas::NONE,
                effects: vec![Effect::TotalCommander(Err(DaemonFailure::Lost))]
            }
        );
        assert!(!model.polls_status(false));
        assert_eq!(model.search.list.results.count(), 10);
        let fetch = model.find_total_commander();
        assert_eq!(
            model.forget_total_commander(),
            Accepted {
                touched: Areas::NONE,
                effects: vec![Effect::Cancel(Purpose::Program)]
            }
        );
        assert_eq!(
            model.accept(failed(&fetch, DaemonFailure::Lost)),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
    }

    /// A count that arrives after the Include settings changed again, or
    /// after Settings closed, never shows as the preview's "After OK".
    #[test]
    fn a_stale_include_count_is_never_shown() {
        let mut model = showing(10);
        let counted = |fetch: &Fetch| Fetched {
            purpose: Purpose::IncludeCount,
            generation: fetch.generation,
            outcome: Outcome::IncludedCount(7),
        };
        let first = model.count_included(IndexSettings::default());
        assert_eq!(
            model.forget_included_count(),
            Accepted {
                touched: Areas::NONE,
                effects: vec![Effect::Cancel(Purpose::IncludeCount)]
            }
        );
        assert_eq!(
            model.accept(counted(&first)),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
        assert_eq!(model.included_count(), None);
        let second = model.count_included(IndexSettings::default());
        assert_eq!(
            model.accept(counted(&second)),
            Accepted {
                touched: Areas::NONE,
                effects: vec![Effect::PostSettings]
            }
        );
        assert_eq!(model.included_count(), Some(&Ok(7)));
        // Closing Settings forgets it, so the next session starts empty.
        assert_eq!(
            model.forget_included_count(),
            Accepted {
                touched: Areas::NONE,
                effects: vec![]
            }
        );
        assert_eq!(model.included_count(), None);
    }

    /// The rows fetch that `fetch`'s chunk ask stands for, so [`rows`]
    /// answers a program search.
    fn fetch_rows(fetch: &Fetch) -> Fetch {
        let Ask::Program(ask) = &fetch.ask else {
            panic!("a program search: {fetch:?}")
        };
        Fetch {
            generation: fetch.generation,
            ask: Ask::Rows(List::Search, ask.clone()),
        }
    }

    /// The middle group shows one job: the newest of the running ones, so
    /// an export started during a copy shows, and the copy shows again
    /// when the export ends only if it started later (DESIGN.md: Status
    /// bar; DESIGN.md: File operations).
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
                    phase: gozler_fileops::RunningPhase::Executing,
                    items: 0,
                    bytes: 0,
                    passed: 0,
                    total: None,
                    renames: false,
                    paused: false,
                    yielded: false,
                    current: None,
                },
            })
        };
        model.set_operation(running(1));
        assert_eq!(model.job(), Job::Operation);
        let receipt = model.next_job_moment();
        export(&mut model);
        assert_eq!(model.job(), Job::Export);
        model.set_operation_at(running(2), receipt);
        assert_eq!(model.job(), Job::Export);
        model.set_operation(running(3));
        assert_eq!(model.job(), Job::Operation);
        assert_eq!(
            crate::core::captions::settings_job(&model, model.strings(), &English, true),
            crate::core::captions::JobCaption::default(),
        );
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
            Accepted {
                touched: Areas::STATUS,
                effects: vec![Effect::ExportFailed {
                    path: r"D:\results.csv".into(),
                    error: ExportError::Write(denied)
                }]
            }
        );
        assert_eq!(captions::export_status(&model, &English), "");
        assert!(model.can_export());
    }
}
