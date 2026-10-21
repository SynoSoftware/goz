//! What the person asks for besides the typed text: the match controls, the
//! filters, the sort, and the search that they make with the text.

use super::glyph::Glyph;
use gozler_core::proto::VolumeStatus;
use gozler_core::query::{QueryError, QueryStart, SIZE_WORDS, TermMatch};
use gozler_core::search::{Search, SearchInput, TextMatch};
use gozler_core::types::{SortDir, SortKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The search that `text`, the match controls and the filters ask for. The
/// window parses with no working folder, so a relative path is the parser's
/// [`QueryError::RelativePath`], which asks for the full path.
pub(crate) fn search(
    text: &str,
    matching: Matching,
    filters: &Filters,
    query_start: &QueryStart,
) -> Result<Search, QueryError> {
    let text_match = match matching.mode {
        MatchMode::StartOfWords => TextMatch::Terms(TermMatch::WordStart),
        MatchMode::StartOfName => TextMatch::Terms(TermMatch::NameStart),
        MatchMode::Anywhere => TextMatch::Terms(TermMatch::Anywhere),
        MatchMode::WholeWords => TextMatch::Terms(TermMatch::WholeWords),
        MatchMode::RegularExpression => TextMatch::Regex,
    };
    let folder = filters.location.as_deref();
    let (scope, parent) = if filters.only_directly {
        (None, folder)
    } else {
        (folder, None)
    };
    Search::new(&SearchInput {
        text,
        text_match,
        match_case: matching.case,
        match_path: matching.path,
        filters: &filters.clauses(),
        regexes: &[],
        scope,
        parent,
        working_dir: None,
        query_start,
    })
}

/// How the typed name terms match; one choice, because the modes exclude
/// each other (DESIGN.md: Search box). Its names are `ui.json`'s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatchMode {
    #[default]
    StartOfWords,
    StartOfName,
    Anywhere,
    WholeWords,
    RegularExpression,
}

impl MatchMode {
    /// In the Match list's order.
    pub(crate) const ALL: [MatchMode; 5] = [
        MatchMode::StartOfWords,
        MatchMode::StartOfName,
        MatchMode::Anywhere,
        MatchMode::WholeWords,
        MatchMode::RegularExpression,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            MatchMode::StartOfWords => "match.start_of_words",
            MatchMode::StartOfName => "match.start_of_name",
            MatchMode::Anywhere => "match.anywhere",
            MatchMode::WholeWords => "match.whole_words",
            MatchMode::RegularExpression => "match.regular_expression",
        }
    }
}

/// The match controls beside the search box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Matching {
    pub(crate) mode: MatchMode,
    pub(crate) case: bool,
    pub(crate) path: bool,
}

/// How many chosen folders the Location list keeps: as many as it shows
/// rows, so the list stays a short choice of places, not a history.
const CHOSEN_FOLDERS: usize = 5;

/// The filter pane's choices (DESIGN.md: Filter pane). Each group but Location
/// stands for query text that the parser reads on its own, so the window
/// and `gozler` read a filter alike. Its names are `ui.json`'s.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Filters {
    pub(crate) kind: Kind,
    /// The folder the search is limited to, as `-path`; `None` is
    /// Everywhere.
    pub(crate) location: Option<String>,
    /// The folders that Choose folder… and Search in this folder chose,
    /// oldest first. Each stays an item of the Location list while another
    /// item is chosen, so the person can go back to it, and the list does not
    /// change under their arrow keys.
    pub(crate) chosen_folders: Vec<String>,
    /// Only the entries directly in the Location folder, as `-parent`.
    pub(crate) only_directly: bool,
    pub(crate) date: DateModified,
    pub(crate) size: SizeFilter,
    /// In [`ATTRIBUTES`]' order.
    pub(crate) attributes: [AttributeChoice; 6],
}

impl Filters {
    /// Whether a group is set to another choice than its first, which
    /// Clear filters undoes (DESIGN.md: C21).
    pub(crate) fn is_set(&self) -> bool {
        *self
            != Filters {
                chosen_folders: self.chosen_folders.clone(),
                ..Filters::default()
            }
    }

    /// Every group at its first choice; the chosen folders stay Location
    /// items.
    pub(crate) fn cleared(&self) -> Filters {
        Filters {
            chosen_folders: self.chosen_folders.clone(),
            ..Filters::default()
        }
    }

