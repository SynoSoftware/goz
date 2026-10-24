//! The window's text: the shipped languages, the loaded [`Strings`] of one of
//! them, and the formatting of sizes, cells and menu labels that
//! `core::captions` builds the captions with. Numbers come through
//! [`Numbers`], so the regional format has one owner, Windows.
//!
//! A catalogue is JSON with one level of objects, such as
//! `{"menu": {"file": "&File"}}`, read as the dotted key `menu.file`. A
//! counted message has the forms `_one` and `_other`. A message comes from
//! the language, then from its parent language, always as a whole message
//! with all its forms. A message that neither has, or whose placeholders
//! differ from English, shows its dotted key, so the gap is visible.

use super::model::Column;
use gozler_core::proto::ResultItem;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// A shipped language: its tag, its names and its embedded catalogue.
pub(crate) struct Language {
    pub(crate) tag: &'static str,
    /// Its name as its speakers write it.
    own_name: &'static str,
    /// The catalogue key of its name in the current language, with the
    /// access key it has in the Language menu; `None` for the
    /// pseudo-language, which has no name to translate.
    name_key: Option<&'static str>,
    pub(crate) catalogue: &'static str,
}

impl Language {
    /// Its item in the Language menu and the Language list: its name in the
    /// current language, then its own name in brackets when the two differ,
    /// such as "&Dutch (Nederlands)", so a person who picked a language they
    /// cannot read still finds their own (DESIGN.md: Language).
    pub(crate) fn label(&self, strings: &Strings) -> String {
        let Some(key) = self.name_key else {
            return format!("&{}", self.own_name);
        };
        let name = strings.text(key);
        if without_access_key(name) == self.own_name {
            name.to_owned()
        } else {
            format!("{name} ({})", self.own_name)
        }
    }
}

const ENGLISH: Language = Language {
    tag: "en",
    own_name: "English",
    name_key: Some("language.english"),
    catalogue: include_str!("../../resources/en.json"),
};

const SPANISH: Language = Language {
    tag: "es",
    own_name: "Español",
    name_key: Some("language.spanish"),
    catalogue: include_str!("../../resources/es.json"),
};

const DUTCH: Language = Language {
    tag: "nl",
    own_name: "Nederlands",
    name_key: Some("language.dutch"),
    catalogue: include_str!("../../resources/nl.json"),
};

const TURKISH: Language = Language {
    tag: "tr",
    own_name: "Türkçe",
    name_key: Some("language.turkish"),
    catalogue: include_str!("../../resources/tr.json"),
};

/// Every shipped language, English first; the Language menu and the
/// Language list read this table, so listing them loads no catalogue.
#[cfg(not(feature = "pseudo-language"))]
pub(crate) static LANGUAGES: &[Language] = &[ENGLISH, SPANISH, DUTCH, TURKISH];

#[cfg(feature = "pseudo-language")]
pub(crate) static LANGUAGES: &[Language] = &[
    ENGLISH,
    SPANISH,
    DUTCH,
    TURKISH,
    Language {
        tag: "qps-ploc",
        own_name: "qps-ploc",
        name_key: None,
        catalogue: include_str!(concat!(env!("OUT_DIR"), "/qps-ploc.json")),
    },
];

/// The saved language choice; `Windows` follows the Windows display
/// languages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum LanguageChoice {
    #[default]
    Windows,
    Tag(String),
}

impl LanguageChoice {
    /// The choice at `place` in the Language menu and the Language list:
    /// `None` for Windows default, or an index of [`LANGUAGES`].
    pub(crate) fn at(place: Option<usize>) -> LanguageChoice {
        match place {
            Some(index) => LanguageChoice::Tag(LANGUAGES[index].tag.to_owned()),
            None => LanguageChoice::Windows,
        }
    }

    /// The index in [`LANGUAGES`] of a chosen language that ships; `None`
    /// for Windows default and for a saved tag that no longer ships.
    pub(crate) fn shipped(&self) -> Option<usize> {
        match self {
            LanguageChoice::Windows => None,
            LanguageChoice::Tag(tag) => LANGUAGES
                .iter()
                .position(|language| language.tag.eq_ignore_ascii_case(tag)),
        }
    }

