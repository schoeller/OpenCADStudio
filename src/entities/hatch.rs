use acadrust::entities::{BoundaryEdge, Hatch};
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{center_grip, edit_prop as edit, parse_f64, ro_prop as ro};
use crate::entities::traits::{FallbackTess, Grippable, PropertyEditable, Transformable};
use crate::scene::model::object::{GripApply, GripDef, PropSection, PropValue, Property};
use crate::scene::convert::tess_util::{arc_segments, arc_signed_span, wire_chord_tol, FallbackGeometry};
use crate::scene::model::wire_model::SnapHint;

fn properties(h: &Hatch) -> PropSection {
    let pattern_type = match h.pattern_type {
        acadrust::entities::HatchPatternType::Predefined => "Predefined",
        acadrust::entities::HatchPatternType::UserDefined => "User Defined",
        acadrust::entities::HatchPatternType::Custom => "Custom",
    };
    let style = match h.style {
        acadrust::entities::HatchStyleType::Normal => "Normal",
        acadrust::entities::HatchStyleType::Outer => "Outer",
        acadrust::entities::HatchStyleType::Ignore => "Ignore",
    };
    let fill_type = if h.gradient_color.enabled {
        format!("Gradient ({})", h.gradient_color.name)
    } else if h.is_solid {
        "Solid".into()
    } else {
        format!("Pattern ({})", h.pattern.name)
    };
    let boundary_count: usize = h
        .paths
        .iter()
        .map(|p| {
            p.edges
                .iter()
                .map(|e| match e {
                    BoundaryEdge::Polyline(poly) => poly.vertices.len(),
                    _ => 1,
                })
                .sum::<usize>()
        })
        .sum();
    PropSection {
        title: "Geometry".into(),
        props: vec![
            ro("Fill Type", "fill_type", fill_type),
            Property {
                label: "Pattern Name".into(),
                field: "pattern_name",
                value: PropValue::HatchPatternChoice(h.pattern.name.clone()),
            },
            ro("Pattern Type", "pattern_type", pattern_type),
            edit(
                "Pattern Angle",
                "pattern_angle",
                h.pattern_angle.to_degrees(),
            ),
            edit("Pattern Scale", "pattern_scale", h.pattern_scale),
            ro("Style", "style", style),
            ro("Boundary Paths", "path_count", h.paths.len().to_string()),
            ro("Boundary Verts", "vert_count", boundary_count.to_string()),
            ro("Double", "double", if h.is_double { "Yes" } else { "No" }),
            ro(
                "Associative",
                "associative",
                if h.is_associative { "Yes" } else { "No" },
            ),
            edit("Elevation", "elevation", h.elevation),
            ro("Seed Points", "seed_count", h.seed_points.len().to_string()),
            ro("Pixel Size", "pixel_size", format!("{:.6}", h.pixel_size)),
            ro(
                "Normal",
                "normal",
                format!("{:.3}, {:.3}, {:.3}", h.normal.x, h.normal.y, h.normal.z),
            ),
        ],
    }
}

fn apply_geom_prop(h: &mut Hatch, field: &str, value: &str) {
    let Some(v) = parse_f64(value) else {
        return;
    };
    match field {
        "pattern_angle" => h.pattern_angle = v.to_radians(),
        "pattern_scale" if v > 0.0 => h.pattern_scale = v,
        "elevation" => h.elevation = v,
        _ => {}
    }
}

fn apply_transform(h: &mut Hatch, t: &EntityTransform) {
    crate::scene::view::transform::apply_standard_entity_transform(h, t, |entity, p1, p2| {
        // Delegate the mirror to acadrust's transform_hatch (via the Entity
        // trait): it flips the boundary-arc direction flags, re-mirrors the
        // stored angles and preserves the stored sweep — including the
        // wrap-encoded end angles above 2π that AutoCAD writes. The old
        // hand-rolled angle-swap here was only valid for ccw boundary arcs on
        // an axis-aligned mirror line and went stale the moment those
        // conventions were fixed upstream.
        let t = crate::scene::view::transform::reflection_about_xy_line(p1, p2);
        acadrust::entities::Entity::apply_transform(entity, &t);
    });
}

impl PropertyEditable for Hatch {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        properties(self)
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        apply_geom_prop(self, field, value);
    }
}

