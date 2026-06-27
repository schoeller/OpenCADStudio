//! `file` arms and helpers, split out of the original `update.rs` (#mechanical decomposition).

#![allow(unused_imports)]
use super::util::*;
use super::{format_size, VIEWCUBE_HIT_SIZE};
use crate::app::helpers::{
    ortho_constrain, parse_coord, polar_constrain_near, ucs_rotate_vec, ucs_to_wcs, ucs_z_axis,
    CoordKind,
};
use crate::app::{Message, OpenCADStudio, POLY_START_DELAY_MS};
use crate::modules::ModuleEvent;
use crate::scene::pick::grip::{find_hit_grip, find_hit_grip_paper, find_hit_grip_rte, GripEdit};
use crate::scene::model::object::GripApply;
use crate::scene::{
    self, hover_id, CubeRegion, Scene, VIEWCUBE_DRAW_PX, VIEWCUBE_PAD, VIEWCUBE_PX,
};
use crate::ui::PropertiesPanel;
use acadrust::types::Color as AcadColor;
use acadrust::{EntityType as AcadEntityType, Handle};
use iced::time::Instant;
use iced::{mouse, Point, Task};


impl OpenCADStudio {
    /// Snapshot the persisted UI preferences from live state.
    pub(in crate::app) fn current_settings(&self) -> crate::app::settings::UserSettings {
        crate::app::settings::UserSettings {
            dyn_input: self.dyn_input,
            ortho: self.ortho_mode,
            polar: self.polar_mode,
            polar_increment_deg: self.polar_increment_deg,
            snap_enabled: self.snapper.snap_enabled,
            otrack: self.snapper.otrack_enabled,
            snap_modes: crate::app::settings::UserSettings::modes_from(self.snapper.enabled.iter()),
            default_assoc_prompted: self.default_assoc_prompted,
            disabled_plugins: {
                let mut v: Vec<String> = self.disabled_plugins.iter().cloned().collect();
                v.sort();
                v
            },
            plugin_repos: self.plugin_repos.clone(),
            texteditmode: self.texteditmode,
            bg_color: self.default_bg_color.map(f4_to_u3),
            paper_bg_color: self.default_paper_bg_color.map(f4_to_u3),
        }
    }

    /// Apply restored preferences to live state.
    pub(in crate::app) fn apply_settings(&mut self, s: &crate::app::settings::UserSettings) {
        self.dyn_input = s.dyn_input;
        self.ortho_mode = s.ortho;
        self.polar_mode = s.polar;
        self.polar_increment_deg = s.polar_increment_deg;
        self.snapper.snap_enabled = s.snap_enabled;
        self.snapper.otrack_enabled = s.otrack;
        self.snapper.enabled = s.snap_modes.iter().copied().collect();
        self.default_assoc_prompted = s.default_assoc_prompted;
        self.disabled_plugins = s.disabled_plugins.iter().cloned().collect();
        self.plugin_repos = s.plugin_repos.clone();
        self.texteditmode = s.texteditmode;
        self.default_bg_color = s.bg_color.map(u3_to_f4);
        self.default_paper_bg_color = s.paper_bg_color.map(u3_to_f4);
        // Push the restored background onto every drawing tab that exists now
        // (the start tab and any initial drawing). Tabs created later pick it
        // up via `apply_bg_default` at their construction site.
        for idx in 0..self.tabs.len() {
            self.apply_bg_default(idx);
        }
        self.rebuild_ribbon_modules();
    }

    /// Apply the persisted default background(s) to tab `idx`. No-op for the
    /// start tab or when no default is set. Refreshes the tab's cached wires
    /// and meshes so background-adaptive colours pick up the change.
    pub(in crate::app) fn apply_bg_default(&mut self, idx: usize) {
        let bg = self.default_bg_color;
        let paper_bg = self.default_paper_bg_color;
        if bg.is_none() && paper_bg.is_none() {
            return;
        }
        let tab = &mut self.tabs[idx];
        if tab.is_start {
            return;
        }
        if let Some(c) = bg {
            tab.bg_color = Some(c);
            tab.scene.bg_color = c;
        }
        if let Some(c) = paper_bg {
            tab.paper_bg_color = Some(c);
            tab.scene.paper_bg_color = c;
        }
        tab.scene.recolor_meshes();
        tab.scene.bump_geometry();
    }

    /// Check if a suspended command exists on the active tab and resume it
    /// with the outcome of the text editor.
    pub(in crate::app) fn post_editor_closed(&mut self, committed: bool) -> Task<Message> {
        let i = self.active_tab;
        if let Some(mut cmd) = self.tabs[i].suspended_cmd.take() {
            let res = cmd.on_editor_closed(committed);
            self.tabs[i].active_cmd = Some(cmd);
            self.apply_cmd_result(res)
        } else {
            Task::none()
        }
    }

    /// Rebuild the ribbon's tab list from the registry, dropping the tabs of any
    /// disabled plugins. Call after `disabled_plugins` changes.
    pub(in crate::app) fn rebuild_ribbon_modules(&mut self) {
        let modules =
            crate::plugin::ribbon_modules_enabled(&self.disabled_plugins);
        self.ribbon.set_modules(modules);
    }