    /// Location `folder`, which joins the chosen folders unless it is one of
    /// them already. Past [`CHOSEN_FOLDERS`], the oldest leaves.
    pub(crate) fn choose_folder(&mut self, folder: String) {
        if !self
            .chosen_folders
            .iter()
            .any(|chosen| chosen.eq_ignore_ascii_case(&folder))
        {
            self.chosen_folders.push(folder.clone());
            let extra = self.chosen_folders.len().saturating_sub(CHOSEN_FOLDERS);
            self.chosen_folders.drain(..extra);
        }
        self.location = Some(folder);
    }

    /// The query text of each set group but Location.
    pub(crate) fn clauses(&self) -> Vec<String> {
        let attributes = ATTRIBUTES.iter().zip(self.attributes).filter_map(
            |((letter, _), choice)| match choice {
                AttributeChoice::Any => None,
                AttributeChoice::Only => Some(format!("attrib:{letter}")),
                AttributeChoice::Exclude => Some(format!("!attrib:{letter}")),
            },
        );
        [self.kind.clause(), self.date.clause(), self.size.clause()]
            .into_iter()
            .flatten()
            .chain(attributes)
            .collect()
    }
}

/// The Kind group's choices, in the list's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    #[default]
    All,
    Folders,
    Files,
    Audio,
    Archives,
    Documents,
    Executables,
    Pictures,
    Videos,
}

impl Kind {
    pub(crate) const ALL: [Kind; 9] = [
        Kind::All,
        Kind::Folders,
        Kind::Files,
        Kind::Audio,
        Kind::Archives,
        Kind::Documents,
        Kind::Executables,
        Kind::Pictures,
        Kind::Videos,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Kind::All => "filter.all",
            Kind::Folders => "filter.folders",
            Kind::Files => "filter.files",
            Kind::Audio => "filter.audio",
            Kind::Archives => "filter.archives",
            Kind::Documents => "filter.documents",
            Kind::Executables => "filter.executables",
            Kind::Pictures => "filter.pictures",
            Kind::Videos => "filter.videos",
        }
    }

    /// The icon before its label in the Kind list.
    pub(crate) fn glyph(self) -> Glyph {
        match self {
            Kind::All => Glyph::ViewAll,
            Kind::Folders => Glyph::Folder,
            Kind::Files => Glyph::Page,
            Kind::Audio => Glyph::MusicNote,
            Kind::Archives => Glyph::Package,
            Kind::Documents => Glyph::Document,
            Kind::Executables => Glyph::CommandPrompt,
            Kind::Pictures => Glyph::Picture,
            Kind::Videos => Glyph::Video,
        }
    }

    fn clause(self) -> Option<String> {
        let extensions = match self {
            Kind::All => return None,
            Kind::Folders => return Some("folder:".to_owned()),
            Kind::Files => return Some("file:".to_owned()),
            Kind::Audio => "mp3;wav;flac;aac;ogg;opus;wma;m4a",
            Kind::Archives => "zip;7z;rar;tar;gz;bz2;xz;zst;cab;iso",
            Kind::Documents => "pdf;doc;docx;xls;xlsx;ppt;pptx;odt;ods;odp;rtf;txt;md;csv",
            Kind::Executables => "exe;msi;bat;cmd;ps1;com;scr",
            Kind::Pictures => "jpg;jpeg;png;gif;bmp;webp;tif;tiff;heic;svg;ico",
            Kind::Videos => "mp4;mkv;avi;mov;wmv;webm;m4v;mpg;mpeg",
        };
        Some(format!("ext:{extensions}"))
    }
}

/// An item of the Location list: Everywhere, each indexed volume by its
/// mount prefix, then the chosen folders (DESIGN.md: Filter pane).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LocationItem<'a> {
    Everywhere,
    Volume(&'a VolumeStatus),
    Folder(&'a str),
}

impl LocationItem<'_> {
    /// The folder the item limits the search to.
    pub(crate) fn folder(&self) -> Option<&str> {
        match self {
            LocationItem::Everywhere => None,
            LocationItem::Volume(volume) => Some(volume.mounted().display_prefix()),
            LocationItem::Folder(folder) => Some(folder),
        }
    }
}

/// The Location list's items for `filters` and the latest status's
/// `volumes`. A Location that is none of them, such as a saved volume
/// before the first status reply, is an item of its own, so the list always
/// shows the Location that is set.
pub(crate) fn location_items<'a>(
    filters: &'a Filters,
    volumes: &'a [VolumeStatus],
) -> Vec<LocationItem<'a>> {
    let mut items = vec![LocationItem::Everywhere];
    items.extend(volumes.iter().map(LocationItem::Volume));
    for folder in filters.chosen_folders.iter().chain(&filters.location) {
        if !items.iter().any(|item| {
            item.folder()
                .is_some_and(|f| f.eq_ignore_ascii_case(folder))
        }) {
            items.push(LocationItem::Folder(folder));
        }
    }
    items
}

