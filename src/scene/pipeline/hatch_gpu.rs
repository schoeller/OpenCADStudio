// The whole batched renderer is native-only: WebGL2 lacks storage buffers, so
// on wasm hatches go through `hatch_web_gpu` and nothing here is called.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

// Hatch rendering — the single, canonical GPU hatch renderer. One draw
// call for all hatches, per-instance data fetched from storage buffers in
// the vertex shader. There is NO per-family cap here: every pattern line
// family is uploaded (`family_count` per instance) and the fragment shader
// loops over all of them, so complex baked patterns (ROOFING, GRAVEL,
// CAROLINA-LEDGESTONE with dozens–hundreds of families) draw in full,
// matching the PDF export. `None` on WebGL2 (wasm), which lacks
// vertex-stage storage buffers. The old per-hatch renderer with the
// 16-family cap now lives in `wipeout_gpu.rs` and serves only wipeouts.
//
// Data layout — three storage buffers fed from `HatchModel`s:
//
//   InstanceBuffer  (binding 0)   :  HatchInstance[]      (128 B each)
//                                    color, color2, mode, gradient,
//                                    pattern angle/scale, world_origin,
//                                    boundary_offset, boundary_count,
//                                    family_offset, family_count,
//                                    dash_offset, dash_count, aabb,
//                                    visibility flag (CPU writes; GPU
//                                    skip)
//   BoundaryBuffer  (binding 1)   :  vec4<f32>[]          (all boundary
//                                    verts concatenated; NaN markers
//                                    preserved as separators just like
//                                    the per-hatch path)
//   FamilyBuffer    (binding 2)   :  LineFamilyGpu[]      (all line
//                                    families concatenated)
//   DashBuffer      (binding 3)   :  f32[]                (all dash
//                                    lengths concatenated)
//
// The vertex buffer holds each hatch's tessellated boundary triangles as
// per-vertex local-space positions (`local_xy`) plus a per-vertex
// `instance_index` (u32 attribute) — instance_index lets us avoid relying
// on `@builtin(instance_index)` for portability. An instance whose
// boundary failed to tessellate instead gets an AABB quad here, with
// `poly_test == 1` on its `HatchInstance` so the fragment shader still
// runs the `in_polygon` test to clip the quad to the real shape; on the
// tessellated fast path (`poly_test == 0`) the triangles already bound
// the fill and the fragment shader skips that test. When the visibility
// flag is 0, the vertex shader returns a degenerate position and the
// fragment shader runs zero times for that instance.
//
// Two storage usages — vertex shader reads InstanceBuffer + Boundary
// for the AABB / boundary range; fragment shader reads
// InstanceBuffer + Boundary + Family + Dash. Both stages share group
// 1 with `read_only` access.

use crate::scene::model::hatch_model::{HatchModel, HatchPattern, PatFamily};
use iced::wgpu;
use iced::wgpu::util::DeviceExt;

// ── GPU structs ────────────────────────────────────────────────────────────
//
// Layout matches the WGSL `HatchInstance` exactly. `repr(C)` + manual
// padding keeps WGSL's 16-byte alignment rules satisfied for arrays of
// this struct.

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HatchInstance {
    pub color: [f32; 4],            //   0
    pub color2: [f32; 4],           //  16  (gradient end)
    pub aabb: [f32; 4],             //  32  (local-space xmin,ymin,xmax,ymax)
    pub world_origin: [f32; 2],     //  48  (anchor high half, added back in VS)
    pub world_origin_low: [f32; 2], //  56  (anchor low residual — double-single)
    pub angle_offset: f32,          //  64
    pub scale: f32,                 //  68
    pub grad_cos: f32,              //  72
    pub grad_sin: f32,              //  76
    pub grad_min: f32,              //  80
    pub grad_range: f32,            //  84
    pub mode: u32,                  //  88  (0=pattern, 1=solid, 2=gradient)
    pub visible: u32,               //  92  (CPU sets to 0 to skip)
    pub boundary_offset: u32,       //  96  (first boundary vert index)
    pub boundary_count: u32,        // 100
    pub family_offset: u32,         // 104
    pub family_count: u32,          // 108
    /// Signed draw-order depth (-1,1); 0.0 = neutral. Applied as a clip-z
    /// bias in the vertex shader so this fill orders against other types.
    pub draw_depth: f32,            // 112
    /// 1 = run the per-fragment `in_polygon` boundary test (fallback path for
    /// an instance whose boundary failed to tessellate); 0 = the rasterized
    /// triangles already bound the fill, skip the test.
    pub poly_test: u32,             // 116
    /// Gradient shape (`GradientKind::shader_kind`), bit 4 = inverted stops.
    pub grad_kind: u32,             // 120
    pub _pad2: u32,                 // 124
}

