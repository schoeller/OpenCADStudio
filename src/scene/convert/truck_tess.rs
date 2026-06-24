// Unified tessellation for all truck topology levels.
//
//   Vertex              → TruckTessResult::Point
//   Edge                → TruckTessResult::Lines   (curve sampled with tolerance)
//   Wire                → TruckTessResult::Lines   (edges joined, no duplicate junctions)
//   Face / Shell / Solid → TruckTessResult::Mesh    (triangle mesh via truck-meshalgo)
//
// The caller (acad_to_truck / tessellate) decides which function to call based on
// the truck topology level of the object.

#[cfg(feature = "solid3d")]
use truck_meshalgo::tessellation::{MeshableShape, MeshedShape};
use truck_modeling::base::{BoundedCurve, ParameterDivision1D};
use truck_modeling::{Edge, Shell, Solid, Vertex, Wire};
#[cfg(feature = "solid3d")]
use truck_polymesh::PolygonMesh;


// Chord-height tolerance used for adaptive curve sampling (world units).
const CURVE_TOL: f64 = 0.005;
// Triangle mesh tolerance (world units).
#[allow(dead_code)]
pub const MESH_TOL: f64 = 0.01;

// ── Zoom-adaptive curve tolerance override ────────────────────────────────
//
// The Scene sets this once per render frame to a value derived from
// `world_per_pixel` (target ≈ 0.5 px chord height). All Edge tessellations
// inside the frame — including those running on rayon worker threads — read
// from the same atomic, so callers don't need to thread a tolerance
// parameter through every entity-converter signature. Zero means "use
// `CURVE_TOL`"; that is the default, and what BlockCache::build expects.
use std::sync::atomic::{AtomicU64, Ordering};
static CURVE_TOL_BITS: AtomicU64 = AtomicU64::new(0);

/// Set the per-frame curve tolerance. `None` (or any non-finite/<=0 value)
/// reverts to the default `CURVE_TOL`.
pub fn set_curve_tol_override(tol: Option<f64>) {
    let bits = match tol {
        Some(t) if t > 0.0 && t.is_finite() => t.to_bits(),
        _ => 0,
    };
    CURVE_TOL_BITS.store(bits, Ordering::Relaxed);
}

/// Resolve the active curve tolerance. Clamped to the floor `CURVE_TOL` so
/// extreme zoom-in never under-samples below the existing baseline quality.
pub(crate) fn current_curve_tol() -> f64 {
    let bits = CURVE_TOL_BITS.load(Ordering::Relaxed);
    if bits == 0 {
        CURVE_TOL
    } else {
        f64::from_bits(bits).max(CURVE_TOL)
    }
}

/// Returns `Some(tol)` only when a Scene-driven per-frame override is
/// active (i.e. we're tessellating inside a render frame, not at load
/// time or in a snap / hit-test pass). Hatch boundary outlines use this
/// to apply zoom-adaptive sampling.
pub(crate) fn active_curve_tol() -> Option<f64> {
    let bits = CURVE_TOL_BITS.load(Ordering::Relaxed);
    if bits == 0 {
        None
    } else {
        Some(f64::from_bits(bits).max(CURVE_TOL))
    }
}

// ── Public result type ────────────────────────────────────────────────────

/// Output of any truck topology tessellation.
#[allow(dead_code)]
pub enum TruckTessResult {
    /// A single world-space point (from Vertex). Double-single (high, low).
    Point([f32; 3], [f32; 3]),
    /// An ordered sequence of points forming a polyline (from Edge or Wire),
    /// returned as parallel double-single (high, low) f32 buffers so the GPU
    /// keeps sub-unit precision at UTM-scale coordinates.
    Lines(Vec<[f32; 3]>, Vec<[f32; 3]>),
    /// A triangle mesh (from Face, Shell, or Solid). `verts` is the high half
    /// of the double-single position; `verts_low` the paired residual.
    Mesh {
        verts: Vec<[f32; 3]>,
        verts_low: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        indices: Vec<u32>,
    },
}

// ── Shared offset helper ──────────────────────────────────────────────────

/// Convert a truck f64 point to local f32 by subtracting world_offset in f64
/// before truncating.  This preserves sub-unit decimal precision even when the
/// raw world coordinates are in the millions (e.g. Turkish UTM).
#[inline]
fn to_local(x: f64, y: f64, z: f64) -> [f32; 3] {
    [
        x as f32,
        y as f32,
        z as f32,
    ]
}

/// Sub-f32 residual of `to_local`, computed in f64 — the renderer pairs this
/// with the high half so the double-single shader path reconstructs the f64
/// source even when the high half is quantized to half a metre at UTM scale.
#[inline]
fn to_local_low(x: f64, y: f64, z: f64, hi: [f32; 3]) -> [f32; 3] {
    [
        (x - hi[0] as f64) as f32,
        (y - hi[1] as f64) as f32,
        (z - hi[2] as f64) as f32,
    ]
}

// ── Vertex ────────────────────────────────────────────────────────────────

pub fn tessellate_vertex(v: &Vertex) -> TruckTessResult {
    let p = v.point();
    let hi = to_local(p.x, p.y, p.z);
    let lo = to_local_low(p.x, p.y, p.z, hi);
    TruckTessResult::Point(hi, lo)
}

// ── Edge ──────────────────────────────────────────────────────────────────

