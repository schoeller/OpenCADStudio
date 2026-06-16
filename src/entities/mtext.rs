use acadrust::entities::{AttachmentPoint, DrawingDirection, MText};
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{edit_prop as edit, ro_prop as ro, square_grip, triangle_grip};
use crate::entities::text_support::{
    layout_mtext, resolve_text_style, GlyphBox, MTextRenderOpts, MTextVAnchor,
};
use crate::entities::traits::{Grippable, PropertyEditable, Transformable, TruckConvertible};
use crate::scene::convert::acad_to_truck::{TruckEntity, TruckObject};
use crate::scene::model::object::{GripApply, GripDef, PropSection, PropValue, Property};
use crate::scene::model::wire_model::SnapHint;

fn attachment_str(a: &AttachmentPoint) -> &'static str {
    match a {
        AttachmentPoint::TopLeft => "Top Left",
        AttachmentPoint::TopCenter => "Top Center",
        AttachmentPoint::TopRight => "Top Right",
        AttachmentPoint::MiddleLeft => "Middle Left",
        AttachmentPoint::MiddleCenter => "Middle Center",
        AttachmentPoint::MiddleRight => "Middle Right",
        AttachmentPoint::BottomLeft => "Bottom Left",
        AttachmentPoint::BottomCenter => "Bottom Center",
        AttachmentPoint::BottomRight => "Bottom Right",
    }
}

fn mtext_halign_str(a: &AttachmentPoint) -> &'static str {
    match a {
        AttachmentPoint::TopLeft | AttachmentPoint::MiddleLeft | AttachmentPoint::BottomLeft => {
            "Left"
        }
        AttachmentPoint::TopCenter
        | AttachmentPoint::MiddleCenter
        | AttachmentPoint::BottomCenter => "Center",
        AttachmentPoint::TopRight | AttachmentPoint::MiddleRight | AttachmentPoint::BottomRight => {
            "Right"
        }
    }
}

fn mtext_valign_str(a: &AttachmentPoint) -> &'static str {
    match a {
        AttachmentPoint::TopLeft | AttachmentPoint::TopCenter | AttachmentPoint::TopRight => "Top",
        AttachmentPoint::MiddleLeft
        | AttachmentPoint::MiddleCenter
        | AttachmentPoint::MiddleRight => "Middle",
        AttachmentPoint::BottomLeft
        | AttachmentPoint::BottomCenter
        | AttachmentPoint::BottomRight => "Bottom",
    }
}

fn mtext_attachment_from_align(h: &str, v: &str) -> Option<AttachmentPoint> {
    Some(match (h, v) {
        ("Left", "Top") => AttachmentPoint::TopLeft,
        ("Center", "Top") => AttachmentPoint::TopCenter,
        ("Right", "Top") => AttachmentPoint::TopRight,
        ("Left", "Middle") => AttachmentPoint::MiddleLeft,
        ("Center", "Middle") => AttachmentPoint::MiddleCenter,
        ("Right", "Middle") => AttachmentPoint::MiddleRight,
        ("Left", "Bottom") => AttachmentPoint::BottomLeft,
        ("Center", "Bottom") => AttachmentPoint::BottomCenter,
        ("Right", "Bottom") => AttachmentPoint::BottomRight,
        _ => return None,
    })
}

fn drawing_dir_str(d: &DrawingDirection) -> &'static str {
    match d {
        DrawingDirection::LeftToRight => "Left to Right",
        DrawingDirection::TopToBottom => "Top to Bottom",
        DrawingDirection::ByStyle => "By Style",
    }
}

