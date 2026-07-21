//! OpenCADStudio-style object snap (OSNAP) engine.
//!
//! Implemented modes:
//!   Endpoint, Midpoint, Center, Node, Quadrant, Intersection,
//!   Extension, Insertion, Perpendicular, Nearest, ApparentIntersection, Grid, Tangent

use glam::{Mat4, Vec3};
use iced::time::Instant;
use iced::{Point, Rectangle};

use crate::command::TangentObject;
use crate::scene::model::wire_model::{SnapHint, TangentGeom, WireModel};
use crate::ui::overlay::CROSSHAIR_ARM;

// ── Snap type ─────────────────────────────────────────────────────────────

/// Every OSNAP mode — mirrors the OpenCADStudio list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapType {
    Endpoint,
    Midpoint,
    Center,
    Node,
    Quadrant,
    Intersection,
    Extension,
    Insertion,
    Perpendicular,
    Tangent,
    Nearest,
    ApparentIntersection,
    Parallel,
    Grid,
    /// Object acquisition (domain-object pick, e.g. network structure) — orange marker.
    ObjectPick,
}

/// Ordered list used by the popup and snap engine.
pub const ALL_SNAP_MODES: &[(SnapType, &str, &str)] = &[
    (SnapType::Endpoint, "◻", "Endpoint"),
    (SnapType::Midpoint, "△", "Midpoint"),
    (SnapType::Center, "◯", "Center"),
    (SnapType::Node, "◆", "Node"),
    (SnapType::Quadrant, "◇", "Quadrant"),
    (SnapType::Intersection, "✕", "Intersection"),
    (SnapType::Extension, "—", "Extension"),
    (SnapType::Insertion, "⊾", "Insertion"),
    (SnapType::Perpendicular, "⊥", "Perpendicular"),
    (SnapType::Tangent, "⌒", "Tangent"),
    (SnapType::Nearest, "✧", "Nearest"),
    (SnapType::ApparentIntersection, "✗", "Apparent Intersection"),
    (SnapType::Parallel, "∥", "Parallel"),
    // NOTE: Grid is intentionally NOT an object-snap mode. Grid snap is a
    // separate system (`Snapper::grid_snap_on`) so object snap never catches a
    // grid point; it is toggled on its own and handled directly in `snap()`.
];

// ── Snap result ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct SnapResult {
    pub world: glam::DVec3,
    pub screen: Point,
    pub snap_type: SnapType,
    /// Set when `snap_type == Tangent`; provides entity geometry for TTR/TTT.
    pub tangent_obj: Option<TangentObject>,
    /// Screen position of the endpoint an Extension snap extends from, so the
    /// overlay can draw the dashed extension guide line back to it. `None` for
    /// every other snap type. (#238)
    pub extension_base: Option<Point>,
    /// Second extension-guide base, set only for an extended intersection
    /// (`snap_type == Intersection` where two extension lines cross): the
    /// overlay draws a dashed guide from each base to the crossing so both
    /// contributing extensions stay visible. `None` otherwise. (#247, #259)
    pub extension_base2: Option<Point>,
}

/// Object-snap-tracking alignment: the cursor projected onto a ray from an
/// acquired tracking point.
#[derive(Debug, Clone, Copy)]
pub struct OtrackHit {
    /// Cursor projected onto the tracking ray.
    pub aligned: Vec3,
    /// Unit ray direction toward the cursor side (for typed-distance entry).
    pub dir: Vec3,
    /// The tracking point the ray emanates from.
    pub base: Vec3,
}

// ── Snapper ───────────────────────────────────────────────────────────────

use rustc_hash::FxHashSet as HashSet;

pub struct Snapper {
    /// Global snap on/off toggle.  When false, all snapping is bypassed
    /// but the `enabled` set is preserved so it can be restored.
    pub snap_enabled: bool,
    /// Which snap modes are configured (used when `snap_enabled` is true).
    pub enabled: HashSet<SnapType>,
    /// Grid snap on/off — a system fully separate from object snap. When on,
    /// `snap()` can pick the nearest grid corner; object snap never does.
    pub grid_snap_on: bool,
    /// World-space grid spacing.
    pub grid_spacing: f32,
    /// Pixel-radius snap aperture, shared by OSNAP, tracking, polar and
    /// extension so the catch distance is the same everywhere.
    pub osnap_radius_px: f32,
    /// Object Snap Tracking on/off (F11).
    pub otrack_enabled: bool,
    /// Acquired OST points (world XZ, Y=0 plane).
    pub tracking_points: Vec<Vec3>,
    /// Edge directions at each acquired point (parallel to `tracking_points`):
    /// the line direction of every wire segment meeting at that corner, so
    /// OTRACK can offer an alignment ray along a segment's extension, not only
    /// the ortho/polar axes. Pulling the cursor along an acquired corner's edge
    /// then locks to that line (#219). Empty for a point that is not a segment
    /// endpoint (e.g. a midpoint or centre acquisition).
    pub tracking_dirs: Vec<Vec<Vec3>>,
    /// Last snap world position (for dwell detection).
    pub last_snap_world: Option<Vec3>,
    /// When the cursor first rested near `last_snap_world`.
    pub dwell_since: Option<Instant>,
    /// Whether the current dwell already acquired/removed a point (fire once).
    pub dwell_acquired: bool,
    /// The point the in-progress command is drawing *from* (the rubber-band
    /// origin), if any. Perpendicular snap drops its foot from here so the new
    /// segment is genuinely perpendicular to the target — without it, perp
    /// would just give the nearest point on the line. Set before each `snap`.
    pub from_point: Option<Vec3>,
    /// Parallel snap: the acquired reference as (unit direction, a point on the
    /// line). When the cursor's direction from the command's start point runs
    /// parallel to this, the point locks onto that parallel line; the point half
    /// marks the reference on screen. Works with only the Parallel object snap
    /// on — independent of OTRACK (#277).
    pub parallel_ref: Option<(Vec3, Vec3)>,
    /// Dwell state for acquiring/removing `parallel_ref`: the candidate line
    /// direction + point under the cursor, when it was first hovered, and
    /// whether this dwell has already fired (so it acquires/toggles once).
    parallel_dwell: Option<(Vec3, Vec3, Instant, bool)>,
}

impl Default for Snapper {
    fn default() -> Self {
        let mut enabled = HashSet::default();
        enabled.insert(SnapType::Endpoint);
        enabled.insert(SnapType::Midpoint);
        enabled.insert(SnapType::Center);
        enabled.insert(SnapType::Node);
        enabled.insert(SnapType::Quadrant);
        enabled.insert(SnapType::Intersection);
        enabled.insert(SnapType::Nearest);
        Self {
            snap_enabled: false,
            enabled,
            grid_snap_on: false,
            grid_spacing: 1.0,
            osnap_radius_px: CROSSHAIR_ARM * 0.25,
            otrack_enabled: false,
            tracking_points: Vec::new(),
            tracking_dirs: Vec::new(),
            last_snap_world: None,
            dwell_since: None,
            dwell_acquired: false,
            from_point: None,
            parallel_ref: None,
            parallel_dwell: None,
        }
    }
}

impl Snapper {
    /// True when snap is globally on AND at least one mode is configured.
    pub fn is_active(&self) -> bool {
        self.snap_enabled && !self.enabled.is_empty()
    }

    pub fn is_on(&self, t: SnapType) -> bool {
        self.enabled.contains(&t)
    }

    /// Whether temporary tracking points are being acquired and drawn: OTRACK
    /// on, or the Extension object snap on. Extension tracks a segment's line
    /// only from an acquired endpoint, and works independently of OTRACK's
    /// on/off state (#262).
    pub fn tracking_active(&self) -> bool {
        self.otrack_enabled || (self.snap_enabled && self.is_on(SnapType::Extension))
    }

    /// Whether an alignment guide may be on screen — tracking (above) OR a
    /// Parallel lock. Used to gate the guide-line projection; Parallel draws its
    /// alignment guide without acquiring tracking points. (#277)
    pub fn alignment_active(&self) -> bool {
        self.tracking_active() || (self.snap_enabled && self.is_on(SnapType::Parallel))
    }

    /// True when `p` coincides with one of the acquired temporary tracking
    /// points. Extension snaps a segment's line only from such acquired
    /// endpoints (#262), so extensions aren't live for every object in the
    /// drawing. The tolerance mirrors `edge_dirs_at`: acquired points are f32
    /// truncations of the true vertices, so the match window scales with
    /// coordinate magnitude with a tight floor near the origin.
    fn is_tracked_endpoint(&self, p: glam::DVec3) -> bool {
        if self.tracking_points.is_empty() {
            return false;
        }
        let pf = p.as_vec3();
        let tol = 1e-4_f32.max(4e-7 * pf.x.abs().max(pf.y.abs()));
        let tol2 = tol * tol;
        self.tracking_points.iter().any(|t| {
            let dx = t.x - pf.x;
            let dy = t.y - pf.y;
            dx * dx + dy * dy < tol2
        })
    }

    pub fn toggle_global(&mut self) {
        self.snap_enabled = !self.snap_enabled;
    }

    /// Grid snap on/off — independent of the object-snap master and mode set.
    pub fn grid_snap(&self) -> bool {
        self.grid_snap_on
    }

    pub fn toggle_grid_snap(&mut self) {
        self.grid_snap_on = !self.grid_snap_on;
    }

    pub fn toggle(&mut self, t: SnapType) {
        if !self.enabled.remove(&t) {
            self.enabled.insert(t);
        }
    }

    pub fn all_on(&self) -> bool {
        ALL_SNAP_MODES
            .iter()
            .all(|(t, _, _)| self.enabled.contains(t))
    }
    pub fn none_on(&self) -> bool {
        self.enabled.is_empty()
    }

    pub fn enable_all(&mut self) {
        for &(t, _, _) in ALL_SNAP_MODES {
            self.enabled.insert(t);
        }
    }
    pub fn disable_all(&mut self) {
        self.enabled.clear();
    }

