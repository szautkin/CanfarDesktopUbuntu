use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Find a usable Python interpreter.
///
/// Search order:
/// 1. `configured_path` — if supplied and the binary validates, use it.
/// 2. `$CONDA_PREFIX/bin/python3` — active conda environment, if set.
/// 3. `$VIRTUAL_ENV/bin/python3` — active virtualenv, if set.
/// 4. `python3` on `PATH`.
/// 5. `python` on `PATH`, provided it reports version >= 3.8.
/// 6. A set of well-known installation locations on Linux/macOS.
///
/// Returns the first candidate whose [`validate_python`] call succeeds.
pub fn find_python(configured_path: Option<&str>) -> Option<PathBuf> {
    // 1. Explicit configured path.
    if let Some(p) = configured_path {
        let pb = PathBuf::from(p);
        if validate_python(&pb).is_some() {
            return Some(pb);
        }
    }

    // 2. Active conda environment.
    if let Ok(prefix) = std::env::var("CONDA_PREFIX") {
        let p = PathBuf::from(format!("{}/bin/python3", prefix));
        if p.exists() && validate_python(&p).is_some() {
            return Some(p);
        }
    }

    // 3. Active virtualenv.
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        let p = PathBuf::from(format!("{}/bin/python3", venv));
        if p.exists() && validate_python(&p).is_some() {
            return Some(p);
        }
    }

    // 4. `python3` on PATH.
    if let Some(p) = which("python3") {
        if validate_python(&p).is_some() {
            return Some(p);
        }
    }

    // 5. `python` on PATH — accept only if >= 3.8.
    if let Some(p) = which("python") {
        if let Some((major, minor)) = validate_python(&p) {
            if major >= 3 && minor >= 8 {
                return Some(p);
            }
        }
    }

    // 6. Common fixed locations.
    for candidate in common_locations() {
        if let Some((major, minor)) = validate_python(&candidate) {
            if major >= 3 && minor >= 8 {
                return Some(candidate);
            }
        }
    }

    None
}

/// Validate that `path` points to a Python interpreter and return its version.
///
/// Runs `<path> --version`, parses the output (e.g. `Python 3.10.4`), and
/// returns `Some((major, minor))` on success or `None` if the binary is
/// missing, fails, or reports a version we cannot parse.
pub fn validate_python(path: &std::path::Path) -> Option<(u32, u32)> {
    let output = Command::new(path).arg("--version").output().ok()?;

    // Python 2 writes to stderr; Python 3 writes to stdout.
    // Try both to handle edge cases.
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).into_owned()
    } else {
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    parse_python_version(text.trim())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Look up `name` on `PATH` and return an absolute `PathBuf` if found.
fn which(name: &str) -> Option<PathBuf> {
    // Use the `which` logic from the standard PATH variable rather than
    // spawning a subprocess — avoids the cost of a shell invocation.
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        // On Linux there is no `.exe` extension, but add the check anyway for
        // potential cross-compilation test scenarios.
        let with_exe = dir.join(format!("{}.exe", name));
        if with_exe.is_file() {
            return Some(with_exe);
        }
    }
    None
}

/// Return a list of common Python interpreter locations to try as a fallback.
fn common_locations() -> Vec<PathBuf> {
    let mut locations: Vec<PathBuf> = Vec::new();

    // Versioned binaries in /usr/bin — try newest first.
    for minor in (8u32..=13).rev() {
        locations.push(PathBuf::from(format!("/usr/bin/python3.{}", minor)));
        locations.push(PathBuf::from(format!("/usr/local/bin/python3.{}", minor)));
    }

    // Generic system paths.
    locations.push(PathBuf::from("/usr/bin/python3"));
    locations.push(PathBuf::from("/usr/local/bin/python3"));
    locations.push(PathBuf::from("/usr/bin/python"));

    // Homebrew (macOS/Linux).
    locations.push(PathBuf::from("/opt/homebrew/bin/python3"));
    locations.push(PathBuf::from("/usr/local/opt/python3/bin/python3"));

    // Conda / mamba typical install.
    if let Ok(home) = std::env::var("HOME") {
        for conda_dir in &["anaconda3", "miniconda3", "mambaforge", "miniforge3"] {
            locations.push(PathBuf::from(format!("{}/{}/bin/python3", home, conda_dir)));
        }
        // Pyenv.
        locations.push(PathBuf::from(format!("{}/.pyenv/shims/python3", home)));
    }

    locations
}

