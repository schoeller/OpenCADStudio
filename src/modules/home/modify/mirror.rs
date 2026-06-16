// Mirror tool — ribbon definition + interactive command.
//
// Command:  MIRROR (MI)
//   Requires at least one entity selected.
//   Step 1: pick first mirror-line point
//   Step 2: pick second mirror-line point
//   Step 3: "Erase source objects? [Yes/No] <No>"
//           No  → keep the original, add a mirrored copy
//           Yes → flip the original in place (no copy kept)

use acadrust::Handle;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult, EntityTransform};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

pub fn tool() -> ToolDef {
    ToolDef {
        id: "MIRROR",
        label: "Mirror",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/mirror.svg")),
        event: ModuleEvent::Command("MIRROR".to_string()),
    }
}

enum Step {
    P1,
    P2(Vec3),
    /// Both mirror-line points fixed; waiting on the erase-source answer.
    AskErase { p1: Vec3, p2: Vec3 },
}

pub struct MirrorCommand {
    handles: Vec<Handle>,
    wire_models: Vec<WireModel>,
    step: Step,
}

impl MirrorCommand {
    pub fn new(handles: Vec<Handle>, wire_models: Vec<WireModel>) -> Self {
        Self {
            handles,
            wire_models,
            step: Step::P1,
        }
    }
}

impl CadCommand for MirrorCommand {
    fn name(&self) -> &'static str {
        "MIRROR"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::P1 => format!(
                "MIRROR  Specify first mirror-line point  [{} objects]:",
                self.handles.len()
            ),
            Step::P2(p1) => format!(
                "MIRROR  Specify second point  [p1={:.2},{:.2}]:",
                p1.x, p1.y
            ),
            Step::AskErase { .. } => "MIRROR  Erase source objects? [Yes/No] <No>:".to_string(),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match &self.step {
            Step::P1 => {
                self.step = Step::P2(pt);
                CmdResult::NeedPoint
            }
            Step::P2(p1) => {
                self.step = Step::AskErase { p1: *p1, p2: pt };
                CmdResult::NeedPoint
            }
            // Second point is fixed; further clicks ignored until the
            // erase-source question is answered via the command line.
            Step::AskErase { .. } => CmdResult::NeedPoint,
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        // Enter at the erase prompt accepts the default (No → keep source).
        match &self.step {
            Step::AskErase { p1, p2 } => self.finish(*p1, *p2, false),
            _ => CmdResult::Cancel,
        }
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn wants_text_input(&self) -> bool {
        matches!(self.step, Step::AskErase { .. })
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let Step::AskErase { p1, p2 } = &self.step else {
            return None;
        };
        let (p1, p2) = (*p1, *p2);
        let t = text.trim().to_ascii_lowercase();
        let erase = match t.as_str() {
            "y" | "yes" => true,
            // Empty input (bare Enter) or an explicit No keeps the source.
            "" | "n" | "no" => false,
            // Unrecognised input: re-ask without committing.
            _ => return Some(CmdResult::NeedPoint),
        };
        Some(self.finish(p1, p2, erase))
    }

    fn on_preview_wires(&mut self, pt: Vec3) -> Vec<WireModel> {
        // While picking the second point the ghost tracks the cursor; once it
        // is fixed (erase prompt) the ghost freezes at the chosen axis.
        let (p1, p2) = match &self.step {
            Step::P2(p1) => (*p1, pt),
            Step::AskErase { p1, p2 } => (*p1, *p2),
            _ => return vec![],
        };
        // Mirrored ghosts of all selected objects.
        let mut out: Vec<WireModel> = self
            .wire_models
            .iter()
            .map(|w| w.mirrored(p1, p2))
            .collect();
        // Mirror-axis line (rubber-band).
        out.push(WireModel::solid(
            "rubber_band".into(),
            vec![[p1.x, p1.y, p1.z], [p2.x, p2.y, p2.z]],
            WireModel::CYAN,
            false,
        ));
        out
    }
}

impl MirrorCommand {
    /// Commit the mirror. `erase` true flips the originals in place; false
    /// keeps them and adds a mirrored copy. Either way the command ends.
    fn finish(&self, p1: Vec3, p2: Vec3, erase: bool) -> CmdResult {
        let xform = EntityTransform::Mirror { p1, p2 };
        if erase {
            CmdResult::TransformSelected(self.handles.clone(), xform)
        } else {
            CmdResult::BatchCopy(self.handles.clone(), vec![xform])
        }
    }
}
