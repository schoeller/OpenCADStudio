// Arc tool — ribbon dropdown + all OpenCADStudio arc creation methods.
//
// Methods:
//   ARC     — Center, Start, End    (CSE — default)
//   ARC_3P  — 3-Point
//   ARC_SCE — Start, Center, End
//   ARC_SCA — Start, Center, Angle
//   ARC_SCL — Start, Center, Length of chord
//   ARC_SEA — Start, End, Angle     (sagitta-based pick)
//   ARC_SER — Start, End, Radius    (radius defined by dist cursor→start)
//   ARC_SED — Start, End, Direction (tangent at start)
//   ARC_CSA — Center, Start, Angle
//   ARC_CSL — Center, Start, Length of chord

use acadrust::types::Vector3;
use acadrust::{Arc as CadArc, EntityType};

use crate::command::{CadCommand, CmdResult};
use crate::modules::IconKind;
use crate::scene::model::wire_model::WireModel;
use glam::Vec3;

const TAU: f32 = std::f32::consts::TAU;

// ── Per-method SVG icons ───────────────────────────────────────────────────

const ICON_CSE: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_cse.svg"));
const ICON_3P: IconKind = IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_3p.svg"));
const ICON_SCE: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_sce.svg"));
const ICON_SCA: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_sca.svg"));
const ICON_SCL: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_scl.svg"));
const ICON_SEA: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_sea.svg"));
const ICON_SER: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_ser.svg"));
const ICON_SED: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_sed.svg"));
const ICON_CSA: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_csa.svg"));
const ICON_CSL: IconKind =
    IconKind::Svg(include_bytes!("../../../../assets/icons/arc/arc_csl.svg"));

// ── Dropdown metadata ──────────────────────────────────────────────────────

pub const DROPDOWN_ID: &str = "ARC";

pub const DROPDOWN_ITEMS: &[(&str, &str, IconKind)] = &[
    ("ARC", "Center, Start, End", ICON_CSE),
    ("ARC_3P", "3-Point", ICON_3P),
    ("ARC_SCE", "Start, Center, End", ICON_SCE),
    ("ARC_SCA", "Start, Center, Angle", ICON_SCA),
    ("ARC_SCL", "Start, Center, Length", ICON_SCL),
    ("ARC_SEA", "Start, End, Angle", ICON_SEA),
    ("ARC_SER", "Start, End, Radius", ICON_SER),
    ("ARC_SED", "Start, End, Direction", ICON_SED),
    ("ARC_CSA", "Center, Start, Angle", ICON_CSA),
    ("ARC_CSL", "Center, Start, Length", ICON_CSL),
];

/// Default icon — falls back to CSE before first use.
pub const ICON: IconKind = ICON_CSE;

// ── Shared math helpers ────────────────────────────────────────────────────

/// Angle in radians from `center` to `pt`.
fn angle_xy(center: Vec3, pt: Vec3) -> f32 {
    (pt.y - center.y).atan2(pt.x - center.x)
}

/// Build a CCW arc polyline from `start_angle` to `end_angle` (radians).
fn arc_preview(center: Vec3, radius: f32, start_angle: f32, end_angle: f32) -> WireModel {
    let mut ea = end_angle;
    while ea < start_angle {
        ea += TAU;
    }
    let span = (ea - start_angle).min(TAU);
    let segs = ((span / TAU) * 64.0).ceil().max(4.0) as u32;
    let pts: Vec<[f32; 3]> = (0..=segs)
        .map(|i| {
            let a = start_angle + span * (i as f32 / segs as f32);
            [
                center.x + radius * a.cos(),
                center.y + radius * a.sin(),
                center.z,
            ]
        })
        .collect();
    WireModel::solid("rubber_band".into(), pts, WireModel::CYAN, false)
}

fn make_arc(center: Vec3, radius: f32, start_angle: f32, end_angle: f32) -> EntityType {
    EntityType::Arc(CadArc {
        center: Vector3::new(center.x as f64, center.y as f64, center.z as f64),
        radius: radius as f64,
        start_angle: start_angle as f64,
        end_angle: end_angle as f64,
        ..Default::default()
    })
}