const _: () = assert!(std::mem::size_of::<HatchInstance>() == 128);

/// Split a hatch's f64 world-origin anchor into double-single (high, low) f32
/// pairs so the GPU keeps sub-unit precision at UTM-scale coordinates.
#[inline]
fn split_origin_ds(o: [f64; 2]) -> ([f32; 2], [f32; 2]) {
    let hx = o[0] as f32;
    let hy = o[1] as f32;
    ([hx, hy], [(o[0] - hx as f64) as f32, (o[1] - hy as f64) as f32])
}

/// Mirrors the per-family struct used by the existing per-hatch shader,
/// but the dash slice lives in a separate concatenated DashBuffer (the
/// old shader had it embedded). `dash_offset` / `dash_count` index into
/// that flat array.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineFamilyGpu {
    pub cos_a: f32,        //  0
    pub sin_a: f32,        //  4
    pub x0: f32,           //  8
    pub y0: f32,           // 12
    pub dx: f32,           // 16
    pub dy: f32,           // 20
    pub perp_step: f32,    // 24
    pub along_step: f32,   // 28
    pub line_width: f32,   // 32
    pub period: f32,       // 36
    pub n_dashes: u32,     // 40
    pub dash_offset: u32,  // 44
}

const _: () = assert!(std::mem::size_of::<LineFamilyGpu>() == 48);

/// Per-vertex data — tessellated boundary triangles in local (world_origin-
/// relative) space, each carrying its instance index (avoids relying on
/// `@builtin(instance_index)` across backends). Instances whose boundary
/// failed to tessellate emit an AABB quad here instead (see `build`).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HatchVertex {
    pub local_xy: [f32; 2],  // 0  — local-space position
    pub instance_index: u32, // 8  — index into InstanceBuffer
}

impl HatchVertex {
    pub fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<HatchVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Uint32,
                },
            ],
        }
    }
}

// ── Batch builder ──────────────────────────────────────────────────────────

/// Pack a list of `HatchModel`s into the four concatenated storage
/// buffers + the per-vertex buffer needed by `hatch.wgsl`.
/// Returns `None` when the input slice is empty (caller skips the
/// hatch render pass entirely).
pub struct HatchGpu {
    pub vertex_buffer: wgpu::Buffer,
    pub vertex_count: u32,
    // The four storage buffers below are referenced via `bind_group` —
    // dropping them would invalidate it, but the bind group is the
    // only direct consumer. Keep them as fields to keep ownership in
    // one place; `#[allow(dead_code)]` silences the read-never warning.
    #[allow(dead_code)] pub instance_buffer: wgpu::Buffer,
    #[allow(dead_code)] pub boundary_buffer: wgpu::Buffer,
    #[allow(dead_code)] pub family_buffer:   wgpu::Buffer,
    #[allow(dead_code)] pub dash_buffer:     wgpu::Buffer,
    /// Per-instance visibility flag (1=draw, 0=skip). Stored in its
    /// own small storage buffer so per-frame updates don't have to
    /// touch the large `instance_buffer`. Vertex shader reads
    /// `visibility[instance_index]` — when 0 it emits an out-of-NDC
    /// clip position so the GPU clips the primitive before the
    /// fragment stage runs.
    pub visibility_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    #[allow(dead_code)] pub instance_count: u32,
    /// CPU mirror — `update_visibility` re-uploads this whole slice
    /// when any flag changes. ~4 B per hatch, so 40 KB / 10 k hatches
    /// per pan tick. Far cheaper than touching the 128 B-per-instance
    /// data.
    pub visibility: Vec<u32>,
    /// CPU-side mirror of each instance's local-space AABB (world-
    /// offset-subtracted, world_origin already added back). Used by
    /// `compute_hatch_lod` to evaluate the sub-pixel + frustum cull
    /// without reading back from the GPU.
    pub instance_aabbs: Vec<[f32; 4]>,
}

