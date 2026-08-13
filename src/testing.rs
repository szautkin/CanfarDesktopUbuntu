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
    fn only_the_first_test_module_boundary_matters() {
        // Two test modules, and everything from the first is dropped: a guard
        // that scanned up to the LAST one would still see the tests before it.
        let file = "fn a() {}\n#[cfg(test)]\nmod x { }\n#[cfg(test)]\nmod y { }\n";
        assert_eq!(code(file), "fn a() {}\n");
    }
}
