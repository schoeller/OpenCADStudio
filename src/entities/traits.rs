use acadrust::{CadDocument, EntityType};

use crate::command::EntityTransform;
use crate::scene::acad_to_truck::TruckEntity;
use crate::scene::object::{GripApply, GripDef, PropSection};

pub trait TruckConvertible {
    fn to_truck(&self, document: &CadDocument) -> Option<TruckEntity>;
}

pub trait Grippable {
    fn grips(&self) -> Vec<GripDef>;
    fn apply_grip(&mut self, grip_id: usize, apply: GripApply);
}

pub trait PropertyEditable {
    fn geometry_properties(&self, text_style_names: &[String]) -> PropSection;
    fn apply_geom_prop(&mut self, field: &str, value: &str);
}

pub trait Transformable {
    fn apply_transform(&mut self, t: &EntityTransform);
}

pub trait EntityTypeOps {
    fn to_truck_entity(&self, document: &CadDocument) -> Option<TruckEntity>;
    fn grips(&self) -> Vec<GripDef>;
    fn geometry_properties(&self, text_style_names: &[String]) -> Option<PropSection>;
    fn apply_geom_prop(&mut self, field: &str, value: &str);
    fn apply_grip(&mut self, grip_id: usize, apply: GripApply);
    fn apply_transform(&mut self, t: &EntityTransform);
}

impl EntityTypeOps for EntityType {
    fn to_truck_entity(&self, document: &CadDocument) -> Option<TruckEntity> {
        match self {
            EntityType::Point(pt) => TruckConvertible::to_truck(pt, document),
            EntityType::Line(line) => TruckConvertible::to_truck(line, document),
            EntityType::Circle(circle) => TruckConvertible::to_truck(circle, document),
            EntityType::Arc(arc) => TruckConvertible::to_truck(arc, document),
            EntityType::Ellipse(ellipse) => TruckConvertible::to_truck(ellipse, document),
            EntityType::Spline(spline) => TruckConvertible::to_truck(spline, document),
            EntityType::LwPolyline(pline) => TruckConvertible::to_truck(pline, document),
            EntityType::Polyline(pl) => TruckConvertible::to_truck(pl, document),
            EntityType::Polyline2D(pl) => TruckConvertible::to_truck(pl, document),
            EntityType::Polyline3D(pl) => TruckConvertible::to_truck(pl, document),
            EntityType::Ray(ray) => TruckConvertible::to_truck(ray, document),
            EntityType::XLine(xl) => TruckConvertible::to_truck(xl, document),
            EntityType::RasterImage(img) => TruckConvertible::to_truck(img, document),
            EntityType::Wipeout(wo) => TruckConvertible::to_truck(wo, document),
            EntityType::AttributeDefinition(a) => TruckConvertible::to_truck(a, document),
            EntityType::AttributeEntity(a) => TruckConvertible::to_truck(a, document),
            EntityType::MLine(ml) => TruckConvertible::to_truck(ml, document),
            EntityType::Tolerance(tol) => TruckConvertible::to_truck(tol, document),
            EntityType::Solid(solid) => TruckConvertible::to_truck(solid, document),
            EntityType::Face3D(f) => TruckConvertible::to_truck(f, document),
            EntityType::PolygonMesh(pm) => TruckConvertible::to_truck(pm, document),
            EntityType::PolyfaceMesh(pfm) => TruckConvertible::to_truck(pfm, document),
            EntityType::Table(tbl) => TruckConvertible::to_truck(tbl, document),
            EntityType::Text(text) => TruckConvertible::to_truck(text, document),
            EntityType::MText(text) => TruckConvertible::to_truck(text, document),
            EntityType::Leader(leader) => TruckConvertible::to_truck(leader, document),
            EntityType::MultiLeader(ml) => TruckConvertible::to_truck(ml, document),
            EntityType::Underlay(ul) => TruckConvertible::to_truck(ul, document),
            EntityType::Shape(shp) => TruckConvertible::to_truck(shp, document),
            EntityType::Ole2Frame(ole) => TruckConvertible::to_truck(ole, document),
            _ => None,
        }
    }

