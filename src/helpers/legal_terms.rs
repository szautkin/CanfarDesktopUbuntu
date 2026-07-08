//! Versioned Terms of Use content (English + French) and acceptance logic.
//!
//! 1:1 port of `Helpers/LegalTerms.cs` in CanfarDesktop. Increment
//! [`CURRENT_VERSION`] whenever the terms change materially so existing users are
//! re-prompted on next launch. The wording is adapted for the Linux port: the
//! privacy clause names the system keyring (Secret Service) instead of the
//! Windows Credential Manager, since Verbinal stores credentials there.

/// Current terms version. Bump when the terms text changes materially.
pub const CURRENT_VERSION: u32 = 1;

/// True if a previously-recorded accepted version covers the current terms.
pub fn is_accepted(accepted_version: Option<u32>) -> bool {
    matches!(accepted_version, Some(v) if v >= CURRENT_VERSION)
}

/// True if the supplied two-letter ISO UI language is French.
///
/// Retained for parity with `LegalTerms.IsFrench`; the shell currently resolves
/// the terms language via [`crate::i18n::current_lang`] instead.
#[allow(dead_code)]
pub fn is_french(two_letter_iso_language: Option<&str>) -> bool {
    two_letter_iso_language
        .map(|s| s.eq_ignore_ascii_case("fr"))
        .unwrap_or(false)
}

/// Terms title in the requested language.
pub fn title(french: bool) -> &'static str {
    if french {
        TITLE_FR
    } else {
        TITLE_EN
    }
}

/// Terms body in the requested language.
pub fn body(french: bool) -> &'static str {
    if french {
        BODY_FR
    } else {
        BODY_EN
    }
}

pub const TITLE_EN: &str = "Terms of Use";
pub const TITLE_FR: &str = "Conditions d'utilisation";

pub const BODY_EN: &str = concat!(
    "Please read and accept these Terms of Use before using Verbinal (\"the App\").\n\n",
    "1. No affiliation. Verbinal is an independent, third-party client for the CANFAR ",
    "Science Portal and the Canadian Astronomy Data Centre (CADC). It is not affiliated ",
    "with, endorsed by, or supported by CANFAR, the CADC, the National Research Council ",
    "Canada, or their partners. Your use of CANFAR/CADC services through the App remains ",
    "subject to those services' own terms and policies.\n\n",
    "2. No warranty. The App is provided \"as is\" and \"as available\", without warranties ",
    "of any kind, whether express, implied, or statutory, including but not limited to ",
    "warranties of merchantability, fitness for a particular purpose, accuracy, and ",
    "non-infringement. You use the App and any data obtained through it at your own risk.\n\n",
    "3. Limitation of liability. To the maximum extent permitted by law, the developer ",
    "shall not be liable for any indirect, incidental, special, consequential, or punitive ",
    "damages, or any loss of data, profits, or research, arising out of or related to your ",
    "use of the App. Because the App is provided free of charge, the developer's total ",
    "aggregate liability for any claim shall not exceed CAD $0.\n\n",
    "4. Your responsibilities. You are responsible for safeguarding your CANFAR credentials ",
    "and for all activity performed through your account. You agree to use the App lawfully ",
    "and in accordance with the CANFAR/CADC acceptable-use policies.\n\n",
    "5. Privacy. The App collects no personal data and contains no analytics or telemetry. ",
    "Credentials are stored only on your device (your system keyring via the Secret Service) ",
    "and are sent only to trusted CANFAR/CADC hosts over HTTPS. See the Privacy notice for ",
    "details.\n\n",
    "6. Governing law. These Terms are governed by the laws of the Province of British ",
    "Columbia and the federal laws of Canada applicable therein, without regard to ",
    "conflict-of-laws principles.\n\n",
    "By selecting \"Accept\" you acknowledge that you have read, understood, and agree to ",
    "these Terms of Use. If you do not agree, select \"Decline & Exit\"."
);