    /// Update dwell tracking and possibly acquire a new OST point.
    /// Should be called on every ViewportMove when snap is active.
    /// `snap_world` is the current snap result world point (if any).
    pub fn update_otrack_dwell(
        &mut self,
        snap_world: Option<Vec3>,
        wires: &[WireModel],
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
        now: Instant,
    ) {
        // Temporary tracking points are acquired when OTRACK is on, OR when the
        // Extension object snap is on: Extension tracks a segment's line only
        // from an endpoint the user has acquired, independently of OTRACK's
        // on/off state (#262).
        if !self.tracking_active() {
            self.last_snap_world = None;
            self.dwell_since = None;
            self.dwell_acquired = false;
            return;
        }
        // With OTRACK off, acquisition is Extension-driven, and Extension tracks
        // a line only from a real segment endpoint — so acquire endpoints only.
        // This stops a paused cursor on an extension foot (or a midpoint/centre)
        // from being acquired and evicting, through the 4-point cap, the very
        // endpoint the user acquired — the reason the marker vanished after a
        // few pauses (#262). OTRACK keeps acquiring any snap point.
        let endpoints_only = !self.otrack_enabled;
        // The cursor must rest near a snap point for this long before it is
        // acquired, so that brushing past snap points while moving the mouse
        // does not create accidental tracking points.
        const DWELL_MS: u128 = 250;
        const DWELL_PX: f32 = 8.0;

        match snap_world {
            None => {
                // Leaving all geometry: capture the point we were dwelling on if
                // it qualified, before the reset loses it.
                self.acquire_on_leave(now, DWELL_MS, wires, endpoints_only);
                self.last_snap_world = None;
                self.dwell_since = None;
                self.dwell_acquired = false;
            }
            Some(p) => {
                // Convert to screen to measure pixel distance.
                let is_same = if let Some(prev) = self.last_snap_world {
                    let dp = world_to_screen(p.as_dvec3(), view_rot, eye, bounds);
                    let dp2 = world_to_screen(prev.as_dvec3(), view_rot, eye, bounds);
                    let dx = dp.x - dp2.x;
                    let dy = dp.y - dp2.y;
                    (dx * dx + dy * dy).sqrt() < DWELL_PX
                } else {
                    false
                };
                if is_same {
                    let elapsed = self
                        .dwell_since
                        .map_or(0, |t| now.duration_since(t).as_millis());
                    if !self.dwell_acquired && elapsed >= DWELL_MS {
                        self.dwell_acquired = true;
                        // Dwelling over an already-acquired point removes it;
                        // otherwise acquire it.
                        let existing = self.tracking_points.iter().position(|t| {
                            let d = (*t - p).length();
                            d < self.grid_spacing * 0.1
                        });
                        match existing {
                            Some(idx) => {
                                self.tracking_points.remove(idx);
                                if idx < self.tracking_dirs.len() {
                                    self.tracking_dirs.remove(idx);
                                }
                            }
                            None => self.acquire_tracking_point(p, wires, endpoints_only),
                        }
                    }
                } else {
                    // Moved to a different snap point: capture the previous one
                    // first if it was dwelt on long enough, so a pause-then-drag
                    // gesture reliably acquires it even without in-place events.
                    self.acquire_on_leave(now, DWELL_MS, wires, endpoints_only);
                    self.last_snap_world = Some(p);
                    self.dwell_since = Some(now);
                    self.dwell_acquired = false;
                }
            }
        }
    }

    /// Add `p` as a tracking point (capturing its corner edge directions) unless
    /// it is already tracked; drops the oldest when the 4-point cap is reached.
    /// Edge directions are scanned once here, at acquisition, so OTRACK can align
    /// to a segment's extension without rescanning geometry per move (#219).
    fn acquire_tracking_point(&mut self, p: Vec3, wires: &[WireModel], endpoints_only: bool) {
        if self
            .tracking_points
            .iter()
            .any(|t| (*t - p).length() < self.grid_spacing * 0.1)
        {
            return;
        }
        // Edge directions double as an endpoint test: a point with no incident
        // segment — a midpoint, centre, intersection or extension foot — has
        // none. Extension-driven acquisition (#262) keeps only endpoints.
        let dirs = edge_dirs_at(p, wires);
        if endpoints_only && dirs.is_empty() {
            return;
        }
        if self.tracking_points.len() >= 4 {
            self.tracking_points.remove(0);
            if !self.tracking_dirs.is_empty() {
                self.tracking_dirs.remove(0);
            }
        }
        self.tracking_points.push(p);
        self.tracking_dirs.push(dirs);
    }

    /// If the cursor dwelt on a snap point long enough but the in-place check
    /// never fired (a perfectly still cursor emits no move events, so the timer
    /// is only re-examined once the cursor moves off), acquire it now as the
    /// cursor leaves. This makes "pause on a corner, then drag along its edge"
    /// reliably capture the corner (#219).
    fn acquire_on_leave(
        &mut self,
        now: Instant,
        dwell_ms: u128,
        wires: &[WireModel],
        endpoints_only: bool,
    ) {
        if self.dwell_acquired {
            return; // already handled by the in-place branch
        }
        let Some(prev) = self.last_snap_world else {
            return;
        };
        let elapsed = self
            .dwell_since
            .map_or(0, |t| now.duration_since(t).as_millis());
        if elapsed >= dwell_ms {
            self.acquire_tracking_point(prev, wires, endpoints_only);
        }
    }

    /// Project the cursor onto a tracking ray emanating from one of the
    /// acquired tracking points, in the XY plane. Without `polar_step_deg` the
    /// rays are horizontal / vertical (0° / 90°); with it, every polar
    /// increment is a candidate so the user can track along POLAR angles. Each
    /// acquired corner also contributes a ray along its own edge directions, so
    /// pulling the cursor along a segment's extension locks to that line (#219).
    ///
    /// When the cursor sits near the crossing of two active vectors from
    /// different origins the intersection point wins, so the cursor locks onto
    /// the exact crossing rather than a free point along one vector:
    ///   * two OTRACK vectors from different tracking points (#112), and
    ///   * a POLAR vector from `last_point` crossing an OTRACK vector (#111).
    ///
    /// Returns the aligned point, the unit ray direction (pointing toward the
    /// cursor side, used for typed-distance entry), and the originating point.
    pub fn otrack_snap(
        &self,
        cursor_world: Vec3,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
        polar_step_deg: Option<f32>,
        last_point: Option<Vec3>,
        // Ortho on: the axis from `last_point` is a hard lock. Only crossings of
        // an acquired ray with that axis lock; single tracking rays are
        // suppressed so the cursor can't leave the ortho axis. (#218)
        ortho: bool,
        // UCS→world rotation: tracking rays run along the UCS axes, matching
        // ortho/polar. Identity = world-aligned rays.
        ucs: glam::Mat4,
    ) -> Option<OtrackHit> {
        if !self.otrack_enabled || self.tracking_points.is_empty() {
            return None;
        }

        let cursor_screen = world_to_screen(cursor_world.as_dvec3(), view_rot, eye, bounds);
        // Use the same aperture as OSNAP so the catch distance is uniform.
        let r = self.osnap_radius_px;
        let screen_dist = |w: Vec3| {
            let s = world_to_screen(w.as_dvec3(), view_rot, eye, bounds);
            ((s.x - cursor_screen.x).powi(2) + (s.y - cursor_screen.y).powi(2)).sqrt()
        };

        // Candidate angles in [0,180); each ray extends both ways via the
        // signed projection `t`, so 0°/90° cover horizontal/vertical.
        let mut angles: Vec<f32> = Vec::new();
        match polar_step_deg.filter(|s| *s > 1e-3) {
            Some(step) => {
                let mut a = 0.0_f32;
                while a < 180.0 - 1e-3 {
                    angles.push(a);
                    a += step;
                }
            }
            None => {
                angles.push(0.0);
                angles.push(90.0);
            }
        }

        // Build candidate rays tagged by origin group so two rays sharing an
        // origin (a parallel pencil that only meets at that origin) are never
        // intersected with each other.
        struct Ray {
            origin: Vec3,
            dir: Vec3,
            group: usize,
        }
        let mut rays: Vec<Ray> = Vec::new();
        for (gi, &tp) in self.tracking_points.iter().enumerate() {
            for &adeg in &angles {
                let ar = adeg.to_radians();
                rays.push(Ray {
                    origin: tp,
                    dir: ucs.transform_vector3(Vec3::new(ar.cos(), ar.sin(), 0.0)),
                    group: gi,
                });
            }
            // Extension rays along the corner's own edges (world-space geometry
            // directions — already oriented, no UCS rotation). Included in the
            // single-ray set so pulling the cursor along a segment's extension
            // locks to it. (#219)
            if let Some(edirs) = self.tracking_dirs.get(gi) {
                for &d in edirs {
                    rays.push(Ray {
                        origin: tp,
                        dir: d,
                        group: gi,
                    });
                }
            }
            // Ray from the command's base point toward this acquired corner:
            // once the first point is placed, the cursor can lock onto the line
            // joining that base point and an existing corner — the direction
            // *between* them, not only the ortho/polar axes or the corner's own
            // edges. Grouped with the corner so it never self-intersects with
            // that corner's other rays (they meet only at the corner).
            if let Some(bp) = last_point {
                let d = tp - bp;
                let len2 = d.x * d.x + d.y * d.y;
                if len2 > 1e-12 {
                    let inv = 1.0 / len2.sqrt();
                    rays.push(Ray {
                        origin: bp,
                        dir: Vec3::new(d.x * inv, d.y * inv, 0.0),
                        group: gi,
                    });
                }
            }
        }
        // OTRACK rays come first; the auxiliary rays appended below (polar from
        // last_point, ortho axis from last_point) only participate in
        // intersection locking, never in single-ray fallback.
        let otrack_ray_count = rays.len();
        const POLAR_GROUP: usize = usize::MAX;
        const ORTHO_GROUP: usize = usize::MAX - 1;
        if let (Some(step), Some(lp)) = (polar_step_deg.filter(|s| *s > 1e-3), last_point) {
            let mut a = 0.0_f32;
            while a < 180.0 - 1e-3 {
                let ar = a.to_radians();
                rays.push(Ray {
                    origin: lp,
                    dir: ucs.transform_vector3(Vec3::new(ar.cos(), ar.sin(), 0.0)),
                    group: POLAR_GROUP,
                });
                a += step;
            }
        }
        // Ortho axis rays from `last_point`, so a tracking ray crossing the
        // ortho axis locks on-axis (the useful corner-finding case). (#218)
        let ortho_lock = ortho && last_point.is_some();
        if let (true, Some(lp)) = (ortho_lock, last_point) {
            for &adeg in &[0.0_f32, 90.0] {
                let ar = adeg.to_radians();
                rays.push(Ray {
                    origin: lp,
                    dir: ucs.transform_vector3(Vec3::new(ar.cos(), ar.sin(), 0.0)),
                    group: ORTHO_GROUP,
                });
            }
        }

        // ── Intersection lock — crossing of two vectors from distinct origins.
        let mut best_x: Option<(f32, OtrackHit)> = None;
        for i in 0..rays.len() {
            for j in (i + 1)..rays.len() {
                if rays[i].group == rays[j].group {
                    continue;
                }
                // Under an ortho lock only crossings that involve the ortho axis
                // are valid — every other crossing lies off it. (#218)
                if ortho_lock && rays[i].group != ORTHO_GROUP && rays[j].group != ORTHO_GROUP {
                    continue;
                }
                let Some(x) =
                    line_intersect_xy(rays[i].origin, rays[i].dir, rays[j].origin, rays[j].dir)
                else {
                    continue;
                };
                let sd = screen_dist(x);
                if sd < r && best_x.as_ref().map_or(true, |(bd, _)| sd < *bd) {
                    // Report an acquired tracking ray (not an auxiliary
                    // last_point ray) as base/dir for typed-distance entry.
                    let ot = if rays[i].group != POLAR_GROUP && rays[i].group != ORTHO_GROUP {
                        &rays[i]
                    } else {
                        &rays[j]
                    };
                    let t = (x.x - ot.origin.x) * ot.dir.x + (x.y - ot.origin.y) * ot.dir.y;
                    let dir_out = if t >= 0.0 { ot.dir } else { -ot.dir };
                    best_x = Some((
                        sd,
                        OtrackHit {
                            aligned: x,
                            dir: dir_out,
                            base: ot.origin,
                        },
                    ));
                }
            }
        }
        if let Some((_, h)) = best_x {
            return Some(h);
        }

        // With Ortho on and a base point, the axis is a hard lock: no single
        // tracking ray may pull the cursor off it. Only crossings with the
        // ortho axis (handled above) lock; otherwise defer to the caller's
        // ortho constraint. (#218)
        if ortho_lock {
            return None;
        }

        // ── Single-ray alignment (OTRACK rays only) ──
        let mut best: Option<(f32, OtrackHit)> = None;
        for ray in rays.iter().take(otrack_ray_count) {
            let t = (cursor_world.x - ray.origin.x) * ray.dir.x
                + (cursor_world.y - ray.origin.y) * ray.dir.y;
            let aligned = Vec3::new(
                ray.origin.x + ray.dir.x * t,
                ray.origin.y + ray.dir.y * t,
                ray.origin.z,
            );
            let sd = screen_dist(aligned);
            if sd < r && best.as_ref().map_or(true, |(bd, _)| sd < *bd) {
                let dir_out = if t >= 0.0 { ray.dir } else { -ray.dir };
                best = Some((
                    sd,
                    OtrackHit {
                        aligned,
                        dir: dir_out,
                        base: ray.origin,
                    },
                ));
            }
        }
        best.map(|(_, h)| h)
    }

