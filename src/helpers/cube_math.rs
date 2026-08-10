//! Column-major 4x4 matrix helpers for the cube volume camera.
//!
//! One-to-one port of `CanfarDesktop/Services/CubeViewer/CubeMath.cs`, itself a
//! port of the macOS `makePerspective`/`makeLookAt` from `CubeVolumeRenderer.swift`.
//!
//! The **only** intentional deviation from the C# reference is the clip-space
//! depth range of [`perspective`]: Metal and Direct3D use z ∈ [0, 1], but this
//! renderer targets OpenGL (via GTK's `GLArea`), whose clip-space depth is
//! z ∈ [-1, 1]. The perspective matrix here is adapted to the GL convention;
//! every other matrix is identical to the reference.
//!
//! Storage is **column-major** (OpenGL / glam convention): the element at
//! mathematical row `r`, column `c` lives at flat index `c * 4 + r`. Thus a
//! matrix's four columns occupy contiguous 4-element slices `m[0..4]`,
//! `m[4..8]`, `m[8..12]`, `m[12..16]`. This is exactly the layout a GL
//! `uniformMatrix4fv(..., transpose = GL_FALSE, ...)` expects, so the CPU math
//! matches the GPU with no transpose anywhere in the pipeline.
//!
//! All matrices use the **column-vector** convention: transforming a point is
//! `M · v`, and composition `proj * view * model` is `mul(proj, mul(view, model))`.

/// Column-major 4x4 matrix. Element (row `r`, col `c`) is at index `c * 4 + r`.
pub type Mat4 = [f32; 16];

/// The 4x4 identity matrix.
pub fn identity() -> Mat4 {
    [
        1.0, 0.0, 0.0, 0.0, // col 0
        0.0, 1.0, 0.0, 0.0, // col 1
        0.0, 0.0, 1.0, 0.0, // col 2
        0.0, 0.0, 0.0, 1.0, // col 3
    ]
}

/// Right-handed perspective projection (column-vector convention, **GL clip
/// depth z ∈ [-1, 1]**).
///
/// Adapted from the Swift/C# `makePerspective` (which targets Metal/D3D
/// z ∈ [0, 1]). The `xs`/`ys` focal terms and the `max(aspect, 1e-4)` guard are
/// preserved exactly; only the third row's depth-mapping terms are changed to
/// the OpenGL `(f+n)/(n-f)` / `2fn/(n-f)` form.
///
/// * `fovy_rad` — vertical field of view, radians.
/// * `aspect` — viewport width / height.
/// * `near` — near clip distance (> 0).
/// * `far` — far clip distance.
pub fn perspective(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let ys = 1.0 / (fovy_rad * 0.5).tan();
    let xs = ys / aspect.max(0.0001);
    // OpenGL right-handed depth mapping (eye z<0 in front → clip z ∈ [-1, 1]).
    let a = (far + near) / (near - far); // row 2, col 2
    let b = (2.0 * far * near) / (near - far); // row 2, col 3

    // Columns (column-major, contiguous):
    //   col0=(xs,0,0,0) col1=(0,ys,0,0) col2=(0,0,a,-1) col3=(0,0,b,0)
    [
        xs, 0.0, 0.0, 0.0, // col 0
        0.0, ys, 0.0, 0.0, // col 1
        0.0, 0.0, a, -1.0, // col 2
        0.0, 0.0, b, 0.0, // col 3
    ]
}

/// Right-handed look-at view matrix (column-vector convention). Direct port of
/// the Swift/C# `makeLookAt`.
pub fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> Mat4 {
    let z = normalize(sub(eye, center));
    let x = normalize(cross(up, z));
    let y = cross(z, x);

    // Columns:
    //   col0=(x.x,y.x,z.x,0) col1=(x.y,y.y,z.y,0)
    //   col2=(x.z,y.z,z.z,0)  col3=(-dot(x,eye),-dot(y,eye),-dot(z,eye),1)
    [
        x[0],
        y[0],
        z[0],
        0.0, // col 0
        x[1],
        y[1],
        z[1],
        0.0, // col 1
        x[2],
        y[2],
        z[2],
        0.0, // col 2
        -dot(x, eye),
        -dot(y, eye),
        -dot(z, eye),
        1.0, // col 3
    ]
}

/// Diagonal scale matrix (the cube model matrix = spatial/spectral scale).
pub fn scale(sx: f32, sy: f32, sz: f32) -> Mat4 {
    [
        sx, 0.0, 0.0, 0.0, // col 0
        0.0, sy, 0.0, 0.0, // col 1
        0.0, 0.0, sz, 0.0, // col 2
        0.0, 0.0, 0.0, 1.0, // col 3
    ]
}

