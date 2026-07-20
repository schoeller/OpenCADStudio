use acadrust::entities::{LwPolyline, LwVertex};
use truck_modeling::{builder, Edge, Point3, Wire};

use crate::command::EntityTransform;
use crate::entities::common::{
    edit_prop as edit, parse_f64, rectangle_grip, ro_prop as ro, square_grip,
    stepper_prop as stepper,
};
use crate::entities::traits::TruckConvertible;
use crate::scene::convert::acad_to_truck::{extrusion_wall_tris, TruckEntity, TruckObject};
use crate::scene::model::object::{GripApply, GripDef, PropSection, PropValue, Property};
use crate::scene::model::wire_model::TangentGeom;

const TAU: f64 = std::f64::consts::TAU;

/// Midpoint position on an arc segment defined by its bulge.
fn arc_midpoint(p0: [f64; 2], p1: [f64; 2], bulge: f64) -> [f64; 2] {
    match crate::entities::common::BulgeArc::from_bulge(p0, p1, bulge) {
        Some(arc) => arc.sample(0.5),
        None => [(p0[0] + p1[0]) * 0.5, (p0[1] + p1[1]) * 0.5],
    }
}

/// Compute the DXF bulge for an arc that passes through p0, mid_pt, and p1.
/// Returns None when the three points are collinear (straight segment).
fn bulge_from_midpoint(p0: [f64; 2], p1: [f64; 2], mid: [f64; 2]) -> Option<f64> {
    // Circumcircle of (p0, mid, p1)
    let ax = 2.0 * (mid[0] - p0[0]);
    let ay = 2.0 * (mid[1] - p0[1]);
    let bx = 2.0 * (p1[0] - p0[0]);
    let by = 2.0 * (p1[1] - p0[1]);
    let ca = mid[0] * mid[0] + mid[1] * mid[1] - p0[0] * p0[0] - p0[1] * p0[1];
    let cb = p1[0] * p1[0] + p1[1] * p1[1] - p0[0] * p0[0] - p0[1] * p0[1];
    let det = ax * by - ay * bx;
    if det.abs() < 1e-12 {
        return None; // collinear
    }
    let cx = (ca * by - cb * ay) / det;
    let cy = (ax * cb - bx * ca) / det;
    let a0 = (p0[1] - cy).atan2(p0[0] - cx);
    let a1 = (p1[1] - cy).atan2(p1[0] - cx);
    // Determine arc direction: cross product (p1-p0) × (mid-p0)
    let cross = (p1[0] - p0[0]) * (mid[1] - p0[1]) - (p1[1] - p0[1]) * (mid[0] - p0[0]);
    let (sa, mut ea) = if cross > 0.0 { (a0, a1) } else { (a1, a0) };
    if ea < sa {
        ea += TAU;
    }
    let span = ea - sa; // central angle in (0, TAU]
    let bulge = (span / 4.0).tan();
    Some(if cross >= 0.0 { bulge } else { -bulge })
}

