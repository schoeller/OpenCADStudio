// Circle tool — ribbon dropdown + all OpenCADStudio circle creation methods.
//
// Methods:
//   CIRCLE     — Center, Radius   (default)
//   CIRCLE_CD  — Center, Diameter
//   CIRCLE_2P  — 2-Point (two endpoints of diameter)
//   CIRCLE_3P  — 3-Point (circumscribed circle through 3 points)
//   CIRCLE_TTR — Tan, Tan, Radius (two tangent objects + radius)
//   CIRCLE_TTT — Tan, Tan, Tan    (inscribed circle tangent to three objects)

use acadrust::types::Vector3;
use acadrust::{Circle, EntityType};

use crate::command::{CadCommand, CmdResult, DynField, TangentObject};
use crate::modules::draw::defaults;
use crate::modules::IconKind;
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

const TAU: f64 = std::f64::consts::TAU;

// ── Per-method SVG icons ───────────────────────────────────────────────────

const ICON_CR: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/circle/circle_cr.svg"
));
const ICON_CD: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/circle/circle_cd.svg"
));
const ICON_2P: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/circle/circle_2p.svg"
));
const ICON_3P: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/circle/circle_3p.svg"
));
const ICON_TTR: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/circle/circle_ttr.svg"
));
const ICON_TTT: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/circle/circle_ttt.svg"
));

// ── Dropdown metadata (used by ribbon.rs) ─────────────────────────────────

pub const DROPDOWN_ID: &str = "CIRCLE";

pub const DROPDOWN_ITEMS: &[(&str, &str, IconKind)] = &[
    ("CIRCLE", "Center, Radius", ICON_CR),
    ("CIRCLE_CD", "Center, Diameter", ICON_CD),
    ("CIRCLE_2P", "2-Point", ICON_2P),
    ("CIRCLE_3P", "3-Point", ICON_3P),
    ("CIRCLE_TTR", "Tan, Tan, Radius", ICON_TTR),
    ("CIRCLE_TTT", "Tan, Tan, Tan", ICON_TTT),
];

/// Default icon — shown until first use (falls back to Center, Radius).
pub const ICON: IconKind = ICON_CR;

// ── Shared geometry ────────────────────────────────────────────────────────

fn circle_wire(center: DVec3, radius: f64) -> WireModel {
    let segs = 64u32;
    let mut pts: Vec<[f32; 3]> = (0..=segs)
        .map(|i| {
            let a = (i as f64) * TAU / segs as f64;
            [
                (center.x + radius * a.cos()) as f32,
                (center.y + radius * a.sin()) as f32,
                center.z as f32,
            ]
        })
        .collect();
    if let Some(first) = pts.first().cloned() {
        pts.push(first);
    }
    WireModel::solid("rubber_band".into(), pts, WireModel::CYAN, false)
}

fn make_circle(center: DVec3, radius: f64) -> EntityType {
    EntityType::Circle(Circle {
        center: Vector3::new(center.x, center.y, center.z),
        radius,
        ..Default::default()
    })
}

/// Circumscribed circle through three points.
/// Returns `None` if the points are collinear.
fn circumcircle(a: DVec3, b: DVec3, c: DVec3) -> Option<(DVec3, f64)> {
    let ax = a.x;
    let ay = a.y;
    let bx = b.x;
    let by = b.y;
    let cx = c.x;
    let cy = c.y;
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-9 {
        return None;
    }
    let ux = ((ax * ax + ay * ay) * (by - cy)
        + (bx * bx + by * by) * (cy - ay)
        + (cx * cx + cy * cy) * (ay - by))
        / d;
    let uy = ((ax * ax + ay * ay) * (cx - bx)
        + (bx * bx + by * by) * (ax - cx)
        + (cx * cx + cy * cy) * (bx - ax))
        / d;
    let center = DVec3::new(ux, uy, a.z);
    Some((center, center.distance(a)))
}

// ── Command: Center, Radius ────────────────────────────────────────────────

pub struct CircleCommand {
    step: StepCR,
    default_r: f32,
}
enum StepCR {
    Center,
    Radius(DVec3),
}

impl CircleCommand {
    pub fn new() -> Self {
        Self {
            step: StepCR::Center,
            default_r: defaults::get_circle_radius(),
        }
    }
}

