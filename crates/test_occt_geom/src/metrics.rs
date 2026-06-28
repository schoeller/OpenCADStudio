//! Mesh-derived scalar metrics used as geometry correctness invariants.

use std::path::{Path, PathBuf};
use truck_modeling::Solid;
use OpenCADStudio::scene::convert::truck_tess::{tessellate_solid, TruckTessResult};

/// Default absolute tolerance used by [`assert_metrics`].
pub const DEFAULT_ABS_TOL: f64 = 1e-3;
/// Default relative tolerance used by [`assert_metrics`].
pub const DEFAULT_REL_TOL: f64 = 1e-2;

/// A simple triangle mesh with double-precision positions.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Mesh {
    pub positions: Vec<[f64; 3]>,
    pub normals: Vec<[f64; 3]>,
    pub indices: Vec<u32>,
}

/// Scalar invariants extracted from a tessellated solid.
///
/// These values are kernel-agnostic: they can be computed from OCS output
/// and compared against analytic references or against the output of another
/// kernel such as the optional `cadrum` reference kernel.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GeomMetrics {
    pub volume: f64,
    pub surface_area: f64,
    pub centroid: [f64; 3],
    pub bbox_min: [f64; 3],
    pub bbox_max: [f64; 3],
    pub triangle_count: usize,
}

/// Reference values with optional fields.  Tests compare only the fields that
/// are provided, which makes it easy to ignore tessellation-dependent values
/// such as `triangle_count` or hard-to-compute boolean surface areas.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ExpectedMetrics {
    pub volume: Option<f64>,
    pub surface_area: Option<f64>,
    pub centroid: Option<[f64; 3]>,
    pub bbox_min: Option<[f64; 3]>,
    pub bbox_max: Option<[f64; 3]>,
    pub triangle_count: Option<usize>,
}

impl ExpectedMetrics {
    /// Require every metric to match the given value.
    pub fn exact(metrics: &GeomMetrics) -> Self {
        Self {
            volume: Some(metrics.volume),
            surface_area: Some(metrics.surface_area),
            centroid: Some(metrics.centroid),
            bbox_min: Some(metrics.bbox_min),
            bbox_max: Some(metrics.bbox_max),
            triangle_count: Some(metrics.triangle_count),
        }
    }

    pub fn volume(mut self, v: f64) -> Self {
        self.volume = Some(v);
        self
    }
    pub fn surface_area(mut self, v: f64) -> Self {
        self.surface_area = Some(v);
        self
    }
    pub fn centroid(mut self, v: [f64; 3]) -> Self {
        self.centroid = Some(v);
        self
    }
    pub fn bbox_min(mut self, v: [f64; 3]) -> Self {
        self.bbox_min = Some(v);
        self
    }
    pub fn bbox_max(mut self, v: [f64; 3]) -> Self {
        self.bbox_max = Some(v);
        self
    }
    pub fn triangle_count(mut self, v: usize) -> Self {
        self.triangle_count = Some(v);
        self
    }
}

/// Tessellate a truck `Solid` into a plain [`Mesh`].
pub fn solid_to_mesh(solid: &Solid) -> Mesh {
    match tessellate_solid(solid) {
        TruckTessResult::Mesh {
            verts,
            verts_low,
            normals,
            indices,
        } => {
            let positions: Vec<[f64; 3]> = verts
                .iter()
                .zip(verts_low.iter())
                .map(|(hi, lo)| {
                    [
                        hi[0] as f64 + lo[0] as f64,
                        hi[1] as f64 + lo[1] as f64,
                        hi[2] as f64 + lo[2] as f64,
                    ]
                })
                .collect();
            let normals_f64: Vec<[f64; 3]> = normals
                .iter()
                .map(|n| [n[0] as f64, n[1] as f64, n[2] as f64])
                .collect();
            Mesh {
                positions,
                normals: normals_f64,
                indices,
            }
        }
        _ => Mesh::default(),
    }
}

