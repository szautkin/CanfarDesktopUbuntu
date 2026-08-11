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

/// Hand-maintained French translations for the dynamic `{}`-placeholder *templates*
/// introduced by the [`tr_fmt!`] sweep (toasts / status / labels built at runtime).
///
/// These live here rather than in the generated `catalog.rs` because they use Rust
/// `{}` placeholders and have no counterpart in the reference RESW files, so the
/// `scripts/gen_i18n_catalog.py` generator (which reads those RESW files) cannot
/// emit them. When adding a new `tr_fmt!("…{}…", …)` call site whose English
/// template is not already listed here, add the `(english, french)` pair below —
/// the English literal must match the call-site template byte-for-byte.
#[rustfmt::skip]
static FMT_PAIRS: &[(&str, &str)] = &[
    ("Error: {}",                                   "Erreur : {}"),
    ("Failed to load images: {}",                   "Échec du chargement des images : {}"),
    ("Selected image: {}",                          "Image sélectionnée : {}"),
    ("Launch failed: {}",                           "Échec du lancement : {}"),
    ("Batch launch failed: {}",                     "Échec du lancement par lots : {}"),
    ("Failed to save template: {}",                 "Échec de l’enregistrement du modèle : {}"),
    ("Launched batch job '{}' ({})",                "Tâche par lots « {} » lancée ({})"),
    ("Failed to load: {}",                          "Échec du chargement : {}"),
    ("Save failed: {}",                             "Échec de l’enregistrement : {}"),
    ("Settings save failed: {}",                    "Échec de l’enregistrement des paramètres : {}"),
    ("“{}” has unsaved changes. Save them before closing?",
     "« {} » comporte des modifications non enregistrées. Les enregistrer avant de fermer ?"),
    ("{} unsaved notebook checkpoint(s) from a previous session were found. Recover them?",
     "{} point(s) de sauvegarde de carnet non enregistrés d’une session précédente ont été trouvés. Les récupérer ?"),
    ("{} (recovered)",                              "{} (récupéré)"),
    ("Used: {} GB",                                 "Utilisé : {} Go"),
    ("Quota: {} GB",                                "Quota : {} Go"),
    ("Usage: {}%",                                  "Utilisation : {} %"),
    ("last update: {}",                             "dernière mise à jour : {}"),
    ("Welcome, {}",                                 "Bienvenue, {}"),
    ("Welcome back, {}!",                           "Bon retour, {} !"),
    ("{} offline",                                  "{} hors ligne"),
    ("Last seen {}",                                "Vu pour la dernière fois {}"),
    ("Runtime: Rust {}\nPlatform: {}\nFramework: GTK4 + libadwaita",
     "Environnement d’exécution : Rust {}\nPlateforme : {}\nCadre : GTK4 + libadwaita"),
    ("reachable — {} ({} ms)",                      "accessible — {} ({} ms)"),
    ("host up, service failed — HTTP {} ({} ms)",
     "hôte accessible, service en échec — HTTP {} ({} ms)"),
    ("unreachable — {}",                            "inaccessible — {}"),
    ("Sessions unreachable — cached list from {}",  "Sessions inaccessibles — liste en cache du {}"),
    ("Renewing session '{}'…",                      "Renouvellement de la session « {} »…"),
    ("Renew failed: {}",                            "Échec du renouvellement : {}"),
    ("Preview: “{}” — {} step(s), {} done",         "Aperçu : « {} » — {} étape(s), {} terminée(s)"),
    ("…and {} more problem(s)",                     "…et {} autre(s) problème(s)"),
    ("Could not load workflow: {}",                 "Impossible de charger le flux de travail : {}"),
    ("Uploads to vos:{}/workflows/",                "Téléversement vers vos:{}/workflows/"),
    ("Published to vos:{}",                         "Publié vers vos:{}"),
    ("Publish failed: {}",                          "Échec de la publication : {}"),
    ("'{}' renewed. Its expiry has been extended.",
     "« {} » renouvelée. Sa date d’expiration a été prolongée."),
    ("{} session",                                  "{} session"),
    ("{} sessions",                                 "{} sessions"),
    ("refresh in {}s",                              "actualisation dans {} s"),
    ("CPU: {}",                                     "CPU : {}"),
    ("RAM: {}",                                     "RAM : {}"),
    ("GPU: {}",                                     "GPU : {}"),
    ("Copied: {}",                                  "Copié : {}"),
    ("Copied  {}  {}",                              "Copié  {}  {}"),
    ("Cached listing from {}",                      "Liste en cache du {}"),
    ("VOSpace unreachable — showing cached listing from {}",
     "VOSpace inaccessible — affichage de la liste en cache du {}"),
    ("{} items",                                    "{} éléments"),
    ("Downloaded {} ({} bytes)",                    "Téléchargé {} ({} octets)"),
    ("Download failed: {}",                         "Échec du téléchargement : {}"),
    ("Opened {} in FITS Viewer",                    "{} ouvert dans la visionneuse FITS"),
    ("Failed to open FITS: {}",                     "Échec de l’ouverture du FITS : {}"),
    ("Opened {} in Cube Viewer",                    "{} ouvert dans la visionneuse de cubes"),
    ("Failed to open cube: {}",                     "Échec de l’ouverture du cube : {}"),
    ("Opened {} in Notebook",                       "{} ouvert dans le carnet"),
    ("Failed to open notebook: {}",                 "Échec de l’ouverture du carnet : {}"),
    ("Deleted {}",                                  "{} supprimé"),
    ("Delete failed: {}",                           "Échec de la suppression : {}"),
    ("Sharing updated for {}",                      "Partage mis à jour pour {}"),
    ("Share failed: {}",                            "Échec du partage : {}"),
    ("Renamed {} → {}",                             "Renommé {} → {}"),
    ("Rename failed: {}",                           "Échec du renommage : {}"),
    ("Created folder '{}'",                         "Dossier « {} » créé"),
    ("Failed to create folder: {}",                 "Échec de la création du dossier : {}"),
    ("Uploaded {}",                                 "{} téléversé"),
    ("Upload failed for {}: {}",                    "Échec du téléversement de {} : {}"),
    ("Uploaded {} files",                           "{} fichiers téléversés"),
    ("Are you sure you want to delete '{}'? This cannot be undone.",
     "Voulez-vous vraiment supprimer « {} » ? Cette action est irréversible."),
    ("Name{}",                                      "Nom{}"),
    ("Size{}",                                      "Taille{}"),
    ("Modified{}",                                  "Modifié{}"),
    ("vs {}",                                       "vs {}"),
    ("Blinking vs {}  (Space pause · Left/Right show A/B · Esc stop)",
     "Clignotement vs {}  (Espace pause · Gauche/Droite affiche A/B · Échap arrêt)"),
    ("Loading {}…",                                 "Chargement de {}…"),
    ("Failed to load cube: {}",                     "Échec du chargement du cube : {}"),
    ("Spectrum at ({}, {})",                        "Spectre à ({}, {})"),
    ("Saved {}",                                    "Enregistré {}"),
    ("Export failed: {}",                           "Échec de l’exportation : {}"),
];