impl CadCommand for CircleCommand {
    fn name(&self) -> &'static str {
        "CIRCLE"
    }
    fn prompt(&self) -> String {
        match &self.step {
            StepCR::Center => "CIRCLE  Specify center point:".into(),
            StepCR::Radius(c) => format!(
                "CIRCLE  Specify radius or type value  <{:.4}>  [center ({:.3},{:.3})]:",
                self.default_r, c.x, c.y
            ),
        }
    }
    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match &self.step {
            StepCR::Center => {
                self.step = StepCR::Radius(pt);
                CmdResult::NeedPoint
            }
            StepCR::Radius(c) => {
                let r = c.distance(pt);
                defaults::set_circle_radius(r as f32);
                CmdResult::CommitAndExit(make_circle(*c, r))
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        if let StepCR::Radius(c) = &self.step {
            let c = *c;
            let r = self.default_r;
            return CmdResult::CommitAndExit(make_circle(c, r as f64));
        }
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        if let StepCR::Radius(c) = &self.step {
            Some(circle_wire(*c, c.distance(pt)))
        } else {
            None
        }
    }
    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if let StepCR::Radius(c) = &self.step {
            let r: f64 = text.trim().replace(',', ".").parse().ok()?;
            if r > 0.0 {
                defaults::set_circle_radius(r as f32);
                return Some(CmdResult::CommitAndExit(make_circle(*c, r)));
            }
        }
        None
    }
    fn dyn_field(&self) -> DynField {
        match self.step {
            StepCR::Center => DynField::Point,
            StepCR::Radius(_) => DynField::Distance,
        }
    }

    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        match self.step {
            // Center is a normal first-point pick.
            StepCR::Center => None,
            // Radius: a single R value with one dotted line from the centre to
            // the cursor (no angle, no axis legs).
            StepCR::Radius(c) => Some(DynSpec {
                anchor: DynAnchor::Point(c),
                fields: vec![DynFieldSpec::new(DynRole::Radius)],
                guide: DynGuide::Radius,
                ref_point: None,
            }),
        }
    }
}

// ── Command: Center, Diameter ──────────────────────────────────────────────

pub struct CircleCDCommand {
    step: StepCR,
    default_d: f32,
}

impl CircleCDCommand {
    pub fn new() -> Self {
        Self {
            step: StepCR::Center,
            default_d: defaults::get_circle_diam(),
        }
    }
}

impl CadCommand for CircleCDCommand {
    fn name(&self) -> &'static str {
        "CIRCLE_CD"
    }
    fn prompt(&self) -> String {
        match &self.step {
            StepCR::Center => "CIRCLE CD  Specify center point:".into(),
            StepCR::Radius(c) => format!(
                "CIRCLE CD  Specify diameter or type value  <{:.4}>  [center ({:.3},{:.3})]:",
                self.default_d, c.x, c.y
            ),
        }
    }
    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match &self.step {
            StepCR::Center => {
                self.step = StepCR::Radius(pt);
                CmdResult::NeedPoint
            }
            StepCR::Radius(c) => {
                let d = c.distance(pt) * 2.0;
                defaults::set_circle_diam(d as f32);
                CmdResult::CommitAndExit(make_circle(*c, d / 2.0))
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        if let StepCR::Radius(c) = &self.step {
            let c = *c;
            let d = self.default_d;
            return CmdResult::CommitAndExit(make_circle(c, (d / 2.0) as f64));
        }
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        // Preview radius = distance to cursor; on commit that distance becomes the radius (diameter = 2x).
        if let StepCR::Radius(c) = &self.step {
            Some(circle_wire(*c, c.distance(pt)))
        } else {
            None
        }
    }
    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if let StepCR::Radius(c) = &self.step {
            let d: f64 = text.trim().replace(',', ".").parse().ok()?;
            if d > 0.0 {
                defaults::set_circle_diam(d as f32);
                return Some(CmdResult::CommitAndExit(make_circle(*c, d / 2.0)));
            }
        }
        None
    }

    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        match self.step {
            StepCR::Center => None,
            // Diameter: the box shows/accepts twice the cursor radius; resolved
            // back to a radius point by the host (role scaling).
            StepCR::Radius(c) => Some(DynSpec {
                anchor: DynAnchor::Point(c),
                fields: vec![DynFieldSpec::new(DynRole::Diameter)],
                guide: DynGuide::Radius,
                ref_point: None,
            }),
        }
    }
}

