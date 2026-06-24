use acadrust::entities::{
    Dimension, DimensionAligned, DimensionAngular2Ln, DimensionAngular3Pt, DimensionBase,
    DimensionDiameter, DimensionLinear, DimensionOrdinate, DimensionRadius,
};
use acadrust::Entity;
use glam::{DVec3, Vec3};

use crate::command::EntityTransform;
use crate::entities::common::{
    center_grip, edit_prop as edit, parse_f64, ro_prop as ro, square_grip,
};
use crate::entities::traits::{Grippable, PropertyEditable, Transformable};
use crate::scene::model::object::{GripApply, GripDef, PropSection};

fn base_props(base: &DimensionBase) -> Vec<crate::scene::model::object::Property> {
    vec![
        crate::scene::model::object::Property {
            label: "Text".into(),
            field: "text",
            value: crate::scene::model::object::PropValue::EditText(base.text.clone()),
        },
        crate::scene::model::object::Property {
            label: "User Text".into(),
            field: "user_text",
            value: crate::scene::model::object::PropValue::EditText(
                base.user_text.clone().unwrap_or_default(),
            ),
        },
        crate::scene::model::object::Property {
            label: "Style".into(),
            field: "style_name",
            value: crate::scene::model::object::PropValue::EditText(base.style_name.clone()),
        },
        edit("Text X", "text_x", base.text_middle_point.x),
        edit("Text Y", "text_y", base.text_middle_point.y),
        edit("Text Z", "text_z", base.text_middle_point.z),
        edit("Text Rotation", "text_rotation", base.text_rotation),
        edit(
            "Horizontal Dir",
            "horizontal_direction",
            base.horizontal_direction,
        ),
        edit(
            "Line Spacing",
            "line_spacing_factor",
            base.line_spacing_factor,
        ),
        ro(
            "Measurement",
            "measurement",
            format!("{:.4}", base.actual_measurement),
        ),
    ]
}

fn properties(dim: &Dimension) -> PropSection {
    let mut props = base_props(dim.base());
    match dim {
        Dimension::Aligned(d) => {
            props.extend(linear_like_props(
                d.first_point,
                d.second_point,
                d.definition_point,
            ));
            props.push(edit(
                "Ext Rotation",
                "ext_line_rotation",
                d.ext_line_rotation,
            ));
        }
        Dimension::Linear(d) => {
            props.extend(linear_like_props(
                d.first_point,
                d.second_point,
                d.definition_point,
            ));
            props.push(edit("Rotation", "rotation", d.rotation));
            props.push(edit(
                "Ext Rotation",
                "ext_line_rotation",
                d.ext_line_rotation,
            ));
        }
        Dimension::Radius(d) => {
            props.extend(radius_like_props(d.angle_vertex, d.definition_point));
            props.push(edit("Leader Length", "leader_length", d.leader_length));
        }
        Dimension::Diameter(d) => {
            props.extend(radius_like_props(d.angle_vertex, d.definition_point));
            props.push(edit("Leader Length", "leader_length", d.leader_length));
        }
        Dimension::Angular2Ln(d) => {
            props.extend(angular_props(
                d.angle_vertex,
                d.first_point,
                d.second_point,
                d.definition_point,
            ));
            props.push(edit("Arc X", "dimension_arc_x", d.dimension_arc.x));
            props.push(edit("Arc Y", "dimension_arc_y", d.dimension_arc.y));
            props.push(edit("Arc Z", "dimension_arc_z", d.dimension_arc.z));
        }
        Dimension::Angular3Pt(d) => {
            props.extend(angular_props(
                d.angle_vertex,
                d.first_point,
                d.second_point,
                d.definition_point,
            ));
        }
        Dimension::Ordinate(d) => {
            props.push(edit("Origin X", "definition_x", d.definition_point.x));
            props.push(edit("Origin Y", "definition_y", d.definition_point.y));
            props.push(edit("Origin Z", "definition_z", d.definition_point.z));
            props.push(edit("Feature X", "feature_x", d.feature_location.x));
            props.push(edit("Feature Y", "feature_y", d.feature_location.y));
            props.push(edit("Feature Z", "feature_z", d.feature_location.z));
            props.push(edit("Leader X", "leader_x", d.leader_endpoint.x));
            props.push(edit("Leader Y", "leader_y", d.leader_endpoint.y));
            props.push(edit("Leader Z", "leader_z", d.leader_endpoint.z));
            props.push(ro(
                "Ordinate Type",
                "ordinate_type",
                if d.is_ordinate_type_x { "X" } else { "Y" },
            ));
        }
    }
    PropSection {
        title: "Geometry".into(),
        props,
    }
}

fn linear_like_props(
    first: acadrust::types::Vector3,
    second: acadrust::types::Vector3,
    definition: acadrust::types::Vector3,
) -> Vec<crate::scene::model::object::Property> {
    vec![
        edit("First X", "first_x", first.x),
        edit("First Y", "first_y", first.y),
        edit("First Z", "first_z", first.z),
        edit("Second X", "second_x", second.x),
        edit("Second Y", "second_y", second.y),
        edit("Second Z", "second_z", second.z),
        edit("Definition X", "definition_x", definition.x),
        edit("Definition Y", "definition_y", definition.y),
        edit("Definition Z", "definition_z", definition.z),
    ]
}

fn radius_like_props(
    center: acadrust::types::Vector3,
    point: acadrust::types::Vector3,
) -> Vec<crate::scene::model::object::Property> {
    vec![
        edit("Center X", "center_x", center.x),
        edit("Center Y", "center_y", center.y),
        edit("Center Z", "center_z", center.z),
        edit("Point X", "point_x", point.x),
        edit("Point Y", "point_y", point.y),
        edit("Point Z", "point_z", point.z),
    ]
}

fn angular_props(
    vertex: acadrust::types::Vector3,
    first: acadrust::types::Vector3,
    second: acadrust::types::Vector3,
    definition: acadrust::types::Vector3,
) -> Vec<crate::scene::model::object::Property> {
    vec![
        edit("Vertex X", "vertex_x", vertex.x),
        edit("Vertex Y", "vertex_y", vertex.y),
        edit("Vertex Z", "vertex_z", vertex.z),
        edit("First X", "first_x", first.x),
        edit("First Y", "first_y", first.y),
        edit("First Z", "first_z", first.z),
        edit("Second X", "second_x", second.x),
        edit("Second Y", "second_y", second.y),
        edit("Second Z", "second_z", second.z),
        edit("Definition X", "definition_x", definition.x),
        edit("Definition Y", "definition_y", definition.y),
        edit("Definition Z", "definition_z", definition.z),
    ]
}

fn apply_base_prop(base: &mut DimensionBase, field: &str, value: &str) -> bool {
    match field {
        "text" => {
            base.text = value.to_string();
            true
        }
        "user_text" => {
            base.user_text = if value.trim().is_empty() {
                None
            } else {
                Some(value.to_string())
            };
            true
        }
        "style_name" => {
            base.style_name = value.to_string();
            true
        }
        // Editing the text position in the properties panel pins it to a
        // user-defined location (stops following DIMTAD). See #94.
        "text_x" => {
            base.text_user_positioned = true;
            assign_f64(value, &mut base.text_middle_point.x)
        }
        "text_y" => {
            base.text_user_positioned = true;
            assign_f64(value, &mut base.text_middle_point.y)
        }
        "text_z" => {
            base.text_user_positioned = true;
            assign_f64(value, &mut base.text_middle_point.z)
        }
        "text_rotation" => assign_f64(value, &mut base.text_rotation),
        "horizontal_direction" => assign_f64(value, &mut base.horizontal_direction),
        "line_spacing_factor" => assign_f64(value, &mut base.line_spacing_factor),
        _ => false,
    }
}

fn assign_f64(value: &str, target: &mut f64) -> bool {
    let Some(v) = parse_f64(value) else {
        return false;
    };
    *target = v;
    true
}

fn apply_geom_prop(dim: &mut Dimension, field: &str, value: &str) {
    if apply_base_prop(dim.base_mut(), field, value) {
        return;
    }
    match dim {
        Dimension::Aligned(d) => apply_linear_fields_aligned(d, field, value),
        Dimension::Linear(d) => apply_linear_fields_linear(d, field, value),
        Dimension::Radius(d) => apply_radius_fields(d, field, value),
        Dimension::Diameter(d) => apply_diameter_fields(d, field, value),
        Dimension::Angular2Ln(d) => apply_angular2_fields(d, field, value),
        Dimension::Angular3Pt(d) => apply_angular3_fields(d, field, value),
        Dimension::Ordinate(d) => apply_ordinate_fields(d, field, value),
    }
    dim.base_mut().actual_measurement = dim.measurement();
}

fn apply_linear_fields_aligned(d: &mut DimensionAligned, field: &str, value: &str) {
    apply_linear_common(
        &mut d.first_point,
        &mut d.second_point,
        &mut d.definition_point,
        field,
        value,
    );
    let _ = assign_f64(value, &mut d.ext_line_rotation);
}

fn apply_linear_fields_linear(d: &mut DimensionLinear, field: &str, value: &str) {
    apply_linear_common(
        &mut d.first_point,
        &mut d.second_point,
        &mut d.definition_point,
        field,
        value,
    );
    match field {
        "rotation" => {
            let _ = assign_f64(value, &mut d.rotation);
        }
        "ext_line_rotation" => {
            let _ = assign_f64(value, &mut d.ext_line_rotation);
        }
        _ => {}
    }
}

fn apply_linear_common(
    first: &mut acadrust::types::Vector3,
    second: &mut acadrust::types::Vector3,
    definition: &mut acadrust::types::Vector3,
    field: &str,
    value: &str,
) {
    match field {
        "first_x" => {
            let _ = assign_f64(value, &mut first.x);
        }
        "first_y" => {
            let _ = assign_f64(value, &mut first.y);
        }
        "first_z" => {
            let _ = assign_f64(value, &mut first.z);
        }
        "second_x" => {
            let _ = assign_f64(value, &mut second.x);
        }
        "second_y" => {
            let _ = assign_f64(value, &mut second.y);
        }
        "second_z" => {
            let _ = assign_f64(value, &mut second.z);
        }
        "definition_x" => {
            let _ = assign_f64(value, &mut definition.x);
        }
        "definition_y" => {
            let _ = assign_f64(value, &mut definition.y);
        }
        "definition_z" => {
            let _ = assign_f64(value, &mut definition.z);
        }
        _ => {}
    }
}

fn apply_radius_fields(d: &mut DimensionRadius, field: &str, value: &str) {
    apply_radius_common(&mut d.angle_vertex, &mut d.definition_point, field, value);
    if field == "leader_length" {
        let _ = assign_f64(value, &mut d.leader_length);
    }
}

fn apply_diameter_fields(d: &mut DimensionDiameter, field: &str, value: &str) {
    apply_radius_common(&mut d.angle_vertex, &mut d.definition_point, field, value);
    if field == "leader_length" {
        let _ = assign_f64(value, &mut d.leader_length);
    }
}

fn apply_radius_common(
    center: &mut acadrust::types::Vector3,
    point: &mut acadrust::types::Vector3,
    field: &str,
    value: &str,
) {
    match field {
        "center_x" => {
            let _ = assign_f64(value, &mut center.x);
        }
        "center_y" => {
            let _ = assign_f64(value, &mut center.y);
        }
        "center_z" => {
            let _ = assign_f64(value, &mut center.z);
        }
        "point_x" => {
            let _ = assign_f64(value, &mut point.x);
        }
        "point_y" => {
            let _ = assign_f64(value, &mut point.y);
        }
        "point_z" => {
            let _ = assign_f64(value, &mut point.z);
        }
        _ => {}
    }
}

fn apply_angular2_fields(d: &mut DimensionAngular2Ln, field: &str, value: &str) {
    apply_angular_common(
        &mut d.angle_vertex,
        &mut d.first_point,
        &mut d.second_point,
        &mut d.definition_point,
        field,
        value,
    );
    match field {
        "dimension_arc_x" => {
            let _ = assign_f64(value, &mut d.dimension_arc.x);
        }
        "dimension_arc_y" => {
            let _ = assign_f64(value, &mut d.dimension_arc.y);
        }
        "dimension_arc_z" => {
            let _ = assign_f64(value, &mut d.dimension_arc.z);
        }
        _ => {}
    }
}

fn apply_angular3_fields(d: &mut DimensionAngular3Pt, field: &str, value: &str) {
    apply_angular_common(
        &mut d.angle_vertex,
        &mut d.first_point,
        &mut d.second_point,
        &mut d.definition_point,
        field,
        value,
    );
}

