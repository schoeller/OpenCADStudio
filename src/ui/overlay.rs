//! Viewport overlay widgets.

use glam::{Mat4, Vec3};
use iced::mouse;
use iced::widget::canvas;
use iced::{Color, Element, Length, Point, Size, Theme};

use crate::app::Message;
use crate::scene::model::object::GripShape;
use crate::scene::SelectionState;

/// Half-size of the crosshair center square in screen pixels (square = SQ*2 × SQ*2).
pub const CROSSHAIR_SQ: f32 = 7.5;
/// Arm length of the crosshair from center — used as the snap aperture radius.
pub const CROSSHAIR_ARM: f32 = 60.0;
use crate::snap::SnapType;

// ── Grip marker data ──────────────────────────────────────────────────────

/// Describes one grip to be drawn in the viewport overlay.
#[derive(Clone, Debug)]
pub struct GripMarker {
    /// Screen-space position (viewport-relative pixels).
    pub pos: Point,
    /// Explicit marker shape.
    pub shape: GripShape,
    /// True → grip is currently being dragged (drawn filled red).
    pub is_hot: bool,
    /// World-XY direction vector — only consumed by the `Rectangle`
    /// shape to orient the box along its segment. `None` for grips
    /// that don't need rotation.
    pub dir: Option<[f32; 2]>,
}

// ── Grid display params ───────────────────────────────────────────────────

/// Which world-space plane the grid is drawn on — switches with camera angle.
#[derive(Clone, Copy, PartialEq)]
pub enum GridPlane {
    /// Horizontal XY plane (Z = 0).  Default top-down view (Z-up).
    Xy,
    /// Vertical XZ plane (Y = 0).  Front/back view.
    Xz,
    /// Vertical YZ plane (X = 0).  Side view.
    Yz,
}

/// Passed to the canvas when the GRID display is active.
#[derive(Clone)]
pub struct GridParams {
    pub view_proj: Mat4,
    pub bounds: iced::Rectangle,
    pub plane: GridPlane,
}

/// Compute the adaptive grid step size (world units) that the grid renderer
/// would use for a given view-projection matrix and viewport bounds.
///
/// Returns the smallest power-of-5 multiple of 1.0 that places grid lines at
/// least `MIN_GRID_PX` pixels apart.  This matches exactly what `draw_grid`
/// renders, so callers can sync snap spacing to the visible grid.
pub fn compute_grid_step(vp: Mat4, bounds: iced::Rectangle) -> f32 {
    use glam::Vec3;
    let w2s = |world: Vec3| {
        let ndc = vp.project_point3(world);
        glam::Vec2::new(
            (ndc.x + 1.0) * 0.5 * bounds.width,
            (1.0 - ndc.y) * 0.5 * bounds.height,
        )
    };
    let o = w2s(Vec3::ZERO);
    let a1 = w2s(Vec3::X);
    let a2 = w2s(Vec3::Y);
    let d1 = (a1 - o).length();
    let d2 = (a2 - o).length();
    let px_per_unit = d1.max(d2);
    if px_per_unit < 1e-6 {
        return 1.0;
    }
    let mut s = 1.0_f32;
    while s * px_per_unit < MIN_GRID_PX {
        s *= 5.0;
        if s > 1e9 {
            return 1.0;
        }
    }
    s
}

/// Parameters for the screen-space UCS icon drawn in the viewport corner.
pub struct UcsIconParams {
    /// View-projection matrix used to project world axis directions to screen.
    pub view_proj: Mat4,
    /// Viewport bounds (used for NDC → pixel conversion).
    pub bounds: iced::Rectangle,
}

// ── Selection overlay ───────────────────────────────────────────────────

/// An acquired OST tracking point with its screen position.
#[derive(Clone, Debug)]
pub struct OstTrackPoint {
    pub screen: Point,
}

