// Offset tool — ribbon definition + interactive command.
//
// Command:  OFFSET (O)
//   OFFSET: Creates a parallel copy of an object (line, arc, circle,
//   or lwpolyline) at a specified distance on the chosen side.
//
//   Steps:
//     1. Text input: "Specify offset distance <last>:" → enter float or Enter for default
//     2. Pick object to offset (Line, Arc, Circle, LwPolyline)
//     3. Pick a point on the side to offset toward

use std::f64::consts::TAU;

use crate::modules::draw::modify::spline_ops::{spline_pts_wire, spline_sample_xy};
use acadrust::entities::LwVertex;
use acadrust::entities::{
    Arc as ArcEnt, Circle as CircleEnt, Ellipse as EllipseEnt, Line as LineEnt, LwPolyline,
    Spline as SplineEnt, XLine as XLineEnt,
};
use acadrust::{EntityType, Handle};
use glam::{DVec3, Vec3};

use crate::command::{CadCommand, CmdResult};
use crate::modules::draw::defaults;
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

// ── Ribbon definition ──────────────────────────────────────────────────────

pub fn tool() -> ToolDef {
    ToolDef {
        id: "OFFSET",
        label: "Offset",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/offset.svg")),
        event: ModuleEvent::Command("OFFSET".to_string()),
    }
}

// ── Geometry helpers ────────────────────────────────────────────────────────

/// Infinite-line intersection in 2D.  Returns the point or None if parallel.
fn isect_lines(p0: [f64; 2], p1: [f64; 2], q0: [f64; 2], q1: [f64; 2]) -> Option<[f64; 2]> {
    let dx = p1[0] - p0[0];
    let dy = p1[1] - p0[1];
    let ex = q1[0] - q0[0];
    let ey = q1[1] - q0[1];
    let det = dx * ey - dy * ex;
    if det.abs() < 1e-10 {
        return None;
    }
    let t = ((q0[0] - p0[0]) * ey - (q0[1] - p0[1]) * ex) / det;
    Some([p0[0] + t * dx, p0[1] + t * dy])
}

fn norm_rad(a: f64) -> f64 {
    ((a % TAU) + TAU) % TAU
}

// ── Line offset ────────────────────────────────────────────────────────────

fn offset_line(l: &LineEnt, dist: f64, side_pt: Vec3) -> Option<EntityType> {
    let dx = l.end.x - l.start.x;
    let dy = l.end.y - l.start.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-12 {
        return None;
    }

    let nx = -dy / len; // left-perpendicular
    let ny = dx / len;

    let vx = side_pt.x as f64 - l.start.x;
    let vy = side_pt.y as f64 - l.start.y;
    let cross = dx * vy - dy * vx;
    let sign = if cross >= 0.0 { 1.0 } else { -1.0 };

    let ox = sign * nx * dist;
    let oy = sign * ny * dist;

    let mut new_l = l.clone();
    new_l.common.handle = Handle::NULL;
    new_l.start.x += ox;
    new_l.start.y += oy;
    new_l.end.x += ox;
    new_l.end.y += oy;
    Some(EntityType::Line(new_l))
}

// ── XLine (infinite construction line) offset ────────────────────────────────
// A parallel infinite line: same direction, base point shifted perpendicular by
// `dist` toward the side the cursor is on. (#296)

fn offset_xline(x: &XLineEnt, dist: f64, side_pt: Vec3) -> Option<EntityType> {
    let dx = x.direction.x;
    let dy = x.direction.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-12 {
        return None;
    }

    let nx = -dy / len; // left-perpendicular
    let ny = dx / len;

    let vx = side_pt.x as f64 - x.base_point.x;
    let vy = side_pt.y as f64 - x.base_point.y;
    let cross = dx * vy - dy * vx;
    let sign = if cross >= 0.0 { 1.0 } else { -1.0 };

    let mut new_x = x.clone();
    new_x.common.handle = Handle::NULL;
    new_x.base_point.x += sign * nx * dist;
    new_x.base_point.y += sign * ny * dist;
    Some(EntityType::XLine(new_x))
}

// ── Circle offset ──────────────────────────────────────────────────────────

fn offset_circle(c: &CircleEnt, dist: f64, side_pt: Vec3) -> Option<EntityType> {
    let px = side_pt.x as f64;
    let py = side_pt.y as f64;
    let dc = ((px - c.center.x).powi(2) + (py - c.center.y).powi(2)).sqrt();

    let new_r = if dc < c.radius {
        c.radius - dist
    } else {
        c.radius + dist
    };
    if new_r <= 1e-9 {
        return None;
    }

    let mut new_c = c.clone();
    new_c.common.handle = Handle::NULL;
    new_c.radius = new_r;
    Some(EntityType::Circle(new_c))
}