/// The position of each filter's choice in its list, which the filter pane
/// selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ListPositions {
    pub(crate) kind: usize,
    pub(crate) location: usize,
    pub(crate) date: usize,
    pub(crate) size: usize,
    pub(crate) attributes: [usize; 6],
}

impl Filters {
    /// The positions of the choices, with `locations` the Location list's
    /// items for these filters.
    pub(crate) fn positions(&self, locations: &[LocationItem]) -> ListPositions {
        let found =
            |position: Option<usize>| position.expect("each list holds its filter's choice");
        ListPositions {
            kind: found(Kind::ALL.iter().position(|k| *k == self.kind)),
            location: found(locations.iter().position(|item| {
                match (item.folder(), self.location.as_deref()) {
                    (None, None) => true,
                    (Some(item), Some(location)) => item.eq_ignore_ascii_case(location),
                    _ => false,
                }
            })),
            date: found(
                date_items(self.date)
                    .iter()
                    .position(|item| *item == RangeItem::Choice(self.date)),
            ),
            size: found(
                size_items(self.size)
                    .iter()
                    .position(|item| *item == RangeItem::Choice(self.size)),
            ),
            attributes: self
                .attributes
                .map(|choice| found(AttributeChoice::ALL.iter().position(|a| *a == choice))),
        }
    }
}

/// A calendar day, saved as `2026-10-11`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Day(pub(crate) jiff::civil::Date);

impl Serialize for Day {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Day {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Day, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map(Day).map_err(serde::de::Error::custom)
    }
}

/// A custom date range of Date modified; From and To include their days.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DateRange {
    From(Day),
    Until(Day),
    Between(Day, Day),
}

/// Why the Custom date dialog's input is not a range (DESIGN.md: Dialogs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DateRangeError {
    /// Neither check box is on.
    Neither,
    FromAfterTo,
}

impl DateRange {
    pub(crate) fn new(from: Option<Day>, to: Option<Day>) -> Result<DateRange, DateRangeError> {
        match (from, to) {
            (None, None) => Err(DateRangeError::Neither),
            (Some(from), None) => Ok(DateRange::From(from)),
            (None, Some(to)) => Ok(DateRange::Until(to)),
            (Some(from), Some(to)) if from > to => Err(DateRangeError::FromAfterTo),
            (Some(from), Some(to)) => Ok(DateRange::Between(from, to)),
        }
    }

    /// The days the dialog shows, From and To.
    pub(crate) fn days(self) -> (Option<Day>, Option<Day>) {
        match self {
            DateRange::From(from) => (Some(from), None),
            DateRange::Until(to) => (None, Some(to)),
            DateRange::Between(from, to) => (Some(from), Some(to)),
        }
    }
}

/// The Date modified group's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DateModified {
    #[default]
    Any,
    Today,
    Yesterday,
    #[serde(rename = "last_7_days")]
    Last7Days,
    #[serde(rename = "last_30_days")]
    Last30Days,
    #[serde(rename = "last_365_days")]
    Last365Days,
    Custom(DateRange),
}

impl DateModified {
    /// The choices before a custom range, in the list's order.
    const PRESETS: [DateModified; 6] = [
        DateModified::Any,
        DateModified::Today,
        DateModified::Yesterday,
        DateModified::Last7Days,
        DateModified::Last30Days,
        DateModified::Last365Days,
    ];

    fn clause(self) -> Option<String> {
        Some(match self {
            DateModified::Any => return None,
            DateModified::Today => "dm:today".to_owned(),
            DateModified::Yesterday => "dm:yesterday".to_owned(),
            DateModified::Last7Days => "dm:7days".to_owned(),
            DateModified::Last30Days => "dm:30days".to_owned(),
            DateModified::Last365Days => "dm:365days".to_owned(),
            DateModified::Custom(DateRange::From(from)) => format!("dm:>={}", from.0),
            DateModified::Custom(DateRange::Until(to)) => format!("dm:<={}", to.0),
            DateModified::Custom(DateRange::Between(from, to)) => {
                format!("dm:{}..{}", from.0, to.0)
            }
        })
    }
}

/// An item of the Date modified or Size list: a choice, or Custom…, which
/// opens its dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RangeItem<T> {
    Choice(T),
    Custom,
}

