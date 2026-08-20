//! Which of a notebook's imports are not installed, and how to install them.
//!
//! [`notebook_parser::extract_imports`](crate::helpers::notebook_parser::extract_imports)
//! reads the modules a notebook needs. This module answers the next two
//! questions — *which of them is missing* and *what would install it* — which
//! together are the reference's `DependencyScanner`. Ours had the scanner half
//! only, so the imports were extracted and nobody ever asked.
//!
//! Everything here is a pure function over strings: the probe script, the
//! reading of its output, and the install argv. Running a subprocess is the
//! caller's job, which is what makes the interesting parts testable at all.

/// pip's name for a module, where the two differ.
///
/// `import cv2` installs as `opencv-python`; telling a user to
/// `pip install cv2` sends them to a package that is not the one they want.
/// Same list as the reference's `ModuleToPip`.
#[rustfmt::skip]
const MODULE_TO_PIP: &[(&str, &str)] = &[
    ("PIL",     "Pillow"),
    ("cv2",     "opencv-python"),
    ("sklearn", "scikit-learn"),
    ("yaml",    "PyYAML"),
    ("bs4",     "beautifulsoup4"),
    ("attr",    "attrs"),
    ("dateutil","python-dateutil"),
];

/// The package name to install for `module`.
pub fn pip_name(module: &str) -> &str {
    MODULE_TO_PIP
        .iter()
        .find(|(m, _)| m.eq_ignore_ascii_case(module))
        .map(|(_, pip)| *pip)
        .unwrap_or(module)
}

/// A python program that reports, one module per line, whether it imports.
///
/// One process for every module rather than one per module: a notebook with a
/// dozen imports would otherwise pay a dozen interpreter startups, and the
/// check runs while the user is waiting to read their notebook.
pub fn probe_script(modules: &[String]) -> String {
    let mut out = String::from("import importlib.util as u\n");
    for m in modules {
        // Only ever module names from `extract_imports`, which the import regex
        // restricts to word characters — but quote defensively anyway, since
        // this string becomes code.
        let safe: String = m
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if safe.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "print('{safe}:' + ('ok' if u.find_spec('{safe}') else 'missing'))\n"
        ));
    }
    out
}

/// The modules the probe reported missing, in the order it reported them.
///
/// Unparseable lines are ignored rather than assumed missing: a warning printed
/// by a site-packages import hook should not turn into "install numpy".
pub fn missing_from_probe(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.trim().split_once(':'))
        .filter(|(_, state)| *state == "missing")
        .map(|(module, _)| module.to_string())
        .collect()
}

/// Where an install is allowed to write.
///
/// Debian and Ubuntu mark the system Python as externally managed (PEP 668) and
/// pip refuses to write into it, because pip and apt would be fighting over the
/// same files. The reference never meets this — Windows Python is not
/// externally managed — so it simply runs `pip install`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallScope {
    /// `--user`: the default, and enough for any Python the distribution does
    /// not manage.
    User,
    /// `--break-system-packages`: writes into a Python the distribution owns.
    ///
    /// Never the default and never automatic. The flag is named as a warning,
    /// and the consequences land on a machine we do not own — so it is offered
    /// only after [`externally_managed`] has established that `--user` cannot
    /// work, and only when someone asks for it in as many words.
    OverrideSystemPython,
}

impl InstallScope {
    fn flag(self) -> &'static str {
        match self {
            Self::User => "--user",
            Self::OverrideSystemPython => "--break-system-packages",
        }
    }
}

/// The argv that installs `packages` with the interpreter's own pip.
///
/// `python -m pip`, not a bare `pip`: a machine with several interpreters has
/// several pips, and the one on `PATH` is regularly not the one running the
/// kernel.
pub fn install_args(packages: &[String], scope: InstallScope) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        "pip".to_string(),
        "install".to_string(),
        scope.flag().to_string(),
    ];
    args.extend(packages.iter().cloned());
    args
}

