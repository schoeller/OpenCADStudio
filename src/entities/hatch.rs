use acadrust::entities::{BoundaryEdge, Hatch};
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{center_grip, circle_grip, edit_prop as edit, parse_f64, ro_prop as ro};
use crate::entities::traits::{FallbackTess, Grippable, PropertyEditable, Transformable};
use crate::scene::model::object::{GripApply, GripDef, PropSection, PropValue, Property};
use crate::scene::convert::tess_util::{arc_segments, arc_signed_span, wire_chord_tol, FallbackGeometry};
use crate::scene::model::wire_model::SnapHint;

/// Signed area of every boundary path, summed via the shoelace formula over
/// the OCS boundary vertices (bulge/curved edges approximated by their end
/// points). Returns the absolute total area.
fn boundary_area(h: &Hatch) -> f64 {
    let mut area = 0.0;
    for path in &h.paths {
        let mut ring: Vec<[f64; 2]> = Vec::new();
        for edge in &path.edges {
            match edge {
                BoundaryEdge::Polyline(poly) => {
                    for v in &poly.vertices {
                        ring.push([v.x, v.y]);
                    }
                }
                BoundaryEdge::Line(l) => {
                    ring.push([l.start.x, l.start.y]);
                    ring.push([l.end.x, l.end.y]);
                }
                BoundaryEdge::CircularArc(a) => ring.push([a.center.x, a.center.y]),
                BoundaryEdge::EllipticArc(e) => ring.push([e.center.x, e.center.y]),
                BoundaryEdge::Spline(s) => {
                    let src = if !s.fit_points.is_empty() {
                        s.fit_points.iter().map(|p| [p.x, p.y]).collect::<Vec<_>>()
                    } else {
                        s.control_points
                            .iter()
                            .map(|p| [p.x, p.y])
                            .collect::<Vec<_>>()
                    };
                    ring.extend(src);
                }
            }
        }
        let n = ring.len();
        if n < 3 {
            continue;
        }
        let mut acc = 0.0;
        for i in 0..n {
            let a = ring[i];
            let b = ring[(i + 1) % n];
            acc += a[0] * b[1] - b[0] * a[1];
        }
        area += acc.abs() * 0.5;
    }
    area
}

/// Mean of every boundary edge point (OCS) — the centroid used to place the
/// pattern-origin grip on the hatch body. `None` when there are no boundary
/// points.
fn boundary_centroid(h: &Hatch) -> Option<(f64, f64)> {
    let mut sx = 0.0;
    let mut sy = 0.0;
    let mut n = 0.0;
    let mut add = |x: f64, y: f64| {
        sx += x;
        sy += y;
        n += 1.0;
    };
    for path in &h.paths {
        for edge in &path.edges {
            match edge {
                BoundaryEdge::Polyline(p) => {
                    for v in &p.vertices {
                        add(v.x, v.y);
                    }
                }
                BoundaryEdge::Line(l) => {
                    add(l.start.x, l.start.y);
                    add(l.end.x, l.end.y);
                }
                BoundaryEdge::CircularArc(a) => add(a.center.x, a.center.y),
                BoundaryEdge::EllipticArc(e) => add(e.center.x, e.center.y),
                BoundaryEdge::Spline(s) => {
                    if !s.fit_points.is_empty() {
                        for p in &s.fit_points {
                            add(p.x, p.y);
                        }
                    } else {
                        for p in &s.control_points {
                            add(p.x, p.y);
                        }
                    }
                }
            }
        }
    }
    (n > 0.0).then(|| (sx / n, sy / n))
}