/// Signed rotation from `prev` to `curr` around `center`.
/// Positive = CCW, negative = CW.
/// Signed angle (radians) swept from `prev` to `curr` about `center`.
fn rot_delta(center: Vec3, prev: Vec3, curr: Vec3) -> f32 {
    let p = prev - center;
    let c = curr - center;
    (p.x * c.y - p.y * c.x).atan2(p.x * c.x + p.y * c.y)
}

/// Minimum swept angle before the previewed arc may flip CW/CCW. Filters the
/// per-frame cursor jitter that otherwise reverses the sweep on tiny moves.
const DIR_TOL: f32 = 0.1745; // ~10°

fn line_wire(a: Vec3, b: Vec3) -> WireModel {
    WireModel::solid(
        "rubber_band".into(),
        vec![[a.x, a.y, a.z], [b.x, b.y, b.z]],
        WireModel::CYAN,
        false,
    )
}

/// Circumscribed circle through three points.
fn circumcircle(a: Vec3, b: Vec3, c: Vec3) -> Option<(Vec3, f32)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None;
    }
    let ux = ((a.x * a.x + a.y * a.y) * (b.y - c.y)
        + (b.x * b.x + b.y * b.y) * (c.y - a.y)
        + (c.x * c.x + c.y * c.y) * (a.y - b.y))
        / d;
    let uy = ((a.x * a.x + a.y * a.y) * (c.x - b.x)
        + (b.x * b.x + b.y * b.y) * (a.x - c.x)
        + (c.x * c.x + c.y * c.y) * (b.x - a.x))
        / d;
    let center = Vec3::new(ux, uy, a.z);
    Some((center, center.distance(a)))
}

/// True if `angle` lies inside the CCW arc [start..end] (radians).
fn ccw_contains(angle: f32, start: f32, end: f32) -> bool {
    let a = angle.rem_euclid(TAU);
    let s = start.rem_euclid(TAU);
    let e = end.rem_euclid(TAU);
    if s <= e {
        a >= s && a <= e
    } else {
        a >= s || a <= e
    }
}

/// Arc center+radius from two endpoints and a cursor (sagitta / bow-toward-cursor).
fn arc_from_sagitta(s: Vec3, e: Vec3, cursor: Vec3) -> Option<(Vec3, f32)> {
    let chord_vec = e - s;
    let chord_len = (chord_vec.x * chord_vec.x + chord_vec.y * chord_vec.y).sqrt();
    if chord_len < 1e-6 {
        return None;
    }
    let unit_chord = Vec3::new(chord_vec.x / chord_len, chord_vec.y / chord_len, 0.0);
    let perp = Vec3::new(-unit_chord.y, unit_chord.x, 0.0);
    let mid = (s + e) * 0.5;
    let h = (cursor - mid).dot(perp); // signed sagitta
    if h.abs() < 1e-3 {
        return None;
    }
    let r = (chord_len * chord_len + 4.0 * h * h) / (8.0 * h.abs());
    let d = (r * r - (chord_len * 0.5) * (chord_len * 0.5))
        .max(0.0)
        .sqrt();
    Some((mid - perp * h.signum() * d, r))
}

/// Arc center+radius from start, end, and a radius-magnitude point (dist = dist(pt, start)).
fn arc_from_se_radius(s: Vec3, e: Vec3, radius_pt: Vec3) -> Option<(Vec3, f32)> {
    let r = s.distance(radius_pt).max(1e-3);
    let chord_len = s.distance(e);
    if r < chord_len * 0.5 {
        return None;
    }
    let unit_chord = (e - s) / chord_len;
    let perp = Vec3::new(-unit_chord.y, 0.0, unit_chord.x);
    let mid = (s + e) * 0.5;
    let h_sign = (radius_pt - mid).dot(perp).signum();
    let d = (r * r - (chord_len * 0.5) * (chord_len * 0.5))
        .max(0.0)
        .sqrt();
    Some((mid - perp * h_sign * d, r))
}

