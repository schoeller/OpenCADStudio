// Persistent per-entity wire instance arena (native, behind OCS_WIRE_GPU_PATCH).
//
// The normal wire path re-emits EVERY wire into a fresh instance buffer whenever
// the resident set's content id changes — so any edit on a drawing whose wires
// expand to millions of segments re-uploads the whole (hundreds-of-MB) buffer,
// which stalls for ~1s on a shared-memory GPU. This arena instead keeps one
// persistent instance buffer (plus its shared WireConst storage) laid out as
// per-entity *slabs*, so an edit only writes what changed:
//
//   * Modify in place — a move / rotate / scale / colour change keeps the
//     entity's segment count, so its slab is overwritten where it sits.
//   * Add / a Modify whose segment count changed — bump-allocate a fresh slab at
//     the tail (tombstone the old one); the instance buffer only grows by that
//     entity.
//   * Erase — tombstone the slab (blank instances that render nothing).
//
// Two correctness points make add/remove safe:
//   * draw_depth_map normalises each entity's draw-order z-bias by the block's
//     entity count, so ANY add/remove re-scales EVERY entity's bias. We keep a
//     CPU mirror of the WireConst array and, on a structural change, refresh every
//     slab's draw_depth and re-upload the (small, ~1 MB) const buffer — the huge
//     instance buffer is untouched.
//   * A tail-appended entity draws last, which only mis-orders alpha-blended /
//     coincident wires. So when the set contains ANY transparent wire we bail to a
//     full rebuild instead of appending. Opaque overlap resolves by the z-bias, so
//     it is order-independent and safe to relocate.
//
// A tombstoned instance points at const slot 0 (a blank WireConst, half_width 0),
// so the shader expands it to a zero-area quad — no pixels. When tombstone waste
// or capacity is exceeded, `patch` returns false and the caller compacts via a
// full rebuild. Because a full rebuild is always the fallback, correctness never
// rides on the fast path.
//
// Scope: a SINGLE batch — the set must have no mesh-edge fills (which force the
// draw-order-preserving multi-batch split) and no per-wire scissor (paper content
// viewports). Mixed 2D/3D or scissored sets fall back to the batched path.

#![cfg(not(target_arch = "wasm32"))]

use super::wire_gpu::{emit_wire_native, wire_draw_depth, WireConst, WireGpu, WireInstance};
use crate::scene::model::wire_model::WireModel;
use crate::scene::ChangeKind;
use acadrust::Handle;
use iced::wgpu;
use rustc_hash::FxHashMap;

/// Spare capacity multiplier when (re)allocating, so a run of adds appends
/// without reallocating each time.
const HEADROOM_NUM: u64 = 3;
const HEADROOM_DEN: u64 = 2;
const MIN_INST_CAP: u64 = 4096;
const MIN_CONST_CAP: u64 = 1024;
/// wgpu caps a single buffer at 256 MB. An arena keeps ONE instance buffer per
/// batch, so a batch whose instances exceed this can't be an arena — build
/// returns None and the caller falls back to the chunked batched path. The
/// buffer (with headroom) is also clamped here so it never exceeds the limit.
const MAX_INSTANCES: u64 = 268_435_456 / std::mem::size_of::<WireInstance>() as u64;
const MAX_CONSTS: u64 = 268_435_456 / std::mem::size_of::<WireConst>() as u64;

struct Slab {
    inst_off: u32,
    inst_len: u32,
    const_off: u32,
    const_len: u32,
}

pub struct WireArena {
    inst_buf: wgpu::Buffer,
    inst_cap: u32,
    inst_tail: u32,
    const_buf: wgpu::Buffer,
    const_bind_group: std::sync::Arc<wgpu::BindGroup>,
    const_cap: u32,
    const_tail: u32,
    /// CPU mirror of the const buffer so a structural edit can refresh every
    /// slab's draw_depth (denominator change) without re-emitting geometry.
    consts_cpu: Vec<WireConst>,
    slabs: FxHashMap<Handle, Slab>,
    /// Tombstoned instances (blanked, not reclaimed) — past half the tail a patch
    /// bails so the caller compacts with a full rebuild.
    tombstoned: u32,
    /// Whether this arena's wires are 3D mesh/solid edges (`is_3d_mesh_edge` on
    /// the draw batch): the draw loop hides them in clean-shaded modes and draws
    /// them black in filled-with-edges modes. The regular and mesh-edge subsets
    /// of the resident set each get their own arena so both patch incrementally.
    mesh_edge: bool,
}

