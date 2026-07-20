use acadrust::entities::attribute_definition::{
    HorizontalAlignment as AHA, MTextFlag, VerticalAlignment as AVA,
};
use acadrust::entities::{AttributeDefinition, AttributeEntity};
use acadrust::types::Vector3;

use crate::command::EntityTransform;
use crate::entities::common::{edit_prop as edit, parse_f64, ro_prop as ro, square_grip};
use crate::entities::text_support::{
    layout_mtext, resolve_dxf_special_chars, resolve_text_style, text_local_bounds,
    MTextRenderOpts, MTextVAnchor, ResolvedTextStyle,
};
use crate::entities::traits::{Grippable, PropertyEditable, Transformable, TruckConvertible};
use crate::scene::convert::acad_to_truck::{GlyphRun, TextStroke, TruckEntity, TruckObject};
use crate::scene::model::object::{GripApply, GripDef, PropSection, PropValue, Property};
use crate::scene::model::wire_model::SnapHint;
use crate::scene::text::lff;
use crate::scene::view::transform;

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Bundle of the fields both attribute kinds carry. Lets the truck builder
/// stay generic over ATTDEF vs ATTRIB.
struct AttrTextInputs<'a> {
    value: &'a str,
    insertion_point: Vector3,
    alignment_point: Vector3,
    height: f64,
    rotation: f64,
    width_factor: f64,
    oblique_angle: f64,
    text_style: &'a str,
    text_generation_flags: i16,
    horizontal_alignment: AHA,
    vertical_alignment: AVA,
    normal: Vector3,
    mtext_flag: MTextFlag,
    is_multiline: bool,
    line_count: i16,
}

fn halign_str(a: AHA) -> &'static str {
    match a {
        AHA::Left => "Left",
        AHA::Center => "Center",
        AHA::Right => "Right",
        AHA::Aligned => "Aligned",
        AHA::Middle => "Middle",
        AHA::Fit => "Fit",
    }
}

fn valign_str(a: AVA) -> &'static str {
    match a {
        AVA::Baseline => "Baseline",
        AVA::Bottom => "Bottom",
        AVA::Middle => "Middle",
        AVA::Top => "Top",
    }
}

fn parse_halign(s: &str) -> Option<AHA> {
    Some(match s {
        "Left" => AHA::Left,
        "Center" => AHA::Center,
        "Right" => AHA::Right,
        "Aligned" => AHA::Aligned,
        "Middle" => AHA::Middle,
        "Fit" => AHA::Fit,
        _ => return None,
    })
}

fn parse_valign(s: &str) -> Option<AVA> {
    Some(match s {
        "Baseline" => AVA::Baseline,
        "Bottom" => AVA::Bottom,
        "Middle" => AVA::Middle,
        "Top" => AVA::Top,
        _ => return None,
    })
}

fn bool_yn(b: bool) -> &'static str {
    if b {
        "Yes"
    } else {
        "No"
    }
}

fn mtext_flag_str(f: MTextFlag) -> &'static str {
    match f {
        MTextFlag::SingleLine => "SingleLine",
        MTextFlag::MultiLine => "MultiLine",
        MTextFlag::ConstantMultiLine => "ConstantMultiLine",
    }
}