/// Arc center+radius from start, end, and a tangent-direction point at start.
fn arc_from_direction(s: Vec3, e: Vec3, dir_pt: Vec3) -> Option<(Vec3, f32)> {
    let t = (dir_pt - s).normalize_or_zero();
    if t.length_squared() < 1e-12 {
        return None;
    }
    let perp_t = Vec3::new(-t.y, t.x, 0.0); // rotate tangent 90° CCW
    let chord = e - s;
    let denom = perp_t.x * chord.x + perp_t.y * chord.y;
    if denom.abs() < 1e-9 {
        return None;
    }
    let lambda = chord.length_squared() * 0.5 / denom;
    let center = s + perp_t * lambda;
    Some((center, center.distance(s)))
}

/// Compute end_angle from a chord-length pick (SCL / CSL semantics).
/// `chord_len` is clamped to [0, 2r].
fn end_angle_from_chord_len(start_angle: f32, chord: f32, r: f32) -> f32 {
    let half = (chord.min(2.0 * r) / (2.0 * r)).asin();
    start_angle + 2.0 * half
}

// ── Command 1: Center, Start, End  (ARC = CSE) ────────────────────────────

pub struct ArcCommand {
    step: u8,
    c: Vec3,
    r: f32,
    sa: f32,
    prev_pt: Option<Vec3>,
    cw: bool,
}

impl ArcCommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            c: Vec3::ZERO,
            r: 0.0,
            sa: 0.0,
            prev_pt: None,
            cw: false,
        }
    }
}

impl CadCommand for ArcCommand {
    fn name(&self) -> &'static str {
        "ARC"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC  Specify center:".into(),
            1 => "ARC  Specify start point:".into(),
            _ => format!(
                "ARC  Specify end point  [c=({:.2},{:.2}) r={:.3}]:",
                self.c.x, self.c.y, self.r
            ),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.c = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.r = self.c.distance(pt);
                self.sa = angle_xy(self.c, pt);
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => {
                let ea = angle_xy(self.c, pt);
                let e = if self.cw {
                    make_arc(self.c, self.r, ea, self.sa)
                } else {
                    make_arc(self.c, self.r, self.sa, ea)
                };
                CmdResult::CommitAndExit(e)
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.c, pt)),
            2 => {
                if let Some(prev) = self.prev_pt {
                    // Only flip the sweep once the cursor has moved a clear
                    // angular step; keep the reference point until then so slow
                    // moves accumulate and jitter is ignored.
                    let d = rot_delta(self.c, prev, pt);
                    if d.abs() > DIR_TOL {
                        self.cw = d < 0.0;
                        self.prev_pt = Some(pt);
                    }
                } else {
                    self.prev_pt = Some(pt);
                }
                let ea = angle_xy(self.c, pt);
                Some(if self.cw {
                    arc_preview(self.c, self.r, ea, self.sa)
                } else {
                    arc_preview(self.c, self.r, self.sa, ea)
                })
            }
            _ => None,
        }
    }
}

// ── Command 2: 3-Point  (ARC_3P) ──────────────────────────────────────────

pub struct Arc3PCommand {
    pts: Vec<Vec3>,
}

impl Arc3PCommand {
    pub fn new() -> Self {
        Self { pts: Vec::new() }
    }
}