    /// The shipped language to load: the chosen tag, or the first of
    /// `windows_languages` that ships, each with its parent languages
    /// (`pt` for `pt-BR`), and English when none ships.
    pub(crate) fn resolve(&self, windows_languages: &[String]) -> &'static Language {
        self.resolve_in(windows_languages, LANGUAGES)
    }

    fn resolve_in(
        &self,
        windows_languages: &[String],
        shipped: &'static [Language],
    ) -> &'static Language {
        match self {
            LanguageChoice::Tag(tag) => find_shipped(tag, shipped),
            LanguageChoice::Windows => windows_languages
                .iter()
                .find_map(|tag| find_shipped(tag, shipped)),
        }
        .unwrap_or(&shipped[0])
    }
}

/// The shipped language of `tag`, or of its nearest parent language.
fn find_shipped(tag: &str, shipped: &'static [Language]) -> Option<&'static Language> {
    let mut tag = tag;
    loop {
        if let Some(language) = shipped.iter().find(|l| l.tag.eq_ignore_ascii_case(tag)) {
            return Some(language);
        }
        tag = tag.rsplit_once('-')?.0;
    }
}

/// Regional number, date and time formatting; `win::locale` implements it
/// with `GetNumberFormatEx`, `GetDateFormatEx` and `GetTimeFormatEx`, the
/// tests with English formatting.
pub(crate) trait Numbers {
    fn integer(&self, n: u64) -> String;
    fn decimal(&self, n: f64, digits: u8) -> String;
    /// A FILETIME as local date and time in the short date and time
    /// formats, as File Explorer's Date modified column shows it.
    fn date_time(&self, filetime: i64) -> String;
    /// [`Numbers::date_time`] with the seconds, as the inspector shows it.
    fn date_time_seconds(&self, filetime: i64) -> String;
    /// A day in the short date format.
    fn date(&self, date: jiff::civil::Date) -> String;
    /// The separator of a number's decimals, such as `.` or `,`.
    fn decimal_separator(&self) -> &str;
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    One(String),
    Counted { one: String, other: String },
}

impl Message {
    fn placeholders(&self) -> [BTreeSet<u32>; 2] {
        match self {
            Message::One(text) => [placeholders(text), BTreeSet::new()],
            Message::Counted { one, other } => [placeholders(one), placeholders(other)],
        }
    }

    fn same_shape(&self, english: &Message) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(english)
            && self.placeholders() == english.placeholders()
    }
}

/// One loaded language, with its fallback to the parent language already
/// applied. It does not change; a language switch loads a new one.
#[derive(Clone)]
pub(crate) struct Strings {
    messages: Arc<HashMap<String, Message>>,
}

impl Strings {
    pub(crate) fn load(language: &'static Language) -> Strings {
        Self::load_in(language, LANGUAGES)
    }

    fn load_in(language: &Language, shipped: &[Language]) -> Strings {
        let english = messages(
            &parse_catalogue(shipped[0].catalogue)
                .expect("en.json parses; the catalogue test checks it"),
        );
        // The most general language first, so a nearer one replaces it.
        let mut chain = Vec::new();
        let mut tag = language.tag;
        while let Some((parent, _)) = tag.rsplit_once('-') {
            chain.push(tag);
            tag = parent;
        }
        chain.push(tag);
        let mut merged = HashMap::new();
        for tag in chain.iter().rev() {
            let Some(found) = shipped.iter().find(|l| l.tag.eq_ignore_ascii_case(tag)) else {
                continue;
            };
            // A catalogue that does not parse is a build defect that the
            // catalogue test catches; here its messages show their keys.
            let Ok(entries) = parse_catalogue(found.catalogue) else {
                continue;
            };
            for (key, message) in messages(&entries) {
                if english.get(&key).is_some_and(|e| message.same_shape(e)) {
                    merged.insert(key, message);
                }
            }
        }
        Strings {
            messages: Arc::new(merged),
        }
    }

    /// The message, or the dotted key when the language lacks it, so a
    /// person sees the gap and nothing panics.
    pub(crate) fn text<'a>(&'a self, key: &'a str) -> &'a str {
        match self.messages.get(key) {
            Some(Message::One(text)) => text,
            _ => key,
        }
    }

    /// The message with `{0}`, `{1}`, … replaced by `args`.
    pub(crate) fn format(&self, key: &str, args: &[&str]) -> String {
        fill(self.text(key), args)
    }

