// DIMORDINATE command — ordinate (datum) dimension.
//
// Measures the X or Y distance from the UCS origin (datum) to a feature point.
// The user picks:
//   1. The feature location.
//   2. The leader endpoint (where the annotation line ends).
//
// If the leader moves mainly in Y → X-type ordinate (shows X coordinate).
// If the leader moves mainly in X → Y-type ordinate (shows Y coordinate).

use acadrust::entities::{Dimension, DimensionOrdinate};
use acadrust::types::Vector3;
use acadrust::EntityType;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

pub fn tool() -> ToolDef {
    ToolDef {
        id: "DIMORDINATE",
        label: "Ordinate",
        icon: IconKind::Svg(include_bytes!("../../../assets/icons/dim_ordinate.svg")),
        event: ModuleEvent::Command("DIMORDINATE".to_string()),
    }
}

enum Step {
    FeaturePoint,
    LeaderEndpoint { feature: Vec3 },
}

pub struct OrdinateDimCommand {
    step: Step,
}

impl OrdinateDimCommand {
    pub fn new() -> Self {
        Self {
            step: Step::FeaturePoint,
        }
    }
}

impl CadCommand for OrdinateDimCommand {
    fn name(&self) -> &'static str {
        "DIMORDINATE"
    }

    fn prompt(&self) -> String {
        match self.step {
            Step::FeaturePoint => "DIMORDINATE  Specify feature location:".into(),
            Step::LeaderEndpoint { .. } => "DIMORDINATE  Specify leader endpoint:".into(),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match self.step {
            Step::FeaturePoint => {
                self.step = Step::LeaderEndpoint { feature: pt };
                CmdResult::NeedPoint
            }
            Step::LeaderEndpoint { feature } => {
                let dx = (pt.x - feature.x).abs();
                let dy = (pt.z - feature.z).abs();
                // If leader is more vertical (Y-screen = Z-world moves more) → X ordinate.
                // If leader is more horizontal → Y ordinate.
                let is_x = dy >= dx;
                let feat_v3 = v3(feature);
                let lead_v3 = v3(pt);
                let dim = DimensionOrdinate::new(feat_v3, lead_v3, is_x);
                CmdResult::CommitAndExit(EntityType::Dimension(Dimension::Ordinate(dim)))
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_preview_wires(&mut self, _pt: Vec3) -> Vec<WireModel> {
        vec![]
    }
}

fn v3(p: Vec3) -> Vector3 {
    Vector3::new(p.x as f64, 0.0, p.z as f64)
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["DIMORDINATE", "DOR"] });  // OrdinateDimCommand
