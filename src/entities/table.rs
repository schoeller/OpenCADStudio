use acadrust::entities::Table;
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{ro_prop as ro, square_grip};
use crate::entities::traits::{Grippable, PropertyEditable, Transformable, TruckConvertible};
use crate::scene::acad_to_truck::{TruckEntity, TruckObject};
use crate::scene::object::{GripApply, GripDef, PropSection};
use crate::scene::wire_model::SnapHint;
use crate::scene::{cxf, transform};

fn v3(v: &acadrust::types::Vector3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

impl TruckConvertible for Table {
    fn to_truck(&self, _document: &acadrust::CadDocument) -> Option<TruckEntity> {
        if self.rows.is_empty() || self.columns.is_empty() {
            return None;
        }

        let origin = v3(&self.insertion_point);
        let h_raw = v3(&self.horizontal_direction);
        let h = if h_raw.length_squared() > 1e-10 {
            h_raw.normalize()
        } else {
            Vec3::X
        };
        // Perpendicular "down" direction in the drawing plane (tables grow downward)
        let v_down = Vec3::new(h.y, -h.x, 0.0);

        let col_offsets: Vec<f32> = {
            let mut off = 0.0f32;
            let mut v = vec![0.0f32];
            for col in &self.columns {
                off += col.width as f32;
                v.push(off);
            }
            v
        };
        let total_w = *col_offsets.last().unwrap_or(&0.0);

        let row_offsets: Vec<f32> = {
            let mut off = 0.0f32;
            let mut v = vec![0.0f32];
            for row in &self.rows {
                off += row.height as f32;
                v.push(off);
            }
            v
        };
        let total_h = *row_offsets.last().unwrap_or(&0.0);

        let mut pts: Vec<[f32; 3]> = Vec::new();

        // Per-cell borders. When a cell carries a CellStyle, honour the
        // visibility / `invisible` flag of each of its four borders so
        // hidden borders disappear from the grid. Cells with no style still
        // emit the standard four borders. To avoid drawing each shared edge
        // twice we coalesce the segments by their (start, end) coordinates.
        use std::collections::HashSet;
        let mut emitted: HashSet<(i32, i32, i32, i32)> = HashSet::new();
        let try_add = |a: Vec3, b: Vec3, vis: bool, emitted: &mut HashSet<(i32, i32, i32, i32)>, pts: &mut Vec<[f32; 3]>| {
            if !vis {
                return;
            }
            let key = (
                (a.x * 1_000.0) as i32,
                (a.y * 1_000.0) as i32,
                (b.x * 1_000.0) as i32,
                (b.y * 1_000.0) as i32,
            );
            let key_rev = (key.2, key.3, key.0, key.1);
            if emitted.contains(&key) || emitted.contains(&key_rev) {
                return;
            }
            emitted.insert(key);
            if !pts.is_empty() {
                pts.push([f32::NAN; 3]);
            }
            pts.push([a.x, a.y, a.z]);
            pts.push([b.x, b.y, b.z]);
        };
        for (ri, row) in self.rows.iter().enumerate() {
            let row_top = row_offsets[ri];
            let row_bot = row_offsets
                .get(ri + 1)
                .copied()
                .unwrap_or(row_top + row.height as f32);
            for (ci, cell) in row.cells.iter().enumerate() {
                let col_left = col_offsets[ci];
                let col_right = col_offsets
                    .get(ci + 1)
                    .copied()
                    .unwrap_or(col_left
                        + self.columns.get(ci).map(|c| c.width as f32).unwrap_or(1.0));
                // Default to visible when no style override is present.
                let (top_vis, right_vis, bottom_vis, left_vis) = cell
                    .style
                    .as_ref()
                    .map(|s| {
                        (
                            !s.top_border.invisible,
                            !s.right_border.invisible,
                            !s.bottom_border.invisible,
                            !s.left_border.invisible,
                        )
                    })
                    .unwrap_or((true, true, true, true));
                let tl = origin + h * col_left + v_down * row_top;
                let tr = origin + h * col_right + v_down * row_top;
                let br_ = origin + h * col_right + v_down * row_bot;
                let bl = origin + h * col_left + v_down * row_bot;
                try_add(tl, tr, top_vis, &mut emitted, &mut pts);
                try_add(tr, br_, right_vis, &mut emitted, &mut pts);
                try_add(bl, br_, bottom_vis, &mut emitted, &mut pts);
                try_add(tl, bl, left_vis, &mut emitted, &mut pts);
            }
        }
        // Suppress unused-variable warnings now that the simple grid-pass
        // is gone — col/row offsets still feed cell drawing below.
        let _ = (total_w, total_h);

        // Cell text — lifted into Lines points via 2D strokes
        let text_height = 0.18_f32;
        let margin = text_height * 0.5_f32;

        for (ri, row) in self.rows.iter().enumerate() {
            let row_top = row_offsets[ri];
            let row_bot = row_offsets
                .get(ri + 1)
                .copied()
                .unwrap_or(row_top + row.height as f32);
            let row_mid = (row_top + row_bot) * 0.5;

            for (ci, cell) in row.cells.iter().enumerate() {
                let text = cell.text_value();
                if text.is_empty() {
                    continue;
                }

                let col_left = col_offsets[ci];
                let col_width = self.columns.get(ci).map(|c| c.width as f32).unwrap_or(1.0);
                let col_right = col_left + col_width;

                // Alignment: CellStyle.alignment i32 encodes 1-9 (AutoCAD convention):
                // 1=TopLeft 2=TopCenter 3=TopRight
                // 4=MiddleLeft 5=MiddleCenter 6=MiddleRight
                // 7=BottomLeft 8=BottomCenter 9=BottomRight
                // 0/default = MiddleCenter (5)
                let align = cell.style.as_ref().map_or(5, |s| s.alignment);
                let horiz = ((align - 1).rem_euclid(3)) + 1; // 1=left, 2=center, 3=right
                let vert = ((align - 1) / 3) + 1; // 1=top, 2=middle, 3=bottom

                let text_w = cxf::measure_text(text, text_height, 1.0, "txt");

                let x_offset = match horiz {
                    1 => col_left + margin,                     // left
                    3 => col_right - margin - text_w,           // right
                    _ => col_left + (col_width - text_w) * 0.5, // center (default)
                };
                let y_offset = match vert {
                    1 => row_top + margin,               // top
                    3 => row_bot - margin - text_height, // bottom
                    _ => row_mid - text_height * 0.5,    // middle (default)
                };

                let text_origin = origin + h * x_offset + v_down * y_offset;

                let strokes = cxf::tessellate_text_ex(
                    [text_origin.x, text_origin.y],
                    text_height,
                    0.0,
                    1.0,
                    0.0,
                    "txt",
                    text,
                );
                for stroke in strokes {
                    if !pts.is_empty() {
                        pts.push([f32::NAN; 3]);
                    }
                    for [x, y] in stroke {
                        pts.push([x, y, origin.z]);
                    }
                }
            }
        }

        // Table currently does its layout in glam::Vec3 (f32). The world_offset
        // subtraction in tessellate.rs needs f64, so widen at the boundary —
        // precision is already limited by the f32 math above (separate fix-up).
        let pts_f64: Vec<[f64; 3]> = pts
            .into_iter()
            .map(|[x, y, z]| {
                if x.is_nan() {
                    [f64::NAN, f64::NAN, f64::NAN]
                } else {
                    [x as f64, y as f64, z as f64]
                }
            })
            .collect();
        Some(TruckEntity {
            object: TruckObject::Lines(pts_f64),
            snap_pts: vec![(origin, SnapHint::Insertion)],
            tangent_geoms: vec![],
            key_vertices: vec![],
            fill_tris: vec![],
        })
    }
}

impl Grippable for Table {
    fn grips(&self) -> Vec<GripDef> {
        vec![square_grip(0, v3(&self.insertion_point))]
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        if grip_id == 0 {
            match apply {
                GripApply::Translate(d) => {
                    self.insertion_point.x += d.x as f64;
                    self.insertion_point.y += d.y as f64;
                    self.insertion_point.z += d.z as f64;
                }
                GripApply::Absolute(p) => {
                    self.insertion_point.x = p.x as f64;
                    self.insertion_point.y = p.y as f64;
                    self.insertion_point.z = p.z as f64;
                }
            }
        }
    }
}

impl PropertyEditable for Table {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        let fmt_h = |oh: &Option<acadrust::types::Handle>| -> String {
            match oh {
                Some(h) if !h.is_null() => format!("{:X}", h.value()),
                _ => "(none)".to_string(),
            }
        };
        PropSection {
            title: "Table".into(),
            props: vec![
                ro("Rows", "tbl_rows", self.rows.len().to_string()),
                ro("Columns", "tbl_cols", self.columns.len().to_string()),
                ro(
                    "Insert X",
                    "tbl_ix",
                    format!("{:.4}", self.insertion_point.x),
                ),
                ro(
                    "Insert Y",
                    "tbl_iy",
                    format!("{:.4}", self.insertion_point.y),
                ),
                ro(
                    "Insert Z",
                    "tbl_iz",
                    format!("{:.4}", self.insertion_point.z),
                ),
                ro(
                    "Table Style",
                    "tbl_style_handle",
                    fmt_h(&self.table_style_handle),
                ),
                ro(
                    "Block Record",
                    "tbl_block_rec_handle",
                    fmt_h(&self.block_record_handle),
                ),
                ro("Data Version", "tbl_data_version", self.data_version.to_string()),
                ro(
                    "Value Flags",
                    "tbl_value_flags",
                    format!("{:#010x}", self.value_flags),
                ),
                ro(
                    "Override Flag",
                    "tbl_override_flag",
                    if self.override_flag { "Yes" } else { "No" },
                ),
                ro(
                    "Override Border Color",
                    "tbl_override_border_color",
                    if self.override_border_color { "Yes" } else { "No" },
                ),
                ro(
                    "Override Border LW",
                    "tbl_override_border_lw",
                    if self.override_border_line_weight {
                        "Yes"
                    } else {
                        "No"
                    },
                ),
                ro(
                    "Override Border Vis",
                    "tbl_override_border_vis",
                    if self.override_border_visibility {
                        "Yes"
                    } else {
                        "No"
                    },
                ),
                ro(
                    "Break Spacing",
                    "tbl_break_spacing",
                    format!("{:.4}", self.break_spacing),
                ),
                ro(
                    "Break Flow",
                    "tbl_break_flow",
                    format!("{:?}", self.break_flow_direction),
                ),
                ro(
                    "Break Options",
                    "tbl_break_options",
                    format!("{:#018b}", self.break_options.bits()),
                ),
                ro(
                    "Normal",
                    "tbl_normal",
                    format!(
                        "{:.3}, {:.3}, {:.3}",
                        self.normal.x, self.normal.y, self.normal.z
                    ),
                ),
            ],
        }
    }

    fn apply_geom_prop(&mut self, _field: &str, _value: &str) {}
}

impl Transformable for Table {
    fn apply_transform(&mut self, t: &EntityTransform) {
        transform::apply_standard_entity_transform(self, t, |entity, p1, p2| {
            transform::reflect_xy_point(
                &mut entity.insertion_point.x,
                &mut entity.insertion_point.y,
                p1,
                p2,
            );
            // Reflect the horizontal direction by reflecting a tip point
            let mut tip_x = entity.insertion_point.x + entity.horizontal_direction.x;
            let mut tip_y = entity.insertion_point.y + entity.horizontal_direction.y;
            transform::reflect_xy_point(&mut tip_x, &mut tip_y, p1, p2);
            entity.horizontal_direction.x = tip_x - entity.insertion_point.x;
            entity.horizontal_direction.y = tip_y - entity.insertion_point.y;
        });
    }
}
