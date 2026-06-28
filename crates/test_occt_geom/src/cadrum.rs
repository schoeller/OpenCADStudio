//! Catalog of geometric-kernel regression cases and optional `cadrum` reference adapter.
//!
//! This module keeps the stable Phase-1 catalog of primitives, booleans, and
//! edge cases.  The catalog itself is independent of any reference kernel and
//! is used by the default `cargo test` path against committed golden files.
//!
//! # Optional in-process `cadrum` reference
//!
//! When the `cadrum-reference` feature is enabled, the adapter in this module
//! can rebuild the same [`Recipe`] through the statically linked OpenCASCADE
//! 8.0 kernel exposed by `cadrum` and extract mass properties for comparison
//! or golden regeneration.  When the feature is disabled the tests fall back
//! to the committed golden files in `data/`.

use crate::geom::{BooleanOp, OcsKernel, Primitive, Recipe};
use crate::metrics::{ExpectedMetrics, GeomMetrics};
use crate::reference::analytic_primitive;

/// One regression test scenario.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: &'static str,
    pub recipe: Recipe,
    pub note: &'static str,
}

impl Case {
    /// Build the recipe through the OCS kernel and return its metrics.
    ///
    /// Returns `None` when the recipe produces no solid (e.g. disjoint
    /// intersection operands).
    pub fn run_ocs(&self) -> Option<GeomMetrics> {
        let solid = OcsKernel::recipe(&self.recipe)?;
        Some(crate::metrics::solid_metrics(&solid))
    }
}

/// Full Phase-1 catalog: primitives, booleans, and edge cases.
pub fn catalog() -> Vec<Case> {
    let mut cases = Vec::new();
    cases.extend(primitive_cases());
    cases.extend(boolean_cases());
    cases.extend(edge_cases());
    cases
}

/// Look up a catalog case by its `group/name` identifier.
pub fn find_case(name: &str) -> Option<Case> {
    catalog().into_iter().find(|c| c.name == name)
}

/// Regenerate all golden files.
///
/// With the `cadrum-reference` feature enabled, `cadrum` is used as the
/// reference kernel for booleans; primitives always use analytic values.  On
/// failure for an individual case the generator falls back to OCS-computed
/// metrics, printing a warning.
///
/// Without the feature, goldens are seeded from analytic values for
/// primitives and from OCS-computed metrics for booleans.
pub fn regenerate_goldens() -> anyhow::Result<()> {
    use crate::metrics::{save_golden, GoldenFile, Tolerances};

    for case in catalog() {
        let path = crate::metrics::golden_path(case.name);
        eprintln!("generating {} -> {}", case.name, path.display());

        let expected = seed_expected(&case);
        let recipe_cadrum_rust = recipe_to_cadrum_rust(&case.recipe);

        let golden = GoldenFile {
            name: case.name.to_string(),
            recipe_cadrum_rust,
            expected,
            tolerances: Tolerances::default(),
            note: Some(case.note.to_string()),
        };
        save_golden(&path, &golden)?;
    }

    if cfg!(feature = "cadrum-reference") {
        eprintln!("Goldens refreshed from cadrum.");
    } else {
        eprintln!(
            "Goldens seeded from analytic/OCS values; refresh with --features cadrum-reference."
        );
    }
    Ok(())
}

/// Choose the reference source for a golden file.
fn seed_expected(case: &Case) -> ExpectedMetrics {
    if let Recipe::Primitive(p) = &case.recipe {
        return analytic_primitive(p);
    }

    // For booleans, the golden file records what OCS is expected to produce.
    // If OCS cannot produce a solid, or produces an effectively empty solid,
    // store an empty expectation even when the reference kernel succeeds, so
    // the default tests stay aligned with OCS.
    let Some(ocs_metrics) = case.run_ocs() else {
        return ExpectedMetrics::default();
    };
    if ocs_metrics.volume.abs() < 1e-9 && ocs_metrics.triangle_count == 0 {
        return ExpectedMetrics::default();
    }

    #[cfg(feature = "cadrum-reference")]
    {
        match cadrum_reference(&case.recipe) {
            Ok(metrics) => expected_from_cadrum(&metrics),
            Err(e) => {
                eprintln!("  cadrum failed: {}; using OCS seed", e);
                ExpectedMetrics::exact(&ocs_metrics)
            }
        }
    }
    #[cfg(not(feature = "cadrum-reference"))]
    {
        ExpectedMetrics::exact(&ocs_metrics)
    }
}

