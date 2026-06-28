# Plan: cadrum-based reference harness for the OCS geometric kernel

## Goal
Keep `crates/test_occt_geom` as the correctness harness for the OCS geometric
kernel. The harness must compare OCS geometry and operations against a
reference kernel indirectly, through the Rust crate `cadrum` (statically linked,
headless OpenCASCADE 8.0). Committed golden JSON files remain the default
reference for `cargo test`; the `cadrum` adapter is used only to regenerate
goldens and to run optional live cross-checks.

## Constraints
- **No direct OCCT use.** No external OpenCASCADE/OCCT binary, library, Tcl
  script, or `DRAWEXE` dependency is permitted. The only permitted path to
  OCCT is indirect, through the optional `cadrum` crate.
- All source, tests, golden files, and tooling stay in `crates/test_occt_geom`.
- Keep the crate name `test_occt_geom` to avoid churn.
- Do not modify the main OCS application kernel; consume it through
  `OpenCADStudio = { path = "../.." }`.
- Default `cargo test` must not require building `cadrum`. Keep `cadrum` an
  optional dependency behind the `cadrum-reference` feature.

## Decisions
1. **Reference model:** `cadrum`.
   - Committed golden JSON files are the default reference for `cargo test`.
   - When the `cadrum-reference` feature is enabled, the in-process adapter
     rebuilds the same `Recipe` in `cadrum` and compares mass properties.
2. **Feature gating:**
   - `cadrum = { version = "0.8", optional = true }` (or latest compatible
     version at implementation time).
   - Crate feature `cadrum-reference` enables the optional dependency.
   - Golden regeneration and the runtime cross-check compile only when the
     feature is enabled.
3. **Coverage (unchanged):** Phase-1 catalog of 18 cases: 6 primitives, 9
   booleans, 3 edge cases (see catalog below).
4. **Metrics compared:** volume, centroid, axis-aligned bbox by default.
   Surface area and triangle count are optional per case.
5. **Tolerances:** default `abs = 1e-3`, `rel = 1e-2`, with per-case overrides
   stored in the golden file.
6. **Recipe abstraction:** Keep the `Recipe` enum (`Primitive` / `Boolean`).
   Add a `cadrum` builder that walks a `Recipe` and returns a `cadrum::Solid`.
7. **Golden files:** `data/<group>/<name>.golden.json`, one per case.
8. **Single update executable:** only `src/bin/generate-goldens.rs` regenerates
   golden files when `cadrum-reference` is enabled and
   `TEST_GEOM_REGENERATE_GOLDENS=1` is set. The duplicate integration test
   `tests/generate_goldens.rs` is removed.
9. **Runtime cross-check:** `tests/cadrum_adapter.rs` compares OCS metrics
   against live `cadrum` metrics for every catalog case when the feature is
   enabled.
10. **No OCCT references in docs:** remove all `OCCT`, `DRAWEXE`, Tcl, and
    OCCT-DRAW-script references from `README.md` and future-work sections.

## Phase-1 catalog
Stored as `Recipe` values in `src/cadrum.rs` and golden files under `data/`.

Primitives:
1. `primitive/box_4x6x8_at_1_2_3`
2. `primitive/wedge_4x5x6_at_minus1_minus2_minus3`
3. `primitive/cylinder_r3_h10`
4. `primitive/cone_r4_h9`
5. `primitive/sphere_r5`
6. `primitive/torus_r8_r2`

Booleans:
7. `boolean/box_box_fuse`
8. `boolean/box_box_cut`
9. `boolean/box_box_common`
10. `boolean/box_cylinder_fuse`
11. `boolean/box_cylinder_cut`
12. `boolean/cylinder_sphere_common`
13. `boolean/sphere_box_fuse`
14. `boolean/torus_box_cut`
15. `boolean/cone_cylinder_fuse`

Edge cases:
16. `boolean/box_box_disjoint` — no overlap; expect empty/`None` result.
17. `boolean/cylinder_cylinder_tangent` — faces just touch.
18. `boolean/box_box_coincident_face` — partially shared face.

## Golden file schema (`data/<group>/<name>.golden.json`)
```json
{
  "name": "boolean/box_box_fuse",
  "recipe_cadrum_rust": "let a = Solid::cube(DVec3::new(-5.0,-5.0,-5.0), DVec3::new(5.0,5.0,5.0)); let b = Solid::cube(DVec3::new(0.0,0.0,0.0), DVec3::new(10.0,10.0,10.0)); let r = (&a + &b).build()?;",
  "expected": {
    "volume": 1875.0,
    "surface_area": null,
    "centroid": [2.5, 2.5, 2.5],
    "bbox_min": [-5.0, -5.0, -5.0],
    "bbox_max": [10.0, 10.0, 10.0],
    "triangle_count": null
  },
  "tolerances": {
    "abs": 1e-3,
    "rel": 1e-2,
    "hausdorff_abs": 1e-3,
    "hausdorff_rel": 5e-2
  },
  "note": "Reference regenerated from cadrum."
}
```
- `recipe_cadrum_rust` is optional, human-readable traceability only.
- `tolerances.hausdorff_abs` and `tolerances.hausdorff_rel` are optional and are
  used only by the optional `mesh_comparison` Hausdorff cross-check.
- For initial population, surface area and triangle count may be `null` for
  boolean cases. Primitive cases should include all metrics where practical.

