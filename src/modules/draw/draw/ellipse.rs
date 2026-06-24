// Ellipse tool — ribbon dropdown + all OpenCADStudio ellipse creation methods.
//
// Commands:
//   ELLIPSE      — Center, Axes  (center → major endpoint → minor distance)
//   ELLIPSE_AXIS — Axis, End     (axis endpoint 1 → endpoint 2 → minor distance)
//   ELLIPSE_ARC  — Ellipse Arc   (shape as above, then start/end parametric angles)

use acadrust::types::Vector3;
use acadrust::{Ellipse, EntityType};

use crate::command::{CadCommand, CmdResult};
use crate::modules::IconKind;
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

fn parse_num(text: &str) -> Option<f64> {
    text.trim().replace(',', ".").parse().ok()
}

const TAU: f64 = std::f64::consts::TAU;

/// Minimum swept angle before the previewed arc may flip CW/CCW — filters the
/// per-frame cursor jitter that otherwise reverses the sweep on tiny moves.
const DIR_TOL: f64 = 0.1745; // ~10°

// ── Icons ─────────────────────────────────────────────────────────────────

const ICON_CTR: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/ellipse/ellipse_ctr.svg"
));
const ICON_AXIS: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/ellipse/ellipse_axis.svg"
));
const ICON_ARC: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/ellipse/ellipse_arc.svg"
));

// ── Dropdown metadata ─────────────────────────────────────────────────────

pub const DROPDOWN_ID: &str = "ELLIPSE";

pub const DROPDOWN_ITEMS: &[(&str, &str, IconKind)] = &[
    ("ELLIPSE", "Center, Axes", ICON_CTR),
    ("ELLIPSE_AXIS", "Axis, End", ICON_AXIS),
    ("ELLIPSE_ARC", "Ellipse Arc", ICON_ARC),
];

pub const ICON: IconKind = ICON_CTR;

// ── Shared helpers ────────────────────────────────────────────────────────

/// Preview wire for a full or partial ellipse.
fn ellipse_wire(
    center: DVec3,
    major: DVec3, // vector from center to major-axis endpoint
    ratio: f64,   // minor/major
    t_start: f64,
    t_end: f64,
) -> WireModel {
    let r_major = major.length();
    if r_major < 1e-9 {
        return WireModel::solid("rubber_band".into(), vec![], WireModel::CYAN, false);
    }
    let major_dir = major / r_major;
    let v = DVec3::Z.cross(major_dir).normalize();
    let segs = 64u32;
    // Unwrap t_end so the arc goes counter-clockwise.
    let t_e = if t_end <= t_start { t_end + TAU } else { t_end };
    let pts: Vec<[f32; 3]> = (0..=segs)
        .map(|i| {
            let t = t_start + (t_e - t_start) * (i as f64 / segs as f64);
            let p = center + t.cos() * r_major * major_dir + t.sin() * r_major * ratio * v;
            [p.x as f32, p.y as f32, p.z as f32]
        })
        .collect();
    WireModel::solid("rubber_band".into(), pts, WireModel::CYAN, false)
}

/// Convert a world point to the parametric angle on the ellipse.
fn param_angle(center: DVec3, major_dir: DVec3, v: DVec3, pt: DVec3, ratio: f64) -> f64 {
    let d = pt - center;
    let u_proj = d.dot(major_dir);
    let v_proj = d.dot(v);
    // Inverse-map: on ellipse x=cos(t)*r_major, y=sin(t)*r_major*ratio
    // → t = atan2(v_proj / (r_major*ratio), u_proj / r_major) but we normalise
    v_proj.atan2(u_proj * ratio).rem_euclid(TAU)
}

/// Build the final Ellipse entity.
fn make_ellipse(center: DVec3, major: DVec3, ratio: f64, t_start: f64, t_end: f64) -> Ellipse {
    Ellipse {
        center: Vector3::new(center.x, center.y, center.z),
        major_axis: Vector3::new(major.x, major.y, major.z),
        minor_axis_ratio: ratio,
        start_parameter: t_start,
        end_parameter: t_end,
        ..Default::default()
    }
}