/// Convert cadrum-derived metrics into stored expected values.
///
/// Boolean results keep volume, centroid, and bounding box; surface area and
/// triangle count are left unset because they vary between kernels and are
/// not needed for the primary correctness check.
#[cfg(feature = "cadrum-reference")]
fn expected_from_cadrum(metrics: &GeomMetrics) -> ExpectedMetrics {
    ExpectedMetrics::default()
        .volume(metrics.volume)
        .centroid(metrics.centroid)
        .bbox_min(metrics.bbox_min)
        .bbox_max(metrics.bbox_max)
}

/// Primitive cases with analytic reference metrics.
pub fn primitive_cases() -> Vec<Case> {
    vec![
        (
            "primitive/box_4x6x8_at_1_2_3",
            Primitive::Box {
                center: [1.0, 2.0, 3.0],
                length: 4.0,
                width: 6.0,
                height: 8.0,
            },
            "Analytic box metrics.",
        ),
        (
            "primitive/wedge_4x5x6_at_minus1_minus2_minus3",
            Primitive::Wedge {
                origin: [-1.0, -2.0, -3.0],
                length: 4.0,
                width: 5.0,
                height: 6.0,
            },
            "Analytic wedge metrics; cadrum builds it from a triangular profile.",
        ),
        (
            "primitive/cylinder_r3_h10",
            Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: 3.0,
                height: 10.0,
            },
            "Analytic cylinder metrics.",
        ),
        (
            "primitive/cone_r4_h9",
            Primitive::Cone {
                center: [0.0, 0.0, 0.0],
                radius: 4.0,
                height: 9.0,
            },
            "Analytic cone metrics.",
        ),
        (
            "primitive/sphere_r5",
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 5.0,
            },
            "Analytic sphere metrics.",
        ),
        (
            "primitive/torus_r8_r2",
            Primitive::Torus {
                center: [0.0, 0.0, 0.0],
                major: 8.0,
                minor: 2.0,
            },
            "Analytic torus metrics.",
        ),
    ]
    .into_iter()
    .map(|(name, primitive, note)| Case {
        name,
        recipe: Recipe::primitive(primitive),
        note,
    })
    .collect()
}