/// Translation matrix (column-vector convention: translation lives in column 3).
pub fn translate(x: f32, y: f32, z: f32) -> Mat4 {
    [
        1.0, 0.0, 0.0, 0.0, // col 0
        0.0, 1.0, 0.0, 0.0, // col 1
        0.0, 0.0, 1.0, 0.0, // col 2
        x, y, z, 1.0, // col 3
    ]
}

/// Multiply two true-math matrices: `result = a · b` (so `result · v = a · (b · v)`),
/// matching the Swift `proj * view * model` ordering. Column-major indexing.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            // result(row,col) = Σ_k a(row,k) * b(k,col)
            r[col * 4 + row] = a[row] * b[col * 4]
                + a[4 + row] * b[col * 4 + 1]
                + a[8 + row] * b[col * 4 + 2]
                + a[12 + row] * b[col * 4 + 3];
        }
    }
    r
}

/// Invert a true-math matrix; returns `None` if the matrix is singular
/// (degenerate camera). Full analytic 4x4 cofactor inverse (Mesa
/// `gluInvertMatrix`), computed in `f64` for numerical stability, then narrowed.
pub fn invert(m: &Mat4) -> Option<Mat4> {
    let s: [f64; 16] = std::array::from_fn(|i| m[i] as f64);
    let mut inv = [0.0f64; 16];

    inv[0] = s[5] * s[10] * s[15] - s[5] * s[11] * s[14] - s[9] * s[6] * s[15]
        + s[9] * s[7] * s[14]
        + s[13] * s[6] * s[11]
        - s[13] * s[7] * s[10];
    inv[4] = -s[4] * s[10] * s[15] + s[4] * s[11] * s[14] + s[8] * s[6] * s[15]
        - s[8] * s[7] * s[14]
        - s[12] * s[6] * s[11]
        + s[12] * s[7] * s[10];
    inv[8] = s[4] * s[9] * s[15] - s[4] * s[11] * s[13] - s[8] * s[5] * s[15]
        + s[8] * s[7] * s[13]
        + s[12] * s[5] * s[11]
        - s[12] * s[7] * s[9];
    inv[12] = -s[4] * s[9] * s[14] + s[4] * s[10] * s[13] + s[8] * s[5] * s[14]
        - s[8] * s[6] * s[13]
        - s[12] * s[5] * s[10]
        + s[12] * s[6] * s[9];
    inv[1] = -s[1] * s[10] * s[15] + s[1] * s[11] * s[14] + s[9] * s[2] * s[15]
        - s[9] * s[3] * s[14]
        - s[13] * s[2] * s[11]
        + s[13] * s[3] * s[10];
    inv[5] = s[0] * s[10] * s[15] - s[0] * s[11] * s[14] - s[8] * s[2] * s[15]
        + s[8] * s[3] * s[14]
        + s[12] * s[2] * s[11]
        - s[12] * s[3] * s[10];
    inv[9] = -s[0] * s[9] * s[15] + s[0] * s[11] * s[13] + s[8] * s[1] * s[15]
        - s[8] * s[3] * s[13]
        - s[12] * s[1] * s[11]
        + s[12] * s[3] * s[9];
    inv[13] = s[0] * s[9] * s[14] - s[0] * s[10] * s[13] - s[8] * s[1] * s[14]
        + s[8] * s[2] * s[13]
        + s[12] * s[1] * s[10]
        - s[12] * s[2] * s[9];
    inv[2] = s[1] * s[6] * s[15] - s[1] * s[7] * s[14] - s[5] * s[2] * s[15]
        + s[5] * s[3] * s[14]
        + s[13] * s[2] * s[7]
        - s[13] * s[3] * s[6];
    inv[6] = -s[0] * s[6] * s[15] + s[0] * s[7] * s[14] + s[4] * s[2] * s[15]
        - s[4] * s[3] * s[14]
        - s[12] * s[2] * s[7]
        + s[12] * s[3] * s[6];
    inv[10] = s[0] * s[5] * s[15] - s[0] * s[7] * s[13] - s[4] * s[1] * s[15]
        + s[4] * s[3] * s[13]
        + s[12] * s[1] * s[7]
        - s[12] * s[3] * s[5];
    inv[14] = -s[0] * s[5] * s[14] + s[0] * s[6] * s[13] + s[4] * s[1] * s[14]
        - s[4] * s[2] * s[13]
        - s[12] * s[1] * s[6]
        + s[12] * s[2] * s[5];
    inv[3] = -s[1] * s[6] * s[11] + s[1] * s[7] * s[10] + s[5] * s[2] * s[11]
        - s[5] * s[3] * s[10]
        - s[9] * s[2] * s[7]
        + s[9] * s[3] * s[6];
    inv[7] = s[0] * s[6] * s[11] - s[0] * s[7] * s[10] - s[4] * s[2] * s[11]
        + s[4] * s[3] * s[10]
        + s[8] * s[2] * s[7]
        - s[8] * s[3] * s[6];
    inv[11] = -s[0] * s[5] * s[11] + s[0] * s[7] * s[9] + s[4] * s[1] * s[11]
        - s[4] * s[3] * s[9]
        - s[8] * s[1] * s[7]
        + s[8] * s[3] * s[5];
    inv[15] = s[0] * s[5] * s[10] - s[0] * s[6] * s[9] - s[4] * s[1] * s[10]
        + s[4] * s[2] * s[9]
        + s[8] * s[1] * s[6]
        - s[8] * s[2] * s[5];

    let det = s[0] * inv[0] + s[1] * inv[4] + s[2] * inv[8] + s[3] * inv[12];
    if det.abs() < 1e-20 || !det.is_finite() {
        return None;
    }
    let inv_det = 1.0 / det;
    Some(std::array::from_fn(|i| (inv[i] * inv_det) as f32))
}

