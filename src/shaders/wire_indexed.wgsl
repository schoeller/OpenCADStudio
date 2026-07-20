// Wire shader (native) — same as wire.wgsl, but the per-wire constants
// (color / half_width / dash pattern / draw_depth) live in a per-wire storage
// buffer indexed by `wire_id` instead of being replicated on every segment
// instance. Cuts the instance from 104 B to 60 B and removes the redundant
// per-segment re-fetch of constants. WebGL2 has no vertex-stage storage
// buffers, so the wasm build uses wire.wgsl (fat instance) instead.

struct Uniforms {
    viewport_size:    vec2<f32>,
    world_per_pixel:  f32,
    lwdisplay_enable: f32,
    flat_shade: f32,
    transparency_enable: f32,
    _pad: vec2<f32>,
    view_rot:         mat4x4<f32>,
    eye_high:         vec3<f32>,
    _pad_eh:          f32,
    eye_low:          vec3<f32>,
    _pad_el:          f32,
}
@group(0) @binding(0) var<uniform> u: Uniforms;

// Per-wire constants (std430). Must match `WireConst` in wire_gpu.rs.
struct WireConst {
    color:          vec4<f32>,
    pat0:           vec4<f32>,
    pat1:           vec4<f32>,
    half_width:     f32,
    pattern_length: f32,
    draw_depth:     f32,
    align_end:      f32,
    align_total:    f32,
    world_half_width: f32,
    _pad1:          f32,
    _pad2:          f32,
}
@group(1) @binding(0) var<storage, read> wire_consts: array<WireConst>;

struct InstanceIn {
    @location(0) pos_a:      vec3<f32>,
    @location(1) pos_b:      vec3<f32>,
    @location(2) pos_a_low:  vec3<f32>,
    @location(3) pos_b_low:  vec3<f32>,
    @location(4) distance_a: f32,
    @location(5) distance_b: f32,
    @location(6) wire_id:    u32,
}

const DRAW_ORDER_BIAS: f32 = 0.001;

struct VertexOut {
    @builtin(position)              clip_pos:       vec4<f32>,
    @location(0)                    color:          vec4<f32>,
    @location(1)                    distance:       f32,
    @location(2)                    pattern_length: f32,
    @location(3)                    pat0:           vec4<f32>,
    @location(4)                    pat1:           vec4<f32>,
    @location(5) @interpolate(flat) min_elem:       f32,
    @location(6) @interpolate(flat) align_end:      f32,
    @location(7) @interpolate(flat) align_total:    f32,
}

@vertex fn vs_main(@builtin(vertex_index) vid: u32, in: InstanceIn) -> VertexOut {
    let c = wire_consts[in.wire_id];

    let which_end_arr = array<f32, 6>(0.0, 1.0, 1.0, 0.0, 1.0, 0.0);
    let side_arr      = array<f32, 6>(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0);
    let which_end = which_end_arr[vid];
    let side      = side_arr[vid];

    let rel_a = (in.pos_a - u.eye_high) + (in.pos_a_low - u.eye_low);
    let rel_b = (in.pos_b - u.eye_high) + (in.pos_b_low - u.eye_low);
    let clip_a = u.view_rot * vec4<f32>(rel_a, 1.0);
    let clip_b = u.view_rot * vec4<f32>(rel_b, 1.0);

    let ndc_a = clip_a.xy / clip_a.w;
    let ndc_b = clip_b.xy / clip_b.w;

    let screen_a = ndc_a * u.viewport_size * 0.5;
    let screen_b = ndc_b * u.viewport_size * 0.5;

    let seg = screen_b - screen_a;
    let seg_len = length(seg);
    var perp: vec2<f32>;
    if seg_len > 1e-4 {
        let dir = seg / seg_len;
        perp = vec2<f32>(-dir.y, dir.x);
    } else {
        perp = vec2<f32>(0.0, 1.0);
    }

    let perp_ndc = perp / (u.viewport_size * 0.5);

    let clip_pos = mix(clip_a, clip_b, which_end);

    // A wide polyline carries its band width in world units: expand the quad
    // by `world_half_width / world_per_pixel` (screen pixels) so the band grows
    // and shrinks with zoom. A normal wire (world_half_width == 0) uses the
    // screen-pixel half-width, honouring the LWDISPLAY toggle.
    var hw: f32;
    if c.world_half_width > 0.0 {
        // Clamp to a half-pixel so a zoomed-out band never drops below a
        // hairline (its centre-line) instead of vanishing.
        hw = max(c.world_half_width / u.world_per_pixel, 0.5);
    } else {
        hw = select(0.5, c.half_width, u.lwdisplay_enable > 0.5);
    }

    let ndc_offset = perp_ndc * hw * side;
    let final_clip = clip_pos + vec4<f32>(ndc_offset * clip_pos.w, 0.0, 0.0);

    var min_elem: f32 = c.pattern_length;
    let elems = array<f32, 8>(
        c.pat0.x, c.pat0.y, c.pat0.z, c.pat0.w,
        c.pat1.x, c.pat1.y, c.pat1.z, c.pat1.w,
    );
    for (var i = 0u; i < 8u; i++) {
        let e = abs(elems[i]);
        if e > 0.0 && e < min_elem { min_elem = e; }
    }

    var out: VertexOut;
    out.clip_pos       = final_clip;
    out.clip_pos.z     = out.clip_pos.z - c.draw_depth * DRAW_ORDER_BIAS * out.clip_pos.w;
    out.color          = c.color;
    out.distance       = mix(in.distance_a, in.distance_b, which_end);
    out.pattern_length = c.pattern_length;
    out.pat0           = c.pat0;
    out.pat1           = c.pat1;
    out.min_elem       = min_elem;
    out.align_end      = c.align_end;
    out.align_total    = c.align_total;
    return out;
}