fn apply_angular_common(
    vertex: &mut acadrust::types::Vector3,
    first: &mut acadrust::types::Vector3,
    second: &mut acadrust::types::Vector3,
    definition: &mut acadrust::types::Vector3,
    field: &str,
    value: &str,
) {
    match field {
        "vertex_x" => {
            let _ = assign_f64(value, &mut vertex.x);
        }
        "vertex_y" => {
            let _ = assign_f64(value, &mut vertex.y);
        }
        "vertex_z" => {
            let _ = assign_f64(value, &mut vertex.z);
        }
        "first_x" => {
            let _ = assign_f64(value, &mut first.x);
        }
        "first_y" => {
            let _ = assign_f64(value, &mut first.y);
        }
        "first_z" => {
            let _ = assign_f64(value, &mut first.z);
        }
        "second_x" => {
            let _ = assign_f64(value, &mut second.x);
        }
        "second_y" => {
            let _ = assign_f64(value, &mut second.y);
        }
        "second_z" => {
            let _ = assign_f64(value, &mut second.z);
        }
        "definition_x" => {
            let _ = assign_f64(value, &mut definition.x);
        }
        "definition_y" => {
            let _ = assign_f64(value, &mut definition.y);
        }
        "definition_z" => {
            let _ = assign_f64(value, &mut definition.z);
        }
        _ => {}
    }
}

fn apply_ordinate_fields(d: &mut DimensionOrdinate, field: &str, value: &str) {
    match field {
        "definition_x" => {
            let _ = assign_f64(value, &mut d.definition_point.x);
        }
        "definition_y" => {
            let _ = assign_f64(value, &mut d.definition_point.y);
        }
        "definition_z" => {
            let _ = assign_f64(value, &mut d.definition_point.z);
        }
        "feature_x" => {
            let _ = assign_f64(value, &mut d.feature_location.x);
        }
        "feature_y" => {
            let _ = assign_f64(value, &mut d.feature_location.y);
        }
        "feature_z" => {
            let _ = assign_f64(value, &mut d.feature_location.z);
        }
        "leader_x" => {
            let _ = assign_f64(value, &mut d.leader_endpoint.x);
        }
        "leader_y" => {
            let _ = assign_f64(value, &mut d.leader_endpoint.y);
        }
        "leader_z" => {
            let _ = assign_f64(value, &mut d.leader_endpoint.z);
        }
        _ => {}
    }
}

fn apply_transform(dim: &mut Dimension, t: &EntityTransform) {
    match t {
        EntityTransform::Translate(d) => dim.translate(acadrust::types::Vector3::new(
            d.x as f64, d.y as f64, d.z as f64,
        )),
        EntityTransform::Rotate { center, angle_rad } => {
            transform_dimension_points(dim, |pt| rotate_point(pt, *center, *angle_rad))
        }
        EntityTransform::Scale { center, factor } => {
            transform_dimension_points(dim, |pt| scale_point(pt, *center, *factor))
        }
        EntityTransform::Mirror { p1, p2 } => {
            transform_dimension_points(dim, |pt| mirror_point(pt, *p1, *p2))
        }
    }
    dim.base_mut().actual_measurement = dim.measurement();
}

fn transform_dimension_points<F>(dim: &mut Dimension, mut f: F)
where
    F: FnMut(&mut acadrust::types::Vector3),
{
    f(&mut dim.base_mut().text_middle_point);
    f(&mut dim.base_mut().insertion_point);
    match dim {
        Dimension::Aligned(d) => {
            f(&mut d.first_point);
            f(&mut d.second_point);
            f(&mut d.definition_point);
        }
        Dimension::Linear(d) => {
            f(&mut d.first_point);
            f(&mut d.second_point);
            f(&mut d.definition_point);
        }
        Dimension::Radius(d) => {
            f(&mut d.angle_vertex);
            f(&mut d.definition_point);
        }
        Dimension::Diameter(d) => {
            f(&mut d.angle_vertex);
            f(&mut d.definition_point);
        }
        Dimension::Angular2Ln(d) => {
            f(&mut d.dimension_arc);
            f(&mut d.first_point);
            f(&mut d.second_point);
            f(&mut d.angle_vertex);
            f(&mut d.definition_point);
        }
        Dimension::Angular3Pt(d) => {
            f(&mut d.first_point);
            f(&mut d.second_point);
            f(&mut d.angle_vertex);
            f(&mut d.definition_point);
        }
        Dimension::Ordinate(d) => {
            f(&mut d.definition_point);
            f(&mut d.feature_location);
            f(&mut d.leader_endpoint);
        }
    }
}

fn rotate_point(p: &mut acadrust::types::Vector3, center: DVec3, angle_rad: f32) {
    let dx = p.x - center.x;
    let dy = p.y - center.y;
    let (s, c) = (angle_rad as f64).sin_cos();
    p.x = center.x + dx * c - dy * s;
    p.y = center.y + dx * s + dy * c;
}

fn scale_point(p: &mut acadrust::types::Vector3, center: DVec3, factor: f32) {
    let f = factor as f64;
    p.x = center.x + (p.x - center.x) * f;
    p.y = center.y + (p.y - center.y) * f;
    p.z = center.z + (p.z - center.z) * f;
}

fn mirror_point(p: &mut acadrust::types::Vector3, p1: DVec3, p2: DVec3) {
    crate::scene::view::transform::reflect_xy_point(&mut p.x, &mut p.y, p1, p2);
}

impl PropertyEditable for Dimension {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        properties(self)
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        apply_geom_prop(self, field, value);
    }
}

impl Transformable for Dimension {
    fn apply_transform(&mut self, t: &EntityTransform) {
        apply_transform(self, t);
    }
}

// ── Grippable ─────────────────────────────────────────────────────────────────

/// f64 variant for grip positions — grips must not round UTM-scale
/// coordinates through f32 before the world-offset subtraction.
fn dv3(v: &acadrust::types::Vector3) -> glam::DVec3 {
    glam::DVec3::new(v.x, v.y, v.z)
}

fn set_v3(target: &mut acadrust::types::Vector3, p: DVec3) {
    target.x = p.x;
    target.y = p.y;
    target.z = p.z;
}

fn translate_v3(target: &mut acadrust::types::Vector3, d: DVec3) {
    target.x += d.x;
    target.y += d.y;
    target.z += d.z;
}

fn apply_to_v3(target: &mut acadrust::types::Vector3, apply: &GripApply) {
    match apply {
        GripApply::Absolute(p) => set_v3(target, *p),
        GripApply::Translate(d) => translate_v3(target, *d),
    }
}

impl Grippable for Dimension {
    fn grips(&self) -> Vec<GripDef> {
        let text = dv3(&self.base().text_middle_point);
        match self {
            Dimension::Linear(d) => vec![
                square_grip(0, dv3(&d.first_point)),
                center_grip(1, dv3(&d.second_point)),
                center_grip(2, dv3(&d.definition_point)),
                center_grip(3, text),
            ],
            Dimension::Aligned(d) => vec![
                square_grip(0, dv3(&d.first_point)),
                center_grip(1, dv3(&d.second_point)),
                center_grip(2, dv3(&d.definition_point)),
                center_grip(3, text),
            ],
            Dimension::Radius(d) => vec![
                square_grip(0, dv3(&d.angle_vertex)),
                center_grip(1, dv3(&d.definition_point)),
                center_grip(2, text),
            ],
            Dimension::Diameter(d) => vec![
                square_grip(0, dv3(&d.angle_vertex)),
                center_grip(1, dv3(&d.definition_point)),
                center_grip(2, text),
            ],
            Dimension::Angular2Ln(d) => vec![
                square_grip(0, dv3(&d.angle_vertex)),
                center_grip(1, dv3(&d.first_point)),
                center_grip(2, dv3(&d.second_point)),
                center_grip(3, dv3(&d.definition_point)),
                center_grip(4, text),
            ],
            Dimension::Angular3Pt(d) => vec![
                square_grip(0, dv3(&d.angle_vertex)),
                center_grip(1, dv3(&d.first_point)),
                center_grip(2, dv3(&d.second_point)),
                center_grip(3, dv3(&d.definition_point)),
                center_grip(4, text),
            ],
            Dimension::Ordinate(d) => vec![
                square_grip(0, dv3(&d.definition_point)),
                center_grip(1, dv3(&d.feature_location)),
                center_grip(2, dv3(&d.leader_endpoint)),
                center_grip(3, text),
            ],
        }
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        // Last grip always moves the text.
        let text_grip = match self {
            Dimension::Linear(_) | Dimension::Aligned(_) => 3,
            Dimension::Radius(_) | Dimension::Diameter(_) => 2,
            Dimension::Angular2Ln(_) | Dimension::Angular3Pt(_) => 4,
            Dimension::Ordinate(_) => 3,
        };
        if grip_id == text_grip {
            apply_to_v3(&mut self.base_mut().text_middle_point, &apply);
            // Dragging the text grip pins it to a user-defined location, so it
            // no longer follows the style (DIMTAD). See #94.
            self.base_mut().text_user_positioned = true;
            return;
        }

        match self {
            Dimension::Linear(d) => match grip_id {
                0 => apply_to_v3(&mut d.first_point, &apply),
                1 => apply_to_v3(&mut d.second_point, &apply),
                2 => apply_to_v3(&mut d.definition_point, &apply),
                _ => {}
            },
            Dimension::Aligned(d) => match grip_id {
                0 => apply_to_v3(&mut d.first_point, &apply),
                1 => apply_to_v3(&mut d.second_point, &apply),
                2 => apply_to_v3(&mut d.definition_point, &apply),
                _ => {}
            },
            Dimension::Radius(d) => match grip_id {
                0 => apply_to_v3(&mut d.angle_vertex, &apply),
                1 => apply_to_v3(&mut d.definition_point, &apply),
                _ => {}
            },
            Dimension::Diameter(d) => match grip_id {
                0 => apply_to_v3(&mut d.angle_vertex, &apply),
                1 => apply_to_v3(&mut d.definition_point, &apply),
                _ => {}
            },
            Dimension::Angular2Ln(d) => match grip_id {
                0 => apply_to_v3(&mut d.angle_vertex, &apply),
                1 => apply_to_v3(&mut d.first_point, &apply),
                2 => apply_to_v3(&mut d.second_point, &apply),
                3 => apply_to_v3(&mut d.definition_point, &apply),
                _ => {}
            },
            Dimension::Angular3Pt(d) => match grip_id {
                0 => apply_to_v3(&mut d.angle_vertex, &apply),
                1 => apply_to_v3(&mut d.first_point, &apply),
                2 => apply_to_v3(&mut d.second_point, &apply),
                3 => apply_to_v3(&mut d.definition_point, &apply),
                _ => {}
            },
            Dimension::Ordinate(d) => match grip_id {
                0 => apply_to_v3(&mut d.definition_point, &apply),
                1 => apply_to_v3(&mut d.feature_location, &apply),
                2 => apply_to_v3(&mut d.leader_endpoint, &apply),
                _ => {}
            },
        }
        self.base_mut().actual_measurement = self.measurement();
    }

    fn grip_menu(&self, grip_id: usize) -> Vec<crate::scene::model::object::GripMenuItem> {
        use crate::scene::model::object::{GripMenuAction, GripMenuItem};
        let (dim_line_grip, text_grip) = match self {
            Dimension::Linear(_) | Dimension::Aligned(_) => (2, 3),
            Dimension::Radius(_) | Dimension::Diameter(_) => (1, 2),
            Dimension::Angular2Ln(_) | Dimension::Angular3Pt(_) => (3, 4),
            Dimension::Ordinate(_) => (0, 3),
        };
        if grip_id == text_grip {
            vec![
                GripMenuItem {
                    label: "Stretch",
                    action: GripMenuAction::Stretch,
                },
                GripMenuItem {
                    label: "Move with Dim Line",
                    action: GripMenuAction::MoveWithDimLine,
                },
                GripMenuItem {
                    label: "Move with Leader",
                    action: GripMenuAction::MoveWithLeader,
                },
                GripMenuItem {
                    label: "Move Independent",
                    action: GripMenuAction::MoveIndependent,
                },
                GripMenuItem {
                    label: "Reset Text",
                    action: GripMenuAction::ResetText,
                },
                GripMenuItem {
                    label: "Rotate Text",
                    action: GripMenuAction::RotateText,
                },
                GripMenuItem {
                    label: "Above Dim Line",
                    action: GripMenuAction::AboveDimLine,
                },
                GripMenuItem {
                    label: "Center",
                    action: GripMenuAction::Center,
                },
            ]
        } else if grip_id == dim_line_grip {
            vec![
                GripMenuItem {
                    label: "Stretch",
                    action: GripMenuAction::Stretch,
                },
                GripMenuItem {
                    label: "Reverse Arrows",
                    action: GripMenuAction::ReverseArrows,
                },
            ]
        } else {
            vec![GripMenuItem {
                label: "Stretch",
                action: GripMenuAction::Stretch,
            }]
        }
    }

