// Wavefront OBJ mesh importer.
//
// Parses vertex positions, optional normals, and triangle/quad faces.
// Quads are split into two triangles.
// Only the first object/group is imported (no multi-object support needed).

use crate::scene::model::mesh_model::MeshModel;

/// Parse OBJ text into a MeshModel.
/// Returns `None` if the file has no usable geometry.
pub fn parse_obj(src: &str, color: [f32; 4]) -> Option<MeshModel> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals_raw: Vec<[f32; 3]> = Vec::new();
    // Each face vertex: (pos_idx, normal_idx_opt)
    let mut face_verts: Vec<(usize, Option<usize>)> = Vec::new();

    for line in src.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let keyword = parts.next().unwrap_or("");

        match keyword {
            "v" => {
                let vals: Vec<f32> = parts.filter_map(|s| s.parse().ok()).collect();
                if vals.len() >= 3 {
                    // OBJ is Y-up right-handed; OpenCADStudio's world is Z-up.
                    // Rotate +90° about X so OBJ Y→world Z (up): (x, y, z) → (x, -z, y).
                    positions.push([vals[0], -vals[2], vals[1]]);
                }
            }
            "vn" => {
                let vals: Vec<f32> = parts.filter_map(|s| s.parse().ok()).collect();
                if vals.len() >= 3 {
                    normals_raw.push([vals[0], -vals[2], vals[1]]);
                }
            }
            "f" => {
                // Collect vertex descriptors "v", "v/vt", "v/vt/vn", "v//vn"
                let descs: Vec<(usize, Option<usize>)> = parts
                    .filter_map(|token| {
                        let mut it = token.split('/');
                        let pos_i: usize = it.next()?.parse::<i32>().ok()
                            .map(|i| if i < 0 { positions.len() as i32 + i } else { i - 1 })? as usize;
                        it.next(); // skip vt
                        let norm_i = it.next().and_then(|s| s.parse::<i32>().ok())
                            .map(|i| if i < 0 { normals_raw.len() as i32 + i } else { i - 1 } as usize);
                        Some((pos_i, norm_i))
                    })
                    .collect();
                // Fan-triangulate: (0,1,2), (0,2,3), …
                for k in 1..(descs.len() as isize - 1) {
                    face_verts.push(descs[0]);
                    face_verts.push(descs[k as usize]);
                    face_verts.push(descs[k as usize + 1]);
                }
            }
            _ => {}
        }
    }

    if positions.is_empty() || face_verts.is_empty() {
        return None;
    }

    // Build flat (un-indexed) vertex + normal arrays.
    let mut verts: Vec<[f32; 3]> = Vec::with_capacity(face_verts.len());
    let mut norms: Vec<[f32; 3]> = Vec::with_capacity(face_verts.len());
    let mut indices: Vec<u32> = Vec::with_capacity(face_verts.len());

    for (vi, (pos_i, norm_i)) in face_verts.iter().enumerate() {
        let pos = *positions.get(*pos_i).unwrap_or(&[0.0; 3]);
        verts.push(pos);
        let norm = norm_i
            .and_then(|ni| normals_raw.get(ni).copied())
            .unwrap_or([0.0, 0.0, 0.0]);
        norms.push(norm);
        indices.push(vi as u32);
    }

    // If no normals were provided in the OBJ file, compute face normals.
    if normals_raw.is_empty() {
        for tri in indices.chunks_exact(3) {
            let a = verts[tri[0] as usize];
            let b = verts[tri[1] as usize];
            let c = verts[tri[2] as usize];
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let nx = ab[1] * ac[2] - ab[2] * ac[1];
            let ny = ab[2] * ac[0] - ab[0] * ac[2];
            let nz = ab[0] * ac[1] - ab[1] * ac[0];
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-12);
            let n = [nx / len, ny / len, nz / len];
            norms[tri[0] as usize] = n;
            norms[tri[1] as usize] = n;
            norms[tri[2] as usize] = n;
        }
    }

    Some(MeshModel {
        name: String::new(),
        verts,
        normals: norms,
        indices,
        color,
        selected: false,
    })
}