// ── 1. Center mode ────────────────────────────────────────────────────────
//   Step 1: center   Step 2: major-axis endpoint   Step 3: minor-axis point

enum CtrStep {
    Center,
    MajorAxis { center: DVec3 },
    MinorRatio { center: DVec3, major: DVec3 },
}

pub struct EllipseCommand {
    step: CtrStep,
}

impl EllipseCommand {
    pub fn new() -> Self {
        Self {
            step: CtrStep::Center,
        }
    }
}

impl CadCommand for EllipseCommand {
    fn name(&self) -> &'static str {
        "ELLIPSE"
    }

    fn prompt(&self) -> String {
        match &self.step {
            CtrStep::Center => "ELLIPSE  Specify center:".into(),
            CtrStep::MajorAxis { .. } => "ELLIPSE  Specify major axis endpoint:".into(),
            CtrStep::MinorRatio { major, .. } => format!(
                "ELLIPSE  Specify minor axis point or type half-length  [major r={:.3}]:",
                major.length()
            ),
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match &self.step {
            CtrStep::Center => {
                self.step = CtrStep::MajorAxis { center: pt };
                CmdResult::NeedPoint
            }
            CtrStep::MajorAxis { center } => {
                let center = *center;
                self.step = CtrStep::MinorRatio {
                    center,
                    major: pt - center,
                };
                CmdResult::NeedPoint
            }
            CtrStep::MinorRatio { center, major } => {
                let (center, major) = (*center, *major);
                let ratio = minor_ratio(center, major, pt);
                CmdResult::CommitAndExit(EntityType::Ellipse(make_ellipse(
                    center, major, ratio, 0.0, TAU,
                )))
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if let CtrStep::MinorRatio { center, major } = &self.step {
            let r_minor = parse_num(text)?;
            if r_minor > 0.0 {
                let ratio = (r_minor / major.length()).clamp(1e-6, 1.0);
                return Some(CmdResult::CommitAndExit(EntityType::Ellipse(make_ellipse(
                    *center, *major, ratio, 0.0, TAU,
                ))));
            }
        }
        None
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        match &self.step {
            CtrStep::MajorAxis { center } => Some(line_wire(*center, pt)),
            CtrStep::MinorRatio { center, major } => {
                let ratio = minor_ratio(*center, *major, pt).max(0.001);
                Some(ellipse_wire(*center, *major, ratio, 0.0, TAU))
            }
            _ => None,
        }
    }

    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        match &self.step {
            // Center + major axis endpoint: ordinary point picks (legacy polar
            // anchored at the previous point).
            CtrStep::Center | CtrStep::MajorAxis { .. } => None,
            // Minor axis: half-length measured square to the major axis. Show
            // the perpendicular drop from the cursor onto the major axis.
            CtrStep::MinorRatio { center, major } => Some(DynSpec {
                anchor: DynAnchor::Point(*center),
                fields: vec![DynFieldSpec::new(DynRole::Distance)],
                guide: DynGuide::Perp,
                ref_point: Some(*center + *major),
            }),
        }
    }

    fn dyn_live_value(&self, cursor: DVec3) -> Option<f64> {
        if let CtrStep::MinorRatio { center, major } = &self.step {
            Some(minor_ratio(*center, *major, cursor) * major.length())
        } else {
            None
        }
    }
}

// ── 2. Axis, End mode ─────────────────────────────────────────────────────
//   Step 1: axis endpoint 1   Step 2: axis endpoint 2   Step 3: minor-axis point

enum AxisStep {
    Pt1,
    Pt2 { p1: DVec3 },
    MinorRatio { center: DVec3, major: DVec3 },
}

pub struct EllipseAxisCommand {
    step: AxisStep,
}

impl EllipseAxisCommand {
    pub fn new() -> Self {
        Self {
            step: AxisStep::Pt1,
        }
    }
}

impl CadCommand for EllipseAxisCommand {
    fn name(&self) -> &'static str {
        "ELLIPSE_AXIS"
    }

    fn prompt(&self) -> String {
        match &self.step {
            AxisStep::Pt1 => "ELLIPSE (Axis)  Specify first endpoint of major axis:".into(),
            AxisStep::Pt2 { .. } => "ELLIPSE (Axis)  Specify second endpoint of major axis:".into(),
            AxisStep::MinorRatio { major, .. } => format!(
                "ELLIPSE (Axis)  Specify minor axis point or type half-length  [major r={:.3}]:",
                major.length()
            ),
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match &self.step {
            AxisStep::Pt1 => {
                self.step = AxisStep::Pt2 { p1: pt };
                CmdResult::NeedPoint
            }
            AxisStep::Pt2 { p1 } => {
                let center = (*p1 + pt) * 0.5;
                let major = pt - center; // half-vector
                self.step = AxisStep::MinorRatio { center, major };
                CmdResult::NeedPoint
            }
            AxisStep::MinorRatio { center, major } => {
                let (center, major) = (*center, *major);
                let ratio = minor_ratio(center, major, pt);
                CmdResult::CommitAndExit(EntityType::Ellipse(make_ellipse(
                    center, major, ratio, 0.0, TAU,
                )))
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if let AxisStep::MinorRatio { center, major } = &self.step {
            let r_minor = parse_num(text)?;
            if r_minor > 0.0 {
                let ratio = (r_minor / major.length()).clamp(1e-6, 1.0);
                return Some(CmdResult::CommitAndExit(EntityType::Ellipse(make_ellipse(
                    *center, *major, ratio, 0.0, TAU,
                ))));
            }
        }
        None
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        match &self.step {
            AxisStep::Pt1 => None,
            AxisStep::Pt2 { p1 } => Some(line_wire(*p1, pt)),
            AxisStep::MinorRatio { center, major } => {
                let ratio = minor_ratio(*center, *major, pt).max(0.001);
                Some(ellipse_wire(*center, *major, ratio, 0.0, TAU))
            }
        }
    }

    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        match &self.step {
            // Endpoints define the full major axis — legacy polar (anchored at
            // the previous point) is right.
            AxisStep::Pt1 | AxisStep::Pt2 { .. } => None,
            // Minor half-length, square to the major axis.
            AxisStep::MinorRatio { center, major } => Some(DynSpec {
                anchor: DynAnchor::Point(*center),
                fields: vec![DynFieldSpec::new(DynRole::Distance)],
                guide: DynGuide::Perp,
                ref_point: Some(*center + *major),
            }),
        }
    }

    fn dyn_live_value(&self, cursor: DVec3) -> Option<f64> {
        if let AxisStep::MinorRatio { center, major } = &self.step {
            Some(minor_ratio(*center, *major, cursor) * major.length())
        } else {
            None
        }
    }
}

// ── 3. Ellipse Arc mode ───────────────────────────────────────────────────
//   Same shape steps as Center mode, then: start parameter, end parameter.

enum ArcStep {
    Center,
    MajorAxis {
        center: DVec3,
    },
    MinorRatio {
        center: DVec3,
        major: DVec3,
    },
    StartAngle {
        center: DVec3,
        major: DVec3,
        ratio: f64,
    },
    EndAngle {
        center: DVec3,
        major: DVec3,
        ratio: f64,
        t_start: f64,
    },
}

pub struct EllipseArcCommand {
    step: ArcStep,
    prev_pt: Option<DVec3>,
    cw: bool,
}

impl EllipseArcCommand {
    pub fn new() -> Self {
        Self {
            step: ArcStep::Center,
            prev_pt: None,
            cw: false,
        }
    }
}

impl CadCommand for EllipseArcCommand {
    fn name(&self) -> &'static str {
        "ELLIPSE_ARC"
    }

    fn prompt(&self) -> String {
        match &self.step {
            ArcStep::Center => "ELLIPSE ARC  Specify center:".into(),
            ArcStep::MajorAxis { .. } => "ELLIPSE ARC  Specify major axis endpoint:".into(),
            ArcStep::MinorRatio { major, .. } => format!(
                "ELLIPSE ARC  Specify minor axis point or type half-length  [major r={:.3}]:",
                major.length()
            ),
            ArcStep::StartAngle { .. } => {
                "ELLIPSE ARC  Specify start angle point or type degrees:".into()
            }
            ArcStep::EndAngle { t_start, .. } => format!(
                "ELLIPSE ARC  Specify end angle point or type degrees  [start={:.1}°]:",
                t_start.to_degrees()
            ),
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match &self.step {
            ArcStep::Center => {
                self.step = ArcStep::MajorAxis { center: pt };
                CmdResult::NeedPoint
            }
            ArcStep::MajorAxis { center } => {
                let center = *center;
                self.step = ArcStep::MinorRatio {
                    center,
                    major: pt - center,
                };
                CmdResult::NeedPoint
            }
            ArcStep::MinorRatio { center, major } => {
                let (center, major) = (*center, *major);
                let ratio = minor_ratio(center, major, pt);
                self.step = ArcStep::StartAngle {
                    center,
                    major,
                    ratio,
                };
                CmdResult::NeedPoint
            }
            ArcStep::StartAngle {
                center,
                major,
                ratio,
            } => {
                let (center, major, ratio) = (*center, *major, *ratio);
                let t_start = angle_from_point(center, major, ratio, pt);
                self.prev_pt = None; // reset direction tracking for end-angle step
                self.step = ArcStep::EndAngle {
                    center,
                    major,
                    ratio,
                    t_start,
                };
                CmdResult::NeedPoint
            }
            ArcStep::EndAngle {
                center,
                major,
                ratio,
                t_start,
            } => {
                let (center, major, ratio, t_start) = (*center, *major, *ratio, *t_start);
                let t_end = angle_from_point(center, major, ratio, pt);
                let entity = if self.cw {
                    make_ellipse(center, major, ratio, t_end, t_start)
                } else {
                    make_ellipse(center, major, ratio, t_start, t_end)
                };
                CmdResult::CommitAndExit(EntityType::Ellipse(entity))
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let val = parse_num(text)?;
        match &self.step {
            ArcStep::MinorRatio { center, major } => {
                if val > 0.0 {
                    let ratio = (val / major.length()).clamp(1e-6, 1.0);
                    let (c, m) = (*center, *major);
                    self.step = ArcStep::StartAngle {
                        center: c,
                        major: m,
                        ratio,
                    };
                    return Some(CmdResult::NeedPoint);
                }
            }
            ArcStep::StartAngle {
                center,
                major,
                ratio,
            } => {
                let t_start = val.to_radians();
                let (c, m, r) = (*center, *major, *ratio);
                self.prev_pt = None;
                self.step = ArcStep::EndAngle {
                    center: c,
                    major: m,
                    ratio: r,
                    t_start,
                };
                return Some(CmdResult::NeedPoint);
            }
            ArcStep::EndAngle {
                center,
                major,
                ratio,
                t_start,
            } => {
                // Typed degrees: positive = CCW, negative = CW.
                let t_end = val.to_radians();
                return Some(CmdResult::CommitAndExit(EntityType::Ellipse(make_ellipse(
                    *center, *major, *ratio, *t_start, t_end,
                ))));
            }
            _ => {}
        }
        None
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        match &self.step {
            ArcStep::MajorAxis { center } => Some(line_wire(*center, pt)),
            ArcStep::MinorRatio { center, major } => {
                let ratio = minor_ratio(*center, *major, pt).max(0.001);
                Some(ellipse_wire(*center, *major, ratio, 0.0, TAU))
            }
            ArcStep::StartAngle { center, .. } => {
                // Only the start angle is being chosen here — show a line from
                // the centre to the cursor to indicate that angle, not a
                // (misleading) full arc preview.
                Some(line_wire(*center, pt))
            }
            ArcStep::EndAngle {
                center,
                major,
                ratio,
                t_start,
            } => {
                // Detect sweep direction from the change in PARAMETRIC angle
                // (the visual sweep along the ellipse), not the geometric angle
                // about the centre — on a flat ellipse a large visible move can
                // be a tiny centre angle, which made the direction stick. A
                // tolerance ignores jitter; the reference advances only on a
                // clear move so slow sweeps accumulate.
                if let Some(prev) = self.prev_pt {
                    let t_prev = angle_from_point(*center, *major, *ratio, prev);
                    let t_cur = angle_from_point(*center, *major, *ratio, pt);
                    let mut d = t_cur - t_prev;
                    while d > std::f64::consts::PI {
                        d -= TAU;
                    }
                    while d <= -std::f64::consts::PI {
                        d += TAU;
                    }
                    if d.abs() > DIR_TOL {
                        self.cw = d < 0.0;
                        self.prev_pt = Some(pt);
                    }
                } else {
                    self.prev_pt = Some(pt);
                }
                let t_end = angle_from_point(*center, *major, *ratio, pt);
                Some(if self.cw {
                    ellipse_wire(*center, *major, *ratio, t_end, *t_start)
                } else {
                    ellipse_wire(*center, *major, *ratio, *t_start, t_end)
                })
            }
            _ => None,
        }
    }

    fn dyn_spec(&self) -> Option<crate::command::DynSpec> {
        use crate::command::{DynAnchor, DynFieldSpec, DynGuide, DynRole, DynSpec};
        match &self.step {
            ArcStep::Center | ArcStep::MajorAxis { .. } => None,
            // Minor half-length, square to the major axis.
            ArcStep::MinorRatio { center, major } => Some(DynSpec {
                anchor: DynAnchor::Point(*center),
                fields: vec![DynFieldSpec::new(DynRole::Distance)],
                guide: DynGuide::Perp,
                ref_point: Some(*center + *major),
            }),
            // Start / end sweep angles measured at the centre (the last point
            // is the previous pick, so anchor the angle arc at the centre).
            ArcStep::StartAngle { center, .. } | ArcStep::EndAngle { center, .. } => {
                Some(DynSpec {
                    anchor: DynAnchor::Point(*center),
                    fields: vec![DynFieldSpec::new(DynRole::Angle)],
                    guide: DynGuide::Polar,
                    ref_point: None,
                })
            }
        }
    }

    fn dyn_live_value(&self, cursor: DVec3) -> Option<f64> {
        if let ArcStep::MinorRatio { center, major } = &self.step {
            Some(minor_ratio(*center, *major, cursor) * major.length())
        } else {
            None
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────

fn minor_ratio(center: DVec3, major: DVec3, pt: DVec3) -> f64 {
    let r_major = major.length();
    if r_major < 1e-9 {
        return 0.5;
    }
    let major_dir = major / r_major;
    let to_pt = pt - center;
    let perp = to_pt - major_dir * to_pt.dot(major_dir);
    let r_minor = perp.length().max(1e-6);
    (r_minor / r_major).clamp(1e-6, 1.0)
}

fn angle_from_point(center: DVec3, major: DVec3, ratio: f64, pt: DVec3) -> f64 {
    let r_major = major.length();
    if r_major < 1e-9 {
        return 0.0;
    }
    let major_dir = major / r_major;
    let v = DVec3::Z.cross(major_dir).normalize();
    param_angle(center, major_dir, v, pt, ratio)
}

fn line_wire(from: DVec3, to: DVec3) -> WireModel {
    WireModel {
        name: "rubber_band".into(),
        points: vec![
            [from.x as f32, from.y as f32, from.z as f32],
            [to.x as f32, to.y as f32, to.z as f32],
        ],
        points_low: Vec::new(),
        color: WireModel::CYAN,
        selected: false,
        pattern_length: 0.0,
        pattern: [0.0; 8],
        line_weight_px: 1.0,
        snap_pts: vec![],
        tangent_geoms: vec![],
        aci: 0,
        key_vertices: vec![],
        aabb: WireModel::UNBOUNDED_AABB,
        plinegen: true,
        vp_scissor: None,
        fill_tris: vec![],
        fill_tris_low: Vec::new(),
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["ELLIPSE_ARC"] });  // EllipseArcCommand
inventory::submit!(crate::command::CommandRegistration { names: &["ELLIPSE_AXIS"] });  // EllipseAxisCommand
inventory::submit!(crate::command::CommandRegistration { names: &["EL", "ELLIPSE"] });  // EllipseCommand