fn in_dash(dist: f32, pat_len: f32, p0: vec4<f32>, p1: vec4<f32>, align_end: f32, align_total: f32) -> bool {
    let elems = array<f32, 8>(p0.x, p0.y, p0.z, p0.w, p1.x, p1.y, p1.z, p1.w);
    var count = 0u;
    for (var i = 0u; i < 8u; i++) {
        if elems[i] != 0.0 { count = i + 1u; }
    }

    var d: f32;
    if align_total > 0.0 {
        // "A"-type alignment: the line begins and ends with a solid dash of
        // length `align_end`. Force the two end regions lit, then phase the
        // interior so the element AFTER the first dash resumes exactly at
        // `align_end` (the interior meets each end dash on a gap boundary).
        if dist <= align_end || dist >= align_total - align_end {
            return true;
        }
        var first_dash = 0.0;
        for (var i = 0u; i < count; i++) {
            if elems[i] > 0.0 { first_dash = elems[i]; break; }
        }
        d = ((dist - align_end + first_dash) % pat_len + pat_len) % pat_len;
    } else {
        d = ((dist % pat_len) + pat_len) % pat_len;
    }

    var pos = 0.0f;
    let dot_half = u.world_per_pixel * 0.75;
    for (var i = 0u; i < count; i++) {
        let elem = elems[i];
        if elem == 0.0 {
            let dd = abs(d - pos);
            if min(dd, pat_len - dd) <= dot_half { return true; }
        } else if elem > 0.0 {
            if d >= pos && d < pos + elem { return true; }
            pos += elem;
        } else {
            pos += -elem;
        }
    }
    return false;
}

@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if in.pattern_length > 0.0 {
        if in.min_elem >= u.world_per_pixel {
            if !in_dash(in.distance, in.pattern_length, in.pat0, in.pat1, in.align_end, in.align_total) {
                discard;
            }
        }
    }
    let alpha = select(1.0, in.color.a, u.transparency_enable > 0.5);
    return vec4<f32>(in.color.rgb, alpha);
}

// Black variant: used for 3D mesh outline edges in filled render modes so the
// mesh reads as a shaded surface framed by black edges. Keeps the dash/LOD
// logic identical to `fs_main`; only the RGB is forced to black.
@fragment fn fs_black(in: VertexOut) -> @location(0) vec4<f32> {
    if in.pattern_length > 0.0 {
        if in.min_elem >= u.world_per_pixel {
            if !in_dash(in.distance, in.pattern_length, in.pat0, in.pat1, in.align_end, in.align_total) {
                discard;
            }
        }
    }
    let alpha = select(1.0, in.color.a, u.transparency_enable > 0.5);
    return vec4<f32>(0.0, 0.0, 0.0, alpha);
}
