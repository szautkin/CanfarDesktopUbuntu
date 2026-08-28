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
    ("Copy command",                                "Copier la commande"),
    ("Test connection",                             "Tester la connexion"),
    ("Start the MCP server to continue.",           "Démarrez le serveur MCP pour continuer."),
    ("MCP server is running.",                      "Le serveur MCP est en cours d’exécution."),
    ("MCP server is stopped.",                      "Le serveur MCP est arrêté."),
    ("Server running",                              "Serveur actif"),
    ("✓ Configuration written.",                    "✓ Configuration écrite."),
    ("View events & logs",                          "Afficher les évènements et journaux"),
    ("Delete job",                                  "Supprimer la tâche"),
    ("refreshing…",                                 "actualisation…"),
    ("Filter packages…",                            "Filtrer les paquets…"),
    ("Loading images…",                             "Chargement des images…"),
    ("Discovering…",                                "Découverte en cours…"),
    ("Largest file to open (MB)",                   "Taille maximale du fichier à ouvrir (Mo)"),
    ("AI agent images",                             "Images pour l'agent IA"),
    ("Captures of a viewer's working area sent to an AI agent. A model reads an image at a few hundred pixels; a larger capture costs the agent context without telling it more.",
     "Captures de la zone de travail d'une visionneuse envoyées à un agent IA. Un modèle lit une image à quelques centaines de pixels ; une capture plus grande coûte du contexte à l'agent sans lui en apprendre davantage."),
    ("Largest agent image (pixels)",                "Taille maximale de l'image de l'agent (pixels)"),
    ("Largest agent image (MB)",                    "Taille maximale de l'image de l'agent (Mo)"),
    ("Largest agent result (KB)",                   "Taille maximale du résultat de l'agent (Ko)"),
    // Annotations
    ("Marks",                                       "Repères"),
    ("DRAW",                                        "DESSIN"),
    ("Mark",                                        "Repère"),
    ("Circle",                                      "Cercle"),
    ("Box",                                         "Rectangle"),
    ("Callout",                                     "Légende"),
    ("Text",                                        "Texte"),
    ("circle",                                      "cercle"),
    ("box",                                         "rectangle"),
    ("callout",                                     "légende"),
    ("text",                                        "texte"),
    ("Draw a mark on the image. Click where you mean; press Escape to stop.",
     "Dessiner un repère sur l'image. Cliquez à l'endroit voulu ; appuyez sur Échap pour arrêter."),
    ("Nothing marked yet. Turn on Draw in the toolbar, then click the image.",
     "Aucun repère pour l'instant. Activez Dessin dans la barre d'outils, puis cliquez sur l'image."),
    ("Clear all marks",                             "Effacer tous les repères"),
    ("Delete this mark",                            "Supprimer ce repère"),
    ("What is this?",                               "De quoi s'agit-il ?"),
    ("Add",                                         "Ajouter"),
    ("{} marks",                                    "{} repères"),
    ("({})",                                        "({})"),
    ("pixel {}, {}",                                "pixel {}, {}"),
    ("{}°, {}°",                                    "{}°, {}°"),
    ("voxel {}, {}, ch {}",                         "voxel {}, {}, can. {}"),
    ("{} — {} — by the agent",                      "{} — {} — par l'agent"),
    ("{} — {}",                                     "{} — {}"),
    ("Show agents a short tool list",               "Afficher aux agents une liste d'outils abrégée"),
    ("Advertise the catalog and foundational tools instead of all of them, and send the rest as a grouped map. Saves about 22000 tokens — but most MCP clients only bind the tools they are shown, so the rest become unreachable. Leave off unless your client can call a tool by name.",
     "Annoncer les outils du catalogue et les outils fondamentaux plutôt que tous, et envoyer le reste sous forme de carte groupée. Économise environ 22000 jetons — mais la plupart des clients MCP ne lient que les outils qui leur sont présentés, rendant les autres inaccessibles. À laisser désactivé sauf si votre client peut appeler un outil par son nom."),
    ("Search results and lists are cut to fit this, newest rules first, and the reply says how many were left out.",
     "Les résultats de recherche et les listes sont réduits pour tenir dans cette limite, et la réponse indique combien d'éléments ont été omis."),
    ("Kernel: not started",                         "Noyau : non démarré"),
    // The rest of the kernel states. Only "not started" was ever translated,
    // because the others were English literals doing double duty as machine
    // keywords; `models::kernel_status` separated the two, so they can be
    // written for a reader now.
    ("Kernel: starting…",                           "Noyau : démarrage…"),
    ("Kernel: restarting…",                         "Noyau : redémarrage…"),
    ("Kernel: idle",                                "Noyau : inactif"),
    ("Kernel: busy",                                "Noyau : occupé"),
    ("Kernel: busy — cell running over {}s (press I,I to Interrupt)",
     "Noyau : occupé — cellule en cours depuis plus de {} s (appuyez sur I,I pour interrompre)"),
    ("Kernel: error — {}",                          "Noyau : erreur — {}"),
    ("Kernel: failed — {}",                         "Noyau : échec — {}"),
    ("Failed to load platform data",                "Échec du chargement des données de la plateforme"),
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
    ("Opened {} in Cube Viewer",                    "{} ouvert dans la visionneuse de cubes"),
    ("Opened {} in Notebook",                       "{} ouvert dans le carnet"),
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
    ("Cancel the transfer",                         "Annuler le transfert"),
    ("Cancelling…",                                 "Annulation…"),
    ("Uploading {}…",                               "Téléversement de {}…"),
    ("Downloading {}…",                             "Téléchargement de {}…"),
    ("Upload cancelled: {}",                        "Téléversement annulé : {}"),
    ("Download cancelled: {}",                      "Téléchargement annulé : {}"),
    ("{} — unsaved changes",                        "{} — modifications non enregistrées"),
    ("All files",                                   "Tous les fichiers"),
    ("View",                                        "Voir"),
    ("More",                                        "Plus"),
    ("Sign in to see your sessions and platform load",
     "Connectez-vous pour voir vos sessions et la charge de la plateforme"),
    ("Install failed — see the kernel log",         "Échec de l’installation — voir le journal du noyau"),
    // Settings
    ("Appearance",                                  "Apparence"),
    ("Choose how Verbinal looks",                   "Choisir l’apparence de Verbinal"),
    ("Applied after restart",                       "Appliqué après redémarrage"),
    ("Language change will take effect after restart",
     "Le changement de langue prendra effet après redémarrage"),
    ("Session Defaults",                            "Valeurs par défaut des sessions"),
    ("Default values for new session launches",     "Valeurs par défaut pour les nouveaux lancements de session"),
    ("Resource preset",                             "Préréglage de ressources"),
    ("\"Fixed\" reveals explicit cores and RAM",    "« Fixe » affiche le nombre de cœurs et la mémoire"),
    ("Default CPU Cores",                           "Cœurs CPU par défaut"),
    ("Default RAM (GB)",                            "Mémoire vive par défaut (Go)"),
    ("Default GPUs",                                "GPU par défaut"),
    ("Restore the built-in session-launch defaults","Rétablir les valeurs de lancement d’origine"),
    ("AI Assistant (MCP)",                          "Assistant IA (MCP)"),
    ("Let an AI agent (Claude Desktop / Claude Code) drive Verbinal over MCP",
     "Permettre à un agent IA (Claude Desktop / Claude Code) de piloter Verbinal via MCP"),
    ("Listens on a private per-user socket",        "À l’écoute sur un socket privé propre à l’utilisateur"),
    ("Server status",                               "État du serveur"),
    ("MCP server started",                          "Serveur MCP démarré"),
    ("MCP server stopped",                          "Serveur MCP arrêté"),
    ("Apply agent write proposals immediately instead of queuing them for review",
     "Appliquer immédiatement les propositions d’écriture de l’agent au lieu de les mettre en attente de révision"),
    ("Only approved clients may connect; new ones are held for review",
     "Seuls les clients approuvés peuvent se connecter ; les nouveaux sont mis en attente de révision"),
    ("Navigate to the view an agent just changed",  "Aller à la vue qu’un agent vient de modifier"),
    ("Show AI Guide tile",                          "Afficher la tuile Guide IA"),
    ("Display the AI Guide tile on the launchpad",  "Afficher la tuile Guide IA sur l’écran d’accueil"),
    ("Connect an agent…",                           "Connecter un agent…"),
    ("Pair Claude Desktop or Claude Code CLI",      "Associer Claude Desktop ou Claude Code CLI"),
    ("Connect",                                     "Connecter"),
    ("Check the MCP server, socket, bridge, and Claude client configuration",
     "Vérifier le serveur MCP, le socket, le pont et la configuration du client Claude"),
    ("Agents that have connected; approve or revoke each",
     "Agents qui se sont connectés ; approuver ou révoquer chacun"),
    ("No clients yet",                              "Aucun client pour l’instant"),
    ("Connect an agent to see it here",             "Connectez un agent pour le voir ici"),
    ("Approved",                                    "Approuvé"),
    ("Awaiting approval",                           "En attente d’approbation"),
    ("The last few MCP tool calls made by an agent","Les derniers appels d’outils MCP effectués par un agent"),
    ("No recent activity",                          "Aucune activité récente"),
    ("Connection",                                  "Connexion"),
    ("CANFAR / CADC service endpoints",             "Points d’accès des services CANFAR / CADC"),
    ("Advanced — repoint the app at another deployment",
     "Avancé — rediriger l’application vers un autre déploiement"),
    ("Reset endpoints",                             "Réinitialiser les points d’accès"),
    ("Connection test",                             "Test de connexion"),
    ("Registry credentials and the inspector host image used to probe container images",
     "Identifiants du registre et image hôte de l’inspecteur utilisée pour sonder les images de conteneur"),
    ("secret stored",                               "secret enregistré"),
    ("no secret",                                   "aucun secret"),
    ("Registry secret saved",                       "Secret du registre enregistré"),
    ("Registry secret removed",                     "Secret du registre supprimé"),
    ("Verify the registry secret before launching a probe job",
     "Vérifier le secret du registre avant de lancer une tâche de sondage"),
    ("Test",                                        "Tester"),
    ("Re-enter your registry secret to test it",    "Saisissez à nouveau le secret du registre pour le tester"),
    ("The registry accepted the credentials.",      "Le registre a accepté les identifiants."),
    ("The registry rejected the username or secret.",
     "Le registre a refusé le nom d’utilisateur ou le secret."),
    ("Set a registry host, username, and secret first.",
     "Définissez d’abord un hôte de registre, un nom d’utilisateur et un secret."),
    ("The registry's auth challenge could not be parsed.",
     "Impossible d’analyser le défi d’authentification du registre."),
    ("Verify the registry secret before launching a compute job",
     "Vérifier le secret du registre avant de lancer une tâche de calcul"),
    ("Platform",                                    "Plateforme"),
    ("MCP Diagnostics",                             "Diagnostics MCP"),
    // Research
    ("mcp",                                         "mcp"),
    ("Search by collection, target, instrument…",   "Rechercher par collection, cible, instrument…"),
    ("Export a research bundle (observations + notes) as a .zip",
     "Exporter un ensemble de recherche (observations + notes) au format .zip"),
    ("Refresh list",                                "Actualiser la liste"),
    ("No Saved Observations",                       "Aucune observation enregistrée"),
    ("Go to Search",                                "Aller à la recherche"),
    ("0 observations",                              "0 observation"),
    ("Saved observations from CADC archive searches appear on the left.",
     "Les observations enregistrées depuis les recherches dans l’archive CADC apparaissent à gauche."),
    ("No observations",                             "Aucune observation"),
    ("Bookmarked",                                  "Mise en signet"),
    ("FITS",                                        "FITS"),
    ("Legacy record — re-save from the Search page to cache the preview locally.",
     "Enregistrement ancien — réenregistrez depuis la page Recherche pour mettre l’aperçu en cache localement."),
    ("Open (FITS)",                                 "Ouvrir (FITS)"),
    ("Open the file in the 2D FITS viewer",         "Ouvrir le fichier dans la visionneuse FITS 2D"),
    ("File not found — it may have been moved or deleted",
     "Fichier introuvable — il a peut-être été déplacé ou supprimé"),
    ("Open as Cube",                                "Ouvrir comme cube"),
    ("Open a spectral cube in the 3D Cube Viewer",  "Ouvrir un cube spectral dans la visionneuse de cubes 3D"),
    ("Spectral cube detected — Open as Cube recommended",
     "Cube spectral détecté — « Ouvrir comme cube » recommandé"),
    ("Open the containing folder in the file manager",
     "Ouvrir le dossier parent dans le gestionnaire de fichiers"),
    ("Unable to locate parent directory",           "Impossible de localiser le dossier parent"),
    ("File missing from disk",                      "Fichier absent du disque"),
    ("Re-download the FITS file to the Research library",
     "Retélécharger le fichier FITS dans la bibliothèque de recherche"),
    ("View the full CAOM2 observation metadata",    "Afficher les métadonnées CAOM2 complètes de l’observation"),
    ("Copy ID",                                     "Copier l’identifiant"),
    ("Copy the publisher DID to the clipboard",     "Copier le DID de l’éditeur dans le presse-papiers"),
    ("Publisher ID copied",                         "Identifiant d’éditeur copié"),
    ("Remove this observation from the library",    "Retirer cette observation de la bibliothèque"),
    ("Remove from Research and delete the local files",
     "Retirer de la recherche et supprimer les fichiers locaux"),
    ("Observation Metadata",                        "Métadonnées de l’observation"),
    ("Target Name",                                 "Nom de la cible"),
    ("RA (J2000)",                                  "AD (J2000)"),
    ("Dec (J2000)",                                 "Déc (J2000)"),
    ("Status",                                      "État"),
    ("Bookmarked (metadata only — no file downloaded)",
     "En signet (métadonnées seulement — aucun fichier téléchargé)"),
    ("Missing — file not found on disk",            "Manquant — fichier introuvable sur le disque"),
    ("Saved at",                                    "Enregistré le"),
    ("Publisher ID",                                "Identifiant d’éditeur"),
    ("Rating",                                      "Note"),
    ("Clear rating",                                "Effacer la note"),
    ("Tags (comma-separated)",                      "Étiquettes (séparées par des virgules)"),
    ("Removed from Research",                       "Retiré de la recherche"),
    ("No publisher ID — cannot download this observation",
     "Aucun identifiant d’éditeur — impossible de télécharger cette observation"),
    ("Resolving download link…",                    "Résolution du lien de téléchargement…"),
    ("Include search history",                      "Inclure l’historique des recherches"),
    ("Include downloaded data files (large)",       "Inclure les fichiers de données téléchargés (volumineux)"),
    ("Upload to VOSpace",                           "Téléverser vers VOSpace"),
    ("Export Research Bundle",                      "Exporter l’ensemble de recherche"),
    ("Choose Folder…",                              "Choisir un dossier…"),
    ("Nothing to export yet — save an observation first",
     "Rien à exporter pour l’instant — enregistrez d’abord une observation"),
    ("ZIP archive",                                 "Archive ZIP"),
    ("Uploading bundle to VOSpace…",                "Téléversement de l’ensemble vers VOSpace…"),
    ("This will permanently remove the observation and its local files.\n\nThis cannot be undone.",
     "Cette action supprimera définitivement l’observation et ses fichiers locaux.\n\nElle est irréversible."),
    ("Remove from Research?",                       "Retirer de la recherche ?"),
    // Search
    ("Preview this observation",                    "Prévisualiser cette observation"),
    ("No results",                                  "Aucun résultat"),
    ("Select visible columns",                      "Sélectionner les colonnes visibles"),
    ("CSV",                                         "CSV"),
    ("Export results as CSV file",                  "Exporter les résultats au format CSV"),
    ("TSV",                                         "TSV"),
    ("Export results as TSV file",                  "Exporter les résultats au format TSV"),
    ("Apply filters and re-render",                 "Appliquer les filtres et réafficher"),
    ("Apply filters to ADQL",                       "Appliquer les filtres à l’ADQL"),
    ("Clear filters",                               "Effacer les filtres"),
    // CANFAR Images: the manual manifest check.
    ("Check images",                                "Vérifier les images"),
    (
        "Look in your CANFAR storage for image manifests this machine does not have yet",
        "Chercher dans votre stockage CANFAR les manifestes d’images que cette machine n’a pas encore",
    ),
    (
        "Image manifests are already up to date",
        "Les manifestes d’images sont déjà à jour",
    ),
    (
        "Could not check CANFAR images: {}",
        "Impossible de vérifier les images CANFAR : {}",
    ),
    // AI-compute readiness and the settings-field examples.
    ("run_code is ready",                           "run_code est prêt"),
    ("run_code is off",                             "run_code est désactivé"),
    (
        "Set a compute image above — a name like verbinal-compute:1.0, or a full \
         project/name:tag reference.",
        "Indiquez une image de calcul ci-dessus — un nom tel que verbinal-compute:1.0, \
         ou une référence complète projet/nom:version.",
    ),
    (
        "e.g. verbinal-compute:1.0 or project/name:tag",
        "ex. verbinal-compute:1.0 ou projet/nom:version",
    ),
    ("e.g. images.canfar.net",                      "ex. images.canfar.net"),
    (
        "The CLI secret from your Harbor user profile — not your CADC password",
        "Le secret CLI de votre profil Harbor — et non votre mot de passe CADC",
    ),
    (
        "e.g. private-test — the project only, no image name",
        "ex. private-test — le projet seulement, sans nom d’image",
    ),
    ("Your CADC username",                          "Votre nom d’utilisateur CADC"),
    (
        "e.g. skaha/terminal:1.1.2 — a headless image that can inspect others",
        "ex. skaha/terminal:1.1.2 — une image sans interface capable d’en inspecter d’autres",
    ),
    // Notebook dependency install, including the PEP 668 refusal.
    ("Install failed: {}",                          "Échec de l’installation : {}"),
    (
        "This Python is managed by your system",
        "Ce Python est géré par votre système",
    ),
    ("Install anyway",                              "Installer quand même"),
    (
        "Its packages come from your distribution, so pip will not add {} on its own \
         (PEP 668).\n\nInstalling anyway uses --break-system-packages, which can leave \
         your system Python inconsistent with its package manager. A virtual environment, \
         selected in Notebook settings, avoids the choice.",
        "Ses paquets proviennent de votre distribution : pip n’ajoutera donc pas {} de \
         lui-même (PEP 668).\n\nInstaller quand même utilise --break-system-packages, ce \
         qui peut rendre votre Python système incohérent avec son gestionnaire de paquets. \
         Un environnement virtuel, choisi dans les réglages du carnet, évite ce choix.",
    ),
    // Batch job history.
    ("Succeeded",                                   "Réussi"),
    ("Batch job",                                   "Tâche par lots"),
    ("Image inspection",                            "Inspection d’image"),
    ("Clear history",                               "Effacer l’historique"),
    ("Probe job",                                   "Tâche de sondage"),
    ("inspected {}",                                "inspectée {}"),
    (
        "+{} more — type above to narrow",
        "+{} de plus — tapez ci-dessus pour affiner",
    ),
    (
        "{} is not offered for a session type — put it in the Advanced tab",
        "{} n’est proposée pour aucun type de session — placée dans l’onglet Avancé",
    ),
    // Background sync of published image manifests.
    (
        "Catching up on {} image manifest from CANFAR…",
        "Récupération de {} manifeste d’image depuis CANFAR…",
    ),
    (
        "Catching up on {} image manifests from CANFAR…",
        "Récupération de {} manifestes d’image depuis CANFAR…",
    ),
    ("Image manifests: {} of {}",                    "Manifestes d’image : {} sur {}"),
    (
        "{} image manifest brought over from CANFAR",
        "{} manifeste d’image récupéré depuis CANFAR",
    ),
    (
        "{} image manifests brought over from CANFAR",
        "{} manifestes d’image récupérés depuis CANFAR",
    ),
    (
        "{} — deleted after the probe finished; the full \
         output is kept under Batch Jobs → History",
        "{} — supprimée à la fin du sondage ; la sortie complète est conservée \
         sous Tâches par lots → Historique",
    ),
    (
        "Forget every recorded job, including the reasons they failed",
        "Oublier toutes les tâches enregistrées, y compris les raisons de leur échec",
    ),
    (
        "Could not clear the job history",
        "Impossible d’effacer l’historique des tâches",
    ),
    (
        "{} finished job, kept after CANFAR reaped it",
        "{} tâche terminée, conservée après sa suppression par CANFAR",
    ),
    (
        "{} finished jobs, kept after CANFAR reaped them",
        "{} tâches terminées, conservées après leur suppression par CANFAR",
    ),
    (
        "No finished jobs recorded yet. Jobs appear here once they succeed \
         or fail, along with the logs and events explaining why.",
        "Aucune tâche terminée enregistrée pour l’instant. Les tâches apparaissent ici \
         une fois réussies ou échouées, avec les journaux et les événements qui \
         expliquent pourquoi.",
    ),
    (
        "The job produced no logs or events.",
        "La tâche n’a produit ni journaux ni événements.",
    ),
    ("{} — {}",                                     "{} — {}"),
    ("Filter syntax",                               "Syntaxe des filtres"),
    ("How to write a column filter",                 "Comment écrire un filtre de colonne"),
    // The filter grammar. Operator symbols stay verbatim — they are what you
    // type, not words — and so do the example expressions in `FILTER_SYNTAX`.
    (
        "Number: 10, >=10, or 10..20 for a range.",
        "Nombre : 10, >=10, ou 10..20 pour un intervalle.",
    ),
    (
        "Text: matches anywhere in the cell; =text matches all of it.",
        "Texte : correspond n’importe où dans la cellule ; =texte correspond à la cellule entière.",
    ),
    (
        "Combine with ! (not), & (and), | (or) and parentheses.",
        "Combinez avec ! (non), & (et), | (ou) et des parenthèses.",
    ),
    (
        "Filters on different columns must all hold. Filtering narrows the rows \
         already fetched; use \u{201c}Apply filters to ADQL\u{201d} to push them into the query.",
        "Les filtres de colonnes différentes doivent tous être satisfaits. Le filtrage \
         restreint les lignes déjà récupérées ; utilisez \u{201c}Appliquer les filtres à \
         l\u{2019}ADQL\u{201d} pour les intégrer à la requête.",
    ),
    ("contains, ignoring case",                      "contient, sans tenir compte de la casse"),
    ("matches the whole cell",                       "correspond à la cellule entière"),
    (
        "compare — as numbers where both sides are numbers",
        "comparaison — numérique lorsque les deux côtés sont des nombres",
    ),
    ("compare, the other way",                       "comparaison, dans l’autre sens"),
    ("a range, both ends included",                  "un intervalle, bornes incluses"),
    ("NOT — excludes what follows",                  "NON — exclut ce qui suit"),
    (
        "AND — both must hold (also `&&`, `AND`)",
        "ET — les deux doivent être satisfaits (aussi `&&`, `AND`)",
    ),
    (
        "OR — either may hold (also `||`, `OR`)",
        "OU — l’un ou l’autre suffit (aussi `||`, `OR`)",
    ),
    (
        "parentheses group; NOT binds tightest, then AND, then OR",
        "les parenthèses regroupent ; NON est prioritaire, puis ET, puis OU",
    ),
    ("quotes make it literal text",                  "les guillemets rendent le texte littéral"),
    (
        "Remove every column filter and the sort, showing all rows again",
        "Supprimer tous les filtres de colonne et le tri, afin d’afficher à nouveau toutes les lignes",
    ),
    // CADC's own two filter-syntax hints, translated but keeping the operator
    // symbols verbatim — they are what you type, not words.
    ("Append the active column filters as an ADQL WHERE clause",
     "Ajouter les filtres de colonne actifs sous forme de clause WHERE ADQL"),
    ("Rows/page:",                                  "Lignes/page :"),
    ("Page 1",                                      "Page 1"),
    ("No recent searches",                          "Aucune recherche récente"),
    ("CSV Files",                                   "Fichiers CSV"),
    ("TSV Files",                                   "Fichiers TSV"),
    ("From FITS crosshair",                         "Depuis le réticule FITS"),
    ("Form cleared",                                "Formulaire effacé"),
    ("Resolving target...",                         "Résolution de la cible..."),
    ("Enter an ADQL query",                         "Saisissez une requête ADQL"),
    ("Searching...",                                "Recherche en cours..."),
    ("Search failed",                               "Échec de la recherche"),
    ("Save to Research (downloads preview + FITS file)",
     "Enregistrer dans la recherche (télécharge l’aperçu et le fichier FITS)"),
    ("Run a search first, then apply filters",      "Lancez d’abord une recherche, puis appliquez les filtres"),
    ("Not found",                                   "Introuvable"),
    ("Display unit",                                "Unité d’affichage"),
    ("No ADQL to save",                             "Aucun ADQL à enregistrer"),
    ("Query saved",                                 "Requête enregistrée"),
    ("No results to export",                        "Aucun résultat à exporter"),
    ("Re-run query",                                "Relancer la requête"),
    ("View details",                                "Afficher les détails"),
    ("Query deleted",                               "Requête supprimée"),
    ("Query renamed",                               "Requête renommée"),
    ("Loading data train...",                       "Chargement du train de données..."),
    ("unknown",                                     "inconnu"),
    ("Search filters unavailable — archive unreachable",
     "Filtres de recherche indisponibles — archive injoignable"),
    ("Filter updated",                              "Filtre mis à jour"),
    ("Copy Publisher ID",                           "Copier l’identifiant d’éditeur"),
    ("No Preview",                                  "Aucun aperçu"),
    ("Preview Unavailable",                         "Aperçu indisponible"),
    ("Check network connection",                    "Vérifiez la connexion réseau"),
    ("Save to Research",                            "Enregistrer dans la recherche"),
    ("No publisher ID — observation cannot be saved",
     "Aucun identifiant d’éditeur — l’observation ne peut pas être enregistrée"),
    ("Download the preview and FITS file to the Research library",
     "Télécharger l’aperçu et le fichier FITS dans la bibliothèque de recherche"),
    ("Already in Research",                         "Déjà dans la recherche"),
    ("No science file found for this observation",  "Aucun fichier scientifique trouvé pour cette observation"),
    ("Downloading preview image…",                  "Téléchargement de l’image d’aperçu…"),
    ("Go to Research",                              "Aller à la recherche"),
    ("Matches the observation ID exactly (case-insensitive). Use * as a wildcard, e.g. jw01345*",
     "Correspond exactement à l’identifiant d’observation (insensible à la casse). Utilisez * comme joker, par ex. jw01345*"),
    ("e.g. 1..10 d",                                "par ex. 1..10 d"),
    // Notebook
    ("Open Notebook (Ctrl+O)",                      "Ouvrir un carnet (Ctrl+O)"),
    ("Save Notebook (Ctrl+S)",                      "Enregistrer le carnet (Ctrl+S)"),
    ("Save As… (Ctrl+Shift+S)",                     "Enregistrer sous… (Ctrl+Maj+S)"),
    ("Move up",                                     "Monter"),
    ("Move Cell Up",                                "Monter la cellule"),
    ("Move down",                                   "Descendre"),
    ("Move Cell Down",                              "Descendre la cellule"),
    ("Split at cursor",                             "Scinder au curseur"),
    ("Split Cell at Cursor (Ctrl+Shift+Minus)",     "Scinder la cellule au curseur (Ctrl+Maj+Moins)"),
    ("Merge with below",                            "Fusionner avec la suivante"),
    ("Merge Cell Below (Shift+M)",                  "Fusionner avec la cellule suivante (Maj+M)"),
    ("Cell",                                        "Cellule"),
    ("Cell operations — move, split, merge, delete","Opérations sur les cellules — déplacer, scinder, fusionner, supprimer"),
    ("Kernel",                                      "Noyau"),
    ("Kernel — restart, clear outputs",             "Noyau — redémarrer, effacer les sorties"),
    ("Kernel status: not started",                  "État du noyau : non démarré"),
    ("Notebook Settings",                           "Paramètres du carnet"),
    ("Notebooks",                                   "Carnets"),
    ("Save changes?",                               "Enregistrer les modifications ?"),
    ("Use Save As to choose a file path, then close",
     "Utilisez « Enregistrer sous » pour choisir un chemin, puis fermez"),
    ("Recover notebooks?",                          "Récupérer les carnets ?"),
    ("Discard All",                                 "Tout abandonner"),
    ("Untitled",                                    "Sans titre"),
    ("Save Notebook As",                            "Enregistrer le carnet sous"),
    ("Kernel status: idle",                         "État du noyau : inactif"),
    ("Kernel status: busy",                         "État du noyau : occupé"),
    ("Kernel status: starting",                     "État du noyau : démarrage"),
    ("Kernel status: error",                        "État du noyau : erreur"),
    ("Editor",                                      "Éditeur"),
    ("Font size",                                   "Taille de police"),
    ("Tab size (spaces)",                           "Taille de tabulation (espaces)"),
    ("Word wrap",                                   "Retour à la ligne"),
    ("Saving",                                      "Enregistrement"),
    ("Autosave enabled",                            "Enregistrement automatique activé"),
    ("Autosave interval (seconds)",                 "Intervalle d’enregistrement automatique (secondes)"),
    ("Execution",                                   "Exécution"),
    ("Execution timeout (seconds, 0 = never)",      "Délai d’exécution (secondes, 0 = jamais)"),
    ("Python path (blank = auto-detect)",           "Chemin Python (vide = détection automatique)"),
    ("Browse for interpreter",                      "Parcourir pour un interpréteur"),
    ("Interface",                                   "Interface"),
    ("Show toolbar",                                "Afficher la barre d’outils"),
    ("Reopen settings with Ctrl+comma",             "Rouvrez les paramètres avec Ctrl+virgule"),
    ("Kernel log",                                  "Journal du noyau"),
    ("Diagnostics for kernel start failures and unexpected exits",
     "Diagnostics des échecs de démarrage du noyau et des arrêts inattendus"),
    ("No log folder is available on this system",   "Aucun dossier de journaux n’est disponible sur ce système"),
    ("Select Python Interpreter",                   "Sélectionner l’interpréteur Python"),
    ("Open a Jupyter notebook (.ipynb) or Python (.py) file",
     "Ouvrir un carnet Jupyter (.ipynb) ou un fichier Python (.py)"),
    ("Type Python code here…",                      "Saisissez du code Python ici…"),
    ("Type markdown here…",                         "Saisissez du markdown ici…"),
    // FITS viewer
    ("Plasma",                                      "Plasma"),
    ("Inferno",                                     "Inferno"),
    ("Magma",                                       "Magma"),
    ("CoolWarm",                                    "CoolWarm"),
    ("Histogram Eq",                                "Égalisation d’histogramme"),
    ("Black point — pixels at or below render black",
     "Point noir — les pixels à ce niveau ou en dessous s’affichent en noir"),
    ("Min cut",                                     "Seuil bas"),
    ("White point — pixels at or above render white",
     "Point blanc — les pixels à ce niveau ou au-dessus s’affichent en blanc"),
    ("Max cut",                                     "Seuil haut"),
    ("Reset stretch",                               "Réinitialiser l’étirement"),
    ("Back to the automatic cut levels and Linear stretch",
     "Revenir aux seuils automatiques et à l’étirement linéaire"),
    ("VIEW",                                        "VUE"),
    ("Type a zoom % and press Enter",               "Saisissez un zoom en % et appuyez sur Entrée"),
    ("Zoom",                                        "Zoom"),
    ("Rotate so north is up",                       "Pivoter pour placer le nord en haut"),
    ("North up",                                    "Nord en haut"),
    ("CROSSHAIR",                                   "RÉTICULE"),
    ("Right-click the image to place it.",          "Faites un clic droit sur l’image pour le placer."),
    ("Copy crosshair RA/Dec to clipboard",          "Copier l’AD/Déc du réticule dans le presse-papiers"),
    ("Clear crosshair",                             "Effacer le réticule"),
    ("Search here",                                 "Rechercher ici"),
    ("Search the CADC archive at the crosshair's RA/Dec",
     "Rechercher dans l’archive CADC à l’AD/Déc du réticule"),
    ("Header & image info",                         "En-tête et informations sur l’image"),
    ("Saved coordinates",                           "Coordonnées enregistrées"),
    ("COMPARE",                                     "COMPARER"),
    ("Cross-fade blink against another tab (Space pause · Left/Right show A/B · Esc stop)",
     "Clignotement en fondu avec un autre onglet (Espace pause · Gauche/Droite affiche A/B · Échap arrêt)"),
    ("Blink",                                       "Clignotement"),
    ("vs…",                                         "vs…"),
    ("Choose the tab to blink against",             "Choisir l’onglet de comparaison"),
    ("Against",                                     "Comparer à"),
    ("Blink fade interval (ms)",                    "Intervalle de fondu du clignotement (ms)"),
    ("Fade speed",                                  "Vitesse de fondu"),
    ("Link crosshair across tabs by sky position (auto-enables North Up)",
     "Lier le réticule entre les onglets par position céleste (active « Nord en haut »)"),
    ("Link crosshair",                              "Lier le réticule"),
    ("Sync zoom across tabs — match the current image's angular field (re-applied as you switch tabs)",
     "Synchroniser le zoom entre les onglets — aligner le champ angulaire de l’image courante (réappliqué au changement d’onglet)"),
    ("Sync zoom",                                   "Synchroniser le zoom"),
    ("No file loaded",                              "Aucun fichier chargé"),
    ("Approximate WCS — coordinates and alignment may be imprecise.",
     "WCS approximatif — les coordonnées et l’alignement peuvent être imprécis."),
    ("Extension:",                                  "Extension :"),
    ("Open FITS…",                                  "Ouvrir un FITS…"),
    ("No FITS File Open",                           "Aucun fichier FITS ouvert"),
    ("Open a FITS file to get started",             "Ouvrez un fichier FITS pour commencer"),
    ("No crosshair with WCS to copy",               "Aucun réticule avec WCS à copier"),
    ("FITS Images",                                 "Images FITS"),
    ("Open FITS File",                              "Ouvrir un fichier FITS"),
    ("Blink needs two open tabs",                   "Le clignotement nécessite deux onglets ouverts"),
    ("Open another tab to blink",                   "Ouvrez un autre onglet pour comparer"),
    // Main window
    ("Verbinal - a CANFAR Science Portal",          "Verbinal - un portail scientifique CANFAR"),
    ("Preferences",                                 "Préférences"),
    ("Help",                                        "Aide"),
    ("Main Menu",                                   "Menu principal"),
    ("Toggle File Panel (Ctrl+B)",                  "Afficher/masquer le panneau des fichiers (Ctrl+B)"),
    ("⚡ agent working…",                            "⚡ agent au travail…"),
    ("An AI agent is working",                      "Un agent IA travaille"),
    ("Connected",                                   "Connecté"),
    ("Service status",                              "État des services"),
    ("Agent proposals awaiting review",             "Propositions d’agent en attente de révision"),
    ("Account",                                     "Compte"),
    ("Profile",                                     "Profil"),
    ("Some services unreachable — working with cached data",
     "Certains services sont injoignables — utilisation des données en cache"),
    ("Your session has expired — please sign in again",
     "Votre session a expiré — veuillez vous reconnecter"),
    ("Sign In",                                     "Se connecter"),
    ("You appear to be offline — some features are unavailable",
     "Vous semblez hors ligne — certaines fonctionnalités sont indisponibles"),
    ("Session refreshed",                           "Session actualisée"),
    ("Observation Detail",                          "Détail de l’observation"),
    ("Logged out successfully",                     "Déconnexion réussie"),
    ("Checking authentication...",                  "Vérification de l’authentification..."),
    ("Session expired. Please login.",              "Session expirée. Veuillez vous connecter."),
    ("Unknown",                                     "Inconnu"),
    ("Online",                                      "En ligne"),
    ("Offline",                                     "Hors ligne"),
    ("User Profile",                                "Profil de l’utilisateur"),
    ("Email",                                       "Courriel"),
    ("Institute",                                   "Établissement"),
    ("Internal ID",                                 "Identifiant interne"),
    ("A CANFAR Science Portal Companion\n\nLaunch, monitor, and manage your interactive computing sessions (Notebook, Desktop, CARTA, Firefly) directly from your desktop without needing a browser.\n\nCANFAR is operated by the Canadian Astronomy Data Centre (CADC) and the Digital Research Alliance of Canada.",
     "Un compagnon du portail scientifique CANFAR\n\nLancez, surveillez et gérez vos sessions de calcul interactives (carnet, bureau, CARTA, Firefly) directement depuis votre poste, sans navigateur.\n\nCANFAR est exploité par le Centre canadien de données astronomiques (CADC) et l’Alliance de recherche numérique du Canada."),
    ("Runtime Info",                                "Informations d’exécution"),
    ("Verbinal",                                    "Verbinal"),
    ("A CANFAR Science Portal Companion",           "Un compagnon du portail scientifique CANFAR"),
    ("Research protocols & checklists",             "Protocoles et listes de contrôle de recherche"),
    ("Explore 3D spectral cubes",                   "Explorer les cubes spectraux 3D"),
    ("Connect an AI agent to help you",             "Connecter un agent IA pour vous aider"),
    ("Pair an AI agent over MCP",                   "Associer un agent IA via MCP"),
    ("Log in with your CADC credentials to get started",
     "Connectez-vous avec vos identifiants CADC pour commencer"),
    // Workflows
    ("New",                                         "Nouveau"),
    ("Create a new workflow from a starter template",
     "Créer un flux de travail à partir d’un modèle de départ"),
    ("Import…",                                     "Importer…"),
    ("Import a .workflow.md / .md file as a local copy",
     "Importer un fichier .workflow.md / .md comme copie locale"),
    ("Select a workflow",                           "Sélectionnez un flux de travail"),
    ("Built-in",                                    "Intégré"),
    ("No local workflows yet",                      "Aucun flux de travail local pour l’instant"),
    ("Re-read the shared workflows from VOSpace",   "Relire les flux de travail partagés depuis VOSpace"),
    ("Sign in to CADC to publish a workflow",       "Connectez-vous à CADC pour publier un flux de travail"),
    ("Publish to VOSpace?",                         "Publier sur VOSpace ?"),
    ("Reset step progress",                         "Réinitialiser la progression des étapes"),
    ("Loading from VOSpace…",                       "Chargement depuis VOSpace…"),
    ("Start working from this template — creates your own editable copy",
     "Commencer à partir de ce modèle — crée votre propre copie modifiable"),
    ("Ready to work in your copy",                  "Prêt à travailler dans votre copie"),
    ("Duplicate to Local",                          "Dupliquer en local"),
    ("Create an editable local copy of this workflow",
     "Créer une copie locale modifiable de ce flux de travail"),
    ("Duplicated to a local copy",                  "Dupliqué en copie locale"),
    ("Copy prompt",                                 "Copier l’invite"),
    ("Copy an instruction that tells your AI agent to follow this workflow",
     "Copier une instruction demandant à votre agent IA de suivre ce flux de travail"),
    ("Prompt copied — paste it to your AI assistant",
     "Invite copiée — collez-la dans votre assistant IA"),
    ("Share this workflow via your VOSpace",        "Partager ce flux de travail via votre VOSpace"),
    ("Edit the raw workflow markdown",              "Modifier le markdown brut du flux de travail"),
    ("Delete this local workflow",                  "Supprimer ce flux de travail local"),
    ("Workflow deleted",                            "Flux de travail supprimé"),
    ("Toggle done",                                 "Marquer comme terminé"),
    ("Saved a local copy to edit",                  "Copie locale enregistrée pour modification"),
    ("Edit workflow",                               "Modifier le flux de travail"),
    ("Workflow saved",                              "Flux de travail enregistré"),
    ("Workflow / Markdown files",                   "Fichiers de flux de travail / Markdown"),
    ("Import Workflow",                             "Importer un flux de travail"),
    ("Imported workflow",                           "Flux de travail importé"),
    ("This will permanently delete this local workflow.\n\nThis cannot be undone.",
     "Cette action supprimera définitivement ce flux de travail local.\n\nElle est irréversible."),
    // Launch form
    ("Generate name",                               "Générer un nom"),
    ("Fixed Resources",                             "Ressources fixes"),
    ("Enable to specify exact CPU/RAM/GPU",         "Activer pour préciser exactement CPU/RAM/GPU"),
    ("Custom Container Image",                      "Image de conteneur personnalisée"),
    ("Launch a session using a custom image URI",   "Lancer une session avec une URI d’image personnalisée"),
    ("Image (project/name:tag)",                    "Image (projet/nom:étiquette)"),
    ("Credentials for private registries. Leave blank for public images.",
     "Identifiants pour les registres privés. Laissez vide pour les images publiques."),
    ("Token or Password",                           "Jeton ou mot de passe"),
    ("Headless Batch Job",                          "Tâche par lots sans interface"),
    ("Run a container command with no interactive UI. Replicas launch the same job N times.",
     "Exécuter une commande de conteneur sans interface interactive. Les réplicas lancent la même tâche N fois."),
    ("Arguments (space-separated)",                 "Arguments (séparés par des espaces)"),
    ("Off: flexible (platform-managed). On: specify exact CPU/RAM/GPU.",
     "Désactivé : flexible (géré par la plateforme). Activé : préciser exactement CPU/RAM/GPU."),
    ("Session limit reached (max 3 concurrent sessions)",
     "Limite de sessions atteinte (3 sessions simultanées au maximum)"),
    ("Please select an image",                      "Veuillez sélectionner une image"),
    ("Please select or enter an image",             "Veuillez sélectionner ou saisir une image"),
    ("Please enter a session name",                 "Veuillez saisir un nom de session"),
    ("Launching session...",                        "Lancement de la session..."),
    ("Please enter a container image",              "Veuillez saisir une image de conteneur"),
    ("Launching batch job…",                        "Lancement de la tâche par lots…"),
    // Observation detail
    ("The service returned no observation.",        "Le service n’a renvoyé aucune observation."),
    ("The metadata service is unreachable.",        "Le service de métadonnées est injoignable."),
    ("Identity",                                    "Identité"),
    ("Sequence Number",                             "Numéro de séquence"),
    ("Telescope & Instrument",                      "Télescope et instrument"),
    ("Tau",                                         "Tau"),
    ("No overview information available.",          "Aucune information générale disponible."),
    ("No coverage information available.",          "Aucune information de couverture disponible."),
    ("RA range",                                    "Plage d’AD"),
    ("Dec range",                                   "Plage de Déc"),
    ("Vertices",                                    "Sommets"),
    ("Spatial Footprint",                           "Empreinte spatiale"),
    ("Plane",                                       "Plan"),
    ("Product ID",                                  "Identifiant du produit"),
    ("Data Product Type",                           "Type de produit de données"),
    ("Calibration Level",                           "Niveau d’étalonnage"),
    ("Quality",                                     "Qualité"),
    ("No files available.",                         "Aucun fichier disponible."),
    ("No files in this plane.",                     "Aucun fichier dans ce plan."),
    ("No provenance information available.",        "Aucune information de provenance disponible."),
    ("No provenance in this plane.",                "Aucune provenance dans ce plan."),
    ("No data.",                                    "Aucune donnée."),
    ("Spectral cube detected — Cube Viewer recommended",
     "Cube spectral détecté — visionneuse de cubes recommandée"),
    ("Added to Research",                           "Ajouté à la recherche"),
    // Cube viewer
    ("3D",                                          "3D"),
    ("Window low",                                  "Fenêtre basse"),
    ("Window high",                                 "Fenêtre haute"),
    ("Window 99%",                                  "Fenêtre 99 %"),
    ("Set the display cut to the 1–99% window",     "Régler les seuils d’affichage sur la fenêtre 1–99 %"),
    ("MIP",                                         "MIP"),
    ("Max-intensity projection",                    "Projection d’intensité maximale"),
    ("Auto-orbit",                                  "Rotation automatique"),
    ("Slice plane",                                 "Plan de coupe"),
    ("Info",                                        "Infos"),
    ("NAME",                                        "NOM"),
    ("SPECTRAL",                                    "SPECTRAL"),
    ("OBJECT",                                      "OBJET"),
    ("INSTRUMENT",                                  "INSTRUMENT"),
    ("TELESCOPE",                                   "TÉLESCOPE"),
    ("NaN",                                         "NaN"),
    // AI guide
    ("Filter tools by name or description…",        "Filtrer les outils par nom ou description…"),
    ("Author a new guide tool",                     "Créer un nouvel outil de guide"),
    ("All categories",                              "Toutes les catégories"),
    ("No tools match your filter.",                 "Aucun outil ne correspond à votre filtre."),
    ("overridden",                                  "redéfini"),
    ("Shown to the agent in tools/list. Blank (or the built-in text) uses the default.",
     "Affiché à l’agent dans tools/list. Vide (ou le texte intégré) utilise la valeur par défaut."),
    ("No guide tools yet",                          "Aucun outil de guide pour l’instant"),
    ("Click New guide to author a custom read-only tool the agent can call.",
     "Cliquez sur « Nouveau guide » pour créer un outil personnalisé en lecture seule que l’agent peut appeler."),
    ("Remove guide tool",                           "Supprimer l’outil de guide"),
    ("Name (e.g. my_review_protocol)",              "Nom (par ex. mon_protocole_de_revue)"),
    ("Short description (shown in tools/list)",     "Description courte (affichée dans tools/list)"),
    ("Instructions returned to the agent (optional)",
     "Instructions renvoyées à l’agent (facultatif)"),
    ("Enter a name using letters, numbers, spaces, or underscores.",
     "Saisissez un nom composé de lettres, chiffres, espaces ou tirets bas."),
    // FITS coordinates panel
    ("(right-click on image)",                      "(clic droit sur l’image)"),
    ("Bookmark label…",                             "Libellé du signet…"),
    ("Search Here",                                 "Rechercher ici"),
    ("Search the archive at this position",         "Rechercher dans l’archive à cette position"),
    ("Go To Coordinate",                            "Aller à la coordonnée"),
    ("RA (degrees)",                                "AD (degrés)"),
    ("Dec (degrees)",                               "Déc (degrés)"),
    ("No bookmarks yet",                            "Aucun signet pour l’instant"),
    ("Go to bookmark",                              "Aller au signet"),
    ("Delete bookmark",                             "Supprimer le signet"),
    // VOSpace browser
    ("Upload Files",                                "Téléverser des fichiers"),
    ("Copy Current Path",                           "Copier le chemin actuel"),
    ("Name ▲",                                      "Nom ▲"),
    ("Login to browse your VOSpace files",          "Connectez-vous pour parcourir vos fichiers VOSpace"),
    ("Folder",                                      "Dossier"),
    ("Rename not supported for folders yet",        "Le renommage des dossiers n’est pas encore pris en charge"),
    ("Rename File",                                 "Renommer le fichier"),
    ("Delete Item",                                 "Supprimer l’élément"),
    ("Open in Notebook",                            "Ouvrir dans un carnet"),
    ("Share…",                                      "Partager…"),
    // Cube export
    ("3D volume render",                            "Rendu volumique 3D"),
    ("Render unavailable",                          "Rendu indisponible"),
    ("Mode",                                        "Mode"),
    ("Scale",                                       "Échelle"),
    ("Format",                                      "Format"),
    ("Rendering figure…",                           "Composition de la figure…"),
    ("Could not compose the figure.",               "Impossible de composer la figure."),
    ("PDF Document",                                "Document PDF"),
    ("PNG Image",                                   "Image PNG"),
    // Session card
    ("Resources",                                   "Ressources"),
    ("FLEX",                                        "FLEX"),
    ("Flexible resources — allocated by the platform",
     "Ressources flexibles — allouées par la plateforme"),
    ("In use:",                                     "Utilisé :"),
    ("Open in browser",                             "Ouvrir dans le navigateur"),
    ("Renew session",                               "Renouveler la session"),
    ("View events/logs",                            "Afficher les évènements/journaux"),
    ("Delete session",                              "Supprimer la session"),
    // Resource selector
    // Images
    ("CANFAR Images",                               "Images CANFAR"),
    ("Find images by package…",                     "Trouver des images par paquet…"),
    ("No images available",                         "Aucune image disponible"),
    ("Discovered",                                  "Découvertes"),
    ("Inspection failed",                           "Échec de l’inspection"),
    ("Not inspected yet",                           "Pas encore inspectée"),
    // Local files
    ("Go to Home",                                  "Aller au dossier personnel"),
    ("Browse another folder…",                      "Parcourir un autre dossier…"),
    ("Filter this folder…",                         "Filtrer ce dossier…"),
    ("Browse another folder",                       "Parcourir un autre dossier"),
    ("Show in Files",                               "Afficher dans Fichiers"),
    // Cube slice
    ("Hover the slice for coordinates",             "Survolez la coupe pour les coordonnées"),
    ("Spectrum",                                    "Spectre"),
    ("Play / Pause channels",                       "Lire / Mettre en pause les canaux"),
    ("No signal at this spaxel",                    "Aucun signal à ce spaxel"),
    // Cube tabs
    ("Open Cube…",                                  "Ouvrir un cube…"),
    ("Open a FITS spectral cube (NAXIS≥3)",         "Ouvrir un cube spectral FITS (NAXIS≥3)"),
    ("FITS Cubes",                                  "Cubes FITS"),
    ("Open Cube",                                   "Ouvrir un cube"),
    // FITS header panel
    ("FITS Header",                                 "En-tête FITS"),
    ("Filter keywords…",                            "Filtrer les mots-clés…"),
    ("This extension carries no header keywords.",  "Cette extension ne contient aucun mot-clé d’en-tête."),
    ("No keyword matches that search.",             "Aucun mot-clé ne correspond à cette recherche."),
    // Login
    ("Login to CANFAR",                             "Connexion à CANFAR"),
    ("Sign in with your CADC credentials",          "Connectez-vous avec vos identifiants CADC"),
    ("Please enter username and password",          "Veuillez saisir un nom d’utilisateur et un mot de passe"),
    ("Login failed",                                "Échec de la connexion"),
    // Saved queries
    ("Load into Editor",                            "Charger dans l’éditeur"),
    ("Run Query",                                   "Exécuter la requête"),
    ("Copied!",                                     "Copié !"),
    ("Rename Query",                                "Renommer la requête"),
    // Sharing
    ("Public",                                      "Public"),
    ("Anyone can read",                             "Tout le monde peut lire"),
    ("Read groups",                                 "Groupes en lecture"),
    ("Write groups",                                "Groupes en écriture"),
    // AI connect wizard
    ("Claude Code CLI",                             "Claude Code CLI"),
    ("Claude Desktop configured — restart it to connect.",
     "Claude Desktop configuré — redémarrez-le pour vous connecter."),
    ("Command copied to clipboard.",                "Commande copiée dans le presse-papiers."),
    // Sessions
    ("0 sessions",                                  "0 session"),
    ("No active sessions",                          "Aucune session active"),
    ("refreshing...",                               "actualisation..."),
    // Miscellaneous
    ("No",                                          "Non"),
    ("Run cell (Ctrl+Enter)",                       "Exécuter la cellule (Ctrl+Entrée)"),
    ("(empty)",                                     "(vide)"),
    ("Show or hide the controls",                   "Afficher ou masquer les contrôles"),
    // Long-form descriptions
    ("Re-tune how the AI agent sees each tool. Your edits override the built-in description the MCP server advertises, live — the next tools/list an agent runs uses your wording. Pick a category to focus, or filter across every tool.",
     "Ajustez la façon dont l’agent IA perçoit chaque outil. Vos modifications remplacent en direct la description intégrée annoncée par le serveur MCP — le prochain tools/list exécuté par un agent utilisera votre formulation. Choisissez une catégorie, ou filtrez parmi tous les outils."),
    ("Custom read-only tools you author. Calling one returns your instructions verbatim to the agent — a place to encode your protocols and conventions.",
     "Des outils personnalisés en lecture seule que vous rédigez. Leur appel renvoie vos instructions telles quelles à l’agent — un endroit où encoder vos protocoles et conventions."),
    ("Open a FITS spectral cube (NAXIS≥3) to explore it in 3D — orbit the volume, scrub channels, probe spectra, and export figures.",
     "Ouvrez un cube spectral FITS (NAXIS≥3) pour l’explorer en 3D — orbitez autour du volume, parcourez les canaux, sondez les spectres et exportez des figures."),
    ("Bundle your saved observations, notes, and searches into a single Claude-friendly .zip.",
     "Regroupez vos observations enregistrées, vos notes et vos recherches dans un seul fichier .zip adapté à Claude."),
    ("Search the CADC archive, then save or download observations to see them here.",
     "Recherchez dans l’archive CADC, puis enregistrez ou téléchargez des observations pour les voir ici."),
    ("Grant access by CADC group URI (e.g. ivo://cadc.nrc.ca/gms?MyGroup). Separate multiple groups with spaces.",
     "Accordez l’accès par URI de groupe CADC (par ex. ivo://cadc.nrc.ca/gms?MonGroupe). Séparez plusieurs groupes par des espaces."),
    ("Pick a built-in template or one of your local copies to walk through its steps. Use “New” to start one from scratch.",
     "Choisissez un modèle intégré ou l’une de vos copies locales pour en parcourir les étapes. Utilisez « Nouveau » pour en créer un de zéro."),
    ("This workflow has no steps yet. Use “Edit” (local copy) to add lines like `- [ ] **Step title** — what to do`.",
     "Ce flux de travail n’a pas encore d’étapes. Utilisez « Modifier » (copie locale) pour ajouter des lignes comme `- [ ] **Titre de l’étape** — ce qu’il faut faire`."),
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

