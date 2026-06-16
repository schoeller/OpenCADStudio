// Grippable + PropertyEditable for Solid3D, Region, Body.
//
// Geometry lives in ACIS data — we cannot edit it via the properties panel.
// We expose the point_of_reference as a translate grip and show ACIS size
// as read-only info.  Grip translate also updates wire points so the wire
// fallback stays in sync; the caller (scene/mod.rs apply_grip) translates
// the MeshModel vertices to match.

use acadrust::entities::{Body, Region, Solid3D};
use glam::Vec3;

use crate::entities::common::{center_grip, ro_prop as ro};
use crate::entities::traits::{Grippable, PropertyEditable};
use crate::scene::model::object::{GripApply, GripDef, PropSection};

// ── shared helpers ────────────────────────────────────────────────────────────

fn dvec3(v: &acadrust::types::Vector3) -> glam::DVec3 {
    glam::DVec3::new(v.x, v.y, v.z)
}

fn translate_wires(wires: &mut Vec<acadrust::entities::Wire>, d: Vec3) {
    for wire in wires.iter_mut() {
        for pt in wire.points.iter_mut() {
            pt.x += d.x as f64;
            pt.y += d.y as f64;
            pt.z += d.z as f64;
        }
    }
}

fn acis_size_str(has_data: bool, sat_len: usize, sab_len: usize, is_binary: bool) -> String {
    if !has_data {
        return "none".to_string();
    }
    if is_binary {
        format!("{} bytes (SAB)", sab_len)
    } else {
        format!("{} bytes (SAT)", sat_len)
    }
}

// ── Solid3D ───────────────────────────────────────────────────────────────────

impl Grippable for Solid3D {
    fn grips(&self) -> Vec<GripDef> {
        vec![center_grip(0, dvec3(&self.point_of_reference))]
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        if grip_id != 0 {
            return;
        }
        if let GripApply::Translate(d) = apply {
            self.point_of_reference.x += d.x as f64;
            self.point_of_reference.y += d.y as f64;
            self.point_of_reference.z += d.z as f64;
            translate_wires(&mut self.wires, d);
        }
    }
}

impl PropertyEditable for Solid3D {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        let size = acis_size_str(
            self.acis_data.has_data(),
            self.acis_data.sat_data.len(),
            self.acis_data.sab_data.len(),
            self.acis_data.is_binary,
        );
        PropSection {
            title: "Geometry".into(),
            props: vec![
                ro(
                    "Ref Pt X",
                    "s3d_px",
                    format!("{:.4}", self.point_of_reference.x),
                ),
                ro(
                    "Ref Pt Y",
                    "s3d_py",
                    format!("{:.4}", self.point_of_reference.y),
                ),
                ro(
                    "Ref Pt Z",
                    "s3d_pz",
                    format!("{:.4}", self.point_of_reference.z),
                ),
                ro("ACIS Data", "s3d_acis", size),
                ro(
                    "UID",
                    "s3d_uid",
                    if self.uid.is_empty() {
                        "(none)".to_string()
                    } else {
                        self.uid.clone()
                    },
                ),
                ro(
                    "Silhouettes",
                    "s3d_silhouettes",
                    self.silhouettes.len().to_string(),
                ),
                ro(
                    "History",
                    "s3d_history",
                    match self.history_handle {
                        Some(h) if !h.is_null() => format!("{:X}", h.value()),
                        _ => "(none)".to_string(),
                    },
                ),
            ],
        }
    }

    fn apply_geom_prop(&mut self, _field: &str, _value: &str) {}
}

// ── Region ────────────────────────────────────────────────────────────────────

impl Grippable for Region {
    fn grips(&self) -> Vec<GripDef> {
        vec![center_grip(0, dvec3(&self.point_of_reference))]
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        if grip_id != 0 {
            return;
        }
        if let GripApply::Translate(d) = apply {
            self.point_of_reference.x += d.x as f64;
            self.point_of_reference.y += d.y as f64;
            self.point_of_reference.z += d.z as f64;
            translate_wires(&mut self.wires, d);
        }
    }
}

