//! State + edit rules for the opacity transfer-function editor.
//!
//! One-to-one port of CanfarDesktop `Services/CubeViewer/TransferFunctionModel.cs`.
//! Control points map value `x` ∈ [0,1] → alpha `y` ∈ [0,1] with add / drag /
//! remove semantics, feeding [`crate::helpers::cube_colormaps::transfer_ramp`].
//!
//! The two extreme-X endpoints are *pinned in X* (they move only in alpha) so the
//! curve always spans the full [0,1] domain. Pure (no GTK) so it is unit-testable.

use crate::helpers::cube_colormaps;

/// Opacity transfer function: a set of `(x, alpha)` control points in [0,1]².
///
/// Points are stored unsorted; [`ramp`](TransferFunctionModel::ramp) sorts them.
pub struct TransferFunctionModel {
    /// Control points `(x in [0,1], alpha in [0,1])`. Endpoints (min/max X) are pinned in X.
    pub points: Vec<(f32, f32)>,
}

impl Default for TransferFunctionModel {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferFunctionModel {
    /// A minimal editor: just the two X-pinned endpoints spanning [0,1] (a linear ramp).
    pub fn new() -> Self {
        Self {
            points: vec![(0.0, 0.0), (1.0, 1.0)],
        }
    }

    /// A fresh editor seeded with the renderer's default ramp
    /// ([`cube_colormaps::DEFAULT_TRANSFER`]).
    pub fn default_ramp() -> Self {
        Self {
            points: cube_colormaps::DEFAULT_TRANSFER.to_vec(),
        }
    }

    /// Index of the point with the smallest X (the left endpoint). 0 if empty.
    fn min_x_index(&self) -> usize {
        let mut m = 0;
        for i in 1..self.points.len() {
            if self.points[i].0 < self.points[m].0 {
                m = i;
            }
        }
        m
    }

    /// Index of the point with the largest X (the right endpoint). 0 if empty.
    fn max_x_index(&self) -> usize {
        let mut m = 0;
        for i in 1..self.points.len() {
            if self.points[i].0 > self.points[m].0 {
                m = i;
            }
        }
        m
    }

    /// Whether a point is one of the two X-pinned endpoints.
    pub fn is_endpoint(&self, index: usize) -> bool {
        if self.points.is_empty() {
            return false;
        }
        index == self.min_x_index() || index == self.max_x_index()
    }

