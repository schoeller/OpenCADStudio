# Rendering Optimization Roadmap: Culling & Level of Detail

## Background

H7CAD renders all entities every frame regardless of camera position or zoom level. No spatial
acceleration, no LOD, no frustum culling. Every wire, hatch, mesh, and image is uploaded and
drawn unconditionally. This scales poorly for large drawings (100k+ entities) and dense 3D solids.

Current pipeline order:
1. Hatch fills → 2. Images → 3. Meshes → 4. Face3D fills → 5. Face3D edges →
6. Wires → 7. Wipeouts → 8. Selection overlay → 9. MSAA resolve → 10. Blit

---

## Phase 1 — Viewport Bounding-Box Culling

**Goal:** Skip CPU upload and draw calls for entities outside the camera view.

### 1.4 Scissor Rects (Partial — Hatch/Image still pending)

Viewport scissoring for wires already exists (`wire_pixel_scissors` / `WireModel.vp_scissor`).
Extend same logic to hatch and image passes so paper-space viewport content is clipped at the
GPU stage instead of overdrawn.

---

## Phase 2 — Spatial Acceleration Structure

**Goal:** O(log n) visibility queries instead of O(n) linear scan.

### 2.1 Quadtree (2D Documents)

Partition world space into a quadtree keyed by `Aabb2`. Build once on document load, update
incrementally on entity add/remove/modify.

```
QuadTree<EntityId>
  ├── query_rect(Aabb2) → Vec<EntityId>
  └── insert / remove / update
```

Store in `scene/mod.rs` alongside the entity cache. Invalidate only the nodes touched by a
modified entity.

### 2.2 Octree (3D Solids)

For `MeshModel` / `solid3d` entities, use an octree with `Aabb3` keys. Query returns candidate
mesh IDs for frustum culling.

### 2.3 Integration

Replace linear entity scans in `pipeline/mod.rs` render preparation with spatial queries:

```rust
let candidates = scene.quadtree.query_rect(view_aabb);
// only upload GPU buffers for candidates
```

**Estimated gain:** near-constant frame cost regardless of total entity count when zoomed in.

---

## Phase 3 — Level of Detail

**Goal:** Reduce geometric complexity when entities are small or far away.

### 3.3 Hatch LOD — Density Reduction

At low zoom, hatch line spacing appears smaller than 1px. Options:

- **Solid fill substitution:** replace hatch pattern with solid color below a density threshold.
- **Skip hatch entirely:** at very low zoom, hatches are not readable; skip the hatch pass.

Threshold: `hatch_spacing_px = hatch_spacing_world * px_per_unit`. If `< 2.0`, use solid fill.

### 3.4 Mesh LOD — Tessellation Resolution

`truck_tess.rs` / `solid3d_tess.rs` tessellate ACIS solids at a fixed tolerance. Switch to
multiple precomputed LOD meshes:

```rust
pub struct MeshModel {
    pub lod: [Option<GpuMesh>; 3],  // [high, mid, low]
}
```

| LOD | Tolerance | Use when projected diagonal |
|-----|-----------|------------------------------|
| 0   | 0.01 mm   | > 200 px                     |
| 1   | 0.5 mm    | 50–200 px                    |
| 2   | 5.0 mm    | < 50 px                      |

Build LOD 0 eagerly; build LOD 1 & 2 lazily in a background thread (already have
async tessellation pattern from `truck_tess.rs`).

---

## Phase 4 — GPU-Side Culling (Advanced)

**Goal:** Offload culling to the GPU; zero CPU cost for large entity counts.

### 4.1 Indirect Draw + Compute Cull

Convert per-entity draw calls to indirect draw calls (`draw_indirect` / `draw_indexed_indirect`).
Run a compute shader pre-pass that tests each entity's AABB against the frustum and writes
`DrawIndirectArgs` only for visible entities.

```wgsl
// cull.wgsl
@compute @workgroup_size(64)
fn cull_entities(@builtin(global_invocation_id) id: vec3<u32>) {
    let entity = entities[id.x];
    if frustum_test(entity.aabb) {
        // atomically append to indirect draw buffer
        let slot = atomicAdd(&draw_count, 1u);
        draw_args[slot] = entity.draw_args;
    }
}
```

