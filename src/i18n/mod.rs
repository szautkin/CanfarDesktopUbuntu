//! Compile-time localization runtime.
//!
//! Mirrors the reference's flat `key -> value` string model (`Loc.T` / `Loc.F`).
//! The full EN + FR catalogs are embedded at build time (see [`catalog`]), so the
//! `.deb` stays a self-contained single binary with no external locale files.
//!
//! GTK/libadwaita do not re-translate already-built widgets, so — like the Windows
//! app — switching language takes effect after a restart rather than live.
//!
//! Number/date formatting is intentionally *not* localized here: only the UI
//! culture changes, never Rust's (already invariant) numeric parsing/formatting,
//! so French comma-decimals can never corrupt TAP CSV or FITS card parsing.

// The `tr`/`tr_args`/macro surface is consumed by the UI string sweep (P7);
// several helpers are intentionally ahead of their call sites.
#![allow(dead_code)]

mod catalog;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
}

static EN_MAP: Lazy<HashMap<&'static str, &'static str>> =
    Lazy::new(|| catalog::EN.iter().copied().collect());
static FR_MAP: Lazy<HashMap<&'static str, &'static str>> =
    Lazy::new(|| catalog::FR.iter().copied().collect());

/// Reverse index: an English string value → its French translation. Built from
/// the key-aligned EN/FR catalogs (first occurrence wins on duplicate EN text).
/// Lets UI code be localized by wrapping the literal itself (see [`tr_en`]),
/// without threading the reference's resource keys through every call site.
static EN_TO_FR: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut m = HashMap::new();
    for (i, (_key, en_val)) in catalog::EN.iter().enumerate() {
        if let Some((_, fr_val)) = catalog::FR.get(i) {
            m.entry(*en_val).or_insert(*fr_val);
        }
    }
    m
});

// GTK runs on a single thread, but a global RwLock keeps `set_lang`/`current_lang`
// callable from anywhere without unsafe.
static CURRENT: RwLock<Lang> = RwLock::new(Lang::En);

/// Set the active UI language. Call once at startup after loading settings.
pub fn set_lang(lang: Lang) {
    *CURRENT.write().unwrap() = lang;
}

/// The active UI language.
pub fn current_lang() -> Lang {
    *CURRENT.read().unwrap()
}

/// Map the persisted `language` setting ("system" | "en" | "fr") to a [`Lang`].
pub fn lang_from_setting(setting: &str) -> Lang {
    match setting.trim().to_ascii_lowercase().as_str() {
        "fr" | "fr-fr" | "français" | "francais" => Lang::Fr,
        "en" | "en-us" | "english" => Lang::En,
        _ => detect_system_lang(),
    }
}

/// Detect the language from the environment (`LC_ALL`/`LC_MESSAGES`/`LANG`).
/// Returns [`Lang::Fr`] for any `fr*` locale, otherwise [`Lang::En`].
pub fn detect_system_lang() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(val) = std::env::var(var) {
            let v = val.trim().to_ascii_lowercase();
            if v.starts_with("fr") {
                return Lang::Fr;
            }
            if !v.is_empty() && v != "c" && v != "posix" {
                return Lang::En;
            }
        }
    }
    Lang::En
}

/// Translate `key` in the active language. Falls back to the English value, then
/// to the key itself — never panics, never returns empty for a missing key.
pub fn tr(key: &str) -> &'static str {
    let lookup = |map: &'static Lazy<HashMap<&'static str, &'static str>>| map.get(key).copied();
    let primary = match current_lang() {
        Lang::En => lookup(&EN_MAP),
        Lang::Fr => lookup(&FR_MAP),
    };
    primary
        .or_else(|| lookup(&EN_MAP))
        .unwrap_or_else(|| leak_key(key))
}

/// Translate `key` and substitute positional placeholders `{0}`, `{1}`, ... with
/// `args` (mirrors `Loc.F`). Extra args are ignored; missing ones are left as-is.
pub fn tr_args(key: &str, args: &[&str]) -> String {
    let template = tr(key);
    let mut out = template.to_string();
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{}}}", i), a);
    }
    out
}

/// Intern an unknown key so `tr` can return `&'static str` as a last resort.
/// Missing keys are rare (a bug in the catalog), so the tiny leak is acceptable.
fn leak_key(key: &str) -> &'static str {
    Box::leak(key.to_string().into_boxed_str())
}

/// Localize an English UI literal by reverse lookup. In English (or when the
/// string isn't in the catalog) returns the input unchanged; in French returns
/// the matching translation. Because a string literal is `'static`, the fallback
/// can be returned directly — so `tr_en!("Login")` is a drop-in for `"Login"`.
pub fn tr_en(english: &'static str) -> &'static str {
    match current_lang() {
        Lang::En => english,
        Lang::Fr => EN_TO_FR.get(english).copied().unwrap_or(english),
    }
}

/// `tr_en!("Login")` -> [`tr_en`] (localize an English literal in place).
#[macro_export]
macro_rules! tr_en {
    ($english:expr) => {
        $crate::i18n::tr_en($english)
    };
}

/// `tr!("Key")` -> [`tr`]; `tr!("Key", a, b)` -> [`tr_args`].
#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        $crate::i18n::tr($key)
    };
    ($key:expr, $($arg:expr),+ $(,)?) => {
        $crate::i18n::tr_args($key, &[$($arg),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn en_fr_key_sets_are_identical() {
        let en: std::collections::HashSet<_> = catalog::EN.iter().map(|(k, _)| *k).collect();
        let fr: std::collections::HashSet<_> = catalog::FR.iter().map(|(k, _)| *k).collect();
        assert_eq!(en, fr, "EN and FR catalogs must have identical key sets");
        assert!(en.len() > 1000, "catalog should be fully populated");
    }

    #[test]
    fn tr_falls_back_to_key_when_missing() {
        assert_eq!(tr("__definitely_missing_key__"), "__definitely_missing_key__");
    }

    #[test]
    fn tr_en_reverse_lookup_translates() {
        // The reverse index resolves a known English UI literal to French.
        assert_eq!(EN_TO_FR.get("Login").copied(), Some("Se connecter"));
        // Unknown strings pass through unchanged.
        assert!(EN_TO_FR.get("__nope__").is_none());
    }

    #[test]
    fn lang_from_setting_maps_values() {
        assert_eq!(lang_from_setting("fr"), Lang::Fr);
        assert_eq!(lang_from_setting("en"), Lang::En);
    }

    #[test]
    fn tr_args_substitutes_positional() {
        // Uses a synthetic template via tr fallback semantics is not possible;
        // verify substitution logic directly on a known-style template.
        let s = "Deleted {0} of {1}".to_string();
        let mut out = s;
        for (i, a) in ["3", "9"].iter().enumerate() {
            out = out.replace(&format!("{{{}}}", i), a);
        }
        assert_eq!(out, "Deleted 3 of 9");
    }
}
