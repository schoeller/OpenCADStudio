// Auto-split from scene/mod.rs. Pure text-move; behaviour unchanged.
use super::*;

impl Scene {
    pub fn grid_views(&self, vw: f32, vh: f32) -> Vec<(iced::Rectangle, Camera, Handle)> {
        self.active_viewports(vw, vh, acadrust::entities::ViewportRenderMode::Wireframe2D)
            .into_iter()
            .filter(|inst| inst.grid_on)
            .map(|inst| (inst.screen_rect, inst.camera, inst.handle))
            .collect()
    }
    /// The viewports to render this frame, one entry per scissor pass.
    ///
    /// - **Model layout**: a single full-canvas instance driven by the
    ///   scene camera (tiled splits will append more later). `model_mode`
    ///   supplies its render mode (held on the tab, not the scene).
    /// - **Paper layout**: one instance per content viewport entity
    ///   (`id > 1`, owned by the current layout block, switched on),
    ///   using each viewport's own camera and render mode.
    pub fn active_viewports(
        &self,
        canvas_w: f32,
        canvas_h: f32,
        model_mode: acadrust::entities::ViewportRenderMode,
    ) -> Vec<ViewportInstance> {
        if self.current_layout == "Model" {
            let tiles = self.model_tiles.borrow();
            let active = self.active_model_tile.get().min(tiles.len().saturating_sub(1));
            return tiles
                .iter()
                .enumerate()
                .map(|(i, tile)| {
                    // The active tile renders the live camera (orbit/pan act
                    // on it); inactive tiles use their stored snapshot.
                    let camera = if i == active {
                        self.camera.borrow().clone()
                    } else {
                        tile.camera.clone()
                    };
                    ViewportInstance {
                        handle: Handle::NULL,
                        tile_idx: Some(i),
                        screen_rect: iced::Rectangle {
                            x: tile.rect.x * canvas_w,
                            y: tile.rect.y * canvas_h,
                            width: tile.rect.width * canvas_w,
                            height: tile.rect.height * canvas_h,
                        },
                        camera,
                        // The active tile shows the live mode the picker
                        // drives; every other tile keeps its own stored
                        // style so editing one never disturbs the rest.
                        render_mode: if i == active { model_mode } else { tile.render_mode },
                        active: i == active,
                        grid_on: tile.grid_on,
                        paper_sheet: false,
                    }
                })
                .collect();
        }
        let layout_block = self.current_layout_block_handle();
        let mut out: Vec<ViewportInstance> = Vec::new();
        // The full-canvas sheet viewport renders the paper-space entities
        // themselves — the layout's own view, drawn first so the floating
        // content viewports overlay it. Its camera keeps the paper pan/zoom
        // (target + ortho size) but is LOCKED to the top/plan orientation:
        // paper is 2-D, so the sheet never orbits.
        let mut sheet_cam = self.camera.borrow().clone();
        sheet_cam.yaw = 0.0;
        sheet_cam.pitch = std::f32::consts::FRAC_PI_2;
        sheet_cam.rotation = view::camera::yaw_pitch_to_quat(0.0, std::f32::consts::FRAC_PI_2, 0.0);
        sheet_cam.projection = view::camera::Projection::Orthographic;
        let sheet_grid_on = match self
            .document
            .get_entity(self.current_layout_sheet_viewport_handle())
        {
            Some(EntityType::Viewport(vp)) => vp.status.grid_on,
            _ => false,
        };
        out.push(ViewportInstance {
            handle: Handle::NULL,
            tile_idx: None,
            screen_rect: iced::Rectangle {
                x: 0.0,
                y: 0.0,
                width: canvas_w,
                height: canvas_h,
            },
            camera: sheet_cam,
            render_mode: acadrust::entities::ViewportRenderMode::Wireframe2D,
            active: false,
            grid_on: sheet_grid_on,
            paper_sheet: true,
        });
        for e in self.document.entities() {
            let EntityType::Viewport(vp) = e else {
                continue;
            };
            if !self.is_content_viewport_in_layout(vp, layout_block)
                || !vp.status.is_on
            {
                continue;
            }
            let h = vp.common.handle;
            let (Some(screen_rect), Some(camera)) = (
                self.viewport_screen_rect(h, (canvas_w, canvas_h)),
                self.camera_for_viewport(h),
            ) else {
                continue;
            };
            out.push(ViewportInstance {
                handle: h,
                tile_idx: None,
                screen_rect,
                camera,
                render_mode: vp.render_mode,
                active: self.active_viewport == Some(h),
                grid_on: vp.status.grid_on,
                paper_sheet: false,
            });
        }
        out
    }

