// MEASUREGEOM command (alias MEA) — multi-mode geometry measurement.
//
// First step prompts for a mode keyword:
//   Distance / Radius / Angle / ARea
// then collects the geometry the mode needs and prints a one-line readout
// via `CmdResult::Measurement`, ending the command.
//
//   DISTANCE — two points → distance, delta X/Y/Z, angle in the XY plane.
//   AREA     — points until Enter → area (f64 shoelace relative to the first
//              vertex, mirroring inquiry/area.rs for precision) + perimeter.
//   ANGLE    — three points (vertex + two ray endpoints) → angle in degrees.
//   RADIUS   — pick a Circle or Arc → radius + diameter.
//
// All arithmetic is kept in f64 (picked points stay full precision; downcasting
// to f32 loses several hundredths of a unit at survey-scale coordinates).

use acadrust::{EntityType, Handle};
use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

// ── Ribbon definition ─────────────────────────────────────────────────────

#[allow(dead_code)] // ribbon definition ready for wiring; command works via the command line
pub fn tool() -> ToolDef {
    ToolDef {
        id: "MEASUREGEOM",
        label: "Measure Geometry",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/line.svg")),
        event: ModuleEvent::Command("MEASUREGEOM".to_string()),
    }
}

// ── Mode ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Awaiting the mode keyword.
    Choose,
    Distance,
    Area,
    Angle,
    Radius,
}

// ── Command implementation ──────────────────────────────────────────────────

pub struct MeasureGeomCommand {
    mode: Mode,
    /// Picked points for the active point-pick mode.
    points: Vec<DVec3>,
    /// The picked entity for RADIUS, injected before `on_entity_pick`.
    picked: Option<EntityType>,
}

impl MeasureGeomCommand {
    pub fn new() -> Self {
        Self {
            mode: Mode::Choose,
            points: vec![],
            picked: None,
        }
    }