/// Transform a 3D point through a matrix and apply the perspective divide,
/// returning the resulting coordinates (NDC when `m` is a view-projection).
///
/// The point is promoted to homogeneous `(x, y, z, 1)`, multiplied by `m`
/// (column-vector convention, matching the GPU's `mat * vec`), then divided by
/// the resulting `w`. This is the CPU analog the axis-caption overlay uses to
/// project box corners into normalized device coordinates with the exact same
/// matrix the GPU uses. If `w` is ~0 (point on the camera plane) the raw,
/// undivided coordinates are returned to avoid a division blow-up.
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let (px, py, pz) = (p[0], p[1], p[2]);
    // Row r of the column-major matrix is [m[r], m[4+r], m[8+r], m[12+r]].
    let x = m[0] * px + m[4] * py + m[8] * pz + m[12];
    let y = m[1] * px + m[5] * py + m[9] * pz + m[13];
    let z = m[2] * px + m[6] * py + m[10] * pz + m[14];
    let w = m[3] * px + m[7] * py + m[11] * pz + m[15];
    if w.abs() < 1e-6 {
        return [x, y, z];
    }
    let inv_w = 1.0 / w;
    [x * inv_w, y * inv_w, z * inv_w]
}

/// Camera eye position for an orbit camera about `center`. Port of the Swift
/// `cameraPosition()`: `offset = d·(cosEl·sinAz, sinEl, cosEl·cosAz)`, with the
/// orbit center added (the C# reference always orbits the origin; this port
/// generalizes it so `center` may be non-zero, e.g. a re-centered volume).
///
/// * `az_rad` — azimuth (radians).
/// * `el_rad` — elevation (radians).
/// * `dist` — orbit radius.
/// * `center` — point the camera looks at / orbits around.
pub fn orbit_eye(az_rad: f32, el_rad: f32, dist: f32, center: [f32; 3]) -> [f32; 3] {
    let ce = el_rad.cos();
    let se = el_rad.sin();
    [
        center[0] + dist * ce * az_rad.sin(),
        center[1] + dist * se,
        center[2] + dist * ce * az_rad.cos(),
    ]
}