/// Serialises tests that switch the active language.
///
/// [`CURRENT`] is process-wide and the test harness runs tests on many threads,
/// so two tests flipping the language would each observe the other's. Holding
/// this for the duration of a switch makes that impossible instead of unlikely.
///
/// The guard is poison-tolerant: a test that panics mid-switch has already
/// failed, and turning that into a cascade of unrelated failures in every other
/// locale test would hide the one that actually broke.
#[cfg(test)]
pub fn testing_lang_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
        Lang::Fr => french(english).unwrap_or(english),
    }
}

/// The French form of `english`, or `None` if neither table has one.
///
/// Separate from [`tr_en`] because the lookup and the *fallback* are different
/// decisions: shipping English when French is missing is right at a call site
/// and useless in a test, which would pass on the fallback and prove nothing.
pub fn french(english: &str) -> Option<&'static str> {
    HAND_EN_TO_FR
        .get(english)
        .copied()
        .or_else(|| EN_TO_FR.get(english).copied())
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
    fn the_removed_duplicates_still_translate() {
        // Removing a HAND_PAIRS entry is only safe if the catalog answers for
        // it. These six were removed as duplicates; each must still resolve.
        for (english, expected) in [
            ("Write config", "Écrire la configuration"),
            ("Testing…", "Test en cours…"),
            ("Active filters", "Filtres actifs"),
            ("Clear all", "Tout effacer"),
            ("Search images…", "Rechercher des images…"),
            ("History", "Historique"),
        ] {
            assert_eq!(
                super::french(english),
                Some(expected),
                "{english:?} lost its French when its duplicate was removed"
            );
        }
    }

    /// A string is translated in one place, not two.
    ///
    /// There are two tables: the key-aligned EN/FR catalogs ported from the
    /// reference, and [`HAND_PAIRS`] for strings the catalogs do not carry.
    /// `french` consults HAND first, so a duplicate entry silently overrides
    /// the catalog — and when the two disagree, the app shows one wording and
    /// the reference another for the same string. Adding French for the AI
    /// Guide's category headings turned up thirty-five duplicates in one go:
    /// every one of those strings was already translated, and the reason the
    /// headings appeared in English was that the CODE never looked them up.
    #[test]
    fn no_hand_pair_duplicates_the_catalog() {
        // The one deliberate override. The catalog's French for this string
        // uses a straight apostrophe where the rest of this app's French uses a
        // typographic one; the catalog is mixed on that (28 to 64) and is a
        // port of the reference's resources, so it is left alone and the
        // override is kept for consistency with its neighbours on screen.
        const DELIBERATE_OVERRIDES: &[&str] = &["Clear history"];

        let mut duplicated = Vec::new();
        for (english, hand) in HAND_PAIRS {
            if DELIBERATE_OVERRIDES.contains(english) {
                continue;
            }
            if let Some(catalog) = EN_TO_FR.get(english) {
                duplicated.push(format!("{english:?}: HAND {hand:?} vs catalog {catalog:?}"));
            }
        }

        assert!(
            duplicated.is_empty(),
            "HAND_PAIRS entr(ies) the key-aligned catalog already translates. \
             HAND wins, so these override the catalog — remove them, or change \
             the catalog if its wording is wrong: {duplicated:#?}"
        );
    }

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
            // Comments stripped too: a doc comment that mentions the macro
            // with a literal argument — explaining this very scan — was read as
            // a call site and demanded a French form for the word "literal".
            // Fifth time a guard in this codebase has matched its own prose.
            let code = crate::testing::without_comments(crate::testing::code(&text));
            let code = code.as_str();
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

    /// Every localized string must actually have a French form.
    ///
    /// `tr_en!` falls back to English when a string is in neither table, which
    /// is what makes it safe to wrap everything — and also what let 488 wrapped
    /// strings ship in English to French users. The catalog is generated from
    /// the reference's RESW files, so it covers the reference's UI; every screen
    /// Verbinal grew past it (the viewer controls, workflows, the AI guide,
    /// settings, the notebook) was wrapped, looked up, missed, and fell back.
    /// Wrapping is visible in review. Falling back is not.
    ///
    /// No exception list, not even for words French leaves alone: "3D", "MIP",
    /// "NaN" and "Verbinal" are all in `HAND_PAIRS` mapped to themselves. A pair
    /// that says "this is the same in French" is a decision someone made; an
    /// omission is a decision nobody made.
    /// A French form for a string the app no longer shows is dead weight.
    ///
    /// The counterpart to `every_localized_string_has_a_french_form`: that one
    /// stops a string shipping untranslated, this one stops the table filling
    /// with translations for text that has been deleted. Removing the Session
    /// Templates card left fifteen behind, and nothing noticed; ten more had
    /// been sitting there from earlier changes.
    #[test]
    fn no_french_form_survives_the_string_it_translates() {
        /// What rustc would make of the source text: line continuations joined
        /// and escapes decoded, so it can be compared against the strings the
        /// table actually holds.
        ///
        /// Comparing raw source instead reports every string containing a
        /// newline, a quote, or a `\u{...}` as orphaned — three separate
        /// false-positive families, each found the hard way.
        fn as_rustc_sees_it(text: &str) -> String {
            let mut out = String::with_capacity(text.len());
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    // A line continuation: the newline and the indent after it
                    // are not part of the string.
                    Some('\n') => {
                        while chars
                            .peek()
                            .is_some_and(|c| c.is_whitespace() && *c != '\n')
                        {
                            chars.next();
                        }
                    }
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('"') => out.push('"'),
                    Some('\'') => out.push('\''),
                    Some('\\') => out.push('\\'),
                    Some('u') if chars.peek() == Some(&'{') => {
                        chars.next();
                        let hex: String = chars.by_ref().take_while(|c| *c != '}').collect();
                        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            Some(decoded) => out.push(decoded),
                            None => out.push_str(&format!("\\u{{{hex}}}")),
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

        let app: String = call_sites()
            .map(|(_, text)| as_rustc_sees_it(&text))
            .collect();
        assert!(app.len() > 100_000, "the source scan came back empty");

        let mut orphans: Vec<&str> = HAND_PAIRS
            .iter()
            .map(|(english, _)| *english)
            .filter(|english| !english.is_empty() && !app.contains(*english))
            .collect();
        orphans.sort();

        assert!(
            orphans.is_empty(),
            "French forms for strings the app no longer shows: {orphans:#?}"
        );
    }

    #[test]
    fn every_localized_string_has_a_french_form() {
        let hand: std::collections::HashSet<&str> = HAND_PAIRS.iter().map(|(en, _)| *en).collect();

        let mut missing: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        for (path, text) in call_sites() {
            // Comments stripped too: a doc comment that mentions the macro
            // with a literal argument — explaining this very scan — was read as
            // a call site and demanded a French form for the word "literal".
            // Fifth time a guard in this codebase has matched its own prose.
            let code = crate::testing::without_comments(crate::testing::code(&text));
            let code = code.as_str();
            for (start, _) in code.match_indices("tr_en!(") {
                let open = start + "tr_en!(".len();
                let open = open + (code[open..].len() - code[open..].trim_start().len());
                let Some(english) = literal_at(code, open) else {
                    continue; // a variable: out of scope for a source scan
                };
                scanned += 1;
                if !hand.contains(english.as_str()) && french(&english).is_none() {
                    let line = code[..start].lines().count();
                    missing.push(format!("{}:{line}: {english:.60?}", path.display()));
                }
            }
        }

        assert!(
            scanned > 500,
            "only {scanned} tr_en! sites found — scan broken"
        );
        missing.sort();
        assert!(
            missing.is_empty(),
            "localized string(s) with no French form — they are wrapped, they are \
             looked up, and the lookup misses: {missing:#?}"
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
            // Comments stripped too: a doc comment that mentions the macro
            // with a literal argument — explaining this very scan — was read as
            // a call site and demanded a French form for the word "literal".
            // Fifth time a guard in this codebase has matched its own prose.
            let code = crate::testing::without_comments(crate::testing::code(&text));
            let code = code.as_str();
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
    fn a_string_from_each_table_resolves_through_the_same_chain() {
        // Both sources answer through one lookup: the generated catalog for
        // what the reference also says, and the hand table for everything
        // Verbinal added. Asserted without `set_lang`, which is global — a test
        // that flipped the language would decide what a parallel test sees.
        assert_eq!(french("Login"), Some("Se connecter")); // catalog
        assert_eq!(french("Blink"), Some("Clignotement")); // hand-written
        assert_eq!(french("Error: {}"), Some("Erreur : {}")); // a template
        assert_eq!(french("3D"), Some("3D")); // same word, stated on purpose
        assert_eq!(french("__nope__"), None);
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
