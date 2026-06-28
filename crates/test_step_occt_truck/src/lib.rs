//! STEP-based geometry comparison between the OCS kernel (truck) and the
//! optional OpenCASCADE reference kernel (cadrum).

pub mod case;
pub mod mesh_to_solid;
pub mod metrics;
pub mod ocs;

#[cfg(feature = "cadrum-reference")]
pub mod cadrum;

/// Convenience re-exports for tests and binaries.
pub use case::{all_cases, step_cases, StepCase};
pub use mesh_to_solid::solid_from_mesh;
pub use metrics::{
    assert_metrics, bbox_diagonal, load_golden, mesh_metrics, save_golden, solid_metrics,
    solid_to_mesh, symmetric_hausdorff, ExpectedMetrics, GeomMetrics, GoldenFile, Mesh, Tolerances,
};
pub use ocs::step_solid;

#[cfg(feature = "cadrum-reference")]
pub use cadrum::{cadrum_mesh, cadrum_metrics, read_step_solid};
