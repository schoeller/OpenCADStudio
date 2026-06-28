//! Stable test-facing API for the OCS geometric kernel.

use truck_modeling::Solid;
use OpenCADStudio::scene::model::solid_model::{self, Bool};

/// A primitive solid recipe.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Primitive {
    Box {
        center: [f64; 3],
        length: f64,
        width: f64,
        height: f64,
    },
    Wedge {
        origin: [f64; 3],
        length: f64,
        width: f64,
        height: f64,
    },
    Cylinder {
        center: [f64; 3],
        radius: f64,
        height: f64,
    },
    Cone {
        center: [f64; 3],
        radius: f64,
        height: f64,
    },
    Sphere {
        center: [f64; 3],
        radius: f64,
    },
    Torus {
        center: [f64; 3],
        major: f64,
        minor: f64,
    },
}

/// CSG boolean operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BooleanOp {
    Union,
    Subtract,
    Intersect,
}

/// A declarative construction recipe.
///
/// Recipes can be executed by [`OcsKernel`] and rebuilt through the optional
/// `cadrum` reference adapter in `crate::cadrum`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Recipe {
    Primitive(Primitive),
    Boolean {
        op: BooleanOp,
        left: Box<Recipe>,
        right: Box<Recipe>,
    },
    /// Extrude a closed planar profile along a vector.
    Extrude {
        profile: Vec<[f64; 3]>,
        direction: [f64; 3],
    },
    /// Revolve a closed planar profile around an axis by an angle in radians.
    Revolve {
        profile: Vec<[f64; 3]>,
        axis_origin: [f64; 3],
        axis_direction: [f64; 3],
        angle: f64,
    },
    /// Loft a solid through a sequence of closed planar profiles.
    Loft {
        profiles: Vec<Vec<[f64; 3]>>,
    },
    /// Sweep a closed planar profile along a polyline path.
    Sweep {
        profile: Vec<[f64; 3]>,
        path: Vec<[f64; 3]>,
    },
}

impl Recipe {
    /// Convenience: wrap a primitive in a recipe.
    pub fn primitive(p: Primitive) -> Self {
        Self::Primitive(p)
    }

    /// Convenience: build a boolean recipe from two primitive recipes.
    pub fn boolean(op: BooleanOp, left: Primitive, right: Primitive) -> Self {
        Self::Boolean {
            op,
            left: Box::new(Self::Primitive(left)),
            right: Box::new(Self::Primitive(right)),
        }
    }

    /// Convenience: extrude a closed profile along a vector.
    pub fn extrude(profile: Vec<[f64; 3]>, direction: [f64; 3]) -> Self {
        Self::Extrude { profile, direction }
    }

    /// Convenience: revolve a closed profile around an axis by an angle (radians).
    pub fn revolve(
        profile: Vec<[f64; 3]>,
        axis_origin: [f64; 3],
        axis_direction: [f64; 3],
        angle: f64,
    ) -> Self {
        Self::Revolve {
            profile,
            axis_origin,
            axis_direction,
            angle,
        }
    }

    /// Convenience: loft through a sequence of closed profiles.
    pub fn loft(profiles: Vec<Vec<[f64; 3]>>) -> Self {
        Self::Loft { profiles }
    }

    /// Convenience: sweep a closed profile along a polyline path.
    pub fn sweep(profile: Vec<[f64; 3]>, path: Vec<[f64; 3]>) -> Self {
        Self::Sweep { profile, path }
    }
}

/// Thin wrapper around the OCS kernel builders used by the application.
pub struct OcsKernel;

impl OcsKernel {
    /// Build one of the supported primitives.
    pub fn primitive(p: &Primitive) -> Solid {
        match *p {
            Primitive::Box {
                center,
                length,
                width,
                height,
            } => solid_model::box_solid(center, length, width, height),
            Primitive::Wedge {
                origin,
                length,
                width,
                height,
            } => solid_model::wedge_solid(origin, length, width, height),
            Primitive::Cylinder {
                center,
                radius,
                height,
            } => solid_model::cylinder_solid(center, radius, height),
            Primitive::Cone {
                center,
                radius,
                height,
            } => solid_model::cone_solid(center, radius, height),
            Primitive::Sphere { center, radius } => solid_model::sphere_solid(center, radius),
            Primitive::Torus {
                center,
                major,
                minor,
            } => solid_model::torus_solid(center, major, minor),
        }
    }

    /// Apply a boolean operation.  Returns `None` if the kernel cannot produce
    /// a result (e.g. the operands do not overlap for a union).
    pub fn boolean(op: BooleanOp, a: &Solid, b: &Solid) -> Option<Solid> {
        let kind = match op {
            BooleanOp::Union => Bool::Union,
            BooleanOp::Subtract => Bool::Subtract,
            BooleanOp::Intersect => Bool::Intersect,
        };
        solid_model::boolean(kind, a, b)
    }

    /// Execute a [`Recipe`], returning `None` if a boolean sub-operation cannot
    /// produce a result (e.g. the operands do not overlap).
    pub fn recipe(r: &Recipe) -> Option<Solid> {
        match r {
            Recipe::Primitive(p) => Some(Self::primitive(p)),
            Recipe::Boolean { op, left, right } => {
                let a = Self::recipe(left)?;
                let b = Self::recipe(right)?;
                Self::boolean(*op, &a, &b)
            }
            Recipe::Extrude { profile, direction } => {
                solid_model::extrude_solid(profile, *direction)
            }
            Recipe::Revolve {
                profile,
                axis_origin,
                axis_direction,
                angle,
            } => solid_model::revolve_solid(profile, *axis_origin, *axis_direction, *angle),
            Recipe::Loft { profiles } => solid_model::loft_solid(profiles),
            Recipe::Sweep { profile, path } => solid_model::sweep_solid(profile, path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::solid_metrics;

    #[test]
    fn extrude_recipe_produces_solid() {
        let profile = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let recipe = Recipe::extrude(profile, [0.0, 1.0, 0.0]);
        let solid = OcsKernel::recipe(&recipe).expect("extrude recipe must produce a solid");
        let metrics = solid_metrics(&solid);
        assert!(metrics.volume > 1e-6, "extruded solid has no volume");
    }

    #[test]
    fn revolve_recipe_produces_solid() {
        // Triangle in the XZ plane, revolved 360° about the Z axis.
        let profile = vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 1.0]];
        let recipe = Recipe::revolve(
            profile,
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            std::f64::consts::TAU,
        );
        let solid = OcsKernel::recipe(&recipe).expect("revolve recipe must produce a solid");
        let metrics = solid_metrics(&solid);
        assert!(metrics.volume > 1e-6, "revolved solid has no volume");
    }

    #[test]
    fn loft_recipe_produces_solid() {
        let profiles = vec![
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            vec![
                [0.0, 0.0, 2.0],
                [1.0, 0.0, 2.0],
                [1.0, 1.0, 2.0],
                [0.0, 1.0, 2.0],
            ],
        ];
        let recipe = Recipe::loft(profiles);
        let solid = OcsKernel::recipe(&recipe).expect("loft recipe must produce a solid");
        let metrics = solid_metrics(&solid);
        assert!(metrics.volume > 1e-6, "lofted solid has no volume");
    }

    #[test]
    fn sweep_recipe_produces_solid() {
        let profile = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        let path = vec![[0.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        let recipe = Recipe::sweep(profile, path);
        let solid = OcsKernel::recipe(&recipe).expect("sweep recipe must produce a solid");
        let metrics = solid_metrics(&solid);
        assert!(metrics.volume > 1e-6, "swept solid has no volume");
    }
}