pub const BODY_FR: &str = concat!(
    "Veuillez lire et accepter les présentes conditions d'utilisation avant d'utiliser ",
    "Verbinal (« l'Application »).\n\n",
    "1. Aucune affiliation. Verbinal est un client indépendant et tiers du portail ",
    "scientifique CANFAR et du Centre canadien de données astronomiques (CADC). Il n'est ",
    "ni affilié, ni approuvé, ni pris en charge par CANFAR, le CADC, le Conseil national ",
    "de recherches du Canada ou leurs partenaires. Votre utilisation des services ",
    "CANFAR/CADC via l'Application demeure assujettie aux conditions et politiques propres ",
    "à ces services.\n\n",
    "2. Aucune garantie. L'Application est fournie « telle quelle » et « selon ",
    "disponibilité », sans garantie d'aucune sorte, expresse, implicite ou légale, y ",
    "compris, sans s'y limiter, les garanties de qualité marchande, d'adéquation à un ",
    "usage particulier, d'exactitude et d'absence de contrefaçon. Vous utilisez ",
    "l'Application et toute donnée obtenue par son intermédiaire à vos propres risques.\n\n",
    "3. Limitation de responsabilité. Dans la mesure maximale permise par la loi, le ",
    "développeur ne saurait être tenu responsable de tout dommage indirect, accessoire, ",
    "spécial, consécutif ou punitif, ni de toute perte de données, de profits ou de travaux ",
    "de recherche, découlant de votre utilisation de l'Application ou s'y rapportant. ",
    "L'Application étant fournie gratuitement, la responsabilité globale totale du ",
    "développeur pour toute réclamation ne dépassera pas 0 $ CA.\n\n",
    "4. Vos responsabilités. Vous êtes responsable de la protection de vos identifiants ",
    "CANFAR et de toute activité effectuée au moyen de votre compte. Vous acceptez ",
    "d'utiliser l'Application de manière licite et conformément aux politiques ",
    "d'utilisation acceptable de CANFAR/CADC.\n\n",
    "5. Confidentialité. L'Application ne recueille aucune donnée personnelle et ne contient ",
    "aucun outil d'analyse ou de télémétrie. Les identifiants sont stockés uniquement sur ",
    "votre appareil (votre trousseau système via le Secret Service) et ne sont transmis ",
    "qu'à des hôtes CANFAR/CADC de confiance, en HTTPS. Consultez l'avis de confidentialité ",
    "pour plus de détails.\n\n",
    "6. Droit applicable. Les présentes conditions sont régies par les lois de la province ",
    "de la Colombie-Britannique et les lois fédérales du Canada qui y sont applicables, sans ",
    "égard aux principes de conflits de lois.\n\n",
    "En sélectionnant « Accepter », vous reconnaissez avoir lu, compris et accepté les ",
    "présentes conditions d'utilisation. Si vous n'êtes pas d'accord, sélectionnez ",
    "« Refuser et quitter »."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unaccepted_when_no_prior_version() {
        assert!(!is_accepted(None));
    }

    #[test]
    fn older_version_is_not_accepted() {
        assert!(!is_accepted(Some(CURRENT_VERSION.saturating_sub(1).min(0))));
        // An explicit older-than-current value must re-prompt.
        assert!(!is_accepted(Some(0)));
    }

    #[test]
    fn current_or_newer_is_accepted() {
        assert!(is_accepted(Some(CURRENT_VERSION)));
        assert!(is_accepted(Some(CURRENT_VERSION + 1)));
    }

    #[test]
    fn french_detection_is_case_insensitive() {
        assert!(is_french(Some("fr")));
        assert!(is_french(Some("FR")));
        assert!(!is_french(Some("en")));
        assert!(!is_french(None));
    }

    #[test]
    fn language_selects_matching_text() {
        assert_eq!(title(false), TITLE_EN);
        assert_eq!(title(true), TITLE_FR);
        assert_eq!(body(false), BODY_EN);
        assert_eq!(body(true), BODY_FR);
        assert!(body(false).contains("Terms of Use"));
        assert!(body(true).contains("conditions d'utilisation"));
        // The Linux port must not mention the Windows Credential Manager.
        assert!(!body(false).contains("Windows Credential Manager"));
        assert!(body(false).contains("Secret Service"));
    }
}