    /// Nearest point within `radius` of `(x, y)` — all in normalized [0,1] space.
    /// Returns `None` when nothing is in range.
    ///
    /// Port of the C# `HitTest`; there the radius is elliptical `(rx, ry)` so a
    /// circular pixel target can be expressed on a non-square canvas — here the
    /// shared contract uses a single (circular) `radius`.
    pub fn hit_test(&self, x: f32, y: f32, radius: f32) -> Option<usize> {
        if radius <= 0.0 {
            return None;
        }
        let r2 = radius * radius;
        let mut best: Option<usize> = None;
        let mut best_d = f32::MAX;
        for (i, &(px, py)) in self.points.iter().enumerate() {
            let dx = px - x;
            let dy = py - y;
            let d = dx * dx + dy * dy;
            if d <= r2 && d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    }

    /// Move a point to `(x, y)`, clamped to [0,1]; endpoints are locked in X.
    pub fn drag(&mut self, index: usize, x: f32, y: f32) {
        if index >= self.points.len() {
            return;
        }
        let nx = if self.is_endpoint(index) {
            self.points[index].0
        } else {
            x.clamp(0.0, 1.0)
        };
        self.points[index] = (nx, y.clamp(0.0, 1.0));
    }

    /// Add a control point at `(x, y)`, clamped to [0,1].
    pub fn add(&mut self, x: f32, y: f32) {
        self.points.push((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
    }

    /// Remove a control point. Refused (`false`) for endpoints so the curve keeps
    /// spanning [0,1].
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.points.len() || self.is_endpoint(index) {
            return false;
        }
        self.points.remove(index);
        true
    }

    /// Restore the default curve ([`cube_colormaps::DEFAULT_TRANSFER`]).
    pub fn reset(&mut self) {
        self.points.clear();
        self.points
            .extend_from_slice(cube_colormaps::DEFAULT_TRANSFER);
    }

    /// Build a 256-entry alpha ramp from the current control points.
    pub fn ramp(&self) -> [u8; 256] {
        cube_colormaps::transfer_ramp(&self.points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_spans_full_domain_with_pinned_endpoints() {
        let m = TransferFunctionModel::new();
        assert_eq!(m.points.len(), 2);
        assert_eq!(m.min_x_index(), 0);
        assert_eq!(m.max_x_index(), 1);
        assert!(m.is_endpoint(0));
        assert!(m.is_endpoint(1));
    }

    #[test]
    fn add_appends_clamped_point_between_endpoints() {
        let mut m = TransferFunctionModel::new();
        m.add(0.5, 0.5);
        assert_eq!(m.points.len(), 3);
        // The new interior point is not an endpoint.
        assert!(!m.is_endpoint(2));
        // Endpoints are still the extremes at X=0 and X=1.
        assert!(m.is_endpoint(0));
        assert!(m.is_endpoint(1));
    }

    #[test]
    fn add_clamps_out_of_range_coords() {
        let mut m = TransferFunctionModel::new();
        m.add(2.0, -1.0);
        assert_eq!(m.points[2], (1.0, 0.0));
        m.add(-3.0, 5.0);
        assert_eq!(m.points[3], (0.0, 1.0));
    }

    #[test]
    fn remove_refuses_endpoints() {
        let mut m = TransferFunctionModel::new();
        // Both points are endpoints -> neither can be removed.
        assert!(!m.remove(0));
        assert!(!m.remove(1));
        assert_eq!(m.points.len(), 2);
    }

    #[test]
    fn remove_interior_point_succeeds() {
        let mut m = TransferFunctionModel::new();
        m.add(0.5, 0.5); // index 2, interior
        assert!(m.remove(2));
        assert_eq!(m.points.len(), 2);
        // Out-of-range index refused.
        assert!(!m.remove(99));
    }

    #[test]
    fn remove_reassigns_endpoint_after_deletion() {
        // Points: (0,0)=endpoint, (0.3,..)=interior, (0.7,..)=interior, (1,1)=endpoint.
        let mut m = TransferFunctionModel {
            points: vec![(0.0, 0.0), (0.3, 0.3), (0.7, 0.7), (1.0, 1.0)],
        };
        // The (0.7) point at index 2 is interior and removable.
        assert!(m.remove(2));
        assert_eq!(m.points.len(), 3);
        // Endpoints remain X=0 (idx 0) and X=1 (idx 2).
        assert!(m.is_endpoint(0));
        assert!(m.is_endpoint(2));
        assert!(!m.is_endpoint(1));
    }

    #[test]
    fn drag_endpoint_pins_x_moves_alpha() {
        let mut m = TransferFunctionModel::new(); // (0,0),(1,1)
                                                  // Try to drag the left endpoint's X to 0.5 -> X stays 0, alpha updates.
        m.drag(0, 0.5, 0.8);
        assert_eq!(m.points[0], (0.0, 0.8));
        // Right endpoint pinned at X=1.
        m.drag(1, 0.2, 0.1);
        assert_eq!(m.points[1], (1.0, 0.1));
    }

    #[test]
    fn drag_interior_moves_and_clamps_both_axes() {
        let mut m = TransferFunctionModel::new();
        m.add(0.5, 0.5); // idx 2
        m.drag(2, 0.4, 0.9);
        assert_eq!(m.points[2], (0.4, 0.9));
        // Clamp out-of-range drag.
        m.drag(2, 5.0, -2.0);
        assert_eq!(m.points[2], (1.0, 0.0));
    }

    #[test]
    fn drag_out_of_range_index_is_noop() {
        let mut m = TransferFunctionModel::new();
        m.drag(42, 0.5, 0.5);
        assert_eq!(m.points.len(), 2);
    }

    #[test]
    fn hit_test_finds_nearest_within_radius() {
        let mut m = TransferFunctionModel::new();
        m.add(0.5, 0.5); // idx 2
                         // Close to the interior point.
        assert_eq!(m.hit_test(0.51, 0.49, 0.05), Some(2));
        // Nothing within a tiny radius near an empty region.
        assert_eq!(m.hit_test(0.2, 0.8, 0.01), None);
        // Non-positive radius never hits.
        assert_eq!(m.hit_test(0.5, 0.5, 0.0), None);
    }

    #[test]
    fn hit_test_picks_closest_of_several() {
        let m = TransferFunctionModel {
            points: vec![(0.0, 0.0), (0.48, 0.5), (0.52, 0.5), (1.0, 1.0)],
        };
        // (0.5, 0.5) is nearer to idx 1 (0.48) than idx 2 (0.52)? equal dist; pick first found.
        assert_eq!(m.hit_test(0.49, 0.5, 0.2), Some(1));
        assert_eq!(m.hit_test(0.51, 0.5, 0.2), Some(2));
    }

    #[test]
    fn reset_restores_default_curve() {
        let mut m = TransferFunctionModel::new();
        m.add(0.5, 0.5);
        m.reset();
        assert_eq!(m.points, cube_colormaps::DEFAULT_TRANSFER.to_vec());
    }

    #[test]
    fn ramp_is_monotonic_nondecreasing_for_default() {
        let m = TransferFunctionModel::default_ramp();
        let r = m.ramp();
        assert_eq!(r.len(), 256);
        for i in 1..256 {
            assert!(r[i] >= r[i - 1], "ramp should be non-decreasing");
        }
        assert_eq!(r[0], 0);
    }
}