/// Tessellate a thick polyline segment list into NaN-separated Lines geometry.
/// Shared by LwPolyline and Polyline2D thickness paths.
fn thick_segments(
    seg_data: &[(f64, f64, f64, f64)], // (x0, y0, x1, y1) per seg — or use run of (x,y,bulge)
    path_pts: &[[f64; 3]],
    thickness: f64,
    normal: (f64, f64, f64),
    key_verts: Vec<[f64; 3]>,
    tangents: Vec<TangentGeom>,
) -> TruckEntity {
    let (nx, ny, nz) = normal;
    let t = thickness;
    let off = |p: [f64; 3]| -> [f64; 3] { [p[0] + t * nx, p[1] + t * ny, p[2] + t * nz] };
    let mut pts: Vec<[f64; 3]> = Vec::with_capacity(path_pts.len() * 2 + seg_data.len() * 3 + 4);
    // Bottom path
    pts.extend_from_slice(path_pts);
    pts.push([f64::NAN; 3]);
    // Top path
    for &p in path_pts {
        pts.push(off(p));
    }
    // Walls at each vertex (seg_data.0/.1 = start x/y of each seg, last seg appends its end too)
    if !seg_data.is_empty() {
        pts.push([f64::NAN; 3]);
        for (k, &(x0, y0, _x1, _y1)) in seg_data.iter().enumerate() {
            let pb = key_verts[k];
            let _ = (x0, y0); // key_verts already has correct WCS
            pts.push(pb);
            pts.push(off(pb));
            if k + 1 < seg_data.len() {
                pts.push([f64::NAN; 3]);
            }
        }
        // Last wall at the final vertex
        if let Some(&last) = key_verts.last() {
            pts.push([f64::NAN; 3]);
            pts.push(last);
            pts.push(off(last));
        }
    }
    TruckEntity {
        pick_tris: extrusion_wall_tris(path_pts, [t * nx, t * ny, t * nz]),
        object: TruckObject::Lines(pts),
        snap_pts: vec![],
        tangent_geoms: tangents,
        key_vertices: key_verts,
        fill_tris: vec![],
    }
}

/// Extrude a *wide* LwPolyline's band into a 3-D tube: each band segment's outer
/// and inner boundary rises by `thickness` (along the normal) into a wall,
/// leaving the two radial segment-end edges open (they are internal to the
/// band). The flat bottom band is already drawn by the wide-fill path; this
/// adds the vertical walls and their outline. Used instead of `thick_segments`
/// (which extrudes only the centreline) when the polyline carries a width.
fn thick_wide_band(
    pl: &LwPolyline,
    thickness: f64,
    to_wcs: &dyn Fn(f64, f64) -> (f64, f64, f64),
    normal: (f64, f64, f64),
    key_verts: Vec<[f64; 3]>,
    tangents: Vec<TangentGeom>,
) -> TruckEntity {
    let (origin, fills) = wide_fills(pl);
    let (fill_tris, lines) =
        crate::entities::common::thick_band_tube(origin, &fills, thickness, normal, to_wcs);

    TruckEntity {
        pick_tris: fill_tris.clone(),
        object: TruckObject::Lines(lines),
        snap_pts: vec![],
        tangent_geoms: tangents,
        key_vertices: key_verts,
        fill_tris,
    }
}