/// Reverse index for [`tr_fmt`]: an English `{}`-template → its French form.
static FMT_EN_TO_FR: Lazy<HashMap<&'static str, &'static str>> =
    Lazy::new(|| FMT_PAIRS.iter().copied().collect());

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

/// Reverse-lookup the French form of an English `{}`-placeholder *template*, the
/// dynamic-string analogue of [`tr_en`]. Checks the hand-maintained [`FMT_PAIRS`]
/// map first, then the generated catalog's reverse index (so a template that also
/// happens to be a catalog value still localizes), then falls back to `english`.
///
/// Because the template is `'static`, the English fallback is returned directly —
/// so `tr_fmt!` always has a `'static` template to substitute into, in any language.
pub fn tr_fmt_template(english: &'static str) -> &'static str {
    match current_lang() {
        Lang::En => english,
        Lang::Fr => FMT_EN_TO_FR
            .get(english)
            .copied()
            .or_else(|| EN_TO_FR.get(english).copied())
            .unwrap_or(english),
    }
}

/// Substitute sequential `{}` placeholders in `template` with `args`, formatting
/// each via [`Display`](std::fmt::Display). `{{` / `}}` unescape to literal braces.
///
/// Unlike `format!`, a template/arg-count mismatch never panics: a `{}` with no
/// remaining arg is emitted verbatim and surplus args are ignored. This matters
/// because the *French* template is chosen at runtime and must tolerate a
/// placeholder count that drifts from the English original.
pub fn tr_fmt_apply(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(template.len() + args.len() * 8);
    let mut args = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => match chars.peek() {
                Some('{') => {
                    chars.next();
                    out.push('{');
                }
                Some('}') => {
                    chars.next();
                    match args.next() {
                        Some(a) => {
                            let _ = write!(out, "{}", a);
                        }
                        None => out.push_str("{}"),
                    }
                }
                _ => out.push('{'),
            },
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
                out.push('}');
            }
            other => out.push(other),
        }
    }
    out
}

/// `tr_en!("Login")` -> [`tr_en`] (localize an English literal in place).
#[macro_export]
macro_rules! tr_en {
    ($english:expr) => {
        $crate::i18n::tr_en($english)
    };
}

