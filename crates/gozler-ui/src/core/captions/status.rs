use super::{Narrow, ValuePart, failure_text, joined, too_many};
use crate::core::fetch::List;
use crate::core::file_operations::{self, Bar};
use crate::core::fixer::{
    CommandFix, CommandResult, CommandStep, FixState, Fixer, SetupFix, SetupResult, SetupStep,
};
use crate::core::glyph::Glyph;
use crate::core::layout::WIDEST_TOTAL;
use crate::core::model::{Export, Job, Model};
use crate::core::results::{ListMessage, Shown};
use crate::core::search_list::SearchList;
use crate::core::selection::{SELECTION_LIMIT, Selection};
use crate::core::settings::Mode;
use crate::core::text::{self, Numbers, Strings};
use gozler_core::types::{IndexProgress, VolumePhase};
use std::path::PathBuf;

/// The status bar's three groups (DESIGN.md: Status bar).
pub(crate) struct StatusCaptions {
    /// What the view holds: the results and the selection.
    pub(crate) view: String,
    pub(crate) job: JobCaption,
    pub(crate) indexing: Vec<IndexingCaption>,
    /// The service and index state, followed by the search's totals, each
    /// after its icon.
    pub(crate) right: Vec<ValuePart>,
    /// The main window's memory group; `None` while Settings > General
    /// hides it.
    pub(crate) memory: Option<MemoryCaptions>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexingCaption {
    pub(crate) label: String,
    pub(crate) compact: String,
    pub(crate) value: String,
    pub(crate) progress: JobProgress,
}

/// The status bar's memory group: the working sets of the service and of
/// the app, each after its icon, with a tooltip for each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MemoryCaptions {
    pub(crate) service: ValuePart,
    pub(crate) app: ValuePart,
    pub(crate) service_tip: String,
    pub(crate) app_tip: String,
}

impl MemoryCaptions {
    fn new(model: &Model, numbers: &dyn Numbers) -> MemoryCaptions {
        let strings = model.strings();
        let figure = |bytes: Option<u64>| {
            bytes.map_or_else(
                || strings.text("status.memory_unknown").to_owned(),
                |bytes| text::bytes(strings, numbers, bytes),
            )
        };
        let service = figure(model.service_working_set());
        let app = figure(model.app_working_set());
        MemoryCaptions {
            service: ValuePart {
                connector: None,
                glyph: Some(Glyph::Component),
                text: strings.format("status.service_memory", &[&service]),
                narrow: None,
            },
            app: ValuePart {
                connector: None,
                glyph: Some(Glyph::Favicon),
                text: strings.format("status.ui_memory", &[&app]),
                narrow: None,
            },
            service_tip: strings.format("status.service_memory_tooltip", &[&service]),
            app_tip: strings.format("status.ui_memory_tooltip", &[&app]),
        }
    }
}

/// The memory group's widest texts, which its fixed width fits (DESIGN.md:
/// Look).
pub(crate) struct MemoryWidest {
    pub(crate) service: Vec<String>,
    pub(crate) app: Vec<String>,
}

impl MemoryWidest {
    /// Every form of a figure in MB and in GB, the units a process's
    /// working set takes, and the unknown figure.
    pub(super) fn new(strings: &Strings, numbers: &dyn Numbers) -> MemoryWidest {
        let figures: Vec<String> = (2..=3)
            .flat_map(|unit| [9.99, 99.9, 999.0].map(|value| (value * 1024f64.powi(unit)) as u64))
            .map(|bytes| text::bytes(strings, numbers, bytes))
            .chain([strings.text("status.memory_unknown").to_owned()])
            .collect();
        let all = |key: &str| -> Vec<String> {
            figures
                .iter()
                .map(|figure| strings.format(key, &[figure]))
                .collect()
        };
        MemoryWidest {
            service: all("status.service_memory"),
            app: all("status.ui_memory"),
        }
    }
}

/// The status bar's middle group: the running job, or the last job's
/// result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct JobCaption {
    pub(crate) text: String,
    pub(crate) transfer: Option<crate::core::status_transfer::Caption>,
    pub(crate) transfer_only: bool,
    pub(crate) progress: JobProgress,
    /// Whether the work shown here can stop now.
    pub(crate) cancel: bool,
}

/// The progress bar beside a job's text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum JobProgress {
    /// No bar: the job waits for the person, or it ended.
    #[default]
    None,
    /// A bar that moves without measuring, because the job does not know
    /// how much is left.
    Unmeasured,
    /// A bar filled to `done` of `total`.
    Measured { done: u64, total: u64 },
}