fn to_truck(pline: &LwPolyline) -> TruckEntity {
    let verts = &pline.vertices;
    if verts.is_empty() {
        return TruckEntity {
            pick_tris: Vec::new(),
            object: TruckObject::Point(builder::vertex(Point3::new(0.0, 0.0, 0.0))),
            snap_pts: vec![],
            tangent_geoms: vec![],
            key_vertices: vec![],
            fill_tris: vec![],
        };
    }

    let elev = pline.elevation;
    let normal = (pline.normal.x, pline.normal.y, pline.normal.z);
    let count = verts.len();
    let seg_count = if pline.is_closed { count } else { count - 1 };
    let mut edges: Vec<Edge> = Vec::new();
    let mut tangents: Vec<TangentGeom> = Vec::new();
    let mut key_verts: Vec<[f64; 3]> = Vec::new();

    // Convert OCS (x, y, elevation) to WCS Point3.
    let to_wcs = |x: f64, y: f64| -> (f64, f64, f64) {
        crate::scene::view::transform::ocs_point_to_wcs((x, y, elev), normal)
    };
    let to_pt = |v: &LwVertex| -> Point3 {
        let (wx, wy, wz) = to_wcs(v.location.x, v.location.y);
        Point3::new(wx, wy, wz)
    };

    if pline.thickness.abs() > 1e-10 {
        let mut path: Vec<[f64; 3]> = Vec::new();
        let mut kv: Vec<[f64; 3]> = Vec::new();
        let mut tgs: Vec<TangentGeom> = Vec::new();
        let mut seg_data: Vec<(f64, f64, f64, f64)> = Vec::new();
        // First vertex
        let (w0x, w0y, w0z) = to_wcs(verts[0].location.x, verts[0].location.y);
        path.push([w0x, w0y, w0z]);
        kv.push([w0x, w0y, w0z]);
        for i in 0..seg_count {
            let va = &verts[i];
            let vb = &verts[(i + 1) % count];
            let (ox0, oy0) = (va.location.x, va.location.y);
            let (ox1, oy1) = (vb.location.x, vb.location.y);
            let bulge = va.bulge;
            if bulge.abs() < 1e-9 {
                let (wx, wy, wz) = to_wcs(ox1, oy1);
                path.push([wx, wy, wz]);
                let p1_pt = path[path.len() - 2];
                let p2_pt = *path.last().unwrap();
                tgs.push(TangentGeom::Line {
                    p1: [p1_pt[0] as f32, p1_pt[1] as f32, p1_pt[2] as f32],
                    p2: [p2_pt[0] as f32, p2_pt[1] as f32, p2_pt[2] as f32],
                });
            } else if let Some(arc) =
                crate::entities::common::BulgeArc::from_bulge([ox0, oy0], [ox1, oy1], bulge)
            {
                let (wcx, wcy, wcz) = to_wcs(arc.center[0], arc.center[1]);
                tgs.push(TangentGeom::Circle {
                    center: [wcx as f32, wcy as f32, wcz as f32],
                    radius: arc.radius as f32,
                });
                for j in 1..=16usize {
                    let s = arc.sample(j as f64 / 16.0);
                    let (wx, wy, wz) = to_wcs(s[0], s[1]);
                    path.push([wx, wy, wz]);
                }
            }
            let (wbx, wby, wbz) = to_wcs(ox1, oy1);
            kv.push([wbx, wby, wbz]);
            seg_data.push((ox0, oy0, ox1, oy1));
        }
        // A wide polyline extrudes its whole band into a tube (outer + inner
        // walls); a zero-width one just extrudes its centreline.
        let is_wide = pline.constant_width > 1e-9
            || pline
                .vertices
                .iter()
                .any(|v| v.start_width > 1e-9 || v.end_width > 1e-9);
        if is_wide {
            return thick_wide_band(pline, pline.thickness, &to_wcs, normal, kv, tgs);
        }
        return thick_segments(&seg_data, &path, pline.thickness, normal, kv, tgs);
    }

    // plinegen=false: NaN-separated segments so the linetype pattern restarts per vertex.
    if !pline.plinegen {
        let mut pts: Vec<[f64; 3]> = Vec::new();
        let mut tgs: Vec<TangentGeom> = Vec::new();
        let mut kv: Vec<[f64; 3]> = Vec::new();
        let to_f32 = |p: [f64; 3]| -> [f32; 3] { [p[0] as f32, p[1] as f32, p[2] as f32] };
        for i in 0..seg_count {
            let va = &verts[i];
            let vb = &verts[(i + 1) % count];
            let (ox0, oy0) = (va.location.x, va.location.y);
            let (ox1, oy1) = (vb.location.x, vb.location.y);
            let bulge = va.bulge;
            let (wx0, wy0, wz0) = to_wcs(ox0, oy0);
            let p_start = [wx0, wy0, wz0];
            pts.push(p_start);
            if i == 0 {
                kv.push(p_start);
            }
            if bulge.abs() < 1e-9 {
                let (wx1, wy1, wz1) = to_wcs(ox1, oy1);
                let p_end = [wx1, wy1, wz1];
                pts.push(p_end);
                kv.push(p_end);
                tgs.push(TangentGeom::Line {
                    p1: to_f32(p_start),
                    p2: to_f32(p_end),
                });
            } else if let Some(arc) =
                crate::entities::common::BulgeArc::from_bulge([ox0, oy0], [ox1, oy1], bulge)
            {
                for j in 1..=16usize {
                    let s = arc.sample(j as f64 / 16.0);
                    let (wx, wy, wz) = to_wcs(s[0], s[1]);
                    pts.push([wx, wy, wz]);
                }
                let (wx1, wy1, wz1) = to_wcs(ox1, oy1);
                kv.push([wx1, wy1, wz1]);
                let (wcx, wcy, wcz) = to_wcs(arc.center[0], arc.center[1]);
                tgs.push(TangentGeom::Circle {
                    center: [wcx as f32, wcy as f32, wcz as f32],
                    radius: arc.radius as f32,
                });
            }
            if i + 1 < seg_count {
                pts.push([f64::NAN; 3]);
            }
        }
        let (fill_origin, fills) = wide_fills(pline);
        return TruckEntity {
            pick_tris: crate::entities::common::wide_band_tris(fill_origin, &fills),
            object: TruckObject::SegmentedLines(pts),
            snap_pts: vec![],
            tangent_geoms: tgs,
            key_vertices: kv,
            fill_tris: vec![],
        };
    }

    for i in 0..seg_count {
        let v0 = &verts[i];
        let v1 = &verts[(i + 1) % count];
        let p0 = to_pt(v0);
        let p1 = to_pt(v1);
        let bulge = v0.bulge;

        if bulge.abs() < 1e-9 {
            let tv0 = builder::vertex(p0);
            let tv1 = builder::vertex(p1);
            edges.push(builder::line(&tv0, &tv1));
            tangents.push(TangentGeom::Line {
                p1: [p0.x as f32, p0.y as f32, p0.z as f32],
                p2: [p1.x as f32, p1.y as f32, p1.z as f32],
            });
        } else if let Some(arc) = crate::entities::common::BulgeArc::from_bulge(
            [v0.location.x, v0.location.y],
            [v1.location.x, v1.location.y],
            bulge as f64,
        ) {
            let mid_s = arc.sample(0.5);
            let (mid_wx, mid_wy, mid_wz) = to_wcs(mid_s[0], mid_s[1]);
            let p_mid = Point3::new(mid_wx, mid_wy, mid_wz);
            let tv0 = builder::vertex(p0);
            let tv1 = builder::vertex(p1);
            edges.push(builder::circle_arc(&tv0, &tv1, p_mid));
            let (wcx, wcy, wcz) = to_wcs(arc.center[0], arc.center[1]);
            tangents.push(TangentGeom::Circle {
                center: [wcx as f32, wcy as f32, wcz as f32],
                radius: arc.radius as f32,
            });
        }

        if i == 0 {
            key_verts.push([p0.x, p0.y, p0.z]);
        }
        key_verts.push([p1.x, p1.y, p1.z]);
    }

    let (fill_origin, fills) = wide_fills(pline);
    TruckEntity {
        pick_tris: crate::entities::common::wide_band_tris(fill_origin, &fills),
        object: TruckObject::Contour(edges.into_iter().collect::<Wire>()),
        snap_pts: vec![],
        tangent_geoms: tangents,
        key_vertices: key_verts,
        fill_tris: vec![],
    }
}