// ── Command: 2-Point ──────────────────────────────────────────────────────

pub struct Circle2PCommand {
    p1: Option<DVec3>,
}

impl Circle2PCommand {
    pub fn new() -> Self {
        Self { p1: None }
    }
}

impl CadCommand for Circle2PCommand {
    fn name(&self) -> &'static str {
        "CIRCLE_2P"
    }
    fn prompt(&self) -> String {
        if self.p1.is_none() {
            "CIRCLE 2P  Specify first end of diameter:".into()
        } else {
            "CIRCLE 2P  Specify second end of diameter:".into()
        }
    }
    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match self.p1 {
            None => {
                self.p1 = Some(pt);
                CmdResult::NeedPoint
            }
            Some(p1) => {
                let center = (p1 + pt) * 0.5;
                let radius = p1.distance(pt) / 2.0;
                CmdResult::CommitAndExit(make_circle(center, radius))
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        let p1 = self.p1?;
        let center = (p1 + pt) * 0.5;
        let radius = p1.distance(pt) / 2.0;
        Some(circle_wire(center, radius))
    }
}

// ── Command: 3-Point ──────────────────────────────────────────────────────

pub struct Circle3PCommand {
    pts: Vec<DVec3>,
}

impl Circle3PCommand {
    pub fn new() -> Self {
        Self { pts: Vec::new() }
    }
}

impl CadCommand for Circle3PCommand {
    fn name(&self) -> &'static str {
        "CIRCLE_3P"
    }
    fn prompt(&self) -> String {
        match self.pts.len() {
            0 => "CIRCLE 3P  Specify first point:".into(),
            1 => "CIRCLE 3P  Specify second point:".into(),
            _ => "CIRCLE 3P  Specify third point:".into(),
        }
    }
    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        self.pts.push(pt);
        if self.pts.len() < 3 {
            return CmdResult::NeedPoint;
        }
        let (a, b, c) = (self.pts[0], self.pts[1], self.pts[2]);
        match circumcircle(a, b, c) {
            Some((center, radius)) => CmdResult::CommitAndExit(make_circle(center, radius)),
            None => {
                self.pts.pop();
                CmdResult::NeedPoint
            } // collinear — ask again
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        match self.pts.len() {
            0 => None,
            1 => {
                // Show circle preview with p1→cursor as diameter (same as 2P).
                let p1 = self.pts[0];
                let center = (p1 + pt) * 0.5;
                let radius = p1.distance(pt) / 2.0;
                Some(circle_wire(center, radius))
            }
            _ => {
                // Show circumcircle preview if non-collinear, else polyline.
                let (a, b) = (self.pts[0], self.pts[1]);
                if let Some((center, radius)) = circumcircle(a, b, pt) {
                    Some(circle_wire(center, radius))
                } else {
                    Some(WireModel::solid(
                        "rubber_band".into(),
                        vec![
                            [a.x as f32, a.y as f32, a.z as f32],
                            [b.x as f32, b.y as f32, b.z as f32],
                            [pt.x as f32, pt.y as f32, pt.z as f32],
                        ],
                        WireModel::CYAN,
                        false,
                    ))
                }
            }
        }
    }
}

// ── 2-D geometry for TTR/TTT ────────────────────────────────────

#[derive(Clone, Copy)]
struct Line2D {
    a: f64,
    b: f64,
    c: f64,
} // ax + by + c = 0, a²+b²=1

impl Line2D {
    fn from_obj(p1: DVec3, p2: DVec3) -> Self {
        let dx = p2.x - p1.x;
        let dy = p2.y - p1.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            return Self {
                a: 1.0,
                b: 0.0,
                c: -p1.x,
            };
        }
        let a = -dy / len;
        let b = dx / len;
        Self {
            a,
            b,
            c: -(a * p1.x + b * p1.y),
        }
    }
}

