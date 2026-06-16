//! CPU-side hit-testing for wire geometry.
//!
//! All tests are performed in **screen space** — wire vertices are projected
//! to 2-D pixel coordinates, then compared against the cursor or selection box.
//! This matches the visual result the user sees.

use rustc_hash::FxHashMap as HashMap;

use acadrust::Handle;
use glam::{Mat4, Vec3};
use iced::{Point, Rectangle};

use crate::scene::model::hatch_model::HatchModel;
use crate::scene::model::mesh_model::MeshModel;
use crate::scene::model::wire_model::WireModel;

/// Pixel radius used for single-click wire detection.
const CLICK_THRESHOLD_PX: f32 = 8.0;

// ── Single-click hit test ─────────────────────────────────────────────────

/// Return the `name` of the closest wire whose screen-space segments pass
/// within `CLICK_THRESHOLD_PX` pixels of `cursor`.
///
/// Returns `None` when no wire is close enough.
pub fn click_hit<'a>(
    cursor: Point,
    wires: &'a [WireModel],
    view_proj: Mat4,
    bounds: Rectangle,
) -> Option<&'a str> {
    let mut best_dist = CLICK_THRESHOLD_PX;
    let mut best: Option<&str> = None;

    // World z only shifts the *screen* x/y when the view is tilted (orbit /
    // perspective). In the flat top-down ortho view — the case where hover lag
    // on large drawings actually bites — a wire's screen position depends only
    // on its world x/y, so its world-space AABB projects exactly and we can
    // reject wires nowhere near the cursor without projecting any of their
    // points (the dominant per-move cost on 100 k-wire drawings).
    let z_flat = view_proj.z_axis.x.abs() < 1e-9 && view_proj.z_axis.y.abs() < 1e-9;

    // Q: lazy projection — no Vec allocation per wire; NaN resets the segment chain.
    for wire in wires {
        // Cheap AABB pre-reject (flat view only; never for the unbounded
        // sentinel used by previews / greeked text).
        if z_flat && wire.aabb != WireModel::UNBOUNDED_AABB {
            let [minx, miny, maxx, maxy] = wire.aabb;
            // Project all four corners — a plan view can be rotated about Z, so
            // the screen footprint isn't axis-aligned and the two diagonal
            // corners alone wouldn't bound it.
            let mut sx0 = f32::MAX;
            let mut sy0 = f32::MAX;
            let mut sx1 = f32::MIN;
            let mut sy1 = f32::MIN;
            for (cx, cy) in [(minx, miny), (maxx, miny), (maxx, maxy), (minx, maxy)] {
                let s = world_to_screen(Vec3::new(cx, cy, 0.0), view_proj, bounds);
                sx0 = sx0.min(s.x);
                sx1 = sx1.max(s.x);
                sy0 = sy0.min(s.y);
                sy1 = sy1.max(s.y);
            }
            let t = CLICK_THRESHOLD_PX;
            if cursor.x < sx0 - t || cursor.x > sx1 + t || cursor.y < sy0 - t || cursor.y > sy1 + t
            {
                continue;
            }
        }
        let mut prev: Option<Point> = None;
        for &[px, py, pz] in &wire.points {
            if px.is_nan() {
                prev = None;
                continue;
            }
            let cur = world_to_screen(Vec3::new(px, py, pz), view_proj, bounds);
            if let Some(p0) = prev {
                let d = dist_point_to_segment(cursor, p0, cur);
                if d < best_dist {
                    best_dist = d;
                    best = Some(&wire.name);
                }
            }
            prev = Some(cur);
        }
    }

    best
}

/// Like `click_hit` but returns every wire within the click threshold,
/// nearest first. Used by selection cycling to step through overlapping
/// objects under the cursor.
pub fn click_hits_all<'a>(
    cursor: Point,
    wires: &'a [WireModel],
    view_proj: Mat4,
    bounds: Rectangle,
) -> Vec<&'a str> {
    let mut hits: Vec<(f32, &str)> = Vec::new();
    for wire in wires {
        let mut prev: Option<Point> = None;
        let mut best_for_wire = CLICK_THRESHOLD_PX;
        let mut hit = false;
        for &[px, py, pz] in &wire.points {
            if px.is_nan() {
                prev = None;
                continue;
            }
            let cur = world_to_screen(Vec3::new(px, py, pz), view_proj, bounds);
            if let Some(p0) = prev {
                let d = dist_point_to_segment(cursor, p0, cur);
                if d < best_for_wire {
                    best_for_wire = d;
                    hit = true;
                }
            }
            prev = Some(cur);
        }
        if hit {
            hits.push((best_for_wire, &wire.name));
        }
    }
    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    hits.into_iter().map(|(_, name)| name).collect()
}

