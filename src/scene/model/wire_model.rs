/// Tag for pre-baked snap candidates stored inside a WireModel.
/// Kept separate from `snap::SnapType` to avoid circular module dependencies.
#[derive(Clone, Copy, Debug)]
pub enum SnapHint {
    /// Geometric center of a circle, arc, or ellipse.
    Center,
    /// Point entity location.
    Node,
    /// 0 / 90 / 180 / 270 ° point on a circle/arc (within arc span).
    Quadrant,
    /// Insertion point of text or block.
    Insertion,
    /// Midpoint of a curve that has one well-defined midpoint (an arc's
    /// arc-length centre, a spline's `t = 0.5`). Lines / polylines do
    /// not use this — their midpoints are derived from `key_vertices`.
    Midpoint,
}

/// Geometric primitive used by the tangent-snap engine.
#[derive(Clone, Debug)]
pub enum TangentGeom {
    /// Infinite line through these two world-space points.
    Line { p1: [f32; 3], p2: [f32; 3] },
    /// Circle/arc.
    Circle { center: [f32; 3], radius: f32 },
}

/// A 1-D entity (line, arc, polyline) represented as an ordered set of
/// world-space points rendered as a quad strip (TriangleList).
///
/// Linetype is encoded as a GPU-side dash pattern so the CPU never needs to
/// split wires into per-dash segments.  `pattern_length = 0.0` means solid.
#[derive(Clone, Debug)]
pub struct WireModel {
    /// Unique identifier — the handle value as a decimal string.
    pub name: String,
    /// Ordered world-space positions forming a strip of quads. Each entry is
    /// the "high" half of a double-single f32 pair; [`points_low`] carries the
    /// matching residual so the shader can reconstruct the f64 source.
    pub points: Vec<[f32; 3]>,
    /// Low-bit residual paired index-for-index with [`points`]. Empty means
    /// "all-zero residual" (interactive draw / preview wires whose coordinates
    /// don't need sub-f32 precision). Tessellation from CAD f64 fills it.
    pub points_low: Vec<[f32; 3]>,
    /// RGBA colour in [0, 1].
    pub color: [f32; 4],
    /// Whether this wire is currently selected.
    #[allow(dead_code)]
    pub selected: bool,
    /// Total length of one pattern repeat (world units).  0 = solid line.
    pub pattern_length: f32,
    /// Up to 8 pattern elements: positive = dash length, negative = gap length.
    /// Unused slots must be 0.0 (acts as end-of-pattern sentinel in shader).
    pub pattern: [f32; 8],
    /// Rendered line width in screen pixels (half-width = line_weight_px / 2).
    pub line_weight_px: f32,
    /// World-space band width (drawing units). `0.0` = a normal wire whose
    /// width comes from `line_weight_px` (screen pixels). Non-zero = a wide
    /// polyline: the shader expands this centre-line to `world_width` world
    /// units (scaling with zoom) so the band IS the wire — the linetype dash
    /// pattern then applies to the band instead of a separate hatch fill.
    pub world_width: f32,
    /// Per-point full band width (drawing units), aligned index-for-index with
    /// [`points`], for a polyline whose width VARIES (a taper). Empty = a
    /// constant band of `world_width`. The wire shader reads the two endpoint
    /// widths of each segment and interpolates, so the band tapers smoothly.
    pub taper_widths: Vec<f32>,
    /// ACI color index (1-255).  0 means true-color or unknown (no CTB lookup).
    pub aci: u8,
    /// Pre-baked snap candidates (Center, Node, Quadrant, Insertion).
    pub snap_pts: Vec<(glam::DVec3, SnapHint)>,
    /// Per-segment tangent geometry for Tangent snap.
    /// Line/Arc entities: 1 entry.  LwPolyline: 1 entry per segment.
    pub tangent_geoms: Vec<TangentGeom>,
    /// True polyline vertices used for Endpoint/Midpoint snap.
    /// Non-empty only for entities with distinct vertex positions (Line, LwPolyline).
    /// Empty for tessellated curves (Circle, Arc, Ellipse) which use snap_pts instead.
    pub key_vertices: Vec<[f64; 3]>,
    /// World-space 2-D bounding box [min_x, min_y, max_x, max_y].
    /// Set from acadrust `bounding_box()` in `tessellate_entity()`.
    /// Preview / interim wires use `UNBOUNDED_AABB` so they are never pre-rejected
    /// by the snap world-space filter.
    pub aabb: [f32; 4],
    /// When false the linetype pattern restarts at each NaN-separated segment
    /// (DXF PLINEGEN=0).  When true the pattern runs continuously (PLINEGEN=1).
    pub plinegen: bool,
    /// DGN line-style marker. When false (every standard linetype) the dash
    /// pattern uses the normal phase: A-type end alignment for dash-first
    /// patterns, else centred. When true (DGN pipe walls) the pattern is drawn
    /// from the START vertex with continuous phase and no A-type end forcing —
    /// DGN line styles are not end-aligned.
    pub dash_from_start: bool,
    /// Shared "A"-type end-dash length for MLINE elements. `Some(len)` makes the
    /// dash shader use `len` as the begin/end solid-dash length for EVERY
    /// parallel element (derived once from the multiline's centre-line length),
    /// while `align_total` stays each element's own length — so all elements
    /// share one interior phase (perpendicular dashes line up) yet each still
    /// ends on a dash. `None` (the default) = per-wire A-type / from-start.
    pub dash_align_end: Option<f32>,
    /// Pre-triangulated solid fill: flat vertex list, 3 per triangle (world-offset applied).
    /// Non-empty only for PolyfaceMesh / PolygonMesh entities.
    pub fill_tris: Vec<[f32; 3]>,
    /// Low residual paired with [`fill_tris`] (double-single). Empty = all-zero;
    /// tessellation from CAD f64 fills it so fills stay precise at UTM scale.
    pub fill_tris_low: Vec<[f32; 3]>,
    /// Pre-triangulated pick-only geometry: flat vertex list, 3 per triangle.
    /// Hit-testing treats these as solid; this wire's own draw never uses them.
    ///
    /// Carries the interior of things whose surface the cursor would otherwise
    /// fall through, because [`points`] only bounds them:
    ///
    /// - An entity extruded by a DXF thickness (code 39) — the swept wall
    ///   between each base segment and its extruded copy. Drawn as its four
    ///   wireframe edges, with nothing in between to pick.
    /// - A wide polyline (codes 43 / 40 / 41) — the solid band. It *is* drawn,
    ///   but by the hatch pipeline off `wide_fills`, so the wire that carries
    ///   its centreline has no fill of its own to hit-test.
    ///
    /// Separate from [`fill_tris`] because that channel reaches the GPU: a
    /// thickness wall put there would render shaded and change how every such
    /// drawing looks, and a polyline band would be drawn a second time on top
    /// of the hatch that already draws it.
    pub pick_tris: Vec<[f32; 3]>,
    /// Low residual paired with [`pick_tris`] (double-single). Empty = all-zero.
    pub pick_tris_low: Vec<[f32; 3]>,
    /// SDF glyph quads for this entity's text (TEXT / MTEXT / dimension text /
    /// block-internal text). Non-empty only when SDF text is enabled and this
    /// wire carries a text run. Rides with the wire so it is cached by the
    /// tess memo, cloned on hit, and transformed by the block-expand loop
    /// exactly like `points` — no separate collector pass. The renderer
    /// gathers these across all wires into the text vertex buffer.
    pub text_verts: Vec<crate::scene::pipeline::text_gpu::TextVertex>,
    /// Block-local composed draw-order offset in (-1, 1) for a wire that must
    /// order against its *siblings inside the block* — currently wide-polyline
    /// bands, whose solid area would otherwise cover later-drawn siblings.
    /// `None` (all other wires) = the whole-insert depth resolved from `name`.
    /// `Some(local)` composes at draw time: `insert_depth + local * insert_half`,
    /// mirroring how exploded block fills seed their depth, so bands and fills
    /// from the same block interleave by the block's internal draw order.
    pub depth_override: Option<f32>,
    /// `true` when [`fill_tris`] is a real 3-D surface (PolyfaceMesh /
    /// PolygonMesh face) that must render with hidden-surface depth and only in
    /// shaded modes. `false` for a flat 2-D overlay fill (SOLID arrowhead,
    /// dimension / MultiLeader text background, greek-LOD text) that draws in
    /// every view mode. The render pass can't infer this from `fill_tris_low`
    /// alone — a 2-D fill at UTM scale carries a low residual too.
    pub fill_is_3d: bool,
}