/// Render text strokes for an attribute, honouring alignment, oblique angle,
/// width factor, generation flags (backward / upside-down), text-style
/// resolution, and basic multiline splitting on `\n` / `\\P`.
fn build_attr_truck(input: AttrTextInputs<'_>, document: &acadrust::CadDocument) -> TruckEntity {
    let normal = (input.normal.x, input.normal.y, input.normal.z);
    let (wsx, wsy, wsz) = transform::ocs_point_to_wcs(
        (
            input.insertion_point.x,
            input.insertion_point.y,
            input.insertion_point.z,
        ),
        normal,
    );
    let snap_pt = glam::DVec3::new(wsx, wsy, wsz);

    let resolved = resolve_text_style(input.text_style, document);

    // The entity stores the FINAL width factor / oblique angle (same rule
    // as TEXT). Use it as-is; fall back to the style only when the parser
    // reports a default-omitted 0.0.
    let base_wf = if input.width_factor.abs() > 1e-9 {
        (input.width_factor as f32).clamp(0.01, 100.0)
    } else {
        resolved.width_factor.max(0.01)
    };
    // text_generation_flags bit 2 (backward) flips width-factor sign; the
    // TextStyle's own is_backward is XOR-combined so mirror-twice cancels.
    let attr_backward = (input.text_generation_flags & 2) != 0;
    let mut width_factor = base_wf;
    if attr_backward ^ resolved.is_backward {
        width_factor = -width_factor;
    }

    // Upside-down (bit 4 / TextStyle.is_upside_down) rotates by π around the
    // insertion point. Combined with rotation we get a 180° flip about the
    // anchor — same as Text.
    let attr_upside_down = (input.text_generation_flags & 4) != 0;
    let upside_down = attr_upside_down ^ resolved.is_upside_down;
    let rotation = if upside_down {
        input.rotation as f32 + std::f32::consts::PI
    } else {
        input.rotation as f32
    };
    let oblique_angle = if input.oblique_angle.abs() > 1e-9 {
        input.oblique_angle as f32
    } else {
        resolved.oblique_angle
    };

    // Anchor selection mirrors Text: only Left/Baseline uses insertion_point;
    // every other alignment uses alignment_point.
    let needs_align_pt = !(matches!(input.horizontal_alignment, AHA::Left)
        && matches!(input.vertical_alignment, AVA::Baseline));
    let anchor_f64 = if needs_align_pt {
        [input.alignment_point.x, input.alignment_point.y]
    } else {
        [input.insertion_point.x, input.insertion_point.y]
    };

    // MText-flag attributes (`mtext_flag = MultiLine | ConstantMultiLine`)
    // route through the shared MText pipeline so every inline format code
    // (`\f`, `\C`/`\c`, `\H`, `\W`, `\Q`, `\T`, `\A`, `\p…`, decorations,
    // stacked fractions, …) reaches the stroke output. SingleLine attribs
    // keep the Text-style anchor math below — they don't accept MText codes
    // in the DXF spec.
    if matches!(
        input.mtext_flag,
        MTextFlag::MultiLine | MTextFlag::ConstantMultiLine
    ) {
        let display_value = if input.value.is_empty() {
            String::new()
        } else {
            input.value.to_string()
        };
        let attach_h_anchor: f32 = match input.horizontal_alignment {
            AHA::Left => 0.0,
            AHA::Center | AHA::Middle => 0.5,
            AHA::Right | AHA::Aligned | AHA::Fit => 1.0,
        };
        let v_anchor = match input.vertical_alignment {
            AVA::Top => MTextVAnchor::Top,
            AVA::Middle => MTextVAnchor::Middle,
            AVA::Baseline | AVA::Bottom => MTextVAnchor::Bottom,
        };
        let needs_align_pt = !(matches!(input.horizontal_alignment, AHA::Left)
            && matches!(input.vertical_alignment, AVA::Baseline));
        let anchor_pt = if needs_align_pt {
            input.alignment_point
        } else {
            input.insertion_point
        };
        // Compose a ResolvedTextStyle that carries the merged width-factor
        // sign (entity backward XOR style backward) and the
        // entity-overridden oblique. is_upside_down is false because the
        // backwards / upside-down flips are already folded into `rotation`
        // and `width_factor`.
        let style_for_mtext = ResolvedTextStyle {
            font_name: resolved.font_name.clone(),
            width_factor: width_factor.abs(),
            oblique_angle,
            is_backward: width_factor < 0.0,
            is_upside_down: false,
        };
        let layout = layout_mtext(&MTextRenderOpts {
            // Not an MTEXT: text in a fixed box, never columnar.
            columns: Default::default(),
            value: &display_value,
            insertion: [anchor_pt.x, anchor_pt.y, anchor_pt.z],
            height: input.height as f32,
            rect_w: 0.0,
            rotation,
            style: &style_for_mtext,
            attach_h_anchor,
            v_anchor,
            line_spacing_factor: 1.0,
            vertical_text: false,
            want_glyph_boxes: false,
        });
        let _ = input.line_count;
        let _ = input.is_multiline;
        return TruckEntity {
            pick_tris: Vec::new(),
            object: TruckObject::Text(layout.strokes),
            snap_pts: vec![(snap_pt, SnapHint::Insertion)],
            tangent_geoms: vec![],
            key_vertices: vec![],
            fill_tris: vec![],
        };
    }

    // SingleLine path — anchor maths uses glyph bounds for accurate
    // horizontal / vertical positioning against alignment_point.
    let raw_value = input.value.to_string();
    let plain: Vec<String> = raw_value
        .replace("\\P", "\n")
        .split('\n')
        .map(|l| l.to_string())
        .collect();
    // An empty attribute value renders nothing — matching AutoCAD, where a
    // filled-in INSERT attribute left blank shows no text. The old `[value]`
    // fallback only ever fired when the value was already empty (and never
    // carried the tag), so it drew stray "[]" brackets for every blank
    // attribute. An ATTDEF preview passes its tag as the value (non-empty), so
    // it never reached this branch and is unaffected.
    let lines: Vec<String> = if plain.iter().all(|l| l.is_empty()) {
        Vec::new()
    } else {
        plain
    };

    let line_height = (input.height as f32) * 1.4; // typical CXF inter-line gap
    let (cos_r, sin_r) = (rotation.cos() as f64, rotation.sin() as f64);

    let mut strokes_all = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        // For width calculations strip MText decorations / DXF specials.
        let value_for_bounds = resolve_dxf_special_chars(line);
        let bounds = text_local_bounds(
            &resolved.font_name,
            &value_for_bounds,
            input.height as f32,
            width_factor,
            oblique_angle,
        );
        let (anchor_local_x, anchor_local_y) =
            if let Some(b) = bounds {
                // Horizontal anchor uses the pen advance box so leading /
                // trailing spaces keep their width.
                let ax = match input.horizontal_alignment {
                    AHA::Left => 0.0,
                    AHA::Center | AHA::Middle => b.advance * 0.5,
                    AHA::Right | AHA::Aligned | AHA::Fit => b.advance,
                };
                let ay = match input.vertical_alignment {
                    AVA::Baseline => 0.0,
                    AVA::Bottom => b.ink_min[1],
                    AVA::Middle => (b.ink_min[1] + b.ink_max[1]) * 0.5,
                    AVA::Top => b.ink_max[1],
                };
                (ax, ay)
            } else {
                (0.0, 0.0)
            };
        let line_offset_y = -(i as f32) * line_height;
        let local_y_for_line = anchor_local_y - line_offset_y;
        let origin: [f64; 2] = [
            anchor_f64[0] - (anchor_local_x as f64 * cos_r - local_y_for_line as f64 * sin_r),
            anchor_f64[1] - (anchor_local_x as f64 * sin_r + local_y_for_line as f64 * cos_r),
        ];
        // Parse `%%` codes through acadrust (same as TEXT), re-encoded for the
        // stroke tessellator, so attribute text shares the one parser.
        let encoded = crate::entities::text::acad_text_encode(line);
        let (strokes, fill_tris) = lff::tessellate_text_ex(
            [0.0, 0.0],
            input.height as f32,
            rotation,
            width_factor,
            oblique_angle,
            &resolved.font_name,
            &encoded,
        );
        strokes_all.push(TextStroke {
            strokes,
            origin,
            color: None,
            fill_tris,
            // Carry a GlyphRun so the attribute renders as SDF glyph quads
            // (same as TEXT); the strokes above are the historical fallback and
            // are suppressed by the per-group SDF path.
            run: Some(GlyphRun {
                text: encoded,
                font: resolved.font_name.clone(),
                height: input.height as f32,
                rotation,
                width_factor,
                oblique: oblique_angle,
                tracking: 1.0,
                bold: false,
            }),
        });
    }
    let _ = input.line_count; // round-trip only — recomputed above

    TruckEntity {
        pick_tris: Vec::new(),
        object: TruckObject::Text(strokes_all),
        snap_pts: vec![(snap_pt, SnapHint::Insertion)],
        tangent_geoms: vec![],
        key_vertices: vec![],
        fill_tris: vec![],
    }
}

