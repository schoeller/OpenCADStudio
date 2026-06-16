// Polyline tool — ribbon definition + interactive command.
//
// Command:  PLINE (PL)
//   Each click adds a vertex.
//   Type A = switch to Arc segment mode.
//   Type L = switch back to Line segment mode.
//   Enter / C = close and commit.  Escape = commit as-is (if ≥2 vertices).
//
// Arc mode: arcs are tangent-continuous with the preceding segment.
// Bulge is stored per vertex (segment i→i+1); positive = CCW, negative = CW.

use acadrust::entities::LwVertex;
use acadrust::types::Vector2;
use acadrust::{EntityType, LwPolyline};
use glam::{Vec2, Vec3};

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

// ── Ribbon definition ──────────────────────────────────────────────────────

pub fn tool() -> ToolDef {
    ToolDef {
        id: "PLINE",
        label: "Polyline",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/polyline.svg")),
        event: ModuleEvent::Command("PLINE".to_string()),
    }
}

// ── Segment mode ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum SegMode {
    Line,
    Arc,
}

// ── Command implementation ─────────────────────────────────────────────────

pub struct PlineCommand {
    vertices: Vec<Vec3>,
    /// Bulge for segment i → i+1 (one entry per vertex; last entry unused on commit).
    bulges: Vec<f64>,
    mode: SegMode,
    /// Unit direction of the last committed segment (for arc tangent continuity).
    last_tangent: Option<Vec2>,
}

impl PlineCommand {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            bulges: Vec::new(),
            mode: SegMode::Line,
            last_tangent: None,
        }
    }

    fn build_entity(&self, closed: bool) -> Option<EntityType> {
        if self.vertices.len() < 2 {
            return None;
        }
        let lw_verts: Vec<LwVertex> = self
            .vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let mut lv = LwVertex::new(Vector2::new(v.x as f64, v.y as f64));
                lv.bulge = self.bulges.get(i).copied().unwrap_or(0.0);
                lv
            })
            .collect();
        let pline = LwPolyline {
            vertices: lw_verts,
            is_closed: closed,
            ..Default::default()
        };
        Some(EntityType::LwPolyline(pline))
    }
}

// ── Arc geometry helpers ───────────────────────────────────────────────────

/// Compute the bulge for the arc from `a` to `b` that is tangent to `tangent` at `a`.
/// Returns 0.0 if the points are coincident or the tangent is parallel to the chord.
fn compute_bulge(a: Vec2, tangent: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let len_sq = d.length_squared();
    if len_sq < 1e-10 {
        return 0.0;
    }
    // Perpendicular to tangent (CCW) — this is the direction to the arc center.
    let perp = Vec2::new(-tangent.y, tangent.x);
    let dot = d.dot(perp);
    if dot.abs() < 1e-10 {
        // Tangent is perpendicular to chord → straight line (bulge = 0).
        return 0.0;
    }
    // t = distance from a to center along perp.
    let t = len_sq / (2.0 * dot);
    let center = a + perp * t;

    // Arc angle from start to end (signed).
    let start_angle = (a - center).y.atan2((a - center).x);
    let end_angle = (b - center).y.atan2((b - center).x);
    let mut arc_angle = end_angle - start_angle;

    if t > 0.0 {
        // CCW arc: ensure arc_angle is in (0, 2π].
        if arc_angle <= 0.0 {
            arc_angle += std::f32::consts::TAU;
        }
    } else {
        // CW arc: ensure arc_angle is in [-2π, 0).
        if arc_angle >= 0.0 {
            arc_angle -= std::f32::consts::TAU;
        }
    }
    (arc_angle as f64 / 4.0).tan()
}

/// Update `tangent` after an arc segment described by `bulge` from `a` to `b`.
fn update_tangent_after_arc(tangent: &mut Option<Vec2>, bulge: f64) {
    let Some(t) = *tangent else {
        return;
    };
    // The arc sweeps theta = 4*atan(bulge) radians, so the exit tangent is
    // the entry tangent rotated by that angle.
    let theta = 4.0 * (bulge as f32).atan();
    let (sin_t, cos_t) = theta.sin_cos();
    *tangent =
        Some(Vec2::new(t.x * cos_t - t.y * sin_t, t.x * sin_t + t.y * cos_t).normalize_or_zero());
}

/// Sample a circular arc defined by bulge into `n` line-segment points.
/// Returns the sampled [x, y, z] points (uses `z` from `a`).
fn arc_sample_points(a: Vec3, bulge: f64, b: Vec3, n: usize) -> Vec<[f32; 3]> {
    let ax = a.x as f64;
    let ay = a.y as f64;
    let bx = b.x as f64;
    let by = b.y as f64;

    let dx = bx - ax;
    let dy = by - ay;
    let chord_len = (dx * dx + dy * dy).sqrt();
    if chord_len < 1e-10 || bulge.abs() < 1e-10 {
        return vec![[a.x, a.y, a.z], [b.x, b.y, b.z]];
    }

    // Center of the arc.
    // Formula: center = midpoint + offset * perp_unit
    // where offset = chord_len * (1 - bulge²) / (4 * bulge).
    let b2 = bulge * bulge;
    let offset = chord_len * (1.0 - b2) / (4.0 * bulge);
    let perp_x = -dy / chord_len;
    let perp_y = dx / chord_len;
    let mx = (ax + bx) / 2.0;
    let my = (ay + by) / 2.0;
    let cx = mx + offset * perp_x;
    let cy = my + offset * perp_y;

    let r = ((ax - cx) * (ax - cx) + (ay - cy) * (ay - cy)).sqrt();
    let start_angle = (ay - cy).atan2(ax - cx);
    // Total arc angle (signed).
    let theta = 4.0 * bulge.atan();

    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let angle = start_angle + t * theta;
        pts.push([
            (cx + r * angle.cos()) as f32,
            (cy + r * angle.sin()) as f32,
            a.z,
        ]);
    }
    pts
}

