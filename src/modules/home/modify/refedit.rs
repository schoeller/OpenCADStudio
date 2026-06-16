// REFEDIT — in-place block reference editing.
//
// Workflow:
//   1. REFEDIT: user picks an INSERT entity.
//   2. The block's entities are copied into model space with the INSERT
//      transform applied (translate + rotate + uniform scale).
//   3. The user edits them with normal commands (MOVE, COPY, DELETE, …).
//   4. REFCLOSE SAVE: temp entities are inverse-transformed back into the
//      block definition; all INSERT references auto-update.
//      REFCLOSE DISCARD: temp entities are removed, block unchanged.
//
// Limitation: non-uniform scale inserts (x_scale ≠ y_scale) are rejected
// with an error message — full matrix inversion for those cases would
// require per-entity matrix transforms not yet in EntityTransform.

use acadrust::{EntityType, Handle};
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};

// ── Session state (held in DocumentTab) ───────────────────────────────────

/// Active REFEDIT session.  Lives in `DocumentTab::refedit_session`.
#[derive(Debug, Clone)]
pub struct RefEditSession {
    /// Name of the block being edited.
    pub block_name: String,
    /// Handle of the block record (owns the block entities).
    pub br_handle: Handle,
    /// Handles of the temporary model-space entities added for editing.
    pub temp_handles: Vec<Handle>,
    // ── INSERT transform (needed for inverse on SAVE) ──────────────────
    pub insert_x: f64,
    pub insert_y: f64,
    pub insert_z: f64,
    /// Rotation in degrees (stored as degrees in acadrust).
    pub rotation_deg: f64,
    /// Uniform scale factor (same for X/Y/Z after validation).
    pub scale: f64,
}

// ── REFEDIT pick command ───────────────────────────────────────────────────

/// Step 1: wait for the user to pick a single INSERT entity.
pub struct RefEditPickCommand;

impl RefEditPickCommand {
    pub fn new() -> Self {
        Self
    }
}

impl CadCommand for RefEditPickCommand {
    fn name(&self) -> &'static str {
        "REFEDIT"
    }
    fn prompt(&self) -> String {
        "REFEDIT  Select block reference to edit:".into()
    }

    fn needs_entity_pick(&self) -> bool {
        true
    }

    fn on_entity_pick(&mut self, handle: Handle, _pt: Vec3) -> CmdResult {
        if handle.is_null() {
            return CmdResult::NeedPoint;
        }
        // Signal the host to enter the editing session for this handle.
        // We reuse Relaunch("REFEDIT_BEGIN:<handle>") as a convention.
        CmdResult::Relaunch(format!("REFEDIT_BEGIN:{}", handle.value()), vec![handle])
    }

    fn on_point(&mut self, _pt: Vec3) -> CmdResult {
        CmdResult::NeedPoint
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
}

// ── REFCLOSE command ───────────────────────────────────────────────────────

/// Step 4: prompt for SAVE or DISCARD.
pub struct RefCloseCommand;

impl RefCloseCommand {
    pub fn new() -> Self {
        Self
    }
}

impl CadCommand for RefCloseCommand {
    fn name(&self) -> &'static str {
        "REFCLOSE"
    }
    fn prompt(&self) -> String {
        "REFCLOSE  [Save/Discard] <Save>:".into()
    }
    fn wants_text_input(&self) -> bool {
        true
    }
    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let t = text.trim().to_uppercase();
        let save = t.is_empty() || t == "S" || t == "SAVE";
        let discard = t == "D" || t == "DISCARD";
        if save {
            Some(CmdResult::Relaunch("REFCLOSE_SAVE".into(), vec![]))
        } else if discard {
            Some(CmdResult::Relaunch("REFCLOSE_DISCARD".into(), vec![]))
        } else {
            Some(CmdResult::NeedPoint) // re-prompt
        }
    }
    fn on_enter(&mut self) -> CmdResult {
        // Default: SAVE
        CmdResult::Relaunch("REFCLOSE_SAVE".into(), vec![])
    }
    fn on_point(&mut self, _pt: Vec3) -> CmdResult {
        CmdResult::NeedPoint
    }
}

// ── Geometry helpers ───────────────────────────────────────────────────────

/// Apply the INSERT's forward transform to a block-local entity so it
/// appears at its correct world-space position.
/// Order: scale → rotate (around origin) → translate.
pub fn apply_insert_transform(entity: &mut EntityType, session: &RefEditSession) {
    use crate::command::EntityTransform;
    use crate::scene::view::dispatch;

    let origin = Vec3::ZERO;

    // 1. Uniform scale (if not 1.0)
    if (session.scale - 1.0).abs() > 1e-10 {
        dispatch::apply_transform(
            entity,
            &EntityTransform::Scale {
                center: origin,
                factor: session.scale as f32,
            },
        );
    }

    // 2. Rotate around origin
    if session.rotation_deg.abs() > 1e-10 {
        dispatch::apply_transform(
            entity,
            &EntityTransform::Rotate {
                center: origin,
                angle_rad: session.rotation_deg.to_radians() as f32,
            },
        );
    }

    // 3. Translate to insert position
    dispatch::apply_transform(
        entity,
        &EntityTransform::Translate(Vec3::new(
            session.insert_x as f32,
            session.insert_y as f32,
            session.insert_z as f32,
        )),
    );
}

/// Apply the INSERT's inverse transform to a world-space entity to bring it
/// back to block-local coordinates.
/// Order: un-translate → un-rotate → un-scale.
pub fn apply_insert_inverse_transform(entity: &mut EntityType, session: &RefEditSession) {
    use crate::command::EntityTransform;
    use crate::scene::view::dispatch;

    let origin = Vec3::ZERO;

    // 1. Un-translate
    dispatch::apply_transform(
        entity,
        &EntityTransform::Translate(Vec3::new(
            -session.insert_x as f32,
            -session.insert_y as f32,
            -session.insert_z as f32,
        )),
    );

    // 2. Un-rotate
    if session.rotation_deg.abs() > 1e-10 {
        dispatch::apply_transform(
            entity,
            &EntityTransform::Rotate {
                center: origin,
                angle_rad: -session.rotation_deg.to_radians() as f32,
            },
        );
    }

    // 3. Un-scale
    if (session.scale - 1.0).abs() > 1e-10 && session.scale.abs() > 1e-12 {
        dispatch::apply_transform(
            entity,
            &EntityTransform::Scale {
                center: origin,
                factor: (1.0 / session.scale) as f32,
            },
        );
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["REFCLOSE"] });  // RefCloseCommand
inventory::submit!(crate::command::CommandRegistration { names: &["REFEDIT"] });  // RefEditPickCommand
