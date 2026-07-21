// Hatch/Gradient/Boundary commands — OpenCADStudio Home > Draw > Hatch dropdown.
//
// Commands:
//   HATCH    — ANSI31: 45° hatch lines (pick inside or type S for manual)
//   GRADIENT — Linear gradient fill (pick inside or type S for manual)
//   BOUNDARY — Traces the enclosing boundary as a closed LwPolyline
//
// Primary workflow (matches OpenCADStudio):
//   Click a point INSIDE a closed region → boundary auto-detected.
//   Type "S" to switch to manual vertex-picking mode (HATCH/GRADIENT only).

use crate::command::{CadCommand, CmdResult};
use crate::modules::IconKind;
use crate::scene::model::hatch_model::{HatchModel, HatchPattern, PatFamily};
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

// ── Icons ──────────────────────────────────────────────────────────────────

const ICON_HATCH: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/hatch/hatch_lines.svg"
));
const ICON_GRADIENT: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/hatch/hatch_gradient.svg"
));
const ICON_BOUNDARY: IconKind = IconKind::Svg(include_bytes!(
    "../../../../assets/icons/hatch/hatch_boundary.svg"
));

// ── Dropdown metadata ──────────────────────────────────────────────────────

pub const DROPDOWN_ID: &str = "HATCH";
pub const ICON: IconKind = ICON_HATCH;

pub const DROPDOWN_ITEMS: &[(&str, &str, IconKind)] = &[
    ("HATCH", "Hatch", ICON_HATCH),
    ("GRADIENT", "Gradient", ICON_GRADIENT),
    ("BOUNDARY", "Boundary", ICON_BOUNDARY),
];

// ── Shared mode ────────────────────────────────────────────────────────────

enum Mode {
    /// Primary: click inside a closed shape → boundary auto-detected.
    PickInside,
    /// Fallback: user manually picks polygon vertices (type "S" to enter).
    Manual,
}

// ── CPU point-in-polygon (ray casting) ────────────────────────────────────

