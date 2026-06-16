// STL binary export — converts all tessellated MeshModels in the scene to a
// single binary STL file.
//
// Binary STL format:
//   80-byte header
//   4-byte triangle count (u32 LE)
//   Per triangle (50 bytes):
//     3 × f32 normal
//     3 × 3 × f32 vertices
//     2-byte attribute (0)

use std::io::Write;

use crate::scene::model::mesh_model::MeshModel;

/// Build a binary STL byte buffer from a slice of mesh models.
/// Returns `None` if there are no triangles to export.
pub fn build_stl(meshes: &[&MeshModel]) -> Option<Vec<u8>> {
    // Collect all triangles.
    struct Tri {
        normal: [f32; 3],
        v: [[f32; 3]; 3],
    }

    let mut tris: Vec<Tri> = Vec::new();

    for mesh in meshes {
        let verts = &mesh.verts;
        let idx = &mesh.indices;
        let n_tri = idx.len() / 3;
        for t in 0..n_tri {
            let i0 = idx[t * 3] as usize;
            let i1 = idx[t * 3 + 1] as usize;
            let i2 = idx[t * 3 + 2] as usize;
            if i0 >= verts.len() || i1 >= verts.len() || i2 >= verts.len() {
                continue;
            }
            let a = verts[i0];
            let b = verts[i1];
            let c = verts[i2];

            // Compute face normal.
            let normal = if !mesh.normals.is_empty() && i0 < mesh.normals.len() {
                mesh.normals[i0]
            } else {
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let nx = ab[1] * ac[2] - ab[2] * ac[1];
                let ny = ab[2] * ac[0] - ab[0] * ac[2];
                let nz = ab[0] * ac[1] - ab[1] * ac[0];
                let len = (nx * nx + ny * ny + nz * nz).sqrt().max(f32::EPSILON);
                [nx / len, ny / len, nz / len]
            };

            tris.push(Tri {
                normal,
                v: [a, b, c],
            });
        }
    }

    if tris.is_empty() {
        return None;
    }

    let mut buf: Vec<u8> = Vec::with_capacity(84 + tris.len() * 50);

    // 80-byte header.
    let mut header = [0u8; 80];
    let title = b"Open CAD Studio STL export";
    header[..title.len()].copy_from_slice(title);
    buf.extend_from_slice(&header);

    // Triangle count.
    buf.extend_from_slice(&(tris.len() as u32).to_le_bytes());

    for tri in &tris {
        // Normal.
        for &f in &tri.normal {
            buf.write_all(&f.to_le_bytes()).ok()?;
        }
        // Vertices.
        for v in &tri.v {
            for &f in v {
                buf.write_all(&f.to_le_bytes()).ok()?;
            }
        }
        // Attribute byte count = 0.
        buf.extend_from_slice(&0u16.to_le_bytes());
    }

    Some(buf)
}