/// Pick a 3D solid by clicking anywhere on its shaded body: project each
/// mesh triangle to screen space and test whether the cursor lands inside it.
/// Returns the front-most hit (smallest projected depth). Lets meshed solids
/// (whose only wire geometry is thin edges) be selected on their faces.
pub fn mesh_click_hit<'a>(
    cursor: Point,
    meshes: impl Iterator<Item = (Handle, &'a MeshModel)>,
    view_proj: Mat4,
    bounds: Rectangle,
) -> Option<Handle> {
    let mut best: Option<(f32, Handle)> = None;
    for (handle, mesh) in meshes {
        let v = &mesh.verts;
        let idx = &mesh.indices;
        let mut t = 0;
        while t + 2 < idx.len() {
            let tri = [
                v[idx[t] as usize],
                v[idx[t + 1] as usize],
                v[idx[t + 2] as usize],
            ];
            t += 3;
            let mut sp = [Point::ORIGIN; 3];
            let mut depth = 0.0f32;
            for (j, w) in tri.iter().enumerate() {
                let ndc = view_proj.project_point3(Vec3::new(w[0], w[1], w[2]));
                sp[j] = Point::new(
                    (ndc.x + 1.0) * 0.5 * bounds.width,
                    (1.0 - ndc.y) * 0.5 * bounds.height,
                );
                depth += ndc.z;
            }
            if point_in_polygon(cursor, &sp) {
                let d = depth / 3.0;
                if best.map_or(true, |(bd, _)| d < bd) {
                    best = Some((d, handle));
                }
                break; // one hit per mesh is enough
            }
        }
    }
    best.map(|(_, h)| h)
}

// ── Box / window selection ────────────────────────────────────────────────

/// Return the names of wires selected by a completed rectangular selection box.
///
/// - **Window mode** (`crossing = false`, left→right drag):
///   ALL projected points must lie inside the box.
/// - **Crossing mode** (`crossing = true`, right→left drag):
///   ANY projected point inside the box, OR any wire segment crosses the box
///   boundary (so large entities like viewport frames are caught even when
///   no corner falls inside the selection rectangle).
pub fn box_hit<'a>(
    corner_a: Point,
    corner_b: Point,
    crossing: bool,
    wires: &'a [WireModel],
    view_proj: Mat4,
    bounds: Rectangle,
) -> Vec<&'a str> {
    let min_x = corner_a.x.min(corner_b.x);
    let max_x = corner_a.x.max(corner_b.x);
    let min_y = corner_a.y.min(corner_b.y);
    let max_y = corner_a.y.max(corner_b.y);

    // Ignore zero-area boxes.
    if (max_x - min_x) < 1.0 || (max_y - min_y) < 1.0 {
        return vec![];
    }

    let inside = |sp: Point| sp.x >= min_x && sp.x <= max_x && sp.y >= min_y && sp.y <= max_y;

    // Box corners for segment-intersection tests (crossing mode only).
    let box_tl = Point { x: min_x, y: min_y };
    let box_tr = Point { x: max_x, y: min_y };
    let box_bl = Point { x: min_x, y: max_y };
    let box_br = Point { x: max_x, y: max_y };

    // Q: lazy projection — accumulate screen points without allocating per-wire Vec.
    wires
        .iter()
        .filter_map(|wire| {
            // Fallback: when wire has no line geometry (e.g. greek text emits
            // only fill_tris) treat the AABB rectangle as the hit-test shape
            // so low-LOD text stays selectable. See #19.
            let aabb_pts: Vec<[f32; 3]>;
            let pts: &[[f32; 3]] = if !wire.points.is_empty() {
                &wire.points
            } else if wire.aabb != WireModel::UNBOUNDED_AABB {
                let [ax, ay, bx, by] = wire.aabb;
                aabb_pts = vec![
                    [ax, ay, 0.0],
                    [bx, ay, 0.0],
                    [bx, by, 0.0],
                    [ax, by, 0.0],
                    [ax, ay, 0.0],
                ];
                &aabb_pts
            } else {
                return None;
            };

            let mut hit = false;
            let mut all_inside = true;
            let mut prev: Option<Point> = None;

            for &[px, py, pz] in pts {
                if px.is_nan() {
                    prev = None;
                    continue;
                }
                let sp = world_to_screen(Vec3::new(px, py, pz), view_proj, bounds);
                if crossing {
                    if inside(sp) {
                        hit = true;
                    }
                    if let Some(p0) = prev {
                        if !hit {
                            hit = segments_intersect(p0, sp, box_tl, box_tr)
                                || segments_intersect(p0, sp, box_tr, box_br)
                                || segments_intersect(p0, sp, box_br, box_bl)
                                || segments_intersect(p0, sp, box_bl, box_tl);
                        }
                    }
                } else {
                    if !inside(sp) {
                        all_inside = false;
                    }
                }
                prev = Some(sp);
            }

            let result = if crossing {
                hit
            } else {
                all_inside && prev.is_some()
            };
            if result {
                Some(wire.name.as_str())
            } else {
                None
            }
        })
        .collect()
}