impl CadCommand for Arc3PCommand {
    fn name(&self) -> &'static str {
        "ARC_3P"
    }
    fn prompt(&self) -> String {
        match self.pts.len() {
            0 => "ARC 3P  Specify start point:".into(),
            1 => "ARC 3P  Specify second point on arc:".into(),
            _ => "ARC 3P  Specify end point:".into(),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        self.pts.push(pt);
        if self.pts.len() < 3 {
            return CmdResult::NeedPoint;
        }
        let (p1, p2, p3) = (self.pts[0], self.pts[1], self.pts[2]);
        match circumcircle(p1, p2, p3) {
            None => {
                self.pts.pop();
                CmdResult::NeedPoint
            } // collinear — retry
            Some((center, radius)) => {
                let a1 = angle_xy(center, p1);
                let a2 = angle_xy(center, p2);
                let a3 = angle_xy(center, p3);
                // Choose arc direction so that p2 lies on the arc from p1 to p3.
                let (sa, ea) = if ccw_contains(a2, a1, a3) {
                    (a1, a3)
                } else {
                    (a3, a1)
                };
                CmdResult::CommitAndExit(make_arc(center, radius, sa, ea))
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.pts.len() {
            0 => None,
            1 => Some(line_wire(self.pts[0], pt)),
            _ => {
                let (p1, p2) = (self.pts[0], self.pts[1]);
                if let Some((center, radius)) = circumcircle(p1, p2, pt) {
                    let a1 = angle_xy(center, p1);
                    let a2 = angle_xy(center, p2);
                    let a3 = angle_xy(center, pt);
                    let (sa, ea) = if ccw_contains(a2, a1, a3) {
                        (a1, a3)
                    } else {
                        (a3, a1)
                    };
                    Some(arc_preview(center, radius, sa, ea))
                } else {
                    Some(WireModel::solid(
                        "rubber_band".into(),
                        vec![[p1.x, p1.y, p1.z], [p2.x, p2.y, p2.z], [pt.x, pt.y, pt.z]],
                        WireModel::CYAN,
                        false,
                    ))
                }
            }
        }
    }
}

// ── Command 3: Start, Center, End  (ARC_SCE) ──────────────────────────────

pub struct ArcSCECommand {
    step: u8,
    s: Vec3,
    c: Vec3,
    r: f32,
    sa: f32,
    prev_pt: Option<Vec3>,
    cw: bool,
}

impl ArcSCECommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            s: Vec3::ZERO,
            c: Vec3::ZERO,
            r: 0.0,
            sa: 0.0,
            prev_pt: None,
            cw: false,
        }
    }
}

impl CadCommand for ArcSCECommand {
    fn name(&self) -> &'static str {
        "ARC_SCE"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC SCE  Specify start point:".into(),
            1 => "ARC SCE  Specify center:".into(),
            _ => format!("ARC SCE  Specify end point  [r={:.3}]:", self.r),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.s = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.c = pt;
                self.r = pt.distance(self.s);
                self.sa = angle_xy(pt, self.s);
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => {
                let ea = angle_xy(self.c, pt);
                let e = if self.cw {
                    make_arc(self.c, self.r, ea, self.sa)
                } else {
                    make_arc(self.c, self.r, self.sa, ea)
                };
                CmdResult::CommitAndExit(e)
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.s, pt)),
            2 => {
                if let Some(prev) = self.prev_pt {
                    // Only flip the sweep once the cursor has moved a clear
                    // angular step; keep the reference point until then so slow
                    // moves accumulate and jitter is ignored.
                    let d = rot_delta(self.c, prev, pt);
                    if d.abs() > DIR_TOL {
                        self.cw = d < 0.0;
                        self.prev_pt = Some(pt);
                    }
                } else {
                    self.prev_pt = Some(pt);
                }
                let ea = angle_xy(self.c, pt);
                Some(if self.cw {
                    arc_preview(self.c, self.r, ea, self.sa)
                } else {
                    arc_preview(self.c, self.r, self.sa, ea)
                })
            }
            _ => None,
        }
    }
}

// ── Command 4: Start, Center, Angle  (ARC_SCA) ────────────────────────────
// Interactive: cursor direction from center defines span.  Typing: degrees of span.

pub struct ArcSCACommand {
    step: u8,
    s: Vec3,
    c: Vec3,
    r: f32,
    sa: f32,
    prev_pt: Option<Vec3>,
    cw: bool,
}

impl ArcSCACommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            s: Vec3::ZERO,
            c: Vec3::ZERO,
            r: 0.0,
            sa: 0.0,
            prev_pt: None,
            cw: false,
        }
    }
}

