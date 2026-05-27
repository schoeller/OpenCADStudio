//! Viewport overlay widgets:
//!   • nav_toolbar()     — vertical orbit/pan/zoom buttons on the right

use glam::{Mat4, Vec3};
use iced::mouse;
use iced::widget::{button, canvas, column, container, text};
use iced::{Background, Border, Color, Element, Length, Point, Size, Theme};

use crate::app::Message;
use crate::scene::object::GripShape;
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
}

// ── Nav toolbar ───────────────────────────────────────────────────────────

pub fn nav_toolbar<'a>() -> Element<'a, Message> {
    let b = |icon: &'a str, cmd: &'a str| -> Element<'a, Message> {
        button(text(icon).size(14).color(Color::WHITE))
            .on_press(Message::Command(cmd.into()))
            .style(|_: &Theme, status| button::Style {
                background: Some(Background::Color(match status {
                    button::Status::Hovered => Color {
                        r: 0.32,
                        g: 0.32,
                        b: 0.32,
                        a: 0.95,
                    },
                    button::Status::Pressed => Color {
                        r: 0.18,
                        g: 0.42,
                        b: 0.70,
                        a: 1.00,
                    },
                    _ => Color {
                        r: 0.20,
                        g: 0.20,
                        b: 0.20,
                        a: 0.85,
                    },
                })),
                border: Border {
                    color: Color {
                        r: 0.30,
                        g: 0.30,
                        b: 0.30,
                        a: 1.0,
                    },
                    width: 1.0,
                    radius: 2.0.into(),
                },
                text_color: Color::WHITE,
                shadow: iced::Shadow::default(),
                snap: false,
            })
            .padding([6, 8])
            .into()
    };
    container(
        column![
            b("⟳", "3DORBIT"),
            b("✥", "PAN"),
            b("⊕", "ZOOMIN"),
            b("⊖", "ZOOMOUT"),
            b("⊡", "ZOOMEXTENTS")
        ]
        .spacing(2),
    )
    .padding(4)
    .into()
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
        if cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
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

        // ── Grid display ──────────────────────────────────────────────────
        if let Some(ref g) = self.grid {
            draw_grid(&mut frame, g.view_proj, g.plane, g.bounds);
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
            let h = crate::scene::grip::GRIP_HALF_PX;
            let path = match grip.shape {
                GripShape::Square => canvas::Path::rectangle(
                    Point::new(sp.x - h, sp.y - h),
                    Size::new(h * 2.0, h * 2.0),
                ),
                GripShape::Diamond => canvas::Path::new(|b| {
                    b.move_to(Point::new(sp.x, sp.y - h));
                    b.line_to(Point::new(sp.x + h, sp.y));
                    b.line_to(Point::new(sp.x, sp.y + h));
                    b.line_to(Point::new(sp.x - h, sp.y));
                    b.close();
                }),
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
            let yellow = Color {
                r: 1.0,
                g: 0.9,
                b: 0.1,
                a: 1.0,
            };
            let stroke = canvas::Stroke {
                width: 1.5,
                style: canvas::Style::Solid(yellow),
                ..Default::default()
            };
            match snap_type {
                SnapType::Endpoint => {
                    let half = 5.0_f32;
                    let rect = canvas::Path::rectangle(
                        Point::new(sp.x - half, sp.y - half),
                        Size::new(half * 2.0, half * 2.0),
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
                // Other snap types use the same diamond marker.
                _ => {
                    let r = 5.0_f32;
                    let path = canvas::Path::new(|b| {
                        b.move_to(Point::new(sp.x, sp.y - r));
                        b.line_to(Point::new(sp.x + r, sp.y));
                        b.line_to(Point::new(sp.x, sp.y + r));
                        b.line_to(Point::new(sp.x - r, sp.y));
                        b.close();
                    });
                    frame.stroke(&path, stroke);
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
        if !over_viewcube {
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
    let w2s = |world: Vec3| -> Point {
        let ndc = vp.project_point3(world);
        Point::new(
            (ndc.x + 1.0) * 0.5 * bounds.width,
            (1.0 - ndc.y) * 0.5 * bounds.height,
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
            (ndc.x + 1.0) * 0.5 * bounds.width,
            (1.0 - ndc.y) * 0.5 * bounds.height,
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
            (ndc.x + 1.0) * 0.5 * bounds.width,
            (1.0 - ndc.y) * 0.5 * bounds.height,
        )
    };

    let Some(org) = w2ndc(Vec3::ZERO) else { return };
    let Some(xn) = w2ndc(Vec3::X) else { return };
    let Some(yn) = w2ndc(Vec3::Y) else { return };
    let Some(zn) = w2ndc(Vec3::Z) else { return };

    let org_s = ndc2s(org);
    let icon_origin = Point::new(
        UCS_ICON_MARGIN,
        (bounds.height - UCS_ICON_MARGIN).max(UCS_ICON_MARGIN),
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

/// Draw a small coordinate / distance-angle tooltip near the cursor.
///
/// `cursor_screen` — cursor position in viewport pixels.
/// `label` — text to display (e.g. "X: 12.34  Y: 56.78").
/// One labelled box in the dynamic-input overlay (e.g. `d` = distance,
/// `<` = angle, `X` / `Y` = ordinates).
#[derive(Clone)]
pub struct DynBox {
    pub label: String,
    pub value: String,
    /// TAB-focused box — keystrokes edit this one.
    pub active: bool,
    /// User has typed a value (the box no longer tracks the cursor).
    pub locked: bool,
}

pub fn dynamic_input_overlay<'a>(cursor_screen: Point, boxes: Vec<DynBox>) -> Element<'a, Message> {
    canvas(DynInputCanvas {
        cursor_screen,
        boxes,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

struct DynInputCanvas {
    cursor_screen: Point,
    boxes: Vec<DynBox>,
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
        if self.boxes.is_empty() {
            return vec![frame.into_geometry()];
        }

        // Offset the row 14 px right and 20 px below the cursor.
        const OFFSET_X: f32 = 14.0;
        const OFFSET_Y: f32 = 20.0;
        const PAD: f32 = 4.0;
        const GAP: f32 = 6.0;
        const FONT_SIZE: f32 = 11.0;
        const CHAR_W: f32 = FONT_SIZE * 0.62; // monospace-ish width estimate
        const BOX_H: f32 = FONT_SIZE + PAD * 2.0;

        // Each box is "<label>:<value>"; width tracks the text length.
        let texts: Vec<String> = self
            .boxes
            .iter()
            .map(|b| format!("{}:{}", b.label, b.value))
            .collect();
        let widths: Vec<f32> = texts
            .iter()
            .map(|t| (t.len() as f32 * CHAR_W) + PAD * 2.0)
            .collect();
        let total_w: f32 = widths.iter().sum::<f32>() + GAP * (self.boxes.len() as f32 - 1.0);

        let mut bx = self.cursor_screen.x + OFFSET_X;
        let mut by = self.cursor_screen.y + OFFSET_Y;
        if bx + total_w > bounds.width {
            bx = (self.cursor_screen.x - total_w - 4.0).max(0.0);
        }
        if by + BOX_H > bounds.height {
            by = (self.cursor_screen.y - BOX_H - 4.0).max(0.0);
        }

        let mut x = bx;
        for (i, b) in self.boxes.iter().enumerate() {
            let w = widths[i];
            let rect = canvas::Path::rectangle(Point { x, y: by }, Size { width: w, height: BOX_H });
            // Active box: brighter fill + accent border. Locked (typed)
            // boxes get a warm border so it's clear they hold a fixed
            // value rather than tracking the cursor.
            let (fill, border) = if b.active {
                (
                    Color { r: 0.12, g: 0.18, b: 0.30, a: 0.92 },
                    Color { r: 0.45, g: 0.70, b: 1.0, a: 1.0 },
                )
            } else if b.locked {
                (
                    Color { r: 0.05, g: 0.05, b: 0.12, a: 0.85 },
                    Color { r: 0.95, g: 0.75, b: 0.30, a: 0.9 },
                )
            } else {
                (
                    Color { r: 0.05, g: 0.05, b: 0.12, a: 0.85 },
                    Color { r: 0.35, g: 0.55, b: 0.90, a: 0.9 },
                )
            };
            frame.fill(&rect, fill);
            frame.stroke(
                &rect,
                canvas::Stroke::default()
                    .with_color(border)
                    .with_width(if b.active { 1.6 } else { 1.0 }),
            );
            frame.fill_text(canvas::Text {
                content: texts[i].clone(),
                position: Point { x: x + PAD, y: by + PAD },
                color: Color { r: 0.92, g: 0.92, b: 0.92, a: 1.0 },
                size: iced::Pixels(FONT_SIZE),
                ..Default::default()
            });
            x += w + GAP;
        }

        vec![frame.into_geometry()]
    }
}