// ── Polygon / lasso selection ─────────────────────────────────────────────

/// Return the names of wires selected by a freehand polygon lasso.
///
/// - **Window mode** (`crossing = false`): ALL projected points inside polygon.
/// - **Crossing mode** (`crossing = true`): ANY point inside OR any wire
///   segment crosses a polygon edge.
pub fn poly_hit<'a>(
    poly: &[Point],
    crossing: bool,
    wires: &'a [WireModel],
    view_proj: Mat4,
    bounds: Rectangle,
) -> Vec<&'a str> {
    if poly.len() < 3 {
        return vec![];
    }

    // Q: lazy projection — no Vec allocation per wire.
    wires
        .iter()
        .filter_map(|wire| {
            // Same AABB fallback as `box_hit`: when a wire has no line
            // geometry (e.g. greek-LOD text emits only fill_tris) treat the
            // AABB rectangle as the hit-test shape so low-LOD text stays
            // selectable. See #19.
            let aabb_pts: Vec<[f32; 3]>;
            let pts: &[[f32; 3]] = if !wire.points.is_empty() {
                &wire.points
            } else if wire.aabb != WireModel::UNBOUNDED_AABB {
                let [ax, ay, bx, by] = wire.aabb;
                aabb_pts = vec![
                    [ax, ay, 0.0],
                    [bx, ay, 0.0],
                    [bx, by, 0.0],
                    [ax, by, 0.0],
                    [ax, ay, 0.0],
                ];
                &aabb_pts
            } else {
                return None;
            };

            let mut hit = false;
            let mut all_inside = true;
            let mut prev: Option<Point> = None;

            for &[px, py, pz] in pts {
                if px.is_nan() {
                    prev = None;
                    continue;
                }
                let sp = world_to_screen(Vec3::new(px, py, pz), view_proj, bounds);
                if crossing {
                    if point_in_polygon(sp, poly) {
                        hit = true;
                    }
                    if !hit {
                        if let Some(p0) = prev {
                            if segment_crosses_polygon(p0, sp, poly) {
                                hit = true;
                            }
                        }
                    }
                } else {
                    if !point_in_polygon(sp, poly) {
                        all_inside = false;
                    }
                }
                prev = Some(sp);
            }

            let result = if crossing {
                hit
            } else {
                all_inside && prev.is_some()
            };
            if result {
                Some(wire.name.as_str())
            } else {
                None
            }
        })
        .collect()
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn world_to_screen(world: Vec3, view_proj: Mat4, bounds: Rectangle) -> Point {
    let ndc = view_proj.project_point3(world);
    Point::new(
        (ndc.x + 1.0) * 0.5 * bounds.width,
        (1.0 - ndc.y) * 0.5 * bounds.height,
    )
}