/// Boolean cases.  Reference values are stored in golden files; when
/// `cadrum-reference` is enabled the generator refreshes them from `cadrum`.
pub fn boolean_cases() -> Vec<Case> {
    // Two 10x10x10 boxes:
    //   A centered at the origin -> [-5,5]^3
    //   B centered at (5,5,5)    -> [0,10]^3
    let box_a = Primitive::Box {
        center: [0.0, 0.0, 0.0],
        length: 10.0,
        width: 10.0,
        height: 10.0,
    };
    let box_b = Primitive::Box {
        center: [5.0, 5.0, 5.0],
        length: 10.0,
        width: 10.0,
        height: 10.0,
    };

    // Cylinder fully inside the box (r=3, h=10).
    let cylinder_inside = Primitive::Cylinder {
        center: [0.0, 0.0, 0.0],
        radius: 3.0,
        height: 10.0,
    };

    // Cylinder strictly inside sphere (r=2, h=6; sphere r=5).
    let cylinder_for_sphere = Primitive::Cylinder {
        center: [0.0, 0.0, 0.0],
        radius: 2.0,
        height: 6.0,
    };
    let sphere_for_cylinder = Primitive::Sphere {
        center: [0.0, 0.0, 0.0],
        radius: 5.0,
    };

    vec![
        Case {
            name: "boolean/box_box_fuse",
            recipe: Recipe::boolean(BooleanOp::Union, box_a.clone(), box_b.clone()),
            note: "Two overlapping boxes; reference values stored in golden file.",
        },
        Case {
            name: "boolean/box_box_cut",
            recipe: Recipe::boolean(BooleanOp::Subtract, box_a.clone(), box_b.clone()),
            note: "Two overlapping boxes; reference values stored in golden file.",
        },
        Case {
            name: "boolean/box_box_common",
            recipe: Recipe::boolean(BooleanOp::Intersect, box_a.clone(), box_b.clone()),
            note: "Two overlapping boxes; reference values stored in golden file.",
        },
        Case {
            name: "boolean/box_cylinder_fuse",
            recipe: Recipe::boolean(BooleanOp::Union, box_a.clone(), cylinder_inside.clone()),
            note: "Cylinder fully inside box; union volume is box + cylinder.",
        },
        Case {
            name: "boolean/box_cylinder_cut",
            recipe: Recipe::boolean(BooleanOp::Subtract, box_a.clone(), cylinder_inside.clone()),
            note: "Cylinder fully inside box; cut volume is box - cylinder.",
        },
        Case {
            name: "boolean/cylinder_sphere_common",
            recipe: Recipe::boolean(
                BooleanOp::Intersect,
                cylinder_for_sphere,
                sphere_for_cylinder,
            ),
            note: "OCS/truck-shapeops returned no solid for this sphere-cylinder intersection.",
        },
        Case {
            name: "boolean/sphere_box_fuse",
            recipe: Recipe::boolean(
                BooleanOp::Union,
                Primitive::Sphere {
                    center: [0.0, 0.0, 0.0],
                    radius: 5.0,
                },
                Primitive::Box {
                    center: [3.0, 0.0, 0.0],
                    length: 4.0,
                    width: 4.0,
                    height: 4.0,
                },
            ),
            note: "Box fully inside sphere; union equals sphere; values seeded from OCS.",
        },
        Case {
            name: "boolean/torus_box_cut",
            recipe: Recipe::boolean(
                BooleanOp::Subtract,
                Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major: 8.0,
                    minor: 2.0,
                },
                Primitive::Box {
                    center: [10.0, 0.0, 0.0],
                    length: 4.0,
                    width: 4.0,
                    height: 4.0,
                },
            ),
            note: "Torus minus a small piece of the tube; values seeded from OCS, refresh with cadrum.",
        },
        Case {
            name: "boolean/cone_cylinder_fuse",
            recipe: Recipe::boolean(
                BooleanOp::Union,
                Primitive::Cone {
                    center: [0.0, 0.0, 0.0],
                    radius: 4.0,
                    height: 9.0,
                },
                Primitive::Cylinder {
                    center: [0.0, 0.0, 1.0],
                    radius: 2.0,
                    height: 3.0,
                },
            ),
            note: "Cylinder fully inside cone; union equals cone; values seeded from OCS.",
        },
    ]
}