impl WireModel {
    pub const WHITE: [f32; 4] = [1.00, 1.00, 1.00, 1.0];
    pub const CYAN: [f32; 4] = [0.25, 0.85, 1.00, 1.0];
    pub const SELECTED: [f32; 4] = [0.15, 0.55, 1.00, 1.0];
    /// Rollover (hover) highlight — orange, distinct from the blue selection.
    pub const HOVER: [f32; 4] = [0.95, 0.55, 0.10, 1.0];
    /// Sentinel AABB that never rejects any snap query.
    pub const UNBOUNDED_AABB: [f32; 4] = [
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::INFINITY,
    ];

    /// Double-single split: `high + low ≈ v` to ~f64 precision in two f32s.
    /// Matches the renderer's relative-to-eye reconstruction.
    #[inline]
    pub fn split_ds(v: f64) -> (f32, f32) {
        let high = v as f32;
        (high, (v - high as f64) as f32)
    }

    /// Create a solid preview wire from f64 points, filling the double-single
    /// `points_low` buffer so the line stays precise at UTM-scale coordinates.
    /// Rubber-band previews built straight from f32 absolute points jitter
    /// ~0.5 m at UTM because the wire pass is relative-to-eye and expects the
    /// low residual; this keeps the preview glued to the cursor.
    pub fn solid_f64(name: String, points: Vec<[f64; 3]>, color: [f32; 4], selected: bool) -> Self {
        let mut hi = Vec::with_capacity(points.len());
        let mut lo = Vec::with_capacity(points.len());
        for [x, y, z] in points {
            let (hx, lx) = Self::split_ds(x);
            let (hy, ly) = Self::split_ds(y);
            let (hz, lz) = Self::split_ds(z);
            hi.push([hx, hy, hz]);
            lo.push([lx, ly, lz]);
        }
        let mut w = Self::solid(name, hi, color, selected);
        w.points_low = lo;
        w
    }

