//! Resolving the exact ACIS subentity — a face, an edge or a vertex — under a
//! hover point, and the highlight geometry that goes with it.
//!
//! The scene keeps every solid's lifted kernel body in `Scene::solid_models`,
//! so a hover on a solid can be resolved against exact topology instead of the
//! displayed triangles: the triangles carry only a face slot (see
//! `MeshModel::triangle_face_keys`), while edges and vertices exist only in
//! the body. `subentity_candidates` turns a world-space hover point into the
//! subentities near it, sorted nearest-first, and `subentity_highlight_wires`
//! turns one candidate into the world-space polylines the preview-wire
//! machinery draws (`Scene::set_preview_wires`, the path the PRESSPULL
//! boundary hover feeds). The quad wiring resolves a candidate per hover,
//! highlights it, and cycles coincident ones by walking the sorted list.
//!
//! Both functions are total: a missing body (an entity that never lifted, or a
//! handle gone between hover and query) yields empty output. Bodies live in
//! world coordinates — the frame the hover point arrives in — so nothing is
//! transformed here.

use glam::DVec3;

use codec::Handle;
use kernel::brep::{Curve3, EdgeKey, FaceKey, Surface};

use crate::scene::Scene;

/// Where two samples of a boundary walk are the same joint.
const JOINT_TOLERANCE: f64 = 1e-9;

/// How many candidates one call returns. Coincident subentities at a corner or
/// a zoomed-in edge sit far below this; the cap bounds a pathological solid.
pub const MAX_SUBENTITY_CANDIDATES: usize = 32;

/// Sample count for a curved edge or boundary arc, mirroring the density the
/// solid snap path (`nearest_edge`) reads, so a hover resolves a curved
/// subentity to the same polyline the snap reader sees.
const CURVE_SAMPLES: usize = 64;

/// Which kind of subentity a candidate names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubKind {
    /// A B-rep face.
    Face,
    /// A B-rep edge — the curve between two vertices, not a display line.
    Edge,
    /// A B-rep vertex.
    Vertex,
}

impl SubKind {
    /// Tie-break rank for candidates of equal distance: the sharper, smaller
    /// things first — a vertex before the edges that meet there before the
    /// faces that bound it. At a box's edge midpoint the edge sits at distance
    /// zero inside both adjacent faces' supporting planes, so the edge wins
    /// the tie and the faces follow it.
    fn rank(self) -> u8 {
        match self {
            Self::Vertex => 0,
            Self::Edge => 1,
            Self::Face => 2,
        }
    }
}

/// Identity of one subentity within the body the query ran against.
///
/// `(idx, gen)` mirrors the kernel arena key, which keeps its generation
/// private and exposes only the slot (`Key::slot`): ACIS-sourced candidates
/// carry `(slot, 0)` and are matched back onto a body by slot. A key is only
/// meaningful for the body it was resolved from — one from another handle, or
/// across an edit that rebuilt the body, names nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubKey {
    pub idx: u32,
    pub gen: u32,
}

/// One resolvable subentity under a hover point.
///
/// `point` is the nearest point of the subentity's own geometry — the vertex
/// itself, the closest curve point for an edge, the hover point folded onto
/// the plane for a planar face — where a grip or a quad function should act.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubTarget {
    pub kind: SubKind,
    pub key: SubKey,
    pub point: DVec3,
}

