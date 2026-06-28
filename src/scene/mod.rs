// Scene modules grouped by role:
//   convert — DXF/ACIS entities → truck solids & tessellated geometry
//   text    — LFF stroke + TrueType font engines and shaping
//   model   — per-entity GPU render models (wire, hatch, mesh, image, object)
//   pick    — hit-testing, selection, grips, spatial index, xclip
//   view    — camera, transforms, viewport, render pipeline driver
//   cache   — block-definition and property caches
pub mod cache;
pub mod convert;
pub mod model;
pub mod pick;
pub mod pipeline;
pub mod text;
pub mod view;

// Topic submodules split out of this root (each contributes `impl Scene`
// blocks and/or free functions). Pure text-move from the original mod.rs.
mod entity;
mod group_layer;
mod camera_ops;
mod layout;
mod modify;
mod mspace;
mod paper;
mod preview;
mod project;
mod selection;

// Parallel tessellation free functions live in `convert::tess` (alongside the
// other tessellation code); re-exported here so this root and sibling topic
// modules (each does `use super::*`) keep referencing them unqualified.
pub(crate) use convert::tess::{
    entity_aabb, entity_world_aabb_f64, is_unindexable_entity, tessellate_entity,
    tessellate_entity_dim_text,
};

/// Result of `Scene::entity_index()`. The wire path queries `tree` for
/// view-rect candidates and also always emits `unbounded_handles`
/// (entities with no usable bbox — legacy `UNBOUNDED_AABB` sentinel).
pub(super) struct EntityIndex {
    pub tree: pick::quadtree::QuadTree,
    pub unbounded_handles: Vec<Handle>,
}

use view::camera::Camera;
pub use view::camera::Projection;
pub use model::hatch_model::HatchModel;
pub use model::image_model::ImageModel;
pub use model::mesh_model::MeshLodSet;
pub use model::object::{GripApply, GripDef};
pub use pipeline::uniforms::Uniforms;
pub use pipeline::viewcube::{
    hit_test, hit_test_cardinal, hover_id, CubeRegion, NudgeDir, VIEWCUBE_DRAW_PX, VIEWCUBE_PAD,
    VIEWCUBE_PX, VIEWCUBE_REGION_PX,
};
pub use pick::selection_state::SelectionState;
pub use model::wire_model::WireModel;

use crate::command::EntityTransform;
use acadrust::entities::{Block, BlockEnd, Insert as DxfInsert};
use acadrust::entities::{
    BoundaryEdge, BoundaryPath, Hatch as DxfHatch, PolylineEdge, Solid as DxfSolid,
};
use acadrust::objects::ObjectType;
use acadrust::types::Vector2;
use acadrust::{CadDocument, EntityType, Handle, TableEntry};
use glam;
use truck_modeling::{
    base::{BoundedCurve, ParameterDivision1D},
    BSplineCurve as TruckBSpline, KnotVec, NurbsCurve, Point3, Vector4,
};

use iced::time::Duration;
use std::cell::RefCell;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Global counter so every Scene and every geometry mutation gets a
/// process-wide unique epoch. This prevents two different tabs (Scenes)
/// from ever sharing the same epoch value, which would cause the shared
/// GPU Pipeline to skip re-uploading geometry when switching tabs.
static GEOMETRY_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Process-wide monotonic id stamped each time the Model wire set is built.
/// The set is held static across camera moves, so the id stays the same every
/// frame until the geometry epoch changes — it uniquely identifies a wire
/// buffer's *content* across frames. The GPU pipeline gates wire re-upload on
/// it: an unchanged id means the world-space wire buffer is not re-sent.
/// Monotonic (never reused) → free of the ABA hazard a raw `Arc` pointer would
/// carry when an address is freed and reallocated.
static WIRE_CONTENT_GEN: AtomicU64 = AtomicU64::new(1);

/// Resolve a viewport's paper-to-model scale ratio from its two
/// DXF-derived sources.
///
/// `view_height` (model-space view extent) is the canonical source — it
/// is what AutoCAD actually uses to draw, and what we keep in sync on
/// every write. `custom_scale` is consulted only when `view_height` is
/// missing or zero (some third-party exporters omit it).
#[inline]
pub fn vp_effective_scale(custom_scale: f64, view_height: f64, vp_height: f64) -> f64 {
    if view_height.abs() > 1e-9 {
        return vp_height / view_height;
    }
    if custom_scale.abs() > 1e-9 {
        return custom_scale;
    }
    1.0
}

/// Pre-built entity caches returned by [`build_derived_caches`].
/// Produced in the file-load background task so the UI thread only assigns.
#[derive(Debug, Clone)]
pub struct DerivedCaches {
    pub local_extent_max: f32,
    pub local_center: [f64; 2],
    pub hatches: HashMap<Handle, HatchModel>,
    pub images: HashMap<Handle, ImageModel>,
    pub meshes: HashMap<Handle, MeshLodSet>,
    /// Block-definition solid meshes, block-local frame (instanced per INSERT). (#123)
    pub block_meshes: HashMap<Handle, MeshLodSet>,
    /// Number of entities removed by the corrupt-entity guard during load.
    /// Reported back to the UI so the user knows when a file had parser-junk
    /// entities silently dropped.
    pub corrupt_dropped: usize,
    /// Number of corrupt entities dropped while merging external references.
    /// Carried back from the background xref resolution pass so the UI can
    /// surface the warning without re-scanning the document.
    pub xref_dropped: usize,
    /// XREF resolution status returned by the background thread. The UI uses
    /// this to print the loaded/not-found/unloaded list without re-scanning
    /// the block table.
    pub xref_infos: Vec<crate::io::xref::XrefInfo>,
    /// Background-thread open-phase timings in milliseconds (parse, purge,
    /// derived-cache build, xref resolution). Filled in by `open_path_with_phase`;
    /// surfaced in the open-complete breakdown log so open-time regressions
    /// are visible.
    pub timings: OpenTimings,
}

/// Wall-clock breakdown of the file-open phases, in milliseconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenTimings {
    pub parse_ms: u32,
    pub purge_ms: u32,
    pub caches_ms: u32,
    /// Time spent resolving external references on the background thread.
    pub xref_ms: u32,
}

/// Build hatch / image / mesh caches from a document without needing `&mut Scene`.
/// Intended to run on a background thread during file load.
pub fn build_derived_caches(doc: &CadDocument) -> DerivedCaches {
    // model-space block handle (same logic as Scene::model_space_block_handle)
    let model_block = doc
        .objects
        .values()
        .find_map(|obj| {
            if let acadrust::objects::ObjectType::Layout(l) = obj {
                if l.name == "Model" && !l.block_record.is_null() {
                    Some(l.block_record)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            doc.block_records
                .get("*Model_Space")
                .map(|br| br.handle)
                .unwrap_or(Handle::NULL)
        });

    // world_offset selection
    //
    // Header `$EXTMIN`/`$EXTMAX` is the fast path, but it's untrustworthy:
    // the sentinel (1e20 / -1e20) when the writer never computed extents,
    // stale values when a drawing was edited and extents weren't refreshed,
    // and top-level extents that span only an Insert's bounding box rather
    // than the actual MSPACE geometry. Any of those
    // leave the precision-preserving offset wrong, so direct MSPACE
    // entities render at huge magnitudes and f32 wires lose precision.
    //
    // Cross-check the header against a per-entity AABB scan of MSPACE
    // (same `bounding_box()` API and same SANE_EXTENT/zero-placeholder
    // filters that `cache::block_cache::build_defn` already uses for block defns)
    // and prefer the entity-scan when the header center drifts more than
    // 10× its own half-span away from the entity centroid.
    use crate::par::prelude::*;

    // Single pass over entities does triple duty: classify cache-kind handle
    // lists (hatch / image / mesh) AND accumulate per-entity centroids for the
    // world_offset median. Folding the offset scan in here collapses what were
    // two O(N) `entities()` walks (offset scan + handle collection) into one.
    // Heavy tessellation runs in parallel below, reading entities via
    // `doc.get_entity(h)` (O(1) HashMap lookup); no clones in this pass.
    let prep = offset_prep(doc, model_block);
    let mut hatch_handles: Vec<Handle> = Vec::new();
    let mut image_handles: Vec<Handle> = Vec::new();
    let mut mesh_handles: Vec<Handle> = Vec::new();
    let mut centers: Vec<[f64; 3]> = Vec::new();
    for e in doc.entities() {
        let h = e.common().handle;
        match e {
            EntityType::Hatch(_) | EntityType::Solid(_) => hatch_handles.push(h),
            EntityType::RasterImage(_) => image_handles.push(h),
            EntityType::Solid3D(_) | EntityType::Region(_) | EntityType::Body(_) | EntityType::Surface(_) => {
                mesh_handles.push(h)
            }
            _ => {}
        }
        if let Some(c) = offset_centroid(e, model_block, &prep) {
            centers.push(c);
        }
    }
    let (local_center, local_extent_max) = cluster_extent_from_centers(centers, &doc.header);

    // Default bg adaptation target at load: the model background (paper
    // bg is only relevant after the user enters a paper layout, and
    // `synced_hatch_models` re-runs `render_style` per-frame anyway so
    // the per-layout adaptation kicks in later regardless).
    const LOAD_BG: [f32; 4] = [0.11, 0.11, 0.11, 1.0];

    // hatches
    let hatches: HashMap<Handle, HatchModel> = hatch_handles
        .par_iter()
        .filter_map(|&handle| {
            let e = doc.get_entity(handle)?;
            let (raw, ..) = view::render::render_style_for(doc, e);
            let color = view::render::adapt_to_bg(raw, LOAD_BG);
            let model = match e {
                EntityType::Hatch(dxf) => Scene::hatch_model_from_dxf(dxf, color),
                EntityType::Solid(solid) => Some(Scene::solid_hatch_model(solid, color)),
                _ => None,
            };
            model.map(|m| (handle, m))
        })
        .collect();

    // images
    let images: HashMap<Handle, ImageModel> = image_handles
        .par_iter()
        .filter_map(|&handle| {
            if let EntityType::RasterImage(img) = doc.get_entity(handle)? {
                ImageModel::from_raster_image(img).map(|m| (handle, m))
            } else {
                None
            }
        })
        .collect();

    // meshes (parallel tessellation). FACETRES (header.facet_resolution)
    // scales the per-LOD segment counts so users with finer drawings get
    // smoother solids; clamped to AutoCAD's [0.01, 10.0] range inside.
    // Top-level (layout-owned) solids are offset into the render frame; block
    // definition solids keep block-local coords for per-INSERT instancing. (#123)
    let facet_res = doc.header.facet_resolution;
    // Real layout blocks come from the Layout objects' block_record handles —
    // `BlockRecord::is_layout()` is unreliable here (it flags ordinary blocks).
    let layout_blocks: std::collections::HashSet<Handle> = doc
        .objects
        .values()
        .filter_map(|o| match o {
            acadrust::objects::ObjectType::Layout(l) if !l.block_record.is_null() => {
                Some(l.block_record)
            }
            _ => None,
        })
        .collect();
    let built: Vec<(Handle, MeshLodSet, bool)> = mesh_handles
        .par_iter()
        .filter_map(|&handle| {
            let e = doc.get_entity(handle)?;
            let (raw, ..) = view::render::render_style_for(doc, e);
            let color = view::render::adapt_to_bg(raw, LOAD_BG);
            let top_level = layout_blocks.contains(&e.common().owner_handle);
            crate::entities::solid3d::tessellate_volume(e, color, facet_res).map(|m| {
                let m = if top_level { offset_mesh_lod_set(m) } else { m };
                (handle, m, top_level)
            })
        })
        .collect();
    let mut meshes: HashMap<Handle, MeshLodSet> = HashMap::default();
    let mut block_meshes: HashMap<Handle, MeshLodSet> = HashMap::default();
    for (handle, m, top_level) in built {
        if top_level {
            meshes.insert(handle, m);
        } else {
            block_meshes.insert(handle, m);
        }
    }

    DerivedCaches {
        local_extent_max,
        local_center,
        hatches,
        images,
        meshes,
        block_meshes,
        corrupt_dropped: 0,
        xref_dropped: 0,
        xref_infos: Vec::new(),
        timings: OpenTimings::default(),
    }
}

/// Mirrors `cache::block_cache::SANE_EXTENT` — wire coords past this magnitude
/// are treated as corruption rather than precision-relevant geometry.
const CLUSTER_SANE_EXTENT: f64 = 1.0e8;

/// MSPACE-membership prep shared by the world-offset centroid scan.
///
/// The filter here MUST agree with `belongs_to_visible_block` (the
/// render-time filter): if rendering treats an entity as MSPACE but we skip
/// it here, our offset misses on-screen geometry and direct WCS-coordinate
/// wires drag f32 precision to its knees. Conversely, including block-defn
/// entities the render path drops would pull the centroid toward block-local
/// origins.
struct OffsetPrep {
    /// `Some` when the model BlockRecord enumerates its entities; the offset
    /// scan uses this set directly. `None` falls back to the legacy
    /// permissive owner-based interpretation.
    mspace_set: Option<rustc_hash::FxHashSet<Handle>>,
    any_enumerated: bool,
    owned_by_other_block: rustc_hash::FxHashSet<Handle>,
}

fn offset_prep(doc: &acadrust::CadDocument, model_block: Handle) -> OffsetPrep {
    let model_br = doc
        .block_records
        .iter()
        .find(|br| br.handle == model_block);
    let mspace_set: Option<rustc_hash::FxHashSet<Handle>> = model_br
        .filter(|br| !br.entity_handles.is_empty())
        .map(|br| br.entity_handles.iter().copied().collect());
    let any_enumerated = doc
        .block_records
        .iter()
        .any(|br| !br.entity_handles.is_empty());
    let owned_by_other_block: rustc_hash::FxHashSet<Handle> = if mspace_set.is_none() {
        doc.block_records
            .iter()
            .filter(|br| br.handle != model_block)
            .flat_map(|br| br.entity_handles.iter().copied())
            .collect()
    } else {
        rustc_hash::FxHashSet::default()
    };
    OffsetPrep { mspace_set, any_enumerated, owned_by_other_block }
}

/// Per-entity centroid for the world-offset scan, or `None` if the entity is
/// not MSPACE geometry / has no usable bbox. Single-outlier-robust because
/// the caller takes the median of these per-entity centroids rather than a
/// global min/max midpoint.
fn offset_centroid(
    e: &EntityType,
    model_block: Handle,
    prep: &OffsetPrep,
) -> Option<[f64; 3]> {
    let c = e.common();
    let h = c.handle;
    let include = if let Some(ref set) = prep.mspace_set {
        set.contains(&h)
    } else if c.owner_handle == model_block {
        true
    } else if !c.owner_handle.is_null() {
        false
    } else if prep.owned_by_other_block.contains(&h) {
        false
    } else {
        // owner null + h not enumerated by any block: legacy permissive
        // when no block enumerated at all, strict drop otherwise (same
        // as belongs_to_visible_block).
        !prep.any_enumerated
    };
    if !include {
        return None;
    }
    // Skip block-defn sentinels and AttributeDefinition — same as
    // cache::block_cache::build_defn. Their bboxes don't represent drawable
    // MSPACE geometry.
    if matches!(
        e,
        EntityType::Block(_) | EntityType::BlockEnd(_) | EntityType::AttributeDefinition(_)
    ) {
        return None;
    }
    let (bmin, bmax) = match e {
        EntityType::Insert(ins) => (ins.insert_point, ins.insert_point),
        _ => {
            let bb = e.as_entity().bounding_box();
            (bb.min, bb.max)
        }
    };
    // Empty-entity placeholder (Polyline/Hatch/Spline/Mesh with no
    // vertices). Including these would pull the centroid toward origin
    // and destroy precision on UTM-authored content.
    if bmin.x == 0.0
        && bmin.y == 0.0
        && bmin.z == 0.0
        && bmax.x == 0.0
        && bmax.y == 0.0
        && bmax.z == 0.0
    {
        return None;
    }
    let cx = (bmin.x + bmax.x) * 0.5;
    let cy = (bmin.y + bmax.y) * 0.5;
    let cz = (bmin.z + bmax.z) * 0.5;
    if !cx.is_finite() || !cy.is_finite() || !cz.is_finite() {
        return None;
    }
    if cx.abs() > CLUSTER_SANE_EXTENT || cy.abs() > CLUSTER_SANE_EXTENT {
        return None;
    }
    Some([cx, cy, cz])
}

/// Pick the model-space precision-preserving offset and the `fit_all`
/// outlier-rejection limit from the collected per-entity `centers`.
///
/// Prefers the entity-centroid median; cross-checks against header
/// `$EXTMIN/$EXTMAX` only as a fallback when the entity scan found nothing.
/// `centers` is gathered by the caller's single entity walk (see
/// [`build_derived_caches`]) so no separate AABB pass is needed.
/// Returns `(center, half_span)` of the dense entity cluster. The center is the
/// median of entity centroids — robust against a second, far cluster (e.g. a
/// small-coordinate legend beside a UTM survey), unlike the raw extents centre
/// which would land in the empty gap between them.
fn cluster_extent_from_centers(
    centers: Vec<[f64; 3]>,
    header: &acadrust::document::HeaderVariables,
) -> ([f64; 2], f32) {
    const SANE_EXTENT: f64 = CLUSTER_SANE_EXTENT;
    let entity_ok = !centers.is_empty();

    // 95th-percentile distance from the median × 2 gives the half-span of the
    // dense cluster while leaving room for legitimate outliers (sparse leaders,
    // dimensions, scattered annotations).
    let median = |v: &mut Vec<f64>| -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v[v.len() / 2]
    };
    let percentile = |v: &mut Vec<f64>, frac: f64| -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let i = ((v.len() as f64 - 1.0) * frac).round() as usize;
        v[i]
    };
    let (ecenter, espan_max) = if entity_ok {
        let mut xs: Vec<f64> = centers.iter().map(|c| c[0]).collect();
        let mut ys: Vec<f64> = centers.iter().map(|c| c[1]).collect();
        let mx = median(&mut xs);
        let my = median(&mut ys);
        let mut dx: Vec<f64> = centers.iter().map(|c| (c[0] - mx).abs()).collect();
        let mut dy: Vec<f64> = centers.iter().map(|c| (c[1] - my).abs()).collect();
        let p95 = percentile(&mut dx, 0.95).max(percentile(&mut dy, 0.95));
        ([mx, my], (p95 * 2.0).max(1.0) as f32)
    } else {
        ([0.0, 0.0], 0.0)
    };

    // ── Header extents (fallback only) ───────────────────────────────────
    let hmin = header.model_space_extents_min;
    let hmax = header.model_space_extents_max;
    let header_ok = hmin.x < hmax.x
        && hmin.y < hmax.y
        && hmin.x.abs() < SANE_EXTENT
        && hmax.x.abs() < SANE_EXTENT
        && hmin.y.abs() < SANE_EXTENT
        && hmax.y.abs() < SANE_EXTENT;

    // Geometry reaches the GPU as absolute coordinates (the double-single
    // relative-to-eye path keeps it precise at UTM scale), so only the
    // cluster span — for camera fit and cull — is derived from the content.
    if entity_ok {
        (ecenter, espan_max)
    } else if header_ok {
        let hw = ((hmax.x - hmin.x) * 0.5) as f32;
        let hh = ((hmax.y - hmin.y) * 0.5) as f32;
        let hz = ((hmax.z - hmin.z) * 0.5).max(1.0) as f32;
        let hcenter = [(hmin.x + hmax.x) * 0.5, (hmin.y + hmax.y) * 0.5];
        (hcenter, hw.max(hh).max(hz) * 10.0)
    } else {
        ([0.0, 0.0], 1e9_f32)
    }
}

