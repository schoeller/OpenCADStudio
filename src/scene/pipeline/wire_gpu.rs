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
// Instance layout (step_mode = Instance):
//   pos_a          [f32; 3]   — segment start (high half, world / offset-relative)
//   pos_a_low      [f32; 3]   — segment start low residual (double-single pair)
//   pos_b          [f32; 3]   — segment end (high)
//   pos_b_low      [f32; 3]   — segment end low residual
//   color          [u8;  4]   — RGBA, Unorm8x4 → vec4<f32> in shader
//   distance_a     f32        — arc-length at endpoint A
//   distance_b     f32        — arc-length at endpoint B
//   half_width     f32        — half line width in pixels
//   pattern_length f32        — dash pattern total length
//   pat0           [f32; 4]   — pattern elements 0-3
//   pat1           [f32; 4]   — pattern elements 4-7
//   draw_depth     f32        — normalized draw-order depth bias
// The high+low pair encodes the f64 source so the relative-to-eye shader
// stays precise at UTM-scale coordinates and after a cross-drawing paste.

use crate::scene::model::wire_model::WireModel;
use iced::wgpu;

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

// ── Native: slim per-segment instance + shared per-wire constants ───────────
//
// Every segment of a wire used to carry the wire's color / line-weight / dash
// pattern / draw-depth (~44 B) on each instance — re-fetched once per segment
// even though it's constant along the wire. On native we hoist those into a
// per-wire `WireConst` storage buffer indexed by `wire_id`, so the instance
// keeps only the per-segment data (endpoints + arc-length distances). Cuts the
// instance from 104 B to 60 B (~42 %) and removes the redundant per-segment
// re-fetch of the shared constants. WebGL2 has no vertex-stage storage buffers,
// so the wasm build below keeps the original self-contained fat instance.
#[cfg(not(target_arch = "wasm32"))]
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WireInstance {
    pub pos_a: [f32; 3],
    pub pos_a_low: [f32; 3],
    pub pos_b: [f32; 3],
    pub pos_b_low: [f32; 3],
    pub distance_a: f32,
    pub distance_b: f32,
    /// Index into the per-wire `WireConst` storage buffer (group 1).
    pub wire_id: u32,
    /// World-space half-width at each endpoint for a TAPERED band. `0.0` =
    /// use the per-wire constant (`WireConst.world_half_width`). The shader
    /// interpolates between the two so the band tapers across the segment.
    pub world_hw_a: f32,
    pub world_hw_b: f32,
}

#[cfg(not(target_arch = "wasm32"))]
impl WireInstance {
    pub fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        // Must match `InstanceIn` in wire_indexed.wgsl.
        const ATTRS: &[wgpu::VertexAttribute] = &[
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pos_a) as u64,      shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pos_b) as u64,      shader_location: 1, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pos_a_low) as u64,  shader_location: 2, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pos_b_low) as u64,  shader_location: 3, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, distance_a) as u64, shader_location: 4, format: wgpu::VertexFormat::Float32   },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, distance_b) as u64, shader_location: 5, format: wgpu::VertexFormat::Float32   },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, wire_id) as u64,    shader_location: 6, format: wgpu::VertexFormat::Uint32    },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, world_hw_a) as u64, shader_location: 7, format: wgpu::VertexFormat::Float32   },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, world_hw_b) as u64, shader_location: 8, format: wgpu::VertexFormat::Float32   },
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WireInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: ATTRS,
        }
    }
}

/// Per-wire constants shared by every segment of a wire (native only). std430
/// layout: three vec4 then eight scalars = 80 B, matching `WireConst` in
/// wire_indexed.wgsl. `align_end` / `align_total` carry the "A"-type endpoint
/// alignment (see `wire_distances`); 0.0 total = no alignment.
#[cfg(not(target_arch = "wasm32"))]
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WireConst {
    pub color: [f32; 4],
    pub pat0: [f32; 4],
    pub pat1: [f32; 4],
    pub half_width: f32,
    pub pattern_length: f32,
    pub draw_depth: f32,
    pub align_end: f32,
    pub align_total: f32,
    /// World-space half-width for a wide-polyline band. `0.0` = a normal wire
    /// (uses `half_width`, screen pixels). Non-zero = the vertex shader expands
    /// the quad by `world_half_width / world_per_pixel` so the band tracks zoom
    /// in drawing units.
    pub world_half_width: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

#[cfg(not(target_arch = "wasm32"))]
impl WireConst {
    /// Bind-group layout for the per-wire storage buffer (group 1 of the wire /
    /// xray pipelines). Read-only storage, visible to the vertex stage.
    pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wire_const.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        })
    }
}