    fn apply_grip_menu(&mut self, grip_id: usize, action: crate::scene::model::object::GripMenuAction) {
        use crate::scene::model::object::GripMenuAction as A;
        let (_dim_line_grip, text_grip) = match self {
            Dimension::Linear(_) | Dimension::Aligned(_) => (2, 3),
            Dimension::Radius(_) | Dimension::Diameter(_) => (1, 2),
            Dimension::Angular2Ln(_) | Dimension::Angular3Pt(_) => (3, 4),
            Dimension::Ordinate(_) => (0, 3),
        };
        match action {
            A::ResetText if grip_id == text_grip => {
                // Drop any text-position override — leave it to the
                // renderer to recompute from the dim style.
                let b = self.base_mut();
                b.text_middle_point.x = 0.0;
                b.text_middle_point.y = 0.0;
                b.text_middle_point.z = 0.0;
            }
            A::Center if grip_id == text_grip => {
                // Snap text to the centre of the dimension line.
                // Approximate as midpoint of first/second extension
                // origins for Linear / Aligned dimensions.
                match self {
                    Dimension::Linear(d) => {
                        let mx = (d.first_point.x + d.second_point.x) * 0.5;
                        let my = (d.first_point.y + d.second_point.y) * 0.5;
                        d.base.text_middle_point.x = mx;
                        d.base.text_middle_point.y = my;
                    }
                    Dimension::Aligned(d) => {
                        let mx = (d.first_point.x + d.second_point.x) * 0.5;
                        let my = (d.first_point.y + d.second_point.y) * 0.5;
                        d.base.text_middle_point.x = mx;
                        d.base.text_middle_point.y = my;
                    }
                    _ => {}
                }
            }
            // Stretch / Move-variants / Reverse Arrows / Rotate Text /
            // Above Dim Line need either a follow-up drag or a numeric
            // prompt — wired to default Stretch behaviour for now.
            _ => {}
        }
    }
}

// ── Tessellation ─────────────────────────────────────────────────────────
//
// Per-entity tessellation entry for `Dimension`. The trait + impl live in
// this file so all dimension tess code stays alongside the entity
// definition. Shared dim machinery (`ArrowKind`, `DimGeom`, `append_arrow`,
// arrow blocks, colour resolution, `add_segment` / `add_polyline`,
// `normalized_or`, `entity_z`, `offset_snap_pts`) lives in
// `scene::convert::tessellate` and is reused by Leader / MultiLeader too.

use acadrust::entities::{MText, Text};
use acadrust::tables::DimStyle;
use acadrust::types::{Color as AcadColor, Vector3};
use acadrust::{CadDocument, EntityType, Handle};

use crate::scene::convert::tess_util::aci_to_rgba;
use crate::scene::convert::tessellate::{
    add_polyline, add_segment, append_arrow, arrow_from_block, normalized_or, ArrowKind, DimGeom,
};
use crate::scene::model::wire_model::{SnapHint, WireModel};

pub trait DimensionTess {
    fn tessellate(
        &self,
        document: &CadDocument,
        handle: Handle,
        selected: bool,
        entity_color: [f32; 4],
        line_weight_px: f32,
        anno_scale: f32,
        selected_set: &rustc_hash::FxHashSet<acadrust::Handle>,
        active_viewport: Option<acadrust::Handle>,
        bg_color: [f32; 4],
        view_aabb: Option<[f32; 4]>,
        world_per_pixel: Option<f32>,
    ) -> Vec<WireModel>;
}

impl DimensionTess for Dimension {
    fn tessellate(
        &self,
        document: &CadDocument,
        handle: Handle,
        selected: bool,
        entity_color: [f32; 4],
        line_weight_px: f32,
        anno_scale: f32,
        selected_set: &rustc_hash::FxHashSet<acadrust::Handle>,
        active_viewport: Option<acadrust::Handle>,
        bg_color: [f32; 4],
        view_aabb: Option<[f32; 4]>,
        world_per_pixel: Option<f32>,
    ) -> Vec<WireModel> {
        tessellate_dimension_inner(
            document,
            handle,
            self,
            selected,
            entity_color,
            line_weight_px,
            anno_scale,
            selected_set,
            active_viewport,
            bg_color,
            view_aabb,
            world_per_pixel,
        )
    }
}

fn tessellate_dimension_inner(
    document: &CadDocument,
    handle: Handle,
    dim: &Dimension,
    selected: bool,
    entity_color: [f32; 4],
    line_weight_px: f32,
    anno_scale: f32,
    // LOD hints — when present, synthesised dim text routes through the
    // top-level LOD ladder (baseline / greek / full) instead of the truck
    // path so far-out drawings collapse to a colored rect or baseline.
    selected_set: &rustc_hash::FxHashSet<acadrust::Handle>,
    active_viewport: Option<acadrust::Handle>,
    bg_color: [f32; 4],
    view_aabb: Option<[f32; 4]>,
    world_per_pixel: Option<f32>,
) -> Vec<WireModel> {
    let name = handle.value().to_string();
    // (Baked-block fast path moved up into scene::tessellate_entity so the
    // recursive call goes through the LOD ladder, not the truck path.)

    let style_name = &dim.base().style_name;
    let style = document.dim_styles.iter().find(|s| {
        s.name.eq_ignore_ascii_case(style_name)
            || (style_name.trim().is_empty() && s.name.eq_ignore_ascii_case("Standard"))
    });

    // DIMSCALE rule:
    //   dimstyle.dimscale > 0  →  final multiplier; ignore anno_scale.
    //   dimstyle.dimscale == 0 →  annotative: use anno_scale (= 1/vp_scale).
    let dim_scale = style
        .map(|s| {
            if s.dimscale > 1e-6 {
                s.dimscale
            } else {
                anno_scale as f64
            }
        })
        .unwrap_or(1.0);

    let (
        dimasz_raw,
        dimexo,
        dimexe,
        dim_txt,
        dimtsz_raw,
        dimsah,
        dimse1,
        dimse2,
        dimsd1,
        dimsd2,
        dimdle,
        dimfxl,
        dimfxlon,
        dimsoxd,
        dimcen,
    ) = style
        .map(|s| {
            (
                s.dimasz * dim_scale,
                (s.dimexo * dim_scale) as f32,
                (s.dimexe * dim_scale) as f32,
                s.dimtxt * dim_scale,
                s.dimtsz * dim_scale,
                s.dimsah,
                s.dimse1,
                s.dimse2,
                s.dimsd1,
                s.dimsd2,
                (s.dimdle * dim_scale) as f32,
                (s.dimfxl * dim_scale) as f32,
                s.dimfxlon,
                s.dimsoxd,
                (s.dimcen * dim_scale) as f32,
            )
        })
        .unwrap_or((
            0.18, 0.0, 0.0, 2.5, 0.0, false, false, false, false, false, 0.0, 1.0, false, false,
            0.09,
        ));

    // Arrow selection precedence:
    //   1. DIMTSZ>0 → oblique tick (overrides DIMBLK*).
    //   2. DIMSAH false → DIMBLK on both ends.
    //   3. DIMSAH true  → DIMBLK1 (first end), DIMBLK2 (second end).
    // Unknown / NULL block handles fall back to ClosedFilled.
    let dimasz = (dimasz_raw as f32).max(0.001);
    let (arrow1, arrow2) = if dimtsz_raw > 1e-9 {
        let t = ArrowKind::Tick {
            size: (dimtsz_raw as f32).max(0.001),
        };
        (t, t)
    } else if let Some(s) = style {
        if dimsah {
            (
                arrow_from_block(document, s.dimblk1, dimasz),
                arrow_from_block(document, s.dimblk2, dimasz),
            )
        } else {
            let a = arrow_from_block(document, s.dimblk, dimasz);
            (a, a)
        }
    } else {
        let a = ArrowKind::Triangle {
            size: dimasz,
            filled: true,
            size_mul: 1.0,
        };
        (a, a)
    };

    // Text box (local space) so the dim line can be broken where the text
    // crosses it — lets a DIMTFILL background sit over the line. The renderer
    // draws 2D fills under all wires, so the line is gapped rather than masked.
    let dimgap_local = style.map(|s| (s.dimgap * dim_scale) as f32).unwrap_or(0.09);
    let text_break = {
        let tp = vec3_local(
            dimension_text_pos_f64(dim, style, dim_txt, dim_scale),
        );
        let tw = dimension_text_value(dim, style)
            .map(|t| t.chars().count() as f32 * dim_txt as f32 * 0.6)
            .unwrap_or(0.0);
        if tw > 0.0 {
            // Vertical threshold is the bare text half-height (no DIMGAP): the
            // line only breaks when it actually passes under the glyphs. Text
            // placed above/below the line (DIMTAD 1/4) sits exactly
            // `text_half + DIMGAP` away, so excluding the gap here keeps it
            // strictly outside and the line continuous — otherwise the two
            // terms cancel at the same scaled value and the gap flickers with
            // DIMGAP/DIMSCALE. The horizontal half-width keeps DIMGAP so a
            // genuine break still clears the text comfortably. (#94)
            Some((tp, tw * 0.5 + dimgap_local, dim_txt as f32 * 0.5))
        } else {
            None
        }
    };

    let mut geom = dimension_geometry(
        dim,
        &arrow1,
        &arrow2,
        DimLineParams {
            dimexo,
            dimexe,
            dimdle,
            dimfxl,
            dimfxlon,
            dimsoxd,
            dimcen,
            ticks: dimtsz_raw > 1e-9,
            arrow_len: dimasz,
            text_break,
        },
        SuppressFlags {
            ext1: dimse1,
            ext2: dimse2,
            dim1: dimsd1,
            dim2: dimsd2,
        },
    );

    // DIMTMOVE = 1: when the saved text_middle_point sits far from the
    // dim-line anchor, draw a short leader connecting them. (=0 anchors text
    // to the dim line — no leader; =2 frees text without a leader.)
    if let Some(s) = style {
        if s.dimtmove == 1 {
            if let Some((anchor, txt)) = dimtmove_leader_endpoints(dim) {
                let gap = dim_txt as f32 * 0.5;
                if (txt - anchor).length() > gap * 2.0 {
                    add_segment(&mut geom.dim_lines, anchor, txt);
                }
            }
        }
        // Fit: text that doesn't fit between the extension lines is slid outside
        // in the text-placement pass (unless DIMTIX), and arrowheads that don't
        // fit are flipped outside in append_linear_dimension. DIMTOFL (force the
        // dim line inside) is already satisfied for linear/aligned, whose dim
        // line is always drawn between the extension points. DIMATFIT's exact
        // mode ordering (arrows-first vs text-first) and DIMUPT (reposition on
        // create) aren't differentiated yet — read here for round-trip.
        let _ = (s.dimtofl, s.dimatfit, s.dimupt);
        // DIMTXTDIRECTION (RTL) needs per-instance text mirroring on the Text
        // entity, which the current text struct can't carry. Tracked: read
        // and ignore so the file round-trips on save.
        let _ = s.dimtxtdirection;
        // DIMARCSYM only applies to arc-length dims; DIMJOGANG only to
        // jogged-radius dims. We don't ship those Dimension variants yet,
        // so the values are read for round-trip but not drawn.
        let _ = (s.dimarcsym, s.dimjogang);
        // DIMUNIT is the obsolete pre-R2000 linear unit format; DIMLUNIT
        // supersedes it. Read but not honoured.
        let _ = s.dimunit;
    }
    // Dimension entity fields that the render path doesn't yet use but are
    // preserved on save:
    //   - base.insertion_point: legacy anchor reference; render uses
    //     text_middle_point + dim-line geometry instead.
    //   - base.block_name: AutoCAD-style "*D..." anonymous block name for
    //     the dim graphics — we re-tessellate so don't need it.
    //   - base.version: DXF format marker (metadata only).
    let _ = (
        dim.base().insertion_point,
        &dim.base().block_name,
        dim.base().version,
    );

    // Per-spec colours: DIMCLRD (dim/arrows), DIMCLRE (ext), DIMCLRT (text).
    // 0=ByBlock and 256=ByLayer fall through to entity_color.
    let dim_color = if selected {
        WireModel::SELECTED
    } else {
        resolve_dim_color(style.map(|s| s.dimclrd).unwrap_or(0), entity_color)
    };
    let ext_color = if selected {
        WireModel::SELECTED
    } else {
        resolve_dim_color(style.map(|s| s.dimclre).unwrap_or(0), entity_color)
    };
    let text_color = if selected {
        entity_color // text wire color set by inner tessellate; keep entity tint
    } else {
        resolve_dim_color(style.map(|s| s.dimclrt).unwrap_or(0), entity_color)
    };

    let snap_pts = dimension_snap_pts(dim);
    let key_vertices: Vec<[f64; 3]> = geom
        .dim_lines
        .iter()
        .chain(geom.ext_lines.iter())
        .copied()
        .filter(|p| !(p[0].is_nan() || p[1].is_nan() || p[2].is_nan()))
        .map(|[x, y, z]| [x as f64, y as f64, z as f64])
        .collect();

    // DIMLWD (dim line + arrows) and DIMLWE (extension lines). Negative
    // codes fall through to the entity's own resolved weight.
    let lw_dim = resolve_dim_lineweight_px(style.map(|s| s.dimlwd).unwrap_or(-2), line_weight_px);
    let lw_ext = resolve_dim_lineweight_px(style.map(|s| s.dimlwe).unwrap_or(-2), line_weight_px);

    // DIMLTEX (dim line) / DIMLTEX1 (ext1) / DIMLTEX2 (ext2) — linetype
    // handles → pattern. Looked up in document.line_types by handle.
    let lt_scale = document.header.linetype_scale as f32 * dim.base().common.linetype_scale as f32;
    let (dim_pat_len, dim_pat) = style
        .map(|s| resolve_pattern_by_handle(document, s.dimltex_handle, lt_scale))
        .unwrap_or((0.0, [0.0; 8]));
    let (ext1_pat_len, ext1_pat) = style
        .map(|s| resolve_pattern_by_handle(document, s.dimltex1_handle, lt_scale))
        .unwrap_or((0.0, [0.0; 8]));
    let (ext2_pat_len, ext2_pat) = style
        .map(|s| resolve_pattern_by_handle(document, s.dimltex2_handle, lt_scale))
        .unwrap_or((0.0, [0.0; 8]));

    let mut wires = Vec::new();

    if !geom.ext_lines.is_empty() {
        // If ext1 and ext2 have different linetypes, split into two wires so
        // each can carry its own pattern. Otherwise emit as a single wire.
        let split = ext1_pat_len != ext2_pat_len || ext1_pat != ext2_pat;
        if split {
            let (ext1, ext2) = split_ext_lines(&geom.ext_lines);
            if !ext1.is_empty() {
                wires.push(WireModel {
                    name: name.clone(),
                    points: ext1,
                    points_low: Vec::new(),
                    color: ext_color,
                    selected,
                    aci: 0,
                    pattern_length: ext1_pat_len,
                    pattern: ext1_pat,
                    line_weight_px: lw_ext,
                    snap_pts: vec![],
                    tangent_geoms: vec![],
                    key_vertices: vec![],
                    aabb: WireModel::UNBOUNDED_AABB,
                    plinegen: true,
                    vp_scissor: None,
                    fill_tris: vec![],
                    fill_tris_low: Vec::new(),
                });
            }
            if !ext2.is_empty() {
                wires.push(WireModel {
                    name: name.clone(),
                    points: ext2,
                    points_low: Vec::new(),
                    color: ext_color,
                    selected,
                    aci: 0,
                    pattern_length: ext2_pat_len,
                    pattern: ext2_pat,
                    line_weight_px: lw_ext,
                    snap_pts: vec![],
                    tangent_geoms: vec![],
                    key_vertices: vec![],
                    aabb: WireModel::UNBOUNDED_AABB,
                    plinegen: true,
                    vp_scissor: None,
                    fill_tris: vec![],
                    fill_tris_low: Vec::new(),
                });
            }
        } else {
            wires.push(WireModel {
                name: name.clone(),
                points: geom.ext_lines,
                points_low: Vec::new(),
                color: ext_color,
                selected,
                aci: 0,
                pattern_length: ext1_pat_len,
                pattern: ext1_pat,
                line_weight_px: lw_ext,
                snap_pts: vec![],
                tangent_geoms: vec![],
                key_vertices: vec![],
                aabb: WireModel::UNBOUNDED_AABB,
                plinegen: true,
                vp_scissor: None,
                fill_tris: vec![],
                fill_tris_low: Vec::new(),
            });
        }
    }

    wires.push(WireModel {
        name: name.clone(),
        points: geom.dim_lines,
        points_low: Vec::new(),
        color: dim_color,
        selected,
        aci: 0,
        pattern_length: dim_pat_len,
        pattern: dim_pat,
        line_weight_px: lw_dim,
        snap_pts,
        tangent_geoms: vec![],
        key_vertices,
        aabb: WireModel::UNBOUNDED_AABB,
        plinegen: true,
        vp_scissor: None,
        fill_tris: geom.arrow_fill,
        fill_tris_low: Vec::new(),
    });

    // DIMTFILL: 0=none, 1=drawing background (mask), 2=DIMTFILLCLR.
    if let Some(s) = style {
        if s.dimtfill == 1 || s.dimtfill == 2 {
            if let Some(rect) = text_fill_rect(dim, style, dim_txt, dim_scale) {
                let fill_color = if selected {
                    WireModel::SELECTED
                } else if s.dimtfill == 1 {
                    // Drawing-background fill: mask out geometry behind the text.
                    bg_color
                } else {
                    let c = AcadColor::from_index(s.dimtfillclr);
                    aci_to_rgba(&c)
                };
                wires.push(WireModel {
                    name: name.clone(),
                    points: vec![],
                    points_low: Vec::new(),
                    color: fill_color,
                    selected,
                    aci: 0,
                    pattern_length: 0.0,
                    pattern: [0.0; 8],
                    line_weight_px: 1.0,
                    snap_pts: vec![],
                    tangent_geoms: vec![],
                    key_vertices: vec![],
                    aabb: WireModel::UNBOUNDED_AABB,
                    plinegen: true,
                    vp_scissor: None,
                    fill_tris: rect,
                    fill_tris_low: Vec::new(),
                });
            }
        }
    }

    if let Some(synth_text_entity) = dimension_text_entity(dim, dim_txt, style, document, dim_scale)
    {
        // Tolerance Text rendered separately so DIMTFAC scales its height
        // and DIMTOLJ aligns it vertically against the primary text.
        let tol_entity = dimension_tolerance_entity(dim, style, &synth_text_entity, dim_txt);
        // Route synthesised dim text through tessellate_entity so the
        // baseline/greek/full LOD ladder applies (zoom-out behaviour
        // matches top-level Text / MText). The text already has dim_scale
        // baked into its height, so anno_scale stays 1.0.
        let text_wires = crate::scene::tessellate_entity_dim_text(
            document,
            selected_set,
            active_viewport,
            bg_color,
            1.0,
            &synth_text_entity,
            view_aabb,
            world_per_pixel,
            text_color,
        );
        for mut w in text_wires {
            w.name = name.clone();
            wires.push(w);
        }

        if let Some(tol_entity_e) = tol_entity {
            let tol_wires = crate::scene::tessellate_entity_dim_text(
                document,
                selected_set,
                active_viewport,
                bg_color,
                1.0,
                &tol_entity_e,
                view_aabb,
                world_per_pixel,
                text_color,
            );
            for mut w in tol_wires {
                w.name = name.clone();
                wires.push(w);
            }
        }
    }

    wires
}
fn resolve_dim_color(idx: i16, fallback: [f32; 4]) -> [f32; 4] {
    // DIMCLR* convention: 0 = BYBLOCK, 256 = BYLAYER → entity colour wins.
    if idx == 0 || idx == 256 {
        return fallback;
    }
    aci_to_rgba(&AcadColor::from_index(idx))
}