/// Read the hatch background colour from its `HATCHBACKGROUNDCOLOR` extended
/// data (group 1071, packed true colour: high byte = colour method, low three =
/// RGB). `None` when unset or not a true-colour value.
pub fn background_color(h: &Hatch) -> Option<acadrust::types::Color> {
    let rec = h.common.extended_data.get_record("HATCHBACKGROUNDCOLOR")?;
    let raw = rec.values.iter().find_map(|v| match v {
        acadrust::xdata::XDataValue::Integer32(n) => Some(*n as u32),
        _ => None,
    })?;
    match (raw >> 24) & 0xFF {
        // Colour-method byte: C0 = ByLayer, C1 = ByBlock, C2 = true colour,
        // C3 = ACI index.
        0xC0 => Some(acadrust::types::Color::ByLayer),
        0xC1 => Some(acadrust::types::Color::ByBlock),
        0xC2 => Some(acadrust::types::Color::Rgb {
            r: ((raw >> 16) & 0xFF) as u8,
            g: ((raw >> 8) & 0xFF) as u8,
            b: (raw & 0xFF) as u8,
        }),
        0xC3 => Some(acadrust::types::Color::Index((raw & 0xFF) as u8)),
        _ => None,
    }
}

/// Pack an RGB colour into a `HATCHBACKGROUNDCOLOR` group-1071 true-colour word.
pub fn pack_background_color(c: &acadrust::types::Color) -> i32 {
    use acadrust::types::Color;
    (match c {
        Color::ByLayer => 0xC0000000u32,
        Color::ByBlock => 0xC1000000u32,
        Color::Index(i) => 0xC3000000u32 | (*i as u8 as u32),
        _ => {
            let (r, g, b) = c.rgb().unwrap_or((255, 255, 255));
            0xC2000000u32 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        }
    }) as i32
}

/// Replace (or insert) the hatch's `HATCHBACKGROUNDCOLOR` extended-data record,
/// preserving every other application's records.
pub fn set_background_color(h: &mut Hatch, c: &acadrust::types::Color) {
    use acadrust::xdata::{ExtendedDataRecord, XDataValue};
    const APP: &str = "HATCHBACKGROUNDCOLOR";
    let kept: Vec<ExtendedDataRecord> = h
        .common
        .extended_data
        .records()
        .iter()
        .filter(|r| r.application_name != APP)
        .cloned()
        .collect();
    h.common.extended_data.clear();
    for r in kept {
        h.common.extended_data.add_record(r);
    }
    let mut rec = ExtendedDataRecord::new(APP);
    rec.add_value(XDataValue::Integer32(pack_background_color(c)));
    h.common.extended_data.add_record(rec);
}

/// Remove the hatch's `HATCHBACKGROUNDCOLOR` extended-data record (background
/// off), preserving every other application's records.
pub fn clear_background_color(h: &mut Hatch) {
    use acadrust::xdata::ExtendedDataRecord;
    const APP: &str = "HATCHBACKGROUNDCOLOR";
    let kept: Vec<ExtendedDataRecord> = h
        .common
        .extended_data
        .records()
        .iter()
        .filter(|r| r.application_name != APP)
        .cloned()
        .collect();
    h.common.extended_data.clear();
    for r in kept {
        h.common.extended_data.add_record(r);
    }
}