/// The Date modified list for the choice `chosen`: the presets, a custom
/// range just above Custom… while one is set, so the list always shows the
/// filter that is set, then Custom….
pub(crate) fn date_items(chosen: DateModified) -> Vec<RangeItem<DateModified>> {
    let custom = matches!(chosen, DateModified::Custom(_)).then_some(chosen);
    DateModified::PRESETS
        .into_iter()
        .chain(custom)
        .map(RangeItem::Choice)
        .chain([RangeItem::Custom])
        .collect()
}

/// The size words of `size:`, in [`SIZE_WORDS`]' order; their names are
/// the words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SizeWord {
    Empty,
    Tiny,
    Small,
    Medium,
    Large,
    Huge,
    Gigantic,
}

impl SizeWord {
    pub(crate) const ALL: [SizeWord; 7] = [
        SizeWord::Empty,
        SizeWord::Tiny,
        SizeWord::Small,
        SizeWord::Medium,
        SizeWord::Large,
        SizeWord::Huge,
        SizeWord::Gigantic,
    ];

    /// The word and the range that the parser reads it as.
    pub(crate) fn word(self) -> &'static gozler_core::query::SizeWord {
        &SIZE_WORDS[self as usize]
    }

    /// Its label, whose `{0}` is its range.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SizeWord::Empty => "filter.empty",
            SizeWord::Tiny => "filter.tiny",
            SizeWord::Small => "filter.small",
            SizeWord::Medium => "filter.medium",
            SizeWord::Large => "filter.large",
            SizeWord::Huge => "filter.huge",
            SizeWord::Gigantic => "filter.gigantic",
        }
    }
}

const _: () = assert!(
    SizeWord::ALL.len() == SIZE_WORDS.len(),
    "every word of gozler's size table has its choice"
);

/// The unit of a custom size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SizeUnit {
    Kb,
    #[default]
    Mb,
    Gb,
}

impl SizeUnit {
    pub(crate) const ALL: [SizeUnit; 3] = [SizeUnit::Kb, SizeUnit::Mb, SizeUnit::Gb];

    pub(crate) fn bytes(self) -> u64 {
        match self {
            SizeUnit::Kb => 1 << 10,
            SizeUnit::Mb => 1 << 20,
            SizeUnit::Gb => 1 << 30,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            SizeUnit::Kb => "unit.kb",
            SizeUnit::Mb => "unit.mb",
            SizeUnit::Gb => "unit.gb",
        }
    }
}

/// A custom size range in bytes, either end open, and the unit the person
/// typed it in, which it shows in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SizeRange {
    pub(crate) min: Option<u64>,
    pub(crate) max: Option<u64>,
    pub(crate) unit: SizeUnit,
}

/// Why the Custom size dialog's input is not a range (DESIGN.md: Dialogs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SizeRangeError {
    MinimumNotANumber,
    MaximumNotANumber,
    Neither,
    MinimumAboveMaximum,
}

impl SizeRange {
    /// The range of the dialog's `minimum` and `maximum`, numbers written
    /// with the Windows decimal separator `separator`, in `unit`.
    pub(crate) fn parse(
        minimum: &str,
        maximum: &str,
        unit: SizeUnit,
        separator: &str,
    ) -> Result<SizeRange, SizeRangeError> {
        let min = bytes(minimum, unit, separator).ok_or(SizeRangeError::MinimumNotANumber)?;
        let max = bytes(maximum, unit, separator).ok_or(SizeRangeError::MaximumNotANumber)?;
        match (min, max) {
            (None, None) => Err(SizeRangeError::Neither),
            (Some(min), Some(max)) if min > max => Err(SizeRangeError::MinimumAboveMaximum),
            _ => Ok(SizeRange { min, max, unit }),
        }
    }

    /// The dialog's Minimum and Maximum for the range, in its unit with the
    /// separator `separator`, as [`SizeRange::parse`] reads them back.
    pub(crate) fn fields(&self, separator: &str) -> (String, String) {
        let field = |bytes: Option<u64>| {
            bytes.map_or_else(String::new, |bytes| {
                let value = format!("{:.2}", bytes as f64 / self.unit.bytes() as f64);
                let value = value.trim_end_matches('0').trim_end_matches('.');
                value.replace('.', separator)
            })
        };
        (field(self.min), field(self.max))
    }
}