fn handle_of(w: &WireModel) -> Option<Handle> {
    crate::scene::Scene::handle_from_wire_name(&w.name)
}

/// True when `w`'s edge segments belong to a mesh/solid whose fill is drawn in a
/// separate pass — such wires need the draw loop's mesh-edge treatment, so they
/// go in their own arena. `mesh_names` are the entities that emit a fill.
pub fn is_mesh_edge(w: &WireModel, mesh_names: &rustc_hash::FxHashSet<u64>) -> bool {
    !w.points.is_empty() && handle_of(w).map_or(false, |h| mesh_names.contains(&h.value()))
}

/// True when appending a new entity at the tail could change the image, so the
/// arena must fall back to a full rebuild instead of relocating a slab. Two
/// cases where draw order (not the z-bias) decides the winning pixel:
///   * transparency — alpha blends in submission order;
///   * a wire with NO draw-order depth — 3D solids (Solid3D / Region / Body /
///     Surface) are excluded from `draw_depth_map`, so their fallback edge wires
///     get draw_depth 0.0. Two coincident opaque such wires share a z-bias and
///     resolve by submission order, which a tail relocation would flip.
fn append_unsafe(wires: &[&WireModel], depth_map: &FxHashMap<u64, f32>) -> bool {
    wires.iter().any(|w| {
        w.color[3] < 0.999
            || handle_of(w).map_or(true, |h| !depth_map.contains_key(&h.value()))
    })
}

/// handle → wire-slot index for the selection / text-highlight overlays, built
/// from the resident Vec (independent of the arena's slab layout).
pub fn build_handle_index(wires: &[WireModel]) -> std::sync::Arc<FxHashMap<u64, Vec<u32>>> {
    let mut index: FxHashMap<u64, Vec<u32>> = FxHashMap::default();
    index.reserve(wires.len());
    for (idx, w) in wires.iter().enumerate() {
        if let Ok(h) = w.name.parse::<u64>() {
            index.entry(h).or_default().push(idx as u32);
        }
    }
    std::sync::Arc::new(index)
}

/// Group `wires` (draw-order sorted, entity-contiguous) into per-handle ranges.
fn handle_ranges(wires: &[&WireModel]) -> Option<Vec<(Handle, usize, usize)>> {
    let mut out: Vec<(Handle, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < wires.len() {
        let h = handle_of(wires[i])?;
        let mut j = i + 1;
        while j < wires.len() && handle_of(wires[j]) == Some(h) {
            j += 1;
        }
        out.push((h, i, j));
        i = j;
    }
    Some(out)
}

fn make_const_bg(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
) -> std::sync::Arc<wgpu::BindGroup> {
    std::sync::Arc::new(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("wire_arena.const.bg"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    }))
}

