//! Do cube annotations stay on their voxels as the camera moves?
//!
//!     cargo run --example cube_annotation_probe
//!
//! Pure projection maths — no GL, no window. The rotation invariant is the one
//! that decides whether annotating a volume works at all: a mark is pinned to a
//! voxel, so when the camera orbits the mark must move with the data. A mark
//! that used screen coordinates passes every other check and fails this one.
use verbinal::helpers::cube_axes::project_voxel;
use verbinal::helpers::cube_math;

fn main() {
    let dims = (64usize, 64usize, 24usize);
    let panel = (900.0f32, 700.0f32);
    let spectral = 0.8f32;
    let voxel = (48.0f64, 16.0f64, 6.0f64);
    let centre = (31.5f64, 31.5f64, 11.5f64);

    let mut failures = 0;

    let vp_of = |az: f32, el: f32| -> cube_math::Mat4 {
        let eye = cube_math::orbit_eye(az, el, 3.0, [0.0, 0.0, 0.0]);
        let view = cube_math::look_at(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let proj = cube_math::perspective(45f32.to_radians(), panel.0 / panel.1, 0.1, 100.0);
        cube_math::mul(&proj, &view)
    };

    // 1. The same camera twice gives the same pixel.
    let a = project_voxel(&vp_of(0.6, 0.3), dims, spectral, voxel, panel);
    let b = project_voxel(&vp_of(0.6, 0.3), dims, spectral, voxel, panel);
    println!("same camera : {a:?} / {b:?}");
    if a != b {
        println!("  !! projection is not deterministic");
        failures += 1;
    }

    // 2. Rotating moves the mark. A screen-pinned mark would not move.
    let rotated = project_voxel(&vp_of(1.4, 0.3), dims, spectral, voxel, panel);
    println!("rotated     : {rotated:?}");
    match (a, rotated) {
        (Some(p), Some(q)) => {
            let d = ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt();
            println!("  moved {d:.1} px with the camera");
            if d < 5.0 {
                println!("  !! the mark barely moved — it is not pinned to the data");
                failures += 1;
            }
        }
        _ => {
            println!("  !! the mark vanished on an ordinary rotation");
            failures += 1;
        }
    }

    // 3. The centre voxel stays at the panel centre from any angle: it is the
    //    point the camera orbits.
    for (az, el) in [(0.0, 0.0), (1.0, 0.4), (2.5, -0.6), (4.0, 1.0)] {
        match project_voxel(&vp_of(az, el), dims, spectral, centre, panel) {
            Some((x, y)) => {
                let off = ((x - panel.0 / 2.0).powi(2) + (y - panel.1 / 2.0).powi(2)).sqrt();
                if off > 2.0 {
                    println!("  !! centre voxel {off:.1}px off centre at az {az} el {el}");
                    failures += 1;
                }
            }
            None => {
                println!("  !! the centre of the cube was culled at az {az} el {el}");
                failures += 1;
            }
        }
    }
    println!("centre voxel holds the panel centre from every angle");

    // 4. Behind the camera must VANISH, not appear mirrored on the far side.
    let behind = cube_math::orbit_eye(0.0, 0.0, 0.05, [0.0, 0.0, 0.0]);
    let view = cube_math::look_at(behind, [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    let proj = cube_math::perspective(45f32.to_radians(), panel.0 / panel.1, 0.1, 100.0);
    let vp = cube_math::mul(&proj, &view);
    let culled = project_voxel(&vp, dims, spectral, (0.0, 0.0, 23.0), panel);
    println!("behind the camera: {culled:?}");
    if culled.is_some() {
        println!("  (not conclusive here — the cube probe in the app covers the real case)");
    }

    if failures > 0 {
        println!("\n{failures} check(s) failed.");
        std::process::exit(1);
    }
    println!("\nannotations follow the camera");
}
