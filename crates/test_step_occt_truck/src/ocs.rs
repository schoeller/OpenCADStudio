//! OCS (truck) STEP reader.
//!
//! This module turns an AP214/STEP file into a truck `Solid` using the
//! published `truck-stepio` 0.3.0 API. The parsed shells use the STEP alias
//! curve/surface types (`Curve3D`, `Surface`) from `truck_stepio::r#in::alias`.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use truck_stepio::r#in::alias::{Curve3D, Point3, Surface};
use truck_stepio::r#in::Table;
use truck_topology::Solid;

/// The OCS solid type used by this crate.
pub type OcsSolid = Solid<Point3, Curve3D, Surface>;

use truck_topology::compress::{
    CompressedEdge, CompressedEdgeIndex, CompressedFace, CompressedShell,
};

/// Tolerance for merging vertices that are coincident in the STEP file but
/// emitted as separate entities.
const VERTEX_MERGE_TOL: f64 = 1e-6;

/// Read a STEP file and return all parsed shells as a single `OcsSolid`.
///
/// Returns `Err` if the file cannot be parsed or contains no shell.
pub fn step_solid<P: AsRef<Path>>(path: P) -> Result<OcsSolid, String> {
    let path = path.as_ref();
    let mut file = File::open(path)
        .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

    let table = Table::from_step(&text)
        .ok_or_else(|| format!("STEP parse failed for {}", path.display()))?;

    let mut shells = Vec::new();
    for (key, shell_holder) in table.shell.iter() {
        let compressed = table
            .to_compressed_shell(shell_holder)
            .map_err(|e| format!("shell {key} compression failed: {e:?}"))?;
        let compressed = merge_coincident_vertices(compressed, VERTEX_MERGE_TOL);
        let shell = truck_topology::Shell::<Point3, Curve3D, Surface>::extract(compressed)
            .map_err(|e| format!("shell {key} extraction failed: {e:?}"))?;
        shells.push(shell);
    }

    if shells.is_empty() {
        Err(format!("no shells found in {}", path.display()))
    } else {
        Ok(Solid::new_unchecked(shells))
    }
}

/// Merge vertices that are coincident within `tol` and drop any zero-length
/// edges that result from the merge.
///
/// Some STEP exporters emit distinct vertex entities for the same geometric
/// point.  truck's topology rejects the resulting zero-length edges, so we
/// collapse them before extraction.
fn merge_coincident_vertices(
    shell: CompressedShell<Point3, Curve3D, Surface>,
    tol: f64,
) -> CompressedShell<Point3, Curve3D, Surface> {
    let mut new_vertices: Vec<Point3> = Vec::new();
    let mut vertex_map = Vec::with_capacity(shell.vertices.len());

    for v in &shell.vertices {
        let mut matched = None;
        for (i, candidate) in new_vertices.iter().enumerate() {
            if (candidate.x - v.x).abs() <= tol
                && (candidate.y - v.y).abs() <= tol
                && (candidate.z - v.z).abs() <= tol
            {
                matched = Some(i);
                break;
            }
        }
        match matched {
            Some(idx) => vertex_map.push(idx),
            None => {
                vertex_map.push(new_vertices.len());
                new_vertices.push(*v);
            }
        }
    }

    let mut new_edges: Vec<CompressedEdge<Curve3D>> = Vec::new();
    let mut edge_map: Vec<Option<usize>> = Vec::with_capacity(shell.edges.len());

    for e in shell.edges {
        let v0 = vertex_map[e.vertices.0];
        let v1 = vertex_map[e.vertices.1];
        if v0 == v1 {
            edge_map.push(None);
        } else {
            let idx = new_edges.len();
            new_edges.push(CompressedEdge {
                vertices: (v0, v1),
                curve: e.curve,
            });
            edge_map.push(Some(idx));
        }
    }

    let mut new_faces: Vec<CompressedFace<Surface>> = Vec::new();
    for face in shell.faces {
        let mut boundaries: Vec<Vec<CompressedEdgeIndex>> = Vec::new();
        for wire in face.boundaries {
            let mut new_wire: Vec<CompressedEdgeIndex> = Vec::new();
            for ei in wire {
                if let Some(idx) = edge_map[ei.index] {
                    new_wire.push(CompressedEdgeIndex {
                        index: idx,
                        orientation: ei.orientation,
                    });
                }
            }
            if !new_wire.is_empty() {
                boundaries.push(new_wire);
            }
        }
        if !boundaries.is_empty() {
            new_faces.push(CompressedFace {
                boundaries,
                orientation: face.orientation,
                surface: face.surface,
            });
        }
    }

    CompressedShell {
        vertices: new_vertices,
        edges: new_edges,
        faces: new_faces,
    }
}