    fn grips(&self) -> Vec<GripDef> {
        match self {
            EntityType::Line(line) => Grippable::grips(line),
            EntityType::Circle(circle) => Grippable::grips(circle),
            EntityType::Arc(arc) => Grippable::grips(arc),
            EntityType::Ellipse(ellipse) => Grippable::grips(ellipse),
            EntityType::LwPolyline(pline) => Grippable::grips(pline),
            EntityType::Polyline(pl) => Grippable::grips(pl),
            EntityType::Polyline2D(pl) => Grippable::grips(pl),
            EntityType::Polyline3D(pl) => Grippable::grips(pl),
            EntityType::Ray(ray) => Grippable::grips(ray),
            EntityType::XLine(xl) => Grippable::grips(xl),
            EntityType::RasterImage(img) => Grippable::grips(img),
            EntityType::Wipeout(wo) => Grippable::grips(wo),
            EntityType::AttributeDefinition(a) => Grippable::grips(a),
            EntityType::AttributeEntity(a) => Grippable::grips(a),
            EntityType::MLine(ml) => Grippable::grips(ml),
            EntityType::Tolerance(tol) => Grippable::grips(tol),
            EntityType::Solid(solid) => Grippable::grips(solid),
            EntityType::Face3D(f) => Grippable::grips(f),
            EntityType::PolygonMesh(pm) => Grippable::grips(pm),
            EntityType::PolyfaceMesh(pfm) => Grippable::grips(pfm),
            EntityType::Table(tbl) => Grippable::grips(tbl),
            EntityType::Point(pt) => Grippable::grips(pt),
            EntityType::Spline(spline) => Grippable::grips(spline),
            EntityType::Text(text) => Grippable::grips(text),
            EntityType::MText(text) => Grippable::grips(text),
            EntityType::Viewport(vp) => Grippable::grips(vp),
            EntityType::Insert(ins) => Grippable::grips(ins),
            EntityType::Leader(leader) => Grippable::grips(leader),
            EntityType::MultiLeader(ml) => Grippable::grips(ml),
            EntityType::Dimension(dim) => Grippable::grips(dim),
            EntityType::Hatch(hatch) => Grippable::grips(hatch),
            EntityType::Underlay(ul) => Grippable::grips(ul),
            EntityType::Shape(shp) => Grippable::grips(shp),
            EntityType::Ole2Frame(ole) => Grippable::grips(ole),
            _ => vec![],
        }
    }