/// One viewport to render this frame — a camera, the screen rectangle it
/// occupies, and the render mode it draws with. The unified renderer
/// produces a `Vec<ViewportInstance>` for both layouts: a Model layout is
/// one full-canvas instance (or several tiled ones), a paper layout is one
/// instance per floating content viewport. The pipeline draws each in its
/// own scissor pass, so a single shader widget covers every case.
#[derive(Clone)]
pub struct ViewportInstance {
    /// Source viewport entity handle, or `Handle::NULL` for the implicit
    /// full-canvas Model view that has no backing entity yet.
    pub handle: Handle,
    /// Source Model-space tile index, or `None` for paper-layout viewports
    /// (they're identified by `handle` instead). Used as the cache key for
    /// `Scene::model_tile_wires_arc` so each pane reuses its own entry on
    /// camera moves instead of accumulating one per camera hash.
    pub tile_idx: Option<usize>,
    /// Screen rectangle (pixels, canvas-relative) this viewport fills.
    pub screen_rect: iced::Rectangle,
    pub camera: Camera,
    pub render_mode: acadrust::entities::ViewportRenderMode,
    /// `true` when this is the viewport receiving cursor input.
    pub active: bool,
    /// `true` when this view's grid is switched on — drives `grid_views`, so the
    /// grid overlay enumerates the exact same sub-views (tile or floating
    /// viewport) the renderer does, instead of a parallel copy.
    pub grid_on: bool,
    /// `true` for the full-canvas paper "sheet" viewport — the layout's own
    /// view (paper-space entities, top-locked), the paper equivalent of the
    /// Model view. Floating content viewports overlay it.
    pub paper_sheet: bool,
}

/// One pane of the Model-space tiled viewport layout: the normalized screen
/// rectangle it fills and the camera it last had. The active tile uses the
/// live `Scene::camera` (so orbit/pan/zoom drive it); inactive tiles keep a
/// snapshot here, swapped in when they become active.
#[derive(Clone)]
pub(crate) struct ModelTile {
    pub(crate) rect: iced::Rectangle,
    pub(crate) camera: Camera,
    /// Visual style for this tile alone — each pane carries its own so
    /// changing one tile's render mode never touches the others.
    pub(crate) render_mode: acadrust::entities::ViewportRenderMode,
    /// Grid display + grid-snap for this viewport alone, round-tripped through
    /// its VPort entry. The app mirrors the *active* tile's pair into the live
    /// grid/snap toggles. (#121)
    pub(crate) grid_on: bool,
    pub(crate) snap_on: bool,
}

/// Gap (pixels) between Model panes — the `pane_grid` spacing and the visible
/// divider width. The renderer derives tile rects through this same spacing so
/// the drawn viewports line up exactly with the pane_grid layout.
pub const TILE_DIVIDER_PX: f32 = 2.0;

/// Shift every vertex of a freshly tessellated `MeshLodSet` into the
/// scene's local f32 space by subtracting `world_offset`. ACIS / SAT
/// tessellation hands us WCS coordinates; the wire / hatch / face3d
/// paths run in `(WCS - world_offset)` so meshes at large UTM-scale
/// origins would otherwise float far away from the rest of the
/// geometry. Also recomputes `world_aabb` so per-frame LOD / cull math
/// uses the same space.
fn offset_mesh_lod_set(mut set: MeshLodSet) -> MeshLodSet {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for lod in &mut set.lods {
        // Reconstruct the f64 absolute position from the double-single pair,
        // subtract world_offset in f64, then re-split into (high, low) so the
        // relative-to-eye shader keeps sub-unit precision at UTM scale.
        let has_low = lod.verts_low.len() == lod.verts.len();
        if !has_low {
            lod.verts_low = vec![[0.0; 3]; lod.verts.len()];
        }
        for (v, vl) in lod.verts.iter_mut().zip(lod.verts_low.iter_mut()) {
            let ax = v[0] as f64 + vl[0] as f64;
            let ay = v[1] as f64 + vl[1] as f64;
            let az = v[2] as f64 + vl[2] as f64;
            let hx = ax as f32;
            let hy = ay as f32;
            let hz = az as f32;
            *v = [hx, hy, hz];
            *vl = [(ax - hx as f64) as f32, (ay - hy as f64) as f32, (az - hz as f64) as f32];
            if hx < min_x { min_x = hx; }
            if hy < min_y { min_y = hy; }
            if hx > max_x { max_x = hx; }
            if hy > max_y { max_y = hy; }
        }
    }
    if min_x.is_finite() {
        set.world_aabb = [min_x, min_y, max_x, max_y];
    }
    set
}

/// Instance a block-local mesh into the render frame: apply the accumulated
/// INSERT transform (block-local → world/DXF) then subtract world_offset, so a
/// block scaled at the INSERT renders at the right size. Normals are rotated by
/// the transform's linear part and re-normalized. (#123)
fn transform_block_mesh_lod_set(
    set: &MeshLodSet,
    xform: &acadrust::types::Transform,
) -> MeshLodSet {
    use acadrust::types::Vector3;
    let mut out = set.clone();
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for lod in &mut out.lods {
        let has_low = lod.verts_low.len() == lod.verts.len();
        if !has_low {
            lod.verts_low = vec![[0.0; 3]; lod.verts.len()];
        }
        for (v, vl) in lod.verts.iter_mut().zip(lod.verts_low.iter_mut()) {
            // Reconstruct the block-local f64, apply the INSERT transform and
            // subtract world_offset in f64, then re-split into (high, low).
            let w = xform.apply(Vector3::new(
                v[0] as f64 + vl[0] as f64,
                v[1] as f64 + vl[1] as f64,
                v[2] as f64 + vl[2] as f64,
            ));
            let ax = w.x;
            let ay = w.y;
            let az = w.z;
            let hx = ax as f32;
            let hy = ay as f32;
            let hz = az as f32;
            *v = [hx, hy, hz];
            *vl = [(ax - hx as f64) as f32, (ay - hy as f64) as f32, (az - hz as f64) as f32];
            if hx < min_x { min_x = hx; }
            if hy < min_y { min_y = hy; }
            if hx > max_x { max_x = hx; }
            if hy > max_y { max_y = hy; }
        }
        for n in &mut lod.normals {
            let d = xform.apply_rotation(Vector3::new(n[0] as f64, n[1] as f64, n[2] as f64));
            let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
            if len > 1e-12 {
                n[0] = (d.x / len) as f32;
                n[1] = (d.y / len) as f32;
                n[2] = (d.z / len) as f32;
            }
        }
    }
    if min_x.is_finite() {
        out.world_aabb = [min_x, min_y, max_x, max_y];
    }
    out
}

/// World-XY rectangle the model camera currently sees, expanded by `margin`
/// (1.0 = tight), as `[min_x, min_y, max_x, max_y]` for the entity R-tree cull.
///
/// The screen is a rectangle in the camera's right/up basis, not the world
/// axes — under a view twist or yaw that rectangle is rotated in world XY, so
/// projecting its four corners and taking their bounds gives the correct
/// enclosing box. A naive `target ± (w, h)` box (world-axis aligned) under-
/// covers a rotated view and culls the geometry that lands in the rotated
/// corners.
///
/// Returns `None` for a tilted (non-plan) view, where the view direction is
/// not vertical and a flat XY box cannot bound the visible region (depth
/// collapses onto the plane); callers then skip the frustum cull rather than
/// wrongly hide geometry.
fn view_cull_aabb(cam: &Camera, aspect: f32, margin: f32) -> Option<[f32; 4]> {
    // Plan view ⇔ line of sight is (near) vertical. Anything else can't be
    // bounded by a single world-XY rectangle.
    let fwd = cam.rotation * glam::Vec3::Z;
    if fwd.z.abs() < 0.999 {
        return None;
    }
    let h = cam.ortho_size();
    let w = h * aspect.max(0.01);
    let right = cam.rotation * glam::Vec3::X;
    let up = cam.rotation * glam::Vec3::Y;
    let c = cam.target.as_vec3();
    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (sw, sh) in [(-w, -h), (w, -h), (-w, h), (w, h)] {
        let p = c + right * sw + up * sh;
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    let (cx, cy) = ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);
    let (hw, hh) = ((max_x - min_x) * 0.5 * margin, (max_y - min_y) * 0.5 * margin);
    Some([cx - hw, cy - hh, cx + hw, cy + hh])
}

