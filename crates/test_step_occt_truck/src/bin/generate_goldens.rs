//! Generate golden files for every STEP file in `input_brep/` and
//! `input_brep_surface/`.
//!
//! The golden files are written to `data/step_json/` and contain the reference
//! metrics computed from the OpenCASCADE/cadrum kernel.
//!
//! The output directory can be overridden with the `GOLDEN_OUT_DIR` environment
//! variable, which is useful on Windows when the default directory is protected
//! by Controlled Folder Access.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p test_step_occt_truck --bin generate_goldens --features cadrum-reference
//! ```

#[cfg(feature = "cadrum-reference")]
use test_step_occt_truck::{
    all_cases,
    cadrum::cadrum_metrics,
    metrics::{ExpectedMetrics, GoldenFile, Tolerances, golden_path_in},
};

fn main() {
    #[cfg(not(feature = "cadrum-reference"))]
    {
        eprintln!(
            "generate_goldens requires the `cadrum-reference` feature to compute reference metrics."
        );
        eprintln!("Run with: cargo run -p test_step_occt_truck --bin generate_goldens --features cadrum-reference");
        std::process::exit(1);
    }

    #[cfg(feature = "cadrum-reference")]
    {
        if let Err(e) = run() {
            eprintln!("error: {e}");
            eprintln!("debug chain: {e:?}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "cadrum-reference")]
fn run() -> anyhow::Result<()> {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_base = std::env::var_os("GOLDEN_OUT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| manifest.join("data"));

    for case in all_cases() {
        eprintln!("processing: {}", case.step_path.display());
        let metrics = cadrum_metrics(&case.step_path).map_err(|e| {
            anyhow::anyhow!(
                "cadrum failed to read {}: {}",
                case.step_path.display(),
                e
            )
        })?;

        let expected = if case.is_surface() {
            ExpectedMetrics::surface_like(&metrics)
        } else {
            ExpectedMetrics::exact(&metrics)
        };

        let golden = GoldenFile {
            name: case.name.clone(),
            recipe_cadrum_rust: Some(format!(
                "let r = Solid::read_step(&mut std::fs::File::open(\"{}\")?)?.pop();",
                case.step_path.display()
            )),
            expected,
            tolerances: Tolerances::default(),
            note: Some(case.note.clone()),
        };

        let golden_path = golden_path_in(&output_base, &case.name);
        if let Some(parent) = golden_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&golden)
            .map_err(|e| anyhow::anyhow!("json serialization failed: {e}"))?;
        std::fs::write(&golden_path, text)
            .map_err(|e| anyhow::anyhow!("write failed for {}: {e}", golden_path.display()))?;
        println!(
            "generated golden for {}: {} (surface_area={:.6e}, volume={:.6e}, triangles={})",
            case.name,
            golden_path.display(),
            metrics.surface_area,
            metrics.volume,
            metrics.triangle_count
        );
    }

    Ok(())
}