/// The texts each fixed group of the status bar can show at its widest,
/// which the layout sizes the group for, so a group never moves with its
/// text (DESIGN.md: Look).
pub(crate) struct StatusWidest {
    pub(crate) view: Vec<String>,
    pub(crate) indexing: String,
    /// A running job's texts, which share the middle group with its
    /// progress bar and Cancel.
    pub(crate) running: Vec<String>,
    /// A job's results, which have the middle group to themselves.
    pub(crate) done: Vec<String>,
    /// The right group's texts in Commander mode, which the layout sizes the
    /// group for at the bar's right end, so the middle group has the rest
    /// (DESIGN.md: The status bar in Commander mode).
    pub(crate) panel_index: Vec<String>,
}

impl StatusCaptions {
    /// Commander mode shows no results, so it leaves out their counts and
    /// totals.
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> StatusCaptions {
        let service = index_status(model);
        let search = model.mode() == Mode::Search;
        StatusCaptions {
            view: if search {
                joined(
                    model.strings(),
                    results_status(model, numbers),
                    selection_status(model, numbers),
                )
            } else {
                String::new()
            },
            job: {
                let (text, progress) = match model.job() {
                    Job::Export => (
                        export_status(model, numbers),
                        if model.exporting() {
                            JobProgress::Unmeasured
                        } else {
                            JobProgress::None
                        },
                    ),
                    Job::Fix => fix_status(model.strings(), numbers, model.fix()),
                    Job::Operation => operation_status(model, numbers),
                };
                JobCaption {
                    text,
                    transfer: model.transfer_caption(),
                    transfer_only: model.job() == Job::Operation
                        && model
                            .operation()
                            .is_some_and(file_operations::StatusOperation::is_transfer),
                    progress,
                    cancel: model.can_cancel_job(),
                }
            },
            right: std::iter::once(ValuePart::plain(service))
                .filter(|part| !part.text.is_empty())
                .chain(
                    status_list(model)
                        .map(|list| totals_status(model, list, numbers))
                        .unwrap_or_default(),
                )
                .collect(),
            indexing: match model.service() {
                (Some(status), None) => status
                    .volumes
                    .iter()
                    .filter_map(|volume| indexing_progress(model.strings(), numbers, volume))
                    .enumerate()
                    .map(|(index, mut caption)| {
                        caption.label = if index == 0 {
                            model.strings().format("status.indexing", &[&caption.label])
                        } else {
                            format!("· {}", caption.label)
                        };
                        caption
                    })
                    .collect(),
                _ => Vec::new(),
            },
            memory: model
                .choices()
                .show_memory
                .then(|| MemoryCaptions::new(model, numbers)),
        }
    }
}

/// A release's version at its widest, which the status bar's middle group
/// is sized for.
const WIDEST_VERSION: &str = "10.10.10";

/// The texts a fix shows in the status bar's middle group while it runs,
/// and once it ended, which the group is sized for. Windows' words for a
/// failure have no widest form, and a development build's instructions
/// would widen the window for a case only developers meet, so a long one
/// is cut off, as an exported file's long name is, and its tooltip shows
/// it whole.
fn fix_widest(strings: &Strings, numbers: &dyn Numbers, sizes: &[u64]) -> [Vec<String>; 2] {
    let version = || WIDEST_VERSION.to_owned();
    let commands = |fix, state| Fixer::Commands { fix, state };
    let setup = |state| Fixer::Setup {
        fix: SetupFix::Update,
        state,
    };
    let fixes = [CommandFix::Start, CommandFix::Install, CommandFix::UseLocal];
    let mut running: Vec<Fixer> = fixes
        .map(|fix| commands(fix, FixState::Running(CommandStep::RunningCommands)))
        .into_iter()
        .chain([
            commands(
                CommandFix::Start,
                FixState::Running(CommandStep::AskingWindows),
            ),
            commands(
                CommandFix::Start,
                FixState::Running(CommandStep::WaitingForService),
            ),
            setup(FixState::Running(SetupStep::FindingRelease)),
            setup(FixState::Running(SetupStep::CheckingFile {
                version: version(),
            })),
            setup(FixState::Running(SetupStep::StartingSetup {
                version: version(),
                path: PathBuf::new(),
            })),
        ])
        .collect();
    running.extend(sizes.iter().map(|&size| {
        setup(FixState::Running(SetupStep::Downloading {
            version: version(),
            bytes: size,
            size,
        }))
    }));
    let failed = CommandResult::CommandsFailed {
        code: u32::MAX,
        said: None,
    };
    let ended: Vec<Fixer> = fixes
        .map(|fix| commands(fix, FixState::Ended(failed.clone())))
        .into_iter()
        .chain(
            [
                CommandResult::Done,
                CommandResult::Cancelled,
                CommandResult::Refused,
                CommandResult::NoAnswer,
            ]
            .map(|result| commands(CommandFix::Start, FixState::Ended(result))),
        )
        .chain(
            [
                SetupResult::Cancelled,
                SetupResult::DownloadFailed,
                SetupResult::Damaged,
            ]
            .map(|result| setup(FixState::Ended(result))),
        )
        .collect();
    [running, ended].map(|fixes| {
        fixes
            .iter()
            .map(|fix| fix_status(strings, numbers, fix).0)
            .collect()
    })
}

