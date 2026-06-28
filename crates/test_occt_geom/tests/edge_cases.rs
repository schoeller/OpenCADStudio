//! Edge-case behaviour for boolean operations.
//!
//! Disjoint operands, tangent operands, and coincident/overlapping faces are
//! exercised here.  Golden files record either the expected scalar metrics or
//! an empty result.

use test_occt_geom::{assert_metrics, cadrum, metrics::load_golden};

#[test]
fn edge_case_goldens() {
    for case in cadrum::edge_cases() {
        let golden = load_golden(case.name)
            .unwrap_or_else(|e| panic!("failed to load golden for {}: {}", case.name, e));
        let result = test_occt_geom::OcsKernel::recipe(&case.recipe);

        let has_any_expected = golden.expected.volume.is_some()
            || golden.expected.surface_area.is_some()
            || golden.expected.centroid.is_some()
            || golden.expected.bbox_min.is_some()
            || golden.expected.bbox_max.is_some()
            || golden.expected.triangle_count.is_some();

        if !has_any_expected {
            if let Some(solid) = result {
                let metrics = test_occt_geom::metrics::solid_metrics(&solid);
                assert!(
                    metrics.volume.abs() < 1e-9 && metrics.triangle_count == 0,
                    "{}: expected no solid, but got volume={} tris={}",
                    case.name,
                    metrics.volume,
                    metrics.triangle_count
                );
            }
            continue;
        }

        let solid = result.unwrap_or_else(|| panic!("{}: expected a solid, got None", case.name));
        let actual = test_occt_geom::metrics::solid_metrics(&solid);
        let (abs, rel) = golden.tolerances();
        assert_metrics(&actual, &golden.expected, abs, rel);
    }
}