/// `text` in `unit` as bytes: `Some(None)` when it is empty, `None` when
/// it is not a number of at most one `separator` and digits, or the bytes
/// do not fit.
fn bytes(text: &str, unit: SizeUnit, separator: &str) -> Option<Option<u64>> {
    let text = text.trim();
    if text.is_empty() {
        return Some(None);
    }
    let (whole, fraction) = text.split_once(separator).unwrap_or((text, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() && fraction.is_empty() || !digits(whole) || !digits(fraction) {
        return None;
    }
    let value: f64 = format!("{whole}.{fraction}0").parse().ok()?;
    let bytes = (value * unit.bytes() as f64).round();
    (bytes < u64::MAX as f64).then_some(Some(bytes as u64))
}

/// The Size group's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SizeFilter {
    #[default]
    Any,
    Word(SizeWord),
    Custom(SizeRange),
}

impl SizeFilter {
    fn clause(self) -> Option<String> {
        match self {
            SizeFilter::Any => None,
            SizeFilter::Word(word) => Some(format!("size:{}", word.word().word)),
            SizeFilter::Custom(range) => {
                let end = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_default();
                Some(format!("size:{}..{}", end(range.min), end(range.max)))
            }
        }
    }
}

/// The Size list for the choice `chosen`, as [`date_items`] builds the Date
/// modified list.
pub(crate) fn size_items(chosen: SizeFilter) -> Vec<RangeItem<SizeFilter>> {
    let custom = matches!(chosen, SizeFilter::Custom(_)).then_some(chosen);
    std::iter::once(SizeFilter::Any)
        .chain(SizeWord::ALL.map(SizeFilter::Word))
        .chain(custom)
        .map(RangeItem::Choice)
        .chain([RangeItem::Custom])
        .collect()
}

/// One attribute's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttributeChoice {
    #[default]
    Any,
    Only,
    Exclude,
}

impl AttributeChoice {
    pub(crate) const ALL: [AttributeChoice; 3] = [
        AttributeChoice::Any,
        AttributeChoice::Only,
        AttributeChoice::Exclude,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            AttributeChoice::Any => "filter.attribute_any",
            AttributeChoice::Only => "filter.attribute_only",
            AttributeChoice::Exclude => "filter.attribute_exclude",
        }
    }
}

/// The attributes the pane offers, with their `attrib:` letter and label;
/// other letters are left to `attrib:` in the search box (DESIGN.md: Filter
/// pane).
pub(crate) const ATTRIBUTES: [(char, &str); 6] = [
    ('h', "filter.hidden"),
    ('s', "filter.system"),
    ('r', "filter.read_only"),
    ('l', "filter.link"),
    ('c', "filter.compressed"),
    ('e', "filter.encrypted"),
];

/// The sort keys in View > Sort by's order.
pub(crate) const SORT_KEYS: [SortKey; 6] = [
    SortKey::Name,
    SortKey::Path,
    SortKey::Size,
    SortKey::DateModified,
    SortKey::DateCreated,
    SortKey::Extension,
];

pub(crate) const SORT_DIRECTIONS: [SortDir; 2] = [SortDir::Asc, SortDir::Desc];

pub(crate) fn sort_key_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "sort.name",
        SortKey::Path => "sort.path",
        SortKey::Size => "sort.size",
        SortKey::DateModified => "sort.date_modified",
        SortKey::DateCreated => "sort.date_created",
        SortKey::Extension => "sort.extension",
    }
}