/// Resolve a DIMLWD / DIMLWE table value (the i16 lineweight code) into a
/// pixel width. -1 (ByLayer) / -2 (ByBlock) / -3 (Default) fall through to
/// the entity's already-resolved width.
fn resolve_dim_lineweight_px(code: i16, fallback_px: f32) -> f32 {
    const MM_TO_PX: f32 = 96.0 / 25.4;
    if code < 0 {
        return fallback_px;
    }
    // i16 value 0..=211 represents 1/100 mm.
    let mm = code as f32 / 100.0;
    (mm * MM_TO_PX).max(1.0)
}

/// Look up a linetype in the document's line_types table by handle and
/// resolve it to a (pattern_length, pattern) pair compatible with WireModel.
fn resolve_pattern_by_handle(
    doc: &CadDocument,
    handle: acadrust::types::Handle,
    scale: f32,
) -> (f32, [f32; 8]) {
    if handle.is_null() {
        return (0.0, [0.0; 8]);
    }
    let name = doc
        .line_types
        .iter()
        .find(|lt| lt.handle == handle)
        .map(|lt| lt.name.clone());
    match name {
        Some(n) => crate::scene::view::render::resolve_pattern(&doc.line_types, &n, scale),
        None => (0.0, [0.0; 8]),
    }
}