    fn geometry_properties(&self, text_style_names: &[String]) -> Option<PropSection> {
        match self {
            EntityType::Line(line) => Some(PropertyEditable::geometry_properties(
                line,
                text_style_names,
            )),
            EntityType::Circle(circle) => Some(PropertyEditable::geometry_properties(
                circle,
                text_style_names,
            )),
            EntityType::Arc(arc) => {
                Some(PropertyEditable::geometry_properties(arc, text_style_names))
            }
            EntityType::Ellipse(ellipse) => Some(PropertyEditable::geometry_properties(
                ellipse,
                text_style_names,
            )),
            EntityType::LwPolyline(pline) => Some(PropertyEditable::geometry_properties(
                pline,
                text_style_names,
            )),
            EntityType::Polyline(pl) => Some(PropertyEditable::geometry_properties(
                pl,
                text_style_names,
            )),
            EntityType::Polyline2D(pl) => Some(PropertyEditable::geometry_properties(
                pl,
                text_style_names,
            )),
            EntityType::Polyline3D(pl) => Some(PropertyEditable::geometry_properties(
                pl,
                text_style_names,
            )),
            EntityType::Ray(ray) => Some(PropertyEditable::geometry_properties(
                ray,
                text_style_names,
            )),
            EntityType::XLine(xl) => Some(PropertyEditable::geometry_properties(
                xl,
                text_style_names,
            )),
            EntityType::RasterImage(img) => Some(PropertyEditable::geometry_properties(
                img,
                text_style_names,
            )),
            EntityType::Wipeout(wo) => Some(PropertyEditable::geometry_properties(
                wo,
                text_style_names,
            )),
            EntityType::AttributeDefinition(a) => Some(PropertyEditable::geometry_properties(
                a,
                text_style_names,
            )),
            EntityType::AttributeEntity(a) => Some(PropertyEditable::geometry_properties(
                a,
                text_style_names,
            )),
            EntityType::MLine(ml) => Some(PropertyEditable::geometry_properties(
                ml,
                text_style_names,
            )),
            EntityType::Tolerance(tol) => Some(PropertyEditable::geometry_properties(
                tol,
                text_style_names,
            )),
            EntityType::Solid(solid) => Some(PropertyEditable::geometry_properties(
                solid,
                text_style_names,
            )),
            EntityType::Face3D(f) => Some(PropertyEditable::geometry_properties(
                f,
                text_style_names,
            )),
            EntityType::PolygonMesh(pm) => Some(PropertyEditable::geometry_properties(
                pm,
                text_style_names,
            )),
            EntityType::PolyfaceMesh(pfm) => Some(PropertyEditable::geometry_properties(
                pfm,
                text_style_names,
            )),
            EntityType::Table(tbl) => Some(PropertyEditable::geometry_properties(
                tbl,
                text_style_names,
            )),
            EntityType::Hatch(hatch) => Some(PropertyEditable::geometry_properties(
                hatch,
                text_style_names,
            )),
            EntityType::Point(pt) => {
                Some(PropertyEditable::geometry_properties(pt, text_style_names))
            }
            EntityType::Spline(spline) => Some(PropertyEditable::geometry_properties(
                spline,
                text_style_names,
            )),
            EntityType::Text(text) => Some(PropertyEditable::geometry_properties(
                text,
                text_style_names,
            )),
            EntityType::MText(text) => Some(PropertyEditable::geometry_properties(
                text,
                text_style_names,
            )),
            EntityType::Viewport(vp) => {
                Some(PropertyEditable::geometry_properties(vp, text_style_names))
            }
            EntityType::Insert(ins) => {
                Some(PropertyEditable::geometry_properties(ins, text_style_names))
            }
            EntityType::Dimension(dim) => Some(PropertyEditable::geometry_properties(
                dim,
                text_style_names,
            )),
            EntityType::Leader(leader) => Some(PropertyEditable::geometry_properties(
                leader,
                text_style_names,
            )),
            EntityType::MultiLeader(ml) => Some(PropertyEditable::geometry_properties(
                ml,
                text_style_names,
            )),
            EntityType::Underlay(ul) => Some(PropertyEditable::geometry_properties(
                ul,
                text_style_names,
            )),
            EntityType::Shape(shp) => Some(PropertyEditable::geometry_properties(
                shp,
                text_style_names,
            )),
            EntityType::Ole2Frame(ole) => Some(PropertyEditable::geometry_properties(
                ole,
                text_style_names,
            )),
            _ => None,
        }
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        match self {
            EntityType::Line(line) => PropertyEditable::apply_geom_prop(line, field, value),
            EntityType::Circle(circle) => PropertyEditable::apply_geom_prop(circle, field, value),
            EntityType::Arc(arc) => PropertyEditable::apply_geom_prop(arc, field, value),
            EntityType::Ellipse(ellipse) => {
                PropertyEditable::apply_geom_prop(ellipse, field, value)
            }
            EntityType::LwPolyline(pline) => PropertyEditable::apply_geom_prop(pline, field, value),
            EntityType::Polyline(pl) => PropertyEditable::apply_geom_prop(pl, field, value),
            EntityType::Polyline2D(pl) => PropertyEditable::apply_geom_prop(pl, field, value),
            EntityType::Polyline3D(pl) => PropertyEditable::apply_geom_prop(pl, field, value),
            EntityType::Ray(ray) => PropertyEditable::apply_geom_prop(ray, field, value),
            EntityType::XLine(xl) => PropertyEditable::apply_geom_prop(xl, field, value),
            EntityType::RasterImage(img) => PropertyEditable::apply_geom_prop(img, field, value),
            EntityType::Wipeout(wo) => PropertyEditable::apply_geom_prop(wo, field, value),
            EntityType::AttributeDefinition(a) => PropertyEditable::apply_geom_prop(a, field, value),
            EntityType::AttributeEntity(a) => PropertyEditable::apply_geom_prop(a, field, value),
            EntityType::MLine(ml) => PropertyEditable::apply_geom_prop(ml, field, value),
            EntityType::Tolerance(tol) => PropertyEditable::apply_geom_prop(tol, field, value),
            EntityType::Solid(solid) => PropertyEditable::apply_geom_prop(solid, field, value),
            EntityType::Face3D(f) => PropertyEditable::apply_geom_prop(f, field, value),
            EntityType::PolygonMesh(pm) => PropertyEditable::apply_geom_prop(pm, field, value),
            EntityType::PolyfaceMesh(pfm) => PropertyEditable::apply_geom_prop(pfm, field, value),
            EntityType::Table(tbl) => PropertyEditable::apply_geom_prop(tbl, field, value),
            EntityType::Hatch(hatch) => PropertyEditable::apply_geom_prop(hatch, field, value),
            EntityType::Point(pt) => PropertyEditable::apply_geom_prop(pt, field, value),
            EntityType::Spline(spline) => PropertyEditable::apply_geom_prop(spline, field, value),
            EntityType::Text(text) => PropertyEditable::apply_geom_prop(text, field, value),
            EntityType::MText(text) => PropertyEditable::apply_geom_prop(text, field, value),
            EntityType::Viewport(vp) => PropertyEditable::apply_geom_prop(vp, field, value),
            EntityType::Insert(ins) => PropertyEditable::apply_geom_prop(ins, field, value),
            EntityType::Dimension(dim) => PropertyEditable::apply_geom_prop(dim, field, value),
            EntityType::Leader(leader) => PropertyEditable::apply_geom_prop(leader, field, value),
            EntityType::MultiLeader(ml) => PropertyEditable::apply_geom_prop(ml, field, value),
            EntityType::Underlay(ul) => PropertyEditable::apply_geom_prop(ul, field, value),
            EntityType::Shape(shp) => PropertyEditable::apply_geom_prop(shp, field, value),
            EntityType::Ole2Frame(ole) => PropertyEditable::apply_geom_prop(ole, field, value),
            _ => {}
        }
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        match self {
            EntityType::Line(line) => Grippable::apply_grip(line, grip_id, apply),
            EntityType::Circle(circle) => Grippable::apply_grip(circle, grip_id, apply),
            EntityType::Arc(arc) => Grippable::apply_grip(arc, grip_id, apply),
            EntityType::Ellipse(ellipse) => Grippable::apply_grip(ellipse, grip_id, apply),
            EntityType::LwPolyline(pline) => Grippable::apply_grip(pline, grip_id, apply),
            EntityType::Polyline(pl) => Grippable::apply_grip(pl, grip_id, apply),
            EntityType::Polyline2D(pl) => Grippable::apply_grip(pl, grip_id, apply),
            EntityType::Polyline3D(pl) => Grippable::apply_grip(pl, grip_id, apply),
            EntityType::Ray(ray) => Grippable::apply_grip(ray, grip_id, apply),
            EntityType::XLine(xl) => Grippable::apply_grip(xl, grip_id, apply),
            EntityType::RasterImage(img) => Grippable::apply_grip(img, grip_id, apply),
            EntityType::Wipeout(wo) => Grippable::apply_grip(wo, grip_id, apply),
            EntityType::AttributeDefinition(a) => Grippable::apply_grip(a, grip_id, apply),
            EntityType::AttributeEntity(a) => Grippable::apply_grip(a, grip_id, apply),
            EntityType::MLine(ml) => Grippable::apply_grip(ml, grip_id, apply),
            EntityType::Tolerance(tol) => Grippable::apply_grip(tol, grip_id, apply),
            EntityType::Solid(solid) => Grippable::apply_grip(solid, grip_id, apply),
            EntityType::Face3D(f) => Grippable::apply_grip(f, grip_id, apply),
            EntityType::PolygonMesh(pm) => Grippable::apply_grip(pm, grip_id, apply),
            EntityType::PolyfaceMesh(pfm) => Grippable::apply_grip(pfm, grip_id, apply),
            EntityType::Table(tbl) => Grippable::apply_grip(tbl, grip_id, apply),
            EntityType::Point(pt) => Grippable::apply_grip(pt, grip_id, apply),
            EntityType::Spline(spline) => Grippable::apply_grip(spline, grip_id, apply),
            EntityType::Text(text) => Grippable::apply_grip(text, grip_id, apply),
            EntityType::MText(text) => Grippable::apply_grip(text, grip_id, apply),
            EntityType::Viewport(vp) => Grippable::apply_grip(vp, grip_id, apply),
            EntityType::Insert(ins) => Grippable::apply_grip(ins, grip_id, apply),
            EntityType::Leader(leader) => Grippable::apply_grip(leader, grip_id, apply),
            EntityType::MultiLeader(ml) => Grippable::apply_grip(ml, grip_id, apply),
            EntityType::Dimension(dim) => Grippable::apply_grip(dim, grip_id, apply),
            EntityType::Hatch(hatch) => Grippable::apply_grip(hatch, grip_id, apply),
            EntityType::Underlay(ul) => Grippable::apply_grip(ul, grip_id, apply),
            EntityType::Shape(shp) => Grippable::apply_grip(shp, grip_id, apply),
            EntityType::Ole2Frame(ole) => Grippable::apply_grip(ole, grip_id, apply),
            _ => {}
        }
    }