impl CadCommand for ArcSCACommand {
    fn name(&self) -> &'static str {
        "ARC_SCA"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC SCA  Specify start point:".into(),
            1 => "ARC SCA  Specify center:".into(),
            _ => format!(
                "ARC SCA  Click end direction or type arc span in degrees  [start={:.1}°]:",
                self.sa.to_degrees()
            ),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.s = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.c = pt;
                self.r = pt.distance(self.s);
                self.sa = angle_xy(pt, self.s);
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => {
                let ea = angle_xy(self.c, pt);
                let e = if self.cw {
                    make_arc(self.c, self.r, ea, self.sa)
                } else {
                    make_arc(self.c, self.r, self.sa, ea)
                };
                CmdResult::CommitAndExit(e)
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if self.step == 2 {
            let span: f32 = text.trim().replace(',', ".").parse().ok()?;
            // Negative span = CW; positive = CCW.
            let ea = self.sa + span.to_radians();
            return Some(CmdResult::CommitAndExit(make_arc(
                self.c, self.r, self.sa, ea,
            )));
        }
        None
    }
    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        // Included angle (span) at the centre. Typed value is the span, handled
        // by on_text_input; the box previews the live span via dyn_live_value.
        (self.step == 2).then(|| DynSpec {
            anchor: DynAnchor::Point(self.c),
            fields: vec![DynFieldSpec::new(DynRole::Angle)],
            guide: DynGuide::Polar,
            ref_point: Some(self.c + Vec3::new(self.sa.cos(), self.sa.sin(), 0.0)),
        })
    }
    fn dyn_commit_as_text(&self) -> bool {
        self.step == 2
    }
    fn dyn_live_value(&self, cursor: Vec3) -> Option<f64> {
        (self.step == 2)
            .then(|| crate::command::dyn_display_angle_deg(angle_xy(self.c, cursor) - self.sa) as f64)
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.s, pt)),
            2 => {
                if let Some(prev) = self.prev_pt {
                    // Only flip the sweep once the cursor has moved a clear
                    // angular step; keep the reference point until then so slow
                    // moves accumulate and jitter is ignored.
                    let d = rot_delta(self.c, prev, pt);
                    if d.abs() > DIR_TOL {
                        self.cw = d < 0.0;
                        self.prev_pt = Some(pt);
                    }
                } else {
                    self.prev_pt = Some(pt);
                }
                let ea = angle_xy(self.c, pt);
                Some(if self.cw {
                    arc_preview(self.c, self.r, ea, self.sa)
                } else {
                    arc_preview(self.c, self.r, self.sa, ea)
                })
            }
            _ => None,
        }
    }
}

// ── Command 5: Start, Center, Length  (ARC_SCL) ───────────────────────────
// "Length" = chord length from start to end of arc.
// Interactive: cursor distance from start_pt drives the chord length.

pub struct ArcSCLCommand {
    step: u8,
    s: Vec3,
    c: Vec3,
    r: f32,
    sa: f32,
}

impl ArcSCLCommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            s: Vec3::ZERO,
            c: Vec3::ZERO,
            r: 0.0,
            sa: 0.0,
        }
    }
}

impl CadCommand for ArcSCLCommand {
    fn name(&self) -> &'static str {
        "ARC_SCL"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC SCL  Specify start point:".into(),
            1 => "ARC SCL  Specify center:".into(),
            _ => format!(
                "ARC SCL  Click chord end or type chord length  [r={:.3}]:",
                self.r
            ),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.s = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.c = pt;
                self.r = pt.distance(self.s);
                self.sa = angle_xy(pt, self.s);
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => {
                let chord = self.s.distance(pt);
                let ea = end_angle_from_chord_len(self.sa, chord, self.r);
                CmdResult::CommitAndExit(make_arc(self.c, self.r, self.sa, ea))
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if self.step == 2 {
            let chord: f32 = text.trim().replace(',', ".").parse().ok()?;
            if chord > 0.0 {
                let ea = end_angle_from_chord_len(self.sa, chord, self.r);
                return Some(CmdResult::CommitAndExit(make_arc(
                    self.c, self.r, self.sa, ea,
                )));
            }
        }
        None
    }
    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        // Chord length from the start point (typed → on_text_input).
        (self.step == 2).then(|| DynSpec {
            anchor: DynAnchor::Point(self.s),
            fields: vec![DynFieldSpec::new(DynRole::Distance)],
            guide: DynGuide::Radius,
            ref_point: None,
        })
    }
    fn dyn_commit_as_text(&self) -> bool {
        self.step == 2
    }
    fn dyn_live_value(&self, cursor: Vec3) -> Option<f64> {
        (self.step == 2).then(|| self.s.distance(cursor) as f64)
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.s, pt)),
            2 => {
                let chord = self.s.distance(pt);
                let ea = end_angle_from_chord_len(self.sa, chord, self.r);
                Some(arc_preview(self.c, self.r, self.sa, ea))
            }
            _ => None,
        }
    }
}