    /// Clear all acquired tracking points (e.g. when command ends).
    pub fn clear_tracking(&mut self) {
        self.tracking_points.clear();
        self.tracking_dirs.clear();
        self.parallel_ref = None;
        self.parallel_dwell = None;
        self.last_snap_world = None;
        self.dwell_since = None;
        self.dwell_acquired = false;
    }

    /// Parallel snap acquisition. When the Parallel object snap is on, hovering
    /// a line (or polyline segment) for a short dwell acquires it as the parallel
    /// reference (its direction + a point on it, which marks it on screen); the
    /// reference persists once the cursor moves off so the user can then draw
    /// parallel to it. Dwelling on the SAME reference line again removes it
    /// (toggle). Curves (circle/arc/ellipse) are ignored — "parallel to a curve"
    /// is undefined. Call on every viewport move. (#277)
    pub fn update_parallel(
        &mut self,
        cursor_world: Vec3,
        wires: &[WireModel],
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
        now: Instant,
    ) {
        if !(self.snap_enabled && self.is_on(SnapType::Parallel)) {
            self.parallel_ref = None;
            self.parallel_dwell = None;
            return;
        }
        const PAR_DWELL_MS: u128 = 150;
        let parallel = |a: Vec3, b: Vec3| (a.x * b.x + a.y * b.y).abs() > 0.9998;
        let Some((dir, pt)) =
            nearest_segment(cursor_world, wires, view_rot, eye, bounds, self.osnap_radius_px)
        else {
            // Off all lines: drop the in-progress candidate, keep the reference.
            self.parallel_dwell = None;
            return;
        };
        // Restart the dwell when the hovered line changes (different direction,
        // or a parallel line far from the candidate's point on screen).
        let same_candidate = self.parallel_dwell.map_or(false, |(cd, cp, _, _)| {
            parallel(cd, dir)
                && screen_perp_dist(pt, cp, cd, view_rot, eye, bounds) < self.osnap_radius_px
        });
        match self.parallel_dwell {
            Some((cd, cp, since, fired)) if same_candidate => {
                if !fired && now.duration_since(since).as_millis() >= PAR_DWELL_MS {
                    // Dwelt long enough: acquire this line, or remove it if it is
                    // already the reference (hovering it a second time toggles).
                    let is_ref = self.parallel_ref.map_or(false, |(rd, rp)| {
                        parallel(rd, dir)
                            && screen_perp_dist(pt, rp, rd, view_rot, eye, bounds)
                                < self.osnap_radius_px
                    });
                    self.parallel_ref = if is_ref { None } else { Some((dir, pt)) };
                    self.parallel_dwell = Some((cd, cp, since, true));
                }
            }
            _ => self.parallel_dwell = Some((dir, pt, now, false)),
        }
    }

    /// Parallel lock: if the cursor's direction from `base` runs parallel to the
    /// acquired reference, snap the point onto the line through `base` parallel
    /// to the reference (locking when the cursor sits within the snap aperture
    /// of that line). Independent of OTRACK. (#277)
    pub fn parallel_snap(
        &self,
        cursor_world: Vec3,
        base: Option<Vec3>,
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
    ) -> Option<SnapResult> {
        if !(self.snap_enabled && self.is_on(SnapType::Parallel)) {
            return None;
        }
        let (dir, _) = self.parallel_ref?;
        let base = base?;
        let d = cursor_world - base;
        // Need a bit of travel from the base, and the cursor must be pulling
        // roughly along the reference (not backward-only noise near the base).
        if (d.x * d.x + d.y * d.y).sqrt() < self.grid_spacing * 0.01 {
            return None;
        }
        let t = d.x * dir.x + d.y * dir.y;
        let locked = base + dir * t;
        let sl = world_to_screen(locked.as_dvec3(), view_rot, eye, bounds);
        let sc = world_to_screen(cursor_world.as_dvec3(), view_rot, eye, bounds);
        if dist2(sl, sc) > self.osnap_radius_px * self.osnap_radius_px {
            return None; // cursor not near the parallel line — don't lock
        }
        Some(SnapResult {
            world: locked.as_dvec3(),
            screen: sl,
            snap_type: SnapType::Parallel,
            tangent_obj: None,
            extension_base: None,
            extension_base2: None,
        })
    }

    /// Only runs Tangent snap — used when a command needs object picks via tangent.
    pub fn snap_tangent_only(
        &self,
        cursor_world: Vec3,
        cursor_screen: Point,
        wires: &[WireModel],
        view_rot: Mat4,
        eye: glam::DVec3,
        bounds: Rectangle,
    ) -> Option<SnapResult> {
        let tmp = Snapper {
            snap_enabled: true,
            enabled: {
                let mut s = HashSet::default();
                s.insert(SnapType::Tangent);
                s
            },
            grid_snap_on: false,
            grid_spacing: self.grid_spacing,
            osnap_radius_px: self.osnap_radius_px,
            otrack_enabled: false,
            tracking_points: Vec::new(),
            tracking_dirs: Vec::new(),
            last_snap_world: None,
            dwell_since: None,
            dwell_acquired: false,
            from_point: None,
            parallel_ref: None,
            parallel_dwell: None,
        };
        // Tangent-only: Grid is disabled here, so the grid basis is irrelevant.
        tmp.snap(
            cursor_world.as_dvec3(),
            cursor_screen,
            wires,
            view_rot,
            eye,
            bounds,
            Vec3::ZERO,
            Mat4::IDENTITY,
        )
    }

