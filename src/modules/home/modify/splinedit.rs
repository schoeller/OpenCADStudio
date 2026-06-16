// SPLINEDIT command — interactive spline editing.
//
// Phase 1: select a spline (entity pick)
// Phase 2: choose a sub-command:
//   CLOSE  — set the spline as closed (wrap last control point to first)
//   OPEN   — remove the closure
//   REVERSE— reverse the control point order
//   EXIT   — done (Enter / Escape)
//
// Control-point dragging is already supported via the grip editing system.

use acadrust::EntityType;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

#[allow(dead_code)]
pub fn tool() -> ToolDef {
    ToolDef {
        id: "SPLINEDIT",
        label: "Spline Edit",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/spline.svg")),
        event: ModuleEvent::Command("SPLINEDIT".to_string()),
    }
}

enum Step {
    /// Waiting for the user to pick a spline entity.
    SelectSpline,
    /// Spline selected; waiting for sub-command text input.
    SubCommand { handle: acadrust::Handle },
}

pub struct SplineditCommand {
    step: Step,
}

impl SplineditCommand {
    pub fn new() -> Self {
        Self {
            step: Step::SelectSpline,
        }
    }
}

impl CadCommand for SplineditCommand {
    fn name(&self) -> &'static str {
        "SPLINEDIT"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::SelectSpline => "SPLINEDIT  Select spline:".to_string(),
            Step::SubCommand { .. } => "SPLINEDIT  [CLOSE/OPEN/REVERSE/EXIT]:".to_string(),
        }
    }

    fn needs_entity_pick(&self) -> bool {
        matches!(self.step, Step::SelectSpline)
    }

    fn on_entity_pick(&mut self, handle: acadrust::Handle, _pt: Vec3) -> CmdResult {
        if handle.is_null() {
            return CmdResult::NeedPoint;
        }
        self.step = Step::SubCommand { handle };
        CmdResult::NeedPoint
    }

    fn wants_text_input(&self) -> bool {
        matches!(self.step, Step::SubCommand { .. })
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let Step::SubCommand { handle } = &self.step else {
            return None;
        };
        let handle = *handle;
        match text.trim().to_uppercase().as_str() {
            "CLOSE" | "C" => Some(CmdResult::ReplaceEntity(
                handle,
                vec![SplineOp::Close.into_marker(handle)],
            )),
            "OPEN" | "O" => Some(CmdResult::ReplaceEntity(
                handle,
                vec![SplineOp::Open.into_marker(handle)],
            )),
            "REVERSE" | "R" | "REV" => Some(CmdResult::ReplaceEntity(
                handle,
                vec![SplineOp::Reverse.into_marker(handle)],
            )),
            "EXIT" | "X" | "" => Some(CmdResult::Cancel),
            _ => None,
        }
    }

    fn on_point(&mut self, _pt: Vec3) -> CmdResult {
        CmdResult::NeedPoint
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_preview_wires(&mut self, _pt: Vec3) -> Vec<WireModel> {
        vec![]
    }
}

// ── Spline operation helpers ───────────────────────────────────────────────
//
// Because `CmdResult::ReplaceEntity` takes `Vec<EntityType>` and we need the
// host to apply it to the actual document, we use a small "deferred op" trick:
// the host applies the spline transform in `apply_cmd_result` by detecting
// that we replaced an entity with zero new entities (delete) or one new one.
//
// Here we actually build the modified entity without accessing the document —
// we only have the handle.  The real transformation (close/open/reverse) is
// applied in the Scene via `apply_spline_op`.

enum SplineOp {
    Close,
    Open,
    Reverse,
}

impl SplineOp {
    /// Return a placeholder that encodes the op in a comment field.
    /// The host `cmd_result.rs` detects SPLINEDIT replaces and calls
    /// `Scene::apply_spline_op`.
    fn into_marker(self, _handle: acadrust::Handle) -> EntityType {
        // We can't build the modified spline here without the document.
        // Return a sentinel — the actual transformation is handled by the
        // apply_spline_op path in Scene.  We use an XLine as a sentinel
        // that the host will never actually commit (it detects the SPLINEDIT
        // replace path and calls apply_spline_op instead).
        //
        // In practice: use ReplaceMany with empty new entities + a SplineOp
        // side-channel via the xattach_path mechanism isn't clean.
        //
        // Simpler: encode op in the sentinel entity's layer field.
        let mut sentinel = acadrust::entities::XLine::new(
            acadrust::types::Vector3::zero(),
            acadrust::types::Vector3::new(1.0, 0.0, 0.0),
        );
        sentinel.common.layer = match self {
            SplineOp::Close => "__SPLINEDIT_CLOSE__".to_string(),
            SplineOp::Open => "__SPLINEDIT_OPEN__".to_string(),
            SplineOp::Reverse => "__SPLINEDIT_REVERSE__".to_string(),
        };
        EntityType::XLine(sentinel)
    }
}

/// Apply a spline operation (CLOSE/OPEN/REVERSE) to a spline entity.
/// Called from `cmd_result.rs` when the ReplaceEntity sentinel is detected.
pub fn apply_spline_op(doc: &mut acadrust::CadDocument, handle: acadrust::Handle, op: &str) {
    let Some(EntityType::Spline(spline)) = doc.get_entity_mut(handle) else {
        return;
    };
    match op {
        "__SPLINEDIT_CLOSE__" => {
            if spline.control_points.len() >= 2 {
                let first = spline.control_points[0];
                spline.control_points.push(first);
                // Regenerate clamped knots.
                spline.knots = acadrust::entities::Spline::generate_clamped_knots(
                    spline.degree as usize,
                    spline.control_points.len(),
                );
                spline.fit_points.clear();
            }
        }
        "__SPLINEDIT_OPEN__" => {
            if spline.control_points.len() >= 2 {
                let n = spline.control_points.len();
                let first = spline.control_points[0];
                let last = spline.control_points[n - 1];
                if (first.x - last.x).abs() < 1e-9
                    && (first.y - last.y).abs() < 1e-9
                    && (first.z - last.z).abs() < 1e-9
                {
                    spline.control_points.pop();
                    spline.knots = acadrust::entities::Spline::generate_clamped_knots(
                        spline.degree as usize,
                        spline.control_points.len(),
                    );
                    spline.fit_points.clear();
                }
            }
        }
        "__SPLINEDIT_REVERSE__" => {
            spline.control_points.reverse();
            spline.fit_points.reverse();
            spline.knots = acadrust::entities::Spline::generate_clamped_knots(
                spline.degree as usize,
                spline.control_points.len(),
            );
        }
        _ => {}
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["SPE", "SPLINEDIT"] });  // SplineditCommand
