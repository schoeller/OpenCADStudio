// Arcball orbit camera — quaternion-based rotation, no gimbal lock.
//
// The camera orbits around a `target` point using a unit quaternion (`rotation`)
// that maps the canonical "camera looks down -Z" pose to the current view.
//
// Pan:       translates `target` in the view-plane (no rotation change).
// Orbit:     updates `rotation` via arcball delta — converts screen drag delta
//            to a rotation axis/angle, then pre-multiplies the current quaternion.
// Zoom:      adjusts `distance` (exponential feel).
// Snap:      directly assigns yaw+pitch encoded as a quaternion (for ViewCube).
//
// Coordinate convention: Z-up world space (same as the rest of OpenCADStudio).

use glam::{vec3, Mat4, Quat, Vec3};
use iced::{Point, Rectangle};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Projection {
    Orthographic,
    Perspective,
}

#[derive(Clone)]
pub struct Camera {
    /// World-space pivot point the camera orbits around.
    pub target: Vec3,
    /// Arcball rotation: maps canonical pose to current orientation.
    pub rotation: Quat,
    /// Distance from eye to target.
    pub distance: f32,
    /// Vertical field of view in radians (perspective only).
    pub fov_y: f32,
    pub projection: Projection,

    // --- Legacy yaw/pitch exposed only for ViewCube hit-test compatibility ---
    // Kept in sync with `rotation` whenever orbit() or snap_angles() is called.
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for Camera {
    fn default() -> Self {
        // Default: look straight down at the XY drawing plane (top view, Z-up).
        // yaw = 0, pitch = PI/2  →  eye is directly above target.
        let yaw = 0.0_f32;
        let pitch = std::f32::consts::FRAC_PI_2;
        Self {
            target: Vec3::ZERO,
            rotation: yaw_pitch_to_quat(yaw, pitch, 0.0),
            distance: 60.36,
            fov_y: 45.0_f32.to_radians(),
            projection: Projection::Orthographic,
            yaw,
            pitch,
        }
    }
}

pub const OPENGL_TO_WGPU: Mat4 = glam::mat4(
    glam::vec4(1.0, 0.0, 0.0, 0.0),
    glam::vec4(0.0, 1.0, 0.0, 0.0),
    glam::vec4(0.0, 0.0, 0.5, 0.0),
    glam::vec4(0.0, 0.0, 0.5, 1.0),
);

impl Camera {
    // ── Eye position ───────────────────────────────────────────────────────

    pub fn eye(&self) -> Vec3 {
        // The canonical eye direction is +Z (looking at origin from above).
        // The rotation maps that canonical pose to the current orientation.
        let eye_dir = self.rotation * Vec3::Z;
        self.target + eye_dir * self.distance
    }

    /// Half-height of the orthographic frustum in world units.
    pub fn ortho_size(&self) -> f32 {
        self.distance * (self.fov_y * 0.5).tan()
    }

    // ── Projection matrices ────────────────────────────────────────────────

    pub fn view_proj(&self, bounds: Rectangle) -> Mat4 {
        let near = self.distance * 0.001;
        let far = self.distance * 1000.0;
        let aspect = bounds.width / bounds.height;

        // Up vector: use the rotation to find which world direction is "up"
        // in the current camera frame.
        let up_dir = self.rotation * Vec3::Y;

        let view = Mat4::look_at_rh(self.eye(), self.target, up_dir);
        let proj = match self.projection {
            Projection::Perspective => Mat4::perspective_rh(self.fov_y, aspect, near, far),
            Projection::Orthographic => {
                let h = self.ortho_size();
                let w = h * aspect;
                Mat4::orthographic_rh(-w, w, -h, h, near, far)
            }
        };
        OPENGL_TO_WGPU * proj * view
    }

    /// Project a screen point onto an arbitrary world-space plane.
    ///
    /// The plane is defined by `plane_normal` (unit vector) and a `plane_point`
    /// that lies on it.  Returns the intersection of the view ray with the plane;
    /// falls back to `plane_point` when the ray is nearly parallel to the plane.
    pub fn pick_on_plane(
        &self,
        screen: Point,
        bounds: Rectangle,
        plane_normal: Vec3,
        plane_point: Vec3,
    ) -> Vec3 {
        let ndc_x = (screen.x / bounds.width) * 2.0 - 1.0;
        let ndc_y = 1.0 - (screen.y / bounds.height) * 2.0;
        let inv = self.view_proj(bounds).inverse();

        let (ray_origin, ray_dir) = match self.projection {
            Projection::Perspective => {
                let near_pt = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
                let far_pt = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
                let dir = (far_pt - near_pt).normalize();
                (near_pt, dir)
            }
            Projection::Orthographic => {
                let origin = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
                let forward = (self.target - self.eye()).normalize();
                (origin, forward)
            }
        };

        let denom = ray_dir.dot(plane_normal);
        if denom.abs() < 1e-6 {
            return plane_point;
        }
        let t = (plane_point - ray_origin).dot(plane_normal) / denom;
        if t < 0.0 {
            return plane_point;
        }
        ray_origin + ray_dir * t
    }

