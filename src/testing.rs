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

/// Strip Rust comment lines, leaving only code.
///
/// Source-scanning guards keep catching their own explanations: a comment
/// naming the defect ("this used to call `of_state`") satisfies a guard looking
/// for that call, so the guard passes while the defect is present. Prose about
/// a bug is not the bug, and three guards learned that the same way.
///
/// Line comments only — a `/* */` spanning lines would need a parser, and no
/// guard has needed one.
pub fn without_comments(code: &str) -> String {
    without_line_comments(code, "//")
}

/// The same, for a language whose line comments start with `prefix`.
///
/// The Python kernel harness is scanned by guards too, and `//` is not its
/// comment marker — so a guard reading it through [`without_comments`] found
/// the very explanation it was meant to look past. Same trap, different
/// language.
pub fn without_line_comments(code: &str, prefix: &str) -> String {
    code.lines()
        .filter(|line| !line.trim_start().starts_with(prefix))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Python source with its `#` comments AND its docstrings removed.
///
/// Stripping `#` lines is not enough for Python: most of the explanation in a
/// module lives in triple-quoted docstrings, which are expressions rather than
/// comments. A guard on the kernel harness asserting that a defective call is
/// ABSENT found it in the docstring describing why it was removed — the same
/// trap [`without_comments`] exists for, one syntax further along.
///
/// Both quote styles are handled, and an unterminated opener is treated as
/// running to the end of the file: a guard that sees too little fails loudly,
/// one that sees prose passes silently.
pub fn python_code(source: &str) -> String {
    const QUOTES: [&str; 2] = ["\"\"\"", "'''"];
    let stripped = without_line_comments(source, "#");
    let mut out = String::with_capacity(stripped.len());
    let mut rest = stripped.as_str();
    loop {
        let next = QUOTES
            .iter()
            .filter_map(|q| rest.find(q).map(|at| (at, *q)))
            .min_by_key(|(at, _)| *at);
        let Some((at, quote)) = next else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at]);
        let after = &rest[at + quote.len()..];
        match after.find(quote) {
            Some(end) => rest = &after[end + quote.len()..],
            None => return out,
        }
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

    /// A docstring explaining a removed call must not look like the call.
    #[test]
    fn a_python_docstring_is_not_part_of_the_code() {
        let file = concat!(
            "def f():\n",
            "    \"\"\"This used to compile(code, \"eval\").\"\"\"\n",
            "    return ast.parse(code)\n"
        );
        let code = super::python_code(file);
        assert!(
            !code.contains("compile(code"),
            "a guard could find the very call the docstring says was removed: {code}"
        );
        assert!(
            code.contains("ast.parse(code)"),
            "real code was lost: {code}"
        );
    }

    #[test]
    fn both_docstring_styles_are_removed() {
        let code = super::python_code("a = 1\n'''gone'''\nb = 2\n");
        assert!(!code.contains("gone"), "{code}");
        assert!(code.contains("a = 1") && code.contains("b = 2"), "{code}");

        // An unterminated opener swallows the rest of the file. A guard that
        // sees too little fails loudly; one that sees prose passes silently.
        let code = super::python_code("a = 1\n\"\"\"unterminated\nb = 2\n");
        assert!(!code.contains("unterminated"), "{code}");
    }

    #[test]
    fn a_comment_naming_a_defect_does_not_look_like_the_defect() {
        let file = "// this used to call of_state(x)\nlet y = keep(x);\n";
        let code = super::without_comments(file);
        assert!(!code.contains("of_state("));
        assert!(code.contains("keep(x)"));
    }

    #[test]
    fn a_python_comment_is_stripped_when_asked_for() {
        // The kernel harness is Python; `//` is not its comment marker, so a
        // guard reading it through the Rust helper found its own explanation.
        let harness = "# used to compile(code, \"eval\")\nast.parse(code)";
        let code = super::without_line_comments(harness, "#");
        assert!(!code.contains("compile("));
        assert!(code.contains("ast.parse"));
    }

    #[test]
    fn a_doc_comment_is_stripped_too() {
        // `///` is where the explanations that fooled the guards actually live.
        assert!(!super::without_comments("/// calls foo()\nfn bar() {}").contains("foo()"));
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