fn properties(h: &Hatch) -> Vec<PropSection> {
    let pattern_type = match h.pattern_type {
        acadrust::entities::HatchPatternType::Predefined => "Predefined",
        acadrust::entities::HatchPatternType::UserDefined => "User Defined",
        acadrust::entities::HatchPatternType::Custom => "Custom",
    };
    let style = match h.style {
        acadrust::entities::HatchStyleType::Normal => "Normal",
        acadrust::entities::HatchStyleType::Outer => "Outer",
        acadrust::entities::HatchStyleType::Ignore => "Ignore",
    };
    let area = boundary_area(h);
    let g = &h.gradient_color;
    let default_col = acadrust::types::Color::Index(7);
    let grad_c1 = g.colors.first().map(|e| e.color).unwrap_or(default_col);
    let grad_c2 = g.colors.get(1).map(|e| e.color).unwrap_or(default_col);
    let bg_on = background_color(h).is_some();
    let bg_col = background_color(h).unwrap_or(default_col);

    if g.enabled {
        // ── Gradient fill ──────────────────────────────────────────────────
        let grad_type = if g.is_single_color { "One color" } else { "Two color" };
        let centered = if g.shift.abs() < 1e-9 { "Yes" } else { "No" };
        let mut sections = vec![
            PropSection {
                title: "Pattern".into(),
                props: vec![
                    Property {
                        label: "Type".into(),
                        field: "fill_type",
                        value: PropValue::Choice {
                            selected: grad_type.to_string(),
                            options: vec!["Two color".into(), "One color".into()],
                        },
                    },
                    ro("Gradient name", "gradient_name", g.name.clone()),
                    Property {
                        label: "Color 1".into(),
                        field: "gradient_color_1",
                        value: PropValue::ColorChoice(grad_c1),
                    },
                    Property {
                        label: "Color 2".into(),
                        field: "gradient_color_2",
                        value: PropValue::ColorChoice(grad_c2),
                    },
                    edit("Angle", "pattern_angle", g.angle.to_degrees()),
                    ro("Centered", "gradient_centered", centered),
                ],
            },
            PropSection {
                title: "Geometry".into(),
                props: vec![
                    edit("Elevation", "elevation", h.elevation),
                    ro("Area", "area", format!("{:.4}", area)),
                    ro("Cumulative area", "cumulative_area", format!("{:.4}", area)),
                ],
            },
            PropSection {
                title: "Misc".into(),
                props: vec![
                    ro(
                        "Associative",
                        "associative",
                        if h.is_associative { "Yes" } else { "No" },
                    ),
                    ro("Annotative", "annotative", String::new()),
                    ro("Island detection style", "style", style),
                    Property {
                        label: "Background".into(),
                        field: "bg_enabled",
                        value: PropValue::BoolToggle {
                            field: "bg_enabled",
                            value: bg_on,
                        },
                    },
                ],
            },
        ];
        if bg_on {
            if let Some(sec) = sections.last_mut() {
                sec.props.push(Property {
                    label: "Background color".into(),
                    field: "background_color",
                    value: PropValue::ColorChoice(bg_col),
                });
            }
        }
        return sections;
    }


    // ── Hatch (pattern / solid) ────────────────────────────────────────────
    // "Type" = pattern definition source (Predefined / User Defined / Custom).
    let type_row = Property {
        label: "Type".into(),
        field: "pattern_type_label",
        value: PropValue::Choice {
            selected: pattern_type.to_string(),
            options: vec!["Predefined".into(), "User Defined".into(), "Custom".into()],
        },
    };
    let pattern_name_row = Property {
        label: "Pattern name".into(),
        field: "pattern_name",
        value: PropValue::HatchPatternChoice(h.pattern.name.clone()),
    };
    let associative_row = Property {
        label: "Associative".into(),
        field: "associative",
        value: PropValue::BoolToggle {
            field: "associative",
            value: h.is_associative,
        },
    };
    let double_row = Property {
        label: "Double".into(),
        field: "double",
        value: PropValue::BoolToggle {
            field: "double",
            value: h.is_double,
        },
    };
    let island_row = Property {
        label: "Island detection style".into(),
        field: "style",
        value: PropValue::Choice {
            selected: style.to_string(),
            options: vec!["Normal".into(), "Outer".into(), "Ignore".into()],
        },
    };
    let spacing_row = edit(
        "Spacing",
        "spacing",
        h.pattern
            .lines
            .first()
            .map(|l| l.offset.length())
            .unwrap_or_default(),
    );
    // Pattern tiling origin: the base point the pattern lines are anchored to.
    let (origin_x, origin_y) = h
        .pattern
        .lines
        .first()
        .map(|l| (l.base_point.x, l.base_point.y))
        .unwrap_or((0.0, 0.0));

    let mut sections = vec![
        PropSection {
            title: "Pattern".into(),
            props: vec![
                type_row,
                pattern_name_row,
                ro("Annotative", "annotative", String::new()),
                edit("Angle", "pattern_angle", h.pattern_angle.to_degrees()),
                edit("Scale", "pattern_scale", h.pattern_scale),
                edit("Origin X", "origin_x", origin_x),
                edit("Origin Y", "origin_y", origin_y),
                spacing_row,
                ro("ISO pen width", "iso_pen_width", String::new()),
                double_row,
                associative_row,
                island_row,
                Property {
                    label: "Background".into(),
                    field: "bg_enabled",
                    value: PropValue::BoolToggle {
                        field: "bg_enabled",
                        value: bg_on,
                    },
                },
            ],
        },
        PropSection {
            title: "Geometry".into(),
            props: vec![
                edit("Elevation", "elevation", h.elevation),
                ro("Area", "area", format!("{:.4}", area)),
                ro("Cumulative area", "cumulative_area", format!("{:.4}", area)),
            ],
        },
    ];
    if bg_on {
        if let Some(sec) = sections.first_mut() {
            sec.props.push(Property {
                label: "Background color".into(),
                field: "background_color",
                value: PropValue::ColorChoice(bg_col),
            });
        }
    }
    sections
}