// ── Arc offset ─────────────────────────────────────────────────────────────

fn offset_arc(a: &ArcEnt, dist: f64, side_pt: Vec3) -> Option<EntityType> {
    let px = side_pt.x as f64;
    let py = side_pt.y as f64;
    let dc = ((px - a.center.x).powi(2) + (py - a.center.y).powi(2)).sqrt();

    let new_r = if dc < a.radius {
        a.radius - dist
    } else {
        a.radius + dist
    };
    if new_r <= 1e-9 {
        return None;
    }

    let mut new_a = a.clone();
    new_a.common.handle = Handle::NULL;
    new_a.radius = new_r;
    Some(EntityType::Arc(new_a))
}

// ── LwPolyline offset ──────────────────────────────────────────────────────
//
// Algorithm:
//   1. Offset every segment by `dist` in the direction perpendicular to it
//      (sign is determined once from the first non-degenerate segment + side_pt).
//   2. Reconnect adjacent offset segments:
//      - Open: first / last vertex use the raw offset endpoints;
//        interior vertices are the intersection of adjacent offset segments.
//      - Closed: every vertex is the intersection of the previous and next
//        offset segments.
//   3. Bulge values are preserved from the original vertices (arc segments
//      keep the same angle; the radius changes implicitly via the new chord
//      length — a minor approximation acceptable for modest offsets).

fn offset_lwpolyline(p: &LwPolyline, dist: f64, side_pt: Vec3) -> Option<EntityType> {
    let n = p.vertices.len();
    if n < 2 {
        return None;
    }

    let n_segs = if p.is_closed { n } else { n - 1 };

    // Determine the offset sign (which side the left normal `(-dy, dx)` is
    // scaled toward).
    let sign: f64 = if p.is_closed {
        // For a closed loop the side is unambiguous: a pick inside the loop
        // offsets inward, outside offsets outward. Decide that with a
        // point-in-polygon test and map it to the normal via the winding —
        // the left normal points inward for a CCW loop. (The first-segment
        // heuristic used for open paths misreads a pick placed *beside* the
        // shape: it is outside the loop yet on the inner half-plane of the
        // first edge's infinite line, so a CCW rectangle offset outward by a
        // side pick wrongly collapsed inward.)
        let pts: Vec<[f64; 2]> = p
            .vertices
            .iter()
            .map(|v| [v.location.x, v.location.y])
            .collect();
        // Signed area ×2: > 0 ⇒ counter-clockwise.
        let mut area2 = 0.0;
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            area2 += a[0] * b[1] - b[0] * a[1];
        }
        let ccw = area2 > 0.0;
        // Ray-cast point-in-polygon for the pick point.
        let (sx, sy) = (side_pt.x as f64, side_pt.y as f64);
        let mut inside = false;
        let mut j = pts.len() - 1;
        for i in 0..pts.len() {
            let (xi, yi) = (pts[i][0], pts[i][1]);
            let (xj, yj) = (pts[j][0], pts[j][1]);
            if ((yi > sy) != (yj > sy))
                && (sx < (xj - xi) * (sy - yi) / (yj - yi) + xi)
            {
                inside = !inside;
            }
            j = i;
        }
        // left normal inward ⇔ CCW; want inward ⇔ pick is inside.
        if inside == ccw {
            1.0
        } else {
            -1.0
        }
    } else {
        // Open path: no inside/outside, so use the side of the first
        // non-degenerate segment relative to the pick.
        (0..n_segs).find_map(|i| {
            let v0 = &p.vertices[i];
            let v1 = &p.vertices[(i + 1) % n];
            let dx = v1.location.x - v0.location.x;
            let dy = v1.location.y - v0.location.y;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-12 {
                return None;
            }
            let vx = side_pt.x as f64 - v0.location.x;
            let vy = side_pt.y as f64 - v0.location.y;
            let cross = dx * vy - dy * vx;
            Some(if cross >= 0.0 { 1.0 } else { -1.0 })
        })?
    };

    // Offset each segment.  A segment may be degenerate (zero length) → None.
    struct OffSeg {
        p0: [f64; 2],
        p1: [f64; 2],
    }

    let segs: Vec<Option<OffSeg>> = (0..n_segs)
        .map(|i| {
            let v0 = &p.vertices[i];
            let v1 = &p.vertices[(i + 1) % n];
            let dx = v1.location.x - v0.location.x;
            let dy = v1.location.y - v0.location.y;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-12 {
                return None;
            }
            let ox = sign * (-dy / len) * dist;
            let oy = sign * (dx / len) * dist;
            Some(OffSeg {
                p0: [v0.location.x + ox, v0.location.y + oy],
                p1: [v1.location.x + ox, v1.location.y + oy],
            })
        })
        .collect();

    let m = segs.len();

    // Helper: corner vertex from the intersection of two consecutive offset segments.
    let corner = |prev: &OffSeg, curr: &OffSeg| -> [f64; 2] {
        isect_lines(prev.p0, prev.p1, curr.p0, curr.p1).unwrap_or([
            (prev.p1[0] + curr.p0[0]) * 0.5,
            (prev.p1[1] + curr.p0[1]) * 0.5,
        ])
    };

    let mut new_verts: Vec<LwVertex> = Vec::new();

    if p.is_closed {
        for i in 0..m {
            let prev_idx = (i + m - 1) % m;
            let prev = match &segs[prev_idx] {
                Some(s) => s,
                None => continue,
            };
            let curr = match &segs[i] {
                Some(s) => s,
                None => continue,
            };
            let pt = corner(prev, curr);
            let mut v = LwVertex::from_coords(pt[0], pt[1]);
            v.bulge = p.vertices[i].bulge;
            new_verts.push(v);
        }
    } else {
        // First vertex
        if let Some(s) = &segs[0] {
            let mut v = LwVertex::from_coords(s.p0[0], s.p0[1]);
            v.bulge = p.vertices[0].bulge;
            new_verts.push(v);
        }
        // Interior vertices
        for i in 1..m {
            let prev = match &segs[i - 1] {
                Some(s) => s,
                None => continue,
            };
            let curr = match &segs[i] {
                Some(s) => s,
                None => continue,
            };
            let pt = corner(prev, curr);
            let mut v = LwVertex::from_coords(pt[0], pt[1]);
            v.bulge = p.vertices[i].bulge;
            new_verts.push(v);
        }
        // Last vertex
        if let Some(s) = &segs[m - 1] {
            new_verts.push(LwVertex::from_coords(s.p1[0], s.p1[1]));
        }
    }

    if new_verts.len() < 2 {
        return None;
    }

    let mut new_p = p.clone();
    new_p.common.handle = Handle::NULL;
    new_p.vertices = new_verts;
    Some(EntityType::LwPolyline(new_p))
}