/// Split the combined ext-lines point list (NaN-separated segment pairs)
/// into "first" / "second" halves. `append_linear_dimension` writes ext1
/// before ext2, so the first segment is ext1 and the second is ext2.
fn split_ext_lines(points: &[[f32; 3]]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let mut groups: Vec<Vec<[f32; 3]>> = Vec::new();
    let mut current: Vec<[f32; 3]> = Vec::new();
    for &p in points {
        if p[0].is_nan() {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
        } else {
            current.push(p);
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    let mut iter = groups.into_iter();
    let first = iter.next().unwrap_or_default();
    let rest: Vec<[f32; 3]> = iter.flatten().collect();
    (first, rest)
}

/// Endpoints for the DIMTMOVE=1 leader: (anchor on the dim line, saved
/// text_middle_point). Returns None when the dim has no saved text position
/// or has no well-defined dim-line midpoint (radius/diameter handled by
/// their own leg).
fn dimtmove_leader_endpoints(dim: &Dimension) -> Option<(Vec3, Vec3)> {
    let base = dim.base();
    let txt = base.text_middle_point;
    if txt.x * txt.x + txt.y * txt.y + txt.z * txt.z <= 1e-16 {
        return None;
    }
    let lv = |v| vec3_local(v);
    let anchor = match dim {
        Dimension::Linear(d) => {
            let perp = Vec3::new(-(d.rotation.sin() as f32), d.rotation.cos() as f32, 0.0);
            let first = lv(d.first_point);
            let second = lv(d.second_point);
            let def = lv(d.definition_point);
            let off1 = def.dot(perp) - first.dot(perp);
            let off2 = def.dot(perp) - second.dot(perp);
            (first + perp * off1 + second + perp * off2) * 0.5
        }
        Dimension::Aligned(d) => {
            let first = lv(d.first_point);
            let second = lv(d.second_point);
            let axis = normalized_or(second - first, Vec3::X);
            let perp = Vec3::new(-axis.y, axis.x, 0.0);
            let def = lv(d.definition_point);
            let off1 = def.dot(perp) - first.dot(perp);
            let off2 = def.dot(perp) - second.dot(perp);
            (first + perp * off1 + second + perp * off2) * 0.5
        }
        Dimension::Radius(d) => lv(d.definition_point),
        Dimension::Diameter(d) => (lv(d.angle_vertex) + lv(d.definition_point)) * 0.5,
        _ => return None,
    };
    Some((anchor, lv(txt)))
}

/// Build a rectangle of filled triangles sitting under the dim text, used
/// when DIMTFILL = 2 (explicit fill colour). The rect width is estimated
/// from the formatted text length × character-cell width; an absolutely
/// correct box would need full text metrics from the font cache.
fn text_fill_rect(
    dim: &Dimension,
    style: Option<&DimStyle>,
    text_height: f64,
    dim_scale: f64,
) -> Option<Vec<[f32; 3]>> {
    let value = dimension_text_value(dim, style)?;
    if value.is_empty() {
        return None;
    }
    let pos = dimension_text_pos_f64(dim, style, text_height, dim_scale);
    let dimgap = style.map(|s| s.dimgap).unwrap_or(0.0).max(0.0) * dim_scale;
    // ~0.6 × text_height per character; matches average glyph aspect for
    // the bundled stick fonts. Inflate by 1 DIMGAP on each side.
    let approx_w = value.chars().count() as f64 * text_height * 0.6 + dimgap * 2.0;
    let approx_h = text_height + dimgap * 2.0;
    let rot = if dim.base().text_rotation.abs() > 1e-9 {
        dim.base().text_rotation
    } else {
        dimension_text_natural_rotation(dim)
    };
    let (sr, cr) = rot.sin_cos();
    let hx = approx_w * 0.5;
    let hy = approx_h * 0.5;
    let [ox, oy, oz] = [0.0_f64; 3];
    let cx = (pos.x - ox) as f32;
    let cy = (pos.y - oy) as f32;
    let cz = (pos.z - oz) as f32;
    let corner = |dx: f64, dy: f64| -> [f32; 3] {
        let lx = dx * cr - dy * sr;
        let ly = dx * sr + dy * cr;
        [cx + lx as f32, cy + ly as f32, cz]
    };
    let p1 = corner(-hx, -hy);
    let p2 = corner(hx, -hy);
    let p3 = corner(hx, hy);
    let p4 = corner(-hx, hy);
    Some(vec![p1, p2, p3, p1, p3, p4])
}
struct SuppressFlags {
    ext1: bool,
    ext2: bool,
    dim1: bool,
    dim2: bool,
}

#[derive(Clone, Copy)]
struct DimLineParams {
    dimexo: f32,
    dimexe: f32,
    dimdle: f32,
    dimfxl: f32,
    dimfxlon: bool,
    dimsoxd: bool,
    dimcen: f32,
    ticks: bool,
    /// Arrowhead length (DIMASZ, scaled) — used to decide arrow-outside fit.
    arrow_len: f32,
    /// Text box (local centre, half-width, half-height) used to break the
    /// dimension line where the text sits on it, so a DIMTFILL background reads
    /// over the line. None when the text doesn't overlap the line.
    text_break: Option<(Vec3, f32, f32)>,
}
fn dimension_geometry(
    dim: &Dimension,
    arrow1: &ArrowKind,
    arrow2: &ArrowKind,
    params: DimLineParams,
    suppress: SuppressFlags,
) -> DimGeom {
    let lv = |v| vec3_local(v);
    let mut g = DimGeom::new();
    match dim {
        Dimension::Aligned(d) => {
            let first = lv(d.first_point);
            let second = lv(d.second_point);
            let def = lv(d.definition_point);
            let axis = normalized_or(second - first, Vec3::X);
            append_linear_dimension(
                &mut g,
                first,
                second,
                def,
                axis,
                arrow1,
                arrow2,
                params,
                suppress,
                d.ext_line_rotation as f32,
            );
        }
        Dimension::Linear(d) => {
            let first = lv(d.first_point);
            let second = lv(d.second_point);
            let def = lv(d.definition_point);
            let axis = Vec3::new(d.rotation.cos() as f32, d.rotation.sin() as f32, 0.0);
            append_linear_dimension(
                &mut g,
                first,
                second,
                def,
                normalized_or(axis, Vec3::X),
                arrow1,
                arrow2,
                params,
                suppress,
                d.ext_line_rotation as f32,
            );
        }
        Dimension::Radius(d) => {
            let center = lv(d.angle_vertex);
            let point = lv(d.definition_point);
            let text = dimension_text_position(dim);
            add_segment(&mut g.dim_lines, center, point);
            // Honour leader_length: extend from the arrow tip past it
            // toward the text by that distance along (text - point).
            let leader_dir = normalized_or(text - point, Vec3::X);
            let leader = if d.leader_length.abs() > 1e-9 {
                point + leader_dir * (d.leader_length as f32)
            } else {
                text
            };
            add_segment(&mut g.dim_lines, point, leader);
            append_arrow(
                &mut g,
                point,
                normalized_or(center - point, Vec3::X),
                arrow1,
            );
            let radius = (point - center).length();
            append_center_mark(&mut g, center, params.dimcen, radius);
        }
        Dimension::Diameter(d) => {
            let p1 = lv(d.angle_vertex);
            let p2 = lv(d.definition_point);
            add_segment(&mut g.dim_lines, p1, p2);
            append_arrow(&mut g, p1, normalized_or(p2 - p1, Vec3::X), arrow1);
            append_arrow(&mut g, p2, normalized_or(p1 - p2, Vec3::X), arrow2);
            // DIMETER leader: continue past p2 toward the text.
            if d.leader_length.abs() > 1e-9 {
                let text = dimension_text_position(dim);
                let leader_dir = normalized_or(text - p2, p2 - p1);
                add_segment(
                    &mut g.dim_lines,
                    p2,
                    p2 + leader_dir * (d.leader_length as f32),
                );
            }
            let radius = (p2 - p1).length() * 0.5;
            append_center_mark(&mut g, (p1 + p2) * 0.5, params.dimcen, radius);
        }
        Dimension::Angular2Ln(d) => {
            append_angular_dimension(
                &mut g,
                lv(d.angle_vertex),
                lv(d.first_point),
                lv(d.second_point),
                lv(d.dimension_arc),
                arrow1,
                arrow2,
            );
        }
        Dimension::Angular3Pt(d) => {
            append_angular_dimension(
                &mut g,
                lv(d.angle_vertex),
                lv(d.first_point),
                lv(d.second_point),
                lv(d.definition_point),
                arrow1,
                arrow2,
            );
        }
        Dimension::Ordinate(d) => {
            add_segment(
                &mut g.dim_lines,
                lv(d.feature_location),
                lv(d.definition_point),
            );
            add_segment(
                &mut g.dim_lines,
                lv(d.definition_point),
                lv(d.leader_endpoint),
            );
        }
    }
    g
}

fn append_linear_dimension(
    g: &mut DimGeom,
    first: Vec3,
    second: Vec3,
    def: Vec3,
    axis: Vec3,
    arrow1: &ArrowKind,
    arrow2: &ArrowKind,
    params: DimLineParams,
    suppress: SuppressFlags,
    ext_line_rotation: f32,
) {
    let perp = Vec3::new(-axis.y, axis.x, 0.0);
    let dim_line_pos = def.dot(perp);
    let offset1 = dim_line_pos - first.dot(perp);
    let offset2 = dim_line_pos - second.dot(perp);
    let d1 = first + perp * offset1;
    let d2 = second + perp * offset2;
    let sign1 = if offset1 >= 0.0 { 1.0_f32 } else { -1.0 };
    let sign2 = if offset2 >= 0.0 { 1.0_f32 } else { -1.0 };

    // ext_line_rotation (DIMEDIT "Oblique"): rotate the extension lines by
    // this angle relative to perpendicular. The ext line still starts at
    // the def point; only the direction differs.
    let ext_dir = if ext_line_rotation.abs() > 1e-6 {
        let c = ext_line_rotation.cos();
        let s = ext_line_rotation.sin();
        // Rotate `perp` by ext_line_rotation around Z.
        Vec3::new(perp.x * c - perp.y * s, perp.x * s + perp.y * c, 0.0)
    } else {
        perp
    };

    // DIMFXLON / DIMFXL: fixed extension-line length from the dim line back
    // toward (but not past) the definition point. Otherwise grow from the
    // def point with DIMEXO gap, extending DIMEXE past the dim line.
    // When oblique, lengths are measured along ext_dir instead of perp.
    let (ext1_start, ext1_end, ext2_start, ext2_end) = if params.dimfxlon {
        let fxl = params.dimfxl.max(0.0);
        let s1 = d1 - ext_dir * (sign1 * fxl);
        let e1 = d1 + ext_dir * (sign1 * params.dimexe);
        let s2 = d2 - ext_dir * (sign2 * fxl);
        let e2 = d2 + ext_dir * (sign2 * params.dimexe);
        (s1, e1, s2, e2)
    } else {
        (
            first + ext_dir * (sign1 * params.dimexo),
            d1 + ext_dir * (sign1 * params.dimexe),
            second + ext_dir * (sign2 * params.dimexo),
            d2 + ext_dir * (sign2 * params.dimexe),
        )
    };
    if !suppress.ext1 {
        add_segment(&mut g.ext_lines, ext1_start, ext1_end);
    }
    if !suppress.ext2 {
        add_segment(&mut g.ext_lines, ext2_start, ext2_end);
    }

    // DIMDLE: dim line overshoots the ext line by `dimdle` at each end,
    // but only when ticks are in use (DIMTSZ > 0). With arrowheads this
    // is ignored, matching AutoCAD.
    let dle = if params.ticks { params.dimdle } else { 0.0 };
    let dir_d1_to_d2 = normalized_or(d2 - d1, axis);
    let d1_out = d1 - dir_d1_to_d2 * dle;
    let d2_out = d2 + dir_d1_to_d2 * dle;

    // DIMATFIT fit: when the arrowheads don't fit between the extension lines
    // they're flipped to the outside (point still on the ext line, body
    // outside) with a short stub for them to sit on. DIMSOXD suppresses those
    // outer stubs. Ticks always fit, so this only affects arrowheads.
    let gap = (d2 - d1).length();
    let arrows_outside = !params.ticks && params.arrow_len > 1e-6 && gap < 2.0 * params.arrow_len;

    // DIMSD1/DIMSD2: when *both* set, omit the dim line entirely. AutoCAD
    // splits at text otherwise — without that pivot info, leave as-is.
    if !(suppress.dim1 && suppress.dim2) {
        // Break the dim line where the text sits on it, so a DIMTFILL
        // background reads over the line (2-D fills draw under all wires, so the
        // line is gapped rather than z-masked).
        let mut drew = false;
        if let Some((tc, half_w, half_h)) = params.text_break {
            let dir = normalized_or(d2_out - d1_out, axis);
            let len = (d2_out - d1_out).length();
            let along = (tc - d1_out).dot(dir);
            let perp_dist = (tc - (d1_out + dir * along)).length();
            if perp_dist < half_h && along - half_w > 0.0 && along + half_w < len {
                add_segment(&mut g.dim_lines, d1_out, d1_out + dir * (along - half_w));
                add_segment(&mut g.dim_lines, d1_out + dir * (along + half_w), d2_out);
                drew = true;
            }
        }
        if !drew {
            add_segment(&mut g.dim_lines, d1_out, d2_out);
        }
        if arrows_outside && !params.dimsoxd {
            let stub = params.arrow_len * 2.0;
            add_segment(&mut g.dim_lines, d1 - dir_d1_to_d2 * stub, d1);
            add_segment(&mut g.dim_lines, d2, d2 + dir_d1_to_d2 * stub);
        }
    }

    if arrows_outside {
        // Tip on the ext line, body pointing outward.
        append_arrow(g, d1, normalized_or(d1 - d2, -axis), arrow1);
        append_arrow(g, d2, normalized_or(d2 - d1, axis), arrow2);
    } else {
        append_arrow(g, d1, normalized_or(d2 - d1, axis), arrow1);
        append_arrow(g, d2, normalized_or(d1 - d2, -axis), arrow2);
    }
}

/// Draw a center mark for radius/diameter dimensions.
///   DIMCEN > 0 → small "+" of half-length |DIMCEN| at the centre.
///   DIMCEN < 0 → small "+" *plus* four line segments extending from the
///                circle (radius - |DIMCEN|) outward to (radius + |DIMCEN|).
///   DIMCEN = 0 → no mark.
fn append_center_mark(g: &mut DimGeom, center: Vec3, dimcen: f32, radius: f32) {
    let mag = dimcen.abs();
    if mag <= 1e-6 {
        return;
    }
    // Small "+" at the centre.
    let h = mag;
    add_segment(
        &mut g.dim_lines,
        Vec3::new(center.x - h, center.y, center.z),
        Vec3::new(center.x + h, center.y, center.z),
    );
    add_segment(
        &mut g.dim_lines,
        Vec3::new(center.x, center.y - h, center.z),
        Vec3::new(center.x, center.y + h, center.z),
    );
    if dimcen < 0.0 && radius > mag + 1e-6 {
        let inner = (radius - mag).max(0.0);
        let outer = radius + mag;
        // Four short radial strokes spanning the circle edge.
        add_segment(
            &mut g.dim_lines,
            Vec3::new(center.x + inner, center.y, center.z),
            Vec3::new(center.x + outer, center.y, center.z),
        );
        add_segment(
            &mut g.dim_lines,
            Vec3::new(center.x - inner, center.y, center.z),
            Vec3::new(center.x - outer, center.y, center.z),
        );
        add_segment(
            &mut g.dim_lines,
            Vec3::new(center.x, center.y + inner, center.z),
            Vec3::new(center.x, center.y + outer, center.z),
        );
        add_segment(
            &mut g.dim_lines,
            Vec3::new(center.x, center.y - inner, center.z),
            Vec3::new(center.x, center.y - outer, center.z),
        );
    }
}

fn append_angular_dimension(
    g: &mut DimGeom,
    vertex: Vec3,
    first: Vec3,
    second: Vec3,
    arc_point: Vec3,
    arrow1: &ArrowKind,
    arrow2: &ArrowKind,
) {
    add_segment(&mut g.ext_lines, vertex, first);
    add_segment(&mut g.ext_lines, vertex, second);

    let radius = vertex.distance(arc_point);
    if radius <= 1e-6 {
        return;
    }

    let start = (first.y - vertex.y).atan2(first.x - vertex.x);
    let mut end = (second.y - vertex.y).atan2(second.x - vertex.x);
    let mut delta = end - start;
    while delta <= 0.0 {
        delta += std::f32::consts::TAU;
    }
    if delta > std::f32::consts::PI {
        end -= std::f32::consts::TAU;
        delta = end - start;
    }

    let steps = 32;
    let mut arc_pts = Vec::with_capacity((steps + 1) as usize);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let a = start + delta * t;
        arc_pts.push(vertex + Vec3::new(a.cos() * radius, a.sin() * radius, 0.0));
    }
    add_polyline(&mut g.dim_lines, &arc_pts);

    if arc_pts.len() >= 2 {
        append_arrow(
            g,
            arc_pts[0],
            normalized_or(arc_pts[1] - arc_pts[0], Vec3::X),
            arrow1,
        );
        let n = arc_pts.len();
        append_arrow(
            g,
            arc_pts[n - 1],
            normalized_or(arc_pts[n - 2] - arc_pts[n - 1], Vec3::X),
            arrow2,
        );
    }
}

fn dimension_snap_pts(dim: &Dimension) -> Vec<(glam::DVec3, SnapHint)> {
    let lv = |v: acadrust::types::Vector3| {
        glam::DVec3::new(
            v.x,
            v.y,
            v.z,
        )
    };
    let node = |v: acadrust::types::Vector3| (lv(v), SnapHint::Node);
    match dim {
        Dimension::Linear(d) => vec![
            node(d.first_point),
            node(d.second_point),
            node(d.definition_point),
        ],
        Dimension::Aligned(d) => vec![
            node(d.first_point),
            node(d.second_point),
            node(d.definition_point),
        ],
        Dimension::Radius(d) => vec![node(d.angle_vertex), node(d.definition_point)],
        Dimension::Diameter(d) => vec![node(d.angle_vertex), node(d.definition_point)],
        Dimension::Angular2Ln(d) => vec![
            node(d.angle_vertex),
            node(d.first_point),
            node(d.second_point),
            node(d.definition_point),
        ],
        Dimension::Angular3Pt(d) => vec![
            node(d.angle_vertex),
            node(d.first_point),
            node(d.second_point),
            node(d.definition_point),
        ],
        Dimension::Ordinate(d) => vec![
            node(d.definition_point),
            node(d.feature_location),
            node(d.leader_endpoint),
        ],
    }
}

/// Cheap heuristic: does this string contain anything the MText parser would
/// interpret? Used by `dimension_text_entity` to pick between a synthetic
/// `Text` (plain DXF special chars only) and a synthetic `MText` (full inline
/// format-code pipeline) for the dim text override.
fn value_has_mtext_codes(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' || c == '}' {
            return true;
        }
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                // Any backslash followed by a known MText escape letter.
                if matches!(
                    next,
                    'H' | 'W'
                        | 'Q'
                        | 'T'
                        | 'A'
                        | 'C'
                        | 'c'
                        | 'f'
                        | 'F'
                        | 'p'
                        | 'L'
                        | 'l'
                        | 'O'
                        | 'o'
                        | 'K'
                        | 'k'
                        | 'S'
                        | 's'
                        | 'P'
                        | 'n'
                        | 'N'
                        | 't'
                        | 'U'
                        | 'u'
                        | 'M'
                        | 'X'
                        | '~'
                        | '{'
                        | '}'
                ) {
                    return true;
                }
            }
        }
    }
    false
}