// ── Command 6: Start, End, Angle  (ARC_SEA) ───────────────────────────────
// Interactive: cursor distance from chord defines sagitta → arc shape.

pub struct ArcSEACommand {
    step: u8,
    s: Vec3,
    e: Vec3,
}

impl ArcSEACommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            s: Vec3::ZERO,
            e: Vec3::ZERO,
        }
    }
}

impl CadCommand for ArcSEACommand {
    fn name(&self) -> &'static str {
        "ARC_SEA"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC SEA  Specify start point:".into(),
            1 => "ARC SEA  Specify end point:".into(),
            _ => "ARC SEA  Specify angle (move cursor perpendicular to chord):".into(),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.s = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.e = pt;
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => match arc_from_sagitta(self.s, self.e, pt) {
                Some((center, radius)) => {
                    let sa = angle_xy(center, self.s);
                    let ea = angle_xy(center, self.e);
                    CmdResult::CommitAndExit(make_arc(center, radius, sa, ea))
                }
                None => CmdResult::Cancel,
            },
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.s, pt)),
            2 => {
                if let Some((center, radius)) = arc_from_sagitta(self.s, self.e, pt) {
                    let sa = angle_xy(center, self.s);
                    let ea = angle_xy(center, self.e);
                    Some(arc_preview(center, radius, sa, ea))
                } else {
                    Some(line_wire(self.s, self.e))
                }
            }
            _ => None,
        }
    }
}

// ── Command 7: Start, End, Radius  (ARC_SER) ──────────────────────────────
// Interactive: radius = distance(cursor, start_point).

pub struct ArcSERCommand {
    step: u8,
    s: Vec3,
    e: Vec3,
}

impl ArcSERCommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            s: Vec3::ZERO,
            e: Vec3::ZERO,
        }
    }
}

impl CadCommand for ArcSERCommand {
    fn name(&self) -> &'static str {
        "ARC_SER"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC SER  Specify start point:".into(),
            1 => "ARC SER  Specify end point:".into(),
            _ => "ARC SER  Click radius point or type radius value:".into(),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.s = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.e = pt;
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => match arc_from_se_radius(self.s, self.e, pt) {
                Some((center, radius)) => {
                    let sa = angle_xy(center, self.s);
                    let ea = angle_xy(center, self.e);
                    CmdResult::CommitAndExit(make_arc(center, radius, sa, ea))
                }
                None => CmdResult::Cancel,
            },
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if self.step == 2 {
            let r: f32 = text.trim().replace(',', ".").parse().ok()?;
            let chord = self.s.distance(self.e);
            if r > 0.0 && r >= chord / 2.0 {
                let mid = (self.s + self.e) * 0.5;
                let perp = {
                    let cv = (self.e - self.s) / chord;
                    Vec3::new(-cv.y, cv.x, 0.0)
                };
                let d = (r * r - (chord / 2.0) * (chord / 2.0)).max(0.0).sqrt();
                let center = mid - perp * d;
                let sa = angle_xy(center, self.s);
                let ea = angle_xy(center, self.e);
                return Some(CmdResult::CommitAndExit(make_arc(center, r, sa, ea)));
            }
        }
        None
    }
    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        // Radius value (typed → on_text_input); the preview arc is the guide.
        (self.step == 2).then(|| DynSpec {
            anchor: DynAnchor::Point(self.s),
            fields: vec![DynFieldSpec::new(DynRole::Radius)],
            guide: DynGuide::None,
            ref_point: None,
        })
    }
    fn dyn_commit_as_text(&self) -> bool {
        self.step == 2
    }
    fn dyn_live_value(&self, cursor: Vec3) -> Option<f64> {
        if self.step != 2 {
            return None;
        }
        arc_from_se_radius(self.s, self.e, cursor).map(|(_, r)| r as f64)
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.s, pt)),
            2 => {
                if let Some((center, radius)) = arc_from_se_radius(self.s, self.e, pt) {
                    let sa = angle_xy(center, self.s);
                    let ea = angle_xy(center, self.e);
                    Some(arc_preview(center, radius, sa, ea))
                } else {
                    Some(line_wire(self.s, self.e))
                }
            }
            _ => None,
        }
    }
}