fn line_line_isect(l1: Line2D, l2: Line2D) -> Option<DVec3> {
    let det = l1.a * l2.b - l2.a * l1.b;
    if det.abs() < 1e-9 {
        return None;
    }
    let x = (-l1.c * l2.b + l2.c * l1.b) / det;
    let y = (-l1.a * l2.c + l2.a * l1.c) / det;
    Some(DVec3::new(x, y, 0.0))
}

fn solve_quadratic(a: f64, b: f64, c: f64) -> Vec<f64> {
    if a.abs() < 1e-9 {
        if b.abs() < 1e-9 {
            return vec![];
        }
        return vec![-c / b];
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return vec![];
    }
    let sq = disc.sqrt();
    vec![(-b - sq) / (2.0 * a), (-b + sq) / (2.0 * a)]
}

fn best_of(candidates: &[DVec3], hint: DVec3) -> Option<DVec3> {
    candidates.iter().copied().min_by(|a, b| {
        a.distance(hint)
            .partial_cmp(&b.distance(hint))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn best_circle_of(candidates: &[(DVec3, f64)], hint: DVec3) -> Option<(DVec3, f64)> {
    candidates
        .iter()
        .copied()
        .filter(|&(_, r)| r > 1e-4)
        .min_by(|(a, _), (b, _)| {
            a.distance(hint)
                .partial_cmp(&b.distance(hint))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// All candidate circle centers tangent to two objects with radius r.
fn ttr_candidates(obj1: TangentObject, obj2: TangentObject, r: f64) -> Vec<DVec3> {
    match (obj1, obj2) {
        (TangentObject::Line { p1: a, p2: b }, TangentObject::Line { p1: c, p2: d }) => {
            let l1 = Line2D::from_obj(a, b);
            let l2 = Line2D::from_obj(c, d);
            let mut out = Vec::new();
            for s1 in [-r, r] {
                for s2 in [-r, r] {
                    let ol1 = Line2D { c: l1.c + s1, ..l1 };
                    let ol2 = Line2D { c: l2.c + s2, ..l2 };
                    if let Some(pt) = line_line_isect(ol1, ol2) {
                        out.push(pt);
                    }
                }
            }
            out
        }
        (
            TangentObject::Line { p1, p2 },
            TangentObject::Circle {
                center: cp,
                radius: cr,
            },
        )
        | (
            TangentObject::Circle {
                center: cp,
                radius: cr,
            },
            TangentObject::Line { p1, p2 },
        ) => {
            let l = Line2D::from_obj(p1, p2);
            ttr_lc_candidates(l, cp, cr, r)
        }
        (
            TangentObject::Circle {
                center: c1,
                radius: r1,
            },
            TangentObject::Circle {
                center: c2,
                radius: r2,
            },
        ) => ttr_cc_candidates(c1, r1, c2, r2, r),
    }
}

fn ttr_lc_candidates(l: Line2D, cp: DVec3, cr: f64, r: f64) -> Vec<DVec3> {
    let mut out = Vec::new();
    for s1 in [-1.0f64, 1.0] {
        for s2 in [-1.0f64, 1.0] {
            let rho = cr + s2 * r;
            if rho < 0.0 {
                continue;
            }
            let c_off = l.c + s1 * r;
            // The candidate center (cx, cy) satisfies:
            //   l.a*cx + l.b*cy + c_off = 0
            //   (cx - cp.x)^2 + (cy - cp.y)^2 = rho^2
            // Parameterise cx = -l.b*t - l.a*c_off, cy = l.a*t - l.b*c_off
            // where t is the free parameter along the offset line.
            // Substituting into the circle equation:
            //   (-l.b*t - l.a*c_off - cp.x)^2 + (l.a*t - l.b*c_off - cp.y)^2 = rho^2
            // Let alpha = l.a*c_off + cp.x, beta = l.b*c_off + cp.y (using sign convention)
            // Actually: u = -l.a*c_off - cp.x, v = -l.b*c_off - cp.y
            let u = -(l.a * c_off) - cp.x;
            let v = -(l.b * c_off) - cp.y;
            // (-l.b*t + u)^2 + (l.a*t + v)^2 = rho^2
            // (l.b^2 + l.a^2)*t^2 + 2*(-l.b*u + l.a*v)*t + (u^2 + v^2 - rho^2) = 0
            // Since a^2 + b^2 = 1:
            let qa = 1.0f64;
            let qb = 2.0 * (-l.b * u + l.a * v);
            let qc = u * u + v * v - rho * rho;
            for t in solve_quadratic(qa, qb, qc) {
                let cx = -l.b * t - l.a * c_off;
                let cy = l.a * t - l.b * c_off;
                out.push(DVec3::new(cx, cy, 0.0));
            }
        }
    }
    out
}

fn ttr_cc_candidates(c1: DVec3, r1: f64, c2: DVec3, r2: f64, r: f64) -> Vec<DVec3> {
    let mut out = Vec::new();
    for s1 in [-1.0f64, 1.0] {
        for s2 in [-1.0f64, 1.0] {
            let rho1 = r1 + s1 * r;
            let rho2 = r2 + s2 * r;
            if rho1 < 0.0 || rho2 < 0.0 {
                continue;
            }
            let ax = c2.x - c1.x;
            let ay = c2.y - c1.y;
            let k = 0.5
                * (rho1 * rho1 - rho2 * rho2 + c2.x * c2.x + c2.y * c2.y
                    - c1.x * c1.x
                    - c1.y * c1.y);
            let a2 = ax * ax + ay * ay;
            if a2 < 1e-12 {
                continue;
            }
            if ay.abs() >= ax.abs() {
                let iy = 1.0 / ay;
                let qa = 1.0 + ax * ax * iy * iy;
                let p = k * iy - c1.y;
                let qb = -2.0 * (c1.x + ax * iy * p);
                let qc = c1.x * c1.x + p * p - rho1 * rho1;
                let disc = qb * qb - 4.0 * qa * qc;
                if disc < 0.0 {
                    continue;
                }
                for sign in [-1.0f64, 1.0] {
                    let cx = (-qb + sign * disc.sqrt()) / (2.0 * qa);
                    let cy = (k - ax * cx) * iy;
                    out.push(DVec3::new(cx, cy, 0.0));
                }
            } else {
                let ix = 1.0 / ax;
                let qa = ay * ay * ix * ix + 1.0;
                let p = k * ix - c1.x;
                let qb = -2.0 * (ay * ix * p + c1.y);
                let qc = p * p + c1.y * c1.y - rho1 * rho1;
                let disc = qb * qb - 4.0 * qa * qc;
                if disc < 0.0 {
                    continue;
                }
                for sign in [-1.0f64, 1.0] {
                    let cy = (-qb + sign * disc.sqrt()) / (2.0 * qa);
                    let cx = (k - ay * cy) * ix;
                    out.push(DVec3::new(cx, cy, 0.0));
                }
            }
        }
    }
    out
}

/// Unified TTT solver: circle tangent to three objects. Returns all (center, radius) candidates.
fn ttt_candidates(
    obj1: TangentObject,
    obj2: TangentObject,
    obj3: TangentObject,
) -> Vec<(DVec3, f64)> {
    let objs = [obj1, obj2, obj3];
    let mut results = Vec::new();
    let sign_combos: [[f64; 3]; 8] = [
        [-1., -1., -1.],
        [-1., -1., 1.],
        [-1., 1., -1.],
        [-1., 1., 1.],
        [1., -1., -1.],
        [1., -1., 1.],
        [1., 1., -1.],
        [1., 1., 1.],
    ];
    for eps in &sign_combos {
        for (center, r) in ttt_solve_sign(&objs, eps) {
            if r > 1e-4 {
                results.push((center, r));
            }
        }
    }
    results
}

// LinEq: lx*cx + ly*cy + lr*r = k
struct LinEq {
    lx: f64,
    ly: f64,
    lr: f64,
    k: f64,
}

fn ttt_solve_sign(objs: &[TangentObject; 3], eps: &[f64; 3]) -> Vec<(DVec3, f64)> {
    let mut lin_eqs: Vec<LinEq> = Vec::new();
    let mut circle_idx: Vec<usize> = Vec::new();

    for (i, &obj) in objs.iter().enumerate() {
        match obj {
            TangentObject::Line { p1, p2 } => {
                let l = Line2D::from_obj(p1, p2);
                lin_eqs.push(LinEq {
                    lx: l.a,
                    ly: l.b,
                    lr: -eps[i],
                    k: -l.c,
                });
            }
            TangentObject::Circle { .. } => {
                circle_idx.push(i);
            }
        }
    }

    // Circle-pair differences → additional linear equations
    for j in 1..circle_idx.len() {
        let i0 = circle_idx[0];
        let i1 = circle_idx[j];
        if let (
            TangentObject::Circle {
                center: p0,
                radius: r0,
            },
            TangentObject::Circle {
                center: p1,
                radius: r1,
            },
        ) = (objs[i0], objs[i1])
        {
            let lx = 2.0 * (p1.x - p0.x);
            let ly = 2.0 * (p1.y - p0.y);
            let lr = -2.0 * (r0 * eps[i0] - r1 * eps[i1]);
            let k = (r0 * r0 - r1 * r1) + p1.x * p1.x + p1.y * p1.y - p0.x * p0.x - p0.y * p0.y;
            lin_eqs.push(LinEq { lx, ly, lr, k });
        }
    }

    if lin_eqs.len() < 2 {
        return vec![];
    }

    let e0 = &lin_eqs[0];
    let e1 = &lin_eqs[1];
    let det = e0.lx * e1.ly - e1.lx * e0.ly;
    if det.abs() < 1e-9 {
        return vec![];
    }

    // cx = a_cx + b_cx*r,  cy = a_cy + b_cy*r
    let a_cx = (e0.k * e1.ly - e1.k * e0.ly) / det;
    let b_cx = -(e0.lr * e1.ly - e1.lr * e0.ly) / det;
    let a_cy = (e0.lx * e1.k - e1.lx * e0.k) / det;
    let b_cy = -(e0.lx * e1.lr - e1.lx * e0.lr) / det;

    let r_vals: Vec<f64> = if lin_eqs.len() >= 3 {
        let e2 = &lin_eqs[2];
        let r_coeff = b_cx * e2.lx + b_cy * e2.ly + e2.lr;
        let r_const = e2.k - a_cx * e2.lx - a_cy * e2.ly;
        if r_coeff.abs() < 1e-9 {
            vec![]
        } else {
            vec![r_const / r_coeff]
        }
    } else if !circle_idx.is_empty() {
        let ci = circle_idx[0];
        if let TangentObject::Circle {
            center: cp,
            radius: cr,
        } = objs[ci]
        {
            let p = a_cx - cp.x;
            let q = a_cy - cp.y;
            let e = eps[ci];
            let a_q = b_cx * b_cx + b_cy * b_cy - e * e;
            let b_q = 2.0 * (p * b_cx + q * b_cy - cr * e);
            let c_q = p * p + q * q - cr * cr;
            solve_quadratic(a_q, b_q, c_q)
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    r_vals
        .into_iter()
        .filter_map(|r| {
            if r <= 1e-4 {
                return None;
            }
            let cx = a_cx + b_cx * r;
            let cy = a_cy + b_cy * r;
            Some((DVec3::new(cx, cy, 0.0), r))
        })
        .collect()
}

// ── Command: Tan, Tan, Radius ──────────────────────────────────────────────

pub struct CircleTTRCommand {
    step: StepTTR,
}

enum StepTTR {
    First,
    Second {
        obj1: TangentObject,
        hit1: DVec3,
    },
    Radius {
        obj1: TangentObject,
        obj2: TangentObject,
        hit1: DVec3,
        hit2: DVec3,
    },
}

impl CircleTTRCommand {
    pub fn new() -> Self {
        Self {
            step: StepTTR::First,
        }
    }
}

impl CadCommand for CircleTTRCommand {
    fn name(&self) -> &'static str {
        "CIRCLE_TTR"
    }

    fn needs_tangent_pick(&self) -> bool {
        matches!(self.step, StepTTR::First | StepTTR::Second { .. })
    }

    fn wants_text_input(&self) -> bool {
        matches!(self.step, StepTTR::Radius { .. })
    }

    fn dyn_field(&self) -> crate::command::DynField {
        if matches!(self.step, StepTTR::Radius { .. }) {
            crate::command::DynField::Scalar
        } else {
            crate::command::DynField::Point
        }
    }

    fn prompt(&self) -> String {
        match &self.step {
            StepTTR::First => "CIRCLE TTR  Select first tangent object:".into(),
            StepTTR::Second { .. } => "CIRCLE TTR  Select second tangent object:".into(),
            StepTTR::Radius { .. } => "CIRCLE TTR  Specify radius:".into(),
        }
    }

    fn on_point(&mut self, _pt: DVec3) -> CmdResult {
        CmdResult::NeedPoint
    }

    fn on_tangent_point(&mut self, obj: TangentObject, hit: DVec3) -> CmdResult {
        match &self.step {
            StepTTR::First => {
                self.step = StepTTR::Second {
                    obj1: obj,
                    hit1: hit,
                };
                CmdResult::NeedPoint
            }
            StepTTR::Second { obj1, hit1 } => {
                let (o1, h1) = (*obj1, *hit1);
                self.step = StepTTR::Radius {
                    obj1: o1,
                    obj2: obj,
                    hit1: h1,
                    hit2: hit,
                };
                CmdResult::NeedPoint
            }
            StepTTR::Radius { .. } => CmdResult::NeedPoint,
        }
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let r: f64 = text.trim().parse().ok()?;
        if r <= 0.0 {
            return Some(CmdResult::Cancel);
        }
        if let StepTTR::Radius {
            obj1,
            obj2,
            hit1,
            hit2,
        } = &self.step
        {
            let hint = (*hit1 + *hit2) * 0.5;
            let candidates = ttr_candidates(*obj1, *obj2, r);
            if let Some(center) = best_of(&candidates, hint) {
                Some(CmdResult::CommitAndExit(make_circle(center, r)))
            } else {
                Some(CmdResult::Cancel)
            }
        } else {
            None
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, _pt: DVec3) -> Option<WireModel> {
        None
    }
}

// ── Command: Tan, Tan, Tan ────────────────────────────────────────────────

pub struct CircleTTTCommand {
    objs: Vec<TangentObject>,
    hits: Vec<DVec3>,
}

impl CircleTTTCommand {
    pub fn new() -> Self {
        Self {
            objs: Vec::new(),
            hits: Vec::new(),
        }
    }
}

impl CadCommand for CircleTTTCommand {
    fn name(&self) -> &'static str {
        "CIRCLE_TTT"
    }

    fn needs_tangent_pick(&self) -> bool {
        self.objs.len() < 3
    }

    fn prompt(&self) -> String {
        match self.objs.len() {
            0 => "CIRCLE TTT  Select first tangent object:".into(),
            1 => "CIRCLE TTT  Select second tangent object:".into(),
            _ => "CIRCLE TTT  Select third tangent object:".into(),
        }
    }

    fn on_point(&mut self, _pt: DVec3) -> CmdResult {
        CmdResult::NeedPoint
    }

    fn on_tangent_point(&mut self, obj: TangentObject, hit: DVec3) -> CmdResult {
        self.objs.push(obj);
        self.hits.push(hit);
        if self.objs.len() < 3 {
            return CmdResult::NeedPoint;
        }
        let hint = self.hits.iter().fold(DVec3::ZERO, |a, &b| a + b) / 3.0;
        let candidates = ttt_candidates(self.objs[0], self.objs[1], self.objs[2]);
        match best_circle_of(&candidates, hint) {
            Some((center, r)) => CmdResult::CommitAndExit(make_circle(center, r)),
            None => {
                self.objs.pop();
                self.hits.pop();
                CmdResult::NeedPoint
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, _pt: DVec3) -> Option<WireModel> {
        None
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["CIRCLE_2P"] });  // Circle2PCommand
inventory::submit!(crate::command::CommandRegistration { names: &["CIRCLE_3P"] });  // Circle3PCommand
inventory::submit!(crate::command::CommandRegistration { names: &["CIRCLE_CD"] });  // CircleCDCommand
inventory::submit!(crate::command::CommandRegistration { names: &["C", "CIRCLE"] });  // CircleCommand
inventory::submit!(crate::command::CommandRegistration { names: &["CIRCLE_TTR"] });  // CircleTTRCommand
inventory::submit!(crate::command::CommandRegistration { names: &["CIRCLE_TTT"] });  // CircleTTTCommand