pub(super) fn status_widest(strings: &Strings, numbers: &dyn Numbers) -> StatusWidest {
    let limit = numbers.integer(SELECTION_LIMIT);
    let showing = strings.count("status.results", WIDEST_TOTAL, numbers, &[]);
    // Which size is widest depends on the font and the language, so every
    // unit's widest forms are measured: 1,023 bytes, then a three-digit size
    // with two, one and no decimals in each unit.
    let sizes: Vec<u64> =
        std::iter::once(1023)
            .chain((1..=4).flat_map(|unit| {
                [9.99, 99.9, 999.0].map(|value| (value * 1024f64.powi(unit)) as u64)
            }))
            .collect();
    let mut selected: Vec<String> = sizes
        .iter()
        .map(|&n| {
            strings.format(
                "status.selected",
                &[&limit, &text::bytes(strings, numbers, n)],
            )
        })
        .collect();
    selected.push(strings.format("status.selected_only", &[&limit]));
    selected.push(too_many(strings, numbers, WIDEST_TOTAL));
    selected.push(strings.text("status.reading").to_owned());
    let results = [
        strings.format("status.filtered", &[&showing]),
        strings.text("status.searching").to_owned(),
    ];
    let [fix_running, fix_ended] = fix_widest(strings, numbers, &sizes);
    let [operation_running, operation_ended] =
        file_operations::status_widest(strings, numbers, WIDEST_TOTAL);
    StatusWidest {
        indexing: strings.text("status.indexing_short").to_owned(),
        view: results
            .iter()
            .flat_map(|results| {
                selected
                    .iter()
                    .map(|selection| joined(strings, results.clone(), selection.clone()))
            })
            .collect(),
        running: std::iter::once(strings.count("status.exporting", WIDEST_TOTAL, numbers, &[]))
            .chain(fix_running)
            .chain(operation_running)
            .collect(),
        done: std::iter::once(strings.count(
            "status.exported",
            WIDEST_TOTAL,
            numbers,
            &["results.csv"],
        ))
        .chain(fix_ended)
        .chain(operation_ended)
        .collect(),
        panel_index: {
            // An active search panel's index state and totals, as Search
            // shows them; the layout adds the room of their two glyphs.
            let widest = sizes[sizes.len() - 1];
            let files = strings.count(
                "status.files",
                WIDEST_TOTAL,
                numbers,
                &[&text::bytes(strings, numbers, widest)],
            );
            let folders = strings.count("inspector.folders", WIDEST_TOTAL, numbers, &[]);
            let totals = format!("{files} {} {folders}", strings.text("status.and"));
            let volumes = ["status.volume_offline", "status.volume_failed"]
                .map(|key| joined(strings, strings.format(key, &["C:"]), totals.clone()));
            [
                strings.format("panel.not_indexed", &["C:\\"]),
                strings.format("panel.indexing", &["C:\\"]),
                update_text(strings, WIDEST_VERSION),
            ]
            .into_iter()
            .chain(volumes)
            .collect()
        },
    }
}

/// The list whose search the status bar describes: Search's, or in Two
/// panels the active panel's while it searches (DESIGN.md: The status bar in
/// Commander mode).
fn status_list(model: &Model) -> Option<List> {
    match model.mode() {
        Mode::Search => Some(List::Search),
        Mode::TwoPanels => {
            let active = model.panels().active();
            model.panels().search(active).map(|_| List::Panel(active))
        }
    }
}