impl Transformable for Hatch {
    fn apply_transform(&mut self, t: &EntityTransform) {
        apply_transform(self, t);
    }
}

// ── Grip editing ───────────────────────────────────────────────────────────

/// Assign sequential grip IDs across all boundary paths and edges.
/// Exposed control points per edge type:
///   Polyline       → each vertex (x, y)
///   Line           → start, end
///   CircularArc    → center
///   EllipticArc    → center
///   Spline         → fit points if present, else control points (x, y)
impl Grippable for Hatch {
    fn grips(&self) -> Vec<GripDef> {
        let elev = self.elevation;
        let mut out = Vec::new();
        let mut id = 0usize;
        for path in &self.paths {
            for edge in &path.edges {
                match edge {
                    BoundaryEdge::Polyline(p) => {
                        for v in &p.vertices {
                            out.push(center_grip(id, glam::DVec3::new(v.x, v.y, elev)));
                            id += 1;
                        }
                    }
                    BoundaryEdge::Line(l) => {
                        out.push(center_grip(
                            id,
                            glam::DVec3::new(l.start.x, l.start.y, elev),
                        ));
                        id += 1;
                        out.push(center_grip(id, glam::DVec3::new(l.end.x, l.end.y, elev)));
                        id += 1;
                    }
                    BoundaryEdge::CircularArc(a) => {
                        out.push(center_grip(
                            id,
                            glam::DVec3::new(a.center.x, a.center.y, elev),
                        ));
                        id += 1;
                    }
                    BoundaryEdge::EllipticArc(e) => {
                        out.push(center_grip(
                            id,
                            glam::DVec3::new(e.center.x, e.center.y, elev),
                        ));
                        id += 1;
                    }
                    BoundaryEdge::Spline(s) => {
                        let pts: Vec<[f64; 2]> = if !s.fit_points.is_empty() {
                            s.fit_points.iter().map(|p| [p.x, p.y]).collect()
                        } else {
                            s.control_points.iter().map(|p| [p.x, p.y]).collect()
                        };
                        for [x, y] in pts {
                            out.push(center_grip(id, glam::DVec3::new(x, y, elev)));
                            id += 1;
                        }
                    }
                }
            }
        }
        out
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        let elev = self.elevation as f32;
        let mut id = 0usize;

        fn resolve(apply: &GripApply, cur: Vec3) -> (f64, f64) {
            let p = match apply {
                GripApply::Absolute(p) => *p,
                GripApply::Translate(d) => cur + *d,
            };
            (p.x as f64, p.y as f64)
        }

        'outer: for path in &mut self.paths {
            for edge in &mut path.edges {
                match edge {
                    BoundaryEdge::Polyline(p) => {
                        for v in &mut p.vertices {
                            if id == grip_id {
                                let (nx, ny) =
                                    resolve(&apply, Vec3::new(v.x as f32, v.y as f32, elev));
                                v.x = nx;
                                v.y = ny;
                                break 'outer;
                            }
                            id += 1;
                        }
                    }
                    BoundaryEdge::Line(l) => {
                        if id == grip_id {
                            let (nx, ny) = resolve(
                                &apply,
                                Vec3::new(l.start.x as f32, l.start.y as f32, elev),
                            );
                            l.start.x = nx;
                            l.start.y = ny;
                            break 'outer;
                        }
                        id += 1;
                        if id == grip_id {
                            let (nx, ny) =
                                resolve(&apply, Vec3::new(l.end.x as f32, l.end.y as f32, elev));
                            l.end.x = nx;
                            l.end.y = ny;
                            break 'outer;
                        }
                        id += 1;
                    }
                    BoundaryEdge::CircularArc(a) => {
                        if id == grip_id {
                            let (nx, ny) = resolve(
                                &apply,
                                Vec3::new(a.center.x as f32, a.center.y as f32, elev),
                            );
                            a.center.x = nx;
                            a.center.y = ny;
                            break 'outer;
                        }
                        id += 1;
                    }
                    BoundaryEdge::EllipticArc(e) => {
                        if id == grip_id {
                            let (nx, ny) = resolve(
                                &apply,
                                Vec3::new(e.center.x as f32, e.center.y as f32, elev),
                            );
                            e.center.x = nx;
                            e.center.y = ny;
                            break 'outer;
                        }
                        id += 1;
                    }
                    BoundaryEdge::Spline(s) => {
                        if !s.fit_points.is_empty() {
                            for fp in &mut s.fit_points {
                                if id == grip_id {
                                    let (nx, ny) =
                                        resolve(&apply, Vec3::new(fp.x as f32, fp.y as f32, elev));
                                    fp.x = nx;
                                    fp.y = ny;
                                    break 'outer;
                                }
                                id += 1;
                            }
                        } else {
                            for cp in &mut s.control_points {
                                if id == grip_id {
                                    let (nx, ny) =
                                        resolve(&apply, Vec3::new(cp.x as f32, cp.y as f32, elev));
                                    cp.x = nx;
                                    cp.y = ny;
                                    break 'outer;
                                }
                                id += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    fn grip_menu(&self, _grip_id: usize) -> Vec<crate::scene::model::object::GripMenuItem> {
        use crate::scene::model::object::{GripMenuAction, GripMenuItem};
        vec![
            GripMenuItem {
                label: "Stretch",
                action: GripMenuAction::Stretch,
            },
            GripMenuItem {
                label: "Origin Point",
                action: GripMenuAction::OriginPoint,
            },
            GripMenuItem {
                label: "Hatch Angle",
                action: GripMenuAction::HatchAngle,
            },
            GripMenuItem {
                label: "Hatch Scale",
                action: GripMenuAction::HatchScale,
            },
        ]
    }

    fn apply_grip_menu(&mut self, _grip_id: usize, _action: crate::scene::model::object::GripMenuAction) {
        // Origin / Angle / Scale need a follow-up value — handled by
        // `apply_grip_menu_value`.
    }

    fn grip_menu_value_prompt(
        &self,
        _grip_id: usize,
        action: crate::scene::model::object::GripMenuAction,
    ) -> Option<&'static str> {
        use crate::scene::model::object::GripMenuAction as A;
        match action {
            A::HatchAngle => Some("Angle (deg)"),
            A::HatchScale => Some("Scale"),
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
        match action {
            A::HatchAngle => {
                self.pattern_angle = value.to_radians();
            }
            A::HatchScale => {
                if value > 0.0 {
                    self.pattern_scale = value;
                }
            }
            _ => {}
        }
    }
}

impl FallbackTess for Hatch {
    fn fallback_geometry(&self, world_offset: [f64; 3]) -> FallbackGeometry {
        let [ox, oy, oz] = world_offset;
        let normal = (self.normal.x, self.normal.y, self.normal.z);
        // Convert a 2D OCS hatch boundary point to WCS, then subtract world_offset.
        let to_wcs = |x: f64, y: f64| -> [f32; 3] {
            let (wx, wy, wz) =
                crate::scene::view::transform::ocs_point_to_wcs((x, y, self.elevation), normal);
            [(wx - ox) as f32, (wy - oy) as f32, (wz - oz) as f32]
        };
        let mut pts: Vec<[f32; 3]> = Vec::new();
        let mut key_verts: Vec<[f32; 3]> = Vec::new();
        let mut snap_pts: Vec<(Vec3, SnapHint)> = Vec::new();
        for path in &self.paths {
            for edge in &path.edges {
                match edge {
                    BoundaryEdge::Polyline(poly) => {
                        // Hatch-boundary polyline vertices encode bulge in
                        // `Vector3.z`; straight segments emit just the
                        // start vertex, bulged segments tessellate the arc
                        // between v0 → v1.
                        let verts = &poly.vertices;
                        let count = verts.len();
                        if count == 0 {
                            continue;
                        }
                        // Break the wire between this polyline and whatever
                        // preceded it — without the separator the renderer
                        // draws a ghost segment from the previous edge / path
                        // straight to this polyline's first vertex, which
                        // shows up as a stray boundary line between hatch
                        // regions.
                        if !pts.is_empty() {
                            pts.push([f32::NAN; 3]);
                        }
                        let start_idx = pts.len();
                        let seg_count = if poly.is_closed {
                            count
                        } else {
                            count.saturating_sub(1)
                        };
                        for i in 0..seg_count {
                            let v0 = &verts[i];
                            let v1 = &verts[(i + 1) % count];
                            let bulge = v0.z;
                            let arc = if bulge.abs() < 1e-9 {
                                None
                            } else {
                                crate::entities::common::BulgeArc::from_bulge(
                                    [v0.x, v0.y],
                                    [v1.x, v1.y],
                                    bulge,
                                )
                            };
                            let Some(arc) = arc else {
                                let p = to_wcs(v0.x, v0.y);
                                pts.push(p);
                                key_verts.push(p);
                                continue;
                            };
                            let segs = arc_segments(
                                arc.radius,
                                arc.sweep.abs(),
                                wire_chord_tol(arc.radius),
                            );
                            for j in 0..segs {
                                let s = arc.sample(j as f64 / segs as f64);
                                let p = to_wcs(s[0], s[1]);
                                pts.push(p);
                                if j == 0 {
                                    key_verts.push(p);
                                }
                            }
                        }
                        // Close the loop visually for closed polylines by
                        // returning to the first emitted point.
                        if poly.is_closed {
                            if let Some(first) = pts.get(start_idx).cloned() {
                                if first[0].is_finite() {
                                    pts.push(first);
                                }
                            }
                        } else if let Some(last) = verts.last() {
                            let p = to_wcs(last.x, last.y);
                            pts.push(p);
                            key_verts.push(p);
                        }
                    }
                    BoundaryEdge::Line(ln) => {
                        let p0 = to_wcs(ln.start.x, ln.start.y);
                        let p1 = to_wcs(ln.end.x, ln.end.y);
                        if !pts.is_empty() {
                            pts.push([f32::NAN; 3]);
                        }
                        pts.push(p0);
                        pts.push(p1);
                        key_verts.push(p0);
                        key_verts.push(p1);
                    }
                    BoundaryEdge::CircularArc(arc) => {
                        let (sa, span) =
                            arc_signed_span(arc.start_angle, arc.end_angle, arc.counter_clockwise);
                        let segs = arc_segments(arc.radius, span.abs(), wire_chord_tol(arc.radius));
                        if !pts.is_empty() {
                            pts.push([f32::NAN; 3]);
                        }
                        for i in 0..=segs {
                            let t = sa + span * (i as f64 / segs as f64);
                            let p = to_wcs(
                                arc.center.x + arc.radius * t.cos(),
                                arc.center.y + arc.radius * t.sin(),
                            );
                            pts.push(p);
                            if i == 0 || i == segs {
                                key_verts.push(p);
                            }
                        }
                        snap_pts.push((
                            Vec3::from(to_wcs(arc.center.x, arc.center.y)),
                            SnapHint::Center,
                        ));
                    }
                    BoundaryEdge::EllipticArc(ell) => {
                        let r_maj = (ell.major_axis_endpoint.x * ell.major_axis_endpoint.x
                            + ell.major_axis_endpoint.y * ell.major_axis_endpoint.y)
                            .sqrt();
                        let r_min = r_maj * ell.minor_axis_ratio;
                        let rot = ell.major_axis_endpoint.y.atan2(ell.major_axis_endpoint.x);
                        let (sa, span) =
                            arc_signed_span(ell.start_angle, ell.end_angle, ell.counter_clockwise);
                        let segs = arc_segments(r_maj, span.abs(), wire_chord_tol(r_maj));
                        if !pts.is_empty() {
                            pts.push([f32::NAN; 3]);
                        }
                        let (cr, sr) = (rot.cos(), rot.sin());
                        for i in 0..=segs {
                            let t = sa + span * (i as f64 / segs as f64);
                            let lx = r_maj * t.cos();
                            let ly = r_min * t.sin();
                            let p = to_wcs(
                                ell.center.x + lx * cr - ly * sr,
                                ell.center.y + lx * sr + ly * cr,
                            );
                            pts.push(p);
                            if i == 0 || i == segs {
                                key_verts.push(p);
                            }
                        }
                        snap_pts.push((
                            Vec3::from(to_wcs(ell.center.x, ell.center.y)),
                            SnapHint::Center,
                        ));
                    }
                    _ => {}
                }
            }
        }
        if pts.is_empty() {
            pts = vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
        }
        (pts, snap_pts, vec![], key_verts)
    }
}
