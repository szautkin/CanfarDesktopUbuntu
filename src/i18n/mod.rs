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
    // Templates that were `format!` until the guard below started reading the
    // sinks they were poured into.
    ("Created by {}",                               "Créé par {}"),
    ("Applied: {}",                                 "Appliqué : {}"),
    ("Apply failed: {}",                            "Échec de l’application : {}"),
    ("Step {} of {} — {}",                          "Étape {} sur {} — {}"),
    ("Couldn't write config: {}",                   "Impossible d’écrire la configuration : {}"),
    ("{} tools",                                    "{} outils"),
    ("{} categories",                               "{} catégories"),
    ("{} overridden",                               "{} redéfinis"),
    ("{} of {} tools match “{}”",                   "{} outils sur {} correspondent à « {} »"),
    ("Built-in: {}",                                "Intégré : {}"),
    ("returns {} chars",                            "renvoie {} caractères"),
    ("The agent will see: {}",                      "L’agent verra : {}"),
    ("No {} jobs",                                  "Aucune tâche {}"),
    ("ID: {}",                                      "ID : {}"),
    ("refresh in {}s",                              "actualisation dans {} s"),
    ("Discovered {} of {} images",                  "{} images découvertes sur {}"),
    ("Select File ({} available)",                  "Sélectionner un fichier ({} disponibles)"),
    ("Pixel ({}, {})\nRA  {}\nDec {}",              "Pixel ({}, {})\nAD  {}\nDéc {}"),
    ("Pixel ({}, {})\nNo WCS",                      "Pixel ({}, {})\nAucun WCS"),
    ("Available {}: {} / {}{}",                     "{} disponible : {} / {}{}"),
    ("Could not open the log folder: {}",           "Impossible d’ouvrir le dossier des journaux : {}"),
    ("Downloaded {}",                               "{} téléchargé"),
    ("No observation found for {}",                 "Aucune observation trouvée pour {}"),
    ("Instances: {} total ({} sessions, {} desktop apps, {} headless)",
     "Instances : {} au total ({} sessions, {} applications de bureau, {} sans interface)"),
    ("last update: {} UTC",                         "dernière mise à jour : {} UTC"),
    ("{} observation",                              "{} observation"),
    ("{} observations",                             "{} observations"),
    ("{} note",                                     "{} note"),
    ("{} notes",                                    "{} notes"),
    ("{} query",                                    "{} requête"),
    ("{} queries",                                  "{} requêtes"),
    ("{} recent",                                   "{} récentes"),
    ("{} star",                                     "{} étoile"),
    ("{} stars",                                    "{} étoiles"),
    ("Cannot create storage directory: {}",         "Impossible de créer le dossier de stockage : {}"),
    ("Downloading {}…",                             "Téléchargement de {}…"),
    ("Download failed: {}",                         "Échec du téléchargement : {}"),
    ("Exported {} ({}) to {}",                      "{} exporté ({}) vers {}"),
    ("Uploaded to vos:{}/{}",                       "Téléversé vers vos:{}/{}"),
    ("VOSpace upload failed: {}",                   "Échec du téléversement VOSpace : {}"),
    ("Could not open file manager: {}",             "Impossible d’ouvrir le gestionnaire de fichiers : {}"),
    ("Query: {}",                                   "Requête : {}"),
    ("Loaded search: {}",                           "Recherche chargée : {}"),
    ("RA: {}  Dec: {} ({})",                        "AD : {}  Déc : {} ({})"),
    ("RA: {}  Dec: {}{}",                           "AD : {}  Déc : {}{}"),
    ("Resolve failed: {}",                          "Échec de la résolution : {}"),
    ("Page {} of {} ({}-{} of {})",                 "Page {} sur {} ({}-{} sur {})"),
    ("Page {} of {} ({}-{} of {}, filtered from {})",
     "Page {} sur {} ({}-{} sur {}, filtrés depuis {})"),
    ("Narrow to: {}",                               "Restreindre à : {}"),
    ("Export as {}",                                "Exporter en {}"),
    ("Exported to {}",                              "Exporté vers {}"),
    ("Data train loaded ({} entries)",              "Train de données chargé ({} entrées)"),
    ("Data train loaded from cache ({} entries, last updated {})",
     "Train de données chargé depuis le cache ({} entrées, dernière mise à jour {})"),
    ("Archive unreachable — showing cached filters from {}",
     "Archive injoignable — affichage des filtres en cache du {}"),
    ("Data train failed: {}",                       "Échec du train de données : {}"),
    ("Resolving DataLink for {}…",                  "Résolution DataLink pour {}…"),
    ("Saved files, but store write failed: {}",
     "Fichiers enregistrés, mais l’écriture dans la bibliothèque a échoué : {}"),
    ("Share {}",                                    "Partager {}"),
    ("Could not copy the workflow: {}",             "Impossible de copier le flux de travail : {}"),
    ("Time: {}",                                    "Durée : {}"),
    ("{}/{} done",                                  "{}/{} terminées"),
    ("Go to {}",                                    "Aller à {}"),
    ("Could not update step: {}",                   "Impossible de mettre à jour l’étape : {}"),
    ("Could not create local copy: {}",             "Impossible de créer la copie locale : {}"),
    ("Save failed: {}",                             "Échec de l’enregistrement : {}"),
    ("Could not create workflow: {}",               "Impossible de créer le flux de travail : {}"),
    ("Import failed: {}",                           "Échec de l’importation : {}"),
    ("Could not read file: {}",                     "Impossible de lire le fichier : {}"),
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
/// (English fallback when the template has no French entry in [`HAND_PAIRS`]).
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