pub fn selection_overlay<'a>(
    selection: SelectionState,
    snap: Option<(Point, SnapType)>,
    grips: Vec<GripMarker>,
    grid: Option<GridParams>,
    ucs_icon: Option<UcsIconParams>,
    ost_points: Vec<OstTrackPoint>,
    cursor_screen: Point,
    show_viewcube: bool,
    tile_edges: Vec<crate::scene::TileEdge>,
) -> Element<'a, Message> {
    canvas(SelectionCanvas {
        selection,
        snap,
        grips,
        grid,
        ucs_icon,
        ost_points,
        cursor_screen,
        show_viewcube,
        tile_edges,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

struct SelectionCanvas {
    selection: SelectionState,
    snap: Option<(Point, SnapType)>,
    grips: Vec<GripMarker>,
    grid: Option<GridParams>,
    ucs_icon: Option<UcsIconParams>,
    ost_points: Vec<OstTrackPoint>,
    cursor_screen: Point,
    show_viewcube: bool,
    tile_edges: Vec<crate::scene::TileEdge>,
}

impl SelectionCanvas {
    /// Returns the orientation of the Model-tile divider the cursor sits
    /// on (within a few pixels of perpendicular distance, inside the
    /// edge's span). Used by both `mouse_interaction` (to pick a resize
    /// cursor) and `draw` (to suppress the CAD crosshair).
    fn tile_edge_under(
        &self,
        cursor: mouse::Cursor,
        bounds: iced::Rectangle,
    ) -> Option<crate::scene::TileEdgeOrient> {
        const TOL_PX: f32 = 4.0;
        let pos = cursor.position_in(bounds)?;
        for e in &self.tile_edges {
            let (perp, edge_px, span_lo, span_hi, pos_along) = match e.orient {
                crate::scene::TileEdgeOrient::Vertical => (
                    pos.x,
                    e.coord * bounds.width,
                    e.span.0 * bounds.height,
                    e.span.1 * bounds.height,
                    pos.y,
                ),
                crate::scene::TileEdgeOrient::Horizontal => (
                    pos.y,
                    e.coord * bounds.height,
                    e.span.0 * bounds.width,
                    e.span.1 * bounds.width,
                    pos.x,
                ),
            };
            if (perp - edge_px).abs() <= TOL_PX
                && pos_along >= span_lo
                && pos_along <= span_hi
            {
                return Some(e.orient);
            }
        }
        None
    }
}

impl canvas::Program<Message> for SelectionCanvas {
    type State = ();

    fn mouse_interaction(
        &self,
        _state: &(),
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.show_viewcube {
            if let Some(pos) = cursor.position_in(bounds) {
                use crate::scene::{VIEWCUBE_DRAW_PX, VIEWCUBE_PAD};
                let vc_x = bounds.width - VIEWCUBE_DRAW_PX - VIEWCUBE_PAD;
                let vc_y = VIEWCUBE_PAD;
                if pos.x >= vc_x
                    && pos.x <= vc_x + VIEWCUBE_DRAW_PX
                    && pos.y >= vc_y
                    && pos.y <= vc_y + VIEWCUBE_DRAW_PX
                {
                    return mouse::Interaction::None;
                }
            }
        }
        // Hover over a Model-tile divider → resize cursor cue. The
        // system cursor is intentionally shown here so the OS arrow gives
        // its own resize affordance; the draw step suppresses the custom
        // CAD crosshair while we're over a divider.
        if let Some(orient) = self.tile_edge_under(cursor, bounds) {
            return match orient {
                crate::scene::TileEdgeOrient::Vertical => {
                    mouse::Interaction::ResizingHorizontally
                }
                crate::scene::TileEdgeOrient::Horizontal => {
                    mouse::Interaction::ResizingVertically
                }
            };
        }
        // Over the viewport (no divider, no viewcube): hide the system
        // cursor entirely. `Interaction::None` would let the stack fall
        // through to a sibling — `Hidden` is the explicit "no cursor"
        // signal that actually suppresses the OS arrow.
        if cursor.is_over(bounds) {
            mouse::Interaction::Hidden
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // ── Tile dividers (Model-space tiled layout) ──────────────────────
        // Drawn first so all other overlays sit on top — the line is just
        // a visual cue for the resize-drag handles.
        if !self.tile_edges.is_empty() {
            const DIVIDER: Color = Color {
                r: 0.40,
                g: 0.45,
                b: 0.55,
                a: 0.85,
            };
            for e in &self.tile_edges {
                let (a, b) = match e.orient {
                    crate::scene::TileEdgeOrient::Vertical => {
                        let x = e.coord * bounds.width;
                        (
                            Point::new(x, e.span.0 * bounds.height),
                            Point::new(x, e.span.1 * bounds.height),
                        )
                    }
                    crate::scene::TileEdgeOrient::Horizontal => {
                        let y = e.coord * bounds.height;
                        (
                            Point::new(e.span.0 * bounds.width, y),
                            Point::new(e.span.1 * bounds.width, y),
                        )
                    }
                };
                let path = canvas::Path::new(|p| {
                    p.move_to(a);
                    p.line_to(b);
                });
                frame.stroke(
                    &path,
                    canvas::Stroke {
                        width: 2.0,
                        style: canvas::Style::Solid(DIVIDER),
                        line_cap: canvas::LineCap::Butt,
                        ..Default::default()
                    },
                );
            }
        }

        // ── Grid display ──────────────────────────────────────────────────
        if let Some(ref g) = self.grid {
            // Clip to the active tile's rectangle so grid lines don't spill
            // into neighbouring panes in a tiled layout.
            let plane = g.plane;
            let view_proj = g.view_proj;
            let gb = g.bounds;
            frame.with_clip(gb, |f| draw_grid(f, view_proj, plane, gb));
        }

        if let (Some(a), Some(b)) = (self.selection.box_anchor, self.selection.box_current) {
            let (fill, stroke) = if self.selection.box_crossing {
                (
                    Color {
                        r: 0.20,
                        g: 0.72,
                        b: 0.44,
                        a: 0.12,
                    },
                    Color {
                        r: 0.20,
                        g: 0.72,
                        b: 0.44,
                        a: 0.9,
                    },
                )
            } else {
                (
                    Color {
                        r: 0.20,
                        g: 0.44,
                        b: 0.72,
                        a: 0.12,
                    },
                    Color {
                        r: 0.20,
                        g: 0.44,
                        b: 0.72,
                        a: 0.9,
                    },
                )
            };
            let x0 = a.x.min(b.x);
            let y0 = a.y.min(b.y);
            let w = (a.x - b.x).abs();
            let h = (a.y - b.y).abs();
            let rect = canvas::Path::rectangle(Point::new(x0, y0), Size::new(w, h));
            frame.fill(&rect, fill);
            frame.stroke(
                &rect,
                canvas::Stroke {
                    width: 1.0,
                    style: canvas::Style::Solid(stroke),
                    ..Default::default()
                },
            );
        }

        if self.selection.poly_active && self.selection.poly_points.len() > 1 {
            let (fill, stroke) = if self.selection.poly_crossing {
                (
                    Color {
                        r: 0.20,
                        g: 0.72,
                        b: 0.44,
                        a: 0.12,
                    },
                    Color {
                        r: 0.20,
                        g: 0.72,
                        b: 0.44,
                        a: 0.9,
                    },
                )
            } else {
                (
                    Color {
                        r: 0.20,
                        g: 0.44,
                        b: 0.72,
                        a: 0.12,
                    },
                    Color {
                        r: 0.20,
                        g: 0.44,
                        b: 0.72,
                        a: 0.9,
                    },
                )
            };
            if let Some(cur) = self.selection.last_move_pos {
                let start = self.selection.poly_points[0];
                let fill_path = canvas::Path::new(|p| {
                    p.move_to(start);
                    for pt in &self.selection.poly_points[1..] {
                        p.line_to(*pt);
                    }
                    p.line_to(cur);
                    p.line_to(start);
                });
                frame.fill(&fill_path, fill);
            }
            let path = canvas::Path::new(|p| {
                p.move_to(self.selection.poly_points[0]);
                for pt in &self.selection.poly_points[1..] {
                    p.line_to(*pt);
                }
            });
            frame.stroke(
                &path,
                canvas::Stroke {
                    width: 1.0,
                    style: canvas::Style::Solid(stroke),
                    ..Default::default()
                },
            );
            if let Some(cur) = self.selection.last_move_pos {
                let start = self.selection.poly_points[0];
                let last = *self.selection.poly_points.last().unwrap();
                let preview = canvas::Path::new(|p| {
                    p.move_to(last);
                    p.line_to(cur);
                    p.line_to(start);
                });
                frame.stroke(
                    &preview,
                    canvas::Stroke {
                        width: 1.0,
                        style: canvas::Style::Solid(stroke),
                        ..Default::default()
                    },
                );
            }
        }

        // ── Grip markers ──────────────────────────────────────────────────
        for grip in &self.grips {
            let sp = grip.pos;
            let h = crate::scene::pick::grip::GRIP_HALF_PX;
            let path = match grip.shape {
                GripShape::Square => canvas::Path::rectangle(
                    Point::new(sp.x - h, sp.y - h),
                    Size::new(h * 2.0, h * 2.0),
                ),
                GripShape::Rectangle => {
                    // Mid-segment stretch handle: small box, longer along
                    // the segment direction so the affordance reads as
                    // "stretch perpendicular to the segment". `dir` is a
                    // world-XY direction vector; project it onto the
                    // screen-X / screen-Y axes implied by the grip's
                    // 2-D screen position to compute the in-plane angle.
                    let half_long = h * 1.4;
                    let half_short = h * 0.7;
                    let (cos_t, sin_t) = match grip.dir {
                        Some([dx, dy]) if (dx * dx + dy * dy) > 1e-12 => {
                            let n = (dx * dx + dy * dy).sqrt();
                            // Screen Y is inverted vs world Y → flip sin.
                            (dx / n, -dy / n)
                        }
                        _ => (1.0, 0.0),
                    };
                    let ax = (cos_t * half_long, sin_t * half_long);
                    let ay = (-sin_t * half_short, cos_t * half_short);
                    canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x + ax.0 + ay.0, sp.y + ax.1 + ay.1));
                        b.line_to(Point::new(sp.x + ax.0 - ay.0, sp.y + ax.1 - ay.1));
                        b.line_to(Point::new(sp.x - ax.0 - ay.0, sp.y - ax.1 - ay.1));
                        b.line_to(Point::new(sp.x - ax.0 + ay.0, sp.y - ax.1 + ay.1));
                        b.close();
                    })
                }
                GripShape::Triangle => canvas::Path::new(|b| {
                    b.move_to(Point::new(sp.x, sp.y - h));
                    b.line_to(Point::new(sp.x + h, sp.y + h));
                    b.line_to(Point::new(sp.x - h, sp.y + h));
                    b.close();
                }),
            };

            if grip.is_hot {
                // Hot grip: filled red marker
                let color = Color {
                    r: 1.0,
                    g: 0.15,
                    b: 0.10,
                    a: 1.0,
                };
                frame.fill(&path, color);
            } else {
                // Normal grip: hollow blue marker
                let color = Color {
                    r: 0.10,
                    g: 0.45,
                    b: 0.90,
                    a: 1.0,
                };
                let stroke = canvas::Stroke {
                    width: 1.5,
                    style: canvas::Style::Solid(color),
                    ..Default::default()
                };
                // Fill with semi-transparent background then stroke
                frame.fill(
                    &path,
                    Color {
                        r: 0.10,
                        g: 0.10,
                        b: 0.20,
                        a: 0.7,
                    },
                );
                frame.stroke(&path, stroke);
            }
        }

        // ── Snap marker ───────────────────────────────────────────────────
        if let Some((sp, snap_type)) = self.snap {
            let (r, g, b) = if snap_type == SnapType::ObjectPick {
                (0.95_f32, 0.50, 0.08) // orange object-snap marker
            } else {
                (1.0, 0.9, 0.1) // classic yellow OSNAP
            };
            let marker = Color { r, g, b, a: 1.0 };
            let stroke = canvas::Stroke {
                width: if snap_type == SnapType::ObjectPick { 2.0 } else { 1.5 },
                style: canvas::Style::Solid(marker),
                ..Default::default()
            };
            match snap_type {
                SnapType::ObjectPick => {
                    // Target box + center dot (object-acquisition glyph).
                    let h = 7.0_f32;
                    let rect = canvas::Path::rectangle(
                        Point::new(sp.x - h, sp.y - h),
                        Size::new(h * 2.0, h * 2.0),
                    );
                    frame.stroke(&rect, stroke.clone());
                    let r = 3.0_f32;
                    frame.fill(
                        &canvas::Path::circle(sp, r),
                        Color {
                            r: 0.95,
                            g: 0.50,
                            b: 0.08,
                            a: 0.85,
                        },
                    );
                }
                SnapType::Endpoint => {
                    let h = 5.0_f32;
                    let rect = canvas::Path::rectangle(
                        Point::new(sp.x - h, sp.y - h),
                        Size::new(h * 2.0, h * 2.0),
                    );
                    frame.stroke(&rect, stroke);
                }
                SnapType::Midpoint => {
                    let r = 6.0_f32;
                    let path = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x, sp.y - r));
                        b.line_to(Point::new(sp.x + r * 0.866, sp.y + r * 0.5));
                        b.line_to(Point::new(sp.x - r * 0.866, sp.y + r * 0.5));
                        b.close();
                    });
                    frame.stroke(&path, stroke);
                }
                SnapType::Center => {
                    let r = 5.5_f32;
                    let path = canvas::Path::circle(sp, r);
                    frame.stroke(&path, stroke);
                }
                SnapType::Node => {
                    // Circle with an inscribed X.
                    let r = 5.5_f32;
                    let cpath = canvas::Path::circle(sp, r);
                    frame.stroke(&cpath, stroke.clone());
                    let d = r * std::f32::consts::FRAC_1_SQRT_2;
                    let x1 = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - d, sp.y - d));
                        b.line_to(Point::new(sp.x + d, sp.y + d));
                    });
                    let x2 = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - d, sp.y + d));
                        b.line_to(Point::new(sp.x + d, sp.y - d));
                    });
                    frame.stroke(&x1, stroke.clone());
                    frame.stroke(&x2, stroke);
                }
                SnapType::Quadrant => {
                    let r = 6.0_f32;
                    let path = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x, sp.y - r));
                        b.line_to(Point::new(sp.x + r, sp.y));
                        b.line_to(Point::new(sp.x, sp.y + r));
                        b.line_to(Point::new(sp.x - r, sp.y));
                        b.close();
                    });
                    frame.stroke(&path, stroke);
                }
                SnapType::Intersection => {
                    let r = 5.0_f32;
                    let p1 = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - r, sp.y - r));
                        b.line_to(Point::new(sp.x + r, sp.y + r));
                    });
                    let p2 = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - r, sp.y + r));
                        b.line_to(Point::new(sp.x + r, sp.y - r));
                    });
                    frame.stroke(&p1, stroke.clone());
                    frame.stroke(&p2, stroke);
                }
                SnapType::ApparentIntersection => {
                    // X like Intersection, framed by a small square so the
                    // two are visually distinguishable.
                    let r = 5.0_f32;
                    let rect = canvas::Path::rectangle(
                        Point::new(sp.x - r, sp.y - r),
                        Size::new(r * 2.0, r * 2.0),
                    );
                    frame.stroke(&rect, stroke.clone());
                    let xr = r - 1.5;
                    let p1 = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - xr, sp.y - xr));
                        b.line_to(Point::new(sp.x + xr, sp.y + xr));
                    });
                    let p2 = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - xr, sp.y + xr));
                        b.line_to(Point::new(sp.x + xr, sp.y - xr));
                    });
                    frame.stroke(&p1, stroke.clone());
                    frame.stroke(&p2, stroke);
                }
                SnapType::Insertion => {
                    // Two overlapping rectangles (a small "tag" glyph).
                    let r = 5.0_f32;
                    let inner = canvas::Path::rectangle(
                        Point::new(sp.x - r * 0.5, sp.y - r),
                        Size::new(r, r * 2.0),
                    );
                    let outer = canvas::Path::rectangle(
                        Point::new(sp.x - r, sp.y - r * 0.5),
                        Size::new(r * 2.0, r),
                    );
                    frame.stroke(&outer, stroke.clone());
                    frame.stroke(&inner, stroke);
                }
                SnapType::Perpendicular => {
                    // Right-angle hook in the lower-left quadrant.
                    let r = 6.0_f32;
                    let p = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - r, sp.y - r));
                        b.line_to(Point::new(sp.x - r, sp.y + r));
                        b.line_to(Point::new(sp.x + r, sp.y + r));
                    });
                    let foot = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - r, sp.y));
                        b.line_to(Point::new(sp.x, sp.y));
                        b.line_to(Point::new(sp.x, sp.y + r));
                    });
                    frame.stroke(&p, stroke.clone());
                    frame.stroke(&foot, stroke);
                }
                SnapType::Tangent => {
                    // Circle with a tangent bar across the top.
                    let r = 5.5_f32;
                    let c = canvas::Path::circle(sp, r);
                    frame.stroke(&c, stroke.clone());
                    let bar = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - r, sp.y - r));
                        b.line_to(Point::new(sp.x + r, sp.y - r));
                    });
                    frame.stroke(&bar, stroke);
                }
                SnapType::Nearest => {
                    // Bowtie / hourglass — two opposed triangles meeting at sp.
                    let r = 5.5_f32;
                    let path = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - r, sp.y - r));
                        b.line_to(Point::new(sp.x + r, sp.y - r));
                        b.line_to(Point::new(sp.x - r, sp.y + r));
                        b.line_to(Point::new(sp.x + r, sp.y + r));
                        b.close();
                    });
                    frame.stroke(&path, stroke);
                }
                SnapType::Extension => {
                    // Three dots strung along a tracked direction.
                    let r = 1.4_f32;
                    for k in [-7.0_f32, 0.0, 7.0] {
                        let dot = canvas::Path::circle(Point::new(sp.x + k, sp.y), r);
                        frame.fill(&dot, marker);
                    }
                }
                SnapType::Parallel => {
                    // Two short parallel diagonal bars.
                    let r = 6.0_f32;
                    let off = 3.0_f32;
                    let b1 = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - r - off, sp.y + r));
                        b.line_to(Point::new(sp.x + r - off, sp.y - r));
                    });
                    let b2 = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - r + off, sp.y + r));
                        b.line_to(Point::new(sp.x + r + off, sp.y - r));
                    });
                    frame.stroke(&b1, stroke.clone());
                    frame.stroke(&b2, stroke);
                }
                SnapType::Grid => {
                    let arm = 4.0_f32;
                    let h = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x - arm, sp.y));
                        b.line_to(Point::new(sp.x + arm, sp.y));
                    });
                    let v = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x, sp.y - arm));
                        b.line_to(Point::new(sp.x, sp.y + arm));
                    });
                    frame.stroke(&h, stroke.clone());
                    frame.stroke(&v, stroke);
                }
            }
        }

        // ── CAD crosshair cursor ──────────────────────────────────────────────
        let over_viewcube = self.show_viewcube && {
            use crate::scene::{VIEWCUBE_DRAW_PX, VIEWCUBE_PAD};
            cursor.position_in(bounds).map_or(false, |pos| {
                let vc_x = bounds.width - VIEWCUBE_DRAW_PX - VIEWCUBE_PAD;
                let vc_y = VIEWCUBE_PAD;
                pos.x >= vc_x
                    && pos.x <= vc_x + VIEWCUBE_DRAW_PX
                    && pos.y >= vc_y
                    && pos.y <= vc_y + VIEWCUBE_DRAW_PX
            })
        };
        // Over a Model-tile divider the OS cursor switches to a resize
        // arrow (see `mouse_interaction`); drawing the CAD crosshair on
        // top of it would double up the visual feedback.
        let over_divider = self.tile_edge_under(cursor, bounds).is_some();
        if !over_viewcube && !over_divider {
            if let Some(cp) = self.selection.last_move_pos {
                let color = Color {
                    r: 0.85,
                    g: 0.85,
                    b: 0.85,
                    a: 0.90,
                };
                let stroke = canvas::Stroke {
                    width: 1.0,
                    style: canvas::Style::Solid(color),
                    ..Default::default()
                };
                let sq = CROSSHAIR_SQ; // square half-size → 15×15
                let arm = CROSSHAIR_ARM; // crosshair arm length from center

                // Horizontal arms (start at square edge, end at arm length)
                let h_left = canvas::Path::new(|b| {
                    b.move_to(Point::new(cp.x - sq, cp.y));
                    b.line_to(Point::new(cp.x - arm, cp.y));
                });
                let h_right = canvas::Path::new(|b| {
                    b.move_to(Point::new(cp.x + sq, cp.y));
                    b.line_to(Point::new(cp.x + arm, cp.y));
                });
                // Vertical arms
                let v_top = canvas::Path::new(|b| {
                    b.move_to(Point::new(cp.x, cp.y - sq));
                    b.line_to(Point::new(cp.x, cp.y - arm));
                });
                let v_bot = canvas::Path::new(|b| {
                    b.move_to(Point::new(cp.x, cp.y + sq));
                    b.line_to(Point::new(cp.x, cp.y + arm));
                });
                // Center square
                let square = canvas::Path::rectangle(
                    Point::new(cp.x - sq, cp.y - sq),
                    Size::new(sq * 2.0, sq * 2.0),
                );

                frame.stroke(&h_left, stroke.clone());
                frame.stroke(&h_right, stroke.clone());
                frame.stroke(&v_top, stroke.clone());
                frame.stroke(&v_bot, stroke.clone());
                frame.stroke(&square, stroke);
            }
        } // end !over_viewcube

        // ── UCS icon ──────────────────────────────────────────────────────
        if let Some(ref ucs) = self.ucs_icon {
            draw_ucs_icon(&mut frame, ucs.view_proj, ucs.bounds);
        }

        // ── Object Snap Tracking lines ────────────────────────────────────
        for ost in &self.ost_points {
            let tp = ost.screen;
            let cx = self.cursor_screen.x;
            let cy = self.cursor_screen.y;
            let track_color = Color {
                r: 0.15,
                g: 0.85,
                b: 0.95,
                a: 0.7,
            };
            let dash_stroke = canvas::Stroke::default()
                .with_color(track_color)
                .with_width(1.0);

            // Draw horizontal line from tracking point to cursor.
            if (cy - tp.y).abs() < 8.0 {
                let path = canvas::Path::line(tp, Point { x: cx, y: tp.y });
                frame.stroke(&path, dash_stroke.clone());
            }
            // Draw vertical line.
            if (cx - tp.x).abs() < 8.0 {
                let path = canvas::Path::line(tp, Point { x: tp.x, y: cy });
                frame.stroke(&path, dash_stroke.clone());
            }
            // Small cross at the tracking point.
            let sz = 5.0_f32;
            let h = canvas::Path::line(
                Point {
                    x: tp.x - sz,
                    y: tp.y,
                },
                Point {
                    x: tp.x + sz,
                    y: tp.y,
                },
            );
            let v = canvas::Path::line(
                Point {
                    x: tp.x,
                    y: tp.y - sz,
                },
                Point {
                    x: tp.x,
                    y: tp.y + sz,
                },
            );
            frame.stroke(&h, dash_stroke.clone());
            frame.stroke(&v, dash_stroke);
        }

        vec![frame.into_geometry()]
    }
}