fn grips(pline: &LwPolyline) -> Vec<GripDef> {
    let elev = pline.elevation;
    let n = pline.vertices.len();
    let seg_count = if pline.is_closed {
        n
    } else {
        n.saturating_sub(1)
    };

    let mut out: Vec<GripDef> = pline
        .vertices
        .iter()
        .enumerate()
        .map(|(i, v)| square_grip(i, glam::DVec3::new(v.location.x, v.location.y, elev)))
        .collect();

    // One mid-segment stretch grip per segment (straight or arc). The
    // marker is a small box rotated along the chord direction so the
    // shape itself signals which way the segment runs.
    for i in 0..seg_count {
        let v0 = &pline.vertices[i];
        let v1 = &pline.vertices[(i + 1) % n];
        let (mx, my) = if v0.bulge.abs() < 1e-9 {
            (
                (v0.location.x + v1.location.x) * 0.5,
                (v0.location.y + v1.location.y) * 0.5,
            )
        } else {
            let m = arc_midpoint(
                [v0.location.x, v0.location.y],
                [v1.location.x, v1.location.y],
                v0.bulge,
            );
            (m[0], m[1])
        };
        let dx = (v1.location.x - v0.location.x) as f32;
        let dy = (v1.location.y - v0.location.y) as f32;
        out.push(rectangle_grip(
            n + i,
            glam::DVec3::new(mx, my, elev),
            [dx, dy],
        ));
    }
    out
}