// ---------------------------------------------------------------------------
// Small vec3 helpers (private — the public surface trades only in [f32; N]).
// ---------------------------------------------------------------------------

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len <= f32::EPSILON {
        [0.0, 0.0, 0.0]
    } else {
        let inv = 1.0 / len;
        [v[0] * inv, v[1] * inv, v[2] * inv]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn mat_approx(a: &Mat4, b: &Mat4, eps: f32) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| approx(*x, *y, eps))
    }

    #[test]
    fn identity_is_multiplicative_unit() {
        // A deliberately non-symmetric matrix so left/right identity are distinct checks.
        let m: Mat4 = [
            2.0, 3.0, 5.0, 7.0, 11.0, 13.0, 17.0, 19.0, 23.0, 29.0, 31.0, 37.0, 41.0, 43.0, 47.0,
            53.0,
        ];
        let id = identity();
        assert!(mat_approx(&mul(&id, &m), &m, 1e-4), "identity * m == m");
        assert!(mat_approx(&mul(&m, &id), &m, 1e-4), "m * identity == m");
    }

    #[test]
    fn mul_matches_column_vector_transform() {
        // (A·B)·v must equal A·(B·v) for the column-vector convention.
        let a = translate(1.0, 2.0, 3.0);
        let b = scale(2.0, 4.0, 8.0);
        let ab = mul(&a, &b);
        let p = [1.0f32, 1.0, 1.0];
        // A·(B·p): scale then translate → (2+1, 4+2, 8+3) = (3, 6, 11).
        let direct = transform_point(&ab, p);
        assert!(approx(direct[0], 3.0, 1e-5));
        assert!(approx(direct[1], 6.0, 1e-5));
        assert!(approx(direct[2], 11.0, 1e-5));
    }

    #[test]
    fn invert_perspective_round_trips_a_point() {
        let p = perspective(38.0_f32.to_radians(), 16.0 / 9.0, 0.01, 50.0);
        let pinv = invert(&p).expect("perspective is invertible");
        // Point in eye space, in front of the RH camera (z < 0), within the frustum.
        let pt = [0.3f32, -0.2, -5.0];
        let ndc = transform_point(&p, pt);
        let back = transform_point(&pinv, ndc);
        // The perspective divide in both directions cancels, recovering the point.
        assert!(approx(back[0], pt[0], 1e-3), "x: {} vs {}", back[0], pt[0]);
        assert!(approx(back[1], pt[1], 1e-3), "y: {} vs {}", back[1], pt[1]);
        assert!(approx(back[2], pt[2], 1e-3), "z: {} vs {}", back[2], pt[2]);
    }

    #[test]
    fn perspective_uses_gl_depth_range() {
        // A point at the near plane maps to clip z = -1, at the far plane to +1
        // (the OpenGL convention, distinct from D3D/Metal's [0, 1]).
        let near = 0.5f32;
        let far = 20.0f32;
        let p = perspective(60.0_f32.to_radians(), 1.0, near, far);
        let at_near = transform_point(&p, [0.0, 0.0, -near]);
        let at_far = transform_point(&p, [0.0, 0.0, -far]);
        assert!(
            approx(at_near[2], -1.0, 1e-4),
            "near z -> -1: {}",
            at_near[2]
        );
        assert!(approx(at_far[2], 1.0, 1e-4), "far z -> +1: {}", at_far[2]);
    }

    #[test]
    fn orbit_eye_at_zero_angles_is_on_plus_z() {
        let eye = orbit_eye(0.0, 0.0, 3.0, [0.0, 0.0, 0.0]);
        assert!(approx(eye[0], 0.0, 1e-5));
        assert!(approx(eye[1], 0.0, 1e-5));
        assert!(approx(eye[2], 3.0, 1e-5));
    }

    #[test]
    fn orbit_eye_honors_center_offset() {
        let center = [1.0f32, 2.0, 3.0];
        let eye = orbit_eye(0.0, 0.0, 4.0, center);
        assert!(approx(eye[0], 1.0, 1e-5));
        assert!(approx(eye[1], 2.0, 1e-5));
        assert!(approx(eye[2], 7.0, 1e-5)); // 3 + dist
    }

    #[test]
    fn look_at_places_eye_on_negative_z_axis_in_view_space() {
        // Camera on +Z looking at the origin: the world origin should land in
        // front of the camera at view-space z = -dist.
        let eye = [0.0f32, 0.0, 5.0];
        let view = look_at(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let o = transform_point(&view, [0.0, 0.0, 0.0]);
        assert!(approx(o[0], 0.0, 1e-5));
        assert!(approx(o[1], 0.0, 1e-5));
        assert!(approx(o[2], -5.0, 1e-5));
    }

    #[test]
    fn invert_singular_returns_none() {
        // A zero third column (rank-deficient) matrix has no inverse.
        let mut m = identity();
        m[8] = 0.0;
        m[10] = 0.0; // collapse the z axis
        assert!(invert(&m).is_none());
    }

    #[test]
    fn invert_round_trips_view_matrix() {
        let view = look_at([1.0, 2.0, 6.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let inv = invert(&view).expect("view invertible");
        let round = mul(&view, &inv);
        assert!(mat_approx(&round, &identity(), 1e-4), "view * view^-1 == I");
    }
}
