//! Runtime cross-check against the in-process `cadrum` reference kernel.
//!
//! When the `cadrum-reference` feature is enabled, every catalog case is run
//! through both the OCS kernel and `cadrum`, and the scalar metrics are
//! compared.  Without the feature this test compiles to a no-op.

#[cfg(feature = "cadrum-reference")]
#[test]
fn cadrum_cross_check() {
    use test_occt_geom::{
        assert_metrics, cadrum,
        geom::Recipe,
        metrics::{load_golden, ExpectedMetrics},
    };

    for case in cadrum::catalog() {
        let ocs = case.run_ocs();
        let cadrum_result = cadrum::cadrum_reference(&case.recipe);

        match (ocs, cadrum_result) {
            (Some(ocs_metrics), Ok(cadrum_metrics)) => {
                // Build an expected set that excludes triangle_count and
                // bounding box (tessellation / B-rep differences make those
                // unreliable for a kernel-to-kernel cross-check) and excludes
                // surface area for boolean results.
                let mut expected = ExpectedMetrics::default()
                    .volume(cadrum_metrics.volume)
                    .centroid(cadrum_metrics.centroid);
                if matches!(case.recipe, Recipe::Primitive(_)) {
                    expected = expected.surface_area(cadrum_metrics.surface_area);
                }

                let golden = load_golden(case.name)
                    .unwrap_or_else(|e| panic!("failed to load golden for {}: {}", case.name, e));
                let (abs, rel) = golden.tolerances();
                assert_metrics(&ocs_metrics, &expected, abs, rel);
            }
            (None, Ok(cadrum_metrics)) => {
                let golden = load_golden(case.name)
                    .unwrap_or_else(|e| panic!("failed to load golden for {}: {}", case.name, e));
                let has_any_expected = golden.expected.volume.is_some()
                    || golden.expected.surface_area.is_some()
                    || golden.expected.centroid.is_some()
                    || golden.expected.bbox_min.is_some()
                    || golden.expected.bbox_max.is_some()
                    || golden.expected.triangle_count.is_some();
                if has_any_expected {
                    panic!(
                        "{}: OCS returned empty but the golden file expects a solid",
                        case.name
                    );
                }
                if cadrum_metrics.volume.abs() >= 1e-9 {
                    eprintln!(
                        "{}: OCS returned empty (expected); cadrum produced volume {}",
                        case.name, cadrum_metrics.volume
                    );
                }
            }
            (Some(_), Err(e)) => {
                // cadrum and OCS disagree on some edge cases (tangent solids,
                // coincident faces).  Report the divergence as a warning rather
                // than failing the optional cross-check.
                eprintln!(
                    "{}: OCS produced a solid but cadrum failed: {}",
                    case.name, e
                );
            }
            (None, Err(_)) => {
                eprintln!("{}: both OCS and cadrum produced no result", case.name);
            }
        }
    }
}

#[cfg(not(feature = "cadrum-reference"))]
#[test]
fn cadrum_cross_check() {
    eprintln!("cadrum-reference feature disabled; skipping runtime cross-check");
}