fn dimension_text_entity(
    dim: &Dimension,
    text_height: f64,
    style: Option<&DimStyle>,
    document: &CadDocument,
    dim_scale: f64,
) -> Option<EntityType> {
    let value = dimension_text_value(dim, style)?;
    // Use f64 position directly to avoid f32 round-trip precision loss at large
    // coordinates (e.g. Turkish UTM ~4,000,000 m). tessellate() will apply
    // world_offset when rendering this synthetic entity.
    let pos_f64 = dimension_text_pos_f64(dim, style, text_height, dim_scale);
    let base = dim.base();

    // DIMTIH/DIMTOH: when set, text is forced horizontal (rotation = 0)
    // regardless of the dim line angle. Honour explicit base.text_rotation
    // first, then horizontal_direction override, then DIMTIH/DIMTOH.
    let dimtih = style.map(|s| s.dimtih).unwrap_or(false);
    let dimtoh = style.map(|s| s.dimtoh).unwrap_or(false);
    let dimjust = style.map(|s| s.dimjust).unwrap_or(0);
    let rotation = if base.text_rotation.abs() > 1e-9 {
        base.text_rotation
    } else if base.horizontal_direction.abs() > 1e-9 {
        // horizontal_direction is the in-plane reading direction the writer
        // baked in (used for vertical / oblique dims).
        base.horizontal_direction
    } else if dimtih || dimtoh {
        0.0
    } else {
        let mut r = dimension_text_natural_rotation(dim);
        // DIMJUST 3/4: text reads along the extension lines, i.e. rotated 90°
        // from the dimension line.
        if dimjust == 3 || dimjust == 4 {
            r += std::f64::consts::FRAC_PI_2;
        }
        r
    };

    // Text style resolution priority:
    //   1. DIMTXSTY by handle (most reliable; survives rename)
    //   2. DIMTXSTY by name
    //   3. dim's own style_name (rare fallback)
    let style_name = style
        .and_then(|s| {
            if !s.dimtxsty_handle.is_null() {
                document
                    .text_styles
                    .iter()
                    .find(|ts| ts.handle == s.dimtxsty_handle)
                    .map(|ts| ts.name.clone())
            } else {
                None
            }
        })
        .or_else(|| {
            style
                .map(|s| s.dimtxsty.clone())
                .filter(|n| !n.trim().is_empty())
        })
        .unwrap_or_else(|| base.style_name.clone());

    // Route through MText whenever the value carries inline format codes
    // (`\f`, `\C`, `\H`, `\S`, brace scopes, …). Otherwise stay on the Text
    // path — single-line dim text doesn't need the full MText pipeline.
    if value_has_mtext_codes(&value) {
        use acadrust::entities::dimension::AttachmentPointType as DA;
        use acadrust::entities::AttachmentPoint as MA;
        let attachment_point = match base.attachment_point {
            DA::TopLeft => MA::TopLeft,
            DA::TopCenter => MA::TopCenter,
            DA::TopRight => MA::TopRight,
            DA::MiddleLeft => MA::MiddleLeft,
            DA::MiddleCenter => MA::MiddleCenter,
            DA::MiddleRight => MA::MiddleRight,
            DA::BottomLeft => MA::BottomLeft,
            DA::BottomCenter => MA::BottomCenter,
            DA::BottomRight => MA::BottomRight,
        };
        let mut mtext = MText::with_value(value, pos_f64);
        mtext.height = text_height;
        mtext.rotation = rotation;
        mtext.style = style_name;
        mtext.attachment_point = attachment_point;
        if base.line_spacing_factor.abs() > 1e-9 {
            mtext.line_spacing_factor = base.line_spacing_factor;
        }
        mtext.normal = base.normal;
        mtext.common = base.common.clone();
        return Some(EntityType::MText(mtext));
    }

    let mut text = Text::with_value(value, pos_f64)
        .with_height(text_height)
        .with_rotation(rotation);
    text.style = style_name;

    // Map AttachmentPointType (1..9 grid) to Text horizontal + vertical
    // alignments. 1=TopLeft … 9=BottomRight (column-major).
    let (ha, va) = attachment_to_text_align(base.attachment_point);
    text.horizontal_alignment = ha;
    text.vertical_alignment = va;
    // line_spacing_factor controls multi-line text spacing in MText. Our
    // synthetic Text is single-line so this is a no-op, but pass through
    // for completeness.
    let _ = base.line_spacing_factor;
    // normal would rotate the dim plane out of XY. The local 2D pipeline
    // assumes XY, so non-XY normals are read but not applied here.
    let _ = base.normal;

    text.common = base.common.clone();
    Some(EntityType::Text(text))
}

fn attachment_to_text_align(
    attach: acadrust::entities::dimension::AttachmentPointType,
) -> (
    acadrust::entities::text::TextHorizontalAlignment,
    acadrust::entities::text::TextVerticalAlignment,
) {
    use acadrust::entities::dimension::AttachmentPointType as A;
    use acadrust::entities::text::{TextHorizontalAlignment as H, TextVerticalAlignment as V};
    match attach {
        A::TopLeft => (H::Left, V::Top),
        A::TopCenter => (H::Center, V::Top),
        A::TopRight => (H::Right, V::Top),
        A::MiddleLeft => (H::Left, V::Middle),
        A::MiddleCenter => (H::Center, V::Middle),
        A::MiddleRight => (H::Right, V::Middle),
        A::BottomLeft => (H::Left, V::Bottom),
        A::BottomCenter => (H::Center, V::Bottom),
        A::BottomRight => (H::Right, V::Bottom),
    }
}

fn dimension_text_natural_rotation(dim: &Dimension) -> f64 {
    let angle = match dim {
        Dimension::Linear(d) => d.rotation,
        Dimension::Aligned(d) => {
            let dx = d.second_point.x - d.first_point.x;
            let dy = d.second_point.y - d.first_point.y;
            dy.atan2(dx)
        }
        _ => 0.0,
    };
    // Clamp to (-π/2, π/2] so text never appears upside-down.
    let pi = std::f64::consts::PI;
    if angle > pi / 2.0 {
        angle - pi
    } else if angle <= -pi / 2.0 {
        angle + pi
    } else {
        angle
    }
}

fn dimension_text_value(dim: &Dimension, style: Option<&DimStyle>) -> Option<String> {
    let (main, tol) = dimension_text_parts(dim, style)?;
    // Tolerance is appended inline for callers (e.g. fill rect width) that
    // don't render a separate tolerance entity. The visual pipeline that
    // does emit a separate tolerance text re-derives the parts itself.
    match tol {
        Some(t) => Some(format!("{} {}", main, t)),
        None => Some(main),
    }
}

/// Returns (primary_text, tolerance_suffix). The tolerance is emitted as a
/// separate Text entity so DIMTFAC can scale its height and DIMTOLJ can
/// align it vertically against the primary value.
fn dimension_text_parts(
    dim: &Dimension,
    style: Option<&DimStyle>,
) -> Option<(String, Option<String>)> {
    let base = dim.base();
    let is_angular = matches!(dim, Dimension::Angular2Ln(_) | Dimension::Angular3Pt(_));

    // Auto-generated body (the value AutoCAD would emit if the user did not
    // override it). Built first so user_text "<>" substitution can re-use it.
    let primary_raw = if is_angular {
        format_angular_value(dim.measurement(), style)
    } else {
        let v = format_linear_value(dim.measurement(), style);
        match dim {
            Dimension::Radius(_) => format!("R{}", v),
            Dimension::Diameter(_) => format!("Ø{}", v),
            _ => v,
        }
    };

    // Build tolerance / limits suffix separately so the caller can render
    // it as its own Text entity at DIMTFAC × DIMTXT height.
    let tolerance_suffix = build_tolerance_suffix(dim.measurement(), style, is_angular);
    let primary = apply_dimpost(&primary_raw, style);

    // Alternate units appended in brackets when DIMALT is on (linear only).
    let primary = if !is_angular {
        match alternate_units_text(dim.measurement(), style) {
            Some(alt) => format!("{} [{}]", primary, alt),
            None => primary,
        }
    } else {
        primary
    };

    // Explicit user override (mtext-style "user_text") wins, but "<>" inside
    // it substitutes the measured value. " " (single space) suppresses text.
    if let Some(user_text) = &base.user_text {
        if user_text.is_empty() || user_text.trim().is_empty() {
            return None;
        }
        return Some((user_text.replace("<>", &primary), tolerance_suffix));
    }
    if !base.text.trim().is_empty() {
        return Some((base.text.replace("<>", &primary), tolerance_suffix));
    }
    Some((primary, tolerance_suffix))
}

fn build_tolerance_suffix(
    measurement: f64,
    style: Option<&DimStyle>,
    is_angular: bool,
) -> Option<String> {
    let s = style?;
    let dimtdec = s.dimtdec.max(0) as usize;
    let dimtzin = s.dimtzin;
    let fmt = |v: f64| -> String {
        let raw = format!("{:.*}", dimtdec, v);
        apply_linear_zero_suppression(&raw, dimtzin)
    };
    if s.dimlim {
        let high = measurement + s.dimtp;
        let low = measurement - s.dimtm;
        return Some(format!("{}/{}", fmt(high), fmt(low)));
    }
    if s.dimtol {
        let unit = if is_angular { "°" } else { "" };
        if (s.dimtp - s.dimtm).abs() < 1e-12 && s.dimtp.abs() > 1e-12 {
            return Some(format!("±{}{}", fmt(s.dimtp), unit));
        }
        if s.dimtp.abs() > 1e-12 || s.dimtm.abs() > 1e-12 {
            return Some(format!(
                "+{}{} / -{}{}",
                fmt(s.dimtp),
                unit,
                fmt(s.dimtm),
                unit
            ));
        }
    }
    None
}

