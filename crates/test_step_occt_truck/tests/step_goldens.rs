//! Compare OCS (truck) STEP geometry against the cadrum-generated golden files.

#[test]
fn step_goldens() {
    use test_step_occt_truck::{
        all_cases, assert_metrics, load_golden, ocs::step_solid, solid_metrics,
    };

    let cases = all_cases();
    assert!(
        !cases.is_empty(),
        "no .stp files found in input_brep/ or input_brep_surface/; cannot run STEP golden tests"
    );

    for case in cases {
        let golden = load_golden(&case.name)
            .unwrap_or_else(|e| panic!("failed to load golden for {}: {}", case.name, e));
        let solid = step_solid(&case.step_path).unwrap_or_else(|e| {
            panic!(
                "OCS (truck) failed to read STEP file {}: {e}",
                case.step_path.display()
            )
        });
        let actual = solid_metrics(&solid);
        let (abs, rel) = golden.tolerances();
        assert_metrics(&actual, &golden.expected, abs, rel);
    }
}
