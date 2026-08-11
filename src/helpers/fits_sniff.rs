//! Cheap FITS shape sniff — is this file a spectral cube or a 2D image?
//!
//! Port of `Helpers/FitsSniff.cs`. Used right after a download / before opening
//! to pick the viewer to suggest (Cube Viewer vs FITS Viewer). Reads only header
//! metadata (via the existing cfitsio helpers), never pixels.
//!
//! Any trouble returns [`FitsKind::NotFits`]. That is the conservative answer,
//! not merely the ignorant one: callers read anything else as "this IS a FITS
//! file" and act on it — registering it in the Research library as a science
//! observation and routing it to a viewer.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitsKind {
    NotFits,
    Image2D,
    Cube,
}

/// The shape a file presents to the viewers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FitsShape {
    pub kind: FitsKind,
    /// The third axis is spectral (CTYPE3 = FREQ/WAVE/VELO/…).
    pub is_spectral: bool,
}

impl FitsShape {
    /// A real third image axis (NAXIS3 > 1) exists — the Cube Viewer CAN open it.
    pub fn has_cube_axis(&self) -> bool {
        self.kind == FitsKind::Cube
    }

    /// The cube axis is spectral — the Cube Viewer is the RIGHT default. A detector
    /// stack (no spectral CTYPE3) is a cube by shape but best viewed in 2D.
    pub fn recommend_cube(&self) -> bool {
        self.kind == FitsKind::Cube && self.is_spectral
    }
}

/// FITS WCS Paper III spectral algorithm codes (CTYPE3 prefix). CGPS uses `VELO-LSR`.
const SPECTRAL_CODES: &[&str] = &[
    "FREQ", "ENER", "WAVN", "VRAD", "WAVE", "VOPT", "ZOPT", "AWAV", "VELO", "BETA", "FELO",
    "VELOCITY",
];

fn is_spectral_axis(ctype3: &str) -> bool {
    let up = ctype3.trim().trim_matches('\'').trim().to_ascii_uppercase();
    SPECTRAL_CODES.iter().any(|c| up.starts_with(c))
}

/// Inspect a file's shape. `NotFits` on any parse trouble.
#[cfg(feature = "fits")]
pub fn inspect(path: &Path) -> FitsShape {
    let hdus = match crate::helpers::fits_loader::list_hdus(path) {
        Ok(h) => h,
        Err(_) => {
            return FitsShape {
                kind: FitsKind::NotFits,
                is_spectral: false,
            }
        }
    };
    let has_cube = hdus.iter().any(|h| h.is_image && h.depth >= 2);
    let has_image = hdus.iter().any(|h| h.is_image);
    if has_cube {
        let is_spectral = crate::helpers::cube_loader::cube_header(path)
            .ok()
            .and_then(|hm| hm.get("CTYPE3").cloned())
            .map(|c| is_spectral_axis(&c))
            .unwrap_or(false);
        FitsShape {
            kind: FitsKind::Cube,
            is_spectral,
        }
    } else if has_image {
        FitsShape {
            kind: FitsKind::Image2D,
            is_spectral: false,
        }
    } else {
        FitsShape {
            kind: FitsKind::NotFits,
            is_spectral: false,
        }
    }
}

/// Without cfitsio compiled in, nothing can be determined — and nothing can be
/// opened either, so say so.
///
/// This used to answer `Image2D`, described as the "safe suggestion". It was the
/// opposite: callers treat anything that is not `NotFits` as a FITS file, so a
/// downloaded preview PNG or README was registered in the Research library as a
/// science observation and offered to a viewer that could not open it.
/// `NotFits` routes the file to the OS-default open, which is the only thing
/// that actually works in this build.
#[cfg(not(feature = "fits"))]
pub fn inspect(_path: &Path) -> FitsShape {
    FitsShape {
        kind: FitsKind::NotFits,
        is_spectral: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file that is not FITS — or that we cannot inspect — must never be
    /// reported as one.
    ///
    /// Callers treat any kind other than `NotFits` as "this IS a FITS file":
    /// they register it in the Research library as a science observation and
    /// offer it to a viewer. Answering `Image2D` when nothing was actually read
    /// therefore filed preview PNGs and READMEs as science data.
    #[test]
    fn a_file_that_cannot_be_inspected_is_not_claimed_as_fits() {
        // A path that does not exist stands in for "nothing could be read",
        // which is also exactly what the no-cfitsio build always faces.
        let shape = inspect(Path::new("/nonexistent/not-a-file.bin"));
        assert_eq!(shape.kind, FitsKind::NotFits);
        assert!(!shape.has_cube_axis());
        assert!(!shape.recommend_cube());
    }

    #[test]
    fn spectral_axis_detection() {
        assert!(is_spectral_axis("FREQ"));
        assert!(is_spectral_axis("'VELO-LSR'"));
        assert!(is_spectral_axis("WAVE-F2W"));
        assert!(!is_spectral_axis("RA---TAN"));
        assert!(!is_spectral_axis("DETECTOR"));
    }

    #[test]
    fn shape_predicates() {
        let cube = FitsShape {
            kind: FitsKind::Cube,
            is_spectral: true,
        };
        assert!(cube.has_cube_axis());
        assert!(cube.recommend_cube());
        let stack = FitsShape {
            kind: FitsKind::Cube,
            is_spectral: false,
        };
        assert!(stack.has_cube_axis());
        assert!(!stack.recommend_cube());
    }
}
