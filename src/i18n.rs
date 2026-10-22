//! The app's own text in many languages. Text is written in English in the code and looked up
//! here: `t("Size")` is "Boyut" while the language is Turkish. The translations live in
//! `locales/<code>.json` (English text -> translation) and are built into the app; missing
//! entries stay English. Placeholders like `{n}` are filled in with [`tf`].

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::settings::Lang;

/// The languages, with their code and their name in their own language. English first; the
/// order is the settings' list.
pub const LANGS: [(Lang, &str, &str); 17] = [
    (Lang::En, "en", "English"),
    (Lang::Tr, "tr", "Türkçe"),
    (Lang::De, "de", "Deutsch"),
    (Lang::Fr, "fr", "Français"),
    (Lang::Es, "es", "Español"),
    (Lang::It, "it", "Italiano"),
    (Lang::PtBr, "pt-BR", "Português (Brasil)"),
    (Lang::Nl, "nl", "Nederlands"),
    (Lang::Pl, "pl", "Polski"),
    (Lang::Ru, "ru", "Русский"),
    (Lang::Uk, "uk", "Українська"),
    (Lang::Ar, "ar", "العربية"),
    (Lang::Id, "id", "Bahasa Indonesia"),
    (Lang::Ja, "ja", "日本語"),
    (Lang::Ko, "ko", "한국어"),
    (Lang::ZhTw, "zh-TW", "繁體中文"),
    (Lang::ZhCn, "zh-CN", "简体中文"),
];

/// The translation files, built in.
static FILES: &[(&str, &str)] = &[
    ("tr", include_str!("../locales/tr.json")),
    ("de", include_str!("../locales/de.json")),
    ("fr", include_str!("../locales/fr.json")),
    ("es", include_str!("../locales/es.json")),
    ("it", include_str!("../locales/it.json")),
    ("pt-BR", include_str!("../locales/pt-BR.json")),
    ("nl", include_str!("../locales/nl.json")),
    ("pl", include_str!("../locales/pl.json")),
    ("ru", include_str!("../locales/ru.json")),
    ("uk", include_str!("../locales/uk.json")),
    ("ar", include_str!("../locales/ar.json")),
    ("id", include_str!("../locales/id.json")),
    ("ja", include_str!("../locales/ja.json")),
    ("ko", include_str!("../locales/ko.json")),
    ("zh-TW", include_str!("../locales/zh-TW.json")),
    ("zh-CN", include_str!("../locales/zh-CN.json")),
];

/// The language in use: its index in [`LANGS`].
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Sets the language for [`t`]; `Auto` follows the system.
pub fn set(lang: Lang) {
    let lang = match lang {
        Lang::Auto => system(),
        l => l,
    };
    let i = LANGS.iter().position(|(l, ..)| *l == lang).unwrap_or(0);
    CURRENT.store(i as u8, Ordering::Relaxed);
}

/// The index in [`LANGS`] of the language in use.
#[cfg(not(test))]
fn index() -> usize {
    CURRENT.load(Ordering::Relaxed) as usize % LANGS.len()
}

/// Tests run in parallel: each sees English unless it asks for another with [`with_lang`].
#[cfg(test)]
fn index() -> usize {
    TEST_LANG.with(std::cell::Cell::get).unwrap_or(0)
}

/// The language in use (never `Auto`).
pub fn current() -> Lang {
    LANGS[index()].0
}

/// The code of the language in use ("en", "tr", "zh-TW").
pub fn code() -> &'static str {
    LANGS[index()].1
}

