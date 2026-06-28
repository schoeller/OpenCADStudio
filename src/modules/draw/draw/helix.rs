// Parametric helix tool — interactive command.
//
// Command: HELIX — build a parametric 3D helix and commit it as a single 3D
// polyline whose straight segments approximate the curve. The user picks a
// base centre point, then types the base radius, the top radius (Enter keeps
// it equal to the base radius for a cylindrical helix), the height, and the
// number of turns (Enter accepts a default of three turns).
//
// The curve is sampled at a fixed angular resolution: with `SEG_PER_TURN`
// samples per turn the helix has `turns * SEG_PER_TURN` segments. For each
// sample the radius interpolates linearly from base to top, the elevation
// interpolates linearly from zero to the height, and the planar position is
// `(radius*cos(angle), radius*sin(angle))` offset from the centre.

use acadrust::entities::Polyline3D;
use acadrust::types::Vector3;
use acadrust::EntityType;

use crate::command::{CadCommand, CmdResult, DynField};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

/// Angular samples per full turn of the helix.
const SEG_PER_TURN: usize = 36;
/// Default number of turns when the user accepts the prompt with Enter.
const DEFAULT_TURNS: f64 = 3.0;

// ── Ribbon definition ─────────────────────────────────────────────────────

#[allow(dead_code)] // ribbon definition ready for wiring; command works via the command line
pub fn tool() -> ToolDef {
    ToolDef {
        id: "HELIX",
        label: "Helix",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/line.svg")),
        event: ModuleEvent::Command("HELIX".to_string()),
    }
}

// ── Command implementation ────────────────────────────────────────────────

/// Which parameter the command is currently collecting.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Center,
    BaseRadius,
    TopRadius,
    Height,
    Turns,
}

pub struct HelixCommand {
    step: Step,
    center: DVec3,
    base_radius: f64,
    top_radius: f64,
    height: f64,
    turns: f64,
}

impl HelixCommand {
    pub fn new() -> Self {
        Self {
            step: Step::Center,
            center: DVec3::ZERO,
            base_radius: 0.0,
            top_radius: 0.0,
            height: 0.0,
            turns: DEFAULT_TURNS,
        }
    }

    /// Sample the helix into a list of world points.
    fn sample_points(&self, base_radius: f64, top_radius: f64, height: f64, turns: f64) -> Vec<DVec3> {
        let turns = if turns <= 0.0 { DEFAULT_TURNS } else { turns };
        let segments = ((turns * SEG_PER_TURN as f64).round() as usize).max(1);
        let mut pts = Vec::with_capacity(segments + 1);
        for t in 0..=segments {
            let frac = t as f64 / segments as f64;
            let angle = turns * std::f64::consts::TAU * frac;
            let radius = base_radius + (top_radius - base_radius) * frac;
            let z = height * frac;
            pts.push(DVec3::new(
                self.center.x + radius * angle.cos(),
                self.center.y + radius * angle.sin(),
                self.center.z + z,
            ));
        }
        pts
    }

    /// Build the committed helix entity from the collected parameters.
    fn build(&self) -> Option<EntityType> {
        let pts = self.sample_points(self.base_radius, self.top_radius, self.height, self.turns);
        if pts.len() < 2 {
            return None;
        }
        let verts: Vec<Vector3> = pts.iter().map(|p| Vector3::new(p.x, p.y, p.z)).collect();
        let mut pl = Polyline3D::from_points(verts);
        pl.flags.closed = false;
        Some(EntityType::Polyline3D(pl))
    }

    /// Parse a positive distance from typed text; returns `None` on failure.
    fn parse_value(text: &str) -> Option<f64> {
        text.trim().parse::<f64>().ok()
    }
}