impl HatchGpu {
    /// One-time build from the full hatch list. Re-uploaded only when
    /// `geometry_epoch` advances (mirrors the existing per-hatch
    /// upload trigger). Per-frame visibility flips go through
    /// [`upload_visibility`].
    pub fn build(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        hatches: &[HatchModel],
    ) -> Option<Self> {
        if hatches.is_empty() {
            return None;
        }

        let mut instances: Vec<HatchInstance> = Vec::with_capacity(hatches.len());
        let mut boundary: Vec<[f32; 4]> = Vec::new();
        let mut families: Vec<LineFamilyGpu> = Vec::new();
        let mut dashes: Vec<f32> = Vec::new();

        for h in hatches {
            let boundary_offset = boundary.len() as u32;
            for &[x, y] in h.boundary.iter() {
                boundary.push([x, y, 0.0, 0.0]);
            }
            let boundary_count = boundary.len() as u32 - boundary_offset;


            let family_offset = families.len() as u32;
            let mut family_count = 0u32;

            let mut grad_kind = 0u32;
            let (mode, color2, grad_cos, grad_sin, grad_min, grad_range) = match &h.pattern {
                HatchPattern::Solid => (1u32, [0.0; 4], 0.0, 0.0, 0.0, 1.0),
                HatchPattern::Gradient { angle_deg, color2, kind, invert } => {
                    grad_kind = kind.shader_kind() | if *invert { 16 } else { 0 };
                    if kind.radial() {
                        // Radial fill: the boundary is stored relative to its
                        // centre (`world_origin`), so the centre is the local
                        // origin; radius = the farthest boundary vertex. mode 3.
                        let radius = radial_radius(&h.boundary);
                        (3u32, *color2, 0.0, 0.0, 0.0, radius)
                    } else {
                        let r = angle_deg.to_radians();
                        // Gradient projection range (proj_min / proj_range) —
                        // computed at upload time, identical to per-hatch path.
                        let (gmin, gmax) = boundary_projection_range(&h.boundary, r);
                        let grange = (gmax - gmin).max(1.0);
                        (2u32, *color2, r.cos(), r.sin(), gmin, grange)
                    }
                }
                HatchPattern::Pattern(fams) => {
                    for fam in fams {
                        let dash_offset = dashes.len() as u32;
                        for &d in &fam.dashes {
                            dashes.push(d);
                        }
                        let n_dashes = (dashes.len() as u32 - dash_offset).min(u32::MAX);
                        // QCAD PAT local-frame convention (mirrors
                        // `build_family_batch` in hatch_gpu.rs): `dy` is
                        // the perpendicular spacing, `dx` is the along-line
                        // phase shift — both in family-local coords. The
                        // shader applies cos_off/sin_off to rotate them.
                        let perp_step = fam.dy;
                        let along_step = fam.dx;
                        // Screen-space derivative drives 1-px line width
                        // in the shader; this stored field is unused.
                        let line_width = 0.0_f32;
                        let period: f32 = fam.dashes.iter().map(|d| d.abs()).sum();
                        families.push(LineFamilyGpu {
                            cos_a: fam.angle_deg.to_radians().cos(),
                            sin_a: fam.angle_deg.to_radians().sin(),
                            x0: fam.x0,
                            y0: fam.y0,
                            dx: fam.dx,
                            dy: fam.dy,
                            perp_step,
                            along_step,
                            line_width,
                            period: if n_dashes > 0 { period } else { 0.0 },
                            n_dashes,
                            dash_offset,
                        });
                        family_count += 1;
                    }
                    (0u32, [0.0; 4], 0.0, 0.0, 0.0, 1.0)
                }
            };

            // Boundary AABB in local space (matches the corner quad
            // emitted by the vertex shader). The verts are already in
            // `world_origin`-relative coords (see scene/mod.rs hatch
            // packing), so this AABB lives in that frame.
            let mut min_x = f32::INFINITY;
            let mut min_y = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            let mut max_y = f32::NEG_INFINITY;
            for &[x, y] in h.boundary.iter() {
                if x.is_finite() && y.is_finite() {
                    if x < min_x { min_x = x; }
                    if y < min_y { min_y = y; }
                    if x > max_x { max_x = x; }
                    if y > max_y { max_y = y; }
                }
            }
            if !min_x.is_finite() {
                // Empty / all-NaN — skip but keep the slot so indices
                // stay in lockstep with the input list (visibility=0).
                let (wo_hi, wo_lo) = split_origin_ds(h.world_origin);
                instances.push(HatchInstance {
                    color: h.color,
                    color2,
                    aabb: [0.0, 0.0, 0.0, 0.0],
                    world_origin: wo_hi,
                    world_origin_low: wo_lo,
                    angle_offset: h.angle_offset,
                    scale: h.scale.max(1e-6),
                    grad_cos,
                    grad_sin,
                    grad_min,
                    grad_range,
                    mode,
                    visible: 0,
                    boundary_offset,
                    boundary_count,
                    family_offset,
                    family_count,
                    draw_depth: h.draw_depth,
                    poly_test: 1,
                    grad_kind,
                    _pad2: 0,
                });
                continue;
            }

            // Pad the AABB so the quad covers any pattern halo + the
            // family origin. Mirrors the per-hatch shader's quad sizing
            // logic — `diag * 0.8 + max_spacing * 2 * scale`.
            let diag = ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt();
            // `perp_step.abs()` per family — uses the same QCAD local-
            // frame convention as `LineFamilyGpu.perp_step` above so
            // the quad padding matches what the shader will sample.
            let max_spacing = match &h.pattern {
                HatchPattern::Pattern(fs) => fs
                    .iter()
                    .map(|f| f.dy.abs())
                    .fold(0.0f32, f32::max),
                _ => 5.0,
            };
            let pad = (diag * 0.8 + max_spacing * 2.0 * h.scale).max(1.0);

            let (wo_hi, wo_lo) = split_origin_ds(h.world_origin);
            instances.push(HatchInstance {
                color: h.color,
                color2,
                aabb: [min_x - pad, min_y - pad, max_x + pad, max_y + pad],
                world_origin: wo_hi,
                world_origin_low: wo_lo,
                angle_offset: h.angle_offset,
                scale: h.scale.max(1e-6),
                grad_cos,
                grad_sin,
                grad_min,
                grad_range,
                mode,
                visible: 1,
                boundary_offset,
                boundary_count,
                family_offset,
                family_count,
                draw_depth: h.draw_depth,
                // Always resolve inside/outside per fragment on the GPU (the
                // boundary is uploaded below). Dropping CPU pre-triangulation
                // keeps the vertex buffer at 6 verts/hatch regardless of
                // boundary complexity, so a hatch-dense drawing can't overflow
                // the device buffer limit. Each instance draws its AABB quad.
                poly_test: 1,
                grad_kind,
                _pad2: 0,
            });
        }
        // Empty fallbacks — storage buffers can't be zero-sized.
        if boundary.is_empty() {
            boundary.push([0.0; 4]);
        }
        if families.is_empty() {
            families.push(LineFamilyGpu::default_filler());
        }
        if dashes.is_empty() {
            dashes.push(0.0);
        }

        // Vertex buffer — one AABB quad (two triangles, BL,BR,TL, BR,TR,TL) per
        // instance; the GPU shader clips each fragment to the boundary via the
        // in_polygon test (poly_test == 1). 6 verts/hatch, independent of
        // boundary complexity, so the buffer can never overflow the device
        // limit no matter how dense the drawing.
        let mut verts: Vec<HatchVertex> = Vec::with_capacity(instances.len() * 6);
        for (i, inst) in instances.iter().enumerate() {
            let [xmin, ymin, xmax, ymax] = inst.aabb;
            let quad = [
                [xmin, ymin], [xmax, ymin], [xmin, ymax],
                [xmax, ymin], [xmax, ymax], [xmin, ymax],
            ];
            for c in quad {
                verts.push(HatchVertex { local_xy: c, instance_index: i as u32 });
            }
        }

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hatch.vertex"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hatch.instances"),
            contents: bytemuck::cast_slice(&instances),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let boundary_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hatch.boundary"),
            contents: bytemuck::cast_slice(&boundary),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let family_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hatch.families"),
            contents: bytemuck::cast_slice(&families),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let dash_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hatch.dashes"),
            contents: bytemuck::cast_slice(&dashes),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let visibility: Vec<u32> = instances.iter().map(|i| i.visible).collect();
        let visibility_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hatch.visibility"),
            contents: bytemuck::cast_slice(&visibility),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hatch.bg"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: boundary_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: family_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: dash_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: visibility_buffer.as_entire_binding(),
                },
            ],
        });

        // CPU AABB mirror — local-space rect with world_origin added
        // back, ready for `aabb_offscreen` / `aabb_below_pixel` to
        // compare against the camera view_proj.
        let instance_aabbs: Vec<[f32; 4]> = instances
            .iter()
            .map(|i| {
                let ox = i.world_origin[0];
                let oy = i.world_origin[1];
                [
                    i.aabb[0] + ox,
                    i.aabb[1] + oy,
                    i.aabb[2] + ox,
                    i.aabb[3] + oy,
                ]
            })
            .collect();

        Some(Self {
            vertex_buffer,
            vertex_count: verts.len() as u32,
            instance_buffer,
            boundary_buffer,
            family_buffer,
            dash_buffer,
            visibility_buffer,
            bind_group,
            instance_count: instances.len() as u32,
            visibility,
            instance_aabbs,
        })
    }

    /// Push the CPU `visibility` slice to GPU. Call when any
    /// element changes (typically per-frame from compute_hatch_lod).
    pub fn upload_visibility(&self, queue: &wgpu::Queue) {
        queue.write_buffer(
            &self.visibility_buffer,
            0,
            bytemuck::cast_slice(&self.visibility),
        );
    }

    /// Group-1 bind group layout — shared by the pipeline so it can be
    /// constructed once at startup. All four bindings are read-only
    /// storage and visible to both VS (AABB+visibility lookup) and FS
    /// (boundary / family / dash sampling).
    #[allow(dead_code)]
    pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        let entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hatch.bgl"),
            entries: &[entry(0), entry(1), entry(2), entry(3), entry(4)],
        })
    }
}

