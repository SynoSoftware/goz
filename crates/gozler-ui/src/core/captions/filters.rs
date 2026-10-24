use crate::core::model::Model;
use crate::core::query::{
    ATTRIBUTES, AttributeChoice, DateModified, DateRange, Day, Kind, LocationItem, RangeItem,
    SizeFilter, SizeRange, SizeUnit, SizeWord, date_items, size_items,
};
use crate::core::text::{self, Numbers, Strings};
use gozler_core::types::VolumePhase;

/// The filter pane's captions and the items of its lists, in the lists'
/// orders.
pub(crate) struct FilterCaptions {
    pub(crate) title: String,
    pub(crate) clear: String,
    pub(crate) clear_tip: String,
    /// The names and tips of the icons that hide and show the pane.
    pub(crate) hide: String,
    pub(crate) show: String,
    pub(crate) kind: String,
    /// In [`Kind::ALL`]'s order.
    pub(crate) kinds: [String; 9],
    pub(crate) location: String,
    /// In [`Model::location_items`]' order.
    pub(crate) locations: Vec<String>,
    pub(crate) choose_folder: String,
    pub(crate) only_directly: String,
    pub(crate) date: String,
    /// In [`date_items`]' order.
    pub(crate) dates: Vec<String>,
    pub(crate) size: String,
    /// In [`size_items`]' order.
    pub(crate) sizes: Vec<String>,
    pub(crate) attributes: String,
    /// In [`ATTRIBUTES`]' order.
    pub(crate) attribute_names: [String; 6],
    /// In [`AttributeChoice::ALL`]'s order.
    pub(crate) attribute_choices: [String; 3],
    /// The Date modified and Size items the pane is sized for: each choice
    /// but a custom range, and a custom range of each as wide as a usual
    /// one, so the pane keeps its width when one is set.
    pub(crate) ranges_widest: Vec<String>,
}

impl FilterCaptions {
    pub(crate) fn new(model: &Model, numbers: &dyn Numbers) -> FilterCaptions {
        let strings = model.strings();
        let filters = model.filters();
        let text = |key: &'static str| strings.text(key).to_owned();
        let dates = |chosen: DateModified| -> Vec<String> {
            date_items(chosen)
                .into_iter()
                .map(|item| match item {
                    RangeItem::Choice(choice) => date_text(strings, numbers, choice),
                    RangeItem::Custom => text("filter.custom"),
                })
                .collect()
        };
        let sizes = |chosen: SizeFilter| -> Vec<String> {
            size_items(chosen)
                .into_iter()
                .map(|item| match item {
                    RangeItem::Choice(choice) => size_text(strings, numbers, choice),
                    RangeItem::Custom => text("filter.custom"),
                })
                .collect()
        };
        let day = Day(jiff::civil::date(2026, 12, 30));
        let samples = [
            dates(DateModified::Custom(DateRange::Between(day, day))),
            sizes(SizeFilter::Custom(SizeRange {
                min: Some(150 * 1024),
                max: Some(1536 * 1024),
                unit: SizeUnit::Mb,
            })),
        ];
        FilterCaptions {
            title: text("filter.title"),
            clear: text("filter.clear"),
            clear_tip: text("filter.clear_tooltip"),
            hide: text("filter.hide"),
            show: text("filter.show"),
            kind: text("filter.kind"),
            kinds: Kind::ALL.map(|kind| text(kind.label())),
            location: text("filter.location"),
            locations: location_items(model),
            choose_folder: text("filter.choose_folder"),
            only_directly: text("filter.only_directly"),
            date: text("filter.date"),
            dates: dates(filters.date),
            size: text("filter.size"),
            sizes: sizes(filters.size),
            attributes: text("filter.attributes"),
            attribute_names: ATTRIBUTES.map(|(_, label)| text(label)),
            attribute_choices: AttributeChoice::ALL.map(|choice| text(choice.label())),
            ranges_widest: samples.into_iter().flatten().collect(),
        }
    }
}

/// The Location list's items: Everywhere, each volume by its mount prefix
/// with its state when it is not complete (DESIGN.md: Filter pane), and the
/// chosen folders.
pub(crate) fn location_items(model: &Model) -> Vec<String> {
    let strings = model.strings();
    model
        .location_items()
        .iter()
        .map(|item| match item {
            LocationItem::Everywhere => strings.text("filter.everywhere").to_owned(),
            LocationItem::Volume(volume) => {
                let prefix = volume.mounted().display_prefix();
                match volume.phase {
                    VolumePhase::Live | VolumePhase::Snapshot => prefix.to_owned(),
                    VolumePhase::Bootstrapping | VolumePhase::Rescanning => {
                        strings.format("filter.location_indexing", &[prefix])
                    }
                    VolumePhase::Offline => strings.format("filter.location_offline", &[prefix]),
                    VolumePhase::Failed { .. } => {
                        strings.format("filter.location_failed", &[prefix])
                    }
                }
            }
            LocationItem::Folder(folder) => (*folder).to_owned(),
        })
        .collect()
}