// ── Command 8: Start, End, Direction  (ARC_SED) ───────────────────────────
// Interactive: cursor position defines tangent direction at start (cursor − start).

pub struct ArcSEDCommand {
    step: u8,
    s: Vec3,
    e: Vec3,
}

impl ArcSEDCommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            s: Vec3::ZERO,
            e: Vec3::ZERO,
        }
    }
}

impl CadCommand for ArcSEDCommand {
    fn name(&self) -> &'static str {
        "ARC_SED"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC SED  Specify start point:".into(),
            1 => "ARC SED  Specify end point:".into(),
            _ => "ARC SED  Specify tangent direction at start:".into(),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.s = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.e = pt;
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => match arc_from_direction(self.s, self.e, pt) {
                Some((center, radius)) => {
                    let sa = angle_xy(center, self.s);
                    let ea = angle_xy(center, self.e);
                    CmdResult::CommitAndExit(make_arc(center, radius, sa, ea))
                }
                None => CmdResult::Cancel,
            },
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.s, pt)),
            2 => {
                if let Some((center, radius)) = arc_from_direction(self.s, self.e, pt) {
                    let sa = angle_xy(center, self.s);
                    let ea = angle_xy(center, self.e);
                    Some(arc_preview(center, radius, sa, ea))
                } else {
                    Some(line_wire(self.s, self.e))
                }
            }
            _ => None,
        }
    }
}

// ── Command 9: Center, Start, Angle  (ARC_CSA) ────────────────────────────
// Interactive: angle direction indicated by cursor position relative to center.

pub struct ArcCSACommand {
    step: u8,
    c: Vec3,
    r: f32,
    sa: f32,
    prev_pt: Option<Vec3>,
    cw: bool,
}

impl ArcCSACommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            c: Vec3::ZERO,
            r: 0.0,
            sa: 0.0,
            prev_pt: None,
            cw: false,
        }
    }
}

impl CadCommand for ArcCSACommand {
    fn name(&self) -> &'static str {
        "ARC_CSA"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC CSA  Specify center:".into(),
            1 => "ARC CSA  Specify start point:".into(),
            _ => format!(
                "ARC CSA  Click end direction or type arc span in degrees  [start={:.1}°]:",
                self.sa.to_degrees()
            ),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.c = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.r = self.c.distance(pt);
                self.sa = angle_xy(self.c, pt);
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => {
                let ea = angle_xy(self.c, pt);
                let e = if self.cw {
                    make_arc(self.c, self.r, ea, self.sa)
                } else {
                    make_arc(self.c, self.r, self.sa, ea)
                };
                CmdResult::CommitAndExit(e)
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if self.step == 2 {
            let span: f32 = text.trim().replace(',', ".").parse().ok()?;
            let ea = self.sa + span.to_radians();
            return Some(CmdResult::CommitAndExit(make_arc(
                self.c, self.r, self.sa, ea,
            )));
        }
        None
    }
    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        (self.step == 2).then(|| DynSpec {
            anchor: DynAnchor::Point(self.c),
            fields: vec![DynFieldSpec::new(DynRole::Angle)],
            guide: DynGuide::Polar,
            ref_point: Some(self.c + Vec3::new(self.sa.cos(), self.sa.sin(), 0.0)),
        })
    }
    fn dyn_commit_as_text(&self) -> bool {
        self.step == 2
    }
    fn dyn_live_value(&self, cursor: Vec3) -> Option<f64> {
        (self.step == 2)
            .then(|| crate::command::dyn_display_angle_deg(angle_xy(self.c, cursor) - self.sa) as f64)
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.c, pt)),
            2 => {
                if let Some(prev) = self.prev_pt {
                    // Only flip the sweep once the cursor has moved a clear
                    // angular step; keep the reference point until then so slow
                    // moves accumulate and jitter is ignored.
                    let d = rot_delta(self.c, prev, pt);
                    if d.abs() > DIR_TOL {
                        self.cw = d < 0.0;
                        self.prev_pt = Some(pt);
                    }
                } else {
                    self.prev_pt = Some(pt);
                }
                let ea = angle_xy(self.c, pt);
                Some(if self.cw {
                    arc_preview(self.c, self.r, ea, self.sa)
                } else {
                    arc_preview(self.c, self.r, self.sa, ea)
                })
            }
            _ => None,
        }
    }
}

