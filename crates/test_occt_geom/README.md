# `test_occt_geom` — OCS geometric-kernel correctness harness

This crate implements the strategy for verifying the OCS geometric kernel
against known-good reference test cases.  The default test path uses committed
golden JSON files; an optional in-process `cadrum` reference kernel can
regenerate those goldens and run live cross-checks.

## Strategy overview

1. **Treat `cadrum` as an optional reference kernel.**
   `cadrum` provides mass properties through a statically linked reference
   kernel.  It is enabled only via the `cadrum-reference` feature so default
   builds stay fast.

2. **Map each case to an OCS recipe.**
   A case is represented as a Rust [`Recipe`](src/geom.rs) value
   (`Primitive`, `BooleanOp`, …) that builds the same geometry through the OCS
   kernel wrapper (`OcsKernel`).  This keeps the tests declarative and avoids
   scattering kernel calls across many test files.

3. **Compute kernel-agnostic invariants.**
   Instead of comparing exact B-reps or meshes (which differ between kernels),
   the harness extracts scalar properties from a tessellation:
   * volume
   * surface area
   * centroid
   * axis-aligned bounding box
   * triangle count
   These values can be compared against analytic solutions or against
   pre-computed `cadrum` references.

4. **Use three reference sources.**
    * **Analytic** — exact formulas for primitives (sphere, cylinder, box, …).
    * **Golden files** — JSON snapshots produced from `cadrum` (for booleans)
      or analytic values (for primitives) and stored under `data/`.  These are
      the default reference for `cargo test`.
    * **Runtime `cadrum` adapter** — when the `cadrum-reference` feature is
      enabled, an in-process adapter rebuilds the same `Recipe` in `cadrum`
      and compares mass properties against the OCS kernel.

5. **Keep everything inside this crate.**
   No other crate in the workspace is modified.  `test_occt_geom` depends on the
   workspace root as a library so it exercises the real OCS geometry layer
   (`src/scene/model/solid_model.rs`, `src/scene/convert/truck_tess.rs`,
   truck-shapeops booleans).

## Running the tests

```bash
cargo test -p test_occt_geom
```

This compares OCS metrics against the committed golden files in `data/`.
No external kernel installation is required.

## Runtime `cadrum` cross-check

Enable the `cadrum-reference` feature to run every catalog case through both
OCS and the in-process `cadrum` kernel:

```bash
cargo test -p test_occt_geom --features cadrum-reference --test cadrum_adapter
cargo test -p test_occt_geom --features cadrum-reference --test mesh_comparison
```

`cadrum_adapter` compares volume, centroid, and (for primitives) surface area.
`mesh_comparison` tessellates both kernels and compares the resulting meshes
using the symmetric Hausdorff distance.  Some edge cases produce different
topologies in the two kernels; these are reported as warnings but do not fail
the test.

`cadrum` downloads and statically links prebuilt binaries, so the first build
with `cadrum-reference` may take a while.  On unsupported targets, enable the
`source-build` feature of `cadrum`.

## Regenerating golden files

With the `cadrum-reference` feature enabled, set the environment variable
`TEST_GEOM_REGENERATE_GOLDENS=1` and run the generator binary to refresh the
JSON files under `data/`:

```bash
# Unix shell
export TEST_GEOM_REGENERATE_GOLDENS=1
cargo run -p test_occt_geom --bin generate-goldens --features cadrum-reference
```

```powershell
# PowerShell
$env:TEST_GEOM_REGENERATE_GOLDENS = "1"
cargo run -p test_occt_geom --bin generate-goldens --features cadrum-reference
```

```cmd
:: Windows Command Prompt
set TEST_GEOM_REGENERATE_GOLDENS=1
cargo run -p test_occt_geom --bin generate-goldens --features cadrum-reference
```

> **Note:** On Windows, Windows Defender Controlled Folder Access may block
> the generator from overwriting files under `Documents`.  If you see
> `Zugriff verweigert` / `Access denied`, either disable Controlled Folder
> Access for the build or run the generator from a directory that is not
> protected and copy the resulting `data/` files into place.

## Extending the catalog

1. Add a new [`Primitive`](src/geom.rs) variant or a new [`Recipe`](src/geom.rs)
   variant (extrude / revolve / loft / sweep) if the OCS kernel already supports
   the operation.
2. Add analytic expected values in `src/reference.rs` or a golden file in
   `data/<group>/<name>.golden.json`.
3. Add a case to `src/cadrum.rs`.
4. Use `assert_metrics` to compare actual and expected values with appropriate
   scalar tolerances, and set `tolerances.hausdorff_abs` / `tolerances.hausdorff_rel`
   when the case is exercised by the optional Hausdorff cross-check.

## Future work

* Add denser point sampling for the Hausdorff distance (edge midpoints /
   triangle centroids) to tighten the mesh-to-mesh comparison.