// ── AttributeDefinition ───────────────────────────────────────────────────────

impl TruckConvertible for AttributeDefinition {
    fn to_truck(&self, document: &acadrust::CadDocument) -> Option<TruckEntity> {
        // An attribute definition previews its tag when it has no default
        // value, so the placeholder is visible where a block will prompt for
        // input. (Passed as a non-empty value, so it renders as-is.)
        let display_value = if self.default_value.is_empty() {
            self.tag.clone()
        } else {
            self.default_value.clone()
        };
        Some(build_attr_truck(
            AttrTextInputs {
                value: &display_value,
                insertion_point: self.insertion_point,
                alignment_point: self.alignment_point,
                height: self.height,
                rotation: self.rotation,
                width_factor: self.width_factor,
                oblique_angle: self.oblique_angle,
                text_style: &self.text_style,
                text_generation_flags: self.text_generation_flags,
                horizontal_alignment: self.horizontal_alignment,
                vertical_alignment: self.vertical_alignment,
                normal: self.normal,
                mtext_flag: self.mtext_flag,
                is_multiline: self.is_multiline,
                line_count: self.line_count,
            },
            document,
        ))
    }
}

impl Grippable for AttributeDefinition {
    fn grips(&self) -> Vec<GripDef> {
        // lock_position blocks the position grip. Show an empty grip set so
        // the renderer can't drag the entity in either AttributeFlags or
        // top-level lock_position is on.
        if self.lock_position || self.flags.locked_position {
            return vec![];
        }
        vec![square_grip(
            0,
            glam::DVec3::new(
                self.insertion_point.x,
                self.insertion_point.y,
                self.insertion_point.z,
            ),
        )]
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        if self.lock_position || self.flags.locked_position {
            return;
        }
        if grip_id == 0 {
            match apply {
                GripApply::Translate(d) => {
                    self.insertion_point.x += d.x as f64;
                    self.insertion_point.y += d.y as f64;
                    self.insertion_point.z += d.z as f64;
                    self.alignment_point.x += d.x as f64;
                    self.alignment_point.y += d.y as f64;
                    self.alignment_point.z += d.z as f64;
                }
                GripApply::Absolute(p) => {
                    self.insertion_point.x = p.x as f64;
                    self.insertion_point.y = p.y as f64;
                    self.insertion_point.z = p.z as f64;
                    self.alignment_point.x = p.x as f64;
                    self.alignment_point.y = p.y as f64;
                    self.alignment_point.z = p.z as f64;
                }
            }
        }
    }
}