fn properties(pline: &LwPolyline) -> Vec<PropSection> {
    let n = pline.vertices.len();
    // The panel's Current Vertex focus, clamped to this polyline's range.
    let vi = if n == 0 {
        0
    } else {
        crate::scene::view::dispatch::prop_current_vertex().min(n - 1)
    };
    let v = pline.vertices.get(vi);
    let vx = v.map_or(0.0, |v| v.location.x);
    let vy = v.map_or(0.0, |v| v.location.y);
    let start_w = v.map_or(0.0, |v| v.start_width);
    let end_w = v.map_or(0.0, |v| v.end_width);
    let mp = <LwPolyline as crate::entities::traits::MassPropsCalc>::mass_props(pline);
    let vertex_label = if n == 0 {
        "—".to_string()
    } else {
        format!("{} / {}", vi + 1, n)
    };
    vec![
        PropSection {
            title: "Geometry".into(),
            props: vec![
                stepper("Current Vertex", "current_vertex", vertex_label),
                edit("Vertex X", "vertex_x", vx),
                edit("Vertex Y", "vertex_y", vy),
                edit("Start segment width", "start_width", start_w),
                edit("End segment width", "end_width", end_w),
                edit("Global width", "global_width", pline.constant_width),
                edit("Elevation", "elevation", pline.elevation),
                ro("Area", "area", format!("{:.4}", mp.area)),
                ro("Length", "length", format!("{:.4}", mp.perimeter)),
            ],
        },
        PropSection {
            title: "Misc".into(),
            props: vec![
                Property {
                    label: "Closed".into(),
                    field: "closed",
                    value: PropValue::BoolToggle {
                        field: "closed",
                        value: pline.is_closed,
                    },
                },
                Property {
                    label: "Linetype generation".into(),
                    field: "plinegen",
                    value: PropValue::BoolToggle {
                        field: "plinegen",
                        value: pline.plinegen,
                    },
                },
            ],
        },
    ]
}

fn apply_geom_prop(pline: &mut LwPolyline, field: &str, value: &str) {
    match field {
        "closed" => {
            pline.is_closed = if value == "toggle" {
                !pline.is_closed
            } else {
                value == "true"
            };
            return;
        }
        "plinegen" => {
            pline.plinegen = if value == "toggle" {
                !pline.plinegen
            } else {
                value == "true"
            };
            return;
        }
        _ => {}
    }
    let Some(v) = parse_f64(value) else {
        return;
    };
    // Per-vertex edits target the vertex the panel is focused on.
    let n = pline.vertices.len();
    let vi = if n == 0 {
        0
    } else {
        crate::scene::view::dispatch::prop_current_vertex().min(n - 1)
    };
    match field {
        "elevation" => pline.elevation = v,
        "global_width" => pline.constant_width = v,
        "vertex_x" => {
            if let Some(vtx) = pline.vertices.get_mut(vi) {
                vtx.location.x = v;
            }
        }
        "vertex_y" => {
            if let Some(vtx) = pline.vertices.get_mut(vi) {
                vtx.location.y = v;
            }
        }
        "start_width" => {
            if let Some(vtx) = pline.vertices.get_mut(vi) {
                vtx.start_width = v;
            }
        }
        "end_width" => {
            if let Some(vtx) = pline.vertices.get_mut(vi) {
                vtx.end_width = v;
            }
        }
        _ => {}
    }
}