// ── Ellipse offset ─────────────────────────────────────────────────────────
//
// A true offset of an ellipse is a Lamé curve, not an ellipse. As an
// acceptable CAD approximation we scale both semi-axes uniformly and keep
// the same orientation, center and parameter range.  The sign of the offset
// is determined by whether side_pt is inside or outside the ellipse.

fn offset_ellipse(e: &EllipseEnt, dist: f64, side_pt: Vec3) -> Option<EntityType> {
    let a = (e.major_axis.x.powi(2) + e.major_axis.y.powi(2)).sqrt();
    if a < 1e-9 {
        return None;
    }
    let b = a * e.minor_axis_ratio;
    let nx = e.major_axis.x / a;
    let ny = e.major_axis.y / a;
    // Project side_pt onto ellipse local frame and test inside/outside.
    let rx = side_pt.x as f64 - e.center.x;
    let ry = side_pt.y as f64 - e.center.y;
    let xl = rx * nx + ry * ny;
    let yl = -rx * ny + ry * nx;
    let inside = (xl / a).powi(2) + (yl / b).powi(2) < 1.0;
    let sign = if inside { -1.0 } else { 1.0 };

    let new_a = a + sign * dist;
    let new_b = b + sign * dist;
    if new_a <= 1e-9 || new_b <= 1e-9 {
        return None;
    }

    let mut new_e = e.clone();
    new_e.common.handle = Handle::NULL;
    // Scale the major_axis vector proportionally.
    let scale = new_a / a;
    new_e.major_axis.x *= scale;
    new_e.major_axis.y *= scale;
    new_e.major_axis.z *= scale;
    new_e.minor_axis_ratio = new_b / new_a;
    Some(EntityType::Ellipse(new_e))
}

// ── Spline offset ──────────────────────────────────────────────────────────
//
// Strategy: sample the spline into N points, offset each sample point by
// `dist` along the local perpendicular (based on the finite-difference
// tangent), then fit a new spline through the offset points.