/// `list`'s file count and size and folder count, each after its icon;
/// a kind with no match is left out. None while the status bar
/// names a search as running, or when the service sent no sums (DESIGN.md:
/// Status bar).
fn totals_status(model: &Model, list: List, numbers: &dyn Numbers) -> Vec<ValuePart> {
    let strings = model.strings();
    let Some(results) = model.list(list).map(SearchList::results) else {
        return Vec::new();
    };
    let Some(sums) = results.sums().filter(|_| !results.search_named()) else {
        return Vec::new();
    };
    let file_bytes = text::bytes(strings, numbers, sums.file_bytes);
    let mut parts: Vec<ValuePart> = [
        (
            Glyph::Page,
            strings.count("status.files", sums.files, numbers, &[&file_bytes]),
            sums.files,
        ),
        (
            Glyph::Folder,
            strings.count("inspector.folders", sums.folders, numbers, &[]),
            sums.folders,
        ),
    ]
    .into_iter()
    .filter(|(_, _, count)| *count > 0)
    .map(|(glyph, text, _)| ValuePart {
        connector: (glyph == Glyph::Page && sums.folders > 0)
            .then(|| strings.text("status.and").to_owned()),
        glyph: Some(glyph),
        text,
        narrow: None,
    })
    .collect();
    if sums.files > 0 {
        let tip = (parts.iter())
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join("\r\n");
        parts[0].narrow = Some(Narrow {
            // The inspector counts files in the same words.
            text: strings.count("inspector.files", sums.files, numbers, &[]),
            tip,
        });
    }
    parts
}

/// How many results, or that a search runs once the status bar names it
/// (DESIGN.md: Searching). A filter that the hidden pane cannot show adds
/// "· Filtered" (DESIGN.md: S10).
pub(crate) fn results_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    let results = model.search_list().results();
    if results.search_named() {
        return strings.text("status.searching").to_owned();
    }
    match results.shown() {
        Shown::Rows(_) => {
            let count = strings.count("status.results", results.total(), numbers, &[]);
            if model.filters().is_set() && !model.panes().filters {
                strings.format("status.filtered", &[&count])
            } else {
                count
            }
        }
        Shown::Nothing | Shown::Message(_) => String::new(),
    }
}

/// How many entries are selected and the size of the selected files, or
/// that the selection is read.
pub(crate) fn selection_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    match model.search_list().selection() {
        Selection::None => String::new(),
        Selection::Reading { .. } | Selection::Filtering { .. } => {
            strings.text("status.reading").to_owned()
        }
        Selection::TooMany(count) => too_many(strings, numbers, *count),
        Selection::Files(_) | Selection::Folder { .. } => {
            let sums = model.search_list().selection_sums();
            let count = numbers.integer(sums.entries());
            // The size counts files only, so folders alone have none.
            if sums.files == 0 {
                strings.format("status.selected_only", &[&count])
            } else {
                strings.format(
                    "status.selected",
                    &[&count, &text::bytes(strings, numbers, sums.bytes)],
                )
            }
        }
    }
}

/// The export's progress, or the last export's result.
pub(crate) fn export_status(model: &Model, numbers: &dyn Numbers) -> String {
    let strings = model.strings();
    match model.export() {
        Export::Idle => String::new(),
        Export::Running { rows, .. } => strings.count("status.exporting", *rows, numbers, &[]),
        Export::Done { path, rows } => {
            let path = path.to_string_lossy();
            strings.count("status.exported", *rows, numbers, &[text::name_of(&path)])
        }
    }
}

/// The file operation's line, with its bar while it runs (DESIGN.md: File
/// operations).
fn operation_status(model: &Model, numbers: &dyn Numbers) -> (String, JobProgress) {
    let Some(operation) = model.operation() else {
        return (String::new(), JobProgress::None);
    };
    if operation.is_transfer() {
        return (String::new(), JobProgress::None);
    }
    let (mut text, bar) =
        file_operations::status_line(model.strings(), numbers, operation, model.operation_speed());
    let backlog = model.operation_backlog();
    if !backlog.is_empty() {
        if matches!(
            operation,
            file_operations::StatusOperation::Pending {
                phase: file_operations::Pending::Interrupted,
                ..
            }
        ) {
            text = backlog.to_owned();
        } else {
            text.push_str(model.strings().text("file_operations.separator"));
            text.push_str(backlog);
        }
    }
    let progress = match bar {
        None => JobProgress::None,
        Some(Bar::Moving) => JobProgress::Unmeasured,
        Some(Bar::Measured(measure)) => JobProgress::Measured {
            done: measure.done,
            total: measure.total,
        },
    };
    (text, progress)
}