/// Per-visible-character world-space boxes for the MText editor's
/// click-to-select preview. Uses the exact same layout opts as `to_truck`
/// so the boxes line up with the rendered glyphs.
pub fn glyph_boxes(t: &MText, document: &acadrust::CadDocument) -> Vec<GlyphBox> {
    let resolved_style = resolve_text_style(&t.style, document);
    let attach_h_anchor: f32 = match t.attachment_point {
        AttachmentPoint::TopCenter
        | AttachmentPoint::MiddleCenter
        | AttachmentPoint::BottomCenter => 0.5,
        AttachmentPoint::TopRight | AttachmentPoint::MiddleRight | AttachmentPoint::BottomRight => {
            1.0
        }
        _ => 0.0,
    };
    let v_anchor = match t.attachment_point {
        AttachmentPoint::TopLeft | AttachmentPoint::TopCenter | AttachmentPoint::TopRight => {
            MTextVAnchor::Top
        }
        AttachmentPoint::MiddleLeft
        | AttachmentPoint::MiddleCenter
        | AttachmentPoint::MiddleRight => MTextVAnchor::Middle,
        AttachmentPoint::BottomLeft
        | AttachmentPoint::BottomCenter
        | AttachmentPoint::BottomRight => MTextVAnchor::Bottom,
    };
    let rotation = if resolved_style.is_upside_down {
        t.rotation as f32 + std::f32::consts::PI
    } else {
        t.rotation as f32
    };
    let layout = layout_mtext(&MTextRenderOpts {
        value: &t.value,
        insertion: [
            t.insertion_point.x,
            t.insertion_point.y,
            t.insertion_point.z,
        ],
        height: t.height as f32,
        rect_w: t.rectangle_width as f32,
        rotation,
        style: &resolved_style,
        attach_h_anchor,
        v_anchor,
        line_spacing_factor: t.line_spacing_factor as f32,
        vertical_text: matches!(t.drawing_direction, DrawingDirection::TopToBottom),
        want_glyph_boxes: true,
    });
    layout.glyph_boxes
}

fn to_truck(t: &MText, document: &acadrust::CadDocument) -> TruckEntity {
    let resolved_style = resolve_text_style(&t.style, document);
    let attach_h_anchor: f32 = match t.attachment_point {
        AttachmentPoint::TopCenter
        | AttachmentPoint::MiddleCenter
        | AttachmentPoint::BottomCenter => 0.5,
        AttachmentPoint::TopRight | AttachmentPoint::MiddleRight | AttachmentPoint::BottomRight => {
            1.0
        }
        _ => 0.0,
    };
    let v_anchor = match t.attachment_point {
        AttachmentPoint::TopLeft | AttachmentPoint::TopCenter | AttachmentPoint::TopRight => {
            MTextVAnchor::Top
        }
        AttachmentPoint::MiddleLeft
        | AttachmentPoint::MiddleCenter
        | AttachmentPoint::MiddleRight => MTextVAnchor::Middle,
        AttachmentPoint::BottomLeft
        | AttachmentPoint::BottomCenter
        | AttachmentPoint::BottomRight => MTextVAnchor::Bottom,
    };
    let rotation = if resolved_style.is_upside_down {
        t.rotation as f32 + std::f32::consts::PI
    } else {
        t.rotation as f32
    };
    let layout = layout_mtext(&MTextRenderOpts {
        value: &t.value,
        insertion: [
            t.insertion_point.x,
            t.insertion_point.y,
            t.insertion_point.z,
        ],
        height: t.height as f32,
        rect_w: t.rectangle_width as f32,
        rotation,
        style: &resolved_style,
        attach_h_anchor,
        v_anchor,
        line_spacing_factor: t.line_spacing_factor as f32,
        vertical_text: matches!(t.drawing_direction, DrawingDirection::TopToBottom),
        want_glyph_boxes: false,
    });
    let insertion = Vec3::new(
        t.insertion_point.x as f32,
        t.insertion_point.y as f32,
        t.insertion_point.z as f32,
    );
    TruckEntity {
        object: TruckObject::Text(layout.strokes),
        snap_pts: vec![(insertion, SnapHint::Insertion)],
        tangent_geoms: vec![],
        key_vertices: vec![],
        fill_tris: vec![],
    }
}

