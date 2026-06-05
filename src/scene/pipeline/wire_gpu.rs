// Wire GPU buffers — instanced quad rendering for thick lines.
//
// Each segment [A→B] is one INSTANCE; the vertex shader expands a 6-vertex
// unit quad whose corners are derived from `@builtin(vertex_index)`. This
// cuts upload bandwidth by ~6.5× versus the old layout (which duplicated
// the segment payload across six vertex records).
//
// NaN sentinel: text glyphs pack multiple disconnected strokes into one
// WireModel, separated by [NaN, NaN, NaN] points. Segments where either
// endpoint contains NaN are silently skipped during emission.
//
// Instance layout (76 bytes, stride = 76, step_mode = Instance):
//   pos_a          [f32; 3]   offset  0   12 B  — segment start (world)
//   pos_b          [f32; 3]   offset 12   12 B  — segment end   (world)
//   color          [u8;  4]   offset 24    4 B  — RGBA, Unorm8x4 → vec4<f32> in shader
//   distance_a     f32        offset 28    4 B  — arc-length at endpoint A
//   distance_b     f32        offset 32    4 B  — arc-length at endpoint B
//   half_width     f32        offset 36    4 B  — half line width in pixels
//   pattern_length f32        offset 40    4 B  — dash pattern total length
//   pat0           [f32; 4]   offset 44   16 B  — pattern elements 0-3
//   pat1           [f32; 4]   offset 60   16 B  — pattern elements 4-7
//                                          ------
//                                           76 B / instance

use crate::scene::wire_model::WireModel;
use iced::wgpu;
use rayon::prelude::*;

/// Allocate a VERTEX buffer with `mapped_at_creation` and write `data` directly
/// into the mapped slice. Skips the intermediate staging copy that
/// `create_buffer_init` performs and avoids holding a second `Vec` worth of
/// memory during upload — meaningful on cold open where wire buffers can run
/// into the hundreds of MB.
fn instance_buffer_mapped(
    device: &wgpu::Device,
    label: &str,
    data: &[WireInstance],
) -> wgpu::Buffer {
    let bytes: &[u8] = bytemuck::cast_slice(data);
    // wgpu rejects size-0 buffers; the renderer already guards `instance_count`
    // before issuing a draw, so a placeholder allocation is fine here.
    let size = bytes.len().max(std::mem::size_of::<WireInstance>()) as u64;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: true,
    });
    {
        let mut view = buf.slice(..).get_mapped_range_mut();
        view[..bytes.len()].copy_from_slice(bytes);
    }
    buf.unmap();
    buf
}

// ── Instance layout ───────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WireInstance {
    pub pos_a: [f32; 3],
    pub pos_b: [f32; 3],
    /// RGBA packed as `Unorm8x4` — the vertex shader receives a `vec4<f32>`
    /// in [0, 1] after the GPU does the conversion. 8 bits per channel is
    /// indistinguishable from f32 at 8-bit display output.
    pub color: [u8; 4],
    pub distance_a: f32,
    pub distance_b: f32,
    pub half_width: f32,
    pub pattern_length: f32,
    pub pat0: [f32; 4],
    pub pat1: [f32; 4],
    /// Normalized draw-order depth in (0,1); applied as a small clip-z bias
    /// in the shader so this wire orders against other entity types.
    pub draw_depth: f32,
}

impl WireInstance {
    pub fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WireInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                }, // pos_a
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                }, // pos_b
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Unorm8x4,
                }, // color
                wgpu::VertexAttribute {
                    offset: 28,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32,
                }, // distance_a
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32,
                }, // distance_b
                wgpu::VertexAttribute {
                    offset: 36,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32,
                }, // half_width
                wgpu::VertexAttribute {
                    offset: 40,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32,
                }, // pattern_length
                wgpu::VertexAttribute {
                    offset: 44,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x4,
                }, // pat0
                wgpu::VertexAttribute {
                    offset: 60,
                    shader_location: 8,
                    format: wgpu::VertexFormat::Float32x4,
                }, // pat1
                wgpu::VertexAttribute {
                    offset: 76,
                    shader_location: 9,
                    format: wgpu::VertexFormat::Float32,
                }, // draw_depth
            ],
        }
    }
}