// ── Grid line drawing ─────────────────────────────────────────────────────

/// Minimum pixel gap between adjacent grid lines before stepping up to next spacing.
const MIN_GRID_PX: f32 = 20.0;

fn draw_grid(frame: &mut canvas::Frame, vp: Mat4, plane: GridPlane, bounds: iced::Rectangle) {
    // World → canvas screen: include bounds origin so the grid lands in the
    // active tile's rectangle (the screen → world unproject below stays
    // tile-local, which is what feeds the visible-extent computation).
    let w2s = |world: Vec3| -> Point {
        let ndc = vp.project_point3(world);
        Point::new(
            bounds.x + (ndc.x + 1.0) * 0.5 * bounds.width,
            bounds.y + (1.0 - ndc.y) * 0.5 * bounds.height,
        )
    };

    // Plane-tangent axes: axis1 and axis2 span the grid plane.
    let (axis1, axis2) = match plane {
        GridPlane::Xz => (Vec3::X, Vec3::Z),
        GridPlane::Xy => (Vec3::X, Vec3::Y),
        GridPlane::Yz => (Vec3::Y, Vec3::Z),
    };

    // Adaptive spacing: measure pixels per 1-unit step along each axis,
    // then find the smallest power-of-5 multiple that gives ≥ MIN_GRID_PX.
    let o = w2s(Vec3::ZERO);
    let a1s = w2s(axis1);
    let a2s = w2s(axis2);
    let px1 = ((a1s.x - o.x).powi(2) + (a1s.y - o.y).powi(2)).sqrt();
    let px2 = ((a2s.x - o.x).powi(2) + (a2s.y - o.y).powi(2)).sqrt();
    let px_per_unit = px1.max(px2);
    if px_per_unit < 1e-6 {
        return;
    }

    let mut s = 1.0_f32;
    while s * px_per_unit < MIN_GRID_PX {
        s *= 5.0;
        if s > 1e9 {
            return;
        }
    }

    // Visible world extent: unproject screen corners (mid-depth approximation)
    // and project them onto the grid axes.
    let inv = vp.inverse();
    let unproject = |sx: f32, sy: f32| -> Vec3 {
        let ndc_x = (sx / bounds.width) * 2.0 - 1.0;
        let ndc_y = 1.0 - (sy / bounds.height) * 2.0;
        inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.5))
    };
    let corners = [
        unproject(0.0, 0.0),
        unproject(bounds.width, 0.0),
        unproject(0.0, bounds.height),
        unproject(bounds.width, bounds.height),
    ];
    let range = |ax: Vec3| -> (f32, f32) {
        let vals: Vec<f32> = corners.iter().map(|p| p.dot(ax)).collect();
        (
            vals.iter().cloned().fold(f32::INFINITY, f32::min),
            vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        )
    };
    let (min1, max1) = range(axis1);
    let (min2, max2) = range(axis2);

    let n1_s = (min1 / s).floor() as i32 - 1;
    let n1_e = (max1 / s).ceil() as i32 + 1;
    let n2_s = (min2 / s).floor() as i32 - 1;
    let n2_e = (max2 / s).ceil() as i32 + 1;
    if (n1_e - n1_s) > 500 || (n2_e - n2_s) > 500 {
        return;
    }

    let gc = Color {
        r: 0.28,
        g: 0.28,
        b: 0.28,
        a: 0.7,
    };
    let st = canvas::Stroke {
        width: 0.5,
        style: canvas::Style::Solid(gc),
        ..Default::default()
    };

    // Lines parallel to axis2 (varying axis1 position)
    for i in n1_s..=n1_e {
        let v = i as f32 * s;
        let p0 = w2s(axis1 * v + axis2 * (min2 - s));
        let p1 = w2s(axis1 * v + axis2 * (max2 + s));
        frame.stroke(
            &canvas::Path::new(|b| {
                b.move_to(p0);
                b.line_to(p1);
            }),
            st.clone(),
        );
    }
    // Lines parallel to axis1 (varying axis2 position)
    for i in n2_s..=n2_e {
        let v = i as f32 * s;
        let p0 = w2s(axis2 * v + axis1 * (min1 - s));
        let p1 = w2s(axis2 * v + axis1 * (max1 + s));
        frame.stroke(
            &canvas::Path::new(|b| {
                b.move_to(p0);
                b.line_to(p1);
            }),
            st.clone(),
        );
    }

    // World-space axes always drawn on top of the grid lines.
    let extent = (min1.abs().max(max1.abs()).max(min2.abs()).max(max2.abs()) + s) * 1.5;
    draw_axes(frame, vp, bounds, extent.max(10.0));
}

