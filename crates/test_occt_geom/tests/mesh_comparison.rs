//! Mesh-level comparison between OCS and the cadrum reference kernel.
//!
//! This test runs only when the `cadrum-reference` feature is enabled. It
//! tessellates each catalog case through both kernels and compares the resulting
//! meshes using the symmetric Hausdorff distance. Scalar metrics can miss local
//! shape errors (e.g. missing faces or shifted vertices); this check catches
//! those cases.

#[cfg(feature = "cadrum-reference")]
#[test]
fn mesh_hausdorff_cross_check() {
    use test_occt_geom::{
        cadrum,
        metrics::{bbox_diagonal, load_golden, solid_to_mesh, symmetric_hausdorff},
        OcsKernel,
    };

    for case in cadrum::catalog() {
        let Some(ocs_solid) = OcsKernel::recipe(&case.recipe) else {
            eprintln!(
                "{}: OCS produced no solid; skipping Hausdorff check",
                case.name
            );
            continue;
        };
        let ocs_mesh = solid_to_mesh(&ocs_solid);

        let Ok(cadrum_mesh) = cadrum::recipe_to_mesh(&case.recipe) else {
            eprintln!(
                "{}: cadrum meshing failed; skipping Hausdorff check",
                case.name
            );
            continue;
        };

        let golden = load_golden(case.name)
            .unwrap_or_else(|e| panic!("failed to load golden for {}: {}", case.name, e));
        let (abs_tol, rel_tol) = golden.mesh_tolerances();

        let diag = bbox_diagonal(&ocs_mesh);
        let h = symmetric_hausdorff(&ocs_mesh, &cadrum_mesh);
        let rel_h = if diag > 1e-9 { h / diag } else { 0.0 };

        eprintln!(
            "{}: Hausdorff = {:.6e} (relative = {:.6e})",
            case.name, h, rel_h
        );

        if h > abs_tol + rel_tol * diag {
            panic!(
                "{}: symmetric Hausdorff distance {:.6e} exceeds tolerance (abs={:.6e}, rel={:.6e}, diagonal={:.6e})",
                case.name, h, abs_tol, rel_tol, diag
            );
        }
    }
}

#[cfg(not(feature = "cadrum-reference"))]
#[test]
fn mesh_hausdorff_cross_check() {
    eprintln!("cadrum-reference feature disabled; skipping mesh Hausdorff cross-check");
}