pub fn tessellate_edge(e: &Edge) -> TruckTessResult {
    // oriented_curve() respects the edge direction (inverts if needed).
    let curve = e.oriented_curve();
    let (t0, t1) = curve.range_tuple();
    // parameter_division samples adaptively: fewer points on straight segments,
    // more on tight curves, all within the active chord-height tolerance.
    // The Scene scales this with zoom so far-out arcs aren't oversampled.
    let (_, pts) = curve.parameter_division((t0, t1), current_curve_tol());
    let mut high: Vec<[f32; 3]> = Vec::with_capacity(pts.len());
    let mut low: Vec<[f32; 3]> = Vec::with_capacity(pts.len());
    for p in &pts {
        let hi = to_local(p.x, p.y, p.z);
        let lo = to_local_low(p.x, p.y, p.z, hi);
        high.push(hi);
        low.push(lo);
    }
    TruckTessResult::Lines(high, low)
}

// ── Wire ──────────────────────────────────────────────────────────────────

pub fn tessellate_wire(w: &Wire) -> TruckTessResult {
    let mut high: Vec<[f32; 3]> = Vec::new();
    let mut low: Vec<[f32; 3]> = Vec::new();
    for edge in w.edge_iter() {
        if let TruckTessResult::Lines(eh, el) = tessellate_edge(edge) {
            if high.is_empty() {
                high = eh;
                low = el;
            } else {
                // Skip the first point of each continuation edge to avoid
                // duplicating the shared junction vertex.
                high.extend_from_slice(&eh[1..]);
                low.extend_from_slice(&el[1..]);
            }
        }
    }
    TruckTessResult::Lines(high, low)
}

// ── Shell ─────────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[cfg(feature = "solid3d")]
pub fn tessellate_shell(s: &Shell) -> TruckTessResult {
    let meshed = s.triangulation(MESH_TOL);
    polygon_to_result(meshed.to_polygon())
}

/// Without `solid3d` (e.g. wasm) there is no mesher; a shell yields no mesh.
#[cfg(not(feature = "solid3d"))]
pub fn tessellate_shell(_s: &Shell) -> TruckTessResult {
    TruckTessResult::Mesh {
        verts: Vec::new(),
        verts_low: Vec::new(),
        normals: Vec::new(),
        indices: Vec::new(),
    }
}

// ── Solid ─────────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[cfg(feature = "solid3d")]
pub fn tessellate_solid(s: &Solid) -> TruckTessResult {
    let meshed = s.triangulation(MESH_TOL);
    polygon_to_result(meshed.to_polygon())
}

/// Without `solid3d` (e.g. wasm) there is no mesher; a solid yields no mesh.
#[allow(dead_code)]
#[cfg(not(feature = "solid3d"))]
pub fn tessellate_solid(_s: &Solid) -> TruckTessResult {
    TruckTessResult::Mesh {
        verts: Vec::new(),
        verts_low: Vec::new(),
        normals: Vec::new(),
        indices: Vec::new(),
    }
}

// ── Internal ─────────────────────────────────────────────────────────────

#[cfg(feature = "solid3d")]
fn polygon_to_result(mesh: PolygonMesh) -> TruckTessResult {
    let positions = mesh.positions();
    let mut verts: Vec<[f32; 3]> = Vec::with_capacity(positions.len());
    let mut verts_low: Vec<[f32; 3]> = Vec::with_capacity(positions.len());
    for p in positions.iter() {
        let hi = to_local(p.x, p.y, p.z);
        verts_low.push(to_local_low(p.x, p.y, p.z, hi));
        verts.push(hi);
    }

    // Per-vertex normals: if the mesh has normals, map each triangle vertex's
    // normal index back to the normals array.  Fall back to empty if absent.
    let mesh_normals = mesh.normals();

    let indices: Vec<u32> = mesh
        .tri_faces()
        .iter()
        .flat_map(|tri| [tri[0].pos as u32, tri[1].pos as u32, tri[2].pos as u32])
        .collect();

    // Build a per-position normal by averaging normals of all faces that share it.
    // If mesh has no normals, leave the Vec empty.
    let normals: Vec<[f32; 3]> = if !mesh_normals.is_empty() {
        let n = verts.len();
        let mut acc = vec![[0.0_f32; 3]; n];
        let mut cnt = vec![0u32; n];
        for tri in mesh.tri_faces() {
            for sv in tri {
                let pos_idx = sv.pos;
                if let Some(nor_idx) = sv.nor {
                    if let Some(nv) = mesh_normals.get(nor_idx) {
                        acc[pos_idx][0] += nv.x as f32;
                        acc[pos_idx][1] += nv.y as f32;
                        acc[pos_idx][2] += nv.z as f32;
                        cnt[pos_idx] += 1;
                    }
                }
            }
        }
        acc.iter()
            .zip(cnt.iter())
            .map(|(s, &c)| {
                if c == 0 {
                    [0.0, 1.0, 0.0]
                } else {
                    let inv = 1.0 / c as f32;
                    let nx = s[0] * inv;
                    let ny = s[1] * inv;
                    let nz = s[2] * inv;
                    let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-9);
                    [nx / len, ny / len, nz / len]
                }
            })
            .collect()
    } else {
        vec![]
    };

    TruckTessResult::Mesh {
        verts,
        verts_low,
        normals,
        indices,
    }
}