fn grips(t: &MText) -> Vec<GripDef> {
    let p = glam::DVec3::new(
        t.insertion_point.x,
        t.insertion_point.y,
        t.insertion_point.z,
    );
    let dir = glam::DVec3::new(t.rotation.cos(), t.rotation.sin(), 0.0);
    let width_grip = p + dir * t.rectangle_width.max(0.0);
    vec![square_grip(0, p), triangle_grip(1, width_grip)]
}

fn properties(t: &MText, text_style_names: &[String]) -> PropSection {
    PropSection {
        title: "Geometry".into(),
        props: vec![
            edit("Insert X", "ins_x", t.insertion_point.x),
            edit("Insert Y", "ins_y", t.insertion_point.y),
            edit("Insert Z", "ins_z", t.insertion_point.z),
            edit("Height", "height", t.height),
            edit("Width", "rect_w", t.rectangle_width),
            edit("Rect Height", "rect_h", t.rectangle_height.unwrap_or(0.0)),
            edit("Rotation", "rotation", t.rotation.to_degrees()),
            edit("Line Spacing", "line_spacing", t.line_spacing_factor),
            Property {
                label: "H-Align".into(),
                field: "h_align",
                value: PropValue::Choice {
                    selected: mtext_halign_str(&t.attachment_point).to_string(),
                    options: ["Left", "Center", "Right"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                },
            },
            Property {
                label: "V-Align".into(),
                field: "v_align",
                value: PropValue::Choice {
                    selected: mtext_valign_str(&t.attachment_point).to_string(),
                    options: ["Top", "Middle", "Bottom"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                },
            },
            ro(
                "Attachment",
                "attachment",
                attachment_str(&t.attachment_point).to_string(),
            ),
            ro(
                "Direction",
                "direction",
                drawing_dir_str(&t.drawing_direction).to_string(),
            ),
            Property {
                label: "Content".into(),
                field: "content",
                value: PropValue::EditText(t.value.clone()),
            },
            Property {
                label: "Style".into(),
                field: "style",
                value: PropValue::Choice {
                    selected: if t.style.trim().is_empty() {
                        "Standard".into()
                    } else {
                        t.style.clone()
                    },
                    options: text_style_names.to_vec(),
                },
            },
        ],
    }
}

fn apply_geom_prop(t: &mut MText, field: &str, value: &str) {
    match field {
        "content" => {
            t.value = value.to_string();
            return;
        }
        "style" => {
            t.style = value.to_string();
            return;
        }
        "h_align" => {
            if let Some(next) =
                mtext_attachment_from_align(value, mtext_valign_str(&t.attachment_point))
            {
                t.attachment_point = next;
            }
            return;
        }
        "v_align" => {
            if let Some(next) =
                mtext_attachment_from_align(mtext_halign_str(&t.attachment_point), value)
            {
                t.attachment_point = next;
            }
            return;
        }
        _ => {}
    }
    let Some(v) = crate::entities::common::parse_f64(value) else {
        return;
    };
    match field {
        "ins_x" => t.insertion_point.x = v,
        "ins_y" => t.insertion_point.y = v,
        "ins_z" => t.insertion_point.z = v,
        "height" if v > 0.0 => t.height = v,
        "rect_w" if v > 0.0 => t.rectangle_width = v,
        "rect_h" if v > 0.0 => t.rectangle_height = Some(v),
        "rotation" => t.rotation = v.to_radians(),
        "line_spacing" if v > 0.0 => t.line_spacing_factor = v,
        _ => {}
    }
}

fn apply_grip(t: &mut MText, grip_id: usize, apply: GripApply) {
    match (grip_id, apply) {
        (0, GripApply::Absolute(p)) => {
            t.insertion_point.x = p.x as f64;
            t.insertion_point.y = p.y as f64;
            t.insertion_point.z = p.z as f64;
        }
        (0, GripApply::Translate(d)) => {
            t.insertion_point.x += d.x as f64;
            t.insertion_point.y += d.y as f64;
            t.insertion_point.z += d.z as f64;
        }
        (1, GripApply::Absolute(p)) => {
            let dir_x = t.rotation.cos();
            let dir_y = t.rotation.sin();
            let dx = p.x as f64 - t.insertion_point.x;
            let dy = p.y as f64 - t.insertion_point.y;
            let projected = dx * dir_x + dy * dir_y;
            t.rectangle_width = projected.max(0.01);
        }
        _ => {}
    }
}

fn apply_transform(t: &mut MText, tr: &EntityTransform) {
    crate::scene::view::transform::apply_standard_entity_transform(t, tr, |entity, p1, p2| {
        crate::scene::view::transform::reflect_xy_point(
            &mut entity.insertion_point.x,
            &mut entity.insertion_point.y,
            p1,
            p2,
        );
        let dx = (p2.x - p1.x) as f64;
        let dy = (p2.y - p1.y) as f64;
        let line_angle = dy.atan2(dx);
        entity.rotation = 2.0 * line_angle - entity.rotation;
    });
}

impl TruckConvertible for MText {
    fn to_truck(&self, document: &acadrust::CadDocument) -> Option<TruckEntity> {
        Some(to_truck(self, document))
    }
}

impl Grippable for MText {
    fn grips(&self) -> Vec<GripDef> {
        grips(self)
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        apply_grip(self, grip_id, apply);
    }

    fn grip_menu(&self, grip_id: usize) -> Vec<crate::scene::model::object::GripMenuItem> {
        use crate::scene::model::object::{GripMenuAction, GripMenuItem};
        if grip_id == 0 {
            // Insertion point
            vec![
                GripMenuItem {
                    label: "Stretch",
                    action: GripMenuAction::Stretch,
                },
                GripMenuItem {
                    label: "Move with Text",
                    action: GripMenuAction::MoveWithText,
                },
                GripMenuItem {
                    label: "Rotate",
                    action: GripMenuAction::RotateText,
                },
            ]
        } else {
            // Width grip
            vec![GripMenuItem {
                label: "Stretch",
                action: GripMenuAction::Stretch,
            }]
        }
    }

    fn apply_grip_menu(&mut self, _grip_id: usize, _action: crate::scene::model::object::GripMenuAction) {
        // Rotate needs a follow-up angle handled by
        // `apply_grip_menu_value`; Move-with-Text is the default drag.
    }

    fn grip_menu_value_prompt(
        &self,
        _grip_id: usize,
        action: crate::scene::model::object::GripMenuAction,
    ) -> Option<&'static str> {
        use crate::scene::model::object::GripMenuAction as A;
        match action {
            A::RotateText => Some("Rotation (deg)"),
            _ => None,
        }
    }

    fn apply_grip_menu_value(
        &mut self,
        _grip_id: usize,
        action: crate::scene::model::object::GripMenuAction,
        value: f64,
    ) {
        use crate::scene::model::object::GripMenuAction as A;
        if matches!(action, A::RotateText) {
            self.rotation = value.to_radians();
        }
    }
}

impl PropertyEditable for MText {
    fn geometry_properties(&self, text_style_names: &[String]) -> PropSection {
        properties(self, text_style_names)
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        apply_geom_prop(self, field, value);
    }
}

impl Transformable for MText {
    fn apply_transform(&mut self, t: &EntityTransform) {
        apply_transform(self, t);
    }
}

impl crate::entities::traits::TextContent for acadrust::entities::MText {
    fn text_content(&self) -> Option<String> {
        Some(self.value.clone())
    }
    fn replace_text(&mut self, search: &str, rep: &str) {
        let search_lc = search.to_lowercase();
        if self.value.to_lowercase().contains(&search_lc) {
            self.value = self.value.replace(search, rep);
        }
    }
}
