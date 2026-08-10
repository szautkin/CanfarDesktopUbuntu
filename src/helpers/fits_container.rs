//! Transparently unwrap the tar containers CADC sometimes serves around FITS
//! products.
//!
//! Mirrors CanfarDesktop's `Services/Fits/FitsContainer.cs`: a CADC
//! "download all" bundle is a **tar** archive (optionally **gzip**-compressed,
//! `.tar.gz` / `.tgz`) whose members are the individual FITS products.  When
//! such a bundle is saved with a FITS-looking name and fed straight to the
//! parser, the tar header is read as FITS cards, no `END` card is ever found,
//! and the load fails with a misleading error.
//!
//! [`resolve_fits_path`] detects a tar container by its file extension,
//! extracts the first FITS member into a temporary directory, and hands back
//! the extracted path.  Plain FITS inputs (`.fits`, `.fits.gz`, `.fits.fz`)
//! are returned unchanged because CFITSIO already opens those directly
//! (transparent gzip / tile decompression included).

use std::fs::File;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// A possibly-wrapped FITS input resolved to a path CFITSIO can open directly.
///
/// When the input was a tar / `.tar.gz` / `.tgz` archive, [`path`](Self::path)
/// points at a file extracted into [`_tempdir`](Self::_tempdir); that
/// `TempDir` must stay alive for as long as the caller needs `path`, because
/// dropping it deletes the extracted file.  For a plain-FITS pass-through,
/// `_tempdir` is `None` and `path` is the original path.
#[derive(Debug)]
pub struct ResolvedFits {
    /// A path CFITSIO can open directly.
    pub path: PathBuf,
    /// Keeps the extraction directory alive; `None` for a pass-through.
    pub _tempdir: Option<TempDir>,
}

/// Resolve `path` to a usable FITS path, unwrapping a surrounding tar
/// container when present.
///
/// * `*.tar`, `*.tar.gz`, `*.tgz` — open the archive (gzip-decompressing when
///   needed), extract the **first** member whose name ends in a FITS
///   extension into a fresh temp dir, and return that path plus the owning
///   `TempDir`.
/// * anything else — returned unchanged (CFITSIO handles `.fits` / `.gz` /
///   `.fz` natively).
pub fn resolve_fits_path(path: &Path) -> Result<ResolvedFits, String> {
    match tar_kind(path) {
        Some(is_gzip) => extract_first_fits_member(path, is_gzip),
        None => Ok(ResolvedFits {
            path: path.to_path_buf(),
            _tempdir: None,
        }),
    }
}

/// Classify `path` by its container extension.
///
/// Returns `Some(true)` for a gzip-wrapped tar (`.tar.gz` / `.tgz`),
/// `Some(false)` for a plain `.tar`, and `None` for everything else.
fn tar_kind(path: &Path) -> Option<bool> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(true)
    } else if name.ends_with(".tar") {
        Some(false)
    } else {
        None
    }
}

/// True when `name` (a tar member name) ends in a FITS-image extension.
fn is_fits_member_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower.ends_with(".fits")
        || lower.ends_with(".fit")
        || lower.ends_with(".fts")
        || lower.ends_with(".fz")
}

/// Open a tar archive and extract its first FITS member into a temp dir.
fn extract_first_fits_member(path: &Path, is_gzip: bool) -> Result<ResolvedFits, String> {
    let file =
        File::open(path).map_err(|e| format!("Cannot open archive '{}': {}", path.display(), e))?;
    if is_gzip {
        extract_from_reader(flate2::read::GzDecoder::new(file), path)
    } else {
        extract_from_reader(file, path)
    }
}