    /// Find the best snap candidate near the cursor.
    pub fn snap(
        &self,
        cursor_world: glam::DVec3,
        cursor_screen: Point,
        wires: &[WireModel],
        view_rot: Mat4,
        eye: glam::DVec3,
        bounds: Rectangle,
        // Grid origin (render/wire space) and UCS→world rotation, so grid snap
        // lands on the UCS grid the user sees. `(ZERO, IDENTITY)` = world grid.
        grid_origin: Vec3,
        grid_rot: Mat4,
    ) -> Option<SnapResult> {
        // Object-snap selection is priority-then-distance, NOT nearest-wins.
        // "Continuous" snaps (Nearest, Perpendicular, …) sit on the geometry
        // and are therefore almost always closer to the cursor than a discrete
        // Endpoint/Midpoint/Center, so a pure-distance pick would let them mask
        // every other enabled snap. Instead a higher-priority snap inside the
        // snap circle wins even when a lower-priority one is closer; distance
        // only breaks ties within the same priority. See #118.
        let radius2 = self.osnap_radius_px * self.osnap_radius_px;
        let mut best: Option<SnapResult> = None;
        let mut best_rank = u8::MAX;
        let mut best_sub = u8::MAX;
        let mut best_d2 = f32::MAX;

        // Reject candidates projecting outside the pane rectangle. The GPU
        // scissors viewport content to exactly `bounds`, but the hit-test wire
        // set reaches past it (the cull keeps a margin and lines run beyond the
        // rect), so without this a snap could land on geometry clipped out of
        // the viewport. `bounds` is the full canvas in model space, so this is a
        // no-op there.
        let in_bounds = |s: Point| -> bool {
            s.x >= 0.0 && s.x <= bounds.width && s.y >= 0.0 && s.y <= bounds.height
        };

        // ── Grid snap — a SEPARATE system from object snap ───────────────────
        // Grid snap has its own toggle (`grid_snap_on`) and is independent of
        // the object-snap master (`snap_enabled`) and the object-snap mode set.
        // Object snaps therefore NEVER catch grid points; only when grid snap is
        // on can a grid corner be picked. It is evaluated first and at the
        // lowest priority, so any object snap inside the aperture overrides it.
        if self.grid_snap_on {
            let s = self.grid_spacing as f64;
            if s.abs() > 1e-9 {
                // Round in the UCS grid frame, then map back to world.
                let ax = grid_rot.transform_vector3(Vec3::X).as_dvec3();
                let ay = grid_rot.transform_vector3(Vec3::Y).as_dvec3();
                let az = grid_rot.transform_vector3(Vec3::Z).as_dvec3();
                let origin = grid_origin.as_dvec3();
                let rel = cursor_world - origin;
                let ux = (rel.dot(ax) / s).round() * s;
                let uy = (rel.dot(ay) / s).round() * s;
                let uz = (rel.dot(az) / s).round() * s;
                let gp = origin + ax * ux + ay * uy + az * uz;
                let screen = world_to_screen(gp, view_rot, eye, bounds);
                let d2 = dist2(screen, cursor_screen);
                if d2 < radius2 && in_bounds(screen) {
                    best = Some(SnapResult {
                        world: gp,
                        screen,
                        snap_type: SnapType::Grid,
                        tangent_obj: None,
                        extension_base: None,
                        extension_base2: None,
                    });
                    best_rank = snap_tier(SnapType::Grid);
                    best_sub = snap_priority(SnapType::Grid);
                    best_d2 = d2;
                }
            }
        }

        // Object snaps are gated by the object-snap master toggle. With it off
        // only the grid result (if any) stands.
        if !self.snap_enabled {
            return best;
        }

        // World-space snap radius — derived from the view scale so wires whose
        // entire extent is clearly outside the snap circle can be skipped cheaply
        // before projecting any of their vertices to screen space.
        // view_proj col-0 x = 2*zoom / viewport_width for an orthographic camera,
        // so scale_x * (width/2) = pixels per world unit.
        let world_snap_r = {
            let s = view_rot.col(0).x.abs() * bounds.width * 0.5;
            if s > 1e-6 {
                self.osnap_radius_px / s
            } else {
                f32::MAX
            }
        };

        // Returns false when the wire's AABB does not overlap the snap circle —
        // safe to skip all vertex work for this wire.
        // UNBOUNDED_AABB (±infinity) passes through automatically without a
        // special-case branch because the arithmetic is exact for infinities.
        let wire_in_range = |wire: &WireModel| -> bool {
            // The AABB is stored in f32, so at UTM-scale coordinates each bound
            // is quantized by up to ~1 ulp (≈ coord × 2⁻²³ ≈ 0.7 m at 5.7e6).
            // When zoomed in hard the snap radius shrinks below that, so the
            // raw f32 bound can wrongly exclude a wire the cursor is on. Pad the
            // test by the bound's own quantization so the cull never rejects a
            // genuinely in-range wire (it only ever over-includes, which the
            // per-vertex screen test below then rejects precisely).
            let mag = wire
                .aabb
                .iter()
                .fold(0.0f32, |m, c| m.max(c.abs()));
            let pad = (mag * f32::EPSILON * 2.0) as f64;
            let r = world_snap_r as f64 + pad;
            cursor_world.x + r >= wire.aabb[0] as f64
                && cursor_world.x - r <= wire.aabb[2] as f64
                && cursor_world.y + r >= wire.aabb[1] as f64
                && cursor_world.y - r <= wire.aabb[3] as f64
        };

        // The per-wire snaps below skip out-of-aperture wires via `wire_in_range`,
        // so they stay O(n) and snap at any zoom. Intersection / ApparentIntersection
        // instead compare pairs of in-range segments — O(k²) — which hangs when the
        // aperture spans the whole drawing. Count the in-range wires once and gate
        // only those pairwise passes; the single-wire snaps always run.
        // Intersection / ApparentIntersection compare every in-range segment
        // against every other — O(total in-range segments²). Gate on the
        // in-range *point* (≈ segment) count, not the wire count: a handful of
        // curved wires, each hundreds of tessellated points, blows the
        // quadratic up even though the wire count looks modest. (A dense survey
        // cursor cell held ~1k wires / ~240k points → 15 s pre-gate.)
        const MAX_PAIRWISE_POINTS: usize = 3_000;
        let mut in_range_pts = 0usize;
        for w in wires.iter().filter(|w| wire_in_range(w)) {
            in_range_pts += w.points.len();
            if in_range_pts > MAX_PAIRWISE_POINTS {
                break;
            }
        }
        let allow_pairwise = in_range_pts <= MAX_PAIRWISE_POINTS;

        let mut try_pt = |world: glam::DVec3, snap_type: SnapType| {
            let screen = world_to_screen(world, view_rot, eye, bounds);
            if !in_bounds(screen) {
                return;
            }
            let d2 = dist2(screen, cursor_screen);
            // `!(d2 < radius2)` (not `d2 >= radius2`) so a NaN distance from
            // degenerate geometry is rejected: with priority selection a NaN
            // would otherwise pass the gate and be chosen on rank alone,
            // feeding a NaN snap point to the renderer. (#118)
            if !(d2 < radius2) {
                return;
            }
            let (tier, sub) = (snap_tier(snap_type), snap_priority(snap_type));
            if snap_better(tier, d2, sub, (best_rank, best_d2, best_sub)) {
                best_rank = tier;
                best_sub = sub;
                best_d2 = d2;
                best = Some(SnapResult {
                    world,
                    screen,
                    snap_type,
                    tangent_obj: None,
                    extension_base: None,
                    extension_base2: None,
                });
            }
        };

        // ── Pre-baked snap points (Center, Node, Quadrant, Insertion) ──────
        for wire in wires {
            for &(world, hint) in &wire.snap_pts {
                let snap_type = match hint {
                    SnapHint::Center => SnapType::Center,
                    SnapHint::Node => SnapType::Node,
                    SnapHint::Quadrant => SnapType::Quadrant,
                    SnapHint::Insertion => SnapType::Insertion,
                    SnapHint::Midpoint => SnapType::Midpoint,
                };
                if self.is_on(snap_type) {
                    try_pt(world, snap_type);
                }
            }
        }

        // ── Endpoint ───────────────────────────────────────────────────────
        if self.is_on(SnapType::Endpoint) {
            for wire in wires {
                if !wire_in_range(wire) {
                    continue;
                }
                if !wire.key_vertices.is_empty() {
                    // Use explicit vertices (Line, LwPolyline): every vertex is an endpoint.
                    for &p in &wire.key_vertices {
                        try_pt(
                            glam::DVec3::new(p[0], p[1], p[2]),
                            SnapType::Endpoint,
                        );
                    }
                } else {
                    // Tessellated curves (Circle, Arc, Ellipse): only an OPEN
                    // one (an arc) has real endpoints. A full circle / ellipse is
                    // closed — its tessellation's first/last is a seam point, not
                    // an endpoint — and is the only tessellated curve that carries
                    // Quadrant snap hints (arcs never do), so emit no Endpoint for
                    // those (#275).
                    let closed = wire
                        .snap_pts
                        .iter()
                        .any(|(_, h)| matches!(h, SnapHint::Quadrant));
                    if !closed {
                        if let Some(&p) = wire.points.first() {
                            try_pt(glam::DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64), SnapType::Endpoint);
                        }
                        if wire.points.len() > 1 {
                            if let Some(&p) = wire.points.last() {
                                try_pt(glam::DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64), SnapType::Endpoint);
                            }
                        }
                    }
                }
            }
        }

        // ── Midpoint ───────────────────────────────────────────────────────
        // Only explicit vertex sets (Line, LwPolyline) contribute per-segment
        // midpoints. Tessellated curves (Circle, Arc, Ellipse, Spline) emit a
        // single `SnapHint::Midpoint` snap_pt where one exists — iterating
        // every chord here would otherwise turn a circle's tessellation into
        // a haze of false midpoint hits. See #34.
        if self.is_on(SnapType::Midpoint) {
            for wire in wires {
                if !wire_in_range(wire) {
                    continue;
                }
                if !wire.key_vertices.is_empty() {
                    for seg in wire.key_vertices.windows(2) {
                        let a = glam::DVec3::new(seg[0][0], seg[0][1], seg[0][2]);
                        let b = glam::DVec3::new(seg[1][0], seg[1][1], seg[1][2]);
                        if a.distance_squared(b) > 1e-12 {
                            try_pt((a + b) * 0.5, SnapType::Midpoint);
                        }
                    }
                }
            }
        }

        // ── Nearest — closest point on any segment (clamped) ──────────────
        if self.is_on(SnapType::Nearest) {
            for wire in wires {
                if !wire_in_range(wire) {
                    continue;
                }
                for i in 0..wire.points.len().saturating_sub(1) {
                    let p = nearest_on_segment(cursor_world, wp_f64(wire, i), wp_f64(wire, i + 1));
                    try_pt(p, SnapType::Nearest);
                }
            }
        }

        // ── Perpendicular — foot of perpendicular from the drawing base ──
        // Drop the foot from the point the command is drawing *from* (so the
        // new segment is truly perpendicular to the target). Only when there
        // is no base point — e.g. picking the very first point — does it fall
        // back to the cursor (a plain nearest-on-line). The candidate is gated
        // on its screen distance to the cursor like every other snap, so it
        // offers when the cursor is near the perpendicular foot. (#118)
        if self.is_on(SnapType::Perpendicular) {
            let q = self.from_point.map(|v| v.as_dvec3()).unwrap_or(cursor_world);
            for wire in wires {
                if !wire_in_range(wire) {
                    continue;
                }
                for i in 0..wire.points.len().saturating_sub(1) {
                    if let Some(foot) = perp_foot(q, wp_f64(wire, i), wp_f64(wire, i + 1)) {
                        try_pt(foot, SnapType::Perpendicular);
                    }
                }
            }
        }

        // ── Intersection — segment-segment intersections (pairwise, gated) ──
        if self.is_on(SnapType::Intersection) && allow_pairwise {
            for i in 0..wires.len() {
                if !wire_in_range(&wires[i]) {
                    continue;
                }
                for j in (i + 1)..wires.len() {
                    if !wire_in_range(&wires[j]) {
                        continue;
                    }
                    for ai in 0..wires[i].points.len().saturating_sub(1) {
                        // S: pre-convert outside inner loop
                        let a0 = wp_f64(&wires[i], ai);
                        let a1 = wp_f64(&wires[i], ai + 1);
                        let a_min_x = a0.x.min(a1.x);
                        let a_max_x = a0.x.max(a1.x);
                        let a_min_y = a0.y.min(a1.y);
                        let a_max_y = a0.y.max(a1.y);
                        for bi in 0..wires[j].points.len().saturating_sub(1) {
                            let b0 = wp_f64(&wires[j], bi);
                            let b1 = wp_f64(&wires[j], bi + 1);
                            // O: tight per-segment AABB overlap cull
                            if a_max_x < b0.x.min(b1.x)
                                || a_min_x > b0.x.max(b1.x)
                                || a_max_y < b0.y.min(b1.y)
                                || a_min_y > b0.y.max(b1.y)
                            {
                                continue;
                            }
                            if let Some(pt) = seg_intersect_3d(a0, a1, b0, b1) {
                                try_pt(pt, SnapType::Intersection);
                            }
                        }
                    }
                }
            }
        }

        // ── Extension — along the extension of a segment beyond endpoints ──
        // Every segment's line can be extended past either endpoint, so a
        // polyline offers an extension off each of its vertices, not just the
        // first and last (#259). Extension is live only from endpoints the user
        // has acquired as temporary tracking points (#262), so with none
        // acquired there is nothing to extend — skip the whole scan. That is the
        // common case and keeps a large drawing responsive.
        if self.is_on(SnapType::Extension) && !self.tracking_points.is_empty() {
            for wire in wires {
                let n = wire.points.len();
                if n < 2 {
                    continue;
                }
                for i in 0..n - 1 {
                    let a = wp_f64(wire, i);
                    let b = wp_f64(wire, i + 1);
                    // NaN sentinels separate sub-paths — skip a segment spanning one.
                    if !a.x.is_finite() || !b.x.is_finite() || (a - b).length_squared() < 1e-18 {
                        continue;
                    }
                    // Only extend from an endpoint the user has acquired as a
                    // temporary tracking point, so the extension isn't live for
                    // every object in the drawing (#262).
                    // Beyond `a`, away from `b`.
                    if self.is_tracked_endpoint(a) {
                        if let Some(ext) = extension_snap(
                            cursor_world,
                            a,
                            a - b,
                            view_rot,
                            eye,
                            bounds,
                            self.osnap_radius_px,
                        ) {
                            try_pt(ext, SnapType::Extension);
                        }
                    }
                    // Beyond `b`, away from `a`.
                    if self.is_tracked_endpoint(b) {
                        if let Some(ext) = extension_snap(
                            cursor_world,
                            b,
                            b - a,
                            view_rot,
                            eye,
                            bounds,
                            self.osnap_radius_px,
                        ) {
                            try_pt(ext, SnapType::Extension);
                        }
                    }
                }
            }

            // Extended intersection: where two segments would cross if their
            // lines were extended. That crossing can be far from both segments,
            // so `wire_in_range` (near-cursor segment) is the wrong gate — gather
            // segments whose *infinite line* passes near the cursor instead, then
            // pair them. A crossing inside both segments is a real Intersection,
            // so skip it here (#247).
            let filter2 = (self.osnap_radius_px * 2.0).powi(2);
            let mut cand: Vec<(glam::DVec3, glam::DVec3)> = Vec::new();
            for wire in wires {
                for k in 0..wire.points.len().saturating_sub(1) {
                    let a0 = wp_f64(wire, k);
                    let a1 = wp_f64(wire, k + 1);
                    if !a0.x.is_finite() || !a1.x.is_finite() {
                        continue;
                    }
                    // Only lines with an acquired endpoint contribute an extended
                    // crossing, matching the per-segment extension gate (#262).
                    if !self.is_tracked_endpoint(a0) && !self.is_tracked_endpoint(a1) {
                        continue;
                    }
                    let s0 = world_to_screen(a0, view_rot, eye, bounds);
                    let s1 = world_to_screen(a1, view_rot, eye, bounds);
                    let ex = s1.x - s0.x;
                    let ey = s1.y - s0.y;
                    let l2 = ex * ex + ey * ey;
                    if l2 < 1e-6 {
                        continue;
                    }
                    // Perpendicular screen distance² from the cursor to the line.
                    let cross = ex * (cursor_screen.y - s0.y) - ey * (cursor_screen.x - s0.x);
                    if cross * cross / l2 <= filter2 {
                        cand.push((a0, a1));
                    }
                }
            }
            for i in 0..cand.len() {
                let (a0, a1) = cand[i];
                let d1 = a1 - a0;
                for &(b0, b1) in cand.iter().skip(i + 1) {
                    let d2 = b1 - b0;
                    let denom = d1.x * d2.y - d1.y * d2.x;
                    if denom.abs() < 1e-12 {
                        continue; // parallel
                    }
                    let t1 = ((b0.x - a0.x) * d2.y - (b0.y - a0.y) * d2.x) / denom;
                    let t2 = ((b0.x - a0.x) * d1.y - (b0.y - a0.y) * d1.x) / denom;
                    if (0.0..=1.0).contains(&t1) && (0.0..=1.0).contains(&t2) {
                        continue; // real crossing — handled by Intersection
                    }
                    // Emit as an Intersection, not an Extension: the crossing is
                    // a distinct point and must outrank the per-segment extension
                    // feet (which sit closer to the cursor on their own lines),
                    // or the cursor would snap to a line instead of the crossing.
                    let pt = glam::DVec3::new(a0.x + t1 * d1.x, a0.y + t1 * d1.y, a0.z);
                    try_pt(pt, SnapType::Intersection);
                }
            }
        }

        // ── Apparent Intersection — screen-space intersections (pairwise, gated) ──
        // L: pre-project each in-range wire's points to screen once, not once per segment pair.
        if self.is_on(SnapType::ApparentIntersection) && allow_pairwise {
            let screen_pts: Vec<Option<Vec<Point>>> = wires
                .iter()
                .map(|w| {
                    if !wire_in_range(w) {
                        return None;
                    }
                    Some(
                        (0..w.points.len())
                            .map(|i| world_to_screen(wp_f64(w, i), view_rot, eye, bounds))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();

            for i in 0..wires.len() {
                let Some(ref si) = screen_pts[i] else {
                    continue;
                };
                for j in (i + 1)..wires.len() {
                    let Some(ref sj) = screen_pts[j] else {
                        continue;
                    };
                    for ai in 0..wires[i].points.len().saturating_sub(1) {
                        let sa0 = si[ai];
                        let sa1 = si[ai + 1];
                        for bi in 0..wires[j].points.len().saturating_sub(1) {
                            let sb0 = sj[bi];
                            let sb1 = sj[bi + 1];
                            if let Some((ta, _)) = seg_intersect_2d(sa0, sa1, sb0, sb1) {
                                let wa0 = wp_f64(&wires[i], ai);
                                let wa1 = wp_f64(&wires[i], ai + 1);
                                try_pt(wa0 + ta as f64 * (wa1 - wa0), SnapType::ApparentIntersection);
                            }
                        }
                    }
                }
            }
        }

        // ── Tangent ────────────────────────────────────────────────────────
        // Operates directly on tangent_geoms geometry — independent of the
        // wire.points rendering structure so polyline segments work correctly.
        if self.is_on(SnapType::Tangent) {
            for wire in wires {
                for tg in &wire.tangent_geoms {
                    let (world_pt, d2) = match tg {
                        TangentGeom::Line { p1, p2 } => {
                            let sp0 = world_to_screen(glam::DVec3::new(p1[0] as f64, p1[1] as f64, p1[2] as f64), view_rot, eye, bounds);
                            let sp1 = world_to_screen(glam::DVec3::new(p2[0] as f64, p2[1] as f64, p2[2] as f64), view_rot, eye, bounds);
                            let d2 = dist2_to_segment(cursor_screen, sp0, sp1);
                            let t = t_on_segment(cursor_screen, sp0, sp1);
                            let w = Vec3::from(*p1) + t * (Vec3::from(*p2) - Vec3::from(*p1));
                            (w, d2)
                        }
                        TangentGeom::Circle { center, radius } => {
                            let cv = Vec3::from(*center);
                            let r = *radius;
                            let sc = world_to_screen(cv.as_dvec3(), view_rot, eye, bounds);
                            let rim = world_to_screen(
                                glam::DVec3::new((cv.x + r) as f64, cv.y as f64, cv.z as f64),
                                view_rot,
                                eye,
                                bounds,
                            );
                            let sr = dist2(sc, rim).sqrt();
                            let dc = dist2(cursor_screen, sc).sqrt();
                            // Trigger by proximity to the circle EDGE (hovering
                            // the circle), independent of where the tangent point
                            // lands.
                            let edge_d = (dc - sr).abs();
                            // A TRUE tangent: from the point the command draws
                            // from (P), the tangent point T on the circle has the
                            // radius CT perpendicular to PT, so T lies at the
                            // half-angle acos(r/|CP|) either side of the C→P
                            // direction. Two solutions — take the one nearer the
                            // cursor. Without P (a deferred tangent picked first)
                            // or with P inside the circle, fall back to the circle
                            // point facing the cursor. Previously this always used
                            // the facing point, so Tangent behaved like Nearest
                            // (#274).
                            let w = self
                                .from_point
                                .and_then(|p| circle_tangent_points(p, cv, r))
                                .map(|(t0, t1)| {
                                    // Two tangents — take the one nearer the cursor.
                                    let s0 = world_to_screen(t0.as_dvec3(), view_rot, eye, bounds);
                                    let s1 = world_to_screen(t1.as_dvec3(), view_rot, eye, bounds);
                                    if dist2(s0, cursor_screen) <= dist2(s1, cursor_screen) {
                                        t0
                                    } else {
                                        t1
                                    }
                                })
                                .unwrap_or_else(|| {
                                    let dx = cursor_screen.x - sc.x;
                                    let dy = cursor_screen.y - sc.y;
                                    let dl = (dx * dx + dy * dy).sqrt();
                                    let (nx, ny) =
                                        if dl > 1e-6 { (dx / dl, -dy / dl) } else { (1.0, 0.0) };
                                    Vec3::new(cv.x + r * nx, cv.y + r * ny, cv.z)
                                });
                            (w, edge_d * edge_d)
                        }
                    };
                    let (tier, sub) =
                        (snap_tier(SnapType::Tangent), snap_priority(SnapType::Tangent));
                    let screen_pt = world_to_screen(world_pt.as_dvec3(), view_rot, eye, bounds);
                    if d2 < radius2
                        && in_bounds(screen_pt)
                        && snap_better(tier, d2, sub, (best_rank, best_d2, best_sub))
                    {
                        best_rank = tier;
                        best_sub = sub;
                        best_d2 = d2;
                        let tangent_obj = match tg {
                            TangentGeom::Line { p1, p2 } => TangentObject::Line {
                                p1: glam::DVec3::new(p1[0] as f64, p1[1] as f64, p1[2] as f64),
                                p2: glam::DVec3::new(p2[0] as f64, p2[1] as f64, p2[2] as f64),
                            },
                            TangentGeom::Circle { center, radius } => TangentObject::Circle {
                                center: glam::DVec3::new(center[0] as f64, center[1] as f64, center[2] as f64),
                                radius: *radius as f64,
                            },
                        };
                        best = Some(SnapResult {
                            world: world_pt.as_dvec3(),
                            screen: screen_pt,
                            snap_type: SnapType::Tangent,
                            tangent_obj: Some(tangent_obj),
                            extension_base: None,
                            extension_base2: None,
                        });
                    }
                }
            }
        }

        // ── Center via curve proximity ─────────────────────────────────────
        // A circle/arc/ellipse's centre is offset from its curve — for an arc
        // it usually sits in empty space well off the geometry — so gating the
        // Center snap purely on the cursor's distance to the centre *point*
        // (the pre-baked pass above) means hovering the curve, the natural
        // gesture, never offers it. Mirror running-osnap behaviour: when the
        // cursor is near such a curve, offer its centre, ranked by how close
        // the cursor is to the curve. Runs here, after `try_pt`'s borrow ends,
        // so it can update the candidate state directly. (#152)
        if self.is_on(SnapType::Center) {
            for wire in wires {
                if !wire_in_range(wire) {
                    continue;
                }
                // Only tessellated curves carry a pre-baked Center hint; reuse
                // it as the snap target. Lines / polylines have none → skip.
                let Some(center) = wire
                    .snap_pts
                    .iter()
                    .find(|(_, h)| matches!(h, SnapHint::Center))
                    .map(|&(c, _)| c)
                else {
                    continue;
                };
                // Nearest screen distance from the cursor to the curve itself.
                let mut curve_d2 = f32::INFINITY;
                for i in 0..wire.points.len().saturating_sub(1) {
                    let p = nearest_on_segment(cursor_world, wp_f64(wire, i), wp_f64(wire, i + 1));
                    let sp = world_to_screen(p, view_rot, eye, bounds);
                    curve_d2 = curve_d2.min(dist2(sp, cursor_screen));
                }
                let screen = world_to_screen(center, view_rot, eye, bounds);
                // Rim-hover Center: the cursor is on the CURVE, not near the
                // centre, so its distance metric (curve_d2 ≈ 0 anywhere on the
                // rim) must not compete inside the discrete tier — it would
                // mask the circle's own Quadrants at every rim position
                // (#420). Tier 1 sits below every discrete point snap and
                // above the continuous ones; the pre-baked Center point (true
                // cursor-to-centre distance) stays in tier 0.
                let (tier, sub) = (1u8, snap_priority(SnapType::Center));
                if curve_d2 < radius2
                    && in_bounds(screen)
                    && snap_better(tier, curve_d2, sub, (best_rank, best_d2, best_sub))
                {
                    best_rank = tier;
                    best_sub = sub;
                    best_d2 = curve_d2;
                    best = Some(SnapResult {
                        world: center,
                        screen,
                        snap_type: SnapType::Center,
                        tangent_obj: None,
                        extension_base: None,
                        extension_base2: None,
                    });
                }
            }
        }

        // If an Extension snap or an extended intersection won, re-find the
        // endpoint(s) whose ray(s) it lies on so the overlay can draw the dashed
        // guide line(s) back to them. An Extension yields one base; an extended
        // intersection yields both crossing extensions. A genuine on-segment
        // intersection yields none, so its guides simply don't draw. (#238, #247, #259)
        if let Some(b) = best.as_mut() {
            if matches!(b.snap_type, SnapType::Extension | SnapType::Intersection) {
                let (b1, b2) = extension_bases_screen(b.world, wires, view_rot, eye, bounds);
                b.extension_base = b1;
                b.extension_base2 = b2;
            }
        }

        best
    }
}

// ── Object-snap priority ───────────────────────────────────────────────────

/// Selection priority for an object snap — lower wins. Discrete snaps that
/// land on a specific feature (Endpoint, Midpoint, Center, …) outrank the
/// "continuous" snaps (Perpendicular, Tangent, Nearest) that can sit anywhere
/// along the geometry, so enabling a continuous snap can't suppress the
/// discrete ones the user also turned on. Mirrors the usual CAD running-osnap
/// precedence. See #118.
/// Priority TIER for candidate selection. All discrete point snaps
/// (Endpoint … Insertion) share one tier so the cursor's nearest feature wins
/// among them — a circle's Center must not mask its Quadrants just by rank
/// (#420). Continuous snaps keep their individual lower tiers, preserving the
/// #118 guarantee that they never suppress a discrete snap in the aperture.
fn snap_tier(t: SnapType) -> u8 {
    match t {
        SnapType::Endpoint
        | SnapType::Intersection
        | SnapType::ApparentIntersection
        | SnapType::Midpoint
        | SnapType::Center
        | SnapType::Node
        | SnapType::Quadrant
        | SnapType::Insertion => 0,
        other => snap_priority(other),
    }
}

/// Candidate ordering: tier first, then cursor distance, and only when two
/// candidates are effectively equidistant (coincident features) the classic
/// sub-priority — so an Endpoint still beats an Intersection sitting on the
/// exact same point. Distances are screen-px²; 4.0 ≈ a 2 px coincidence band.
fn snap_better(tier: u8, d2: f32, sub: u8, best: (u8, f32, u8)) -> bool {
    let (bt, bd2, bsub) = best;
    if tier != bt {
        return tier < bt;
    }
    if (d2 - bd2).abs() > 4.0 {
        return d2 < bd2;
    }
    sub < bsub
}

fn snap_priority(t: SnapType) -> u8 {
    match t {
        SnapType::Endpoint => 0,
        SnapType::Intersection => 1,
        SnapType::ApparentIntersection => 2,
        SnapType::Midpoint => 3,
        SnapType::Center => 4,
        SnapType::Node => 5,
        SnapType::Quadrant => 6,
        SnapType::Insertion => 7,
        SnapType::ObjectPick => 8,
        SnapType::Perpendicular => 9,
        SnapType::Tangent => 10,
        SnapType::Parallel => 11,
        SnapType::Extension => 12,
        SnapType::Nearest => 13,
        SnapType::Grid => 14,
    }
}

// ── Geometric helpers ─────────────────────────────────────────────────────

/// Line directions of every wire segment that has an endpoint at `p` (an
/// acquired corner), deduped by near-parallelism and capped. OTRACK offers an
/// alignment ray along each so the cursor can track a segment's extension, not
/// just the ortho/polar axes (#219). Scanned once, at acquisition — not per
/// move. Empty when `p` is not a segment endpoint (midpoint / centre / node).
fn edge_dirs_at(p: Vec3, wires: &[WireModel]) -> Vec<Vec3> {
    // The acquired point is an f32 truncation of the true (f64) vertex, so its
    // error grows with coordinate magnitude (~1 ULP ≈ 1.2e-7·mag). Scale the
    // endpoint-match tolerance to a few multiples of that, with a tight floor
    // near the origin — a fixed fraction would span metres at UTM scale and
    // match unrelated vertices, while a magnitude-blind floor would miss the
    // corner once f32 rounding exceeds it.
    let tol = 1e-4_f32.max(4e-7 * p.x.abs().max(p.y.abs()));
    let tol2 = (tol * tol) as f64;
    let pd = p.as_dvec3();
    let mut dirs: Vec<Vec3> = Vec::new();
    'outer: for wire in wires {
        let n = wire.points.len();
        if n < 2 {
            continue;
        }
        for i in 0..n - 1 {
            let a = wp_f64(wire, i);
            let b = wp_f64(wire, i + 1);
            if !a.x.is_finite() || !b.x.is_finite() {
                continue; // NaN sentinel separates sub-paths
            }
            if (a - pd).length_squared() >= tol2 && (b - pd).length_squared() >= tol2 {
                continue; // neither endpoint is the acquired corner
            }
            let seg = b - a;
            let l = (seg.x * seg.x + seg.y * seg.y).sqrt();
            if l < 1e-9 {
                continue;
            }
            let d = Vec3::new((seg.x / l) as f32, (seg.y / l) as f32, 0.0);
            // Skip a direction already present (parallel within ~0.5°); the ray
            // is bidirectional, so opposite signs are the same alignment line.
            if dirs.iter().any(|e| (e.x * d.x + e.y * d.y).abs() > 0.99996) {
                continue;
            }
            dirs.push(d);
            if dirs.len() >= 6 {
                break 'outer;
            }
        }
    }
    dirs
}