fn apply_grip(pline: &mut LwPolyline, grip_id: usize, apply: GripApply) {
    let n = pline.vertices.len();
    if grip_id < n {
        // Vertex position grip
        let v = &mut pline.vertices[grip_id];
        match apply {
            GripApply::Absolute(p) => {
                v.location.x = p.x as f64;
                v.location.y = p.y as f64;
            }
            GripApply::Translate(d) => {
                v.location.x += d.x as f64;
                v.location.y += d.y as f64;
            }
        }
    } else {
        // Mid-segment stretch grip for segment (grip_id - n).
        // Straight segments translate both endpoints by the drag delta
        // (the shared vertices then carry along whichever adjacent
        // segments share them). Arc segments adjust their bulge from
        // the new midpoint position.
        let seg = grip_id - n;
        let count = if pline.is_closed {
            n
        } else {
            n.saturating_sub(1)
        };
        if seg >= count {
            return;
        }
        let i0 = seg;
        let i1 = (seg + 1) % n;
        let is_arc = pline.vertices[i0].bulge.abs() >= 1e-9;
        if !is_arc {
            let d = match apply {
                GripApply::Translate(d) => [d.x as f64, d.y as f64],
                GripApply::Absolute(p) => {
                    let old_mid = (
                        (pline.vertices[i0].location.x + pline.vertices[i1].location.x) * 0.5,
                        (pline.vertices[i0].location.y + pline.vertices[i1].location.y) * 0.5,
                    );
                    [p.x as f64 - old_mid.0, p.y as f64 - old_mid.1]
                }
            };
            pline.vertices[i0].location.x += d[0];
            pline.vertices[i0].location.y += d[1];
            pline.vertices[i1].location.x += d[0];
            pline.vertices[i1].location.y += d[1];
            return;
        }
        let new_mid: [f64; 2] = match apply {
            GripApply::Absolute(p) => [p.x as f64, p.y as f64],
            GripApply::Translate(d) => {
                let v0 = &pline.vertices[i0];
                let v1 = &pline.vertices[i1];
                let old = arc_midpoint(
                    [v0.location.x, v0.location.y],
                    [v1.location.x, v1.location.y],
                    v0.bulge,
                );
                [old[0] + d.x as f64, old[1] + d.y as f64]
            }
        };
        let p0 = [pline.vertices[i0].location.x, pline.vertices[i0].location.y];
        let p1 = [pline.vertices[i1].location.x, pline.vertices[i1].location.y];
        if let Some(new_bulge) = bulge_from_midpoint(p0, p1, new_mid) {
            pline.vertices[i0].bulge = new_bulge.clamp(-1e6, 1e6);
        }
    }
}

fn apply_transform(pline: &mut LwPolyline, t: &EntityTransform) {
    crate::scene::view::transform::apply_standard_entity_transform(pline, t, |entity, p1, p2| {
        for v in &mut entity.vertices {
            crate::scene::view::transform::reflect_xy_point(&mut v.location.x, &mut v.location.y, p1, p2);
            // Bulge encodes which side the arc bows to; a reflection
            // reverses it or every curved segment flips to the wrong side.
            v.bulge = -v.bulge;
        }
    });
}

impl TruckConvertible for LwPolyline {
    fn to_truck(&self, _document: &acadrust::CadDocument) -> Option<TruckEntity> {
        Some(to_truck(self))
    }
}

