// Spline tool — ribbon definition + interactive command.
//
// Command:  SPLINE (SPL)
//   Click to add control points.  Enter (≥2 pts) → commits EntityType::Spline.

use acadrust::types::Vector3;
use acadrust::{EntityType, Spline};

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::Vec3;

#[allow(dead_code)]
pub fn tool() -> ToolDef {
    ToolDef {
        id: "SPLINE",
        label: "Spline",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/spline.svg")),
        event: ModuleEvent::Command("SPLINE".to_string()),
    }
}

pub struct SplineCommand {
    pts: Vec<Vec3>,
}

impl SplineCommand {
    pub fn new() -> Self {
        Self { pts: Vec::new() }
    }

    fn build(&self) -> Option<EntityType> {
        if self.pts.len() < 2 {
            return None;
        }
        let control_points = self
            .pts
            .iter()
            .map(|p| Vector3::new(p.x as f64, p.y as f64, p.z as f64))
            .collect();
        let n = self.pts.len();
        // Uniform open knot vector for degree-3 clamped B-spline
        let degree = 3_i32.min((n - 1) as i32);
        let knots = uniform_knots(n, degree as usize);
        let spline = Spline {
            degree,
            control_points,
            knots,
            ..Default::default()
        };
        Some(EntityType::Spline(spline))
    }
}

fn uniform_knots(n: usize, d: usize) -> Vec<f64> {
    let m = n + d + 1;
    (0..m)
        .map(|i| {
            if i <= d {
                0.0
            } else if i >= m - d - 1 {
                1.0
            } else {
                (i - d) as f64 / (n - d) as f64
            }
        })
        .collect()
}

impl CadCommand for SplineCommand {
    fn name(&self) -> &'static str {
        "SPLINE"
    }

    fn prompt(&self) -> String {
        if self.pts.is_empty() {
            "SPLINE  Specify first control point:".into()
        } else {
            format!(
                "SPLINE  Specify next point  [{} pts | Enter=done]:",
                self.pts.len()
            )
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        self.pts.push(pt);
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        match self.build() {
            Some(e) => CmdResult::CommitEntity(e),
            None => CmdResult::Cancel,
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        match self.build() {
            Some(e) => CmdResult::CommitEntity(e),
            None => CmdResult::Cancel,
        }
    }

    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        if self.pts.is_empty() {
            return None;
        }
        // Show all committed points + cursor as a connected polyline.
        let mut pts: Vec<[f32; 3]> = self.pts.iter().map(|p| [p.x, p.y, p.z]).collect();
        pts.push([pt.x, pt.y, pt.z]);
        Some(WireModel::solid(
            "rubber_band".into(),
            pts,
            WireModel::CYAN,
            false,
        ))
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["SPL", "SPLINE"] });  // SplineCommand
