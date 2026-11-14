//! The text Kiritori shows, kept in language files (English first, translations
//! later).
//!
//! English is built in (`locales/en.lang`). A `<language>.lang` file in the Kiritori folder's
//! `locales\` folder overrides it key by key, so a translation can be dropped in without a new
//! build. The tray menu and the toolbar use it today; the rest of the text moves over as it is
//! touched.

use std::collections::HashMap;
use std::sync::OnceLock;

const ENGLISH: &str = include_str!("../locales/en.lang");

static TABLE: OnceLock<HashMap<String, &'static str>> = OnceLock::new();

/// "key = text" lines; `\t` in the text is a tab.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().replace("\\t", "\t")))
        .collect()
}

/// Load English, then the chosen language's file from `folder` if there is one.
pub fn init(folder: &std::path::Path, language: &str) {
    let mut table: HashMap<String, &'static str> = HashMap::new();
    for (k, v) in parse(ENGLISH) {
        table.insert(k, Box::leak(v.into_boxed_str()));
    }
    if language != "en"
        && let Ok(text) = std::fs::read_to_string(folder.join("locales").join(format!("{language}.lang")))
    {
        for (k, v) in parse(&text) {
            table.insert(k, Box::leak(v.into_boxed_str()));
        }
    }
    let _ = TABLE.set(table);
}

/// The text for a key; the key itself if it is missing (so a gap shows up, never a blank).
pub fn t(key: &'static str) -> &'static str {
    TABLE
        .get_or_init(|| parse(ENGLISH).into_iter().map(|(k, v)| (k, &*Box::leak(v.into_boxed_str()))).collect())
        .get(key)
        .copied()
        .unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_has_every_key_used() {
        let keys: Vec<String> = parse(ENGLISH).into_iter().map(|(k, _)| k).collect();
        // Every t("…") in the source must have an English entry.
        let src = [include_str!("shell/mod.rs"), include_str!("layer/view.rs")].concat();
        let mut i = 0;
        while let Some(p) = src[i..].find("t(\"") {
            let start = i + p + 3;
            let end = start + src[start..].find('"').unwrap();
            let key = &src[start..end];
            if key.contains('.') {
                assert!(keys.iter().any(|k| k == key), "{key} has no English text");
            }
            i = end;
        }
        assert_eq!(t("tray.quit"), "Quit Kiritori");
        assert_eq!(t("no.such.key"), "no.such.key");
    }

    #[test]
    fn files_parse() {
        let p = parse("# comment\n a.b = Hello \\t there\n\nbroken line\n");
        assert_eq!(p, vec![("a.b".to_string(), "Hello \t there".to_string())]);
    }
}
