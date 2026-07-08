//! Cheap FITS shape sniff — is this file a spectral cube or a 2D image?
//!
//! Port of `Helpers/FitsSniff.cs`. Used right after a download / before opening
//! to pick the viewer to suggest (Cube Viewer vs FITS Viewer). Reads only header
//! metadata (via the existing cfitsio helpers), never pixels. Any trouble returns
//! the 2D image as the safe default.

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

/// Without cfitsio compiled in, default to the 2D image (safe suggestion).
#[cfg(not(feature = "fits"))]
pub fn inspect(_path: &Path) -> FitsShape {
    FitsShape {
        kind: FitsKind::Image2D,
        is_spectral: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