fn offset_spline(spl: &SplineEnt, dist: f64, side_pt: Vec3) -> Option<EntityType> {
    let (ts_knot, pts) = spline_sample_xy(spl, 64);
    let n = pts.len();
    if n < 2 {
        return None;
    }

    // Determine offset sign from the first non-degenerate tangent.
    let sign: f64 = (0..n - 1).find_map(|i| {
        let dx = pts[i + 1][0] - pts[i][0];
        let dy = pts[i + 1][1] - pts[i][1];
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-12 {
            return None;
        }
        let vx = side_pt.x as f64 - pts[i][0];
        let vy = side_pt.y as f64 - pts[i][1];
        let cross = dx * vy - dy * vx;
        Some(if cross >= 0.0 { 1.0 } else { -1.0 })
    })?;

    // Offset each sample point along the local normal.
    let offset_pts: Vec<acadrust::types::Vector3> = pts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            // Tangent via central / forward / backward difference.
            let (dx, dy) = if i == 0 {
                let d = [pts[1][0] - pts[0][0], pts[1][1] - pts[0][1]];
                (d[0], d[1])
            } else if i == n - 1 {
                let d = [pts[n - 1][0] - pts[n - 2][0], pts[n - 1][1] - pts[n - 2][1]];
                (d[0], d[1])
            } else {
                (
                    (pts[i + 1][0] - pts[i - 1][0]) * 0.5,
                    (pts[i + 1][1] - pts[i - 1][1]) * 0.5,
                )
            };
            let len = (dx * dx + dy * dy).sqrt().max(1e-12);
            let nx = -dy / len; // left perpendicular
            let ny = dx / len;
            let z = spl.control_points.first().map(|v| v.z).unwrap_or(0.0);
            acadrust::types::Vector3::new(p[0] + sign * nx * dist, p[1] + sign * ny * dist, z)
        })
        .collect();

    let _ = ts_knot;
    // Build a new spline from the offset control points (treat sample pts as fit pts → ctrl pts).
    let degree = spl.degree.max(1) as usize;
    let new_ctrl: Vec<acadrust::types::Vector3> = offset_pts;
    let n_ctrl = new_ctrl.len();
    let kv = truck_modeling::KnotVec::uniform_knot(degree, n_ctrl - 1);
    let mut new_spl = spl.clone();
    new_spl.common.handle = Handle::NULL;
    new_spl.control_points = new_ctrl;
    new_spl.knots = kv.iter().copied().collect();
    new_spl.fit_points.clear();
    new_spl.weights.clear();
    Some(EntityType::Spline(new_spl))
}

// ── Dispatch ───────────────────────────────────────────────────────────────

fn compute_offset(entity: &EntityType, dist: f64, side_pt: Vec3) -> Option<EntityType> {
    match entity {
        EntityType::Line(l) => offset_line(l, dist, side_pt),
        EntityType::Circle(c) => offset_circle(c, dist, side_pt),
        EntityType::Arc(a) => offset_arc(a, dist, side_pt),
        EntityType::LwPolyline(p) => offset_lwpolyline(p, dist, side_pt),
        EntityType::Ellipse(e) => offset_ellipse(e, dist, side_pt),
        EntityType::Spline(s) => offset_spline(s, dist, side_pt),
        EntityType::XLine(x) => offset_xline(x, dist, side_pt),
        _ => None,
    }
}

// ── Through-mode distance ─────────────────────────────────────────────────
//
// Nearest distance from the cursor to the entity outline, used by "through"
// mode so the offset copy passes through the cursor. Measured against the
// tessellated wire (point-to-segment), which approximates the perpendicular
// distance for every supported entity type.