    /// Convert a paper-space Viewport entity's position/size into a pixel
    /// `Rectangle` relative to the top-left of the canvas.
    ///
    /// Uses the same top-down ortho transform as the GPU sheet viewport so the
    /// overlay lands exactly over the drawn viewport border regardless of zoom
    /// or pan level.
    pub fn viewport_screen_rect(
        &self,
        vp_handle: Handle,
        canvas_px: (f32, f32),
    ) -> Option<iced::Rectangle> {
        let vp = match self.document.get_entity(vp_handle) {
            Some(EntityType::Viewport(vp)) => vp,
            _ => return None,
        };

        let (canvas_w, canvas_h) = canvas_px;
        if canvas_w < 1.0 || canvas_h < 1.0 {
            return None;
        }

        let cam = self.camera.borrow();
        let aspect = canvas_w / canvas_h;
        let half_h = cam.ortho_size();
        let half_w = half_h * aspect;
        let tx = cam.target.x as f32;
        let ty = cam.target.y as f32;
        drop(cam);

        // Top-down ortho mapping matching the GPU sheet viewport's camera.
        let to_px = |wx: f32, wy: f32| -> (f32, f32) {
            let x = (wx - tx + half_w) / (2.0 * half_w) * canvas_w;
            let y = (ty + half_h - wy) / (2.0 * half_h) * canvas_h;
            (x, y)
        };

        let cx = vp.center.x as f32;
        let cy = vp.center.y as f32;
        let hw = (vp.width / 2.0) as f32;
        let hh = (vp.height / 2.0) as f32;

        let (x0, y0) = to_px(cx - hw, cy + hh); // top-left in screen
        let (x1, y1) = to_px(cx + hw, cy - hh); // bottom-right in screen

        let w = (x1 - x0).max(1.0);
        let h = (y1 - y0).max(1.0);

        Some(iced::Rectangle {
            x: x0,
            y: y0,
            width: w,
            height: h,
        })
    }
    // ── Paper-space helpers ───────────────────────────────────────────────

    /// Paper-layout hatch fills, restricted to the active layout block (used by
    /// paper-space hatch hit-testing / export). The GPU-rendered
    /// content viewports already draw model-block hatches inside their
    /// own scissor; including those here would also draw them on the
    /// paper sheet through the paper camera (huge / off-position), so
    /// restrict the canvas list to entities owned by the active paper
    /// layout block. Iterates the source `self.hatches` map (keyed by
    /// entity handle) rather than the already-flattened arc — the
    /// flattened arc carries pattern names, not handles, so filtering
    /// there is unreliable.
    pub fn paper_canvas_hatches(&self) -> Arc<Vec<HatchModel>> {
        let layout_block = self.current_layout_block_handle();
        let layer_hidden = |layer: &str| {
            self.document
                .layers
                .get(layer)
                .map(|l| l.flags.off || l.flags.frozen)
                .unwrap_or(false)
        };
        let mut models: Vec<HatchModel> = Vec::new();
        for (&handle, model) in self.hatches.iter() {
            let Some(entity) = self.document.get_entity(handle) else {
                continue;
            };
            let c = entity.common();
            if c.invisible || layer_hidden(&c.layer) {
                continue;
            }
            if !self.belongs_to_visible_block(handle, c.owner_handle, layout_block) {
                continue;
            }
            let mut m = model.clone();
            m.color = self.render_style(entity).0;
            if let EntityType::Hatch(dxf) = entity {
                // Only re-apply pattern_scale/angle for catalog-derived patterns
                // (empty stored lines). A pattern built from the hatch's own
                // stored lines is already final (scale 1 / angle 0).
                if let model::hatch_model::HatchPattern::Pattern(_) = &m.pattern {
                    if dxf.pattern.lines.is_empty() {
                        m.angle_offset = dxf.pattern_angle as f32;
                        m.scale = dxf.pattern_scale as f32;
                    }
                }
            }
            if self.selected.contains(&handle) {
                m.color = [0.15, 0.55, 1.00, m.color[3]];
            }
            models.push(m);
        }
        // Hatch fills nested inside a block INSERT are owned by the block
        // record, so the loop above — which only keeps hatches owned by
        // `layout_block` — never sees them. Explode the layout's visible
        // INSERTs and materialize their fills at world position, exactly as the
        // viewport does, so the export carries the block's colours instead of
        // bare outlines. (No selection tint on export.)
        let hatch_bg = if self.current_layout != "Model" {
            self.paper_bg_color
        } else {
            self.bg_color
        };
        let exploded = self.exploded_insert_hatch_models(layout_block, hatch_bg, false);
        models.extend(exploded);
        Arc::new(models)
    }