    pub fn pick_on_target_plane(&self, screen: Point, bounds: Rectangle) -> Vec3 {
        let ndc_x = (screen.x / bounds.width) * 2.0 - 1.0;
        let ndc_y = 1.0 - (screen.y / bounds.height) * 2.0;
        let inv = self.view_proj(bounds).inverse();

        match self.projection {
            Projection::Perspective => {
                let near_pt = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
                let far_pt = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
                let dir = (far_pt - near_pt).normalize();
                let forward = (self.target - self.eye()).normalize();
                let denom = dir.dot(forward);
                if denom.abs() < 1e-6 {
                    return self.target;
                }
                let t = (self.target - near_pt).dot(forward) / denom;
                if t < 0.0 {
                    return self.target;
                }
                near_pt + dir * t
            }
            Projection::Orthographic => {
                let ray_origin = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
                let forward = (self.target - self.eye()).normalize();
                let t = (self.target - ray_origin).dot(forward) / forward.dot(forward);
                ray_origin + forward * t
            }
        }
    }

    pub fn position_vec4(&self) -> glam::Vec4 {
        glam::Vec4::from((self.eye(), 0.0))
    }

    // ── ViewCube rotation matrix ───────────────────────────────────────────

    /// Returns the rotation matrix for the ViewCube.
    ///
    /// The camera quaternion maps canonical pose (+Z eye) → current view.
    /// The ViewCube needs the inverse so the cube stays world-aligned.
    /// Inverse of a unit quaternion = its conjugate.
    pub fn view_rotation_mat(&self) -> Mat4 {
        Mat4::from_quat(self.rotation.conjugate())
    }

    /// The camera's roll — rotation about the view axis, in radians — the
    /// inverse of the `roll` argument to [`yaw_pitch_to_quat`]. Recovered by
    /// removing the yaw/pitch frame from the live rotation, so a saved view can
    /// store its twist and round-trip it. Exact for a plan view (the twisted-
    /// UCS case); approximate after a free 3D orbit.
    pub fn roll(&self) -> f32 {
        let q_yp = yaw_pitch_to_quat(self.yaw, self.pitch, 0.0);
        let q_roll = q_yp.conjugate() * self.rotation;
        // q_roll is (nominally) a rotation about Z; extract its angle.
        2.0 * q_roll.z.atan2(q_roll.w)
    }

    // ── Navigation ────────────────────────────────────────────────────────

    /// Arcball orbit: drag delta (dx, dy) in screen pixels.
    pub fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        if delta_x.abs() < 1e-6 && delta_y.abs() < 1e-6 {
            return;
        }

        let speed = 0.005_f32;
        let angle = (delta_x * delta_x + delta_y * delta_y).sqrt() * speed;

        // Screen drag → rotation axis: right drag = rotate around cam_up (Y),
        // down drag = rotate around cam_right (X). Negate so drag direction
        // matches intuitive "grab and spin" arcball behaviour.
        let screen_axis = vec3(-delta_y, -delta_x, 0.0).normalize_or_zero();

        let cam_right = self.rotation * Vec3::X;
        let cam_up = self.rotation * Vec3::Y;
        let world_axis = (cam_right * screen_axis.x + cam_up * screen_axis.y).normalize_or_zero();

        if world_axis.length_squared() < 1e-12 {
            return;
        }

        let delta_rot = Quat::from_axis_angle(world_axis, angle);
        self.rotation = (delta_rot * self.rotation).normalize();