// ── CadCommand impl ────────────────────────────────────────────────────────

impl CadCommand for PlineCommand {
    fn name(&self) -> &'static str {
        "PLINE"
    }

    fn prompt(&self) -> String {
        let mode_tag = match self.mode {
            SegMode::Line => "Line",
            SegMode::Arc => "Arc",
        };
        if self.vertices.is_empty() {
            "PLINE  Specify start point:".into()
        } else {
            format!(
                "PLINE [{mode_tag}]  Next pt  [{}pts | A=arc L=line C=close Enter=done]:",
                self.vertices.len()
            )
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        if !self.vertices.is_empty() {
            let last = *self.vertices.last().unwrap();
            let last_idx = self.vertices.len() - 1;

            let bulge = match self.mode {
                SegMode::Line => {
                    let d = Vec2::new(pt.x - last.x, pt.y - last.y);
                    if d.length_squared() > 1e-10 {
                        self.last_tangent = Some(d.normalize());
                    }
                    0.0
                }
                SegMode::Arc => {
                    let a = Vec2::new(last.x, last.y);
                    let b = Vec2::new(pt.x, pt.y);
                    let tangent = self.last_tangent.unwrap_or_else(|| {
                        // No previous tangent: default to pointing right (arbitrary).
                        Vec2::new(1.0, 0.0)
                    });
                    let bulge = compute_bulge(a, tangent, b);
                    update_tangent_after_arc(&mut self.last_tangent, bulge);
                    bulge
                }
            };
            self.bulges[last_idx] = bulge;
        }

        self.vertices.push(pt);
        self.bulges.push(0.0);
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        match self.build_entity(false) {
            Some(e) => CmdResult::CommitAndExit(e),
            None => CmdResult::Cancel,
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        match self.build_entity(false) {
            Some(e) => CmdResult::CommitAndExit(e),
            None => CmdResult::Cancel,
        }
    }

    fn wants_text_input(&self) -> bool {
        // Accept A / L / C once we have at least the first point.
        !self.vertices.is_empty()
    }

    fn point_step_accepts_keywords(&self) -> bool {
        // Each segment is a point pick that also accepts A / L / C / U, so the
        // polar dynamic-input distance/angle stays visible.
        !self.vertices.is_empty()
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        match text.trim().to_uppercase().as_str() {
            "A" | "ARC" => {
                self.mode = SegMode::Arc;
                Some(CmdResult::NeedPoint)
            }
            "L" | "LINE" => {
                self.mode = SegMode::Line;
                Some(CmdResult::NeedPoint)
            }
            "C" | "CLOSE" => match self.build_entity(true) {
                Some(e) => Some(CmdResult::CommitAndExit(e)),
                None => Some(CmdResult::Cancel),
            },
            _ => None,
        }
    }

    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        if self.vertices.is_empty() {
            return None;
        }
        let last = *self.vertices.last().unwrap();

        // Build committed segments first.
        let mut pts: Vec<[f32; 3]> = Vec::new();

        // Re-emit all committed vertices + arc samples between them.
        for i in 0..self.vertices.len() {
            let v = self.vertices[i];
            if i == 0 {
                pts.push([v.x, v.y, v.z]);
            } else {
                let prev = self.vertices[i - 1];
                let b = self.bulges.get(i - 1).copied().unwrap_or(0.0);
                if b.abs() > 1e-6 {
                    // Arc segment: sample it.
                    let arc_pts = arc_sample_points(prev, b, v, 16);
                    pts.extend_from_slice(&arc_pts[1..]); // skip first (already added)
                } else {
                    pts.push([v.x, v.y, v.z]);
                }
            }
        }

        // Rubber-band to cursor.
        match self.mode {
            SegMode::Line => {
                pts.push([pt.x, pt.y, pt.z]);
            }
            SegMode::Arc => {
                let a = Vec2::new(last.x, last.y);
                let b = Vec2::new(pt.x, pt.y);
                let tangent = self.last_tangent.unwrap_or(Vec2::new(1.0, 0.0));
                let bulge = compute_bulge(a, tangent, b);
                let arc_pts = arc_sample_points(last, bulge, pt, 16);
                pts.extend_from_slice(&arc_pts[1..]);
            }
        }

        Some(WireModel::solid(
            "rubber_band".into(),
            pts,
            WireModel::CYAN,
            false,
        ))
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["PL", "PLINE"] });  // PlineCommand