Requires restructuring entity data into GPU-side storage buffers. High complexity; tackle after
Phases 1–3 prove insufficient.

### 4.2 Hierarchical Z-Buffer Occlusion Culling (3D Only)

For dense 3D solid scenes, use a Hi-Z buffer to cull occluded meshes:

1. Render depth-only pass for large opaque solids.
2. Downsample depth into mip chain (Hi-Z pyramid).
3. Compute shader tests each mesh AABB against Hi-Z; skips occluded meshes.

Relevant only for perspective (3D) mode with many overlapping solids.

---

## Implementation Order

```
Phase 1.4  Scissor for hatch/image          low complexity
Phase 2.1  Quadtree for 2D                  medium complexity, scales 2D docs
Phase 3.3  Hatch LOD                        low complexity
Phase 3.4  Mesh LOD                         high complexity, background thread
Phase 2.2  Octree for 3D                    medium, needed for dense 3D
Phase 4.1  Indirect draw + GPU cull         high complexity, defer
Phase 4.2  Hi-Z occlusion                   high complexity, 3D only, last
```

---

## Key Files to Modify

| File | Change |
|------|--------|
| `src/scene/mesh_model.rs` | Add `Aabb3`, LOD mesh array |
| `src/scene/hatch_model.rs` | Add `Aabb2` field |
| `src/scene/image_model.rs` | Add `Aabb2` field |
| `src/scene/truck_tess.rs` | Multi-resolution mesh tessellation |
| `src/scene/mod.rs` | Quadtree/octree; LOD epoch tracking |
| `src/scene/pipeline/mod.rs` | Cull hatch/image entities before upload loops |
| `src/shaders/cull.wgsl` | New — Phase 4 compute culling shader |

---

## Success Metrics

- **Phase 2 target:** Pan/zoom frame cost O(visible) not O(total).
- **Phase 3 target:** Complex solid scene at far zoom → mesh triangle count < 10% of full detail.
- **Phase 4 target:** GPU-cull overhead < 0.5 ms for 1M entity scene.

---

## TEMPORARY: Insert sub-entity count limit (revert when better fix lands)

`tessellate_entity` in `src/scene/mod.rs` has a guard that short-circuits any
Insert whose referenced block contains more than `INSERT_SUB_LIMIT` (currently
5,000) sub-entities. The Insert is rendered as an insertion marker only — no
explode, no sub tessellation.

**Why this exists:** A real-world DWG was found where an xref block contained
~74k sub-entities. Opening the file froze the UI on the first render because
`tessellate_entity` exploded the block and serially style-resolved every
sub-entity (and any single bad sub could hang tessellation forever). The guard
made the file openable.

**Why it should go away:** The limit hides legitimate geometry. Large xrefs are
common in civil/infrastructure drawings — the user shouldn't have to see them
as crosses.

**Proper fixes that would let us drop this guard:**
1. ~~Per-block tessellation cache~~ — **LANDED** (`def72e4`): block defns are
   tessellated once and per-Insert reuses the cached wires via
   `block_cache::expand_insert`. The guard now only fires on the legacy
   fallback path; consider lowering or removing it.
2. Lazy / background Insert tessellation — show marker first, fill in detail
   on a background thread, redraw when ready.
3. Per-sub-entity hardening — find why a single sub can hang tessellation
   (likely Spline NURBS evaluation or malformed boundary edge) and fix the
   root cause; combine with sub-count budget per frame instead of a hard cap.
4. xref attach metadata respected — DWG carries an "unloaded" state for xref
   blocks; honour it instead of always inlining xref content on open.
   (Attempted in `e88a946`, reverted in `0996b75` — the on-disk bit was
   unreliable as a Loaded/Unloaded signal.)

When any of (2)–(3) lands, delete the `INSERT_SUB_LIMIT` block in
`tessellate_entity` (search for `INSERT_SUB_LIMIT`).

Related companion fix in `src/app/update.rs`: after `resolve_xrefs`, a second
`purge_corrupt_entities` pass runs against the newly inlined xref content.
That one is not a hack — it should stay even after the Insert guard goes.