/// Reconstruct the absolute f64 position of wire vertex `i` from its
/// double-single high/low pair. At UTM-scale coordinates the `points` (high)
/// f32 alone is ~0.5 m off; adding the low residual restores f64 precision so
/// computed snaps (nearest/perp/intersection/extension) land on the geometry.
#[inline]
fn wp_f64(wire: &WireModel, i: usize) -> glam::DVec3 {
    let h = wire.points[i];
    let l = wire.points_low.get(i).copied().unwrap_or([0.0; 3]);
    glam::DVec3::new(
        h[0] as f64 + l[0] as f64,
        h[1] as f64 + l[1] as f64,
        h[2] as f64 + l[2] as f64,
    )
}

/// Closest point on segment [p0, p1] to `query`.
fn nearest_on_segment(query: glam::DVec3, p0: glam::DVec3, p1: glam::DVec3) -> glam::DVec3 {
    let d = p1 - p0;
    let len2 = d.x * d.x + d.y * d.y;
    if len2 < 1e-12 {
        return p0;
    }
    let t = ((query.x - p0.x) * d.x + (query.y - p0.y) * d.y) / len2;
    let t = t.clamp(0.0, 1.0);
    glam::DVec3::new(p0.x + t * d.x, p0.y + t * d.y, p0.z + t * d.z)
}