    /// Snapshot of disabled plugin ids — lets the registry skip them while it
    /// holds a `&mut` borrow of the app via `HostSession`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn disabled_plugin_ids(&self) -> rustc_hash::FxHashSet<String> {
        self.disabled_plugins.clone()
    }

    /// Background task: fetch the curated plugin registry.
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::app) fn fetch_registry_task(&self) -> Task<Message> {
        Task::perform(
            async { crate::plugin::marketplace::fetch_registry() },
            Message::PluginRegistryFetched,
        )
    }

    /// Background task: fetch `owner/repo`'s installable release tags.
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::app) fn fetch_releases_task(&self, repo: String) -> Task<Message> {
        let label = repo.clone();
        Task::perform(
            async move {
                crate::plugin::marketplace::fetch_releases(&repo).map(|rs| {
                    rs.into_iter()
                        .filter(|r| r.installable())
                        .map(|r| r.tag)
                        .collect::<Vec<_>>()
                })
            },
            move |res| Message::PluginReleasesFetched(label, res),
        )
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::app) fn fetch_releases_task(&self, _repo: String) -> Task<Message> {
        Task::none()
    }

    /// Background task: download and install the `tag` release of `owner/repo`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::app) fn install_task(&self, repo: String, tag: String) -> Task<Message> {
        Task::perform(
            async move {
                let releases = crate::plugin::marketplace::fetch_releases(&repo)?;
                let rel = releases
                    .into_iter()
                    .find(|r| r.tag == tag)
                    .ok_or_else(|| format!("release {tag} not found"))?;
                crate::plugin::marketplace::install(&rel)
            },
            Message::PluginInstalled,
        )
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::app) fn install_task(&self, _repo: String, _tag: String) -> Task<Message> {
        Task::none()
    }

    /// Write preferences to disk only when they differ from the last write,
    /// so a toggle persists immediately without thrashing the file.
    pub(in crate::app) fn persist_settings_if_changed(&mut self) {
        let cur = self.current_settings();
        if self.last_saved_settings.as_ref() != Some(&cur) {
            cur.save();
            self.last_saved_settings = Some(cur);
        }
    }

    /// Record that the one-time default-association prompt has been answered and
    /// flush it to disk, so the dialog never reappears on later launches.
    pub(in crate::app) fn mark_assoc_prompted(&mut self) {
        self.default_assoc_prompted = true;
        self.persist_settings_if_changed();
    }

