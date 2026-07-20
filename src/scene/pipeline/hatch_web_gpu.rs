// WebGL2 hatch renderer — texture-backed, UNCAPPED (wasm only).
//
// WebGL2 has no storage buffers, so the batched storage-buffer renderer
// (hatch_gpu.rs + hatch.wgsl) is disabled on wasm and real hatch fills never
// rendered on the web build (issue #204). This per-hatch renderer reuses the
// WebGL2-safe hatch algorithm (see wipeout.wgsl / hatch_web.wgsl) but packs the
// variable-length boundary / family / dash arrays into ONE RGBA32F data texture
// read via textureLoad — removing the MAX_FAMILIES / MAX_HATCH_BOUNDARY_VERTS /
// MAX_DASHES caps of the uniform (WipeoutGpu) path. Every hatch type — solid,
// gradient, and arbitrarily complex line patterns — renders on the web.
//
// Native (hatch_gpu.rs) and the wipeout mask renderer (wipeout_gpu.rs) are
// untouched; this module is compiled only for wasm32.

use crate::scene::model::hatch_model::{HatchModel, HatchPattern};
use iced::wgpu;
use iced::wgpu::util::DeviceExt;

/// Width (in texels) of the RGBA32F data texture. Height grows to fit; a hatch
/// with N total texels uses ceil(N / WIDTH) rows.
const DATA_TEX_WIDTH: u32 = 1024;

// ── Vertex ────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HatchVertex {
    pos: [f32; 3],
    _pad: f32,
}

// ── Per-hatch uniform (binding 0) — 96 bytes, matches HatchUniforms in
//    hatch_web.wgsl. ─────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HatchWebUniform {
    color: [f32; 4],      //  0
    color2: [f32; 4],     // 16
    mode: u32,            // 32
    vcount: u32,          // 36
    angle_offset: f32,    // 40
    scale: f32,           // 44
    grad_cos: f32,        // 48
    grad_sin: f32,        // 52
    grad_min: f32,        // 56
    grad_range: f32,      // 60
    origin: [f32; 2],     // 64
    origin_low: [f32; 2], // 72
    n_families: u32,      // 80
    fam_off: u32,         // 84: texel offset of the family section
    dash_off: u32,        // 88: texel offset of the dash section
    tex_width: u32,       // 92
}

// ── Per-hatch GPU handle ────────────────────────────────────────────────────

pub struct HatchWebGpu {
    pub vertex_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    /// Reserved for per-frame AABB LOD (mirrors `WipeoutGpu`); not yet wired
    /// into the web hatch draw loop — the native batched path doesn't do
    /// per-hatch LOD either.
    #[allow(dead_code)]
    pub world_aabb: [f32; 4],
    _uniform_buf: wgpu::Buffer,
    _data_tex: wgpu::Texture,
}

