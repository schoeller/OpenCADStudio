// Phase 4-B — batched hatch shader. All hatches in one draw call;
// per-instance data fetched from storage buffers indexed by the
// `instance_index` vertex attribute (passed from the per-vertex
// (local_xy, instance_index) stream of tessellated boundary-triangle
// positions so we don't depend on @builtin(instance_index) edge cases
// across backends).
//
// Layout — matches `hatch_gpu.rs`:
//   group 1 binding 0  InstanceBuffer  HatchInstance[]   (128 B / inst)
//   group 1 binding 1  BoundaryBuffer  vec4<f32>[]       (xy in .xy)
//   group 1 binding 2  FamilyBuffer    LineFamilyGpu[]   (48 B / fam)
//   group 1 binding 3  DashBuffer      f32[]
//
// The vertex shader emits tessellated boundary triangles in local space
// (an instance whose boundary failed to tessellate falls back to an
// AABB quad with `poly_test == 1`, so the fragment shader still runs
// `in_polygon` to clip it to the real shape). A `visible == 0` instance
// gets an out-of-NDC clip position so the fragment shader never runs
// for it — that's the GPU-side cull (Phase 4-B equivalent of
// `compute_hatch_lod` writing `hatch_skip_flags`).

// ── Group 0: shared frame uniforms (matches hatch.wgsl) ──────────────────

struct Uniforms {
    viewport_size:       vec2<f32>,
    world_per_pixel:     f32,
    lwdisplay_enable:    f32,
    flat_shade:          f32,
    transparency_enable: f32,
    _pad:                vec2<f32>,
    // Relative-to-eye (double-single): see wire.wgsl.
    view_rot:            mat4x4<f32>,
    eye_high:            vec3<f32>,
    _pad_eh:             f32,
    eye_low:             vec3<f32>,
    _pad_el:             f32,
}
@group(0) @binding(0) var<uniform> u: Uniforms;

// ── Group 1: batched hatch storage ───────────────────────────────────────

struct HatchInstance {
    color:           vec4<f32>,
    color2:          vec4<f32>,
    aabb:            vec4<f32>,   // (xmin, ymin, xmax, ymax) — local space
    world_origin:    vec2<f32>,     // anchor high half
    world_origin_low: vec2<f32>,    // anchor low residual (double-single)
    angle_offset:    f32,
    scale:           f32,
    grad_cos:        f32,
    grad_sin:        f32,
    grad_min:        f32,
    grad_range:      f32,
    mode:            u32,         // 0=pattern, 1=solid, 2=gradient
    visible:         u32,         // 0 = skip (CPU writes via compute_hatch_lod)
    boundary_offset: u32,
    boundary_count:  u32,
    family_offset:   u32,
    family_count:    u32,
    draw_depth:      f32,          // signed (-1,1) draw-order bias; 0 = neutral
    poly_test:       u32,         // 1 = run in_polygon (fallback), 0 = skip
    grad_kind:       u32,         // shape (0=linear,1=cyl,2=sph,3=hemi,4=curved), bit4=invert
    _pad2:           u32,
}

// Draw-order depth bias (see wire.wgsl). Higher draw_depth → smaller z →
// drawn on top, ordering this fill against other entity types.
const DRAW_ORDER_BIAS: f32 = 0.001;

struct LineFamily {
    cos_a:       f32,
    sin_a:       f32,
    x0:          f32,
    y0:          f32,
    dx:          f32,
    dy:          f32,
    perp_step:   f32,
    along_step:  f32,
    line_width:  f32,
    period:      f32,
    n_dashes:    u32,
    dash_offset: u32,
}

@group(1) @binding(0) var<storage, read> instances:  array<HatchInstance>;
@group(1) @binding(1) var<storage, read> boundary:   array<vec4<f32>>;
@group(1) @binding(2) var<storage, read> families:   array<LineFamily>;
@group(1) @binding(3) var<storage, read> dashes:     array<f32>;
// Per-instance visibility (Phase 4-B sub-pixel + frustum skip).
// CPU writes `1` to draw / `0` to skip every frame; vertex shader
// emits an out-of-NDC clip position for 0-instances so the GPU
// rasterizer culls the primitive before any fragment runs.
@group(1) @binding(4) var<storage, read> visibility: array<u32>;

// ── Vertex shader ────────────────────────────────────────────────────────

struct VIn {
    @location(0) local_xy:       vec2<f32>,
    @location(1) instance_index: u32,
}

struct VOut {
    @builtin(position) clip:           vec4<f32>,
    @location(0)       xz:             vec2<f32>,
    @location(1) @interpolate(flat) instance_index: u32,
}