fn alloc_inst(device: &wgpu::Device, cap: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wire_arena.ibuf"),
        size: cap * std::mem::size_of::<WireInstance>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn alloc_const(device: &wgpu::Device, cap: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wire_arena.cbuf"),
        size: cap * std::mem::size_of::<WireConst>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn blank_const() -> WireConst {
    <WireConst as bytemuck::Zeroable>::zeroed()
}

/// A blank instance: zero-length segment at const slot 0 (half_width 0) — the
/// shader expands it to a zero-area quad, so it rasterises nothing.
fn blank_instance() -> WireInstance {
    WireInstance {
        pos_a: [0.0; 3],
        pos_a_low: [0.0; 3],
        pos_b: [0.0; 3],
        pos_b_low: [0.0; 3],
        distance_a: 0.0,
        distance_b: 0.0,
        wire_id: 0,
        world_hw_a: 0.0,
        world_hw_b: 0.0,
    }
}

impl WireArena {
    /// Build a fresh arena from the full resident set, or `None` if it isn't a
    /// single scissor-free batch or a wire is unnamed (caller keeps the batched
    /// path).
    pub fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        wires: &[&WireModel],
        depth_map: &FxHashMap<u64, f32>,
        const_bgl: &wgpu::BindGroupLayout,
        mesh_edge: bool,
    ) -> Option<Self> {
        let ranges = handle_ranges(wires)?;

        // const slot 0 = blank tombstone target.
        let mut instances: Vec<WireInstance> = Vec::new();
        let mut consts_cpu: Vec<WireConst> = vec![blank_const()];
        let mut slabs: FxHashMap<Handle, Slab> = FxHashMap::default();
        for (h, i, j) in ranges {
            let inst_off = instances.len() as u32;
            let const_off = consts_cpu.len() as u32;
            for &w in &wires[i..j] {
                let wire_id = consts_cpu.len() as u32;
                // 3D mesh outline edges are occluded by true depth and must NOT
                // take the draw-order z-bias (or hidden back edges peek through
                // the shaded fill) — matching WireGpu::from_run.
                let dd = if mesh_edge { 0.0 } else { wire_draw_depth(w, depth_map) };
                let (mut insts, cst) = emit_wire_native(w, wire_id, w.color, dd);
                instances.append(&mut insts);
                consts_cpu.push(cst);
            }
            slabs.insert(
                h,
                Slab {
                    inst_off,
                    inst_len: instances.len() as u32 - inst_off,
                    const_off,
                    const_len: consts_cpu.len() as u32 - const_off,
                },
            );
        }

        let inst_tail = instances.len() as u32;
        let const_tail = consts_cpu.len() as u32;
        // A batch bigger than one buffer can't be an arena — let the caller chunk
        // it via the batched path.
        if inst_tail as u64 > MAX_INSTANCES || const_tail as u64 > MAX_CONSTS {
            return None;
        }
        let inst_cap = ((inst_tail as u64 * HEADROOM_NUM / HEADROOM_DEN)
            .max(MIN_INST_CAP)
            .min(MAX_INSTANCES)) as u32;
        let const_cap = ((const_tail as u64 * HEADROOM_NUM / HEADROOM_DEN)
            .max(MIN_CONST_CAP)
            .min(MAX_CONSTS)) as u32;
        let inst_buf = alloc_inst(device, inst_cap as u64);
        let const_buf = alloc_const(device, const_cap as u64);
        if inst_tail > 0 {
            queue.write_buffer(&inst_buf, 0, bytemuck::cast_slice(&instances));
        }
        queue.write_buffer(&const_buf, 0, bytemuck::cast_slice(&consts_cpu));
        let const_bind_group = make_const_bg(device, const_bgl, &const_buf);

        Some(Self {
            inst_buf,
            inst_cap,
            inst_tail,
            const_buf,
            const_bind_group,
            const_cap,
            const_tail,
            consts_cpu,
            slabs,
            tombstoned: 0,
            mesh_edge,
        })
    }

    fn write_insts(&self, queue: &wgpu::Queue, off: u32, data: &[WireInstance]) {
        if data.is_empty() {
            return;
        }
        let sz = std::mem::size_of::<WireInstance>() as u64;
        queue.write_buffer(&self.inst_buf, off as u64 * sz, bytemuck::cast_slice(data));
    }

    /// Apply the changed handles in place; returns false (⇒ full rebuild) when the
    /// arena can't absorb the change: not eligible, a transparent append, a
    /// capacity overflow, or too much tombstone waste.
    pub fn patch(
        &mut self,
        queue: &wgpu::Queue,
        changes: &[(Handle, ChangeKind)],
        wires: &[&WireModel],
        depth_map: &FxHashMap<u64, f32>,
    ) -> bool {
        let Some(ranges) = handle_ranges(wires) else {
            return false;
        };
        let range_of: FxHashMap<Handle, (usize, usize)> =
            ranges.into_iter().map(|(h, i, j)| (h, (i, j))).collect();
        let append_unsafe = append_unsafe(wires, depth_map);

        let mut structural = false;
        for &(h, kind) in changes {
            let new_range = range_of.get(&h).copied();

            // Removed / now-hidden ⇒ tombstone the slab. A handle not in THIS
            // arena's subset (it belongs to the other batch) simply isn't in its
            // slabs, so this is a no-op for it.
            if matches!(kind, ChangeKind::Removed) || new_range.is_none() {
                if let Some(slab) = self.slabs.remove(&h) {
                    let blanks = vec![blank_instance(); slab.inst_len as usize];
                    self.write_insts(queue, slab.inst_off, &blanks);
                    self.tombstoned += slab.inst_len;
                    structural = true;
                }
                continue;
            }
            let (i, j) = new_range.unwrap();
            let run = &wires[i..j];

            // Emit into fresh, run-local const slots (patched to absolute below).
            let mut insts: Vec<WireInstance> = Vec::new();
            let mut csts: Vec<WireConst> = Vec::new();
            for &w in run {
                let wire_id = csts.len() as u32;
                let dd = if self.mesh_edge { 0.0 } else { wire_draw_depth(w, depth_map) };
                let (mut wi, c) = emit_wire_native(w, wire_id, w.color, dd);
                insts.append(&mut wi);
                csts.push(c);
            }
            let inst_len = insts.len() as u32;
            let const_len = csts.len() as u32;

            let in_place = self
                .slabs
                .get(&h)
                .map(|s| s.inst_len == inst_len && s.const_len == const_len)
                .unwrap_or(false);

            if in_place {
                let (inst_off, const_off) = {
                    let s = self.slabs.get(&h).unwrap();
                    (s.inst_off, s.const_off)
                };
                for w in insts.iter_mut() {
                    w.wire_id += const_off;
                }
                self.write_insts(queue, inst_off, &insts);
                for (k, c) in csts.iter().enumerate() {
                    self.consts_cpu[const_off as usize + k] = *c;
                }
                // Push the entity's consts to the GPU too — an in-place edit is
                // NOT structural, so the whole-buffer refresh below won't run and
                // a colour change would otherwise never reach the shader.
                let csz = std::mem::size_of::<WireConst>() as u64;
                queue.write_buffer(
                    &self.const_buf,
                    const_off as u64 * csz,
                    bytemuck::cast_slice(&csts),
                );
                continue;
            }

            // Layout changed ⇒ append at the tail. Unsafe to relocate when the set
            // resolves overlap by submission order: transparency, a wire with no
            // draw-order depth, or the mesh-edge arena (all its wires are forced
            // to depth 0, so coincident edges resolve by submission order). Fall
            // back to a full rebuild instead.
            if append_unsafe || self.mesh_edge {
                return false;
            }
            if self.inst_tail + inst_len > self.inst_cap
                || self.const_tail + const_len > self.const_cap
            {
                return false;
            }
            structural = true;
            if let Some(s) = self.slabs.remove(&h) {
                let blanks = vec![blank_instance(); s.inst_len as usize];
                self.write_insts(queue, s.inst_off, &blanks);
                self.tombstoned += s.inst_len;
            }
            let inst_off = self.inst_tail;
            let const_off = self.const_tail;
            for w in insts.iter_mut() {
                w.wire_id += const_off;
            }
            self.write_insts(queue, inst_off, &insts);
            for c in &csts {
                self.consts_cpu.push(*c);
            }
            self.inst_tail += inst_len;
            self.const_tail += const_len;
            self.slabs.insert(
                h,
                Slab {
                    inst_off,
                    inst_len,
                    const_off,
                    const_len,
                },
            );
        }

        if structural {
            // The entity count changed ⇒ draw_depth_map re-normalised every
            // entity's z-bias. Refresh each live slab's draw_depth from the new
            // depth map and re-upload the whole (small) const buffer; the instance
            // buffer is untouched.
            for (h, slab) in &self.slabs {
                // Mesh-edge wires keep depth 0 (no draw-order bias); regular wires
                // take the re-normalised map value.
                let dd = if self.mesh_edge {
                    0.0
                } else {
                    depth_map.get(&h.value()).copied().unwrap_or(0.0)
                };
                for k in 0..slab.const_len {
                    self.consts_cpu[(slab.const_off + k) as usize].draw_depth = dd;
                }
            }
            queue.write_buffer(&self.const_buf, 0, bytemuck::cast_slice(&self.consts_cpu));
            if self.tombstoned > self.inst_tail / 2 {
                return false;
            }
        }
        true
    }

    /// One draw batch wrapping the persistent instance buffer. `instance_count`
    /// is the whole tail (tombstones included — they draw nothing).
    pub fn wire_gpus(&self) -> Vec<WireGpu> {
        if self.inst_tail == 0 {
            return vec![];
        }
        vec![WireGpu {
            instance_buffer: self.inst_buf.clone(),
            instance_count: self.inst_tail,
            is_3d_mesh_edge: self.mesh_edge,
            const_bind_group: Some(self.const_bind_group.clone()),
        }]
    }
}