/// A Date modified choice as its list shows it.
fn date_text(strings: &Strings, numbers: &dyn Numbers, date: DateModified) -> String {
    let day = |day: Day| numbers.date(day.0);
    match date {
        DateModified::Any => strings.text("filter.any_time").to_owned(),
        DateModified::Today => strings.text("filter.today").to_owned(),
        DateModified::Yesterday => strings.text("filter.yesterday").to_owned(),
        DateModified::Last7Days => strings.text("filter.last_7_days").to_owned(),
        DateModified::Last30Days => strings.text("filter.last_30_days").to_owned(),
        DateModified::Last365Days => strings.text("filter.last_365_days").to_owned(),
        DateModified::Custom(DateRange::From(from)) => {
            strings.format("filter.date_from", &[&day(from)])
        }
        DateModified::Custom(DateRange::Until(to)) => {
            strings.format("filter.date_until", &[&day(to)])
        }
        DateModified::Custom(DateRange::Between(from, to)) => {
            strings.format("filter.date_between", &[&day(from), &day(to)])
        }
    }
}

/// A Size choice as its list shows it: a size word with its range, such as
/// "Tiny (up to 10 KB)", or a custom range in its unit.
fn size_text(strings: &Strings, numbers: &dyn Numbers, size: SizeFilter) -> String {
    match size {
        SizeFilter::Any => strings.text("filter.any_size").to_owned(),
        SizeFilter::Word(word) => {
            let range = word.word().range;
            let size = |bytes: u64| round_size(strings, numbers, bytes);
            let text = if range.min <= 1 {
                strings.format("filter.range_up_to", &[&size(range.max)])
            } else if range.max == u64::MAX {
                strings.format("filter.range_over", &[&size(range.min - 1)])
            } else {
                strings.format(
                    "filter.range_between",
                    &[&size(range.min - 1), &size(range.max)],
                )
            };
            // Empty has no range to show.
            match word {
                SizeWord::Empty => strings.text(word.label()).to_owned(),
                _ => strings.format(word.label(), &[&text]),
            }
        }
        SizeFilter::Custom(range) => {
            let size = |bytes: u64| unit_size(strings, numbers, bytes, range.unit);
            match (range.min, range.max) {
                (Some(min), Some(max)) => {
                    strings.format("filter.size_between", &[&size(min), &size(max)])
                }
                (Some(min), None) => strings.format("filter.size_at_least", &[&size(min)]),
                (None, Some(max)) => strings.format("filter.size_up_to", &[&size(max)]),
                (None, None) => strings.text("filter.any_size").to_owned(),
            }
        }
    }
}

const SIZE_UNITS: [(u64, &str); 3] = [
    (1 << 30, "size.gb"),
    (1 << 20, "size.mb"),
    (1 << 10, "size.kb"),
];

/// A size word's boundary, a whole number of a unit, such as "10 KB".
fn round_size(strings: &Strings, numbers: &dyn Numbers, bytes: u64) -> String {
    SIZE_UNITS
        .iter()
        .find(|(unit, _)| bytes >= *unit && bytes.is_multiple_of(*unit))
        .map_or_else(
            || text::bytes(strings, numbers, bytes),
            |(unit, key)| strings.format(key, &[&numbers.integer(bytes / unit)]),
        )
}

/// `bytes` in `unit`, with the decimals the person can have typed, such as
/// "1.5 MB".
fn unit_size(strings: &Strings, numbers: &dyn Numbers, bytes: u64, unit: SizeUnit) -> String {
    let value = bytes as f64 / unit.bytes() as f64;
    let digits = (0u8..2)
        .find(|&digits| {
            let scaled = value * 10f64.powi(i32::from(digits));
            (scaled - scaled.round()).abs() < 1e-6
        })
        .unwrap_or(2);
    let key = match unit {
        SizeUnit::Kb => "size.kb",
        SizeUnit::Mb => "size.mb",
        SizeUnit::Gb => "size.gb",
    };
    strings.format(key, &[&numbers.decimal(value, digits)])
}
