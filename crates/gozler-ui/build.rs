//! Compiles `resources/gozler-ui.rc` into the executable on Windows, and, with
//! the `pseudo-language` feature, writes the pseudo-language catalogue
//! `qps-ploc.json` into `OUT_DIR` for `core::text` to embed.
//!
//! The version reaches the .rc file through `version.rh` in `OUT_DIR`, so the
//! version information has one source, the workspace version.

fn main() {
    println!("cargo::rerun-if-changed=resources");
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        let var = |name: &str| std::env::var(name).expect("cargo sets the version and OUT_DIR");
        let header = format!(
            "#define GOZLER_VERSION \"{}\"\n#define GOZLER_VERSION_NUMBERS {},{},{},0\n",
            var("CARGO_PKG_VERSION"),
            var("CARGO_PKG_VERSION_MAJOR"),
            var("CARGO_PKG_VERSION_MINOR"),
            var("CARGO_PKG_VERSION_PATCH"),
        );
        // embed-resource passes OUT_DIR to rc.exe as an include folder.
        std::fs::write(
            std::path::Path::new(&var("OUT_DIR")).join("version.rh"),
            header,
        )
        .expect("OUT_DIR is writable");
        embed_resource::compile("resources/gozler-ui.rc", embed_resource::NONE)
            .manifest_required()
            .expect("rc.exe compiles resources/gozler-ui.rc");
    }
    #[cfg(feature = "pseudo-language")]
    pseudo::write();
}

/// The pseudo-language: Windows' own long-text pseudo-locale tag, with every
/// message lengthened and accented so that clipping and untranslated text
/// show in a screenshot (docs/plans/ui-design.md: Text and languages).
#[cfg(feature = "pseudo-language")]
mod pseudo {
    use serde_json::Value;

    pub(super) fn write() {
        let english = std::fs::read_to_string("resources/en.json").expect("en.json is readable");
        let mut catalogue: Value = serde_json::from_str(&english).expect("en.json parses");
        let Value::Object(groups) = &mut catalogue else {
            panic!("en.json holds one level of objects")
        };
        for group in groups.values_mut() {
            let Value::Object(messages) = group else {
                panic!("en.json holds one level of objects")
            };
            for message in messages.values_mut() {
                if let Value::String(text) = message {
                    *text = lengthen(text);
                }
            }
        }
        let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"))
            .join("qps-ploc.json");
        std::fs::write(
            out,
            serde_json::to_string(&catalogue).expect("a Value serializes"),
        )
        .expect("OUT_DIR is writable");
    }

    /// A message of up to 12 characters becomes twice as long, a longer one
    /// half as long again. Placeholders and the access key's letter stay as
    /// they are, so formatting and the access keys work as in English.
    fn lengthen(text: &str) -> String {
        let mut out = String::from("[");
        let mut visible: usize = 0;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' => {
                    out.push(c);
                    for c in chars.by_ref() {
                        out.push(c);
                        if c == '}' {
                            break;
                        }
                    }
                    visible += 3;
                }
                '&' => {
                    out.push(c);
                    if let Some(key) = chars.next() {
                        out.push(key);
                    }
                    visible += 1;
                }
                _ => {
                    out.push(accented(c));
                    visible += 1;
                }
            }
        }
        let target = if visible <= 12 {
            visible * 2
        } else {
            (visible * 3).div_ceil(2)
        };
        for _ in visible + 2..target {
            out.push('·');
        }
        out.push(']');
        out
    }

    fn accented(c: char) -> char {
        const PLAIN: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        const ACCENTED: &str = "áƀçďéƒĝĥíĵķĺɱñóþǫŕšŧúṽŵẋýžÁƁÇĎÉƑĜĤÍĴĶĹṀÑÓÞǪŔŠŦÚṼŴẊÝŽ";
        PLAIN
            .chars()
            .position(|p| p == c)
            .and_then(|i| ACCENTED.chars().nth(i))
            .unwrap_or(c)
    }
}