fn apply_geom_prop(h: &mut Hatch, field: &str, value: &str) {
    use acadrust::entities::{HatchPatternType, HatchStyleType};
    // Non-numeric fields (bool toggles / enum choices) — handled before the
    // f64 parse below, which would otherwise reject their string values.
    match field {
        // Background on/off (#415): off removes the HATCHBACKGROUNDCOLOR
        // record entirely (no background), on seeds a white one for the
        // colour picker to change.
        "bg_enabled" => {
            if background_color(h).is_some() {
                clear_background_color(h);
            } else {
                set_background_color(h, &acadrust::types::Color::ByLayer);
            }
            return;
        }
        "double" => {
            h.is_double = if value == "toggle" {
                !h.is_double
            } else {
                value == "true"
            };
            return;
        }
        "associative" => {
            h.is_associative = if value == "toggle" {
                !h.is_associative
            } else {
                value == "true"
            };
            return;
        }
        "pattern_type_label" => {
            h.pattern_type = match value {
                "Predefined" => HatchPatternType::Predefined,
                "User Defined" => HatchPatternType::UserDefined,
                "Custom" => HatchPatternType::Custom,
                _ => h.pattern_type,
            };
            return;
        }
        "style" => {
            h.style = match value {
                "Normal" => HatchStyleType::Normal,
                "Outer" => HatchStyleType::Outer,
                "Ignore" => HatchStyleType::Ignore,
                _ => h.style,
            };
            return;
        }
        "fill_type" => {
            h.gradient_color.is_single_color = value == "One color";
            return;
        }
        _ => {}
    }
    let Some(v) = parse_f64(value) else {
        return;
    };
    match field {
        "pattern_angle" if h.gradient_color.enabled => h.gradient_color.angle = v.to_radians(),
        "pattern_angle" => h.pattern_angle = v.to_radians(),
        // The stored pattern lines are the FINAL rendered geometry (offsets,
        // base points and dashes in world units — see the prebaked path in
        // hatch_model_from_dxf), so changing the scale must rescale them by
        // the ratio or the edit has no visible effect (#415).
        "pattern_scale" if v > 0.0 => {
            let old = h.pattern_scale;
            if old > 1e-12 {
                let k = v / old;
                for line in h.pattern.lines.iter_mut() {
                    line.base_point.x *= k;
                    line.base_point.y *= k;
                    line.offset.x *= k;
                    line.offset.y *= k;
                    for d in line.dash_lengths.iter_mut() {
                        *d *= k;
                    }
                }
            }
            h.pattern_scale = v;
        }
        // Scale every pattern line's offset so the first line's spacing = v,
        // preserving the relative spacing between lines.
        "spacing" if v > 0.0 => {
            let cur = h
                .pattern
                .lines
                .first()
                .map(|l| l.offset.length())
                .unwrap_or(0.0);
            if cur > 1e-9 {
                let s = v / cur;
                for line in h.pattern.lines.iter_mut() {
                    line.offset.x *= s;
                    line.offset.y *= s;
                }
            }
        }
        // Move the pattern origin: shift every line's base point by the delta
        // from the current origin (first line), preserving their relative offsets.
        "origin_x" => {
            if let Some(cur) = h.pattern.lines.first().map(|l| l.base_point.x) {
                let d = v - cur;
                for line in h.pattern.lines.iter_mut() {
                    line.base_point.x += d;
                }
            }
        }
        "origin_y" => {
            if let Some(cur) = h.pattern.lines.first().map(|l| l.base_point.y) {
                let d = v - cur;
                for line in h.pattern.lines.iter_mut() {
                    line.base_point.y += d;
                }
            }
        }
        "elevation" => h.elevation = v,
        _ => {}
    }
}