fn perp_distance(entity: &EntityType, pt: Vec3) -> f64 {
    let pts = entity_wire_pts(entity);
    if pts.len() < 2 {
        return 0.0;
    }
    let px = pt.x as f64;
    let py = pt.y as f64;
    let mut best = f64::INFINITY;
    for w in pts.windows(2) {
        let ax = w[0][0] as f64;
        let ay = w[0][1] as f64;
        let bx = w[1][0] as f64;
        let by = w[1][1] as f64;
        let dx = bx - ax;
        let dy = by - ay;
        let len2 = dx * dx + dy * dy;
        let t = if len2 < 1e-12 {
            0.0
        } else {
            (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
        };
        let cx = ax + t * dx;
        let cy = ay + t * dy;
        let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        if d < best {
            best = d;
        }
    }
    best
}

// ── Wire preview points ─────────────────────────────────────────────────────

fn entity_wire_pts(e: &EntityType) -> Vec<[f32; 3]> {
    match e {
        EntityType::Line(l) => vec![
            [l.start.x as f32, l.start.y as f32, l.start.z as f32],
            [l.end.x as f32, l.end.y as f32, l.end.z as f32],
        ],
        EntityType::Circle(c) => {
            let steps = 64usize;
            (0..=steps)
                .map(|i| {
                    let a = TAU * i as f64 / steps as f64;
                    [
                        (c.center.x + c.radius * a.cos()) as f32,
                        (c.center.y + c.radius * a.sin()) as f32,
                        c.center.z as f32,
                    ]
                })
                .collect()
        }
        EntityType::Arc(a) => {
            let a0 = norm_rad(a.start_angle);
            let a1 = norm_rad(a.end_angle);
            let span = {
                let s = a1 - a0;
                if s <= 0.0 {
                    s + TAU
                } else {
                    s
                }
            };
            let steps = ((span.abs() * 20.0).ceil() as usize).max(4);
            (0..=steps)
                .map(|i| {
                    let ang = a0 + span * (i as f64 / steps as f64);
                    [
                        (a.center.x + a.radius * ang.cos()) as f32,
                        (a.center.y + a.radius * ang.sin()) as f32,
                        a.center.z as f32,
                    ]
                })
                .collect()
        }
        EntityType::LwPolyline(p) => lwpolyline_pts(p),
        EntityType::Ellipse(e) => {
            let a = (e.major_axis.x.powi(2) + e.major_axis.y.powi(2)).sqrt();
            if a < 1e-9 {
                return vec![];
            }
            let b = a * e.minor_axis_ratio;
            let nx = e.major_axis.x / a;
            let ny = e.major_axis.y / a;
            let t0 = e.start_parameter;
            let mut t1 = e.end_parameter;
            if t1 <= t0 {
                t1 += TAU;
            }
            let span = t1 - t0;
            let steps = ((span.abs() * 20.0).ceil() as usize).max(4);
            (0..=steps)
                .map(|i| {
                    let t = t0 + span * (i as f64 / steps as f64);
                    let lx = a * t.cos();
                    let ly = b * t.sin();
                    [
                        (e.center.x + lx * nx - ly * ny) as f32,
                        (e.center.y + lx * ny + ly * nx) as f32,
                        e.center.z as f32,
                    ]
                })
                .collect()
        }
        EntityType::Spline(s) => spline_pts_wire(s),
        EntityType::XLine(x) => {
            // Infinite in both directions — represent it as a very long segment
            // for hit-testing and the offset preview. Long enough to read as
            // infinite at any working zoom; the committed XLine renders true.
            const HL: f64 = 1.0e6;
            let (bx, by, bz) = (x.base_point.x, x.base_point.y, x.base_point.z);
            let (dx, dy, dz) = (x.direction.x, x.direction.y, x.direction.z);
            vec![
                [(bx - dx * HL) as f32, (by - dy * HL) as f32, (bz - dz * HL) as f32],
                [(bx + dx * HL) as f32, (by + dy * HL) as f32, (bz + dz * HL) as f32],
            ]
        }
        _ => vec![],
    }
}

/// Tessellate a LwPolyline into wire points (straight segments + arc bulges).
fn lwpolyline_pts(p: &LwPolyline) -> Vec<[f32; 3]> {
    let n = p.vertices.len();
    if n < 2 {
        return vec![];
    }
    let z = p.elevation as f32;
    let n_segs = if p.is_closed { n } else { n - 1 };
    let mut pts: Vec<[f32; 3]> = Vec::new();

    for i in 0..n_segs {
        let v0 = &p.vertices[i];
        let v1 = &p.vertices[(i + 1) % n];
        let x0 = v0.location.x;
        let y0 = v0.location.y;
        let x1 = v1.location.x;
        let y1 = v1.location.y;

        if pts.is_empty() {
            pts.push([x0 as f32, y0 as f32, z]);
        }

        if v0.bulge.abs() < 1e-10 {
            pts.push([x1 as f32, y1 as f32, z]);
        } else {
            // Arc from bulge
            let b = v0.bulge;
            let chord_x = x1 - x0;
            let chord_y = y1 - y0;
            let chord_len = (chord_x * chord_x + chord_y * chord_y).sqrt();
            if chord_len < 1e-12 {
                pts.push([x1 as f32, y1 as f32, z]);
                continue;
            }

            let b2 = b * b;
            let r = chord_len * (1.0 + b2) / (4.0 * b.abs());
            let d = r * (1.0 - b2) / (1.0 + b2);
            let mx = (x0 + x1) * 0.5;
            let my = (y0 + y1) * 0.5;
            let perp_x = -chord_y / chord_len;
            let perp_y = chord_x / chord_len;
            let sign = b.signum();
            let cx = mx + sign * d * perp_x;
            let cy = my + sign * d * perp_y;

            let a0 = norm_rad((y0 - cy).atan2(x0 - cx));
            let a1 = norm_rad((y1 - cy).atan2(x1 - cx));
            let span = if b > 0.0 {
                let s = a1 - a0;
                if s <= 0.0 {
                    s + TAU
                } else {
                    s
                }
            } else {
                let s = a0 - a1;
                if s <= 0.0 {
                    s + TAU
                } else {
                    s
                }
            };
            let steps = ((span.abs() * 20.0).ceil() as usize).max(4);
            for j in 1..=steps {
                let t = j as f64 / steps as f64;
                let ang = if b > 0.0 {
                    a0 + span * t
                } else {
                    a0 - span * t
                };
                pts.push([(cx + r * ang.cos()) as f32, (cy + r * ang.sin()) as f32, z]);
            }
        }
    }

    if p.is_closed {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    pts
}

// ── Command implementation ─────────────────────────────────────────────────

enum Step {
    /// Classic first step (#418): type the offset distance, press Enter /
    /// Space to accept the last one, or choose Through mode.
    Distance,
    /// Pick the object to offset. `locked == None` is "through" mode: the
    /// magnitude follows the cursor (perpendicular distance to the object).
    SelectObject { locked: Option<f64> },
    PickSide {
        /// The object(s) being offset — one from a pick, or the whole
        /// pre-selection when OFFSET starts with objects selected (#422).
        targets: Vec<EntityType>,
        locked: Option<f64>,
        /// True when the targets came from the pre-selection: the commit ends
        /// the command instead of looping back for another pick.
        from_selection: bool,
    },
}

pub struct OffsetCommand {
    step: Step,
    all_entities: Vec<EntityType>,
    /// Pre-selected offsettable objects (pick-first, #422); consumed when the
    /// distance step resolves.
    preselected: Vec<EntityType>,
}

/// The entity types `compute_offset` can offset.
pub fn is_offsettable(e: &EntityType) -> bool {
    matches!(
        e,
        EntityType::Line(_)
            | EntityType::Circle(_)
            | EntityType::Arc(_)
            | EntityType::LwPolyline(_)
            | EntityType::Ellipse(_)
            | EntityType::Spline(_)
            | EntityType::XLine(_)
    )
}

impl OffsetCommand {
    pub fn new(all_entities: Vec<EntityType>) -> Self {
        Self {
            step: Step::Distance,
            all_entities,
            preselected: Vec::new(),
        }
    }

    /// Pick-first flow (#422): the distance step still comes first, then the
    /// pre-selected objects go straight to the side step.
    pub fn with_selection(all_entities: Vec<EntityType>, targets: Vec<EntityType>) -> Self {
        Self {
            step: Step::Distance,
            all_entities,
            preselected: targets,
        }
    }

    /// Leave the distance step with the given mode (Some = locked distance,
    /// None = through): pre-selected objects jump to the side step, otherwise
    /// the object-pick loop starts.
    fn advance_from_distance(&mut self, locked: Option<f64>) -> CmdResult {
        if self.preselected.is_empty() {
            self.step = Step::SelectObject { locked };
        } else {
            self.step = Step::PickSide {
                targets: std::mem::take(&mut self.preselected),
                locked,
                from_selection: true,
            };
        }
        CmdResult::NeedPoint
    }
}

impl CadCommand for OffsetCommand {
    fn name(&self) -> &'static str {
        "OFFSET"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::Distance => format!(
                "OFFSET  Specify offset distance or [Through] <{:.4}>:",
                defaults::get_offset_dist()
            ),
            Step::SelectObject { .. } => {
                "OFFSET  Select object to offset (Enter to finish):".into()
            }
            Step::PickSide { targets, locked, .. } => {
                let n = if targets.len() > 1 {
                    format!(" ({} objects)", targets.len())
                } else {
                    String::new()
                };
                match locked {
                    Some(d) => format!("OFFSET{n}  Click side  [distance {d:.4}]:"),
                    None => format!("OFFSET{n}  Click through point:"),
                }
            }
        }
    }

    fn options(&self) -> Vec<crate::command::CmdOption> {
        match &self.step {
            Step::Distance => vec![
                crate::command::CmdOption::new("Through", "T"),
                crate::command::CmdOption::enter(&format!(
                    "{:.4}",
                    defaults::get_offset_dist()
                )),
            ],
            _ => Vec::new(),
        }
    }

    fn needs_entity_pick(&self) -> bool {
        matches!(self.step, Step::SelectObject { .. })
    }

    fn on_entity_pick(&mut self, handle: Handle, _pt: DVec3) -> CmdResult {
        let locked = match &self.step {
            Step::SelectObject { locked } => *locked,
            _ => return CmdResult::NeedPoint,
        };
        if handle.is_null() {
            return CmdResult::NeedPoint;
        }

        let entity = self
            .all_entities
            .iter()
            .find(|e| e.common().handle == handle)
            .cloned();

        // Accept every type compute_offset can offset — including XLine (#296),
        // and Ellipse/Spline whose offset functions existed but weren't reachable.
        match entity {
            Some(e) if is_offsettable(&e) => {
                self.step = Step::PickSide {
                    targets: vec![e],
                    locked,
                    from_selection: false,
                };
                CmdResult::NeedPoint
            }
            _ => CmdResult::NeedPoint,
        }
    }

    // The distance step takes a typed magnitude; the side step accepts one
    // too, re-locking the distance mid-command.
    fn wants_text_input(&self) -> bool {
        matches!(self.step, Step::Distance | Step::PickSide { .. })
    }

    fn dyn_field(&self) -> crate::command::DynField {
        match self.step {
            Step::Distance | Step::PickSide { .. } => crate::command::DynField::Scalar,
            _ => crate::command::DynField::Point,
        }
    }

    fn dyn_live_value(&self, cursor: DVec3) -> Option<f64> {
        match &self.step {
            Step::Distance => Some(defaults::get_offset_dist()),
            Step::PickSide { targets, locked, .. } => Some(locked.unwrap_or_else(|| {
                targets
                    .first()
                    .map(|e| perp_distance(e, cursor.as_vec3()))
                    .unwrap_or(0.0)
            })),
            _ => None,
        }
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let t = text.trim().replace(',', ".");
        match &mut self.step {
            Step::Distance => {
                if t.eq_ignore_ascii_case("t") || t.eq_ignore_ascii_case("through") {
                    return Some(self.advance_from_distance(None));
                }
                if let Ok(d) = t.parse::<f64>() {
                    let d = d.abs().max(1e-9);
                    defaults::set_offset_dist(d);
                    return Some(self.advance_from_distance(Some(d)));
                }
                Some(CmdResult::NeedPoint)
            }
            Step::PickSide { locked, .. } => {
                if !t.is_empty() {
                    if let Ok(d) = t.parse::<f64>() {
                        let d = d.abs().max(1e-9);
                        defaults::set_offset_dist(d);
                        *locked = Some(d);
                    }
                }
                // Stay on the side step — the click chooses which side.
                Some(CmdResult::NeedPoint)
            }
            _ => None,
        }
    }

    fn on_hover_entity(&mut self, handle: Handle, _pt: DVec3) -> Vec<WireModel> {
        if handle.is_null() || !matches!(self.step, Step::SelectObject { .. }) {
            return vec![];
        }
        if let Some(entity) = self
            .all_entities
            .iter()
            .find(|e| e.common().handle == handle)
        {
            let pts = entity_wire_pts(entity);
            if !pts.is_empty() {
                return vec![WireModel::solid(
                    "offset_hover".into(),
                    pts,
                    WireModel::CYAN,
                    false,
                )];
            }
        }
        vec![]
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        let (locked, targets, from_selection) = match &self.step {
            Step::PickSide {
                locked,
                targets,
                from_selection,
            } => (*locked, targets.clone(), *from_selection),
            _ => return CmdResult::NeedPoint,
        };
        // Each target offsets by its own through-distance (or the locked
        // magnitude), toward the clicked side.
        let mut news: Vec<EntityType> = Vec::new();
        for entity in &targets {
            let mag = locked.unwrap_or_else(|| perp_distance(entity, pt.as_vec3()));
            if mag < 1e-9 {
                continue;
            }
            if let Some(new_entity) = compute_offset(entity, mag, pt.as_vec3()) {
                news.push(new_entity);
            }
        }
        if news.is_empty() {
            return CmdResult::NeedPoint;
        }
        if from_selection {
            // Pre-selection commit ends the command in one undo step.
            return CmdResult::ReplaceMany(vec![], news);
        }
        // Classic loop (#418): commit this offset and go back to the object
        // pick at the same distance, until Enter / Esc finishes.
        self.step = Step::SelectObject { locked };
        if news.len() == 1 {
            CmdResult::CommitEntity(news.pop().unwrap())
        } else {
            CmdResult::ReplaceMany(vec![], news)
        }
    }

    fn on_preview_wires(&mut self, pt: DVec3) -> Vec<WireModel> {
        let (locked, targets) = match &self.step {
            Step::PickSide { locked, targets, .. } => (*locked, targets.clone()),
            _ => return vec![],
        };
        let mut wires = Vec::new();
        for (n, entity) in targets.iter().enumerate() {
            let mag = locked.unwrap_or_else(|| perp_distance(entity, pt.as_vec3()));
            if mag < 1e-9 {
                continue;
            }
            if let Some(result) = compute_offset(entity, mag, pt.as_vec3()) {
                let pts = entity_wire_pts(&result);
                if !pts.is_empty() {
                    wires.push(WireModel::solid(
                        format!("offset_preview_{n}"),
                        pts,
                        WireModel::CYAN,
                        false,
                    ));
                }
            }
        }
        wires
    }

    fn on_enter(&mut self) -> CmdResult {
        match &self.step {
            // Enter / Space on the distance step accepts the last distance —
            // the "repeat the same value with just Space" flow (#418).
            Step::Distance => {
                let d = defaults::get_offset_dist();
                self.advance_from_distance(Some(d.abs().max(1e-9)))
            }
            _ => CmdResult::Cancel,
        }
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["OFFSET"] });  // OffsetCommand