    /// Create a solid wire (no dash pattern, 1px weight).
    pub fn solid(name: String, points: Vec<[f32; 3]>, color: [f32; 4], selected: bool) -> Self {
        Self {
            taper_widths: Vec::new(),
            world_width: 0.0,
            depth_override: None,
            fill_is_3d: false,
            pick_tris: Vec::new(),
            pick_tris_low: Vec::new(),
            text_verts: Vec::new(),
            name,
            points,
            points_low: Vec::new(),
            color,
            selected,
            aci: 0,
            pattern_length: 0.0,
            pattern: [0.0; 8],
            line_weight_px: 1.0,
            snap_pts: vec![],
            tangent_geoms: vec![],
            key_vertices: vec![],
            aabb: Self::UNBOUNDED_AABB,
            plinegen: true,
            dash_from_start: false,
            dash_align_end: None,
            fill_tris: vec![],
            fill_tris_low: Vec::new(),
        }
    }

    /// Return a clone with every point translated by `delta`.
    pub fn translated(&self, delta: glam::Vec3) -> Self {
        let mut out = self.clone();
        out.name = format!("preview_{}", self.name);
        out.color = Self::CYAN;
        out.selected = false;
        for p in &mut out.points {
            p[0] += delta.x;
            p[1] += delta.y;
            p[2] += delta.z;
        }
        if !out.text_verts.is_empty() {
            let (dx, dy, dz) = (delta.x as f64, delta.y as f64, delta.z as f64);
            out.text_verts =
                map_text_verts(&self.text_verts, |x, y, z| (x + dx, y + dy, z + dz));
        }
        out
    }

