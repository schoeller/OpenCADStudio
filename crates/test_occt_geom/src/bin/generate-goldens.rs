//! Golden-file generator for the geometric-kernel correctness harness.
//!
//! Usage:
//!
//! ```text
//! # Set TEST_GEOM_REGENERATE_GOLDENS=1 in your shell, e.g.:
//! #   Unix: export TEST_GEOM_REGENERATE_GOLDENS=1
//! #   PowerShell: $env:TEST_GEOM_REGENERATE_GOLDENS = "1"
//! #   Windows CMD: set TEST_GEOM_REGENERATE_GOLDENS=1
//! cargo run -p test_occt_geom --bin generate-goldens --features cadrum-reference
//! ```
//!
//! When the `cadrum-reference` feature is enabled, the generator runs each
//! catalog case through `cadrum` and stores the parsed metrics.  When the
//! feature is disabled, the generator exits with an error.

#[cfg(feature = "cadrum-reference")]
fn main() -> anyhow::Result<()> {
    let regenerate = std::env::var_os("TEST_GEOM_REGENERATE_GOLDENS")
        .map(|s| s == "1")
        .unwrap_or(false);
    if !regenerate {
        eprintln!("TEST_GEOM_REGENERATE_GOLDENS is not set to 1; skipping golden regeneration.");
        std::process::exit(0);
    }

    eprintln!("Regenerating goldens with cadrum reference kernel.");
    test_occt_geom::cadrum::regenerate_goldens()
}

#[cfg(not(feature = "cadrum-reference"))]
fn main() {
    eprintln!("This binary requires the cadrum-reference feature.");
    eprintln!(
        "Run: cargo run -p test_occt_geom --bin generate-goldens --features cadrum-reference"
    );
    std::process::exit(1);
}
