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
use glam::{DVec3, Mat4, Vec3};

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
    verts: Vec<DVec3>,
    ucs: Mat4,
}

impl LeaderCommand {
    pub fn new() -> Self {
        Self { verts: Vec::new(), ucs: Mat4::IDENTITY }
    }
}

impl CadCommand for LeaderCommand {
    fn name(&self) -> &'static str {
        "LEADER"
    }

    fn set_ucs(&mut self, ucs: Mat4) {
        self.ucs = ucs;
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

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        self.verts.push(pt);
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        if self.verts.len() < 2 {
            return CmdResult::Cancel;
        }
        // Place the leader plus an empty MText annotation, link them, then open
        // the in-place MText editor so the user types the annotation text.
        let leader = build_leader(&self.verts, self.ucs);
        let (anchor, attach) = annotation_anchor(&self.verts, leader.text_height, self.ucs);
        let mtext = build_mtext("", anchor, leader.text_height, attach, self.ucs);
        CmdResult::CommitManyAndEditText {
            entities: vec![EntityType::Leader(leader), EntityType::MText(mtext)],
            edit_index: 1,
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        if self.verts.is_empty() {
            return None;
        }
        let mut pts: Vec<Vec3> = self.verts.iter().map(|p| p.as_vec3()).collect();
        pts.push(pt.as_vec3());
        Some(preview_wire(&pts))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn dv3(p: DVec3) -> Vector3 {
    Vector3::new(p.x, p.y, p.z)
}

fn build_leader(verts: &[DVec3], ucs: Mat4) -> Leader {
    let mut l = Leader::from_vertices(verts.iter().map(|p| dv3(*p)).collect());
    l.creation_type = LeaderCreationType::WithText;
    l.hookline_enabled = true;
    // The hookline / text read along the UCS X axis (DXF "horizontal direction
    // for text"); the renderer uses it instead of world horizontal.
    let ux = ucs.transform_vector3(Vec3::X).normalize_or(Vec3::X);
    l.horizontal_direction = Vector3::new(ux.x as f64, ux.y as f64, 0.0);
    l
}

/// Text anchor at the end of the landing line, and the attachment point that
/// keeps the text reading away from the leader (text to the right of a
/// left-pointing landing, to the left of a right-pointing one).
fn annotation_anchor(
    verts: &[DVec3],
    text_height: f64,
    ucs: Mat4,
) -> (DVec3, AttachmentPoint) {
    let last = *verts.last().unwrap();
    let prev = verts[verts.len() - 2];
    // Side + landing run along the UCS X axis (identity = world).
    let ux = ucs.transform_vector3(Vec3::X).normalize_or(Vec3::X).as_dvec3();
    let to_right = (last - prev).dot(ux) >= 0.0;
    let sign = if to_right { 1.0_f64 } else { -1.0_f64 };
    let anchor = last + ux * (sign * text_height * 1.5);
    let attach = if to_right {
        AttachmentPoint::MiddleLeft
    } else {
        AttachmentPoint::MiddleRight
    };
    (anchor, attach)
}

fn build_mtext(
    text: &str,
    pos: DVec3,
    height: f64,
    attach: AttachmentPoint,
    ucs: Mat4,
) -> MText {
    let mut m = MText::new();
    m.value = text.to_string();
    m.insertion_point = dv3(pos);
    m.height = height;
    m.attachment_point = attach;
    // Text reads along the UCS X axis.
    let ux = ucs.transform_vector3(Vec3::X);
    m.rotation = (ux.y as f64).atan2(ux.x as f64);
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
        taper_widths: Vec::new(),
        world_width: 0.0,
        fill_is_3d: false,
        pick_tris: Vec::new(),
        pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
        name: "leader_preview".into(),
        points,
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
        fill_tris: vec![],
        fill_tris_low: Vec::new(),
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
inventory::submit!(crate::command::CommandRegistration { names: &["LEADER"] });  // LeaderCommand