/// Foot of perpendicular from `query` to the line through [p0, p1] (XY plane, unclamped).
/// Returns `None` if the segment is degenerate.
fn perp_foot(query: glam::DVec3, p0: glam::DVec3, p1: glam::DVec3) -> Option<glam::DVec3> {
    let d = p1 - p0;
    let len2 = d.x * d.x + d.y * d.y;
    if len2 < 1e-12 {
        return None;
    }
    let t = ((query.x - p0.x) * d.x + (query.y - p0.y) * d.y) / len2;
    // Reject if the foot is far outside the segment (more than 2× segment length).
    if t < -1.0 || t > 2.0 {
        return None;
    }
    Some(glam::DVec3::new(p0.x + t * d.x, p0.y + t * d.y, p0.z + t * d.z))
}

/// XY-plane segment-segment intersection.  Returns `None` if parallel or outside.
/// True 3D intersection of two segments: the point where their plan (XY)
/// projections cross **and** both segments are at the same height there. Returns
/// `None` when parallel in plan, out of range, or the segments only *appear* to
/// cross in plan because they sit at different Z — a real Intersection requires
/// the objects to actually meet, so that case is left to Apparent Intersection
/// (the view-space crossing). (#335)
fn seg_intersect_3d(a0: glam::DVec3, a1: glam::DVec3, b0: glam::DVec3, b1: glam::DVec3) -> Option<glam::DVec3> {
    let d1x = a1.x - a0.x;
    let d1y = a1.y - a0.y;
    let d2x = b1.x - b0.x;
    let d2y = b1.y - b0.y;
    let cross = d1x * d2y - d1y * d2x;
    if cross.abs() < 1e-9 {
        return None;
    } // parallel in plan
    let ex = b0.x - a0.x;
    let ey = b0.y - a0.y;
    let t = (ex * d2y - ey * d2x) / cross;
    let s = (ex * d1y - ey * d1x) / cross;
    if t < 0.0 || t > 1.0 || s < 0.0 || s > 1.0 {
        return None;
    }
    // The plans cross; the segments truly meet only if they are at the same
    // height at that crossing. Different Z ⇒ they merely overlap in plan, which
    // is an apparent (view-dependent) intersection, not a real one.
    let za = a0.z + t * (a1.z - a0.z);
    let zb = b0.z + s * (b1.z - b0.z);
    let tol = 1e-6_f64.max(1e-9 * za.abs().max(zb.abs()));
    if (za - zb).abs() > tol {
        return None;
    }
    Some(glam::DVec3::new(a0.x + t * d1x, a0.y + t * d1y, 0.5 * (za + zb)))
}

