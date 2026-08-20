//! Telling a local path from a remote one, before something is written to the
//! wrong filesystem.
//!
//! Tools that write to the local disk and tools that write to VOSpace both take
//! a `path`, and a caller that has been working in VOSpace will reach for the
//! VOSpace path. `save_notebook` did exactly that: it created a local directory
//! tree named after the remote location, reported success with a `filePath` the
//! caller recognised, and left VOSpace empty.
//!
//! `download_vospace_file` is the sharpest case, because there the two live
//! side by side: `path` is remote by definition and `local_path` is not, and
//! nothing about the pair says which is which.

/// Schemes that mean "not on this machine".
const REMOTE_PREFIXES: &[&str] = &["vos:", "arc:", "ivo:", "http://", "https://"];

/// For a tool that WRITES locally: the file has to land here first.
pub const SAVE_THEN_UPLOAD: &str =
    "Write it to a local path first, then copy it across with upload_file_to_vospace.";

/// For a tool that READS locally: the file has to be here already.
pub const FETCH_IT_FIRST: &str =
    "Fetch it to a local path first with download_vospace_file, then pass that path.";

/// For the research bundle, which can make the trip itself.
pub const USE_THE_UPLOAD_FLAG: &str =
    "Give a local path and set uploadToVospace — the export copies the finished bundle across.";