    /// The `_one` form for 1 and the `_other` form otherwise, with `count`
    /// as `{0}` and `args` as `{1}`, `{2}`, ….
    pub(crate) fn count(
        &self,
        key: &str,
        count: u64,
        numbers: &dyn Numbers,
        args: &[&str],
    ) -> String {
        let text = match self.messages.get(key) {
            Some(Message::Counted { one, .. }) if count == 1 => one.as_str(),
            Some(Message::Counted { other, .. }) => other.as_str(),
            _ => key,
        };
        let count = numbers.integer(count);
        let mut all = vec![count.as_str()];
        all.extend_from_slice(args);
        fill(text, &all)
    }
}

/// Replaces each `{n}` with `args[n]`; a placeholder without an argument
/// stays as it is.
fn fill(text: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}');
        match close
            .and_then(|close| after[..close].parse::<usize>().ok().map(|n| (n, close)))
            .and_then(|(n, close)| args.get(n).map(|arg| (arg, close)))
        {
            Some((arg, close)) => {
                out.push_str(arg);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn placeholders(text: &str) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('}')
            && let Ok(n) = rest[..close].parse()
        {
            found.insert(n);
        }
    }
    found
}

/// The messages of a catalogue's entries. A counted message missing one of
/// its forms is left out, so its whole message falls back.
fn messages(entries: &[(String, String)]) -> HashMap<String, Message> {
    let mut messages = HashMap::new();
    let mut ones = HashMap::new();
    let mut others = HashMap::new();
    for (key, text) in entries {
        if let Some(base) = key.strip_suffix("_one") {
            ones.insert(base.to_owned(), text.clone());
        } else if let Some(base) = key.strip_suffix("_other") {
            others.insert(base.to_owned(), text.clone());
        } else {
            messages.insert(key.clone(), Message::One(text.clone()));
        }
    }
    for (base, one) in ones {
        if let Some(other) = others.remove(&base) {
            messages.insert(base, Message::Counted { one, other });
        }
    }
    messages
}

/// Reads a catalogue into its dotted keys and texts. A repeated group or key
/// is an error, because `serde_json` would keep the last of two equal keys
/// without a word.
pub(crate) fn parse_catalogue(text: &str) -> Result<Vec<(String, String)>, serde_json::Error> {
    struct Groups;
    impl<'de> Visitor<'de> for Groups {
        type Value = Vec<(String, String)>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an object of message groups")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut entries = Vec::new();
            let mut seen = HashSet::new();
            while let Some(group) = map.next_key::<String>()? {
                if !seen.insert(group.clone()) {
                    return Err(de::Error::custom(format!("repeated group {group}")));
                }
                map.next_value_seed(Group {
                    name: &group,
                    entries: &mut entries,
                })?;
            }
            Ok(entries)
        }
    }

    struct Group<'a> {
        name: &'a str,
        entries: &'a mut Vec<(String, String)>,
    }
    impl<'de> DeserializeSeed<'de> for Group<'_> {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
            deserializer.deserialize_map(self)
        }
    }
    impl<'de> Visitor<'de> for Group<'_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an object of messages")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut seen = HashSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if !seen.insert(key.clone()) {
                    return Err(de::Error::custom(format!(
                        "repeated key {}.{key}",
                        self.name
                    )));
                }
                let text = map.next_value::<String>()?;
                self.entries.push((format!("{}.{key}", self.name), text));
            }
            Ok(())
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(text);
    let entries = deserializer.deserialize_map(Groups)?;
    deserializer.end()?;
    Ok(entries)
}

/// Compact sizes use whole bytes and KB, then three significant digits from
/// MB upward. Extra digits are dropped; 1,000 of a unit moves to the next,
/// so 1,000 KB shows as 0.97 MB.
pub(crate) fn bytes(strings: &Strings, numbers: &dyn Numbers, n: u64) -> String {
    if n < 1024 {
        return strings.count("size.bytes", n, numbers, &[]);
    }
    const UNITS: [&str; 4] = ["size.kb", "size.mb", "size.gb", "size.tb"];
    // Whole numbers, so no float rounding can add a digit.
    let n = u128::from(n);
    let mut unit_bytes: u128 = 1024;
    let mut unit = 0;
    while n / unit_bytes >= 1000 && unit < UNITS.len() - 1 {
        unit_bytes *= 1024;
        unit += 1;
    }
    let digits: u8 = match (unit, n / unit_bytes) {
        (0, _) => 0,
        (_, 0..=9) => 2,
        (_, 10..=99) => 1,
        _ => 0,
    };
    let scale = 10u128.pow(u32::from(digits));
    let kept = n * scale / unit_bytes;
    strings.format(
        UNITS[unit],
        &[&numbers.decimal(kept as f64 / scale as f64, digits)],
    )
}