/// Intersection of two infinite lines in the XY plane, each given by an origin
/// and a direction. Returns `None` when the lines are parallel.
fn line_intersect_xy(o1: Vec3, d1: Vec3, o2: Vec3, d2: Vec3) -> Option<Vec3> {
    let cross = d1.x * d2.y - d1.y * d2.x;
    if cross.abs() < 1e-9 {
        return None;
    }
    let ex = o2.x - o1.x;
    let ey = o2.y - o1.y;
    let t = (ex * d2.y - ey * d2.x) / cross;
    Some(Vec3::new(o1.x + d1.x * t, o1.y + d1.y * t, o1.z))
}

/// Screen-space 2D segment intersection.  Returns `(t, s)` parameters if found.
fn seg_intersect_2d(a0: Point, a1: Point, b0: Point, b1: Point) -> Option<(f32, f32)> {
    let d1x = a1.x - a0.x;
    let d1y = a1.y - a0.y;
    let d2x = b1.x - b0.x;
    let d2y = b1.y - b0.y;
    let cross = d1x * d2y - d1y * d2x;
    if cross.abs() < 1e-6 {
        return None;
    }
    let ex = b0.x - a0.x;
    let ey = b0.y - a0.y;
    let t = (ex * d2y - ey * d2x) / cross;
    let s = (ex * d1y - ey * d1x) / cross;
    if t < 0.0 || t > 1.0 || s < 0.0 || s > 1.0 {
        return None;
    }
    Some((t, s))
}

/// The two points on a circle (`center`, `radius`, in its XY plane) where a
/// line drawn from `p` touches tangentially, or `None` when `p` is inside or on
/// the circle (no external tangent). Each tangent point's radius is
/// perpendicular to the line from `p`, so it sits at the half-angle
/// `acos(radius / |center→p|)` on either side of the center→p direction. This
/// is the real Tangent osnap; snapping to the circle point merely facing the
/// cursor made Tangent behave like Nearest (#274).
fn circle_tangent_points(p: Vec3, center: Vec3, radius: f32) -> Option<(Vec3, Vec3)> {
    let vx = p.x - center.x;
    let vy = p.y - center.y;
    let d = (vx * vx + vy * vy).sqrt();
    if d <= radius + 1e-6 {
        return None;
    }
    let base = vy.atan2(vx);
    let off = (radius / d).acos();
    let at = |a: f32| Vec3::new(center.x + radius * a.cos(), center.y + radius * a.sin(), center.z);
    Some((at(base + off), at(base - off)))
}

/// Snap to the extension of a ray beyond `origin` in `dir` direction.
/// Returns `None` if the cursor is not near the extension line.
fn extension_snap(
    cursor_world: glam::DVec3,
    origin: glam::DVec3,
    dir: glam::DVec3,
    view_rot: Mat4,
        eye: glam::DVec3,
    bounds: Rectangle,
    radius_px: f32,
) -> Option<glam::DVec3> {
    let len2 = dir.x * dir.x + dir.y * dir.y;
    if len2 < 1e-12 {
        return None;
    }
    let t = ((cursor_world.x - origin.x) * dir.x + (cursor_world.y - origin.y) * dir.y) / len2;
    if t < 0.05 {
        return None;
    } // only beyond the endpoint
    let world_pt = glam::DVec3::new(origin.x + t * dir.x, origin.y + t * dir.y, origin.z);
    let screen_pt = world_to_screen(world_pt, view_rot, eye, bounds);
    let cursor_screen = world_to_screen(cursor_world, view_rot, eye, bounds);
    if dist2(screen_pt, cursor_screen) > radius_px * radius_px {
        return None;
    }
    Some(world_pt)
}

/// Find the endpoint(s) whose outward extension the snapped point lies on, and
/// return their screen positions so the overlay can draw a dashed guide from
/// each back to the snap point. A lone Extension snap yields one base; an
/// extended intersection (two extension lines crossing) yields both — so both
/// contributing extensions stay drawn when the crossing is caught. A genuine
/// on-segment intersection yields none: its crossing is between the endpoints,
/// never past them (`t < 0.05`). (#238, #247, #259)
fn extension_bases_screen(
    snapped: glam::DVec3,
    wires: &[WireModel],
    view_rot: Mat4,
    eye: glam::DVec3,
    bounds: Rectangle,
) -> (Option<Point>, Option<Point>) {
    let snapped_screen = world_to_screen(snapped, view_rot, eye, bounds);
    // Collect qualifying endpoints, off-ray distance measured in screen space so
    // the tolerance stays scale-independent at UTM coordinates (a world² test
    // would reject the crossing base once coordinates reach ~1e7).
    let mut found: Vec<(f32, glam::DVec3, Point)> = Vec::new();
    for wire in wires {
        let n = wire.points.len();
        if n < 2 {
            continue;
        }
        // A round curve (circle / arc / ellipse) has no meaningful straight
        // extension: its tessellation chords point every which way, so one
        // almost always has an outward chord-extension that grazes the snapped
        // point, drawing a spurious dashed guide radiating from the curve when
        // the user only caught a plain line-line intersection (#276). An arc is
        // open, so it carries no Quadrant hint — key off the Center hint that
        // every round curve carries (the same signal `nearest_segment` uses),
        // which straight lines never have.
        if wire
            .snap_pts
            .iter()
            .any(|(_, h)| matches!(h, SnapHint::Quadrant | SnapHint::Center))
        {
            continue;
        }
        // Match the extension snap: every segment can be extended past either
        // endpoint, so scan them all to find the base(s) the snapped point sits on.
        for i in 0..n - 1 {
            let a = wp_f64(wire, i);
            let b = wp_f64(wire, i + 1);
            if !a.x.is_finite() || !b.x.is_finite() {
                continue;
            }
            for (origin, other) in [(a, b), (b, a)] {
                let dir = origin - other;
                let len2 = dir.x * dir.x + dir.y * dir.y;
                if len2 < 1e-12 {
                    continue;
                }
                let t = ((snapped.x - origin.x) * dir.x + (snapped.y - origin.y) * dir.y) / len2;
                if t < 0.05 {
                    continue; // must be beyond the endpoint, matching extension_snap
                }
                let on = glam::DVec3::new(origin.x + t * dir.x, origin.y + t * dir.y, origin.z);
                let off = dist2(world_to_screen(on, view_rot, eye, bounds), snapped_screen);
                if off <= 4.0 {
                    let base = world_to_screen(origin, view_rot, eye, bounds);
                    found.push((off, origin, base));
                }
            }
        }
    }
    // Nearest-fit first, then keep up to two with distinct origins (collinear
    // segments sharing an endpoint must not draw the same guide twice).
    found.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut bases: [Option<Point>; 2] = [None, None];
    let mut origins: Vec<glam::DVec3> = Vec::new();
    for (_, origin, base) in found {
        if origins.iter().any(|o| (*o - origin).length_squared() < 1e-12) {
            continue;
        }
        origins.push(origin);
        if bases[0].is_none() {
            bases[0] = Some(base);
        } else {
            bases[1] = Some(base);
            break;
        }
    }
    (bases[0], bases[1])
}

// ── Projection helpers ────────────────────────────────────────────────────

/// Project a world point to screen relative-to-eye: subtract the f64 eye first
/// so the result is precise at UTM-scale absolute coordinates (a full
/// view-projection with a ~1e7 translation cancels catastrophically in f32).
/// `view_rot` is the rotation-only view-projection (Camera::view_proj_rte).
fn world_to_screen(world: glam::DVec3, view_rot: Mat4, eye: glam::DVec3, bounds: Rectangle) -> Point {
    let rel = (world - eye).as_vec3();
    let ndc = view_rot.project_point3(rel);
    Point::new(
        (ndc.x + 1.0) * 0.5 * bounds.width,
        (1.0 - ndc.y) * 0.5 * bounds.height,
    )
}

#[inline]
fn dist2(a: Point, b: Point) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