// ── GPU handle ────────────────────────────────────────────────────────────

pub struct WireGpu {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
    /// Paper-space bbox [x0, y0, x1, y1] for GPU scissor clipping.
    /// Set only for viewport-projected wires; None for regular wires.
    pub vp_scissor: Option<[f32; 4]>,
    /// `true` when the source `WireModel` also carries `fill_tris`
    /// (i.e. it is a 3D mesh face — PolyfaceMesh / PolygonMesh — whose
    /// outline lives in `points`). The wire pass skips these instances
    /// in shaded modes so the surface reads as a clean solid; pure
    /// wireframe / HiddenLine / *WithEdges modes draw them.
    pub is_3d_mesh_edge: bool,
}

/// Expand one `WireModel` into its per-segment instance stream (1 instance per
/// finite segment). Pulled out so both the single-wire and batched paths share
/// the same emission logic, and so the batched path can `par_iter` across
/// wires on cold open.
fn pack_color(color: [f32; 4]) -> [u8; 4] {
    [
        (color[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (color[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
    ]
}

fn emit_wire_instances(wire: &WireModel, color: [f32; 4], draw_depth: f32) -> Vec<WireInstance> {
    let color_u8 = pack_color(color);
    let pat0 = [
        wire.pattern[0],
        wire.pattern[1],
        wire.pattern[2],
        wire.pattern[3],
    ];
    let pat1 = [
        wire.pattern[4],
        wire.pattern[5],
        wire.pattern[6],
        wire.pattern[7],
    ];
    let half_width = wire.line_weight_px * 0.5;

    let n = wire.points.len();
    let seg_count = n.saturating_sub(1);
    if seg_count == 0 {
        return Vec::new();
    }

    let mut dists = vec![0.0_f32; n];
    let mut has_break = false;
    for i in 1..n {
        let p = wire.points[i - 1];
        let q = wire.points[i];
        if !p[0].is_finite() || !q[0].is_finite() {
            has_break = true;
            // plinegen=false: reset to 0 at the first real point after a NaN separator.
            dists[i] = if !wire.plinegen && !p[0].is_finite() && q[0].is_finite() {
                0.0
            } else {
                dists[i - 1]
            };
        } else {
            let dx = q[0] - p[0];
            let dy = q[1] - p[1];
            let dz = q[2] - p[2];
            dists[i] = dists[i - 1] + (dx * dx + dy * dy + dz * dz).sqrt();
        }
    }

    // Center the dash pattern on the wire instead of starting it at the first
    // vertex. The shader reads the pattern phase as `dist % pattern_length`, so
    // adding a constant offset to every arc-length shifts that phase. We place
    // the wire midpoint at the center of the first dash element, which makes the
    // line begin and end with matching partial dashes. Skipped for wires with
    // NaN breaks (per-segment dash restarts), where a single offset can't center
    // every segment.
    let pat_len = wire.pattern_length;
    if pat_len > 1e-6 && !has_break && n >= 2 {
        let total = dists[n - 1];
        if total > 1e-6 {
            // First dash element (positive), else fall back to the first element.
            let first_dash = wire
                .pattern
                .iter()
                .copied()
                .find(|&v| v > 0.0)
                .unwrap_or_else(|| wire.pattern[0].abs());
            // Phase that puts the wire midpoint at the dash center.
            let offset = first_dash * 0.5 + total * 0.5;
            for d in dists.iter_mut() {
                *d += offset;
            }
        }
    }

    let mut instances: Vec<WireInstance> = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let a = wire.points[i];
        let b = wire.points[i + 1];
        if !a[0].is_finite()
            || !a[1].is_finite()
            || !a[2].is_finite()
            || !b[0].is_finite()
            || !b[1].is_finite()
            || !b[2].is_finite()
        {
            continue;
        }
        instances.push(WireInstance {
            pos_a: a,
            pos_b: b,
            color: color_u8,
            distance_a: dists[i],
            distance_b: dists[i + 1],
            half_width,
            pattern_length: wire.pattern_length,
            pat0,
            pat1,
            draw_depth,
        });
    }
    instances
}

/// Looks up a wire's draw-order depth from the per-entity map using the
/// handle encoded in its `name`. Falls back to 0.0 (transient / preview
/// wires that carry no document handle).
fn wire_draw_depth(wire: &WireModel, depth_map: &rustc_hash::FxHashMap<u64, f32>) -> f32 {
    wire
        .name
        .parse::<u64>()
        .ok()
        .and_then(|h| depth_map.get(&h).copied())
        .unwrap_or(0.0)
}

impl WireGpu {

    /// Merge a run of WireModels that share scissor + mesh-edge state into one
    /// (or, past the 256 MB GPU limit, a few) instance buffer(s), then stamp
    /// the shared `scissor` / `mesh_edge` onto each so the draw loop treats the
    /// whole run as a single batch.
    ///
    /// Unlike [`from_batch`], instance order is **guaranteed** to follow wire
    /// order (parallel `collect` is index-ordered; the flatten is sequential).
    /// The main wire pass depends on that — depth-biased overlap *and* alpha
    /// blending both resolve in submission order, so a reorder would change the
    /// image for transparent / coincident wires.
    pub fn from_run(
        device: &wgpu::Device,
        wires: &[WireModel],
        depth_map: &rustc_hash::FxHashMap<u64, f32>,
        scissor: Option<[f32; 4]>,
        mesh_edge: bool,
    ) -> Vec<Self> {
        use rayon::prelude::*;
        const MAX_INSTANCES: usize = 268_435_456 / std::mem::size_of::<WireInstance>();
        let per: Vec<Vec<WireInstance>> = wires
            .par_iter()
            .map(|w| emit_wire_instances(w, w.color, wire_draw_depth(w, depth_map)))
            .collect();
        let mut instances: Vec<WireInstance> = Vec::with_capacity(per.iter().map(Vec::len).sum());
        for mut v in per {
            instances.append(&mut v);
        }
        if instances.is_empty() {
            return vec![];
        }
        instances
            .chunks(MAX_INSTANCES)
            .map(|chunk| {
                let buf = instance_buffer_mapped(device, "wire.run.ibuf", chunk);
                Self {
                    instance_buffer: buf,
                    instance_count: chunk.len() as u32,
                    vp_scissor: scissor,
                    is_3d_mesh_edge: mesh_edge,
                }
            })
            .collect()
    }

    /// Merge multiple WireModels into GPU instance buffers, chunked to fit the
    /// 256 MB GPU limit. Each wire keeps its own color and pattern — they live
    /// per-instance.
    pub fn from_batch(
        device: &wgpu::Device,
        wires: &[WireModel],
        depth_map: &rustc_hash::FxHashMap<u64, f32>,
    ) -> Vec<Self> {
        let total_segs: usize = wires.iter().map(|w| w.points.len().saturating_sub(1)).sum();
        if total_segs == 0 {
            return vec![];
        }

        // Parallel per-wire instance emission. Each wire's stream is
        // independent — `block_cache` groups wires by style upstream, so order
        // within a batch does not affect correctness. `reduce` concatenates the
        // per-wire Vec<WireInstance> chunks while letting rayon spread work
        // across cores.
        let instances: Vec<WireInstance> = wires
            .par_iter()
            .map(|wire| emit_wire_instances(wire, wire.color, wire_draw_depth(wire, depth_map)))
            .reduce(Vec::new, |mut acc, mut chunk| {
                if acc.is_empty() {
                    chunk
                } else if chunk.is_empty() {
                    acc
                } else {
                    acc.append(&mut chunk);
                    acc
                }
            });

        if instances.is_empty() {
            return vec![];
        }

        // GPU max buffer size is 256 MB; chunk to stay within the limit.
        const MAX_INSTANCES: usize = 268_435_456 / std::mem::size_of::<WireInstance>();

        instances
            .chunks(MAX_INSTANCES)
            .enumerate()
            .map(|(i, chunk)| {
                let label = format!("wire.batch.ibuf.{i}");
                let instance_buffer = instance_buffer_mapped(device, &label, chunk);
                Self {
                    instance_buffer,
                    instance_count: chunk.len() as u32,
                    vp_scissor: None,
                    is_3d_mesh_edge: false,
                }
            })
            .collect()
    }
}