// ── Web (WebGL2): self-contained fat instance (no vertex-stage storage) ─────
#[cfg(target_arch = "wasm32")]
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WireInstance {
    pub pos_a: [f32; 3],
    pub pos_a_low: [f32; 3],
    pub pos_b: [f32; 3],
    pub pos_b_low: [f32; 3],
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
    /// "A"-type endpoint alignment (see `wire_distances`): the end-dash length
    /// and the total wire length. `align_total == 0.0` = not aligned.
    pub align_end: f32,
    pub align_total: f32,
    /// World-space half-width for a wide-polyline band (see `WireConst`). `0.0`
    /// = a normal wire (uses `half_width`, screen pixels).
    pub world_half_width: f32,
    /// Per-endpoint world half-width for a tapered band (0 = use the constant
    /// `world_half_width`). The shader interpolates across the segment.
    pub world_hw_a: f32,
    pub world_hw_b: f32,
}

#[cfg(target_arch = "wasm32")]
impl WireInstance {
    pub fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        // Offsets come from the struct layout (must match the shader location
        // indices in wire.wgsl). Scalars ride in PACKED vec4/vec2 attributes —
        // WebGL2 / WebGPU cap vertex attributes at 16 and the one-scalar-per-
        // location layout had grown to 17, so the pipeline failed to build and
        // the web viewport drew no lines at all (#414). The struct fields are
        // laid out so each packed group is contiguous.
        const ATTRS: &[wgpu::VertexAttribute] = &[
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pos_a) as u64,          shader_location: 0,  format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pos_b) as u64,          shader_location: 1,  format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, color) as u64,          shader_location: 2,  format: wgpu::VertexFormat::Unorm8x4  },
            // dists = (distance_a, distance_b, half_width, pattern_length)
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, distance_a) as u64,     shader_location: 3,  format: wgpu::VertexFormat::Float32x4 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pat0) as u64,           shader_location: 4,  format: wgpu::VertexFormat::Float32x4 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pat1) as u64,           shader_location: 5,  format: wgpu::VertexFormat::Float32x4 },
            // misc = (draw_depth, align_end, align_total, world_half_width)
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, draw_depth) as u64,     shader_location: 6,  format: wgpu::VertexFormat::Float32x4 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pos_a_low) as u64,      shader_location: 7,  format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, pos_b_low) as u64,      shader_location: 8,  format: wgpu::VertexFormat::Float32x3 },
            // taper = (world_hw_a, world_hw_b)
            wgpu::VertexAttribute { offset: std::mem::offset_of!(WireInstance, world_hw_a) as u64,     shader_location: 9,  format: wgpu::VertexFormat::Float32x2 },
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WireInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: ATTRS,
        }
    }
}

// ── GPU handle ────────────────────────────────────────────────────────────

pub struct WireGpu {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
    /// `true` when the source `WireModel` also carries `fill_tris`
    /// (i.e. it is a 3D mesh face — PolyfaceMesh / PolygonMesh — whose
    /// outline lives in `points`). The wire pass skips these instances
    /// in shaded modes so the surface reads as a clean solid; pure
    /// wireframe / HiddenLine / *WithEdges modes draw them.
    pub is_3d_mesh_edge: bool,
    /// Per-wire constants storage (group 1), shared across all chunks of one
    /// build. `None` on web (the fat instance carries the constants inline) and
    /// for empty buffers. The draw loop binds it as group 1 when present.
    pub const_bind_group: Option<std::sync::Arc<wgpu::BindGroup>>,
}