/// Compute [`GeomMetrics`] from a [`Mesh`].
pub fn mesh_metrics(mesh: &Mesh) -> GeomMetrics {
    let mut metrics = GeomMetrics {
        triangle_count: mesh.indices.len() / 3,
        ..GeomMetrics::default()
    };

    if mesh.positions.is_empty() {
        return metrics;
    }

    // Bounding box.
    let mut min = mesh.positions[0];
    let mut max = mesh.positions[0];
    for p in &mesh.positions {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    metrics.bbox_min = min;
    metrics.bbox_max = max;

    // Signed volume, surface area and centroid via the divergence theorem.
    // For a closed, outward-facing triangle mesh:
    //   V  = (1/6) * Σ  dot(p0, p1 × p2)
    //   C  = (1/(4*Σ dot(p0, p1 × p2))) * Σ dot(p0, p1 × p2) * (p0+p1+p2)
    let mut vol6 = 0.0;
    let mut centroid_sum = [0.0; 3];
    let mut area = 0.0;

    for tri in mesh.indices.chunks_exact(3) {
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        let p0 = mesh.positions[i0];
        let p1 = mesh.positions[i1];
        let p2 = mesh.positions[i2];

        let e0 = sub(&p1, &p0);
        let e1 = sub(&p2, &p0);
        let n = cross(&e0, &e1);
        area += 0.5 * norm(&n);

        let v6 = dot(&p0, &cross(&p1, &p2));
        vol6 += v6;

        for (i, sum) in centroid_sum.iter_mut().enumerate() {
            *sum += v6 * (p0[i] + p1[i] + p2[i]);
        }
    }

    metrics.volume = vol6.abs() / 6.0;
    metrics.surface_area = area;

    if vol6.abs() > 1e-18 {
        let inv = 1.0 / (4.0 * vol6);
        for (i, c) in metrics.centroid.iter_mut().enumerate() {
            *c = centroid_sum[i] * inv;
        }
    } else {
        metrics.centroid = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
    }

    metrics
}

/// Convenience: tessellate `solid` and compute its metrics.
pub fn solid_metrics(solid: &Solid) -> GeomMetrics {
    mesh_metrics(&solid_to_mesh(solid))
}

/// Compute the axis-aligned bounding box diagonal of a mesh.
///
/// Returns `0.0` for an empty mesh.
pub fn bbox_diagonal(mesh: &Mesh) -> f64 {
    if mesh.positions.is_empty() {
        return 0.0;
    }
    let mut min = mesh.positions[0];
    let mut max = mesh.positions[0];
    for p in &mesh.positions {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    norm(&sub(&max, &min))
}

/// Directed Hausdorff distance from `from` to `to`.
///
/// For every vertex in `from`, this finds the closest point on any triangle of
/// `to` and returns the maximum of those distances.  This is a discrete
/// approximation that works well when both meshes are reasonably tessellated.
///
/// Returns `0.0` if either mesh is empty.
pub fn directed_hausdorff(from: &Mesh, to: &Mesh) -> f64 {
    if from.positions.is_empty() || to.indices.len() < 3 {
        return 0.0;
    }
    let mut max_dist = 0.0;
    for p in &from.positions {
        let mut min_dist = f64::INFINITY;
        for tri in to.indices.chunks_exact(3) {
            let i0 = tri[0] as usize;
            let i1 = tri[1] as usize;
            let i2 = tri[2] as usize;
            let q = closest_point_on_triangle(
                p,
                &to.positions[i0],
                &to.positions[i1],
                &to.positions[i2],
            );
            let d = norm(&sub(p, &q));
            if d < min_dist {
                min_dist = d;
            }
        }
        if min_dist > max_dist {
            max_dist = min_dist;
        }
    }
    max_dist
}

/// Symmetric Hausdorff distance between two meshes.
///
/// This is the maximum of the directed distances in both directions, making it
/// sensitive to local over- and under-shooting on either surface.
pub fn symmetric_hausdorff(a: &Mesh, b: &Mesh) -> f64 {
    directed_hausdorff(a, b).max(directed_hausdorff(b, a))
}

fn closest_point_on_triangle(p: &[f64; 3], a: &[f64; 3], b: &[f64; 3], c: &[f64; 3]) -> [f64; 3] {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(&ab, &ap);
    let d2 = dot(&ac, &ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return *a;
    }

    let bp = sub(p, b);
    let d3 = dot(&ab, &bp);
    let d4 = dot(&ac, &bp);
    if d3 >= 0.0 && d4 <= d3 {
        return *b;
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return add(a, &scale(&ab, v));
    }

    let cp = sub(p, c);
    let d5 = dot(&ab, &cp);
    let d6 = dot(&ac, &cp);
    if d6 >= 0.0 && d5 <= d6 {
        return *c;
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return add(a, &scale(&ac, w));
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, &scale(&sub(c, b), w));
    }

    // Inside the triangle face.
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    add(a, &add(&scale(&ab, v), &scale(&ac, w)))
}

/// Compare `actual` against the fields set in `expected`.
///
/// `abs` is the absolute tolerance; `rel` is the relative tolerance applied to
/// the larger of the two values.  Centroid and bounding boxes are checked per
/// axis.
pub fn assert_metrics(actual: &GeomMetrics, expected: &ExpectedMetrics, abs: f64, rel: f64) {
    if let Some(v) = expected.volume {
        assert_close(v, actual.volume, abs, rel, "volume");
    }
    if let Some(v) = expected.surface_area {
        assert_close(v, actual.surface_area, abs, rel, "surface_area");
    }
    if let Some(v) = expected.centroid {
        assert_close_3(v, actual.centroid, abs, rel, "centroid");
    }
    if let Some(v) = expected.bbox_min {
        assert_close_3(v, actual.bbox_min, abs, rel, "bbox_min");
    }
    if let Some(v) = expected.bbox_max {
        assert_close_3(v, actual.bbox_max, abs, rel, "bbox_max");
    }
    if let Some(v) = expected.triangle_count {
        assert_eq!(
            actual.triangle_count, v,
            "triangle_count mismatch: got {}, expected {}",
            actual.triangle_count, v
        );
    }
}

fn assert_close(expected: f64, actual: f64, abs: f64, rel: f64, label: &str) {
    let diff = (expected - actual).abs();
    let scale = expected.abs().max(actual.abs()).max(1e-18);
    assert!(
        diff <= abs + rel * scale,
        "{label} mismatch: expected {expected:.12e}, got {actual:.12e} (diff {diff:.12e})",
    );
}

fn assert_close_3(expected: [f64; 3], actual: [f64; 3], abs: f64, rel: f64, label: &str) {
    for i in 0..3 {
        assert_close(expected[i], actual[i], abs, rel, &format!("{label}[{i}]"));
    }
}

fn sub(a: &[f64; 3], b: &[f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: &[f64; 3], b: &[f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(v: &[f64; 3], s: f64) -> [f64; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

fn cross(a: &[f64; 3], b: &[f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(v: &[f64; 3]) -> f64 {
    dot(v, v).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_triangle_mesh(p0: [f64; 3], p1: [f64; 3], p2: [f64; 3]) -> Mesh {
        Mesh {
            positions: vec![p0, p1, p2],
            normals: Vec::new(),
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn identical_meshes_have_zero_hausdorff() {
        let m = single_triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert_eq!(directed_hausdorff(&m, &m), 0.0);
        assert_eq!(symmetric_hausdorff(&m, &m), 0.0);
    }

    #[test]
    fn translated_meshes_hausdorff_equals_shift() {
        let a = single_triangle_mesh([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = single_triangle_mesh([0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]);
        assert!((directed_hausdorff(&a, &b) - 2.0).abs() < 1e-9);
        assert!((symmetric_hausdorff(&a, &b) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn bbox_diagonal_computed_correctly() {
        let m = single_triangle_mesh([0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [0.0, 4.0, 0.0]);
        assert!((bbox_diagonal(&m) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn golden_tolerances_default_hausdorff_fields() {
        let text = r#"{
            "name": "test/case",
            "tolerances": { "abs": 1e-3, "rel": 1e-2 }
        }"#;
        let golden: GoldenFile = serde_json::from_str(text).unwrap();
        let (abs_tol, rel_tol) = golden.mesh_tolerances();
        assert_eq!(abs_tol, 1e-3);
        assert_eq!(rel_tol, 5e-2);
    }
}

// ── Golden file I/O ─────────────────────────────────────────────────────────

/// Tolerances stored with a golden file.
///
/// `abs` and `rel` are used for scalar metric comparisons. `hausdorff_abs` and
/// `hausdorff_rel` are used by the optional mesh-to-mesh Hausdorff cross-check.
/// Missing fields deserialize to the defaults.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Tolerances {
    pub abs: f64,
    pub rel: f64,
    pub hausdorff_abs: f64,
    pub hausdorff_rel: f64,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            abs: DEFAULT_ABS_TOL,
            rel: DEFAULT_REL_TOL,
            hausdorff_abs: 1e-3,
            hausdorff_rel: 5e-2,
        }
    }
}

/// On-disk golden file schema.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GoldenFile {
    pub name: String,
    /// Optional human-readable traceability snippet showing the cadrum
    /// construction equivalent to the recipe.  Ignored by the tests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_cadrum_rust: Option<String>,
    #[serde(default)]
    pub expected: ExpectedMetrics,
    #[serde(default)]
    pub tolerances: Tolerances,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Return the path to a golden file for a case named like `group/case`.
///
/// The path is rooted at `CARGO_MANIFEST_DIR/data` so it works both from
/// `cargo test` and from a binary run with `cargo run`.
pub fn golden_path(name: &str) -> PathBuf {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut path = manifest.join("data");
    for part in name.split('/') {
        path = path.join(part);
    }
    path.with_extension("golden.json")
}

/// Load a golden file by case name.
///
/// Fails with a helpful message if the file is missing, telling the user to
/// run the generator or create the file manually.
pub fn load_golden(name: &str) -> anyhow::Result<GoldenFile> {
    let path = golden_path(name);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        anyhow::anyhow!(
            "golden file not found for '{}': {}\n  expected: {}\n  Run the generator (--features cadrum-reference + TEST_GEOM_REGENERATE_GOLDENS=1) or create it manually.",
            name,
            e,
            path.display()
        )
    })?;
    let golden: GoldenFile = serde_json::from_str(&text).map_err(|e| {
        anyhow::anyhow!(
            "failed to parse golden file for '{}': {}\n  file: {}",
            name,
            e,
            path.display()
        )
    })?;
    Ok(golden)
}

/// Write a golden file, creating parent directories as needed.
pub fn save_golden(path: &Path, golden: &GoldenFile) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(golden)?;
    std::fs::write(path, text)?;
    Ok(())
}

impl GoldenFile {
    /// Return the scalar tolerances from the file, or defaults.
    pub fn tolerances(&self) -> (f64, f64) {
        (self.tolerances.abs, self.tolerances.rel)
    }

    /// Return the mesh tolerances (absolute and relative) for the Hausdorff
    /// cross-check, or defaults.
    pub fn mesh_tolerances(&self) -> (f64, f64) {
        (self.tolerances.hausdorff_abs, self.tolerances.hausdorff_rel)
    }
}
