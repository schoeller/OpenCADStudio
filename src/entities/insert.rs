use acadrust::entities::Insert;
use acadrust::types::{Matrix3, Transform, Vector3};
use acadrust::{EntityType, Handle};
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{edit_prop as edit, parse_f64, ro_prop as ro, square_grip};

use crate::scene::model::object::{GripApply, GripDef, PropSection};
use crate::scene::model::wire_model::WireModel;
use crate::scene::cache::block_cache;
use crate::scene::convert::tessellate;
use crate::scene::view::render;

fn grips(ins: &Insert) -> Vec<GripDef> {
    // `insert_point` is in the OCS defined by `normal`; the grip must sit at
    // the world placement, so map it through the OCS. Identity for +Z.
    let w = Matrix3::arbitrary_axis(ins.normal) * ins.insert_point;
    vec![square_grip(0, glam::DVec3::new(w.x, w.y, w.z))]
}

fn properties(ins: &Insert) -> PropSection {
    PropSection {
        title: "Geometry".into(),
        props: vec![
            edit("Insert X", "ins_x", ins.insert_point.x),
            edit("Insert Y", "ins_y", ins.insert_point.y),
            edit("Insert Z", "ins_z", ins.insert_point.z),
            edit("Scale X", "x_scale", ins.x_scale()),
            edit("Scale Y", "y_scale", ins.y_scale()),
            edit("Scale Z", "z_scale", ins.z_scale()),
            edit("Rotation", "rotation", ins.rotation.to_degrees()),
            ro("Block", "block", ins.block_name.clone()),
        ],
    }
}

fn apply_geom_prop(ins: &mut Insert, field: &str, value: &str) {
    let Some(v) = parse_f64(value) else {
        return;
    };
    match field {
        "ins_x" => ins.insert_point.x = v,
        "ins_y" => ins.insert_point.y = v,
        "ins_z" => ins.insert_point.z = v,
        "x_scale" => ins.set_x_scale(v),
        "y_scale" => ins.set_y_scale(v),
        "z_scale" => ins.set_z_scale(v),
        "rotation" => ins.rotation = v.to_radians(),
        _ => {}
    }
}

fn apply_grip(ins: &mut Insert, _grip_id: usize, apply: GripApply) {
    // The grip works in world space, but `insert_point` is stored in the OCS
    // defined by `normal`. Round-trip through the OCS so dragging a block
    // whose extrusion direction isn't +Z moves along world axes. Identity OCS
    // for a +Z normal, so this matches the old direct assignment there.
    let ocs = Matrix3::arbitrary_axis(ins.normal);
    let world = match apply {
        GripApply::Absolute(p) => Vector3::new(p.x as f64, p.y as f64, p.z as f64),
        GripApply::Translate(d) => {
            ocs * ins.insert_point + Vector3::new(d.x as f64, d.y as f64, d.z as f64)
        }
    };
    ins.insert_point = ocs.transpose() * world;
}

fn apply_transform(ins: &mut Insert, t: &EntityTransform) {
    crate::scene::view::transform::apply_standard_entity_transform(ins, t, |entity, p1, p2| {
        let dx = (p2.x - p1.x) as f64;
        let dy = (p2.y - p1.y) as f64;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-12 {
            return;
        }

        let ux = dx / len;
        let uy = dy / len;
        let mirror = acadrust::types::Matrix4 {
            m: [
                [2.0 * ux * ux - 1.0, 2.0 * ux * uy, 0.0, 0.0],
                [2.0 * ux * uy, 2.0 * uy * uy - 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let t = Transform::from_translation(Vector3::new(-(p1.x as f64), -(p1.y as f64), 0.0))
            .then(&Transform::from_matrix(mirror))
            .then(&Transform::from_translation(Vector3::new(
                p1.x as f64,
                p1.y as f64,
                0.0,
            )));
        acadrust::Entity::apply_transform(entity, &t);
    });
}

crate::impl_entity_basics!(Insert);

impl crate::entities::traits::FallbackTess for Insert {
    fn fallback_geometry(
        &self,
    ) -> crate::scene::convert::tess_util::FallbackGeometry {
        let (ipx, ipy, ipz) = (
            self.insert_point.x,
            self.insert_point.y,
            self.insert_point.z,
        );
        let ip = Vec3::new(ipx as f32, ipy as f32, ipz as f32);
        let s = 0.1_f64;
        let pts = vec![
            [ipx - s, ipy, ipz],
            [ipx + s, ipy, ipz],
            [ipx, ipy - s, ipz],
            [ipx, ipy + s, ipz],
        ];
        (
            pts,
            vec![(ip, crate::scene::model::wire_model::SnapHint::Insertion)],
            vec![],
            vec![],
        )
    }
}
pub(crate) fn append_insert_attribute_wires(
    wires: &mut Vec<WireModel>,
    document: &acadrust::CadDocument,
    ins: &acadrust::entities::Insert,
    insert_handle: Handle,
    sel: bool,
    ins_color: [f32; 4],
    ins_pat_len: f32,
    ins_pat: [f32; 8],
    ins_lw_px: f32,
    bg_color: [f32; 4],
    is_xref: bool,
    pslt_factor: f32,
    anno_scale: f32,
) {
    if ins.attributes.is_empty() {
        return;
    }
    // ATTMODE (header.attribute_visibility):
    //   0 = Off    — every attribute hidden
    //   1 = Normal — honour per-attribute `invisible` flag (default)
    //   2 = On     — every attribute forced visible, ignoring its flag
    let attmode = document.header.attribute_visibility;
    if attmode == 0 {
        return;
    }
    for attr in &ins.attributes {
        let per_attr_hidden = attr.common.invisible || attr.flags.invisible;
        if attmode == 1 && per_attr_hidden {
            continue;
        }
        let attr_entity = EntityType::AttributeEntity(attr.clone());
        let (sub_color, sub_plen, sub_pat, sub_lw_px, sub_aci) = render::render_style_for_block_sub(
            document,
            &attr_entity,
            ins_color,
            ins_pat_len,
            ins_pat,
            ins_lw_px,
        );
        let sub_color = render::adapt_to_bg(sub_color, bg_color);
        let sub_color = if is_xref && !sel {
            block_cache::fade_toward_bg(sub_color, bg_color)
        } else {
            sub_color
        };
        let sub_aabb = crate::scene::entity_aabb(&attr_entity);
        let mut attr_wires = tessellate::tessellate(
            document,
            insert_handle,
            &attr_entity,
            sel,
            sub_color,
            sub_plen * pslt_factor,
            sub_pat.map(|v| v * pslt_factor),
            sub_lw_px,
            anno_scale,
            None,
        );
        // Use the INSERT's handle so selection / picking groups attribute
        // text with the parent insert instead of treating it as a stray text.
        for w in &mut attr_wires {
            w.name = insert_handle.value().to_string();
            w.aci = sub_aci;
            w.aabb = sub_aabb;
        }
        wires.extend(attr_wires);
    }
}