impl crate::entities::traits::Grippable for LwPolyline {
    fn grips(&self) -> Vec<crate::scene::model::object::GripDef> {
        grips(self)
    }
    fn apply_grip(&mut self, grip_id: usize, apply: crate::scene::model::object::GripApply) {
        apply_grip(self, grip_id, apply);
    }
    fn grip_menu(&self, grip_id: usize) -> Vec<crate::scene::model::object::GripMenuItem> {
        use crate::scene::model::object::{GripMenuAction, GripMenuItem};
        let n = self.vertices.len();
        if grip_id < n {
            // Vertex grip.
            return vec![
                GripMenuItem {
                    label: "Stretch",
                    action: GripMenuAction::Stretch,
                },
                GripMenuItem {
                    label: "Add Vertex",
                    action: GripMenuAction::AddVertex,
                },
                GripMenuItem {
                    label: "Remove Vertex",
                    action: GripMenuAction::RemoveVertex,
                },
            ];
        }
        // Segment midpoint grip.
        let seg = grip_id - n;
        let is_arc = self
            .vertices
            .get(seg)
            .map_or(false, |v| v.bulge.abs() >= 1e-9);
        let convert = if is_arc {
            GripMenuItem {
                label: "Convert to Line",
                action: GripMenuAction::ConvertToLine,
            }
        } else {
            GripMenuItem {
                label: "Convert to Arc",
                action: GripMenuAction::ConvertToArc,
            }
        };
        vec![
            GripMenuItem {
                label: "Stretch",
                action: GripMenuAction::Stretch,
            },
            GripMenuItem {
                label: "Add Vertex",
                action: GripMenuAction::AddVertex,
            },
            convert,
        ]
    }
    fn apply_grip_menu(&mut self, grip_id: usize, action: crate::scene::model::object::GripMenuAction) {
        use crate::scene::model::object::GripMenuAction as A;
        let n = self.vertices.len();
        match action {
            A::Stretch => {}
            A::AddVertex => {
                // Insert a new vertex at the chord midpoint between the
                // hovered vertex (or segment) and its neighbour. The new
                // vertex inherits the previous vertex's bulge so an
                // existing arc is split into two arcs of the same chord.
                let (i0, i1) = if grip_id < n {
                    let i0 = grip_id;
                    let i1 = (grip_id + 1) % n;
                    (i0, i1)
                } else {
                    let seg = grip_id - n;
                    (seg, (seg + 1) % n)
                };
                if i1 == 0 && !self.is_closed {
                    return;
                }
                let v0 = &self.vertices[i0];
                let v1 = &self.vertices[i1];
                let mx = (v0.location.x + v1.location.x) * 0.5;
                let my = (v0.location.y + v1.location.y) * 0.5;
                let inherited = v0.clone();
                let mut new_v = inherited.clone();
                new_v.location.x = mx;
                new_v.location.y = my;
                let insert_at = (i0 + 1).min(self.vertices.len());
                self.vertices.insert(insert_at, new_v);
            }
            A::RemoveVertex if grip_id < n && self.vertices.len() > 2 => {
                self.vertices.remove(grip_id);
            }
            A::ConvertToArc if grip_id >= n => {
                if let Some(v) = self.vertices.get_mut(grip_id - n) {
                    if v.bulge.abs() < 1e-9 {
                        v.bulge = 0.5;
                    }
                }
            }
            A::ConvertToLine if grip_id >= n => {
                if let Some(v) = self.vertices.get_mut(grip_id - n) {
                    v.bulge = 0.0;
                }
            }
            _ => {}
        }
    }
}

impl crate::entities::traits::PropertyEditable for LwPolyline {
    fn geometry_properties(
        &self,
        _text_style_names: &[String],
    ) -> Vec<crate::scene::model::object::PropSection> {
        properties(self)
    }
    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        apply_geom_prop(self, field, value);
    }
}

impl crate::entities::traits::Transformable for LwPolyline {
    fn apply_transform(&mut self, t: &crate::command::EntityTransform) {
        apply_transform(self, t);
    }
}

