pub mod face3d_gpu;
pub mod hatch_gpu;
/// WebGL2 texture-backed hatch renderer (wasm only) — the storage-buffer
/// `hatch_gpu` path is unavailable on WebGL2, so hatches render through this.
#[cfg(target_arch = "wasm32")]
pub mod hatch_web_gpu;
pub mod wipeout_gpu;
pub mod image_gpu;
pub mod mesh_gpu;
pub mod text_gpu;
pub mod uniforms;
pub mod viewcube;
/// Persistent per-entity wire instance arena (native), enabled by
/// `OCS_WIRE_GPU_PATCH` — patches one entity's slab via `write_buffer` instead
/// of re-uploading the whole wire set on an edit.
#[cfg(not(target_arch = "wasm32"))]
pub mod wire_arena;
pub mod wire_gpu;

use iced::wgpu;
use iced::wgpu::util::DeviceExt;
use iced::{Rectangle, Size};

pub use face3d_gpu::Face3DGpu;
pub use wipeout_gpu::WipeoutGpu;
pub use image_gpu::ImageGpu;
pub use mesh_gpu::MeshLodGpu;
pub use uniforms::Uniforms;
pub use viewcube::ViewCubePipeline;
pub use wire_gpu::WireGpu;

use crate::scene::model::hatch_model::HatchModel;
use crate::scene::model::image_model::ImageModel;
use crate::scene::model::mesh_model::MeshLodSet;
use crate::scene::model::wire_model::WireModel;

/// MSAA sample count for the main drawing pipelines.
const MSAA_SAMPLES: u32 = 4;

pub struct Pipeline {
    wire_pipeline: wgpu::RenderPipeline,
    /// Stamps clip-boundary polygons into the stencil buffer (viewports + XCLIP).
    clip_mask_pipeline: wgpu::RenderPipeline,
    /// Black-fragment variant of `wire_pipeline` for 3D mesh outline edges in
    /// filled render modes.
    wire_black_pipeline: wgpu::RenderPipeline,
    /// Same shader as wire_pipeline but depth_compare=Greater, depth_write_enabled=false.
    /// Used to draw ghost copies of selected wires through occluding geometry.
    wire_xray_pipeline: wgpu::RenderPipeline,
    /// Layout for the per-wire `WireConst` storage buffer (group 1 of the wire /
    /// xray pipelines). `Some` on native; `None` on web (fat instance, no
    /// storage). Passed to `WireGpu::from_run` / `from_batch` so they can build
    /// each batch's per-wire bind group.
    pub(crate) wire_const_bgl: Option<wgpu::BindGroupLayout>,
    wipeout_pipeline: wgpu::RenderPipeline,
    /// Phase 4-B — single-draw batched hatch pipeline. Per-instance
    /// data lives in storage buffers; one draw call covers every
    /// hatch in the frame. `None` on WebGL2 (wasm), which lacks
    /// vertex-stage storage buffers.
    hatch_pipeline: Option<wgpu::RenderPipeline>,
    /// WebGL2 (wasm) texture-backed hatch pipeline + its group-1 layout. Used
    /// where `hatch_pipeline` is `None` because WebGL2 lacks storage buffers.
    #[cfg(target_arch = "wasm32")]
    hatch_web_bgl1: wgpu::BindGroupLayout,
    #[cfg(target_arch = "wasm32")]
    hatch_web_pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    /// SDF text-quad pipeline (Phase 2b): draws per-glyph quads sampling the
    /// shared glyph atlas. Fed only when `OCS_TEXT_SDF` is set (else no verts).
    text_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    /// Depth-write-disabled variant of `mesh_pipeline` for non-opaque solids.
    mesh_transparent_pipeline: wgpu::RenderPipeline,
    mesh_highlight_pipeline: wgpu::RenderPipeline,
    /// Wireframe variant of the mesh pipeline (LineList topology, same
    /// vertex layout / shader). Used when the active render mode is
    /// Wireframe 2D or Wireframe 3D so 3D solids draw as their
    /// triangle edges instead of filled faces.
    mesh_wireframe_pipeline: wgpu::RenderPipeline,
    /// Edge pipeline that forces black, for the edge overlay in filled modes.
    mesh_edge_black_pipeline: wgpu::RenderPipeline,
    /// Depth-only variant of the mesh pipeline (TriangleList, no color
    /// writes, writes depth). Used in HiddenLine mode so 3D solids
    /// occlude wires behind them without painting visible pixels.
    mesh_depth_pipeline: wgpu::RenderPipeline,
    face3d_pipeline: wgpu::RenderPipeline,
    /// Depth-only variant of the face3d pipeline (no color writes,
    /// writes depth). Paired with `mesh_depth_pipeline` for HiddenLine.
    face3d_depth_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    wipeout_bgl1: wgpu::BindGroupLayout,
    /// Group-1 layout for the batched hatch pipeline (storage buffers
    /// for instances / boundary / families / dashes). `None` on WebGL2, where
    /// hatches render through `hatch_web_gpu` and this is never read.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    hatch_bgl1: Option<wgpu::BindGroupLayout>,
    image_bgl1: wgpu::BindGroupLayout,
    /// Group-1 layout for the text pipeline (atlas texture + sampler).
    text_atlas_bgl: wgpu::BindGroupLayout,
    /// GPU glyph atlas (texture + sampler + bind group). Rebuilt when the shared
    /// CPU atlas grows (new glyphs baked). `None` until the first text upload.
    text_atlas_gpu: Option<text_gpu::TextAtlasGpu>,
    /// All glyph-quad vertices for the frame, one buffer, `None` when empty.
    text_vbuf: Option<wgpu::Buffer>,
    text_vcount: u32,
    /// Tinted glyph quads of just the selected / hovered text, drawn over the
    /// base text pass so a selection / rollover recolours the glyphs (the text
    /// analogue of the selected-wire xray overlay). Rebuilt on selection change.
    text_highlight_vbuf: Option<wgpu::Buffer>,
    text_highlight_vcount: u32,
    /// Live grip-drag / command-preview SDF glyph quads. The base text buffer
    /// only re-uploads on a geometry-epoch change, but a grip drag hides the
    /// dragged entity from the base wire set and shows it as a per-frame
    /// overlay — so its glyphs ride here, re-uploaded every frame like
    /// `gpu_preview_wires`, or the dragged text vanishes until release re-tesses
    /// it back into the base set (issue #316).
    text_preview_vbuf: Option<wgpu::Buffer>,
    text_preview_vcount: u32,
    /// Per-frame DISPSILH silhouette line list — rebuilt every prepare() from
    /// the mesh sets' curved-face generators and the current view direction, so
    /// the outline tracks the camera. Reuses the mesh vertex format / pipeline.
    silhouette_vbuf: Option<wgpu::Buffer>,
    silhouette_vcount: u32,
    /// Last requested render size (the full viewport rect, in pixels). The
    /// geometry passes render at this size; the blit UV is scaled by
    /// `depth_texture_size / alloc_size` so it samples only the filled region.
    depth_texture_size: Size<u32>,
    /// Actual allocated size of the depth / MSAA / resolve textures. Rounded
    /// up from the requested size to a coarse grid so a divider drag (which
    /// changes the pane size a few pixels every frame) doesn't recreate these
    /// textures every frame. Recreation is what hangs the ANGLE → D3D11 path
    /// on Windows-Firefox (issue #191); grow-only bucketing makes it rare.
    alloc_size: Size<u32>,
    depth_view: wgpu::TextureView,
    /// 4× MSAA color buffer for the main drawing passes.
    msaa_view: wgpu::TextureView,
    /// Single-sample texture that receives the MSAA resolve result.
    resolve_view: wgpu::TextureView,
    /// Pipeline + resources for blitting the resolve texture to the surface target.
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
    blit_bind_group: wgpu::BindGroup,
    /// UV transform (offset + scale) consumed by the blit shader so a
    /// partially off-canvas viewport still composites the right portion of
    /// its resolve texture to the visible portion of the surface.
    blit_uniform_buffer: wgpu::Buffer,
    /// Cached texture format (needed to recreate MSAA / depth textures on resize).
    surface_format: wgpu::TextureFormat,
    /// The resident wire batches, shared by `std::sync::Arc` with every other
    /// pipeline slot rendering the *same* `wire_content_id` (see
    /// `MultiPipeline::wire_buffer_cache`). A `wgpu::Buffer` is internally
    /// ref-counted, so N paper viewports (or Model tiles) drawing one identical
    /// resident set hold one copy of the GPU vertex buffers between them —
    /// their camera uniforms + scissor stay per-slot, only the geometry is
    /// deduplicated. Never mutated in place, only reassigned on content change.
    pub(crate) gpu_wires: std::sync::Arc<Vec<WireGpu>>,
    /// Persistent per-entity wire instance arena (native, `OCS_WIRE_GPU_PATCH`).
    /// When active, `gpu_wires` is a thin wrapper over this arena's buffers and an
    /// edit patches one entity's slab in place instead of rebuilding every wire.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) wire_arena: Option<wire_arena::WireArena>,
    /// Second arena for the mesh/solid EDGE wires (drawn with the mesh-edge skip /
    /// black treatment); the resident set is split into this + `wire_arena` so
    /// both patch incrementally. Shares `wire_arena_id`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) wire_arena_mesh: Option<wire_arena::WireArena>,
    /// The Model content id both arenas currently mirror (`u64::MAX` = none).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) wire_arena_id: u64,
    /// This content viewport's non-rectangular clip boundary as a triangle-fan
    /// vertex buffer in the render target's normalized device coords (`None` =
    /// rectangular / unclipped, where the viewport's own render rectangle does
    /// the clipping). Stamped into the stencil once per frame; every content
    /// pass then draws with stencil reference 1 so only the interior survives.
    clip_boundary: Option<(wgpu::Buffer, u32)>,
    /// Ghost copies (25% alpha) of selected wires for the X-ray depth pass.
    gpu_selected_wires: Vec<WireGpu>,
    /// Command-preview / interim / grip-drag overlay wires. Re-uploaded every
    /// frame they are present (small), drawn on top of the base wire pass — so
    /// a live drag never re-uploads the resident base buffer.
    gpu_preview_wires: Vec<WireGpu>,
    /// The canonical hatch fills — a single batched GPU resource drawn in one
    /// call with per-instance visibility masking the rest. All pattern line
    /// families are uploaded (no cap). `None` on WebGL2.
    gpu_hatch: Option<hatch_gpu::HatchGpu>,
    /// WebGL2 hatch fills — one texture-backed `HatchWebGpu` per hatch, drawn in
    /// the hatch pass where the batched `gpu_hatch` is `None` (wasm).
    #[cfg(target_arch = "wasm32")]
    gpu_hatches_web: Vec<hatch_web_gpu::HatchWebGpu>,
    /// Wipeout masks — solid fills rendered after wires in a separate pass via
    /// the legacy per-primitive `WipeoutGpu` renderer.
    gpu_wipeouts: Vec<WipeoutGpu>,
    /// Per-wipeout draw-time skip flag (Phase 2.3 frustum cull). `true`
    /// when the wipeout's projected AABB sits entirely outside the
    /// viewport rect. Recomputed by `compute_wipeout_lod`.
    wipeout_skip_flags: Vec<bool>,
    gpu_images: Vec<ImageGpu>,
    gpu_meshes: Vec<MeshLodGpu>,
    /// Batched mesh geometry — every solid's LOD0 concatenated into a few large
    /// buffers so the whole set draws in a handful of calls instead of one per
    /// solid. Rebuilt only when the geometry epoch changes (see
    /// `cached_mesh_batch_epoch`), so hover / selection never re-pack it.
    gpu_mesh_batch: Vec<mesh_gpu::MeshBatchChunk>,
    /// `geometry_epoch` the batch was built for. `u64::MAX` = not built yet.
    pub cached_mesh_batch_epoch: u64,
    /// Tinted copies of just the selected / hovered solids, drawn over the
    /// static base batch so highlight shows without re-packing the whole set.
    /// Rebuilt only when the selection/hover or geometry changes — O(highlighted).
    gpu_mesh_highlight: Vec<mesh_gpu::MeshGpu>,
    /// `(geometry_epoch, selection_generation)` the highlight overlay was built for.
    pub cached_highlight_key: (u64, u64),
    /// Per-mesh LOD level (0=high, 1=mid, 2=low) picked each frame from
    /// the projected pixel diagonal. Mirrors `hatch_pixel_scissors` —
    /// recomputed in `compute_mesh_lod`.
    mesh_lod_levels: Vec<usize>,
    /// Per-mesh frustum-visible flag (Phase 2.2). `false` when the
    /// mesh's projected AABB falls entirely outside the viewport rect
    /// — the draw loop skips it. Mirrors `mesh_lod_levels` length /
    /// index space; recomputed alongside it.
    mesh_visible: Vec<bool>,
    /// Batched 3DFACE fill (all faces in one buffer) and edges (merged wire).
    gpu_face3d_fill: Option<Face3DGpu>,
    gpu_face3d_edges: Vec<WireGpu>,
    pub viewcube: ViewCubePipeline,
    /// Last `(geometry_epoch, camera_generation)` value for which GPU buffers
    /// were uploaded. We re-upload when either side changes — pan/zoom bumps
    /// camera_generation, which triggers re-culling and a fresh upload.
    /// `(geometry_epoch, camera_generation, selection_generation)` of the
    /// last static-buffer upload. Selection is tracked so the selected-hatch
    /// tint refreshes on select / deselect (issue #71).
    pub cached_epoch: (u64, u64, u64),
    /// Content id of the wire buffer currently resident on the GPU (Phase
    /// 3.2). When the incoming `ViewportData.wire_content_id` matches, the
    /// world-space wire vertices are unchanged (e.g. a pure pan reused the
    /// Model-tile tessellation) and `upload_wires` is skipped. `u64::MAX` =
    /// nothing uploaded yet.
    pub cached_wire_id: u64,
    /// `(wire_content_id, selection_generation)` the selection xray overlay
    /// (`gpu_selected_wires`) was last built for. Rebuilt when either changes —
    /// a pick bumps only `selection_generation`, refreshing the overlay without
    /// touching the main wire buffers.
    pub cached_selection: (u64, u64),
    /// `(geometry_epoch, selection_generation)` the resident mesh buffers were
    /// uploaded for. Bumps on a selection/hover change so highlighted solids
    /// re-upload with their tint.
    #[allow(dead_code)] // per-mesh LOD path, bypassed by the batched mesh draw
    pub cached_mesh_key: (u64, u64),
    /// `(geometry_epoch, face3d_fill_active)` the Face3D edge/fill buffers were
    /// uploaded for. The buffers are world-space and selection-independent, so
    /// they only change when the geometry changes or the 3D-fill mode toggles —
    /// never on a pan/orbit. Keyed separately from `cached_epoch` (which carries
    /// `camera_generation`) so a camera move no longer re-walks every wire to
    /// rebuild the Face3D fill buffer.
    pub cached_face3d_key: (u64, bool),
    /// Handle → indices into the resident wire set, built once per wire upload
    /// (when `cached_wire_id` changes). Lets the selection/hover xray overlay
    /// gather just the highlighted entity's wires (`O(highlighted)`) instead of
    /// scanning and string-parsing every wire on each hover change. Shared by
    /// `std::sync::Arc` alongside `gpu_wires` — built once per `wire_content_id`
    /// and cloned into every slot that renders it.
    pub(crate) wire_handle_index: std::sync::Arc<rustc_hash::FxHashMap<u64, Vec<u32>>>,
    /// Signature of the last frame's rendered scene (camera uniforms, geometry
    /// / selection generations, wire content id, render-mode flags, clip size,
    /// live preview). When the next frame's signature matches, the image is
    /// pixel-identical, so `render` skips every geometry pass + the MSAA
    /// resolve and only re-blits the resolve texture (which still holds it).
    /// This turns a pure cursor move over a large drawing from an O(N)
    /// re-rasterization into a single fullscreen blit. `u64::MAX` = nothing
    /// rendered yet (forces the first frame to render fully).
    pub render_sig: u64,
    /// Set by `prepare` each frame: `true` when this frame's signature matched
    /// `render_sig`, so `render` may skip the geometry passes. Read (never
    /// written) by `render`, which always runs after `prepare` for the frame.
    pub skip_geometry: bool,
    /// Interaction LOD: set by `prepare` when the view is actively navigating, so
    /// `render` skips the (per-pixel, GPU-dominating) hatch pass this frame. The
    /// scene-render cache holds the full-quality frame once the view settles.
    pub skip_hatch_frame: bool,
    /// Stable identity of the viewport that last used this (index-addressed)
    /// pipeline slot. The renderer's viewport list drops off-canvas viewports,
    /// so a slot can be reused by a *different* viewport across frames; when the
    /// occupant changes, all cache keys above belong to the previous one and are
    /// reset so every buffer re-uploads. `u64::MAX` = never used.
    pub slot_id: u64,
}

