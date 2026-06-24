//! OpenCADStudio-style grip editing.

use acadrust::Handle;
use glam::{DVec3, Vec3};
use iced::{Point, Rectangle};

use crate::scene::model::object::{GripDef, GripShape};

/// Pixel radius for grip hit-detection.
pub const GRIP_THRESHOLD_PX: f32 = 8.0;
/// Half-size of the rendered grip square / diamond in pixels.
pub const GRIP_HALF_PX: f32 = 5.0;

// ── Active drag state ─────────────────────────────────────────────────────

/// Stored on `OpenCADStudio` while a grip is being dragged.
#[derive(Clone, Debug)]
pub struct GripEdit {
    /// Handle of the entity being edited.
    pub handle: Handle,
    /// Index into the entity's grip list.
    pub grip_id: usize,
    /// `true` → midpoint / translate grip; `false` → endpoint / absolute grip.
    pub is_translate: bool,
    /// World-space position of the grip when the drag started (ortho/polar base).
    pub origin_world: DVec3,
    /// Last world-space cursor position (needed for incremental delta on translate drags).
    pub last_world: DVec3,
}

// ── Screen-space helpers ───────────────────────────────────────────────────

/// Project a slice of `GripDef`s to screen space.
/// Returns `(grip_id, screen_pos, is_midpoint, shape)` for each grip.
pub fn grips_to_screen(
    grips: &[GripDef],
    camera: &crate::scene::view::camera::Camera,
    bounds: Rectangle,
) -> Vec<(usize, Point, bool, GripShape, Option<[f32; 2]>)> {
    grips
        .iter()
        .map(|g| {
            // Project from the f64 grip position via the relative-to-eye path so
            // the grip stays glued to the wire at UTM-scale coordinates (an
            // `as_vec3` cast first would quantize it ~0.5 m off at high zoom).
            let screen = match camera.project_f64(g.world, bounds) {
                Some(p) => Point::new(bounds.x + p.x, bounds.y + p.y),
                None => Point::new(f32::NAN, f32::NAN),
            };
            (g.id, screen, g.is_midpoint, g.shape, g.dir)
        })
        .collect()
}

/// Paper-space variant: project grips using the 2-D linear `to_px` transform.
/// Parameters match the `to_px` closure in `paper_canvas.rs`.
pub fn grips_to_screen_paper(
    grips: &[GripDef],
    tx: f32,
    ty: f32,
    half_w: f32,
    half_h: f32,
    bounds: Rectangle,
) -> Vec<(usize, Point, bool, GripShape, Option<[f32; 2]>)> {
    grips
        .iter()
        .map(|g| {
            let screen = Point::new(
                (g.world.x as f32 - tx + half_w) / (2.0 * half_w) * bounds.width,
                (ty + half_h - g.world.y as f32) / (2.0 * half_h) * bounds.height,
            );
            (g.id, screen, g.is_midpoint, g.shape, g.dir)
        })
        .collect()
}

/// Paper-space hit-test variant (mirrors `find_hit_grip` but uses 2-D projection).
pub fn find_hit_grip_paper(
    cursor: Point,
    grips: &[GripDef],
    tx: f32,
    ty: f32,
    half_w: f32,
    half_h: f32,
    bounds: Rectangle,
) -> Option<(usize, bool, DVec3)> {
    let mut best_dist = GRIP_THRESHOLD_PX;
    let mut best: Option<(usize, bool, DVec3)> = None;

    for g in grips {
        let screen = Point::new(
            (g.world.x as f32 - tx + half_w) / (2.0 * half_w) * bounds.width,
            (ty + half_h - g.world.y as f32) / (2.0 * half_h) * bounds.height,
        );
        let dx = screen.x - cursor.x;
        let dy = screen.y - cursor.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d < best_dist {
            best_dist = d;
            best = Some((g.id, g.is_midpoint, g.world));
        }
    }
    best
}

/// Find the closest grip within `GRIP_THRESHOLD_PX` pixels of `cursor`.
/// Returns `(grip_id, is_translate, world_pos)` if found, else `None`.
pub fn find_hit_grip(
    cursor: Point,
    grips: &[GripDef],
    camera: &crate::scene::view::camera::Camera,
    bounds: Rectangle,
) -> Option<(usize, bool, DVec3)> {
    let mut best_dist = GRIP_THRESHOLD_PX;
    let mut best: Option<(usize, bool, DVec3)> = None;

    for g in grips {
        let Some(screen) = camera.project_f64(g.world, bounds) else {
            continue;
        };
        let screen = Point::new(screen.x, screen.y);
        let dx = screen.x - cursor.x;
        let dy = screen.y - cursor.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d < best_dist {
            best_dist = d;
            best = Some((g.id, g.is_midpoint, g.world));
        }
    }
    best
}