/// Walk `reader` as a tar stream, copy the first FITS member into a fresh
/// temp dir, and return the extracted path plus its owning `TempDir`.
fn extract_from_reader<R: std::io::Read>(
    reader: R,
    archive_path: &Path,
) -> Result<ResolvedFits, String> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|e| {
        format!(
            "Cannot read tar archive '{}': {}",
            archive_path.display(),
            e
        )
    })?;

    let mut member_count = 0usize;
    for entry in entries {
        let mut entry = entry.map_err(|e| {
            format!(
                "Cannot read tar entry in '{}': {}",
                archive_path.display(),
                e
            )
        })?;

        // Regular files only (matches TarEntryType.RegularFile / V7RegularFile).
        if !entry.header().entry_type().is_file() {
            continue;
        }
        member_count += 1;

        let name = entry
            .path()
            .map_err(|e| {
                format!(
                    "Cannot read tar entry name in '{}': {}",
                    archive_path.display(),
                    e
                )
            })?
            .to_string_lossy()
            .into_owned();
        if !is_fits_member_name(&name) {
            continue;
        }

        // Extract into a fresh temp dir.  Use only the member's base file name
        // (never a `..`-bearing archive path) so the extension survives — some
        // CFITSIO behaviour keys off `.fz` / `.gz` — without any traversal risk.
        let temp_dir = tempfile::tempdir()
            .map_err(|e| format!("Cannot create temp dir for FITS extraction: {}", e))?;
        let out_name = Path::new(&name)
            .file_name()
            .map(|n| n.to_owned())
            .unwrap_or_else(|| std::ffi::OsString::from("extracted.fits"));
        let out_path = temp_dir.path().join(&out_name);

        let mut out_file = File::create(&out_path).map_err(|e| {
            format!(
                "Cannot create extracted FITS file '{}': {}",
                out_path.display(),
                e
            )
        })?;
        std::io::copy(&mut entry, &mut out_file)
            .map_err(|e| format!("Cannot extract FITS member '{}': {}", name, e))?;
        drop(out_file);

        return Ok(ResolvedFits {
            path: out_path,
            _tempdir: Some(temp_dir),
        });
    }

    Err(if member_count == 0 {
        format!(
            "Archive '{}' is empty — there is no FITS file to open.",
            archive_path.display()
        )
    } else {
        format!(
            "Archive '{}' contains no .fits file to open.",
            archive_path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Write a `.tar.gz` at `archive_path` holding `members` in order.
    fn write_tar_gz(archive_path: &Path, members: &[(&str, &[u8])]) {
        let file = File::create(archive_path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        for (name, contents) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *contents).unwrap();
        }
        // Finish the tar, then finish the gzip stream.
        builder.into_inner().unwrap().finish().unwrap();
    }

    /// Write a plain `.tar` at `archive_path` holding `members` in order.
    fn write_tar(archive_path: &Path, members: &[(&str, &[u8])]) {
        let file = File::create(archive_path).unwrap();
        let mut builder = tar::Builder::new(file);
        for (name, contents) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *contents).unwrap();
        }
        // `into_inner` writes the tar trailer and returns the inner File.
        builder.into_inner().unwrap();
    }

    fn read_all(path: &Path) -> Vec<u8> {
        let mut buf = Vec::new();
        File::open(path).unwrap().read_to_end(&mut buf).unwrap();
        buf
    }

    #[test]
    fn tar_kind_recognises_extensions() {
        assert_eq!(tar_kind(Path::new("bundle.tar.gz")), Some(true));
        assert_eq!(tar_kind(Path::new("BUNDLE.TGZ")), Some(true));
        assert_eq!(tar_kind(Path::new("bundle.tar")), Some(false));
        assert_eq!(tar_kind(Path::new("image.fits")), None);
        assert_eq!(tar_kind(Path::new("image.fits.fz")), None);
        assert_eq!(tar_kind(Path::new("image.fits.gz")), None);
    }

    #[test]
    fn fits_member_name_matching() {
        assert!(is_fits_member_name("a.fits"));
        assert!(is_fits_member_name("A.FIT"));
        assert!(is_fits_member_name("x.fts"));
        assert!(is_fits_member_name("prod_flt.fits.fz"));
        assert!(is_fits_member_name("prod.fz"));
        assert!(!is_fits_member_name("readme.txt"));
        assert!(!is_fits_member_name("catalog.csv"));
    }

    #[test]
    fn unwraps_tar_gz_with_fits_member() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bundle.tar.gz");
        let fits_bytes: &[u8] = b"SIMPLE  =                    T / fake fits image\nEND";
        // A leading non-FITS member verifies we skip past it to the first FITS.
        write_tar_gz(
            &archive,
            &[
                ("HST/readme.txt", b"not fits"),
                ("HST/product/x_flt.fits", fits_bytes),
            ],
        );

        let resolved = resolve_fits_path(&archive).expect("should unwrap");
        assert!(resolved._tempdir.is_some(), "tempdir must be held alive");
        assert!(resolved.path.exists(), "extracted file must exist");
        assert_eq!(
            resolved.path.file_name().unwrap().to_str().unwrap(),
            "x_flt.fits",
            "base name (extension) must be preserved"
        );
        assert_eq!(read_all(&resolved.path), fits_bytes);

        // Dropping the ResolvedFits deletes the temp dir with the file.
        let extracted = resolved.path.clone();
        drop(resolved);
        assert!(
            !extracted.exists(),
            "temp file should be cleaned up on drop"
        );
    }

    #[test]
    fn unwraps_plain_tar() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bundle.tar");
        let fits_bytes: &[u8] = b"SIMPLE  =                    T\nEND";
        write_tar(&archive, &[("image.fits", fits_bytes)]);

        let resolved = resolve_fits_path(&archive).expect("should unwrap plain tar");
        assert!(resolved._tempdir.is_some());
        assert_eq!(read_all(&resolved.path), fits_bytes);
    }

    #[test]
    fn plain_fits_path_passes_through() {
        let p = Path::new("/some/dir/image.fits");
        let resolved = resolve_fits_path(p).expect("plain path should pass through");
        assert_eq!(resolved.path, p);
        assert!(resolved._tempdir.is_none(), "no temp dir for pass-through");

        // .fits.fz / .fits.gz are handled natively by CFITSIO — also pass-through.
        for name in ["/d/x.fits.fz", "/d/x.fits.gz", "/d/x.fz"] {
            let r = resolve_fits_path(Path::new(name)).unwrap();
            assert_eq!(r.path, Path::new(name));
            assert!(r._tempdir.is_none());
        }
    }

    #[test]
    fn archive_without_fits_member_errors() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bundle.tar.gz");
        write_tar_gz(&archive, &[("readme.txt", b"nope"), ("data.csv", b"a,b")]);

        let err = resolve_fits_path(&archive).unwrap_err();
        assert!(err.contains("no .fits file"), "unexpected error: {err}");
    }
}