/// Solid-fill bands for a wide LwPolyline, plus the `world_origin` they are
/// relative to. The band vertices are small f32 offsets from `origin` (the
/// first vertex), so the relative-to-eye hatch fill keeps sub-unit precision at
/// UTM-scale coordinates — building them in absolute f32 collapsed the band into
/// a string of squares far from the origin.
pub(crate) fn wide_fills(pl: &acadrust::entities::LwPolyline) -> ([f64; 2], Vec<Vec<[f32; 2]>>) {
    // Codes 43 / 40 / 41 store the band's FULL width, and
    // `polyline_segment_fill` offsets ±hw about the centreline — so halve it.
    // Feeding the stored width in whole draws every wide polyline twice as wide
    // as the file asks for, which on a donut (a closed 2-vertex bulge-1 polyline)
    // shows up as a disc 1.5× its real radius.
    let hw_const = pl.constant_width as f32 * 0.5;
    let verts = &pl.vertices;
    let n = verts.len();
    if n < 2 {
        return ([0.0; 2], vec![]);
    }
    let origin = [verts[0].location.x, verts[0].location.y];
    let seg_count = if pl.is_closed { n } else { n - 1 };
    let mut out = Vec::new();
    for i in 0..seg_count {
        let v0 = &verts[i];
        let v1 = &verts[(i + 1) % n];
        let hw0 = if v0.start_width > 1e-9 {
            v0.start_width as f32 * 0.5
        } else {
            hw_const
        };
        let hw1 = if v0.end_width > 1e-9 {
            v0.end_width as f32 * 0.5
        } else {
            hw_const
        };
        if hw0 < 1e-6 && hw1 < 1e-6 {
            continue;
        }
        let p0 = [
            (v0.location.x - origin[0]) as f32,
            (v0.location.y - origin[1]) as f32,
        ];
        let p1 = [
            (v1.location.x - origin[0]) as f32,
            (v1.location.y - origin[1]) as f32,
        ];
        if let Some(poly) =
            crate::entities::common::polyline_segment_fill(p0, p1, hw0, hw1, v0.bulge as f32)
        {
            out.push(poly);
        }
    }
    (origin, out)
}

impl crate::entities::traits::MassPropsCalc for acadrust::entities::LwPolyline {
    fn mass_props(&self) -> crate::entities::traits::MassProps {
        let p = self;
        let n = p.vertices.len();
        if n < 2 {
            return crate::entities::traits::MassProps {
                area: 0.0,
                perimeter: 0.0,
                cx: 0.0,
                cy: 0.0,
            };
        }
        // Shoelace area + perimeter
        let mut area_sum = 0.0f64;
        let mut perimeter = 0.0f64;
        let mut cx_sum = 0.0f64;
        let mut cy_sum = 0.0f64;
        let n_segs = if p.is_closed { n } else { n - 1 };
        for idx in 0..n_segs {
            let v0 = &p.vertices[idx];
            let v1 = &p.vertices[(idx + 1) % n];
            let x0 = v0.location.x;
            let y0 = v0.location.y;
            let x1 = v1.location.x;
            let y1 = v1.location.y;
            area_sum += x0 * y1 - x1 * y0;
            perimeter += ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            cx_sum += (x0 + x1) * (x0 * y1 - x1 * y0);
            cy_sum += (y0 + y1) * (x0 * y1 - x1 * y0);
        }
        let area = (area_sum / 2.0).abs();
        let (cx, cy) = if area > 1e-12 {
            (cx_sum / (6.0 * area), cy_sum / (6.0 * area))
        } else {
            let sx: f64 = p.vertices.iter().map(|v| v.location.x).sum::<f64>() / n as f64;
            let sy: f64 = p.vertices.iter().map(|v| v.location.y).sum::<f64>() / n as f64;
            (sx, sy)
        };
        crate::entities::traits::MassProps {
            area,
            perimeter,
            cx,
            cy,
        }
    }
}
