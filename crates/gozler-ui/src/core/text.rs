//! The window's text: the shipped languages, the loaded [`Strings`] of one of
//! them, and the functions that build each caption from facts (design rule
//! 12). Numbers come through [`Numbers`], so the regional format has one
//! owner, Windows.
//!
//! A catalogue is JSON with one level of objects, such as
//! `{"menu": {"file": "&File"}}`, read as the dotted key `menu.file`. A
//! counted message has the forms `_one` and `_other`. A message comes from
//! the language, then from its parent language, then from English, always as
//! a whole message with all its forms; a translation whose placeholders
//! differ from English is ignored, so English shows.

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, Visitor};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

/// A shipped language: its tag, its own name with the access key it has in
/// the Language menu, and its embedded catalogue.
pub(crate) struct Language {
    pub(crate) tag: &'static str,
    pub(crate) name: &'static str,
    pub(crate) catalogue: &'static str,
}

const ENGLISH: Language = Language {
    tag: "en",
    name: "&English",
    catalogue: include_str!("../../resources/en.json"),
};

/// Every shipped language, English first; the Language menu and the
/// Language list read this table, so listing them loads no catalogue.
#[cfg(not(feature = "pseudo-language"))]
pub(crate) static LANGUAGES: &[Language] = &[ENGLISH];

#[cfg(feature = "pseudo-language")]
pub(crate) static LANGUAGES: &[Language] = &[
    ENGLISH,
    Language {
        tag: "qps-ploc",
        name: "&qps-ploc",
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

/// Regional number formatting; `win::locale` implements it with
/// `GetNumberFormatEx`, the tests with English formatting.
pub(crate) trait Numbers {
    fn integer(&self, n: u64) -> String;
    fn decimal(&self, n: f64, digits: u8) -> String;
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

/// One loaded language, with its fallback to the parent language and to
/// English already applied. It does not change; a language switch loads a
/// new one.
pub(crate) struct Strings {
    messages: HashMap<String, Message>,
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
        let mut merged = english.clone();
        for tag in chain.iter().rev() {
            let Some(found) = shipped.iter().find(|l| l.tag.eq_ignore_ascii_case(tag)) else {
                continue;
            };
            if std::ptr::eq(found.catalogue, shipped[0].catalogue) {
                continue;
            }
            // A catalogue that does not parse is a build defect that the
            // catalogue test catches; here its messages show in English.
            let Ok(entries) = parse_catalogue(found.catalogue) else {
                continue;
            };
            for (key, message) in messages(&entries) {
                if english.get(&key).is_some_and(|e| message.same_shape(e)) {
                    merged.insert(key, message);
                }
            }
        }
        Strings { messages: merged }
    }

    /// The message, or the dotted key when English lacks it, so a person
    /// sees the gap and nothing panics.
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

/// A size as the selection and the inspector show it: bytes below 1 KB,
/// then three significant digits in KB, MB, GB or TB, as File Explorer's
/// Properties does. A value that rounds to 1,000 moves to the next unit, so
/// 999.6 KB shows as 0.98 MB.
pub(crate) fn bytes(strings: &Strings, numbers: &dyn Numbers, n: u64) -> String {
    if n < 1024 {
        return strings.count("size.bytes", n, numbers, &[]);
    }
    const UNITS: [&str; 4] = ["size.kb", "size.mb", "size.gb", "size.tb"];
    let mut value = n as f64 / 1024.0;
    let mut unit = 0;
    while value.round() >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    // The most decimals that keep three significant digits after rounding:
    // 9.996 rounds to 10.00, which has four, so it shows as 10.0.
    let digits = [2u8, 1]
        .into_iter()
        .find(|&digits| {
            let scale = 10f64.powi(i32::from(digits));
            (value * scale).round() / scale < 10f64.powi(3 - i32::from(digits))
        })
        .unwrap_or(0);
    strings.format(UNITS[unit], &[&numbers.decimal(value, digits)])
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
    }

    pub(crate) fn english() -> Strings {
        Strings::load(&LANGUAGES[0])
    }

    static TEST_LANGUAGES: &[Language] = &[
        Language {
            tag: "en",
            name: "English",
            catalogue: r#"{"a": {"plain": "Plain", "only_en": "English only",
                "files_one": "{0} file", "files_other": "{0} files", "moved": "{0} to {1}"}}"#,
        },
        Language {
            tag: "pt",
            name: "Português",
            catalogue: r#"{"a": {"plain": "Simples", "files_one": "{0} arquivo",
                "files_other": "{0} arquivos"}}"#,
        },
        Language {
            tag: "pt-BR",
            name: "Português (Brasil)",
            // `files` lacks its `_other` form and `moved` drops `{1}`, so
            // both show the message of a more general language whole.
            catalogue: r#"{"a": {"plain": "Simples (BR)", "files_one": "{0} ficheiro",
                "moved": "{0} para"}}"#,
        },
    ];

    #[test]
    fn a_message_falls_back_whole_to_the_parent_language_then_to_english() {
        let strings = Strings::load_in(&TEST_LANGUAGES[2], TEST_LANGUAGES);
        assert_eq!(strings.text("a.plain"), "Simples (BR)");
        assert_eq!(strings.count("a.files", 1, &English, &[]), "1 arquivo");
        assert_eq!(strings.count("a.files", 2, &English, &[]), "2 arquivos");
        assert_eq!(strings.text("a.only_en"), "English only");
        assert_eq!(strings.format("a.moved", &["x", "y"]), "x to y");
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
    /// one shows English or dotted keys only at run time, in the language
    /// that has the defect.
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
    fn a_size_keeps_three_significant_digits() {
        let strings = english();
        let shown = |n| bytes(&strings, &English, n);
        // 999.51 KB rounds to 1,000 KB.
        assert_eq!(shown(1_023_500), "0.98 MB");
        // 9.9955 MB rounds to 10.00 MB.
        assert_eq!(shown(10_481_000), "10.0 MB");
    }
}