/// A fix's step, with its progress, or its result (DESIGN.md: Service
/// fixes).
pub(super) fn fix_status(
    strings: &Strings,
    numbers: &dyn Numbers,
    fix: &Fixer,
) -> (String, JobProgress) {
    let ended = |text: String| (text, JobProgress::None);
    match fix {
        Fixer::Idle => ended(String::new()),
        Fixer::Commands {
            fix,
            state: FixState::Running(step),
        } => command_step(strings, *fix, step),
        Fixer::Commands {
            fix,
            state: FixState::Ended(result),
        } => ended(command_result(strings, *fix, result)),
        Fixer::Setup {
            state: FixState::Running(step),
            ..
        } => setup_step(strings, numbers, step),
        Fixer::Setup {
            state: FixState::Ended(result),
            ..
        } => ended(setup_result(strings, result)),
    }
}

fn command_step(strings: &Strings, fix: CommandFix, step: &CommandStep) -> (String, JobProgress) {
    let text = |key: &'static str| strings.text(key).to_owned();
    match step {
        CommandStep::AskingWindows => (text("fixing.asking"), JobProgress::None),
        CommandStep::RunningCommands => {
            let key = match fix {
                CommandFix::Start => "fixing.starting",
                CommandFix::Install => "fixing.installing",
                CommandFix::UseLocal => "fixing.updating",
            };
            (text(key), JobProgress::Unmeasured)
        }
        CommandStep::WaitingForService => (text("fixing.waiting"), JobProgress::Unmeasured),
    }
}

fn setup_step(strings: &Strings, numbers: &dyn Numbers, step: &SetupStep) -> (String, JobProgress) {
    let text = |key: &'static str| strings.text(key).to_owned();
    match step {
        SetupStep::FindingRelease => (text("fixing.finding"), JobProgress::Unmeasured),
        SetupStep::Confirming { .. } => (text("fixing.finding"), JobProgress::None),
        SetupStep::Downloading {
            version,
            bytes,
            size,
        } => (
            strings.format(
                "fixing.downloading",
                &[
                    version,
                    &text::bytes(strings, numbers, *bytes),
                    &text::bytes(strings, numbers, *size),
                ],
            ),
            JobProgress::Measured {
                done: *bytes,
                total: *size,
            },
        ),
        SetupStep::CheckingFile { .. } => (text("fixing.checking"), JobProgress::Unmeasured),
        SetupStep::StartingSetup { version, .. } => (
            strings.format("fixing.closing", &[version]),
            JobProgress::None,
        ),
    }
}

fn command_result(strings: &Strings, fix: CommandFix, result: &CommandResult) -> String {
    let text = |key: &'static str| strings.text(key).to_owned();
    match result {
        CommandResult::Done => String::new(),
        CommandResult::Cancelled => text("fixing.cancelled"),
        CommandResult::Refused => text("fixing.refused"),
        CommandResult::Failed(why) => strings.format("fixing.failed", &[why]),
        CommandResult::CommandsFailed { code, said } => {
            let reason = said
                .clone()
                .unwrap_or_else(|| strings.format("fixing.exit_code", &[&code.to_string()]));
            let key = match fix {
                CommandFix::Start => "fixing.start_failed",
                CommandFix::Install => "fixing.install_failed",
                CommandFix::UseLocal => "fixing.update_failed",
            };
            strings.format(key, &[&reason])
        }
        CommandResult::NoAnswer => text("fixing.no_answer"),
    }
}

fn setup_result(strings: &Strings, result: &SetupResult) -> String {
    let text = |key: &'static str| strings.text(key).to_owned();
    match result {
        SetupResult::Cancelled => text("fixing.download_cancelled"),
        // The person said no, or a message box said why there is no
        // setup, so the status bar adds nothing.
        SetupResult::Declined | SetupResult::NoSetup => String::new(),
        SetupResult::DownloadFailed => text("fixing.download_failed"),
        SetupResult::NotSaved { folder, why } => {
            strings.format("fixing.not_saved", &[&folder.display().to_string(), why])
        }
        SetupResult::Damaged => text("fixing.damaged"),
        SetupResult::Failed(why) => strings.format("fixing.setup_failed", &[why]),
    }
}