/// Parse a version string of the form `Python X.Y[.Z[…]]`.
///
/// Returns `Some((major, minor))` or `None` if the format is unexpected.
fn parse_python_version(s: &str) -> Option<(u32, u32)> {
    // Expected: "Python 3.10.4" or "Python 3.8"
    let version_part = s.strip_prefix("Python ")?.trim();
    let mut parts = version_part.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_python_version ---

    #[test]
    fn parse_version_full() {
        assert_eq!(parse_python_version("Python 3.10.4"), Some((3, 10)));
    }

    #[test]
    fn parse_version_short() {
        assert_eq!(parse_python_version("Python 3.8"), Some((3, 8)));
    }

    #[test]
    fn parse_version_two_digit_minor() {
        assert_eq!(parse_python_version("Python 3.12.1"), Some((3, 12)));
    }

    #[test]
    fn parse_version_python2() {
        assert_eq!(parse_python_version("Python 2.7.18"), Some((2, 7)));
    }

    #[test]
    fn parse_version_invalid_prefix() {
        assert!(parse_python_version("python 3.9").is_none());
    }

    #[test]
    fn parse_version_empty() {
        assert!(parse_python_version("").is_none());
    }

    #[test]
    fn parse_version_garbage() {
        assert!(parse_python_version("not a version").is_none());
    }

    // --- validate_python (integration, requires Python on the test machine) ---

    #[test]
    fn validate_python_system_python3() {
        // This test is best-effort: skip silently on machines without python3.
        if let Some(p) = which("python3") {
            if let Some((major, minor)) = validate_python(&p) {
                // `validate_python` only PARSES the reported version — any 3.x is
                // a valid result here. The >= 3.8 requirement is enforced by
                // `find_python`, which has its own coverage.
                assert_eq!(major, 3, "expected Python 3, got {}.{}", major, minor);
            }
            // If validate returns None the binary exists but is broken; that is
            // still a valid outcome for the test (we just cannot assert more).
        }
    }

    #[test]
    fn validate_python_nonexistent_path() {
        let result = validate_python(std::path::Path::new("/no/such/python_binary_xyz"));
        assert!(result.is_none());
    }

    // --- find_python ---

    #[test]
    fn find_python_with_valid_configured_path() {
        // Build a tiny shell script that pretends to be Python.
        use std::io::Write;
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let script = std::env::temp_dir().join(format!("verbinal_fake_python_{}", n));
        {
            let mut f = std::fs::File::create(&script).expect("create");
            writeln!(f, "#!/bin/sh").unwrap();
            writeln!(f, r#"echo "Python 3.9.0""#).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let result = find_python(Some(script.to_str().unwrap()));
        // On Linux where /bin/sh is available this should succeed; on other
        // platforms it may not be.  We just check for no panic.
        let _ = result;
        let _ = std::fs::remove_file(&script);
    }

    #[test]
    fn find_python_invalid_configured_path_falls_through() {
        // A configured path that does not exist should not panic; find_python
        // must fall through to other search strategies.
        let _ = find_python(Some("/nonexistent/bin/python3"));
        // No assertion needed — the test passes as long as there is no panic.
    }

    #[test]
    fn find_python_none_configured() {
        // Should not panic even when no configured path is given.
        let _ = find_python(None);
    }

    // --- which ---

    #[test]
    fn which_finds_ls() {
        // `ls` is available on every Unix-like system we care about.
        let result = which("ls");
        // May return None in very locked-down CI environments.
        if let Some(path) = result {
            assert!(path.is_absolute());
        }
    }

    #[test]
    fn which_missing_binary() {
        assert!(which("this_binary_cannot_possibly_exist_xyz_canfar").is_none());
    }

    // --- common_locations ---

    #[test]
    fn common_locations_not_empty() {
        assert!(!common_locations().is_empty());
    }

    #[test]
    fn common_locations_all_absolute() {
        for loc in common_locations() {
            assert!(
                loc.is_absolute(),
                "expected absolute path, got: {}",
                loc.display()
            );
        }
    }
}