impl PropertyEditable for AttributeDefinition {
    fn geometry_properties(&self, text_style_names: &[String]) -> Vec<PropSection> {
        let mut text_props = vec![
            Property {
                label: "Tag".into(),
                field: "att_tag",
                value: PropValue::EditText(self.tag.clone()),
            },
            Property {
                label: "Prompt".into(),
                field: "att_prompt",
                value: PropValue::EditText(self.prompt.clone()),
            },
            Property {
                label: "Value".into(),
                field: "att_default",
                value: PropValue::EditText(self.default_value.clone()),
            },
            Property {
                label: "Style".into(),
                field: "att_style",
                value: PropValue::Choice {
                    selected: if self.text_style.trim().is_empty() {
                        "Standard".into()
                    } else {
                        self.text_style.clone()
                    },
                    options: text_style_names.to_vec(),
                },
            },
            ro(
                "Annotative",
                "att_annotative",
                bool_yn(self.flags.annotative),
            ),
            Property {
                label: "Justify".into(),
                field: "att_halign",
                value: PropValue::Choice {
                    selected: halign_str(self.horizontal_alignment).to_string(),
                    options: ["Left", "Center", "Right", "Aligned", "Middle", "Fit"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                },
            },
            Property {
                label: "V-Align".into(),
                field: "att_valign",
                value: PropValue::Choice {
                    selected: valign_str(self.vertical_alignment).to_string(),
                    options: ["Baseline", "Bottom", "Middle", "Top"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                },
            },
            edit("Height", "att_h", self.height),
            edit("Rotation", "att_rot", self.rotation.to_degrees()),
            edit("Width factor", "att_wf", self.width_factor),
            edit("Obliquing", "att_ob", self.oblique_angle.to_degrees()),
            edit("Text alignment X", "att_ax", self.alignment_point.x),
            edit("Text alignment Y", "att_ay", self.alignment_point.y),
            edit("Text alignment Z", "att_az", self.alignment_point.z),
            ro(
                "Boundary width",
                "att_field_len",
                self.field_length.to_string(),
            ),
            ro(
                "Upside down",
                "att_upside_down",
                bool_yn(self.text_generation_flags & 0x4 != 0),
            ),
            ro(
                "Backward",
                "att_backward",
                bool_yn(self.text_generation_flags & 0x2 != 0),
            ),
        ];
        // Constant attributes can't be edited at insert time — surface that
        // by marking the Value field read-only.
        if self.flags.constant {
            if let Some(p) = text_props.iter_mut().find(|p| p.field == "att_default") {
                p.value = PropValue::ReadOnly(self.default_value.clone());
            }
        }
        vec![
            PropSection {
                title: "Text".into(),
                props: text_props,
            },
            PropSection {
                title: "Geometry".into(),
                props: vec![
                    edit("Position X", "att_ix", self.insertion_point.x),
                    edit("Position Y", "att_iy", self.insertion_point.y),
                    edit("Position Z", "att_iz", self.insertion_point.z),
                ],
            },
            PropSection {
                title: "Misc".into(),
                props: vec![
                    ro("Invisible", "att_invisible", bool_yn(self.flags.invisible)),
                    ro("Constant", "att_constant", bool_yn(self.flags.constant)),
                    ro("Verify", "att_verify", bool_yn(self.flags.verify)),
                    ro("Preset", "att_preset", bool_yn(self.flags.preset)),
                    ro("Lock position", "att_lock_pos", bool_yn(self.lock_position)),
                    Property {
                        label: "Multiple lines".into(),
                        field: "att_mtext_flag",
                        value: PropValue::Choice {
                            selected: mtext_flag_str(self.mtext_flag).to_string(),
                            options: ["SingleLine", "MultiLine", "ConstantMultiLine"]
                                .into_iter()
                                .map(str::to_string)
                                .collect(),
                        },
                    },
                ],
            },
        ]
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        // String / enum fields first.
        if field == "att_tag" {
            self.tag = value.to_string();
            return;
        }
        if field == "att_prompt" {
            self.prompt = value.to_string();
            return;
        }
        if field == "att_default" {
            if !self.flags.constant {
                self.default_value = value.to_string();
            }
            return;
        }
        if field == "att_style" {
            self.text_style = value.to_string();
            return;
        }
        if field == "att_halign" {
            if let Some(a) = parse_halign(value) {
                self.horizontal_alignment = a;
            }
            return;
        }
        if field == "att_valign" {
            if let Some(a) = parse_valign(value) {
                self.vertical_alignment = a;
            }
            return;
        }
        if field == "att_mtext_flag" {
            self.mtext_flag = match value {
                "MultiLine" => MTextFlag::MultiLine,
                "ConstantMultiLine" => MTextFlag::ConstantMultiLine,
                _ => MTextFlag::SingleLine,
            };
            return;
        }
        // Numeric scalars.
        let Some(v) = parse_f64(value) else {
            return;
        };
        match field {
            "att_ix" => self.insertion_point.x = v,
            "att_iy" => self.insertion_point.y = v,
            "att_iz" => self.insertion_point.z = v,
            "att_ax" => self.alignment_point.x = v,
            "att_ay" => self.alignment_point.y = v,
            "att_az" => self.alignment_point.z = v,
            "att_h" if v > 0.0 => self.height = v,
            "att_rot" => self.rotation = v.to_radians(),
            "att_wf" if v.abs() > 1e-9 => self.width_factor = v,
            "att_ob" => self.oblique_angle = v.to_radians(),
            _ => {}
        }
    }
}

impl Transformable for AttributeDefinition {
    fn apply_transform(&mut self, t: &EntityTransform) {
        transform::apply_standard_entity_transform(self, t, |entity, p1, p2| {
            transform::reflect_xy_point(
                &mut entity.insertion_point.x,
                &mut entity.insertion_point.y,
                p1,
                p2,
            );
            transform::reflect_xy_point(
                &mut entity.alignment_point.x,
                &mut entity.alignment_point.y,
                p1,
                p2,
            );
        });
    }
}

// ── AttributeEntity ───────────────────────────────────────────────────────────

impl TruckConvertible for AttributeEntity {
    fn to_truck(&self, document: &acadrust::CadDocument) -> Option<TruckEntity> {
        Some(build_attr_truck(
            AttrTextInputs {
                value: &self.value,
                insertion_point: self.insertion_point,
                alignment_point: self.alignment_point,
                height: self.height,
                rotation: self.rotation,
                width_factor: self.width_factor,
                oblique_angle: self.oblique_angle,
                text_style: &self.text_style,
                text_generation_flags: self.text_generation_flags,
                horizontal_alignment: self.horizontal_alignment,
                vertical_alignment: self.vertical_alignment,
                normal: self.normal,
                mtext_flag: self.mtext_flag,
                is_multiline: self.is_multiline,
                line_count: self.line_count,
            },
            document,
        ))
    }
}

impl Grippable for AttributeEntity {
    fn grips(&self) -> Vec<GripDef> {
        if self.lock_position || self.flags.locked_position {
            return vec![];
        }
        vec![square_grip(
            0,
            glam::DVec3::new(
                self.insertion_point.x,
                self.insertion_point.y,
                self.insertion_point.z,
            ),
        )]
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        if self.lock_position || self.flags.locked_position {
            return;
        }
        if grip_id == 0 {
            match apply {
                GripApply::Translate(d) => {
                    self.insertion_point.x += d.x as f64;
                    self.insertion_point.y += d.y as f64;
                    self.insertion_point.z += d.z as f64;
                    self.alignment_point.x += d.x as f64;
                    self.alignment_point.y += d.y as f64;
                    self.alignment_point.z += d.z as f64;
                }
                GripApply::Absolute(p) => {
                    self.insertion_point.x = p.x as f64;
                    self.insertion_point.y = p.y as f64;
                    self.insertion_point.z = p.z as f64;
                    self.alignment_point.x = p.x as f64;
                    self.alignment_point.y = p.y as f64;
                    self.alignment_point.z = p.z as f64;
                }
            }
        }
    }
}

impl PropertyEditable for AttributeEntity {
    fn geometry_properties(&self, text_style_names: &[String]) -> Vec<PropSection> {
        vec![
            PropSection {
                title: "Text".into(),
                props: vec![
                    Property {
                        label: "Tag".into(),
                        field: "atte_tag",
                        value: PropValue::EditText(self.tag.clone()),
                    },
                    ro("Prompt", "atte_prompt", String::new()),
                    Property {
                        label: "Value".into(),
                        field: "atte_val",
                        value: PropValue::EditText(self.value.clone()),
                    },
                    Property {
                        label: "Style".into(),
                        field: "atte_style",
                        value: PropValue::Choice {
                            selected: if self.text_style.trim().is_empty() {
                                "Standard".into()
                            } else {
                                self.text_style.clone()
                            },
                            options: text_style_names.to_vec(),
                        },
                    },
                    Property {
                        label: "Justify".into(),
                        field: "atte_halign",
                        value: PropValue::Choice {
                            selected: halign_str(self.horizontal_alignment).to_string(),
                            options: ["Left", "Center", "Right", "Aligned", "Middle", "Fit"]
                                .into_iter()
                                .map(str::to_string)
                                .collect(),
                        },
                    },
                    Property {
                        label: "V-Align".into(),
                        field: "atte_valign",
                        value: PropValue::Choice {
                            selected: valign_str(self.vertical_alignment).to_string(),
                            options: ["Baseline", "Bottom", "Middle", "Top"]
                                .into_iter()
                                .map(str::to_string)
                                .collect(),
                        },
                    },
                    ro(
                        "Annotative",
                        "atte_annotative",
                        bool_yn(self.flags.annotative),
                    ),
                    edit("Height", "atte_h", self.height),
                    edit("Rotation", "atte_rot", self.rotation.to_degrees()),
                    edit("Width factor", "atte_wf", self.width_factor),
                    edit("Obliquing", "atte_ob", self.oblique_angle.to_degrees()),
                    edit("Text alignment X", "atte_ax", self.alignment_point.x),
                    edit("Text alignment Y", "atte_ay", self.alignment_point.y),
                    edit("Text alignment Z", "atte_az", self.alignment_point.z),
                    ro(
                        "Boundary width",
                        "atte_field_len",
                        self.field_length.to_string(),
                    ),
                ],
            },
            PropSection {
                title: "Geometry".into(),
                props: vec![
                    edit("Position X", "atte_ix", self.insertion_point.x),
                    edit("Position Y", "atte_iy", self.insertion_point.y),
                    edit("Position Z", "atte_iz", self.insertion_point.z),
                ],
            },
            PropSection {
                title: "Misc".into(),
                props: vec![
                    ro(
                        "Upside down",
                        "atte_upside_down",
                        bool_yn(self.text_generation_flags & 0x4 != 0),
                    ),
                    ro(
                        "Backward",
                        "atte_backward",
                        bool_yn(self.text_generation_flags & 0x2 != 0),
                    ),
                    ro("Invisible", "atte_invisible", bool_yn(self.flags.invisible)),
                    ro(
                        "Multiple lines",
                        "atte_mtext_flag",
                        mtext_flag_str(self.mtext_flag),
                    ),
                    ro("Constant", "atte_constant", bool_yn(self.flags.constant)),
                    ro("Verify", "atte_verify", bool_yn(self.flags.verify)),
                    ro("Preset", "atte_preset", bool_yn(self.flags.preset)),
                    ro(
                        "Lock position",
                        "atte_lock_pos",
                        bool_yn(self.lock_position),
                    ),
                ],
            },
        ]
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        if field == "atte_tag" {
            self.tag = value.to_string();
            return;
        }
        if field == "atte_val" {
            if !self.flags.constant {
                self.value = value.to_string();
            }
            return;
        }
        if field == "atte_style" {
            self.text_style = value.to_string();
            return;
        }
        if field == "atte_halign" {
            if let Some(a) = parse_halign(value) {
                self.horizontal_alignment = a;
            }
            return;
        }
        if field == "atte_valign" {
            if let Some(a) = parse_valign(value) {
                self.vertical_alignment = a;
            }
            return;
        }
        let Some(v) = parse_f64(value) else {
            return;
        };
        match field {
            "atte_ix" => self.insertion_point.x = v,
            "atte_iy" => self.insertion_point.y = v,
            "atte_iz" => self.insertion_point.z = v,
            "atte_ax" => self.alignment_point.x = v,
            "atte_ay" => self.alignment_point.y = v,
            "atte_az" => self.alignment_point.z = v,
            "atte_h" if v > 0.0 => self.height = v,
            "atte_rot" => self.rotation = v.to_radians(),
            "atte_wf" if v.abs() > 1e-9 => self.width_factor = v,
            "atte_ob" => self.oblique_angle = v.to_radians(),
            _ => {}
        }
    }
}

impl Transformable for AttributeEntity {
    fn apply_transform(&mut self, t: &EntityTransform) {
        transform::apply_standard_entity_transform(self, t, |entity, p1, p2| {
            transform::reflect_xy_point(
                &mut entity.insertion_point.x,
                &mut entity.insertion_point.y,
                p1,
                p2,
            );
            transform::reflect_xy_point(
                &mut entity.alignment_point.x,
                &mut entity.alignment_point.y,
                p1,
                p2,
            );
        });
    }
}

impl crate::entities::traits::TextContent for acadrust::entities::AttributeDefinition {
    fn text_content(&self) -> Option<String> {
        Some(self.default_value.clone())
    }
    fn replace_text(&mut self, search: &str, rep: &str) {
        let search_lc = search.to_lowercase();
        if self.default_value.to_lowercase().contains(&search_lc) {
            self.default_value = self.default_value.replace(search, rep);
        }
    }
}

impl crate::entities::traits::TextContent for acadrust::entities::AttributeEntity {
    fn text_content(&self) -> Option<String> {
        Some(self.get_value().to_string())
    }
    fn replace_text(&mut self, search: &str, rep: &str) {
        let search_lc = search.to_lowercase();
        let cur = self.get_value().to_string();
        if cur.to_lowercase().contains(&search_lc) {
            self.set_value(cur.replace(search, rep));
        }
    }
}