fn indexing_progress(
    strings: &Strings,
    numbers: &dyn Numbers,
    volume: &gozler_core::proto::VolumeStatus,
) -> Option<IndexingCaption> {
    if !matches!(
        volume.phase,
        VolumePhase::Bootstrapping | VolumePhase::Rescanning
    ) {
        return None;
    }
    let prefix = volume.mounted().display_prefix();
    let (value, progress) = match volume.progress {
        Some(IndexProgress::Scanning {
            percent: Some(percent),
            ..
        }) => (
            strings.format(
                "status.indexing_percent",
                &[&numbers.integer(u64::from(percent))],
            ),
            JobProgress::Measured {
                done: u64::from(percent),
                total: 100,
            },
        ),
        Some(IndexProgress::Scanning { entries, .. } | IndexProgress::Preparing { entries }) => (
            strings.count("status.indexing_entries", entries, numbers, &[]),
            JobProgress::Unmeasured,
        ),
        None => (String::new(), JobProgress::Unmeasured),
    };
    Some(IndexingCaption {
        label: prefix.trim_end_matches('\\').to_owned(),
        compact: strings.text("status.indexing_short").to_owned(),
        value,
        progress,
    })
}

/// The status bar's right group: the service's failure while the list
/// shows rows or another message, so the window says that the service
/// stopped also when it keeps its rows (DESIGN.md: T10); else the index
/// state, only when it affects the answer (DESIGN.md: Status bar); else a
/// newer release that Check for updates found (DESIGN.md: The General
/// page).
pub(crate) fn index_status(model: &Model) -> String {
    let strings = model.strings();
    let list = status_list(model);
    let shows_failure = (list.and_then(|list| model.list(list))).is_some_and(|list| {
        matches!(
            list.results().shown(),
            Shown::Message(ListMessage::Failure(_))
        )
    });
    if let (_, Some(failure)) = model.service()
        && !shows_failure
    {
        return failure_text(strings, failure);
    }
    let Some(list) = list else {
        return panel_index_status(model)
            .or_else(|| {
                model
                    .newer_release()
                    .map(|version| update_text(strings, version))
            })
            .unwrap_or_default();
    };
    let coverage = model.coverage(list);
    if let Some(volume) = coverage.volumes().find(|volume| {
        matches!(
            volume.phase,
            VolumePhase::Offline | VolumePhase::Failed { .. }
        )
    }) {
        let key = match volume.phase {
            VolumePhase::Live
            | VolumePhase::Snapshot
            | VolumePhase::Bootstrapping
            | VolumePhase::Rescanning => {
                unreachable!("only offline and failed volumes reach this branch")
            }
            VolumePhase::Offline => "status.volume_offline",
            VolumePhase::Failed { .. } => "status.volume_failed",
        };
        return strings.format(key, &[volume.mounted().display_prefix()]);
    }
    coverage
        .sizes_or_dates_unknown()
        .filter(|volume| volume.phase.is_complete())
        .map(|volume| strings.format("status.metadata", &[volume.mounted().display_prefix()]))
        .or_else(|| {
            model
                .newer_release()
                .map(|version| update_text(strings, version))
        })
        .unwrap_or_default()
}

/// Why the active panel's folder sizes may be unknown, as the status bar's
/// right group says it in Commander mode (DESIGN.md: Folder sizes).
fn panel_index_status(model: &Model) -> Option<String> {
    let strings = model.strings();
    let coverage = model.panel_coverage()?;
    let Some(volume) = coverage.volumes().next() else {
        let panels = model.panels();
        let root = panels.panel(panels.active()).root();
        return Some(strings.format("panel.not_indexed", &[&root]));
    };
    (matches!(
        volume.phase,
        VolumePhase::Offline | VolumePhase::Failed { .. }
    ) || (volume.phase.is_complete() && volume.metadata_pending))
        .then(|| strings.format("panel.indexing", &[volume.mounted().display_prefix()]))
}

fn update_text(strings: &Strings, version: &str) -> String {
    strings.format("status.update_available", &[version])
}

/// Whether the status bar's right group names a newer release, so a click
/// on it runs Update (DESIGN.md: The General page).
pub(crate) fn shows_update(model: &Model) -> bool {
    model
        .newer_release()
        .is_some_and(|version| index_status(model) == update_text(model.strings(), version))
}
