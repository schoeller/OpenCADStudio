use std::cell::Cell;

use crate::scene::model::object::{GripDef, GripShape, PropValue, Property};

/// Linear / angular unit format pulled from the document header so the
/// per-thread properties pipeline can format values consistently without
/// passing the document through every callsite.
#[derive(Clone, Copy, Default)]
pub struct UnitContext {
    /// LUNITS — 1=Sci, 2=Decimal, 3=Engineering, 4=Architectural, 5=Fractional
    pub lunits: i16,
    /// LUPREC — decimal places (linear)
    pub luprec: i16,
    /// AUNITS — 0=Decimal degrees, 1=DMS, 2=Grad, 3=Rad. Surfaced via
    /// `format_angle`, which is read on demand by code that already
    /// formats angular values via radians.
    #[allow(dead_code)]
    pub aunits: i16,
    /// AUPREC — decimal places (angular)
    #[allow(dead_code)]
    pub auprec: i16,
}

thread_local! {
    static UNIT_CTX: Cell<UnitContext> = const { Cell::new(UnitContext {
        lunits: 2,
        luprec: 4,
        aunits: 0,
        auprec: 0,
    }) };
}

/// Set the per-thread unit context. Properties helpers consult it when
/// they format f64 values into display strings.
pub fn set_unit_context(ctx: UnitContext) {
    UNIT_CTX.with(|c| c.set(ctx));
}

pub fn unit_context() -> UnitContext {
    UNIT_CTX.with(|c| c.get())
}

/// Format a linear length using LUNITS / LUPREC. Architectural / fractional
/// produce "n'-d/D"" style strings (1 unit = 1 inch); decimal / scientific /
/// engineering / Windows-desktop fall back to plain decimal at LUPREC places.
pub fn format_length(value: f64) -> String {
    let ctx = unit_context();
    let prec = ctx.luprec.max(0) as usize;
    match ctx.lunits {
        1 => format!("{:.*e}", prec, value),
        3 => {
            // Engineering: ft-inches, decimal inches.
            let sign = if value < 0.0 { "-" } else { "" };
            let abs = value.abs();
            let feet = (abs / 12.0).trunc();
            let rem = abs - feet * 12.0;
            format!("{}{:.0}'-{:.*}\"", sign, feet, prec, rem)
        }
        4 | 5 => {
            // Architectural / Fractional — n + fraction with 1/2^p denom (1
            // unit = 1 inch). Use 6 as a moderate denominator power so the
            // result reads like 1/64".
            let sign = if value < 0.0 { "-" } else { "" };
            let abs = value.abs();
            let (feet, in_rem) = if ctx.lunits == 4 {
                let f = (abs / 12.0).trunc();
                (Some(f as i64), abs - f * 12.0)
            } else {
                (None, abs)
            };
            let whole = in_rem.trunc();
            let frac = in_rem - whole;
            let denom = 64u64;
            let numer = (frac * denom as f64).round() as i64;
            let mut n = numer as u64;
            let mut d = denom;
            while d > 1 && n % 2 == 0 && d % 2 == 0 {
                n /= 2;
                d /= 2;
            }
            let frac_str = if n == 0 || d == 1 {
                String::new()
            } else {
                format!(" {}/{}", n, d)
            };
            let unit_suffix = if ctx.lunits == 4 { "\"" } else { "" };
            match feet {
                Some(f) => format!("{}{}'-{:.0}{}{}", sign, f, whole, frac_str, unit_suffix),
                None => format!("{}{:.0}{}", sign, whole, frac_str),
            }
        }
        _ => format!("{:.*}", prec, value),
    }
}

/// Format an angle (input in radians) using AUNITS / AUPREC.
#[allow(dead_code)]
pub fn format_angle(value_rad: f64) -> String {
    let ctx = unit_context();
    let prec = ctx.auprec.max(0) as usize;
    match ctx.aunits {
        1 => {
            // DMS — degrees / minutes / seconds.
            let deg = value_rad.to_degrees();
            let sign = if deg < 0.0 { "-" } else { "" };
            let a = deg.abs();
            let d = a.floor();
            let m_full = (a - d) * 60.0;
            let m = m_full.floor();
            let s = (m_full - m) * 60.0;
            format!("{}{:.0}°{:.0}'{:.*}\"", sign, d, m, prec, s)
        }
        2 => {
            let g = value_rad.to_degrees() / 0.9;
            format!("{:.*}g", prec, g)
        }
        3 => format!("{:.*}r", prec, value_rad),
        _ => format!("{:.*}°", prec, value_rad.to_degrees()),
    }
}

pub fn square_grip(id: usize, world: glam::DVec3) -> GripDef {
    GripDef {
        id,
        world,
        is_midpoint: false,
        shape: GripShape::Square,
        dir: None,
    }
}