/// Subentities of the solid at `handle` near the world-space point `world`,
/// sorted nearest-first, deduplicated by subentity, capped at
/// [`MAX_SUBENTITY_CANDIDATES`].
///
/// Every subentity within `tol` world units is a candidate. Distances are
/// real world distances to the subentity: exact for vertices, exact for a
/// line edge (a curved one is sampled at [`CURVE_SAMPLES`] steps and never
/// over-reports), and the distance to the supporting surface for faces — a
/// trimmed boundary only ever pushes the true face distance further out, so
/// `tol` may catch a face the actual patch sits just beyond, but never misses
/// a face the hover sits on.
///
/// Ties break by kind — vertex, then edge, then face ([`SubKind`] rank) —
/// then by key. Among faces at the same distance, a face the kernel's exact
/// planar containment query identifies as the one under the cursor (the
/// PRESSPULL face pick) outranks its coplanar or opposite twins.
///
/// `SubKey` keeps the kernel arena slot with a placeholder generation: the
/// kernel's `Key` generation is not public, so keys resolve by slot scan
/// against the CURRENT lifted body. A body is never rebuilt while a quad
/// target holds a key (any key/pick press closes the quad first), but a
/// future path that carries a [`SubTarget`] across a body rebuild must
/// re-resolve by slot — the generation cannot disambiguate today.
///
/// The list is recomputed per call with no scene-visible cache: one walk over
/// the body — a surface evaluation per face plus the containment sweep, at
/// most [`CURVE_SAMPLES`] samples per curved edge and O(1) per line edge and
/// vertex — then a sort of the few collected candidates. That is
/// O(faces + edges·samples + vertices) with small constants: cheap per cursor
/// move for the few-hundred-face solids a drawing holds, not asymptotically
/// free for a ten-thousand-face one.
pub fn subentity_candidates(
    scene: &Scene,
    handle: Handle,
    world: DVec3,
    tol: f64,
) -> Vec<SubTarget> {
    let mut candidates: Vec<(f64, bool, SubTarget)> = Vec::new();
    let Some(body) = scene.solid_models.get(&handle) else {
        return Vec::new();
    };
    let tol = if tol.is_finite() { tol.max(0.0) } else { 0.0 };
    if !world.is_finite() {
        return Vec::new();
    }
    let pick = world.to_array();

    // A planar face that exactly contains the hover (the kernel's PRESSPULL
    // face pick, trimmed-boundary tolerance included) answers whatever the
    // tolerance drops, and outranks the other faces it ties with — a coplanar
    // neighbour, or the same plane's far side — by being the face the cursor
    // is actually on. It joins the same distance sort as everything else, so
    // a strictly nearer edge or vertex still wins the list.
    let contained = kernel::brep::presspull::planar_face_at_point(body, pick, tol);
    for face in body.face_keys() {
        let Some(surface) = face_surface(body, face) else {
            continue;
        };
        let distance = surface.distance_to(pick).abs();
        if distance <= tol {
            if let Some(target) = face_target(body, face, world) {
                candidates.push((
                    distance,
                    contained == Some(face),
                    target,
                ));
            }
        }
    }
    for edge in body.edge_keys() {
        let Some((distance, point)) = edge_distance_and_point(body, edge, world) else {
            continue;
        };
        if distance <= tol {
            candidates.push((
                distance,
                false,
                SubTarget {
                    kind: SubKind::Edge,
                    key: SubKey {
                        idx: edge.slot(),
                        gen: 0,
                    },
                    point,
                },
            ));
        }
    }
    for (vertex, node) in body.vertices.iter() {
        let point = DVec3::from_array(node.point);
        if !point.is_finite() {
            continue;
        }
        let distance = world.distance(point);
        if distance <= tol {
            candidates.push((
                distance,
                false,
                SubTarget {
                    kind: SubKind::Vertex,
                    key: SubKey {
                        idx: vertex.slot(),
                        gen: 0,
                    },
                    point,
                },
            ));
        }
    }

    candidates.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| a.2.kind.rank().cmp(&b.2.kind.rank()))
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.key.idx.cmp(&b.2.key.idx))
            .then_with(|| a.2.key.gen.cmp(&b.2.key.gen))
    });
    let mut out: Vec<SubTarget> = Vec::with_capacity(candidates.len().min(MAX_SUBENTITY_CANDIDATES));
    for (_, _, target) in candidates {
        // The containment sweep and the distance walk name the same face
        // twice; the first (sorted) occurrence is the one kept.
        if out
            .last()
            .is_some_and(|kept: &SubTarget| kept.kind == target.kind && kept.key == target.key)
        {
            continue;
        }
        out.push(target);
        if out.len() >= MAX_SUBENTITY_CANDIDATES {
            break;
        }
    }
    out
}

/// The supporting surface of a face.
fn face_surface(body: &kernel::brep::Body, face: FaceKey) -> Option<&Surface> {
    let node = body.faces.get(face)?;
    body.surfaces.get(node.surface)
}