@vertex fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    let inst = instances[v.instance_index];

    // Per-frame visibility (CPU-driven sub-pixel + frustum skip).
    // 0 → emit a clip position whose x/y exceed |w| so the GPU
    // frustum-culls the primitive and no fragment runs. (WGSL
    // forbids literal NaN so this out-of-NDC trick replaces the
    // usual NaN-degenerate-triangle.)
    if visibility[v.instance_index] == 0u {
        o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        o.xz = vec2<f32>(0.0, 0.0);
        o.instance_index = v.instance_index;
        return o;
    }

    let local = v.local_xy;
    // Double-single relative-to-eye: the anchor high half cancels exactly
    // against eye_high (Sterbenz); local + anchor low + (−eye_low) carry the
    // residual. `local` is small (boundary-relative), so adding it in the low
    // term keeps full precision at UTM-scale anchors.
    let hi = vec3<f32>(inst.world_origin.x - u.eye_high.x,
                       inst.world_origin.y - u.eye_high.y,
                       -u.eye_high.z);
    let lo = vec3<f32>(local.x + inst.world_origin_low.x - u.eye_low.x,
                       local.y + inst.world_origin_low.y - u.eye_low.y,
                       -u.eye_low.z);
    o.clip = u.view_rot * vec4<f32>(hi + lo, 1.0);
    o.clip.z = o.clip.z - inst.draw_depth * DRAW_ORDER_BIAS * o.clip.w;
    o.xz = local;
    o.instance_index = v.instance_index;
    return o;
}

// ── Point-in-polygon (ray casting) over a sub-range of BoundaryBuffer ────

fn valid_vertex(p: vec2<f32>) -> bool {
    return p.x == p.x && p.y == p.y;
}

fn edge_crosses(p: vec2<f32>, a: vec2<f32>, c: vec2<f32>) -> bool {
    if (a.y > p.y) != (c.y > p.y) {
        let x_int = (c.x - a.x) * (p.y - a.y) / (c.y - a.y) + a.x;
        return p.x < x_int;
    }
    return false;
}

fn in_polygon(p: vec2<f32>, offset: u32, count: u32) -> bool {
    var inside = false;
    var prev = vec2<f32>(0.0, 0.0);
    var first = vec2<f32>(0.0, 0.0);
    var have_prev = false;
    for (var i = 0u; i < count; i++) {
        let vi = boundary[offset + i].xy;
        if !valid_vertex(vi) {
            // Close the sub-loop that just ended (last → first edge). An
            // unclosed boundary — e.g. a SOLID's 4 corners, which are not
            // repeated — otherwise miscounts crossings and the fill bleeds
            // outside the shape. (#140)
            if have_prev && edge_crosses(p, prev, first) {
                inside = !inside;
            }
            have_prev = false;
            continue;
        }
        if have_prev {
            if edge_crosses(p, prev, vi) {
                inside = !inside;
            }
        } else {
            first = vi;
        }
        prev = vi;
        have_prev = true;
    }
    if have_prev && edge_crosses(p, prev, first) {
        inside = !inside;
    }
    return inside;
}

// ── Per-family hatch test (same math as hatch.wgsl, dashes from
// global DashBuffer instead of per-hatch FamilyBatch) ────────────────────

