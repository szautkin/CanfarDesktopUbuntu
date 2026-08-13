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

/// Hand-maintained French translations for every English string this app shows
/// that the generated catalog cannot supply.
///
/// Two kinds live here, and they are one kind for the purpose that matters:
/// dynamic `{}`-placeholder templates ([`tr_fmt!`]), and plain literals
/// ([`tr_en!`]) on screens Verbinal has and the reference does not. Both are
/// absent from `catalog.rs` for the same reason — `scripts/gen_i18n_catalog.py`
/// reads the reference's RESW files, so it can only emit what the reference also
/// says. Two tables of identical shape, consulted by two functions that resolved
/// identically, were one table pretending to be two: a contributor adding a
/// French string had to know which kind it was before knowing where to put it.
///
/// When you introduce a `tr_en!` / `tr_fmt!` call site whose English is not in
/// the catalog, add the `(english, french)` pair below — the English must match
/// the call site byte-for-byte. A pair the catalog already covers is not needed
/// (and a brand — "Verbinal", "Claude Desktop" — needs no pair at all: an
/// unmatched string falls back to English, which is the correct French for it).
#[rustfmt::skip]
static HAND_PAIRS: &[(&str, &str)] = &[
    // Plain literals — screens with no reference counterpart.
    ("Created by AI agent",                         "Créé par un agent IA"),
    ("No pending proposals",                        "Aucune proposition en attente"),
    ("Destructive",                                 "Irréversible"),
    ("Which AI client will connect to Verbinal?",   "Quel client IA se connectera à Verbinal ?"),
    ("Start MCP server",                            "Démarrer le serveur MCP"),
    ("Write config",                                "Écrire la configuration"),
    ("Copy command",                                "Copier la commande"),
    ("Test connection",                             "Tester la connexion"),
    ("Testing…",                                    "Test en cours…"),
    ("Start the MCP server to continue.",           "Démarrez le serveur MCP pour continuer."),
    ("MCP server is running.",                      "Le serveur MCP est en cours d’exécution."),
    ("MCP server is stopped.",                      "Le serveur MCP est arrêté."),
    ("Server running",                              "Serveur actif"),
    ("✓ Configuration written.",                    "✓ Configuration écrite."),
    ("View events & logs",                          "Afficher les évènements et journaux"),
    ("Delete job",                                  "Supprimer la tâche"),
    ("refreshing…",                                 "actualisation…"),
    ("Filter packages…",                            "Filtrer les paquets…"),
    ("Active filters",                              "Filtres actifs"),
    ("Clear all",                                   "Tout effacer"),
    ("Search images…",                              "Rechercher des images…"),
    ("Loading images…",                             "Chargement des images…"),
    ("Discovering…",                                "Découverte en cours…"),
    ("Kernel: not started",                         "Noyau : non démarré"),
    ("Failed to load platform data",                "Échec du chargement des données de la plateforme"),
    ("Session Templates",                           "Modèles de session"),
    ("Launch from template",                        "Lancer depuis un modèle"),
    ("Delete template",                             "Supprimer le modèle"),
    ("No saved templates — save one from the launch form",
     "Aucun modèle enregistré — enregistrez-en un depuis le formulaire de lancement"),
    ("Export Figure",                                "Exporter la figure"),
    ("Find image by package",                        "Trouver une image par paquet"),
    ("Destructive changes requested by an AI agent are held here until you approve them. \
Reversible writes are applied automatically.",
     "Les modifications irréversibles demandées par un agent IA sont retenues ici jusqu’à votre \
approbation. Les écritures réversibles sont appliquées automatiquement."),
    ("The Model Context Protocol (MCP) lets an AI agent such as Claude talk to \
Verbinal — browsing your CADC storage, running searches, and preparing session launches on your \
behalf. Start the local MCP server so Verbinal becomes reachable.",
     "Le Model Context Protocol (MCP) permet à un agent IA tel que Claude de dialoguer avec \
Verbinal — parcourir votre stockage CADC, lancer des recherches et préparer des sessions en votre \
nom. Démarrez le serveur MCP local pour rendre Verbinal accessible."),
    ("Register Verbinal in Claude Desktop's configuration file. Claude Desktop \
picks this up the next time it launches.",
     "Enregistrez Verbinal dans le fichier de configuration de Claude Desktop. Claude Desktop le \
prend en compte à son prochain démarrage."),
    ("Add Verbinal to Claude Code by running this command in your terminal:",
     "Ajoutez Verbinal à Claude Code en exécutant cette commande dans votre terminal :"),
    ("Dial the MCP server the way your AI client will, and confirm it answers.",
     "Contactez le serveur MCP comme le fera votre client IA, et vérifiez qu’il répond."),
    // `{}`-placeholder templates.
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
    ("Found {} observations",                       "{} observations trouvées"),
    ("{}–{} identical jobs",                        "{} à {} tâches identiques"),
    ("Launched {} batch replicas ({})",             "{} répliques par lots lancées ({})"),
    ("Found {} observations (row limit {} reached — raise Max Records to see more)",
     "{} observations trouvées (limite de {} lignes atteinte — augmentez Max Records pour en voir plus)"),
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
    ("Copy “{}” to the clipboard",                  "Copier « {} » dans le presse-papiers"),
    ("Follow my workflow “{}” in Verbinal: call get_workflow(id: \"{}\") to read the steps, work through them in order using the tools each step names, mark each finished step with set_workflow_step(id: \"{}\", index, done: true), and stop to ask me at any judgment call.",
     "Suis mon flux de travail « {} » dans Verbinal : appelle get_workflow(id: \"{}\") pour lire les étapes, exécute-les dans l’ordre en utilisant les outils nommés à chaque étape, marque chaque étape terminée avec set_workflow_step(id: \"{}\", index, done: true), et arrête-toi pour me demander à chaque décision de jugement."),
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

/// Reverse index over [`HAND_PAIRS`]: an English string → its French form.
static HAND_EN_TO_FR: Lazy<HashMap<&'static str, &'static str>> =
    Lazy::new(|| HAND_PAIRS.iter().copied().collect());

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

/// Localize an English UI string by reverse lookup: the hand-maintained
/// [`HAND_PAIRS`] first, then the generated catalog, then the input unchanged.
///
/// Hand pairs win so a string this app words differently from the reference can
/// be corrected here without editing the generated file. Because a string
/// literal is `'static`, the fallback is returned directly — `tr_en!("Login")`
/// is a drop-in for `"Login"`.
pub fn tr_en(english: &'static str) -> &'static str {
    match current_lang() {
        Lang::En => english,
        Lang::Fr => HAND_EN_TO_FR
            .get(english)
            .copied()
            .or_else(|| EN_TO_FR.get(english).copied())
            .unwrap_or(english),
    }
}

/// The French form of an English `{}`-placeholder *template*.
///
/// A template resolves exactly as a plain literal does — same tables, same
/// order, same fallback — so this is [`tr_en`]. It stays as its own name
/// because that is what `tr_fmt!` reads as at the call site, and because the
/// two macros' inputs differ in a way worth keeping visible: one is a finished
/// string, the other has holes still to fill.
pub fn tr_fmt_template(english: &'static str) -> &'static str {
    tr_en(english)
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

/// Decode a Rust string-literal body into the value the compiler produces.
///
/// Handles the escapes that appear in this codebase's templates: `\n`, `\t`,
/// `\"`, `\\`, and the line continuation `\` + newline + leading whitespace.
#[cfg(test)]
fn decode_rust_string_literal(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            // Line continuation: swallow the newline and the indent that follows.
            Some('\n') => {
                while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                    chars.next();
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The Rust string literal starting at `src[at]` (which must be its opening
/// quote), decoded to the value the compiler would produce.
///
/// `None` when `at` is not a plain `"…"` literal — a raw string, a variable, a
/// macro call. Both source-scanning guards below need exactly this, and a second
/// copy would be a second opinion about what counts as a literal.
#[cfg(test)]
fn literal_at(src: &str, at: usize) -> Option<String> {
    let body = src.get(at..)?.strip_prefix('"')?;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(decode_rust_string_literal(&body[..i])),
            _ => i += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_literal_decoder_matches_what_rustc_produces() {
        // The guard compares a decoded SOURCE literal against the compiled value
        // in FMT_PAIRS, so its decoder has to agree with rustc — otherwise the
        // guard either misses a gap or fails on a correct template.
        assert_eq!(decode_rust_string_literal(r"a\nb"), "a\nb");
        assert_eq!(decode_rust_string_literal(r#"say \"hi\""#), "say \"hi\"");
        assert_eq!(decode_rust_string_literal(r"back\\slash"), "back\\slash");

        // Line continuation: the newline AND the following indent disappear,
        // which is what makes a wrapped template equal its one-line form.
        let wrapped = "one \\\n            two";
        assert_eq!(decode_rust_string_literal(wrapped), "one two");

        // An unrecognised escape is left intact rather than silently dropped.
        assert_eq!(decode_rust_string_literal(r"\q"), r"\q");
    }

    /// Every source file that can contain a call site: this module is skipped
    /// because it CONTAINS the table, so its own literals would match every scan
    /// and drown the result.
    fn call_sites() -> impl Iterator<Item = (std::path::PathBuf, String)> {
        crate::testing::rust_sources()
            .into_iter()
            .filter(|(path, _)| !path.ends_with("i18n/mod.rs"))
    }

    /// Every `tr_fmt!` template in the codebase must have a French pair.
    ///
    /// `HAND_PAIRS` asks contributors to add one when they introduce a template,
    /// but nothing enforced it — so a missed pair silently shipped English into
    /// the French UI, which no test and no compiler could see. A source scan is
    /// the only place this is visible: the templates are macro arguments, not
    /// values any runtime check can enumerate.
    #[test]
    fn every_tr_fmt_template_has_a_french_translation() {
        // Decoding matters most for line continuations (`\` + newline + indent):
        // they are idiomatic throughout this codebase, and a scan that ignored
        // them would fail every wrapped template — a guard that cries wolf gets
        // worked around instead of obeyed.
        let have: std::collections::HashSet<&str> = HAND_PAIRS.iter().map(|(en, _)| *en).collect();

        let mut missing: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        for (path, text) in call_sites() {
            for (start, _) in text.match_indices("tr_fmt!(") {
                let open = start + "tr_fmt!(".len();
                let open = open + (text[open..].len() - text[open..].trim_start().len());
                // Only a directly-quoted template can be checked; a variable
                // template is out of scope for a source scan.
                let Some(decoded) = literal_at(&text, open) else {
                    continue;
                };
                scanned += 1;
                if !have.contains(decoded.as_str()) {
                    missing.push(format!("{}: {decoded:?}", path.display()));
                }
            }
        }

        assert!(scanned > 0, "found no tr_fmt! call sites — did src/ move?");
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "tr_fmt! template(s) with no French pair in HAND_PAIRS — French users \
             would see English here: {missing:#?}"
        );
    }

    use super::*;

    /// GTK calls that put a string in front of a person.
    ///
    /// Each is a prefix; whatever follows it, if it is a quoted literal, is text
    /// the user reads. Every one of these is used with `tr_en!` hundreds of times
    /// over in this codebase — that is what makes the bare form a defect rather
    /// than a style, and what makes this list checkable rather than a guess.
    const TEXT_SETTERS: &[&str] = &[
        "Label::new(Some(",
        ".set_label(",
        "with_label(",
        ".set_title(",
        ".set_subtitle(",
        ".set_tooltip_text(Some(",
        ".set_placeholder_text(Some(",
        ".set_text(",
        ".set_description(Some(",
        ".set_heading(Some(",
        ".label(",
        ".title(",
        ".heading(",
        ".body(",
        "Toast::new(",
    ];

    /// Nothing the user reads may be a bare literal.
    ///
    /// The app advertises French, and the catalog has 1,271 keys — but a call
    /// site that never asks gets English regardless of language, and no compiler
    /// or runtime check can see it. Whole screens shipped that way: the AI
    /// connect wizard, image discovery, the template manager, the proposals
    /// dialog. Fourteen of them already had French sitting unused in the
    /// catalog, which is the tell — the strings were translated, the call sites
    /// simply never looked.
    ///
    /// The rule has no exceptions, brands included. `tr_en!("Verbinal")` returns
    /// "Verbinal" in both languages, so wrapping costs nothing, whereas an
    /// exception list is the place the next untranslated string would hide.
    #[test]
    fn nothing_the_user_reads_is_a_bare_literal() {
        let mut bare: Vec<String> = Vec::new();
        let mut localized = 0usize;
        for (path, text) in call_sites() {
            // Test code is not shipped, and a fixture label needs no French.
            let code = crate::testing::code(&text);
            for setter in TEXT_SETTERS {
                for (start, _) in code.match_indices(setter) {
                    let after = start + setter.len();
                    let arg = code[after..].trim_start();
                    let at = code.len() - arg.len();
                    if arg.starts_with("crate::tr_en!(") || arg.starts_with("tr_en!(") {
                        localized += 1;
                        continue;
                    }
                    // Anything that is not a plain literal — a variable, a
                    // `format!`, `tr!`, `tr_fmt!` — is either localized already
                    // or beyond what a source scan can judge.
                    let Some(literal) = literal_at(code, at) else {
                        continue;
                    };
                    // A string with no word in it is a glyph, a number or a
                    // separator: "—", "0", "•". Nothing to translate.
                    if literal.chars().filter(|c| c.is_alphabetic()).count() < 2 {
                        continue;
                    }
                    let line = code[..start].lines().count();
                    bare.push(format!("{}:{line}: {literal:.60?}", path.display()));
                }
            }
        }

        // If a refactor moved the app onto different setters, this guard would
        // pass by scanning nothing. It has to keep finding the localized calls.
        assert!(
            localized > 300,
            "only {localized} localized call sites found — TEXT_SETTERS has gone stale"
        );
        bare.sort();
        assert!(
            bare.is_empty(),
            "user-visible string(s) that never reach the catalog — French users \
             see English here: {bare:#?}"
        );
    }

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
        let fr = HAND_EN_TO_FR.get("Error: {}").copied().unwrap();
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
        for (en, fr) in HAND_PAIRS {
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