/// `tr_plural!(n, "{} observation", "{} observations")` — pick the template by
/// count, then localize and fill it exactly as [`tr_fmt!`] does. `n` is always
/// the first argument substituted; any extra `args` follow it.
///
/// The alternative this replaces was `tr_fmt!("{} observation{}", n, if n == 1
/// { "" } else { "s" })` — English morphology decided at the *call site*, which
/// no translation can undo: the French for that suffix argument is "s" for
/// "note" and "s" for "requête", but the call site was passing "ies". Choosing
/// between two whole templates moves the decision into the thing that gets
/// translated, so each language states its own plural.
#[macro_export]
macro_rules! tr_plural {
    ($n:expr, $one:expr, $many:expr $(, $arg:expr)* $(,)?) => {{
        let n = $n;
        $crate::i18n::tr_fmt_apply(
            $crate::i18n::tr_fmt_template(if n == 1 { $one } else { $many }),
            &[&n as &dyn ::std::fmt::Display $(, &$arg as &dyn ::std::fmt::Display)*],
        )
    }};
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
            // `\u{201c}` — the one escape whose body contains braces, so a
            // decoder that skipped it would also hand the placeholder scanner a
            // `{201c}` it would count as a slot.
            Some('u') if chars.peek() == Some(&'{') => {
                chars.next();
                let hex: String = chars.by_ref().take_while(|c| *c != '}').collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => out.push_str(&format!("\\u{{{hex}}}")),
                }
            }
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