    /// Build a cyan preview wire connecting the picked points and the cursor.
    fn preview_wire(name: &str, pts: Vec<[f32; 3]>) -> WireModel {
        WireModel {
            name: name.to_string(),
            points: pts,
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

    /// Extract (radius) from a Circle or Arc; `None` for anything else.
    fn radius_of(entity: &EntityType) -> Option<f64> {
        match entity {
            EntityType::Circle(c) => Some(c.radius),
            EntityType::Arc(a) => Some(a.radius),
            _ => None,
        }
    }

    /// DISTANCE readout for the two collected points.
    fn distance_msg(p1: DVec3, p2: DVec3) -> String {
        let delta = p2 - p1;
        let dist = delta.length();
        let dx = delta.x;
        let dy = delta.y; // drawing plane is world XY
        let dz = delta.z; // elevation
        let angle_xy = dy.atan2(dx).to_degrees();
        format!(
            "Distance = {dist:.4},  Angle in XY Plane = {angle_xy:.4}°\n  Delta X = {dx:.4},  Delta Y = {dy:.4},  Delta Z = {dz:.4}",
        )
    }

    /// AREA readout: shoelace area (f64, relative to first vertex) + perimeter.
    fn area_msg(points: &[DVec3]) -> String {
        let n = points.len();
        let origin = points[0];
        let mut area_sum = 0.0f64;
        let mut perimeter = 0.0f64;
        for idx in 0..n {
            let a = points[idx] - origin;
            let b = points[(idx + 1) % n] - origin;
            area_sum += a.x * b.y - b.x * a.y;
            perimeter += (points[(idx + 1) % n] - points[idx]).length();
        }
        let area = (area_sum * 0.5).abs();
        format!("Area = {area:.4},  Perimeter = {perimeter:.4}")
    }

    /// ANGLE readout: angle at `vertex` between the rays to `a` and `b`.
    fn angle_msg(vertex: DVec3, a: DVec3, b: DVec3) -> String {
        let va = a - vertex;
        let vb = b - vertex;
        let la = va.length();
        let lb = vb.length();
        if la == 0.0 || lb == 0.0 {
            return "Angle = 0.0000° (degenerate rays)".to_string();
        }
        let cos = (va.dot(vb) / (la * lb)).clamp(-1.0, 1.0);
        let angle = cos.acos().to_degrees();
        format!("Angle = {angle:.4}°")
    }
}

impl CadCommand for MeasureGeomCommand {
    fn name(&self) -> &'static str {
        "MEASUREGEOM"
    }

    fn prompt(&self) -> String {
        match self.mode {
            Mode::Choose => {
                "MEASUREGEOM  Enter an option [Distance/Radius/Angle/ARea]:".into()
            }
            Mode::Distance => {
                if self.points.is_empty() {
                    "MEASUREGEOM  Specify first point:".into()
                } else {
                    "MEASUREGEOM  Specify second point:".into()
                }
            }
            Mode::Area => {
                if self.points.is_empty() {
                    "MEASUREGEOM  Specify first corner point (Enter to cancel):".into()
                } else {
                    format!(
                        "MEASUREGEOM  Specify next point ({} picked, Enter to calculate):",
                        self.points.len()
                    )
                }
            }
            Mode::Angle => match self.points.len() {
                0 => "MEASUREGEOM  Specify vertex point:".into(),
                1 => "MEASUREGEOM  Specify first ray point:".into(),
                _ => "MEASUREGEOM  Specify second ray point:".into(),
            },
            Mode::Radius => "MEASUREGEOM  Select arc or circle:".into(),
        }
    }

    fn wants_text_input(&self) -> bool {
        // Only the opening mode-keyword step reads a typed token.
        self.mode == Mode::Choose
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if self.mode != Mode::Choose {
            return None;
        }
        let t = text.trim().to_uppercase();
        self.mode = match t.as_str() {
            "D" | "DISTANCE" => Mode::Distance,
            "R" | "RADIUS" => Mode::Radius,
            "A" | "ANGLE" => Mode::Angle,
            "AR" | "AREA" => Mode::Area,
            _ => return Some(CmdResult::NeedPoint), // re-prompt on unknown keyword
        };
        Some(CmdResult::NeedPoint)
    }

    fn needs_entity_pick(&self) -> bool {
        self.mode == Mode::Radius
    }

    fn inject_before_entity_pick(&self) -> bool {
        true
    }

    fn inject_picked_entity(&mut self, entity: EntityType) {
        self.picked = Some(entity);
    }

    fn on_entity_pick(&mut self, handle: Handle, _pt: DVec3) -> CmdResult {
        if handle.is_null() {
            return CmdResult::NeedPoint;
        }
        match self.picked.as_ref().and_then(Self::radius_of) {
            Some(radius) => {
                let diameter = radius * 2.0;
                CmdResult::Measurement(format!(
                    "Radius = {radius:.4},  Diameter = {diameter:.4}"
                ))
            }
            // Picked something that is not a circle or arc — keep prompting.
            None => CmdResult::NeedPoint,
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match self.mode {
            Mode::Distance => {
                self.points.push(pt);
                if self.points.len() == 2 {
                    CmdResult::Measurement(Self::distance_msg(self.points[0], self.points[1]))
                } else {
                    CmdResult::NeedPoint
                }
            }
            Mode::Angle => {
                self.points.push(pt);
                if self.points.len() == 3 {
                    CmdResult::Measurement(Self::angle_msg(
                        self.points[0],
                        self.points[1],
                        self.points[2],
                    ))
                } else {
                    CmdResult::NeedPoint
                }
            }
            Mode::Area => {
                self.points.push(pt);
                CmdResult::NeedPoint
            }
            // Choose / Radius do not take point picks.
            _ => CmdResult::NeedPoint,
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        match self.mode {
            Mode::Area => {
                if self.points.len() < 3 {
                    CmdResult::Cancel
                } else {
                    CmdResult::Measurement(Self::area_msg(&self.points))
                }
            }
            _ => CmdResult::Cancel,
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        let f = |p: DVec3| [p.x as f32, p.y as f32, p.z as f32];
        match self.mode {
            Mode::Distance | Mode::Angle => {
                if self.points.is_empty() {
                    return None;
                }
                let mut pts: Vec<[f32; 3]> = self.points.iter().map(|p| f(*p)).collect();
                pts.push(f(pt));
                Some(Self::preview_wire("measuregeom_preview", pts))
            }
            Mode::Area => {
                if self.points.is_empty() {
                    return None;
                }
                let mut pts: Vec<[f32; 3]> = self.points.iter().map(|p| f(*p)).collect();
                pts.push(f(pt));
                pts.push(f(self.points[0]));
                Some(Self::preview_wire("measuregeom_preview", pts))
            }
            _ => None,
        }
    }
}

// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration {
    names: &["MEASUREGEOM", "MEA"]
}); // MeasureGeomCommand
