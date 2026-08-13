//! Helpers for the source-scanning guards.
//!
//! A number of invariants in this app cannot be checked at runtime — GTK layout
//! has no unit coverage, and "is this control on screen" is a question about a
//! widget tree no test can build. Those are guarded by reading the source with
//! `include_str!`.
//!
//! Every one of them meets the same trap: **the test lives in the file it
//! scans**, so an assertion that mentions the thing it is looking for finds
//! itself and passes against the very bug it was written for. That has happened
//! five times here, each time discovered only by deliberately re-introducing the
//! bug. The workaround so far was to assemble the needle at runtime, which works
//! but has to be remembered every time.
//!
//! [`code`] removes the trap instead of dodging it: scan the part of the file
//! that is not tests, and a test can no longer be its own evidence.

/// Every `.rs` file under `src/`, as `(path, text)`.
///
/// Guards that check a whole-codebase invariant — "no user-visible literal ships
/// untranslated", "every `tr_fmt!` template has a French pair" — all need the
/// same walk, and it is the kind of code that gets copied with a subtle
/// difference (one guard skipping a directory the other scans, so an invariant
/// quietly stops covering a module).
///
/// The text is returned **raw**, not stripped: whether test code counts is the
/// caller's decision, and a walk that silently dropped it would make one guard's
/// coverage depend on another guard's needs. Apply [`code`] at the call site.
pub fn rust_sources() -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push((path, text));
                }
            }
        }
    }
    // Directory order is filesystem order; sort so a failing guard names files in
    // the same order twice running.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The source with its test modules removed.
///
/// Everything from the first `#[cfg(test)]` onwards is dropped — test modules
/// are conventionally last in this codebase, and a guard has no business
/// asserting anything about test code anyway.
pub fn code(source: &str) -> &str {
    match source.find("#[cfg(test)]") {
        Some(at) => &source[..at],
        None => source,
    }
}

#[cfg(test)]
mod tests {
    use super::code;

    #[test]
    fn tests_are_not_part_of_the_code_being_scanned() {
        let file = "fn real() {}\n#[cfg(test)]\nmod tests { fn pretend() {} }\n";
        assert!(code(file).contains("fn real()"));
        assert!(
            !code(file).contains("fn pretend()"),
            "a guard must not be able to find itself"
        );
    }

    #[test]
    fn a_file_with_no_tests_is_returned_whole() {
        assert_eq!(code("fn only() {}"), "fn only() {}");
    }

    #[test]
    fn the_walk_finds_the_whole_source_tree() {
        let files = super::rust_sources();
        // Nested directories, not just the top level.
        assert!(
            files.iter().any(|(p, _)| p.ends_with("ui/fits_viewer.rs")),
            "the walk should reach src/ui/"
        );
        assert!(
            files
                .iter()
                .any(|(p, _)| p.ends_with("mcp/tools/catalog.rs")),
            "the walk should reach src/mcp/tools/"
        );
        assert!(files.len() > 100, "src has more files than this walk found");
        // Raw, so a guard can decide for itself whether tests count. This file's
        // own test module is proof the text is not pre-stripped.
        let (_, own) = files
            .iter()
            .find(|(p, _)| p.ends_with("testing.rs"))
            .expect("testing.rs is under src/");
        assert!(own.contains("mod tests"));
    }

    #[test]
    fn only_the_first_test_module_boundary_matters() {
        // Two test modules, and everything from the first is dropped: a guard
        // that scanned up to the LAST one would still see the tests before it.
        let file = "fn a() {}\n#[cfg(test)]\nmod x { }\n#[cfg(test)]\nmod y { }\n";
        assert_eq!(code(file), "fn a() {}\n");
    }
}