fn check_family(
    xz:      vec2<f32>,
    fam:     LineFamily,
    cos_off: f32,
    sin_off: f32,
    scale:   f32,
) -> bool {
    let cos_a = fam.cos_a * cos_off - fam.sin_a * sin_off;
    let sin_a = fam.sin_a * cos_off + fam.cos_a * sin_off;

    let ox = (fam.x0 * cos_off - fam.y0 * sin_off) * scale;
    let oz = (fam.x0 * sin_off + fam.y0 * cos_off) * scale;

    let px = xz.x - ox;
    let pz = xz.y - oz;

    let perp_step = fam.perp_step * scale;
    let line_w    = abs(fam.line_width * scale);

    let perp   = -px * sin_a + pz * cos_a;
    let k      = round(perp / perp_step);
    let dperp  = perp - k * perp_step;
    let d      = abs(dperp);
    let half_px = length(vec2<f32>(dpdx(perp), dpdy(perp))) * 0.5;

    // World units per screen pixel on each axis — used to light exactly the
    // one pixel that contains a dot's centre (pixel-snapped, so the dot stays
    // a steady single pixel at any pattern angle instead of flickering).
    let wpx = length(vec2<f32>(dpdx(xz.x), dpdy(xz.x)));
    let wpy = length(vec2<f32>(dpdx(xz.y), dpdy(xz.y)));

    // A fragment within ~1px of a line may be a dot; everything further out is
    // empty fill. (A dot's pixel sits on a line, so its perp offset is < 1px.)
    if d > half_px * 2.0 { return false; }
    if fam.n_dashes == 0u { return d <= half_px; }

    let along_step = fam.along_step * scale;
    let period     = fam.period * scale;
    let along      = px * cos_a + pz * sin_a;
    let t          = along - k * along_step;
    let t_mod      = ((t % period) + period) % period;

    var pos = 0.0;
    for (var j = 0u; j < fam.n_dashes; j++) {
        let sv = dashes[fam.dash_offset + j] * scale;
        if sv > 0.0 {
            if d <= half_px && t_mod >= pos && t_mod < pos + sv { return true; }
            pos = pos + sv;
        } else if sv < 0.0 {
            pos = pos - sv;
        } else {
            // Dot: signed distance to its lattice centre (along the line and
            // across lines), rotated back to world, then snapped to the pixel
            // grid. The dot grid rotates with the pattern; the lit pixel does
            // not, so it never thins/flickers.
            let dtv = (t - pos) - round((t - pos) / period) * period;
            let owx = -dtv * cos_a + dperp * sin_a;
            let owy = -dtv * sin_a - dperp * cos_a;
            if abs(owx / wpx) <= 0.5 && abs(owy / wpy) <= 0.5 { return true; }
        }
    }
    return false;
}

// ── Fragment shader ──────────────────────────────────────────────────────

@fragment fn fs_main(v: VOut) -> @location(0) vec4<f32> {
    let inst = instances[v.instance_index];

    // 1. Boundary test — only on the fallback path (poly_test==1). On the
    //    tessellated fast path the triangles already bound the fill.
    if inst.poly_test == 1u {
        if !in_polygon(v.xz, inst.boundary_offset, inst.boundary_count) {
            discard;
        }
    }

    // 2. Mode dispatch.
    if inst.mode == 1u {
        return inst.color;
    } else if inst.mode == 2u {
        let proj = v.xz.x * inst.grad_cos + v.xz.y * inst.grad_sin;
        var t = clamp((proj - inst.grad_min) / inst.grad_range, 0.0, 1.0);
        // Shape profile: cylinder mirrors around the middle, curved eases in.
        let k = inst.grad_kind & 15u;
        if k == 1u {
            t = 1.0 - abs(2.0 * t - 1.0);
        } else if k == 4u {
            t = t * t;
        }
        if (inst.grad_kind & 16u) != 0u {
            t = 1.0 - t;
        }
        return mix(inst.color, inst.color2, t);
    } else if inst.mode == 3u {
        // Radial gradient: centre is (grad_cos, grad_sin), radius is grad_range.
        let d = length(v.xz - vec2<f32>(inst.grad_cos, inst.grad_sin));
        var t = clamp(d / inst.grad_range, 0.0, 1.0);
        // Hemispherical shades faster near the centre (dome profile).
        if (inst.grad_kind & 15u) == 3u {
            t = sqrt(t);
        }
        if (inst.grad_kind & 16u) != 0u {
            t = 1.0 - t;
        }
        return mix(inst.color, inst.color2, t);
    }

    // 3. Pattern LOD: when the densest family's spacing projects below
    //    2 px, lines blur into a solid fill — return color instead of
    //    iterating every family (mirrors Phase 3.3 LOD in hatch.wgsl).
    if u.world_per_pixel > 0.0 && inst.family_count > 0u {
        var min_spacing_world: f32 = 1.0e30;
        for (var i = 0u; i < inst.family_count; i++) {
            let s = abs(families[inst.family_offset + i].perp_step) * inst.scale;
            if s > 0.0 && s < min_spacing_world {
                min_spacing_world = s;
            }
        }
        if min_spacing_world / u.world_per_pixel < 2.0 {
            return inst.color;
        }
    }

    // 4. Pattern evaluation.
    let cos_off = cos(inst.angle_offset);
    let sin_off = sin(inst.angle_offset);
    for (var i = 0u; i < inst.family_count; i++) {
        if check_family(v.xz, families[inst.family_offset + i], cos_off, sin_off, inst.scale) {
            return inst.color;
        }
    }
    discard;
    // Unreachable: `discard` kills the fragment before this runs, but
    // DX12/FXC reports E_FAIL X3507 ("not all control paths return a
    // value") without an explicit return after every terminal discard.
    return vec4<f32>(0.0);
}