/// Even-odd ray-casting test: is `p` inside the polygon?
///
/// Handles multi-path boundaries: NaN points (used as path separators by
/// hatches with islands / holes) reset the previous-vertex tracking so
/// that the ray-cast doesn't draw a spurious closing edge between the
/// end of one sub-path and the start of the next. Each sub-path with at
/// least 2 finite vertices contributes its segments to the parity flip.
fn point_in_polygon(p: Point, poly: &[Point]) -> bool {
    // Ray-cast crossing test for a single edge a→b.
    fn cross(p: Point, a: Point, b: Point, inside: &mut bool) {
        if (a.y > p.y) != (b.y > p.y)
            && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x
        {
            *inside = !*inside;
        }
    }

    let mut inside = false;
    let mut prev: Option<Point> = None;
    let mut path_start: Option<Point> = None;
    // Vertices in the current sub-path. A boundary can be encoded either as a
    // ring (`[v0,v1,v2,v3]`, needs an implicit closing edge) or as an explicit
    // edge list (`[v0,v1, NaN, v1,v2, NaN, …]`, already closed). Only close a
    // sub-path that is a real ring (≥3 verts); closing a 2-point explicit edge
    // would add a degenerate back-edge that cancels its own crossing.
    let mut count = 0usize;
    let close = |prev: Option<Point>, path_start: Option<Point>, count: usize, inside: &mut bool| {
        if count >= 3 {
            if let (Some(pv), Some(sv)) = (prev, path_start) {
                cross(p, pv, sv, inside);
            }
        }
    };
    for &pt in poly {
        if !pt.x.is_finite() || !pt.y.is_finite() {
            close(prev, path_start, count, &mut inside);
            prev = None;
            path_start = None;
            count = 0;
            continue;
        }
        if let Some(prev_v) = prev {
            cross(p, prev_v, pt, &mut inside);
        } else {
            path_start = Some(pt);
        }
        prev = Some(pt);
        count += 1;
    }
    close(prev, path_start, count, &mut inside);
    inside
}

/// Does segment `[a, b]` cross any edge of the polygon?
fn segment_crosses_polygon(a: Point, b: Point, poly: &[Point]) -> bool {
    let n = poly.len();
    for i in 0..n {
        let c = poly[i];
        let d = poly[(i + 1) % n];
        if segments_intersect(a, b, c, d) {
            return true;
        }
    }
    false
}

/// Do segments `[a,b]` and `[c,d]` intersect?
fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let cross = |o: Point, p: Point, q: Point| -> f32 {
        (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x)
    };
    let d1 = cross(c, d, a);
    let d2 = cross(c, d, b);
    let d3 = cross(a, b, c);
    let d4 = cross(a, b, d);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    false
}

// ── Hatch hit-testing ─────────────────────────────────────────────────────

/// Return the Handle of the first hatch whose screen-space boundary polygon
/// contains `cursor`.
pub fn click_hit_hatch(
    cursor: Point,
    hatches: &HashMap<Handle, HatchModel>,
    view_proj: Mat4,
    bounds: Rectangle,
) -> Option<Handle> {
    for (&handle, hatch) in hatches {
        if hatch_contains_screen_point(hatch, cursor, view_proj, bounds) {
            return Some(handle);
        }
    }
    None
}

/// Same as `click_hit_hatch` but iterates `(Handle, HatchModel)` pairs
/// where the Handle is the parent Insert handle (block-internal
/// hatches). The first matching pair returns its Insert handle so
/// clicking a sub-hatch of a block selects the Insert, matching
/// AutoCAD's behaviour for block sub-entities.
pub fn click_hit_insert_hatch(
    cursor: Point,
    insert_hatches: &[(Handle, HatchModel)],
    view_proj: Mat4,
    bounds: Rectangle,
) -> Option<Handle> {
    for (handle, hatch) in insert_hatches {
        if hatch_contains_screen_point(hatch, cursor, view_proj, bounds) {
            return Some(*handle);
        }
    }
    None
}

fn hatch_contains_screen_point(
    hatch: &HatchModel,
    cursor: Point,
    view_proj: Mat4,
    bounds: Rectangle,
) -> bool {
    // boundary verts are stored as small f32 offsets from
    // `world_origin` (f64). Reconstruct offset-rel WCS before
    // projecting to screen.
    let ox = hatch.world_origin[0] as f32;
    let oy = hatch.world_origin[1] as f32;
    let screen: Vec<Point> = hatch
        .boundary
        .iter()
        .map(|&[x, y]| {
            if x.is_finite() && y.is_finite() {
                world_to_screen(Vec3::new(x + ox, y + oy, 0.0), view_proj, bounds)
            } else {
                // Preserve path separators for the NaN-aware
                // point_in_polygon ray-cast.
                Point::new(f32::NAN, f32::NAN)
            }
        })
        .collect();
    screen.len() >= 3 && point_in_polygon(cursor, &screen)
}