/// Expand one `WireModel` into its per-segment instance stream (1 instance per
/// finite segment). Pulled out so both the single-wire and batched paths share
/// the same emission logic, and so the batched path can `par_iter` across
/// wires on cold open.
#[cfg(target_arch = "wasm32")]
fn pack_color(color: [f32; 4]) -> [u8; 4] {
    [
        (color[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (color[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
    ]
}

/// Cumulative arc-length per point (NaN-break aware) plus the `"A"`-type
/// alignment pair `(align_end, align_total)`. Shared by the wasm and native
/// emission paths.
///
/// AutoCAD-style linetypes are implicitly `A` (aligned): a dashed line must
/// begin AND end on a solid dash, keeping the interior dashes at their nominal
/// length and stretching/shrinking only the two end dashes symmetrically to
/// absorb the leftover (so parallel lines share an identical interior). We
/// express that on the GPU by handing the shader the total wire length
/// (`align_total`) and the end-dash length (`align_end`); the pattern walk then
/// forces the two end regions lit and phases the interior to resume right after
/// the first dash. `align_total == 0.0` disables it and the shader falls back to
/// the legacy centred repeating pattern.
///
/// Alignment applies only to a single continuous run (`!has_break`) whose
/// pattern begins with a dash. NaN-separated (plinegen=false) polylines and
/// non-dash-first patterns keep the legacy centred phase.
fn wire_distances(wire: &WireModel) -> (Vec<f32>, f32, f32) {
    let n = wire.points.len();
    let mut dists = vec![0.0_f32; n];
    let mut has_break = false;
    // Accumulate arc-length in f64 from double-single deltas (high + low). An
    // f32-high-only delta `q[0] - p[0]` quantises ~0.1 at UTM coordinates
    // (−1.2M), which shifts the dash phase and drifts parallel lines out of sync
    // — the reason MLINE elements were CPU-dashed. `points_low` may be empty for
    // non-RTE wires → treat the low half as zero (same as the old behaviour).
    let mut acc = 0.0_f64;
    for i in 1..n {
        let p = wire.points[i - 1];
        let q = wire.points[i];
        if !p[0].is_finite() || !q[0].is_finite() {
            has_break = true;
            // plinegen=false: reset to 0 at the first real point after a NaN separator.
            if !wire.plinegen && !p[0].is_finite() && q[0].is_finite() {
                acc = 0.0;
            }
            dists[i] = acc as f32;
        } else {
            let pl = wire.points_low.get(i - 1).copied().unwrap_or([0.0; 3]);
            let ql = wire.points_low.get(i).copied().unwrap_or([0.0; 3]);
            let dx = (q[0] as f64 - p[0] as f64) + (ql[0] as f64 - pl[0] as f64);
            let dy = (q[1] as f64 - p[1] as f64) + (ql[1] as f64 - pl[1] as f64);
            let dz = (q[2] as f64 - p[2] as f64) + (ql[2] as f64 - pl[2] as f64);
            acc += (dx * dx + dy * dy + dz * dz).sqrt();
            dists[i] = acc as f32;
        }
    }

    let pat_len = wire.pattern_length;
    if pat_len <= 1e-6 || has_break || n < 2 {
        return (dists, 0.0, 0.0);
    }
    let total = dists[n - 1];
    if total <= 1e-6 {
        return (dists, 0.0, 0.0);
    }

    // DGN line styles draw the pattern from the START vertex with continuous
    // phase and are NOT end-aligned. The raw arc-length distances already put
    // dist 0 at the first vertex, so a dash-first pattern begins a dash exactly
    // there. Return before the A-type / centring logic that standard linetypes
    // use (see `WireModel::dash_from_start`).
    if wire.dash_from_start {
        return (dists, 0.0, 0.0);
    }

    // Shared "A"-type for MLINE elements: the caller fixes the begin/end
    // solid-dash length (`dash_align_end`, derived once from the multiline's
    // centre-line length) so every parallel element runs the SAME interior phase
    // — the shader's interior walk depends on `align_end`, not on the wire's own
    // length — while `align_total` stays this wire's own length, so each element
    // still ends on a dash at its own endpoint.
    if let Some(ae) = wire.dash_align_end {
        if total <= pat_len {
            // Shorter than one full period → solid (matches the per-wire path).
            return (dists, total, total);
        }
        return (dists, ae.clamp(1e-4, total * 0.5), total);
    }

    // Align only a proper alternating pattern that BEGINS with a dash followed
    // by a gap — every standard linetype does (DASHED/DASHDOT/CENTER/HIDDEN/…).
    // Gap-first, dot-first, single-element, or consecutive-dash patterns keep
    // the legacy centred phase: the A-type interior-resume assumes the element
    // after the leading dash is a gap, and force-lighting an end dash on a
    // non-dash-start would paint over a leading blank.
    if wire.pattern[0] > 0.0 && wire.pattern[1] < 0.0 {
        let a = wire.pattern[0];
        let p = pat_len;
        if total <= p {
            // Shorter than one full pattern period → drawn as a single solid
            // dash spanning the whole line (aligned linetypes can't fit a
            // dash-gap-dash in less than one period).
            return (dists, total, total);
        }
        // "A" alignment for a dash-first pattern of period P laid out as
        //   [D] [gap] ([dash] [gap])*(k-1) [D]
        // gives  L = 2D + (k-1)a + k(P-a)  =>  D = (L - k*P + a) / 2.
        // Pick the interior period count k so the end dash D stays near nominal a.
        let mut k = ((total - a) / p).round().max(1.0);
        let mut d_end = (total - k * p + a) * 0.5;
        if d_end <= 1e-4 {
            // End dash underflowed (period ≫ first dash); drop one period so the
            // ends stay visible.
            k = (k - 1.0).max(0.0);
            d_end = (total - k * p + a) * 0.5;
        }
        let d_end = d_end.clamp(1e-4, total * 0.5);
        return (dists, d_end, total);
    }

    // Legacy centred phase for non-aligned patterns (behaviour unchanged from
    // before A-type). The shader reads phase as `dist % pattern_length`, so a
    // constant offset shifts it; place the wire midpoint at the first dash's
    // centre so the two ends stay symmetric.
    let first_dash = wire
        .pattern
        .iter()
        .copied()
        .find(|&v| v > 0.0)
        .unwrap_or_else(|| wire.pattern[0].abs());
    let offset = first_dash * 0.5 + total * 0.5;
    for d in dists.iter_mut() {
        *d += offset;
    }
    (dists, 0.0, 0.0)
}

#[inline]
fn finite3(p: [f32; 3]) -> bool {
    p[0].is_finite() && p[1].is_finite() && p[2].is_finite()
}

/// Web: emit fat per-segment instances (each carries the wire's constants).
#[cfg(target_arch = "wasm32")]
fn emit_wire_instances(wire: &WireModel, color: [f32; 4], draw_depth: f32) -> Vec<WireInstance> {
    let color_u8 = pack_color(color);
    let pat0 = [wire.pattern[0], wire.pattern[1], wire.pattern[2], wire.pattern[3]];
    let pat1 = [wire.pattern[4], wire.pattern[5], wire.pattern[6], wire.pattern[7]];
    let half_width = wire.line_weight_px * 0.5;
    let n = wire.points.len();
    let seg_count = n.saturating_sub(1);
    if seg_count == 0 {
        return Vec::new();
    }
    let (dists, align_end, align_total) = wire_distances(wire);
    let low = |i: usize| -> [f32; 3] { wire.points_low.get(i).copied().unwrap_or([0.0; 3]) };
    let tw = |i: usize| -> f32 { wire.taper_widths.get(i).copied().unwrap_or(0.0) * 0.5 };
    let mut instances: Vec<WireInstance> = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let a = wire.points[i];
        let b = wire.points[i + 1];
        if !finite3(a) || !finite3(b) {
            continue;
        }
        instances.push(WireInstance {
            pos_a: a,
            pos_a_low: low(i),
            pos_b: b,
            pos_b_low: low(i + 1),
            color: color_u8,
            distance_a: dists[i],
            distance_b: dists[i + 1],
            half_width,
            pattern_length: wire.pattern_length,
            pat0,
            pat1,
            draw_depth,
            align_end,
            align_total,
            world_half_width: wire.world_width * 0.5,
            world_hw_a: tw(i),
            world_hw_b: tw(i + 1),
        });
    }
    instances
}

/// Native: emit slim per-segment instances (positions + distances + `wire_id`)
/// plus the one `WireConst` record every segment of this wire shares.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn emit_wire_native(
    wire: &WireModel,
    wire_id: u32,
    color: [f32; 4],
    draw_depth: f32,
) -> (Vec<WireInstance>, WireConst) {
    let (dists, align_end, align_total) = wire_distances(wire);
    let cst = WireConst {
        color,
        pat0: [wire.pattern[0], wire.pattern[1], wire.pattern[2], wire.pattern[3]],
        pat1: [wire.pattern[4], wire.pattern[5], wire.pattern[6], wire.pattern[7]],
        half_width: wire.line_weight_px * 0.5,
        pattern_length: wire.pattern_length,
        draw_depth,
        align_end,
        align_total,
        world_half_width: wire.world_width * 0.5,
        _pad1: 0.0,
        _pad2: 0.0,
    };
    let n = wire.points.len();
    let seg_count = n.saturating_sub(1);
    if seg_count == 0 {
        return (Vec::new(), cst);
    }
    let low = |i: usize| -> [f32; 3] { wire.points_low.get(i).copied().unwrap_or([0.0; 3]) };
    // Per-point half-width for a tapered band (empty ⇒ 0 ⇒ shader uses the
    // per-wire constant width).
    let tw = |i: usize| -> f32 { wire.taper_widths.get(i).copied().unwrap_or(0.0) * 0.5 };
    let mut instances: Vec<WireInstance> = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let a = wire.points[i];
        let b = wire.points[i + 1];
        if !finite3(a) || !finite3(b) {
            continue;
        }
        instances.push(WireInstance {
            pos_a: a,
            pos_a_low: low(i),
            pos_b: b,
            pos_b_low: low(i + 1),
            distance_a: dists[i],
            distance_b: dists[i + 1],
            wire_id,
            world_hw_a: tw(i),
            world_hw_b: tw(i + 1),
        });
    }
    (instances, cst)
}

/// Looks up a wire's draw-order depth from the per-entity map using the
/// handle encoded in its `name`. Falls back to 0.0 (transient / preview
/// wires that carry no document handle). A wire carrying a block-local
/// `depth_override` (a wide-polyline band inside a block) composes it into
/// the insert's own sub-range so the band orders against its block siblings.
pub(crate) fn wire_draw_depth(
    wire: &WireModel,
    depth_map: &rustc_hash::FxHashMap<u64, [f32; 2]>,
) -> f32 {
    let base = wire
        .name
        .parse::<u64>()
        .ok()
        .and_then(|h| depth_map.get(&h).copied());
    match (base, wire.depth_override) {
        (Some([d, half]), Some(local)) => d + local * half,
        (Some([d, _]), None) => d,
        (None, _) => 0.0,
    }
}

/// Build the shared per-wire `WireConst` storage buffer and its bind group
/// (native only). All instance-buffer chunks from one build reference the same
/// buffer via their global `wire_id`, so a single bind group is cloned into
/// each chunk.
#[cfg(not(target_arch = "wasm32"))]
fn build_const_bind_group(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    consts: &[WireConst],
) -> std::sync::Arc<wgpu::BindGroup> {
    use wgpu::util::DeviceExt;
    // wgpu rejects zero-sized buffers; pad with one zeroed record when empty.
    let one = [<WireConst as bytemuck::Zeroable>::zeroed()];
    let data: &[WireConst] = if consts.is_empty() { &one } else { consts };
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("wire_const.buf"),
        contents: bytemuck::cast_slice(data),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("wire_const.bg"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    });
    std::sync::Arc::new(bg)
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
        depth_map: &rustc_hash::FxHashMap<u64, [f32; 2]>,
        mesh_edge: bool,
        const_bgl: Option<&wgpu::BindGroupLayout>,
    ) -> Vec<Self> {
        const MAX_INSTANCES: usize = 268_435_456 / std::mem::size_of::<WireInstance>();
        #[cfg(not(target_arch = "wasm32"))]
        {
            use crate::par::prelude::*;
            // Global `wire_id` = wire index; one shared WireConst buffer for all
            // chunks. Indexed `collect` preserves wire order (the pass relies on
            // submission order for depth-biased / transparent overlap).
            let per: Vec<(Vec<WireInstance>, WireConst)> = wires
                .par_iter()
                .enumerate()
                .map(|(idx, w)| {
                    // 3D mesh outline edges are real geometry occluded by true
                    // depth — they must NOT take the draw-order z-bias (which
                    // pulls 2D wires toward the camera), or the hidden edges of a
                    // small / distant mesh peek through its own shaded fill.
                    let dd = if mesh_edge { 0.0 } else { wire_draw_depth(w, depth_map) };
                    emit_wire_native(w, idx as u32, w.color, dd)
                })
                .collect();
            let mut instances: Vec<WireInstance> =
                Vec::with_capacity(per.iter().map(|(v, _)| v.len()).sum());
            let mut consts: Vec<WireConst> = Vec::with_capacity(per.len());
            for (mut v, c) in per {
                instances.append(&mut v);
                consts.push(c);
            }
            if instances.is_empty() {
                return vec![];
            }
            let bg = const_bgl.map(|bgl| build_const_bind_group(device, bgl, &consts));
            instances
                .chunks(MAX_INSTANCES)
                .map(|chunk| {
                    let buf = instance_buffer_mapped(device, "wire.run.ibuf", chunk);
                    Self {
                        instance_buffer: buf,
                        instance_count: chunk.len() as u32,
                        is_3d_mesh_edge: mesh_edge,
                        const_bind_group: bg.clone(),
                    }
                })
                .collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = const_bgl;
            let per: Vec<Vec<WireInstance>> = wires
                .iter()
                .map(|w| {
                    let dd = if mesh_edge { 0.0 } else { wire_draw_depth(w, depth_map) };
                    emit_wire_instances(w, w.color, dd)
                })
                .collect();
            let mut instances: Vec<WireInstance> =
                Vec::with_capacity(per.iter().map(Vec::len).sum());
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
                        is_3d_mesh_edge: mesh_edge,
                        const_bind_group: None,
                    }
                })
                .collect()
        }
    }

    /// Merge multiple WireModels into GPU instance buffers, chunked to fit the
    /// 256 MB GPU limit. Each wire keeps its own color and pattern — they live
    /// per-instance.
    pub fn from_batch(
        device: &wgpu::Device,
        wires: &[WireModel],
        depth_map: &rustc_hash::FxHashMap<u64, [f32; 2]>,
        const_bgl: Option<&wgpu::BindGroupLayout>,
    ) -> Vec<Self> {
        let total_segs: usize = wires.iter().map(|w| w.points.len().saturating_sub(1)).sum();
        if total_segs == 0 {
            return vec![];
        }
        // GPU max buffer size is 256 MB; chunk to stay within the limit.
        const MAX_INSTANCES: usize = 268_435_456 / std::mem::size_of::<WireInstance>();

        #[cfg(not(target_arch = "wasm32"))]
        {
            use crate::par::prelude::*;
            // `block_cache` groups wires by style upstream; order within a batch
            // doesn't affect correctness, but indexed `collect` gives each wire a
            // stable `wire_id` into the shared WireConst buffer.
            let per: Vec<(Vec<WireInstance>, WireConst)> = wires
                .par_iter()
                .enumerate()
                .map(|(idx, w)| {
                    emit_wire_native(w, idx as u32, w.color, wire_draw_depth(w, depth_map))
                })
                .collect();
            let mut instances: Vec<WireInstance> =
                Vec::with_capacity(per.iter().map(|(v, _)| v.len()).sum());
            let mut consts: Vec<WireConst> = Vec::with_capacity(per.len());
            for (mut v, c) in per {
                instances.append(&mut v);
                consts.push(c);
            }
            if instances.is_empty() {
                return vec![];
            }
            let bg = const_bgl.map(|bgl| build_const_bind_group(device, bgl, &consts));
            instances
                .chunks(MAX_INSTANCES)
                .enumerate()
                .map(|(i, chunk)| {
                    let label = format!("wire.batch.ibuf.{i}");
                    let instance_buffer = instance_buffer_mapped(device, &label, chunk);
                    Self {
                        instance_buffer,
                        instance_count: chunk.len() as u32,
                        is_3d_mesh_edge: false,
                        const_bind_group: bg.clone(),
                    }
                })
                .collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = const_bgl;
            let instances: Vec<WireInstance> = wires
                .iter()
                .flat_map(|w| emit_wire_instances(w, w.color, wire_draw_depth(w, depth_map)))
                .collect();
            if instances.is_empty() {
                return vec![];
            }
            instances
                .chunks(MAX_INSTANCES)
                .enumerate()
                .map(|(i, chunk)| {
                    let label = format!("wire.batch.ibuf.{i}");
                    let instance_buffer = instance_buffer_mapped(device, &label, chunk);
                    Self {
                        instance_buffer,
                        instance_count: chunk.len() as u32,
                        is_3d_mesh_edge: false,
                        const_bind_group: None,
                    }
                })
                .collect()
        }
    }
}