    /// Paper-layout wipeout fills (paper hit-testing / export). Same rationale as
    /// `paper_canvas_hatches` — only include wipeouts owned by the
    /// active paper layout block, so model wipeouts (drawn through their
    /// content viewport's GPU pipeline) don't get a second mis-projected
    /// copy on the paper sheet.
    pub fn paper_canvas_wipeouts(&self) -> Arc<Vec<HatchModel>> {
        let layout_block = self.current_layout_block_handle();
        let bg_color = self.paper_bg_color;
        let mut models = Vec::new();
        for entity in self.document.entities() {
            let EntityType::Wipeout(wo) = entity else {
                continue;
            };
            if wo.common.invisible {
                continue;
            }
            if self
                .document
                .layers
                .get(&wo.common.layer)
                .map(|l| l.flags.off || l.flags.frozen)
                .unwrap_or(false)
            {
                continue;
            }
            if !self.belongs_to_visible_block(wo.common.handle, wo.common.owner_handle, layout_block)
            {
                continue;
            }
            // Paper-block wipeouts live in paper coords — no `world_offset`.
            let (fill_origin, boundary) = Self::wipeout_boundary_2d(wo);
            if boundary.len() < 3 {
                continue;
            }
            let mut fill_color = bg_color;
            if self.selected.contains(&wo.common.handle) {
                fill_color = [0.15, 0.55, 1.00, 0.35];
            }
            models.push(HatchModel {
                boundary: Arc::new(boundary),
                boundary_wcs: None,
                pattern: model::hatch_model::HatchPattern::Solid,
                name: "WIPEOUT_FILL".into(),
                color: fill_color,
                angle_offset: 0.0,
                scale: 1.0,
                world_origin: fill_origin,
                draw_depth: 0.0,
            });
        }
        Arc::new(models)
    }