/// The bytes of a full path that the Name or Path cell shows, `None` for
/// another column. Name and Path split the path at its last `\`, as
/// `gozler`'s Name and Path columns do.
pub(crate) fn path_part(column: Column, path: &str) -> Option<Range<usize>> {
    let name_start = path.rfind('\\').map_or(0, |i| i + 1);
    match column {
        Column::Name => Some(name_start..path.len()),
        Column::Path => Some(0..name_start.saturating_sub(1)),
        Column::Size
        | Column::DateModified
        | Column::DateCreated
        | Column::Extension
        | Column::Attributes => None,
    }
}

/// The name at the end of a Windows path, after its last `\`, as the Name
/// cell shows it. `std::path` would split at `/` alone on another OS, where
/// the core's tests also run.
pub(crate) fn name_of(path: &str) -> &str {
    &path[path_part(Column::Name, path).expect("Name is a part of the path")]
}

/// How the Size column shows a known size, which Settings > General
/// chooses. Each shows every row in one unit, so sizes line up for
/// comparison (DESIGN.md: Results list).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SizeFormat {
    Bytes,
    #[default]
    Kb,
    Mb,
}

impl SizeFormat {
    /// Later rows do not refill the columns, so Size reserves room for its
    /// full range and both forms of the unit before those rows arrive.
    pub(crate) fn width_samples(self, strings: &Strings, numbers: &dyn Numbers) -> [String; 2] {
        [
            self.text(strings, numbers, 1),
            self.text(strings, numbers, u64::MAX),
        ]
    }

    /// The choices of its list on the General page, from the smallest unit
    /// to the largest.
    pub(crate) const ALL: [SizeFormat; 3] = [SizeFormat::Bytes, SizeFormat::Kb, SizeFormat::Mb];

    /// The catalogue key of its name in the list.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SizeFormat::Kb => "unit.kb",
            SizeFormat::Mb => "unit.mb",
            SizeFormat::Bytes => "unit.bytes",
        }
    }

    /// `size` bytes in this format. KB rounds up, as File Explorer does, and
    /// MB rounds up to the hundredth, so a file that is not empty never
    /// shows as 0 KB or 0.00 MB.
    pub(crate) fn text(self, strings: &Strings, numbers: &dyn Numbers, size: u64) -> String {
        match self {
            SizeFormat::Kb => strings.format("size.kb", &[&numbers.integer(size.div_ceil(1024))]),
            SizeFormat::Mb => {
                let hundredths = (u128::from(size) * 100).div_ceil(1 << 20);
                strings.format("size.mb", &[&numbers.decimal(hundredths as f64 / 100.0, 2)])
            }
            SizeFormat::Bytes => strings.count("size.bytes", size, numbers, &[]),
        }
    }
}

/// A label or value with an optional short form for a narrow column.
#[derive(Default)]
pub(crate) struct Text {
    pub(crate) full: String,
    pub(crate) short: Option<String>,
}

impl Text {
    pub(crate) fn compact(&self) -> &str {
        self.short.as_deref().unwrap_or(&self.full)
    }

    pub(crate) fn fitting(&self, fits: impl FnOnce(&str) -> bool) -> &str {
        match self.short.as_deref() {
            Some(short) if !fits(&self.full) => short,
            _ => &self.full,
        }
    }
}

impl From<String> for Text {
    fn from(full: String) -> Self {
        Self { full, short: None }
    }
}