/// Whether pip refused because the interpreter is externally managed.
///
/// Read from pip's own words rather than probed beforehand: the marker file
/// PEP 668 describes lives in a directory that varies by distribution and
/// build, and pip already knows the answer. Getting this wrong in the safe
/// direction costs a generic error message; getting it wrong the other way
/// would offer to override something that was never in the way.
pub fn externally_managed(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("externally-managed-environment") || lower.contains("externally managed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_targets_the_users_own_site_by_default() {
        let args = install_args(&["numpy".into()], InstallScope::User);
        assert_eq!(args, ["-m", "pip", "install", "--user", "numpy"]);
    }

    #[test]
    fn the_override_is_a_different_flag_and_a_deliberate_one() {
        // On Ubuntu the system Python is externally managed and `--user` fails
        // by design. The way past it is named for what it does, and it is never
        // what a plain install picks.
        let args = install_args(&["numpy".into()], InstallScope::OverrideSystemPython);
        assert!(args.contains(&"--break-system-packages".to_string()));
        assert!(!args.contains(&"--user".to_string()));
    }

    #[test]
    fn pips_refusal_is_recognised_from_its_own_words() {
        assert!(externally_managed(
            "error: externally-managed-environment\n\n\u{d7} This environment is externally managed"
        ));
        assert!(externally_managed("This environment is externally managed"));
    }

    #[test]
    fn an_ordinary_failure_is_not_mistaken_for_it() {
        // Offering to override the system Python because a package name was
        // wrong would be worse than the original failure.
        assert!(!externally_managed(
            "ERROR: Could not find a version that satisfies the requirement nosuchpkg"
        ));
        assert!(!externally_managed("ERROR: Network is unreachable"));
        assert!(!externally_managed(""));
    }

    #[test]
    fn a_module_that_installs_under_another_name_gets_that_name() {
        assert_eq!(pip_name("cv2"), "opencv-python");
        assert_eq!(pip_name("PIL"), "Pillow");
        // Case-insensitively, as the reference's map is.
        assert_eq!(pip_name("Yaml"), "PyYAML");
        // Everything else installs under its own name.
        assert_eq!(pip_name("astropy"), "astropy");
    }

    #[test]
    fn the_probe_asks_about_every_module_once() {
        let script = probe_script(&["numpy".into(), "astropy".into()]);
        assert!(script.contains("find_spec('numpy')"));
        assert!(script.contains("find_spec('astropy')"));
        assert_eq!(script.lines().count(), 3); // one import + two prints
    }

    #[test]
    fn the_probe_cannot_be_talked_into_running_something_else() {
        // The script is code; a module name carrying a quote would otherwise
        // close the string and run whatever follows.
        let script = probe_script(&["os'); __import__('shutil').rmtree('/".into()]);
        // The punctuation that would close the string and start a statement is
        // what has to be gone; the letters are harmless inside an identifier no
        // module answers to. Every quoted region is a bare name, and there are
        // exactly two of them — so nothing escaped its quotes.
        for quoted in script.split('\'').skip(1).step_by(2) {
            assert!(
                quoted
                    .trim_end_matches(':')
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_'),
                "{quoted:?} escaped its quotes in {script}"
            );
        }
    }

    #[test]
    fn only_the_missing_ones_are_reported() {
        let out = "numpy:ok\nastropy:missing\nscipy:missing\n";
        assert_eq!(missing_from_probe(out), vec!["astropy", "scipy"]);
    }

    #[test]
    fn noise_on_the_probes_output_is_not_a_missing_package() {
        // Import hooks and deprecation warnings print to stdout in the wild.
        let out = "RuntimeWarning: numpy.ndarray size changed\nastropy:missing\n";
        assert_eq!(missing_from_probe(out), vec!["astropy"]);
    }

    #[test]
    fn install_runs_the_interpreters_own_pip() {
        let args = install_args(&["astropy".into(), "Pillow".into()], InstallScope::User);
        assert_eq!(
            args,
            ["-m", "pip", "install", "--user", "astropy", "Pillow"]
        );
    }
}