/// Centre / translate grip — same square marker as a vertex grip but
/// flagged as a "whole-object move" handle for the grip-edit code.
pub fn center_grip(id: usize, world: glam::DVec3) -> GripDef {
    GripDef {
        id,
        world,
        is_midpoint: true,
        shape: GripShape::Square,
        dir: None,
    }
}

/// Circle grip — a round handle, flagged as a whole-object move. Used for
/// special anchors like a hatch's pattern origin.
pub fn circle_grip(id: usize, world: glam::DVec3) -> GripDef {
    GripDef {
        id,
        world,
        is_midpoint: true,
        shape: GripShape::Circle,
        dir: None,
    }
}

/// Mid-segment stretch grip oriented along `dir` (the segment's in-plane
/// world-XY direction). Drawn as a small rectangle elongated along the
/// segment so the affordance reads as "stretch perpendicular".
pub fn rectangle_grip(id: usize, world: glam::DVec3, dir: [f32; 2]) -> GripDef {
    GripDef {
        id,
        world,
        is_midpoint: true,
        shape: GripShape::Rectangle,
        dir: Some(dir),
    }
}

#[allow(dead_code)]
pub fn triangle_grip(id: usize, world: glam::DVec3) -> GripDef {
    GripDef {
        id,
        world,
        is_midpoint: false,
        shape: GripShape::Triangle,
        dir: None,
    }
}

pub fn edit_prop(label: &'static str, field: &'static str, value: f64) -> Property {
    Property {
        label: label.into(),
        field,
        value: PropValue::EditText(format_length(value)),
    }
}

pub fn ro_prop(label: &'static str, field: &'static str, value: impl Into<String>) -> Property {
    Property {
        label: label.into(),
        field,
        value: PropValue::ReadOnly(value.into()),
    }
}

/// A numeric row that is an editable box when `editable`, otherwise a grayed
/// read-only value using the same length formatting. Used where a field's
/// editability depends on entity state (e.g. a text point that is only live for
/// certain justifications, or an MText column dimension).
pub fn num_prop(label: &'static str, field: &'static str, value: f64, editable: bool) -> Property {
    if editable {
        edit_prop(label, field, value)
    } else {
        ro_prop(label, field, format_length(value))
    }
}

/// A ◀ / ▶ index navigator row (e.g. a polyline's Current Vertex). `display` is
/// the label shown between the arrows (e.g. "2 / 7").
pub fn stepper_prop(
    label: &'static str,
    field: &'static str,
    display: impl Into<String>,
) -> Property {
    Property {
        label: label.into(),
        field,
        value: PropValue::Stepper {
            field,
            display: display.into(),
        },
    }
}

pub fn parse_f64(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok()
}

/// Bulge → arc geometry for a polyline segment.
///
/// DXF/DWG polyline arcs are encoded as a bulge factor on each vertex —
/// `bulge = tan(theta/4)` where `theta` is the included angle of the arc
/// from `p0` to `p1`. Sign convention: positive bulge = CCW from p0 to p1,
/// negative = CW. `|bulge| = 1` is a half-circle.
///
/// This struct centralises the (formerly duplicated) math that takes
/// `(p0, p1, bulge)` and produces the canonical `(center, radius,
/// start_angle, sweep)` quadruple. Callsites pick the fields they need.
#[derive(Clone, Copy, Debug)]
pub struct BulgeArc {
    pub center: [f64; 2],
    pub radius: f64,
    /// Angle from center to p0 (atan2, range -π..π).
    pub start_angle: f64,
    /// Angle from center to p1 (atan2, range -π..π).
    pub end_angle: f64,
    /// Signed sweep from p0 to p1. Positive ⇒ CCW (bulge > 0),
    /// negative ⇒ CW (bulge < 0). For exact half-turns the sign of
    /// `bulge` decides the direction.
    pub sweep: f64,
}