impl CadCommand for HelixCommand {
    fn name(&self) -> &'static str {
        "HELIX"
    }

    fn prompt(&self) -> String {
        match self.step {
            Step::Center => "HELIX  Specify centre point of base:".to_string(),
            Step::BaseRadius => "HELIX  Specify base radius:".to_string(),
            Step::TopRadius => "HELIX  Specify top radius <same as base>:".to_string(),
            Step::Height => "HELIX  Specify helix height:".to_string(),
            Step::Turns => "HELIX  Enter number of turns <3>:".to_string(),
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match self.step {
            Step::Center => {
                self.center = pt;
                self.step = Step::BaseRadius;
                CmdResult::NeedPoint
            }
            Step::BaseRadius => {
                // A point click resolves the base radius as the planar distance
                // from the centre to the picked point.
                let r = ((pt.x - self.center.x).powi(2) + (pt.y - self.center.y).powi(2)).sqrt();
                if r > 0.0 {
                    self.base_radius = r;
                    self.step = Step::TopRadius;
                }
                CmdResult::NeedPoint
            }
            Step::TopRadius => {
                let r = ((pt.x - self.center.x).powi(2) + (pt.y - self.center.y).powi(2)).sqrt();
                self.top_radius = if r > 0.0 { r } else { self.base_radius };
                self.step = Step::Height;
                CmdResult::NeedPoint
            }
            Step::Height => {
                // Use the elevation of the picked point relative to the centre.
                let h = pt.z - self.center.z;
                if h.abs() > 0.0 {
                    self.height = h;
                    self.step = Step::Turns;
                }
                CmdResult::NeedPoint
            }
            Step::Turns => CmdResult::NeedPoint,
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        match self.step {
            Step::TopRadius => {
                // Enter keeps the top radius equal to the base radius.
                self.top_radius = self.base_radius;
                self.step = Step::Height;
                CmdResult::NeedPoint
            }
            Step::Turns => match self.build() {
                Some(e) => CmdResult::CommitAndExit(e),
                None => CmdResult::Cancel,
            },
            _ => CmdResult::NeedPoint,
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn wants_text_input(&self) -> bool {
        // Every step past the centre pick reads a typed numeric value.
        self.step != Step::Center
    }

    fn dyn_field(&self) -> DynField {
        match self.step {
            Step::Center => DynField::Point,
            Step::BaseRadius | Step::TopRadius => DynField::Distance,
            Step::Height => DynField::Distance,
            Step::Turns => DynField::Scalar,
        }
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let t = text.trim();
        match self.step {
            Step::Center => None,
            Step::BaseRadius => {
                if let Some(v) = Self::parse_value(t) {
                    if v > 0.0 {
                        self.base_radius = v;
                        self.step = Step::TopRadius;
                    }
                }
                Some(CmdResult::NeedPoint)
            }
            Step::TopRadius => {
                if t.is_empty() {
                    // Empty entry keeps the top radius equal to the base radius.
                    self.top_radius = self.base_radius;
                    self.step = Step::Height;
                } else if let Some(v) = Self::parse_value(t) {
                    self.top_radius = if v > 0.0 { v } else { self.base_radius };
                    self.step = Step::Height;
                }
                Some(CmdResult::NeedPoint)
            }
            Step::Height => {
                if let Some(v) = Self::parse_value(t) {
                    if v != 0.0 {
                        self.height = v;
                        self.step = Step::Turns;
                    }
                }
                Some(CmdResult::NeedPoint)
            }
            Step::Turns => {
                self.turns = if t.is_empty() {
                    DEFAULT_TURNS
                } else {
                    match Self::parse_value(t) {
                        Some(v) if v > 0.0 => v,
                        _ => return Some(CmdResult::NeedPoint),
                    }
                };
                match self.build() {
                    Some(e) => Some(CmdResult::CommitAndExit(e)),
                    None => Some(CmdResult::Cancel),
                }
            }
        }
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        // Preview the helix once enough parameters are known. Before a base
        // radius is fixed, track the cursor distance to size a live ring.
        match self.step {
            Step::Center => None,
            Step::BaseRadius => {
                let r = ((pt.x - self.center.x).powi(2) + (pt.y - self.center.y).powi(2)).sqrt();
                if r <= 0.0 {
                    return None;
                }
                let pts = self.sample_points(r, r, 0.0, 1.0);
                let raw: Vec<[f64; 3]> = pts.iter().map(|p| [p.x, p.y, p.z]).collect();
                Some(WireModel::solid_f64(
                    "helix_preview".to_string(),
                    raw,
                    WireModel::CYAN,
                    false,
                ))
            }
            _ => {
                // Use the parameters fixed so far; for the height step let the
                // cursor elevation drive the live height.
                let height = if self.step == Step::Height {
                    pt.z - self.center.z
                } else {
                    self.height
                };
                let top = if self.step == Step::TopRadius {
                    let r = ((pt.x - self.center.x).powi(2) + (pt.y - self.center.y).powi(2)).sqrt();
                    if r > 0.0 {
                        r
                    } else {
                        self.base_radius
                    }
                } else {
                    self.top_radius
                };
                let pts = self.sample_points(self.base_radius, top, height, self.turns);
                if pts.len() < 2 {
                    return None;
                }
                let raw: Vec<[f64; 3]> = pts.iter().map(|p| [p.x, p.y, p.z]).collect();
                Some(WireModel::solid_f64(
                    "helix_preview".to_string(),
                    raw,
                    WireModel::CYAN,
                    false,
                ))
            }
        }
    }
}

// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["HELIX"] });  // HelixCommand