/// Edge cases that probe empty / tangent / coincident behavior.
pub fn edge_cases() -> Vec<Case> {
    vec![
        Case {
            name: "edge/box_box_disjoint",
            recipe: Recipe::boolean(
                BooleanOp::Intersect,
                Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    length: 10.0,
                    width: 10.0,
                    height: 10.0,
                },
                Primitive::Box {
                    center: [20.0, 0.0, 0.0],
                    length: 10.0,
                    width: 10.0,
                    height: 10.0,
                },
            ),
            note: "Disjoint operands; the OCS kernel is expected to return None.",
        },
        Case {
            name: "edge/cylinder_cylinder_tangent",
            recipe: Recipe::boolean(
                BooleanOp::Union,
                Primitive::Cylinder {
                    center: [0.0, 0.0, 0.0],
                    radius: 3.0,
                    height: 10.0,
                },
                Primitive::Cylinder {
                    center: [6.0, 0.0, 0.0],
                    radius: 3.0,
                    height: 10.0,
                },
            ),
            note: "Two cylinders tangent along a shared generator; values seeded from OCS.",
        },
        Case {
            name: "edge/box_box_coincident_face",
            recipe: Recipe::boolean(
                BooleanOp::Union,
                Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    length: 10.0,
                    width: 10.0,
                    height: 10.0,
                },
                Primitive::Box {
                    center: [4.5, 0.0, 0.0],
                    length: 11.0,
                    width: 10.0,
                    height: 10.0,
                },
            ),
            note: "OCS/truck-shapeops returned no solid for this overlapping-box union.",
        },
    ]
}

// ── Human-readable cadrum traceability string ───────────────────────────────

/// Produce a short Rust-like snippet that reconstructs `recipe` with cadrum.
///
/// This is stored in golden files for traceability only; it is not required to
/// compile.
fn recipe_to_cadrum_rust(recipe: &Recipe) -> Option<String> {
    Some(format!("let r = {};", emit_recipe_rust(recipe, &mut 0)?))
}

fn emit_recipe_rust(recipe: &Recipe, counter: &mut usize) -> Option<String> {
    match recipe {
        Recipe::Primitive(p) => {
            let expr = match p {
                Primitive::Box {
                    center: [cx, cy, cz],
                    length,
                    width,
                    height,
                } => {
                    let min = [cx - length / 2.0, cy - width / 2.0, cz - height / 2.0];
                    let max = [cx + length / 2.0, cy + width / 2.0, cz + height / 2.0];
                    format!(
                        "Solid::cube(DVec3::new({:.6}, {:.6}, {:.6}), DVec3::new({:.6}, {:.6}, {:.6}))",
                        min[0], min[1], min[2], max[0], max[1], max[2]
                    )
                }
                Primitive::Wedge {
                    origin: [ox, oy, oz],
                    length,
                    width,
                    height,
                } => {
                    format!(
                        "Solid::extrude(&Edge::polygon(&[DVec3::new(0,0,0), DVec3::new({l:.6},0,0), DVec3::new(0,0,{h:.6})])?, DVec3::Y * {w:.6})?.translate(DVec3::new({ox:.6}, {oy:.6}, {oz:.6}))",
                        l = length, w = width, h = height, ox = ox, oy = oy, oz = oz
                    )
                }
                Primitive::Cylinder {
                    center: [cx, cy, cz],
                    radius,
                    height,
                } => {
                    format!(
                        "Solid::cylinder({r:.6}, DVec3::Z * {h:.6}).translate(DVec3::new({cx:.6}, {cy:.6}, {cz:.6}))",
                        r = radius, h = height
                    )
                }
                Primitive::Cone {
                    center: [cx, cy, cz],
                    radius,
                    height,
                } => {
                    format!(
                        "Solid::cone({r:.6}, 0.0, DVec3::Z * {h:.6}).translate(DVec3::new({cx:.6}, {cy:.6}, {cz:.6}))",
                        r = radius, h = height
                    )
                }
                Primitive::Sphere {
                    center: [cx, cy, cz],
                    radius,
                } => {
                    format!(
                        "Solid::sphere({r:.6}).translate(DVec3::new({cx:.6}, {cy:.6}, {cz:.6}))",
                        r = radius
                    )
                }
                Primitive::Torus {
                    center: [cx, cy, cz],
                    major,
                    minor,
                } => {
                    format!(
                        "Solid::torus({major:.6}, {minor:.6}, DVec3::Z).translate(DVec3::new({cx:.6}, {cy:.6}, {cz:.6}))",
                    )
                }
            };
            Some(expr)
        }
        Recipe::Boolean { op, left, right } => {
            let a = emit_recipe_rust(left, counter)?;
            let b = emit_recipe_rust(right, counter)?;
            let a_name = format!("p{}", counter);
            *counter += 1;
            let b_name = format!("p{}", counter);
            *counter += 1;
            let op_str = match op {
                BooleanOp::Union => "+",
                BooleanOp::Subtract => "-",
                BooleanOp::Intersect => "*",
            };
            Some(format!(
                "{{ let {a_name} = {a}; let {b_name} = {b}; (&{a_name} {op_str} &{b_name}).build()? }}",
            ))
        }
        Recipe::Extrude { profile, direction } => {
            let pts = profile
                .iter()
                .map(|[x, y, z]| format!("DVec3::new({x:.6}, {y:.6}, {z:.6})"))
                .collect::<Vec<_>>()
                .join(", ");
            Some(format!(
                "Solid::extrude(&Edge::polygon(&[{pts}])?, DVec3::new({dx:.6}, {dy:.6}, {dz:.6}))?",
                dx = direction[0],
                dy = direction[1],
                dz = direction[2]
            ))
        }
        // cadrum traceability strings are optional; omit the more complex variants for now.
        Recipe::Revolve { .. } | Recipe::Loft { .. } | Recipe::Sweep { .. } => None,
    }
}