impl HatchWebGpu {
    /// Group-1 layout: uniform header (binding 0) + non-filterable float data
    /// texture (binding 1).
    pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hatch_web.bgl1"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        })
    }

    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &HatchModel,
        bgl1: &wgpu::BindGroupLayout,
    ) -> Self {
        // ── Decode pattern mode (mirrors WipeoutGpu::new) ─────────────────
        let (mode, color2, grad_cos, grad_sin) = match &model.pattern {
            HatchPattern::Solid => (1u32, [0.0f32; 4], 0.0f32, 0.0f32),
            HatchPattern::Pattern(_) => (0u32, [0.0f32; 4], 0.0f32, 0.0f32),
            HatchPattern::Gradient { angle_deg, color2, radial } => {
                if *radial {
                    // Radial: centre is the local origin; grad_cos/sin unused.
                    (3u32, *color2, 0.0, 0.0)
                } else {
                    let r = angle_deg.to_radians();
                    (2u32, *color2, r.cos(), r.sin())
                }
            }
        };

        // ── Bounding box ─────────────────────────────────────────────────
        let (mut min_x, mut max_x, mut min_y, mut max_y) =
            (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
        for &[x, y] in model.boundary.iter() {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }

        let max_spacing = match &model.pattern {
            HatchPattern::Pattern(families) => {
                families.iter().map(|f| f.dy.abs()).fold(0.0f32, f32::max)
            }
            _ => 5.0,
        };
        let diag = ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt();
        let pad = (diag * 0.8 + max_spacing * 2.0 * model.scale).max(1.0);

        // Anchor pattern phase at `world_origin` with the boundary stored raw,
        // matching the desktop batched renderer (hatch_gpu.rs) — NOT WipeoutGpu,
        // whose f64 origin grid-snap is dead code (wipeouts are always solid)
        // and would phase-shift every line pattern relative to desktop. No drift.
        let origin = model.world_origin;
        let drift = [0.0f32, 0.0f32];
        let (x0, x1, y0, y1) = (
            min_x + drift[0] - pad,
            max_x + drift[0] + pad,
            min_y + drift[1] - pad,
            max_y + drift[1] + pad,
        );

        let quad = [
            HatchVertex { pos: [x0, y0, 0.0], _pad: 0.0 },
            HatchVertex { pos: [x1, y0, 0.0], _pad: 0.0 },
            HatchVertex { pos: [x1, y1, 0.0], _pad: 0.0 },
            HatchVertex { pos: [x0, y0, 0.0], _pad: 0.0 },
            HatchVertex { pos: [x1, y1, 0.0], _pad: 0.0 },
            HatchVertex { pos: [x0, y1, 0.0], _pad: 0.0 },
        ];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hatch_web.vbuf"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // ── Gradient projection range (snapped-local space) ───────────────
        let (grad_min, grad_range) = if mode == 2 {
            let projs: Vec<f32> = model
                .boundary
                .iter()
                .filter(|v| v[0].is_finite() && v[1].is_finite())
                .map(|&[x, y]| (x + drift[0]) * grad_cos + (y + drift[1]) * grad_sin)
                .collect();
            if projs.is_empty() {
                (0.0, 1.0)
            } else {
                let proj_min = projs.iter().cloned().fold(f32::INFINITY, f32::min);
                let proj_max = projs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                // Floor matches the desktop hatch renderer (hatch_gpu.rs).
                (proj_min, (proj_max - proj_min).max(1.0))
            }
        } else if mode == 3 {
            // Radial: range = the farthest boundary vertex from the centre.
            let radius = model
                .boundary
                .iter()
                .filter(|v| v[0].is_finite() && v[1].is_finite())
                .map(|&[x, y]| (x * x + y * y).sqrt())
                .fold(0.0_f32, f32::max)
                .max(1.0);
            (0.0, radius)
        } else {
            (0.0, 1.0)
        };

        // ── Pack the data texture: boundary | families | dashes ───────────
        let mut texels: Vec<[f32; 4]> = Vec::new();
        // Boundary section (texels 0..vcount). NaN separators preserved (they
        // survive the RGBA32F fetch byte-for-byte).
        for &[x, y] in model.boundary.iter() {
            if x.is_finite() && y.is_finite() {
                texels.push([x + drift[0], y + drift[1], 0.0, 0.0]);
            } else {
                texels.push([f32::NAN, f32::NAN, 0.0, 0.0]);
            }
        }
        let vcount = texels.len() as u32;

        // Family section (3 texels each) + a flat dash pool.
        let fam_off = texels.len() as u32;
        let mut n_families = 0u32;
        let mut dash_pool: Vec<f32> = Vec::new();
        if let HatchPattern::Pattern(families) = &model.pattern {
            for fam in families.iter() {
                let dash_rel = dash_pool.len() as u32;
                dash_pool.extend_from_slice(&fam.dashes);
                let n_dashes = fam.dashes.len() as u32;
                let period: f32 = if n_dashes > 0 {
                    fam.dashes.iter().map(|d| d.abs()).sum()
                } else {
                    0.0
                };
                let angle_r = fam.angle_deg.to_radians();
                // QCAD PAT convention: perp_step = dy, along_step = dx.
                texels.push([angle_r.cos(), angle_r.sin(), fam.x0, fam.y0]);
                texels.push([fam.dx, fam.dy, fam.dy, fam.dx]);
                // Counts as exact f32 (small integers → no denormal/bitcast risk).
                texels.push([0.0, period, n_dashes as f32, dash_rel as f32]);
                n_families += 1;
            }
        }

        // Dash section (4 values per texel).
        let dash_off = texels.len() as u32;
        let mut i = 0usize;
        while i < dash_pool.len() {
            let mut t = [0.0f32; 4];
            for (c, slot) in t.iter_mut().enumerate() {
                if let Some(&d) = dash_pool.get(i + c) {
                    *slot = d;
                }
            }
            texels.push(t);
            i += 4;
        }

        // Upload the texture (min 1 texel; pad the last row).
        if texels.is_empty() {
            texels.push([0.0; 4]);
        }
        let width = DATA_TEX_WIDTH;
        let height = ((texels.len() as u32).div_ceil(width)).max(1);
        texels.resize((width * height) as usize, [0.0; 4]);
        let data_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hatch_web.data_tex"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            data_tex.as_image_copy(),
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 16),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let tex_view = data_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // ── Uniform header ────────────────────────────────────────────────
        let uniform_data = HatchWebUniform {
            color: model.color,
            color2,
            mode,
            vcount,
            angle_offset: model.angle_offset,
            // Clamp like the desktop renderer so scale==0 can't make perp_step 0
            // → round(perp/0)=NaN → an invisible hatch.
            scale: model.scale.max(1e-6),
            grad_cos,
            grad_sin,
            grad_min,
            grad_range,
            origin: [origin[0] as f32, origin[1] as f32],
            origin_low: [
                (origin[0] - origin[0] as f32 as f64) as f32,
                (origin[1] - origin[1] as f32 as f64) as f32,
            ],
            n_families,
            fam_off,
            dash_off,
            tex_width: width,
        };
        let _uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hatch_web.uniform"),
            contents: bytemuck::bytes_of(&uniform_data),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hatch_web.bind_group1"),
            layout: bgl1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: _uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&tex_view),
                },
            ],
        });

        let ox = model.world_origin[0] as f32;
        let oy = model.world_origin[1] as f32;
        let world_aabb = if min_x.is_finite() && min_y.is_finite() {
            [min_x + ox, min_y + oy, max_x + ox, max_y + oy]
        } else {
            [min_x, min_y, max_x, max_y]
        };

        Self {
            vertex_buffer,
            bind_group,
            world_aabb,
            _uniform_buf,
            _data_tex: data_tex,
        }
    }
}
