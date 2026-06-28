//! Optional cadrum (OpenCASCADE) reference adapter.
//!
//! This module is compiled only when the `cadrum-reference` feature is enabled.

use std::path::Path;
use crate::metrics::{mesh_metrics, GeomMetrics, Mesh};

/// Read a STEP file with cadrum and return the first solid.
///
/// Returns `Err` if the file cannot be read or contains no solid.
pub fn read_step_solid(path: &Path) -> Result<cadrum::Solid, cadrum::Error> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| cadrum::Error::Unknown(e.to_string().into()))?;
    let mut solids = cadrum::Solid::read_step(&mut file)?;
    solids
        .pop()
        .ok_or_else(|| cadrum::Error::Unknown("no solid in STEP file".into()))
}

/// Read a STEP file and convert the cadrum tessellation to the crate's `Mesh`.
#[allow(dead_code)]
pub fn cadrum_mesh(path: &Path) -> Result<Mesh, cadrum::Error> {
    let solid = read_step_solid(path)?;
    let cadrum_mesh = cadrum::Solid::mesh(
        std::iter::once(&solid),
        cadrum::Tessellation::default(),
    )?;
    let positions = cadrum_mesh
        .vertices
        .iter()
        .map(|v| [v.x, v.y, v.z])
        .collect();
    let indices = cadrum_mesh
        .indices
        .iter()
        .map(|&i| i as u32)
        .collect();
    Ok(Mesh {
        positions,
        normals: Vec::new(),
        indices,
    })
}

/// Compute `GeomMetrics` from a cadrum STEP solid.
///
/// Volume, surface area, centroid and bounding box come from cadrum's mass
/// properties. Triangle count comes from the tessellated mesh.
pub fn cadrum_metrics(path: &Path) -> Result<GeomMetrics, cadrum::Error> {
    let solid = read_step_solid(path)?;
    let [min, max] = solid.bounding_box();
    let mesh = cadrum_mesh(path)?;
    let mut metrics = mesh_metrics(&mesh);
    metrics.volume = solid.volume().abs();
    metrics.surface_area = solid.area();
    metrics.centroid = [solid.center().x, solid.center().y, solid.center().z];
    metrics.bbox_min = [min.x, min.y, min.z];
    metrics.bbox_max = [max.x, max.y, max.z];
    Ok(metrics)
}