fn apply_transform(h: &mut Hatch, t: &EntityTransform) {
    crate::scene::view::transform::apply_standard_entity_transform(h, t, |entity, p1, p2| {
        // Delegate the mirror to acadrust's transform_hatch (via the Entity
        // trait): it flips the boundary-arc direction flags, re-mirrors the
        // stored angles and preserves the stored sweep — including the
        // wrap-encoded end angles above 2π that AutoCAD writes. The old
        // hand-rolled angle-swap here was only valid for ccw boundary arcs on
        // an axis-aligned mirror line and went stale the moment those
        // conventions were fixed upstream.
        let t = crate::scene::view::transform::reflection_about_xy_line(p1, p2);
        acadrust::entities::Entity::apply_transform(entity, &t);
    });
}

impl PropertyEditable for Hatch {
    fn geometry_properties(&self, _text_style_names: &[String]) -> Vec<PropSection> {
        properties(self)
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        apply_geom_prop(self, field, value);
    }
}

impl Transformable for Hatch {
    fn apply_transform(&mut self, t: &EntityTransform) {
        apply_transform(self, t);
    }
}

// ── Grip editing ───────────────────────────────────────────────────────────

/// Assign sequential grip IDs across all boundary paths and edges.
/// Exposed control points per edge type:
///   Polyline       → each vertex (x, y)
///   Line           → start, end
///   CircularArc    → center
///   EllipticArc    → center
///   Spline         → fit points if present, else control points (x, y)
impl Grippable for Hatch {
    fn grips(&self) -> Vec<GripDef> {
        let elev = self.elevation;
        let mut out = Vec::new();
        let mut id = 0usize;
        // Grip 0 = pattern-origin handle, shown at the hatch centroid (only when a
        // pattern exists). Dragging it moves the pattern tiling origin; boundary
        // grips follow from the next id.
        if let Some(l0) = self.pattern.lines.first() {
            let (gx, gy) =
                boundary_centroid(self).unwrap_or((l0.base_point.x, l0.base_point.y));
            out.push(circle_grip(id, glam::DVec3::new(gx, gy, elev)));
            id += 1;
        }
        for path in &self.paths {
            for edge in &path.edges {
                match edge {
                    BoundaryEdge::Polyline(p) => {
                        for v in &p.vertices {
                            out.push(center_grip(id, glam::DVec3::new(v.x, v.y, elev)));
                            id += 1;
                        }
                    }
                    BoundaryEdge::Line(l) => {
                        out.push(center_grip(
                            id,
                            glam::DVec3::new(l.start.x, l.start.y, elev),
                        ));
                        id += 1;
                        out.push(center_grip(id, glam::DVec3::new(l.end.x, l.end.y, elev)));
                        id += 1;
                    }
                    BoundaryEdge::CircularArc(a) => {
                        out.push(center_grip(
                            id,
                            glam::DVec3::new(a.center.x, a.center.y, elev),
                        ));
                        id += 1;
                    }
                    BoundaryEdge::EllipticArc(e) => {
                        out.push(center_grip(
                            id,
                            glam::DVec3::new(e.center.x, e.center.y, elev),
                        ));
                        id += 1;
                    }
                    BoundaryEdge::Spline(s) => {
                        let pts: Vec<[f64; 2]> = if !s.fit_points.is_empty() {
                            s.fit_points.iter().map(|p| [p.x, p.y]).collect()
                        } else {
                            s.control_points.iter().map(|p| [p.x, p.y]).collect()
                        };
                        for [x, y] in pts {
                            out.push(center_grip(id, glam::DVec3::new(x, y, elev)));
                            id += 1;
                        }
                    }
                }
            }
        }
        out
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        let elev = self.elevation as f32;

        fn resolve(apply: &GripApply, cur: Vec3) -> (f64, f64) {
            let p = match apply {
                GripApply::Absolute(p) => *p,
                GripApply::Translate(d) => cur.as_dvec3() + *d,
            };
            (p.x, p.y)
        }

        let mut id = 0usize;
        // Grip 0 = pattern origin: shift every line's base point by the delta,
        // preserving relative offsets (mirrors the origin_x / origin_y edits).
        if let Some((ox, oy)) = self
            .pattern
            .lines
            .first()
            .map(|l| (l.base_point.x, l.base_point.y))
        {
            if grip_id == id {
                let (nx, ny) = resolve(&apply, Vec3::new(ox as f32, oy as f32, elev));
                let (dx, dy) = (nx - ox, ny - oy);
                for line in self.pattern.lines.iter_mut() {
                    line.base_point.x += dx;
                    line.base_point.y += dy;
                }
                return;
            }
            id += 1;
        }

        'outer: for path in &mut self.paths {
            for edge in &mut path.edges {
                match edge {
                    BoundaryEdge::Polyline(p) => {
                        for v in &mut p.vertices {
                            if id == grip_id {
                                let (nx, ny) =
                                    resolve(&apply, Vec3::new(v.x as f32, v.y as f32, elev));
                                v.x = nx;
                                v.y = ny;
                                break 'outer;
                            }
                            id += 1;
                        }
                    }
                    BoundaryEdge::Line(l) => {
                        if id == grip_id {
                            let (nx, ny) = resolve(
                                &apply,
                                Vec3::new(l.start.x as f32, l.start.y as f32, elev),
                            );
                            l.start.x = nx;
                            l.start.y = ny;
                            break 'outer;
                        }
                        id += 1;
                        if id == grip_id {
                            let (nx, ny) =
                                resolve(&apply, Vec3::new(l.end.x as f32, l.end.y as f32, elev));
                            l.end.x = nx;
                            l.end.y = ny;
                            break 'outer;
                        }
                        id += 1;
                    }
                    BoundaryEdge::CircularArc(a) => {
                        if id == grip_id {
                            let (nx, ny) = resolve(
                                &apply,
                                Vec3::new(a.center.x as f32, a.center.y as f32, elev),
                            );
                            a.center.x = nx;
                            a.center.y = ny;
                            break 'outer;
                        }
                        id += 1;
                    }
                    BoundaryEdge::EllipticArc(e) => {
                        if id == grip_id {
                            let (nx, ny) = resolve(
                                &apply,
                                Vec3::new(e.center.x as f32, e.center.y as f32, elev),
                            );
                            e.center.x = nx;
                            e.center.y = ny;
                            break 'outer;
                        }
                        id += 1;
                    }
                    BoundaryEdge::Spline(s) => {
                        if !s.fit_points.is_empty() {
                            for fp in &mut s.fit_points {
                                if id == grip_id {
                                    let (nx, ny) =
                                        resolve(&apply, Vec3::new(fp.x as f32, fp.y as f32, elev));
                                    fp.x = nx;
                                    fp.y = ny;
                                    break 'outer;
                                }
                                id += 1;
                            }
                        } else {
                            for cp in &mut s.control_points {
                                if id == grip_id {
                                    let (nx, ny) =
                                        resolve(&apply, Vec3::new(cp.x as f32, cp.y as f32, elev));
                                    cp.x = nx;
                                    cp.y = ny;
                                    break 'outer;
                                }
                                id += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    fn grip_menu(&self, _grip_id: usize) -> Vec<crate::scene::model::object::GripMenuItem> {
        use crate::scene::model::object::{GripMenuAction, GripMenuItem};
        vec![
            GripMenuItem {
                label: "Stretch",
                action: GripMenuAction::Stretch,
            },
            GripMenuItem {
                label: "Origin Point",
                action: GripMenuAction::OriginPoint,
            },
            GripMenuItem {
                label: "Hatch Angle",
                action: GripMenuAction::HatchAngle,
            },
            GripMenuItem {
                label: "Hatch Scale",
                action: GripMenuAction::HatchScale,
            },
        ]
    }

    fn apply_grip_menu(&mut self, _grip_id: usize, _action: crate::scene::model::object::GripMenuAction) {
        // Origin / Angle / Scale need a follow-up value — handled by
        // `apply_grip_menu_value`.
    }

    fn grip_menu_value_prompt(
        &self,
        _grip_id: usize,
        action: crate::scene::model::object::GripMenuAction,
    ) -> Option<&'static str> {
        use crate::scene::model::object::GripMenuAction as A;
        match action {
            A::HatchAngle => Some("Angle (deg)"),
            A::HatchScale => Some("Scale"),
            _ => None,
        }
    }

    fn apply_grip_menu_value(
        &mut self,
        _grip_id: usize,
        action: crate::scene::model::object::GripMenuAction,
        value: f64,
    ) {
        use crate::scene::model::object::GripMenuAction as A;
        match action {
            A::HatchAngle => {
                self.pattern_angle = value.to_radians();
            }
            A::HatchScale => {
                if value > 0.0 {
                    self.pattern_scale = value;
                }
            }
            _ => {}
        }
    }
}

impl FallbackTess for Hatch {
    fn fallback_geometry(&self) -> FallbackGeometry {
        let normal = (self.normal.x, self.normal.y, self.normal.z);
        // Convert a 2D OCS hatch boundary point to absolute WCS.
        let to_wcs = |x: f64, y: f64| -> [f64; 3] {
            let (wx, wy, wz) =
                crate::scene::view::transform::ocs_point_to_wcs((x, y, self.elevation), normal);
            [wx, wy, wz]
        };
        // Snap point at a world (f64) location, cast to the f32 snap buffer.
        let snap_at = |w: [f64; 3]| Vec3::new(w[0] as f32, w[1] as f32, w[2] as f32);
        let mut pts: Vec<[f64; 3]> = Vec::new();
        let mut key_verts: Vec<[f64; 3]> = Vec::new();
        let mut snap_pts: Vec<(Vec3, SnapHint)> = Vec::new();
        for path in &self.paths {
            for edge in &path.edges {
                match edge {
                    BoundaryEdge::Polyline(poly) => {
                        // Hatch-boundary polyline vertices encode bulge in
                        // `Vector3.z`; straight segments emit just the
                        // start vertex, bulged segments tessellate the arc
                        // between v0 → v1.
                        let verts = &poly.vertices;
                        let count = verts.len();
                        if count == 0 {
                            continue;
                        }
                        // Break the wire between this polyline and whatever
                        // preceded it — without the separator the renderer
                        // draws a ghost segment from the previous edge / path
                        // straight to this polyline's first vertex, which
                        // shows up as a stray boundary line between hatch
                        // regions.
                        if !pts.is_empty() {
                            pts.push([f64::NAN; 3]);
                        }
                        let start_idx = pts.len();
                        let seg_count = if poly.is_closed {
                            count
                        } else {
                            count.saturating_sub(1)
                        };
                        for i in 0..seg_count {
                            let v0 = &verts[i];
                            let v1 = &verts[(i + 1) % count];
                            let bulge = v0.z;
                            let arc = if bulge.abs() < 1e-9 {
                                None
                            } else {
                                crate::entities::common::BulgeArc::from_bulge(
                                    [v0.x, v0.y],
                                    [v1.x, v1.y],
                                    bulge,
                                )
                            };
                            let Some(arc) = arc else {
                                let p = to_wcs(v0.x, v0.y);
                                pts.push(p);
                                key_verts.push(p);
                                continue;
                            };
                            let segs = arc_segments(
                                arc.radius,
                                arc.sweep.abs(),
                                wire_chord_tol(arc.radius),
                            );
                            for j in 0..segs {
                                let s = arc.sample(j as f64 / segs as f64);
                                let p = to_wcs(s[0], s[1]);
                                pts.push(p);
                                if j == 0 {
                                    key_verts.push(p);
                                }
                            }
                        }
                        // Close the loop visually for closed polylines by
                        // returning to the first emitted point.
                        if poly.is_closed {
                            if let Some(first) = pts.get(start_idx).cloned() {
                                if first[0].is_finite() {
                                    pts.push(first);
                                }
                            }
                        } else if let Some(last) = verts.last() {
                            let p = to_wcs(last.x, last.y);
                            pts.push(p);
                            key_verts.push(p);
                        }
                    }
                    BoundaryEdge::Line(ln) => {
                        let p0 = to_wcs(ln.start.x, ln.start.y);
                        let p1 = to_wcs(ln.end.x, ln.end.y);
                        if !pts.is_empty() {
                            pts.push([f64::NAN; 3]);
                        }
                        pts.push(p0);
                        pts.push(p1);
                        key_verts.push(p0);
                        key_verts.push(p1);
                    }
                    BoundaryEdge::CircularArc(arc) => {
                        let (sa, span) =
                            arc_signed_span(arc.start_angle, arc.end_angle, arc.counter_clockwise);
                        let segs = arc_segments(arc.radius, span.abs(), wire_chord_tol(arc.radius));
                        if !pts.is_empty() {
                            pts.push([f64::NAN; 3]);
                        }
                        for i in 0..=segs {
                            let t = sa + span * (i as f64 / segs as f64);
                            let p = to_wcs(
                                arc.center.x + arc.radius * t.cos(),
                                arc.center.y + arc.radius * t.sin(),
                            );
                            pts.push(p);
                            if i == 0 || i == segs {
                                key_verts.push(p);
                            }
                        }
                        snap_pts.push((
                            snap_at(to_wcs(arc.center.x, arc.center.y)),
                            SnapHint::Center,
                        ));
                    }
                    BoundaryEdge::EllipticArc(ell) => {
                        let r_maj = (ell.major_axis_endpoint.x * ell.major_axis_endpoint.x
                            + ell.major_axis_endpoint.y * ell.major_axis_endpoint.y)
                            .sqrt();
                        let r_min = r_maj * ell.minor_axis_ratio;
                        let rot = ell.major_axis_endpoint.y.atan2(ell.major_axis_endpoint.x);
                        let (sa, span) =
                            arc_signed_span(ell.start_angle, ell.end_angle, ell.counter_clockwise);
                        let segs = arc_segments(r_maj, span.abs(), wire_chord_tol(r_maj));
                        if !pts.is_empty() {
                            pts.push([f64::NAN; 3]);
                        }
                        let (cr, sr) = (rot.cos(), rot.sin());
                        for i in 0..=segs {
                            let t = sa + span * (i as f64 / segs as f64);
                            let lx = r_maj * t.cos();
                            let ly = r_min * t.sin();
                            let p = to_wcs(
                                ell.center.x + lx * cr - ly * sr,
                                ell.center.y + lx * sr + ly * cr,
                            );
                            pts.push(p);
                            if i == 0 || i == segs {
                                key_verts.push(p);
                            }
                        }
                        snap_pts.push((
                            snap_at(to_wcs(ell.center.x, ell.center.y)),
                            SnapHint::Center,
                        ));
                    }
                    _ => {}
                }
            }
        }
        if pts.is_empty() {
            pts = vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
        }
        (pts, snap_pts, vec![], key_verts)
    }
}