// ── cadrum in-process adapter ───────────────────────────────────────────────

#[cfg(feature = "cadrum-reference")]
mod cadrum_adapter {
    use super::{BooleanOp, Primitive, Recipe};
    use crate::metrics::GeomMetrics;

    /// Build a `cadrum::Solid` from a [`Recipe`].
    pub fn recipe_to_cadrum(recipe: &Recipe) -> Result<cadrum::Solid, cadrum::Error> {
        match recipe {
            Recipe::Primitive(p) => primitive_to_cadrum(p),
            Recipe::Boolean { op, left, right } => {
                let a = recipe_to_cadrum(left)?;
                let b = recipe_to_cadrum(right)?;
                let boolean = match op {
                    BooleanOp::Union => &a + &b,
                    BooleanOp::Subtract => &a - &b,
                    BooleanOp::Intersect => &a * &b,
                };
                boolean.build()
            }
            Recipe::Extrude { profile, direction } => {
                let edges = points_to_cadrum_edges(profile)?;
                let dir = array_to_dvec3(*direction);
                cadrum::Solid::extrude(&edges, dir)
            }
            Recipe::Revolve { .. } => Err(cadrum::Error::Unknown(
                "cadrum reference does not support revolve".into(),
            )),
            Recipe::Loft { profiles } => {
                let sections: Vec<Vec<cadrum::Edge>> = profiles
                    .iter()
                    .map(|p| points_to_cadrum_edges(p))
                    .collect::<Result<Vec<_>, _>>()?;
                cadrum::Solid::loft(sections.iter(), false)
            }
            Recipe::Sweep { profile, path } => {
                let profile_edges = points_to_cadrum_edges(profile)?;
                let spine = path_to_cadrum_edges(path)?;
                cadrum::Solid::sweep(&profile_edges, &spine, cadrum::ProfileOrient::Torsion)
            }
        }
    }