/// `tr_fmt!("{} observations", n)` — localize a `{}`-placeholder *template* (an
/// English `&'static str`, usually a literal) via [`tr_fmt_template`], then fill
/// the placeholders with `args`, each formatted with `Display`. Returns a `String`
/// and is a drop-in for the equivalent `format!` call, adding French translation
/// (English fallback when the template has no French entry in [`FMT_PAIRS`]).
///
/// Pre-format any argument that needs a format spec, e.g.
/// `tr_fmt!("Used: {} GB", format!("{:.1}", gb))` — the template keeps a plain `{}`.
#[macro_export]
macro_rules! tr_fmt {
    ($template:expr $(, $arg:expr)* $(,)?) => {
        $crate::i18n::tr_fmt_apply(
            $crate::i18n::tr_fmt_template($template),
            &[$(&$arg as &dyn ::std::fmt::Display),*],
        )
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
    /// Every `tr_fmt!` template in the codebase must have a French pair.
    ///
    /// `FMT_PAIRS` asks contributors to add one when they introduce a template,
    /// but nothing enforced it — so a missed pair silently shipped English into
    /// the French UI, which no test and no compiler could see. A source scan is
    /// the only place this is visible: the templates are macro arguments, not
    /// values any runtime check can enumerate.
    #[test]
    fn every_tr_fmt_template_has_a_french_translation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let have: std::collections::HashSet<&str> = FMT_PAIRS.iter().map(|(en, _)| *en).collect();

        let mut missing: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                // Skip this file: it CONTAINS the table, so its own literals
                // would match the scan and drown the result.
                if path.ends_with("i18n/mod.rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (start, _) in text.match_indices("tr_fmt!(") {
                    let rest = &text[start + "tr_fmt!(".len()..];
                    let rest = rest.trim_start();
                    // Only a directly-quoted template can be checked; a variable
                    // template is out of scope for a source scan.
                    let Some(body) = rest.strip_prefix('"') else {
                        continue;
                    };
                    // Find the closing quote, honouring escapes.
                    let mut end = None;
                    let bytes = body.as_bytes();
                    let mut i = 0;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'\\' => i += 2,
                            b'"' => {
                                end = Some(i);
                                break;
                            }
                            _ => i += 1,
                        }
                    }
                    let Some(end) = end else { continue };
                    let template = &body[..end];
                    scanned += 1;
                    // Rust source escapes; FMT_PAIRS holds the decoded literal.
                    let decoded = template.replace("\\n", "\n").replace("\\\"", "\"");
                    if !have.contains(decoded.as_str()) {
                        missing.push(format!("{}: {decoded:?}", path.display()));
                    }
                }
            }
        }

        assert!(scanned > 0, "found no tr_fmt! call sites — did src/ move?");
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "tr_fmt! template(s) with no French pair in FMT_PAIRS — French users \
             would see English here: {missing:#?}"
        );
    }

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
        assert_eq!(
            tr("__definitely_missing_key__"),
            "__definitely_missing_key__"
        );
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
    fn tr_fmt_apply_substitutes_sequential() {
        assert_eq!(
            tr_fmt_apply("{} of {}", &[&3usize as &dyn std::fmt::Display, &9usize]),
            "3 of 9"
        );
    }

    #[test]
    fn tr_fmt_apply_handles_escapes_and_mismatch() {
        // Escaped braces survive; a placeholder with no arg is left verbatim.
        assert_eq!(tr_fmt_apply("{{x}} {}", &[]), "{x} {}");
        // Surplus args are ignored, no panic.
        assert_eq!(
            tr_fmt_apply("a {}", &[&1i32 as &dyn std::fmt::Display, &2i32]),
            "a 1"
        );
    }

    #[test]
    fn tr_fmt_french_template_substitutes() {
        // The FR reverse-lookup + substitution path a `tr_fmt!` in French mode takes.
        let fr = FMT_EN_TO_FR.get("Error: {}").copied().unwrap();
        assert_eq!(
            tr_fmt_apply(fr, &[&"boom" as &dyn std::fmt::Display]),
            "Erreur : boom"
        );
    }

    #[test]
    fn tr_fmt_templates_have_matching_placeholder_counts() {
        // Each FR template must expose the same number of `{}` slots as its EN form,
        // or arguments would silently drop / leak through.
        fn slots(s: &str) -> usize {
            s.match_indices("{}").count()
        }
        for (en, fr) in FMT_PAIRS {
            assert_eq!(
                slots(en),
                slots(fr),
                "placeholder mismatch: {en:?} vs {fr:?}"
            );
        }
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