impl Pipeline {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        // ── Shared frame uniform buffer (view_proj etc.) ───────────────────
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewer.uniform_buffer"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group layout 0 — shared by wire and hatch pipelines.
        let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewer.frame_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewer.bind_group"),
            layout: &frame_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // ── Wire pipeline ──────────────────────────────────────────────────
        // Native: per-wire constants live in a storage buffer (group 1), so the
        // slim instance only carries per-segment data. WebGL2 has no
        // vertex-stage storage buffers, so the wasm build keeps the fat instance
        // and a single bind group.
        #[cfg(not(target_arch = "wasm32"))]
        let wire_const_bgl = wire_gpu::WireConst::bind_group_layout(device);
        #[cfg(not(target_arch = "wasm32"))]
        let wire_bgls: &[&wgpu::BindGroupLayout] = &[&frame_bgl, &wire_const_bgl];
        #[cfg(target_arch = "wasm32")]
        let wire_bgls: &[&wgpu::BindGroupLayout] = &[&frame_bgl];
        let wire_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wire.pipeline_layout"),
            bind_group_layouts: wire_bgls,
            push_constant_ranges: &[],
        });

        let depth_tex = create_depth_texture(device, Size::new(1, 1));
        let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let wire_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wire.shader"),
            #[cfg(not(target_arch = "wasm32"))]
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/wire_indexed.wgsl"
            ))),
            #[cfg(target_arch = "wasm32")]
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/wire.wgsl"
            ))),
        });

        // Stencil test shared by every paper content pipeline: draw only where
        // the stencil equals the bound reference. Non-clipped content binds
        // reference 0 (matching the frame's stencil, cleared to 0); a clip
        // region's content binds reference 1 after its boundary has been stamped
        // into the stencil by the clip-mask pipeline. In Model space nothing
        // masks the stencil, so it stays 0 and reference 0 always passes.
        let content_stencil_face = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Equal,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::Keep,
        };
        let content_stencil = wgpu::StencilState {
            front: content_stencil_face,
            back: content_stencil_face,
            read_mask: 0xff,
            write_mask: 0x00,
        };

        let wire_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wire.pipeline"),
            layout: Some(&wire_layout),
            vertex: wgpu::VertexState {
                module: &wire_shader,
                entry_point: Some("vs_main"),
                buffers: &[wire_gpu::WireInstance::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &wire_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // ── Clip-mask pipeline ─────────────────────────────────────────────
        // Stamps a viewport / XCLIP boundary polygon into the stencil buffer:
        // no colour, no depth, stencil `Invert` (even-odd fill → interior marked
        // for any polygon, convex or not). The boundary is drawn as a triangle
        // fan in paper coordinates and transformed exactly like wires.
        let clip_mask_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("clip_mask.shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/clip_mask.wgsl"
            ))),
        });
        let clip_mask_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("clip_mask.pipeline_layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let clip_mask_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("clip_mask.pipeline"),
            layout: Some(&clip_mask_layout),
            vertex: wgpu::VertexState {
                module: &clip_mask_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x2,
                    }],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState {
                    front: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Always,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Invert,
                    },
                    back: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Always,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Invert,
                    },
                    read_mask: 0xff,
                    write_mask: 0xff,
                },
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &clip_mask_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // Black variant of `wire_pipeline` — same geometry/depth, black fragment
        // — for 3D mesh outline edges in filled modes (see the wire draw loop).
        let wire_black_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wire.black.pipeline"),
            layout: Some(&wire_layout),
            vertex: wgpu::VertexState {
                module: &wire_shader,
                entry_point: Some("vs_main"),
                buffers: &[wire_gpu::WireInstance::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &wire_shader,
                entry_point: Some("fs_black"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // Selection overlay variant: renders selected wires on top of everything
        // (depth_compare=Always), without writing depth. Ensures selected entities
        // are always fully visible regardless of occluding geometry.
        let wire_xray_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wire_xray.pipeline"),
            layout: Some(&wire_layout),
            vertex: wgpu::VertexState {
                module: &wire_shader,
                entry_point: Some("vs_main"),
                buffers: &[wire_gpu::WireInstance::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &wire_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // ── Hatch pipeline ─────────────────────────────────────────────────
        // binding 0 (HatchUniforms) is read by the vertex shader too — it
        // pulls `origin` to undo the CPU-side hatch-local pre-shift when
        // computing clip position. bindings 1 (Boundary) and 2
        // (FamilyBatch) stay fragment-only.
        let hatch_entry = |binding: u32, vis: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let frag = wgpu::ShaderStages::FRAGMENT;
        let vert_frag = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let wipeout_bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wipeout.bgl1"),
            entries: &[
                hatch_entry(0, vert_frag),
                hatch_entry(1, frag),
                hatch_entry(2, frag),
            ],
        });

        let wipeout_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wipeout.pipeline_layout"),
            bind_group_layouts: &[&frame_bgl, &wipeout_bgl1],
            push_constant_ranges: &[],
        });

        let wipeout_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wipeout.shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/wipeout.wgsl"
            ))),
        });

        let wipeout_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wipeout.pipeline"),
            layout: Some(&wipeout_layout),
            vertex: wgpu::VertexState {
                module: &wipeout_shader,
                entry_point: Some("vs_main"),
                buffers: &[wipeout_gpu::HatchVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &wipeout_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // ── Hatch batched pipeline (Phase 4-B) ─────────────────────────────
        // WebGL2 (the wasm fallback backend) has no vertex-stage storage
        // buffers, which this pipeline's bind group requires. Skip it on wasm
        // and run without the batched-hatch optimization.
        #[cfg(target_arch = "wasm32")]
        let (hatch_bgl1, hatch_pipeline): (
            Option<wgpu::BindGroupLayout>,
            Option<wgpu::RenderPipeline>,
        ) = (None, None);
        #[cfg(not(target_arch = "wasm32"))]
        let (hatch_bgl1, hatch_pipeline) = {
        let hatch_bgl1 = hatch_gpu::HatchGpu::bind_group_layout(device);
        let hatch_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hatch.pipeline_layout"),
            bind_group_layouts: &[&frame_bgl, &hatch_bgl1],
            push_constant_ranges: &[],
        });
        let hatch_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hatch.shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/hatch.wgsl"
            ))),
        });
        let hatch_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hatch.pipeline"),
            layout: Some(&hatch_layout),
            vertex: wgpu::VertexState {
                module: &hatch_shader,
                entry_point: Some("vs_main"),
                buffers: &[hatch_gpu::HatchVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &hatch_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });
        (Some(hatch_bgl1), Some(hatch_pipeline))
        };

        // WebGL2 hatch pipeline: texture-backed, replaces the storage-buffer
        // batched path (disabled above on wasm). Same frame layout / depth /
        // MSAA state as the wipeout pass.
        #[cfg(target_arch = "wasm32")]
        let (hatch_web_bgl1, hatch_web_pipeline) = {
            let bgl1 = hatch_web_gpu::HatchWebGpu::bind_group_layout(device);
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hatch_web.pipeline_layout"),
                bind_group_layouts: &[&frame_bgl, &bgl1],
                push_constant_ranges: &[],
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("hatch_web.shader"),
                source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                    "../../shaders/hatch_web.wgsl"
                ))),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("hatch_web.pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 16,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        }],
                    }],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth24PlusStencil8,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: content_stencil.clone(),
                    bias: wgpu::DepthBiasState {
                        constant: 1,
                        slope_scale: 1.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState {
                    count: MSAA_SAMPLES,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview: None,
                cache: None,
            });
            (bgl1, pipeline)
        };

        // ── Mesh pipeline ──────────────────────────────────────────────────
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh.shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/mesh.wgsl"
            ))),
        });

        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh.pipeline_layout"),
            bind_group_layouts: &[&frame_bgl],
            push_constant_ranges: &[],
        });

        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh.pipeline"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: Some("vs_main"),
                buffers: &[mesh_gpu::MeshVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, // two-sided: ACIS import winding is unreliable
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // Transparent variant — identical to `mesh_pipeline` but with depth
        // writes disabled. Non-opaque solids are drawn after the opaque fills
        // with this pipeline so they blend over the geometry behind them (which
        // has already written depth) instead of erasing it.
        let mesh_transparent_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mesh.transparent.pipeline"),
                layout: Some(&mesh_layout),
                vertex: wgpu::VertexState {
                    module: &mesh_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[mesh_gpu::MeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth24PlusStencil8,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: content_stencil.clone(),
                    bias: wgpu::DepthBiasState {
                        constant: 1,
                        slope_scale: 1.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState {
                    count: MSAA_SAMPLES,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &mesh_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview: None,
                cache: None,
            });

        // Highlight variant — same shader / vertex layout, but `depth_compare =
        // Always` and no depth write, so a hovered / selected solid's tint is
        // drawn on top of everything (it shows through occluding geometry) and
        // doesn't disturb the depth buffer for later passes.
        let mesh_highlight_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh.highlight.pipeline"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: Some("vs_main"),
                buffers: &[mesh_gpu::MeshVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // Wireframe variant — same shader / vertex layout / depth state,
        // only the input topology changes (LineList) and back-face
        // culling drops out (each triangle edge is shared between two
        // faces, one of which would otherwise hide the edge).
        // Edge/wireframe pipeline (LineList). `fs_edge` outputs the flat entity
        // colour — no lighting — for the lines-only modes. A `fs_edge_black`
        // twin (below) forces black for the edge overlay in filled modes.
        let make_edge_pipeline = |label: &'static str, fs: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&mesh_layout),
                vertex: wgpu::VertexState {
                    module: &mesh_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[mesh_gpu::MeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth24PlusStencil8,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: content_stencil.clone(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: MSAA_SAMPLES,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &mesh_shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview: None,
                cache: None,
            })
        };
        let mesh_wireframe_pipeline = make_edge_pipeline("mesh.wireframe.pipeline", "fs_edge");
        let mesh_edge_black_pipeline = make_edge_pipeline("mesh.edge_black.pipeline", "fs_edge_black");

        // Depth-only variant — TriangleList, back-face culling stays on
        // (we only want front-facing fragments to write depth so wires
        // on the far side of the mesh stay hidden), `write_mask` zero
        // so no fragment ever reaches the colour buffer.
        let mesh_depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh.depth.pipeline"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: Some("vs_main"),
                buffers: &[mesh_gpu::MeshVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, // two-sided: ACIS import winding is unreliable
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // ── Face3D pipeline ────────────────────────────────────────────────
        let face3d_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("face3d.shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/face3d.wgsl"
            ))),
        });

        let face3d_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("face3d.pipeline_layout"),
            bind_group_layouts: &[&frame_bgl],
            push_constant_ranges: &[],
        });

        let face3d_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("face3d.pipeline"),
            layout: Some(&face3d_layout),
            vertex: wgpu::VertexState {
                module: &face3d_shader,
                entry_point: Some("vs_main"),
                buffers: &[face3d_gpu::Face3DVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &face3d_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // Depth-only variant — write_mask zero, no blend. The face3d
        // shader still runs but its colour output is discarded, so we
        // get a pure depth prepass for HiddenLine.
        let face3d_depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("face3d.depth.pipeline"),
            layout: Some(&face3d_layout),
            vertex: wgpu::VertexState {
                module: &face3d_shader,
                entry_point: Some("vs_main"),
                buffers: &[face3d_gpu::Face3DVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &face3d_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // ── Image pipeline ─────────────────────────────────────────────────
        let image_bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image.bgl1"),
            entries: &[
                // binding 0: texture
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding 1: sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // binding 2: ImageParams uniform (read in vertex for the
                // draw-order z bias and in fragment for opacity).
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let image_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image.pipeline_layout"),
            bind_group_layouts: &[&frame_bgl, &image_bgl1],
            push_constant_ranges: &[],
        });

        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image.shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/image.wgsl"
            ))),
        });

        let image_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("image.pipeline"),
            layout: Some(&image_layout),
            vertex: wgpu::VertexState {
                module: &image_shader,
                entry_point: Some("vs_main"),
                buffers: &[image_gpu::ImageVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: content_stencil.clone(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &image_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        // ── Text (SDF glyph quads) ─────────────────────────────────────────
        let text_atlas_bgl = text_gpu::TextAtlasGpu::bind_group_layout(device);
        let text_pipeline =
            text_gpu::create_pipeline(device, &frame_bgl, &text_atlas_bgl, format, MSAA_SAMPLES);

        let viewcube = ViewCubePipeline::new(device, queue, format);

        let init_size = Size::new(1, 1);
        let msaa_view = create_msaa_texture(device, init_size, format)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let resolve_tex = create_resolve_texture(device, init_size, format);
        let resolve_view = resolve_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // ── Blit pipeline (resolve texture → surface target) ──────────────
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit.shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "../../shaders/blit.wgsl"
            ))),
        });

        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // UV crop uniform: [uv_offset_x, uv_offset_y, uv_scale_x, uv_scale_y]
        // padded to 16 bytes (std140 vec2 alignment). Defaulted to the
        // identity crop (offset 0, scale 1) for the common on-canvas case.
        let blit_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("blit.uniform_buffer"),
            contents: bytemuck::cast_slice(&[0.0f32, 0.0, 1.0, 1.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit.pipeline_layout"),
            bind_group_layouts: &[&blit_bgl],
            push_constant_ranges: &[],
        });

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit.pipeline"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Premultiplied-alpha blend: the geometry passes already
                    // wrote into a transparent MSAA target with standard
                    // `SrcAlpha / 1-SrcAlpha` blending, so AA-edge fragments
                    // sit as `(rgb * a, a)` in the resolve texture. Treating
                    // that as straight alpha during the surface blit would
                    // multiply by alpha a second time and darken thin lines
                    // / curves. `PREMULTIPLIED_ALPHA_BLENDING` uses `One` as
                    // the source colour factor and leaves the dst weighted
                    // by `1-SrcAlpha`, which is the correct compositing
                    // operator for already-premultiplied content.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });

        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit.sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit.bind_group"),
            layout: &blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resolve_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&blit_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: blit_uniform_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            wire_pipeline,
            clip_mask_pipeline,
            wire_black_pipeline,
            wire_xray_pipeline,
            #[cfg(not(target_arch = "wasm32"))]
            wire_const_bgl: Some(wire_const_bgl),
            #[cfg(target_arch = "wasm32")]
            wire_const_bgl: None,
            wipeout_pipeline,
            hatch_pipeline,
            #[cfg(target_arch = "wasm32")]
            hatch_web_bgl1,
            #[cfg(target_arch = "wasm32")]
            hatch_web_pipeline,
            image_pipeline,
            text_pipeline,
            text_atlas_bgl,
            text_atlas_gpu: None,
            text_vbuf: None,
            text_vcount: 0,
            text_highlight_vbuf: None,
            text_highlight_vcount: 0,
            text_preview_vbuf: None,
            text_preview_vcount: 0,
            silhouette_vbuf: None,
            silhouette_vcount: 0,
            mesh_pipeline,
            mesh_transparent_pipeline,
            mesh_highlight_pipeline,
            mesh_wireframe_pipeline,
            mesh_edge_black_pipeline,
            mesh_depth_pipeline,
            face3d_pipeline,
            face3d_depth_pipeline,
            uniform_buffer,
            uniform_bind_group,
            wipeout_bgl1,
            hatch_bgl1,
            image_bgl1,
            depth_texture_size: Size::new(1, 1),
            // (0, 0) forces the first `ensure_depth_texture` to allocate at the
            // real rounded size — the constructor textures above are placeholders.
            alloc_size: Size::new(0, 0),
            depth_view,
            msaa_view,
            resolve_view,
            blit_pipeline,
            blit_bind_group_layout: blit_bgl,
            blit_sampler,
            blit_bind_group,
            blit_uniform_buffer,
            surface_format: format,
            gpu_wires: std::sync::Arc::new(vec![]),
            #[cfg(not(target_arch = "wasm32"))]
            wire_arena: None,
            #[cfg(not(target_arch = "wasm32"))]
            wire_arena_mesh: None,
            #[cfg(not(target_arch = "wasm32"))]
            wire_arena_id: u64::MAX,
            clip_boundary: None,
            gpu_selected_wires: vec![],
            gpu_preview_wires: vec![],
            gpu_hatch: None,
            #[cfg(target_arch = "wasm32")]
            gpu_hatches_web: vec![],
            gpu_wipeouts: vec![],
            wipeout_skip_flags: vec![],
            gpu_images: vec![],
            gpu_meshes: vec![],
            gpu_mesh_batch: vec![],
            cached_mesh_batch_epoch: u64::MAX,
            gpu_mesh_highlight: vec![],
            cached_highlight_key: (u64::MAX, u64::MAX),
            mesh_lod_levels: vec![],
            mesh_visible: vec![],
            gpu_face3d_fill: None,
            gpu_face3d_edges: vec![],
            viewcube,
            cached_epoch: (u64::MAX, u64::MAX, u64::MAX),
            cached_wire_id: u64::MAX,
            cached_selection: (u64::MAX, u64::MAX),
            cached_mesh_key: (u64::MAX, u64::MAX),
            cached_face3d_key: (u64::MAX, false),
            wire_handle_index: std::sync::Arc::new(rustc_hash::FxHashMap::default()),
            render_sig: u64::MAX,
            skip_geometry: false,
            skip_hatch_frame: false,
            slot_id: u64::MAX,
        }
    }

    /// Build the resident wire batches + handle index for `wires`, wrapped in
    /// `Arc` so the caller can cache them by `wire_content_id` and share one
    /// copy across every slot that renders that content (see
    /// `MultiPipeline::wire_buffer_cache`). Takes `&self` (reads only the const
    /// bind-group layout) so the shared cache — not the slot — owns the result.
    pub fn build_wire_buffers(
        &self,
        device: &wgpu::Device,
        wires: &[WireModel],
        depth_map: &rustc_hash::FxHashMap<u64, [f32; 2]>,
    ) -> (
        std::sync::Arc<Vec<WireGpu>>,
        std::sync::Arc<rustc_hash::FxHashMap<u64, Vec<u32>>>,
    ) {
        // Batch the wire pass: instead of one GPU buffer + one draw call per
        // WireModel (tens of thousands on a large drawing), merge maximal runs
        // of *consecutive* wires that share scissor + mesh-edge state into one
        // concatenated instance buffer each. The draw loop then issues one draw
        // per run. Runs must be consecutive (not regrouped) so the original
        // wire order — already sorted by draw order — is preserved; depth bias
        // and alpha blending both depend on it. Scissor and mesh-edge stay
        // grouping keys because the draw loop sets one scissor per batch and
        // skips whole mesh-edge batches in shaded modes.
        // A 3D mesh entity (PolyfaceMesh / PolygonMesh) emits its face fill and
        // its outline edges as *separate* WireModels sharing the entity handle
        // (`name`): the fill carries `fill_tris` + a non-empty `fill_tris_low`
        // (real 3D depth, same test face3d uses); the edge carries `points`.
        // Flag the edge wire as `is_3d_mesh_edge` so the draw loop can hide it in
        // clean-shaded modes and draw it black in filled-with-edges modes.
        let mesh_names: rustc_hash::FxHashSet<&str> = wires
            .iter()
            .filter(|w| !w.fill_tris.is_empty() && !w.fill_tris_low.is_empty())
            .map(|w| w.name.as_str())
            .collect();
        let is_mesh_edge =
            |w: &WireModel| !w.points.is_empty() && mesh_names.contains(w.name.as_str());
        let mut batches: Vec<WireGpu> = Vec::new();
        let mut i = 0;
        while i < wires.len() {
            let mesh_edge = is_mesh_edge(&wires[i]);
            let mut j = i + 1;
            while j < wires.len() && is_mesh_edge(&wires[j]) == mesh_edge {
                j += 1;
            }
            batches.extend(WireGpu::from_run(device, &wires[i..j], depth_map, mesh_edge, self.wire_const_bgl.as_ref()));
            i = j;
        }

        // Index handle → wire slots once, here, so the per-hover selection
        // overlay can gather just the highlighted wires instead of scanning +
        // string-parsing the whole set every time the hovered entity changes.
        let mut index: rustc_hash::FxHashMap<u64, Vec<u32>> = rustc_hash::FxHashMap::default();
        index.reserve(wires.len());
        for (idx, w) in wires.iter().enumerate() {
            if let Ok(h) = w.name.parse::<u64>() {
                index.entry(h).or_default().push(idx as u32);
            }
        }
        (std::sync::Arc::new(batches), std::sync::Arc::new(index))
    }

    /// Build the selection xray overlay: full-brightness copies of the wires
    /// whose entity handle is in `highlight`, drawn on top so the selection is
    /// always visible. Selection is no longer baked into the wire tessellation,
    /// so this is driven by the live highlight set and rebuilt only when the
    /// selection (or the underlying wire content) changes — picking an entity
    /// refreshes just this overlay instead of re-tessellating the model. The
    /// xray pass applies neither scissor nor mesh-edge skip, so everything
    /// merges into one order-preserving run.
    pub fn upload_selected_wires(
        &mut self,
        device: &wgpu::Device,
        wires: &[WireModel],
        selected: &rustc_hash::FxHashSet<acadrust::Handle>,
        hover: Option<acadrust::Handle>,
        depth_map: &rustc_hash::FxHashMap<u64, [f32; 2]>,
    ) {
        let hover = hover.filter(|h| !selected.contains(h));
        if selected.is_empty() && hover.is_none() {
            self.gpu_selected_wires = vec![];
            return;
        }
        // Gather the highlighted entities' wires via the prebuilt index —
        // O(highlighted), no per-wire string parse. Selection recolours blue,
        // hover orange. Drawn on top (depth-compare Always) over the base pass.
        let mut out: Vec<WireModel> = Vec::new();
        let push = |handle_val: u64, color: [f32; 4], wires: &[WireModel], out: &mut Vec<WireModel>| {
            if let Some(idxs) = self.wire_handle_index.get(&handle_val) {
                let mut slots = idxs.clone();
                slots.sort_unstable();
                for &i in &slots {
                    if let Some(w) = wires.get(i as usize) {
                        let mut c = w.clone();
                        c.color = color;
                        out.push(c);
                    }
                }
            }
        };
        for h in selected {
            push(h.value(), WireModel::SELECTED, wires, &mut out);
        }
        if let Some(h) = hover {
            push(h.value(), WireModel::HOVER, wires, &mut out);
        }
        self.gpu_selected_wires =
            WireGpu::from_run(device, &out, depth_map, false, self.wire_const_bgl.as_ref());
    }

    /// Build the text-highlight overlay: the glyph quads of just the selected /
    /// hovered text, recoloured (selection blue, hover orange) and drawn over
    /// the base text pass. Uses the same handle→wire index as the selected-wire
    /// overlay, so it is O(highlighted). Empty when nothing is highlighted.
    pub fn upload_text_highlight(
        &mut self,
        device: &wgpu::Device,
        wires: &[WireModel],
        selected: &rustc_hash::FxHashSet<acadrust::Handle>,
        hover: Option<acadrust::Handle>,
    ) {
        let hover = hover.filter(|h| !selected.contains(h));
        if selected.is_empty() && hover.is_none() {
            self.text_highlight_vbuf = None;
            self.text_highlight_vcount = 0;
            return;
        }
        let mut out: Vec<text_gpu::TextVertex> = Vec::new();
        let push =
            |handle_val: u64, tint: [f32; 4], wires: &[WireModel], out: &mut Vec<text_gpu::TextVertex>| {
                if let Some(idxs) = self.wire_handle_index.get(&handle_val) {
                    for &i in idxs {
                        if let Some(w) = wires.get(i as usize) {
                            for v in &w.text_verts {
                                out.push(text_gpu::TextVertex {
                                    color: [tint[0], tint[1], tint[2], v.color[3]],
                                    ..*v
                                });
                            }
                        }
                    }
                }
            };
        for h in selected {
            push(h.value(), WireModel::SELECTED, wires, &mut out);
        }
        if let Some(h) = hover {
            push(h.value(), WireModel::HOVER, wires, &mut out);
        }
        self.text_highlight_vcount = out.len() as u32;
        self.text_highlight_vbuf = text_gpu::upload_vertices(device, &out);
    }

    /// Upload the live overlay (command preview / interim / grip-drag) wires.
    /// Small and refreshed each frame they're present; kept separate from the
    /// base wire buffer so a drag never re-uploads the resident base set.
    pub fn upload_preview_wires(
        &mut self,
        device: &wgpu::Device,
        wires: &[WireModel],
        depth_map: &rustc_hash::FxHashMap<u64, [f32; 2]>,
    ) {
        self.gpu_preview_wires = if wires.is_empty() {
            vec![]
        } else {
            WireGpu::from_run(device, wires, depth_map, false, self.wire_const_bgl.as_ref())
        };
    }

    /// Upload the live grip-drag / command-preview SDF glyph quads. Re-uploaded
    /// every frame they're present, kept separate from the epoch-cached base
    /// text buffer so a drag never re-uploads the (potentially huge) resident
    /// text set — and so the dragged entity's glyphs (hidden from the base set
    /// while the drag is live) still reach the GPU each frame (issue #316).
    pub fn upload_preview_text(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        verts: &[text_gpu::TextVertex],
    ) {
        if verts.is_empty() {
            self.text_preview_vbuf = None;
            self.text_preview_vcount = 0;
            return;
        }
        // Ensure the glyph atlas exists / is current even when the base text
        // pass didn't run this frame (e.g. the dragged text is the only text).
        if let Ok(mut atlas) = crate::scene::text::sdf_atlas::text_atlas().lock() {
            if self.text_atlas_gpu.is_none() || atlas.is_dirty() {
                self.text_atlas_gpu = Some(text_gpu::TextAtlasGpu::upload(
                    device,
                    queue,
                    &atlas,
                    &self.text_atlas_bgl,
                ));
                atlas.clear_dirty();
            }
        }
        self.text_preview_vbuf = text_gpu::upload_vertices(device, verts);
        self.text_preview_vcount = verts.len() as u32;
    }

    // The math lives outside the GPU method so it can be unit-tested; see the
    // module test below.

    /// Rebuild the per-frame DISPSILH silhouette line list from the mesh sets'
    /// curved-face generators and the current eye. For each cone/cylinder face
    /// the silhouette runs at the two angles where the surface turns edge-on to
    /// the view — `θ = φ ± acos(-tanα·(view·axis) / |view⊥|)`, which reduces to
    /// `φ ± π/2` for a cylinder. Segments are uploaded in the mesh vertex format
    /// so they draw through the existing wireframe pipeline.
    pub fn upload_silhouettes(
        &mut self,
        device: &wgpu::Device,
        sets: &[crate::scene::model::mesh_model::MeshLodSet],
        view_dir: glam::Vec3,
    ) {
        // Silhouettes follow the view *angle* only — a single parallel direction
        // for the whole scene, not the eye-to-surface vector — so the outline
        // stays put under pan and doesn't foreshorten. This is the orthographic
        // silhouette a CAD wireframe expects.
        let view = glam::DVec3::new(view_dir.x as f64, view_dir.y as f64, view_dir.z as f64)
            .normalize_or(glam::DVec3::NEG_Z);
        use crate::scene::model::mesh_model::CurvedGen;
        use crate::scene::pipeline::mesh_gpu::MeshVertex;
        let mut verts: Vec<MeshVertex> = Vec::new();
        let d3 = |a: [f32; 3]| glam::DVec3::new(a[0] as f64, a[1] as f64, a[2] as f64);
        let lo = |c: [f32; 3], l: [f32; 3]| {
            glam::DVec3::new(
                c[0] as f64 + l[0] as f64,
                c[1] as f64 + l[1] as f64,
                c[2] as f64 + l[2] as f64,
            )
        };
        for set in sets {
            let color = set.lods.first().map(|m| m.color).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let mk = |w: glam::DVec3| -> MeshVertex {
                let (hx, hy, hz) = (w.x as f32, w.y as f32, w.z as f32);
                MeshVertex {
                    position: [hx, hy, hz],
                    normal: [0.0, 1.0, 0.0],
                    color,
                    position_low: [
                        (w.x - hx as f64) as f32,
                        (w.y - hy as f64) as f32,
                        (w.z - hz as f64) as f32,
                    ],
                }
            };
            for g in &set.curved_gens {
                match g {
                    CurvedGen::Cone {
                        base, base_low, axis, u_dir, v_dir, radius, tan_a,
                        h_max, theta_min, theta_span, full,
                    } => {
                        let base = lo(*base, *base_low);
                        let (axis, u, v) = (d3(*axis), d3(*u_dir), d3(*v_dir));
                        let Some((t0, t1)) =
                            silhouette_thetas(view.dot(u), view.dot(v), view.dot(axis), *tan_a as f64)
                        else {
                            continue;
                        };
                        let r0 = *radius as f64;
                        let r1 = *radius as f64 + *h_max as f64 * *tan_a as f64;
                        for theta in [t0, t1] {
                            if !full {
                                let off = (theta - *theta_min as f64).rem_euclid(std::f64::consts::TAU);
                                if off > *theta_span as f64 {
                                    continue;
                                }
                            }
                            let (c, s) = (theta.cos(), theta.sin());
                            let radial = u * c + v * s;
                            verts.push(mk(base + radial * r0));
                            verts.push(mk(base + radial * r1 + axis * *h_max as f64));
                        }
                    }
                    CurvedGen::Sphere {
                        center, center_low, pole, u_dir, v_dir, radius,
                        theta_min, theta_span, full, phi_min, phi_max,
                    } => {
                        let c = lo(*center, *center_low);
                        let (pole, u, v) = (d3(*pole), d3(*u_dir), d3(*v_dir));
                        let r = *radius as f64;
                        // Great circle in the plane perpendicular to the view.
                        let mut e1 = view.cross(pole);
                        if e1.length_squared() < 1e-12 {
                            e1 = view.cross(u);
                        }
                        let e1 = e1.normalize();
                        let e2 = view.cross(e1).normalize();
                        const N: usize = 64;
                        let mut prev: Option<glam::DVec3> = None;
                        for i in 0..=N {
                            let a = std::f64::consts::TAU * (i as f64 / N as f64);
                            let dir = e1 * a.cos() + e2 * a.sin();
                            // Keep only the arc that lies on the actual face.
                            let on_face = *full || {
                                let phi = dir.dot(pole).clamp(-1.0, 1.0).acos();
                                let th = dir.dot(v).atan2(dir.dot(u));
                                let toff = (th - *theta_min as f64).rem_euclid(std::f64::consts::TAU);
                                phi >= *phi_min as f64
                                    && phi <= *phi_max as f64
                                    && (toff <= *theta_span as f64)
                            };
                            let p = if on_face { Some(c + dir * r) } else { None };
                            if let (Some(a), Some(b)) = (prev, p) {
                                verts.push(mk(a));
                                verts.push(mk(b));
                            }
                            prev = p;
                        }
                    }
                    CurvedGen::Torus {
                        center, center_low, axis, u_dir, v_dir, major, minor,
                        phi_min, phi_span, full,
                    } => {
                        let ctr = lo(*center, *center_low);
                        let (axis, u, v) = (d3(*axis), d3(*u_dir), d3(*v_dir));
                        let (major, minor) = (*major as f64, *minor as f64);
                        // True silhouette: at each revolution angle the tube is a
                        // circle; the two points where its normal turns edge-on
                        // trace two curves around the ring. Sample the revolution
                        // and connect consecutive edge-on points.
                        const N: usize = 72;
                        let span = if *full { std::f64::consts::TAU } else { *phi_span as f64 };
                        let mut prev: [Option<glam::DVec3>; 2] = [None, None];
                        for i in 0..=N {
                            let phi = *phi_min as f64 + span * (i as f64 / N as f64);
                            let radial = u * phi.cos() + v * phi.sin();
                            let ring = ctr + radial * major;
                            let (rv, av) = (radial.dot(view), axis.dot(view));
                            if rv.abs() < 1e-9 && av.abs() < 1e-9 {
                                prev = [None, None];
                                continue;
                            }
                            // tube normal(θ) = radial·cosθ + axis·sinθ; ⟂ view at
                            // θ = atan2(-rv, av) and +π.
                            let th = (-rv).atan2(av);
                            let cur = [th, th + std::f64::consts::PI];
                            for k in 0..2 {
                                let t = cur[k];
                                let p = ring + (radial * t.cos() + axis * t.sin()) * minor;
                                if let Some(pp) = prev[k] {
                                    verts.push(mk(pp));
                                    verts.push(mk(p));
                                }
                                prev[k] = Some(p);
                            }
                        }
                    }
                }
            }
        }
        if verts.is_empty() {
            self.silhouette_vbuf = None;
            self.silhouette_vcount = 0;
            return;
        }
        use wgpu::util::DeviceExt;
        self.silhouette_vbuf = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh.silhouette.vbuf"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        }));
        self.silhouette_vcount = verts.len() as u32;
    }

    /// Upload this content viewport's non-rectangular clip boundary as a
    /// triangle-fan vertex buffer in render-target NDC. The boundary is already
    /// projected (paper shape → the same visible-sub-rect crop as the content),
    /// so the mask pipeline stamps it straight into the stencil with `Invert`
    /// (even-odd fill → interior marked, any convexity). Empty input clears the
    /// boundary so the viewport renders unclipped (its render rectangle clips).
    pub fn upload_clip_boundary(&mut self, device: &wgpu::Device, boundary_ndc: &[[f32; 2]]) {
        use wgpu::util::DeviceExt;
        if boundary_ndc.len() < 3 {
            self.clip_boundary = None;
            return;
        }
        // Triangle fan (p0, pi, pi+1).
        let mut verts: Vec<f32> = Vec::with_capacity((boundary_ndc.len() - 2) * 6);
        for i in 1..boundary_ndc.len() - 1 {
            for &p in &[boundary_ndc[0], boundary_ndc[i], boundary_ndc[i + 1]] {
                verts.extend_from_slice(&[p[0], p[1]]);
            }
        }
        if verts.is_empty() {
            self.clip_boundary = None;
            return;
        }
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("clip_boundary.vbuf"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.clip_boundary = Some((vbuf, (verts.len() / 2) as u32));
    }

    /// Per-frame visibility refresh for the batched hatch path.
    /// Combines Phase 3.3 sub-pixel LOD skip with Phase 2.3 frustum
    /// cull and pushes the resulting 0/1 mask through to the GPU
    /// `visibility_buffer`. Vertex shader maps 0 → out-of-NDC clip,
    /// so the rasterizer culls the primitive before any fragment
    /// runs.
    pub fn compute_hatch_lod(
        &mut self,
        queue: &wgpu::Queue,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        clip_w: u32,
        clip_h: u32,
    ) {
        let Some(batch) = &mut self.gpu_hatch else {
            return;
        };
        for (i, aabb) in batch.instance_aabbs.iter().enumerate() {
            let skip = aabb_below_pixel(*aabb, view_rot, eye, clip_w, clip_h, 2.0)
                || aabb_offscreen(*aabb, view_rot, eye, clip_w, clip_h);
            batch.visibility[i] = if skip { 0 } else { 1 };
        }
        batch.upload_visibility(queue);
    }

    /// Per-frame wipeout frustum-skip flag (Phase 2.3). Mirrors
    /// `compute_hatch_lod`'s frustum branch. No sub-pixel skip:
    /// wipeouts mask, so dropping a sub-pixel one wouldn't be wrong
    /// but also wouldn't pay off — they're usually few.
    pub fn compute_wipeout_lod(&mut self, view_rot: glam::Mat4, eye: glam::DVec3, clip_w: u32, clip_h: u32) {
        self.wipeout_skip_flags = self
            .gpu_wipeouts
            .iter()
            .map(|h| aabb_offscreen(h.world_aabb, view_rot, eye, clip_w, clip_h))
            .collect();
    }

    /// Upload all 3DFACE entities as two batched GPU objects:
    /// - `gpu_face3d_fill`: filled triangles (1 buffer, 1 draw call)
    /// - `gpu_face3d_edges`: merged edge wires (1 buffer, 1 draw call)
    pub fn upload_face3d(
        &mut self,
        device: &wgpu::Device,
        face3d_wires: &[WireModel],
        all_wires: &[WireModel],
        wireframe_only: bool,
        depth_map: &rustc_hash::FxHashMap<u64, [f32; 2]>,
    ) {
        // Edge buffer is always built from `face3d_wires`, so 3DFACE
        // outlines stay on the screen regardless of mode.
        self.gpu_face3d_edges =
            WireGpu::from_batch(device, face3d_wires, depth_map, self.wire_const_bgl.as_ref());
        // Fill buffer split: 3D quads + PolyfaceMesh / PolygonMesh face
        // tris go to `chunks_3d` (gated by `keep_3d_mesh_fills`);
        // 2D fills (text-LOD greek, MultiLeader background) go to
        // `chunks_2d` and are visible in every mode.
        let keep_3d_mesh_fills = !wireframe_only;
        let has_any_2d_fill = all_wires
            .iter()
            .any(|w| !w.fill_tris.is_empty() && w.points.is_empty());
        let has_any_3d_fill = !face3d_wires.is_empty()
            || all_wires
                .iter()
                .any(|w| !w.fill_tris.is_empty() && !w.points.is_empty());
        let has_fills = has_any_2d_fill || (keep_3d_mesh_fills && has_any_3d_fill);
        if !has_fills {
            self.gpu_face3d_fill = None;
        } else {
            self.gpu_face3d_fill = Some(Face3DGpu::from_wires(
                device,
                face3d_wires,
                all_wires,
                keep_3d_mesh_fills,
                depth_map,
            ));
        }
    }

    #[allow(dead_code)] // per-mesh upload path, bypassed by upload_mesh_batch
    pub fn upload_meshes(
        &mut self,
        device: &wgpu::Device,
        meshes: &[MeshLodSet],
        selected: &rustc_hash::FxHashSet<acadrust::Handle>,
        hover: Option<acadrust::Handle>,
    ) {
        self.gpu_meshes = meshes
            .iter()
            .filter(|s| s.lods.iter().any(|m| !m.indices.is_empty()))
            .map(|s| {
                // A mesh's name is its source entity handle (decimal). Selection
                // wins over hover when both apply to the same solid.
                let h = s
                    .lods
                    .first()
                    .and_then(|m| m.name.parse::<u64>().ok())
                    .map(acadrust::Handle::new);
                let mode = match h {
                    Some(h) if selected.contains(&h) => mesh_gpu::Highlight::Selected,
                    Some(h) if Some(h) == hover => mesh_gpu::Highlight::Hover,
                    _ => mesh_gpu::Highlight::None,
                };
                MeshLodGpu::new(device, s, mode)
            })
            .collect();
    }

    /// Build the batched mesh buffers (a few large buffers for the whole solid
    /// set, drawn in a handful of calls). Selection/hover tint is intentionally
    /// not applied here — the batch is geometry-only and stays resident across
    /// camera moves and pick changes.
    pub fn upload_mesh_batch(&mut self, device: &wgpu::Device, meshes: &[MeshLodSet]) {
        let (chunks, _tris) = mesh_gpu::build_mesh_batch(device, meshes);
        self.gpu_mesh_batch = chunks;
    }

    /// Build tinted copies of just the selected / hovered solids. Drawn over the
    /// static base batch with the LessEqual mesh pipeline so the tint shows on
    /// top at the same depth. O(highlighted), so a pick never touches the rest.
    pub fn upload_mesh_highlight(
        &mut self,
        device: &wgpu::Device,
        meshes: &[MeshLodSet],
        selected: &rustc_hash::FxHashSet<acadrust::Handle>,
        hover: Option<acadrust::Handle>,
    ) {
        use mesh_gpu::{Highlight, MeshGpu};
        let mut out = Vec::new();
        if selected.is_empty() && hover.is_none() {
            self.gpu_mesh_highlight = out;
            return;
        }
        for set in meshes {
            let Some(m) = set.lods.iter().find(|m| !m.indices.is_empty()) else {
                continue;
            };
            let h = m.name.parse::<u64>().ok().map(acadrust::Handle::new);
            let mode = match h {
                Some(h) if selected.contains(&h) => Highlight::Selected,
                Some(h) if Some(h) == hover => Highlight::Hover,
                _ => continue,
            };
            out.push(MeshGpu::new(device, m, mode));
        }
        self.gpu_mesh_highlight = out;
    }

    /// Per-frame mesh LOD selector. Picks slot 0/1/2 based on the
    /// projected pixel diagonal of each mesh's `world_aabb` (Phase 3.4
    /// ladder: >200 px → 0, 50–200 → 1, <50 → 2). Falls back to the
    /// nearest available lower slot when a level wasn't generated.
    pub fn compute_mesh_lod(&mut self, view_rot: glam::Mat4, eye: glam::DVec3, clip_w: u32, clip_h: u32) {
        self.mesh_lod_levels = self
            .gpu_meshes
            .iter()
            .map(|m| pick_mesh_lod(m, view_rot, eye, clip_w, clip_h))
            .collect();
        // Phase 2.2 — frustum-visibility flag per mesh. Cheap: same
        // 4-corner projection used for LOD selection, just answering a
        // different question (any corner inside the viewport rect?).
        self.mesh_visible = self
            .gpu_meshes
            .iter()
            .map(|m| !aabb_offscreen(m.world_aabb, view_rot, eye, clip_w, clip_h))
            .collect();
    }

    pub fn upload_hatches(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        hatches: &[HatchModel],
    ) {
        // WebGL2: no storage-buffer batched pipeline — render each hatch through
        // the texture-backed per-hatch renderer instead (issue #204).
        #[cfg(target_arch = "wasm32")]
        {
            let webs: Vec<hatch_web_gpu::HatchWebGpu> = hatches
                .iter()
                .filter(|h| h.boundary.len() >= 3)
                .map(|h| hatch_web_gpu::HatchWebGpu::new(device, queue, h, &self.hatch_web_bgl1))
                .collect();
            self.gpu_hatches_web = webs;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = queue;
            let Some(bgl1) = &self.hatch_bgl1 else {
                self.gpu_hatch = None;
                return;
            };
            let renderable: Vec<HatchModel> =
                hatches.iter().filter(|h| h.boundary.len() >= 3).cloned().collect();
            self.gpu_hatch = hatch_gpu::HatchGpu::build(device, bgl1, &renderable);
        }
    }

    pub fn upload_wipeouts(&mut self, device: &wgpu::Device, wipeouts: &[HatchModel]) {
        self.gpu_wipeouts = wipeouts
            .iter()
            .filter(|h| h.boundary.len() >= 3)
            .map(|h| WipeoutGpu::new(device, h, &self.wipeout_bgl1))
            .collect();
    }

    pub fn upload_images(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        images: &[ImageModel],
    ) {
        self.gpu_images = images
            .iter()
            .filter_map(|m| ImageGpu::new(device, queue, m, &self.image_bgl1))
            .collect();
    }

    /// Upload the frame's SDF text-quad vertices, and (re)build the GPU glyph
    /// atlas from the shared CPU atlas when it grew (new glyphs baked by the
    /// text collector). `verts` empty (flag off) leaves nothing to draw.
    pub fn upload_text(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        verts: &[text_gpu::TextVertex],
    ) {
        if let Ok(mut atlas) = crate::scene::text::sdf_atlas::text_atlas().lock() {
            if self.text_atlas_gpu.is_none() || atlas.is_dirty() {
                self.text_atlas_gpu = Some(text_gpu::TextAtlasGpu::upload(
                    device,
                    queue,
                    &atlas,
                    &self.text_atlas_bgl,
                ));
                atlas.clear_dirty();
            }
        }
        self.text_vbuf = text_gpu::upload_vertices(device, verts);
        self.text_vcount = verts.len() as u32;
    }

    pub fn upload_uniforms(&self, queue: &wgpu::Queue, uniforms: &Uniforms) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(uniforms));
    }

    /// Write the blit shader's UV crop uniform. Call in `prepare` (the only
    /// place with a `&Queue`) — `render` then just submits the draw call.
    pub fn upload_blit_uv(&self, queue: &wgpu::Queue, uv_offset: [f32; 2], uv_scale: [f32; 2]) {
        // The geometry passes render at `depth_texture_size` into the top-left
        // corner of the (possibly larger) `alloc_size` resolve texture, so the
        // rendered image occupies UV [0, render/alloc]. Scale the incoming crop
        // — computed in full-image-normalized units — down into that region.
        let fx = self.depth_texture_size.width as f32 / self.alloc_size.width.max(1) as f32;
        let fy = self.depth_texture_size.height as f32 / self.alloc_size.height.max(1) as f32;
        queue.write_buffer(
            &self.blit_uniform_buffer,
            0,
            bytemuck::cast_slice(&[
                uv_offset[0] * fx,
                uv_offset[1] * fy,
                uv_scale[0] * fx,
                uv_scale[1] * fy,
            ]),
        );
    }

    /// Render the geometry passes at `vp_size` (the full viewport size — the
    /// MSAA / resolve textures are this size) and blit the resulting resolve
    /// to `surface_dest` on the swap-chain. The UV crop is read from the
    /// blit uniform buffer (written by `upload_blit_uv` during `prepare`)
    /// so a viewport that hangs off the canvas still composites the correct
    /// sub-rectangle to the visible portion of the surface.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        vp_size: Size<u32>,
        surface_dest: Rectangle<u32>,
        bg_color: [f32; 4],
        mesh_wireframe: bool,
        hidden_line: bool,
        show_3d_edges: bool,
    ) {
        let vp = Rectangle::<u32> {
            x: 0,
            y: 0,
            width: vp_size.width,
            height: vp_size.height,
        };
        let msaa = &self.msaa_view;
        let [r, g, b, a] = bg_color;
        let clear_color = wgpu::Color {
            r: r as f64,
            g: g as f64,
            b: b as f64,
            a: a as f64,
        };
        // Non-rectangular viewport clip: the boundary is stamped into the
        // just-cleared (0x00) stencil with `Invert`, so an odd (interior)
        // coverage becomes 0xFF. Every content pass then draws with reference
        // 0xFF so only the interior survives. Rectangular / unclipped viewports
        // leave the stencil at 0 and draw with reference 0 (the viewport's own
        // render rectangle does the clipping).
        let stencil_ref: u32 = if self.clip_boundary.is_some() { 0xFF } else { 0 };

        // Scene-render cache: when `prepare` found this frame pixel-identical
        // to the last (unchanged view / geometry / selection / preview), the
        // resolve texture already holds the image. Skip every geometry pass +
        // the MSAA resolve and fall straight through to the blit below. This
        // is what makes a pure cursor move cost one fullscreen blit instead of
        // re-rasterizing the whole drawing every frame.
        if !self.skip_geometry {
        // ── Pass 1: hatch fills ────────────────────────────────────────────
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hatch.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Clear MSAA to background color on the first pass.
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    // Clip stencil starts at 0 (= "unclipped", passes content
                    // bound to reference 0); clip masks stamp 1 into interiors.
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0),
                        store: wgpu::StoreOp::Store,
                    }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // MSAA texture is clip-bounds-sized, so viewport starts at (0, 0).
            pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
            // Stamp the viewport clip boundary into the just-cleared stencil
            // (interior → 1) before any content draws, so every pass below can
            // clip to the shape with reference 1.
            if let Some((vbuf, vcount)) = &self.clip_boundary {
                pass.set_pipeline(&self.clip_mask_pipeline);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.draw(0..*vcount, 0..1);
            }
            // Phase 4-B — single batched draw covers every hatch.
            // Vertex shader culls per-instance via the `visibility`
            // buffer (sub-pixel LOD + frustum cull written each frame
            // by `compute_hatch_lod`). Per-hatch viewport scissor
            // (paper-space MSPACE) isn't ported to the batched path
            // yet — follow-up if it shows up as a visual issue.
            if let (Some(batch), Some(pipeline)) =
                (&self.gpu_hatch, &self.hatch_pipeline)
            {
                // Skipped while navigating (interaction LOD) — the per-pixel
                // hatch pass dominates the GPU frame on hatch-heavy drawings.
                if !self.skip_hatch_frame {
                    pass.set_pipeline(pipeline);
                    pass.set_stencil_reference(stencil_ref);
                    pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    pass.set_bind_group(1, &batch.bind_group, &[]);
                    pass.set_vertex_buffer(0, batch.vertex_buffer.slice(..));
                    pass.draw(0..batch.vertex_count, 0..1);
                }
            }

            // WebGL2: the batched `gpu_hatch` is always None (no storage
            // buffers), so draw the texture-backed per-hatch renderer in this
            // same pass (before wires, so wires stay on top). (#204)
            #[cfg(target_arch = "wasm32")]
            if !self.skip_hatch_frame {
                pass.set_pipeline(&self.hatch_web_pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_stencil_reference(stencil_ref);
                for hatch in &self.gpu_hatches_web {
                    pass.set_bind_group(1, &hatch.bind_group, &[]);
                    pass.set_vertex_buffer(0, hatch.vertex_buffer.slice(..));
                    pass.draw(0..6, 0..1);
                }
            }
        }

        // ── Pass 2: raster images ─────────────────────────────────────────
        if !self.gpu_images.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("image.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
            pass.set_pipeline(&self.image_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_stencil_reference(stencil_ref);
            for img in self.gpu_images.iter() {
                pass.set_bind_group(1, &img.bind_group, &[]);
                pass.set_vertex_buffer(0, img.vertex_buffer.slice(..));
                pass.draw(0..img.vertex_count, 0..1);
            }
        }

        // ── Pass 4: solid meshes (batched) ────────────────────────────────
        if !self.gpu_mesh_batch.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_stencil_reference(stencil_ref);
            // Four draw paths share this pass:
            //  - Solid:           `mesh_pipeline` + triangle index buf.
            //  - Wireframe:       `mesh_wireframe_pipeline` + the
            //                     pre-built `wire_index_buffer`.
            //  - HiddenLine:      depth prepass (`mesh_depth_pipeline`,
            //                     writes Z, no colour) → wire overlay.
            //  - Solid+Edges:     `mesh_pipeline` shaded fill → wire
            //                     overlay; LessEqual depth test on the
            //                     wire pass keeps the edges crisp on
            //                     top of the shaded surface.
            let want_solid_with_edges = !hidden_line && !mesh_wireframe && show_3d_edges;
            // Each path now binds a chunk's buffers and draws the whole chunk in
            // one call — a handful of draws total instead of one per solid. No
            // per-solid LOD / frustum cull in the batched path (the batch is
            // resident in full); that is reintroduced separately if needed.
            if hidden_line {
                // Depth-only prepass: every solid surface occludes hidden edges,
                // so both the opaque and the transparent tris write depth here.
                pass.set_pipeline(&self.mesh_depth_pipeline);
                for c in &self.gpu_mesh_batch {
                    pass.set_vertex_buffer(0, c.vertex_buffer.slice(..));
                    if c.index_count != 0 {
                        pass.set_index_buffer(
                            c.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..c.index_count, 0, 0..1);
                    }
                    if c.transp_index_count != 0 {
                        pass.set_index_buffer(
                            c.transp_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..c.transp_index_count, 0, 0..1);
                    }
                }
                pass.set_pipeline(&self.mesh_wireframe_pipeline);
                for c in &self.gpu_mesh_batch {
                    // Plain-mesh triangulation edges.
                    if c.wire_index_count != 0 {
                        pass.set_vertex_buffer(0, c.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            c.wire_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..c.wire_index_count, 0, 0..1);
                    }
                    // ACIS solid B-rep feature edges (LineList, non-indexed).
                    if c.edge_vertex_count != 0 {
                        pass.set_vertex_buffer(0, c.edge_vertex_buffer.slice(..));
                        pass.draw(0..c.edge_vertex_count, 0..1);
                    }
                }
                // DISPSILH silhouettes (whole batch, one buffer).
                if let Some(ref vb) = self.silhouette_vbuf {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.draw(0..self.silhouette_vcount, 0..1);
                }
            } else {
                if mesh_wireframe {
                    pass.set_pipeline(&self.mesh_wireframe_pipeline);
                    for c in &self.gpu_mesh_batch {
                        if c.wire_index_count != 0 {
                            pass.set_vertex_buffer(0, c.vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                c.wire_index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.draw_indexed(0..c.wire_index_count, 0, 0..1);
                        }
                        if c.edge_vertex_count != 0 {
                            pass.set_vertex_buffer(0, c.edge_vertex_buffer.slice(..));
                            pass.draw(0..c.edge_vertex_count, 0..1);
                        }
                    }
                    // DISPSILH silhouettes.
                    if let Some(ref vb) = self.silhouette_vbuf {
                        pass.set_vertex_buffer(0, vb.slice(..));
                        pass.draw(0..self.silhouette_vcount, 0..1);
                    }
                } else {
                    // Opaque fills first (they write depth).
                    pass.set_pipeline(&self.mesh_pipeline);
                    for c in &self.gpu_mesh_batch {
                        if c.index_count == 0 {
                            continue;
                        }
                        pass.set_vertex_buffer(0, c.vertex_buffer.slice(..));
                        pass.set_index_buffer(c.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..c.index_count, 0, 0..1);
                    }
                    // Transparent fills last, with depth writes disabled, so they
                    // blend over the opaque geometry behind them instead of
                    // culling it via the depth buffer.
                    pass.set_pipeline(&self.mesh_transparent_pipeline);
                    for c in &self.gpu_mesh_batch {
                        if c.transp_index_count == 0 {
                            continue;
                        }
                        pass.set_vertex_buffer(0, c.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            c.transp_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..c.transp_index_count, 0, 0..1);
                    }
                }
                // Selection / hover highlight: tinted copies of the picked
                // solids, drawn last with the Always-depth highlight pipeline so
                // the tint shows on top even when the solid is behind others.
                if !self.gpu_mesh_highlight.is_empty() {
                    pass.set_pipeline(&self.mesh_highlight_pipeline);
                    for g in &self.gpu_mesh_highlight {
                        if g.index_count == 0 {
                            continue;
                        }
                        pass.set_vertex_buffer(0, g.vertex_buffer.slice(..));
                        pass.set_index_buffer(g.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..g.index_count, 0, 0..1);
                    }
                }
                // *WithEdges variants: overlay edge segments on top of the shaded
                // fill in black (mesh_edge_black_pipeline). The LessEqual depth
                // test keeps the edges visible over the fragments the fill wrote.
                if want_solid_with_edges {
                    pass.set_pipeline(&self.mesh_edge_black_pipeline);
                    for c in &self.gpu_mesh_batch {
                        if c.wire_index_count != 0 {
                            pass.set_vertex_buffer(0, c.vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                c.wire_index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.draw_indexed(0..c.wire_index_count, 0, 0..1);
                        }
                        if c.edge_vertex_count != 0 {
                            pass.set_vertex_buffer(0, c.edge_vertex_buffer.slice(..));
                            pass.draw(0..c.edge_vertex_count, 0..1);
                        }
                    }
                    // DISPSILH silhouettes over the shaded fill.
                    if let Some(ref vb) = self.silhouette_vbuf {
                        pass.set_vertex_buffer(0, vb.slice(..));
                        pass.draw(0..self.silhouette_vcount, 0..1);
                    }
                }
            }
        }

        // ── Pass 5a: 3DFACE fills (3D + 2D split) ─────────────────────────
        // 3D quads + PolyfaceMesh face tris go through the depth-only
        // pipeline in HiddenLine so wires hidden behind them disappear.
        // 2D fills (text greek, MultiLeader bg) always draw with colour.
        if let Some(ref fill) = self.gpu_face3d_fill {
            if !fill.chunks_3d.is_empty() || !fill.chunks_2d.is_empty() {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("face3d.render_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: msaa,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_stencil_reference(stencil_ref);
                if !fill.chunks_3d.is_empty() {
                    if hidden_line {
                        pass.set_pipeline(&self.face3d_depth_pipeline);
                    } else {
                        pass.set_pipeline(&self.face3d_pipeline);
                    }
                    for c in &fill.chunks_3d {
                        pass.set_vertex_buffer(0, c.vertex_buffer.slice(..));
                        pass.draw(0..c.vertex_count, 0..1);
                    }
                }
                if !fill.chunks_2d.is_empty() {
                    pass.set_pipeline(&self.face3d_pipeline);
                    for c in &fill.chunks_2d {
                        pass.set_vertex_buffer(0, c.vertex_buffer.slice(..));
                        pass.draw(0..c.vertex_count, 0..1);
                    }
                }
            }
        }

        // ── Pass 5b: 3DFACE edges (batched, possibly multiple chunks) ────
        // FlatShaded / GouraudShaded hide the 3DFACE outline (the user
        // chose a clean shaded look); every other mode keeps it.
        if show_3d_edges && !self.gpu_face3d_edges.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("face3d_edges.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
            pass.set_pipeline(&self.wire_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_stencil_reference(stencil_ref);
            for edges in &self.gpu_face3d_edges {
                if edges.instance_count > 0 {
                    if let Some(bg) = &edges.const_bind_group {
                        pass.set_bind_group(1, bg.as_ref(), &[]);
                    }
                    pass.set_vertex_buffer(0, edges.instance_buffer.slice(..));
                    pass.draw(0..6, 0..edges.instance_count);
                }
            }
        }

        // ── Pass 5: wires ─────────────────────────────────────────────────
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wire.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
            pass.set_pipeline(&self.wire_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            // In a filled-with-edges mode the mesh outline edges frame the shaded
            // fill and should read black; wireframe / hidden-line keep the entity
            // colour. `wire_black_pipeline` shares the wire layout, so the switch
            // needs no bind-group rebind.
            let want_solid_with_edges = !hidden_line && !mesh_wireframe && show_3d_edges;
            let mut black_active = false;
            pass.set_stencil_reference(stencil_ref);
            for wire in self.gpu_wires.iter() {
                if wire.instance_count == 0 {
                    continue;
                }
                // PolyfaceMesh / PolygonMesh outline edges live in
                // `gpu_wires` (their `WireModel` has both `points` and
                // `fill_tris`). In FlatShaded / GouraudShaded the user
                // wants a clean shaded surface, so the wire pass skips
                // these instances; the *WithEdges and pure wireframe
                // modes leave the flag at true and draw them.
                if !show_3d_edges && wire.is_3d_mesh_edge {
                    continue;
                }
                let use_black = want_solid_with_edges && wire.is_3d_mesh_edge;

                if use_black != black_active {
                    pass.set_pipeline(if use_black {
                        &self.wire_black_pipeline
                    } else {
                        &self.wire_pipeline
                    });
                    black_active = use_black;
                }
                if let Some(bg) = &wire.const_bind_group {
                    pass.set_bind_group(1, bg.as_ref(), &[]);
                }
                pass.set_vertex_buffer(0, wire.instance_buffer.slice(..));
                pass.draw(0..6, 0..wire.instance_count);
            }
            // Live overlay wires (command preview / interim / grip drag) always
            // on top: the xray pipeline (depth_compare=Always, no depth write)
            // keeps them visible through any occluding geometry — a 3D solid, or
            // 2D geometry drawn in front — so a command preview is never hidden.
            // No scissor.
            if self.gpu_preview_wires.iter().any(|pw| pw.instance_count > 0) {
                pass.set_pipeline(&self.wire_xray_pipeline);
                for pw in &self.gpu_preview_wires {
                    if pw.instance_count > 0 {
                        if let Some(bg) = &pw.const_bind_group {
                            pass.set_bind_group(1, bg.as_ref(), &[]);
                        }
                        pass.set_vertex_buffer(0, pw.instance_buffer.slice(..));
                        pass.draw(0..6, 0..pw.instance_count);
                    }
                }
            }
        }

        // ── Pass 5c: SDF text quads (drawn over wires) ────────────────────
        // The highlight buffer (selected / hovered text, tinted) is drawn in
        // the same pass right after the base text so it recolours those glyphs.
        if let Some(atlas) = &self.text_atlas_gpu {
            let have_base = self.text_vbuf.is_some() && self.text_vcount > 0;
            let have_hl = self.text_highlight_vbuf.is_some() && self.text_highlight_vcount > 0;
            let have_preview =
                self.text_preview_vbuf.is_some() && self.text_preview_vcount > 0;
            if have_base || have_hl || have_preview {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("text.render_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: msaa,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
                pass.set_pipeline(&self.text_pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_stencil_reference(stencil_ref);
                pass.set_bind_group(1, &atlas.bind_group, &[]);
                if let Some(vbuf) = &self.text_vbuf {
                    if self.text_vcount > 0 {
                        pass.set_vertex_buffer(0, vbuf.slice(..));
                        pass.draw(0..self.text_vcount, 0..1);
                    }
                }
                if let Some(hlbuf) = &self.text_highlight_vbuf {
                    if self.text_highlight_vcount > 0 {
                        pass.set_vertex_buffer(0, hlbuf.slice(..));
                        pass.draw(0..self.text_highlight_vcount, 0..1);
                    }
                }
                // Grip-drag / command-preview glyphs, drawn over the base text.
                if let Some(pbuf) = &self.text_preview_vbuf {
                    if self.text_preview_vcount > 0 {
                        pass.set_vertex_buffer(0, pbuf.slice(..));
                        pass.draw(0..self.text_preview_vcount, 0..1);
                    }
                }
            }
        }

        // ── Pass 6: wipeout fills (drawn after wires to mask them) ────────
        if !self.gpu_wipeouts.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wipeout.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
            pass.set_pipeline(&self.wipeout_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_stencil_reference(stencil_ref);
            for (i, wipeout) in self.gpu_wipeouts.iter().enumerate() {
                if self.wipeout_skip_flags.get(i).copied().unwrap_or(false) {
                    continue;
                }
                pass.set_bind_group(1, &wipeout.bind_group, &[]);
                pass.set_vertex_buffer(0, wipeout.vertex_buffer.slice(..));
                pass.draw(0..6, 0..1);
            }
        }

        // ── Pass 7: selected wire overlay pass ───────────────────────────
        // Redraws selected wires with depth_compare=Always so they appear on
        // top of all other geometry at full brightness.
        if !self.gpu_selected_wires.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wire_xray.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(0.0, 0.0, vp.width as f32, vp.height as f32, 0.0, 1.0);
            pass.set_pipeline(&self.wire_xray_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_stencil_reference(stencil_ref);
            for wire in &self.gpu_selected_wires {
                if wire.instance_count > 0 {
                    if let Some(bg) = &wire.const_bind_group {
                        pass.set_bind_group(1, bg.as_ref(), &[]);
                    }
                    pass.set_vertex_buffer(0, wire.instance_buffer.slice(..));
                    pass.draw(0..6, 0..wire.instance_count);
                }
            }
        }

        // ── Resolve MSAA → resolve texture ────────────────────────────────
        // Both are the same (rounded `alloc_size`) offscreen texture, so the
        // resolve never touches the surface. Only the drawn [0, render_size]
        // corner holds content; the rounded border stays the cleared bg color
        // and is never sampled (the blit UV is scaled to render/alloc).
        {
            let _resolve = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("msaa.resolve_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa,
                    depth_slice: None,
                    resolve_target: Some(&self.resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // No draw calls — the pass itself triggers the MSAA resolve.
        }
        } // end `if !self.skip_geometry`

        // ── Blit resolve texture → surface target at surface_dest position ──
        // The viewport maps the NDC quad to exactly `surface_dest` in the
        // swap-chain; `uv_offset` + `uv_scale` (passed through the blit
        // uniform) crop the resolve so we sample only the visible portion
        // of the full viewport's MSAA texture.
        if surface_dest.width > 0 && surface_dest.height > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(
                surface_dest.x as f32,
                surface_dest.y as f32,
                surface_dest.width as f32,
                surface_dest.height as f32,
                0.0,
                1.0,
            );
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &self.blit_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    pub fn ensure_depth_texture(&mut self, device: &wgpu::Device, size: Size<u32>) {
        // Record the requested render size every frame (the blit UV scale reads
        // it); only reallocate the textures when the *rounded* size changes.
        self.depth_texture_size = size;
        let alloc = Size::new(round_up_tex(size.width), round_up_tex(size.height));
        if self.alloc_size != alloc {
            let size = alloc;
            let depth_tex = create_depth_texture(device, size);
            self.depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
            let msaa_tex = create_msaa_texture(device, size, self.surface_format);
            self.msaa_view = msaa_tex.create_view(&wgpu::TextureViewDescriptor::default());
            let resolve_tex = create_resolve_texture(device, size, self.surface_format);
            let resolve_view = resolve_tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blit.bind_group"),
                layout: &self.blit_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&resolve_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.blit_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.blit_uniform_buffer.as_entire_binding(),
                    },
                ],
            });
            self.resolve_view = resolve_view;
            self.alloc_size = alloc;
        }
    }
}

/// Round a texture dimension up to a coarse grid so a live divider drag
/// doesn't recreate the depth / MSAA / resolve textures on every frame. See
/// `alloc_size` — this is the mitigation for the Windows-Firefox freeze (#191).
fn round_up_tex(n: u32) -> u32 {
    const GRID: u32 = 128;
    ((n.max(1) + GRID - 1) / GRID) * GRID
}

/// Project a mesh's `world_aabb` and pick a LOD slot:
///   diagonal > 200 px → 0 (HIGH)
///   diagonal 50–200 px → 1 (MID)
///   diagonal < 50 px   → 2 (LOW)
/// When the picked slot is missing from `gpu_meshes[i].lods`, walks down
/// to the next available coarser/finer level so the mesh always renders.
fn pick_mesh_lod(
    mesh: &MeshLodGpu,
    view_rot: glam::Mat4,
    eye: glam::DVec3,
    clip_w: u32,
    clip_h: u32,
) -> usize {
    let diag_px = aabb_diagonal_pixels(mesh.world_aabb, view_rot, eye, clip_w, clip_h);
    let target = if diag_px > 200.0 {
        0
    } else if diag_px > 50.0 {
        1
    } else {
        2
    };
    // Walk down to nearest available LOD (some entities won't have all 3).
    for level in (0..=target).rev() {
        if mesh.lods.get(level).is_some() {
            return level;
        }
    }
    0
}

fn aabb_diagonal_pixels(
    aabb: [f32; 4],
    view_rot: glam::Mat4,
    eye: glam::DVec3,
    clip_w: u32,
    clip_h: u32,
) -> f32 {
    let [x0, y0, x1, y1] = aabb;
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return f32::INFINITY;
    }
    let w = clip_w as f32;
    let h = clip_h as f32;
    let corners = [
        view_rot.project_point3((glam::DVec3::new(x0 as f64, y0 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x1 as f64, y0 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x0 as f64, y1 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x1 as f64, y1 as f64, 0.0) - eye).as_vec3()),
    ];
    let mut min_px = f32::INFINITY;
    let mut max_px = f32::NEG_INFINITY;
    let mut min_py = f32::INFINITY;
    let mut max_py = f32::NEG_INFINITY;
    for c in &corners {
        let px = (c.x + 1.0) * 0.5 * w;
        let py = (1.0 - c.y) * 0.5 * h;
        if px < min_px { min_px = px; }
        if px > max_px { max_px = px; }
        if py < min_py { min_py = py; }
        if py > max_py { max_py = py; }
    }
    let dx = max_px - min_px;
    let dy = max_py - min_py;
    (dx * dx + dy * dy).sqrt()
}

/// `true` when the world-XY AABB projects entirely outside the
/// viewport rect (extended by `MARGIN_FRAC` to absorb pan inertia and
/// avoid edge pop-in). Phase 2.2 mesh-frustum / Phase 2.3 hatch +
/// wipeout cull. Equivalent to a 2D bounding-box rejection test in
/// NDC; uses the same 4-corner projection that LOD picking already
/// does, so the extra cost is negligible.
///
/// IMPORTANT: the AABB must be in the same local space (world_offset
/// subtracted) that `view_proj` expects. `WipeoutGpu.world_aabb` rebuilds
/// the absolute local-space rect from `model.world_origin + boundary
/// extents` for this reason; meshes already store an absolute rect.
fn aabb_offscreen(
    aabb: [f32; 4],
    view_rot: glam::Mat4,
    eye: glam::DVec3,
    clip_w: u32,
    clip_h: u32,
) -> bool {
    let [x0, y0, x1, y1] = aabb;
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return false;
    }
    let w = clip_w as f32;
    let h = clip_h as f32;
    let corners = [
        view_rot.project_point3((glam::DVec3::new(x0 as f64, y0 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x1 as f64, y0 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x0 as f64, y1 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x1 as f64, y1 as f64, 0.0) - eye).as_vec3()),
    ];
    let mut min_px = f32::INFINITY;
    let mut max_px = f32::NEG_INFINITY;
    let mut min_py = f32::INFINITY;
    let mut max_py = f32::NEG_INFINITY;
    for c in &corners {
        let px = (c.x + 1.0) * 0.5 * w;
        let py = (1.0 - c.y) * 0.5 * h;
        if px < min_px { min_px = px; }
        if px > max_px { max_px = px; }
        if py < min_py { min_py = py; }
        if py > max_py { max_py = py; }
    }
    // 25% pad on each side — matches `view_world_aabb` (wire path),
    // keeps edge geometry rendered while panning before the next
    // upload reaches the GPU.
    const MARGIN_FRAC: f32 = 0.25;
    let mx = w * MARGIN_FRAC;
    let my = h * MARGIN_FRAC;
    max_px < -mx || min_px > w + mx || max_py < -my || min_py > h + my
}