fn point_in_polygon(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let vi = poly[i];
        let vj = poly[j];
        if (vi[1] > p[1]) != (vj[1] > p[1]) {
            let x_int = (vj[0] - vi[0]) * (p[1] - vi[1]) / (vj[1] - vi[1]) + vi[0];
            if p[0] < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Shoelace-area magnitude of a polygon. Used to pick the smallest enclosing
/// outline when a click falls inside several nested boundaries.
fn polygon_area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0_f64;
    let mut j = n - 1;
    for i in 0..n {
        a += (poly[j][0] as f64) * (poly[i][1] as f64)
            - (poly[i][0] as f64) * (poly[j][1] as f64);
        j = i;
    }
    (a * 0.5).abs() as f32
}

/// True when every vertex of `inner` lies inside `outer`. Sufficient to
/// recognise a closed hatch outline as nested inside another for the common
/// rectangle / closed-polyline case.
fn polygon_contains_polygon(outer: &[[f32; 2]], inner: &[[f32; 2]]) -> bool {
    if inner.len() < 3 {
        return false;
    }
    inner.iter().all(|&v| point_in_polygon(v, outer))
}

/// Resolve the hatch boundary for a "pick inside" click.
///
/// The outer ring is the *smallest* outline containing the click point — the
/// innermost region the point belongs to. Its holes are that ring's **direct
/// children**: outlines nested one level inside it with no other outline in
/// between. Deeper (grandchild) outlines belong to those children's own fills,
/// so they are left out — otherwise even-odd rasterisation would flip the
/// innermost island back on for 3+ nesting levels. The result is intuitive and
/// draw-order independent:
///   * click inside the innermost shape → hatch just that shape,
///   * click in a gap → hatch that ring, with the next level in as holes.
fn resolve_hatch_rings(
    outlines: &[Vec<[f32; 2]>],
    p: [f32; 2],
) -> Option<Vec<Vec<[f64; 2]>>> {
    let mut containing: Vec<(usize, f32)> = outlines
        .iter()
        .enumerate()
        .filter(|(_, o)| point_in_polygon(p, o))
        .map(|(i, o)| (i, polygon_area(o)))
        .collect();
    if containing.is_empty() {
        return None;
    }
    // Innermost (smallest-area) outline containing the point is the fill.
    containing.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let outer_idx = containing[0].0;
    let outer = &outlines[outer_idx];

    let mut rings: Vec<Vec<[f64; 2]>> =
        vec![outer.iter().map(|&[x, y]| [x as f64, y as f64]).collect()];
    for (i, o) in outlines.iter().enumerate() {
        if i == outer_idx {
            continue;
        }
        // Candidate hole: fully nested inside the fill outline. (An outline the
        // click sits in cannot qualify — it would have been the smaller fill.)
        if !polygon_contains_polygon(outer, o) || point_in_polygon(p, o) {
            continue;
        }
        // Only DIRECT children become holes. If another outline sits strictly
        // between `outer` and `o` (inside `outer`, and enclosing `o`), then `o`
        // belongs to that intermediate region's own fill; flagging it here would
        // re-fill it under even-odd once nesting reaches three levels.
        let has_intermediate = outlines.iter().enumerate().any(|(k, x)| {
            k != i
                && k != outer_idx
                && polygon_contains_polygon(outer, x)
                && polygon_contains_polygon(x, o)
        });
        if !has_intermediate {
            rings.push(o.iter().map(|&[x, y]| [x as f64, y as f64]).collect());
        }
    }
    Some(rings)
}

/// Pack one or more rings (outer boundary + optional holes) into the Hatch
/// model storage: the `boundary` f32 ring list (NaN-separated) plus the exact
/// `boundary_wcs` (NaN-separated) used for persistence. The first vertex of the
/// first ring anchors the shared origin.
fn pack_rings(rings: &[Vec<[f64; 2]>]) -> (Vec<[f32; 2]>, [f64; 2], Vec<[f64; 2]>) {
    let mut wcs: Vec<[f64; 2]> = Vec::new();
    let mut first = true;
    for ring in rings {
        if !first {
            wcs.push([f64::NAN, f64::NAN]);
        }
        first = false;
        wcs.extend(ring.iter().copied());
    }
    let (rel, origin) = rte_boundary(wcs.iter().map(|&[x, y]| (x, y)));
    (rel, origin, wcs)
}

/// Split an absolute boundary into the `(f32 offsets, f64 origin)` pair that
/// `HatchModel` expects: the origin anchors on the first vertex in full f64 so a
/// typed coordinate (issue #311) and large/UTM positions keep their precision,
/// and `add_hatch` reconstructs each WCS vertex as `origin + offset`. A zero
/// origin with absolute f32 offsets — the previous command output — quantized
/// typed points and mis-placed the fill at large coordinates.
fn rte_boundary(pts: impl Iterator<Item = (f64, f64)>) -> (Vec<[f32; 2]>, [f64; 2]) {
    let pts: Vec<(f64, f64)> = pts.collect();
    let Some(&(ox, oy)) = pts.first() else {
        return (vec![], [0.0; 2]);
    };
    let rel = pts
        .iter()
        .map(|&(x, y)| [(x - ox) as f32, (y - oy) as f32])
        .collect();
    (rel, [ox, oy])
}

// ── HATCH command ──────────────────────────────────────────────────────────

pub struct HatchCommand {
    outlines: Vec<Vec<[f32; 2]>>,
    mode: Mode,
    manual_pts: Vec<DVec3>,
    missed: bool,
}

impl HatchCommand {
    pub fn new(outlines: Vec<Vec<[f32; 2]>>) -> Self {
        Self {
            outlines,
            mode: Mode::PickInside,
            manual_pts: vec![],
            missed: false,
        }
    }

    fn make_hatch(&self, rings: Vec<Vec<[f64; 2]>>) -> HatchModel {
        let (rel, origin, wcs) = pack_rings(&rings);
        // Default: ANSI31 from catalog; fallback to a single 45° family.
        let pat_name = "ANSI31";
        let families = crate::scene::model::hatch_patterns::find(pat_name)
            .and_then(|e| {
                if let HatchPattern::Pattern(f) = &e.gpu {
                    Some(f.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                // 45° lines, perpendicular spacing ≈ 5 world units.
                let dy = 5.0_f32 / (45.0_f32.to_radians().cos());
                vec![PatFamily {
                    angle_deg: 45.0,
                    x0: 0.0,
                    y0: 0.0,
                    dx: 0.0,
                    dy,
                    dashes: vec![],
                }]
            });
        HatchModel {
            boundary: std::sync::Arc::new(rel),
            pattern: HatchPattern::Pattern(families),
            name: pat_name.into(),
            color: [0.75, 0.75, 0.75, 0.85],
            angle_offset: 0.0,
            scale: 1.0,
            world_origin: origin,
            boundary_wcs: Some(std::sync::Arc::new(wcs)),
            draw_depth: 0.0,
        }
    }
}

impl CadCommand for HatchCommand {
    fn name(&self) -> &'static str {
        "HATCH"
    }

    fn prompt(&self) -> String {
        match &self.mode {
            Mode::PickInside => {
                let miss = if self.missed {
                    "  ⚠ No closed boundary found."
                } else {
                    ""
                };
                format!("HATCH  Pick internal point:{miss}")
            }
            Mode::Manual => {
                if self.manual_pts.is_empty() {
                    "HATCH  Boundary point 1:".into()
                } else {
                    format!("HATCH  Point {}:", self.manual_pts.len() + 1)
                }
            }
        }
    }

    fn options(&self) -> Vec<crate::command::CmdOption> {
        use crate::command::CmdOption;
        match &self.mode {
            Mode::PickInside => vec![CmdOption::new("Draw manually", "S")],
            Mode::Manual => {
                // Enter accepts the boundary once at least 3 points are picked.
                if self.manual_pts.len() >= 3 {
                    vec![CmdOption::enter("Accept")]
                } else {
                    vec![]
                }
            }
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match &self.mode {
            Mode::PickInside => {
                // Hit-test against the f32 outline catalog (screen-space).
                let xy = [pt.x as f32, pt.y as f32];
                match resolve_hatch_rings(&self.outlines, xy) {
                    Some(rings) => {
                        self.missed = false;
                        return CmdResult::CommitHatch(self.make_hatch(rings));
                    }
                    None => {
                        self.missed = true;
                        CmdResult::NeedPoint
                    }
                }
            }
            Mode::Manual => {
                // Keep the typed/snapped point exact (issue #311).
                self.manual_pts.push(pt);
                CmdResult::NeedPoint
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        match &self.mode {
            Mode::PickInside => CmdResult::Cancel,
            Mode::Manual => {
                if self.manual_pts.len() < 3 {
                    return CmdResult::Cancel;
                }
                let wcs = self.manual_pts.iter().map(|p| [p.x, p.y]).collect();
                CmdResult::CommitHatch(self.make_hatch(vec![wcs]))
            }
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn wants_text_input(&self) -> bool {
        matches!(self.mode, Mode::PickInside)
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if text.trim().eq_ignore_ascii_case("s") {
            self.mode = Mode::Manual;
            self.missed = false;
            return Some(CmdResult::NeedPoint);
        }
        None
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> { let pt = pt.as_vec3();
        if let Mode::Manual = &self.mode {
            if self.manual_pts.is_empty() {
                return None;
            }
            let mut pts: Vec<[f32; 3]> = self
                .manual_pts
                .iter()
                .map(|p| [p.x as f32, p.y as f32, p.z as f32])
                .collect();
            pts.push([pt.x, pt.y, pt.z]);
            pts.push([
                self.manual_pts[0].x as f32,
                self.manual_pts[0].y as f32,
                self.manual_pts[0].z as f32,
            ]);
            return Some(WireModel::solid(
                "rubber_band".into(),
                pts,
                WireModel::CYAN,
                false,
            ));
        }
        None
    }
}

// ── GRADIENT command ───────────────────────────────────────────────────────

pub struct GradientCommand {
    outlines: Vec<Vec<[f32; 2]>>,
    mode: Mode,
    manual_pts: Vec<DVec3>,
    missed: bool,
    /// Gradient shape, switchable via the prompt options (#415).
    kind: crate::scene::model::hatch_model::GradientKind,
    /// Swap the two colour stops.
    invert: bool,
}

impl GradientCommand {
    pub fn new(outlines: Vec<Vec<[f32; 2]>>) -> Self {
        Self {
            outlines,
            mode: Mode::PickInside,
            manual_pts: vec![],
            missed: false,
            kind: crate::scene::model::hatch_model::GradientKind::Linear,
            invert: false,
        }
    }

    fn make_hatch(&self, rings: Vec<Vec<[f64; 2]>>) -> HatchModel {
        let (rel, origin, wcs) = pack_rings(&rings);
        HatchModel {
            boundary: std::sync::Arc::new(rel),
            pattern: HatchPattern::Gradient {
                angle_deg: 0.0,
                color2: [0.18, 0.18, 0.18, 0.0],
                kind: self.kind,
                invert: self.invert,
            },
            name: self.kind.dxf_name(self.invert).into(),
            color: [0.30, 0.60, 0.95, 0.80],
            angle_offset: 0.0,
            scale: 1.0,
            world_origin: origin,
            boundary_wcs: Some(std::sync::Arc::new(wcs)),
            draw_depth: 0.0,
        }
    }
}

impl CadCommand for GradientCommand {
    fn name(&self) -> &'static str {
        "GRADIENT"
    }

    fn prompt(&self) -> String {
        match &self.mode {
            Mode::PickInside => {
                let miss = if self.missed {
                    "  ⚠ No closed boundary found."
                } else {
                    ""
                };
                format!(
                    "GRADIENT ({}{})  Pick internal point:{miss}",
                    self.kind.label(),
                    if self.invert { ", inverted" } else { "" }
                )
            }
            Mode::Manual => {
                if self.manual_pts.is_empty() {
                    "GRADIENT  Boundary point 1:".into()
                } else {
                    format!("GRADIENT  Point {}:", self.manual_pts.len() + 1)
                }
            }
        }
    }

    fn options(&self) -> Vec<crate::command::CmdOption> {
        use crate::command::CmdOption;
        match &self.mode {
            Mode::PickInside => {
                let mut opts = vec![CmdOption::new("Draw manually", "S")];
                for k in crate::scene::model::hatch_model::GradientKind::ALL {
                    if k != self.kind {
                        opts.push(CmdOption::new(k.label(), k.label()));
                    }
                }
                opts.push(CmdOption::new(
                    if self.invert { "Invert: on" } else { "Invert: off" },
                    "I",
                ));
                opts
            }
            Mode::Manual => {
                // Enter accepts the boundary once at least 3 points are picked.
                if self.manual_pts.len() >= 3 {
                    vec![CmdOption::enter("Accept")]
                } else {
                    vec![]
                }
            }
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match &self.mode {
            Mode::PickInside => {
                // Hit-test against the f32 outline catalog (screen-space).
                let xy = [pt.x as f32, pt.y as f32];
                match resolve_hatch_rings(&self.outlines, xy) {
                    Some(rings) => {
                        self.missed = false;
                        return CmdResult::CommitHatch(self.make_hatch(rings));
                    }
                    None => {
                        self.missed = true;
                        CmdResult::NeedPoint
                    }
                }
            }
            Mode::Manual => {
                // Keep the typed/snapped point exact (issue #311).
                self.manual_pts.push(pt);
                CmdResult::NeedPoint
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        match &self.mode {
            Mode::PickInside => CmdResult::Cancel,
            Mode::Manual => {
                if self.manual_pts.len() < 3 {
                    return CmdResult::Cancel;
                }
                let wcs = self.manual_pts.iter().map(|p| [p.x, p.y]).collect();
                CmdResult::CommitHatch(self.make_hatch(vec![wcs]))
            }
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn wants_text_input(&self) -> bool {
        matches!(self.mode, Mode::PickInside)
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let t = text.trim();
        if t.eq_ignore_ascii_case("s") {
            self.mode = Mode::Manual;
            self.missed = false;
            return Some(CmdResult::NeedPoint);
        }
        // Gradient type keywords / buttons + the invert toggle (#415).
        if t.eq_ignore_ascii_case("i") || t.eq_ignore_ascii_case("invert") {
            self.invert = !self.invert;
            return Some(CmdResult::NeedPoint);
        }
        if let Some(k) = crate::scene::model::hatch_model::GradientKind::ALL
            .iter()
            .copied()
            .find(|k| k.label().eq_ignore_ascii_case(t))
        {
            self.kind = k;
            return Some(CmdResult::NeedPoint);
        }
        None
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> { let pt = pt.as_vec3();
        if let Mode::Manual = &self.mode {
            if self.manual_pts.is_empty() {
                return None;
            }
            let mut pts: Vec<[f32; 3]> = self
                .manual_pts
                .iter()
                .map(|p| [p.x as f32, p.y as f32, p.z as f32])
                .collect();
            pts.push([pt.x, pt.y, pt.z]);
            pts.push([
                self.manual_pts[0].x as f32,
                self.manual_pts[0].y as f32,
                self.manual_pts[0].z as f32,
            ]);
            return Some(WireModel::solid(
                "rubber_band".into(),
                pts,
                WireModel::CYAN,
                false,
            ));
        }
        None
    }
}

// ── BOUNDARY command ───────────────────────────────────────────────────────

pub struct BoundaryCommand {
    outlines: Vec<Vec<[f32; 2]>>,
    missed: bool,
}

impl BoundaryCommand {
    pub fn new(outlines: Vec<Vec<[f32; 2]>>) -> Self {
        Self {
            outlines,
            missed: false,
        }
    }
}

impl CadCommand for BoundaryCommand {
    fn name(&self) -> &'static str {
        "BOUNDARY"
    }

    fn prompt(&self) -> String {
        let miss = if self.missed {
            "  ⚠ No closed boundary found."
        } else {
            ""
        };
        format!("BOUNDARY  Pick internal point:{miss}")
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        // Hit-test against the f32 outline catalog (screen-space).
        let xy = [pt.x as f32, pt.y as f32];
        match resolve_hatch_rings(&self.outlines, xy) {
            Some(rings) => {
                self.missed = false;
                // Store as a Hatch entity (solid fill) so it is selectable.
                let (rel, origin, wcs) = pack_rings(&rings);
                let model = HatchModel {
                    boundary: std::sync::Arc::new(rel),
                    pattern: HatchPattern::Solid,
                    name: "SOLID".into(),
                    color: [0.45, 0.45, 0.45, 0.60],
                    angle_offset: 0.0,
                    scale: 1.0,
                    world_origin: origin,
                    boundary_wcs: Some(std::sync::Arc::new(wcs)),
                    draw_depth: 0.0,
                };
                CmdResult::CommitHatch(model)
            }
            None => {
                self.missed = true;
                CmdResult::NeedPoint
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["BOUNDARY"] });  // BoundaryCommand
inventory::submit!(crate::command::CommandRegistration { names: &["GRADIENT"] });  // GradientCommand
inventory::submit!(crate::command::CommandRegistration { names: &["HATCH"] });  // HatchCommand

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    // Two nested rectangles, regardless of draw order, the resolution must be
    // deterministic and independent of which was drawn first.
    fn nested(draw_order: bool) -> Vec<Vec<[f32; 2]>> {
        let big = rect(-10.0, -10.0, 10.0, 10.0);
        let small = rect(-5.0, -5.0, 5.0, 5.0);
        if draw_order {
            vec![big, small]
        } else {
            vec![small, big]
        }
    }

    #[test]
    fn click_inside_small_hatches_only_small() {
        for order in [true, false] {
            let rings = resolve_hatch_rings(&nested(order), [0.0, 0.0]).unwrap();
            // Exactly one ring (no hole) and it is the small rectangle.
            assert_eq!(rings.len(), 1, "order {order}");
            assert_eq!(rings[0].len(), 4);
            assert!((rings[0][0][0] - (-5.0)).abs() < 1e-9, "order {order}");
        }
    }

    #[test]
    fn click_between_hatches_ring_with_hole() {
        for order in [true, false] {
            let rings = resolve_hatch_rings(&nested(order), [8.0, 0.0]).unwrap();
            // Outer ring + the small rectangle as a hole.
            assert_eq!(rings.len(), 2, "order {order}");
            // Outer is the big rectangle.
            assert!((rings[0][0][0] - (-10.0)).abs() < 1e-9, "order {order}");
            // Hole is the small rectangle.
            assert!((rings[1][0][0] - (-5.0)).abs() < 1e-9, "order {order}");
        }
    }

    #[test]
    fn click_outside_returns_none() {
        assert!(resolve_hatch_rings(&nested(true), [50.0, 50.0]).is_none());
    }

    #[test]
    fn three_nested_levels() {
        let a = rect(-30.0, -30.0, 30.0, 30.0);
        let b = rect(-15.0, -15.0, 15.0, 15.0);
        let c = rect(-5.0, -5.0, 5.0, 5.0);
        // Click in the middle ring (between b and c).
        let rings = resolve_hatch_rings(&[a.clone(), b.clone(), c.clone()], [10.0, 0.0]).unwrap();
        assert_eq!(rings.len(), 2, "middle ring fill with inner hole");
        // Click inside the innermost.
        let rings = resolve_hatch_rings(&[a, b, c], [0.0, 0.0]).unwrap();
        assert_eq!(rings.len(), 1, "innermost fill has no hole");
    }

    #[test]
    fn click_outer_band_only_direct_child_is_hole() {
        let a = rect(-30.0, -30.0, 30.0, 30.0);
        let b = rect(-15.0, -15.0, 15.0, 15.0);
        let c = rect(-5.0, -5.0, 5.0, 5.0);
        // Click in the outermost band (between a and b): fill = a with only its
        // direct child b as a hole. The grandchild c must be excluded — adding
        // it would flip the innermost square back on under even-odd fill.
        let rings = resolve_hatch_rings(&[a, b, c], [20.0, 0.0]).unwrap();
        assert_eq!(rings.len(), 2, "outer band = a with b as its only hole");
        assert!((rings[0][0][0] - (-30.0)).abs() < 1e-9, "outer ring is a");
        assert!((rings[1][0][0] - (-15.0)).abs() < 1e-9, "hole is direct child b");
    }
}