pub(super) fn on_open_file(&mut self) -> Task<Message> {
                // Native: pick a path, then load on a worker thread. Web: the
                // browser hands back bytes, so pick + parse in one step and feed
                // the shared `FileOpened` handler directly.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    Task::perform(crate::io::pick_open_path(), Message::OpenPathPicked)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    // `FileOpened` only installs the result when an open is in
                    // progress, so mark one. The browser picker + parse happen
                    // inside `pick_and_load_web`; the real name is unknown until
                    // then, so show a generic label meanwhile.
                    self.opening = Some(crate::app::OpenProgress {
                        name: "Opening…".into(),
                        size_bytes: 0,
                        phase: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                            crate::app::OPEN_PHASE_READING,
                        )),
                        started: Instant::now(),
                    });
                    Task::perform(crate::io::pick_and_load_web(), Message::FileOpened)
                }
    }

    pub(super) fn on_file_opened(&mut self, name: String, path: std::path::PathBuf, doc: acadrust::CadDocument, caches: crate::scene::DerivedCaches) -> Task<Message> {
                // If the user clicked Cancel while the parser was running, the
                // overlay state was cleared and we silently drop the result.
                if self.opening.is_none() {
                    return Task::none();
                }
                let open_started = self.opening.take().map(|p| p.started);
                let timings = caches.timings;
                let entity_count = doc.entities().count();
                self.command_line
                    .push_output(&format!("Opened \"{name}\" — {entity_count} entities"));
                if caches.corrupt_dropped > 0 {
                    self.command_line.push_error(&format!(
                        "Warning: {} corrupt entities dropped (parser junk — bad normals / counts)",
                        caches.corrupt_dropped
                    ));
                }
                self.app_menu.push_recent(path.clone());

                let current_is_empty = {
                    let t = &self.tabs[self.active_tab];
                    !t.is_start
                        && t.current_path.is_none()
                        && !t.dirty
                        && self.tabs[self.active_tab].scene.document.entities().count() == 0
                };
                let i = if current_is_empty {
                    self.active_tab
                } else {
                    self.tab_counter += 1;
                    let new_tab = crate::app::document::DocumentTab::new_drawing(self.tab_counter);
                    self.tabs.push(new_tab);
                    let idx = self.tabs.len() - 1;
                    self.active_tab = idx;
                    self.apply_bg_default(idx);
                    idx
                };

                self.tabs[i].current_path = Some(path.clone());
                self.tabs[i].scene.document = doc;
                // Follow the file's saved current UCS from the moment it opens.
                self.tabs[i].adopt_active_ucs_from_header();
                // Route shared CJK ideographs to the language matching this
                // drawing's code page (web per-language font split). Drop the
                // glyph cache if it changed so Han re-resolves to the new
                // language's font; geometry is (re)built below regardless. (#141)
                if crate::scene::text::web_font::set_cjk_lang_from_codepage(
                    &self.tabs[i].scene.document.header.code_page,
                ) {
                    crate::scene::text::ttf_glyph::clear_fallback_cache();
                }
                // Current model-space annotation scale comes from the drawing's
                // CANNOSCALEVALUE (paper/drawing factor); the multiplier we use
                // for text/dim sizing is its inverse (1:50 -> 0.02 -> 50.0).
                let cannoscale_value = self.tabs[i].scene.document.header.annotation_scale_value;
                self.tabs[i].scene.annotation_scale = if cannoscale_value > 1e-9 {
                    (1.0 / cannoscale_value) as f32
                } else {
                    1.0
                };

                // Auto-resolve XREFs relative to the opened file's directory.
                let mut xref_ms = 0u32;
                if let Some(base_dir) = path.parent() {
                    // xref content arrives un-purged: parser-garbage entities
                    // inside the referenced file can trigger infinite loops in
                    // tessellation. `resolve_xrefs` runs the corrupt-entity
                    // guard inline as it merges each xref, so no second
                    // full-document walk is needed here.
                    let t_xref = Instant::now();
                    let (xrefs, extra_dropped) =
                        crate::io::xref::resolve_xrefs(&mut self.tabs[i].scene.document, base_dir);
                    xref_ms = t_xref.elapsed().as_millis() as u32;
                    if extra_dropped > 0 {
                        self.command_line.push_error(&format!(
                            "Warning: {extra_dropped} corrupt xref entities dropped"
                        ));
                    }
                    for info in &xrefs {
                        match info.status {
                            crate::io::xref::XrefStatus::Loaded => {
                                self.command_line
                                    .push_output(&format!("XREF  Loaded \"{}\"", info.name));
                            }
                            crate::io::xref::XrefStatus::NotFound => {
                                self.command_line.push_error(&format!(
                                    "XREF  Not found: \"{}\" ({})",
                                    info.name, info.path
                                ));
                            }
                            crate::io::xref::XrefStatus::Unloaded => {
                                self.command_line.push_info(&format!(
                                    "XREF  Unloaded (skipped): \"{}\"",
                                    info.name
                                ));
                            }
                        }
                    }
                }

                // Open-time breakdown so regressions are visible immediately.
                // `total` is wall time from the Open click to here (post-xref,
                // pre-first-frame); the phase figures are the background-thread
                // parse/purge/cache spans plus the UI-thread xref resolve.
                let total_ms = open_started
                    .map(|s| s.elapsed().as_millis() as u32)
                    .unwrap_or(0);
                self.command_line.push_info(&format!(
                    "  parse {}ms · purge {}ms · caches {}ms · xref {}ms · total {}ms",
                    timings.parse_ms, timings.purge_ms, timings.caches_ms, xref_ms, total_ms
                ));

                // Caches were built on the background thread inside open_path().
                self.tabs[i].scene.local_extent_max = caches.local_extent_max;
                self.tabs[i].scene.local_center = caches.local_center;
                self.tabs[i].scene.hatches = caches.hatches;
                self.tabs[i].scene.images = caches.images;
                self.tabs[i].scene.meshes = caches.meshes;
                self.tabs[i].scene.block_meshes = caches.block_meshes;
                // Invalidate the wire cache so the new document is tessellated.
                self.tabs[i].scene.bump_geometry();
                self.tabs[i].scene.selected = rustc_hash::FxHashSet::default();
                self.tabs[i].scene.preview_wires = vec![];
                self.tabs[i].scene.current_layout = "Model".to_string();
                crate::io::linetypes::populate_document(&mut self.tabs[i].scene.document);
                self.tabs[i].properties = PropertiesPanel::empty();
                // Seed the current table / multileader style from the file's
                // header so the ✓ marks the right one (text/dim/mline come from
                // the document header directly). DXF provides these via
                // $CTABLESTYLE / $CMLEADERSTYLE; DWG leaves them at "Standard".
                self.ribbon.active_table_style = self.tabs[i]
                    .scene
                    .document
                    .header
                    .current_table_style_name
                    .clone();
                self.tabs[i].active_mleader_style = self.tabs[i]
                    .scene
                    .document
                    .header
                    .current_mleader_style_name
                    .clone();
                let doc_layers = self.tabs[i].scene.document.layers.clone();
                let vp_info = self.tabs[i].scene.viewport_list();
                self.tabs[i]
                    .layers
                    .sync_with_viewports(&doc_layers, vp_info);
                self.sync_ribbon_layers();
                // Load the Annotate-ribbon style dropdowns (text / dimension /
                // multileader / table) from the opened document instead of
                // leaving them on the hard-coded "Standard" default.
                self.sync_ribbon_styles();
                // Reset the Home-ribbon Color / Linetype / Lineweight chips
                // to the newly opened document's CECOLOR / CELTYPE / CELWEIGHT
                // defaults (or to ByLayer when the file leaves them empty).
                // Without this they stick to whatever the prior tab had
                // selected — see #21.
                self.sync_ribbon_from_selection();
                self.tabs[i].scene.restore_saved_camera();
                // Grid/snap are per-drawing view settings — adopt the opened
                // file's active viewport state rather than a global preference.
                self.adopt_view_display(i);
                self.sync_render_mode_to_active_tile(i);
                self.tabs[i].last_synced_camera_gen = self.tabs[i].scene.camera_generation;
                self.tabs[i].dirty = false;
                self.tabs[i].history = crate::app::document::HistoryState::default();
                self.refresh_selected_grips();
                Task::none()
    }

    pub(super) fn on_wblock_save_result_some(&mut self, block_name: String, path: std::path::PathBuf) -> Task<Message> {
                let i = self.active_tab;
                let result = if block_name == "*" {
                    let handles: Vec<_> = self.tabs[i].scene.selected.iter().copied().collect();
                    crate::modules::insert::wblock::extract_entities_to_doc(
                        &self.tabs[i].scene.document,
                        &handles,
                    )
                } else {
                    crate::modules::insert::wblock::extract_block_to_doc(
                        &self.tabs[i].scene.document,
                        &block_name,
                    )
                };
                match result {
                    Ok(doc) => match crate::io::save(&doc, &path) {
                        Ok(()) => {
                            let fname = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.to_string_lossy().into_owned());
                            self.command_line.push_output(&format!(
                                "WBLOCK  Saved \"{block_name}\" → \"{fname}\""
                            ));
                        }
                        Err(e) => self
                            .command_line
                            .push_error(&format!("WBLOCK save failed: {e}")),
                    },
                    Err(e) => self.command_line.push_error(&format!("WBLOCK: {e}")),
                }
                Task::none()
    }

    pub(super) fn on_stl_export_path_some(&mut self, path: std::path::PathBuf) -> Task<Message> {
                // Re-build STL bytes (we can't easily pass them through the message).
                let i = self.active_tab;
                // STL gets the highest-resolution LOD (slot 0) so the
                // exported geometry isn't downgraded by the view-dependent
                // mesh LOD ladder used for rendering.
                let meshes: Vec<crate::scene::model::mesh_model::MeshModel> = self.tabs[i]
                    .scene
                    .meshes
                    .values()
                    .filter_map(|s| s.lods.first().cloned())
                    .collect();
                let mesh_refs: Vec<&crate::scene::model::mesh_model::MeshModel> = meshes.iter().collect();
                match crate::io::stl::build_stl(&mesh_refs) {
                    Some(bytes) => match std::fs::write(&path, bytes) {
                        Ok(()) => self
                            .command_line
                            .push_output(&format!("STLOUT: exported to \"{}\"", path.display())),
                        Err(e) => self
                            .command_line
                            .push_error(&format!("STLOUT: write error: {e}")),
                    },
                    None => self
                        .command_line
                        .push_error("STLOUT: no mesh data to export."),
                }
                Task::none()
    }

    pub(super) fn on_step_export_path_some(&mut self, path: std::path::PathBuf) -> Task<Message> {
                let i = self.active_tab;
                // Export uses LOD 0 (full resolution); see StlExportPath above.
                let meshes: Vec<crate::scene::model::mesh_model::MeshModel> = self.tabs[i]
                    .scene
                    .meshes
                    .values()
                    .filter_map(|s| s.lods.first().cloned())
                    .collect();
                let mesh_refs: Vec<&crate::scene::model::mesh_model::MeshModel> = meshes.iter().collect();
                match crate::io::step::build_step(&mesh_refs) {
                    Some(text) => match std::fs::write(&path, text.as_bytes()) {
                        Ok(()) => self
                            .command_line
                            .push_output(&format!("STEPOUT: exported to \"{}\"", path.display())),
                        Err(e) => self
                            .command_line
                            .push_error(&format!("STEPOUT: write error: {e}")),
                    },
                    None => self
                        .command_line
                        .push_error("STEPOUT: no mesh data to export."),
                }
                Task::none()
    }

    pub(super) fn on_obj_import_path_some(&mut self, path: std::path::PathBuf) -> Task<Message> {
                let src = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        self.command_line
                            .push_error(&format!("IMPORTOBJ: read error: {e}"));
                        return Task::none();
                    }
                };
                let color = [0.7f32, 0.7, 0.85, 1.0];
                match crate::io::obj::parse_obj(&src, color) {
                    None => {
                        self.command_line
                            .push_error("IMPORTOBJ: no usable geometry in file.");
                    }
                    Some(mut mesh) => {
                        let i = self.active_tab;
                        let file_stem = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "obj_mesh".into());
                        mesh.name = file_stem.clone();
                        self.push_undo_snapshot(i, "IMPORTOBJ");
                        use crate::modules::insert::solid3d_cmds::empty_solid3d;
                        let entity = empty_solid3d();
                        let handle = self.tabs[i].scene.add_entity(entity);
                        if !handle.is_null() {
                            self.tabs[i]
                                .scene
                                .meshes
                                .insert(handle, crate::scene::MeshLodSet::from_single(mesh));
                            self.tabs[i].dirty = true;
                            self.command_line.push_output(&format!(
                                "IMPORTOBJ: imported \"{}\" as mesh.",
                                file_stem
                            ));
                        }
                    }
                }
                Task::none()
    }

    /// Resolve the effective DWG save version for `tab`.
    ///
    /// A drawing that carries data locked to its source DWG version (e.g.
    /// AEC/Civil3D objects with no published spec, raw MLEADER/Surface records,
    /// EED) can only be written losslessly in that version. When the requested
    /// version is in a different encoding family, fall back to the source
    /// version so nothing is dropped and the file opens cleanly elsewhere, and
    /// note it on the command line. Non-DWG / convertible saves are unchanged.
    /// Shared by every save path (Save, Save As, save-before-close).
    pub(crate) fn dwg_save_version(
        &mut self,
        tab: usize,
        ext: &str,
        requested: acadrust::DxfVersion,
    ) -> acadrust::DxfVersion {
        if ext != "dwg" {
            return requested;
        }
        let fallback = {
            let doc = &self.tabs[tab].scene.document;
            match doc.dwg_source_version {
                Some(src) if src != requested && doc.has_version_locked_data(requested) => Some(src),
                _ => None,
            }
        };
        match fallback {
            Some(src) => {
                self.command_line.push_output(&format!(
                    "Saved in source DWG version ({src:?}) to preserve objects that cannot be \
                     converted to the selected version."
                ));
                src
            }
            None => requested,
        }
    }

    pub(super) fn on_save_file(&mut self) -> Task<Message> {
                if self.read_only {
                    self.command_line
                        .push_error("Read-only session (--read-only): saving is disabled.");
                    return Task::none();
                }
                let i = self.active_tab;
                // Stamp the live grid/snap toggles onto the VPort so the file
                // reflects them even if they came from settings with no
                // in-session toggle (#121).
                self.sync_vport_display(i);
                // Native: save straight to the known path. Web has no path
                // (downloads instead), so always go through the Save dialog.
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = self.tabs[i].current_path.clone() {
                    self.tabs[i].scene.document.header.user_real1 =
                        self.tabs[i].scene.annotation_scale as f64;
                    // Preserve the document's version, but fall back to the
                    // source version if it carries version-locked data (so a
                    // direct Save can't silently drop AEC/Civil3D objects).
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let requested = self.tabs[i].scene.document.version;
                    let version = self.dwg_save_version(i, &ext, requested);
                    match crate::io::save_as_version(&self.tabs[i].scene.document, &path, version) {
                        Ok(()) => {
                            self.command_line
                                .push_output(&format!("Saved: {}", path.display()));
                            self.tabs[i].dirty = false;
                        }
                        Err(e) => self.command_line.push_error(&format!("Save failed: {e}")),
                    }
                    return Task::none();
                }
                self.save_dialog_for_unsaved = false;
                self.open_save_dialog_window(i)
    }

    pub(super) fn on_save_dialog_confirm(&mut self) -> Task<Message> {
                let (ext, version) = crate::io::parse_save_format(&self.save_dialog_format);
                // The user need not type an extension: append the selected
                // format's one when the entered name carries none.
                let name = self.save_dialog_filename.trim();
                let filename = if name.is_empty() {
                    format!("drawing.{ext}")
                } else if std::path::Path::new(name).extension().is_none() {
                    format!("{name}.{ext}")
                } else {
                    name.to_string()
                };
                self.save_dialog_filename = filename.clone();
                self.save_dialog_filename = filename.clone();
                let close = self.close_save_dialog_window();
                let i = self.active_tab;
                sync_annotation_scale_header(&mut self.tabs[i].scene);

                // Fall back to the source version for files with version-locked
                // data (e.g. AEC/Civil3D objects) so nothing is dropped.
                let version = self.dwg_save_version(i, ext, version);

                // Native: write to the chosen path. Web: download the bytes
                // under the chosen name (no filesystem).
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let path = self.save_dialog_folder.join(&filename);
                    match crate::io::save_as_version(&self.tabs[i].scene.document, &path, version) {
                        Ok(()) => {
                            self.command_line
                                .push_output(&format!("Saved: {}", path.display()));
                            self.tabs[i].current_path = Some(path.clone());
                            self.tabs[i].dirty = false;
                            if self.save_dialog_for_unsaved {
                                let next = self.update(Message::UnsavedPickedSavePath(Some(path)));
                                return Task::batch([close, next]);
                            }
                        }
                        Err(e) => self.command_line.push_error(&format!("Save failed: {e}")),
                    }
                    close
                }
                #[cfg(target_arch = "wasm32")]
                {
                    match crate::io::save_to_bytes(&self.tabs[i].scene.document, ext, version) {
                        Ok(bytes) => {
                            crate::sys::download_bytes(&filename, &bytes);
                            self.tabs[i].dirty = false;
                            self.command_line.push_output(&format!("Saved: {filename}"));
                        }
                        Err(e) => self.command_line.push_error(&format!("Save failed: {e}")),
                    }
                    // Continue a pending tab close.
                    if self.save_dialog_for_unsaved {
                        if let Some(crate::app::PendingClose::Tab(idx)) = self.pending_close.take() {
                            let cont = self.update(Message::TabClose(idx));
                            return Task::batch([close, cont]);
                        }
                    }
                    close
                }
    }

    pub(super) fn on_page_setup_commit(&mut self) -> Task<Message> {
                let i = self.active_tab;
                let layout_name = self.tabs[i].scene.current_layout.clone();
                if layout_name != "Model" {
                    let w: f64 = self.page_setup_w.parse::<f64>().unwrap_or(297.0).max(1.0);
                    let h: f64 = self.page_setup_h.parse::<f64>().unwrap_or(210.0).max(1.0);
                    let plot_area = self.page_setup_plot_area.clone();
                    let center = self.page_setup_center;
                    let offset_x = self.page_setup_offset_x.parse::<f64>().unwrap_or(0.0);
                    let offset_y = self.page_setup_offset_y.parse::<f64>().unwrap_or(0.0);
                    let rotation: i16 = self.page_setup_rotation.parse().unwrap_or(0);
                    let scale_str = self.page_setup_scale.clone();

                    // Update the Layout object's limits AND its embedded
                    // PlotSettings fields. `paper_limits()` (sheet rendering) and
                    // the DWG writer both read these from the Layout, so a page
                    // setup that only touched a side PlotSettings object would not
                    // reflect on screen or survive a save. The dialog's w/h are
                    // the final sheet dimensions, so store them verbatim with no
                    // further rotation swap (#156).
                    for obj in self.tabs[i].scene.document.objects.values_mut() {
                        if let acadrust::objects::ObjectType::Layout(l) = obj {
                            if l.name == layout_name {
                                l.min_limits = (0.0, 0.0);
                                l.max_limits = (w, h);
                                l.min_extents = (0.0, 0.0, 0.0);
                                l.max_extents = (w, h, 0.0);
                                l.paper_width = w;
                                l.paper_height = h;
                                l.plot_rotation = 0;
                                l.plot_paper_units = 1; // millimetres
                                l.plot_origin_x = offset_x;
                                l.plot_origin_y = offset_y;
                                // Custom dimensions no longer match a named size.
                                l.paper_size = String::new();
                                break;
                            }
                        }
                    }

                    // Find or create the PlotSettings object for this layout.
                    use acadrust::objects::{
                        ObjectType, PlotPaperUnits, PlotRotation, PlotSettings, PlotType,
                    };
                    let plot_handle =
                        self.tabs[i]
                            .scene
                            .document
                            .objects
                            .iter()
                            .find_map(|(h, obj)| {
                                if let ObjectType::PlotSettings(ps) = obj {
                                    if ps.page_name == layout_name {
                                        Some(*h)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });

                    let ps_entry = if let Some(h) = plot_handle {
                        self.tabs[i].scene.document.objects.get_mut(&h)
                    } else {
                        // Create a new PlotSettings object and insert it.
                        let mut ps = PlotSettings::new(layout_name.clone());
                        ps.handle = self.tabs[i].scene.document.allocate_handle();
                        let h = ps.handle;
                        self.tabs[i]
                            .scene
                            .document
                            .objects
                            .insert(h, ObjectType::PlotSettings(ps));
                        self.tabs[i].scene.document.objects.get_mut(&h)
                    };

                    if let Some(ObjectType::PlotSettings(ps)) = ps_entry {
                        ps.paper_width = w;
                        ps.paper_height = h;
                        ps.paper_units = PlotPaperUnits::Millimeters;
                        ps.plot_type = if plot_area == "Extents" {
                            PlotType::Extents
                        } else {
                            PlotType::Layout
                        };
                        ps.flags.plot_centered = center;
                        ps.origin_x = offset_x;
                        ps.origin_y = offset_y;
                        ps.rotation = match rotation {
                            90 => PlotRotation::Degrees90,
                            180 => PlotRotation::Degrees180,
                            270 => PlotRotation::Degrees270,
                            _ => PlotRotation::None,
                        };
                        // Apply plot scale.
                        use acadrust::objects::ScaledType;
                        let (num, den) = parse_plot_scale(&scale_str);
                        if scale_str == "Fit" {
                            ps.set_scale_to_fit();
                        } else {
                            ps.scale_type = ScaledType::CustomScale;
                            ps.scale_numerator = num;
                            ps.scale_denominator = den;
                        }
                    }

                    self.tabs[i].dirty = true;
                    // The paper sheet fill is cached; bump geometry so the new
                    // sheet size re-tessellates and shows immediately.
                    self.tabs[i].scene.bump_geometry();
                    self.command_line.push_info(&format!(
                        "Page setup: {w:.1}×{h:.1} mm  area={plot_area}  \
                         center={center}  rot={rotation}°"
                    ));
                }
                self.active_modal = None;
                Task::none()
    }

    pub(super) fn on_plot_export_path_some(&mut self, path: std::path::PathBuf) -> Task<Message> {
                let i = self.active_tab;
                let scene = &self.tabs[i].scene;
                let wires = scene.entity_wires();
                let hatches = scene.paper_canvas_hatches();
                let wipeouts = scene.paper_canvas_wipeouts();

                // Read PlotSettings for current layout (if available).
                use acadrust::objects::PlotType;
                let ps_snap = scene.effective_plot_settings();

                // Determine paper size and drawing offset.
                let (paper_w, paper_h, mut draw_ox, mut draw_oy, rotation_deg) =
                    if let Some(((x0, y0), (x1, y1))) = scene.paper_limits() {
                        let (pw, ph) = (x1 - x0, y1 - y0);

                        // If PlotSettings says Extents, use model space extents instead.
                        let use_extents = ps_snap
                            .as_ref()
                            .map(|ps| matches!(ps.plot_type, PlotType::Extents))
                            .unwrap_or(false);

                        let (ox, oy) = if use_extents {
                            if let Some((mn, _mx)) = scene.model_space_extents() {
                                (-mn.x as f64, -mn.y as f64)
                            } else {
                                (-x0, -y0)
                            }
                        } else {
                            (-x0, -y0)
                        };

                        let rot = ps_snap
                            .as_ref()
                            .map(|ps| ps.rotation.to_degrees() as i32)
                            .unwrap_or(0);

                        (pw, ph, ox, oy, rot)
                    } else {
                        // Model space: fit with 5% margin.
                        let margin = 1.05_f64;
                        if let Some((mn, mx)) = scene.model_space_extents() {
                            let w = ((mx.x - mn.x) as f64 * margin).max(1.0);
                            let h = ((mx.y - mn.y) as f64 * margin).max(1.0);
                            let pad_x = (w - (mx.x - mn.x) as f64) * 0.5;
                            let pad_y = (h - (mx.y - mn.y) as f64) * 0.5;
                            (w, h, -(mn.x as f64) + pad_x, -(mn.y as f64) + pad_y, 0)
                        } else {
                            (297.0, 210.0, 0.0, 0.0, 0)
                        }
                    };

                // Apply PlotSettings offset and centering.
                if let Some(ref ps) = ps_snap {
                    if ps.flags.plot_centered {
                        // Centering: compute wire extents and re-centre.
                        let all_x: Vec<f32> = wires
                            .iter()
                            .flat_map(|w| w.points.iter().map(|p| p[0]))
                            .filter(|v| !v.is_nan())
                            .collect();
                        let all_y: Vec<f32> = wires
                            .iter()
                            .flat_map(|w| w.points.iter().map(|p| p[1]))
                            .filter(|v| !v.is_nan())
                            .collect();
                        if let (Some(&min_x), Some(&max_x), Some(&min_y), Some(&max_y)) = (
                            all_x.iter().copied().reduce(f32::min).as_ref(),
                            all_x.iter().copied().reduce(f32::max).as_ref(),
                            all_y.iter().copied().reduce(f32::min).as_ref(),
                            all_y.iter().copied().reduce(f32::max).as_ref(),
                        ) {
                            let cx = (min_x + max_x) as f64 / 2.0;
                            let cy = (min_y + max_y) as f64 / 2.0;
                            draw_ox += paper_w / 2.0 - cx;
                            draw_oy += paper_h / 2.0 - cy;
                        }
                    } else {
                        draw_ox += ps.origin_x;
                        draw_oy += ps.origin_y;
                    }
                }

                // For rotation: swap paper dimensions and note angle for export.
                let (eff_w, eff_h) = match rotation_deg {
                    90 | 270 => (paper_h, paper_w),
                    _ => (paper_w, paper_h),
                };

                match crate::io::pdf_export::export_pdf(
                    &wires,
                    hatches.as_slice(),
                    wipeouts.as_slice(),
                    eff_w,
                    eff_h,
                    draw_ox as f32,
                    draw_oy as f32,
                    rotation_deg,
                    &path,
                    self.active_plot_style.as_ref(),
                ) {
                    Ok(()) => self.command_line.push_info(&format!(
                        "Exported: {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )),
                    Err(e) => self.command_line.push_error(&format!("Export failed: {e}")),
                }
                Task::none()
    }

    pub(super) fn on_print_to_printer(&mut self) -> Task<Message> {
                let i = self.active_tab;
                let scene = &self.tabs[i].scene;
                let wires = scene.entity_wires();
                let hatches: Vec<_> = scene.paper_canvas_hatches().as_ref().clone();
                let wipeouts: Vec<_> = scene.paper_canvas_wipeouts().as_ref().clone();
                use acadrust::objects::PlotType;
                let ps_snap = scene.effective_plot_settings();
                let (paper_w, paper_h, draw_ox, draw_oy, rotation_deg) =
                    if let Some(((x0, y0), (x1, y1))) = scene.paper_limits() {
                        let (pw, ph) = (x1 - x0, y1 - y0);
                        let use_extents = ps_snap
                            .as_ref()
                            .map(|ps| matches!(ps.plot_type, PlotType::Extents))
                            .unwrap_or(false);
                        let (ox, oy) = if use_extents {
                            if let Some((mn, _mx)) = scene.model_space_extents() {
                                (-mn.x as f64, -mn.y as f64)
                            } else {
                                (-x0, -y0)
                            }
                        } else {
                            (-x0, -y0)
                        };
                        let rot = ps_snap
                            .as_ref()
                            .map(|ps| ps.rotation.to_degrees() as i32)
                            .unwrap_or(0);
                        (pw, ph, ox, oy, rot)
                    } else {
                        if let Some((mn, mx)) = scene.model_space_extents() {
                            let margin = 1.05_f64;
                            let w = ((mx.x - mn.x) as f64 * margin).max(1.0);
                            let h = ((mx.y - mn.y) as f64 * margin).max(1.0);
                            let pad_x = (w - (mx.x - mn.x) as f64) * 0.5;
                            let pad_y = (h - (mx.y - mn.y) as f64) * 0.5;
                            (w, h, -(mn.x as f64) + pad_x, -(mn.y as f64) + pad_y, 0)
                        } else {
                            (297.0, 210.0, 0.0, 0.0, 0)
                        }
                    };
                let (eff_w, eff_h) = match rotation_deg {
                    90 | 270 => (paper_h, paper_w),
                    _ => (paper_w, paper_h),
                };
                let plot_style = self.active_plot_style.clone();
                self.command_line.push_info("Sending to system printer…");
                Task::perform(
                    async move {
                        crate::io::print_to_printer::print_wires(
                            wires,
                            hatches,
                            wipeouts,
                            eff_w,
                            eff_h,
                            draw_ox as f32,
                            draw_oy as f32,
                            rotation_deg,
                            plot_style,
                        )
                        .await
                    },
                    Message::PrintResult,
                )
    }

    pub(super) fn on_plot_style_panel_apply(&mut self) -> Task<Message> {
                let aci = self.plotstyle_panel_aci as usize;
                if let Some(table) = self.active_plot_style.as_mut() {
                    if let Some(entry) = table.aci_entries.get_mut(aci) {
                        // Parse color.
                        let color_str = self.ps_color_buf.trim();
                        if color_str.is_empty() {
                            entry.color = None;
                        } else if color_str.starts_with('#') && color_str.len() == 7 {
                            let r = u8::from_str_radix(&color_str[1..3], 16).unwrap_or(0);
                            let g = u8::from_str_radix(&color_str[3..5], 16).unwrap_or(0);
                            let b = u8::from_str_radix(&color_str[5..7], 16).unwrap_or(0);
                            entry.color = Some([r, g, b]);
                        }
                        if let Ok(lw) = self.ps_lineweight_buf.trim().parse::<u8>() {
                            entry.lineweight = lw;
                        }
                        if let Ok(sc) = self.ps_screening_buf.trim().parse::<u8>() {
                            entry.screening = sc.min(100);
                        }
                        self.command_line
                            .push_output(&format!("Plot style ACI {aci} updated."));
                    }
                } else {
                    // No table loaded: create an identity table and apply.
                    let mut table = crate::io::plot_style::PlotStyleTable::identity("Custom.ctb");
                    if let Some(entry) = table.aci_entries.get_mut(aci) {
                        let color_str = self.ps_color_buf.trim();
                        if color_str.starts_with('#') && color_str.len() == 7 {
                            let r = u8::from_str_radix(&color_str[1..3], 16).unwrap_or(0);
                            let g = u8::from_str_radix(&color_str[3..5], 16).unwrap_or(0);
                            let b = u8::from_str_radix(&color_str[5..7], 16).unwrap_or(0);
                            entry.color = Some([r, g, b]);
                        }
                        if let Ok(lw) = self.ps_lineweight_buf.trim().parse::<u8>() {
                            entry.lineweight = lw;
                        }
                        if let Ok(sc) = self.ps_screening_buf.trim().parse::<u8>() {
                            entry.screening = sc.min(100);
                        }
                    }
                    self.active_plot_style = Some(table);
                    self.command_line
                        .push_output(&format!("Created new CTB table, ACI {aci} updated."));
                }
                Task::none()
    }

    pub(super) fn on_plot_style_panel_save(&mut self) -> Task<Message> {
                if self.active_plot_style.is_none() {
                    self.command_line
                        .push_error("No plot style table loaded. Load or create one first.");
                    return Task::none();
                }
                let default_name = self
                    .active_plot_style
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or("export.ctb".into());
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Save Plot Style Table")
                            .set_file_name(&default_name)
                            .add_filter("Plot Style Files", &["ctb", "stb", "CTB", "STB"])
                            .add_filter("All Files", &["*"])
                            .save_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    Message::PlotStylePanelSavePath,
                )
    }
}