/// One cell of the results list, as File Explorer shows its columns, with
/// sizes in `sizes` (DESIGN.md: Results list).
pub(crate) fn cell(
    strings: &Strings,
    numbers: &dyn Numbers,
    sizes: SizeFormat,
    dates: &super::dates::Dates,
    column: Column,
    row: &ResultItem,
) -> Text {
    if row.is_gone() {
        // A result snapshot's row whose file is gone has no other value.
        return match column {
            Column::Name => strings.text("cell.gone").to_owned(),
            _ => String::new(),
        }
        .into();
    }
    let name = name_of(&row.path);
    match column {
        Column::Name | Column::Path => row.path
            [path_part(column, &row.path).expect("Name and Path are parts of the path")]
        .to_owned(),
        Column::Size => match row.size {
            Some(size) => sizes.text(strings, numbers, size),
            None => strings.text("cell.unknown").to_owned(),
        },
        Column::DateModified => {
            return row
                .mtime_ft
                .map(|ft| dates.text(strings, numbers, ft))
                .unwrap_or_default();
        }
        Column::DateCreated => {
            return row
                .created_ft
                .map(|ft| dates.text(strings, numbers, ft))
                .unwrap_or_default();
        }
        Column::Extension => gozler_core::query::extension(name.as_bytes(), row.is_dir)
            .map(|ext| String::from_utf8_lossy(ext).into_owned())
            .unwrap_or_default(),
        Column::Attributes => row.attributes.map(|a| a.letters()).unwrap_or_default(),
    }
    .into()
}

/// `text` without its access key's `&`, as a caption reads where it names a
/// command, such as "Edit > Find"; `&&` stays one `&`.
pub(crate) fn without_access_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// The menu text of `label` with its first shortcut after a tab, as Windows
/// menus show it.
pub(crate) fn menu_label(label: &str, shortcut: Option<String>) -> String {
    match shortcut {
        Some(shortcut) => format!("{label}\t{shortcut}"),
        None => label.to_owned(),
    }
}