/// Return `true` when the world-XY AABB's screen-space size is below the
/// given pixel threshold. Used by LOD passes (hatch skip, etc.) to drop
/// draw calls that wouldn't contribute a visible pixel.
fn aabb_below_pixel(
    aabb: [f32; 4],
    view_rot: glam::Mat4,
    eye: glam::DVec3,
    clip_w: u32,
    clip_h: u32,
    threshold_px: f32,
) -> bool {
    let [x0, y0, x1, y1] = aabb;
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return false;
    }
    let w = clip_w as f32;
    let h = clip_h as f32;
    let corners = [
        view_rot.project_point3((glam::DVec3::new(x0 as f64, y0 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x1 as f64, y0 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x0 as f64, y1 as f64, 0.0) - eye).as_vec3()),
        view_rot.project_point3((glam::DVec3::new(x1 as f64, y1 as f64, 0.0) - eye).as_vec3()),
    ];
    let mut min_px = f32::INFINITY;
    let mut max_px = f32::NEG_INFINITY;
    let mut min_py = f32::INFINITY;
    let mut max_py = f32::NEG_INFINITY;
    for c in &corners {
        let px = (c.x + 1.0) * 0.5 * w;
        let py = (1.0 - c.y) * 0.5 * h;
        if px < min_px { min_px = px; }
        if px > max_px { max_px = px; }
        if py < min_py { min_py = py; }
        if py > max_py { max_py = py; }
    }
    (max_px - min_px).max(max_py - min_py) < threshold_px
}

