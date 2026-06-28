// REVERSE command — reverse the direction (vertex / point order) of a single
// picked curve.
//
// The user picks one open or closed curve; the command builds a reversed copy
// and swaps it in via `CmdResult::ReplaceEntity`. Supported curve types:
//
//   * Line        — swap start / end.
//   * LwPolyline  — reverse the vertex list. The bulge stored at a vertex
//                   describes the arc on the segment *starting* at that vertex,
//                   so a plain `vertices.reverse()` would attach each bulge to
//                   the wrong segment (and with the wrong sense). We therefore
//                   recompute every bulge from the original segment bulges (see
//                   `reverse_lwpolyline`).
//   * Polyline3D  — reverse the vertex list (no bulges to reconcile).
//   * Spline      — reverse control points and fit points, then regenerate the
//                   clamped knot vector (mirrors the REVERSE branch of
//                   `splinedit::apply_spline_op`).
//
// Any other entity type is left untouched: the command returns
// `CmdResult::NeedPoint` and keeps prompting so nothing is corrupted.

use acadrust::entities::Spline;
use acadrust::{EntityType, Handle};
use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};

// ── Ribbon definition ───────────────────────────────────────────────────────

#[allow(dead_code)] // ribbon definition ready for wiring; command works via the command line
pub fn tool() -> ToolDef {
    ToolDef {
        id: "REVERSE",
        label: "Reverse",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/line.svg")),
        event: ModuleEvent::Command("REVERSE".to_string()),
    }
}

// ── Command implementation ──────────────────────────────────────────────────

pub struct ReverseCommand {
    /// The picked entity, injected by the host before `on_entity_pick` runs.
    /// `None` until the host injects it.
    picked: Option<EntityType>,
}

impl ReverseCommand {
    pub fn new() -> Self {
        Self { picked: None }
    }

    /// Build a reversed copy of `entity`, or `None` for an unsupported type.
    fn reversed(entity: &EntityType) -> Option<EntityType> {
        match entity {
            EntityType::Line(line) => {
                let mut out = line.clone();
                std::mem::swap(&mut out.start, &mut out.end);
                Some(EntityType::Line(out))
            }
            EntityType::LwPolyline(pl) => Some(EntityType::LwPolyline(reverse_lwpolyline(pl))),
            EntityType::Polyline3D(pl) => {
                let mut out = pl.clone();
                out.vertices.reverse();
                Some(EntityType::Polyline3D(out))
            }
            EntityType::Spline(sp) => Some(EntityType::Spline(reverse_spline(sp))),
            _ => None,
        }
    }
}

/// Reverse an LwPolyline, reconciling per-segment bulges.
///
/// Bulge semantics: `vertices[k].bulge` is the bulge of the segment that
/// *starts* at vertex `k`. For an open polyline with `n` vertices there are
/// `n - 1` segments (the trailing vertex's bulge is unused); for a closed
/// polyline there are `n` segments (the trailing vertex's bulge is the closing
/// segment back to vertex 0).
///
/// After reversal the new vertex `i` is the original vertex `n-1-i`. The new
/// segment from new-vertex `i` to new-vertex `i+1` is the original segment
/// between original vertices `n-1-i` and `n-2-i`, traversed backwards — i.e.
/// the original segment that *starts* at vertex `n-2-i`. Reversing the
/// traversal direction flips an arc's sense, so the new bulge is the negation
/// of that original segment's bulge:
///
///   new_vertices[i].bulge = -old_vertices[n-2-i].bulge        (open / interior)
///
/// For a closed polyline the closing segment wraps, so the general modular form
/// is used: new seg `i` (from new vertex `i`) corresponds to original segment
/// starting at original vertex `(n-1-i-1).rem_euclid(n)`. The widths follow the
/// vertex they are attached to and are carried along with the reversed order;
/// they are not segment-relative, so they need no shift.
fn reverse_lwpolyline(pl: &acadrust::LwPolyline) -> acadrust::LwPolyline {
    let mut out = pl.clone();
    let n = pl.vertices.len();
    if n < 2 {
        return out;
    }

    // Reverse vertex positions and per-vertex widths by cloning in reverse.
    for i in 0..n {
        let src = &pl.vertices[n - 1 - i];
        out.vertices[i].location = src.location;
        out.vertices[i].start_width = src.start_width;
        out.vertices[i].end_width = src.end_width;
    }

    // Recompute bulges from the original segment bulges.
    if pl.is_closed {
        // n segments; closing segment included.
        for i in 0..n {
            let src_seg_start = (n - 1 - i + n - 1) % n; // = (2n - 2 - i) % n
            out.vertices[i].bulge = -pl.vertices[src_seg_start].bulge;
        }
    } else {
        // n - 1 segments. New segment i (for i in 0..n-1) maps to original
        // segment starting at vertex n-2-i. The trailing vertex's bulge is
        // unused for an open polyline; clear it for cleanliness.
        for i in 0..n - 1 {
            out.vertices[i].bulge = -pl.vertices[n - 2 - i].bulge;
        }
        out.vertices[n - 1].bulge = 0.0;
    }

    out
}

/// Reverse a Spline: flip control points and fit points, regenerate the
/// clamped knot vector. Mirrors `splinedit::apply_spline_op`'s REVERSE branch.
fn reverse_spline(sp: &Spline) -> Spline {
    let mut out = sp.clone();
    out.control_points.reverse();
    out.fit_points.reverse();
    // Weights track control points one-to-one for rational splines.
    if out.weights.len() == out.control_points.len() {
        out.weights.reverse();
    }
    out.knots =
        Spline::generate_clamped_knots(out.degree as usize, out.control_points.len());
    out
}

impl CadCommand for ReverseCommand {
    fn name(&self) -> &'static str {
        "REVERSE"
    }

    fn prompt(&self) -> String {
        "REVERSE  Select line, polyline or spline to reverse:".to_string()
    }

    fn needs_entity_pick(&self) -> bool {
        true
    }

    fn inject_before_entity_pick(&self) -> bool {
        true
    }

    fn inject_picked_entity(&mut self, entity: EntityType) {
        self.picked = Some(entity);
    }

    fn on_entity_pick(&mut self, handle: Handle, _pt: DVec3) -> CmdResult {
        if handle.is_null() {
            return CmdResult::NeedPoint;
        }
        match self.picked.as_ref().and_then(Self::reversed) {
            Some(reversed) => CmdResult::ReplaceEntity(handle, vec![reversed]),
            // Unsupported entity type — keep prompting, do not corrupt it.
            None => CmdResult::NeedPoint,
        }
    }

    fn on_point(&mut self, _pt: DVec3) -> CmdResult {
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
}

// ── Autocomplete registry ───────────────────────────────────────────────────
inventory::submit!(crate::command::CommandRegistration {
    names: &["REVERSE"]
}); // ReverseCommand