pub(crate) fn sort_direction_label(direction: SortDir) -> &'static str {
    match direction {
        SortDir::Asc => "sort.ascending",
        SortDir::Desc => "sort.descending",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gozler_core::escompat::{Command, build_search, parse_argv};
    use gozler_core::query::Test;

    fn day(text: &str) -> Day {
        Day(text.parse().expect("a date"))
    }

    /// A filter that the window reads otherwise than the `gozler` query it
    /// stands for shows other files than the command line, and no other
    /// check compares the two for every group (DESIGN.md: The window).
    #[test]
    fn each_filter_group_searches_as_its_gozler_query() {
        let start = QueryStart {
            now: "2026-03-04T12:00:00Z".parse().expect("an instant"),
            time_zone: jiff::tz::TimeZone::UTC,
        };
        let documents = "ext:pdf;doc;docx;xls;xlsx;ppt;pptx;odt;ods;odp;rtf;txt;md;csv";
        let folder = r"C:\repo".to_owned();
        let mut attributes = [AttributeChoice::Any; 6];
        attributes[0] = AttributeChoice::Exclude;
        attributes[2] = AttributeChoice::Only;
        let cases: [(&str, Filters, &[&str]); 7] = [
            (
                "report",
                Filters {
                    kind: Kind::Documents,
                    ..Filters::default()
                },
                &["report", documents],
            ),
            (
                "report",
                Filters {
                    location: Some(folder.clone()),
                    only_directly: true,
                    ..Filters::default()
                },
                &["report", "-parent", r"C:\repo"],
            ),
            (
                "",
                Filters {
                    location: Some(folder.clone()),
                    date: DateModified::Last7Days,
                    ..Filters::default()
                },
                &["-path", r"C:\repo", "dm:7days"],
            ),
            (
                "",
                Filters {
                    date: DateModified::Custom(DateRange::Between(
                        day("2026-01-01"),
                        day("2026-03-31"),
                    )),
                    ..Filters::default()
                },
                &["dm:2026-01-01..2026-03-31"],
            ),
            (
                "",
                Filters {
                    size: SizeFilter::Word(SizeWord::Large),
                    ..Filters::default()
                },
                &["size:large"],
            ),
            (
                "",
                Filters {
                    size: SizeFilter::Custom(SizeRange {
                        min: Some(1_572_864),
                        max: None,
                        unit: SizeUnit::Mb,
                    }),
                    ..Filters::default()
                },
                &["size:>=1572864"],
            ),
            (
                "",
                Filters {
                    attributes,
                    ..Filters::default()
                },
                &["!attrib:h", "attrib:r"],
            ),
        ];
        // Anywhere is the command line's own match mode, so the whole
        // query and the scope must be the same.
        let anywhere = Matching {
            mode: MatchMode::Anywhere,
            ..Matching::default()
        };
        for (text, filters, argv) in cases {
            let window = search(text, anywhere, &filters, &start).expect("parses");
            let args: Vec<String> = argv.iter().map(ToString::to_string).collect();
            let Ok(Command::Run(plan)) = parse_argv(&args) else {
                panic!("{argv:?} is a search")
            };
            let gozler = build_search(&plan, r"C:\", &start).expect("parses");
            assert_eq!(
                (&window.query, window.scope.as_deref()),
                (&gozler.query, gozler.scope.as_deref()),
                "{filters:?}"
            );
        }
        // On one command line the last `ext:` wins, so a typed `ext:rs`
        // joined to Kind would be lost; the window keeps both clauses.
        let documents_only = Filters {
            kind: Kind::Documents,
            ..Filters::default()
        };
        let window = search("ext:rs", anywhere, &documents_only, &start).expect("parses");
        let extensions: Vec<&Test> = window
            .query
            .clauses
            .iter()
            .flat_map(|clause| &clause.terms)
            .map(|term| &term.test)
            .collect();
        assert!(
            matches!(extensions.as_slice(), [Test::Extension(rs), Test::Extension(documents)]
                if rs == &["rs"] && documents.len() == 14),
            "{extensions:?}"
        );
    }

    /// A chosen folder that replaced the one before would make the person
    /// pick an earlier folder again in the folder picker.
    #[test]
    fn choosing_a_folder_keeps_the_earlier_choices() {
        let mut filters = Filters::default();
        for folder in [r"C:\one", r"C:\two", r"c:\ONE"] {
            filters.choose_folder(folder.into());
        }
        assert_eq!(filters.chosen_folders, [r"C:\one", r"C:\two"]);
        assert_eq!(filters.location.as_deref(), Some(r"c:\ONE"));
        for folder in [r"C:\c", r"C:\d", r"C:\e", r"C:\six"] {
            filters.choose_folder(folder.into());
        }
        assert_eq!(
            filters.chosen_folders,
            [r"C:\two", r"C:\c", r"C:\d", r"C:\e", r"C:\six"]
        );
    }

    /// A comma decimal separator read as a dot would make "1,5" a thousand
    /// times larger in a German Windows, or refuse it.
    #[test]
    fn a_custom_size_reads_the_windows_decimal_separator() {
        assert_eq!(
            SizeRange::parse("1,5", "", SizeUnit::Mb, ","),
            Ok(SizeRange {
                min: Some(1_572_864),
                max: None,
                unit: SizeUnit::Mb
            })
        );
        assert_eq!(
            SizeRange::parse("1.5", "", SizeUnit::Mb, ","),
            Err(SizeRangeError::MinimumNotANumber)
        );
        assert_eq!(
            SizeRange::parse("2", "1", SizeUnit::Kb, "."),
            Err(SizeRangeError::MinimumAboveMaximum)
        );
    }
}