impl LineFamilyGpu {
    fn default_filler() -> Self {
        Self {
            cos_a: 1.0,
            sin_a: 0.0,
            x0: 0.0,
            y0: 0.0,
            dx: 1.0,
            dy: 0.0,
            perp_step: 1.0,
            along_step: 1.0,
            line_width: 0.0,
            period: 0.0,
            n_dashes: 0,
            dash_offset: 0,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Project every boundary vertex onto the gradient direction
/// `(cos θ, sin θ)` and return the (min, max) projection. Used to set
/// up the gradient's normalized parameter range. Same math as the
/// per-hatch path; duplicated here to keep this module self-contained.
fn boundary_projection_range(boundary: &[[f32; 2]], theta: f32) -> (f32, f32) {
    let (cs, sn) = (theta.cos(), theta.sin());
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &[x, y] in boundary {
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        let p = x * cs + y * sn;
        if p < lo { lo = p; }
        if p > hi { hi = p; }
    }
    if !lo.is_finite() {
        return (0.0, 1.0);
    }
    (lo, hi)
}

/// Radius of a radial gradient — the distance from the boundary centre (the
/// local origin, since vertices are stored relative to it) to the farthest
/// vertex, so `t = 1` (the end colour) reaches the corners.
fn radial_radius(boundary: &[[f32; 2]]) -> f32 {
    boundary
        .iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .map(|p| (p[0] * p[0] + p[1] * p[1]).sqrt())
        .fold(0.0_f32, f32::max)
        .max(1.0)
}

// PatFamily is re-exported by hatch_model so we don't need to import
// it explicitly anywhere else — but rust needs the type referenced to
// confirm the layout assumption above.
#[allow(dead_code)]
fn _assert_patfamily_fields(f: &PatFamily) -> (f32, f32, f32) {
    (f.angle_deg, f.x0, f.y0)
}