## File structure
```
crates/test_occt_geom/
  Cargo.toml                  # optional cadrum dep + feature
  README.md                   # cadrum feature docs only, no OCCT/DRAWEXE/Tcl
  src/
    lib.rs                    # re-exports
    geom.rs                   # Primitive, BooleanOp, OcsKernel, Recipe
    metrics.rs                # GeomMetrics, ExpectedMetrics, assert_metrics, I/O
    reference.rs              # analytic_primitive, from_geom
    cadrum.rs                 # Case catalog, cadrum adapter, golden regeneration
    bin/
      generate-goldens.rs     # CLI golden generator (feature-gated)
  tests/
    primitives.rs             # primitive goldens -> OCS
    booleans.rs               # boolean goldens -> OCS
    edge_cases.rs             # edge-case goldens -> OCS
    cadrum_adapter.rs         # runtime OCS-vs-cadrum scalar cross-check
    mesh_comparison.rs        # runtime OCS-vs-cadrum Hausdorff cross-check
  data/
    primitive/                # *.golden.json
    boolean/                  # *.golden.json
    edge/                     # *.golden.json
```

## Implementation tasks (in order)
1. **Clean up documentation**
   - Remove all `OCCT`, `DRAWEXE`, Tcl, and OCCT-DRAW-script references from
     `README.md`.
   - Update the future-work section to focus on mesh-level comparison and
     extending the recipe language, not on parsing OCCT artifacts.

2. **Remove duplicate golden regeneration path**
   - Delete `tests/generate_goldens.rs`.
   - Update `README.md` to document only the binary update path:
     `cargo run -p test_occt_geom --bin generate-goldens --features cadrum-reference`.

3. **Clean the catalog data model**
   - Remove the unused `expected: ExpectedMetrics` field from `Case` in
     `src/cadrum.rs`.
   - Update `primitive_cases`, `boolean_cases`, and `edge_cases` to no longer
     compute or store in-code expected values; tests must read expectations
     from `data/*.golden.json` or from `reference::analytic_primitive`.

4. **Audit direct OCCT use**
   - Search the crate for any remaining `OCCT_DRAWEXE`, `drawexe`, Tcl,
     `OpenCASCADE`, or direct OCCT linking. Remove or replace with `cadrum`
     adapter code.
   - Verify `Cargo.toml` has no direct OCCT dependency outside the optional
     `cadrum` crate.

5. **Verify the cadrum adapter remains correct**
   - Confirm `recipe_to_cadrum` maps each `Primitive` to the equivalent
     `cadrum::Solid` call and that booleans use `&a + &b`, `&a - &b`,
     `&a * &b`.
   - Confirm `cadrum_reference` extracts volume, surface area, centroid, and
     bounding box and converts `DVec3` to `[f64; 3]`.

6. **Regenerate / review golden files**
   - With `cadrum-reference` enabled, run the single update binary to refresh
     `data/`.
   - Review diffs; accept expected changes where `cadrum` differs from
     hand-computed or OCS-seeded values.
   - Commit refreshed files under `data/`.

## `cadrum` API mapping notes
- `cadrum` uses `glam::DVec3` for points/directions. Convert to/from `[f64; 3]`
  at the adapter boundary.
- Booleans are implemented via operator overloads on references:
  `&a + &b`, `&a - &b`, `&a * &b`, followed by `.build()`.
- `Solid::cube(min, max)` takes the two opposite box corners.
- `Solid::cylinder(r, DVec3::Z * h)` and `Solid::cone(r1, r2, DVec3::Z * h)`
  are aligned along Z and extruded to the vector length.
- `Solid::sphere(r)` and `Solid::torus(major, minor, DVec3::Z)` are centered at
  the origin; apply `.translate(...)` to match OCS placement.
- `Solid::extrude(profile, dir)` builds a solid from a closed profile; the
  wedge is built from a right-triangle profile in XZ extruded along Y.
- `Solid::bounding_box()` returns `[DVec3; 2]` (min, max).

## Risks and mitigations
- **Build time / binary size:** `cadrum` downloads and statically links OCCT,
  which is large. Mitigation: keep it optional behind `cadrum-reference` so
  default tests do not pay the cost.
- **Platform support:** `cadrum` provides prebuilt binaries for a limited set
  of targets; on unsupported targets the `source-build` feature may be
  required. Document this in README and CI notes.
- **OCCT version differences:** `cadrum` pins OCCT 8.0.0; OCS uses
  `truck-shapeops`. Metric differences are expected and absorbed by
  tolerances; review golden diffs before committing.
- **Boolean robustness:** Some edge cases may fail in either kernel. The
  harness distinguishes "expected empty result" from "unexpected kernel
  failure" and reports both. If `cadrum` returns an error for a case, the
  generator falls back to analytic/OCS seeding and warns.
- **Feature-gated code drift:** Keep the feature-gated modules small and
  isolated so the default test path does not accidentally depend on `cadrum`.

## Validation
- `cargo test -p test_occt_geom` passes with committed goldens (no `cadrum`
  build).
- `cargo test -p test_occt_geom --features cadrum-reference --test cadrum_adapter` passes.
- `cargo run -p test_occt_geom --bin generate-goldens --features cadrum-reference`
  refreshes JSON files when `TEST_GEOM_REGENERATE_GOLDENS=1` is set.
- `cargo check -p test_occt_geom` and
  `cargo check -p test_occt_geom --features cadrum-reference` both succeed.
- No `OCCT_DRAWEXE`, `drawexe`, Tcl, or direct OCCT linkage remains in the
  crate.

## Open questions / future work
- Add denser point sampling for the Hausdorff distance (edge midpoints /
  triangle centroids) to tighten the mesh-to-mesh comparison.