#[cfg(test)]
thread_local! {
    /// The language of [`with_lang`] on this test's thread (English otherwise).
    static TEST_LANG: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` with the language `l` on this thread only (tests).
#[cfg(test)]
pub fn with_lang<T>(l: Lang, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<usize>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_LANG.with(|c| c.set(self.0));
        }
    }
    let i = LANGS.iter().position(|(x, ..)| *x == l).unwrap_or(0);
    let _restore = Restore(TEST_LANG.with(|c| c.replace(Some(i))));
    f()
}

/// Whether the text is Turkish now (for Turkish grammar, e.g. "Ayşe'nin").
pub fn is_turkish() -> bool {
    current() == Lang::Tr
}

/// Whether the language is written right to left.
pub fn is_rtl() -> bool {
    current() == Lang::Ar
}

/// The tables, parsed once: code -> (English -> translation).
fn tables() -> &'static HashMap<&'static str, HashMap<String, String>> {
    static TABLES: OnceLock<HashMap<&str, HashMap<String, String>>> = OnceLock::new();
    TABLES.get_or_init(|| {
        FILES
            .iter()
            .map(|(code, json)| {
                let mut map: HashMap<String, String> =
                    serde_json::from_str(json).expect("valid embedded translation file");
                map.retain(|_, v| !v.is_empty());
                (*code, map)
            })
            .collect()
    })
}

/// `text` in the current language.
pub fn t(text: &str) -> &str {
    translate(code(), text)
}

/// `text` in the language with `code`, or itself when there is no translation.
pub fn translate<'a>(code: &str, text: &'a str) -> &'a str {
    match tables().get(code).and_then(|m| m.get(text)) {
        // A `&'static String` lives as long as anything.
        Some(s) => s.as_str(),
        None => text,
    }
}

/// `text` in Turkish, or itself when there is no translation.
pub fn turkish(text: &str) -> &str {
    translate("tr", text)
}

/// [`t`] with placeholders filled in: `tf("{n} min", &[("n", "5")])`.
pub fn tf(text: &str, args: &[(&str, &str)]) -> String {
    tf_in(code(), text, args)
}

/// [`tf`] in the language with `code`.
pub fn tf_in(code: &str, text: &str, args: &[(&str, &str)]) -> String {
    let mut rest = translate(code, text);
    let mut out = String::with_capacity(rest.len());
    // Substitute once, without interpreting braces in a user's name or other value.
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find('}') else {
            break;
        };
        match args.iter().find(|(key, _)| *key == &rest[1..close]) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The system's language among ours (checked once); English when it is none of them.
pub fn system() -> Lang {
    static SYSTEM: OnceLock<Lang> = OnceLock::new();
    *SYSTEM.get_or_init(|| system_lang().map_or(Lang::En, |l| match_lang(&l)))
}

/// Whether the system's language is Turkish.
pub fn system_is_turkish() -> bool {
    system() == Lang::Tr
}

/// "de_DE.UTF-8", "pt-BR", "zh-Hant-TW", "zh_CN" -> one of ours (English if none).
pub fn match_lang(tag: &str) -> Lang {
    let t = tag
        .split(['.', ':', '@'])
        .next()
        .unwrap_or("")
        .replace('_', "-")
        .to_lowercase();
    let primary = t.split('-').next().unwrap_or("");
    if primary == "zh" {
        // Traditional for Taiwan, Hong Kong, Macau and "Hant"; Simplified otherwise.
        let parts: Vec<&str> = t.split('-').skip(1).collect();
        let traditional = if parts.contains(&"hant") {
            true
        } else if parts.contains(&"hans") {
            false
        } else {
            parts.iter().any(|p| ["tw", "hk", "mo"].contains(p))
        };
        return if traditional { Lang::ZhTw } else { Lang::ZhCn };
    }
    if primary == "pt" {
        return Lang::PtBr;
    }
    LANGS
        .iter()
        .find(|(_, code, _)| *code == primary)
        .map_or(Lang::En, |(l, ..)| *l)
}

#[cfg(not(windows))]
fn env_lang() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
}

#[cfg(target_os = "macos")]
fn system_lang() -> Option<String> {
    // Apps started from Finder have no LANG; the user's preferred languages are in defaults.
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split(|c: char| c == '"' || c == '(' || c == ',' || c.is_whitespace())
        .find(|s| s.len() >= 2 && s.chars().next().is_some_and(char::is_alphabetic))
        .map(String::from)
        .or_else(env_lang)
}