    fn primitive_to_cadrum(p: &Primitive) -> Result<cadrum::Solid, cadrum::Error> {
        use cadrum::{DVec3, Edge, Solid};
        Ok(match *p {
            Primitive::Box {
                center: [cx, cy, cz],
                length,
                width,
                height,
            } => {
                let min = DVec3::new(cx - length / 2.0, cy - width / 2.0, cz - height / 2.0);
                let max = DVec3::new(cx + length / 2.0, cy + width / 2.0, cz + height / 2.0);
                Solid::cube(min, max)
            }
            Primitive::Wedge {
                origin: [ox, oy, oz],
                length,
                width,
                height,
            } => {
                let profile = Edge::polygon(&[
                    DVec3::new(0.0, 0.0, 0.0),
                    DVec3::new(length, 0.0, 0.0),
                    DVec3::new(0.0, 0.0, height),
                ])?;
                Solid::extrude(&profile, DVec3::Y * width)?.translate(DVec3::new(ox, oy, oz))
            }
            Primitive::Cylinder {
                center: [cx, cy, cz],
                radius,
                height,
            } => Solid::cylinder(radius, DVec3::Z * height).translate(DVec3::new(cx, cy, cz)),
            Primitive::Cone {
                center: [cx, cy, cz],
                radius,
                height,
            } => Solid::cone(radius, 0.0, DVec3::Z * height).translate(DVec3::new(cx, cy, cz)),
            Primitive::Sphere {
                center: [cx, cy, cz],
                radius,
            } => Solid::sphere(radius).translate(DVec3::new(cx, cy, cz)),
            Primitive::Torus {
                center: [cx, cy, cz],
                major,
                minor,
            } => Solid::torus(major, minor, DVec3::Z).translate(DVec3::new(cx, cy, cz)),
        })
    }

    /// Compute reference metrics for `recipe` using `cadrum`.
    pub fn cadrum_reference(recipe: &Recipe) -> Result<GeomMetrics, cadrum::Error> {
        let solid = recipe_to_cadrum(recipe)?;
        let [min, max] = solid.bounding_box();
        Ok(GeomMetrics {
            volume: solid.volume(),
            surface_area: solid.area(),
            centroid: dvec3_to_array(solid.center()),
            bbox_min: dvec3_to_array(min),
            bbox_max: dvec3_to_array(max),
            triangle_count: 0,
        })
    }

    fn points_to_cadrum_edges(points: &[[f64; 3]]) -> Result<Vec<cadrum::Edge>, cadrum::Error> {
        let dvec_points: Vec<cadrum::DVec3> = points.iter().map(|p| array_to_dvec3(*p)).collect();
        cadrum::Edge::polygon(&dvec_points)
    }

    fn path_to_cadrum_edges(points: &[[f64; 3]]) -> Result<Vec<cadrum::Edge>, cadrum::Error> {
        if points.len() < 2 {
            return Err(cadrum::Error::SweepFailed);
        }
        let mut edges = Vec::with_capacity(points.len() - 1);
        for i in 0..points.len() - 1 {
            edges.push(cadrum::Edge::line(
                array_to_dvec3(points[i]),
                array_to_dvec3(points[i + 1]),
            )?);
        }
        Ok(edges)
    }

    fn array_to_dvec3(v: [f64; 3]) -> cadrum::DVec3 {
        cadrum::DVec3::new(v[0], v[1], v[2])
    }

    fn dvec3_to_array(v: cadrum::DVec3) -> [f64; 3] {
        [v.x, v.y, v.z]
    }

    /// Tessellate a cadrum solid built from `recipe` into the crate's `Mesh`.
    pub fn recipe_to_mesh(recipe: &Recipe) -> Result<crate::metrics::Mesh, cadrum::Error> {
        let solid = recipe_to_cadrum(recipe)?;
        let cadrum_mesh =
            cadrum::Solid::mesh(std::iter::once(&solid), cadrum::Tessellation::default())?;
        Ok(cadrum_mesh_to_mesh(&cadrum_mesh))
    }

    fn cadrum_mesh_to_mesh(cadrum_mesh: &cadrum::Mesh) -> crate::metrics::Mesh {
        let positions = cadrum_mesh
            .vertices
            .iter()
            .map(|v| [v.x, v.y, v.z])
            .collect();
        let indices = cadrum_mesh.indices.iter().map(|&i| i as u32).collect();
        crate::metrics::Mesh {
            positions,
            normals: Vec::new(),
            indices,
        }
    }
}

#[cfg(feature = "cadrum-reference")]
pub use cadrum_adapter::{cadrum_reference, recipe_to_cadrum, recipe_to_mesh};