fn create_depth_texture(device: &wgpu::Device, size: Size<u32>) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viewer.depth_texture"),
        size: wgpu::Extent3d {
            width: size.width.max(1),
            height: size.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: MSAA_SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth24PlusStencil8,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn create_resolve_texture(
    device: &wgpu::Device,
    size: Size<u32>,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viewer.resolve_texture"),
        size: wgpu::Extent3d {
            width: size.width.max(1),
            height: size.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn create_msaa_texture(
    device: &wgpu::Device,
    size: Size<u32>,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viewer.msaa_texture"),
        size: wgpu::Extent3d {
            width: size.width.max(1),
            height: size.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: MSAA_SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// Holds one inner `Pipeline` per viewport drawn this frame. A single
/// shader widget owns one `MultiPipeline`; the unified renderer grows the
/// `inners` vector to match the viewport count and draws each into its own
/// screen rectangle. Inner `Pipeline` code (upload / LOD / render / blit)
/// is unchanged — it just runs once per viewport.
pub struct MultiPipeline {
    pub(crate) inners: Vec<Pipeline>,
    format: wgpu::TextureFormat,
    /// The resident wire batches, keyed by `wire_content_id` and shared across
    /// every slot (and every pane — one `MultiPipeline` backs all of them) that
    /// renders the same content. `prepare` builds an entry once on a cache miss
    /// then hands `Arc` clones to each slot, so N paper viewports / Model tiles
    /// showing one identical resident set upload the wire vertex buffers exactly
    /// once between them instead of once per slot. Kept trim by dropping entries
    /// no slot still references (`Arc::strong_count == 1`) once it grows past a
    /// small bound.
    pub(crate) wire_buffer_cache: rustc_hash::FxHashMap<
        u64,
        (
            std::sync::Arc<Vec<WireGpu>>,
            std::sync::Arc<rustc_hash::FxHashMap<u64, Vec<u32>>>,
        ),
    >,
}

impl MultiPipeline {
    /// Ensure at least `n` (≥1) inner pipelines exist, creating any missing
    /// ones. Grow-only: extra pipelines are NOT dropped, because per-pane Model
    /// shader widgets share this storage and each only touches its own slot —
    /// truncating mid-frame would destroy another pane's slot. Stale inners (a
    /// closed pane, or fewer paper viewports) are harmless idle GPU resources.
    pub(crate) fn ensure_len(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        n: usize,
    ) {
        let n = n.max(1);
        while self.inners.len() < n {
            self.inners.push(Pipeline::new(device, queue, self.format));
        }
    }
}

/// Send wgpu's uncaptured validation errors to stderr instead of the default
/// handler, which panics — and inside iced's main-thread redraw that panic
/// aborts the process, taking the user's unsaved work with it (#358). A bad
/// frame must degrade, not end the session.
///
/// The handler slot is per-device and single: this also catches iced's own draw
/// errors, and nothing else can report them, so it must stay loud enough to be
/// noticed. A validation error normally repeats on every frame, though, so a
/// plain print would flood stderr at refresh rate and bury the first (most
/// useful) message. Log the 1st, 2nd, 4th, 8th … occurrence instead: the error
/// is never hidden, the count shows it is ongoing, and the output stays finite.
///
/// Installed from `MultiPipeline::new`, which runs once per device — NOT from
/// `Pipeline::new`, which runs again for every pane `ensure_len` adds. If iced
/// ever rebuilds the device it builds a new `MultiPipeline` too, so the fresh
/// device is covered (and the throttle restarts with it).
fn install_gpu_error_handler(device: &wgpu::Device) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEEN: AtomicUsize = AtomicUsize::new(0);
    SEEN.store(0, Ordering::Relaxed);
    device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| {
        let n = SEEN.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            eprintln!("[gpu] uncaptured wgpu error #{n} (frame degraded, session kept alive): {e}");
        }
    }));
}

impl iced::widget::shader::Pipeline for MultiPipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        install_gpu_error_handler(device);
        Self {
            inners: vec![Pipeline::new(device, queue, format)],
            format,
            wire_buffer_cache: rustc_hash::FxHashMap::default(),
        }
    }
}

/// The two silhouette angles of a cone/cylinder face for a view direction,
/// expressed in the face's `(u, v, axis)` frame via the view's components on
/// each: `du = view·u`, `dv = view·v`, `da = view·axis`. `tan_a` is the cone
/// taper (0 for a cylinder).
///
/// The outward normal is edge-on to the view where `du·cosθ + dv·sinθ =
/// -tanα·da`, i.e. `θ = φ ± acos(-tanα·da / |view⊥|)` with `φ = atan2(dv, du)`.
/// `None` when the view runs down the axis (no outline) or the whole cone faces
/// toward/away (`|arg| > 1`).
fn silhouette_thetas(du: f64, dv: f64, da: f64, tan_a: f64) -> Option<(f64, f64)> {
    let r_perp = (du * du + dv * dv).sqrt();
    if r_perp < 1e-6 {
        return None;
    }
    let arg = -tan_a * da / r_perp;
    if arg.abs() > 1.0 {
        return None;
    }
    let phi = dv.atan2(du);
    let delta = arg.acos();
    Some((phi + delta, phi - delta))
}
