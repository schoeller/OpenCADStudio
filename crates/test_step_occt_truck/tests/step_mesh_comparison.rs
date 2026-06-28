//! Direct mesh-to-mesh comparison between OCS (truck) and cadrum (OpenCASCADE).
//!
//! This test is only compiled when the `cadrum-reference` feature is enabled.

#[cfg(feature = "cadrum-reference")]
#[test]
fn step_mesh_hausdorff() {
    use test_step_occt_truck::{
        all_cases, bbox_diagonal, cadrum::cadrum_mesh, load_golden, ocs::step_solid,
        solid_to_mesh, symmetric_hausdorff,
    };

    let cases = all_cases();
    assert!(
        !cases.is_empty(),
        "no .stp files found in input_brep/ or input_brep_surface/; cannot run STEP mesh comparison"
    );

    for case in cases {
        let golden = load_golden(&case.name)
            .unwrap_or_else(|e| panic!("failed to load golden for {}: {}", case.name, e));

        let ocs_solid = step_solid(&case.step_path).unwrap_or_else(|e| {
            panic!(
                "OCS (truck) failed to read STEP file {}: {e}",
                case.step_path.display()
            )
        });
        let ocs_mesh = solid_to_mesh(&ocs_solid);

        let reference_path = case.original_path.as_ref().unwrap_or(&case.step_path);
        let cadrum_mesh = match cadrum_mesh(reference_path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "{}: cadrum meshing failed ({}) - skipping Hausdorff check",
                    case.name, e
                );
                continue;
            }
        };

        let diag = bbox_diagonal(&ocs_mesh);
        let h = symmetric_hausdorff(&ocs_mesh, &cadrum_mesh);
        let (abs_tol, rel_tol) = golden.mesh_tolerances();

        eprintln!(
            "{}: Hausdorff = {:.6e} (relative = {:.6e})",
            case.name,
            h,
            if diag > 1e-9 { h / diag } else { 0.0 }
        );

        assert!(
            h <= abs_tol + rel_tol * diag,
            "{}: symmetric Hausdorff distance {:.6e} exceeds tolerance (abs={:.6e}, rel={:.6e}, diagonal={:.6e})",
            case.name, h, abs_tol, rel_tol, diag
        );
    }
}

#[cfg(not(feature = "cadrum-reference"))]
#[test]
fn step_mesh_hausdorff() {
    eprintln!("cadrum-reference feature disabled; skipping STEP mesh Hausdorff comparison");
}