/// Build the bracketed alternate-units suffix when DIMALT is enabled.
/// When DIMTOL is also on, the bracketed text includes the tolerance
/// component formatted with DIMALTTD / DIMALTTZ.
fn alternate_units_text(measurement: f64, style: Option<&DimStyle>) -> Option<String> {
    let s = style?;
    if !s.dimalt {
        return None;
    }
    let mut v = measurement * s.dimaltf;
    if s.dimaltrnd > 1e-12 {
        v = (v / s.dimaltrnd).round() * s.dimaltrnd;
    }
    let dec = s.dimaltd.max(0) as usize;
    let raw = format_with_unit(v, s.dimaltu, dec, s.dimfrac, s.dimaltz);
    let suppressed = apply_linear_zero_suppression(&raw, s.dimaltz);
    let sep_swapped = swap_decimal_sep(&suppressed, s.dimdsep);
    // Alt-unit tolerance suffix using DIMALTTD / DIMALTTZ.
    let alt_value = if s.dimtol {
        let alttdec = s.dimalttd.max(0) as usize;
        let alttzin = s.dimalttz;
        let fmt = |x: f64| -> String {
            let raw = format!("{:.*}", alttdec, x * s.dimaltf);
            swap_decimal_sep(&apply_linear_zero_suppression(&raw, alttzin), s.dimdsep)
        };
        if (s.dimtp - s.dimtm).abs() < 1e-12 && s.dimtp.abs() > 1e-12 {
            format!("{}±{}", sep_swapped, fmt(s.dimtp))
        } else if s.dimtp.abs() > 1e-12 || s.dimtm.abs() > 1e-12 {
            format!("{} +{} / -{}", sep_swapped, fmt(s.dimtp), fmt(s.dimtm))
        } else {
            sep_swapped
        }
    } else if s.dimlim {
        let alttdec = s.dimalttd.max(0) as usize;
        let alttzin = s.dimalttz;
        let fmt = |x: f64| -> String {
            let raw = format!("{:.*}", alttdec, x * s.dimaltf);
            swap_decimal_sep(&apply_linear_zero_suppression(&raw, alttzin), s.dimdsep)
        };
        format!(
            "{}/{}",
            fmt(measurement + s.dimtp),
            fmt(measurement - s.dimtm)
        )
    } else {
        sep_swapped
    };
    // DIMAPOST wraps the alt value (same "<>" convention as DIMPOST).
    let wrapped = if s.dimapost.is_empty() {
        alt_value
    } else if s.dimapost.contains("<>") {
        s.dimapost.replace("<>", &alt_value)
    } else {
        format!("{}{}", alt_value, s.dimapost)
    };
    Some(wrapped)
}

/// Build the secondary tolerance Text entity at `DIMTXT × DIMTFAC` height,
/// positioned to the right of the primary text and vertically aligned per
/// `DIMTOLJ` (0=bottom, 1=middle, 2=top). Returns None when DIMTOL/DIMLIM
/// produce no tolerance string (e.g. both DIMTP and DIMTM are zero).
fn dimension_tolerance_entity(
    dim: &Dimension,
    style: Option<&DimStyle>,
    primary: &EntityType,
    primary_height: f64,
) -> Option<EntityType> {
    let s = style?;
    let is_angular = matches!(dim, Dimension::Angular2Ln(_) | Dimension::Angular3Pt(_));
    let tol = build_tolerance_suffix(dim.measurement(), style, is_angular)?;
    let dimtfac = if s.dimtfac.abs() < 1e-12 {
        1.0
    } else {
        s.dimtfac
    };
    let tol_height = primary_height * dimtfac;

    // Pull the geometry we need from the synthetic primary entity (Text or
    // MText — `dimension_text_entity` routes to MText when the dim value
    // carries inline format codes).
    let (primary_value_len, primary_insertion, primary_rotation, primary_style, primary_common) =
        match primary {
            EntityType::Text(t) => (
                t.value.chars().count(),
                t.insertion_point,
                t.rotation,
                t.style.clone(),
                t.common.clone(),
            ),
            EntityType::MText(m) => (
                m.value.chars().count(),
                m.insertion_point,
                m.rotation,
                m.style.clone(),
                m.common.clone(),
            ),
            _ => return None,
        };

    // Approximate widths from glyph counts (~0.6 × cell size per char).
    let primary_w = primary_value_len as f64 * primary_height * 0.6;
    let tol_w = tol.chars().count() as f64 * tol_height * 0.6;
    let gap = primary_height * 0.2;
    let dx_local = primary_w * 0.5 + tol_w * 0.5 + gap;
    let dy_local = match s.dimtolj {
        0 => -primary_height * 0.5 + tol_height * 0.5, // bottom-aligned with primary baseline
        2 => primary_height * 0.5 - tol_height * 0.5,  // top-aligned with primary top
        _ => 0.0,                                      // centred (default for ±)
    };
    let rot = primary_rotation;
    let (sr, cr) = rot.sin_cos();
    let pos = Vector3::new(
        primary_insertion.x + dx_local * cr - dy_local * sr,
        primary_insertion.y + dx_local * sr + dy_local * cr,
        primary_insertion.z,
    );
    let mut t = Text::with_value(tol, pos)
        .with_height(tol_height)
        .with_rotation(rot);
    t.style = primary_style;
    t.common = primary_common;
    t.horizontal_alignment = acadrust::entities::text::TextHorizontalAlignment::Center;
    t.vertical_alignment = acadrust::entities::text::TextVerticalAlignment::Middle;
    Some(EntityType::Text(t))
}

/// Wrap a measured value with the style's DIMPOST prefix/suffix template.
/// "<>" inside DIMPOST is replaced by the value; absent "<>" appends.
fn apply_dimpost(value: &str, style: Option<&DimStyle>) -> String {
    let post = style.map(|s| s.dimpost.as_str()).unwrap_or("");
    if post.is_empty() {
        return value.to_string();
    }
    if post.contains("<>") {
        post.replace("<>", value)
    } else {
        format!("{}{}", value, post)
    }
}

/// Format a linear measurement honouring DIMLFAC, DIMRND, DIMDEC, DIMZIN, DIMDSEP, DIMLUNIT.
fn format_linear_value(measurement: f64, style: Option<&DimStyle>) -> String {
    let (dec, zin, lfac, rnd, dsep, lunit, frac) = style
        .map(|s| {
            (
                s.dimdec, s.dimzin, s.dimlfac, s.dimrnd, s.dimdsep, s.dimlunit, s.dimfrac,
            )
        })
        .unwrap_or((4, 8, 1.0, 0.0, 46, 2, 0));

    let lfac = if lfac.abs() < 1e-12 { 1.0 } else { lfac };
    let mut v = measurement * lfac;
    if rnd > 1e-12 {
        v = (v / rnd).round() * rnd;
    }
    let dec = dec.max(0) as usize;
    let raw = format_with_unit(v, lunit, dec, frac, zin);
    let suppressed = apply_linear_zero_suppression(&raw, zin);
    swap_decimal_sep(&suppressed, dsep)
}

/// Dispatch on DIMLUNIT / DIMALTU.
///   1 = Scientific
///   2 = Decimal (default)
///   3 = Engineering   (feet + decimal inches; 1 unit = 1 inch)
///   4 = Architectural (feet + fractional inches)
///   5 = Fractional    (integer + fractional inches)
///   6 = Windows desktop → falls back to Decimal
/// `dimfrac` controls denominator power for arch/fractional output (0/1/2);
/// rendered inline as "n/d" (stacked glyphs require MText support).
fn format_with_unit(value: f64, unit: i16, dec: usize, dimfrac: i16, zin: i16) -> String {
    match unit {
        1 => format!("{:.*e}", dec, value),
        3 => format_engineering(value, dec),
        4 => format_architectural(value, dimfrac, zin),
        5 => format_fractional(value, dimfrac),
        _ => format!("{:.*}", dec, value),
    }
}

fn format_engineering(inches: f64, dec: usize) -> String {
    let sign = if inches < 0.0 { "-" } else { "" };
    let abs = inches.abs();
    let feet = (abs / 12.0).trunc();
    let rem_in = abs - feet * 12.0;
    format!("{}{:.0}'-{:.*}\"", sign, feet, dec, rem_in)
}

fn format_architectural(inches: f64, dimfrac: i16, zin: i16) -> String {
    let sign = if inches < 0.0 { "-" } else { "" };
    let abs = inches.abs();
    let feet = (abs / 12.0).trunc();
    let rem_in_total = abs - feet * 12.0;
    let whole = rem_in_total.trunc();
    let frac = rem_in_total - whole;
    let frac_str = fraction_string(frac, dimfrac);

    // DIMZIN feet/inch suppression:
    //   0 suppress zero feet & zero inches, 1 include both,
    //   2 include zero feet / suppress zero inches,
    //   3 suppress zero feet / include zero inches.
    let suppress_zero_feet = zin == 0 || zin == 3;
    let suppress_zero_inches = zin == 0 || zin == 2;
    let feet_zero = feet.abs() < 0.5;
    let inches_zero = whole.abs() < 0.5 && frac_str.is_empty();
    let show_feet = !feet_zero || !suppress_zero_feet;
    let show_inches = !inches_zero || !suppress_zero_inches;

    let feet_part = if show_feet {
        format!("{:.0}'", feet)
    } else {
        String::new()
    };
    let inch_part = if show_inches {
        if frac_str.is_empty() {
            format!("{:.0}\"", whole)
        } else {
            format!("{:.0} {}\"", whole, frac_str)
        }
    } else {
        String::new()
    };
    let body = match (feet_part.is_empty(), inch_part.is_empty()) {
        (false, false) => format!("{}-{}", feet_part, inch_part),
        (false, true) => feet_part,
        (true, false) => inch_part,
        (true, true) => "0\"".to_string(),
    };
    format!("{}{}", sign, body)
}

fn format_fractional(value: f64, dimfrac: i16) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let abs = value.abs();
    let whole = abs.trunc();
    let frac = abs - whole;
    let frac_str = fraction_string(frac, dimfrac);
    if frac_str.is_empty() {
        format!("{}{:.0}", sign, whole)
    } else if whole == 0.0 {
        format!("{}{}", sign, frac_str)
    } else {
        format!("{}{:.0} {}", sign, whole, frac_str)
    }
}

fn fraction_string(frac: f64, dimfrac: i16) -> String {
    // DIMFRAC denominator: AutoCAD encodes this on DIMSTYLE via DIMLUNIT
    // pairing — the value we accept is the *power-of-2* exponent (1..=6 ish).
    // Pick a sensible cap so the printed fraction stays readable.
    let exp = (dimfrac.clamp(0, 8) as u32).max(2) + 2; // 2..=10 → 4..=1024
    let denom = 1u64 << exp;
    let numer = (frac * denom as f64).round() as i64;
    if numer <= 0 {
        return String::new();
    }
    let mut n = numer as u64;
    let mut d = denom;
    while n % 2 == 0 && d % 2 == 0 {
        n /= 2;
        d /= 2;
    }
    if n == 0 {
        String::new()
    } else if d == 1 {
        format!("{}", n) // whole-number overflow back to caller
    } else {
        format!("{}/{}", n, d)
    }
}

/// Format an angular measurement (input in degrees as Dimension::measurement
/// returns for angular variants) honouring DIMAUNIT, DIMADEC, DIMAZIN.
fn format_angular_value(measurement_deg: f64, style: Option<&DimStyle>) -> String {
    let (aunit, adec, azin) = style
        .map(|s| (s.dimaunit, s.dimadec, s.dimazin))
        .unwrap_or((0, 2, 0));
    let adec = adec.max(0) as usize;

    match aunit {
        // 1 = Degrees / Minutes / Seconds
        1 => format_dms(measurement_deg, adec, azin),
        // 2 = Gradians
        2 => {
            let g = measurement_deg / 0.9;
            let raw = format!("{:.*}", adec, g);
            format!("{}g", apply_angular_zero_suppression(&raw, azin))
        }
        // 3 = Radians
        3 => {
            let r = measurement_deg.to_radians();
            let raw = format!("{:.*}", adec, r);
            format!("{}r", apply_angular_zero_suppression(&raw, azin))
        }
        // 0 or unknown = Decimal Degrees
        _ => {
            let raw = format!("{:.*}", adec, measurement_deg);
            format!("{}°", apply_angular_zero_suppression(&raw, azin))
        }
    }
}

