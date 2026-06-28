//! Primitive solid correctness tests.
//!
//! Each test loads a committed golden file, builds the same primitive through
//! the OCS kernel, and compares mesh-derived scalar invariants.

use test_occt_geom::{assert_metrics, cadrum, metrics::load_golden};

#[test]
fn primitive_goldens() {
    for case in cadrum::primitive_cases() {
        let golden = load_golden(case.name)
            .unwrap_or_else(|e| panic!("failed to load golden for {}: {}", case.name, e));
        let solid = test_occt_geom::OcsKernel::recipe(&case.recipe)
            .unwrap_or_else(|| panic!("{}: primitive recipe must produce a solid", case.name));
        let actual = test_occt_geom::metrics::solid_metrics(&solid);
        let (abs, rel) = golden.tolerances();
        assert_metrics(&actual, &golden.expected, abs, rel);
    }
}