    /// Return a clone with every point rotated around `center` by `angle_rad`.
    pub fn rotated(&self, center: glam::Vec3, angle_rad: f32) -> Self {
        let (s, c) = angle_rad.sin_cos();
        let mut out = self.clone();
        out.name = format!("preview_{}", self.name);
        out.color = Self::CYAN;
        out.selected = false;
        for p in &mut out.points {
            let dx = p[0] - center.x;
            let dy = p[1] - center.y;
            p[0] = center.x + dx * c - dy * s;
            p[1] = center.y + dx * s + dy * c;
        }
        if !out.text_verts.is_empty() {
            let (cx, cy) = (center.x as f64, center.y as f64);
            let (s, c) = (s as f64, c as f64);
            out.text_verts = map_text_verts(&self.text_verts, |x, y, z| {
                let (dx, dy) = (x - cx, y - cy);
                (cx + dx * c - dy * s, cy + dx * s + dy * c, z)
            });
        }
        out
    }

    /// Return a clone with every point uniformly scaled from `center` by `factor`.
    pub fn scaled(&self, center: glam::Vec3, factor: f32) -> Self {
        let mut out = self.clone();
        out.name = format!("preview_{}", self.name);
        out.color = Self::CYAN;
        out.selected = false;
        for p in &mut out.points {
            p[0] = center.x + (p[0] - center.x) * factor;
            p[1] = center.y + (p[1] - center.y) * factor;
            p[2] = center.z + (p[2] - center.z) * factor;
        }
        if !out.text_verts.is_empty() {
            let (cx, cy, cz) = (center.x as f64, center.y as f64, center.z as f64);
            let f = factor as f64;
            out.text_verts = map_text_verts(&self.text_verts, |x, y, z| {
                (cx + (x - cx) * f, cy + (y - cy) * f, cz + (z - cz) * f)
            });
        }
        out
    }

    /// Return a clone for a stretch preview: every point whose XY lies inside
    /// the crossing window `[win_min, win_max]` is translated by `delta`; points
    /// outside stay put. Exact for line/polyline vertices (the primary stretch
    /// targets); curve tessellation points may deform where a window edge cuts
    /// through them, matching the per-vertex nature of the operation.
    pub fn stretched(&self, win_min: glam::Vec3, win_max: glam::Vec3, delta: glam::Vec3) -> Self {
        let mut out = self.clone();
        out.name = format!("preview_{}", self.name);
        out.color = Self::CYAN;
        out.selected = false;
        for p in &mut out.points {
            if p[0] >= win_min.x && p[0] <= win_max.x && p[1] >= win_min.y && p[1] <= win_max.y {
                p[0] += delta.x;
                p[1] += delta.y;
                p[2] += delta.z;
            }
        }
        if !out.text_verts.is_empty() {
            let (mnx, mny) = (win_min.x as f64, win_min.y as f64);
            let (mxx, mxy) = (win_max.x as f64, win_max.y as f64);
            let (dx, dy, dz) = (delta.x as f64, delta.y as f64, delta.z as f64);
            out.text_verts = map_text_verts(&self.text_verts, |x, y, z| {
                if x >= mnx && x <= mxx && y >= mny && y <= mxy {
                    (x + dx, y + dy, z + dz)
                } else {
                    (x, y, z)
                }
            });
        }
        out
    }