#[cfg(windows)]
fn system_lang() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
    let mut buf = [0u16; 85];
    // Follow the UI language, not the region chosen for numbers and dates.
    let n = unsafe {
        LCIDToLocaleName(
            GetUserDefaultUILanguage() as u32,
            buf.as_mut_ptr(),
            buf.len() as i32,
            0,
        )
    };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_lang() -> Option<String> {
    env_lang()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_parses_and_keeps_placeholders() {
        let tr = &tables()["tr"];
        let pattern = regex::Regex::new(r"\{[a-z_]+\}").unwrap();
        let placeholders = |s: &str| {
            let mut p: Vec<String> = pattern
                .find_iter(s)
                .map(|m| m.as_str().to_string())
                .collect();
            p.sort();
            p
        };
        for (code, json) in FILES {
            let map: HashMap<String, String> =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{code}: {e}"));
            assert_eq!(map.len(), tr.len(), "{code}: translation key count");
            for (en, text) in &map {
                assert!(
                    !text.trim().is_empty(),
                    "{code}: empty translation for {en:?}"
                );
                assert_eq!(
                    placeholders(en),
                    placeholders(text),
                    "{code}: {en:?} -> {text:?} must keep the placeholders"
                );
            }
            for k in tr.keys() {
                assert!(map.contains_key(k), "{code} lacks {k:?}");
            }
        }
    }

    #[test]
    fn every_language_round_trips_in_settings() {
        let mut codes = std::collections::HashSet::new();
        for (lang, code, _) in LANGS {
            assert!(codes.insert(code), "duplicate language {code}");
            assert_eq!(serde_json::to_string(&lang).unwrap(), format!("\"{code}\""));
            assert_eq!(
                serde_json::from_str::<Lang>(&format!("\"{code}\"")).unwrap(),
                lang
            );
            if lang != Lang::En {
                assert!(
                    tables().contains_key(code),
                    "missing embedded locale {code}"
                );
            }
        }
        assert_eq!(match_lang("ja-JP"), Lang::Ja);
        assert_eq!(match_lang("ko_KR.UTF-8"), Lang::Ko);
        assert_eq!(match_lang("ar-SA"), Lang::Ar);
        assert_eq!(match_lang("nl-NL"), Lang::Nl);
        assert_eq!(match_lang("nl:en"), Lang::Nl);
    }

    #[test]
    fn lookup_and_placeholders() {
        assert_eq!(turkish("Size"), "Boyut");
        assert_eq!(turkish("no such text"), "no such text");
        assert_eq!(translate("de", "no such text"), "no such text");
        assert_eq!(t("Size"), "Size");
        assert_eq!(with_lang(Lang::Tr, || t("Size")), "Boyut");
        assert_eq!(
            with_lang(Lang::Tr, || tf("{n} open", &[("n", "3")])),
            "3 açık"
        );
        assert_eq!(tf("{n} open", &[("n", "3")]), "3 open");
        assert_eq!(
            tf_in("en", "{name} is {n}", &[("name", "{n}"), ("n", "3")]),
            "{n} is 3"
        );
    }

    #[test]
    fn system_tags() {
        assert_eq!(match_lang("de_DE.UTF-8"), Lang::De);
        assert_eq!(match_lang("pt-PT"), Lang::PtBr);
        assert_eq!(match_lang("zh-Hant-TW"), Lang::ZhTw);
        assert_eq!(match_lang("zh_TW"), Lang::ZhTw);
        assert_eq!(match_lang("zh-Hans-CN"), Lang::ZhCn);
        assert_eq!(match_lang("zh-Hans-TW"), Lang::ZhCn);
        assert_eq!(match_lang("zh-HK"), Lang::ZhTw);
        assert_eq!(match_lang("zh"), Lang::ZhCn);
        assert_eq!(match_lang("tr-TR"), Lang::Tr);
        assert_eq!(match_lang("sv-SE"), Lang::En);
    }
}