impl BulgeArc {
    /// Build from endpoints + bulge. Returns `None` for degenerate input
    /// (chord ≈ 0 or |bulge| ≈ 0).
    pub fn from_bulge(p0: [f64; 2], p1: [f64; 2], bulge: f64) -> Option<Self> {
        let chord_x = p1[0] - p0[0];
        let chord_y = p1[1] - p0[1];
        let chord_len = (chord_x * chord_x + chord_y * chord_y).sqrt();
        if chord_len < 1e-12 || bulge.abs() < 1e-12 {
            return None;
        }
        let b = bulge;
        let b2 = b * b;
        // r = chord · (1 + b²) / (4·|b|)
        let r = chord_len * (1.0 + b2) / (4.0 * b.abs());
        // d_perp = signed distance from chord midpoint to arc center
        //        = r · (1 - b²) / (1 + b²) = r · cos(theta/2)
        let d_perp = r * (1.0 - b2) / (1.0 + b2);
        let mx = (p0[0] + p1[0]) * 0.5;
        let my = (p0[1] + p1[1]) * 0.5;
        // Left perpendicular to chord (90° CCW).
        let perp_x = -chord_y / chord_len;
        let perp_y = chord_x / chord_len;
        let sign = b.signum();
        let cx = mx + sign * d_perp * perp_x;
        let cy = my + sign * d_perp * perp_y;
        let a0 = (p0[1] - cy).atan2(p0[0] - cx);
        let a1 = (p1[1] - cy).atan2(p1[0] - cx);
        // Wrap sweep to match bulge sign: bulge>0 ⇒ positive (CCW),
        // bulge<0 ⇒ negative (CW). Falls back to ±τ for half-turns.
        const TAU: f64 = std::f64::consts::TAU;
        let mut sweep = a1 - a0;
        if bulge > 0.0 {
            if sweep <= 0.0 {
                sweep += TAU;
            }
        } else if sweep >= 0.0 {
            sweep -= TAU;
        }
        if sweep.abs() < 1e-9 {
            sweep = if bulge > 0.0 { TAU } else { -TAU };
        }
        Some(Self {
            center: [cx, cy],
            radius: r,
            start_angle: a0,
            end_angle: a1,
            sweep,
        })
    }

    /// Sample a point on the arc at parameter `t ∈ [0, 1]`. `t=0` ↦ p0,
    /// `t=1` ↦ p1, walks along the signed sweep direction.
    pub fn sample(&self, t: f64) -> [f64; 2] {
        let a = self.start_angle + self.sweep * t;
        [
            self.center[0] + self.radius * a.cos(),
            self.center[1] + self.radius * a.sin(),
        ]
    }
}

/// Triangulate the solid bands a `wide_fills` returns into the flat WCS f64
/// triangle list `TruckEntity::pick_tris` carries, so a wide polyline is
/// selectable across the band it draws and not just along its centreline.
///
/// `origin` and `fills` are that function's own pair: 2-D offsets from the
/// first vertex, which is the exact frame the band's `HatchModel` renders in
/// (`world_origin` + boundary, no elevation). Building the pick geometry from
/// the same numbers keeps hit-testing on whatever the fill actually drew.
///
/// An arc band is an annular sector — concave on its inner edge — so this ear
/// clips rather than fans.
pub(crate) fn wide_band_tris(origin: [f64; 2], fills: &[Vec<[f32; 2]>]) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for poly in fills {
        let ring: Vec<[f64; 3]> = poly
            .iter()
            .map(|&[x, y]| [origin[0] + x as f64, origin[1] + y as f64, 0.0])
            .collect();
        out.extend(crate::entities::mesh::triangulate_planar(&ring));
    }
    out
}

/// Extrude a wide-polyline band (from `wide_fills`) into a solid tube for a DXF
/// thickness (code 39): a vertical wall between every band-boundary point and
/// its `thickness`-along-`normal` copy, plus triangulated bottom and top caps.
/// Returns `(fill_tris, edge_lines)` as flat WCS f64 lists — the caller wraps
/// them in a `TruckEntity` (object = `Lines(edge_lines)`, `fill_tris`, and
/// `pick_tris = fill_tris`). Shared by LwPolyline and Polyline2D so both wide
/// polyline kinds extrude the same solid instead of just their centre-line.
///
/// `polyline_segment_fill` emits each band loop as the outer boundary forward
/// then the inner boundary back, so its two transition edges (`half-1 → half`
/// and `n-1 → 0`) are radial cap ends inside the band — no wall is drawn there.
pub(crate) fn thick_band_tube(
    origin: [f64; 2],
    fills: &[Vec<[f32; 2]>],
    thickness: f64,
    normal: (f64, f64, f64),
    to_wcs: &dyn Fn(f64, f64) -> (f64, f64, f64),
) -> (Vec<[f64; 3]>, Vec<[f64; 3]>) {
    let (nx, ny, nz) = normal;
    let t = thickness;
    let off = |p: [f64; 3]| -> [f64; 3] { [p[0] + t * nx, p[1] + t * ny, p[2] + t * nz] };
    let push_seg = |lines: &mut Vec<[f64; 3]>, a: [f64; 3], b: [f64; 3]| {
        lines.push(a);
        lines.push(b);
        lines.push([f64::NAN; 3]);
    };
    let mut lines: Vec<[f64; 3]> = Vec::new();
    let mut fill_tris: Vec<[f64; 3]> = Vec::new();
    for poly in fills {
        let n = poly.len();
        if n < 4 {
            continue;
        }
        let half = n / 2;
        let bot: Vec<[f64; 3]> = poly
            .iter()
            .map(|&[x, y]| {
                let (wx, wy, wz) = to_wcs(origin[0] + x as f64, origin[1] + y as f64);
                [wx, wy, wz]
            })
            .collect();
        let top: Vec<[f64; 3]> = bot.iter().map(|&p| off(p)).collect();
        for k in 0..n {
            push_seg(&mut lines, bot[k], top[k]);
            if k == half - 1 || k == n - 1 {
                continue;
            }
            let kn = (k + 1) % n;
            push_seg(&mut lines, bot[k], bot[kn]);
            push_seg(&mut lines, top[k], top[kn]);
            fill_tris.extend_from_slice(&[bot[k], bot[kn], top[kn], bot[k], top[kn], top[k]]);
        }
        fill_tris.extend(crate::entities::mesh::triangulate_planar(&bot));
        fill_tris.extend(crate::entities::mesh::triangulate_planar(&top));
    }
    (fill_tris, lines)
}