    /// Return a clone mirrored across the line through `p1`→`p2`.
    pub fn mirrored(&self, p1: glam::Vec3, p2: glam::Vec3) -> Self {
        let ax = p2.x - p1.x;
        let ay = p2.y - p1.y;
        let len2 = ax * ax + ay * ay;
        let mut out = self.clone();
        out.name = format!("preview_{}", self.name);
        out.color = Self::CYAN;
        out.selected = false;
        if len2 < 1e-12 {
            return out;
        }
        for p in &mut out.points {
            let dx = p[0] - p1.x;
            let dy = p[1] - p1.y;
            let t = (dx * ax + dy * ay) / len2;
            p[0] = p1.x + 2.0 * t * ax - dx;
            p[1] = p1.y + 2.0 * t * ay - dy;
        }
        // World position is the double-single sum `points + points_low` (text /
        // UTM wires split it), so the residual must reflect too — as a direction
        // (linear reflection about the axis, no `p1` offset).
        for p in &mut out.points_low {
            let t = (p[0] * ax + p[1] * ay) / len2;
            p[0] = 2.0 * t * ax - p[0];
            p[1] = 2.0 * t * ay - p[1];
        }
        // Glyph quads reflect wholesale (true mirror) — the caller only routes
        // text through here for MIRRTEXT-on; MIRRTEXT-off relocates via
        // `translated` so glyphs stay readable.
        if !out.text_verts.is_empty() {
            let (ax, ay, len2) = (ax as f64, ay as f64, len2 as f64);
            let (p1x, p1y) = (p1.x as f64, p1.y as f64);
            out.text_verts = map_text_verts(&self.text_verts, |x, y, z| {
                let (dx, dy) = (x - p1x, y - p1y);
                let t = (dx * ax + dy * ay) / len2;
                (p1x + 2.0 * t * ax - dx, p1y + 2.0 * t * ay - dy, z)
            });
        }
        out
    }

    /// Total arc-length of this wire (sum of segment lengths).
    #[allow(dead_code)]
    pub fn length(&self) -> f32 {
        self.points
            .windows(2)
            .map(|w| {
                let dx = w[1][0] - w[0][0];
                let dy = w[1][1] - w[0][1];
                let dz = w[1][2] - w[0][2];
                (dx * dx + dy * dy + dz * dz).sqrt()
            })
            .sum()
    }
}

/// Map every glyph vertex's double-single world position through `f`, re-
/// splitting the result. The preview transforms above move `points`, but SDF
/// glyph quads live in `text_verts` (absolute-world double-single) — so a text
/// ghost (MOVE / COPY / ROTATE / SCALE / STRETCH / MIRROR preview) must carry
/// these along or the dragged text renders frozen at its source (issue #316).
/// Paper-space viewport projection uses it for the same reason (issue #385).
pub(crate) fn map_text_verts(
    verts: &[crate::scene::pipeline::text_gpu::TextVertex],
    f: impl Fn(f64, f64, f64) -> (f64, f64, f64),
) -> Vec<crate::scene::pipeline::text_gpu::TextVertex> {
    use crate::scene::pipeline::text_gpu::split_ds;
    verts
        .iter()
        .map(|v| {
            let (nx, ny, nz) = f(
                v.pos[0] as f64 + v.pos_low[0] as f64,
                v.pos[1] as f64 + v.pos_low[1] as f64,
                v.pos[2] as f64 + v.pos_low[2] as f64,
            );
            let (xh, xl) = split_ds(nx);
            let (yh, yl) = split_ds(ny);
            let (zh, zl) = split_ds(nz);
            crate::scene::pipeline::text_gpu::TextVertex {
                pos: [xh, yh, zh],
                pos_low: [xl, yl, zl],
                ..*v
            }
        })
        .collect()
}

impl Default for WireModel {
    fn default() -> Self {
        Self {
            text_verts: Vec::new(),
            name: String::new(),
            points: Vec::new(),
            points_low: Vec::new(),
            color: Self::WHITE,
            selected: false,
            pattern_length: 0.0,
            pattern: [0.0; 8],
            line_weight_px: 1.0,
            world_width: 0.0,
            taper_widths: Vec::new(),
            aci: 0,
            snap_pts: Vec::new(),
            tangent_geoms: Vec::new(),
            key_vertices: Vec::new(),
            aabb: Self::UNBOUNDED_AABB,
            plinegen: true,
            dash_from_start: false,
            dash_align_end: None,
            fill_tris: Vec::new(),
            fill_tris_low: Vec::new(),
            depth_override: None,
            fill_is_3d: false,
            pick_tris: Vec::new(),
            pick_tris_low: Vec::new(),
        }
    }
}
