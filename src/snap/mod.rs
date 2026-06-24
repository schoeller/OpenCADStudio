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
    (SnapType::Grid, "⊞", "Grid"),
];

// ── Snap result ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct SnapResult {
    pub world: glam::DVec3,
    pub screen: Point,
    pub snap_type: SnapType,
    /// Set when `snap_type == Tangent`; provides entity geometry for TTR/TTT.
    pub tangent_obj: Option<TangentObject>,
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
    /// World-space grid spacing.
    pub grid_spacing: f32,
    /// Pixel-radius snap aperture, shared by OSNAP, tracking, polar and
    /// extension so the catch distance is the same everywhere.
    pub osnap_radius_px: f32,
    /// Object Snap Tracking on/off (F11).
    pub otrack_enabled: bool,
    /// Acquired OST points (world XZ, Y=0 plane).
    pub tracking_points: Vec<Vec3>,
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
            grid_spacing: 1.0,
            osnap_radius_px: CROSSHAIR_ARM * 0.25,
            otrack_enabled: false,
            tracking_points: Vec::new(),
            last_snap_world: None,
            dwell_since: None,
            dwell_acquired: false,
            from_point: None,
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

    pub fn toggle_global(&mut self) {
        self.snap_enabled = !self.snap_enabled;
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
        view_rot: glam::Mat4,
        eye: glam::DVec3,
        bounds: iced::Rectangle,
        now: Instant,
    ) {
        if !self.otrack_enabled {
            self.last_snap_world = None;
            self.dwell_since = None;
            self.dwell_acquired = false;
            return;
        }
        // The cursor must rest near a snap point for this long before it is
        // acquired, so that brushing past snap points while moving the mouse
        // does not create accidental tracking points.
        const DWELL_MS: u128 = 250;
        const DWELL_PX: f32 = 8.0;

        match snap_world {
            None => {
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
                        // otherwise acquire it (max 4 tracked points).
                        let existing = self.tracking_points.iter().position(|t| {
                            let d = (*t - p).length();
                            d < self.grid_spacing * 0.1
                        });
                        match existing {
                            Some(idx) => {
                                self.tracking_points.remove(idx);
                            }
                            None => {
                                if self.tracking_points.len() >= 4 {
                                    self.tracking_points.remove(0);
                                }
                                self.tracking_points.push(p);
                            }
                        }
                    }
                } else {
                    self.last_snap_world = Some(p);
                    self.dwell_since = Some(now);
                    self.dwell_acquired = false;
                }
            }
        }
    }

    /// Project the cursor onto a tracking ray emanating from one of the
    /// acquired tracking points, in the XY plane. Without `polar_step_deg` the
    /// rays are horizontal / vertical (0° / 90°); with it, every polar
    /// increment is a candidate so the user can track along POLAR angles.
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
        }
        // OTRACK rays come first; polar rays (appended below) only participate
        // in intersection locking, never in single-ray fallback.
        let otrack_ray_count = rays.len();
        const POLAR_GROUP: usize = usize::MAX;
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

        // ── Intersection lock — crossing of two vectors from distinct origins.
        let mut best_x: Option<(f32, OtrackHit)> = None;
        for i in 0..rays.len() {
            for j in (i + 1)..rays.len() {
                if rays[i].group == rays[j].group {
                    continue;
                }
                let Some(x) =
                    line_intersect_xy(rays[i].origin, rays[i].dir, rays[j].origin, rays[j].dir)
                else {
                    continue;
                };
                let sd = screen_dist(x);
                if sd < r && best_x.as_ref().map_or(true, |(bd, _)| sd < *bd) {
                    // Report an OTRACK ray as base/dir for typed-distance entry.
                    let ot = if rays[i].group != POLAR_GROUP {
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
        self.last_snap_world = None;
        self.dwell_since = None;
        self.dwell_acquired = false;
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
            grid_spacing: self.grid_spacing,
            osnap_radius_px: self.osnap_radius_px,
            otrack_enabled: false,
            tracking_points: Vec::new(),
            last_snap_world: None,
            dwell_since: None,
            dwell_acquired: false,
            from_point: None,
        };
        // Tangent-only: Grid is disabled here, so the grid basis is irrelevant.
        tmp.snap(
            cursor_world,
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
        cursor_world: Vec3,
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
        if !self.snap_enabled {
            return None;
        }
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
        let mut best_d2 = f32::MAX;

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
            let r = world_snap_r;
            cursor_world.x + r >= wire.aabb[0]
                && cursor_world.x - r <= wire.aabb[2]
                && cursor_world.y + r >= wire.aabb[1]
                && cursor_world.y - r <= wire.aabb[3]
        };

        let mut try_pt = |world: glam::DVec3, snap_type: SnapType| {
            let screen = world_to_screen(world, view_rot, eye, bounds);
            let d2 = dist2(screen, cursor_screen);
            // `!(d2 < radius2)` (not `d2 >= radius2`) so a NaN distance from
            // degenerate geometry is rejected: with priority selection a NaN
            // would otherwise pass the gate and be chosen on rank alone,
            // feeding a NaN snap point to the renderer. (#118)
            if !(d2 < radius2) {
                return;
            }
            let rank = snap_priority(snap_type);
            if rank < best_rank || (rank == best_rank && d2 < best_d2) {
                best_rank = rank;
                best_d2 = d2;
                best = Some(SnapResult {
                    world,
                    screen,
                    snap_type,
                    tangent_obj: None,
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
                    // Tessellated curves (Circle, Arc, Ellipse): only arc endpoints.
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
                        let a = Vec3::new(seg[0][0] as f32, seg[0][1] as f32, seg[0][2] as f32);
                        let b = Vec3::new(seg[1][0] as f32, seg[1][1] as f32, seg[1][2] as f32);
                        if a.distance_squared(b) > 1e-12 {
                            try_pt(((a + b) * 0.5).as_dvec3(), SnapType::Midpoint);
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
                for seg in wire.points.windows(2) {
                    let p =
                        nearest_on_segment(cursor_world, Vec3::from(seg[0]), Vec3::from(seg[1]));
                    try_pt(p.as_dvec3(), SnapType::Nearest);
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
            let q = self.from_point.unwrap_or(cursor_world);
            for wire in wires {
                if !wire_in_range(wire) {
                    continue;
                }
                for seg in wire.points.windows(2) {
                    if let Some(foot) = perp_foot(q, Vec3::from(seg[0]), Vec3::from(seg[1])) {
                        try_pt(foot.as_dvec3(), SnapType::Perpendicular);
                    }
                }
            }
        }

        // ── Intersection — segment-segment intersections ──────────
        if self.is_on(SnapType::Intersection) {
            for i in 0..wires.len() {
                if !wire_in_range(&wires[i]) {
                    continue;
                }
                for j in (i + 1)..wires.len() {
                    if !wire_in_range(&wires[j]) {
                        continue;
                    }
                    for seg_a in wires[i].points.windows(2) {
                        // S: pre-convert outside inner loop
                        let a0 = Vec3::from(seg_a[0]);
                        let a1 = Vec3::from(seg_a[1]);
                        let a_min_x = a0.x.min(a1.x);
                        let a_max_x = a0.x.max(a1.x);
                        let a_min_y = a0.y.min(a1.y);
                        let a_max_y = a0.y.max(a1.y);
                        for seg_b in wires[j].points.windows(2) {
                            let b0 = Vec3::from(seg_b[0]);
                            let b1 = Vec3::from(seg_b[1]);
                            // O: tight per-segment AABB overlap cull
                            if a_max_x < b0.x.min(b1.x)
                                || a_min_x > b0.x.max(b1.x)
                                || a_max_y < b0.y.min(b1.y)
                                || a_min_y > b0.y.max(b1.y)
                            {
                                continue;
                            }
                            if let Some(pt) = seg_intersect_xy(a0, a1, b0, b1) {
                                try_pt(pt.as_dvec3(), SnapType::Intersection);
                            }
                        }
                    }
                }
            }
        }

        // ── Extension — along the extension of a segment beyond endpoints ──
        if self.is_on(SnapType::Extension) {
            for wire in wires {
                let n = wire.points.len();
                if n < 2 {
                    continue;
                }
                // Extend beyond the first point.
                {
                    let p0 = Vec3::from(wire.points[0]);
                    let p1 = Vec3::from(wire.points[1]);
                    if let Some(ext) = extension_snap(
                        cursor_world,
                        p0,
                        p0 - p1,
                        view_rot,
                        eye,
                        bounds,
                        self.osnap_radius_px,
                    ) {
                        try_pt(ext.as_dvec3(), SnapType::Extension);
                    }
                }
                // Extend beyond the last point.
                {
                    let p_last = Vec3::from(wire.points[n - 1]);
                    let p_prev = Vec3::from(wire.points[n - 2]);
                    if let Some(ext) = extension_snap(
                        cursor_world,
                        p_last,
                        p_last - p_prev,
                        view_rot,
                        eye,
                        bounds,
                        self.osnap_radius_px,
                    ) {
                        try_pt(ext.as_dvec3(), SnapType::Extension);
                    }
                }
            }
        }

        // ── Apparent Intersection — screen-space intersections ─────────────
        // L: pre-project each in-range wire's points to screen once, not once per segment pair.
        if self.is_on(SnapType::ApparentIntersection) {
            let screen_pts: Vec<Option<Vec<Point>>> = wires
                .iter()
                .map(|w| {
                    if !wire_in_range(w) {
                        return None;
                    }
                    Some(
                        w.points
                            .iter()
                            .map(|&p| world_to_screen(glam::DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64), view_rot, eye, bounds))
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
                    for (ai, seg_a) in wires[i].points.windows(2).enumerate() {
                        let sa0 = si[ai];
                        let sa1 = si[ai + 1];
                        for (bi, _) in wires[j].points.windows(2).enumerate() {
                            let sb0 = sj[bi];
                            let sb1 = sj[bi + 1];
                            if let Some((ta, _)) = seg_intersect_2d(sa0, sa1, sb0, sb1) {
                                let wa0 = Vec3::from(seg_a[0]);
                                let wa1 = Vec3::from(seg_a[1]);
                                try_pt((wa0 + ta * (wa1 - wa0)).as_dvec3(), SnapType::ApparentIntersection);
                            }
                        }
                    }
                }
            }
        }

        // ── Grid ───────────────────────────────────────────────────────────
        if self.is_on(SnapType::Grid) {
            let s = self.grid_spacing;
            // Round in the UCS grid frame, then map back to world.
            let ax = grid_rot.transform_vector3(Vec3::X);
            let ay = grid_rot.transform_vector3(Vec3::Y);
            let az = grid_rot.transform_vector3(Vec3::Z);
            let rel = cursor_world - grid_origin;
            let ux = (rel.dot(ax) / s).round() * s;
            let uy = (rel.dot(ay) / s).round() * s;
            let uz = (rel.dot(az) / s).round() * s;
            try_pt((grid_origin + ax * ux + ay * uy + az * uz).as_dvec3(), SnapType::Grid);
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
                            let sc = world_to_screen(cv.as_dvec3(), view_rot, eye, bounds);
                            let rim = world_to_screen(
                                glam::DVec3::new((cv.x + radius) as f64, cv.y as f64, cv.z as f64),
                                view_rot,
                                eye,
                                bounds,
                            );
                            let sr = dist2(sc, rim).sqrt();
                            let dc = dist2(cursor_screen, sc).sqrt();
                            let edge_d = (dc - sr).abs();
                            // Snap point: point on circle edge facing cursor
                            let dx = cursor_screen.x - sc.x;
                            let dy = cursor_screen.y - sc.y;
                            let dl = (dx * dx + dy * dy).sqrt();
                            let (nx, ny) = if dl > 1e-6 {
                                (dx / dl, -dy / dl)
                            } else {
                                (1.0, 0.0)
                            };
                            let w = Vec3::new(cv.x + radius * nx, cv.y, cv.y + radius * ny);
                            (w, edge_d * edge_d)
                        }
                    };
                    let rank = snap_priority(SnapType::Tangent);
                    if d2 < radius2 && (rank < best_rank || (rank == best_rank && d2 < best_d2)) {
                        best_rank = rank;
                        best_d2 = d2;
                        let screen_pt = world_to_screen(world_pt.as_dvec3(), view_rot, eye, bounds);
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
                        });
                    }
                }
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

/// Closest point on segment [p0, p1] to `query`.
fn nearest_on_segment(query: Vec3, p0: Vec3, p1: Vec3) -> Vec3 {
    let d = p1 - p0;
    let len2 = d.x * d.x + d.y * d.y;
    if len2 < 1e-12 {
        return p0;
    }
    let t = ((query.x - p0.x) * d.x + (query.y - p0.y) * d.y) / len2;
    let t = t.clamp(0.0, 1.0);
    Vec3::new(p0.x + t * d.x, p0.y + t * d.y, p0.z + t * d.z)
}

/// Foot of perpendicular from `query` to the line through [p0, p1] (XY plane, unclamped).
/// Returns `None` if the segment is degenerate.
fn perp_foot(query: Vec3, p0: Vec3, p1: Vec3) -> Option<Vec3> {
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
    Some(Vec3::new(p0.x + t * d.x, p0.y + t * d.y, p0.z + t * d.z))
}

/// XY-plane segment-segment intersection.  Returns `None` if parallel or outside.
fn seg_intersect_xy(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> Option<Vec3> {
    let d1x = a1.x - a0.x;
    let d1y = a1.y - a0.y;
    let d2x = b1.x - b0.x;
    let d2y = b1.y - b0.y;
    let cross = d1x * d2y - d1y * d2x;
    if cross.abs() < 1e-9 {
        return None;
    } // parallel
    let ex = b0.x - a0.x;
    let ey = b0.y - a0.y;
    let t = (ex * d2y - ey * d2x) / cross;
    let s = (ex * d1y - ey * d1x) / cross;
    if t < 0.0 || t > 1.0 || s < 0.0 || s > 1.0 {
        return None;
    }
    Some(Vec3::new(a0.x + t * d1x, a0.y + t * d1y, 0.0))
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

/// Snap to the extension of a ray beyond `origin` in `dir` direction.
/// Returns `None` if the cursor is not near the extension line.
fn extension_snap(
    cursor_world: Vec3,
    origin: Vec3,
    dir: Vec3,
    view_rot: Mat4,
        eye: glam::DVec3,
    bounds: Rectangle,
    radius_px: f32,
) -> Option<Vec3> {
    let len2 = dir.x * dir.x + dir.y * dir.y;
    if len2 < 1e-12 {
        return None;
    }
    let t = ((cursor_world.x - origin.x) * dir.x + (cursor_world.y - origin.y) * dir.y) / len2;
    if t < 0.05 {
        return None;
    } // only beyond the endpoint
    let world_pt = Vec3::new(origin.x + t * dir.x, origin.y + t * dir.y, origin.z);
    let screen_pt = world_to_screen(world_pt.as_dvec3(), view_rot, eye, bounds);
    let cursor_screen = world_to_screen(cursor_world.as_dvec3(), view_rot, eye, bounds);
    if dist2(screen_pt, cursor_screen) > radius_px * radius_px {
        return None;
    }
    Some(world_pt)
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
