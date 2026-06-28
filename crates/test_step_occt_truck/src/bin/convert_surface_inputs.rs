//! Convert the geometrically-bounded surface STEP files in `input/` into
//! planar B-rep STEP files that `truck-stepio` can read.
//!
//! The conversion is cadrum -> triangle mesh -> truck planar B-rep -> STEP.
//! The resulting files are written to `input_brep/` and are picked up by the
//! test suite and golden generator.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p test_step_occt_truck --bin convert_surface_inputs --features cadrum-reference
//! ```

#[cfg(feature = "cadrum-reference")]
use std::path::PathBuf;

#[cfg(feature = "cadrum-reference")]
use test_step_occt_truck::{
    cadrum::{cadrum_mesh, read_step_solid},
    mesh_to_solid::solid_from_mesh,
};

fn main() {
    #[cfg(not(feature = "cadrum-reference"))]
    {
        eprintln!(
            "convert_surface_inputs requires the `cadrum-reference` feature to read surface models."
        );
        eprintln!("Run with: cargo run -p test_step_occt_truck --bin convert_surface_inputs --features cadrum-reference");
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
    use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModel};

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_base = std::env::var_os("SURFACE_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("input_brep"));
    let input_dir = manifest.join("input");

    for entry in std::fs::read_dir(&input_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("stp") {
            continue;
        }
        let name = path
            .file_stem()
            .ok_or_else(|| anyhow::anyhow!("bad file name: {}", path.display()))?
            .to_string_lossy()
            .to_string();

        eprintln!("converting surface: {}", path.display());
        let _solid = read_step_solid(&path)
            .map_err(|e| anyhow::anyhow!("cadrum failed to read {}: {e}", path.display()))?;
        let mesh = cadrum_mesh(&path)
            .map_err(|e| anyhow::anyhow!("cadrum failed to mesh {}: {e}", path.display()))?;

        let ocs_solid = solid_from_mesh(&mesh, 0.0)
            .map_err(|e| anyhow::anyhow!("failed to build OCS solid from {}: {e}", path.display()))?;

        let out_path = output_base.join(&name).with_extension("stp");
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let compressed = ocs_solid.compress();
        let step_model = StepModel::from(&compressed);
        let display = CompleteStepDisplay::new(step_model, StepHeaderDescriptor::default());
        std::fs::write(&out_path, display.to_string())
            .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", out_path.display()))?;

        println!("converted {}: {}", name, out_path.display());
    }

    Ok(())
}
