//! Generate B-rep STEP inputs for the OCS/truck reader.
//!
//! The committed STEP files in `input/` are geometrically-bounded surface
//! models that `truck-stepio` 0.3.0 cannot parse as B-rep solids.  This binary
//! uses cadrum (OpenCASCADE) to write a small set of planar B-rep solids as
//! STEP files to `input_brep/`.  Those files are the inputs that OCS/truck
//! reads in the tests.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p test_step_occt_truck --bin generate_brep_inputs --features cadrum-reference
//! ```

#[cfg(feature = "cadrum-reference")]
use std::path::PathBuf;

fn main() {
    #[cfg(not(feature = "cadrum-reference"))]
    {
        eprintln!(
            "generate_brep_inputs requires the `cadrum-reference` feature to create B-rep solids."
        );
        eprintln!("Run with: cargo run -p test_step_occt_truck --bin generate_brep_inputs --features cadrum-reference");
        std::process::exit(1);
    }

    #[cfg(feature = "cadrum-reference")]
    {
        if let Err(e) = run() {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "cadrum-reference")]
fn run() -> anyhow::Result<()> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_base = std::env::var_os("BREP_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("input_brep"));

    for (name, solid) in brep_catalog() {
        let out_path = output_base.join(name).with_extension("stp");
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&out_path)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", out_path.display()))?;
        cadrum::Solid::write_step(std::iter::once(&solid), &mut file)
            .map_err(|e| anyhow::anyhow!("failed to write B-rep STEP {}: {e}", out_path.display()))?;
        println!("generated B-rep STEP for {}: {}", name, out_path.display());
    }

    Ok(())
}

#[cfg(feature = "cadrum-reference")]
fn brep_catalog() -> Vec<(&'static str, cadrum::Solid)> {
    use cadrum::{DVec3, Solid};

    vec![
        ("cube_1x2x3", Solid::cube(DVec3::new(-1.0, -1.5, 0.0), DVec3::new(0.0, 0.5, 3.0))),
        (
            "wedge_4x5x3",
            extruded_profile(
                &[DVec3::new(0.0, 0.0, 0.0), DVec3::new(4.0, 0.0, 0.0), DVec3::new(0.0, 0.0, 3.0)],
                DVec3::Y * 5.0,
            ),
        ),
        (
            "hex_prism_r2_h6",
            extruded_profile(
                &[
                    DVec3::new(2.0, 0.0, 0.0),
                    DVec3::new(1.0, 0.0, 1.7320508075688772),
                    DVec3::new(-1.0, 0.0, 1.7320508075688772),
                    DVec3::new(-2.0, 0.0, 0.0),
                    DVec3::new(-1.0, 0.0, -1.7320508075688772),
                    DVec3::new(1.0, 0.0, -1.7320508075688772),
                ],
                DVec3::Y * 6.0,
            ),
        ),
        (
            "l_profile_2x3x4",
            extruded_profile(
                &[
                    DVec3::new(0.0, 0.0, 0.0),
                    DVec3::new(2.0, 0.0, 0.0),
                    DVec3::new(2.0, 0.0, 0.5),
                    DVec3::new(0.5, 0.0, 0.5),
                    DVec3::new(0.5, 0.0, 3.0),
                    DVec3::new(0.0, 0.0, 3.0),
                ],
                DVec3::Y * 4.0,
            ),
        ),
    ]
}

#[cfg(feature = "cadrum-reference")]
fn extruded_profile(points: &[cadrum::DVec3], direction: cadrum::DVec3) -> cadrum::Solid {
    let edges = cadrum::Edge::polygon(points).expect("closed profile");
    cadrum::Solid::extrude(&edges, direction).expect("extrude")
}
