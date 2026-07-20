// IMAGE / IMAGEATTACH command — place a raster image in the drawing.
//
// Workflow:
//   1. File dialog opens (async, handled in update.rs).
//   2. User picks insertion point (first click).
//   3. User drags to pick width; height is computed from the image's aspect ratio.
//   4. Entity is committed.

use acadrust::entities::RasterImage;
use acadrust::types::Vector3;
use acadrust::EntityType;
use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::scene::model::wire_model::WireModel;

pub struct ImageCommand {
    file_path: String,
    pixel_width: u32,
    pixel_height: u32,
    origin: Option<DVec3>,
}

impl ImageCommand {
    pub fn new(file_path: String, pixel_width: u32, pixel_height: u32) -> Self {
        Self {
            file_path,
            pixel_width,
            pixel_height,
            origin: None,
        }
    }

    fn aspect(&self) -> f64 {
        if self.pixel_height == 0 {
            1.0
        } else {
            self.pixel_width as f64 / self.pixel_height as f64
        }
    }

    fn make_entity(&self, origin: DVec3, width_pt: DVec3) -> EntityType {
        let world_width = (width_pt.x - origin.x).abs().max(0.001);
        let world_height = world_width / self.aspect();

        let ins = Vector3::new(origin.x, origin.y, origin.z);

        let mut img = RasterImage::with_size(
            &self.file_path,
            ins,
            self.pixel_width as f64,
            self.pixel_height as f64,
            world_width,
            world_height,
        );
        img.flags = acadrust::entities::ImageDisplayFlags::SHOW_IMAGE
            | acadrust::entities::ImageDisplayFlags::USE_CLIPPING_BOUNDARY;
        EntityType::RasterImage(img)
    }
}

impl CadCommand for ImageCommand {
    fn name(&self) -> &'static str {
        "IMAGE"
    }

    fn prompt(&self) -> String {
        if self.origin.is_none() {
            format!(
                "IMAGE  Specify insertion point ({}):  ",
                short_name(&self.file_path)
            )
        } else {
            "IMAGE  Specify width (drag right):".into()
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        if let Some(origin) = self.origin {
            let entity = self.make_entity(origin, pt);
            CmdResult::CommitAndExit(entity)
        } else {
            self.origin = Some(pt);
            CmdResult::NeedPoint
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        // If origin is set, place with a default width of 1 unit * pixel count / 100
        if let Some(origin) = self.origin {
            let default_w = (self.pixel_width as f64 / 100.0).max(1.0);
            let width_pt = DVec3::new(origin.x + default_w, origin.y, origin.z);
            let entity = self.make_entity(origin, width_pt);
            CmdResult::CommitAndExit(entity)
        } else {
            CmdResult::Cancel
        }
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> { let pt = pt.as_vec3();
        let origin = self.origin?.as_vec3();
        let world_width = (pt.x - origin.x).abs().max(0.001);
        let world_height = world_width / self.aspect() as f32;

        let p0 = [origin.x, origin.y, origin.z];
        let p1 = [origin.x + world_width, origin.y, origin.z];
        let p2 = [origin.x + world_width, origin.y + world_height, origin.z];
        let p3 = [origin.x, origin.y + world_height, origin.z];

        Some(WireModel {
            fill_is_3d: false,
            pick_tris: Vec::new(),
            pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
            name: "image_preview".into(),
            points: vec![p0, p1, p2, p3, p0],
            points_low: Vec::new(),
            color: WireModel::CYAN,
            selected: false,
            pattern_length: 0.0,
            pattern: [0.0; 8],
            line_weight_px: 1.0,
            snap_pts: vec![],
            tangent_geoms: vec![],
            aci: 0,
            key_vertices: vec![],
            aabb: WireModel::UNBOUNDED_AABB,
            plinegen: true,
            fill_tris: vec![],
            fill_tris_low: Vec::new(),
        })
    }
}

fn short_name(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}