    /// Build a Camera oriented and scaled to match a paper-space Viewport entity.
    /// Used by `active_viewports` to render model-space content through each
    /// content viewport's own view direction and scale.
    pub(super) fn camera_for_viewport(&self, vp_handle: Handle) -> Option<view::camera::Camera> {
        let vp = match self.document.get_entity(vp_handle) {
            Some(EntityType::Viewport(vp)) => vp,
            _ => return None,
        };

        // Floating-viewport–specific step: decide saved-view vs auto-fit, then
        // hand the effective view to the shared `camera_from_view` decoder so
        // twist / view_center / distance behave identically to a model VPORT.
        //
        // UTM / coordinate-shifted drawings often arrive with
        // `view_target = (0, 0, 0)` and a stale `view_center` from before the
        // file was geo-referenced; the saved view points at empty WCS while the
        // actual model sits ~`world_offset` away. Decode the saved view first
        // and keep it only if its target actually frames the model cluster.
        //
        // The overlap test runs on the *decoded* target (wire-space, so the
        // cluster is `±cluster_half` about the origin), NOT a raw
        // `view_target + view_center` sum: under a view twist `view_center` is a
        // DCS offset, so the raw sum lands far from the real WCS centre and
        // would wrongly trip the auto-fit — replacing the saved view_height with
        // the whole-cluster fit and rendering the content at the wrong zoom.
        let saved_h = vp.view_height.abs();
        let aspect_d = (vp.width / vp.height.max(1.0)).max(1e-9);
        let cluster_half = self.local_extent_max.max(1.0) as f64;
        // Absolute drawing centre. Geometry now reaches the scene at absolute
        // (UTM) coordinates — the old code centred the overlap test and the
        // auto-fit on the origin, which was right only while world_offset
        // re-centred the model there. Without it a UTM drawing sits ~5.7e6 away,
        // so a stale `(0,0,0)` saved view failed the overlap test AND the
        // auto-fit aimed at empty origin → blank viewports.
        // Frame the overlap test / auto-fit on the robust cluster centre (median
        // of entity centroids), NOT the raw extents centre: a drawing with a
        // far second cluster (e.g. a small-coordinate legend beside a UTM survey)
        // has an extents centre in the empty gap, which would reject a valid
        // saved view and then auto-fit onto blank space. Fall back to the extents
        // centre only when no cluster centre was computed.
        let (cx, cy) = if self.local_center != [0.0, 0.0] {
            (self.local_center[0], self.local_center[1])
        } else {
            self.model_space_extents()
                .map(|(mn, mx)| {
                    (((mn.x + mx.x) * 0.5) as f64, ((mn.y + mx.y) * 0.5) as f64)
                })
                .unwrap_or((0.0, 0.0))
        };

        if let Some(cam) = self.camera_from_view(
            vp.view_direction,
            vp.view_target,
            acadrust::types::Vector2 {
                x: vp.view_center.x,
                y: vp.view_center.y,
            },
            saved_h,
            vp.twist_angle,
        ) {
            let half_h = saved_h * 0.5;
            let half_w = half_h * aspect_d;
            let (tx, ty) = (cam.target.x as f64, cam.target.y as f64);
            let overlaps = tx + half_w >= cx - cluster_half
                && tx - half_w <= cx + cluster_half
                && ty + half_h >= cy - cluster_half
                && ty - half_h <= cy + cluster_half;
            if overlaps {
                return Some(cam);
            }
        }

        // Auto-fit: aim at the content cluster centre, drop the stale view_center.
        let fit_h = cluster_half * 2.0 * 1.05;
        let tgt = acadrust::types::Vector3 {
            x: cx,
            y: cy,
            z: vp.view_target.z,
        };
        self.camera_from_view(
            vp.view_direction,
            tgt,
            acadrust::types::Vector2::ZERO,
            fit_h,
            vp.twist_angle,
        )
    }

    /// Collect model-space WireModels visible through `vp_handle`, respecting
    /// global layer visibility, the viewport's per-viewport layer freeze list,
    /// and the per-viewport frustum + LOD cull derived from
    /// `screen_height_px` (the on-paper pixel height of this viewport).
    fn model_wires_for_viewport(
        &self,
        vp_handle: Handle,
        _screen_height_px: f32,
    ) -> Arc<Vec<WireModel>> {
        use rustc_hash::FxHashSet as HSet;

        // Only content-REAL per-viewport parameters remain: the viewport's own
        // frozen-layer set and its annotation/PSLTSCALE scale. Frustum cull and
        // zoom LOD are gone — every space (Model tiles, BEDIT, the sheet, and
        // content viewports) shares the same un-culled, LOD-free resident
        // infrastructure, so viewports with default parameters all reuse ONE
        // static-hold entry (and its stable content id) instead of re-baking
        // per viewport per zoom step.
        let (frozen, vp_anno_scale) = match self.document.get_entity(vp_handle) {
            Some(EntityType::Viewport(vp)) => {
                let f: HSet<Handle> = vp.frozen_layers.iter().cloned().collect();
                let vp_scale =
                    vp_effective_scale(vp.custom_scale, vp.view_height, vp.height);
                let anno = if vp_scale > 1e-9 {
                    (1.0 / vp_scale) as f32
                } else {
                    1.0_f32
                };
                (f, anno)
            }
            _ => (HSet::default(), 1.0_f32),
        };

        self.resident_wires_for(
            self.model_space_block_handle(),
            Some(vp_anno_scale),
            Some(&frozen),
        )
    }

    /// Resident model wires for a paper content viewport. Just the unified
    /// static-hold (`resident_wires_for`) keyed on the viewport's frozen set +
    /// annotation scale — no per-viewport height/view cache anymore: the set is
    /// camera-independent (no cull, no LOD), so paper zoom and MSPACE frustum
    /// changes reuse it as-is, and viewports with default parameters all share
    /// one entry (and one stable content id).
    pub(crate) fn model_wires_for_viewport_arc(
        &self,
        vp_handle: Handle,
        screen_height_px: f32,
    ) -> Arc<Vec<WireModel>> {
        self.model_wires_for_viewport(vp_handle, screen_height_px)
    }
}