fn format_dms(deg: f64, sec_dec: usize, azin: i16) -> String {
    let sign = if deg < 0.0 { "-" } else { "" };
    let abs = deg.abs();
    let d = abs.floor();
    let m_full = (abs - d) * 60.0;
    let m = m_full.floor();
    let s = (m_full - m) * 60.0;
    let s_str = format!("{:.*}", sec_dec, s);
    let mut out = format!("{}{:.0}°{:.0}'{}\"", sign, d, m, s_str);
    if azin & 4 != 0 {
        // suppress 0° / 0' parts
        if d == 0.0 {
            out = out.trim_start_matches('0').to_string();
            out = out.replacen("°", "", 1);
        }
    }
    out
}

/// Apply DIMZIN bit flags to a formatted linear value.
///  bit 0 (1)  suppress 0' (imperial feet)        — not applicable for decimal
///  bit 1 (2)  suppress 0" (imperial inches)      — not applicable for decimal
///  bit 2 (4)  suppress leading zeros             (e.g. ".5" not "0.5")
///  bit 3 (8)  suppress trailing zeros            (e.g. "1.5" not "1.50")
/// Default = 8 (trailing-zero suppression on).
fn apply_linear_zero_suppression(s: &str, zin: i16) -> String {
    let mut out = s.to_string();
    if zin & 8 != 0 {
        out = strip_trailing_zeros(&out);
    }
    if zin & 4 != 0 {
        out = strip_leading_zero(&out);
    }
    out
}

fn apply_angular_zero_suppression(s: &str, azin: i16) -> String {
    // DIMAZIN: 0=neither, 1=leading, 2=trailing, 3=both.
    let mut out = s.to_string();
    if azin & 2 != 0 {
        out = strip_trailing_zeros(&out);
    }
    if azin & 1 != 0 {
        out = strip_leading_zero(&out);
    }
    out
}

fn strip_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn strip_leading_zero(s: &str) -> String {
    // "0.5" → ".5",  "-0.5" → "-.5",  "0" stays.
    if let Some(rest) = s.strip_prefix("-0.") {
        return format!("-.{rest}");
    }
    if let Some(rest) = s.strip_prefix("0.") {
        return format!(".{rest}");
    }
    s.to_string()
}

fn swap_decimal_sep(s: &str, dsep_code: i16) -> String {
    // DIMDSEP holds an ASCII code (0 means default '.'). 46='.', 44=',', etc.
    if dsep_code <= 0 || dsep_code == 46 {
        return s.to_string();
    }
    let ch = char::from_u32(dsep_code as u32).unwrap_or('.');
    s.replace('.', &ch.to_string())
}

fn dimension_text_position(dim: &Dimension) -> Vec3 {
    let lv = |v| vec3_local(v);
    let base = dim.base();
    let pos = lv(base.text_middle_point);
    if pos.length_squared() > 1e-8 {
        return pos;
    }
    match dim {
        Dimension::Aligned(d) => (lv(d.first_point) + lv(d.second_point)) * 0.5,
        Dimension::Linear(d) => (lv(d.first_point) + lv(d.second_point)) * 0.5,
        Dimension::Radius(d) => (lv(d.angle_vertex) + lv(d.definition_point)) * 0.5,
        Dimension::Diameter(d) => (lv(d.angle_vertex) + lv(d.definition_point)) * 0.5,
        Dimension::Angular2Ln(d) => lv(d.dimension_arc),
        Dimension::Angular3Pt(d) => lv(d.definition_point),
        Dimension::Ordinate(d) => lv(d.leader_endpoint),
    }
}

fn vec3_local(v: Vector3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Style-driven text anchor on the dimension line: a point on the line through
/// `defpt` parallel to (`ax`,`ay`), slid along it per DIMJUST and lifted by
/// `perp_off` toward the side the dimension line sits on.
#[allow(clippy::too_many_arguments)]
fn text_on_dim_line(
    first: Vector3,
    second: Vector3,
    defpt: Vector3,
    ax: f64,
    ay: f64,
    dimjust: i16,
    perp_off: f64,
    text_w: f64,
    arrow: f64,
    dimtix: bool,
    tad: i16,
) -> Vector3 {
    // The perpendicular "up" side: the perpendicular of the dimension line
    // normalised so the text reads left-to-right (or bottom-to-top when
    // vertical). DIMTAD "above" (1) and JIS (3) sit on this side.
    let (mut nx, mut ny) = (ax, ay);
    if nx < 0.0 || (nx == 0.0 && ny < 0.0) {
        nx = -nx;
        ny = -ny;
    }
    let px = -ny;
    let py = nx;
    // DIMTAD side:
    //   1 (above) / 3 (JIS) / 0 (centred) → the text-up side, independent of the
    //     object — keying it off the object flipped "above" to "below" whenever
    //     the dimension ran the opposite way (#144);
    //   4 (below)   → the opposite side;
    //   2 (outside) → the side farthest from the defining points (the object).
    let perp_sign = match tad {
        4 => -1.0,
        2 => {
            let off = (defpt.x - first.x) * px + (defpt.y - first.y) * py;
            if off >= 0.0 {
                1.0
            } else {
                -1.0
            }
        }
        _ => 1.0,
    };
    // Along-axis positions of the extension points relative to the dim line.
    let t1 = (first.x - defpt.x) * ax + (first.y - defpt.y) * ay;
    let t2 = (second.x - defpt.x) * ax + (second.y - defpt.y) * ay;
    // DIMJUST: 0=centred, 1/3=near first ext, 2/4=near second ext.
    let mut along = match dimjust {
        1 | 3 => t1,
        2 | 4 => t2,
        _ => (t1 + t2) * 0.5,
    };
    // DIMATFIT / DIMTIX fit: when centred text can't fit between the extension
    // lines, slide it just past the far one (text-outside placement) unless
    // DIMTIX forces it to stay inside.
    if dimjust == 0 && !dimtix && text_w > 0.0 {
        let lo = t1.min(t2);
        let hi = t1.max(t2);
        if text_w > (hi - lo) - 2.0 * arrow {
            along = hi + arrow + text_w * 0.5;
        }
    }
    let bx = defpt.x + ax * along;
    let by = defpt.y + ay * along;
    Vector3::new(
        bx + px * perp_off * perp_sign,
        by + py * perp_off * perp_sign,
        defpt.z,
    )
}

fn dimension_text_pos_f64(
    dim: &Dimension,
    style: Option<&DimStyle>,
    text_height: f64,
    dim_scale: f64,
) -> Vector3 {
    let base = dim.base();

    // DIMTAD: 0=centred (on the line), 1=above, 4=below. 2 (outside, i.e. the
    // side farthest from the defining points) and 3 (JIS) both resolve to the
    // away-from-object side, which is the same as "above" for 2-D linear dims.
    let dimtad = style.map(|s| s.dimtad).unwrap_or(1);
    // DIMGAP scales with DIMSCALE just like DIMTXT, so the text-to-line gap
    // stays consistent when DIMSCALE != 1.
    let dimgap = style.map(|s| s.dimgap).unwrap_or(0.0) * dim_scale;
    let dimjust = style.map(|s| s.dimjust).unwrap_or(0);
    // DIMTIX forces the text to stay between the extension lines.
    let dimtix = style.map(|s| s.dimtix).unwrap_or(false);
    // DIMTVP vertical-position multiplier (units of dimtxt). Only honoured when
    // DIMTAD == 0; offsets text perpendicular to the dim line.
    let dimtvp = style.map(|s| s.dimtvp).unwrap_or(0.0);
    let perp_off = if dimtad == 0 {
        dimtvp * text_height
    } else {
        text_height * 0.5 + dimgap
    };
    // Rough text width + arrow allowance, used to decide text-outside fit.
    let text_w = dimension_text_value(dim, style)
        .map(|t| t.chars().count() as f64 * text_height * 0.6 + 2.0 * dimgap)
        .unwrap_or(0.0);
    let arrow = text_height; // arrows are roughly text-height sized

    // Explicit per-entity override (text dragged to a custom location): the
    // saved point wins. Otherwise the dimension style governs placement.
    let use_saved = base.text_user_positioned && {
        let p = base.text_middle_point;
        p.x * p.x + p.y * p.y + p.z * p.z > 1e-16
    };
    if use_saved {
        return base.text_middle_point;
    }

    match dim {
        Dimension::Linear(d) => {
            let ax = d.rotation.cos();
            let ay = d.rotation.sin();
            text_on_dim_line(
                d.first_point,
                d.second_point,
                d.definition_point,
                ax,
                ay,
                dimjust,
                perp_off,
                text_w,
                arrow,
                dimtix,
                dimtad,
            )
        }
        Dimension::Aligned(d) => {
            let dx = d.second_point.x - d.first_point.x;
            let dy = d.second_point.y - d.first_point.y;
            let len = (dx * dx + dy * dy).sqrt().max(1e-12);
            text_on_dim_line(
                d.first_point,
                d.second_point,
                d.definition_point,
                dx / len,
                dy / len,
                dimjust,
                perp_off,
                text_w,
                arrow,
                dimtix,
                dimtad,
            )
        }
        _ => {
            // Non-linear (radius / diameter / angular / ordinate): keep the
            // saved text point when present for fidelity, else lift the natural
            // mid point straight up by the style offset.
            let p = base.text_middle_point;
            if p.x * p.x + p.y * p.y + p.z * p.z > 1e-16 {
                return p;
            }
            let mid = match dim {
                Dimension::Radius(d) => Vector3::new(
                    (d.angle_vertex.x + d.definition_point.x) * 0.5,
                    (d.angle_vertex.y + d.definition_point.y) * 0.5,
                    (d.angle_vertex.z + d.definition_point.z) * 0.5,
                ),
                Dimension::Diameter(d) => Vector3::new(
                    (d.angle_vertex.x + d.definition_point.x) * 0.5,
                    (d.angle_vertex.y + d.definition_point.y) * 0.5,
                    (d.angle_vertex.z + d.definition_point.z) * 0.5,
                ),
                Dimension::Angular2Ln(d) => d.dimension_arc,
                Dimension::Angular3Pt(d) => d.definition_point,
                Dimension::Ordinate(d) => d.leader_endpoint,
                _ => base.text_middle_point,
            };
            Vector3::new(mid.x, mid.y + perp_off * perp_sign_default(), mid.z)
        }
    }
}

fn perp_sign_default() -> f64 {
    1.0
}

#[cfg(test)]
mod dimtad_tests {
    use super::text_on_dim_line;
    use acadrust::types::Vector3;

    fn v(x: f64, y: f64) -> Vector3 {
        Vector3::new(x, y, 0.0)
    }

    // DIMTAD=Above must place text on the geometric "up" side of a horizontal
    // dimension whichever way the dimension runs; DIMTAD=Below flips it. (#144)
    #[test]
    fn above_is_up_regardless_of_direction() {
        let (first, second, defpt) = (v(0.0, 10.0), v(20.0, 10.0), v(0.0, 0.0));
        let perp = 5.0;
        let fwd =
            text_on_dim_line(first, second, defpt, 1.0, 0.0, 0, perp, 0.0, 1.0, false, 1);
        let rev =
            text_on_dim_line(first, second, defpt, -1.0, 0.0, 0, perp, 0.0, 1.0, false, 1);
        assert!(fwd.y > 0.0, "above must be +Y, got {}", fwd.y);
        assert!(rev.y > 0.0, "above must be +Y even reversed, got {}", rev.y);
        let below =
            text_on_dim_line(first, second, defpt, 1.0, 0.0, 0, perp, 0.0, 1.0, false, 4);
        assert!(below.y < 0.0, "below must be -Y, got {}", below.y);
    }

    // DIMTAD=Outside (2) stays on the side farthest from the measured points,
    // whichever side of the geometry the dimension line is on.
    #[test]
    fn outside_is_away_from_object() {
        let perp = 5.0;
        // Object at y=0, dim line above it (y=10): away → further above.
        let above = text_on_dim_line(
            v(0.0, 0.0), v(20.0, 0.0), v(0.0, 10.0), 1.0, 0.0, 0, perp, 0.0, 1.0, false, 2,
        );
        assert!(above.y > 10.0, "outside must clear the object side, got {}", above.y);
        // Object at y=0, dim line below it (y=-10): away → further below.
        let below = text_on_dim_line(
            v(0.0, 0.0), v(20.0, 0.0), v(0.0, -10.0), 1.0, 0.0, 0, perp, 0.0, 1.0, false, 2,
        );
        assert!(below.y < -10.0, "outside must clear the object side, got {}", below.y);
    }
}