/// The subentity target for a face: the hover point folded onto a planar
/// face's plane, or for a curved face the hover point itself (the true
/// nearest surface point is within `tol` of it, which is all a hover answer
/// owes).
fn face_target(body: &kernel::brep::Body, face: FaceKey, world: DVec3) -> Option<SubTarget> {
    let point = match face_surface(body, face)? {
        Surface::Plane(plane) => {
            let normal = DVec3::from_array(plane.normal()?);
            let distance = plane.distance_to(world.to_array())?;
            world - normal * distance
        }
        _ => world,
    };
    Some(SubTarget {
        kind: SubKind::Face,
        key: SubKey {
            idx: face.slot(),
            gen: 0,
        },
        point,
    })
}

/// Distance and nearest curve point from `world` to an edge, mirroring
/// `nearest_edge`: a line edge by exact projection, a curved one by the
/// minimum over an even sample of its parameter span.
fn edge_distance_and_point(
    body: &kernel::brep::Body,
    edge: EdgeKey,
    pick: DVec3,
) -> Option<(f64, DVec3)> {
    let node = body.edges.get(edge)?;
    let curve = body.curves.get(node.curve)?;
    let start = DVec3::from_array(curve.point_at(node.start_parameter));
    let end = DVec3::from_array(curve.point_at(node.end_parameter));
    if matches!(curve, Curve3::Line(_)) {
        let span = end - start;
        let length_squared = span.length_squared();
        let along = if length_squared > 0.0 {
            (pick - start).dot(span) / length_squared
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        let point = start + span * along;
        return Some((pick.distance(point), point));
    }
    let mut best = (pick.distance(start), start);
    for step in 1..=CURVE_SAMPLES {
        let parameter = node.start_parameter
            + (node.end_parameter - node.start_parameter) * step as f64 / CURVE_SAMPLES as f64;
        let point = DVec3::from_array(curve.point_at(parameter));
        if point.is_finite() {
            let distance = pick.distance(point);
            if distance < best.0 {
                best = (distance, point);
            }
        }
    }
    Some(best)
}

/// Highlight geometry for one subentity: polylines in world space, shaped like
/// the PRESSPULL boundary hover's wires, handed to `Scene::set_preview_wires`.
/// A face contributes its boundary loops (one closed polyline each, outer
/// loop first, holes after), an edge its own polyline, a vertex no wire. A
/// body that is gone or a key that names nothing yields empty.
pub fn subentity_highlight_wires(
    scene: &Scene,
    handle: Handle,
    sub: SubTarget,
) -> Vec<Vec<DVec3>> {
    let Some(body) = scene.solid_models.get(&handle) else {
        return Vec::new();
    };
    match sub.kind {
        SubKind::Vertex => Vec::new(),
        SubKind::Edge => {
            let Some(edge) = body.edge_keys().find(|edge| edge.slot() == sub.key.idx) else {
                return Vec::new();
            };
            edge_polyline(body, edge).map_or_else(Vec::new, |polyline| vec![polyline])
        }
        SubKind::Face => {
            let Some(face) = body.face_keys().find(|face| face.slot() == sub.key.idx) else {
                return Vec::new();
            };
            let Some(node) = body.faces.get(face) else {
                return Vec::new();
            };
            let mut wires = Vec::new();
            for loop_key in &node.loops {
                let Some(ring) = body.loops.get(*loop_key) else {
                    continue;
                };
                if let Some(ring) = ring_polyline(body, &ring.coedges) {
                    wires.push(ring);
                }
            }
            wires
        }
    }
}

/// One edge as an open polyline: endpoints for a line edge, an even sample of
/// the curve for a curved one.
fn edge_polyline(body: &kernel::brep::Body, edge: EdgeKey) -> Option<Vec<DVec3>> {
    let node = body.edges.get(edge)?;
    let curve = body.curves.get(node.curve)?;
    let samples = curve_samples(curve, node.start_parameter, node.end_parameter, CURVE_SAMPLES);
    samples
        .iter()
        .all(|point| point.is_finite())
        .then_some(samples)
}

/// One boundary loop as a closed polyline: each coedge's curve sampled in the
/// loop's own direction, joints not repeated, the ring closed where it
/// started. None when the ring collapses or cannot be walked.
fn ring_polyline(
    body: &kernel::brep::Body,
    coedges: &[kernel::brep::CoedgeKey],
) -> Option<Vec<DVec3>> {
    let mut points: Vec<DVec3> = Vec::new();
    for coedge in coedges {
        let node = body.coedges.get(*coedge)?;
        let Some(edge) = body.edges.get(node.edge) else {
            continue;
        };
        let Some(curve) = body.curves.get(edge.curve) else {
            continue;
        };
        let mut samples =
            curve_samples(curve, edge.start_parameter, edge.end_parameter, CURVE_SAMPLES);
        if samples.iter().any(|point| !point.is_finite()) {
            continue;
        }
        if !node.forward {
            samples.reverse();
        }
        for point in samples {
            if points
                .last()
                .is_none_or(|last| last.distance(point) > JOINT_TOLERANCE)
            {
                points.push(point);
            }
        }
    }
    // Close the ring: the loop's last walked point is where the first began.
    if let Some(first) = points.first().copied() {
        if points
            .last()
            .is_some_and(|last| last.distance(first) > JOINT_TOLERANCE)
        {
            points.push(first);
        }
    }
    (points.len() >= 3).then_some(points)
}

/// Samples of a curve between two parameters, both ends included; a line
/// needs only its endpoints.
fn curve_samples(
    curve: &Curve3,
    start_parameter: f64,
    end_parameter: f64,
    samples: usize,
) -> Vec<DVec3> {
    if matches!(curve, Curve3::Line(_)) {
        return vec![
            DVec3::from_array(curve.point_at(start_parameter)),
            DVec3::from_array(curve.point_at(end_parameter)),
        ];
    }
    (0..=samples)
        .map(|step| {
            DVec3::from_array(
                curve.point_at(
                    start_parameter
                        + (end_parameter - start_parameter) * step as f64 / samples as f64,
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::model::solid_model::{box_solid, display_from_solid};

    fn box_scene() -> (Scene, Handle) {
        let mut scene = Scene::new();
        let handle = Handle::new(4401);
        scene
            .solid_models
            .insert(handle, box_solid([0.0, 0.0, 0.0], 10.0, 10.0, 10.0).unwrap());
        (scene, handle)
    }

    #[test]
    fn a_tessellated_box_mesh_keeps_face_identity_per_triangle() {
        let body = box_solid([0.0, 0.0, 0.0], 10.0, 10.0, 10.0).unwrap();
        let (set, _, _) = display_from_solid(&body, [0.7, 0.7, 0.7, 1.0], 1.0, None, [0; 2], None)
            .expect("a box tessellates");
        let mesh = &set.lods[0];
        let triangles = mesh.indices.len() / 3;
        assert!(triangles >= 12, "a box has triangles: {triangles}");
        assert_eq!(mesh.triangle_face_keys.len(), triangles);
        let mut faces = mesh.triangle_face_keys.clone();
        faces.sort_unstable();
        faces.dedup();
        assert_eq!(faces.len(), 6, "a box shows exactly six faces: {faces:?}");
        assert!(faces.iter().all(|(_, generation)| *generation == 0));
    }

    #[test]
    fn a_hover_on_a_box_top_resolves_that_face() {
        let (scene, handle) = box_scene();
        let top = DVec3::new(0.0, 0.0, 5.0);
        let candidates = subentity_candidates(&scene, handle, top, 1e-6);
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].kind, SubKind::Face, "{candidates:?}");
        assert!(candidates[0]
            .point
            .distance(top)
            < 1e-9, "{:?}", candidates[0]);
    }

    #[test]
    fn an_edge_midpoint_lists_the_edge_then_its_faces() {
        let (scene, handle) = box_scene();
        let middle = DVec3::new(0.0, -5.0, 5.0);
        let candidates = subentity_candidates(&scene, handle, middle, 1e-6);
        assert_eq!(candidates.len(), 3, "{candidates:?}");
        assert_eq!(candidates[0].kind, SubKind::Edge, "{candidates:?}");
        assert_eq!(candidates[0].point, middle, "{candidates:?}");
        let faces = candidates
            .iter()
            .filter(|candidate| candidate.kind == SubKind::Face)
            .count();
        assert_eq!(faces, 2, "the edge sits between two faces: {candidates:?}");
    }

    #[test]
    fn a_box_corner_resolves_the_vertex_at_zero_tolerance() {
        let (scene, handle) = box_scene();
        let corner = DVec3::new(-5.0, -5.0, 5.0);
        let candidates = subentity_candidates(&scene, handle, corner, 0.0);
        assert_eq!(candidates[0].kind, SubKind::Vertex, "{candidates:?}");
        assert_eq!(candidates[0].point, corner, "{candidates:?}");
        // Coincident at a box corner: the vertex, the three edges that meet
        // there and the three faces it bounds.
        assert_eq!(candidates.len(), 7, "{candidates:?}");
        let again = subentity_candidates(&scene, handle, corner, 0.0);
        assert_eq!(candidates, again, "the walk is deterministic");
    }

    #[test]
    fn a_handle_without_a_body_answers_nothing() {
        let mut scene = Scene::new();
        let handle = Handle::new(4402);
        let world = DVec3::new(0.0, 0.0, 5.0);
        assert!(subentity_candidates(&scene, handle, world, 1.0).is_empty());
        scene
            .solid_models
            .insert(handle, box_solid([0.0, 0.0, 0.0], 10.0, 10.0, 10.0).unwrap());
        assert!(subentity_candidates(&scene, Handle::new(4403), world, 1.0).is_empty());
        let sub = SubTarget {
            kind: SubKind::Face,
            key: SubKey { idx: 0, gen: 0 },
            point: world,
        };
        assert!(subentity_highlight_wires(&scene, Handle::new(4403), sub).is_empty());
    }

    #[test]
    fn a_box_front_edge_highlights_its_segment_and_a_vertex_nothing() {
        let (scene, handle) = box_scene();
        let middle = DVec3::new(0.0, -5.0, 5.0);
        let candidates = subentity_candidates(&scene, handle, middle, 1e-6);
        let edge = candidates
            .iter()
            .find(|candidate| candidate.kind == SubKind::Edge)
            .expect("the sweep edge is within tolerance");
        let wires = subentity_highlight_wires(&scene, handle, *edge);
        assert_eq!(wires.len(), 1, "{wires:?}");
        let mut ends = wires[0].clone();
        ends.sort_by(|a, b| {
            a.x.total_cmp(&b.x)
                .then(a.y.total_cmp(&b.y))
                .then(a.z.total_cmp(&b.z))
        });
        assert_eq!(
            ends,
            vec![DVec3::new(-5.0, -5.0, 5.0), DVec3::new(5.0, -5.0, 5.0)],
            "{wires:?}"
        );
        let corner = DVec3::new(-5.0, -5.0, 5.0);
        let candidates = subentity_candidates(&scene, handle, corner, 0.0);
        let vertex = candidates
            .iter()
            .find(|candidate| candidate.kind == SubKind::Vertex)
            .expect("the corner vertex is within tolerance");
        assert!(subentity_highlight_wires(&scene, handle, *vertex).is_empty());
    }

    #[test]
    fn a_box_face_highlights_one_closed_boundary_loop() {
        let (scene, handle) = box_scene();
        let candidates = subentity_candidates(&scene, handle, DVec3::new(0.0, 0.0, 5.0), 1e-6);
        wires_round_the_box(&scene, handle, &candidates);
    }

    fn wires_round_the_box(scene: &Scene, handle: Handle, candidates: &[SubTarget]) {
        let top = candidates[0];
        let wires = subentity_highlight_wires(scene, handle, top);
        assert_eq!(wires.len(), 1, "a box face bounds one loop: {wires:?}");
        let ring = &wires[0];
        assert_eq!(ring.first(), ring.last(), "a boundary ring closes: {ring:?}");
        assert_eq!(ring.len(), 5, "four corners and the closing repeat: {ring:?}");
    }
}