/// The nearest line / polyline segment under the cursor as (unit direction,
/// world point on it), within `aperture_px` in screen space, or None.
/// Tessellated curves (circle / arc / ellipse) are skipped — they carry a
/// Center snap hint and "parallel to a curve" is meaningless. Used to acquire
/// the Parallel-snap reference. (#277)
fn nearest_segment(
    cursor_world: Vec3,
    wires: &[WireModel],
    view_rot: Mat4,
    eye: glam::DVec3,
    bounds: Rectangle,
    aperture_px: f32,
) -> Option<(Vec3, Vec3)> {
    let cs = world_to_screen(cursor_world.as_dvec3(), view_rot, eye, bounds);
    let mut best_d2 = aperture_px * aperture_px;
    let mut best: Option<(Vec3, Vec3)> = None;
    for wire in wires {
        if wire
            .snap_pts
            .iter()
            .any(|(_, h)| matches!(h, SnapHint::Center))
        {
            continue; // circle / arc / ellipse — no meaningful parallel
        }
        for i in 0..wire.points.len().saturating_sub(1) {
            let a = wp_f64(wire, i);
            let b = wp_f64(wire, i + 1);
            if !a.x.is_finite() || !b.x.is_finite() {
                continue;
            }
            let sa = world_to_screen(a, view_rot, eye, bounds);
            let sb = world_to_screen(b, view_rot, eye, bounds);
            let d2 = dist2_to_segment(cs, sa, sb);
            if d2 < best_d2 {
                let dx = b.x - a.x;
                let dy = b.y - a.y;
                let l = (dx * dx + dy * dy).sqrt();
                if l > 1e-9 {
                    best_d2 = d2;
                    let dir = Vec3::new((dx / l) as f32, (dy / l) as f32, 0.0);
                    let np = nearest_on_segment(cursor_world.as_dvec3(), a, b).as_vec3();
                    best = Some((dir, np));
                }
            }
        }
    }
    best
}

/// Perpendicular screen-space distance (px) from world point `q` to the infinite
/// line through `line_pt` along `line_dir`. Used to tell whether a hovered line
/// is the acquired parallel reference (same line) regardless of zoom. (#277)
fn screen_perp_dist(
    q: Vec3,
    line_pt: Vec3,
    line_dir: Vec3,
    view_rot: Mat4,
    eye: glam::DVec3,
    bounds: Rectangle,
) -> f32 {
    let sq = world_to_screen(q.as_dvec3(), view_rot, eye, bounds);
    let s0 = world_to_screen(line_pt.as_dvec3(), view_rot, eye, bounds);
    let s1 = world_to_screen((line_pt + line_dir).as_dvec3(), view_rot, eye, bounds);
    let ex = s1.x - s0.x;
    let ey = s1.y - s0.y;
    let l = (ex * ex + ey * ey).sqrt();
    if l < 1e-6 {
        return dist2(sq, s0).sqrt();
    }
    (ex * (sq.y - s0.y) - ey * (sq.x - s0.x)).abs() / l
}

/// Squared distance from point p to line segment [a, b] in screen space.
fn dist2_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-6 {
        let ex = p.x - a.x;
        let ey = p.y - a.y;
        return ex * ex + ey * ey;
    }
    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    let nx = a.x + t * dx - p.x;
    let ny = a.y + t * dy - p.y;
    nx * nx + ny * ny
}

/// Parameter t ∈ [0,1] of the closest point on segment [a,b] to p.
fn t_on_segment(p: Point, a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-6 {
        return 0.0;
    }
    (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
}

#[cfg(test)]
mod ext_tests {
    use super::*;

    #[test]
    fn tracking_active_covers_otrack_and_extension() {
        let mut s = Snapper::default();
        // OTRACK off, Extension not enabled → no acquisition.
        s.snap_enabled = true;
        s.otrack_enabled = false;
        assert!(!s.tracking_active());
        // Extension on with the snap master on → acquire, independent of OTRACK.
        s.enabled.insert(SnapType::Extension);
        assert!(s.tracking_active());
        // Extension is gated by the snap master.
        s.snap_enabled = false;
        assert!(!s.tracking_active());
        // OTRACK acquires regardless of the object-snap master.
        s.enabled.remove(&SnapType::Extension);
        s.otrack_enabled = true;
        assert!(s.tracking_active());
    }

    #[test]
    fn extension_only_tracks_acquired_endpoints() {
        let mut s = Snapper::default();
        // Nothing acquired → no endpoint is a live extension source (#262).
        assert!(!s.is_tracked_endpoint(glam::DVec3::new(10.0, 0.0, 0.0)));
        // Acquire an endpoint → only that vertex tracks.
        s.tracking_points.push(Vec3::new(10.0, 0.0, 0.0));
        assert!(s.is_tracked_endpoint(glam::DVec3::new(10.0, 0.0, 0.0)));
        assert!(!s.is_tracked_endpoint(glam::DVec3::new(5.0, 0.0, 0.0)));
        // The match tolerance scales with coordinate magnitude, so an acquired
        // vertex at UTM scale still matches its f32-truncated tracking point.
        let big = 1_234_567.0_f64;
        s.tracking_points.push(Vec3::new(big as f32, 0.0, 0.0));
        assert!(s.is_tracked_endpoint(glam::DVec3::new(big, 0.0, 0.0)));
    }

    #[test]
    fn extension_acquisition_keeps_only_endpoints() {
        let mut s = Snapper::default();
        // A single line segment (0,0)-(10,0): its endpoints are vertices, its
        // midpoint and any extension foot are not.
        let wire = WireModel {
            text_verts: Vec::new(),
            points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
            ..Default::default()
        };
        let wires = [wire];

        // Extension-driven acquisition (endpoints_only): a midpoint — like an
        // extension foot the cursor paused on — is ignored, so it can't fill the
        // buffer and evict the real endpoint (#262).
        s.acquire_tracking_point(Vec3::new(5.0, 0.0, 0.0), &wires, true);
        assert!(s.tracking_points.is_empty());
        // The genuine endpoint is acquired.
        s.acquire_tracking_point(Vec3::new(10.0, 0.0, 0.0), &wires, true);
        assert_eq!(s.tracking_points.len(), 1);
        // OTRACK (endpoints_only = false) still acquires any snap point.
        s.acquire_tracking_point(Vec3::new(5.0, 0.0, 0.0), &wires, false);
        assert_eq!(s.tracking_points.len(), 2);
    }

    #[test]
    fn base_to_corner_direction_is_tracked() {
        let mut s = Snapper::default();
        s.otrack_enabled = true;
        s.osnap_radius_px = 10.0;
        // An acquired corner of an existing line, and the base point (first
        // point) of the line currently being drawn.
        let corner = Vec3::new(4.0, 2.0, 0.0);
        s.tracking_points.push(corner);
        s.tracking_dirs.push(Vec::new());
        let base = Vec3::new(0.0, 0.0, 0.0);

        // A plain orthographic camera: world * 0.1 → NDC, eye at the origin.
        let view_rot = Mat4::from_scale(Vec3::splat(0.1));
        let eye = glam::DVec3::ZERO;
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 1000.0,
        };

        // Cursor past the corner along base→corner (≈26.57°, not an ortho/polar
        // axis), nudged a hair off the line — only the new base→corner ray can
        // lock it.
        let dir = (corner - base).normalize();
        let perp = Vec3::new(-dir.y, dir.x, 0.0);
        let cursor = base + dir * 6.0 + perp * 0.05;

        let hit = s
            .otrack_snap(
                cursor,
                view_rot,
                eye,
                bounds,
                None,
                Some(base),
                false,
                Mat4::IDENTITY,
            )
            .expect("base→corner alignment should lock");
        // The aligned point lies on the base→corner line, and the reported base
        // is the command's base point (so typed-distance runs from there).
        let off = hit.aligned - base;
        let cross = off.x * dir.y - off.y * dir.x;
        assert!(
            cross.abs() < 1e-3,
            "aligned point off the base→corner line: {:?}",
            hit.aligned
        );
        assert!((hit.base - base).length() < 1e-3, "ray base is not the command base");

        // Without a base point the alignment does not exist (nothing to join).
        let none = s.otrack_snap(
            cursor, view_rot, eye, bounds, None, None, false, Mat4::IDENTITY,
        );
        assert!(none.is_none(), "no base point → no base→corner alignment");
    }

    #[test]
    fn tangent_points_are_perpendicular_to_the_radius() {
        let c = Vec3::new(0.0, 0.0, 0.0);
        let p = Vec3::new(10.0, 0.0, 0.0);
        let (t0, t1) = circle_tangent_points(p, c, 5.0).expect("external tangents exist");
        for t in [t0, t1] {
            // On the circle...
            assert!(((t - c).length() - 5.0).abs() < 1e-3, "{t:?} off circle");
            // ...and the radius C→T is perpendicular to the line P→T — the
            // defining property of a tangent (not just the nearest point).
            let ct = t - c;
            let pt = t - p;
            assert!(
                (ct.x * pt.x + ct.y * pt.y).abs() < 1e-3,
                "radius not perpendicular to the line at {t:?}"
            );
        }
        // Known geometry: acos(5/10) = 60°, so the tangents are at (2.5, ±4.330).
        assert!((t0.x - 2.5).abs() < 1e-2 && (t0.y.abs() - 4.330).abs() < 1e-2);
        // A point inside the circle has no external tangent.
        assert!(circle_tangent_points(Vec3::new(1.0, 0.0, 0.0), c, 5.0).is_none());
    }

    #[test]
    fn intersection_requires_matching_z_not_just_plan_crossing() {
        use glam::DVec3;
        // Two segments whose plans cross at the origin, both at z = 0: a real
        // 3D intersection.
        let hit = seg_intersect_3d(
            DVec3::new(-1.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.0, -1.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
        )
        .expect("coplanar crossing is a real intersection");
        assert!(hit.x.abs() < 1e-9 && hit.y.abs() < 1e-9 && hit.z.abs() < 1e-9);

        // Same plan crossing, but the second segment sits at z = 5: the lines
        // only *appear* to cross in top view — not a real intersection (#335).
        assert!(
            seg_intersect_3d(
                DVec3::new(-1.0, 0.0, 0.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(0.0, -1.0, 5.0),
                DVec3::new(0.0, 1.0, 5.0),
            )
            .is_none(),
            "different-Z plan crossing must not be a real Intersection"
        );

        // A real 3D crossing above the ground plane: both segments pass through
        // (0,0,5), so it snaps there at the true height.
        let hi = seg_intersect_3d(
            DVec3::new(-1.0, 0.0, 5.0),
            DVec3::new(1.0, 0.0, 5.0),
            DVec3::new(0.0, -1.0, 5.0),
            DVec3::new(0.0, 1.0, 5.0),
        )
        .expect("coplanar crossing at z=5");
        assert!((hi.z - 5.0).abs() < 1e-9, "z should be the true height, got {}", hi.z);
    }
}
