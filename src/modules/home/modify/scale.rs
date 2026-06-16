// Scale tool — ribbon definition + interactive command.
//
// Command:  SCALE (SC)
//   Requires at least one entity selected.
//   Step 1: pick base (scale center)
//   Step 2: specify scale factor — drag for a live preview (factor = cursor
//           distance from base) or type a factor. A live ghost of the
//           selection tracks the cursor from the first move onward.
//
//   Reference scaling: type `R` at step 2 to define the factor as
//   new-length / reference-length:
//     Step 2a: specify reference length (pick a point or type a length)
//     Step 2b: specify new length      (pick a point or type a length)

use acadrust::Handle;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult, EntityTransform};
use crate::modules::home::defaults;
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

#[allow(dead_code)]
pub fn tool() -> ToolDef {
    ToolDef {
        id: "SCALE",
        label: "Scale",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/scale.svg")),
        event: ModuleEvent::Command("SCALE".to_string()),
    }
}

enum Step {
    Base,
    /// Default flow: factor is the cursor distance from `base`.
    Factor { base: Vec3 },
    /// Reference flow: defining the reference length from `base`.
    RefLen { base: Vec3 },
    /// Reference flow: factor is `cursor_dist / ref_dist` from `base`.
    RefNew { base: Vec3, ref_dist: f32 },
}

pub struct ScaleCommand {
    handles: Vec<Handle>,
    wire_models: Vec<WireModel>,
    step: Step,
    default_factor: f32,
}

impl ScaleCommand {
    pub fn new(handles: Vec<Handle>, wire_models: Vec<WireModel>) -> Self {
        Self {
            handles,
            wire_models,
            step: Step::Base,
            default_factor: defaults::get_scale_factor(),
        }
    }

    /// Commit a uniform scale about `base` and end the command.
    fn commit(&self, base: Vec3, factor: f32) -> CmdResult {
        defaults::set_scale_factor(factor);
        CmdResult::TransformSelected(
            self.handles.clone(),
            EntityTransform::Scale {
                center: base,
                factor,
            },
        )
    }
}

impl CadCommand for ScaleCommand {
    fn name(&self) -> &'static str {
        "SCALE"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::Base => format!(
                "SCALE  Specify base point  [{} objects]:",
                self.handles.len()
            ),
            Step::Factor { .. } => format!(
                "SCALE  Specify scale factor or [Reference]  <{:.4}>:",
                self.default_factor
            ),
            Step::RefLen { .. } => {
                "SCALE  Specify reference length  (pick a point or type a length):".into()
            }
            Step::RefNew { ref_dist, .. } => format!(
                "SCALE  Specify new length  (pick a point or type a length)  [ref={:.3}]:",
                ref_dist
            ),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match &self.step {
            Step::Base => {
                self.step = Step::Factor { base: pt };
                CmdResult::NeedPoint
            }
            Step::Factor { base } => {
                let base = *base;
                let factor = base.distance(pt).max(1e-6);
                self.commit(base, factor)
            }
            Step::RefLen { base } => {
                let base = *base;
                let ref_dist = base.distance(pt).max(1e-6);
                self.step = Step::RefNew { base, ref_dist };
                CmdResult::NeedPoint
            }
            Step::RefNew { base, ref_dist } => {
                let base = *base;
                let new_dist = base.distance(pt).max(1e-6);
                self.commit(base, new_dist / *ref_dist)
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        // Enter at the factor step accepts the stored default factor.
        if let Step::Factor { base } = &self.step {
            let base = *base;
            return self.commit(base, self.default_factor);
        }
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let t = text.trim();
        match &self.step {
            Step::Factor { base } => {
                let base = *base;
                // `R` / `Reference` switches to reference scaling.
                let low = t.to_ascii_lowercase();
                if low == "r" || low == "reference" {
                    self.step = Step::RefLen { base };
                    return Some(CmdResult::NeedPoint);
                }
                let factor: f32 = t.replace(',', ".").parse().ok()?;
                (factor > 0.0).then(|| self.commit(base, factor))
            }
            Step::RefLen { base } => {
                let base = *base;
                let ref_dist: f32 = t.replace(',', ".").parse().ok()?;
                if ref_dist > 0.0 {
                    self.step = Step::RefNew { base, ref_dist };
                    return Some(CmdResult::NeedPoint);
                }
                None
            }
            Step::RefNew { base, ref_dist } => {
                let (base, ref_dist) = (*base, *ref_dist);
                let new_len: f32 = t.replace(',', ".").parse().ok()?;
                (new_len > 0.0).then(|| self.commit(base, new_len / ref_dist))
            }
            Step::Base => None,
        }
    }

    fn on_preview_wires(&mut self, pt: Vec3) -> Vec<WireModel> {
        let (base, factor) = match &self.step {
            // Default flow: scale live by cursor distance from the base.
            Step::Factor { base } => (*base, base.distance(pt).max(1e-6)),
            // Reference flow, new-length step: factor = cursor_dist / ref_dist.
            Step::RefNew { base, ref_dist } => {
                (*base, base.distance(pt).max(1e-6) / ref_dist)
            }
            // Reference-length step: rubber-band only, no factor defined yet.
            Step::RefLen { base } => {
                return vec![WireModel::solid(
                    "rubber_band".into(),
                    vec![[base.x, base.y, base.z], [pt.x, pt.y, pt.z]],
                    WireModel::CYAN,
                    false,
                )];
            }
            Step::Base => return vec![],
        };
        let mut out: Vec<WireModel> = self
            .wire_models
            .iter()
            .map(|w| w.scaled(base, factor))
            .collect();
        out.push(WireModel::solid(
            "rubber_band".into(),
            vec![[base.x, base.y, base.z], [pt.x, pt.y, pt.z]],
            WireModel::CYAN,
            false,
        ));
        out
    }
}