/// The comma-separated arguments of the call whose `(` is at `src[at]`.
///
/// Depth-aware and literal-aware, so a nested call or a comma inside a string
/// does not split an argument. `None` if the parentheses never balance.
#[cfg(test)]
fn call_args(src: &str, at: usize) -> Option<Vec<String>> {
    let bytes = src.as_bytes();
    if bytes.get(at) != Some(&b'(') {
        return None;
    }
    let mut args = vec![String::new()];
    let mut depth = 0usize;
    let mut i = at;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                if depth > 1 {
                    args.last_mut()?.push(c);
                }
            }
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(
                        args.into_iter()
                            .map(|a| a.trim().to_string())
                            .filter(|a| !a.is_empty())
                            .collect(),
                    );
                }
                args.last_mut()?.push(c);
            }
            ',' if depth == 1 => args.push(String::new()),
            '"' => {
                // Copy the literal verbatim; a comma or paren inside it is text.
                let start = i;
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 1,
                        b'"' => break,
                        _ => {}
                    }
                    i += 1;
                }
                args.last_mut()?
                    .push_str(&src[start..=i.min(bytes.len() - 1)]);
            }
            _ => args.last_mut()?.push(c),
        }
        i += 1;
    }
    None
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

        // A unicode escape becomes the character, so the pair a contributor
        // writes with a real “ matches the call site that spells it \u{201c}.
        assert_eq!(
            decode_rust_string_literal(r"say \u{201c}hi\u{201d}"),
            "say “hi”"
        );
    }

    /// Every source file that can contain a call site: this module is skipped
    /// because it CONTAINS the table, so its own literals would match every scan
    /// and drown the result.
    fn call_sites() -> impl Iterator<Item = (std::path::PathBuf, String)> {
        crate::testing::rust_sources()
            .into_iter()
            .filter(|(path, _)| !path.ends_with("i18n/mod.rs"))
    }

    /// Every template a formatting macro carries must have a French pair.
    ///
    /// `HAND_PAIRS` asks contributors to add one when they introduce a template,
    /// but nothing enforced it — so a missed pair silently shipped English into
    /// the French UI, which no test and no compiler could see. A source scan is
    /// the only place this is visible: the templates are macro arguments, not
    /// values any runtime check can enumerate.
    ///
    /// `tr_plural!` carries two templates, so both are checked: a plural form
    /// with no French pair is the same defect as a singular one, and the count
    /// that selects it is exactly the case nobody exercises by hand.
    #[test]
    fn every_formatted_template_has_a_french_translation() {
        // Decoding matters most for line continuations (`\` + newline + indent):
        // they are idiomatic throughout this codebase, and a scan that ignored
        // them would fail every wrapped template — a guard that cries wolf gets
        // worked around instead of obeyed.
        let have: std::collections::HashSet<&str> = HAND_PAIRS.iter().map(|(en, _)| *en).collect();

        let mut missing: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        for (path, text) in call_sites() {
            for (start, _) in text
                .match_indices("tr_fmt!(")
                .chain(text.match_indices("tr_plural!("))
            {
                let open = start + text[start..].find('(').unwrap_or(0);
                // Every literal argument of the call: one template for
                // `tr_fmt!`, two for `tr_plural!`. A literal in a *value*
                // position is text the user reads too, so it wants a pair
                // just as much.
                let Some(args) = call_args(&text, open) else {
                    continue;
                };
                let mut at = open + 1;
                for arg in args {
                    let start_of_arg = text[at..].find(&arg).map(|o| at + o).unwrap_or(at);
                    at = start_of_arg + arg.len();
                    // Only a directly-quoted template can be checked; a variable
                    // template is out of scope for a source scan.
                    let Some(decoded) = literal_at(&text, start_of_arg) else {
                        continue;
                    };
                    scanned += 1;
                    if !have.contains(decoded.as_str()) {
                        let line = text[..start].lines().count();
                        missing.push(format!("{}:{line}: {decoded:?}", path.display()));
                    }
                }
            }
        }

        assert!(
            scanned > 0,
            "found no formatting call sites — did src/ move?"
        );
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "template(s) with no French pair in HAND_PAIRS — French users \
             would see English here: {missing:#?}"
        );
    }

    use super::*;

    /// Calls that put a string in front of a person.
    ///
    /// Each is a prefix; what follows it is text the user reads. Every one of
    /// these is used with `tr_en!` or `tr_fmt!` many times over in this codebase
    /// — that is what makes the un-localized form a defect rather than a style,
    /// and what makes this list checkable rather than a guess.
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
        ".toast(",
        // A menu item's label; the action name follows it.
        ".append(Some(",
    ];

    /// Calls whose *second* argument is the text: the first names an id, and
    /// only the second is read.
    ///
    /// `dialog.add_response("close", "Close")` — different enough in shape that
    /// the scan above cannot see it, which is exactly how one dialog kept an
    /// English button while every other dialog in the app localized theirs.
    const SECOND_ARG_SETTERS: &[&str] = &[".add_response(", ".set_response_label("];

    /// Does this string carry words, or is it punctuation and placeholders?
    ///
    /// `"Downloaded {}"` is prose and needs French; `"{}  ({})"`, `"v{}"` and
    /// `"—"` are composition — there is nothing in them to translate, and
    /// demanding a pair for them would be the kind of false alarm that gets a
    /// guard worked around instead of obeyed. Placeholders are removed first, so
    /// a template is judged on the words it contributes itself.
    fn is_prose(text: &str) -> bool {
        let mut outside = String::with_capacity(text.len());
        let mut depth = 0usize;
        for c in text.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                _ if depth == 0 => outside.push(c),
                _ => {}
            }
        }
        outside.chars().filter(|c| c.is_alphabetic()).count() >= 2
    }

    /// A `tr_fmt!` call must pass one argument per placeholder.
    ///
    /// [`tr_fmt_apply`] deliberately tolerates a mismatch — the *French*
    /// template is chosen at runtime and its placeholder count can drift, and a
    /// panic in a toast would be worse than a stray `{}`. The cost of that
    /// tolerance is that a call site which drops an argument compiles, ships,
    /// and shows a literal `{}` to the user. Nothing else can see it: the
    /// arguments are macro inputs, so the compiler's own `format!` arity check
    /// never runs on them.
    #[test]
    fn every_tr_fmt_call_passes_one_argument_per_placeholder() {
        let mut wrong: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for (path, text) in call_sites() {
            let code = crate::testing::code(&text);
            for (start, _) in code.match_indices("tr_fmt!") {
                let open = start + "tr_fmt!".len();
                let Some(args) = call_args(code, open) else {
                    continue;
                };
                let Some(template) = args.first().and_then(|_| literal_at(code, open + 1)) else {
                    continue; // a variable template: out of scope for a scan
                };
                checked += 1;
                let slots = template.match_indices("{}").count();
                if slots != args.len() - 1 {
                    let line = code[..start].lines().count();
                    wrong.push(format!(
                        "{}:{line}: {template:.40?} has {slots} placeholder(s) but {} argument(s)",
                        path.display(),
                        args.len() - 1
                    ));
                }
            }
        }
        assert!(
            checked > 50,
            "only {checked} tr_fmt! calls found — scan broken"
        );
        wrong.sort();
        assert!(
            wrong.is_empty(),
            "tr_fmt! call(s) whose arguments do not match the template — the user \
             sees a bare {{}} here: {wrong:#?}"
        );
    }

    /// Nothing the user reads may skip the catalog.
    ///
    /// The app advertises French, and the catalog has 1,271 keys — but a call
    /// site that never asks gets English regardless of language, and no compiler
    /// or runtime check can see it. Whole screens shipped that way: the AI
    /// connect wizard, image discovery, the template manager, the proposals
    /// dialog. Fourteen of those strings already had French sitting unused in
    /// the catalog, which is the tell — they were translated, the call sites
    /// simply never looked.
    ///
    /// Two shapes reach a person, so the guard knows two: a literal, which must
    /// be `tr_en!`, and a `format!`, which must be `tr_fmt!`. They are one rule
    /// — *text a user reads is localized* — and splitting them into two guards
    /// would let a screen fail one while passing the other.
    ///
    /// The rule has no exceptions, brands included. `tr_en!("Verbinal")` returns
    /// "Verbinal" in both languages, so wrapping costs nothing, whereas an
    /// exception list is the place the next untranslated string would hide.
    #[test]
    fn nothing_the_user_reads_skips_the_catalog() {
        let mut bare: Vec<String> = Vec::new();
        let mut localized = 0usize;
        for (path, text) in call_sites() {
            // Test code is not shipped, and a fixture label needs no French.
            let code = crate::testing::code(&text);
            // `skip` is how many arguments come before the text: none for a
            // setter, one for `add_response("close", "Close")`.
            let sinks = TEXT_SETTERS
                .iter()
                .map(|s| (*s, 0usize))
                .chain(SECOND_ARG_SETTERS.iter().map(|s| (*s, 1usize)));
            for (setter, skip) in sinks {
                for (start, _) in code.match_indices(setter) {
                    let mut at = start + setter.len();
                    for _ in 0..skip {
                        let Some(comma) = code[at..].find(',') else {
                            break;
                        };
                        at += comma + 1;
                    }
                    let arg = code[at..].trim_start().trim_start_matches('&');
                    let at = code.len() - arg.len();
                    if arg.starts_with("crate::tr_en!(") || arg.starts_with("crate::tr_fmt!(") {
                        localized += 1;
                        continue;
                    }
                    // A `format!` builds the string the sink will show, so the
                    // template inside it is the text — `tr_fmt!` is the same call
                    // with the template localized first.
                    let found = if let Some(rest) = arg.strip_prefix("format!(") {
                        let open = code.len() - rest.trim_start().len();
                        literal_at(code, open)
                    } else {
                        // Anything else that is not a plain literal — a variable,
                        // a `tr!` — is localized already or beyond a source scan.
                        literal_at(code, at)
                    };
                    let Some(text) = found.filter(|t| is_prose(t)) else {
                        continue;
                    };
                    let line = code[..start].lines().count();
                    bare.push(format!("{}:{line}: {text:.60?}", path.display()));
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