/// `Err` when `path` names somewhere other than the local filesystem.
///
/// `remedy` names the tool that DOES do what the caller was reaching for. A
/// caller that has just been told "no" needs to know what "yes" looks like, and
/// the answer differs by direction: a local write wants
/// [`SAVE_THEN_UPLOAD`], a local read wants [`FETCH_IT_FIRST`]. Passing it in
/// rather than baking one in is why this stayed one function when the second
/// direction turned up.
pub fn reject_remote(path: &str, remedy: &str) -> Result<(), String> {
    let p = path.trim();
    if let Some(scheme) = REMOTE_PREFIXES
        .iter()
        .find(|s| p.to_ascii_lowercase().starts_with(**s))
    {
        return Err(format!(
            "`{p}` is a {scheme} location, and this tool works on the local filesystem. {remedy}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_path_is_accepted() {
        assert!(reject_remote("/home/alice/work/run.ipynb", SAVE_THEN_UPLOAD).is_ok());
        assert!(reject_remote("relative/run.ipynb", SAVE_THEN_UPLOAD).is_ok());
        // `/arc/...` is a MOUNT POINT, not a scheme: on a machine where it is
        // mounted it is an ordinary local path, and refusing it would break the
        // one workflow this is meant to protect.
        assert!(reject_remote("/arc/home/alice/run.ipynb", SAVE_THEN_UPLOAD).is_ok());
    }

    #[test]
    fn a_remote_path_is_refused_and_says_what_to_use() {
        for remote in [
            "vos://cadc.nrc.ca~arc/home/alice/run.ipynb",
            "arc:/home/alice/run.ipynb",
            "https://example.org/run.ipynb",
        ] {
            let err = reject_remote(remote, SAVE_THEN_UPLOAD).expect_err(remote);
            assert!(err.contains("upload_file_to_vospace"), "{err}");
        }
    }

    #[test]
    fn the_remedy_follows_the_direction_of_the_transfer() {
        // Telling someone to upload a file they were trying to read is not a
        // remedy; it is a second wrong turn.
        let err = reject_remote("vos:/home/alice/in.fits", FETCH_IT_FIRST).expect_err("remote");
        assert!(err.contains("download_vospace_file"), "{err}");
        assert!(!err.contains("upload_file_to_vospace"), "{err}");
    }

    #[test]
    fn the_scheme_is_recognised_whatever_its_case() {
        assert!(reject_remote("VOS://cadc.nrc.ca~arc/x.ipynb", SAVE_THEN_UPLOAD).is_err());
    }

    /// Every tool named in a remedy is a tool that exists.
    ///
    /// The first version of this message sent callers to `upload_vospace_file`,
    /// which was never a tool — so the one sentence whose whole job was to say
    /// what "yes" looks like pointed at nothing. A rename would have done the
    /// same thing silently.
    #[test]
    fn the_remedies_name_tools_that_exist() {
        let registered: std::collections::HashSet<String> = crate::mcp::tools::family_descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();

        let mut cited = 0usize;
        for remedy in [SAVE_THEN_UPLOAD, FETCH_IT_FIRST] {
            for word in remedy.split_whitespace() {
                let word = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                // Tool names are the snake_case words; prose here is not.
                if !word.contains('_') {
                    continue;
                }
                cited += 1;
                assert!(
                    registered.contains(word),
                    "remedy names `{word}`, which is not a registered tool"
                );
            }
        }
        assert_eq!(cited, 2, "expected one tool name per remedy");
    }
    /// Every tool that names a LOCAL path refuses a remote one.
    ///
    /// The rule was applied where the bug was reported — `save_notebook` — and
    /// nowhere else. Five more tools took a local path and did not check it:
    /// `export_search_results` wrote into a directory it created called `vos:`
    /// and returned `"exported": true`; `open_cube` returned `"opened": true`
    /// for a file it could never have opened.
    ///
    /// So this does not check the sites that were fixed. It walks the whole
    /// registry and refuses to let a path argument go UNCLASSIFIED: a new tool
    /// with a path fails here until someone says which side of the boundary it
    /// is on, and the local list is checked against the handlers.
    #[test]
    fn every_path_argument_in_the_registry_is_classified() {
        // Local: the handler must call `reject_remote` on it.
        const LOCAL: &[(&str, &str)] = &[
            ("download_vospace_file", "localPath"),
            ("export_cube_figure", "path"),
            ("export_research_bundle", "path"),
            ("export_search_results", "path"),
            ("get_fits_header", "localPath"),
            ("get_fits_wcs", "localPath"),
            ("open_cube", "path"),
            ("open_fits_file", "path"),
            ("open_notebook", "path"),
            ("save_notebook", "path"),
            ("upload_file_to_vospace", "localPath"),
        ];
        // Remote by definition — a VOSpace path is the whole point of these.
        const REMOTE: &[(&str, &str)] = &[
            ("create_vospace_folder", "path"),
            ("delete_vospace_node", "path"),
            ("download_vospace_file", "path"),
            ("get_vospace_node", "path"),
            ("read_vospace_file", "path"),
            ("set_vospace_acl", "path"),
            ("upload_file_to_vospace", "path"),
            ("upload_text_to_vospace", "path"),
        ];
        // Caught by the name scan below without being a path at all.
        const NOT_A_PATH: &[(&str, &str)] = &[
            ("export_research_bundle", "includeFiles"),
            ("move_cell", "direction"),
        ];

        let classified: std::collections::HashSet<(&str, &str)> = LOCAL
            .iter()
            .chain(REMOTE)
            .chain(NOT_A_PATH)
            .copied()
            .collect();

        let mut unclassified = Vec::new();
        let mut seen = 0usize;
        for d in crate::mcp::tools::family_descriptors() {
            let Some(props) = d.input_schema.get("properties").and_then(|p| p.as_object()) else {
                continue;
            };
            for name in props.keys() {
                let lname = name.to_ascii_lowercase();
                if !(lname.contains("path") || lname.contains("file") || lname.contains("dir")) {
                    continue;
                }
                seen += 1;
                if !classified.contains(&(d.name.as_str(), name.as_str())) {
                    unclassified.push(format!("{}::{name}", d.name));
                }
            }
        }

        unclassified.sort();
        assert!(
            unclassified.is_empty(),
            "path argument(s) nobody has said are local or remote. If local, the \
             handler must call `reject_remote`; then list it here: {unclassified:#?}"
        );
        assert!(seen >= 20, "only {seen} path arguments found — scan broken");

        // A local path argument says so where the caller reads it. The error
        // message is the second line of defence; the description is the first,
        // and "Destination file path" gave a caller working in VOSpace no
        // reason to think it meant anything else.
        let described: std::collections::HashMap<(String, String), String> =
            crate::mcp::tools::family_descriptors()
                .into_iter()
                .flat_map(|d| {
                    let props = d
                        .input_schema
                        .get("properties")
                        .and_then(|p| p.as_object())
                        .cloned()
                        .unwrap_or_default();
                    props.into_iter().map(move |(name, spec)| {
                        let desc = spec
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        ((d.name.clone(), name), desc)
                    })
                })
                .collect();

        let mut silent = Vec::new();
        for (tool, prop) in LOCAL {
            let key = (tool.to_string(), prop.to_string());
            let desc = described.get(&key).map(String::as_str).unwrap_or("");
            if !desc.to_ascii_lowercase().contains("local") {
                silent.push(format!("{tool}::{prop} — {desc:?}"));
            }
        }
        assert!(
            silent.is_empty(),
            "local path argument(s) whose description never says so: {silent:#?}"
        );
    }

    /// The local list above is a claim about handlers; this counts the checks.
    ///
    /// Asking only whether a FILE mentions `reject_remote` is not enough:
    /// `vospace.rs`, `cube_tab_host.rs` and `notebook_host.rs` each enforce it
    /// twice, and deleting one of the two would leave the file still
    /// mentioning it and the test still green — the same hole that let a
    /// settings guard pass with a field's hint removed.
    ///
    /// So the count is pinned. Moving a call between files fails this too;
    /// the message says so, because a deliberate move is a one-line edit here
    /// and a silent deletion is the thing worth catching.
    #[test]
    fn every_local_path_handler_calls_the_check() {
        // file → how many enforcement points it holds.
        const ENFORCEMENT: &[(&str, usize)] = &[
            // save_notebook + open_notebook
            ("src/ui/notebook_host.rs", 2),
            // download_vospace_file + upload_file_to_vospace
            ("src/mcp/tools/vospace.rs", 2),
            // export_cube_figure + open_cube
            ("src/ui/cube_tab_host.rs", 2),
            // export_search_results
            ("src/ui/search_page/mcp.rs", 1),
            // require_path, the funnel for get_fits_header and get_fits_wcs
            ("src/mcp/tools/fits.rs", 1),
            // export_zip_path, the funnel for `path` and `destFolder`
            ("src/mcp/tools/research.rs", 1),
            // open_fits_file
            ("src/mcp/tools/viewstate.rs", 1),
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut wrong = Vec::new();
        for (file, expected) in ENFORCEMENT {
            let text =
                std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            let code = crate::testing::without_comments(crate::testing::code(&text));
            let found = code.matches("reject_remote(").count();
            if found != *expected {
                wrong.push(format!("{file}: {found} check(s), expected {expected}"));
            }
        }

        assert!(
            wrong.is_empty(),
            "local-path check(s) missing — every tool listed in LOCAL must refuse \
             a remote path. If you moved one, update this table: {wrong:#?}"
        );

        // Every tool in LOCAL is handled in one of those files, so a tool added
        // to the list without a home would otherwise be enforced by nobody.
        let covered: usize = ENFORCEMENT.iter().map(|(_, n)| n).sum();
        assert!(
            covered >= 10,
            "only {covered} enforcement points for 11 local-path tools"
        );
    }
}
