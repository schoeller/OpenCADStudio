// Image GPU buffers — renders raster images as textured quads.
//
// Group 1 bindings per image:
//   binding 0 — texture_2d<f32>   (RGBA image texture)
//   binding 1 — sampler           (bilinear filtering)
//   binding 2 — ImageParams       (opacity uniform, 16 bytes)

use crate::scene::model::image_model::ImageModel;
use iced::wgpu;
use iced::wgpu::util::DeviceExt;

// ── Vertex ────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ImageVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

impl ImageVertex {
    pub fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ImageVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

// ── Uniform ───────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ImageParams {
    opacity: f32,
    /// Signed draw-order depth (-1,1); applied as a clip-z bias in the shader
    /// so the raster orders against other entity types. 0.0 = neutral.
    draw_depth: f32,
    _pad: [f32; 2],
} // 16 bytes

// ── Per-image GPU handle ──────────────────────────────────────────────────

pub struct ImageGpu {
    pub vertex_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    /// Mirrors `ImageModel.vp_scissor` — see `HatchGpu.vp_scissor`.
    pub vp_scissor: Option<[f32; 4]>,
    _texture: wgpu::Texture,
    _sampler: wgpu::Sampler,
    _params_buf: wgpu::Buffer,
}

impl ImageGpu {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &ImageModel,
        bgl1: &wgpu::BindGroupLayout,
    ) -> Option<Self> {
        if model.pixels.is_empty() || model.width == 0 || model.height == 0 {
            return None;
        }

        // ── Upload texture ────────────────────────────────────────────────
        let tex_label = format!("image.texture:{}", model.file_path);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&tex_label),
            size: wgpu::Extent3d {
                width: model.width,
                height: model.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &model.pixels[..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * model.width),
                rows_per_image: Some(model.height),
            },
            wgpu::Extent3d {
                width: model.width,
                height: model.height,
                depth_or_array_layers: 1,
            },
        );
        let tex_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // ── Sampler ───────────────────────────────────────────────────────
        let _sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // ── Opacity uniform ───────────────────────────────────────────────
        let params = ImageParams {
            opacity: model.opacity.clamp(0.0, 1.0),
            draw_depth: model.draw_depth,
            _pad: [0.0; 2],
        };
        let _params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("image.params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        // ── Bind group ────────────────────────────────────────────────────
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image.bind_group1"),
            layout: bgl1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&tex_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: _params_buf.as_entire_binding(),
                },
            ],
        });

        // ── Vertex buffer — two triangles ─────────────────────────────────
        // corners: [p0=BL, p1=BR, p2=TR, p3=TL]
        // UV: BL=(0,1), BR=(1,1), TR=(1,0), TL=(0,0)
        let [p0, p1, p2, p3] = model.corners;
        let verts = [
            ImageVertex {
                pos: p0,
                uv: [0.0, 1.0],
            },
            ImageVertex {
                pos: p1,
                uv: [1.0, 1.0],
            },
            ImageVertex {
                pos: p2,
                uv: [1.0, 0.0],
            },
            ImageVertex {
                pos: p0,
                uv: [0.0, 1.0],
            },
            ImageVertex {
                pos: p2,
                uv: [1.0, 0.0],
            },
            ImageVertex {
                pos: p3,
                uv: [0.0, 0.0],
            },
        ];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("image.vbuf"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Some(Self {
            vertex_buffer,
            bind_group,
            vp_scissor: model.vp_scissor,
            _texture: texture,
            _sampler,
            _params_buf,
        })
    }
}
