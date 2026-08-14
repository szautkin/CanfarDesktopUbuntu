//! Telling a local path from a remote one, before something is written to the
//! wrong filesystem.
//!
//! Tools that write to the local disk and tools that write to VOSpace both take
//! a `path`, and a caller that has been working in VOSpace will reach for the
//! VOSpace path. `save_notebook` did exactly that: it created a local directory
//! tree named after the remote location, reported success with a `filePath` the
//! caller recognised, and left VOSpace empty.

/// Schemes that mean "not on this machine".
const REMOTE_PREFIXES: &[&str] = &["vos:", "arc:", "ivo:", "http://", "https://"];

/// `Err` when `path` names somewhere other than the local filesystem.
///
/// The message names the tool that does write there, because a caller that has
/// just been told "no" needs to know what "yes" looks like.
pub fn reject_remote(path: &str) -> Result<(), String> {
    let p = path.trim();
    if let Some(scheme) = REMOTE_PREFIXES
        .iter()
        .find(|s| p.to_ascii_lowercase().starts_with(**s))
    {
        return Err(format!(
            "`{p}` is a {scheme} location, and this tool writes to the local filesystem. \
             Save locally first, then upload it with upload_vospace_file."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::reject_remote;

    #[test]
    fn a_local_path_is_accepted() {
        assert!(reject_remote("/home/alice/work/run.ipynb").is_ok());
        assert!(reject_remote("relative/run.ipynb").is_ok());
    }

    #[test]
    fn a_remote_path_is_refused_and_says_what_to_use() {
        for remote in [
            "vos://cadc.nrc.ca~arc/home/alice/run.ipynb",
            "arc:/home/alice/run.ipynb",
            "https://example.org/run.ipynb",
        ] {
            let err = reject_remote(remote).expect_err(remote);
            assert!(err.contains("upload_vospace_file"), "{err}");
        }
    }

    #[test]
    fn the_scheme_is_recognised_whatever_its_case() {
        assert!(reject_remote("VOS://cadc.nrc.ca~arc/x.ipynb").is_err());
    }
}