/// Compute the filled boundary polygon for one polyline segment.
/// For straight segments: a rectangle/trapezoid.
/// For arc segments: an arc band (outer arc + reversed inner arc).
/// Returns `None` if the segment is degenerate.
pub(crate) fn polyline_segment_fill(
    p0: [f32; 2],
    p1: [f32; 2],
    hw0: f32,
    hw1: f32,
    bulge: f32,
) -> Option<Vec<[f32; 2]>> {
    if bulge.abs() < 1e-9 {
        // Straight segment — rectangle or trapezoid
        let dx = p1[0] - p0[0];
        let dy = p1[1] - p0[1];
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            return None;
        }
        let nx = -dy / len;
        let ny = dx / len;
        Some(vec![
            [p0[0] + hw0 * nx, p0[1] + hw0 * ny],
            [p1[0] + hw1 * nx, p1[1] + hw1 * ny],
            [p1[0] - hw1 * nx, p1[1] - hw1 * ny],
            [p0[0] - hw0 * nx, p0[1] - hw0 * ny],
        ])
    } else {
        // Arc segment — arc band polygon.
        // Center math matches `bulge_to_arc` in modules/home/modify/explode.rs:
        //   r = chord * (1 + b²) / (4·|b|)
        //   d = r * (1 - b²) / (1 + b²)   (signed: negative ⇒ major arc, center
        //                                  flips to the opposite side of chord)
        //   center = midpoint + sign(b) · d · left_perp(chord)
        let b = bulge as f64;
        let b2 = b * b;
        let dx = (p1[0] - p0[0]) as f64;
        let dy = (p1[1] - p0[1]) as f64;
        let chord_len = (dx * dx + dy * dy).sqrt();
        if chord_len < 1e-9 || b.abs() < 1e-12 {
            return None;
        }
        let r = chord_len * (1.0 + b2) / (4.0 * b.abs());
        let d_perp = r * (1.0 - b2) / (1.0 + b2);
        let mx = ((p0[0] + p1[0]) * 0.5) as f64;
        let my = ((p0[1] + p1[1]) * 0.5) as f64;
        let perp_x = -dy / chord_len;
        let perp_y = dx / chord_len;
        let sign = b.signum();
        let cx = (mx + sign * d_perp * perp_x) as f32;
        let cy = (my + sign * d_perp * perp_y) as f32;
        let a0 = ((p0[1] - cy) as f32).atan2((p0[0] - cx) as f32);
        let a1 = ((p1[1] - cy) as f32).atan2((p1[0] - cx) as f32);
        let (sa, mut ea) = if bulge > 0.0 { (a0, a1) } else { (a1, a0) };
        if ea < sa {
            ea += std::f32::consts::TAU;
        }
        let span = ea - sa;
        let segs = ((span.abs() / std::f32::consts::TAU) * 24.0)
            .ceil()
            .max(4.0) as u32;
        let r = r as f32;
        let r_outer = |t: f32| r + (hw0 + (hw1 - hw0) * t);
        let r_inner = |t: f32| (r - (hw0 + (hw1 - hw0) * t)).max(0.0);
        let mut boundary = Vec::with_capacity((segs as usize + 1) * 2);
        let inv = 1.0 / segs as f32;
        for j in 0..=segs {
            let t = j as f32 * inv;
            let ang = sa + span * t;
            let ro = r_outer(t);
            boundary.push([cx + ro * ang.cos(), cy + ro * ang.sin()]);
        }
        for j in (0..=segs).rev() {
            let t = j as f32 * inv;
            let ang = sa + span * t;
            let ri = r_inner(t);
            boundary.push([cx + ri * ang.cos(), cy + ri * ang.sin()]);
        }
        Some(boundary)
    }
}