impl PropertyEditable for Region {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        let size = acis_size_str(
            self.acis_data.has_data(),
            self.acis_data.sat_data.len(),
            self.acis_data.sab_data.len(),
            self.acis_data.is_binary,
        );
        PropSection {
            title: "Geometry".into(),
            props: vec![
                ro(
                    "Ref Pt X",
                    "rgn_px",
                    format!("{:.4}", self.point_of_reference.x),
                ),
                ro(
                    "Ref Pt Y",
                    "rgn_py",
                    format!("{:.4}", self.point_of_reference.y),
                ),
                ro(
                    "Ref Pt Z",
                    "rgn_pz",
                    format!("{:.4}", self.point_of_reference.z),
                ),
                ro("ACIS Data", "rgn_acis", size),
                ro(
                    "UID",
                    "rgn_uid",
                    if self.uid.is_empty() {
                        "(none)".to_string()
                    } else {
                        self.uid.clone()
                    },
                ),
                ro(
                    "Silhouettes",
                    "rgn_silhouettes",
                    self.silhouettes.len().to_string(),
                ),
            ],
        }
    }

    fn apply_geom_prop(&mut self, _field: &str, _value: &str) {}
}

// ── Body ──────────────────────────────────────────────────────────────────────

impl Grippable for Body {
    fn grips(&self) -> Vec<GripDef> {
        vec![center_grip(0, dvec3(&self.point_of_reference))]
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        if grip_id != 0 {
            return;
        }
        if let GripApply::Translate(d) = apply {
            self.point_of_reference.x += d.x as f64;
            self.point_of_reference.y += d.y as f64;
            self.point_of_reference.z += d.z as f64;
            translate_wires(&mut self.wires, d);
        }
    }
}

impl PropertyEditable for Body {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        let size = acis_size_str(
            self.acis_data.has_data(),
            self.acis_data.sat_data.len(),
            self.acis_data.sab_data.len(),
            self.acis_data.is_binary,
        );
        PropSection {
            title: "Geometry".into(),
            props: vec![
                ro(
                    "Ref Pt X",
                    "bdy_px",
                    format!("{:.4}", self.point_of_reference.x),
                ),
                ro(
                    "Ref Pt Y",
                    "bdy_py",
                    format!("{:.4}", self.point_of_reference.y),
                ),
                ro(
                    "Ref Pt Z",
                    "bdy_pz",
                    format!("{:.4}", self.point_of_reference.z),
                ),
                ro("ACIS Data", "bdy_acis", size),
                ro(
                    "UID",
                    "bdy_uid",
                    if self.uid.is_empty() {
                        "(none)".to_string()
                    } else {
                        self.uid.clone()
                    },
                ),
                ro(
                    "Silhouettes",
                    "bdy_silhouettes",
                    self.silhouettes.len().to_string(),
                ),
            ],
        }
    }

    fn apply_geom_prop(&mut self, _field: &str, _value: &str) {}
}

// ── Accessors for the Solid3D / Region / Body trio ─────────────────────────
//
// These three entity types share a common subset of fields (ACIS data
// + point_of_reference + wires fallback). Code that needs to treat them
// uniformly (mesh tess dispatch, fallback wires, grip translate) used
// to repeat a three-arm `match entity` block at every callsite — the
// helpers below collapse those to a single call.

use crate::scene::model::mesh_model::MeshLodSet;
use crate::scene::convert::solid3d_tess;
use acadrust::{types::Vector3, EntityType};

/// `point_of_reference` of an ACIS-backed volume entity, if applicable.
pub fn point_of_reference(e: &EntityType) -> Option<&Vector3> {
    match e {
        EntityType::Solid3D(s) => Some(&s.point_of_reference),
        EntityType::Region(r) => Some(&r.point_of_reference),
        EntityType::Body(b) => Some(&b.point_of_reference),
        _ => None,
    }
}

/// Pre-stored edge-wire fallback list (used when the SAT/SAB kernel
/// can't produce a mesh — drawings authored by SOLVIEW / 3DPLOT carry
/// these explicitly).
pub fn fallback_wires(e: &EntityType) -> Option<&[acadrust::entities::Wire]> {
    match e {
        EntityType::Solid3D(s) => Some(&s.wires),
        EntityType::Region(r) => Some(&r.wires),
        EntityType::Body(b) => Some(&b.wires),
        EntityType::Surface(s) => Some(&s.wires),
        _ => None,
    }
}

/// Run the appropriate `solid3d_tess::tessellate_*` for the entity,
/// returning `None` for non-volume entities or when the kernel fails.
pub fn tessellate_volume(e: &EntityType, color: [f32; 4], facet_res: f64) -> Option<MeshLodSet> {
    match e {
        EntityType::Solid3D(s) => solid3d_tess::tessellate_solid3d(s, color, facet_res),
        EntityType::Region(r) => solid3d_tess::tessellate_region(r, color, facet_res),
        EntityType::Body(b) => solid3d_tess::tessellate_body(b, color, facet_res),
        EntityType::Surface(s) => solid3d_tess::tessellate_surface(s, color, facet_res),
        _ => None,
    }
}