// ── Coloured world-space axes ─────────────────────────────────────────────

fn draw_axes(frame: &mut canvas::Frame, vp: Mat4, bounds: iced::Rectangle, extent: f32) {
    let w2s = |world: Vec3| -> Point {
        let ndc = vp.project_point3(world);
        Point::new(
            bounds.x + (ndc.x + 1.0) * 0.5 * bounds.width,
            bounds.y + (1.0 - ndc.y) * 0.5 * bounds.height,
        )
    };
    let e = extent;
    let axis_stroke = |r: f32, g: f32, b: f32| canvas::Stroke {
        width: 1.5,
        style: canvas::Style::Solid(Color { r, g, b, a: 0.85 }),
        ..Default::default()
    };
    // X — red
    frame.stroke(
        &canvas::Path::new(|p| {
            p.move_to(w2s(Vec3::new(-e, 0.0, 0.0)));
            p.line_to(w2s(Vec3::new(e, 0.0, 0.0)));
        }),
        axis_stroke(0.90, 0.20, 0.20),
    );
    // Y — green
    frame.stroke(
        &canvas::Path::new(|p| {
            p.move_to(w2s(Vec3::new(0.0, -e, 0.0)));
            p.line_to(w2s(Vec3::new(0.0, e, 0.0)));
        }),
        axis_stroke(0.20, 0.85, 0.20),
    );
    // Z — blue
    frame.stroke(
        &canvas::Path::new(|p| {
            p.move_to(w2s(Vec3::new(0.0, 0.0, -e)));
            p.line_to(w2s(Vec3::new(0.0, 0.0, e)));
        }),
        axis_stroke(0.20, 0.40, 0.90),
    );
}