    fn apply_transform(&mut self, t: &EntityTransform) {
        match self {
            EntityType::Arc(arc) => Transformable::apply_transform(arc, t),
            EntityType::Circle(circle) => Transformable::apply_transform(circle, t),
            EntityType::Ellipse(ellipse) => Transformable::apply_transform(ellipse, t),
            EntityType::Hatch(hatch) => Transformable::apply_transform(hatch, t),
            EntityType::Insert(ins) => Transformable::apply_transform(ins, t),
            EntityType::Line(line) => Transformable::apply_transform(line, t),
            EntityType::LwPolyline(pline) => Transformable::apply_transform(pline, t),
            EntityType::Polyline(pl) => Transformable::apply_transform(pl, t),
            EntityType::Polyline2D(pl) => Transformable::apply_transform(pl, t),
            EntityType::Polyline3D(pl) => Transformable::apply_transform(pl, t),
            EntityType::Ray(ray) => Transformable::apply_transform(ray, t),
            EntityType::XLine(xl) => Transformable::apply_transform(xl, t),
            EntityType::RasterImage(img) => Transformable::apply_transform(img, t),
            EntityType::Wipeout(wo) => Transformable::apply_transform(wo, t),
            EntityType::AttributeDefinition(a) => Transformable::apply_transform(a, t),
            EntityType::AttributeEntity(a) => Transformable::apply_transform(a, t),
            EntityType::MLine(ml) => Transformable::apply_transform(ml, t),
            EntityType::Tolerance(tol) => Transformable::apply_transform(tol, t),
            EntityType::Solid(solid) => Transformable::apply_transform(solid, t),
            EntityType::Face3D(f) => Transformable::apply_transform(f, t),
            EntityType::PolygonMesh(pm) => Transformable::apply_transform(pm, t),
            EntityType::PolyfaceMesh(pfm) => Transformable::apply_transform(pfm, t),
            EntityType::Table(tbl) => Transformable::apply_transform(tbl, t),
            EntityType::MText(text) => Transformable::apply_transform(text, t),
            EntityType::Point(pt) => Transformable::apply_transform(pt, t),
            EntityType::Spline(spline) => Transformable::apply_transform(spline, t),
            EntityType::Text(text) => Transformable::apply_transform(text, t),
            EntityType::Viewport(vp) => Transformable::apply_transform(vp, t),
            EntityType::Dimension(dim) => Transformable::apply_transform(dim, t),
            EntityType::Leader(leader) => Transformable::apply_transform(leader, t),
            EntityType::MultiLeader(ml) => Transformable::apply_transform(ml, t),
            EntityType::Underlay(ul) => Transformable::apply_transform(ul, t),
            EntityType::Shape(shp) => Transformable::apply_transform(shp, t),
            EntityType::Ole2Frame(ole) => Transformable::apply_transform(ole, t),
            _ => {}
        }
    }
}
