//! Regression test harness for the OCS geometric kernel against reference cases.
//!
//! The harness provides:
//!
//! * [`OcsKernel`] - a thin, stable wrapper around the OCS/truck geometry
//!   builders so that tests are written against an operation vocabulary
//!   (primitives, booleans, recipes) rather than raw library calls.
//! * [`Recipe`] - a declarative construction tree that can be executed by
//!   [`OcsKernel`] and rebuilt through the optional `cadrum` reference kernel.
//! * [`GeomMetrics`] - scalar invariants (volume, surface area, centroid,
//!   bounding box, triangle count) extracted from a tessellated solid.
//! * [`ExpectedMetrics`] - reference values, usually analytic or captured from
//!   the optional `cadrum` reference kernel.
//! * [`assert_metrics`] - tolerant comparison used by every scalar test.
//! * [`symmetric_hausdorff`] - optional mesh-to-mesh comparison for cases where
//!   scalar invariants are not discriminative enough.
//! * [`cadrum`] - a catalog of geometric test scenarios mapped to the OCS
//!   kernel, plus an optional in-process adapter for the `cadrum` crate.
//!
//! All code lives in `crates/test_occt_geom`; no other crate is modified.

#![allow(non_snake_case)]

pub mod cadrum;
pub mod geom;
pub mod metrics;
pub mod reference;

pub use geom::{BooleanOp, OcsKernel, Primitive, Recipe};
pub use metrics::{
    assert_metrics, bbox_diagonal, solid_metrics, solid_to_mesh, symmetric_hausdorff,
    ExpectedMetrics, GeomMetrics, Mesh,
};