        // Sync legacy yaw/pitch for hit-test functions.
        self.sync_yaw_pitch();
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance * (1.0 - delta * 0.1)).max(0.001);
    }

    pub fn zoom_about_point(&mut self, screen: Point, bounds: Rectangle, delta: f32) {
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            self.zoom(delta);
            return;
        }

        let before = self.pick_on_target_plane(screen, bounds);
        self.zoom(delta);
        let after = self.pick_on_target_plane(screen, bounds);
        self.target += before - after;
    }

    /// Pan so the world point under the cursor tracks it: screen pixels are
    /// converted to world units via the ortho world-per-pixel scale of a
    /// viewport `viewport_height` pixels tall. Used by tiled panes where the
    /// pane height differs from the full canvas.
    pub fn pan_screen(&mut self, delta_x: f32, delta_y: f32, viewport_height: f32) {
        let wpp = if viewport_height > 0.0 {
            (2.0 * self.ortho_size()) / viewport_height
        } else {
            0.0
        };
        let cam_right = self.rotation * Vec3::X;
        let cam_up = self.rotation * Vec3::Y;
        self.target -= cam_right * delta_x * wpp;
        self.target += cam_up * delta_y * wpp;
    }

    pub fn fit_to_bounds(&mut self, min: Vec3, max: Vec3) {
        self.target = (min + max) * 0.5;
        let size = (max - min).length();
        self.distance = size * 1.5;
    }

    // ── ViewCube snap ─────────────────────────────────────────────────────

    /// Snap to a canonical view direction (called by ViewCubeSnap).
    /// `eye_dir` is the unit vector from the target toward the camera.
    ///
    /// Up vector resolution:
    ///  1. Take the current up.
    ///  2. Pick the world axis (±X, ±Y, ±Z) whose dot product with the
    ///     current up is highest — skipping any axis (anti-)parallel to
    ///     the new gaze direction.
    ///  3. Project that axis onto the plane ⊥ `new_eye` and use that as
    ///     the new up.
    ///
    /// Result: small tilts collapse onto the nearest world axis (so the
    /// view always lands cleanly aligned), while genuine flips of the
    /// up-sense (e.g. orbited upside-down) are preserved.
    pub fn snap_to_direction(&mut self, eye_dir: Vec3) {
        let new_eye = eye_dir.normalize_or(Vec3::Z);
        let cur_up = self.rotation * Vec3::Y;
        let cardinals = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ];
        let mut best_score = f32::NEG_INFINITY;
        let mut best_up = Vec3::Z;
        for axis in cardinals {
            // Skip axes (nearly) collinear with the new gaze — they can't
            // serve as up because the projection onto the plane would
            // vanish.
            if axis.dot(new_eye).abs() > 0.999 {
                continue;
            }
            let score = axis.dot(cur_up);
            if score > best_score {
                best_score = score;
                best_up = axis;
            }
        }
        // Project the chosen axis onto the plane ⊥ new_eye and normalize.
        let projected = best_up - new_eye * best_up.dot(new_eye);
        let new_up = projected.normalize_or(if new_eye.z.abs() < 0.99 {
            (Vec3::Z - new_eye * Vec3::Z.dot(new_eye)).normalize()
        } else {
            (Vec3::Y - new_eye * Vec3::Y.dot(new_eye)).normalize()
        });
        let new_right = new_up.cross(new_eye).normalize();
        // Camera rotation columns: [cam_x | cam_y | cam_z] where
        // cam_z = eye_dir (canonical "+Z is toward eye"), cam_y = up.
        let mat = glam::Mat3::from_cols(new_right, new_up, new_eye);
        self.rotation = Quat::from_mat3(&mat).normalize();
        self.sync_yaw_pitch();
    }

    // ── Internal helpers ───────────────────────────────────────────────────

    /// Derive yaw and pitch from the current quaternion for the ViewCube
    /// hit-test functions (hit_test / hover_id). These use yaw/pitch to
    /// compute the same rotation matrix as the shader, so they must match.
    fn sync_yaw_pitch(&mut self) {
        // Eye direction in world space (canonical eye dir is +Z).
        let eye_dir = self.rotation * Vec3::Z;
        // pitch: angle above/below the XY plane.
        self.pitch = eye_dir.z.clamp(-0.999, 0.999).asin();
        // yaw: atan2(x, y) matches from_rotation_z(yaw) used in view_rotation_mat.
        self.yaw = eye_dir.x.atan2(eye_dir.y);
    }
}

// ── Free helpers ───────────────────────────────────────────────────────────

/// Build a rotation quaternion from yaw (rotation around Z) and pitch
/// (tilt toward Z). Matches the coordinate convention of the ViewCube
/// so snap angles continue to work unchanged.
///
/// Convention (Z-up, Y-forward):
///   yaw   = 0          → camera looks along +Y axis (front view)
///   pitch = PI/2       → camera looks down -Z (top view)
///   pitch = 0          → camera in the XY plane
/// Build a rotation quaternion from yaw, pitch and roll.
/// Positive yaw rotates the view direction clockwise when seen from above (Z-up).
/// Roll rotates the camera around its own view axis (post-multiplied so it
/// composes after the yaw/pitch gaze direction is set).
pub fn yaw_pitch_to_quat(yaw: f32, pitch: f32, roll: f32) -> Quat {
    // +yaw so ViewCube faces match camera direction (FRONT at yaw=0 = +Y world axis).
    let q_yaw = Quat::from_rotation_z(yaw);
    let q_pitch = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2 - pitch);
    let q_roll = Quat::from_rotation_z(roll);
    (q_yaw * q_pitch * q_roll).normalize()
}