pub struct Scene {
    pub camera: Rc<RefCell<Camera>>,
    /// Model-space tiled viewport layout. One full-window tile by default;
    /// the split buttons / VPORTS subdivide the active tile.
    pub(crate) model_tiles: RefCell<Vec<ModelTile>>,
    /// Index of the active model tile (camera input + overlays target it).
    pub(crate) active_model_tile: std::cell::Cell<usize>,
    /// pane_grid layout tree for the Model tab — the source of truth for the
    /// tile split layout, resize and focus. `model_tiles` (the renderer's
    /// per-pane data: camera / render-mode / grid) is kept in lock-step with
    /// it, and its rects are derived from the pane regions. Each pane's value
    /// is the index of its backing `ModelTile`. Paper layout is unaffected.
    /// Plain field (not a `RefCell`) so the view can borrow it for the
    /// `PaneGrid` widget's lifetime; mutated through `&mut Scene` in update.
    pub(crate) model_panes: iced::widget::pane_grid::State<usize>,
    pub selection: Rc<RefCell<SelectionState>>,
    /// The CAD document — single source of truth for all entities.
    pub document: CadDocument,
    /// Currently selected entity handles.
    pub selected: HashSet<Handle>,
    /// Entity handles hidden by Isolate / Hide. Empty = nothing hidden.
    /// `tessellate_block`'s visibility test skips these, so they neither
    /// render nor hit-test until isolation ends.
    pub hidden: HashSet<Handle>,
    /// During in-place block edit (REFEDIT), the handles of the entities being
    /// edited. Everything else is rendered faded toward the background so the
    /// edited geometry stands out while the surrounding drawing stays visible
    /// for context. `None` = not editing. (#136)
    pub refedit_keep: Option<HashSet<Handle>>,
    /// Entity drawn with the selection-highlight colour without being part
    /// of the real selection — used to preview a row in the cycling list box.
    pub hover_highlight: Option<Handle>,
    /// Whether entity transparency is honoured on screen. When false the
    /// wire shader forces every line opaque (a uniform toggle, no retessellate).
    pub transparency_display: bool,
    /// Selection filter: entity-type names excluded from interactive picking.
    /// Empty = every type is selectable.
    pub selection_filter: HashSet<String>,
    /// In-progress preview wires while a command is active (rubber-band + object ghosts).
    pub preview_wires: Vec<WireModel>,
    /// Committed-segment wire drawn during multi-point commands (normal colour).
    pub interim_wire: Option<WireModel>,
    pub camera_generation: u64,
    /// Incremented whenever geometry-affecting state changes (entities, selection,
    /// preview wires, layer visibility, layout). The GPU pipeline uses this to
    /// skip re-uploading unchanged geometry buffers every frame.
    pub geometry_epoch: u64,
    /// Separate epoch for the (expensive) block-definition tessellation cache.
    /// Bumped together with `geometry_epoch` by `bump_geometry`, but NOT by
    /// `bump_geometry_no_blocks` — so edits that provably can't change any
    /// block definition (drawing a top-level entity, grip-moving an
    /// entity/insert) re-tessellate only the visible wires (~baseline cost)
    /// instead of rebuilding every block defn (the edit-time spike).
    pub block_epoch: u64,
    /// Incremented when the selection / hover-highlight set changes WITHOUT a
    /// geometry change. The wire tessellation is selection-independent, so a
    /// pick only refreshes the GPU xray overlay (cheap) instead of bumping
    /// `geometry_epoch` and re-tessellating the whole model.
    pub selection_generation: u64,
    /// Cached tessellation of all visible entity wires for the current layout.
    /// Keyed by `(geometry_epoch, camera_generation)` so a camera change
    /// invalidates the cull-dependent wire list as well as a geometry change.
    /// Uses `Arc` so `build_primitive()` avoids a full Vec clone during navigation.
    wire_cache: RefCell<Option<((u64, u64), Arc<Vec<WireModel>>)>>,
    /// Index built from every SortEntitiesTable in the document.
    /// Maps block_handle → (entity_handle.value() → sort_handle.value()).
    /// Replaces the O(objects) linear scan inside `wires_for_block()` with an O(1) lookup.
    sort_cache: RefCell<Option<(u64, HashMap<Handle, HashMap<u64, u64>>)>>,
    /// Per-entity normalized draw-order depth in (0,1), keyed by
    /// entity_handle.value(). Higher = drawn on top. Built once per
    /// geometry epoch by ranking every entity within its owning block by
    /// effective sort key (SortEntitiesTable override or own handle), then
    /// fed to the 2D pipelines as a small clip-z bias so entities of
    /// *different* types order correctly against each other. 3D meshes are
    /// excluded (they keep real geometric depth).
    draw_depth_cache: RefCell<Option<(u64, Arc<HashMap<u64, f32>>)>>,
    /// Cached hatch fill models, keyed by geometry_epoch. View culling
    /// is handled at draw time via `hatch_skip_flags` in the pipeline,
    /// not at build time — that lets the GPU buffer stay stable across
    /// pan/zoom while still skipping out-of-view hatches.
    /// Keyed by `(geometry_epoch, selection_generation)` — selected hatches
    /// are tinted, so a select/deselect must rebuild even when the geometry
    /// is unchanged (issue #71).
    hatch_cache: RefCell<Option<(u64, u64, Arc<Vec<HatchModel>>)>>,
    /// Cached wipeout fill models, keyed by geometry_epoch. Same
    /// reasoning as `hatch_cache`.
    wipeout_cache: RefCell<Option<(u64, Arc<Vec<HatchModel>>)>>,
    /// Cached image models, keyed by geometry_epoch. Images do their own
    /// per-frame culling in the GPU pipeline (vp_scissor); no camera key
    /// needed here.
    image_cache: RefCell<Option<(u64, Arc<Vec<ImageModel>>)>>,
    /// Cached mesh models, keyed by geometry_epoch.
    mesh_cache: RefCell<Option<(u64, Arc<Vec<MeshLodSet>>)>>,
    /// Per-viewport wire cache for paper-space rendering.
    /// Maps vp_handle → (geometry_epoch, Arc<Vec<WireModel>>).
    viewport_wire_cache: RefCell<HashMap<Handle, ((u64, u32, u64), Arc<Vec<WireModel>>)>>,
    /// Cached tessellation of paper-space layout block entities (title block, annotations, etc.).
    /// Separate from `wire_cache` so the GPU sheet viewport doesn't re-tessellate every frame.
    /// Keyed by `(geometry_epoch, camera_generation)` — paper view changes
    /// on zoom too, so culled wire output depends on camera.
    paper_sheet_cache: RefCell<Option<((u64, u64), Arc<Vec<WireModel>>)>>,
    /// Per-viewport projected wire cache for paper-space content viewports.
    /// Stores projected + clipped wires in paper-space coordinates.
    /// Maps vp_handle → (geometry_epoch, Vec<WireModel>).
    paper_projected_cache: RefCell<HashMap<Handle, (u64, Vec<WireModel>)>>,
    /// Active layout name — "Model" or a paper space layout name.
    pub current_layout: String,
    /// UCS→world rotation for the ViewCube, kept in sync with the tab's active
    /// UCS by `DocumentTab::sync_ucs_to_scene`. Identity = WCS. Applied only in
    /// model space so the cube's faces follow the user's coordinate system.
    pub viewcube_ucs: glam::Mat4,
    /// GPU render data for hatch fills, keyed by the DXF entity Handle.
    pub hatches: HashMap<Handle, HatchModel>,
    /// GPU render data for solid meshes (truck Shell/Solid tessellation).
    /// Top-level (layout-owned) solids only, stored in the offset-relative
    /// render frame and drawn flat.
    pub meshes: HashMap<Handle, MeshLodSet>,
    /// Meshes of block-definition solids, kept in *block-local* coordinates
    /// (no world_offset). They are not drawn directly; each INSERT of the
    /// owning block emits a transformed instance so a block placed at an
    /// INSERT scale renders at the right size. (#123)
    pub block_meshes: HashMap<Handle, MeshLodSet>,
    /// Live truck B-reps for solids created this session by the Model tab,
    /// keyed by entity handle. Backs the Design-group boolean tools (a solid
    /// must be here to be combined). Not persisted — rebuilt only by creating
    /// or combining primitives in-session.
    pub solid_models: HashMap<Handle, truck_modeling::Solid>,
    /// GPU render data for raster images (RasterImage entities), keyed by handle.
    pub images: HashMap<Handle, ImageModel>,
    /// The viewport that is currently "entered" (MSPACE mode).
    /// `None` = paper space editing (PSPACE).  Only meaningful when
    /// `current_layout != "Model"`.
    pub active_viewport: Option<Handle>,
    /// Custom model-space background fill color for Wipeout entities.
    /// Set from the active tab's `bg_color`; defaults to dark grey.
    pub bg_color: [f32; 4],
    /// Custom paper-space background fill color for Wipeout entities.
    pub paper_bg_color: [f32; 4],
    /// Largest local-space coordinate expected from real geometry, derived from
    /// EXTMIN/EXTMAX (10× safety margin). Used by fit_all() to ignore garbage
    /// entity coordinates (origin-stuck entities, bad Ray/XLine direction vectors).
    pub local_extent_max: f32,
    /// Robust centre (median of entity centroids) of the dense model-space
    /// cluster. Used together with `local_extent_max` to frame a viewport whose
    /// saved view is missing — aiming at the raw extents centre would land in
    /// the empty gap when a drawing has a second, far cluster.
    pub local_center: [f64; 2],
    /// Current annotation scale (CANNOSCALE equivalent).
    /// Multiplier applied to Text/MText/Dimension sizes during tessellation.
    /// 1.0 = no scaling. 50.0 = "1:50" drawing scale.
    pub annotation_scale: f32,
    /// Cached model-space bounding box, keyed by geometry_epoch.
    /// Avoids re-tessellating all entities on every ZOOM E / auto-fit call.
    model_extents_cache: RefCell<Option<(u64, Option<(glam::Vec3, glam::Vec3)>)>>,
    /// Reverse map: entity_handle → block_record_handle, built from entity_handles lists.
    /// Keyed by geometry_epoch. Eliminates the O(B) fallback scan in belongs_to_visible_block.
    entity_block_map_cache: RefCell<Option<(u64, HashMap<Handle, Handle>)>>,
    /// Tessellated block definitions in block-local coords, keyed by geometry_epoch.
    /// Lets Insert tessellation transform-copy cached wires instead of
    /// clone+explode+re-tessellate per reference.
    block_defn_cache: RefCell<Option<(u64, Arc<cache::block_cache::BlockCache>)>>,
    /// Spatial index + always-emit list for top-level entities
    /// (Phase 2.1). Lazily rebuilt by `entity_index()` on
    /// `geometry_epoch` change. See `EntityIndex` for what each side
    /// holds and why both are needed.
    entity_index_cache: RefCell<Option<(u64, EntityIndex)>>,
    /// Last viewport aspect ratio captured by the render pipeline. Used by
    /// `view_world_aabb` to compute the world-space view rect on demand.
    last_render_aspect: std::cell::Cell<f32>,
    /// World units that map to one screen pixel at the current camera +
    /// viewport size, captured each render. Drives the LOD pixel-size cull
    /// in expand_insert / tessellate_entity. 0 means "not yet set" — culling
    /// falls back to None.
    last_world_per_pixel: std::cell::Cell<f32>,
    /// ViewCube hover region (0..25, face/edge/corner index), driven by the
    /// `CursorMoved` message that the cube hit-area overlay publishes. Lives
    /// here so the unified render path can read it for the active viewport
    /// without depending on the shader widget's internal `Program::State`
    /// (which can miss events under overlapping overlays).
    pub viewcube_hover: std::cell::Cell<Option<usize>>,
    /// Wall time (ms) of the most recent wire re-tessellation — the work done
    /// on a wire-cache miss in `model_tile_wires_arc` / `paper_sheet_wires_arc`.
    /// Stays at the last value while the cache is hit (idle pan/zoom on a warm
    /// cache reads ~0). Surfaced by the frame-budget HUD (Phase 5.3).
    pub(crate) last_tess_ms: std::cell::Cell<f32>,
    /// Wire count produced by that most recent re-tessellation.
    pub(crate) last_tess_wires: std::cell::Cell<usize>,
    /// Content id ([`WIRE_CONTENT_GEN`]) of the Model wire set returned by the
    /// most recent `model_tile_wires_arc` call — stamped when the static set is
    /// (re)built, otherwise the held value. `build_primitive` reads it right
    /// after the call to gate GPU wire re-upload. 0 = none yet.
    pub(crate) last_model_wire_gen: std::cell::Cell<u64>,
    /// Static-hold cache for the Model layout: the FULL, un-culled tessellation
    /// held resident and reused for every camera (the geometry is
    /// camera-independent — see `model_tile_wires_arc`). `(epoch, gen, wires)`;
    /// rebuilt when `geometry_epoch` changes.
    model_static_wires: RefCell<Option<(u64, u64, Arc<Vec<WireModel>>)>>,
    /// Monotonic per-build nonce for wire sources that must NOT be skipped by
    /// the upload gate — the paper / per-viewport wire paths and any
    /// frame carrying live preview / interim wires. High bit set so it can
    /// never collide with a real [`WIRE_CONTENT_GEN`] id; incremented every
    /// use so the GPU always sees a fresh id and re-uploads.
    pub(crate) wire_force_nonce: std::cell::Cell<u64>,
    /// Memoized `(face3d, other)` split of the Model-tile wire set, keyed by
    /// its [`WIRE_CONTENT_GEN`] id. `split_face3d_wires` is an O(N) per-wire
    /// handle lookup + clone that otherwise re-runs every frame; a pan that
    /// reuses the tessellation (same id) reuses this split too.
    #[allow(clippy::type_complexity)]
    split_cache:
        RefCell<Option<(u64, Arc<Vec<WireModel>>, Arc<Vec<WireModel>>)>>,
    /// Cached `selected ∪ hover` handle set for the GPU xray overlay, keyed by
    /// `selection_generation`. Rebuilt only when the selection changes so
    /// `build_primitive` doesn't clone the set every frame.
    /// Per-entity tessellation memo for the culled Model render path (Phase
    /// 2.2). Maps a top-level handle to its already-tessellated wires so a
    /// single-entity edit re-tessellates only the changed entity and reuses the
    /// rest, instead of re-running the whole model. Keyed implicitly by
    /// `tess_memo_guard` (tol / view / anno / offset / bg); a guard mismatch
    /// (zoom, layout, …) clears it. `bump_geometry` clears it (structural
    /// change); `mark_entity_dirty` drops one handle (incremental edit).
    tess_memo: RefCell<HashMap<Handle, Arc<Vec<WireModel>>>>,
    /// Hash of the tessellation parameters `tess_memo` was built under. When
    /// the current call's parameters differ, the memo is stale and cleared.
    tess_memo_guard: std::cell::Cell<u64>,
}

