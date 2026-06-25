// DIMCONTINUE command — chain linear/aligned dimensions end-to-end.
//
// Each new point becomes the second extension line origin of a new dimension,
// whose first extension line origin is the second extension line of the previous dim.
// The dimension line stays at the same perpendicular offset as the base dimension.
//
// Constructed from commands.rs after finding the last placed linear/aligned dimension.

use acadrust::entities::{Dimension, DimensionLinear};
use acadrust::types::Vector3;
use acadrust::EntityType;
use glam::{DVec3, Vec3};

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

pub const ICON: IconKind = IconKind::Svg(include_bytes!("../../../assets/icons/dim_continue.svg"));

pub fn tool() -> ToolDef {
    ToolDef {
        id: "DIMCONTINUE",
        label: "Continue",
        icon: ICON,
        event: ModuleEvent::Command("DIMCONTINUE".to_string()),
    }
}

pub struct DimContinueCommand {
    /// Fixed first-extension-line origin for the current step (moves each iteration).
    chain_p1: Vec3,
    /// Direction along the dimension axis (0.0 = horizontal, PI/2 = vertical).
    rotation: f64,
    /// Absolute perpendicular coordinate (dot with `perp`) of the base
    /// dimension line. Each continued dim line is projected onto this exact
    /// coordinate so the whole chain stays collinear — even when the extension
    /// origins sit at different perpendicular positions (extension lines of
    /// different lengths). (#151)
    dim_line_perp: f32,
    /// Direction of "up" perpendicular to the dim axis (points toward the dim line).
    perp: Vec3,
    /// True once we have a base dimension loaded.
    ready: bool,
}

impl DimContinueCommand {
    /// No base dim found — will show an error prompt and cancel immediately.
    pub fn new() -> Self {
        Self {
            chain_p1: Vec3::ZERO,
            rotation: 0.0,
            dim_line_perp: 0.0,
            perp: Vec3::Y,
            ready: false,
        }
    }

    /// Build from the last placed dimension.
    ///
    /// `p2` — second extension line origin of the base dim (the chain starts
    ///   here). `p1` is unused — the dim line position comes from
    ///   `definition_point` alone — but kept for signature parity with the
    ///   shared `find_last_linear_dim` tuple (DIMBASELINE uses p1).
    /// `definition_point` — where the dim line was placed; its perpendicular
    ///   coordinate is the line the whole chain stays collinear with.
    /// `rotation` — 0.0 = horizontal dim, PI/2 = vertical dim.
    pub fn from_base(_p1: Vec3, p2: Vec3, definition_point: Vec3, rotation: f64) -> Self {
        // Axis unit vector along the measurement direction — the base dim's
        // rotation angle (any angle, incl. a UCS-aligned one), not a world H/V.
        let axis = Vec3::new(rotation.cos() as f32, rotation.sin() as f32, 0.0);
        // Perpendicular unit vector toward the dim line.
        let perp = Vec3::new(-axis.y, axis.x, 0.0);
        let dim_line_perp = definition_point.dot(perp);
        Self {
            chain_p1: p2,
            rotation,
            dim_line_perp,
            perp,
            ready: true,
        }
    }
}

impl CadCommand for DimContinueCommand {
    fn name(&self) -> &'static str {
        "DIMCONTINUE"
    }

    fn prompt(&self) -> String {
        if !self.ready {
            "DIMCONTINUE  No base dimension found. Place a dimension first.".into()
        } else {
            "DIMCONTINUE  Specify a second extension line origin (Enter to exit):".into()
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult { let pt = pt.as_vec3();
        if !self.ready {
            return CmdResult::Cancel;
        }
        let p1 = self.chain_p1;
        let p2 = pt;

        // Build a new linear dimension.
        let mut dim = DimensionLinear::new(v3(p1), v3(p2));
        dim.rotation = self.rotation;

        // Dim line stays collinear with the base: project both extension
        // origins onto the base dim line's absolute perpendicular coordinate.
        // (A fixed offset from p1 drifts off the base line whenever p1 and p2
        // sit at different perpendicular positions.) (#151)
        let d1 = p1 + self.perp * (self.dim_line_perp - p1.dot(self.perp));
        let d2 = p2 + self.perp * (self.dim_line_perp - p2.dot(self.perp));
        dim.definition_point = v3(d1);
        dim.base.definition_point = v3(d1);
        dim.base.text_middle_point = v3((d1 + d2) * 0.5);
        dim.base.insertion_point = dim.base.text_middle_point;
        dim.base.actual_measurement = dim.measurement();

        // Advance chain: next dim's P1 = this dim's P2.
        self.chain_p1 = p2;

        CmdResult::CommitEntity(EntityType::Dimension(Dimension::Linear(dim)))
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> { let pt = pt.as_vec3();
        if !self.ready {
            return None;
        }
        let p1 = self.chain_p1;
        let dim_line_pt = p1 + self.perp * (self.dim_line_perp - p1.dot(self.perp));
        let dim_line_pt2 = pt + self.perp * (self.dim_line_perp - pt.dot(self.perp));
        Some(WireModel {
            name: "dimcont_preview".into(),
            points: vec![
                [p1.x, p1.y, p1.z],
                [dim_line_pt.x, dim_line_pt.y, dim_line_pt.z],
                [f32::NAN, 0.0, 0.0],
                [pt.x, pt.y, pt.z],
                [dim_line_pt2.x, dim_line_pt2.y, dim_line_pt2.z],
                [f32::NAN, 0.0, 0.0],
                [dim_line_pt.x, dim_line_pt.y, dim_line_pt.z],
                [dim_line_pt2.x, dim_line_pt2.y, dim_line_pt2.z],
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
        })
    }
}

fn v3(p: Vec3) -> Vector3 {
    Vector3::new(p.x as f64, p.y as f64, p.z as f64)
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["DCO", "DIMCONTINUE"] });  // DimContinueCommand