// ── UCS icon ──────────────────────────────────────────────────────────────
//
// Draws a small X/Y/Z axis tripod in the bottom-left corner of the viewport.
// The axis directions are projected from world space so the icon rotates with
// the camera. Axis lengths are proportional (foreshortening preserved), depth
// ordering is computed from NDC Z, and axes going away from the viewer are
// drawn as outlined circles with reduced opacity.

const UCS_ICON_MARGIN: f32 = 50.0;
const UCS_ICON_LEN: f32 = 38.0; // longest axis arm in screen pixels
const UCS_ICON_TIP: f32 = 7.0; // arrowhead size in pixels

fn draw_ucs_icon(frame: &mut canvas::Frame, vp: Mat4, bounds: iced::Rectangle) {
    if bounds.width < 10.0 || bounds.height < 10.0 {
        return;
    }

    // Project to NDC (including depth) then to screen pixels.
    let w2ndc = |world: Vec3| -> Option<Vec3> {
        let ndc = vp.project_point3(world);
        if !ndc.x.is_finite() || !ndc.y.is_finite() || !ndc.z.is_finite() {
            return None;
        }
        Some(ndc)
    };
    let ndc2s = |ndc: Vec3| -> Point {
        Point::new(
            bounds.x + (ndc.x + 1.0) * 0.5 * bounds.width,
            bounds.y + (1.0 - ndc.y) * 0.5 * bounds.height,
        )
    };

    let Some(org) = w2ndc(Vec3::ZERO) else { return };
    let Some(xn) = w2ndc(Vec3::X) else { return };
    let Some(yn) = w2ndc(Vec3::Y) else { return };
    let Some(zn) = w2ndc(Vec3::Z) else { return };

    let org_s = ndc2s(org);
    let icon_origin = Point::new(
        bounds.x + UCS_ICON_MARGIN,
        bounds.y + (bounds.height - UCS_ICON_MARGIN).max(UCS_ICON_MARGIN),
    );

    // Raw screen-space displacement for each axis tip.
    let raw = |ndc_tip: Vec3| -> (f32, f32, f32) {
        let s = ndc2s(ndc_tip);
        let dx = s.x - org_s.x;
        let dy = s.y - org_s.y;
        (dx, dy, (dx * dx + dy * dy).sqrt())
    };

    let (xdx, xdy, xlen) = raw(xn);
    let (ydx, ydy, ylen) = raw(yn);
    let (zdx, zdy, zlen) = raw(zn);

    // Scale so the longest projected axis fills UCS_ICON_LEN; shorter axes
    // stay proportionally shorter (this IS the foreshortening effect).
    let max_len = xlen.max(ylen).max(zlen).max(1e-4);
    let sc = UCS_ICON_LEN / max_len;

    // depth > 0 → tip is farther from viewer than origin (axis going into screen).
    // depth < 0 → tip is closer (axis coming toward viewer).
    struct AxisInfo {
        dx: f32,
        dy: f32,
        sc_len: f32,
        depth: f32,
        r: f32,
        g: f32,
        b: f32,
        label: &'static str,
    }
    let mut axes = [
        AxisInfo {
            dx: xdx * sc,
            dy: xdy * sc,
            sc_len: xlen * sc,
            depth: xn.z - org.z,
            r: 0.90,
            g: 0.22,
            b: 0.22,
            label: "X",
        },
        AxisInfo {
            dx: ydx * sc,
            dy: ydy * sc,
            sc_len: ylen * sc,
            depth: yn.z - org.z,
            r: 0.22,
            g: 0.85,
            b: 0.22,
            label: "Y",
        },
        AxisInfo {
            dx: zdx * sc,
            dy: zdy * sc,
            sc_len: zlen * sc,
            depth: zn.z - org.z,
            r: 0.22,
            g: 0.45,
            b: 0.90,
            label: "Z",
        },
    ];
    // Back-to-front: draw axis farthest from viewer first.
    axes.sort_by(|a, b| {
        b.depth
            .partial_cmp(&a.depth)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for ax in &axes {
        let col = Color {
            r: ax.r,
            g: ax.g,
            b: ax.b,
            a: 1.0,
        };
        let tip = Point::new(icon_origin.x + ax.dx, icon_origin.y + ax.dy);

        // Shaft
        if ax.sc_len > 1.0 {
            let path = canvas::Path::new(|p| {
                p.move_to(icon_origin);
                p.line_to(tip);
            });
            frame.stroke(
                &path,
                canvas::Stroke {
                    width: 2.0,
                    style: canvas::Style::Solid(col),
                    line_cap: canvas::LineCap::Butt,
                    ..Default::default()
                },
            );
        }

        // Filled arrowhead at tip.
        if ax.sc_len > 3.0 {
            let (nx, ny) = if ax.sc_len > 1e-3 {
                (ax.dx / ax.sc_len, ax.dy / ax.sc_len)
            } else {
                (1.0, 0.0)
            };
            let px = -ny;
            let py = nx;
            let tl = Point::new(
                tip.x - nx * UCS_ICON_TIP + px * (UCS_ICON_TIP * 0.45),
                tip.y - ny * UCS_ICON_TIP + py * (UCS_ICON_TIP * 0.45),
            );
            let tr = Point::new(
                tip.x - nx * UCS_ICON_TIP - px * (UCS_ICON_TIP * 0.45),
                tip.y - ny * UCS_ICON_TIP - py * (UCS_ICON_TIP * 0.45),
            );
            let arrow = canvas::Path::new(|p| {
                p.move_to(tip);
                p.line_to(tl);
                p.line_to(tr);
                p.close();
            });
            frame.fill(&arrow, col);
        }

        // Axis label (X / Y / Z) beyond the tip.
        if ax.sc_len > 4.0 {
            let (nx, ny) = if ax.sc_len > 1e-3 {
                (ax.dx / ax.sc_len, ax.dy / ax.sc_len)
            } else {
                (1.0, 0.0)
            };
            frame.fill_text(canvas::Text {
                content: ax.label.to_string(),
                // Offset beyond tip along the axis direction; subtract ~half glyph
                // size to visually center the single character on the axis line.
                position: Point::new(tip.x + nx * 8.0 - 3.5, tip.y + ny * 8.0 - 5.0),
                color: col,
                size: iced::Pixels(10.0),
                shaping: iced::advanced::text::Shaping::Advanced,
                ..Default::default()
            });
        }
    }

    // Origin dot.
    let circle = canvas::Path::circle(icon_origin, 3.5);
    frame.fill(
        &circle,
        Color {
            r: 0.9,
            g: 0.9,
            b: 0.9,
            a: 0.95,
        },
    );
}

// ── Dynamic Input overlay ─────────────────────────────────────────────────

use crate::command::{DynGuide, DynRole};

const DYN_OFFSET_X: f32 = 14.0;
const DYN_PAD: f32 = 4.0;
const DYN_GAP: f32 = 6.0;
const DYN_FONT: f32 = 11.0;
const DYN_CHAR_W: f32 = DYN_FONT * 0.62;
const DYN_BOX_H: f32 = DYN_FONT + DYN_PAD * 2.0;

/// One value box in the dynamic-input overlay. Its `role` drives both the
/// label and where the box is placed relative to the step's guide geometry.
#[derive(Clone)]
pub struct DynBox {
    pub label: String,
    pub value: String,
    /// TAB-focused box — keystrokes edit this one.
    pub active: bool,
    /// User has typed a value (the box no longer tracks the cursor).
    pub locked: bool,
    pub role: DynRole,
}

pub fn dynamic_input_overlay<'a>(
    cursor_screen: Point,
    base_screen: Option<Point>,
    ref_screen: Option<Point>,
    guide: DynGuide,
    boxes: Vec<DynBox>,
    prompt: String,
) -> Element<'a, Message> {
    canvas(DynInputCanvas {
        cursor_screen,
        base_screen,
        ref_screen,
        guide,
        boxes,
        prompt,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

struct DynInputCanvas {
    cursor_screen: Point,
    /// Step anchor in viewport pixels (projected `dyn_anchor`). Guided layouts
    /// (polar / radius / axis-delta) need it; `None` falls back to a cursor row.
    base_screen: Option<Point>,
    /// Far end of the reference line (projected `dyn_ref`) — for `Perp`.
    ref_screen: Option<Point>,
    guide: DynGuide,
    boxes: Vec<DynBox>,
    /// The active command's current prompt, drawn just above the boxes.
    prompt: String,
}

impl DynInputCanvas {
    fn dotted() -> canvas::Stroke<'static> {
        canvas::Stroke {
            width: 1.0,
            style: canvas::Style::Solid(Color { r: 0.55, g: 0.55, b: 0.58, a: 0.9 }),
            line_dash: canvas::LineDash { segments: &[2.0, 3.0], offset: 0 },
            ..Default::default()
        }
    }

    fn box_content(b: &DynBox) -> String {
        match b.role {
            DynRole::Angle => format!("{}\u{00B0}", b.value),
            _ if b.label.is_empty() => b.value.clone(),
            _ => format!("{}{}", b.label, b.value),
        }
    }

    /// Draw a value box centred at `center`, clamped inside `bounds`.
    fn draw_box(frame: &mut canvas::Frame, b: &DynBox, center: Point, bounds: iced::Rectangle) {
        let content = Self::box_content(b);
        let w = (content.len() as f32 * DYN_CHAR_W) + DYN_PAD * 2.0;
        let x = (center.x - w * 0.5).clamp(0.0, (bounds.width - w).max(0.0));
        let y = (center.y - DYN_BOX_H * 0.5).clamp(0.0, (bounds.height - DYN_BOX_H).max(0.0));
        let rect = canvas::Path::rectangle(Point { x, y }, Size { width: w, height: DYN_BOX_H });
        let (fill, border) = Self::box_colors(b);
        frame.fill(&rect, fill);
        frame.stroke(
            &rect,
            canvas::Stroke::default()
                .with_color(border)
                .with_width(if b.active { 1.6 } else { 1.0 }),
        );
        frame.fill_text(canvas::Text {
            content,
            position: Point { x: x + DYN_PAD, y: y + DYN_PAD },
            color: Color { r: 0.92, g: 0.92, b: 0.92, a: 1.0 },
            size: iced::Pixels(DYN_FONT),
            // Force Advanced shaping: the default `Auto` uses Basic shaping for
            // ASCII-only strings, which the web (wgpu/webgl) backend fails to
            // render — so all-digit value boxes came up blank while the angle
            // box (containing the non-ASCII `°`) rendered. (#117)
            shaping: iced::advanced::text::Shaping::Advanced,
            ..Default::default()
        });
    }

    fn box_colors(b: &DynBox) -> (Color, Color) {
        if b.active {
            (
                Color { r: 0.12, g: 0.18, b: 0.30, a: 0.95 },
                Color { r: 0.45, g: 0.70, b: 1.0, a: 1.0 },
            )
        } else if b.locked {
            (
                Color { r: 0.05, g: 0.05, b: 0.12, a: 0.9 },
                Color { r: 0.95, g: 0.75, b: 0.30, a: 0.9 },
            )
        } else {
            (
                Color { r: 0.05, g: 0.05, b: 0.12, a: 0.9 },
                Color { r: 0.35, g: 0.55, b: 0.90, a: 0.9 },
            )
        }
    }

    /// Prompt pill at `pos`.
    fn draw_prompt(&self, frame: &mut canvas::Frame, pos: Point) {
        if self.prompt.is_empty() {
            return;
        }
        let pw = (self.prompt.len() as f32 * DYN_CHAR_W) + DYN_PAD * 2.0;
        let rect = canvas::Path::rectangle(pos, Size { width: pw, height: DYN_BOX_H });
        frame.fill(&rect, Color { r: 0.10, g: 0.10, b: 0.12, a: 1.0 });
        frame.stroke(
            &rect,
            canvas::Stroke::default()
                .with_color(Color { r: 0.35, g: 0.55, b: 0.90, a: 0.9 })
                .with_width(1.0),
        );
        frame.fill_text(canvas::Text {
            content: self.prompt.clone(),
            position: Point { x: pos.x + DYN_PAD, y: pos.y + DYN_PAD },
            color: Color { r: 0.70, g: 0.85, b: 0.70, a: 1.0 },
            size: iced::Pixels(DYN_FONT),
            shaping: iced::advanced::text::Shaping::Advanced,
            ..Default::default()
        });
    }

    /// Guided layout: draw the guide geometry anchored at `base`, then place
    /// each box according to its role.
    fn draw_guided(&self, frame: &mut canvas::Frame, bounds: iced::Rectangle, base: Point) {
        let cursor = self.cursor_screen;
        let (vx, vy) = (cursor.x - base.x, cursor.y - base.y);
        let len = (vx * vx + vy * vy).sqrt().max(1.0);
        let (dx, dy) = (vx / len, vy / len);
        // Perpendicular pointing to the lower half so labels sit under the line.
        let (mut nx, mut ny) = (-dy, dx);
        if ny < 0.0 {
            nx = -nx;
            ny = -ny;
        }
        // Polar arc reference direction: a supplied reference point (e.g. the
        // ROTATE reference), else the +X axis. The arc sweeps the short way
        // from that reference to the cursor.
        let a_cur = dy.atan2(dx);
        let a_ref = self
            .ref_screen
            .map(|r| (r.y - base.y).atan2(r.x - base.x))
            .unwrap_or(0.0);
        let mut sweep = a_cur - a_ref;
        while sweep > std::f32::consts::PI {
            sweep -= std::f32::consts::TAU;
        }
        while sweep <= -std::f32::consts::PI {
            sweep += std::f32::consts::TAU;
        }
        let corner = Point { x: cursor.x, y: base.y }; // axis-delta elbow

        // Perp / PerpDim: perpendicular direction to the reference line, the
        // measured endpoint along it (`end`), and an offset dimension segment
        // (`off_base`→`off_end`) drawn clear of the edge for PerpDim.
        let perp_info = self.ref_screen.map(|r| {
            let (ax, ay) = (r.x - base.x, r.y - base.y);
            let al = (ax * ax + ay * ay).sqrt().max(1.0);
            let (ux, uy) = (ax / al, ay / al); // axis unit (base → ref)
            let (px, py) = (-uy, ux); // perpendicular unit
            let signed = (cursor.x - base.x) * px + (cursor.y - base.y) * py;
            let end = Point { x: base.x + px * signed, y: base.y + py * signed };
            const OFF: f32 = 16.0; // dimension offset, away from the reference
            let off_base = Point { x: base.x - ux * OFF, y: base.y - uy * OFF };
            let off_end = Point { x: end.x - ux * OFF, y: end.y - uy * OFF };
            (end, off_base, off_end)
        });

        // ── Guide geometry ──
        match self.guide {
            DynGuide::Polar => {
                // Reference line along `a_ref` (the +X axis, or the supplied
                // reference direction), then the arc from it to the cursor.
                let href = canvas::Path::new(|p| {
                    p.move_to(base);
                    p.line_to(Point {
                        x: base.x + a_ref.cos() * len,
                        y: base.y + a_ref.sin() * len,
                    });
                });
                frame.stroke(&href, Self::dotted());
                let arc = canvas::Path::new(|p| {
                    let steps = 48;
                    for k in 0..=steps {
                        let a = a_ref + sweep * (k as f32 / steps as f32);
                        let pt = Point {
                            x: base.x + a.cos() * len,
                            y: base.y + a.sin() * len,
                        };
                        if k == 0 {
                            p.move_to(pt);
                        } else {
                            p.line_to(pt);
                        }
                    }
                });
                frame.stroke(&arc, Self::dotted());
            }
            DynGuide::Radius => {
                let line = canvas::Path::new(|p| {
                    p.move_to(base);
                    p.line_to(cursor);
                });
                frame.stroke(&line, Self::dotted());
            }
            DynGuide::Perp => {
                if let Some((end, _, _)) = perp_info {
                    // The measured semi-axis: anchor → perpendicular endpoint.
                    let line = canvas::Path::new(|p| {
                        p.move_to(base);
                        p.line_to(end);
                    });
                    frame.stroke(&line, Self::dotted());
                }
            }
            DynGuide::PerpDim => {
                if let Some((end, ob, oe)) = perp_info {
                    // Dimension segment offset off the edge, with extension
                    // lines back to the two measured corners.
                    let dim = canvas::Path::new(|p| {
                        p.move_to(ob);
                        p.line_to(oe);
                    });
                    frame.stroke(&dim, Self::dotted());
                    let ext = canvas::Path::new(|p| {
                        p.move_to(base);
                        p.line_to(ob);
                        p.move_to(end);
                        p.line_to(oe);
                    });
                    frame.stroke(&ext, Self::dotted());
                }
            }
            DynGuide::AxisDelta | DynGuide::RectSides => {
                // Dotted legs from the anchor along its axes to the cursor.
                let legs = canvas::Path::new(|p| {
                    p.move_to(base);
                    p.line_to(corner);
                    p.line_to(cursor);
                });
                frame.stroke(&legs, Self::dotted());
                if self.guide == DynGuide::RectSides {
                    // Close the rectangle so both side pairs read as a box.
                    let rest = canvas::Path::new(|p| {
                        p.move_to(base);
                        p.line_to(Point { x: base.x, y: cursor.y });
                        p.line_to(cursor);
                    });
                    frame.stroke(&rest, Self::dotted());
                }
            }
            DynGuide::None => {}
        }

        // ── Box placement by role ──
        for b in &self.boxes {
            let center = match b.role {
                DynRole::Angle => {
                    let a_mid = a_ref + sweep * 0.5;
                    // Pull the box back along the ray and lift it to the side
                    // opposite the distance box. A near-zero sweep collapses the
                    // mid-angle direction onto the cursor ray, so placing the box
                    // at full `len` would plant it on the cursor / snap point and
                    // hide it. (#124)
                    let r = (len - DYN_BOX_H * 2.0).max(len * 0.5);
                    Point {
                        x: base.x + a_mid.cos() * r - nx * 18.0,
                        y: base.y + a_mid.sin() * r - ny * 18.0,
                    }
                }
                DynRole::X | DynRole::Width => Point {
                    x: (base.x + cursor.x) * 0.5,
                    y: base.y + 14.0,
                },
                DynRole::Y | DynRole::Height => Point {
                    x: corner.x + 18.0,
                    y: (base.y + cursor.y) * 0.5,
                },
                // Perpendicular measure: on the measured segment / dim line.
                _ if matches!(self.guide, DynGuide::Perp | DynGuide::PerpDim)
                    && perp_info.is_some() =>
                {
                    let (end, ob, oe) = perp_info.unwrap();
                    if self.guide == DynGuide::PerpDim {
                        Point { x: (ob.x + oe.x) * 0.5 + 8.0, y: (ob.y + oe.y) * 0.5 }
                    } else {
                        Point { x: (base.x + end.x) * 0.5 + 8.0, y: (base.y + end.y) * 0.5 }
                    }
                }
                // Distance / Radius / Diameter and anything else ride the line.
                _ => Point {
                    x: base.x + dx * len * 0.5 + nx * 16.0,
                    y: base.y + dy * len * 0.5 + ny * 16.0,
                },
            };
            Self::draw_box(frame, b, center, bounds);
        }
    }

    /// Fallback row layout near the cursor (no anchor / `None` guide).
    fn draw_row(&self, frame: &mut canvas::Frame, bounds: iced::Rectangle) {
        let texts: Vec<String> = self
            .boxes
            .iter()
            .map(|b| {
                if b.label.is_empty() {
                    b.value.clone()
                } else {
                    format!("{}:{}", b.label, b.value)
                }
            })
            .collect();
        let widths: Vec<f32> = texts
            .iter()
            .map(|t| (t.len() as f32 * DYN_CHAR_W) + DYN_PAD * 2.0)
            .collect();
        let total_w: f32 =
            widths.iter().sum::<f32>() + DYN_GAP * (self.boxes.len() as f32 - 1.0);

        // Offset the block off the crosshair by the same gap horizontally and
        // vertically; the prompt sits a gap below the horizontal axis and the
        // value boxes a further gap below the prompt.
        let pad = DYN_OFFSET_X;
        let has_prompt = !self.prompt.is_empty();
        let prompt_w = (self.prompt.len() as f32 * DYN_CHAR_W) + DYN_PAD * 2.0;
        let block_w = total_w.max(if has_prompt { prompt_w } else { 0.0 });
        let mut bx = self.cursor_screen.x + pad;
        let mut py = self.cursor_screen.y + pad;
        let mut by = if has_prompt { py + DYN_BOX_H + pad } else { py };
        if bx + block_w > bounds.width {
            bx = (self.cursor_screen.x - block_w - 4.0).max(0.0);
        }
        if by + DYN_BOX_H > bounds.height {
            // Flip the block above the cursor, keeping the same gaps.
            by = (self.cursor_screen.y - pad - DYN_BOX_H).max(0.0);
            py = (by - pad - DYN_BOX_H).max(0.0);
        }
        if has_prompt {
            self.draw_prompt(frame, Point { x: bx, y: py });
        }

        let mut x = bx;
        for (i, b) in self.boxes.iter().enumerate() {
            let w = widths[i];
            let rect =
                canvas::Path::rectangle(Point { x, y: by }, Size { width: w, height: DYN_BOX_H });
            let (fill, border) = Self::box_colors(b);
            frame.fill(&rect, fill);
            frame.stroke(
                &rect,
                canvas::Stroke::default()
                    .with_color(border)
                    .with_width(if b.active { 1.6 } else { 1.0 }),
            );
            frame.fill_text(canvas::Text {
                content: texts[i].clone(),
                position: Point { x: x + DYN_PAD, y: by + DYN_PAD },
                color: Color { r: 0.92, g: 0.92, b: 0.92, a: 1.0 },
                size: iced::Pixels(DYN_FONT),
                shaping: iced::advanced::text::Shaping::Advanced,
                ..Default::default()
            });
            x += w + DYN_GAP;
        }
    }
}

impl canvas::Program<Message> for DynInputCanvas {
    type State = ();

    fn mouse_interaction(
        &self,
        _state: &(),
        _bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        mouse::Interaction::None
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // No boxes — just the prompt pill near the cursor.
        if self.boxes.is_empty() {
            if !self.prompt.is_empty() {
                let pw = (self.prompt.len() as f32 * DYN_CHAR_W) + DYN_PAD * 2.0;
                let mut px = self.cursor_screen.x + DYN_OFFSET_X;
                let mut py = self.cursor_screen.y + DYN_OFFSET_X;
                if px + pw > bounds.width {
                    px = (self.cursor_screen.x - pw - 4.0).max(0.0);
                }
                if py + DYN_BOX_H > bounds.height {
                    py = (self.cursor_screen.y - DYN_BOX_H - 4.0).max(0.0);
                }
                self.draw_prompt(&mut frame, Point { x: px, y: py });
            }
            return vec![frame.into_geometry()];
        }

        // Guided layouts need the anchor; without it fall back to a cursor row.
        match (self.guide, self.base_screen) {
            (DynGuide::None, _) | (_, None) => self.draw_row(&mut frame, bounds),
            (_, Some(base)) => self.draw_guided(&mut frame, bounds, base),
        }
        vec![frame.into_geometry()]
    }
}