// ── Command 10: Center, Start, Length  (ARC_CSL) ──────────────────────────
// "Length" = chord from start to end.  Interactive: dist(cursor, start_pt) = chord.

pub struct ArcCSLCommand {
    step: u8,
    c: Vec3,
    s: Vec3,
    r: f32,
    sa: f32,
}

impl ArcCSLCommand {
    pub fn new() -> Self {
        Self {
            step: 0,
            c: Vec3::ZERO,
            s: Vec3::ZERO,
            r: 0.0,
            sa: 0.0,
        }
    }
}

impl CadCommand for ArcCSLCommand {
    fn name(&self) -> &'static str {
        "ARC_CSL"
    }
    fn prompt(&self) -> String {
        match self.step {
            0 => "ARC CSL  Specify center:".into(),
            1 => "ARC CSL  Specify start point:".into(),
            _ => format!(
                "ARC CSL  Click chord end or type chord length  [r={:.3}]:",
                self.r
            ),
        }
    }
    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            0 => {
                self.c = pt;
                self.step = 1;
                CmdResult::NeedPoint
            }
            1 => {
                self.s = pt;
                self.r = self.c.distance(pt);
                self.sa = angle_xy(self.c, pt);
                self.step = 2;
                CmdResult::NeedPoint
            }
            _ => {
                let chord = self.s.distance(pt);
                let ea = end_angle_from_chord_len(self.sa, chord, self.r);
                CmdResult::CommitAndExit(make_arc(self.c, self.r, self.sa, ea))
            }
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if self.step == 2 {
            let chord: f32 = text.trim().replace(',', ".").parse().ok()?;
            if chord > 0.0 {
                let ea = end_angle_from_chord_len(self.sa, chord, self.r);
                return Some(CmdResult::CommitAndExit(make_arc(
                    self.c, self.r, self.sa, ea,
                )));
            }
        }
        None
    }
    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        // Chord length from the start point (typed → on_text_input).
        (self.step == 2).then(|| DynSpec {
            anchor: DynAnchor::Point(self.s),
            fields: vec![DynFieldSpec::new(DynRole::Distance)],
            guide: DynGuide::Radius,
            ref_point: None,
        })
    }
    fn dyn_commit_as_text(&self) -> bool {
        self.step == 2
    }
    fn dyn_live_value(&self, cursor: Vec3) -> Option<f64> {
        (self.step == 2).then(|| self.s.distance(cursor) as f64)
    }
    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        match self.step {
            1 => Some(line_wire(self.c, pt)),
            2 => {
                let chord = self.s.distance(pt);
                let ea = end_angle_from_chord_len(self.sa, chord, self.r);
                Some(arc_preview(self.c, self.r, self.sa, ea))
            }
            _ => None,
        }
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_3P"] });  // Arc3PCommand
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_CSA"] });  // ArcCSACommand
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_CSL"] });  // ArcCSLCommand
inventory::submit!(crate::command::CommandRegistration { names: &["A", "ARC"] });  // ArcCommand
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_SCA"] });  // ArcSCACommand
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_SCE"] });  // ArcSCECommand
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_SCL"] });  // ArcSCLCommand
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_SEA"] });  // ArcSEACommand
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_SED"] });  // ArcSEDCommand
inventory::submit!(crate::command::CommandRegistration { names: &["ARC_SER"] });  // ArcSERCommand