impl Scene {
    pub fn new() -> Self {
        Self {
            camera: Rc::new(RefCell::new(Camera::default())),
            model_tiles: RefCell::new(vec![ModelTile {
                rect: iced::Rectangle {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
                camera: Camera::default(),
                render_mode: acadrust::entities::ViewportRenderMode::Wireframe2D,
                grid_on: false,
                snap_on: false,
            }]),
            active_model_tile: std::cell::Cell::new(0),
            // One pane mapped to tile 0 — matches the single default tile above.
            model_panes: iced::widget::pane_grid::State::new(0).0,
            selection: Rc::new(RefCell::new(SelectionState::default())),
            document: CadDocument::new(),
            selected: HashSet::default(),
            hidden: HashSet::default(),
            refedit_keep: None,
            hover_highlight: None,
            transparency_display: true,
            selection_filter: HashSet::default(),
            preview_wires: vec![],
            interim_wire: None,
            camera_generation: 0,
            geometry_epoch: GEOMETRY_EPOCH.fetch_add(1, Ordering::Relaxed),
            block_epoch: GEOMETRY_EPOCH.fetch_add(1, Ordering::Relaxed),
            selection_generation: 0,
            wire_cache: RefCell::new(None),
            sort_cache: RefCell::new(None),
            draw_depth_cache: RefCell::new(None),
            hatch_cache: RefCell::new(None),
            wipeout_cache: RefCell::new(None),
            image_cache: RefCell::new(None),
            mesh_cache: RefCell::new(None),
            viewport_wire_cache: RefCell::new(HashMap::default()),
            paper_sheet_cache: RefCell::new(None),
            paper_projected_cache: RefCell::new(HashMap::default()),
            current_layout: "Model".to_string(),
            viewcube_ucs: glam::Mat4::IDENTITY,
            hatches: HashMap::default(),
            meshes: HashMap::default(),
            block_meshes: HashMap::default(),
            solid_models: HashMap::default(),
            images: HashMap::default(),
            active_viewport: None,
            bg_color: [0.11, 0.11, 0.11, 1.0],
            paper_bg_color: [1.0, 1.0, 1.0, 1.0],
            local_extent_max: 1e9,
            local_center: [0.0, 0.0],
            annotation_scale: 1.0,
            model_extents_cache: RefCell::new(None),
            entity_block_map_cache: RefCell::new(None),
            block_defn_cache: RefCell::new(None),
            entity_index_cache: RefCell::new(None),
            last_render_aspect: std::cell::Cell::new(16.0 / 9.0),
            last_world_per_pixel: std::cell::Cell::new(0.0),
            viewcube_hover: std::cell::Cell::new(None),
            last_tess_ms: std::cell::Cell::new(0.0),
            last_tess_wires: std::cell::Cell::new(0),
            last_model_wire_gen: std::cell::Cell::new(0),
            model_static_wires: RefCell::new(None),
            wire_force_nonce: std::cell::Cell::new(0),
            split_cache: RefCell::new(None),
            tess_memo: RefCell::new(HashMap::default()),
            tess_memo_guard: std::cell::Cell::new(0),
        }
    }

    /// Compute the current camera's world-space XY view AABB with
    /// `world_offset` already subtracted (so the result is in the same f32
    /// space as emitted wire points). Adds a 25% margin around the
    /// frustum to absorb pan inertia and avoid clipped-edge popping.
    pub(super) fn view_world_aabb(&self) -> Option<[f32; 4]> {
        if self.current_layout != "Model" {
            // Paper-space viewport composition handles its own culling; the
            // top-level paper view is small enough not to need it.
            return None;
        }
        // Until the first explicit camera move (typically `fit_all()` after
        // file open), the camera sits at the default origin while geometry
        // lives at large local offsets — culling against the default rect
        // would discard everything and starve fit_all of points to fit to.
        if self.camera_generation == 0 {
            return None;
        }
        let cam = self.camera.borrow();
        let aspect = self.last_render_aspect.get().max(0.01);
        let h = cam.ortho_size();
        let w = h * aspect;
        let margin = 1.25_f32;
        // `cam.target` is in the same local f32 space as emitted wire points
        // (fit_to_bounds populates it from local wire coords). No further
        // `world_offset` subtraction is needed.
        let cx = cam.target.x as f32;
        let cy = cam.target.y as f32;
        Some([
            cx - w * margin,
            cy - h * margin,
            cx + w * margin,
            cy + h * margin,
        ])
    }

    /// Called by the render pipeline once per frame so `view_world_aabb` knows
    /// the active widget's aspect ratio.
    pub fn set_render_aspect(&self, aspect: f32) {
        if aspect.is_finite() && aspect > 0.0 {
            self.last_render_aspect.set(aspect);
        }
    }

    /// World units per screen pixel at the current viewport size. Returns
    /// `None` until the first render captures real bounds.
    ///
    /// Also returns `None` in paper space: `last_world_per_pixel` tracks the
    /// model camera, so a cached value applied to mm-sheet entity AABBs would
    /// be a stale model-world wpp and cull every paper-space annotation.
    /// Matches the same skip already in `view_world_aabb`.
    pub(super) fn world_per_pixel(&self) -> Option<f32> {
        if self.current_layout != "Model" {
            return None;
        }
        let v = self.last_world_per_pixel.get();
        if v > 0.0 && v.is_finite() {
            Some(v)
        } else {
            None
        }
    }

    /// Called from the render path with the current widget bounds so the
    /// LOD pixel-size culler knows how big one world unit projects to.
    pub fn set_render_pixel_scale(&self, width_px: f32, height_px: f32) {
        if !width_px.is_finite() || !height_px.is_finite() || height_px <= 0.0 {
            return;
        }
        let cam = self.camera.borrow();
        // Orthographic only. (Perspective varies with depth — we'd want a
        // depth-aware scale per entity. Skipped for now.)
        let h = cam.ortho_size();
        let world_per_px = (2.0 * h) / height_px;
        if world_per_px.is_finite() && world_per_px > 0.0 {
            self.last_world_per_pixel.set(world_per_px);
        }
    }

    /// Get (or build on miss) the block-definition cache for the current epoch.
    /// Built single-threaded — recursive nested expansion makes parallelization
    /// fiddly and the cache only rebuilds when geometry actually changes.
    pub(super) fn block_cache_arc(&self) -> Arc<cache::block_cache::BlockCache> {
        {
            let cache = self.block_defn_cache.borrow();
            if let Some((epoch, ref arc)) = *cache {
                if epoch == self.block_epoch {
                    return Arc::clone(arc);
                }
            }
        }
        let bg = if self.current_layout == "Model" {
            self.bg_color
        } else {
            self.paper_bg_color
        };
        let anno = if self.current_layout == "Model" {
            self.annotation_scale
        } else {
            1.0
        };
        let built = cache::block_cache::BlockCache::build(&self.document, anno, bg);
        let arc = Arc::new(built);
        *self.block_defn_cache.borrow_mut() = Some((self.block_epoch, Arc::clone(&arc)));
        arc
    }

    pub fn bump_geometry(&mut self) {
        self.geometry_epoch = GEOMETRY_EPOCH.fetch_add(1, Ordering::Relaxed);
        // Default: also invalidate block definitions. Safe for every caller;
        // operations that know blocks are untouched use `bump_geometry_no_blocks`.
        self.block_epoch = GEOMETRY_EPOCH.fetch_add(1, Ordering::Relaxed);
        // Structural change — drop the whole per-entity tessellation memo.
        self.tess_memo.borrow_mut().clear();
    }

    /// Drop a single entity from the tessellation memo so the next render
    /// re-tessellates just that entity while reusing every other. Pair with
    /// [`bump_geometry_no_blocks`] for an incremental single-entity edit.
    pub fn mark_entity_dirty(&mut self, handle: Handle) {
        self.tess_memo.borrow_mut().remove(&handle);
    }

    /// Invalidate the visible-wire tessellation but KEEP the cached block
    /// definitions. Use only when the edit provably can't change any block
    /// defn (top-level entity create/edit, grip-moving an entity or insert) —
    /// it skips the all-blocks re-tessellation that otherwise spikes edit time.
    pub fn bump_geometry_no_blocks(&mut self) {
        self.geometry_epoch = GEOMETRY_EPOCH.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark the selection / hover-highlight set dirty without invalidating the
    /// (selection-independent) wire tessellation. Only the GPU xray overlay is
    /// rebuilt — no re-tessellation. Use this for pure select / deselect /
    /// hover changes; use [`bump_geometry`] when the geometry itself changed.
    pub fn bump_selection(&mut self) {
        self.selection_generation = self.selection_generation.wrapping_add(1);
    }


    /// Re-evaluate every cached mesh's color through `render_style` so a
    /// Register a Model-tab solid: cache its truck B-rep (for boolean ops) and
    /// tessellate it into the shaded mesh pipeline under `handle`. The solid is
    /// in the same offset-relative frame the mesh pipeline uses, so the mesh is
    /// stored as-is (Model-tab geometry is authored at world_offset 0).
    pub fn register_solid_model(&mut self, handle: Handle, solid: truck_modeling::Solid) {
        let color = self
            .document
            .get_entity(handle)
            .map(|e| self.render_style(e).0)
            .unwrap_or([0.8, 0.8, 0.85, 1.0]);
        if let Some(set) = crate::scene::model::solid_model::mesh_from_solid(&solid, color) {
            self.meshes.insert(handle, set);
        }
        self.solid_models.insert(handle, solid);
        self.bump_geometry();
    }

    /// `BACKGROUND` change picks up the new `adapt_to_bg` result without
    /// re-tessellating ACIS geometry. Caller must bump `geometry_epoch`
    /// afterwards so the GPU re-uploads the now-updated colour data.
    pub fn recolor_meshes(&mut self) {
        // Cache colour lookups by handle to avoid borrowing the document
        // re-entrantly through `render_style` inside a `&mut self` loop.
        // Covers both top-level solid meshes and block-definition meshes
        // (instanced per INSERT), so a solid recolours wherever it lives.
        // During REFEDIT, solids outside the edited set render faded.
        let bg = self.bg_color;
        let colors: HashMap<Handle, [f32; 4]> = self
            .meshes
            .keys()
            .chain(self.block_meshes.keys())
            .filter_map(|&h| {
                self.document.get_entity(h).map(|e| {
                    let mut c = self.render_style(e).0;
                    if let Some(keep) = &self.refedit_keep {
                        if !keep.contains(&h) {
                            c = crate::scene::cache::block_cache::fade_toward_bg(c, bg);
                        }
                    }
                    (h, c)
                })
            })
            .collect();
        for (h, set) in self.meshes.iter_mut().chain(self.block_meshes.iter_mut()) {
            if let Some(&c) = colors.get(h) {
                for lod in &mut set.lods {
                    lod.color = c;
                }
            }
        }
    }

    /// Enter / leave the REFEDIT fade. `keep` holds the edited entities (left
    /// bright); everything else renders faded. Re-tessellates wires and
    /// recolours solids so the change shows immediately. (#136)
    pub fn set_refedit_keep(&mut self, keep: Option<HashSet<Handle>>) {
        self.refedit_keep = keep;
        self.recolor_meshes();
        self.bump_geometry();
    }

    /// Fade the colours of wires that belong to entities outside the REFEDIT
    /// keep set (no-op when not editing). The geometry is untouched, so
    /// hit-testing still works on faded entities.
    fn apply_refedit_fade(&self, wires: &mut [WireModel], bg: [f32; 4]) {
        let Some(keep) = &self.refedit_keep else {
            return;
        };
        for w in wires.iter_mut() {
            let keep_bright =
                Self::handle_from_wire_name(&w.name).is_some_and(|h| keep.contains(&h));
            if !keep_bright {
                w.color = crate::scene::cache::block_cache::fade_toward_bg(w.color, bg);
            }
        }
    }

    /// Switch the active layout. Bumps `geometry_epoch` so the wire cache
    /// re-tessellates — `render_style`'s `adapt_to_bg` picks the model or
    /// paper background depending on `current_layout`, so cached wires
    /// from the previous layout would be coloured against the wrong bg.
    /// Also runs `recolor_meshes` so ACIS mesh colour tracks the new bg.
    pub fn set_current_layout(&mut self, name: String) {
        if self.current_layout != name {
            self.current_layout = name;
            self.recolor_meshes();
            self.bump_geometry();
        }
    }

    /// Returns true if this viewport should display model-space content
    /// (i.e. it is a user viewport, not the sheet/overall viewport).
    ///
    /// Rules:
    /// - id=1  → always the sheet viewport → false
    /// - id≥2  → always a user viewport    → true
    /// - id=0 or id<0 (DWG reader omits the id; some DXF exporters write -1):
    ///   use geometry: the sheet viewport is centred at the paper origin (0,0)
    ///   with scale≈1.0 (view_height ≈ paper-space height).
    pub fn is_content_viewport(vp: &acadrust::entities::Viewport) -> bool {
        if vp.id == 1 {
            return false;
        }
        if vp.id > 1 {
            return true;
        }
        // id ≤ 0: DWG files never write group-code 69 (viewport id), so all
        // viewports arrive with id=0.
        //
        // In DWG format the sheet ("overall") viewport always has its center at
        // the paper-space origin (0, 0). Content viewports are placed at their
        // actual position on the paper and therefore have a non-zero center.
        // Using center position is more reliable than a scale heuristic because
        // the sheet viewport's scale is not always exactly 1:1 (observed: 0.8965
        // in real-world files, which the old 0.02 tolerance missed entirely).
        vp.center.x.abs() >= 0.5 || vp.center.y.abs() >= 0.5
    }

    fn current_layout_sheet_viewport_handle(&self) -> Handle {
        self.document.objects.values().find_map(|obj| {
            let ObjectType::Layout(layout) = obj else {
                return None;
            };
            if layout.name == self.current_layout {
                Some(layout.viewport)
            } else {
                None
            }
        }).unwrap_or(Handle::NULL)
    }

    /// Guarantee that a paper layout has its full-screen overall (`id == 1`)
    /// sheet viewport. AutoCAD always writes one, and `add_layout` creates it,
    /// but this is a safety net for layouts that arrive without it. The sheet
    /// viewport is the authoritative paper-space view and the canvas every
    /// floating viewport overlays.
    pub fn ensure_sheet_viewport(&mut self, layout_name: &str) {
        if layout_name == "Model" {
            return;
        }
        // Locate the layout: its object handle, block-record handle, current
        // sheet-viewport link, and paper limits.
        let info = self.document.objects.iter().find_map(|(h, obj)| {
            if let ObjectType::Layout(l) = obj {
                if l.name == layout_name {
                    return Some((*h, l.block_record, l.viewport, l.min_limits, l.max_limits));
                }
            }
            None
        });
        let Some((layout_handle, block_record, cur_vp, min_lim, max_lim)) = info else {
            return;
        };
        if block_record.is_null() {
            return;
        }

        // Already present? Accept either the linked viewport handle or any
        // `id == 1` viewport owned by the layout block.
        let has_sheet = self.document.entities().any(|e| {
            matches!(e, EntityType::Viewport(vp)
                if vp.common.owner_handle == block_record
                    && (vp.id == 1 || vp.common.handle == cur_vp))
        });
        if has_sheet {
            // Keep the layout's link in sync if it was missing.
            if !cur_vp.is_valid() {
                let h = self.document.entities().find_map(|e| match e {
                    EntityType::Viewport(vp)
                        if vp.common.owner_handle == block_record && vp.id == 1 =>
                    {
                        Some(vp.common.handle)
                    }
                    _ => None,
                });
                if let Some(h) = h {
                    if let Some(ObjectType::Layout(l)) =
                        self.document.objects.get_mut(&layout_handle)
                    {
                        l.viewport = h;
                    }
                }
            }
            return;
        }

        // Create the full-screen overall viewport covering the paper limits.
        let pw = (max_lim.0 - min_lim.0).abs().max(1.0);
        let ph = (max_lim.1 - min_lim.1).abs().max(1.0);
        let mut vp = acadrust::entities::Viewport::new();
        vp.id = 1;
        vp.status = acadrust::entities::ViewportStatusFlags::default_on();
        // Paper-space center is a 2D (x, y) point with z = 0 — the same
        // convention MVIEW uses for floating viewports. AutoCAD/TrueView read
        // the viewport center as (x, y); putting the paper-height midpoint in z
        // (with y = 0) left the sheet view centered at y = 0, shifting the whole
        // layout half a page down. See issue #156.
        vp.center = acadrust::types::Vector3::new(
            (min_lim.0 + max_lim.0) / 2.0,
            (min_lim.1 + max_lim.1) / 2.0,
            0.0,
        );
        vp.width = pw;
        vp.height = ph;
        // Frame the new layout on the whole sheet: look straight down at the
        // paper centre with the visible height a touch taller than the page.
        // Without this the viewport keeps `Viewport::new`'s default view
        // (target 0,0 / height 210), so the first time a fresh drawing's
        // layout is opened the camera sits on the paper's bottom-left corner
        // instead of centring the sheet.
        vp.view_target = acadrust::types::Vector3::new(
            (min_lim.0 + max_lim.0) / 2.0,
            (min_lim.1 + max_lim.1) / 2.0,
            0.0,
        );
        vp.view_center = acadrust::types::Vector3::ZERO;
        vp.view_height = ph * 1.1;
        if let Ok(handle) =
            self.document
                .add_entity_to_layout(EntityType::Viewport(vp), layout_name)
        {
            if let Some(ObjectType::Layout(l)) = self.document.objects.get_mut(&layout_handle) {
                l.viewport = handle;
            }
        }
    }

    fn is_content_viewport_in_layout(
        &self,
        vp: &acadrust::entities::Viewport,
        layout_block: Handle,
    ) -> bool {
        if vp.common.owner_handle != layout_block {
            return false;
        }
        let sheet_handle = self.current_layout_sheet_viewport_handle();
        if sheet_handle.is_valid() {
            vp.common.handle != sheet_handle
        } else {
            Self::is_content_viewport(vp)
        }
    }

    /// Public accessor for the block-record handle of the current layout.
    /// Used by external callers (e.g. `commit_entity`) that need the handle
    /// without going through private API.
    pub fn current_layout_block_handle_pub(&self) -> Handle {
        self.current_layout_block_handle()
    }

    /// Returns the block-record handle for `current_layout`.
    ///
    /// Primary path: the Layout object's `block_record` field (set correctly
    /// by the DWG reader).
    ///
    /// Fallback for DXF files: the DXF reader never reads group code 340
    /// (block_record handle), so `block_record` is NULL after loading DXF.
    /// In that case we derive the block-record name from the DXF convention:
    ///   Model            → "*Model_Space"
    ///   first paper tab  → "*Paper_Space"
    ///   second paper tab → "*Paper_Space0"
    ///   Nth paper tab    → "*Paper_Space{N-2}"
    fn current_layout_block_handle(&self) -> Handle {
        // Locate the Layout object for the active layout name.
        let layout = self.document.objects.values().find_map(|obj| {
            if let ObjectType::Layout(l) = obj {
                if l.name == self.current_layout {
                    Some(l)
                } else {
                    None
                }
            } else {
                None
            }
        });

        if let Some(l) = layout {
            // Fast path: block_record already set (DWG reader).
            if !l.block_record.is_null() {
                return l.block_record;
            }

            // Fallback: resolve via conventional DXF block-record name.
            let br_name: String = if self.current_layout == "Model" {
                "*Model_Space".into()
            } else {
                // tab_order 1 → "*Paper_Space",  2 → "*Paper_Space0", etc.
                let tab = l.tab_order;
                if tab <= 1 {
                    "*Paper_Space".into()
                } else {
                    format!("*Paper_Space{}", tab - 2)
                }
            };

            if let Some(br) = self.document.block_records.get(&br_name) {
                return br.handle;
            }

            // Last resort: match by position among paper layouts when tab_order
            // is unreliable (some exporters set it to 0 for all layouts).
            if self.current_layout != "Model" {
                let mut ps_brs: Vec<_> = self
                    .document
                    .block_records
                    .iter()
                    .filter(|br| br.is_paper_space())
                    .collect();
                ps_brs.sort_by(|a, b| a.name.cmp(&b.name));

                let mut paper_layouts: Vec<(i16, &str)> = self
                    .document
                    .objects
                    .values()
                    .filter_map(|obj| {
                        if let ObjectType::Layout(l) = obj {
                            if l.name != "Model" {
                                Some((l.tab_order, l.name.as_str()))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();
                paper_layouts.sort_by_key(|(o, n)| (*o, *n));

                if let Some(pos) = paper_layouts
                    .iter()
                    .position(|(_, n)| *n == self.current_layout)
                {
                    if let Some(br) = ps_brs.get(pos) {
                        return br.handle;
                    }
                }
            } else if let Some(br) = self.document.block_records.get("*Model_Space") {
                return br.handle;
            }
        }

        Handle::NULL
    }

    /// Returns `(min, max)` paper-space limits for the current layout, or `None`
    /// when in Model space.  Falls back to A4 landscape if nothing reliable is found.
    /// A solid white fill covering the paper sheet's printable area, rendered
    /// by the GPU hatch pipeline behind the paper entities. Replaces the 2-D
    /// white-rectangle the old PaperCanvas drew. `None` in model space or when
    /// the layout has no limits.
    pub(super) fn paper_sheet_fill(&self) -> Option<HatchModel> {
        let ((x0, y0), (x1, y1)) = self.paper_limits()?;
        let (x0, y0, x1, y1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
        Some(HatchModel {
            world_origin: [0.0, 0.0],
            boundary: Arc::new(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]),
            pattern: crate::scene::model::hatch_model::HatchPattern::Solid,
            name: "SOLID".to_string(),
            color: self.paper_bg_color,
            angle_offset: 0.0,
            scale: 1.0,
            vp_scissor: None,
            // Draw-order bias is signed: entity fills/wires land in (-1, 1)
            // (0 = neutral). A value below -1 forces the sheet strictly behind
            // EVERY object, in every case, with a tiny z offset (BIAS = 0.001,
            // so no far-plane clipping). The sheet is the canvas, never on top.
            draw_depth: -2.0,
        })
    }

    /// Dashed rectangle marking the printable area — the paper inset by the
    /// layout's plot margins. AutoCAD draws this guide on every layout; with the
    /// margins now preserved we can reflect it too. `None` in model space, when
    /// the layout has no margins, or when the inset would be degenerate.
    pub(super) fn printable_area_wire(&self) -> Option<WireModel> {
        if self.current_layout == "Model" {
            return None;
        }
        let ((x0, y0), (x1, y1)) = self.paper_limits()?;
        let (left, bottom, right, top, rot) =
            self.document.objects.values().find_map(|obj| {
                if let ObjectType::Layout(l) = obj {
                    if l.name == self.current_layout {
                        return Some((
                            l.plot_margin_left,
                            l.plot_margin_bottom,
                            l.plot_margin_right,
                            l.plot_margin_top,
                            l.plot_rotation,
                        ));
                    }
                }
                None
            })?;
        // `paper_limits()` already swaps the sheet for a 90°/270° rotation, so the
        // margins must rotate to the same edges: a margin on a physical side moves
        // to the displayed side that side rotates onto.
        let (ml, mb, mr, mt) = match rot {
            1 | 3 => (bottom, left, top, right),
            2 => (right, top, left, bottom),
            _ => (left, bottom, right, top),
        };
        // Nothing to show when there are no margins (printable area == sheet).
        if ml <= 0.0 && mb <= 0.0 && mr <= 0.0 && mt <= 0.0 {
            return None;
        }
        let (px0, py0, px1, py1) = (x0 + ml, y0 + mb, x1 - mr, y1 - mt);
        if px1 - px0 < 1e-3 || py1 - py0 < 1e-3 {
            return None;
        }
        let (px0, py0, px1, py1) = (px0 as f32, py0 as f32, px1 as f32, py1 as f32);
        let mut wire = WireModel::solid(
            "paper_printable_area".to_string(),
            vec![
                [px0, py0, 0.0],
                [px1, py0, 0.0],
                [px1, py1, 0.0],
                [px0, py1, 0.0],
                [px0, py0, 0.0],
            ],
            [0.5, 0.5, 0.5, 1.0],
            false,
        );
        // Dashed: 4 mm dash, 3 mm gap.
        wire.pattern_length = 7.0;
        wire.pattern = [4.0, -3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        Some(wire)
    }

    /// The effective plot settings for the current layout: a standalone
    /// PlotSettings page setup if one exists, otherwise the settings embedded in
    /// the LAYOUT object (paper size, margins, origin, rotation, scale). Loaded
    /// AutoCAD files keep their settings embedded, so without this fallback the
    /// plot/PDF path would ignore the file's rotation, origin and scale.
    pub fn effective_plot_settings(&self) -> Option<acadrust::objects::PlotSettings> {
        use acadrust::objects::{
            ObjectType, PaperMargin, PlotPaperUnits, PlotRotation, PlotSettings, PlotType,
            PlotWindow, ScaledType,
        };
        let name = &self.current_layout;
        if let Some(ps) = self.document.objects.values().find_map(|o| {
            if let ObjectType::PlotSettings(ps) = o {
                if &ps.page_name == name {
                    return Some(ps.clone());
                }
            }
            None
        }) {
            return Some(ps);
        }
        self.document.objects.values().find_map(|o| {
            let ObjectType::Layout(l) = o else { return None };
            if &l.name != name {
                return None;
            }
            let mut ps = PlotSettings::new(l.name.clone());
            ps.paper_width = l.paper_width;
            ps.paper_height = l.paper_height;
            ps.paper_size = l.paper_size.clone();
            ps.margins = PaperMargin::new(
                l.plot_margin_left,
                l.plot_margin_bottom,
                l.plot_margin_right,
                l.plot_margin_top,
            );
            ps.origin_x = l.plot_origin_x;
            ps.origin_y = l.plot_origin_y;
            ps.plot_window = PlotWindow::new(
                l.plot_window_min_x,
                l.plot_window_min_y,
                l.plot_window_max_x,
                l.plot_window_max_y,
            );
            ps.paper_units = PlotPaperUnits::from_code(l.plot_paper_units);
            ps.rotation = PlotRotation::from_code(l.plot_rotation);
            ps.plot_type = PlotType::from_code(l.plot_type);
            ps.scale_type = ScaledType::from_code(l.plot_scale_type);
            ps.scale_numerator = l.plot_scale_numerator;
            ps.scale_denominator = l.plot_scale_denominator;
            Some(ps)
        })
    }

    pub fn paper_limits(&self) -> Option<((f64, f64), (f64, f64))> {
        if self.current_layout == "Model" {
            return None;
        }

        self.document
            .objects
            .values()
            .find_map(|obj| {
                if let ObjectType::Layout(l) = obj {
                    if l.name != self.current_layout {
                        return None;
                    }

                    // Use the physical paper dimensions from PlotSettings if available
                    // (populated from DWG embedded plot settings or DXF codes 44/45/73).
                    // Rotation 1=90° or 3=270° → swap width and height.
                    if l.paper_width > 1e-6 && l.paper_height > 1e-6 {
                        let (pw, ph) = if l.plot_rotation == 1 || l.plot_rotation == 3 {
                            (l.paper_height, l.paper_width)
                        } else {
                            (l.paper_width, l.paper_height)
                        };
                        let ox = l.min_limits.0.min(0.0);
                        let oy = l.min_limits.1.min(0.0);
                        return Some(((ox, oy), (ox + pw, oy + ph)));
                    }

                    // Fall back to the Layout's drawing limits.
                    let (min, max) = (l.min_limits, l.max_limits);
                    let w = (max.0 - min.0).abs();
                    let h = (max.1 - min.1).abs();
                    if w < 1e-6 || h < 1e-6 {
                        return Some(((0.0, 0.0), (297.0, 210.0)));
                    }
                    Some((min, max))
                } else {
                    None
                }
            })
            .or(Some(((0.0, 0.0), (297.0, 210.0))))
    }

    /// Scale of the first user viewport (id > 1) in the current paper layout,
    /// used for the status-bar display.  Returns `None` in Model space or if
    /// no user viewport exists.
    pub fn first_viewport_scale(&self) -> Option<f64> {
        if self.current_layout == "Model" {
            return None;
        }
        let layout_block = self.current_layout_block_handle();
        if layout_block.is_null() {
            return None;
        }
        self.document.entities().find_map(|e| {
            if let EntityType::Viewport(vp) = e {
                if self.is_content_viewport_in_layout(vp, layout_block) {
                    return Some(vp_effective_scale(
                        vp.custom_scale,
                        vp.view_height,
                        vp.height,
                    ));
                }
            }
            None
        })
    }

    /// Annotation/viewport scales defined in the drawing's scale list
    /// (the `ACAD_SCALELIST` dictionary), as `(label, annotation_multiplier,
    /// viewport_factor)`. The annotation multiplier sizes model-space
    /// text/dims (50.0 for "1:50"); the viewport factor is the paper/drawing
    /// ratio (0.02 for "1:50"). Sorted smallest ratio first (1:100 … 1:1 …
    /// 10:1). Falls back to a standard ratio set when the drawing carries no
    /// scale list of its own, so the scale picker is always usable. (#154)
    pub fn scale_list(&self) -> Vec<(String, f32, f64)> {
        let mut list: Vec<(String, f32, f64)> = self
            .document
            .objects
            .values()
            .filter_map(|o| match o {
                // Skip xref-derived scales. Scales pulled in from an external
                // reference get an "_XREF" suffix ("1:50_XREF"); unbound
                // dependent symbols carry a "xref|name" prefix. Neither
                // belongs to this drawing's own scale list.
                ObjectType::Scale(s)
                    if !s.is_temporary
                        && !s.name.contains('|')
                        && !s.name.to_ascii_uppercase().ends_with("_XREF") =>
                {
                    Some((s.name.clone(), s.inverse_factor() as f32, s.factor()))
                }
                _ => None,
            })
            .collect();
        list.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        if list.is_empty() {
            // Many drawings (and minimal DXF/DWG exports) carry no
            // ACAD_SCALELIST Scale objects at all. Without a fallback the
            // annotation / viewport scale picker would be empty and so appear
            // broken. Substitute the standard ratio set — file scales still
            // win whenever the drawing actually defines any. (#154)
            const DEFAULT_SCALES: &[(&str, f64)] = &[
                ("1:500", 0.002),
                ("1:200", 0.005),
                ("1:100", 0.01),
                ("1:50", 0.02),
                ("1:20", 0.05),
                ("1:10", 0.1),
                ("1:5", 0.2),
                ("1:2", 0.5),
                ("1:1", 1.0),
                ("2:1", 2.0),
                ("5:1", 5.0),
                ("10:1", 10.0),
            ];
            list = DEFAULT_SCALES
                .iter()
                .map(|&(label, vp)| (label.to_string(), (1.0 / vp) as f32, vp))
                .collect();
        }
        list
    }

    /// List of user viewports in the current layout: (handle, label, frozen_layer_handles).
    pub fn viewport_list(&self) -> Vec<(acadrust::Handle, String, Vec<acadrust::Handle>)> {
        if self.current_layout == "Model" {
            return vec![];
        }
        let layout_block = self.current_layout_block_handle();
        if layout_block.is_null() {
            return vec![];
        }
        let mut result: Vec<(acadrust::Handle, String, Vec<acadrust::Handle>)> = self
            .document
            .entities()
            .filter_map(|e| {
                if let EntityType::Viewport(vp) = e {
                    if self.is_content_viewport_in_layout(vp, layout_block) {
                        Some((vp.common.handle, vp.id, vp.frozen_layers.clone()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .enumerate()
            .map(|(i, (h, id, frozen))| {
                let label = if id > 1 {
                    format!("VP {}", id - 1)
                } else {
                    format!("VP {}", i + 1)
                };
                (h, label, frozen)
            })
            .collect();
        result.sort_by_key(|(_, label, _)| label.clone());
        result
    }

    /// Count of user viewports (id > 1) in the current layout.
    pub fn viewport_count(&self) -> usize {
        if self.current_layout == "Model" {
            return 0;
        }
        let layout_block = self.current_layout_block_handle();
        if layout_block.is_null() {
            return 0;
        }
        self.document
            .entities()
            .filter(|e| {
                if let EntityType::Viewport(vp) = e {
                    self.is_content_viewport_in_layout(vp, layout_block)
                } else {
                    false
                }
            })
            .count()
    }

    /// True when any entities are hidden by Isolate / Hide.
    pub fn is_isolation_active(&self) -> bool {
        !self.hidden.is_empty()
    }

    /// Set (or clear) the previewed entity that renders with the selection
    /// highlight without joining the real selection. Only refreshes the GPU
    /// xray overlay (no re-tessellation).
    pub fn set_hover_highlight(&mut self, handle: Option<Handle>) {
        if self.hover_highlight == handle {
            return;
        }
        // Hover is folded into the highlight set (selected ∪ {hover}) that
        // drives the xray overlay. A hover handle that's already selected
        // contributes nothing, so the effective set is unchanged — skip the
        // overlay refresh then. The field is still updated for hit-test / UI.
        let contribution = |h: Option<Handle>| h.filter(|h| !self.selected.contains(h));
        let changed = contribution(self.hover_highlight) != contribution(handle);
        self.hover_highlight = handle;
        if changed {
            self.bump_selection();
        }
    }

    /// Hide every drawable entity except the current selection (Isolate).
    pub fn isolate_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let keep = self.selected.clone();
        self.hidden = self
            .document
            .entities()
            .map(|e| e.common().handle)
            .filter(|h| !h.is_null() && !keep.contains(h))
            .collect();
        self.selected.clear();
        self.bump_geometry();
    }

    /// Hide the current selection (Hide Objects).
    pub fn hide_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        for h in self.selected.iter().copied() {
            self.hidden.insert(h);
        }
        self.selected.clear();
        self.bump_geometry();
    }

    /// Clear isolation — bring every hidden entity back (End Isolation).
    pub fn end_isolation(&mut self) {
        if self.hidden.is_empty() {
            return;
        }
        self.hidden.clear();
        self.bump_geometry();
    }

    /// True if any currently selected entity is a Viewport.
    /// Used to enable the scale picker when a viewport is selected in paper space.
    pub fn has_selected_viewport(&self) -> bool {
        self.selected
            .iter()
            .any(|&h| matches!(self.document.get_entity(h), Some(EntityType::Viewport(_))))
    }

    /// First content viewport handle in the current layout, used as fallback target
    /// when no viewport is active or explicitly selected.
    fn first_viewport_handle(&self) -> Option<Handle> {
        if self.current_layout == "Model" {
            return None;
        }
        let layout_block = self.current_layout_block_handle();
        if layout_block.is_null() {
            return None;
        }
        self.document.entities().find_map(|e| {
            if let EntityType::Viewport(vp) = e {
                if self.is_content_viewport_in_layout(vp, layout_block) {
                    return Some(vp.common.handle);
                }
            }
            None
        })
    }

    /// Set the scale of the active/selected viewport.
    /// Priority: active_viewport → first selected viewport → first viewport in layout.
    pub fn set_viewport_scale(&mut self, scale: f64) {
        let target =
            self.active_viewport
                .or_else(|| {
                    self.selected.iter().copied().find(|&h| {
                        matches!(self.document.get_entity(h), Some(EntityType::Viewport(_)))
                    })
                })
                .or_else(|| self.first_viewport_handle());

        if let Some(handle) = target {
            if let Some(EntityType::Viewport(vp)) = self.document.get_entity_mut(handle) {
                if !vp.status.locked && scale > 1e-9 {
                    vp.custom_scale = scale;
                    vp.view_height = vp.height / scale;
                }
            }
            self.viewport_wire_cache.borrow_mut().remove(&handle);
            self.bump_geometry();
        }
    }

    /// Sorted list of layout names: "Model" first, then paper layouts by tab order.
    pub fn layout_names(&self) -> Vec<String> {
        let mut names = vec!["Model".to_string()];
        // Deduplicate by name: prefer the entry with a non-null block_record (the
        // real layout from the file) over the default placeholder created by
        // CadDocument::new().
        let mut by_name: rustc_hash::FxHashMap<String, (i16, Handle)> = Default::default();
        for obj in self.document.objects.values() {
            if let ObjectType::Layout(l) = obj {
                if l.name == "Model" || l.name.is_empty() {
                    continue;
                }
                let entry = by_name
                    .entry(l.name.clone())
                    .or_insert((l.tab_order, l.block_record));
                if entry.1.is_null() && !l.block_record.is_null() {
                    *entry = (l.tab_order, l.block_record);
                }
            }
        }
        let mut paper: Vec<(i16, String)> = by_name
            .into_iter()
            .map(|(name, (order, _))| (order, name))
            .collect();
        paper.sort_by_key(|(order, _)| *order);
        names.extend(paper.into_iter().map(|(_, n)| n));
        names
    }

    /// Collect closed polygon outlines (world XY) from the current layout.
    pub fn closed_outlines(&self) -> Vec<Vec<[f32; 2]>> {
        self.entity_wires()
            .into_iter()
            .filter_map(|wire| {
                let pts = wire.points;
                if pts.len() < 4 {
                    return None;
                }
                let f = pts.first()?;
                let l = pts.last()?;
                let dx = f[0] - l[0];
                let dy = f[1] - l[1];
                if (dx * dx + dy * dy).sqrt() > 1e-2 {
                    return None;
                }
                // Segment-list wires (e.g. LwPolyline) store each segment as an
                // independent NaN-separated pair, so every shared corner repeats
                // (`A B | B C | C D | D A`). Collapse that back into a clean ring:
                // skip the NaN separators and any vertex coincident with the
                // previous one, so consumers (point-in-polygon, the hatch /
                // boundary commands) see one vertex per corner — not the doubled
                // ring that otherwise shows two grips at every corner.
                let mut ring: Vec<[f32; 2]> = Vec::with_capacity(pts.len());
                for p in &pts {
                    if !p[0].is_finite() || !p[1].is_finite() {
                        continue;
                    }
                    let q = [p[0], p[1]];
                    if let Some(&last) = ring.last() {
                        if (last[0] - q[0]).abs() < 1e-4 && (last[1] - q[1]).abs() < 1e-4 {
                            continue;
                        }
                    }
                    ring.push(q);
                }
                // Drop a trailing vertex equal to the first — the ring is closed
                // implicitly, so keeping it would be a duplicate corner.
                if ring.len() > 1 {
                    let first = ring[0];
                    let last = *ring.last().unwrap();
                    if (first[0] - last[0]).abs() < 1e-4 && (first[1] - last[1]).abs() < 1e-4 {
                        ring.pop();
                    }
                }
                if ring.len() < 3 {
                    return None;
                }
                Some(ring)
            })
            .collect()
    }

    /// Wire set for the Model layout, shared by every tile.
    ///
    /// The model wire geometry is **camera-independent**, so it is tessellated
    /// in full (un-culled, fixed detail) once per geometry epoch, held resident
    /// (`model_static_wires`), and returned for any camera/tile. A pan/zoom only
    /// changes the view uniform — the GPU re-draws the same buffer, with no
    /// frustum cull, no zoom LOD, and no re-tessellation/re-upload on camera
    /// moves. The per-tile camera args are unused (kept for call-site symmetry
    /// with the paper-space wire sources).
    pub(super) fn model_tile_wires_arc(
        &self,
        _tile_idx: usize,
        _cam: &Camera,
        _cam_aspect: f32,
        _tile_pixel_height: f32,
    ) -> Arc<Vec<WireModel>> {
        // Reuse the resident full set if it's already built for this epoch.
        {
            let held = self.model_static_wires.borrow();
            if let Some((epoch, gen, arc)) = held.as_ref() {
                if *epoch == self.geometry_epoch {
                    self.last_model_wire_gen.set(*gen);
                    return Arc::clone(arc);
                }
            }
        }
        // Build once: full tessellation, no cull (region = None), no zoom LOD
        // (wpp = None). Held for the life of this geometry epoch.
        let block = self.model_space_block_handle();
        let t_tess = iced::time::Instant::now();
        let mut wires = self.wires_for_block_culled(block, None, None, None, None);
        self.apply_refedit_fade(&mut wires, self.bg_color);
        self.last_tess_ms.set(t_tess.elapsed().as_secs_f32() * 1000.0);
        self.last_tess_wires.set(wires.len());
        let arc = Arc::new(wires);
        let gen = WIRE_CONTENT_GEN.fetch_add(1, Ordering::Relaxed);
        self.last_model_wire_gen.set(gen);
        *self.model_static_wires.borrow_mut() = Some((self.geometry_epoch, gen, Arc::clone(&arc)));
        arc
    }

    /// Cached tessellation of the current layout block's paper-space entities,
    /// shared by `entity_wires_arc()` and the GPU sheet viewport.
    fn paper_sheet_wires_arc(&self) -> Arc<Vec<WireModel>> {
        let key = (self.geometry_epoch, self.camera_generation);
        {
            let cache = self.paper_sheet_cache.borrow();
            if let Some((cached_key, ref arc)) = *cache {
                if cached_key == key {
                    return Arc::clone(arc);
                }
            }
        }
        let layout_block = self.current_layout_block_handle();
        let t_tess = iced::time::Instant::now();
        let mut wires = self.wires_for_block(layout_block);
        self.last_tess_ms.set(t_tess.elapsed().as_secs_f32() * 1000.0);
        self.last_tess_wires.set(wires.len());
        // The overall "sheet" viewport now IS the paper view itself, so its own
        // border rectangle must not be drawn as an entity on the sheet.
        let sheet = self.current_layout_sheet_viewport_handle();
        if sheet.is_valid() {
            let sheet_name = sheet.value().to_string();
            wires.retain(|w| w.name != sheet_name);
        }
        let bg = if self.current_layout == "Model" {
            self.bg_color
        } else {
            self.paper_bg_color
        };
        self.apply_refedit_fade(&mut wires, bg);
        // Printable-area guide (paper inset by plot margins), paper space only.
        if let Some(pa) = self.printable_area_wire() {
            wires.push(pa);
        }
        let arc = Arc::new(wires);
        *self.paper_sheet_cache.borrow_mut() = Some((key, Arc::clone(&arc)));
        arc
    }

    /// Build WireModels from all document entities for the current layout.
    /// Returns a shared `Arc` so `build_primitive()` can skip the clone during
    /// navigation frames where no preview wires are active.
    pub(super) fn entity_wires_arc(&self) -> Arc<Vec<WireModel>> {
        let key = (self.geometry_epoch, self.camera_generation);
        {
            let cache = self.wire_cache.borrow();
            if let Some((cached_key, ref arc)) = *cache {
                if cached_key == key {
                    return Arc::clone(arc);
                }
            }
        }
        let layout_block = self.current_layout_block_handle();
        // Model space: paper_sheet_wires_arc IS the full entity wire set — share the Arc,
        // no Vec clone needed.
        if self.current_layout == "Model" {
            let arc = self.paper_sheet_wires_arc();
            *self.wire_cache.borrow_mut() = Some((key, Arc::clone(&arc)));
            return arc;
        }
        // Paper space: extend sheet wires with projected viewport content.
        let mut wires = (*self.paper_sheet_wires_arc()).clone();
        wires.extend(self.viewport_content_wires(layout_block, None, None));
        let arc = Arc::new(wires);
        *self.wire_cache.borrow_mut() = Some((key, Arc::clone(&arc)));
        arc
    }

    /// Build WireModels from all document entities + optional preview wire.
    pub fn entity_wires(&self) -> Vec<WireModel> {
        (*self.entity_wires_arc()).clone()
    }

    /// Per-entity normalized draw-order depth, keyed by entity handle value.
    /// Built (and cached per geometry epoch) by ranking every entity within
    /// its owning block by effective sort key (SortEntitiesTable override or
    /// own handle). The result feeds the 2D pipelines as a clip-z bias so
    /// entities of different types order correctly against each other.
    pub(super) fn draw_depth_map(&self) -> Arc<HashMap<u64, f32>> {
        {
            let cache = self.draw_depth_cache.borrow();
            if let Some((epoch, ref arc)) = *cache {
                if epoch == self.geometry_epoch {
                    return Arc::clone(arc);
                }
            }
        }
        use acadrust::objects::ObjectType;
        // Per-block SortEntitiesTable overrides: block -> (entity_val -> sort_val).
        let mut overrides: HashMap<Handle, HashMap<u64, u64>> = HashMap::default();
        for obj in self.document.objects.values() {
            if let ObjectType::SortEntitiesTable(t) = obj {
                if !t.is_empty() {
                    overrides.insert(
                        t.block_owner_handle,
                        t.entries()
                            .map(|e| (e.entity_handle.value(), e.sort_handle.value()))
                            .collect(),
                    );
                }
            }
        }
        let ms = self.model_space_block_handle();
        // Group entities by owning block, carrying each entity's effective key.
        let mut by_block: HashMap<Handle, Vec<(u64, u64)>> = HashMap::default();
        for e in self.document.entities() {
            let c = e.common();
            // 3D meshes keep real geometric depth — exclude them from
            // draw-order biasing so 3D occlusion is never flattened.
            if matches!(
                e,
                EntityType::Solid3D(_) | EntityType::Region(_) | EntityType::Body(_) | EntityType::Surface(_)
            ) {
                continue;
            }
            let block = if c.owner_handle.is_null() {
                ms
            } else {
                c.owner_handle
            };
            let hv = c.handle.value();
            let eff = overrides
                .get(&block)
                .and_then(|m| m.get(&hv))
                .copied()
                .unwrap_or(hv);
            by_block.entry(block).or_default().push((hv, eff));
        }
        let mut depth_map: HashMap<u64, f32> = HashMap::default();
        for (_block, mut v) in by_block {
            v.sort_by_key(|(_, eff)| *eff);
            let denom = (v.len() as f32) + 1.0;
            for (rank, (hv, _)) in v.into_iter().enumerate() {
                // Signed (-1,1): back ranks → negative, front → positive,
                // mid → ~0. The shader applies `z -= draw_depth * BIAS`, so a
                // default/unranked 0.0 means "no bias" (neutral) — which keeps
                // 3D mesh faces and transient wires at their real depth.
                let norm = (rank as f32 + 1.0) / denom; // (0,1)
                depth_map.insert(hv, (norm - 0.5) * 2.0);
            }
        }
        let arc = Arc::new(depth_map);
        *self.draw_depth_cache.borrow_mut() = Some((self.geometry_epoch, Arc::clone(&arc)));
        arc
    }

    pub(super) fn hatch_models_arc(&self) -> Arc<Vec<HatchModel>> {
        // Hatch models bake the selection tint (issue #71), so they depend on
        // the *selected set* — but NOT on hover. Keying on `selection_generation`
        // (which also bumps on every hover) made each hover-over a new entity
        // rebuild every hatch model: an O(N-hatch) stutter on hatch-heavy
        // drawings. Key on a signature of `selected` instead, so hover (which
        // never changes `selected`) keeps the cache warm.
        let sel_sig = self.selected_set_sig();
        {
            let cache = self.hatch_cache.borrow();
            if let Some((cached_epoch, cached_sel, ref arc)) = *cache {
                if cached_epoch == self.geometry_epoch && cached_sel == sel_sig {
                    return Arc::clone(arc);
                }
            }
        }
        let arc = Arc::new(self.synced_hatch_models());
        *self.hatch_cache.borrow_mut() = Some((self.geometry_epoch, sel_sig, Arc::clone(&arc)));
        arc
    }

    /// Order-independent signature of the selected set. Cheap (the set is
    /// normally a handful of entities) and unchanged by hover, so caches that
    /// only depend on what's *selected* don't thrash on rollover.
    fn selected_set_sig(&self) -> u64 {
        let mut sig: u64 = self.selected.len() as u64;
        for h in self.selected.iter() {
            sig ^= h.value().wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        sig
    }

    pub(super) fn wipeout_models_arc(&self) -> Arc<Vec<HatchModel>> {
        {
            let cache = self.wipeout_cache.borrow();
            if let Some((cached_epoch, ref arc)) = *cache {
                if cached_epoch == self.geometry_epoch {
                    return Arc::clone(arc);
                }
            }
        }
        let arc = Arc::new(self.wipeout_models());
        *self.wipeout_cache.borrow_mut() = Some((self.geometry_epoch, Arc::clone(&arc)));
        arc
    }

    pub(super) fn images_arc(&self) -> Arc<Vec<ImageModel>> {
        {
            let cache = self.image_cache.borrow();
            if let Some((cached_epoch, ref arc)) = *cache {
                if cached_epoch == self.geometry_epoch {
                    return Arc::clone(arc);
                }
            }
        }
        let depth_map = self.draw_depth_map();
        let arc = Arc::new(
            self.images
                .iter()
                .map(|(handle, model)| {
                    let mut m = model.clone();
                    m.draw_depth = depth_map.get(&handle.value()).copied().unwrap_or(0.0);
                    m
                })
                .collect(),
        );
        *self.image_cache.borrow_mut() = Some((self.geometry_epoch, Arc::clone(&arc)));
        arc
    }

    /// Images owned by the active paper layout block only. The full-canvas
    /// sheet viewport uses this so model-block images don't bleed onto the
    /// paper sheet (mirrors `paper_canvas_hatches`).
    pub(super) fn paper_sheet_images(&self) -> Arc<Vec<ImageModel>> {
        let layout_block = self.current_layout_block_handle();
        let depth_map = self.draw_depth_map();
        Arc::new(
            self.images
                .iter()
                .filter_map(|(&handle, model)| {
                    let entity = self.document.get_entity(handle)?;
                    let c = entity.common();
                    if c.invisible
                        || !self.belongs_to_visible_block(handle, c.owner_handle, layout_block)
                    {
                        return None;
                    }
                    let mut m = model.clone();
                    m.draw_depth = depth_map.get(&handle.value()).copied().unwrap_or(0.0);
                    Some(m)
                })
                .collect(),
        )
    }

    pub(super) fn meshes_arc(&self) -> Arc<Vec<MeshLodSet>> {
        {
            let cache = self.mesh_cache.borrow();
            if let Some((cached_epoch, ref arc)) = *cache {
                if cached_epoch == self.geometry_epoch {
                    return Arc::clone(arc);
                }
            }
        }
        // Top-level solids: drop those whose layer is off/frozen or that are
        // flagged invisible / isolated-hidden, mirroring the 2D wire path.
        let mut all: Vec<MeshLodSet> = self
            .meshes
            .iter()
            .filter(|(&h, _)| self.mesh_entity_visible(h))
            .map(|(_, set)| set.clone())
            .collect();
        // Block-definition solids are instanced per model-space INSERT so a
        // block placed at an INSERT scale renders at the right size. (#123)
        all.extend(self.instanced_block_meshes(self.model_space_block_handle()));
        let arc = Arc::new(all);
        *self.mesh_cache.borrow_mut() = Some((self.geometry_epoch, Arc::clone(&arc)));
        arc
    }

    /// True when `layer` is turned off or frozen — entities on it never render.
    fn layer_hidden(&self, layer: &str) -> bool {
        self.document
            .layers
            .get(layer)
            .map(|l| l.flags.off || l.flags.frozen)
            .unwrap_or(false)
    }

    /// Visibility test for a solid mesh entity, mirroring the 2D wire path:
    /// honour the invisible flag, the isolate/hide set, and the layer's
    /// off/frozen state.
    fn mesh_entity_visible(&self, handle: Handle) -> bool {
        let Some(c) = self.document.get_entity(handle).map(|e| e.common()) else {
            return false;
        };
        if c.invisible {
            return false;
        }
        if !self.hidden.is_empty() && self.hidden.contains(&handle) {
            return false;
        }
        !self.layer_hidden(&c.layer)
    }

    /// One transformed mesh per block-definition solid instance reached from an
    /// INSERT owned by `layout_block`. Nested INSERTs accumulate their
    /// transform. Empty when no block solids exist. (#123)
    fn instanced_block_meshes(&self, layout_block: Handle) -> Vec<MeshLodSet> {
        if self.block_meshes.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for e in self.document.entities() {
            if e.common().owner_handle != layout_block {
                continue;
            }
            if let EntityType::Insert(ins) = e {
                // INSERT on an off/frozen (or invisible) layer hides the whole
                // instance, block-internal solids included.
                if !self.mesh_entity_visible(ins.common.handle) {
                    continue;
                }
                let start = out.len();
                self.expand_block_meshes(&ins.block_name, &ins.get_transform(), 0, &mut out);
                // Tag the instanced meshes with the parent INSERT handle so the
                // hover / selection highlight (keyed on the mesh name) tints the
                // block, not the inner solid's own handle which nothing selects.
                let name = ins.common.handle.value().to_string();
                for set in &mut out[start..] {
                    for m in &mut set.lods {
                        m.name = name.clone();
                    }
                }
            }
        }
        out
    }

    /// Recursively emit transformed instances of a block's solid meshes,
    /// composing nested-INSERT transforms. (#123)
    fn expand_block_meshes(
        &self,
        block_name: &str,
        accum: &acadrust::types::Transform,
        depth: usize,
        out: &mut Vec<MeshLodSet>,
    ) {
        if depth > 16 {
            return;
        }
        let Some(br) = self.document.block_records.get(block_name) else {
            return;
        };
        let handles: Vec<Handle> = br.entity_handles.clone();
        for h in handles {
            let Some(e) = self.document.get_entity(h) else {
                continue;
            };
            // A block-internal solid / nested INSERT on an off/frozen layer
            // (or flagged invisible) must not render, same as a top-level one.
            if !self.mesh_entity_visible(h) {
                continue;
            }
            if let EntityType::Insert(ins) = e {
                let composed = ins.get_transform().then(accum);
                self.expand_block_meshes(&ins.block_name, &composed, depth + 1, out);
            } else if let Some(set) = self.block_meshes.get(&h) {
                out.push(transform_block_mesh_lod_set(set, accum));
            }
        }
    }

    /// Hatches eligible for click / box / lasso hit-testing in the current
    /// layout. Filters out block-internal source hatches (stored in
    /// `self.hatches` at block-local coords for the block-defn position,
    /// which doesn't project correctly through the offset-rel view_proj
    /// and was causing the wrong hatch to be selected on click).
    pub fn visible_hatches_for_click(&self) -> HashMap<Handle, HatchModel> {
        let layout_block = self.current_layout_block_handle();
        let model_block = self.model_space_block_handle();
        let layer_hidden = |layer: &str| {
            self.document
                .layers
                .get(layer)
                .map(|l| l.flags.off || l.flags.frozen)
                .unwrap_or(false)
        };
        self.hatches
            .iter()
            .filter_map(|(&h, m)| {
                let c = self.document.get_entity(h)?.common();
                if c.invisible || layer_hidden(&c.layer) {
                    return None;
                }
                // Mirror `synced_hatch_models`' visibility test (which drives
                // the fill render) so anything drawn is also clickable on its
                // fill, not just its boundary wire. The model-space fallback
                // matters when the layout block handle differs from the
                // entity's owner (issue: hatch fill not selectable).
                if self.belongs_to_visible_block(h, c.owner_handle, layout_block)
                    || self.belongs_to_visible_block(h, c.owner_handle, model_block)
                {
                    Some((h, m.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Per-Insert hatch models in the current layout, keyed by the Insert
    /// handle so a click on a block-internal hatch can select the parent
    /// Insert (AutoCAD behaviour: sub-entities of a block aren't directly
    /// selectable; the click resolves to the Insert).
    pub fn insert_hatches_for_click(&self) -> Vec<(Handle, HatchModel)> {
        let layout_block = self.current_layout_block_handle();
        let layer_hidden = |layer: &str| {
            self.document
                .layers
                .get(layer)
                .map(|l| l.flags.off || l.flags.frozen)
                .unwrap_or(false)
        };
        let mut out: Vec<(Handle, HatchModel)> = Vec::new();
        for entity in self.document.entities() {
            let EntityType::Insert(ins) = entity else {
                continue;
            };
            if ins.common.invisible || layer_hidden(&ins.common.layer) {
                continue;
            }
            if !self.belongs_to_visible_block(
                ins.common.handle,
                ins.common.owner_handle,
                layout_block,
            ) {
                continue;
            }
            for sub in ins
                .explode_from_document(&self.document)
                .into_iter()
                .map(crate::modules::draw::modify::explode::normalize_insert_entity)
            {
                let EntityType::Hatch(dxf) = sub else {
                    continue;
                };
                if dxf.common.invisible || layer_hidden(&dxf.common.layer) {
                    continue;
                }
                let color = self.render_style(&EntityType::Hatch(dxf.clone())).0;
                if let Some(model) = Self::hatch_model_from_dxf(&dxf, color) {
                    out.push((ins.common.handle, model));
                }
            }
        }
        out
    }

    /// Wires that should participate in hit-testing, snapping, and selection.
    ///
    /// - Model layout: all entity wires (same as entity_wires).
    /// - PSPACE (paper layout, no active viewport): paper-space entities only —
    ///   viewport content is NOT interactive.
    /// - MSPACE (active viewport set): model-space content of the active viewport
    ///   only — paper-space entities are NOT interactive.
    pub fn hit_test_wires(&self) -> Arc<Vec<WireModel>> {
        if self.current_layout == "Model" {
            // entity_wires_arc is culled to the current view and keyed on the
            // camera, so it re-culls when the view changes — picking must reach
            // entities that scroll into view after a pan/zoom.
            return self.entity_wires_arc();
        }
        let layout_block = self.current_layout_block_handle();
        match self.active_viewport {
            None => Arc::new(self.wires_for_block(layout_block)),
            Some(vp_handle) => {
                Arc::new(self.viewport_content_wires(layout_block, Some(vp_handle), None))
            }
        }
    }

    /// Pick a meshed 3D solid by clicking on its shaded body (face), not just
    /// its thin projected edges. Returns the front-most mesh under `cursor`.
    #[allow(dead_code)]
    pub fn mesh_click_hit(
        &self,
        cursor: iced::Point,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
    ) -> Option<Handle> {
        let iter = self
            .meshes
            .iter()
            .filter_map(|(h, set)| set.lods.first().map(|m| (*h, m)));
        pick::hit_test::mesh_click_hit(cursor, iter, view_rot, eye, bounds)
    }

    /// True when any handle resolves to an ACIS volume entity (3D solid /
    /// region / body / surface) — i.e. one whose render geometry is a cached
    /// mesh that must be re-tessellated after an edit.
    pub fn any_solid(&self, handles: &[Handle]) -> bool {
        handles.iter().any(|&h| {
            matches!(
                self.document.get_entity(h),
                Some(EntityType::Solid3D(_))
                    | Some(EntityType::Region(_))
                    | Some(EntityType::Body(_))
                    | Some(EntityType::Surface(_))
            )
        })
    }

    /// Top-level solid handles caught by a rectangular selection box.
    pub fn mesh_box_hit(
        &self,
        a: iced::Point,
        b: iced::Point,
        crossing: bool,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
    ) -> Vec<Handle> {
        let iter = self
            .meshes
            .iter()
            .filter_map(|(h, set)| set.lods.first().map(|m| (*h, m)));
        pick::hit_test::mesh_box_hit(a, b, crossing, iter, view_rot, eye, bounds)
    }

    /// Top-level solid handles caught by a lasso polygon.
    pub fn mesh_poly_hit(
        &self,
        poly: &[iced::Point],
        crossing: bool,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
    ) -> Vec<Handle> {
        let iter = self
            .meshes
            .iter()
            .filter_map(|(h, set)| set.lods.first().map(|m| (*h, m)));
        pick::hit_test::mesh_poly_hit(poly, crossing, iter, view_rot, eye, bounds)
    }

    /// Front-most solid under the cursor across BOTH top-level solid meshes
    /// (keyed by their own handle) and block-internal solid instances (keyed
    /// by the parent INSERT). Combining them in one depth-sorted test means a
    /// block in front of a stray solid wins, instead of the solid always
    /// taking priority by virtue of being tried first.
    pub fn solid_click_hit(
        &self,
        cursor: iced::Point,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
    ) -> Option<Handle> {
        // Block-internal instances must be owned (transformed copies); keep
        // them in a Vec and chain references alongside the top-level meshes so
        // neither set is cloned wholesale.
        let mut block_owned: Vec<(Handle, crate::scene::model::mesh_model::MeshModel)> = Vec::new();
        if !self.block_meshes.is_empty() {
            let layout_block = self.current_layout_block_handle();
            for e in self.document.entities() {
                if e.common().owner_handle != layout_block {
                    continue;
                }
                let EntityType::Insert(ins) = e else { continue };
                if !self.mesh_entity_visible(ins.common.handle) {
                    continue;
                }
                let mut sets = Vec::new();
                self.expand_block_meshes(&ins.block_name, &ins.get_transform(), 0, &mut sets);
                for set in sets {
                    if let Some(m) = set.lods.into_iter().next() {
                        block_owned.push((ins.common.handle, m));
                    }
                }
            }
        }
        let top = self
            .meshes
            .iter()
            .filter_map(|(h, set)| set.lods.first().map(|m| (*h, m)));
        let blk = block_owned.iter().map(|(h, m)| (*h, m));
        pick::hit_test::mesh_click_hit(cursor, top.chain(blk), view_rot, eye, bounds)
    }

    /// Parent INSERT handles whose block-internal solid meshes fall in a
    /// rectangular selection box. A block whose visible body is a solid has
    /// no wires to catch, so box/lasso selection must test its instanced
    /// meshes too.
    pub fn block_mesh_box_hit(
        &self,
        a: iced::Point,
        b: iced::Point,
        crossing: bool,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
    ) -> Vec<Handle> {
        if self.block_meshes.is_empty() {
            return Vec::new();
        }
        let layout_block = self.current_layout_block_handle();
        let mut out = Vec::new();
        for e in self.document.entities() {
            if e.common().owner_handle != layout_block {
                continue;
            }
            let EntityType::Insert(ins) = e else { continue };
            if !self.mesh_entity_visible(ins.common.handle) {
                continue;
            }
            let mut sets = Vec::new();
            self.expand_block_meshes(&ins.block_name, &ins.get_transform(), 0, &mut sets);
            let hit = sets.iter().any(|set| {
                set.lods.first().map_or(false, |m| {
                    !pick::hit_test::mesh_box_hit(
                        a,
                        b,
                        crossing,
                        std::iter::once((ins.common.handle, m)),
                        view_rot,
                        eye,
                        bounds,
                    )
                    .is_empty()
                })
            });
            if hit {
                out.push(ins.common.handle);
            }
        }
        out
    }

    /// Parent INSERT handles whose block-internal solid meshes fall in a lasso.
    pub fn block_mesh_poly_hit(
        &self,
        poly: &[iced::Point],
        crossing: bool,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
    ) -> Vec<Handle> {
        if self.block_meshes.is_empty() {
            return Vec::new();
        }
        let layout_block = self.current_layout_block_handle();
        let mut out = Vec::new();
        for e in self.document.entities() {
            if e.common().owner_handle != layout_block {
                continue;
            }
            let EntityType::Insert(ins) = e else { continue };
            if !self.mesh_entity_visible(ins.common.handle) {
                continue;
            }
            let mut sets = Vec::new();
            self.expand_block_meshes(&ins.block_name, &ins.get_transform(), 0, &mut sets);
            let hit = sets.iter().any(|set| {
                set.lods.first().map_or(false, |m| {
                    !pick::hit_test::mesh_poly_hit(
                        poly,
                        crossing,
                        std::iter::once((ins.common.handle, m)),
                        view_rot,
                        eye,
                        bounds,
                    )
                    .is_empty()
                })
            });
            if hit {
                out.push(ins.common.handle);
            }
        }
        out
    }

    /// Tessellate all non-invisible entities owned by `block_handle`.
    fn wires_for_block(&self, block_handle: Handle) -> Vec<WireModel> {
        // Default culling is driven by the live `Scene::camera`. Multi-tile
        // Model layouts and paper-space content viewports call
        // `wires_for_block_culled` directly with their own per-view cull
        // parameters so each pane culls independently.
        self.wires_for_block_culled(
            block_handle,
            self.view_world_aabb(),
            self.world_per_pixel(),
            None,
            None,
        )
    }

    fn wires_for_block_culled(
        &self,
        block_handle: Handle,
        view_aabb: Option<[f32; 4]>,
        wpp: Option<f32>,
        // Layers frozen specifically through the requesting viewport.
        // Hidden in addition to the document-level off / frozen flags.
        // `None` skips the per-viewport check (Model-space callers).
        frozen_layers: Option<&HashSet<Handle>>,
        // Paper-space content viewports compute their own annotation
        // scale from `vp_effective_scale`; the Model-space and paper-
        // sheet paths use `self.annotation_scale` / 1.0 respectively.
        // `None` selects the default branch on `current_layout`.
        anno_scale_override: Option<f32>,
    ) -> Vec<WireModel> {
        use acadrust::objects::ObjectType;

        // ── Ensure sort-order index is current ────────────────────────────
        // Replaces the old O(objects) find_map with one rebuild per epoch,
        // after which every wires_for_block call is an O(1) HashMap lookup.
        {
            let needs_rebuild = self
                .sort_cache
                .borrow()
                .as_ref()
                .map(|(e, _)| *e != self.geometry_epoch)
                .unwrap_or(true);

            if needs_rebuild {
                let mut idx: HashMap<Handle, HashMap<u64, u64>> = HashMap::default();
                for obj in self.document.objects.values() {
                    if let ObjectType::SortEntitiesTable(t) = obj {
                        if !t.is_empty() {
                            let map = t
                                .entries()
                                .map(|e| (e.entity_handle.value(), e.sort_handle.value()))
                                .collect();
                            idx.insert(t.block_owner_handle, map);
                        }
                    }
                }
                *self.sort_cache.borrow_mut() = Some((self.geometry_epoch, idx));
            }
        }

        // Visibility test reused by both paths below.
        let visibility_ok = |e: &EntityType| -> bool {
            let c = e.common();
            if c.invisible {
                return false;
            }
            // Isolate / Hide: skip entities the user has hidden.
            if !self.hidden.is_empty() && self.hidden.contains(&c.handle) {
                return false;
            }
            // Block/BlockEnd are block-defn sentinels, not drawable geometry.
            // Without this skip they fall through to fallback_geometry's `_`
            // arm and emit a 1-unit phantom segment at world_offset that
            // poisons fit_all and shows up in selection.
            if matches!(e, EntityType::Block(_) | EntityType::BlockEnd(_)) {
                return false;
            }
            let layer = self.document.layers.get(&c.layer);
            if layer.map(|l| l.flags.off || l.flags.frozen).unwrap_or(false) {
                return false;
            }
            if let Some(frozen) = frozen_layers {
                if !frozen.is_empty() {
                    if let Some(lh) = layer.map(|l| l.handle) {
                        if frozen.contains(&lh) {
                            return false;
                        }
                    }
                }
            }
            self.belongs_to_visible_block(e.common().handle, c.owner_handle, block_handle)
        };

        // Phase 2.1 — quadtree-driven candidate selection. When a view
        // AABB exists (Model layout with a settled camera), only iterate
        // entities whose stored WCS bbox intersects the view; unindexable
        // entities (Insert/Viewport) are appended via a small linear scan.
        // Paper space and the first-frame "settle" path fall back to the
        // full doc scan — preserving prior behaviour.
        let visible: Vec<&EntityType> = if let Some(local_view) = view_aabb {
            let view_wcs: [f64; 4] = [
                local_view[0] as f64,
                local_view[1] as f64,
                local_view[2] as f64,
                local_view[3] as f64,
            ];
            let (candidates, unbounded): (Vec<Handle>, Vec<Handle>) = {
                let idx = self.entity_index();
                (idx.tree.query_rect(view_wcs), idx.unbounded_handles.clone())
            };
            let mut out: Vec<&EntityType> =
                Vec::with_capacity(candidates.len() + unbounded.len() + 16);
            for h in candidates {
                if let Some(e) = self.document.get_entity(h) {
                    if visibility_ok(e) {
                        out.push(e);
                    }
                }
            }
            // Unbounded entities — always emit regardless of view, mirroring
            // legacy `entity_aabb`'s UNBOUNDED_AABB sentinel.
            for h in unbounded {
                if let Some(e) = self.document.get_entity(h) {
                    if visibility_ok(e) {
                        out.push(e);
                    }
                }
            }
            // Inserts/Viewports/Block/BlockEnd — handled by their own paths
            // (block expansion, viewport rendering); always candidates.
            for e in self.document.entities() {
                if is_unindexable_entity(e) && visibility_ok(e) {
                    out.push(e);
                }
            }
            out
        } else {
            self.document
                .entities()
                .filter(|e| visibility_ok(e))
                .collect()
        };

        // Tessellate in parallel across all available CPU cores.
        use crate::par::prelude::*;
        let doc = &self.document;
        // Selection / hover highlight is NOT baked into tessellation. It is
        // applied per frame in the GPU xray overlay pass from the live
        // selection set (`Scene::selected` ∪ hover). Keeping `sel` empty here
        // makes the wire cache selection-independent, so picking an entity
        // bumps only `selection_generation` (cheap overlay refresh) instead of
        // `geometry_epoch` (a full model re-tessellation).
        let empty_sel: HashSet<Handle> = HashSet::default();
        let sel: &HashSet<Handle> = &empty_sel;
        let avp = self.active_viewport;
        // A paper-space content viewport renders MODEL block entities while
        // the user is sitting in a paper layout — that path expects
        // `world_offset` subtraction even though `current_layout != "Model"`.
        // Decide based on the block being tessellated, not the layout.
        let is_model_block = block_handle == self.model_space_block_handle();
        let bg = if self.current_layout == "Model" {
            self.bg_color
        } else {
            self.paper_bg_color
        };
        let anno = if let Some(a) = anno_scale_override {
            a
        } else if self.current_layout == "Model" {
            self.annotation_scale
        } else {
            1.0
        };
        let blk_cache = self.block_cache_arc();
        let blk_ref: &cache::block_cache::BlockCache = &blk_cache;
        // Zoom-adaptive curve sampling for top-level Edge tessellation. Target
        // ~0.5 px chord height — far-out arcs that used to emit hundreds of
        // segments now collapse to a handful. The guard clears the override
        // when this scope exits so off-render tessellation (snap previews,
        // hit-test, block_cache rebuild) sees the default.
        struct CurveTolGuard;
        impl Drop for CurveTolGuard {
            fn drop(&mut self) {
                crate::scene::convert::truck_tess::set_curve_tol_override(None);
            }
        }
        let _tol_guard = wpp.map(|w| {
            crate::scene::convert::truck_tess::set_curve_tol_override(Some((w * 0.5) as f64));
            CurveTolGuard
        });
        // Per-entity tessellation memo (Phase 2.2) — only on the culled Model
        // render path. A single-entity edit re-tessellates just the changed
        // entity (dropped from the memo via `mark_entity_dirty`) and reuses the
        // rest, instead of re-running every visible entity. The hit-test path
        // (`view_aabb == None`) and paper / per-viewport paths bypass it so
        // their different cull parameters don't thrash the memo.
        let memo_active = view_aabb.is_some()
            && is_model_block
            && frozen_layers.is_none()
            && anno_scale_override.is_none();
        let mut wires: Vec<WireModel> = if memo_active {
            // Guard hash of everything tessellate_entity output depends on
            // besides the entity itself. A mismatch (zoom/tol, view, anno,
            // offset, bg, entered viewport) means the memo is stale.
            let guard = {
                let mut g: u64 = 0xcbf2_9ce4_8422_2325;
                let mut mix = |x: u64| g = g.rotate_left(13) ^ x;
                mix(wpp.map(|w| w.to_bits() as u64).unwrap_or(u64::MAX));
                if let Some(v) = view_aabb {
                    for c in v {
                        mix(c.to_bits() as u64);
                    }
                }
                mix(anno.to_bits() as u64);
                for c in bg {
                    mix(c.to_bits() as u64);
                }
                mix(avp.map(|h| h.value()).unwrap_or(0));
                g
            };
            if self.tess_memo_guard.get() != guard {
                self.tess_memo.borrow_mut().clear();
                self.tess_memo_guard.set(guard);
            }
            // Classify (serial, cheap): reuse memoized Arcs, collect misses.
            let mut hit_arcs: Vec<Arc<Vec<WireModel>>> = Vec::new();
            let mut misses: Vec<&EntityType> = Vec::new();
            {
                let memo = self.tess_memo.borrow();
                for e in &visible {
                    let h = e.common().handle;
                    match memo.get(&h) {
                        Some(a) => hit_arcs.push(Arc::clone(a)),
                        None => misses.push(*e),
                    }
                }
            }
            // Materialize hits + tessellate misses, both in parallel.
            let hit_wires: Vec<WireModel> =
                hit_arcs.par_iter().flat_map_iter(|a| a.iter().cloned()).collect();
            let miss_pairs: Vec<(Handle, Arc<Vec<WireModel>>)> = misses
                .par_iter()
                .map(|e| {
                    let e: &EntityType = e;
                    let w = tessellate_entity(
                        doc, sel, avp, bg, anno, e, Some(blk_ref), view_aabb, wpp,
                    );
                    (e.common().handle, Arc::new(w))
                })
                .collect();
            let mut out = hit_wires;
            {
                let mut memo = self.tess_memo.borrow_mut();
                for (h, a) in &miss_pairs {
                    out.extend(a.iter().cloned());
                    memo.insert(*h, Arc::clone(a));
                }
            }
            out
        } else {
            visible
                .into_par_iter()
                .flat_map(|e| {
                    tessellate_entity(
                        doc, sel, avp, bg, anno, e, Some(blk_ref), view_aabb, wpp,
                    )
                })
                .collect()
        };

        // Apply draw order via the cached index (O(1) block lookup).
        {
            let cache = self.sort_cache.borrow();
            if let Some((_, ref idx)) = *cache {
                if let Some(sort_map) = idx.get(&block_handle) {
                    wires.sort_by_key(|w| {
                        let key = Self::handle_from_wire_name(&w.name)
                            .map(|h| h.value())
                            .unwrap_or(u64::MAX);
                        // Entities absent from the table sort by their own
                        // handle — the same key space the table's sort handles
                        // live in — so reordered and untouched entities interleave
                        // correctly instead of all collapsing to one constant.
                        sort_map.get(&key).copied().unwrap_or(key)
                    });
                }
            }
        }
        wires
    }

    /// Decide whether an entity should be drawn as direct content of `block_handle`.
    fn belongs_to_visible_block(
        &self,
        entity_handle: Handle,
        owner_handle: Handle,
        block_handle: Handle,
    ) -> bool {
        if block_handle.is_null() {
            return true;
        }
        if owner_handle == block_handle {
            return true;
        }
        if !owner_handle.is_null() {
            return false;
        }

        // owner_handle is null (common in DXF files that omit group code 330).
        // Use the current layout's entity_handles as the authoritative list when
        // available — this prevents block-definition geometry from leaking into
        // the viewport even when owner handles are missing.
        if let Some(br) = self
            .document
            .block_records
            .iter()
            .find(|br| br.handle == block_handle)
        {
            if !br.entity_handles.is_empty() {
                return br.entity_handles.contains(&entity_handle);
            }
        }

        // P: epoch-cached reverse map replaces O(B) block_records scan.
        let map = self.entity_block_map();
        if let Some(&owner) = map.get(&entity_handle) {
            return owner == block_handle;
        }
        // Map miss. Permissive only when NO BlockRecord enumerated its
        // entity_handles — that's a legacy DXF that omits 330 group codes
        // everywhere, where dropping unknown-owner entities would empty
        // model space. When at least one block did enumerate, the file is
        // capable of declaring ownership, so an unknown-owner entity is
        // an orphan (typically a block-defn entity whose owner was lost on
        // round-trip) and must not leak into the queried block.
        if map.is_empty() {
            return true;
        }
        false
    }

    /// Build (and epoch-cache) a reverse map: entity_handle → block_record_handle,
    /// covering every entity explicitly listed in a block_record's entity_handles.
    fn entity_block_map(&self) -> std::cell::Ref<'_, HashMap<Handle, Handle>> {
        {
            let cache = self.entity_block_map_cache.borrow();
            if let Some((epoch, _)) = *cache {
                if epoch == self.geometry_epoch {
                    drop(cache);
                    return std::cell::Ref::map(self.entity_block_map_cache.borrow(), |c| {
                        &c.as_ref().unwrap().1
                    });
                }
            }
        }
        let mut map: HashMap<Handle, Handle> = HashMap::default();
        for br in self.document.block_records.iter() {
            for &eh in &br.entity_handles {
                map.insert(eh, br.handle);
            }
        }
        *self.entity_block_map_cache.borrow_mut() = Some((self.geometry_epoch, map));
        std::cell::Ref::map(self.entity_block_map_cache.borrow(), |c| {
            &c.as_ref().unwrap().1
        })
    }

    /// Spatial index + always-emit list for top-level entities. Lazily
    /// rebuilt on `geometry_epoch` change.
    ///
    /// `tree` holds entities whose `bounding_box()` is finite and
    /// non-degenerate. `unbounded_handles` holds entities whose bbox
    /// is degenerate or non-finite — the legacy `entity_aabb` treated
    /// those as `UNBOUNDED_AABB` (never culled), so the wire path must
    /// always emit them regardless of view. Inserts/Viewports/Blocks
    /// /BlockEnds are filtered out at build time and re-added by the
    /// wire path via a separate scan (their WCS bbox depends on
    /// transforms handled elsewhere).
    pub(super) fn entity_index(&self) -> std::cell::Ref<'_, EntityIndex> {
        {
            let cache = self.entity_index_cache.borrow();
            if let Some((epoch, _)) = *cache {
                if epoch == self.geometry_epoch {
                    drop(cache);
                    return std::cell::Ref::map(self.entity_index_cache.borrow(), |c| {
                        &c.as_ref().unwrap().1
                    });
                }
            }
        }

        let mut items: Vec<(Handle, [f64; 4])> = Vec::new();
        let mut unbounded: Vec<Handle> = Vec::new();
        let mut union: Option<[f64; 4]> = None;
        for e in self.document.entities() {
            if is_unindexable_entity(e) {
                continue;
            }
            match entity_world_aabb_f64(e) {
                Some(ab) => {
                    union = Some(match union {
                        None => ab,
                        Some(u) => [
                            u[0].min(ab[0]),
                            u[1].min(ab[1]),
                            u[2].max(ab[2]),
                            u[3].max(ab[3]),
                        ],
                    });
                    items.push((e.common().handle, ab));
                }
                None => unbounded.push(e.common().handle),
            }
        }
        let root = match union {
            Some(u) => {
                let w = (u[2] - u[0]).max(1.0);
                let h = (u[3] - u[1]).max(1.0);
                let mx = w * 0.01;
                let my = h * 0.01;
                [u[0] - mx, u[1] - my, u[2] + mx, u[3] + my]
            }
            None => [-1.0, -1.0, 1.0, 1.0],
        };
        let mut tree = pick::quadtree::QuadTree::new(root);
        for (h, ab) in items {
            tree.insert(h, ab);
        }

        *self.entity_index_cache.borrow_mut() = Some((
            self.geometry_epoch,
            EntityIndex {
                tree,
                unbounded_handles: unbounded,
            },
        ));
        std::cell::Ref::map(self.entity_index_cache.borrow(), |c| {
            &c.as_ref().unwrap().1
        })
    }

    /// Full tessellation pipeline for one entity.
    fn tessellate_one(&self, e: &EntityType) -> Vec<WireModel> {
        let bg = if self.current_layout == "Model" {
            self.bg_color
        } else {
            self.paper_bg_color
        };
        let anno = if self.current_layout == "Model" {
            self.annotation_scale
        } else {
            1.0
        };
        let blk_cache = self.block_cache_arc();
        // tessellate_one is used for one-off lookups (hit test, properties).
        // Skip culling here so the caller always gets the full geometry.
        tessellate_entity(
            &self.document,
            &self.selected,
            self.active_viewport,
            bg,
            anno,
            e,
            Some(&blk_cache),
            None,
            None,
        )
    }

    fn model_space_block_handle(&self) -> Handle {
        // Primary: Layout object's block_record (DWG reader sets this).
        if let Some(h) = self.document.objects.values().find_map(|obj| {
            if let ObjectType::Layout(l) = obj {
                if l.name == "Model" && !l.block_record.is_null() {
                    Some(l.block_record)
                } else {
                    None
                }
            } else {
                None
            }
        }) {
            return h;
        }
        // Fallback for DXF files: conventional block-record name.
        self.document
            .block_records
            .get("*Model_Space")
            .map(|br| br.handle)
            .unwrap_or(Handle::NULL)
    }

    /// Compute the axis-aligned bounding box of all model-space entities.
    /// Result is epoch-cached so repeated ZOOM E / auto-fit calls are O(1).
    pub fn model_space_extents(&self) -> Option<(glam::Vec3, glam::Vec3)> {
        {
            let cache = self.model_extents_cache.borrow();
            if let Some((epoch, ext)) = *cache {
                if epoch == self.geometry_epoch {
                    return ext;
                }
            }
        }
        let result = self.compute_model_space_extents();
        *self.model_extents_cache.borrow_mut() = Some((self.geometry_epoch, result));
        result
    }

    fn compute_model_space_extents(&self) -> Option<(glam::Vec3, glam::Vec3)> {
        let model_block = self.model_space_block_handle();
        if model_block.is_null() {
            return None;
        }
        let mut min = glam::Vec3::splat(f32::INFINITY);
        let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
        let mut any = false;

        // Prefer the already-computed wire AABB cache when available — avoids re-tessellating.
        if self.current_layout == "Model" {
            let cache = self.wire_cache.borrow();
            if let Some(((epoch, _cam_gen), ref arc)) = *cache {
                if epoch == self.geometry_epoch {
                    for wire in arc.iter() {
                        let [ax, ay, bx, by] = wire.aabb;
                        let lo = glam::Vec3::new(ax, ay, 0.0);
                        let hi = glam::Vec3::new(bx, by, 0.0);
                        // Reject the whole AABB unless every component is finite:
                        // rays/xlines carry an unbounded AABB, and checking only
                        // x let a vertical ray's infinite y poison the extents.
                        if lo.is_finite() && hi.is_finite() {
                            min = min.min(lo);
                            max = max.max(hi);
                            any = true;
                        }
                    }
                    // 3D solids render as meshes, not wires, so fold their
                    // XY AABBs in too — otherwise ZOOM EXTENTS ignores them.
                    for set in self.meshes.values() {
                        let [ax, ay, bx, by] = set.world_aabb;
                        let lo = glam::Vec3::new(ax, ay, 0.0);
                        let hi = glam::Vec3::new(bx, by, 0.0);
                        if lo.is_finite() && hi.is_finite() {
                            min = min.min(lo);
                            max = max.max(hi);
                            any = true;
                        }
                    }
                    return if any { Some((min, max)) } else { None };
                }
            }
        }

        // Fallback: tessellate (first call or paper-space context).
        // wire.key_vertices live in offset-rel coords (world_offset
        // already subtracted at tessellation time). Add it back so the
        // result matches Path 1 above and the caller's expectation —
        // callers (auto_fit_viewport) write the centroid directly to
        // `Viewport.view_target`, which is a WCS field; storing
        // offset-rel coords there silently double-subtracts world_offset
        // inside `camera_for_viewport` and points the viewport at the
        // wrong location on UTM-scale drawings.
        for entity in self.document.entities() {
            let c = entity.common();
            if c.owner_handle != model_block || c.invisible {
                continue;
            }
            for wire in self.tessellate_one(entity) {
                for &[x, y, z] in &wire.key_vertices {
                    let v = glam::Vec3::new(x as f32, y as f32, z as f32);
                    // Check finiteness *after* the f32 cast: a ray/xline endpoint
                    // is a huge-but-finite f64 that overflows to inf in f32, which
                    // the f64 `is_finite` test would have let through.
                    if v.is_finite() {
                        min = min.min(v);
                        max = max.max(v);
                        any = true;
                    }
                }
            }
        }
        // Same mesh inclusion for the tessellate fallback path.
        for set in self.meshes.values() {
            let [ax, ay, bx, by] = set.world_aabb;
            let lo = glam::Vec3::new(ax, ay, 0.0);
            let hi = glam::Vec3::new(bx, by, 0.0);
            if lo.is_finite() && hi.is_finite() {
                min = min.min(lo);
                max = max.max(hi);
                any = true;
            }
        }
        if any {
            return Some((min, max));
        }
        // Last-resort: the header's saved EXTMIN/EXTMAX. AutoCAD writes these
        // on save so opening a file gives ZOOM EXTENTS a useful answer before
        // the wire cache is built.
        const SANE_EXTENT: f64 = 1.0e16;
        let h = &self.document.header;
        let hmin = h.model_space_extents_min;
        let hmax = h.model_space_extents_max;
        if hmin.x < hmax.x
            && hmin.y < hmax.y
            && hmin.x.abs() < SANE_EXTENT
            && hmax.x.abs() < SANE_EXTENT
            && hmin.y.abs() < SANE_EXTENT
            && hmax.y.abs() < SANE_EXTENT
        {
            return Some((
                glam::Vec3::new(hmin.x as f32, hmin.y as f32, hmin.z as f32),
                glam::Vec3::new(hmax.x as f32, hmax.y as f32, hmax.z as f32),
            ));
        }
        None
    }

    /// Set a newly created viewport's `view_target` and `view_height` so that
    /// all model-space content is visible at a reasonable scale.
    pub fn auto_fit_viewport(&mut self, vp_handle: Handle) {
        let extents = self.model_space_extents();
        let (min, max) = match extents {
            Some(e) => e,
            None => return,
        };
        let center = (min + max) * 0.5;
        let content_w = (max.x - min.x).max(1e-3);
        let content_h = (max.y - min.y).max(1e-3);

        let vp = match self.document.get_entity_mut(vp_handle) {
            Some(acadrust::EntityType::Viewport(vp)) => vp,
            _ => return,
        };
        // Set the view target to the model-space centroid (XY plane, z=0).
        vp.view_target.x = center.x as f64;
        vp.view_target.y = center.y as f64;
        vp.view_target.z = 0.0;

        // Choose the scale that fits both dimensions with a small margin.
        let margin = 1.1_f64;
        let scale_w = vp.width / (content_w as f64 * margin);
        let scale_h = vp.height / (content_h as f64 * margin);
        let fit_scale = scale_w.min(scale_h).min(1000.0).max(1e-6);

        vp.custom_scale = fit_scale;
        vp.view_height = vp.height / fit_scale;
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