#[cfg(test)]
mod offset_tests {
    use super::*;
    use acadrust::types::Vector2;

    fn rect(corners: &[[f64; 2]]) -> LwPolyline {
        LwPolyline {
            vertices: corners
                .iter()
                .map(|&[x, y]| LwVertex::new(Vector2::new(x, y)))
                .collect(),
            is_closed: true,
            ..Default::default()
        }
    }

    /// Offset `corners` by 10 toward `side` and return the result's XY bounds.
    fn offset_bbox(corners: &[[f64; 2]], side: Vec3) -> [f64; 4] {
        let out = offset_lwpolyline(&rect(corners), 10.0, side);
        let Some(EntityType::LwPolyline(r)) = out else {
            panic!("offset did not return an lwpolyline");
        };
        let (mut minx, mut miny, mut maxx, mut maxy) =
            (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for v in &r.vertices {
            minx = minx.min(v.location.x);
            miny = miny.min(v.location.y);
            maxx = maxx.max(v.location.x);
            maxy = maxy.max(v.location.y);
        }
        [minx, miny, maxx, maxy]
    }

    fn approx(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    // A pick *outside* the loop must offset outward and a pick *inside* must
    // offset inward, regardless of the rectangle's winding. Regression for the
    // first-segment sign heuristic, which sent a CCW rectangle inward when the
    // outward pick sat beside it (issue 166).
    #[test]
    fn rect_offset_direction_is_winding_independent() {
        let ccw = [[0.0, 0.0], [100.0, 0.0], [100.0, 60.0], [0.0, 60.0]];
        let cw = [[0.0, 0.0], [0.0, 60.0], [100.0, 60.0], [100.0, 0.0]];
        let out = [-10.0, -10.0, 110.0, 70.0];
        let inn = [10.0, 10.0, 90.0, 50.0];
        // Pick beside the rectangle (outside, mid-height) → outward, both windings.
        assert!(approx(offset_bbox(&ccw, Vec3::new(-10.0, 30.0, 0.0)), out));
        assert!(approx(offset_bbox(&cw, Vec3::new(-10.0, 30.0, 0.0)), out));
        // Pick inside → inward, both windings.
        assert!(approx(offset_bbox(&ccw, Vec3::new(50.0, 30.0, 0.0)), inn));
        assert!(approx(offset_bbox(&cw, Vec3::new(50.0, 30.0, 0.0)), inn));
        // Pick clearly outside below → outward (the case that worked before).
        assert!(approx(offset_bbox(&ccw, Vec3::new(50.0, -10.0, 0.0)), out));
    }
}
