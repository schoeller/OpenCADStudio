// LEADER command
//
// Flow:
//   1. Click the arrowhead point, then one or more bend points.
//   2. Enter (≥2 points) places the leader line plus a linked, empty MText
//      annotation at the landing, and opens the in-place MText editor so the
//      user types the annotation. Escape leaves the leader without text.
//
// The MText is a separate entity referenced by the leader's annotation_handle
// (DXF group 340); editing/erasing them stays in sync via that link.

use acadrust::entities::mtext::AttachmentPoint;
use acadrust::entities::{Leader, LeaderCreationType, MText};
use acadrust::types::Vector3;
use acadrust::EntityType;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

pub const ICON: IconKind = IconKind::Svg(include_bytes!("../../../assets/icons/leader.svg"));

pub fn tool() -> ToolDef {
    ToolDef {
        id: "LEADER",
        label: "Leader",
        icon: ICON,
        event: ModuleEvent::Command("LEADER".to_string()),
    }
}

pub struct LeaderCommand {
    verts: Vec<Vec3>,
}

impl LeaderCommand {
    pub fn new() -> Self {
        Self { verts: Vec::new() }
    }
}

impl CadCommand for LeaderCommand {
    fn name(&self) -> &'static str {
        "LEADER"
    }

    fn prompt(&self) -> String {
        if self.verts.is_empty() {
            "LEADER  Specify arrowhead point:".into()
        } else {
            format!(
                "LEADER  Specify next point [{} pts — Enter to place text]:",
                self.verts.len()
            )
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        self.verts.push(pt);
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        if self.verts.len() < 2 {
            return CmdResult::Cancel;
        }
        // Place the leader plus an empty MText annotation, link them, then open
        // the in-place MText editor so the user types the annotation text.
        let leader = build_leader(&self.verts);
        let (anchor, attach) = annotation_anchor(&self.verts, leader.text_height);
        let mtext = build_mtext("", anchor, leader.text_height, attach);
        CmdResult::CommitManyAndEditText {
            entities: vec![EntityType::Leader(leader), EntityType::MText(mtext)],
            edit_index: 1,
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        if self.verts.is_empty() {
            return None;
        }
        let mut pts = self.verts.clone();
        pts.push(pt);
        Some(preview_wire(&pts))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn v3(p: Vec3) -> Vector3 {
    Vector3::new(p.x as f64, p.y as f64, p.z as f64)
}

fn build_leader(verts: &[Vec3]) -> Leader {
    let mut l = Leader::from_vertices(verts.iter().map(|p| v3(*p)).collect());
    l.creation_type = LeaderCreationType::WithText;
    l.hookline_enabled = true;
    l
}

/// Text anchor at the end of the landing line, and the attachment point that
/// keeps the text reading away from the leader (text to the right of a
/// left-pointing landing, to the left of a right-pointing one).
fn annotation_anchor(verts: &[Vec3], text_height: f64) -> (Vec3, AttachmentPoint) {
    let last = *verts.last().unwrap();
    let prev = verts[verts.len() - 2];
    let to_right = last.x >= prev.x;
    let sign = if to_right { 1.0_f32 } else { -1.0_f32 };
    let anchor = Vec3::new(last.x + sign * text_height as f32 * 1.5, last.y, last.z);
    let attach = if to_right {
        AttachmentPoint::MiddleLeft
    } else {
        AttachmentPoint::MiddleRight
    };
    (anchor, attach)
}

fn build_mtext(text: &str, pos: Vec3, height: f64, attach: AttachmentPoint) -> MText {
    let mut m = MText::new();
    m.value = text.to_string();
    m.insertion_point = v3(pos);
    m.height = height;
    m.attachment_point = attach;
    m
}

fn preview_wire(pts: &[Vec3]) -> WireModel {
    let mut points: Vec<[f32; 3]> = pts.iter().map(|p| [p.x, p.y, p.z]).collect();
    if pts.len() >= 2 {
        let [w1, w2] = arrowhead_wings(pts[0], pts[1], 2.5);
        points.push([f32::NAN; 3]);
        points.push([w1.x, w1.y, w1.z]);
        points.push([pts[0].x, pts[0].y, pts[0].z]);
        points.push([w2.x, w2.y, w2.z]);
    }
    WireModel {
        name: "leader_preview".into(),
        points,
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
    }
}

pub fn arrowhead_wings(tip: Vec3, next: Vec3, size: f32) -> [Vec3; 2] {
    let d = next - tip;
    let len = (d.x * d.x + d.y * d.y).sqrt().max(1e-9);
    let (dx, dy) = (d.x / len, d.y / len);
    let angle = std::f32::consts::PI / 6.0;
    let (s, c) = angle.sin_cos();
    [
        Vec3::new(
            tip.x + (dx * c - dy * s) * size,
            tip.y + (dx * s + dy * c) * size,
            tip.z,
        ),
        Vec3::new(
            tip.x + (dx * c + dy * s) * size,
            tip.y + (-dx * s + dy * c) * size,
            tip.z,
        ),
    ]
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["LE", "LEADER"] });  // LeaderCommand