/// Return Handles of hatches selected by a completed rectangular selection box.
pub fn box_hit_hatch(
    corner_a: Point,
    corner_b: Point,
    crossing: bool,
    hatches: &HashMap<Handle, HatchModel>,
    view_proj: Mat4,
    bounds: Rectangle,
) -> Vec<Handle> {
    let min_x = corner_a.x.min(corner_b.x);
    let max_x = corner_a.x.max(corner_b.x);
    let min_y = corner_a.y.min(corner_b.y);
    let max_y = corner_a.y.max(corner_b.y);

    if (max_x - min_x) < 1.0 || (max_y - min_y) < 1.0 {
        return vec![];
    }

    let inside = |sp: Point| sp.x >= min_x && sp.x <= max_x && sp.y >= min_y && sp.y <= max_y;

    hatches
        .iter()
        .filter_map(|(&handle, hatch)| {
            if hatch.boundary.is_empty() {
                return None;
            }
            let ox = hatch.world_origin[0] as f32;
            let oy = hatch.world_origin[1] as f32;
            let screen: Vec<Point> = hatch
                .boundary
                .iter()
                .map(|&[x, y]| world_to_screen(Vec3::new(x + ox, y + oy, 0.0), view_proj, bounds))
                .collect();
            let hit = if crossing {
                screen.iter().any(|&sp| inside(sp))
            } else {
                screen.iter().all(|&sp| inside(sp))
            };
            if hit {
                Some(handle)
            } else {
                None
            }
        })
        .collect()
}

/// Return Handles of hatches selected by a freehand polygon lasso.
pub fn poly_hit_hatch(
    poly: &[Point],
    crossing: bool,
    hatches: &HashMap<Handle, HatchModel>,
    view_proj: Mat4,
    bounds: Rectangle,
) -> Vec<Handle> {
    if poly.len() < 3 {
        return vec![];
    }

    hatches
        .iter()
        .filter_map(|(&handle, hatch)| {
            if hatch.boundary.is_empty() {
                return None;
            }
            let ox = hatch.world_origin[0] as f32;
            let oy = hatch.world_origin[1] as f32;
            let screen: Vec<Point> = hatch
                .boundary
                .iter()
                .map(|&[x, y]| world_to_screen(Vec3::new(x + ox, y + oy, 0.0), view_proj, bounds))
                .collect();
            let hit = if crossing {
                screen.iter().any(|&sp| point_in_polygon(sp, poly))
                    || screen
                        .windows(2)
                        .any(|seg| segment_crosses_polygon(seg[0], seg[1], poly))
            } else {
                screen.iter().all(|&sp| point_in_polygon(sp, poly))
            };
            if hit {
                Some(handle)
            } else {
                None
            }
        })
        .collect()
}

/// Minimum distance from point `p` to line segment `[a, b]` in 2-D.
fn dist_point_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    let t = if len2 < 1e-6 {
        0.0
    } else {
        let apx = p.x - a.x;
        let apy = p.y - a.y;
        ((apx * abx + apy * aby) / len2).clamp(0.0, 1.0)
    };
    let cx = a.x + t * abx;
    let cy = a.y + t * aby;
    let dx = p.x - cx;
    let dy = p.y - cy;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod aabb_reject_tests {
    use super::*;

    fn wire(name: &str, pts: Vec<[f32; 3]>, aabb: [f32; 4]) -> WireModel {
        let mut w = WireModel::solid(name.to_string(), pts, [1.0; 4], false);
        w.aabb = aabb;
        w
    }

    // Identity ortho view: world (x,y) → screen ((x+1)*100, (1-y)*100) for a
    // 200×200 viewport. The view is flat (z_axis.xy == 0) so the AABB pre-reject
    // is active — these tests guard it against false negatives.
    #[test]
    fn aabb_reject_keeps_near_wire_drops_far() {
        let vp = Mat4::IDENTITY;
        let bounds = Rectangle { x: 0.0, y: 0.0, width: 200.0, height: 200.0 };
        let cursor = Point::new(100.0, 100.0); // world origin

        let near = wire("5", vec![[-0.02, 0.0, 0.0], [0.02, 0.0, 0.0]], [-0.02, 0.0, 0.02, 0.0]);
        let far = wire("9", vec![[0.9, 0.9, 0.0], [0.95, 0.9, 0.0]], [0.9, 0.9, 0.95, 0.9]);

        assert_eq!(click_hit(cursor, std::slice::from_ref(&near), vp, bounds), Some("5"));
        assert_eq!(click_hit(cursor, std::slice::from_ref(&far), vp, bounds), None);
        // The far wire must be rejected without hiding the near one.
        assert_eq!(click_hit(cursor, &[far, near], vp, bounds), Some("5"));
    }
}