#[cfg(test)]
/// The access key of `text`, lowercased: the letter after its `&`.
pub(crate) fn access_key(text: &str) -> Option<char> {
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.next() {
                Some('&') => continue,
                Some(key) => return key.to_lowercase().next(),
                None => return None,
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// English formatting, so the tests do not depend on the machine's
    /// regional format.
    pub(crate) struct English;

    impl Numbers for English {
        fn integer(&self, n: u64) -> String {
            let digits = n.to_string();
            let mut out = String::new();
            for (i, c) in digits.chars().enumerate() {
                if i > 0 && (digits.len() - i).is_multiple_of(3) {
                    out.push(',');
                }
                out.push(c);
            }
            out
        }

        fn decimal(&self, n: f64, digits: u8) -> String {
            format!("{n:.*}", usize::from(digits))
        }

        fn date_time(&self, filetime: i64) -> String {
            gozler_core::output::format_date_local(filetime, &jiff::tz::TimeZone::UTC)
        }

        fn date_time_seconds(&self, filetime: i64) -> String {
            self.date_time(filetime)
        }

        fn date(&self, date: jiff::civil::Date) -> String {
            date.to_string()
        }

        fn decimal_separator(&self) -> &str {
            "."
        }
    }

    pub(crate) fn english() -> Strings {
        Strings::load(&LANGUAGES[0])
    }

    static TEST_LANGUAGES: &[Language] = &[
        Language {
            tag: "en",
            own_name: "English",
            name_key: None,
            catalogue: r#"{"a": {"plain": "Plain", "only_en": "English only",
                "files_one": "{0} file", "files_other": "{0} files", "moved": "{0} to {1}"}}"#,
        },
        Language {
            tag: "pt",
            own_name: "Português",
            name_key: None,
            catalogue: r#"{"a": {"plain": "Simples", "files_one": "{0} arquivo",
                "files_other": "{0} arquivos"}}"#,
        },
        Language {
            tag: "pt-BR",
            own_name: "Português (Brasil)",
            name_key: None,
            // `files` lacks its `_other` form, so `pt`'s message shows
            // whole; `moved` drops `{1}` and `pt` lacks it, so its key shows.
            catalogue: r#"{"a": {"plain": "Simples (BR)", "files_one": "{0} ficheiro",
                "moved": "{0} para"}}"#,
        },
    ];

    #[test]
    fn a_message_falls_back_whole_to_the_parent_language_else_shows_its_key() {
        let strings = Strings::load_in(&TEST_LANGUAGES[2], TEST_LANGUAGES);
        assert_eq!(strings.text("a.plain"), "Simples (BR)");
        assert_eq!(strings.count("a.files", 1, &English, &[]), "1 arquivo");
        assert_eq!(strings.count("a.files", 2, &English, &[]), "2 arquivos");
        assert_eq!(strings.text("a.only_en"), "a.only_en");
        assert_eq!(strings.format("a.moved", &["x", "y"]), "a.moved");
        assert_eq!(strings.text("a.missing"), "a.missing");
    }

    #[test]
    fn windows_default_takes_the_first_shipped_language_or_its_parent() {
        let windows = |tags: &[&str]| tags.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let resolve = |choice: LanguageChoice, tags: &[&str]| {
            choice.resolve_in(&windows(tags), TEST_LANGUAGES).tag
        };
        assert_eq!(
            resolve(LanguageChoice::Windows, &["de-DE", "pt-BR"]),
            "pt-BR"
        );
        assert_eq!(resolve(LanguageChoice::Windows, &["pt-PT", "en-US"]), "pt");
        assert_eq!(resolve(LanguageChoice::Windows, &["de-DE"]), "en");
        assert_eq!(resolve(LanguageChoice::Tag("xx".into()), &["pt"]), "en");
    }

    /// The shipped catalogues are data that no compiler checks, and a broken
    /// one shows dotted keys only at run time, in the language that has the
    /// defect.
    #[test]
    fn every_shipped_catalogue_matches_english() {
        let english_entries = parse_catalogue(ENGLISH.catalogue).expect("en.json parses");
        let english = messages(&english_entries);
        let english_keys: HashSet<&str> = english_entries.iter().map(|(k, _)| k.as_str()).collect();
        for language in LANGUAGES {
            let entries = parse_catalogue(language.catalogue)
                .unwrap_or_else(|e| panic!("{}.json does not parse: {e}", language.tag));
            let keys: HashSet<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
            for key in &keys {
                assert!(
                    english_keys.contains(key),
                    "{}.json has {key}, which en.json lacks",
                    language.tag
                );
                let pair = match (key.strip_suffix("_one"), key.strip_suffix("_other")) {
                    (Some(base), _) => Some(format!("{base}_other")),
                    (_, Some(base)) => Some(format!("{base}_one")),
                    _ => None,
                };
                if let Some(pair) = pair {
                    assert!(
                        keys.contains(pair.as_str()),
                        "{}.json has {key} without {pair}",
                        language.tag
                    );
                }
            }
            for key in &english_keys {
                assert!(
                    keys.contains(key),
                    "{}.json lacks {key}, which then shows as its key",
                    language.tag
                );
            }
            for (key, message) in messages(&entries) {
                assert!(
                    message.same_shape(&english[&key]),
                    "{}.json: {key} has other placeholders or forms than English",
                    language.tag
                );
            }
        }
    }

    #[test]
    fn a_repeated_key_is_refused() {
        assert!(parse_catalogue(r#"{"a": {"x": "1", "x": "2"}}"#).is_err());
        assert!(parse_catalogue(r#"{"a": {"x": "1"}, "a": {"y": "2"}}"#).is_err());
    }

    /// Four significant digits, such as "1,000 KB" or "10.00 MB", read
    /// wrongly and are wider than the status bar part measured for a size.
    #[test]
    fn compact_sizes_limit_precision() {
        let strings = english();
        let shown = |n| bytes(&strings, &English, n);
        let kb = |value| strings.format("size.kb", &[value]);
        assert_eq!(shown(1_340), kb("1"));
        assert_eq!(shown(23_506), kb("22"));
        // 1,000 KB moves to the next unit.
        assert_eq!(shown(1_024_000), strings.format("size.mb", &["0.97"]));
    }

    /// A size rounded to the nearest, as the decimal formatting does, shows
    /// a file of a few bytes as 0.00 MB, which reads as empty.
    #[test]
    fn the_size_column_rounds_kb_and_mb_up_and_shows_exact_bytes() {
        let strings = english();
        let shown = |format: SizeFormat, n| format.text(&strings, &English, n);
        let kb = |value| strings.format("size.kb", &[value]);
        let mb = |value| strings.format("size.mb", &[value]);
        assert_eq!(shown(SizeFormat::Kb, 1), kb("1"));
        assert_eq!(shown(SizeFormat::Kb, 1_025), kb("2"));
        assert_eq!(shown(SizeFormat::Mb, 0), mb("0.00"));
        assert_eq!(shown(SizeFormat::Mb, 1), mb("0.01"));
        assert_eq!(shown(SizeFormat::Mb, 1_048_576), mb("1.00"));
        assert_eq!(shown(SizeFormat::Mb, 1_048_577), mb("1.01"));
        assert!(shown(SizeFormat::Bytes, 41_163_399_668).contains("41,163,399,668"));
    }
}
