use super::helpers::{
    ortho_constrain, parse_coord, polar_constrain_near, ucs_rotate_vec, ucs_to_wcs, ucs_z_axis,
    CoordKind,
};
use super::{Message, OpenCADStudio, POLY_START_DELAY_MS};
use crate::modules::ModuleEvent;
use crate::scene::pick::grip::{find_hit_grip, find_hit_grip_paper, GripEdit};
use crate::scene::model::object::GripApply;
use crate::scene::{
    self, hover_id, Scene, TileEdgeOrient, VIEWCUBE_DRAW_PX, VIEWCUBE_PAD, VIEWCUBE_PX,
};
use crate::ui::PropertiesPanel;
use acadrust::types::Color as AcadColor;
use acadrust::{EntityType as AcadEntityType, Handle};
use iced::time::Instant;
use iced::{mouse, Task};

/// Keystroke-derived messages that an open modal dialog must swallow so the
/// keyboard can't reach the main window (command line, F-key toggles, edit
/// shortcuts) while a dialog is up. `CommandEscape` is handled separately (it
/// closes the modal); a modal's own text fields emit their own messages, which
/// are not in this set. See [`OpenCADStudio::update`] and #126.
fn is_modal_blocked_key_msg(msg: &Message) -> bool {
    matches!(
        msg,
        Message::CommandInput(_)
            | Message::CommandAppendChar(_)
            | Message::CommandSpace
            | Message::CommandFinalize
            | Message::CommandBackspace
            | Message::CommandHistoryPrev
            | Message::CommandHistoryNext
            | Message::DynTabNext
            | Message::MTextCaretMove(_)
            | Message::DeleteSelected
            | Message::ToggleSnapEnabled
            | Message::ToggleGrid
            | Message::ToggleOrtho
            | Message::ToggleGridSnap
            | Message::TogglePolar
            | Message::ToggleOTrack
            | Message::ToggleDynInput
            | Message::TabNew
            | Message::OpenFile
            | Message::SaveFile
            | Message::SaveAs
            | Message::Undo
            | Message::Redo
    )
}

const VIEWCUBE_HIT_SIZE: f32 = VIEWCUBE_DRAW_PX;
/// Pixel distance from a Model-tile inner divider that still registers as
/// a resize grip on the press.
const TILE_EDGE_HIT_PX: f32 = 4.0;
/// Normalized minimum tile size before `collapse_small_model_tiles` merges
/// it into a neighbour. Sized to comfortably contain the ViewCube + its
/// padding so a tile that's still GPU-rendered always has room to show
/// the gizmo.
fn tile_min_norm(canvas_w: f32, canvas_h: f32) -> (f32, f32) {
    let px = VIEWCUBE_DRAW_PX + 2.0 * VIEWCUBE_PAD + 16.0;
    (
        (px / canvas_w.max(1.0)).min(0.95),
        (px / canvas_h.max(1.0)).min(0.95),
    )
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

impl OpenCADStudio {
    /// Close the active in-canvas modal (Plan B), mirroring what closing the
    /// old OS window did: a style editor discards its staged (un-applied)
    /// changes, and the ribbon tool that launched the dialog is de-highlighted.
    fn close_active_modal(&mut self) {
        use super::ModalKind::*;
        if matches!(
            self.active_modal,
            Some(TextStyle | DimStyle | TableStyle | MLeaderStyle | MlStyle)
        ) {
            self.style_stage_discard();
        }
        match self.active_modal {
            Some(Layers) => self.ribbon.deactivate_tool_if("LAYERS"),
            Some(PageSetup) => self.ribbon.deactivate_tool_if("PAGESETUP"),
            Some(TextStyle) => {
                self.ribbon.deactivate_tool_if("STYLE");
                self.ribbon.deactivate_tool_if("TEXTSTYLE");
            }
            Some(TableStyle) => self.ribbon.deactivate_tool_if("TABLESTYLE"),
            Some(MlStyle) => self.ribbon.deactivate_tool_if("MLSTYLE"),
            Some(MLeaderStyle) => self.ribbon.deactivate_tool_if("MLEADERSTYLE"),
            Some(LayoutManager) => {
                self.ribbon.deactivate_tool_if("LAYOUTMANAGER");
                self.ribbon.deactivate_tool_if("LAYOUTPANEL");
            }
            Some(Plotstyle) => {
                self.ribbon.deactivate_tool_if("PLOTSTYLE");
                self.ribbon.deactivate_tool_if("STYLESMANAGER");
            }
            Some(DimStyle) => self.ribbon.deactivate_tool_if("DIMSTYLE"),
            Some(Shortcuts) => {
                self.ribbon.deactivate_tool_if("SHORTCUTS");
                self.ribbon.deactivate_tool_if("KEYBOARD");
            }
            Some(About) => self.ribbon.deactivate_tool_if("ABOUT"),
            // Dismissing these via ✕ is the cancel/decline path.
            Some(Unsaved) => self.pending_close = None,
            Some(AssocPrompt) => self.mark_assoc_prompted(),
            _ => {}
        }
        self.active_modal = None;
        // Recentre the next dialog and drop any in-progress drag.
        self.modal_offset = iced::Vector::ZERO;
        self.modal_drag_last = None;
        self.modal_dragging = false;
    }

    pub fn update(&mut self, msg: Message) -> Task<Message> {
        // A modal dialog must capture the keyboard the same way it already
        // captures the mouse. Otherwise keystrokes from the global key
        // subscription leak past the modal into the command line and fire as
        // commands once the dialog closes. While a modal is open, Escape
        // closes it and every other keystroke-derived message is swallowed;
        // the modal's own text fields keep working because they emit their own
        // (non-blocked) messages. (#126)
        if self.active_modal.is_some() {
            if matches!(msg, Message::CommandEscape) {
                return self.update(Message::CloseModal);
            }
            if is_modal_blocked_key_msg(&msg) {
                return Task::none();
            }
        }
        let task = self.update_inner(msg);
        // After every message, mirror the active command step's prompt so
        // its history line stays pinned (non-fading) until the step changes.
        let prompt = self.tabs[self.active_tab]
            .active_cmd
            .as_ref()
            .map(|c| c.prompt());
        self.command_line.set_step_prompt(prompt);
        // Persist UI preferences whenever a toggle changes them (issue #68).
        self.persist_settings_if_changed();
        // OTRACK acquires tracking points only while a command or grip drag is
        // running; drop them once neither is active so the temporary tracking
        // points / vectors disappear when the command ends (issue #64).
        let i = self.active_tab;
        if self.tabs[i].active_cmd.is_none()
            && self.tabs[i].active_grip.is_none()
            && !self.snapper.tracking_points.is_empty()
        {
            self.snapper.clear_tracking();
            self.otrack_active = None;
        }
        task
    }

    /// Drop the OTRACK acquired points and the live alignment vector once a
    /// point has been committed to the active command. Temporary tracking
    /// points are reset on every input so they don't pile up across a
    /// multi-point command and overwhelm the next pick (issue #85).
    fn reset_tracking_after_point(&mut self) {
        self.snapper.clear_tracking();
        self.otrack_active = None;
    }

    /// Snapshot the persisted UI preferences from live state.
    pub(super) fn current_settings(&self) -> super::settings::UserSettings {
        super::settings::UserSettings {
            dyn_input: self.dyn_input,
            ortho: self.ortho_mode,
            polar: self.polar_mode,
            polar_increment_deg: self.polar_increment_deg,
            show_grid: self.show_grid,
            snap_enabled: self.snapper.snap_enabled,
            otrack: self.snapper.otrack_enabled,
            snap_modes: super::settings::UserSettings::modes_from(self.snapper.enabled.iter()),
            default_assoc_prompted: self.default_assoc_prompted,
            disabled_plugins: {
                let mut v: Vec<String> = self.disabled_plugins.iter().cloned().collect();
                v.sort();
                v
            },
            plugin_repos: self.plugin_repos.clone(),
        }
    }

    /// Apply restored preferences to live state.
    pub(super) fn apply_settings(&mut self, s: &super::settings::UserSettings) {
        self.dyn_input = s.dyn_input;
        self.ortho_mode = s.ortho;
        self.polar_mode = s.polar;
        self.polar_increment_deg = s.polar_increment_deg;
        self.show_grid = s.show_grid;
        self.snapper.snap_enabled = s.snap_enabled;
        self.snapper.otrack_enabled = s.otrack;
        self.snapper.enabled = s.snap_modes.iter().copied().collect();
        self.default_assoc_prompted = s.default_assoc_prompted;
        self.disabled_plugins = s.disabled_plugins.iter().cloned().collect();
        self.plugin_repos = s.plugin_repos.clone();
        self.rebuild_ribbon_modules();
    }

    /// Rebuild the ribbon's tab list from the registry, dropping the tabs of any
    /// disabled plugins. Call after `disabled_plugins` changes.
    pub(super) fn rebuild_ribbon_modules(&mut self) {
        let modules =
            crate::plugin::ribbon_modules_enabled(&self.disabled_plugins);
        self.ribbon.set_modules(modules);
    }

    /// Snapshot of disabled plugin ids — lets the registry skip them while it
    /// holds a `&mut` borrow of the app via `HostSession`.
    pub(crate) fn disabled_plugin_ids(&self) -> rustc_hash::FxHashSet<String> {
        self.disabled_plugins.clone()
    }

    /// Background task: fetch the curated plugin registry.
    #[cfg(not(target_arch = "wasm32"))]
    fn fetch_registry_task(&self) -> Task<Message> {
        Task::perform(
            async { crate::plugin::marketplace::fetch_registry() },
            Message::PluginRegistryFetched,
        )
    }

    /// Background task: fetch `owner/repo`'s installable release tags.
    #[cfg(not(target_arch = "wasm32"))]
    fn fetch_releases_task(&self, repo: String) -> Task<Message> {
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
    fn fetch_releases_task(&self, _repo: String) -> Task<Message> {
        Task::none()
    }

    /// Background task: download and install the `tag` release of `owner/repo`.
    #[cfg(not(target_arch = "wasm32"))]
    fn install_task(&self, repo: String, tag: String) -> Task<Message> {
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
    fn install_task(&self, _repo: String, _tag: String) -> Task<Message> {
        Task::none()
    }

    /// Write PDSIZE from the dialog buffer with the current relative/absolute
    /// sign. A relative size is stored negative; absolute positive. Switching to
    /// absolute with an empty/zero size seeds a positive value from the current
    /// on-screen size so the point stays representable (PDSIZE 0 always reads as
    /// relative, so absolute needs a non-zero magnitude).
    fn apply_point_size(&mut self) {
        let i = self.active_tab;
        let mut mag = self
            .point_size_buf
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0)
            .abs();
        if !self.point_size_relative && mag == 0.0 {
            let wpp = self.tabs[i].scene.world_per_pixel().unwrap_or(0.0);
            mag = if wpp > 0.0 {
                crate::entities::point::relative_world_size(0.0, wpp)
            } else {
                1.0
            };
            self.point_size_buf = format!("{mag:.4}");
        }
        let next = if self.point_size_relative { -mag } else { mag };
        self.push_undo_snapshot(i, "PDSIZE");
        self.tabs[i].scene.document.header.point_display_size = next;
        self.tabs[i].scene.bump_geometry();
        self.tabs[i].dirty = true;
    }

    /// Replace the `mask` bits of PDMODE with `value`, rebuild the point glyphs
    /// and mark the document dirty. Used by the Point Style (DDPTYPE) dialog.
    fn set_point_mode_bits(&mut self, mask: i16, value: i16) {
        let i = self.active_tab;
        let cur = self.tabs[i].scene.document.header.point_display_mode;
        let next = (cur & !mask) | (value & mask);
        if next == cur {
            return;
        }
        self.push_undo_snapshot(i, "PDMODE");
        self.tabs[i].scene.document.header.point_display_mode = next;
        self.tabs[i].scene.bump_geometry();
        self.tabs[i].dirty = true;
    }

    /// Write preferences to disk only when they differ from the last write,
    /// so a toggle persists immediately without thrashing the file.
    fn persist_settings_if_changed(&mut self) {
        let cur = self.current_settings();
        if self.last_saved_settings.as_ref() != Some(&cur) {
            cur.save();
            self.last_saved_settings = Some(cur);
        }
    }

    /// Record that the one-time default-association prompt has been answered and
    /// flush it to disk, so the dialog never reappears on later launches.
    fn mark_assoc_prompted(&mut self) {
        self.default_assoc_prompted = true;
        self.persist_settings_if_changed();
    }

    fn update_inner(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Tick(t) => {
                let i = self.active_tab;
                self.tabs[i].scene.update(t - self.start);

                // If the camera moved since we last synced, write it back to
                // the document and mark the file dirty.
                let gen = self.tabs[i].scene.camera_generation;
                if gen != self.tabs[i].last_synced_camera_gen {
                    self.tabs[i].last_synced_camera_gen = gen;
                    if self.tabs[i].scene.sync_camera_to_document() {
                        self.tabs[i].dirty = true;
                    }
                }

                Task::none()
            }

            Message::OpenFile => {
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
                    self.opening = Some(super::OpenProgress {
                        name: "Opening…".into(),
                        size_bytes: 0,
                        phase: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                            super::OPEN_PHASE_READING,
                        )),
                        started: Instant::now(),
                    });
                    Task::perform(crate::io::pick_and_load_web(), Message::FileOpened)
                }
            }

            Message::OpenPathPicked(None) => Task::none(),

            Message::OpenRecent(path) => {
                // Recents are read from disk every save → the path may be
                // stale. Skip silently if the file no longer exists; the
                // entry stays in the list so the user can clean it up.
                match std::fs::metadata(&path) {
                    Ok(m) => self.update(Message::OpenPathPicked(Some((path, m.len())))),
                    Err(_) => {
                        self.command_line.push_error(&format!(
                            "Recent file no longer exists: {}",
                            path.display()
                        ));
                        Task::none()
                    }
                }
            }

            Message::RecentRemove(path) => {
                self.app_menu.remove_recent(&path);
                Task::none()
            }

            Message::OpenPathPicked(Some((path, size_bytes))) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".into());
                let phase = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                    super::OPEN_PHASE_READING,
                ));
                self.opening = Some(super::OpenProgress {
                    name: name.clone(),
                    size_bytes,
                    phase: phase.clone(),
                    started: Instant::now(),
                });
                let size_label = format_size(size_bytes);
                self.command_line
                    .push_info(&format!("Opening \"{name}\" ({size_label})…"));
                Task::perform(
                    crate::io::open_path_with_phase(path, phase),
                    Message::FileOpened,
                )
            }

            Message::OpenCancel => {
                if let Some(p) = self.opening.take() {
                    self.command_line
                        .push_info(&format!("Open cancelled: \"{}\"", p.name));
                }
                Task::none()
            }

            Message::FileOpened(Ok((name, path, doc, caches))) => {
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
                    let new_tab = super::document::DocumentTab::new_drawing(self.tab_counter);
                    self.tabs.push(new_tab);
                    let idx = self.tabs.len() - 1;
                    self.active_tab = idx;
                    idx
                };

                self.tabs[i].current_path = Some(path.clone());
                self.tabs[i].scene.document = doc;
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
                self.tabs[i].scene.world_offset = caches.world_offset;
                self.tabs[i].scene.local_extent_max = caches.local_extent_max;
                self.tabs[i].scene.hatches = caches.hatches;
                self.tabs[i].scene.images = caches.images;
                self.tabs[i].scene.meshes = caches.meshes;
                // Invalidate the wire cache so the new document is tessellated.
                self.tabs[i].scene.bump_geometry();
                self.tabs[i].scene.selected = rustc_hash::FxHashSet::default();
                self.tabs[i].scene.preview_wires = vec![];
                self.tabs[i].scene.current_layout = "Model".to_string();
                crate::linetypes::populate_document(&mut self.tabs[i].scene.document);
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
                self.sync_render_mode_to_active_tile(i);
                self.tabs[i].last_synced_camera_gen = self.tabs[i].scene.camera_generation;
                self.tabs[i].dirty = false;
                self.tabs[i].history = super::document::HistoryState::default();
                self.refresh_selected_grips();
                Task::none()
            }

            Message::FileOpened(Err(e)) => {
                // If the user cancelled, the overlay was already cleared and
                // we suppress the noise.
                let was_open = self.opening.take().is_some();
                if was_open && e != "Cancelled" {
                    self.command_line.push_error(&format!("Open failed: {e}"));
                }
                Task::none()
            }

            Message::ImagePick => {
                Task::perform(crate::io::pick_image_file(), Message::ImagePickResult)
            }

            Message::ImagePickResult(Ok((path, pw, ph))) => {
                use crate::command::CadCommand;
                use crate::modules::draw::draw::raster_image::ImageCommand;
                let path_str = path.to_string_lossy().into_owned();
                let short = std::path::Path::new(&path_str)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path_str)
                    .to_string();
                self.command_line
                    .push_output(&format!("IMAGE  \"{short}\": {pw}×{ph} px"));
                let cmd = ImageCommand::new(path_str, pw, ph);
                let i = self.active_tab;
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
                Task::none()
            }

            Message::ImagePickResult(Err(e)) => {
                if e != "Cancelled" {
                    self.command_line.push_error(&format!("IMAGE: {e}"));
                }
                Task::none()
            }

            Message::XAttachPick => Task::perform(
                async {
                    let handle = rfd::AsyncFileDialog::new()
                        .set_title("Select External Reference File")
                        .add_filter("CAD Files", &["dwg", "dxf", "DWG", "DXF"])
                        .add_filter("DWG Files", &["dwg", "DWG"])
                        .add_filter("DXF Files", &["dxf", "DXF"])
                        .pick_file()
                        .await;
                    match handle {
                        Some(h) => Ok(crate::sys::handle_path(&h)),
                        None => Err("Cancelled".to_string()),
                    }
                },
                Message::XAttachPickResult,
            ),

            Message::XAttachPickResult(Ok(path)) => {
                use crate::command::CadCommand;
                use crate::modules::insert::xattach::XAttachCommand;
                let path_str = path.to_string_lossy().into_owned();
                let cmd = XAttachCommand::with_path(path_str);
                let i = self.active_tab;
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
                Task::none()
            }

            Message::XAttachPickResult(Err(e)) => {
                if e != "Cancelled" {
                    self.command_line.push_error(&format!("XATTACH: {e}"));
                }
                Task::none()
            }

            Message::WblockSave(block_name) => {
                let name = block_name.clone();
                Task::perform(
                    async move {
                        let path = rfd::AsyncFileDialog::new()
                            .set_title("Save Block As")
                            .set_file_name("block.dwg")
                            .add_filter("DWG Files", &["dwg"])
                            .save_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h));
                        (name, path)
                    },
                    |(name, path)| Message::WblockSaveResult(name, path),
                )
            }

            Message::WblockSaveResult(block_name, Some(path)) => {
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

            Message::WblockSaveResult(_, None) => Task::none(),

            Message::DataExtractionSave(csv) => {
                let csv_clone = csv.clone();
                Task::perform(
                    async move {
                        let path = rfd::AsyncFileDialog::new()
                            .set_title("Save Data Extraction")
                            .set_file_name("extraction.csv")
                            .add_filter("CSV", &["csv"])
                            .add_filter("All Files", &["*"])
                            .save_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h));
                        (csv_clone, path)
                    },
                    |(csv, path)| Message::DataExtractionSaveResult(csv, path),
                )
            }

            Message::DataExtractionSaveResult(csv, Some(path)) => {
                match std::fs::write(&path, csv.as_bytes()) {
                    Ok(()) => {
                        let rows = csv.lines().count().saturating_sub(1);
                        let fname = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.to_string_lossy().into_owned());
                        self.command_line
                            .push_output(&format!("DATAEXTRACTION  {rows} rows → \"{fname}\""));
                    }
                    Err(e) => self
                        .command_line
                        .push_error(&format!("DATAEXTRACTION: write failed: {e}")),
                }
                Task::none()
            }

            Message::DataExtractionSaveResult(_, None) => Task::none(),

            Message::StlExport => {
                let i = self.active_tab;
                if self.tabs[i].scene.meshes.is_empty() {
                    self.command_line
                        .push_error("STLOUT: no 3D mesh data in this drawing.");
                    return Task::none();
                }
                Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Export STL")
                            .set_file_name("export.stl")
                            .add_filter("STL Files", &["stl"])
                            .add_filter("All Files", &["*"])
                            .save_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    Message::StlExportPath,
                )
            }

            Message::StlExportPath(Some(path)) => {
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

            Message::StlExportPath(None) => Task::none(),

            // ── STEP AP203 export ─────────────────────────────────────────
            Message::StepExport => {
                let i = self.active_tab;
                if self.tabs[i].scene.meshes.is_empty() {
                    self.command_line
                        .push_error("STEPOUT: no 3D mesh data in this drawing.");
                    return Task::none();
                }
                Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Export STEP AP203")
                            .set_file_name("export.step")
                            .add_filter("STEP Files", &["step", "stp"])
                            .add_filter("All Files", &["*"])
                            .save_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    Message::StepExportPath,
                )
            }

            Message::StepExportPath(Some(path)) => {
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

            Message::StepExportPath(None) => Task::none(),

            // ── OBJ import ────────────────────────────────────────────────
            Message::ObjImport => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Import OBJ Mesh")
                        .add_filter("Wavefront OBJ", &["obj", "OBJ"])
                        .add_filter("All Files", &["*"])
                        .pick_file()
                        .await
                        .map(|h| crate::sys::handle_path(&h))
                },
                Message::ObjImportPath,
            ),

            Message::ObjImportPath(Some(path)) => {
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

            Message::ObjImportPath(None) => Task::none(),

            Message::SaveFile => {
                let i = self.active_tab;
                // Native: save straight to the known path. Web has no path
                // (downloads instead), so always go through the Save dialog.
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = self.tabs[i].current_path.clone() {
                    self.tabs[i].scene.document.header.user_real1 =
                        self.tabs[i].scene.annotation_scale as f64;
                    match crate::io::save(&self.tabs[i].scene.document, &path) {
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

            Message::SaveAs => {
                let i = self.active_tab;
                self.save_dialog_for_unsaved = false;
                self.open_save_dialog_window(i)
            }

            Message::SaveDialogFormatChanged(fmt) => {
                let (ext, _) = crate::io::parse_save_format(&fmt);
                let stem = std::path::Path::new(&self.save_dialog_filename)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "drawing".to_string());
                self.save_dialog_filename = format!("{stem}.{ext}");
                self.save_dialog_format = fmt;
                Task::none()
            }

            Message::SaveDialogFilenameChanged(name) => {
                self.save_dialog_filename = name;
                Task::none()
            }

            Message::SaveDialogNavigate(path) => {
                self.save_dialog_folder = path.clone();
                self.save_dialog_entries = crate::io::read_dir_entries(&path);
                Task::none()
            }

            Message::SaveDialogEntryClicked(path, is_dir) => {
                if is_dir {
                    self.save_dialog_folder = path.clone();
                    self.save_dialog_entries = crate::io::read_dir_entries(&path);
                } else {
                    // Fill filename from clicked file.
                    if let Some(name) = path.file_name() {
                        self.save_dialog_filename = name.to_string_lossy().into_owned();
                    }
                }
                Task::none()
            }

            Message::SaveDialogConfirm => {
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
                        if let Some(super::PendingClose::Tab(idx)) = self.pending_close.take() {
                            let cont = self.update(Message::TabClose(idx));
                            return Task::batch([close, cont]);
                        }
                    }
                    close
                }
            }

            Message::SaveDialogCancel => self.close_save_dialog_window(),

            Message::ClearScene => {
                let i = self.active_tab;
                self.push_undo_snapshot(i, "CLEAR");
                self.tabs[i].scene.clear();
                crate::linetypes::populate_document(&mut self.tabs[i].scene.document);
                self.tabs[i].properties = PropertiesPanel::empty();
                let doc_layers = self.tabs[i].scene.document.layers.clone();
                let vp_info = self.tabs[i].scene.viewport_list();
                self.tabs[i]
                    .layers
                    .sync_with_viewports(&doc_layers, vp_info);
                self.command_line
                    .push_output("Scene cleared. Standard linetypes loaded.");
                self.tabs[i].current_path = None;
                self.tabs[i].dirty = true;
                self.sync_ribbon_layers();
                Task::none()
            }

            Message::SetWireframe(w) => {
                // Back-compat shim: forward to the new render-mode path so
                // the ribbon button + WIREFRAME / SOLID command line still
                // work without duplicating the rendering plumbing.
                let mode = if w {
                    acadrust::entities::ViewportRenderMode::Wireframe2D
                } else {
                    acadrust::entities::ViewportRenderMode::FlatShaded
                };
                Task::done(Message::SetRenderMode(mode))
            }

            Message::SetRenderMode(mode) => {
                use acadrust::entities::ViewportRenderMode as M;
                let i = self.active_tab;
                let label = match mode {
                    M::Wireframe2D => "Wireframe 2D",
                    M::Wireframe3D => "Wireframe 3D",
                    M::HiddenLine => "Hidden Line",
                    M::FlatShaded => "Flat Shaded",
                    M::GouraudShaded => "Gouraud Shaded",
                    M::FlatShadedWithEdges => "Flat Shaded + Edges",
                    M::GouraudShadedWithEdges => "Gouraud Shaded + Edges",
                };
                // In a paper layout with an active (double-clicked)
                // viewport, the picker drives that viewport entity's own
                // render mode; the model-layout tab style is untouched.
                if self.tabs[i].scene.set_active_viewport_render_mode(mode) {
                    self.tabs[i].scene.bump_geometry();
                    self.command_line
                        .push_output(&format!("Viewport visual style: {label}"));
                    return Task::none();
                }
                self.tabs[i].render_mode = mode;
                // Write the style onto the active Model tile alone so it
                // sticks when that tile loses focus and the other tiles keep
                // their own styles.
                self.tabs[i].scene.set_active_model_tile_render_mode(mode);
                // Keep the legacy `wireframe` bool synced — both wireframe
                // modes set it, everything else clears it.
                let wf = matches!(mode, M::Wireframe2D | M::Wireframe3D);
                self.tabs[i].wireframe = wf;
                self.ribbon.set_wireframe(wf);
                self.tabs[i].visual_style = label.into();
                // Re-upload face3d fills on the next frame — the render
                // pipeline keys its upload cache off `geometry_epoch`.
                self.tabs[i].scene.bump_geometry();
                self.command_line
                    .push_output(&format!("Visual style: {label}"));
                Task::none()
            }

            Message::SetProjection(ortho) => {
                use crate::scene::Projection;
                let proj = if ortho {
                    Projection::Orthographic
                } else {
                    Projection::Perspective
                };
                let i = self.active_tab;
                self.tabs[i].scene.camera.borrow_mut().projection = proj;
                self.tabs[i].scene.camera_generation += 1;
                self.ribbon.set_ortho(ortho);
                self.command_line.push_output(if ortho {
                    "Projection: Orthographic"
                } else {
                    "Projection: Perspective"
                });
                Task::none()
            }

            Message::RibbonSelectTab(idx) => {
                self.ribbon.select(idx);
                Task::none()
            }

            Message::RibbonToolClick { tool_id, event } => {
                self.ribbon.activate_tool(&tool_id);
                match event {
                    ModuleEvent::Command(cmd) => return self.dispatch_command(&cmd),
                    ModuleEvent::OpenFileDialog => {
                        self.command_line
                            .push_info("Open DWG/DXF: not yet implemented.");
                    }
                    ModuleEvent::ClearModels => {
                        let i = self.active_tab;
                        self.tabs[i].scene.clear();
                        self.tabs[i].properties = PropertiesPanel::empty();
                        self.command_line.push_output("Scene cleared.");
                    }
                    ModuleEvent::SetWireframe(w) => {
                        let i = self.active_tab;
                        self.tabs[i].wireframe = w;
                        self.ribbon.set_wireframe(w);
                        self.tabs[i].visual_style = if w {
                            "Wireframe".into()
                        } else {
                            "Shaded".into()
                        };
                        self.command_line.push_output(if w {
                            "Visual style: Wireframe"
                        } else {
                            "Visual style: Shaded"
                        });
                    }
                    ModuleEvent::ToggleLayers => {
                        return Task::done(Message::ToggleLayers);
                    }
                    ModuleEvent::PluginFileDialog {
                        command,
                        title,
                        filter_name,
                        extensions,
                    } => {
                        return Task::perform(
                            async move {
                                let exts: Vec<&str> =
                                    extensions.iter().map(|s| s.as_str()).collect();
                                let path = rfd::AsyncFileDialog::new()
                                    .set_title(title)
                                    .add_filter(filter_name, &exts)
                                    .add_filter("All Files", &["*"])
                                    .pick_file()
                                    .await
                                    .map(|h| crate::sys::handle_path(&h));
                                (command, path)
                            },
                            |(command, path)| Message::PluginFileDialogResult { command, path },
                        );
                    }
                }
                Task::none()
            }
            Message::PluginFileDialogResult { command, path } => {
                if let Some(path) = path {
                    // Dispatch "<command> <path>" with original case intact —
                    // the command line would upper-case the whole string and
                    // mangle case-sensitive paths on Linux/macOS.
                    let line = format!("{} {}", command, path.to_string_lossy());
                    let i = self.active_tab;
                    if !crate::plugin::try_dispatch(self, i, &line) {
                        self.command_line
                            .push_error(&format!("No plugin handled: {command}"));
                    }
                }
                Task::none()
            }

            // ── Application menu ──────────────────────────────────────────
            Message::ToggleAppMenu => {
                self.app_menu.toggle();
                Task::none()
            }
            Message::CloseAppMenu => {
                self.app_menu.close();
                Task::none()
            }
            Message::CloseAppMenuAndRun(cmd) => {
                self.app_menu.close();
                self.dispatch_command(&cmd.clone())
            }
            Message::AppMenuSearch(s) => {
                self.app_menu.search = s;
                Task::none()
            }

            // ── Document tabs ─────────────────────────────────────────────
            Message::TabNew => {
                self.tab_counter += 1;
                let new_tab = super::document::DocumentTab::new_drawing(self.tab_counter);
                self.tabs.push(new_tab);
                self.active_tab = self.tabs.len() - 1;
                self.sync_ribbon_layers();
                self.sync_ribbon_styles();
                // #21: reset ribbon Color / Linetype / Lineweight to the
                // fresh tab's defaults (ByLayer) instead of inheriting the
                // previous tab's last selection.
                self.sync_ribbon_from_selection();
                Task::none()
            }

            Message::TabSwitch(idx) => {
                if idx < self.tabs.len() {
                    self.active_tab = idx;
                    self.sync_ribbon_layers();
                    self.sync_ribbon_styles();
                    // #21: also re-seed ribbon Color / Linetype / Lineweight
                    // from the newly active tab so they reflect that doc's
                    // CECOLOR / CELTYPE / CELWEIGHT (or its current selection
                    // if there is one), not the prior tab's choice.
                    self.sync_ribbon_from_selection();
                }
                Task::none()
            }

            Message::TabClose(idx) => {
                // Start tab is fixed — close requests on it are no-ops.
                if self.tabs.get(idx).map_or(false, |t| t.is_start) {
                    return Task::none();
                }
                if self.tabs.get(idx).map_or(false, |t| t.dirty) {
                    self.pending_close = Some(super::PendingClose::Tab(idx));
                    return self.open_unsaved_dialog_window();
                }
                // Only-tab case: when the lone non-start tab closes, fall
                // back to the Start tab if it exists; otherwise spawn a
                // fresh blank drawing (legacy behaviour).
                if self.tabs.len() == 1 {
                    self.tab_counter += 1;
                    self.tabs[0] = super::document::DocumentTab::new_drawing(self.tab_counter);
                    self.active_tab = 0;
                } else {
                    self.tabs.remove(idx);
                    if self.active_tab >= self.tabs.len() {
                        self.active_tab = self.tabs.len() - 1;
                    }
                }
                // The active tab is now either a brand-new blank or a
                // different existing tab; in both cases the ribbon needs
                // to track that doc's defaults / selection. #21.
                self.sync_ribbon_layers();
                self.sync_ribbon_styles();
                self.sync_ribbon_from_selection();
                Task::none()
            }

            Message::CommandInput(s) => {
                // Space submits the current input the same way Enter does
                // (CAD convention) — unless the active command is collecting
                // free-form text (TEXT / MTEXT / DDEDIT / attribute value
                // prompts) where Space must reach the buffer as a literal
                // character. `wants_text_with_spaces()` flags those prompts.
                let i = self.active_tab;
                let allow_literal_space = self.tabs[i]
                    .active_cmd
                    .as_ref()
                    .map(|c| c.wants_text_input() && c.wants_text_with_spaces())
                    .unwrap_or(false);
                if !allow_literal_space && s.ends_with(' ') {
                    self.command_line.input = s.trim_end_matches(' ').to_string();
                    return Task::done(Message::CommandSubmit);
                }
                self.command_line.input = s;
                // Typing invalidates the previous arrow-key cursor —
                // the matches list has likely changed.
                self.command_line.autocomplete_cursor = None;
                Task::none()
            }

            Message::CommandAppendChar(s) => {
                // While the MText preview is up, typed glyphs edit it directly.
                if self.mtext_editor.as_ref().is_some_and(|e| e.show_preview) {
                    if s.chars().all(|c| !c.is_control()) {
                        self.mtext_type(&s);
                    }
                    return Task::none();
                }
                // Filter out control characters — only push the typed
                // glyph(s). `Tab`, etc. arrive as Named keys, not here.
                if s.chars().all(|c| !c.is_control()) {
                    let i = self.active_tab;
                    // `,` is the coordinate separator in dynamic input,
                    // not a decimal point: typing it locks the current
                    // field's buffer and advances to the next coordinate,
                    // reshaping the field set when going polar → cartesian
                    // (Distance → X, Y) or 2-D → 3-D (X, Y → X, Y, Z).
                    // See #35.
                    if s == "," && self.dyn_input && !self.tabs[i].dyn_fields.is_empty() {
                        self.dyn_comma_advance();
                        self.command_line.autocomplete_cursor = None;
                        return self.focus_cmd_input();
                    }
                    // While dynamic input is showing fields, numeric and
                    // expression glyphs edit the focused field instead of
                    // the command line. Letters still go to the command line
                    // so command-option keywords keep working.
                    let dyn_field_char = !s.is_empty()
                        && s.chars().all(|c| {
                            c.is_ascii_digit()
                                || matches!(c, '.' | '-' | '+' | '*' | '/' | '^' | '%' | '(' | ')')
                        });
                    if dyn_field_char && self.dyn_input && !self.tabs[i].dyn_fields.is_empty() {
                        let a = self.tabs[i]
                            .dyn_active
                            .min(self.tabs[i].dyn_fields.len() - 1);
                        self.tabs[i].dyn_fields[a]
                            .buffer
                            .get_or_insert_with(String::new)
                            .push_str(&s);
                    } else {
                        self.command_line.input.push_str(&s);
                    }
                }
                self.command_line.autocomplete_cursor = None;
                self.focus_cmd_input()
            }

            Message::CommandBackspace => {
                if self.mtext_editor.as_ref().is_some_and(|e| e.show_preview) {
                    self.mtext_backspace();
                    return Task::none();
                }
                let i = self.active_tab;
                // Backspace edits the focused dynamic-input field first;
                // emptying it unlocks the field (back to cursor tracking).
                if self.dyn_input && !self.tabs[i].dyn_fields.is_empty() {
                    let a = self.tabs[i]
                        .dyn_active
                        .min(self.tabs[i].dyn_fields.len() - 1);
                    if let Some(buf) = self.tabs[i].dyn_fields[a].buffer.as_mut() {
                        buf.pop();
                        if buf.is_empty() {
                            self.tabs[i].dyn_fields[a].buffer = None;
                        }
                        return self.focus_cmd_input();
                    }
                }
                self.command_line.input.pop();
                self.command_line.autocomplete_cursor = None;
                self.focus_cmd_input()
            }

            Message::DynTabNext if self.grip_popup.is_some() => {
                if let Some(popup) = self.grip_popup.as_mut() {
                    if !popup.items.is_empty() {
                        popup.selected = (popup.selected + 1) % popup.items.len();
                    }
                }
                Task::none()
            }

            Message::DynTabNext => {
                let i = self.active_tab;
                let n = self.tabs[i].dyn_fields.len();
                if n > 0 {
                    self.tabs[i].dyn_active = (self.tabs[i].dyn_active + 1) % n;
                }
                self.focus_cmd_input()
            }

            Message::SplitModelViewport(horizontal) => {
                let i = self.active_tab;
                self.tabs[i].scene.split_active_model_tile(horizontal);
                self.tabs[i].scene.camera_generation += 1;
                Task::none()
            }

            Message::CommandHistoryPrev => {
                // Grip popup wins first — arrow keys walk its items.
                if let Some(popup) = self.grip_popup.as_mut() {
                    if !popup.items.is_empty() {
                        popup.selected = if popup.selected == 0 {
                            popup.items.len() - 1
                        } else {
                            popup.selected - 1
                        };
                    }
                    return Task::none();
                }
                // While autocomplete is showing suggestions, ↑ walks up
                // that list. Otherwise it falls back to recall history.
                let i = self.active_tab;
                if self.tabs[i].active_cmd.is_none() && self.command_line.autocomplete_prev() {
                    return Task::none();
                }
                self.command_line.history_prev();
                Task::none()
            }

            Message::CommandHistoryNext => {
                if let Some(popup) = self.grip_popup.as_mut() {
                    if !popup.items.is_empty() {
                        popup.selected = (popup.selected + 1) % popup.items.len();
                    }
                    return Task::none();
                }
                let i = self.active_tab;
                if self.tabs[i].active_cmd.is_none() && self.command_line.autocomplete_next() {
                    return Task::none();
                }
                self.command_line.history_next();
                Task::none()
            }

            Message::CommandHistoryToggle => {
                self.command_line.toggle_history();
                Task::none()
            }

            Message::CommandSuggestionPick(cmd) => {
                self.command_line.input.clear();
                self.command_line.close_history();
                self.dispatch_command(&cmd)
            }

            Message::CommandSubmit => {
                // Submitting a command implicitly dismisses the history
                // dropdown so the dispatched command's new prompt is
                // immediately visible on the overlay.
                self.command_line.close_history();
                // Grip-menu value prompt — consume the typed number and
                // route it through `apply_grip_menu_value`.
                if let Some(pending) = self.grip_pending.take() {
                    let raw = super::expr_eval::eval_to_string(self.command_line.input.trim());
                    self.command_line.input.clear();
                    let Ok(v) = raw.parse::<f64>() else {
                        self.command_line.push_error(&format!(
                            "{}: expected a number, got \"{raw}\"",
                            pending.label
                        ));
                        return Task::none();
                    };
                    let i = self.active_tab;
                    use crate::entities::traits::EntityTypeOps;
                    self.push_undo_snapshot(i, pending.label);
                    if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(pending.handle)
                    {
                        entity.apply_grip_menu_value(pending.grip_id, pending.action, v);
                    }
                    self.tabs[i].scene.bump_geometry();
                    self.tabs[i].dirty = true;
                    self.refresh_selected_grips();
                    self.refresh_properties();
                    return Task::none();
                }
                // Interactive VPORTS: the entry after a bare `VPORTS` is the
                // tiled configuration. Empty input defaults to SINGLE.
                if self.awaiting_vports {
                    self.awaiting_vports = false;
                    let cfg = self.command_line.input.trim().to_string();
                    self.command_line.input.clear();
                    let cfg = if cfg.is_empty() {
                        "SINGLE".to_string()
                    } else {
                        cfg
                    };
                    return self.dispatch_command(&format!("VPORTS {cfg}"));
                }
                // If the user navigated the autocomplete list with the
                // arrow keys, Enter dispatches the highlighted command
                // rather than the partial text actually in the buffer.
                let i_tab = self.active_tab;
                if self.tabs[i_tab].active_cmd.is_none() {
                    if let Some(picked) = self.command_line.selected_suggestion() {
                        let cmd = picked.to_string();
                        self.command_line.input.clear();
                        self.command_line.autocomplete_cursor = None;
                        return self.dispatch_command(&cmd);
                    }
                }
                let i = self.active_tab;
                // With the command line empty, a typed dynamic-input value
                // commits as a point pick instead of an empty submit.
                if self.tabs[i].active_cmd.is_some() && self.command_line.input.trim().is_empty() {
                    if let Some(task) = self.try_dyn_commit() {
                        return task;
                    }
                }
                if self.tabs[i].active_cmd.is_some() {
                    let text = super::expr_eval::eval_to_string(self.command_line.input.trim());
                    self.command_line.input.clear();

                    // Offer the typed text to the command's option handler
                    // first (keywords like PLINE's A/L/C, a radius, …). If it
                    // consumes the text we're done; if it returns None the
                    // text falls through to the Enter / coordinate handling
                    // below, so a bare Enter still terminates and typed points
                    // still work when dynamic input is off. See #97.
                    if self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.wants_text_input())
                        .unwrap_or(false)
                    {
                        if let Some(result) = self.tabs[i]
                            .active_cmd
                            .as_mut()
                            .and_then(|c| c.on_text_input(&text))
                        {
                            return self.apply_cmd_result(result);
                        }
                    }

                    if text.is_empty() {
                        let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_enter());
                        if let Some(r) = result {
                            return self.apply_cmd_result(r);
                        }
                        return Task::none();
                    }

                    // OTRACK: while aligned to a tracking ray, a bare distance
                    // places the point along the ray from the tracking point
                    // (issue #69).
                    if let Some((base, dir)) = self.otrack_active {
                        if let Some(dist) = super::expr_eval::eval_number(text.trim()) {
                            let pt = base + dir * dist as f32;
                            self.last_point = Some(pt);
                            self.dyn_user_reshaped = false;
                            self.sync_dyn_fields();
                            self.reset_tracking_after_point();
                            let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_point(pt));
                            if let Some(r) = result {
                                let task = self.apply_cmd_result(r);
                                self.refresh_active_cmd_preview(i);
                                return task;
                            }
                            return Task::none();
                        }
                    }

                    if let Some((coord, kind)) = parse_coord(&text) {
                        // Command-line coordinates are absolute by default,
                        // independent of DYN: `@` forces relative, `#` forces
                        // absolute, and a bare value is absolute. (Relative-by-
                        // default lives in the DYN tooltip path — see
                        // `dyn_resolve_point` — matching AutoCAD, where the
                        // command line stays absolute regardless of DYN.)
                        let want_relative = matches!(kind, CoordKind::Relative);
                        let ucs = self.tabs[i].active_ucs.clone();
                        let wcs_pt = match (want_relative, self.last_point) {
                            (true, Some(base)) => {
                                // Offset from the last point, rotated by the
                                // UCS axes (no origin translation).
                                let offset = match &ucs {
                                    Some(u) => ucs_rotate_vec(coord, u),
                                    None => coord,
                                };
                                base + offset
                            }
                            _ => {
                                // Absolute: typed coordinates are in active UCS.
                                match &ucs {
                                    Some(u) => ucs_to_wcs(coord, u),
                                    None => coord,
                                }
                            }
                        };
                        self.last_point = Some(wcs_pt);
                        self.dyn_user_reshaped = false;
                        self.sync_dyn_fields();
                        self.reset_tracking_after_point();
                        let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_point(wcs_pt));
                        if let Some(r) = result {
                            let task = self.apply_cmd_result(r);
                            // The rubber-band preview that the command
                            // last published reflects the *previous*
                            // last_point — a typed coordinate doesn't
                            // fire a mouse-move, so re-run the preview
                            // hook now using the current cursor world
                            // pos so the next segment immediately starts
                            // from the just-committed point. See #32.
                            self.refresh_active_cmd_preview(i);
                            return task;
                        }
                        return Task::none();
                    }

                    if let Some(result) = self.tabs[i]
                        .active_cmd
                        .as_mut()
                        .and_then(|c| c.on_text_input(&text))
                    {
                        return self.apply_cmd_result(result);
                    }

                    self.command_line.push_error(&format!(
                        "Expected coordinates (x,y) or a number, got: \"{text}\""
                    ));
                    return self.focus_cmd_input();
                }
                if let Some(cmd) = self.command_line.submit() {
                    return self.dispatch_command(&cmd);
                }
                // Empty Enter / Space with no active command repeats the
                // last dispatched command — same shortcut `CommandFinalize`
                // already implements, mirrored here so the trailing-space
                // submit path goes through it too.
                if let Some(cmd) = self.tabs[i].last_cmd.clone() {
                    return self.dispatch_command(&cmd);
                }
                Task::none()
            }

            Message::CommandSpace => {
                // Space is a literal space inside the MText preview; otherwise
                // it finalises the active command like Enter.
                if self.mtext_editor.as_ref().is_some_and(|e| e.show_preview) {
                    self.mtext_type(" ");
                    return Task::none();
                }
                return self.update(Message::CommandFinalize);
            }
            Message::CommandFinalize => {
                // In the MText preview, Enter inserts a line break.
                if self.mtext_editor.as_ref().is_some_and(|e| e.show_preview) {
                    self.mtext_type("\n");
                    return Task::none();
                }
                // Grip popup open → Enter commits the highlighted item.
                if self.grip_popup.is_some() {
                    let idx = self.grip_popup.as_ref().map(|p| p.selected).unwrap_or(0);
                    return Task::done(Message::GripMenuPick(idx));
                }
                // Any typed command-line text must be submitted rather than
                // finalising. The focused-input Enter routes through
                // CommandSubmit, but when the field isn't focused — e.g. at
                // startup before the window grabs focus, or a command started
                // from the ribbon — its Enter arrives here. Forward a non-empty
                // buffer to the same submit path so typing a command name (or
                // an option keyword like "R") and pressing Enter works without
                // first clicking into the command line (issue #99).
                if !self.command_line.input.trim().is_empty() {
                    return self.update(Message::CommandSubmit);
                }
                // A typed dynamic-input value commits as a point pick
                // before the plain-Enter (on_enter) path runs.
                if let Some(task) = self.try_dyn_commit() {
                    return task;
                }
                let i = self.active_tab;
                if self.tabs[i].active_cmd.is_some() {
                    let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_enter());
                    if let Some(r) = result {
                        return self.apply_cmd_result(r);
                    }
                    Task::none()
                } else if let Some(cmd) = self.tabs[i].last_cmd.clone() {
                    self.dispatch_command(&cmd)
                } else {
                    Task::none()
                }
            }

            Message::CommandEscape => {
                // Open MText editor swallows Escape (cancel without committing).
                if self.mtext_editor.is_some() {
                    self.mtext_cancel();
                    return Task::none();
                }
                // The in-place TEXT editor likewise cancels on Escape.
                if self.text_inline.is_some() {
                    self.text_inline_cancel();
                    return Task::none();
                }
                // Grip popup intercepts Escape — dismisses the menu
                // without doing anything else.
                if self.grip_popup.take().is_some() {
                    self.grip_hover = None;
                    return Task::none();
                }
                if self.visibility_popup.take().is_some() {
                    return Task::none();
                }
                if self.grip_pending.take().is_some() {
                    self.command_line.input.clear();
                    return Task::none();
                }
                // A hot grip (click-move-click placement in progress) ends on
                // Escape, leaving the entity at its last previewed position.
                if self.tabs[self.active_tab].active_grip.take().is_some() {
                    // An Add-Leader arrow being placed: Esc removes it again.
                    if let Some((h, gid)) = self.grip_add_provisional.take() {
                        let i = self.active_tab;
                        use crate::entities::traits::EntityTypeOps;
                        if let Some(e) = self.tabs[i].scene.document.get_entity_mut(h) {
                            e.apply_grip_menu(
                                gid,
                                crate::scene::model::object::GripMenuAction::RemoveLeader,
                            );
                        }
                        self.tabs[i].scene.bump_geometry();
                        self.refresh_selected_grips();
                    }
                    // Cancel an in-progress grip drag: restore the edited
                    // entity from its pre-drag backup, un-hide it, re-tessellate
                    // once, and drop the preview.
                    if let Some(h) = self.grip_preview_handle.take() {
                        let i = self.active_tab;
                        if let Some(orig) = self.grip_original.take() {
                            if let Some(e) = self.tabs[i].scene.document.get_entity_mut(h) {
                                *e = orig;
                            }
                        }
                        self.tabs[i].scene.hidden.remove(&h);
                        self.tabs[i].scene.clear_preview_wire();
                        // Geometry restored to the backup — re-tessellate just it.
                        self.tabs[i].scene.mark_entity_dirty(h);
                        self.tabs[i].scene.bump_geometry_no_blocks();
                        self.refresh_selected_grips();
                    }
                    self.tabs[self.active_tab].snap_result = None;
                    self.refresh_properties();
                    return Task::none();
                }
                // Cancel layout rename / context menus first, then fall through.
                let i_e = self.active_tab;
                if self.qselect.take().is_some() {
                    return Task::none();
                }
                {
                    let mut sel = self.tabs[i_e].scene.selection.borrow_mut();
                    if sel.context_menu.is_some() {
                        sel.context_menu = None;
                        return Task::none();
                    }
                }
                if self.layout_rename_state.take().is_some()
                    || self.layout_context_menu.take().is_some()
                {
                    return Task::none();
                }
                // Typed text on the command line cancels first — one
                // Esc empties the buffer, a second Esc then escalates
                // to whatever the current mode would otherwise do
                // (cancel command / exit viewport / deselect).
                if !self.command_line.input.is_empty() {
                    self.command_line.input.clear();
                    self.command_line.autocomplete_cursor = None;
                    self.command_line.close_history();
                    return Task::none();
                }
                let i = self.active_tab;
                if self.tabs[i].active_cmd.is_some() {
                    let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_escape());
                    if let Some(r) = result {
                        return self.apply_cmd_result(r);
                    }
                } else if self.tabs[i].scene.active_viewport.is_some() {
                    // ESC while in MSPACE → exit back to paper space.
                    return Task::done(Message::ExitViewport);
                } else {
                    self.tabs[i].scene.deselect_all();
                    self.refresh_properties();
                    let mut sel = self.tabs[i].scene.selection.borrow_mut();
                    sel.box_anchor = None;
                    sel.box_current = None;
                    sel.box_crossing = false;
                }
                Task::none()
            }

            Message::Command(cmd) => {
                // Close viewport context menu if open.
                let i = self.active_tab;
                self.tabs[i].scene.selection.borrow_mut().context_menu = None;
                // Any command also dismisses the Isolate action menu.
                self.isolate_popup_open = false;
                self.dispatch_command(&cmd)
            }

            Message::ToggleLayers => {
                if self.active_modal == Some(super::ModalKind::Layers) {
                    self.ribbon.deactivate_tool_if("LAYERS");
                    self.active_modal = None;
                } else {
                    self.sync_ribbon_layers();
                    self.active_modal = Some(super::ModalKind::Layers);
                }
                Task::none()
            }

            Message::WindowCloseRequested(id) => {
                if self.main_window == Some(id) {
                    if self.tabs.iter().any(|t| t.dirty) {
                        self.pending_close = Some(super::PendingClose::Quit);
                        return self.open_unsaved_dialog_window();
                    }
                    return iced::exit();
                }
                Task::none()
            }

            Message::OsWindowClosed(id) => {
                // Only the main window exists now; all dialogs are in-canvas
                // modals (Plan B). Closing it exits.
                if self.main_window == Some(id) {
                    return iced::exit();
                }
                Task::none()
            }

            // ── Layer panel messages ───────────────────────────────────────
            Message::LayerToggleVisible(idx) => {
                let i = self.active_tab;
                if idx < self.tabs[i].layers.layers.len() {
                    self.push_undo_snapshot(i, "LAYER OFF/ON");
                    let l = &mut self.tabs[i].layers.layers[idx];
                    l.visible = !l.visible;
                    let name = l.name.clone();
                    let on = l.visible;
                    self.tabs[i].scene.toggle_layer_visibility(&name);
                    self.command_line.push_output(&format!(
                        "Layer \"{}\" {}",
                        name,
                        if on { "on" } else { "off" }
                    ));
                }
                Task::none()
            }

            Message::LayerToggleLock(idx) => {
                let i = self.active_tab;
                if idx < self.tabs[i].layers.layers.len() {
                    self.push_undo_snapshot(i, "LAYER LOCK/UNLOCK");
                    let l = &mut self.tabs[i].layers.layers[idx];
                    l.locked = !l.locked;
                    let name = l.name.clone();
                    let locked = l.locked;
                    self.tabs[i].scene.toggle_layer_lock(&name);
                    self.command_line.push_output(&format!(
                        "Layer \"{}\" {}",
                        name,
                        if locked { "locked" } else { "unlocked" }
                    ));
                }
                Task::none()
            }

            Message::LayerToggleFreeze(idx) => {
                let i = self.active_tab;
                if idx < self.tabs[i].layers.layers.len() {
                    self.push_undo_snapshot(i, "LAYER FREEZE");
                    let l = &mut self.tabs[i].layers.layers[idx];
                    l.frozen = !l.frozen;
                    let name = l.name.clone();
                    let frozen = l.frozen;
                    if let Some(dl) = self.tabs[i].scene.document.layers.get_mut(&name) {
                        if frozen {
                            dl.freeze();
                        } else {
                            dl.thaw();
                        }
                    }
                    self.tabs[i].scene.bump_geometry();
                    self.tabs[i].dirty = true;
                }
                Task::none()
            }

            Message::LayerToggleVpFreeze(layer_idx, vp_col_idx) => {
                let i = self.active_tab;
                let vp_handle = self.tabs[i]
                    .layers
                    .vp_cols
                    .get(vp_col_idx)
                    .map(|c| c.handle);
                let layer_name = self.tabs[i]
                    .layers
                    .layers
                    .get(layer_idx)
                    .map(|l| l.name.clone());

                if let (Some(vp_handle), Some(layer_name)) = (vp_handle, layer_name) {
                    // Get the layer handle from the document
                    if let Some(doc_layer) = self.tabs[i].scene.document.layers.get(&layer_name) {
                        let layer_handle = doc_layer.handle;
                        self.push_undo_snapshot(i, "VPLAYER");

                        // Toggle frozen_layers on the viewport entity
                        for e in self.tabs[i].scene.document.entities_mut() {
                            if let acadrust::EntityType::Viewport(vp) = e {
                                if vp.common.handle == vp_handle {
                                    if vp.frozen_layers.contains(&layer_handle) {
                                        vp.frozen_layers.retain(|h| h != &layer_handle);
                                    } else {
                                        vp.frozen_layers.push(layer_handle);
                                    }
                                    break;
                                }
                            }
                        }

                        // Re-sync layer panel with updated VP info
                        let vp_info = self.tabs[i].scene.viewport_list();
                        let doc_layers = self.tabs[i].scene.document.layers.clone();
                        self.tabs[i]
                            .layers
                            .sync_with_viewports(&doc_layers, vp_info);
                        self.tabs[i].scene.bump_geometry();
                        self.tabs[i].dirty = true;
                    }
                }
                Task::none()
            }

            Message::LayerNew => {
                let i = self.active_tab;
                let mut n = 1;
                let new_name = loop {
                    let candidate = format!("Layer{}", n);
                    if !self.tabs[i].scene.document.layers.contains(&candidate) {
                        break candidate;
                    }
                    n += 1;
                };
                self.push_undo_snapshot(i, "LAYER NEW");
                use acadrust::tables::layer::Layer as DocLayer;
                // A layer needs a real handle or it is dropped on a DWG save
                // (the format is handle-based; issue #67).
                let mut dl = DocLayer::new(&new_name);
                // `allocate_handle` advances the seed so the layer gets a
                // unique handle; the non-advancing `next_handle` getter hands
                // out the same value twice and the later object overwrites it.
                dl.handle = self.tabs[i].scene.document.allocate_handle();
                let _ = self.tabs[i].scene.document.layers.add(dl);
                self.tabs[i].dirty = true;
                let doc_layers = self.tabs[i].scene.document.layers.clone();
                let vp_info = self.tabs[i].scene.viewport_list();
                self.tabs[i]
                    .layers
                    .sync_with_viewports(&doc_layers, vp_info);
                let new_idx = self.tabs[i]
                    .layers
                    .layers
                    .iter()
                    .position(|l| l.name == new_name);
                if let Some(idx) = new_idx {
                    self.tabs[i].layers.selected = Some(idx);
                    self.tabs[i].layers.editing = Some(idx);
                    self.tabs[i].layers.edit_buf = new_name.clone();
                }
                self.sync_ribbon_layers();
                Task::none()
            }

            Message::LayerDelete => {
                let i = self.active_tab;
                if let Some(idx) = self.tabs[i].layers.selected {
                    let name = self.tabs[i]
                        .layers
                        .layers
                        .get(idx)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    if name == "0" {
                        return Task::none();
                    }
                    self.push_undo_snapshot(i, "LAYER DELETE");
                    self.tabs[i].scene.document.layers.remove(&name);
                    self.tabs[i].dirty = true;
                    let doc_layers = self.tabs[i].scene.document.layers.clone();
                    let vp_info = self.tabs[i].scene.viewport_list();
                    self.tabs[i]
                        .layers
                        .sync_with_viewports(&doc_layers, vp_info);
                    self.tabs[i].layers.selected = None;
                    self.sync_ribbon_layers();
                }
                Task::none()
            }

            Message::LayerSetCurrent => {
                let i = self.active_tab;
                if let Some(idx) = self.tabs[i].layers.selected {
                    if let Some(layer) = self.tabs[i].layers.layers.get(idx) {
                        let name = layer.name.clone();
                        // Mirror the change into the document header (CLAYER) too,
                        // not just the per-tab default. Otherwise the no-selection
                        // ribbon refresh (e.g. after Esc) re-reads the stale header
                        // layer and the dropdown snaps back to it. See #93.
                        let handle = self.tabs[i]
                            .scene
                            .document
                            .layers
                            .get(&name)
                            .map(|l| l.handle)
                            .unwrap_or(acadrust::types::Handle::NULL);
                        self.tabs[i].scene.document.header.current_layer_name = name.clone();
                        self.tabs[i].scene.document.header.current_layer_handle = handle;
                        self.tabs[i].active_layer = name.clone();
                        self.tabs[i].layers.current_layer = name.clone();
                        self.tabs[i].dirty = true;
                        self.ribbon.active_layer = name;
                    }
                }
                Task::none()
            }

            Message::LayerSelect(idx) => {
                let i = self.active_tab;
                if self.tabs[i].layers.editing.is_some() {
                    return Task::done(Message::LayerRenameCommit);
                }
                self.tabs[i].layers.selected = Some(idx);
                Task::none()
            }

            Message::LayerRenameStart(idx) => {
                let i = self.active_tab;
                self.tabs[i].layers.selected = Some(idx);
                if let Some(layer) = self.tabs[i].layers.layers.get(idx) {
                    self.tabs[i].layers.edit_buf = layer.name.clone();
                }
                self.tabs[i].layers.editing = Some(idx);
                Task::none()
            }

            Message::LayerRenameEdit(s) => {
                let i = self.active_tab;
                self.tabs[i].layers.edit_buf = s;
                Task::none()
            }

            Message::LayerRenameCommit => {
                let i = self.active_tab;
                let editing_idx = self.tabs[i].layers.editing.take();
                if let Some(idx) = editing_idx {
                    let new_name = self.tabs[i].layers.edit_buf.trim().to_string();
                    let old_name = self.tabs[i]
                        .layers
                        .layers
                        .get(idx)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    if !new_name.is_empty()
                        && new_name != old_name
                        && !self.tabs[i].scene.document.layers.contains(&new_name)
                    {
                        self.push_undo_snapshot(i, "LAYER RENAME");
                        // Keep the whole record (handle, color, linetype,
                        // lineweight, flags) and only change the name, so the
                        // renamed layer still has a valid handle and survives a
                        // DWG save (issue #67).
                        if let Some(mut nl) =
                            self.tabs[i].scene.document.layers.get(&old_name).cloned()
                        {
                            nl.name = new_name.clone();
                            if !nl.handle.is_valid() {
                                nl.handle = self.tabs[i].scene.document.allocate_handle();
                            }
                            let _ = self.tabs[i].scene.document.layers.add(nl);
                        }
                        self.tabs[i].scene.document.layers.remove(&old_name);
                        for e in self.tabs[i].scene.document.entities_mut() {
                            if e.as_entity().layer() == old_name {
                                e.as_entity_mut().set_layer(new_name.clone());
                            }
                        }
                        self.tabs[i].dirty = true;
                    }
                    let doc_layers = self.tabs[i].scene.document.layers.clone();
                    let vp_info = self.tabs[i].scene.viewport_list();
                    self.tabs[i]
                        .layers
                        .sync_with_viewports(&doc_layers, vp_info);
                    self.tabs[i].layers.edit_buf.clear();
                    self.sync_ribbon_layers();
                }
                Task::none()
            }

            Message::LayerColorPickerToggle(idx) => {
                let i = self.active_tab;
                let panel = &mut self.tabs[i].layers;
                if panel.color_picker_row == Some(idx) {
                    panel.color_picker_row = None;
                    panel.color_full_palette = false;
                } else {
                    panel.color_picker_row = Some(idx);
                    panel.color_full_palette = false;
                    panel.selected = Some(idx);
                }
                Task::none()
            }

            Message::LayerColorMorePalette => {
                let i = self.active_tab;
                self.tabs[i].layers.color_full_palette = !self.tabs[i].layers.color_full_palette;
                Task::none()
            }

            Message::LayerColorSet(aci) => {
                let i = self.active_tab;
                if let Some(idx) = self.tabs[i].layers.selected {
                    if let Some(layer) = self.tabs[i].layers.layers.get(idx) {
                        let name = layer.name.clone();
                        if let Some(dl) = self.tabs[i].scene.document.layers.get_mut(&name) {
                            dl.color = AcadColor::Index(aci);
                        }
                        use crate::ui::layers::iced_color_from_acad;
                        let new_color = iced_color_from_acad(&AcadColor::Index(aci));
                        if let Some(pl) = self.tabs[i].layers.layers.get_mut(idx) {
                            pl.color = new_color;
                        }
                        self.tabs[i].dirty = true;
                    }
                    self.tabs[i].layers.color_picker_row = None;
                    self.tabs[i].layers.color_full_palette = false;
                    self.sync_ribbon_layers();
                }
                Task::none()
            }

            Message::LayerLinetypeSet(lt) => {
                let i = self.active_tab;
                if let Some(idx) = self.tabs[i].layers.selected {
                    if let Some(layer) = self.tabs[i].layers.layers.get(idx) {
                        let name = layer.name.clone();
                        if let Some(dl) = self.tabs[i].scene.document.layers.get_mut(&name) {
                            dl.line_type = lt.clone();
                        }
                        if let Some(pl) = self.tabs[i].layers.layers.get_mut(idx) {
                            pl.linetype = lt;
                        }
                        self.tabs[i].dirty = true;
                    }
                }
                Task::none()
            }

            Message::LayerLineweightSet(lw) => {
                let i = self.active_tab;
                if let Some(idx) = self.tabs[i].layers.selected {
                    if let Some(layer) = self.tabs[i].layers.layers.get(idx) {
                        let name = layer.name.clone();
                        if let Some(dl) = self.tabs[i].scene.document.layers.get_mut(&name) {
                            dl.line_weight = lw;
                        }
                        if let Some(pl) = self.tabs[i].layers.layers.get_mut(idx) {
                            pl.lineweight = lw;
                        }
                        self.tabs[i].dirty = true;
                    }
                }
                Task::none()
            }

            Message::LayerTransparencyEdit(idx, s) => {
                let i = self.active_tab;
                if let Some(pl) = self.tabs[i].layers.layers.get_mut(idx) {
                    if let Ok(v) = s.parse::<i32>() {
                        pl.transparency = v.clamp(0, 90);
                    } else if s.is_empty() {
                        pl.transparency = 0;
                    }
                }
                Task::none()
            }

            // ── Cursor / viewport messages ─────────────────────────────────
            Message::CursorMoved(p) => {
                // `p` is relative to the ViewCube hit area's top-left. Map
                // it back to full-canvas coordinates so ViewportClick's
                // hit-test lines up. The hit area sits in the top-right of
                // the full canvas in model space, or of the active
                // viewport's screen rectangle in a paper layout.
                let i = self.active_tab;
                let (vw, vh) = self.tabs[i].scene.selection.borrow().vp_size;
                let (ox, oy) = match self.tabs[i]
                    .scene
                    .active_viewport
                    .and_then(|h| self.tabs[i].scene.viewport_screen_rect(h, (vw, vh)))
                {
                    Some(rect) => (
                        rect.x + rect.width - VIEWCUBE_PAD - VIEWCUBE_HIT_SIZE,
                        rect.y + VIEWCUBE_PAD,
                    ),
                    None => {
                        // Model layout: the cube sits in the active tile's
                        // top-right corner.
                        let tb = self.tabs[i].scene.active_model_tile_bounds(vw, vh);
                        (
                            tb.x + tb.width - VIEWCUBE_PAD - VIEWCUBE_HIT_SIZE,
                            tb.y + VIEWCUBE_PAD,
                        )
                    }
                };
                self.cursor_pos = iced::Point::new(ox + p.x, oy + p.y);

                // Drive the ViewCube hover highlight directly from this
                // message — it fires whenever the cube's hit-area overlay
                // sees motion, so we don't depend on the shader widget's
                // `Program::update` receiving the same event (overlays sit
                // above the shader and can mask it). Map the cursor into
                // the active viewport's local box and use that box's size,
                // since that's where the cube is actually drawn.
                let tile = match self.tabs[i]
                    .scene
                    .active_viewport
                    .and_then(|h| self.tabs[i].scene.viewport_screen_rect(h, (vw, vh)))
                {
                    Some(rect) => rect,
                    None => self.tabs[i].scene.active_model_tile_bounds(vw, vh),
                };
                let cam_rot = self.tabs[i].scene.camera.borrow().view_rotation_mat();
                let hover = hover_id(
                    self.cursor_pos.x - tile.x,
                    self.cursor_pos.y - tile.y,
                    tile.width,
                    tile.height,
                    cam_rot,
                    VIEWCUBE_PX,
                );
                self.tabs[i].scene.viewcube_hover.set(hover);
                Task::none()
            }

            Message::ViewportMove(p) => {
                let i = self.active_tab;

                // A Model-tile divider drag short-circuits the rest of
                // the move handling — the cursor neither pans the camera
                // nor updates snap state while the user is resizing
                // panes.
                if let Some(drag) = self.tile_drag.as_mut() {
                    let (vw, vh) = self.tabs[i].scene.selection.borrow().vp_size;
                    if vw > 1.0 && vh > 1.0 {
                        let new_coord = match drag.orient {
                            TileEdgeOrient::Vertical => (p.x / vw).clamp(0.0, 1.0),
                            TileEdgeOrient::Horizontal => (p.y / vh).clamp(0.0, 1.0),
                        };
                        let (min_w, min_h) = tile_min_norm(vw, vh);
                        let min_size = match drag.orient {
                            TileEdgeOrient::Vertical => min_w,
                            TileEdgeOrient::Horizontal => min_h,
                        };
                        self.tabs[i].scene.move_model_tile_edge(
                            drag.orient,
                            drag.last_applied,
                            new_coord,
                            min_size,
                        );
                        drag.last_applied = new_coord;
                        self.tabs[i].scene.camera_generation += 1;
                    }
                    return Task::none();
                }

                // Keep the ViewCube hover in sync as the cursor leaves the
                // hit-area overlay and moves over the rest of the viewport.
                // `hover_id` returns None outside the cube box, which clears
                // any stale highlight from the previous `CursorMoved`.
                let (svw, svh) = self.tabs[i].scene.selection.borrow().vp_size;
                let cube_tile = match self.tabs[i]
                    .scene
                    .active_viewport
                    .and_then(|h| self.tabs[i].scene.viewport_screen_rect(h, (svw, svh)))
                {
                    Some(rect) => rect,
                    None => self.tabs[i].scene.active_model_tile_bounds(svw, svh),
                };
                let cam_rot = self.tabs[i].scene.camera.borrow().view_rotation_mat();
                self.tabs[i].scene.viewcube_hover.set(hover_id(
                    p.x - cube_tile.x,
                    p.y - cube_tile.y,
                    cube_tile.width,
                    cube_tile.height,
                    cam_rot,
                    VIEWCUBE_PX,
                ));

                // Multi-functional grip hover: detect cursor sitting on a
                // selected entity's grip and, after a dwell, open the
                // popup menu. See scene::model::object::GripMenuItem.
                self.update_grip_hover(i, p);

                let mut sel = self.tabs[i].scene.selection.borrow_mut();
                sel.last_move_pos = Some(p);

                if sel.left_down {
                    let press = sel.left_press_pos.unwrap_or(p);
                    let dx = p.x - press.x;
                    let dy = p.y - press.y;
                    let dist2 = dx * dx + dy * dy;
                    let elapsed_ms = sel
                        .left_press_time
                        .map(|t| Instant::now().duration_since(t).as_millis())
                        .unwrap_or(u128::MAX);
                    if !sel.left_dragging && elapsed_ms >= POLY_START_DELAY_MS && dist2 > 9.0 {
                        sel.left_dragging = true;
                        sel.poly_active = true;
                        sel.poly_crossing = p.x < press.x;
                        sel.poly_points.clear();
                        sel.poly_points.push(press);
                        sel.poly_points.push(p);
                    } else if sel.left_dragging && sel.poly_active {
                        if sel.poly_points.last().map_or(true, |lp| {
                            let ddx = p.x - lp.x;
                            let ddy = p.y - lp.y;
                            ddx * ddx + ddy * ddy > 16.0
                        }) {
                            sel.poly_points.push(p);
                        }
                    }
                } else if sel.box_anchor.is_some() {
                    sel.box_current = Some(p);
                    if let Some(a) = sel.box_anchor {
                        sel.box_crossing = p.x < a.x;
                    }
                }

                if sel.right_down {
                    if let Some(press) = sel.right_press_pos {
                        let dx = p.x - press.x;
                        let dy = p.y - press.y;
                        // 8 px (64 squared) threshold so normal hand jitter
                        // between a right-button press and release doesn't
                        // promote a click-and-release to an orbit drag, which
                        // would suppress the context menu on release.
                        if !sel.right_dragging && (dx * dx + dy * dy) > 64.0 {
                            sel.right_dragging = true;
                            sel.context_menu = None;
                        }
                    }
                    if sel.right_dragging {
                        if let Some(last) = sel.right_last_pos {
                            let (dx, dy) = (p.x - last.x, p.y - last.y);
                            if self.tabs[i].scene.active_viewport.is_some() {
                                // Update position before dropping the borrow.
                                sel.right_last_pos = Some(p);
                                drop(sel);
                                self.tabs[i].scene.orbit_active_viewport(dx, dy);
                                // Bump so the GPU re-uploads the viewport's
                                // re-culled wire set after the view rotates.
                                self.tabs[i].scene.camera_generation += 1;
                                return Task::none();
                            } else if self.tabs[i].scene.current_layout == "Model" {
                                sel.right_last_pos = Some(p);
                                drop(sel);
                                self.tabs[i].scene.camera.borrow_mut().orbit(dx, dy);
                                self.tabs[i].scene.camera_generation += 1;
                                return Task::none();
                            } else {
                                // Paper sheet is top-locked: right-drag never
                                // orbits it (orbiting would corrupt the camera
                                // frame and skew subsequent pans).
                                sel.right_last_pos = Some(p);
                                return Task::none();
                            }
                        } else {
                            sel.right_last_pos = Some(p);
                        }
                    }
                }

                let (mid_down, mid_last, vp_size) =
                    (sel.middle_down, sel.middle_last_pos, sel.vp_size);
                if mid_down {
                    if let Some(last) = mid_last {
                        let (dx, dy) = (p.x - last.x, p.y - last.y);
                        // Pan scale uses the active tile's size (ortho size
                        // is relative to viewport height), so a tiled pane
                        // pans at the correct rate.
                        let bounds = self.tabs[i]
                            .scene
                            .active_model_tile_bounds(vp_size.0, vp_size.1);
                        // Drop `sel` before calling mutable scene methods.
                        drop(sel);
                        if self.tabs[i].scene.active_viewport.is_some() {
                            self.tabs[i].scene.pan_active_viewport(dx, dy, bounds);
                            // Bump so the GPU re-uploads the viewport's re-culled
                            // wire set — otherwise newly-revealed lines stay
                            // invisible until MSPACE is exited.
                            self.tabs[i].scene.camera_generation += 1;
                        } else {
                            // `bounds` is the active tile; pan by its height so
                            // the point under the cursor tracks correctly.
                            self.tabs[i].scene.camera.borrow_mut().pan_screen(
                                dx,
                                dy,
                                bounds.height,
                            );
                            self.tabs[i].scene.camera_generation += 1;
                        }
                        self.tabs[i].scene.selection.borrow_mut().middle_last_pos = Some(p);
                        return Task::none();
                    }
                    sel.middle_last_pos = Some(p);
                }

                let dragging = sel.left_down || sel.right_down || sel.middle_down;
                let vp_size = sel.vp_size;
                drop(sel);

                // Hover (no button held): the tile under the cursor becomes
                // active, so the camera + tile bounds used for picking below
                // follow the pane the cursor is in. During a drag the active
                // tile stays put so the operation finishes in its own pane.
                if !dragging && vp_size.0 > 1.0 && vp_size.1 > 1.0 {
                    if self.tabs[i]
                        .scene
                        .set_active_model_tile_at(p.x / vp_size.0, p.y / vp_size.1)
                    {
                        self.tabs[i].scene.camera_generation += 1;
                        self.sync_render_mode_to_active_tile(i);
                    }
                }

                // Tile-relative picking: shadow `p` with the cursor mapped
                // into the active Model tile and `vp_size` with the tile's
                // size, so every pick / snap / view_proj below operates in
                // the active pane. `p_full` keeps the canvas-space cursor
                // for screen overlays (cursor marker, snap glyph).
                let p_full = p;
                let tile_b = self.tabs[i]
                    .scene
                    .active_model_tile_bounds(vp_size.0, vp_size.1);
                let p = iced::Point {
                    x: p_full.x - tile_b.x,
                    y: p_full.y - tile_b.y,
                };
                let vp_size = (tile_b.width, tile_b.height);

                // ── Grip drag ─────────────────────────────────────────────
                if let Some(grip) = self.tabs[i].active_grip.clone() {
                    let (vw, vh) = vp_size;
                    let bounds = iced::Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: vw,
                        height: vh,
                    };
                    let cam = self.tabs[i].scene.camera.borrow();
                    let raw_paper = cam.pick_on_target_plane(p, bounds);
                    let vp_mat = cam.view_proj(bounds);
                    drop(cam);
                    let raw = self.tabs[i].scene.paper_to_model(raw_paper);

                    // First move of this drag: hide the edited entity from the
                    // base tessellation (one re-tess) so subsequent moves only
                    // refresh a cheap overlay preview instead of re-tessellating
                    // the whole model on every move.
                    if self.grip_preview_handle != Some(grip.handle) {
                        if let Some(prev) = self.grip_preview_handle.take() {
                            self.tabs[i].scene.hidden.remove(&prev);
                        }
                        // Back up the original geometry so Esc can cancel the drag.
                        self.grip_original =
                            self.tabs[i].scene.document.get_entity(grip.handle).cloned();
                        self.tabs[i].scene.hidden.insert(grip.handle);
                        // Grip drag never changes a block definition — keep the
                        // block cache so the hide doesn't re-tessellate blocks.
                        self.tabs[i].scene.bump_geometry_no_blocks();
                        self.grip_preview_handle = Some(grip.handle);
                    }

                    // The edited entity is hidden, so it's already absent from
                    // `hit_test_wires` — snap against the set directly, no clone
                    // and no self-snap.
                    let all_wires = self.tabs[i].scene.hit_test_wires();
                    // Grip drag has no single rubber-band origin for a perp foot.
                    self.snapper.from_point = None;
                    let snap_hit = self.snapper.snap(raw, p, &all_wires[..], vp_mat, bounds);
                    let mut snapped = snap_hit.map(|s| s.world).unwrap_or(raw);
                    self.tabs[i].snap_result = snap_hit;
                    if let Some(s) = self.tabs[i].snap_result.as_mut() {
                        s.screen.x += tile_b.x;
                        s.screen.y += tile_b.y;
                    }

                    if snap_hit.is_none() {
                        let base = grip.origin_world;
                        if self.ortho_mode {
                            snapped = ortho_constrain(snapped, base);
                        } else if self.polar_mode {
                            snapped = polar_constrain_near(
                                snapped,
                                base,
                                self.polar_increment_deg,
                                vp_mat,
                                bounds,
                                self.snapper.osnap_radius_px,
                            );
                        }
                    }

                    // Paper-space entities use sheet coordinates (no world_offset).
                    // Only add world_offset when converting local → DXF space in model space.
                    let wo_vec = if self.tabs[i].scene.current_layout == "Model" {
                        let wo = self.tabs[i].scene.world_offset;
                        glam::Vec3::new(wo[0] as f32, wo[1] as f32, wo[2] as f32)
                    } else {
                        glam::Vec3::ZERO
                    };
                    let apply = if grip.is_translate {
                        GripApply::Translate(snapped - grip.last_world)
                    } else {
                        GripApply::Absolute(snapped + wo_vec)
                    };
                    self.tabs[i]
                        .scene
                        .apply_grip(grip.handle, grip.grip_id, apply);
                    self.tabs[i].dirty = true;
                    self.tabs[i].active_grip.as_mut().unwrap().last_world = snapped;
                    // Overlay the moved entity (hidden from the base) — no base
                    // re-tessellation, just a one-entity preview tessellation.
                    let preview = self.tabs[i].scene.wire_models_for(&[grip.handle]);
                    self.tabs[i].scene.set_preview_wires(preview);
                    self.refresh_selected_grips();
                    self.refresh_properties();
                    return Task::none();
                }

                // Keep the coordinate readout live on every move, even with no
                // active command. When a command is running the snap path below
                // overwrites this with the snapped point.
                {
                    let bounds = iced::Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: vp_size.0,
                        height: vp_size.1,
                    };
                    let paper = if let Some(ref ucs) = self.tabs[i].active_ucs {
                        let origin = glam::Vec3::new(
                            ucs.origin.x as f32,
                            ucs.origin.y as f32,
                            ucs.origin.z as f32,
                        );
                        let normal = ucs_z_axis(ucs);
                        self.tabs[i]
                            .scene
                            .camera
                            .borrow()
                            .pick_on_plane(p, bounds, normal, origin)
                    } else {
                        self.tabs[i]
                            .scene
                            .camera
                            .borrow()
                            .pick_on_target_plane(p, bounds)
                    };
                    let world = self.tabs[i].scene.paper_to_model(paper);
                    self.tabs[i].last_cursor_world = world;
                }

                // Rollover highlight: when idle (no active command, no drag),
                // highlight the entity under the cursor. Decoupling selection
                // from tessellation makes this cheap — it only refreshes the GPU
                // xray overlay via `bump_selection`, never re-tessellates.
                if !dragging && self.tabs[i].active_cmd.is_none() {
                    let bounds = iced::Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: vp_size.0,
                        height: vp_size.1,
                    };
                    let view_proj = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                    let all_wires = self.tabs[i].scene.hit_test_wires();
                    let hovered = scene::pick::hit_test::click_hit(p, &all_wires[..], view_proj, bounds)
                        .and_then(|s| Scene::handle_from_wire_name(s));
                    self.tabs[i].scene.set_hover_highlight(hovered);
                } else {
                    // Suppress the rollover during a command or a drag.
                    self.tabs[i].scene.set_hover_highlight(None);
                }

                if self.tabs[i].active_cmd.is_some() {
                    let (vw, vh) = vp_size;
                    let bounds = iced::Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: vw,
                        height: vh,
                    };
                    let cursor_paper = if let Some(ref ucs) = self.tabs[i].active_ucs {
                        let origin = glam::Vec3::new(
                            ucs.origin.x as f32,
                            ucs.origin.y as f32,
                            ucs.origin.z as f32,
                        );
                        let normal = ucs_z_axis(ucs);
                        self.tabs[i]
                            .scene
                            .camera
                            .borrow()
                            .pick_on_plane(p, bounds, normal, origin)
                    } else {
                        self.tabs[i]
                            .scene
                            .camera
                            .borrow()
                            .pick_on_target_plane(p, bounds)
                    };
                    let view_proj = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                    // Sync grid-snap spacing to the adaptive spacing of the visible grid.
                    self.snapper.grid_spacing =
                        crate::ui::overlay::compute_grid_step(view_proj, bounds);
                    // In MSPACE, map paper-space cursor to model space so that
                    // command previews and snapping work in the correct coordinate space.
                    let cursor_world = self.tabs[i].scene.paper_to_model(cursor_paper);

                    let all_wires = self.tabs[i].scene.hit_test_wires();
                    let needs_entity = self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.needs_entity_pick())
                        .unwrap_or(false);
                    let needs_structure = self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.needs_structure_point_pick())
                        .unwrap_or(false);
                    let is_gathering = self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.is_selection_gathering())
                        .unwrap_or(false);
                    let needs_tan = self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.needs_tangent_pick())
                        .unwrap_or(false);
                    self.tabs[i].snap_result = if needs_entity || is_gathering || needs_structure {
                        None
                    } else if needs_tan {
                        self.snapper.snap_tangent_only(
                            cursor_world,
                            p,
                            &all_wires[..],
                            view_proj,
                            bounds,
                        )
                    } else {
                        self.snapper.from_point = self.last_point;
                        self.snapper
                            .snap(cursor_world, p, &all_wires[..], view_proj, bounds)
                    };

                    // Object Snap Tracking: update dwell, then align the cursor
                    // to a tracking ray (and store the alignment so a typed
                    // distance can place a point along it — issue #69).
                    let otrack_hit = {
                        let snap_world = self.tabs[i].snap_result.map(|s| s.world);
                        self.snapper.update_otrack_dwell(
                            snap_world,
                            view_proj,
                            bounds,
                            Instant::now(),
                        );
                        if self.tabs[i].snap_result.is_none() {
                            let step = if self.polar_mode {
                                Some(self.polar_increment_deg)
                            } else {
                                None
                            };
                            self.snapper.otrack_snap(
                                cursor_world,
                                view_proj,
                                bounds,
                                step,
                                self.last_point,
                            )
                        } else {
                            None
                        }
                    };
                    self.otrack_active = otrack_hit.map(|h| (h.base, h.dir));

                    let effective = {
                        let mut pt = if let Some(h) = otrack_hit {
                            // Tracking alignment wins over the free cursor;
                            // ortho/polar don't re-constrain it.
                            h.aligned
                        } else {
                            // snap.world is paper-space for viewport-projected
                            // wires; convert to model-space for previews.
                            let mut pt = self.tabs[i]
                                .snap_result
                                .map(|s| self.tabs[i].scene.paper_to_model(s.world))
                                .unwrap_or(cursor_world);
                            if let Some(base) = self.last_point {
                                if self.ortho_mode {
                                    pt = ortho_constrain(pt, base);
                                } else if self.polar_mode {
                                    pt = polar_constrain_near(
                                        pt,
                                        base,
                                        self.polar_increment_deg,
                                        view_proj,
                                        bounds,
                                        self.snapper.osnap_radius_px,
                                    );
                                }
                            }
                            pt
                        };
                        // Clamp to world XY only when no UCS is active; with a
                        // UCS the point already lies on the UCS XY plane.
                        if self.tabs[i].active_cmd.is_some() && self.tabs[i].active_ucs.is_none() {
                            pt.z = 0.0;
                        }
                        pt
                    };
                    self.tabs[i].last_cursor_world = effective;
                    self.tabs[i].last_cursor_screen = p_full;
                    // Project the step anchor (an explicit `dyn_anchor` or the
                    // last point) so the dynamic-input overlay can place its
                    // guide geometry and labels.
                    let project = |bp: glam::Vec3| {
                        let ndc = view_proj.project_point3(bp);
                        iced::Point::new(
                            (ndc.x + 1.0) * 0.5 * bounds.width,
                            (1.0 - ndc.y) * 0.5 * bounds.height,
                        )
                    };
                    let anchor = self.tabs[i].dyn_anchor.or(self.last_point);
                    self.tabs[i].last_point_screen = anchor.map(project);
                    self.tabs[i].dyn_ref_screen = self.tabs[i].dyn_ref.map(project);

                    // Entity-pick previews (TRIM/EXTEND/FILLET…) compare the
                    // cursor against WCS document entities and return WCS wires.
                    // `effective` is offset-relative, so build a WCS copy for
                    // the click and shift the returned wires back to the
                    // offset-relative frame the renderer expects (model space
                    // only; paper-space entities use sheet coordinates).
                    let wo_local = if self.tabs[i].scene.current_layout == "Model" {
                        self.tabs[i].scene.world_offset
                    } else {
                        [0.0; 3]
                    };
                    let wo_f = glam::Vec3::new(wo_local[0] as f32, wo_local[1] as f32, wo_local[2] as f32);
                    let effective_wcs = effective + wo_f;

                    // Orange object snap (plugin commands implement resolve_object_pick).
                    if needs_structure {
                        use crate::snap::{SnapResult, SnapType};
                        let pick = self.tabs[i].active_cmd.as_ref().and_then(|c| {
                            c.resolve_object_pick(
                                &self.tabs[i].scene,
                                effective.x as f64,
                                effective.y as f64,
                            )
                        });
                        if let Some(pick) = pick {
                            let world = glam::Vec3::new(pick.x as f32, pick.y as f32, effective.z);
                            let ndc = view_proj.project_point3(world);
                            let screen = iced::Point::new(
                                (ndc.x + 1.0) * 0.5 * bounds.width,
                                (1.0 - ndc.y) * 0.5 * bounds.height,
                            );
                            self.tabs[i].snap_result = Some(SnapResult {
                                world,
                                screen,
                                snap_type: SnapType::ObjectPick,
                                tangent_obj: None,
                            });
                            if let Some(cmd) = self.tabs[i].active_cmd.as_mut() {
                                cmd.set_acquisition_hint(Some(pick.label));
                            }
                        } else if let Some(cmd) = self.tabs[i].active_cmd.as_mut() {
                            cmd.set_acquisition_hint(None);
                        }
                    }

                    // Snap glyph is positioned in canvas space; shift the
                    // tile-local snap screen point back to the full canvas.
                    if let Some(s) = self.tabs[i].snap_result.as_mut() {
                        s.screen.x += tile_b.x;
                        s.screen.y += tile_b.y;
                    }

                    let mut previews = if needs_structure {
                        let mut p = self.tabs[i]
                            .active_cmd
                            .as_ref()
                            .map(|c| c.object_pick_hover_previews(&self.tabs[i].scene, effective))
                            .unwrap_or_default();
                        if let Some(cmd) = self.tabs[i].active_cmd.as_mut() {
                            p.extend(cmd.on_preview_wires(effective));
                        }
                        p
                    } else if needs_entity {
                        let hover_handle =
                            scene::pick::hit_test::click_hit(p, &all_wires[..], view_proj, bounds)
                                .and_then(|s| Scene::handle_from_wire_name(s))
                                .unwrap_or(acadrust::Handle::NULL);
                        let mut p = self.tabs[i]
                            .active_cmd
                            .as_mut()
                            .map(|c| c.on_hover_entity(hover_handle, effective_wcs))
                            .unwrap_or_default();
                        // on_hover_entity returns WCS wires; shift to the
                        // offset-relative frame so the preview lands on the
                        // geometry on large-coordinate drawings.
                        if wo_f != glam::Vec3::ZERO {
                            for w in p.iter_mut() {
                                for pt in w.points.iter_mut() {
                                    pt[0] -= wo_f.x;
                                    pt[1] -= wo_f.y;
                                    pt[2] -= wo_f.z;
                                }
                            }
                        }
                        if !hover_handle.is_null() {
                            if let Some(cmd) = self.tabs[i].active_cmd.as_ref() {
                                p.extend(cmd.entity_pick_acquire_previews(
                                    &self.tabs[i].scene,
                                    hover_handle,
                                ));
                            }
                            if let Some(cmd) = self.tabs[i].active_cmd.as_mut() {
                                if let Some(hint) = cmd.entity_pick_acquire_hint(hover_handle) {
                                    cmd.set_acquisition_hint(Some(hint));
                                }
                            }
                        }
                        p
                    } else {
                        self.tabs[i]
                            .active_cmd
                            .as_mut()
                            .map(|c| c.on_preview_wires(effective))
                            .unwrap_or_default()
                    };
                    // Polar tracking guide line: dotted line from last_point along
                    // the snapped angle direction, extending across the drawing.
                    if self.polar_mode && !needs_entity {
                        if let Some(base) = self.last_point {
                            let dx = effective.x - base.x;
                            let dy = effective.y - base.y;
                            // Only show the guide while POLAR is actually engaged
                            // — i.e. the point is snapped onto a polar ray, not
                            // floating free near no angle (issue #70).
                            let step = self.polar_increment_deg.to_radians();
                            let angle = dy.atan2(dx);
                            let snapped =
                                step > 1e-6 && ((angle / step).round() * step - angle).abs() < 1e-3;
                            if snapped && (dx * dx + dy * dy).sqrt() > 1e-4 {
                                let far = 1e5_f32;
                                let dir = glam::Vec3::new(dx, dy, 0.0).normalize();
                                let far_pos = base + dir * far;
                                let far_neg = base - dir * far;
                                let guide = crate::scene::WireModel {
                                    name: "__polar_guide__".into(),
                                    points: vec![
                                        [far_neg.x, far_neg.y, far_neg.z],
                                        [far_pos.x, far_pos.y, far_pos.z],
                                    ],
                                    color: [0.2, 0.7, 0.9, 0.6],
                                    selected: false,
                                    aci: 0,
                                    pattern_length: 0.8,
                                    pattern: [0.5, -0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                                    line_weight_px: 1.0,
                                    snap_pts: vec![],
                                    tangent_geoms: vec![],
                                    key_vertices: vec![],
                                    aabb: crate::scene::WireModel::UNBOUNDED_AABB,
                                    plinegen: true,
                                    vp_scissor: None,
                                    fill_tris: vec![],
                                };
                                previews.push(guide);
                            }
                        }
                    }
                    // OTRACK alignment guide: dashed ray through the tracking
                    // point along the aligned direction (issue #69).
                    if let Some(h) = otrack_hit {
                        if !needs_entity {
                            let far = 1e5_f32;
                            let far_pos = h.base + h.dir * far;
                            let far_neg = h.base - h.dir * far;
                            previews.push(crate::scene::WireModel {
                                name: "__otrack_guide__".into(),
                                points: vec![
                                    [far_neg.x, far_neg.y, far_neg.z],
                                    [far_pos.x, far_pos.y, far_pos.z],
                                ],
                                color: [0.2, 0.9, 0.5, 0.6],
                                selected: false,
                                aci: 0,
                                pattern_length: 0.8,
                                pattern: [0.5, -0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                                line_weight_px: 1.0,
                                snap_pts: vec![],
                                tangent_geoms: vec![],
                                key_vertices: vec![],
                                aabb: crate::scene::WireModel::UNBOUNDED_AABB,
                                plinegen: true,
                                vp_scissor: None,
                                fill_tris: vec![],
                            });
                        }
                    }
                    self.tabs[i].scene.set_preview_wires(previews);
                } else {
                    self.tabs[i].snap_result = None;
                }

                self.sync_dyn_fields();
                Task::none()
            }

            Message::ViewportExit => {
                let i = self.active_tab;
                let mut sel = self.tabs[i].scene.selection.borrow_mut();
                sel.left_down = false;
                sel.left_press_pos = None;
                sel.left_press_time = None;
                sel.left_dragging = false;
                sel.right_down = false;
                sel.right_press_pos = None;
                sel.right_last_pos = None;
                sel.right_dragging = false;
                sel.middle_down = false;
                sel.middle_last_pos = None;
                sel.box_anchor = None;
                sel.box_current = None;
                sel.box_crossing = false;
                sel.poly_active = false;
                sel.poly_points.clear();
                sel.poly_crossing = false;
                drop(sel);
                // Clear the rollover highlight when the cursor leaves the
                // viewport so it doesn't stick while the mouse is over the
                // ribbon / panels.
                self.tabs[i].scene.set_hover_highlight(None);
                // Don't touch `context_menu` here. ViewportExit also fires
                // when an upper overlay (the right-click menu panel) takes
                // the cursor, so clearing the menu state on every exit
                // would close the menu the moment it opens. Outside-click
                // dismiss is handled in `ViewportLeftPress`.
                Task::none()
            }

            Message::ViewportLeftPress => {
                let i = self.active_tab;
                // A click in the viewport dismisses any open ribbon dropdown
                // (e.g. the annotation style combo), which has no backdrop of
                // its own to catch outside clicks.
                self.ribbon.close_dropdown();
                // Likewise dismiss the Properties color dropdowns — a viewport
                // press starts a box selection, so without this they'd only
                // close on the second click (issue #104).
                self.tabs[i].properties.color_picker_open = false;
                self.tabs[i].properties.color_palette_open = false;
                // Click anywhere outside the popup dismisses it. The
                // menu's own buttons live above this mouse_area, so a
                // press that reaches here means the cursor is not on
                // any menu item.
                if self.grip_popup.take().is_some() {
                    self.grip_hover = None;
                    return Task::none();
                }
                // Outside-click dismiss for the visibility-state dropdown
                // (its buttons sit above this mouse_area).
                if self.visibility_popup.take().is_some() {
                    return Task::none();
                }
                // Same dismiss-on-outside-click for the right-click
                // context menu: its panel is opaque, so a press that
                // reaches here is outside the menu.
                {
                    let mut sel = self.tabs[i].scene.selection.borrow_mut();
                    if sel.context_menu.take().is_some() {
                        return Task::none();
                    }
                }
                let (p, vp_size) = {
                    let sel = self.tabs[i].scene.selection.borrow();
                    let p = match sel.last_move_pos {
                        Some(p) => p,
                        None => return Task::none(),
                    };
                    (p, sel.vp_size)
                };
                let (vw, vh) = vp_size;

                if vw > 1.0 && vh > 1.0 {
                    let cam = self.tabs[i].scene.camera.borrow();
                    if scene::hit_test(p.x, p.y, vw, vh, cam.view_rotation_mat(), VIEWCUBE_PX)
                        .is_some()
                    {
                        return Task::none();
                    }
                }

                // Tiled Model layout: an inner divider near the cursor
                // becomes a resize grip. Start the drag and short-circuit
                // (no pick / select / camera swap). Released on
                // `ViewportLeftRelease`.
                if self.tabs[i].active_cmd.is_none()
                    && self.tabs[i].scene.current_layout == "Model"
                    && vw > 1.0
                    && vh > 1.0
                {
                    let canvas_bounds = iced::Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: vw,
                        height: vh,
                    };
                    if let Some(edge) =
                        self.tabs[i]
                            .scene
                            .hit_model_tile_edge(p, canvas_bounds, TILE_EDGE_HIT_PX)
                    {
                        self.tile_drag = Some(crate::app::TileDrag {
                            orient: edge.orient,
                            last_applied: edge.coord,
                        });
                        return Task::none();
                    }
                }

                // Tiled Model layout: clicking a non-active tile activates
                // it (swapping in its camera) instead of selecting / drawing.
                if self.tabs[i].active_cmd.is_none() && vw > 1.0 && vh > 1.0 {
                    if self.tabs[i]
                        .scene
                        .set_active_model_tile_at(p.x / vw, p.y / vh)
                    {
                        self.tabs[i].scene.camera_generation += 1;
                        self.sync_render_mode_to_active_tile(i);
                        return Task::none();
                    }
                }

                // From here the click targets the active tile: map the
                // cursor into it and use the tile's size for picking, so
                // grip / selection hit-tests land in the right pane.
                let p_full = p;
                let tile_b = self.tabs[i].scene.active_model_tile_bounds(vw, vh);
                let p = iced::Point {
                    x: p_full.x - tile_b.x,
                    y: p_full.y - tile_b.y,
                };
                let (vw, vh) = (tile_b.width, tile_b.height);
                let bounds = iced::Rectangle {
                    x: 0.0,
                    y: 0.0,
                    width: vw,
                    height: vh,
                };

                if self.tabs[i].active_cmd.is_none()
                    && self.tabs[i].active_grip.is_none()
                    && !self.tabs[i].selected_grips.is_empty()
                {
                    if let Some(handle) = self.tabs[i].selected_handle {
                        let is_paper = self.tabs[i].scene.current_layout != "Model";
                        let grip_hit = if is_paper {
                            let cam = self.tabs[i].scene.camera.borrow();
                            let aspect = if vh > 0.0 { vw / vh } else { 1.0 };
                            let half_h = cam.ortho_size();
                            let half_w = half_h * aspect;
                            let tx = cam.target.x;
                            let ty = cam.target.y;
                            drop(cam);
                            find_hit_grip_paper(
                                p,
                                &self.tabs[i].selected_grips,
                                tx,
                                ty,
                                half_w,
                                half_h,
                                bounds,
                            )
                        } else {
                            let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                            find_hit_grip(p, &self.tabs[i].selected_grips, vp_mat, bounds)
                        };
                        if let Some((grip_id, is_translate, world)) = grip_hit {
                            // The visibility (lookup) grip opens a state
                            // dropdown instead of starting a stretch drag.
                            if grip_id == super::visibility::VIS_GRIP_ID {
                                self.open_visibility_popup(p);
                                self.grip_hover = None;
                                self.grip_popup = None;
                                return Task::none();
                            }
                            self.tabs[i].active_grip = Some(GripEdit {
                                handle,
                                grip_id,
                                is_translate,
                                origin_world: world,
                                last_world: world,
                            });
                            self.grip_hover = None;
                            self.grip_popup = None;
                            return Task::none();
                        }
                    }
                }

                let mut sel = self.tabs[i].scene.selection.borrow_mut();
                sel.left_down = true;
                // Stored in full-canvas space (like ViewportMove's cursor and
                // the overlay box / lasso drawing); release maps it into the
                // active tile. Tile-local here would double-offset the anchor.
                sel.left_press_pos = Some(p_full);
                sel.left_press_time = Some(Instant::now());
                sel.left_dragging = false;
                Task::none()
            }

            Message::ViewportLeftRelease => {
                let i = self.active_tab;

                // End an in-flight tile-divider drag. Any tile that fell
                // below the minimum (viewcube fits comfortably) gets
                // absorbed into its longest-contact neighbour, so the
                // user can drag a divider all the way to one side to
                // remove a pane.
                if self.tile_drag.take().is_some() {
                    let (vw, vh) = self.tabs[i].scene.selection.borrow().vp_size;
                    let (min_w, min_h) = tile_min_norm(vw, vh);
                    self.tabs[i].scene.collapse_small_model_tiles(min_w, min_h);
                    self.tabs[i].scene.camera_generation += 1;
                    return Task::none();
                }

                let (p, is_click, is_down) = {
                    let sel = self.tabs[i].scene.selection.borrow();
                    let p = match sel.last_move_pos {
                        Some(p) => p,
                        None => return Task::none(),
                    };
                    (p, !sel.left_dragging, sel.left_down)
                };

                // Grip editing: click-move-click (plus legacy press-drag).
                // The grip engages on press (active_grip set). This release
                // commits only if the grip has actually moved, or if it was a
                // press-drag. A bare engaging click (no movement yet) keeps the
                // grip hot so the user can move the cursor and click again to
                // place it. Escape cancels (handled elsewhere).
                if let Some(grip) = self.tabs[i].active_grip.clone() {
                    // Reset mouse state so the lingering press from the engaging
                    // click doesn't read as an in-progress drag on later moves.
                    {
                        let mut sel = self.tabs[i].scene.selection.borrow_mut();
                        sel.left_down = false;
                        sel.left_press_pos = None;
                        sel.left_press_time = None;
                        sel.left_dragging = false;
                    }
                    let moved = grip.last_world != grip.origin_world;
                    if is_click && !moved {
                        // Engaging click — stay hot, wait for the placement click.
                        return Task::none();
                    }
                    self.tabs[i].active_grip = None;
                    // Commit the grip drag: keep the doc's dragged geometry
                    // (drop the cancel backup), un-hide the edited entity and
                    // re-tessellate the base once, dropping the overlay preview.
                    if let Some(h) = self.grip_preview_handle.take() {
                        self.grip_original = None;
                        self.tabs[i].scene.hidden.remove(&h);
                        self.tabs[i].scene.clear_preview_wire();
                        // Only the dragged entity changed — re-tessellate just it.
                        self.tabs[i].scene.mark_entity_dirty(h);
                        self.tabs[i].scene.bump_geometry_no_blocks();
                    }
                    // Placement confirmed — keep the just-added leader.
                    self.grip_add_provisional = None;
                    self.tabs[i].snap_result = None;
                    self.refresh_properties();
                    return Task::none();
                }

                // Map the release point into the active Model tile so the
                // click's pick / on_point / selection use the active pane's
                // camera + bounds. `p_full` keeps the canvas point for the
                // box/poly selection rectangle (drawn in canvas space).
                let p_full = p;
                let (tile_vw, tile_vh, tile_off) = {
                    let (svw, svh) = self.tabs[i].scene.selection.borrow().vp_size;
                    let tb = self.tabs[i].scene.active_model_tile_bounds(svw, svh);
                    (tb.width, tb.height, iced::Point::new(tb.x, tb.y))
                };
                let p = iced::Point {
                    x: p_full.x - tile_off.x,
                    y: p_full.y - tile_off.y,
                };

                let is_gathering = self.tabs[i]
                    .active_cmd
                    .as_ref()
                    .map(|c| c.is_selection_gathering())
                    .unwrap_or(false);

                if is_down && is_click && self.tabs[i].active_cmd.is_some() && !is_gathering {
                    let (vw, vh) = (tile_vw, tile_vh);
                    let bounds = iced::Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: vw,
                        height: vh,
                    };

                    let snap_taken = self.tabs[i].snap_result.take();
                    let tangent_obj_at_click = snap_taken.and_then(|s| s.tangent_obj);

                    let world_pt = {
                        // Project screen point onto the active UCS XY plane (or world XY when
                        // no UCS is active).
                        let raw_paper = if let Some(ref ucs) = self.tabs[i].active_ucs {
                            let origin = glam::Vec3::new(
                                ucs.origin.x as f32,
                                ucs.origin.y as f32,
                                ucs.origin.z as f32,
                            );
                            let normal = ucs_z_axis(ucs);
                            self.tabs[i]
                                .scene
                                .camera
                                .borrow()
                                .pick_on_plane(p, bounds, normal, origin)
                        } else {
                            self.tabs[i]
                                .scene
                                .camera
                                .borrow()
                                .pick_on_target_plane(p, bounds)
                        };
                        // Convert paper-space → model-space when inside a viewport.
                        let raw = self.tabs[i].scene.paper_to_model(raw_paper);
                        let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                        let all_wires = self.tabs[i].scene.hit_test_wires();
                        let needs_tan = self.tabs[i]
                            .active_cmd
                            .as_ref()
                            .map(|c| c.needs_tangent_pick())
                            .unwrap_or(false);
                        let needs_entity_click = self.tabs[i]
                            .active_cmd
                            .as_ref()
                            .map(|c| c.needs_entity_pick())
                            .unwrap_or(false);
                        let snap_hit = if needs_entity_click {
                            None
                        } else if needs_tan {
                            self.snapper
                                .snap_tangent_only(raw, p, &all_wires[..], vp_mat, bounds)
                        } else {
                            self.snapper.from_point = self.last_point;
                            self.snapper.snap(raw, p, &all_wires[..], vp_mat, bounds)
                        };
                        // snap.world is in paper-space (projected wire coords in MSPACE);
                        // convert to model-space so commands receive consistent coordinates.
                        let mut pt = snap_hit
                            .map(|s| self.tabs[i].scene.paper_to_model(s.world))
                            .unwrap_or(raw);
                        // When no UCS is active clamp to world XY; with a UCS the point is
                        // already constrained to that plane by the ray–plane intersection.
                        if self.tabs[i].active_ucs.is_none() {
                            pt.z = 0.0;
                        }
                        // OTRACK alignment wins over ortho/polar; otherwise apply
                        // ortho/polar relative to the last point.
                        let otrack = if snap_hit.is_none() {
                            let step = if self.polar_mode {
                                Some(self.polar_increment_deg)
                            } else {
                                None
                            };
                            self.snapper
                                .otrack_snap(raw, vp_mat, bounds, step, self.last_point)
                        } else {
                            None
                        };
                        if let Some(h) = otrack {
                            pt = h.aligned;
                            if self.tabs[i].active_ucs.is_none() {
                                pt.z = 0.0;
                            }
                        } else if let Some(base) = self.last_point {
                            if self.ortho_mode {
                                pt = ortho_constrain(pt, base);
                            } else if self.polar_mode {
                                pt = polar_constrain_near(
                                    pt,
                                    base,
                                    self.polar_increment_deg,
                                    vp_mat,
                                    bounds,
                                    self.snapper.osnap_radius_px,
                                );
                            }
                        }
                        pt
                    };

                    // `world_pt` is in offset-relative (local) space, matching
                    // the camera and the point-creation commands. Entity-pick /
                    // tangent / structure-pick commands instead compare the
                    // click against WCS document entities, so they need the
                    // world_offset added back (model space only — paper-space
                    // entities are already in sheet coordinates). Without this,
                    // TRIM/EXTEND/FILLET pick the wrong side on UTM-scale files.
                    let pick_wcs = {
                        let wo = if self.tabs[i].scene.current_layout == "Model" {
                            self.tabs[i].scene.world_offset
                        } else {
                            [0.0; 3]
                        };
                        world_pt + glam::Vec3::new(wo[0] as f32, wo[1] as f32, wo[2] as f32)
                    };

                    let result = if self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.needs_structure_point_pick())
                        .unwrap_or(false)
                    {
                        let pick = self.tabs[i].active_cmd.as_ref().and_then(|c| {
                            c.resolve_object_pick(
                                &self.tabs[i].scene,
                                pick_wcs.x as f64,
                                pick_wcs.y as f64,
                            )
                        });
                        if let Some(pick) = pick {
                            let center = glam::Vec3::new(pick.x as f32, pick.y as f32, pick_wcs.z);
                            let result = self.tabs[i]
                                .active_cmd
                                .as_mut()
                                .map(|c| c.on_structure_pick(pick.handle, center));
                            self.command_line
                                .push_info(&format!("{} acquired.", pick.label));
                            result
                        } else {
                            let msg = self.tabs[i]
                                .active_cmd
                                .as_ref()
                                .map(|c| c.object_pick_miss_message())
                                .unwrap_or("No object near click.");
                            self.command_line.push_error(msg);
                            None
                        }
                    } else if self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.needs_entity_pick())
                        .unwrap_or(false)
                    {
                        let vp_mat2 = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                        let all_wires2 = self.tabs[i].scene.hit_test_wires();
                        let hit = scene::pick::hit_test::click_hit(p, &all_wires2[..], vp_mat2, bounds)
                            .and_then(|s| Scene::handle_from_wire_name(s));
                        if let Some(handle) = hit {
                            // Some commands (e.g. SS_CATCHMENT) need the entity
                            // body before `on_entity_pick` can advance.
                            let inject_first = self.tabs[i]
                                .active_cmd
                                .as_ref()
                                .map(|c| c.inject_before_entity_pick())
                                .unwrap_or(false);
                            if inject_first {
                                if let Some(entity) =
                                    self.tabs[i].scene.document.get_entity(handle).cloned()
                                {
                                    if let Some(cmd) = self.tabs[i].active_cmd.as_mut() {
                                        cmd.inject_picked_entity(entity);
                                    }
                                }
                            }

                            let result = self.tabs[i]
                                .active_cmd
                                .as_mut()
                                .map(|c| c.on_entity_pick(handle, pick_wcs));
                            // HATCHEDIT: after pick, inject hatch model data into the command.
                            if self.tabs[i]
                                .active_cmd
                                .as_ref()
                                .map(|c| c.name() == "HATCHEDIT")
                                .unwrap_or(false)
                            {
                                if let Some(model) =
                                    self.tabs[i].scene.hatches.get(&handle).cloned()
                                {
                                    use crate::command::CadCommand;
                                    use crate::modules::draw::draw::hatchedit::HatcheditCommand;
                                    let cmd: Box<dyn CadCommand> =
                                        Box::new(HatcheditCommand::with_handle(
                                            handle,
                                            model.name.clone(),
                                            model.scale,
                                            model.angle_offset,
                                        ));
                                    self.command_line.push_info(&cmd.prompt());
                                    self.tabs[i].active_cmd = Some(cmd);
                                } else {
                                    self.command_line
                                        .push_error("HATCHEDIT: not a hatch entity.");
                                    self.tabs[i].active_cmd = None;
                                }
                            }
                            // DIMTEDIT / MLEADERADD / MLEADERREMOVE: inject cloned entity via trait.
                            {
                                let needs_inject = self.tabs[i]
                                    .active_cmd
                                    .as_ref()
                                    .map(|c| {
                                        matches!(
                                            c.name(),
                                            "DIMTEDIT" | "MLEADERADD" | "MLEADERREMOVE"
                                        )
                                    })
                                    .unwrap_or(false);
                                if needs_inject {
                                    if let Some(entity) =
                                        self.tabs[i].scene.document.get_entity(handle).cloned()
                                    {
                                        if let Some(cmd) = self.tabs[i].active_cmd.as_mut() {
                                            cmd.inject_picked_entity(entity);
                                            let prompt = cmd.prompt();
                                            self.command_line.push_info(&prompt);
                                        }
                                    }
                                }
                            }
                            result
                        } else {
                            self.command_line.push_info("Nothing found at that point.");
                            None
                        }
                    } else if self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.needs_tangent_pick())
                        .unwrap_or(false)
                    {
                        if let Some(obj) = tangent_obj_at_click {
                            self.tabs[i]
                                .active_cmd
                                .as_mut()
                                .map(|c| c.on_tangent_point(obj, pick_wcs))
                        } else {
                            self.command_line.push_info("Select a tangent object.");
                            None
                        }
                    } else {
                        // A scalar typed into the dynamic-input box but not
                        // yet confirmed with Enter is applied before the
                        // point pick — e.g. an OFFSET distance typed and then
                        // clicked takes effect rather than being discarded.
                        let wants_text = self.tabs[i]
                            .active_cmd
                            .as_ref()
                            .map(|c| c.wants_text_input())
                            .unwrap_or(false);
                        if wants_text {
                            if let Some(text) = self.tabs[i]
                                .dyn_fields
                                .iter()
                                .find_map(|f| f.buffer.clone())
                            {
                                let text = super::expr_eval::eval_to_string(text.trim());
                                if let Some(c) = self.tabs[i].active_cmd.as_mut() {
                                    c.on_text_input(&text);
                                }
                                for f in self.tabs[i].dyn_fields.iter_mut() {
                                    f.buffer = None;
                                }
                                self.tabs[i].dyn_active = 0;
                            }
                        }
                        self.last_point = Some(world_pt);
                        self.dyn_user_reshaped = false;
                        self.sync_dyn_fields();
                        self.reset_tracking_after_point();
                        self.tabs[i]
                            .active_cmd
                            .as_mut()
                            .map(|c| c.on_point(world_pt))
                    };

                    if let Some(r) = result {
                        let task = self.apply_cmd_result(r);
                        let mut sel = self.tabs[i].scene.selection.borrow_mut();
                        sel.left_down = false;
                        sel.left_press_pos = None;
                        sel.left_press_time = None;
                        sel.left_dragging = false;
                        return task;
                    }
                    let mut sel = self.tabs[i].scene.selection.borrow_mut();
                    sel.left_down = false;
                    sel.left_press_pos = None;
                    sel.left_press_time = None;
                    sel.left_dragging = false;
                    return Task::none();
                }

                let (is_down2, is_dragging, box_anchor, box_crossing, _vp_size, elapsed_ms) = {
                    let sel = self.tabs[i].scene.selection.borrow();
                    let elapsed = sel
                        .left_press_time
                        .map(|t| Instant::now().duration_since(t).as_millis())
                        .unwrap_or(u128::MAX);
                    (
                        sel.left_down,
                        sel.left_dragging,
                        sel.box_anchor,
                        sel.box_crossing,
                        sel.vp_size,
                        elapsed,
                    )
                };

                let mut selection_just_completed = false;

                // Active-tile-local selection: tile-sized bounds and the box
                // anchor mapped into the tile, so box / crossing selection
                // matches the active pane (p is already tile-local).
                let vp_size = (tile_vw, tile_vh);
                let box_anchor = box_anchor.map(|a| iced::Point {
                    x: a.x - tile_off.x,
                    y: a.y - tile_off.y,
                });

                if is_down2 {
                    let bounds = iced::Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: vp_size.0,
                        height: vp_size.1,
                    };

                    if is_dragging {
                        if elapsed_ms < POLY_START_DELAY_MS {
                            if let Some(a) = box_anchor {
                                let crossing = box_crossing;
                                let all_wires = self.tabs[i].scene.hit_test_wires();
                                let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                                let mut handles: Vec<Handle> = scene::pick::hit_test::box_hit(
                                    a,
                                    p,
                                    crossing,
                                    &all_wires[..],
                                    vp_mat,
                                    bounds,
                                )
                                .into_iter()
                                .filter_map(|s| Scene::handle_from_wire_name(s))
                                .collect();
                                handles.extend(scene::pick::hit_test::box_hit_hatch(
                                    a,
                                    p,
                                    crossing,
                                    &self.tabs[i].scene.visible_hatches_for_click(),
                                    vp_mat,
                                    bounds,
                                ));
                                // Box/lasso accumulates like individual picks
                                // (issue #83): a plain box adds to the current
                                // selection, Shift+box removes the boxed
                                // entities. Esc / empty-space click still clears.
                                if self.shift_down {
                                    for h in &handles {
                                        self.tabs[i].scene.deselect_entity(*h);
                                    }
                                } else {
                                    for h in &handles {
                                        self.tabs[i].scene.select_entity(*h, false);
                                    }
                                    self.tabs[i].scene.expand_selection_for_groups(&handles);
                                }
                                self.refresh_properties();
                                selection_just_completed = true;
                            }
                        } else {
                            let (poly_pts, crossing) = {
                                let sel = self.tabs[i].scene.selection.borrow();
                                // Map lasso points into the active tile.
                                let pts: Vec<iced::Point> = sel
                                    .poly_points
                                    .iter()
                                    .map(|pp| iced::Point {
                                        x: pp.x - tile_off.x,
                                        y: pp.y - tile_off.y,
                                    })
                                    .collect();
                                (pts, sel.poly_crossing)
                            };
                            self.tabs[i].scene.selection.borrow_mut().poly_last_crossing = crossing;
                            let all_wires = self.tabs[i].scene.hit_test_wires();
                            let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                            let mut handles: Vec<Handle> = scene::pick::hit_test::poly_hit(
                                &poly_pts,
                                crossing,
                                &all_wires[..],
                                vp_mat,
                                bounds,
                            )
                            .into_iter()
                            .filter_map(|s| Scene::handle_from_wire_name(s))
                            .collect();
                            handles.extend(scene::pick::hit_test::poly_hit_hatch(
                                &poly_pts,
                                crossing,
                                &self.tabs[i].scene.visible_hatches_for_click(),
                                vp_mat,
                                bounds,
                            ));
                            // Selection filter: keep only allowed types.
                            handles.retain(|&h| self.tabs[i].scene.passes_selection_filter(h));
                            // Accumulate like the box path (issue #83): plain
                            // lasso adds, Shift+lasso removes. An empty lasso
                            // leaves the current selection untouched so a stray
                            // drag never discards hard-won picks.
                            if self.shift_down {
                                for h in &handles {
                                    self.tabs[i].scene.deselect_entity(*h);
                                }
                            } else {
                                for h in &handles {
                                    self.tabs[i].scene.select_entity(*h, false);
                                }
                                self.tabs[i].scene.expand_selection_for_groups(&handles);
                            }
                            self.refresh_properties();
                            selection_just_completed = true;
                        }
                        let mut sel = self.tabs[i].scene.selection.borrow_mut();
                        sel.poly_active = false;
                        sel.poly_points.clear();
                        sel.poly_crossing = false;
                        sel.box_anchor = None;
                        sel.box_current = None;
                    } else {
                        if box_anchor.is_none() {
                            let all_wires = self.tabs[i].scene.hit_test_wires();
                            let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);

                            // Selection cycling: where two or more objects
                            // overlap, open a list box to pick which one; a
                            // single object falls through to the normal click.
                            // Gated behind the toggle, so default picking is
                            // unchanged when off.
                            let mut handled_by_cycling = false;
                            if self.selection_cycling {
                                let cands: Vec<Handle> = scene::pick::hit_test::click_hits_all(
                                    p,
                                    &all_wires[..],
                                    vp_mat,
                                    bounds,
                                )
                                .into_iter()
                                .filter_map(|s| Scene::handle_from_wire_name(s))
                                .filter(|&h| self.tabs[i].scene.passes_selection_filter(h))
                                .collect();
                                if cands.len() >= 2 {
                                    // Overlap: open the list box at the cursor.
                                    self.cycle_candidates = Some((p_full, cands));
                                    handled_by_cycling = true;
                                }
                            }

                            if !handled_by_cycling {
                                let hit =
                                    scene::pick::hit_test::click_hit(p, &all_wires[..], vp_mat, bounds)
                                        .and_then(|s| Scene::handle_from_wire_name(s))
                                        .or_else(|| {
                                            scene::pick::hit_test::click_hit_hatch(
                                                p,
                                                &self.tabs[i].scene.visible_hatches_for_click(),
                                                vp_mat,
                                                bounds,
                                            )
                                        })
                                        .or_else(|| {
                                            // Block-internal hatch: resolve to the
                                            // parent Insert (AutoCAD behaviour).
                                            scene::pick::hit_test::click_hit_insert_hatch(
                                                p,
                                                &self.tabs[i].scene.insert_hatches_for_click(),
                                                vp_mat,
                                                bounds,
                                            )
                                        })
                                        .or_else(|| {
                                            // 3D solids: click anywhere on the shaded
                                            // body, not just the thin projected edges.
                                            self.tabs[i].scene.mesh_click_hit(p, vp_mat, bounds)
                                        });
                                // Selection filter: drop a pick whose type is excluded.
                                let hit =
                                    hit.filter(|&h| self.tabs[i].scene.passes_selection_filter(h));
                                if let Some(handle) = hit {
                                    // Individual picks accumulate (issue #47):
                                    // each plain click adds to the selection,
                                    // Shift+click removes the picked entity.
                                    // Esc / empty-space click clears.
                                    if self.shift_down {
                                        self.tabs[i].scene.deselect_entity(handle);
                                    } else {
                                        self.tabs[i].scene.select_entity(handle, false);
                                        self.tabs[i].scene.expand_selection_for_groups(&[handle]);
                                    }
                                    self.refresh_properties();
                                    selection_just_completed = true;
                                } else {
                                    // Empty-space click only ARMS a box here; it
                                    // no longer clears the selection, so a box can
                                    // add to it (issue #83). The box completion
                                    // (or Esc) decides what happens to the
                                    // selection.
                                    let mut sel = self.tabs[i].scene.selection.borrow_mut();
                                    // Full-canvas space: ViewportMove updates
                                    // box_current in canvas coords and the overlay
                                    // draws there; release maps back into the tile.
                                    sel.box_anchor = Some(p_full);
                                    sel.box_current = Some(p_full);
                                    sel.box_crossing = false;
                                }
                            }
                        } else {
                            let a = box_anchor.unwrap();
                            let crossing = box_crossing;
                            let all_wires = self.tabs[i].scene.hit_test_wires();
                            let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                            let mut handles: Vec<Handle> = scene::pick::hit_test::box_hit(
                                a,
                                p,
                                crossing,
                                &all_wires[..],
                                vp_mat,
                                bounds,
                            )
                            .into_iter()
                            .filter_map(|s| Scene::handle_from_wire_name(s))
                            .collect();
                            handles.extend(scene::pick::hit_test::box_hit_hatch(
                                a,
                                p,
                                crossing,
                                &self.tabs[i].scene.visible_hatches_for_click(),
                                vp_mat,
                                bounds,
                            ));
                            // Selection filter: keep only allowed types.
                            handles.retain(|&h| self.tabs[i].scene.passes_selection_filter(h));
                            // Accumulate (issue #83): a plain box adds to the
                            // current selection, Shift+box removes the boxed
                            // entities. An empty box leaves the selection alone
                            // so an accidental empty drag never discards it.
                            if self.shift_down {
                                for h in &handles {
                                    self.tabs[i].scene.deselect_entity(*h);
                                }
                            } else {
                                for h in &handles {
                                    self.tabs[i].scene.select_entity(*h, false);
                                }
                                self.tabs[i].scene.expand_selection_for_groups(&handles);
                            }
                            self.refresh_properties();
                            let mut sel = self.tabs[i].scene.selection.borrow_mut();
                            sel.box_last = Some((a, p));
                            sel.box_last_crossing = crossing;
                            sel.box_anchor = None;
                            sel.box_current = None;
                            sel.box_crossing = false;
                            selection_just_completed = true;
                        }
                    }

                    let mut sel = self.tabs[i].scene.selection.borrow_mut();
                    sel.left_down = false;
                    sel.left_press_pos = None;
                    sel.left_press_time = None;
                    sel.left_dragging = false;
                }

                if is_gathering && selection_just_completed {
                    let handles: Vec<Handle> = self.tabs[i]
                        .scene
                        .selected_entities()
                        .into_iter()
                        .map(|(h, _)| h)
                        .collect();
                    if let Some(cmd) = self.tabs[i].active_cmd.as_mut() {
                        let result = cmd.on_selection_complete(handles);
                        return self.apply_cmd_result(result);
                    }
                }

                // ── Double-click in Model Space: DDEDIT for Text/MText ────
                if is_click
                    && is_down
                    && self.tabs[i].active_cmd.is_none()
                    && self.tabs[i].scene.current_layout == "Model"
                {
                    let now = Instant::now();
                    let is_double_model = self
                        .last_vp_click_time
                        .map(|t| {
                            let dt = now.duration_since(t).as_millis();
                            let last = self.last_vp_click_pos.unwrap_or(p);
                            let d = (p.x - last.x).hypot(p.y - last.y);
                            dt < 400 && d < 8.0
                        })
                        .unwrap_or(false);

                    self.last_vp_click_time = Some(now);
                    self.last_vp_click_pos = Some(p);

                    if is_double_model {
                        let (vw, vh) = self.tabs[i].scene.selection.borrow().vp_size;
                        let bounds = iced::Rectangle {
                            x: 0.0,
                            y: 0.0,
                            width: vw,
                            height: vh,
                        };
                        let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                        let all_wires = self.tabs[i].scene.hit_test_wires();
                        let hit = scene::pick::hit_test::click_hit(p, &all_wires[..], vp_mat, bounds)
                            .and_then(|s| Scene::handle_from_wire_name(s));
                        if let Some(handle) = hit {
                            // Any text-bearing entity opens its in-place editor
                            // (plain box or rich MText editor, per type). A
                            // Leader resolves to the entity it annotates.
                            let is_editable_text = self.tabs[i]
                                .scene
                                .document
                                .get_entity(handle)
                                .is_some_and(|e| {
                                    super::text_inline::read_text_field(e).is_some()
                                        || matches!(e, AcadEntityType::Leader(_))
                                });
                            if is_editable_text {
                                return self.begin_text_edit(handle);
                            }
                        }
                    }
                }

                // ── Double-click: enter/exit MSPACE ───────────────────────
                // Only when no command is running, no drag, and we're in paper space.
                if is_click
                    && is_down   // ensures there was a matching left-press
                    && self.tabs[i].active_cmd.is_none()
                    && self.tabs[i].scene.current_layout != "Model"
                {
                    let now = Instant::now();
                    let is_double = self
                        .last_vp_click_time
                        .map(|t| {
                            let dt = now.duration_since(t).as_millis();
                            let last = self.last_vp_click_pos.unwrap_or(p);
                            let d = (p.x - last.x).hypot(p.y - last.y);
                            dt < 400 && d < 8.0
                        })
                        .unwrap_or(false);

                    self.last_vp_click_time = Some(now);
                    self.last_vp_click_pos = Some(p);

                    if is_double {
                        let (vw, vh) = self.tabs[i].scene.selection.borrow().vp_size;
                        let bounds = iced::Rectangle {
                            x: 0.0,
                            y: 0.0,
                            width: vw,
                            height: vh,
                        };

                        // 1) Try direct wire hit — works when the border is clicked.
                        let hit_vp: Option<acadrust::Handle> = {
                            let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);
                            let all_wires = self.tabs[i].scene.hit_test_wires();
                            scene::pick::hit_test::click_hit(p, &all_wires[..], vp_mat, bounds)
                                .and_then(|s| Scene::handle_from_wire_name(s))
                                .and_then(|h| {
                                    if let Some(AcadEntityType::Viewport(vp)) =
                                        self.tabs[i].scene.document.get_entity(h)
                                    {
                                        if Scene::is_content_viewport(vp) {
                                            Some(h)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                })
                        };

                        // 2) Geometric fallback: check if the cursor is inside any
                        //    viewport's bounding rectangle in paper space.  This handles
                        //    double-clicks on model-entity content wires or empty areas.
                        let hit_vp = hit_vp.or_else(|| {
                            let paper_pt = self.tabs[i]
                                .scene
                                .camera
                                .borrow()
                                .pick_on_target_plane(p, bounds);
                            self.tabs[i]
                                .scene
                                .viewport_at_paper_point(paper_pt.x, paper_pt.y)
                        });

                        if let Some(handle) = hit_vp {
                            return Task::done(Message::EnterViewport(handle));
                        } else if self.tabs[i].scene.active_viewport.is_some() {
                            // Double-clicked outside all viewports while in MSPACE → exit.
                            return Task::done(Message::ExitViewport);
                        }
                    }
                }

                Task::none()
            }

            Message::ViewportRightPress => {
                let i = self.active_tab;
                self.ribbon.close_dropdown();
                let mut sel = self.tabs[i].scene.selection.borrow_mut();
                let Some(p) = sel.last_move_pos else {
                    return Task::none();
                };
                sel.context_menu = None;
                sel.right_down = true;
                sel.right_press_pos = Some(p);
                sel.right_last_pos = Some(p);
                sel.right_dragging = false;
                Task::none()
            }

            Message::ViewportRightRelease => {
                let i = self.active_tab;
                let mut sel = self.tabs[i].scene.selection.borrow_mut();
                let Some(_p) = sel.last_move_pos else {
                    return Task::none();
                };
                if sel.right_down {
                    if !sel.right_dragging {
                        sel.context_menu = sel.last_move_pos;
                        sel.draworder_submenu = false;
                    }
                    sel.right_down = false;
                    sel.right_press_pos = None;
                    sel.right_last_pos = None;
                    sel.right_dragging = false;
                }
                Task::none()
            }

            Message::ViewportMiddlePress => {
                let i = self.active_tab;
                self.ribbon.close_dropdown();
                let now = Instant::now();
                let is_double = {
                    let sel = self.tabs[i].scene.selection.borrow();
                    sel.middle_last_press_time
                        .map(|t| now.duration_since(t).as_millis() < 300)
                        .unwrap_or(false)
                };
                {
                    let mut sel = self.tabs[i].scene.selection.borrow_mut();
                    let Some(p) = sel.last_move_pos else {
                        return Task::none();
                    };
                    sel.middle_down = true;
                    sel.middle_last_pos = Some(p);
                    sel.middle_last_press_time = Some(now);
                }
                if is_double {
                    self.tabs[i].scene.fit_all();
                    self.command_line.push_output("Zoom Extents");
                }
                Task::none()
            }

            Message::ViewportMiddleRelease => {
                let i = self.active_tab;
                let mut sel = self.tabs[i].scene.selection.borrow_mut();
                sel.middle_down = false;
                sel.middle_last_pos = None;
                Task::none()
            }

            Message::ViewportScroll(delta) => {
                let s = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y * 0.01,
                };
                let i = self.active_tab;
                let cursor = self.tabs[i].scene.selection.borrow().last_move_pos;
                let (vw, vh) = self.tabs[i].scene.selection.borrow().vp_size;
                let bounds = iced::Rectangle {
                    x: 0.0,
                    y: 0.0,
                    width: vw,
                    height: vh,
                };
                if self.tabs[i].scene.active_viewport.is_some() {
                    // In MSPACE: zoom the active viewport's model-space view,
                    // keeping the model point under the cursor stationary.
                    let cursor_paper = cursor.map(|cp| {
                        let pt = self.tabs[i]
                            .scene
                            .camera
                            .borrow()
                            .pick_on_target_plane(cp, bounds);
                        glam::Vec2::new(pt.x, pt.y)
                    });
                    self.tabs[i].scene.zoom_active_viewport(s, cursor_paper);
                    // Bump so the GPU re-uploads the viewport's re-culled wire
                    // set after zooming inside it.
                    self.tabs[i].scene.camera_generation += 1;
                } else {
                    // Model space: zoom about the cursor within the active
                    // tile so the point under it stays put in that pane.
                    let tile_b = self.tabs[i].scene.active_model_tile_bounds(vw, vh);
                    let mut cam = self.tabs[i].scene.camera.borrow_mut();
                    if let Some(cursor) = cursor {
                        let local = iced::Point {
                            x: cursor.x - tile_b.x,
                            y: cursor.y - tile_b.y,
                        };
                        let tb = iced::Rectangle {
                            x: 0.0,
                            y: 0.0,
                            width: tile_b.width,
                            height: tile_b.height,
                        };
                        cam.zoom_about_point(local, tb, s);
                    } else {
                        cam.zoom(s);
                    }
                    drop(cam);
                    self.tabs[i].scene.camera_generation += 1;
                }
                Task::none()
            }

            Message::ViewportClick => {
                let i = self.active_tab;
                let rot = self.tabs[i].scene.active_view_rotation_mat();
                let (vw, vh) = self.tabs[i].scene.selection.borrow().vp_size;
                // The ViewCube draws in the top-right of whichever area
                // owns it: the full canvas in model space, or the active
                // viewport's screen rectangle in a paper layout. Map the
                // cursor into that area before hit-testing so paper-space
                // picks line up with the gizmo.
                let (cx, cy, w, h) = match self.tabs[i]
                    .scene
                    .active_viewport
                    .and_then(|hndl| self.tabs[i].scene.viewport_screen_rect(hndl, (vw, vh)))
                {
                    Some(rect) => (
                        self.cursor_pos.x - rect.x,
                        self.cursor_pos.y - rect.y,
                        rect.width,
                        rect.height,
                    ),
                    None => {
                        // Model layout: hit-test within the active tile.
                        let tb = self.tabs[i].scene.active_model_tile_bounds(vw, vh);
                        (
                            self.cursor_pos.x - tb.x,
                            self.cursor_pos.y - tb.y,
                            tb.width,
                            tb.height,
                        )
                    }
                };
                if let Some(region) = scene::hit_test(cx, cy, w, h, rot, VIEWCUBE_PX) {
                    return Task::done(Message::ViewCubeSnap(region));
                }
                Task::none()
            }

            Message::WindowResized(w, h) => {
                self.vp_size = ((w - 440.0).max(200.0), h);
                Task::none()
            }

            Message::ViewCubeSnap(region) => {
                let i = self.active_tab;
                let mut region = region;
                // "Already there → flip to opposite" check: compare the
                // current gaze direction with the region's target gaze.
                let target_dir = region.snap_direction();
                let cur_dir = {
                    let cam = self.tabs[i].scene.camera.borrow();
                    cam.rotation * glam::Vec3::Z
                };
                if cur_dir.dot(target_dir) > 0.9999 {
                    region = region.opposite();
                }
                let eye_dir = region.snap_direction();

                if self.tabs[i].scene.active_viewport.is_some() {
                    self.tabs[i]
                        .scene
                        .snap_active_viewport_to_direction(eye_dir);
                } else {
                    let mut cam = self.tabs[i].scene.camera.borrow_mut();
                    cam.snap_to_direction(eye_dir);
                }
                self.tabs[i].scene.camera_generation += 1;
                self.command_line
                    .push_output(&format!("View: {}", region.label()));
                Task::none()
            }

            Message::GripDwellTick => {
                let i = self.active_tab;
                // Reuse the move-time logic — `p` is the last cursor
                // position the viewport saw, which is also what the
                // hover state was last set with.
                let p = self.tabs[i]
                    .scene
                    .selection
                    .borrow()
                    .last_move_pos
                    .unwrap_or(self.cursor_pos);
                self.update_grip_hover(i, p);
                Task::none()
            }

            Message::VisibilityPick(idx) => {
                if let Some(popup) = self.visibility_popup.take() {
                    self.apply_visibility_state(popup.insert_handle, idx);
                }
                Task::none()
            }

            Message::GripMenuPick(idx) => {
                let i = self.active_tab;
                let Some(popup) = self.grip_popup.take() else {
                    return Task::none();
                };
                self.grip_hover = None;
                let Some(item) = popup.items.get(idx).cloned() else {
                    return Task::none();
                };
                use crate::entities::traits::EntityTypeOps;
                use crate::scene::model::object::GripMenuAction;
                if matches!(
                    item.action,
                    GripMenuAction::Stretch
                        | GripMenuAction::MoveWithLeader
                        | GripMenuAction::MoveIndependent
                ) {
                    // Stretch / Move = grab this grip. Engage it so the next
                    // click places it (click-move-click) — same as picking the
                    // grip directly in the viewport. Without this the menu just
                    // closed and the grip never became hot (issue #48).
                    if let Some(g) = self.tabs[i]
                        .selected_grips
                        .iter()
                        .find(|g| g.id == popup.grip_id)
                    {
                        // "Move with Leader" drags the whole multileader; the
                        // others move just the picked grip.
                        let (grip_id, is_translate) =
                            if matches!(item.action, GripMenuAction::MoveWithLeader) {
                                (crate::entities::multileader::MOVE_ALL_GRIP, true)
                            } else {
                                (popup.grip_id, g.is_midpoint)
                            };
                        self.tabs[i].active_grip = Some(GripEdit {
                            handle: popup.handle,
                            grip_id,
                            is_translate,
                            origin_world: g.world.as_vec3(),
                            last_world: g.world.as_vec3(),
                        });
                    }
                    return Task::none();
                }
                // Actions that need a follow-up number stash a pending
                // state + prompt; the next typed value drives
                // `apply_grip_menu_value`.
                let prompt = self.tabs[i]
                    .scene
                    .document
                    .get_entity(popup.handle)
                    .and_then(|e| e.grip_menu_value_prompt(popup.grip_id, item.action));
                if let Some(label) = prompt {
                    self.grip_pending = Some(super::GripPendingValue {
                        handle: popup.handle,
                        grip_id: popup.grip_id,
                        action: item.action,
                        label,
                    });
                    self.command_line.push_info(&format!("{label}:"));
                    return self.focus_cmd_input();
                }
                // One-shot action — apply immediately.
                self.push_undo_snapshot(i, item.label);
                // For Add Leader, the new arrow becomes the last grip; remember
                // its id so we can grab it for placement right after.
                let add_leader_gid = if matches!(item.action, GripMenuAction::AddLeader) {
                    self.tabs[i]
                        .scene
                        .document
                        .get_entity(popup.handle)
                        .and_then(|e| match e {
                            acadrust::EntityType::MultiLeader(ml) => Some(
                                ml.context
                                    .leader_roots
                                    .iter()
                                    .flat_map(|r| r.lines.iter())
                                    .map(|l| l.points.len())
                                    .sum::<usize>(),
                            ),
                            _ => None,
                        })
                } else {
                    None
                };
                if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(popup.handle) {
                    entity.apply_grip_menu(popup.grip_id, item.action);
                }
                self.tabs[i].scene.bump_geometry();
                self.tabs[i].dirty = true;
                self.refresh_selected_grips();
                self.refresh_properties();
                // Grab the new arrow so it follows the cursor (click places it,
                // Esc removes it).
                if let Some(new_gid) = add_leader_gid {
                    if let Some(g) = self.tabs[i].selected_grips.iter().find(|g| g.id == new_gid) {
                        self.tabs[i].active_grip = Some(GripEdit {
                            handle: popup.handle,
                            grip_id: new_gid,
                            is_translate: false,
                            origin_world: g.world.as_vec3(),
                            last_world: g.world.as_vec3(),
                        });
                        self.grip_add_provisional = Some((popup.handle, new_gid));
                    }
                }
                Task::none()
            }

            // ── Snap / mode toggles ───────────────────────────────────────
            Message::ToggleSnapEnabled => {
                self.snapper.toggle_global();
                Task::none()
            }
            Message::ToggleGridSnap => {
                self.snapper.toggle(crate::snap::SnapType::Grid);
                Task::none()
            }
            Message::ToggleGrid => {
                self.show_grid ^= true;
                Task::none()
            }
            Message::ToggleOrtho => {
                self.ortho_mode ^= true;
                if self.ortho_mode {
                    self.polar_mode = false;
                }
                // If the user manually toggles ortho during a command that
                // suppressed it (e.g. RECTANG), the toggle is permanent —
                // don't restore the pre-command state when the command ends.
                self.rect_suppressed_ortho = false;
                Task::none()
            }
            Message::ToggleLineweightDisplay => {
                let i = self.active_tab;
                if i < self.tabs.len() {
                    let h = &mut self.tabs[i].scene.document.header;
                    h.lineweight_display = !h.lineweight_display;
                    // No retessellate — the wire shader reads the flag from uniforms.
                    self.tabs[i].dirty = true;
                }
                Task::none()
            }
            Message::TogglePolar => {
                self.polar_mode ^= true;
                if self.polar_mode {
                    self.ortho_mode = false;
                }
                Task::none()
            }
            Message::ToggleDynInput => {
                self.dyn_input ^= true;
                Task::none()
            }
            Message::ToggleViewCube => {
                self.show_viewcube ^= true;
                Task::none()
            }
            Message::ToggleProperties => {
                self.show_properties ^= true;
                Task::none()
            }
            Message::ToggleFileTabs => {
                self.show_file_tabs ^= true;
                Task::none()
            }
            Message::ToggleLayoutTabs => {
                self.show_layout_tabs ^= true;
                Task::none()
            }
            Message::ToggleOTrack => {
                self.snapper.otrack_enabled ^= true;
                if !self.snapper.otrack_enabled {
                    self.snapper.clear_tracking();
                }
                Task::none()
            }
            Message::SetPolarAngle(deg) => {
                self.polar_increment_deg = deg;
                self.polar_mode = true;
                self.ortho_mode = false;
                Task::none()
            }
            Message::SetAnnotationScale(scale) => {
                self.scale_popup_open = false;
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.scene.annotation_scale = scale;
                    tab.scene.bump_geometry();
                }
                Task::none()
            }
            Message::SetViewportScale(scale) => {
                self.scale_popup_open = false;
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.scene.set_viewport_scale(scale);
                }
                Task::none()
            }
            Message::ToggleScalePopup => {
                self.scale_popup_open ^= true;
                Task::none()
            }
            Message::CloseScalePopup => {
                self.scale_popup_open = false;
                Task::none()
            }
            Message::ToggleStatusBarMenu => {
                self.statusbar_menu_open ^= true;
                Task::none()
            }
            Message::CloseStatusBarMenu => {
                self.statusbar_menu_open = false;
                Task::none()
            }
            Message::ToggleStatusPill(pill) => {
                // Keep the menu open so several pills can be toggled in a row.
                self.statusbar_config.toggle(pill);
                Task::none()
            }
            Message::ToggleCleanScreen => {
                self.clean_screen ^= true;
                Task::none()
            }
            Message::ToggleTransparencyDisplay => {
                let i = self.active_tab;
                if i < self.tabs.len() {
                    // No retessellate — the wire shader reads the flag from uniforms.
                    self.tabs[i].scene.transparency_display ^= true;
                }
                Task::none()
            }
            Message::ToggleQuickProperties => {
                self.quick_properties ^= true;
                Task::none()
            }
            Message::ToggleSelectionCycling => {
                self.selection_cycling ^= true;
                self.cycle_candidates = None;
                self.tabs[self.active_tab].scene.set_hover_highlight(None);
                Task::none()
            }
            Message::CycleSelect(handle) => {
                // Add the picked object to the current selection (accumulate).
                self.cycle_candidates = None;
                let i = self.active_tab;
                self.tabs[i].scene.set_hover_highlight(None);
                self.tabs[i].scene.select_entity(handle, false);
                self.tabs[i].scene.expand_selection_for_groups(&[handle]);
                self.refresh_properties();
                Task::none()
            }
            Message::CycleHover(handle) => {
                let i = self.active_tab;
                self.tabs[i].scene.set_hover_highlight(handle);
                Task::none()
            }
            Message::CycleHoverExit(handle) => {
                // Only clear if another row hasn't already taken the highlight;
                // enter/exit can fire out of order when moving between rows.
                let i = self.active_tab;
                if self.tabs[i].scene.hover_highlight == Some(handle) {
                    self.tabs[i].scene.set_hover_highlight(None);
                }
                Task::none()
            }
            Message::CycleCancel => {
                self.cycle_candidates = None;
                self.tabs[self.active_tab].scene.set_hover_highlight(None);
                Task::none()
            }
            Message::ToggleSelectionFilterPopup => {
                self.selection_filter_popup_open ^= true;
                Task::none()
            }
            Message::CloseSelectionFilterPopup => {
                self.selection_filter_popup_open = false;
                Task::none()
            }
            Message::ToggleSelectionFilterType(name) => {
                let f = &mut self.tabs[self.active_tab].scene.selection_filter;
                if !f.remove(&name) {
                    f.insert(name);
                }
                Task::none()
            }
            Message::ToggleUnitsPopup => {
                self.units_popup_open ^= true;
                Task::none()
            }
            Message::CloseUnitsPopup => {
                self.units_popup_open = false;
                Task::none()
            }
            Message::SetDrawingUnits(code) => {
                self.units_popup_open = false;
                let i = self.active_tab;
                self.tabs[i].scene.document.header.insertion_units = code;
                self.tabs[i].dirty = true;
                Task::none()
            }
            Message::ToggleIsolatePopup => {
                self.isolate_popup_open ^= true;
                Task::none()
            }
            Message::CloseIsolatePopup => {
                self.isolate_popup_open = false;
                Task::none()
            }
            Message::ToggleSnap(t) => {
                self.snapper.toggle(t);
                Task::none()
            }
            Message::ToggleSnapPopup => {
                self.snap_popup_open ^= true;
                Task::none()
            }
            Message::CloseSnapPopup => {
                self.snap_popup_open = false;
                Task::none()
            }
            Message::SnapSelectAll => {
                self.snapper.enable_all();
                Task::none()
            }
            Message::SnapClearAll => {
                self.snapper.disable_all();
                Task::none()
            }

            // ── Ribbon dropdowns ──────────────────────────────────────────
            Message::ToggleRibbonDropdown(id) => {
                self.ribbon.toggle_dropdown(&id);
                Task::none()
            }
            Message::CloseRibbonDropdown => {
                self.ribbon.close_dropdown();
                Task::none()
            }
            Message::DropdownSelectItem { dropdown_id, cmd } => {
                self.ribbon.select_dropdown_item(dropdown_id, cmd);
                self.ribbon.activate_tool(cmd);
                self.dispatch_command(cmd)
            }

            Message::DeleteSelected => {
                // In the MText preview, Delete removes text at the caret.
                if self.mtext_editor.as_ref().is_some_and(|e| e.show_preview) {
                    self.mtext_delete();
                    return Task::none();
                }
                let i = self.active_tab;
                self.tabs[i].scene.selection.borrow_mut().context_menu = None;
                let handles: Vec<_> = self.tabs[i].scene.selected.iter().cloned().collect();
                if !handles.is_empty() {
                    self.push_undo_snapshot(i, "ERASE");
                    self.tabs[i].scene.erase_entities(&handles);
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::SetShiftDown(down) => {
                self.shift_down = down;
                Task::none()
            }

            // ── In-place MText editor ───────────────────────────────────
            Message::MTextEdit(action) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.content.perform(action);
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextFmt(kind) => {
                self.mtext_apply_fmt(kind);
                Task::none()
            }
            Message::MTextHeight(s) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.height = s;
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextColor(aci) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.color_aci = aci;
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextStyle(s) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.style = s;
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextFont(f) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.font = if f == "[Style default]" {
                        String::new()
                    } else {
                        f
                    };
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextOblique(s) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.oblique = s;
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextWidth(s) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.width = s;
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextCharSpace(s) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.char_space = s;
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextJustify(ap) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.attachment = ap;
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextAlign(a) => {
                self.mtext_apply_align(a);
                Task::none()
            }
            Message::MTextLineSpacing(f) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.line_spacing = f;
                }
                self.rebuild_mtext_preview();
                Task::none()
            }
            Message::MTextShowPreview(on) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.show_preview = on;
                }
                self.rebuild_mtext_preview();
                // Focus the text area when switching to Edit so the caret
                // shows and typing/clicking edits immediately.
                if on {
                    Task::none()
                } else {
                    iced::widget::operation::focus(iced::widget::Id::new(
                        super::view::MTEXT_TEXT_ID,
                    ))
                }
            }
            Message::MTextSelStart(off) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.sel_anchor = off;
                    ed.sel = Some((off, off));
                    ed.caret = off;
                    ed.caret_blink_on = true;
                }
                Task::none()
            }
            Message::MTextSelTo(off) => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    let a = ed.sel_anchor;
                    ed.sel = Some((a.min(off), a.max(off)));
                    ed.caret = off;
                    ed.caret_blink_on = true;
                }
                Task::none()
            }
            Message::MTextCaretMove(d) => {
                self.mtext_caret_move(d);
                Task::none()
            }
            Message::MTextCaretBlink => {
                if let Some(ed) = self.mtext_editor.as_mut() {
                    ed.caret_blink_on = !ed.caret_blink_on;
                }
                Task::none()
            }
            Message::MTextOk => {
                self.mtext_commit();
                Task::none()
            }
            Message::MTextCancel => {
                self.mtext_cancel();
                Task::none()
            }

            Message::TextInlineInput(s) => {
                if let Some(ed) = self.text_inline.as_mut() {
                    ed.value = s;
                }
                Task::none()
            }
            Message::TextInlineOk => {
                self.text_inline_commit();
                Task::none()
            }

            Message::DrawOrderSubmenuToggle => {
                let i = self.active_tab;
                let mut sel = self.tabs[i].scene.selection.borrow_mut();
                sel.draworder_submenu = !sel.draworder_submenu;
                Task::none()
            }

            Message::DrawOrderPickRef(above) => {
                let i = self.active_tab;
                self.tabs[i].scene.selection.borrow_mut().context_menu = None;
                let to_move: Vec<_> = self.tabs[i].scene.selected.iter().cloned().collect();
                if to_move.is_empty() {
                    self.command_line
                        .push_error("DRAWORDER: select entities first.");
                } else {
                    use crate::command::CadCommand;
                    let cmd = super::commands::DrawOrderRefCommand::new(to_move, above);
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                }
                Task::none()
            }

            Message::SelectSimilar => {
                let i = self.active_tab;
                self.tabs[i].scene.selection.borrow_mut().context_menu = None;
                let added = self.tabs[i].scene.select_similar();
                self.command_line
                    .push_output(&format!("Select Similar: {} added.", added));
                self.refresh_properties();
                Task::none()
            }

            Message::QSelectOpen => {
                let i = self.active_tab;
                self.tabs[i].scene.selection.borrow_mut().context_menu = None;
                // Seed the type filter from the first selected entity so a
                // right-click → Quick Select on a known object opens the
                // panel pre-tuned to that entity's type. Property defaults
                // to "(Any property)" so the user immediately picks what
                // they want to compare.
                let mut type_filter: Option<String> = None;
                if let Some(&h) = self.tabs[i].scene.selected.iter().next() {
                    if let Some(e) = self.tabs[i].scene.document.get_entity(h) {
                        use crate::entities::traits::entity_type_name;
                        type_filter = Some(entity_type_name(e).to_string());
                    }
                }
                self.qselect = Some(crate::app::QSelectState {
                    type_filter,
                    property: None,
                    operator: crate::app::QSelectOp::Eq,
                    value: String::new(),
                    append: false,
                });
                Task::none()
            }

            Message::QSelectClose => {
                self.qselect = None;
                Task::none()
            }

            Message::QSelectSetType(t) => {
                if let Some(state) = self.qselect.as_mut() {
                    // Drop the property when it no longer applies to the
                    // chosen type: type-specific fields like `start_x`
                    // would otherwise stay selected but never match.
                    let kept_property = state.property.clone().and_then(|p| {
                        let i = self.active_tab;
                        let props = self.tabs[i].scene.qselect_properties(t.as_deref());
                        if props.iter().any(|(f, _)| f == &p.field) {
                            Some(p)
                        } else {
                            None
                        }
                    });
                    state.type_filter = t;
                    state.property = kept_property;
                }
                Task::none()
            }

            Message::QSelectSetProperty(p) => {
                if let Some(state) = self.qselect.as_mut() {
                    state.property = p;
                }
                Task::none()
            }

            Message::QSelectSetOperator(op) => {
                if let Some(state) = self.qselect.as_mut() {
                    state.operator = op;
                }
                Task::none()
            }

            Message::QSelectSetValue(v) => {
                if let Some(state) = self.qselect.as_mut() {
                    state.value = v;
                }
                Task::none()
            }

            Message::QSelectSetAppend(b) => {
                if let Some(state) = self.qselect.as_mut() {
                    state.append = b;
                }
                Task::none()
            }

            Message::QSelectApply => {
                let Some(state) = self.qselect.take() else {
                    return Task::none();
                };
                let i = self.active_tab;
                let matched = self.tabs[i].scene.qselect(
                    state.type_filter.as_deref(),
                    state.property.as_ref().map(|p| p.field.as_str()),
                    state.operator,
                    &state.value,
                    state.append,
                );
                self.command_line
                    .push_output(&format!("QSELECT: {} object(s) selected.", matched));
                self.refresh_properties();
                Task::none()
            }

            // ── Properties panel messages ─────────────────────────────────
            Message::PropSelectionGroupChanged(group) => {
                self.tabs[self.active_tab].properties.selected_group = Some(group);
                self.refresh_properties();
                Task::none()
            }

            Message::RibbonLayerChanged(layer) => {
                let i = self.active_tab;
                self.ribbon.close_dropdown();
                let handles = self.property_target_handles(i);
                if handles.is_empty() {
                    // No selection — change the creation default. Persist
                    // into the tab's header (CLAYER) so it survives a tab
                    // switch and rides the next save. #21.
                    let handle = self.tabs[i]
                        .scene
                        .document
                        .layers
                        .get(&layer)
                        .map(|l| l.handle)
                        .unwrap_or(acadrust::types::Handle::NULL);
                    self.tabs[i].scene.document.header.current_layer_name = layer.clone();
                    self.tabs[i].scene.document.header.current_layer_handle = handle;
                    self.tabs[i].active_layer = layer.clone();
                    self.tabs[i].layers.current_layer = layer.clone();
                    self.tabs[i].dirty = true;
                    self.ribbon.active_layer = layer;
                } else {
                    // Apply to selection; leave the creation default alone
                    // (matches AutoCAD; "Make current" is a separate action).
                    self.push_undo_snapshot(i, "CHPROP");
                    for handle in handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_common_prop(entity, "layer", &layer);
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.ribbon.active_layer = layer;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::RibbonColorChanged(color) => {
                let i = self.active_tab;
                self.ribbon.prop_color_palette_open = false;
                self.ribbon.close_dropdown();
                let handles = self.property_target_handles(i);
                if handles.is_empty() {
                    // Persist the new default into the tab's header so it
                    // round-trips through tab switches and writes back on
                    // save (CECOLOR). #21.
                    self.tabs[i].scene.document.header.current_entity_color = color;
                    self.tabs[i].dirty = true;
                    self.ribbon.active_color = color;
                } else {
                    self.push_undo_snapshot(i, "CHPROP");
                    for handle in handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_color(entity, color);
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.ribbon.active_color = color;
                    self.refresh_properties();
                }
                Task::none()
            }
            Message::RibbonColorPaletteToggle => {
                self.ribbon.prop_color_palette_open ^= true;
                Task::none()
            }
            Message::RibbonLinetypeChanged(lt) => {
                let i = self.active_tab;
                self.ribbon.close_dropdown();
                let handles = self.property_target_handles(i);
                if handles.is_empty() {
                    // Persist into the tab's header (CELTYPE). Resolve to a
                    // handle when the name matches a line_types entry so the
                    // handle-based lookup stays in sync. #21.
                    let handle = self.tabs[i]
                        .scene
                        .document
                        .line_types
                        .iter()
                        .find(|x| x.name.eq_ignore_ascii_case(&lt))
                        .map(|x| x.handle)
                        .unwrap_or(acadrust::types::Handle::NULL);
                    self.tabs[i].scene.document.header.current_linetype_name = lt.clone();
                    self.tabs[i].scene.document.header.current_linetype_handle = handle;
                    self.tabs[i].dirty = true;
                    self.ribbon.active_linetype = lt;
                } else {
                    self.push_undo_snapshot(i, "CHPROP");
                    for handle in handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_common_prop(entity, "linetype", &lt);
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.ribbon.active_linetype = lt;
                    self.refresh_properties();
                }
                Task::none()
            }
            Message::RibbonLineweightChanged(lw) => {
                let i = self.active_tab;
                self.ribbon.close_dropdown();
                let handles = self.property_target_handles(i);
                if handles.is_empty() {
                    // Persist into the tab's header (CELWEIGHT). #21.
                    self.tabs[i].scene.document.header.current_line_weight = lw.value();
                    self.tabs[i].dirty = true;
                    self.ribbon.active_lineweight = lw;
                } else {
                    self.push_undo_snapshot(i, "CHPROP");
                    for handle in handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_line_weight(entity, lw);
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.ribbon.active_lineweight = lw;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::RibbonStyleChanged { key, name } => {
                use crate::modules::StyleKey;
                self.ribbon.close_dropdown();
                match key {
                    StyleKey::TextStyle => {
                        self.ribbon.active_text_style = name.clone();
                        let i = self.active_tab;
                        let found = self.tabs[i]
                            .scene
                            .document
                            .text_styles
                            .iter()
                            .find(|s| s.name == name)
                            .map(|ts| ts.handle);
                        if let Some(h) = found {
                            self.tabs[i].scene.document.header.current_text_style_handle = h;
                            self.tabs[i].scene.document.header.current_text_style_name = name;
                        }
                    }
                    StyleKey::DimStyle => {
                        self.ribbon.active_dim_style = name.clone();
                        let i = self.active_tab;
                        let found = self.tabs[i]
                            .scene
                            .document
                            .dim_styles
                            .get(&name)
                            .map(|ds| ds.handle);
                        if let Some(h) = found {
                            self.tabs[i].scene.document.header.current_dimstyle_handle = h;
                            self.tabs[i].scene.document.header.current_dimstyle_name = name;
                        }
                    }
                    StyleKey::MLeaderStyle => {
                        self.ribbon.active_mleader_style = name.clone();
                        let i = self.active_tab;
                        self.tabs[i].active_mleader_style = name;
                    }
                    StyleKey::TableStyle => {
                        self.ribbon.active_table_style = name;
                    }
                }
                Task::none()
            }

            Message::PropLayerChanged(layer) => {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    self.push_undo_snapshot(i, "CHPROP");
                    for &handle in &handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_common_prop(entity, "layer", &layer);
                        }
                    }
                    self.invalidate_property_targets(i, &handles);
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::PropColorChanged(color) => {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    self.push_undo_snapshot(i, "CHPROP");
                    for &handle in &handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_color(entity, color);
                        }
                    }
                    self.invalidate_property_targets(i, &handles);
                    self.tabs[i].properties.color_picker_open = false;
                    self.tabs[i].properties.color_palette_open = false;
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::PropLwChanged(lw) => {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    self.push_undo_snapshot(i, "CHPROP");
                    for &handle in &handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_line_weight(entity, lw);
                        }
                    }
                    self.invalidate_property_targets(i, &handles);
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::PropLinetypeChanged(lt) => {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    self.push_undo_snapshot(i, "CHPROP");
                    for &handle in &handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_common_prop(entity, "linetype", &lt);
                        }
                    }
                    self.invalidate_property_targets(i, &handles);
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::PropHatchPatternChanged(name) => {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    use crate::scene::model::hatch_patterns;
                    if let Some(entry) = hatch_patterns::find(&name) {
                        self.push_undo_snapshot(i, "HATCHEDIT");
                        for &handle in &handles {
                            if let Some(acadrust::EntityType::Hatch(dxf)) =
                                self.tabs[i].scene.document.get_entity_mut(handle)
                            {
                                dxf.pattern = hatch_patterns::build_dxf_pattern(entry);
                                dxf.is_solid = matches!(
                                    entry.gpu,
                                    crate::scene::model::hatch_model::HatchPattern::Solid
                                );
                            }
                            if let Some(model) = self.tabs[i].scene.hatches.get_mut(&handle) {
                                model.pattern = entry.gpu.clone();
                                model.name = name.clone();
                            }
                        }
                        self.invalidate_property_targets(i, &handles);
                        self.tabs[i].dirty = true;
                        self.refresh_properties();
                    }
                }
                Task::none()
            }

            Message::PropBoolToggle(field) => {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    self.push_undo_snapshot(i, "CHPROP");
                    for &handle in &handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            match field {
                                "invisible" => crate::scene::view::dispatch::toggle_invisible(entity),
                                _ => {
                                    crate::scene::view::dispatch::apply_geom_prop(entity, field, "toggle")
                                }
                            }
                        }
                    }
                    self.invalidate_property_targets(i, &handles);
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::PropGeomChoiceChanged { field, value } => {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    self.push_undo_snapshot(i, "CHPROP");
                    if field == "vp_ucs_name" {
                        // Resolve UCS name → cloned data, then mutate viewports.
                        let ucs_data = self.tabs[i]
                            .scene
                            .document
                            .ucss
                            .iter()
                            .find(|u| u.name == value)
                            .cloned();
                        if let Some(ucs) = ucs_data {
                            for handle in &handles {
                                if let Some(acadrust::EntityType::Viewport(vp)) =
                                    self.tabs[i].scene.document.get_entity_mut(*handle)
                                {
                                    vp.ucs_handle = ucs.handle;
                                    vp.ucs_origin = ucs.origin.clone();
                                    vp.ucs_x_axis = ucs.x_axis.clone();
                                    vp.ucs_y_axis = ucs.y_axis.clone();
                                }
                            }
                        }
                    } else if field == "vp_named_view" {
                        // Assign a named view to viewport(s): copy camera parameters.
                        let view_data = self.tabs[i]
                            .scene
                            .document
                            .views
                            .iter()
                            .find(|v| v.name == value)
                            .cloned();
                        if let Some(view) = view_data {
                            for handle in &handles {
                                if let Some(acadrust::EntityType::Viewport(vp)) =
                                    self.tabs[i].scene.document.get_entity_mut(*handle)
                                {
                                    vp.view_target = view.target.clone();
                                    vp.view_direction = view.direction.clone();
                                    if view.height > 0.0 {
                                        vp.view_height = view.height;
                                    }
                                }
                            }
                            self.tabs[i].scene.camera_generation += 1;
                        }
                    } else {
                        for &handle in &handles {
                            if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle)
                            {
                                crate::scene::view::dispatch::apply_geom_prop(entity, field, &value);
                            }
                        }
                    }
                    self.invalidate_property_targets(i, &handles);
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                }
                Task::none()
            }

            Message::PropGeomInput { field, value } => {
                self.tabs[self.active_tab]
                    .properties
                    .edit_buf
                    .insert(field.to_string(), value);
                Task::none()
            }

            Message::PropGeomCommit(field) => {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    if let Some(raw_val) = self.tabs[i].properties.edit_buf.remove(field) {
                        let val = super::expr_eval::eval_to_string(&raw_val);
                        self.push_undo_snapshot(i, "CHPROP");
                        if field == "frozen_layers" {
                            // Resolve layer names → handles, then apply to viewports.
                            let layer_handles: Vec<acadrust::Handle> = val
                                .split(',')
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty())
                                .filter_map(|name| {
                                    self.tabs[i]
                                        .scene
                                        .document
                                        .layers
                                        .iter()
                                        .find(|l| l.name.eq_ignore_ascii_case(name))
                                        .map(|l| l.handle)
                                })
                                .collect();
                            for &handle in &handles {
                                if let Some(acadrust::EntityType::Viewport(vp)) =
                                    self.tabs[i].scene.document.get_entity_mut(handle)
                                {
                                    vp.frozen_layers = layer_handles.clone();
                                }
                            }
                        } else {
                            for &handle in &handles {
                                if let Some(entity) =
                                    self.tabs[i].scene.document.get_entity_mut(handle)
                                {
                                    match field {
                                        "linetype_scale" | "transparency" => {
                                            crate::scene::view::dispatch::apply_common_prop(
                                                entity, field, &val,
                                            );
                                        }
                                        _ => {
                                            crate::scene::view::dispatch::apply_geom_prop(
                                                entity, field, &val,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        self.invalidate_property_targets(i, &handles);
                        self.tabs[i].dirty = true;
                        self.refresh_properties();
                    }
                }
                Task::none()
            }

            Message::PropColorPickerToggle => {
                let i = self.active_tab;
                self.tabs[i].properties.color_picker_open =
                    !self.tabs[i].properties.color_picker_open;
                if self.tabs[i].properties.color_picker_open {
                    self.tabs[i].properties.color_palette_open = false;
                }
                Task::none()
            }

            Message::PropColorPaletteToggle => {
                self.tabs[self.active_tab].properties.color_palette_open =
                    !self.tabs[self.active_tab].properties.color_palette_open;
                Task::none()
            }

            Message::LayoutSwitch(name) => {
                let i = self.active_tab;
                let going_to_paper = name != "Model";
                // Persist the camera of the layout we're leaving BEFORE switching
                // so returning to it restores where the user left off (the
                // periodic sync only fires on a tick, which may not have run
                // since the last pan/zoom).
                self.tabs[i].scene.sync_camera_to_document();
                self.tabs[i].last_synced_camera_gen = self.tabs[i].scene.camera_generation;
                // Cancel any pending rename/context-menu and active viewport when switching.
                self.layout_rename_state = None;
                self.layout_context_menu = None;
                self.tabs[i].scene.active_viewport = None;
                self.tabs[i].scene.set_current_layout(name);
                self.tabs[i].scene.deselect_all();
                self.tabs[i].scene.restore_saved_camera();
                self.tabs[i].last_synced_camera_gen = self.tabs[i].scene.camera_generation;
                // Paper-space tools live in the right-edge side toolbar now, so
                // entering/leaving a layout no longer hijacks the ribbon tab.
                let _ = going_to_paper;
                // Refresh VP freeze columns for the new layout.
                let doc_layers = self.tabs[i].scene.document.layers.clone();
                let vp_info = self.tabs[i].scene.viewport_list();
                self.tabs[i]
                    .layers
                    .sync_with_viewports(&doc_layers, vp_info);
                Task::none()
            }

            Message::LayoutCreate => {
                let i = self.active_tab;
                // Find a unique name (e.g. Layout2, Layout3, ...).
                let existing = self.tabs[i].scene.layout_names();
                let mut idx = existing.len();
                let new_name = loop {
                    let candidate = format!("Layout{}", idx);
                    if !existing.contains(&candidate) {
                        break candidate;
                    }
                    idx += 1;
                };
                self.push_undo_snapshot(i, "LAYOUT");
                match self.tabs[i].scene.document.add_layout(&new_name) {
                    Ok(_) => {
                        // Override the acadrust default limits (12×9 imperial) with A4 landscape.
                        for obj in self.tabs[i].scene.document.objects.values_mut() {
                            if let acadrust::objects::ObjectType::Layout(l) = obj {
                                if l.name == new_name {
                                    l.min_limits = (0.0, 0.0);
                                    l.max_limits = (297.0, 210.0);
                                    l.min_extents = (0.0, 0.0, 0.0);
                                    l.max_extents = (297.0, 210.0, 0.0);
                                    break;
                                }
                            }
                        }
                        self.tabs[i].scene.current_layout = new_name.clone();
                        // Safety net — `add_layout` already creates the overall
                        // sheet viewport; this covers any path that doesn't.
                        self.tabs[i].scene.ensure_sheet_viewport(&new_name);
                        self.tabs[i].scene.deselect_all();
                        self.tabs[i].scene.fit_all();
                        self.command_line.push_output(&format!(
                            "Layout \"{new_name}\" created — use MVIEW to add a viewport"
                        ));
                        self.tabs[i].dirty = true;
                    }
                    Err(e) => self
                        .command_line
                        .push_error(&format!("Failed to create layout: {e}")),
                }
                Task::none()
            }

            Message::LayoutDelete(name) => {
                let i = self.active_tab;
                self.push_undo_snapshot(i, "LAYOUT DEL");
                if self.tabs[i].scene.delete_layout(&name) {
                    self.layout_context_menu = None;
                    self.layout_rename_state = None;
                    self.command_line
                        .push_output(&format!("Layout \"{name}\" silindi"));
                    self.tabs[i].dirty = true;
                }
                Task::none()
            }

            Message::LayoutRenameStart(name) => {
                if name != "Model" {
                    self.layout_rename_state = Some((name.clone(), name));
                    self.layout_context_menu = None;
                    // Focus the inline field so the user types into it
                    // directly instead of the command line (issue #86).
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        crate::ui::statusbar::LAYOUT_RENAME_INPUT_ID,
                    ));
                }
                Task::none()
            }

            Message::LayoutRenameEdit(val) => {
                if let Some((orig, _)) = &self.layout_rename_state {
                    let orig = orig.clone();
                    self.layout_rename_state = Some((orig, val));
                }
                Task::none()
            }

            Message::LayoutRenameCommit => {
                if let Some((orig, new_name)) = self.layout_rename_state.take() {
                    let new_name = new_name.trim().to_string();
                    if !new_name.is_empty() && new_name != orig {
                        let i = self.active_tab;
                        let exists = self.tabs[i]
                            .scene
                            .layout_names()
                            .iter()
                            .any(|n| *n == new_name);
                        if exists {
                            self.command_line
                                .push_error(&format!("\"{}\" name already in use", new_name));
                        } else {
                            self.push_undo_snapshot(i, "LAYOUT RENAME");
                            self.tabs[i].scene.rename_layout(&orig, &new_name);
                            if self.tabs[i].scene.current_layout == orig {
                                self.tabs[i].scene.current_layout = new_name.clone();
                            }
                            self.tabs[i].dirty = true;
                            self.command_line
                                .push_output(&format!("Layout \"{orig}\" → \"{new_name}\""));
                        }
                    }
                }
                Task::none()
            }

            Message::LayoutRenameCancel => {
                self.layout_rename_state = None;
                Task::none()
            }

            Message::LayoutContextMenu(name) => {
                if name != "Model" {
                    self.layout_context_menu = Some(name);
                }
                Task::none()
            }

            Message::LayoutContextMenuClose => {
                self.layout_context_menu = None;
                Task::none()
            }

            // ── Layout Manager Panel ──────────────────────────────────────────
            Message::LayoutManagerOpen => {
                let i = self.active_tab;
                let current = self.tabs[i].scene.current_layout.clone();
                self.layout_manager_selected = current.clone();
                self.layout_manager_rename_buf = if current == "Model" {
                    String::new()
                } else {
                    current
                };
                self.active_modal = Some(super::ModalKind::LayoutManager);
                Task::none()
            }
            Message::LayoutManagerClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::LayoutManagerSelect(name) => {
                self.layout_manager_rename_buf = if name == "Model" {
                    String::new()
                } else {
                    name.clone()
                };
                self.layout_manager_selected = name;
                Task::none()
            }
            Message::LayoutManagerRenameBuf(s) => {
                self.layout_manager_rename_buf = s;
                Task::none()
            }
            Message::LayoutManagerRenameCommit => {
                let i = self.active_tab;
                let old_name = self.layout_manager_selected.clone();
                let new_name = self.layout_manager_rename_buf.trim().to_string();
                if old_name == "Model" {
                    self.command_line
                        .push_error("Cannot rename the Model layout.");
                } else if new_name.is_empty() {
                    self.command_line.push_error("Layout name cannot be empty.");
                } else if new_name == old_name {
                    // no-op
                } else {
                    self.push_undo_snapshot(i, "LAYOUT RENAME");
                    self.tabs[i].scene.rename_layout(&old_name, &new_name);
                    if self.tabs[i].scene.current_layout == old_name {
                        self.tabs[i].scene.current_layout = new_name.clone();
                    }
                    self.layout_manager_selected = new_name.clone();
                    self.tabs[i].dirty = true;
                    self.command_line
                        .push_output(&format!("Layout renamed: '{old_name}' → '{new_name}'"));
                }
                Task::none()
            }
            Message::LayoutManagerNew => {
                let i = self.active_tab;
                let existing = self.tabs[i].scene.layout_names();
                let n = (1usize..)
                    .find(|n| !existing.contains(&format!("Layout{n}")))
                    .unwrap_or(1);
                let name = format!("Layout{n}");
                self.push_undo_snapshot(i, "LAYOUT NEW");
                match self.tabs[i].scene.document.add_layout(&name) {
                    Ok(_) => {
                        self.tabs[i].dirty = true;
                        self.layout_manager_selected = name.clone();
                        self.layout_manager_rename_buf = name.clone();
                        self.command_line
                            .push_output(&format!("Layout '{name}' created."));
                    }
                    Err(e) => self.command_line.push_error(&format!("LAYOUT: {e}")),
                }
                Task::none()
            }
            Message::LayoutManagerDelete => {
                let i = self.active_tab;
                let name = self.layout_manager_selected.clone();
                if name == "Model" {
                    self.command_line
                        .push_error("Cannot delete the Model layout.");
                } else {
                    self.push_undo_snapshot(i, "LAYOUT DELETE");
                    self.tabs[i].scene.delete_layout(&name);
                    self.tabs[i].dirty = true;
                    // Switch to Model if active layout was deleted.
                    if self.tabs[i].scene.current_layout == name {
                        self.tabs[i].scene.current_layout = "Model".to_string();
                        self.tabs[i].scene.bump_geometry();
                    }
                    self.layout_manager_selected = "Model".to_string();
                    self.layout_manager_rename_buf = String::new();
                    self.command_line
                        .push_output(&format!("Layout '{name}' deleted."));
                }
                Task::none()
            }
            Message::LayoutManagerMoveLeft => {
                let i = self.active_tab;
                let name = self.layout_manager_selected.clone();
                if name == "Model" {
                    return Task::none();
                }
                let names = self.tabs[i].scene.layout_names();
                // Find position among paper layouts only.
                let paper: Vec<&str> = names.iter().skip(1).map(|s| s.as_str()).collect();
                if let Some(pos) = paper.iter().position(|&n| n == name) {
                    if pos > 0 {
                        self.push_undo_snapshot(i, "LAYOUT REORDER");
                        self.tabs[i].scene.swap_layout_order(&name, paper[pos - 1]);
                        self.tabs[i].dirty = true;
                    }
                }
                Task::none()
            }
            Message::LayoutManagerMoveRight => {
                let i = self.active_tab;
                let name = self.layout_manager_selected.clone();
                if name == "Model" {
                    return Task::none();
                }
                let names = self.tabs[i].scene.layout_names();
                let paper: Vec<&str> = names.iter().skip(1).map(|s| s.as_str()).collect();
                if let Some(pos) = paper.iter().position(|&n| n == name) {
                    if pos + 1 < paper.len() {
                        self.push_undo_snapshot(i, "LAYOUT REORDER");
                        self.tabs[i].scene.swap_layout_order(&name, paper[pos + 1]);
                        self.tabs[i].dirty = true;
                    }
                }
                Task::none()
            }
            Message::LayoutManagerSetCurrent => {
                let i = self.active_tab;
                let name = self.layout_manager_selected.clone();
                self.tabs[i].scene.current_layout = name.clone();
                self.tabs[i].scene.bump_geometry();
                self.command_line
                    .push_output(&format!("Switched to layout '{name}'."));
                Task::none()
            }

            Message::SetTheme(theme) => {
                self.active_theme = theme;
                Task::none()
            }

            // ── Keyboard Shortcuts Panel ──────────────────────────────────────
            Message::ShortcutsPanelOpen => {
                self.active_modal = Some(super::ModalKind::Shortcuts);
                Task::none()
            }
            Message::ShortcutsPanelClose => {
                self.close_active_modal();
                Task::none()
            }

            // ── About window ──────────────────────────────────────────────
            Message::AboutOpen => {
                self.active_modal = Some(super::ModalKind::About);
                Task::none()
            }

            Message::CloseModal => {
                self.close_active_modal();
                Task::none()
            }
            Message::ModalGrab => {
                // Start a drag; the first ModalDragMove seeds the reference.
                self.modal_dragging = true;
                self.modal_drag_last = None;
                Task::none()
            }
            Message::ModalDragMove(p) => {
                if self.modal_dragging {
                    if let Some(last) = self.modal_drag_last {
                        self.modal_offset.x += p.x - last.x;
                        self.modal_offset.y += p.y - last.y;
                        // Clamp so the dialog stops at the window edge instead
                        // of being squeezed (the off-centre padding shrinks the
                        // dialog once it overlaps a border).
                        if let Some((cw, ch)) = self.modal_outer_size() {
                            let ww = self.vp_size.0 + 440.0;
                            let wh = self.vp_size.1;
                            let max_x = ((ww - cw) * 0.5).max(0.0);
                            let max_y = ((wh - ch) * 0.5).max(0.0);
                            self.modal_offset.x = self.modal_offset.x.clamp(-max_x, max_x);
                            self.modal_offset.y = self.modal_offset.y.clamp(-max_y, max_y);
                        }
                    }
                    self.modal_drag_last = Some(p);
                }
                Task::none()
            }
            Message::ModalDragRelease => {
                self.modal_dragging = false;
                self.modal_drag_last = None;
                Task::none()
            }

            Message::AboutCopyInfo => {
                let info = format!(
                    "Open CAD Studio v{}\nOS: {}\nArch: {}",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    std::env::consts::ARCH,
                );
                iced::clipboard::write(info)
            }

            // ── Plugin Manager window ─────────────────────────────────────
            Message::PluginManagerOpen => {
                // Refresh the on-disk external-plugin list each time the manager
                // opens so newly dropped-in packages show up.
                self.external_plugins = crate::plugin::external::discover();
                self.active_modal = Some(super::ModalKind::PluginManager);
                // Fetch the curated registry and release lists for linked repos.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut tasks = vec![self.fetch_registry_task()];
                    tasks.extend(
                        self.plugin_repos
                            .clone()
                            .into_iter()
                            .map(|r| self.fetch_releases_task(r)),
                    );
                    return Task::batch(tasks);
                }
                #[cfg(target_arch = "wasm32")]
                Task::none()
            }
            Message::PluginManagerClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::SetPluginEnabled(id, enabled) => {
                if enabled {
                    self.disabled_plugins.remove(&id);
                } else {
                    self.disabled_plugins.insert(id);
                }
                self.rebuild_ribbon_modules();
                self.persist_settings_if_changed();
                Task::none()
            }
            Message::PluginRepoInput(s) => {
                self.plugin_repo_input = s;
                Task::none()
            }
            Message::PluginRepoAdd => {
                let repo = self
                    .plugin_repo_input
                    .trim()
                    .trim_start_matches("https://github.com/")
                    .trim_end_matches('/')
                    .to_string();
                if repo.is_empty() || self.plugin_repos.contains(&repo) {
                    return Task::none();
                }
                self.plugin_repos.push(repo.clone());
                self.plugin_repo_input.clear();
                self.persist_settings_if_changed();
                self.marketplace_status = format!("Fetching releases for {repo}…");
                self.fetch_releases_task(repo)
            }
            Message::PluginRepoRemove(repo) => {
                self.plugin_repos.retain(|r| r != &repo);
                self.repo_release_tags.remove(&repo);
                self.repo_selected_tag.remove(&repo);
                self.persist_settings_if_changed();
                Task::none()
            }
            Message::PluginRegistryFetched(Ok(entries)) => {
                // Fetch releases for every curated repo so the dropdowns fill in.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let tasks: Vec<_> = entries
                        .iter()
                        .map(|e| self.fetch_releases_task(e.repo.clone()))
                        .collect();
                    self.plugin_registry = entries;
                    return Task::batch(tasks);
                }
                #[cfg(target_arch = "wasm32")]
                {
                    self.plugin_registry = entries;
                    Task::none()
                }
            }
            Message::PluginRegistryFetched(Err(e)) => {
                self.marketplace_status = format!("Registry: {e}");
                Task::none()
            }
            Message::PluginReleasesFetched(repo, Ok(tags)) => {
                if let Some(first) = tags.first() {
                    self.repo_selected_tag
                        .entry(repo.clone())
                        .or_insert_with(|| first.clone());
                }
                self.marketplace_status =
                    format!("{repo}: {} installable release(s)", tags.len());
                self.repo_release_tags.insert(repo, tags);
                Task::none()
            }
            Message::PluginReleasesFetched(repo, Err(e)) => {
                self.marketplace_status = format!("{repo}: {e}");
                Task::none()
            }
            Message::PluginReleaseSelect(repo, tag) => {
                self.repo_selected_tag.insert(repo, tag);
                Task::none()
            }
            Message::PluginInstall(repo) => {
                let Some(tag) = self.repo_selected_tag.get(&repo).cloned() else {
                    return Task::none();
                };
                self.marketplace_status = format!("Installing {repo} {tag}…");
                self.install_task(repo, tag)
            }
            Message::PluginInstalled(Ok(id)) => {
                self.marketplace_status = format!("Installed '{id}'. Restart to load it.");
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.external_plugins = crate::plugin::external::discover();
                }
                Task::none()
            }
            Message::PluginInstalled(Err(e)) => {
                self.marketplace_status = format!("Install failed: {e}");
                Task::none()
            }
            Message::PluginUninstall(id) => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    match crate::plugin::external::uninstall(&id) {
                        Ok(()) => {
                            self.marketplace_status =
                                format!("Uninstalled '{id}'. Restart to unload it.");
                            self.external_plugins = crate::plugin::external::discover();
                        }
                        Err(e) => {
                            self.marketplace_status = format!("Uninstall failed: {e}");
                        }
                    }
                }
                #[cfg(target_arch = "wasm32")]
                let _ = id;
                Task::none()
            }
            Message::PointStyleSetMode(mode) => {
                self.set_point_mode_bits(!0, mode);
                Task::none()
            }
            Message::PointStyleSizeRelative(relative) => {
                self.point_size_relative = relative;
                self.apply_point_size();
                Task::none()
            }
            Message::PointStyleSizeInput(s) => {
                self.point_size_buf = s;
                Task::none()
            }
            Message::PointStyleApplySize => {
                self.apply_point_size();
                Task::none()
            }
            Message::PointStyleOk => {
                self.apply_point_size();
                self.close_active_modal();
                Task::none()
            }

            Message::EnterViewport(handle) => {
                let i = self.active_tab;
                // Clear paper-space selection before entering model space.
                self.tabs[i].scene.deselect_all();
                self.tabs[i].scene.active_viewport = Some(handle);
                self.refresh_properties();
                self.command_line.push_output("MSPACE");
                Task::none()
            }

            Message::ExitViewport => {
                let i = self.active_tab;
                // Clear model-space selection before returning to paper space.
                self.tabs[i].scene.deselect_all();
                self.refresh_properties();
                self.tabs[i].scene.active_viewport = None;
                self.command_line.push_output("PSPACE");
                Task::none()
            }

            Message::MspaceCommand => {
                let i = self.active_tab;
                if self.tabs[i].scene.current_layout == "Model" {
                    self.command_line
                        .push_error("MS is only available in paper space layouts.");
                    return Task::none();
                }
                if self.tabs[i].scene.active_viewport.is_some() {
                    // Already in MSPACE — nothing to do.
                    return Task::none();
                }
                match self.tabs[i].scene.first_user_viewport() {
                    Some(handle) => Task::done(Message::EnterViewport(handle)),
                    None => {
                        self.command_line
                            .push_error("No viewport found in this layout.");
                        Task::none()
                    }
                }
            }

            Message::PspaceCommand => Task::done(Message::ExitViewport),

            Message::Undo => {
                self.undo_active_tab();
                Task::none()
            }
            Message::Redo => {
                self.redo_active_tab();
                Task::none()
            }

            Message::UndoMany(steps) => {
                self.ribbon.close_dropdown();
                self.undo_steps(steps);
                Task::none()
            }

            Message::RedoMany(steps) => {
                self.ribbon.close_dropdown();
                self.redo_steps(steps);
                Task::none()
            }

            Message::Noop => Task::none(),

            // ── Unsaved-changes dialog ────────────────────────────────────
            Message::UnsavedDialogCancel => {
                self.pending_close = None;
                self.close_unsaved_dialog_window()
            }

            Message::UnsavedDialogDiscard => {
                match self.pending_close.take() {
                    Some(super::PendingClose::Tab(idx)) => {
                        let close_win = self.close_unsaved_dialog_window();
                        if self.tabs.len() == 1 {
                            self.tab_counter += 1;
                            self.tabs[0] =
                                super::document::DocumentTab::new_drawing(self.tab_counter);
                            self.active_tab = 0;
                        } else {
                            self.tabs.remove(idx);
                            if self.active_tab >= self.tabs.len() {
                                self.active_tab = self.tabs.len() - 1;
                            }
                        }
                        // The active tab is now a fresh blank or a
                        // different existing tab; sync ribbon chips so
                        // they don't keep showing the discarded tab's
                        // last selection. #21.
                        self.sync_ribbon_layers();
                        self.sync_ribbon_from_selection();
                        return close_win;
                    }
                    Some(super::PendingClose::Quit) => {
                        if let Some(idx) = self.tabs.iter().position(|t| t.dirty) {
                            self.tabs[idx].dirty = false;
                        }
                        if self.tabs.iter().any(|t| t.dirty) {
                            // More dirty tabs remain — keep window open.
                            self.pending_close = Some(super::PendingClose::Quit);
                        } else {
                            let close_win = self.close_unsaved_dialog_window();
                            return Task::batch(vec![close_win, iced::exit()]);
                        }
                    }
                    None => {}
                }
                Task::none()
            }

            Message::UnsavedDialogSave => {
                match self.pending_close.take() {
                    Some(super::PendingClose::Tab(idx)) => {
                        if let Some(path) = if cfg!(target_arch = "wasm32") { None } else { self.tabs[idx].current_path.clone() } {
                            match crate::io::save(&self.tabs[idx].scene.document, &path) {
                                Ok(()) => {
                                    self.command_line
                                        .push_output(&format!("Saved: {}", path.display()));
                                    self.tabs[idx].dirty = false;
                                    let close_win = self.close_unsaved_dialog_window();
                                    let close_tab = self.update(Message::TabClose(idx));
                                    return Task::batch(vec![close_win, close_tab]);
                                }
                                Err(e) => {
                                    // Keep dialog open for retry.
                                    self.command_line.push_error(&format!("Save failed: {e}"));
                                    self.pending_close = Some(super::PendingClose::Tab(idx));
                                }
                            }
                        } else {
                            // No path — close unsaved dialog, open custom Save As dialog.
                            self.pending_close = Some(super::PendingClose::Tab(idx));
                            self.save_dialog_for_unsaved = true;
                            let close_win = self.close_unsaved_dialog_window();
                            let open_save = self.open_save_dialog_window(idx);
                            return Task::batch([close_win, open_save]);
                        }
                    }
                    Some(super::PendingClose::Quit) => {
                        if let Some(idx) = self.tabs.iter().position(|t| t.dirty) {
                            if let Some(path) = if cfg!(target_arch = "wasm32") { None } else { self.tabs[idx].current_path.clone() } {
                                match crate::io::save(&self.tabs[idx].scene.document, &path) {
                                    Ok(()) => {
                                        self.command_line
                                            .push_output(&format!("Saved: {}", path.display()));
                                        self.tabs[idx].dirty = false;
                                    }
                                    Err(e) => {
                                        self.command_line.push_error(&format!("Save failed: {e}"));
                                        self.pending_close = Some(super::PendingClose::Quit);
                                        return Task::none();
                                    }
                                }
                            } else {
                                // No path — close unsaved dialog, open custom Save As dialog.
                                self.active_tab = idx;
                                self.pending_close = Some(super::PendingClose::Quit);
                                self.save_dialog_for_unsaved = true;
                                let close_win = self.close_unsaved_dialog_window();
                                let open_save = self.open_save_dialog_window(idx);
                                return Task::batch([close_win, open_save]);
                            }
                        }
                        if self.tabs.iter().any(|t| t.dirty) {
                            // More dirty tabs — keep window open.
                            self.pending_close = Some(super::PendingClose::Quit);
                        } else {
                            let close_win = self.close_unsaved_dialog_window();
                            return Task::batch(vec![close_win, iced::exit()]);
                        }
                    }
                    None => {}
                }
                Task::none()
            }

            Message::UnsavedPickedSavePath(Some(path)) => {
                let (_, version) = crate::io::parse_save_format(&self.save_dialog_format);
                match self.pending_close.take() {
                    Some(super::PendingClose::Tab(idx)) => {
                        match crate::io::save_as_version(
                            &self.tabs[idx].scene.document,
                            &path,
                            version,
                        ) {
                            Ok(()) => {
                                self.command_line
                                    .push_output(&format!("Saved: {}", path.display()));
                                self.tabs[idx].current_path = Some(path);
                                self.tabs[idx].dirty = false;
                                return self.update(Message::TabClose(idx));
                            }
                            Err(e) => {
                                self.command_line.push_error(&format!("Save failed: {e}"));
                                self.pending_close = Some(super::PendingClose::Tab(idx));
                                return self.open_unsaved_dialog_window();
                            }
                        }
                    }
                    Some(super::PendingClose::Quit) => {
                        let i = self.active_tab;
                        match crate::io::save_as_version(
                            &self.tabs[i].scene.document,
                            &path,
                            version,
                        ) {
                            Ok(()) => {
                                self.command_line
                                    .push_output(&format!("Saved: {}", path.display()));
                                self.tabs[i].current_path = Some(path);
                                self.tabs[i].dirty = false;
                                if self.tabs.iter().any(|t| t.dirty) {
                                    self.pending_close = Some(super::PendingClose::Quit);
                                    return self.open_unsaved_dialog_window();
                                } else {
                                    return iced::exit();
                                }
                            }
                            Err(e) => {
                                self.command_line.push_error(&format!("Save failed: {e}"));
                                self.pending_close = Some(super::PendingClose::Quit);
                                return self.open_unsaved_dialog_window();
                            }
                        }
                    }
                    None => {}
                }
                Task::none()
            }

            Message::UnsavedPickedSavePath(None) => {
                // User cancelled the save-as dialog — re-open the confirmation dialog.
                if self.pending_close.is_some() {
                    return self.open_unsaved_dialog_window();
                }
                Task::none()
            }

            // ── Page Setup ────────────────────────────────────────────────
            Message::PageSetupOpen => {
                let i = self.active_tab;
                // Populate edit buffers from current paper limits.
                let (w, h) = if let Some(((x0, y0), (x1, y1))) = self.tabs[i].scene.paper_limits() {
                    (x1 - x0, y1 - y0)
                } else {
                    (297.0, 210.0) // A4 default
                };
                self.page_setup_w = format!("{w:.1}");
                self.page_setup_h = format!("{h:.1}");
                self.active_modal = Some(super::ModalKind::PageSetup);
                Task::none()
            }
            Message::PageSetupClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::UpdateCheckResult(latest) => {
                let Some(info) = latest else {
                    return Task::none();
                };
                self.update_notice_version = Some(info.version);
                self.update_notice_body = Some(info.body);
                self.active_modal = Some(super::ModalKind::UpdateNotice);
                Task::none()
            }
            Message::UpdateNoticeClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::UpdateNoticeOpenRelease => {
                crate::sys::open_url(crate::update_check::RELEASES_PAGE);
                self.close_active_modal();
                Task::none()
            }
            Message::AssocPromptYes => {
                self.mark_assoc_prompted();
                self.active_modal = None;
                Task::perform(
                    crate::io::file_association::set_default_app(),
                    Message::AssocResult,
                )
            }
            Message::AssocPromptNo => {
                self.mark_assoc_prompted();
                self.active_modal = None;
                Task::none()
            }
            Message::AssocResult(result) => {
                match result {
                    Ok(msg) => self.command_line.push_info(&msg),
                    Err(err) => self
                        .command_line
                        .push_error(&format!("Could not set default app: {err}")),
                }
                Task::none()
            }
            Message::PageSetupWidthEdit(s) => {
                self.page_setup_w = s;
                Task::none()
            }
            Message::PageSetupHeightEdit(s) => {
                self.page_setup_h = s;
                Task::none()
            }
            Message::PageSetupPreset(name) => {
                // Paper size presets defined in view.rs — mirror them here.
                let sizes: &[(&str, f64, f64)] = &[
                    ("A4 Portrait", 210.0, 297.0),
                    ("A4 Landscape", 297.0, 210.0),
                    ("A3 Portrait", 297.0, 420.0),
                    ("A3 Landscape", 420.0, 297.0),
                    ("A2 Portrait", 420.0, 594.0),
                    ("A2 Landscape", 594.0, 420.0),
                    ("A1 Portrait", 594.0, 841.0),
                    ("A1 Landscape", 841.0, 594.0),
                    ("A0 Portrait", 841.0, 1189.0),
                    ("A0 Landscape", 1189.0, 841.0),
                    ("Letter Portrait", 215.9, 279.4),
                    ("Letter Landscape", 279.4, 215.9),
                ];
                if let Some(&(_, w, h)) = sizes.iter().find(|(n, _, _)| *n == name) {
                    self.page_setup_w = format!("{w:.1}");
                    self.page_setup_h = format!("{h:.1}");
                }
                Task::none()
            }
            Message::PageSetupPlotArea(s) => {
                self.page_setup_plot_area = s;
                Task::none()
            }
            Message::PageSetupCenterToggle => {
                self.page_setup_center = !self.page_setup_center;
                Task::none()
            }
            Message::PageSetupOffsetXEdit(s) => {
                self.page_setup_offset_x = s;
                Task::none()
            }
            Message::PageSetupOffsetYEdit(s) => {
                self.page_setup_offset_y = s;
                Task::none()
            }
            Message::PageSetupRotation(s) => {
                self.page_setup_rotation = s;
                Task::none()
            }
            Message::PageSetupScale(s) => {
                self.page_setup_scale = s;
                Task::none()
            }
            Message::PageSetupCommit => {
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

                    // Update the Layout object's limits.
                    for obj in self.tabs[i].scene.document.objects.values_mut() {
                        if let acadrust::objects::ObjectType::Layout(l) = obj {
                            if l.name == layout_name {
                                l.min_limits = (0.0, 0.0);
                                l.max_limits = (w, h);
                                l.min_extents = (0.0, 0.0, 0.0);
                                l.max_extents = (w, h, 0.0);
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
                    self.command_line.push_info(&format!(
                        "Page setup: {w:.1}×{h:.1} mm  area={plot_area}  \
                         center={center}  rot={rotation}°"
                    ));
                }
                self.active_modal = None;
                Task::none()
            }

            // ── Plot / Export ─────────────────────────────────────────────
            Message::PlotExport => {
                let i = self.active_tab;
                let stem = self.tabs[i]
                    .current_path
                    .as_deref()
                    .and_then(|p: &std::path::Path| p.file_stem())
                    .map(|s: &std::ffi::OsStr| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "drawing".into());
                Task::perform(
                    crate::io::pdf_export::pick_pdf_path_owned(stem),
                    Message::PlotExportPath,
                )
            }
            Message::PlotExportPath(None) => Task::none(),
            Message::PlotExportPath(Some(path)) => {
                let i = self.active_tab;
                let scene = &self.tabs[i].scene;
                let layout_name = scene.current_layout.clone();
                let wires = scene.entity_wires();
                let hatches = scene.paper_canvas_hatches();
                let wipeouts = scene.paper_canvas_wipeouts();

                // Read PlotSettings for current layout (if available).
                use acadrust::objects::{ObjectType, PlotType};
                let ps_snap = scene.document.objects.values().find_map(|obj| {
                    if let ObjectType::PlotSettings(ps) = obj {
                        if ps.page_name == layout_name {
                            Some(ps.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });

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

            // ── Print to system printer ───────────────────────────────────────
            Message::PrintToPrinter => {
                let i = self.active_tab;
                let scene = &self.tabs[i].scene;
                let layout_name = scene.current_layout.clone();
                let wires = scene.entity_wires();
                let hatches: Vec<_> = scene.paper_canvas_hatches().as_ref().clone();
                let wipeouts: Vec<_> = scene.paper_canvas_wipeouts().as_ref().clone();
                use acadrust::objects::{ObjectType, PlotType};
                let ps_snap = scene.document.objects.values().find_map(|obj| {
                    if let ObjectType::PlotSettings(ps) = obj {
                        if ps.page_name == layout_name {
                            Some(ps.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
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
            Message::PrintResult(Ok(printer)) => {
                self.command_line
                    .push_info(&format!("Sent to printer: {printer}"));
                Task::none()
            }
            Message::PrintResult(Err(e)) => {
                self.command_line.push_error(&format!("Print failed: {e}"));
                Task::none()
            }

            // ── Plot Style Table ──────────────────────────────────────────────
            Message::PlotStyleLoad => {
                Task::perform(crate::io::pick_plot_style(), Message::PlotStyleLoaded)
            }
            Message::PlotStyleLoaded(Some(table)) => {
                self.command_line.push_output(&format!(
                    "Plot style '{}' loaded ({} color entries).",
                    table.name,
                    table
                        .aci_entries
                        .iter()
                        .filter(|e| e.color.is_some())
                        .count()
                ));
                self.active_plot_style = Some(table);
                Task::none()
            }
            Message::PlotStyleLoaded(None) => Task::none(),
            Message::PlotStyleClear => {
                self.active_plot_style = None;
                self.command_line.push_output("Plot style table cleared.");
                Task::none()
            }

            // ── Plot Style Panel ──────────────────────────────────────────────
            Message::PlotStylePanelOpen => {
                // Initialise edit buffers for ACI 1.
                self.plotstyle_panel_aci = 1;
                let entry = self
                    .active_plot_style
                    .as_ref()
                    .and_then(|t| t.aci_entries.get(1));
                self.ps_color_buf = entry
                    .and_then(|e| {
                        e.color
                            .map(|[r, g, b]| format!("#{:02X}{:02X}{:02X}", r, g, b))
                    })
                    .unwrap_or_default();
                self.ps_lineweight_buf = entry
                    .map(|e| e.lineweight.to_string())
                    .unwrap_or("255".into());
                self.ps_screening_buf = entry
                    .map(|e| e.screening.to_string())
                    .unwrap_or("100".into());
                self.active_modal = Some(super::ModalKind::Plotstyle);
                Task::none()
            }
            Message::PlotStylePanelClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::PlotStylePanelSelectAci(aci) => {
                self.plotstyle_panel_aci = aci;
                let entry = self
                    .active_plot_style
                    .as_ref()
                    .and_then(|t| t.aci_entries.get(aci as usize));
                self.ps_color_buf = entry
                    .and_then(|e| {
                        e.color
                            .map(|[r, g, b]| format!("#{:02X}{:02X}{:02X}", r, g, b))
                    })
                    .unwrap_or_default();
                self.ps_lineweight_buf = entry
                    .map(|e| e.lineweight.to_string())
                    .unwrap_or("255".into());
                self.ps_screening_buf = entry
                    .map(|e| e.screening.to_string())
                    .unwrap_or("100".into());
                Task::none()
            }
            Message::PlotStylePanelColorBuf(s) => {
                self.ps_color_buf = s;
                Task::none()
            }
            Message::PlotStylePanelLwBuf(s) => {
                self.ps_lineweight_buf = s;
                Task::none()
            }
            Message::PlotStylePanelScreenBuf(s) => {
                self.ps_screening_buf = s;
                Task::none()
            }

            Message::PlotStylePanelApply => {
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

            Message::PlotStylePanelSave => {
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

            Message::PlotStylePanelSavePath(Some(path)) => {
                if let Some(table) = &self.active_plot_style {
                    match table.save(&path) {
                        Ok(()) => self.command_line.push_output(&format!(
                            "Plot style table saved to \"{}\".",
                            path.display()
                        )),
                        Err(e) => self.command_line.push_error(&format!("Save error: {e}")),
                    }
                }
                Task::none()
            }
            Message::PlotStylePanelSavePath(None) => Task::none(),

            // ── TextStyle Font Browser ────────────────────────────────────────
            Message::TextStyleDialogOpen => {
                let i = self.active_tab;
                let cur = self.tabs[i]
                    .scene
                    .document
                    .header
                    .current_text_style_name
                    .clone();
                let exists = self.tabs[i].scene.document.text_styles.get(&cur).is_some();
                self.textstyle_selected = if exists {
                    cur
                } else {
                    self.tabs[i]
                        .scene
                        .document
                        .text_styles
                        .iter()
                        .next()
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "Standard".to_string())
                };
                self.load_textstyle_bufs(i);
                self.active_modal = Some(super::ModalKind::TextStyle);
                self.style_stage_begin();
                Task::none()
            }
            Message::TextStyleDialogClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::TextStyleDialogSelect(name) => {
                let i = self.active_tab;
                self.textstyle_selected = name;
                self.load_textstyle_bufs(i);
                Task::none()
            }
            Message::TextStyleDialogSetCurrent => {
                // Staged: persists on Apply.
                let i = self.active_tab;
                let name = self.textstyle_selected.clone();
                if self.tabs[i].scene.document.text_styles.get(&name).is_some() {
                    self.tabs[i].scene.document.header.current_text_style_name = name.clone();
                    self.sync_ribbon_styles();
                    self.command_line
                        .push_output(&format!("Current text style: {}", name));
                }
                Task::none()
            }
            Message::TextStyleDialogNew => {
                self.style_new(crate::app::StyleKind::Text);
                Task::none()
            }
            Message::TextStyleDialogCopy => {
                self.style_copy(crate::app::StyleKind::Text);
                Task::none()
            }
            Message::TextStyleDialogDelete => {
                self.style_delete(crate::app::StyleKind::Text);
                Task::none()
            }
            // ── Shared inline rename (all style managers) ─────────────────
            Message::StyleRenameStart(kind, name) => {
                self.style_rename_start(kind, name);
                // Focus the freshly-shown rename field so the user can type
                // immediately after the double click.
                iced::widget::operation::focus(crate::ui::style_list::rename_input_id())
            }
            Message::StyleRenameEdit(s) => {
                self.style_rename_buf = s;
                Task::none()
            }
            Message::StyleRenameCommit(kind) => {
                self.style_rename_commit(kind);
                Task::none()
            }
            Message::StyleRenameCancel => {
                self.style_rename_cancel();
                Task::none()
            }
            Message::TextStyleEdit { field, value } => {
                match field {
                    "font" => self.textstyle_font = value,
                    "width" => self.textstyle_width = value,
                    "oblique" => self.textstyle_oblique = value,
                    "height" => self.textstyle_height = value,
                    "bigfont" => self.textstyle_bigfont = value,
                    "ttf" => self.textstyle_ttf = value,
                    _ => {}
                }
                Task::none()
            }
            Message::TextStyleToggle(field) => {
                // Staged: mutate live for preview, persist on Apply.
                let i = self.active_tab;
                let name = self.textstyle_selected.clone();
                if let Some(s) = self.tabs[i].scene.document.text_styles.get_mut(&name) {
                    match field {
                        "backward" => s.flags.backward = !s.flags.backward,
                        "upside_down" => s.flags.upside_down = !s.flags.upside_down,
                        "annotative" => s.annotative = !s.annotative,
                        _ => {}
                    }
                }
                Task::none()
            }
            Message::TextStyleApply => {
                let i = self.active_tab;
                let name = self.textstyle_selected.clone();
                let font = self.textstyle_font.clone();
                let width_str = self.textstyle_width.clone();
                let oblique_str = self.textstyle_oblique.clone();
                let height_str = self.textstyle_height.clone();
                let bigfont = self.textstyle_bigfont.clone();
                let ttf = self.textstyle_ttf.clone();
                if let Some(s) = self.tabs[i].scene.document.text_styles.get_mut(&name) {
                    s.font_file = font;
                    s.big_font_file = bigfont;
                    s.true_type_font = ttf;
                    if let Ok(w) = width_str.trim().parse::<f64>() {
                        s.width_factor = w;
                    }
                    if let Ok(a) = oblique_str.trim().parse::<f64>() {
                        s.oblique_angle = a.to_radians();
                    }
                    if let Ok(h) = height_str.trim().parse::<f64>() {
                        s.height = h.max(0.0);
                    }
                }
                self.style_stage_commit();
                Task::none()
            }
            Message::TextStyleFontPick(font_file) => {
                // Staged: update the buffer + live style; persist on Apply.
                let i = self.active_tab;
                self.textstyle_font = font_file.clone();
                let name = self.textstyle_selected.clone();
                if let Some(s) = self.tabs[i].scene.document.text_styles.get_mut(&name) {
                    s.font_file = font_file;
                }
                Task::none()
            }

            // ── TableStyle Dialog ─────────────────────────────────────────────
            Message::TableStyleDialogOpen => {
                use acadrust::objects::ObjectType;
                let i = self.active_tab;
                self.tablestyle_selected = self.tabs[i]
                    .scene
                    .document
                    .objects
                    .values()
                    .find_map(|o| {
                        if let ObjectType::TableStyle(s) = o {
                            Some(s.name.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "Standard".to_string());
                self.load_tablestyle_bufs(i);
                self.active_modal = Some(super::ModalKind::TableStyle);
                self.style_stage_begin();
                Task::none()
            }
            Message::TableStyleDialogClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::TableStyleDialogSelect(name) => {
                self.tablestyle_selected = name;
                let i = self.active_tab;
                self.load_tablestyle_bufs(i);
                Task::none()
            }

            Message::TableStyleEdit { field, value } => {
                match field {
                    "hmargin" => self.ts_hmargin = value,
                    "vmargin" => self.ts_vmargin = value,
                    "description" => self.ts_description = value,
                    _ => {}
                }
                Task::none()
            }

            Message::TableStyleApply => {
                use acadrust::objects::ObjectType;
                let i = self.active_tab;
                let name = self.tablestyle_selected.clone();
                let h: Option<f64> = self.ts_hmargin.trim().parse().ok();
                let v: Option<f64> = self.ts_vmargin.trim().parse().ok();
                let desc = self.ts_description.clone();
                for obj in self.tabs[i].scene.document.objects.values_mut() {
                    if let ObjectType::TableStyle(s) = obj {
                        if s.name == name {
                            if let Some(h) = h {
                                s.horizontal_margin = h;
                            }
                            if let Some(v) = v {
                                s.vertical_margin = v;
                            }
                            s.description = desc.clone();
                        }
                    }
                }
                self.style_stage_commit();
                Task::none()
            }

            Message::TableStyleSetFlow(value) => {
                use acadrust::objects::TableFlowDirection;
                let i = self.active_tab;
                if let Some(s) = self.tablestyle_mut(i) {
                    s.flow_direction = match value.as_str() {
                        "Up" => TableFlowDirection::Up,
                        _ => TableFlowDirection::Down,
                    };
                }
                Task::none()
            }

            Message::TableColorMore(row, field) => {
                self.ts_color_open = if self.ts_color_open == Some((row, field)) {
                    None
                } else {
                    Some((row, field))
                };
                Task::none()
            }
            Message::TableStyleCellEdit { row, field, value } => {
                self.ts_color_open = None;
                let r = row as usize;
                if r < 3 {
                    match field {
                        "textstyle" => self.ts_cell_textstyle[r] = value,
                        "height" => self.ts_cell_height[r] = value,
                        "textcolor" => self.ts_cell_textcolor[r] = value,
                        "fillcolor" => self.ts_cell_fillcolor[r] = value,
                        "datatype" => self.ts_cell_datatype[r] = value,
                        "unittype" => self.ts_cell_unittype[r] = value,
                        "format" => self.ts_cell_format[r] = value,
                        _ => {}
                    }
                }
                Task::none()
            }

            Message::TableStyleBorderEdit {
                cell,
                border,
                field,
                value,
            } => {
                let (c, b) = (cell as usize, border as usize);
                if c < 3 && b < 6 {
                    match field {
                        "lw" => self.ts_border_lw[c][b] = value,
                        "color" => self.ts_border_color[c][b] = value,
                        "spacing" => self.ts_border_spacing[c][b] = value,
                        _ => {}
                    }
                }
                Task::none()
            }

            Message::TableStyleBorderSetType {
                cell,
                border,
                value,
            } => {
                use acadrust::objects::TableBorderType;
                let i = self.active_tab;
                if let Some(s) = self.tablestyle_mut(i) {
                    if let Some(bd) =
                        Self::ts_cell_of(s, cell).and_then(|c| Self::ts_border_of(c, border))
                    {
                        bd.border_type = match value.as_str() {
                            "Double" => TableBorderType::Double,
                            _ => TableBorderType::Single,
                        };
                    }
                }
                Task::none()
            }

            Message::TableStyleBorderToggleInvisible { cell, border } => {
                let i = self.active_tab;
                if let Some(s) = self.tablestyle_mut(i) {
                    if let Some(bd) =
                        Self::ts_cell_of(s, cell).and_then(|c| Self::ts_border_of(c, border))
                    {
                        bd.is_invisible = !bd.is_invisible;
                    }
                }
                Task::none()
            }

            Message::TableStyleCellToggleFill(row) => {
                let i = self.active_tab;
                if let Some(s) = self.tablestyle_mut(i) {
                    if let Some(c) = Self::ts_cell_of(s, row) {
                        c.fill_enabled = !c.fill_enabled;
                    }
                }
                Task::none()
            }

            Message::TableStyleCellSetAlign { row, value } => {
                use acadrust::objects::CellAlignment;
                let i = self.active_tab;
                if let Some(s) = self.tablestyle_mut(i) {
                    if let Some(c) = Self::ts_cell_of(s, row) {
                        c.alignment = match value.as_str() {
                            "TopLeft" => CellAlignment::TopLeft,
                            "TopCenter" => CellAlignment::TopCenter,
                            "TopRight" => CellAlignment::TopRight,
                            "MiddleLeft" => CellAlignment::MiddleLeft,
                            "MiddleRight" => CellAlignment::MiddleRight,
                            "BottomLeft" => CellAlignment::BottomLeft,
                            "BottomCenter" => CellAlignment::BottomCenter,
                            "BottomRight" => CellAlignment::BottomRight,
                            _ => CellAlignment::MiddleCenter,
                        };
                    }
                }
                Task::none()
            }

            Message::TableStyleCellApply(row) => {
                let i = self.active_tab;
                let r = row as usize;
                if r >= 3 {
                    return Task::none();
                }
                let ts = self.ts_cell_textstyle[r].trim().to_string();
                let height: Option<f64> = self.ts_cell_height[r].trim().parse().ok();
                let tc: Option<i16> = self.ts_cell_textcolor[r].trim().parse().ok();
                let fc: Option<i16> = self.ts_cell_fillcolor[r].trim().parse().ok();
                let dtype: Option<i32> = self.ts_cell_datatype[r].trim().parse().ok();
                let utype: Option<i32> = self.ts_cell_unittype[r].trim().parse().ok();
                let fmt = self.ts_cell_format[r].clone();
                // Per-border numeric edits for this cell.
                let border_vals: [(Option<i16>, Option<i16>, Option<f64>); 6] =
                    std::array::from_fn(|b| {
                        (
                            self.ts_border_lw[r][b].trim().parse().ok(),
                            self.ts_border_color[r][b].trim().parse().ok(),
                            self.ts_border_spacing[r][b].trim().parse().ok(),
                        )
                    });
                if let Some(c) = self
                    .tablestyle_mut(i)
                    .and_then(|s| Self::ts_cell_of(s, row))
                {
                    if !ts.is_empty() {
                        c.text_style_name = ts;
                    }
                    if let Some(h) = height {
                        c.text_height = h;
                    }
                    if let Some(v) = tc {
                        c.text_color = acadrust::types::Color::from_index(v);
                    }
                    if let Some(v) = fc {
                        c.fill_color = acadrust::types::Color::from_index(v);
                    }
                    if let Some(v) = dtype {
                        c.data_type = v;
                    }
                    if let Some(v) = utype {
                        c.unit_type = v;
                    }
                    c.format_string = fmt;
                    for (b, (lw, color, spacing)) in border_vals.into_iter().enumerate() {
                        if let Some(bd) = Self::ts_border_of(c, b as u8) {
                            if let Some(v) = lw {
                                bd.line_weight = acadrust::types::LineWeight::from_value(v);
                            }
                            if let Some(v) = color {
                                bd.color = acadrust::types::Color::from_index(v);
                            }
                            if let Some(v) = spacing {
                                bd.double_line_spacing = v;
                            }
                        }
                    }
                }
                Task::none()
            }

            Message::TableStyleToggle(field) => {
                use acadrust::objects::ObjectType;
                let i = self.active_tab;
                let name = self.tablestyle_selected.clone();
                for obj in self.tabs[i].scene.document.objects.values_mut() {
                    if let ObjectType::TableStyle(s) = obj {
                        if s.name == name {
                            match field {
                                "title_sup" => s.title_suppressed = !s.title_suppressed,
                                "header_sup" => s.header_suppressed = !s.header_suppressed,
                                _ => {}
                            }
                        }
                    }
                }
                Task::none()
            }

            Message::TableStyleToggleAnnotative => {
                use acadrust::objects::ObjectType;
                let i = self.active_tab;
                let name = self.tablestyle_selected.clone();
                for obj in self.tabs[i].scene.document.objects.values_mut() {
                    if let ObjectType::TableStyle(s) = obj {
                        if s.name == name {
                            s.annotative = !s.annotative;
                        }
                    }
                }
                Task::none()
            }

            Message::TableStyleDialogNew => {
                self.style_new(crate::app::StyleKind::Table);
                Task::none()
            }
            Message::TableStyleDialogCopy => {
                self.style_copy(crate::app::StyleKind::Table);
                Task::none()
            }
            Message::TableStyleDialogDelete => {
                self.style_delete(crate::app::StyleKind::Table);
                Task::none()
            }
            Message::TableStyleDialogSetCurrent => {
                // Staged: persists on Apply. The header field is the round-trip
                // source of truth ($CTABLESTYLE); the ribbon mirrors it.
                let i = self.active_tab;
                let name = self.tablestyle_selected.clone();
                if self
                    .style_names(crate::app::StyleKind::Table)
                    .contains(&name)
                {
                    self.tabs[i].scene.document.header.current_table_style_name = name.clone();
                    self.ribbon.active_table_style = name.clone();
                    self.command_line
                        .push_output(&format!("Current table style: {name}"));
                }
                Task::none()
            }

            // ── MLineStyle Dialog ─────────────────────────────────────────────
            Message::MlStyleDialogOpen => {
                use acadrust::objects::ObjectType;
                let i = self.active_tab;
                let cur = self.tabs[i].scene.document.header.multiline_style.clone();
                let exists = self.tabs[i]
                    .scene
                    .document
                    .objects
                    .values()
                    .any(|o| matches!(o, ObjectType::MLineStyle(s) if s.name == cur));
                self.mlstyle_selected = if exists {
                    cur
                } else {
                    self.tabs[i]
                        .scene
                        .document
                        .objects
                        .values()
                        .find_map(|o| {
                            if let ObjectType::MLineStyle(s) = o {
                                Some(s.name.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| "Standard".to_string())
                };
                self.active_modal = Some(super::ModalKind::MlStyle);
                self.style_stage_begin();
                Task::none()
            }
            Message::MlStyleDialogClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::MlStyleDialogSelect(name) => {
                self.mlstyle_selected = name;
                Task::none()
            }
            Message::MlStyleDialogSetCurrent => {
                use acadrust::objects::ObjectType;
                let i = self.active_tab;
                let name = self.mlstyle_selected.clone();
                let exists = self.tabs[i]
                    .scene
                    .document
                    .objects
                    .values()
                    .any(|o| matches!(o, ObjectType::MLineStyle(s) if s.name == name));
                if exists {
                    // Staged: persists on Apply.
                    self.tabs[i].scene.document.header.multiline_style = name.clone();
                    self.command_line
                        .push_output(&format!("Current multiline style: {}", name));
                }
                Task::none()
            }
            // Placeholder so the Multiline manager has the same Set Current +
            // Apply pair as every other style manager. The editor is currently
            // read-only, so there is nothing to apply yet — wire this up when
            // editable MLineStyle properties land.
            Message::MlStyleApply => {
                // Multiline styles have no editable properties yet; Apply still
                // commits any staged structural / current-style changes.
                self.style_stage_commit();
                Task::none()
            }
            Message::MlStyleDialogNew => {
                self.style_new(crate::app::StyleKind::MLine);
                Task::none()
            }
            Message::MlStyleDialogCopy => {
                self.style_copy(crate::app::StyleKind::MLine);
                Task::none()
            }
            Message::MlStyleDialogDelete => {
                self.style_delete(crate::app::StyleKind::MLine);
                Task::none()
            }

            // ── MLeaderStyle Dialog ───────────────────────────────────────────
            Message::MLeaderStyleDialogOpen => {
                use acadrust::objects::ObjectType;
                let i = self.active_tab;
                let cur = self.tabs[i].active_mleader_style.clone();
                let exists = self.tabs[i]
                    .scene
                    .document
                    .objects
                    .values()
                    .any(|o| matches!(o, ObjectType::MultiLeaderStyle(s) if s.name == cur));
                self.mleaderstyle_selected = if exists {
                    cur
                } else {
                    self.tabs[i]
                        .scene
                        .document
                        .objects
                        .values()
                        .find_map(|o| {
                            if let ObjectType::MultiLeaderStyle(s) = o {
                                Some(s.name.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| "Standard".to_string())
                };
                self.load_mleaderstyle_bufs(i);
                self.active_modal = Some(super::ModalKind::MLeaderStyle);
                self.style_stage_begin();
                Task::none()
            }
            Message::MLeaderStyleDialogClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::MLeaderStyleDialogSelect(name) => {
                self.mleaderstyle_selected = name;
                let i = self.active_tab;
                self.load_mleaderstyle_bufs(i);
                Task::none()
            }
            Message::MLeaderStyleDialogSetCurrent => {
                use acadrust::objects::ObjectType;
                let i = self.active_tab;
                let name = self.mleaderstyle_selected.clone();
                let exists = self.tabs[i]
                    .scene
                    .document
                    .objects
                    .values()
                    .any(|o| matches!(o, ObjectType::MultiLeaderStyle(s) if s.name == name));
                if exists {
                    // Staged: header field is the round-trip source of truth
                    // ($CMLEADERSTYLE); the ribbon/tab mirror it.
                    self.tabs[i]
                        .scene
                        .document
                        .header
                        .current_mleader_style_name = name.clone();
                    self.tabs[i].active_mleader_style = name.clone();
                    self.ribbon.active_mleader_style = name.clone();
                    self.command_line
                        .push_output(&format!("Current multileader style: {}", name));
                }
                Task::none()
            }
            Message::MLeaderStyleDialogNew => {
                self.style_new(crate::app::StyleKind::MLeader);
                Task::none()
            }
            Message::MLeaderStyleDialogCopy => {
                self.style_copy(crate::app::StyleKind::MLeader);
                Task::none()
            }
            Message::MLeaderStyleDialogDelete => {
                self.style_delete(crate::app::StyleKind::MLeader);
                Task::none()
            }
            Message::MLeaderStyleEdit { field, value } => {
                self.mls_color_open = None;
                match field {
                    "landing_distance" => self.mls_landing_distance = value,
                    "landing_gap" => self.mls_landing_gap = value,
                    "arrowhead_size" => self.mls_arrowhead_size = value,
                    "text_height" => self.mls_text_height = value,
                    "scale_factor" => self.mls_scale_factor = value,
                    "break_gap" => self.mls_break_gap = value,
                    "first_seg_angle" => self.mls_first_seg_angle = value,
                    "second_seg_angle" => self.mls_second_seg_angle = value,
                    "max_points" => self.mls_max_points = value,
                    "default_text" => self.mls_default_text = value,
                    "line_color" => self.mls_line_color = value,
                    "text_color" => self.mls_text_color = value,
                    "description" => self.mls_description = value,
                    "line_weight" => self.mls_line_weight = value,
                    "align_space" => self.mls_align_space = value,
                    "block_color" => self.mls_block_color = value,
                    "block_rotation" => self.mls_block_rotation = value,
                    "block_scale_x" => self.mls_block_scale_x = value,
                    "block_scale_y" => self.mls_block_scale_y = value,
                    "block_scale_z" => self.mls_block_scale_z = value,
                    _ => {}
                }
                Task::none()
            }
            Message::MLeaderStyleToggle(field) => {
                let i = self.active_tab;
                if let Some(s) = self.mleaderstyle_mut(i) {
                    match field {
                        "enable_landing" => s.enable_landing = !s.enable_landing,
                        "enable_dogleg" => s.enable_dogleg = !s.enable_dogleg,
                        "text_frame" => s.text_frame = !s.text_frame,
                        "text_always_left" => s.text_always_left = !s.text_always_left,
                        "annotative" => s.is_annotative = !s.is_annotative,
                        "enable_block_scale" => s.enable_block_scale = !s.enable_block_scale,
                        "enable_block_rotation" => {
                            s.enable_block_rotation = !s.enable_block_rotation
                        }
                        _ => {}
                    }
                }
                Task::none()
            }
            Message::MLeaderColorMore(field) => {
                self.mls_color_open = if self.mls_color_open == Some(field) {
                    None
                } else {
                    Some(field)
                };
                Task::none()
            }
            Message::MLeaderStyleSetEnum { field, value } => {
                use acadrust::objects::{
                    BlockContentConnectionType, LeaderContentType, LeaderDrawOrderType,
                    MultiLeaderDrawOrderType, MultiLeaderPathType, TextAlignmentType,
                    TextAngleType, TextAttachmentDirectionType, TextAttachmentType,
                };
                // Parse a TextAttachmentType from its debug name.
                fn parse_att(v: &str) -> TextAttachmentType {
                    match v {
                        "TopOfTopLine" => TextAttachmentType::TopOfTopLine,
                        "MiddleOfText" => TextAttachmentType::MiddleOfText,
                        "MiddleOfBottomLine" => TextAttachmentType::MiddleOfBottomLine,
                        "BottomOfBottomLine" => TextAttachmentType::BottomOfBottomLine,
                        "BottomLine" => TextAttachmentType::BottomLine,
                        "BottomOfTopLineUnderlineBottomLine" => {
                            TextAttachmentType::BottomOfTopLineUnderlineBottomLine
                        }
                        "BottomOfTopLineUnderlineTopLine" => {
                            TextAttachmentType::BottomOfTopLineUnderlineTopLine
                        }
                        "BottomOfTopLineUnderlineAll" => {
                            TextAttachmentType::BottomOfTopLineUnderlineAll
                        }
                        "CenterOfText" => TextAttachmentType::CenterOfText,
                        "CenterOfTextOverline" => TextAttachmentType::CenterOfTextOverline,
                        _ => TextAttachmentType::MiddleOfTopLine,
                    }
                }
                let i = self.active_tab;
                if let Some(s) = self.mleaderstyle_mut(i) {
                    match field {
                        "path_type" => {
                            s.path_type = match value.as_str() {
                                "Invisible" => MultiLeaderPathType::Invisible,
                                "Spline" => MultiLeaderPathType::Spline,
                                _ => MultiLeaderPathType::StraightLineSegments,
                            };
                        }
                        "content_type" => {
                            s.content_type = match value.as_str() {
                                "None" => LeaderContentType::None,
                                "Block" => LeaderContentType::Block,
                                "Tolerance" => LeaderContentType::Tolerance,
                                _ => LeaderContentType::MText,
                            };
                        }
                        "text_angle_type" => {
                            s.text_angle_type = match value.as_str() {
                                "ParallelToLastLeaderLine" => {
                                    TextAngleType::ParallelToLastLeaderLine
                                }
                                "Optimized" => TextAngleType::Optimized,
                                _ => TextAngleType::Horizontal,
                            };
                        }
                        "text_alignment" => {
                            s.text_alignment = match value.as_str() {
                                "Center" => TextAlignmentType::Center,
                                "Right" => TextAlignmentType::Right,
                                _ => TextAlignmentType::Left,
                            };
                        }
                        "text_left_attachment" => s.text_left_attachment = parse_att(&value),
                        "text_right_attachment" => s.text_right_attachment = parse_att(&value),
                        "text_top_attachment" => s.text_top_attachment = parse_att(&value),
                        "text_bottom_attachment" => s.text_bottom_attachment = parse_att(&value),
                        "text_attachment_direction" => {
                            s.text_attachment_direction = match value.as_str() {
                                "Vertical" => TextAttachmentDirectionType::Vertical,
                                _ => TextAttachmentDirectionType::Horizontal,
                            };
                        }
                        "block_content_connection" => {
                            s.block_content_connection = match value.as_str() {
                                "BasePoint" => BlockContentConnectionType::BasePoint,
                                _ => BlockContentConnectionType::BlockExtents,
                            };
                        }
                        "leader_draw_order" => {
                            s.leader_draw_order = match value.as_str() {
                                "LeaderTailFirst" => LeaderDrawOrderType::LeaderTailFirst,
                                _ => LeaderDrawOrderType::LeaderHeadFirst,
                            };
                        }
                        "multileader_draw_order" => {
                            s.multileader_draw_order = match value.as_str() {
                                "LeaderFirst" => MultiLeaderDrawOrderType::LeaderFirst,
                                _ => MultiLeaderDrawOrderType::ContentFirst,
                            };
                        }
                        _ => {}
                    }
                }
                Task::none()
            }
            Message::MLeaderStyleSetHandle { field, value } => {
                let i = self.active_tab;
                let doc = &self.tabs[i].scene.document;
                let handle: Option<acadrust::types::Handle> = if value == "None" {
                    None
                } else {
                    match field {
                        "line_type_handle" => doc
                            .line_types
                            .iter()
                            .find(|lt| lt.name == value)
                            .map(|lt| lt.handle),
                        "text_style_handle" => doc
                            .text_styles
                            .iter()
                            .find(|t| t.name == value)
                            .map(|t| t.handle),
                        "arrowhead_handle" | "block_content_handle" => doc
                            .block_records
                            .iter()
                            .find(|b| b.name == value)
                            .map(|b| b.handle),
                        _ => None,
                    }
                };
                if let Some(s) = self.mleaderstyle_mut(i) {
                    match field {
                        "line_type_handle" => s.line_type_handle = handle,
                        "text_style_handle" => s.text_style_handle = handle,
                        "arrowhead_handle" => s.arrowhead_handle = handle,
                        "block_content_handle" => s.block_content_handle = handle,
                        _ => {}
                    }
                }
                Task::none()
            }
            Message::MLeaderStyleApply => {
                let i = self.active_tab;
                let (ld, lg, asz, th, sf, bg, fsa, ssa, mp, dt, lc, tc) = (
                    self.mls_landing_distance.parse::<f64>().ok(),
                    self.mls_landing_gap.parse::<f64>().ok(),
                    self.mls_arrowhead_size.parse::<f64>().ok(),
                    self.mls_text_height.parse::<f64>().ok(),
                    self.mls_scale_factor.parse::<f64>().ok(),
                    self.mls_break_gap.parse::<f64>().ok(),
                    self.mls_first_seg_angle.parse::<f64>().ok(),
                    self.mls_second_seg_angle.parse::<f64>().ok(),
                    self.mls_max_points.parse::<i32>().ok(),
                    self.mls_default_text.clone(),
                    self.mls_line_color.parse::<i16>().ok(),
                    self.mls_text_color.parse::<i16>().ok(),
                );
                let desc = self.mls_description.clone();
                let lw = self.mls_line_weight.parse::<i16>().ok();
                let align = self.mls_align_space.parse::<f64>().ok();
                let bclr = self.mls_block_color.parse::<i16>().ok();
                let brot = self.mls_block_rotation.parse::<f64>().ok();
                let bsx = self.mls_block_scale_x.parse::<f64>().ok();
                let bsy = self.mls_block_scale_y.parse::<f64>().ok();
                let bsz = self.mls_block_scale_z.parse::<f64>().ok();
                if let Some(s) = self.mleaderstyle_mut(i) {
                    if let Some(v) = ld {
                        s.landing_distance = v;
                    }
                    if let Some(v) = lg {
                        s.landing_gap = v;
                    }
                    if let Some(v) = asz {
                        s.arrowhead_size = v;
                    }
                    if let Some(v) = th {
                        s.text_height = v;
                    }
                    if let Some(v) = sf {
                        s.scale_factor = v;
                    }
                    if let Some(v) = bg {
                        s.break_gap_size = v;
                    }
                    if let Some(v) = fsa {
                        s.first_segment_angle = v;
                    }
                    if let Some(v) = ssa {
                        s.second_segment_angle = v;
                    }
                    if let Some(v) = mp {
                        s.max_leader_points = v;
                    }
                    s.default_text = dt;
                    if let Some(v) = lc {
                        s.line_color = acadrust::types::Color::from_index(v);
                    }
                    if let Some(v) = tc {
                        s.text_color = acadrust::types::Color::from_index(v);
                    }
                    s.description = desc;
                    if let Some(v) = lw {
                        s.line_weight = acadrust::types::LineWeight::from_value(v);
                    }
                    if let Some(v) = align {
                        s.align_space = v;
                    }
                    if let Some(v) = bclr {
                        s.block_content_color = acadrust::types::Color::from_index(v);
                    }
                    if let Some(v) = brot {
                        s.block_content_rotation = v;
                    }
                    if let Some(v) = bsx {
                        s.block_content_scale_x = v;
                    }
                    if let Some(v) = bsy {
                        s.block_content_scale_y = v;
                    }
                    if let Some(v) = bsz {
                        s.block_content_scale_z = v;
                    }
                }
                self.style_stage_commit();
                Task::none()
            }

            // ── DimStyle Dialog ───────────────────────────────────────────────
            Message::DimStyleDialogOpen => {
                let i = self.active_tab;
                // Pick the document's current dim style or "Standard".
                let cur = self.tabs[i]
                    .scene
                    .document
                    .header
                    .current_dimstyle_name
                    .clone();
                let selected = if self.tabs[i].scene.document.dim_styles.get(&cur).is_some() {
                    cur
                } else {
                    self.tabs[i]
                        .scene
                        .document
                        .dim_styles
                        .iter()
                        .next()
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "Standard".to_string())
                };
                self.dimstyle_selected = selected.clone();
                self.load_dimstyle_bufs(i);
                self.active_modal = Some(super::ModalKind::DimStyle);
                self.style_stage_begin();
                Task::none()
            }
            Message::DimStyleDialogClose => {
                self.close_active_modal();
                Task::none()
            }
            Message::DimStyleDialogApply => {
                let i = self.active_tab;
                self.apply_dimstyle_bufs(i);
                self.style_stage_commit();
                Task::none()
            }
            Message::DimStyleDialogSelect(name) => {
                let i = self.active_tab;
                self.dimstyle_selected = name;
                self.load_dimstyle_bufs(i);
                Task::none()
            }
            Message::DimStyleDialogTab(tab) => {
                self.dimstyle_tab = tab;
                Task::none()
            }
            Message::DimStyleDialogNew => {
                self.style_new(crate::app::StyleKind::Dim);
                Task::none()
            }
            Message::DimStyleDialogCopy => {
                self.style_copy(crate::app::StyleKind::Dim);
                Task::none()
            }
            Message::DimStyleDialogSetCurrent => {
                // Staged: persists on Apply.
                let i = self.active_tab;
                self.tabs[i].scene.document.header.current_dimstyle_name =
                    self.dimstyle_selected.clone();
                self.sync_ribbon_styles();
                self.command_line.push_output(&format!(
                    "Current dim style set to '{}'.",
                    self.dimstyle_selected
                ));
                Task::none()
            }
            Message::DimStyleDialogDelete => {
                self.style_delete(crate::app::StyleKind::Dim);
                Task::none()
            }
            Message::DsEdit(field, val) => {
                self.apply_ds_edit(field, val);
                self.ds_color_open = None;
                Task::none()
            }
            Message::DsToggle(field) => {
                self.apply_ds_toggle(field);
                Task::none()
            }
            Message::DsColorMore(field) => {
                self.ds_color_open = if self.ds_color_open.as_ref() == Some(&field) {
                    None
                } else {
                    Some(field)
                };
                Task::none()
            }
            Message::OpenColorWindow(target) => {
                self.color_pick_target = Some(target);
                self.ds_color_open = None;
                self.mls_color_open = None;
                self.ts_color_open = None;
                self.ribbon.close_dropdown();
                let i = self.active_tab;
                self.tabs[i].properties.color_picker_open = false;
                self.tabs[i].layers.color_picker_row = None;
                // Shown as a nested modal over the active dialog (Plan B):
                // `color_pick_target.is_some()` drives the overlay in view_main.
                Task::none()
            }
            Message::CloseColorPicker => {
                self.color_pick_target = None;
                Task::none()
            }
            Message::ColorWindowPick(color) => {
                let s = crate::ui::color_select::color_to_aci_string(color);
                let edit = match self.color_pick_target.take() {
                    Some(crate::app::ColorPickTarget::DimStyle(f)) => Some(Message::DsEdit(f, s)),
                    Some(crate::app::ColorPickTarget::MLeader(f)) => {
                        Some(Message::MLeaderStyleEdit { field: f, value: s })
                    }
                    Some(crate::app::ColorPickTarget::Table(r, f)) => {
                        Some(Message::TableStyleCellEdit {
                            row: r,
                            field: f,
                            value: s,
                        })
                    }
                    Some(crate::app::ColorPickTarget::Properties) => {
                        Some(Message::PropColorChanged(color))
                    }
                    Some(crate::app::ColorPickTarget::Ribbon) => {
                        Some(Message::RibbonColorChanged(color))
                    }
                    Some(crate::app::ColorPickTarget::Layer(idx)) => {
                        self.tabs[self.active_tab].layers.selected = Some(idx);
                        match color {
                            acadrust::types::Color::Index(i) => Some(Message::LayerColorSet(i)),
                            _ => None,
                        }
                    }
                    None => None,
                };
                if let Some(m) = edit {
                    self.update(m)
                } else {
                    Task::none()
                }
            }
            Message::DsSetHandle { field, value } => {
                let i = self.active_tab;
                let name = self.dimstyle_selected.clone();
                let is_lt = matches!(
                    field,
                    "dimltex_handle" | "dimltex1_handle" | "dimltex2_handle"
                );
                let doc = &self.tabs[i].scene.document;
                let handle = if value == "Default" || value == "ByBlock" {
                    acadrust::types::Handle::NULL
                } else if is_lt {
                    doc.line_types
                        .iter()
                        .find(|lt| lt.name == value)
                        .map(|lt| lt.handle)
                        .unwrap_or(acadrust::types::Handle::NULL)
                } else {
                    doc.block_records
                        .iter()
                        .find(|b| b.name == value)
                        .map(|b| b.handle)
                        .unwrap_or(acadrust::types::Handle::NULL)
                };
                // Staged: persists on Apply.
                if let Some(ds) = self.tabs[i].scene.document.dim_styles.get_mut(&name) {
                    match field {
                        "dimblk" => ds.dimblk = handle,
                        "dimblk1" => ds.dimblk1 = handle,
                        "dimblk2" => ds.dimblk2 = handle,
                        "dimldrblk" => ds.dimldrblk = handle,
                        "dimltex_handle" => ds.dimltex_handle = handle,
                        "dimltex1_handle" => ds.dimltex1_handle = handle,
                        "dimltex2_handle" => ds.dimltex2_handle = handle,
                        _ => {}
                    }
                }
                Task::none()
            }
        }
    }
}

// ── DimStyle dialog helpers ─────────────────────────────────────────────────

impl OpenCADStudio {
    /// Rebuild the active tab's dynamic-input field set to match what the
    /// command is currently asking for. Called on cursor move and after
    /// command-state changes. The field set only changes shape when the
    /// command's `dyn_field()` or the presence of a base point changes;
    /// existing typed buffers survive an unchanged shape.
    pub(super) fn sync_dyn_fields(&mut self) {
        use super::document::{DynComponent, DynFieldEntry};
        let i = self.active_tab;
        if !self.dyn_input || self.tabs[i].active_cmd.is_none() {
            self.tabs[i].dyn_fields.clear();
            self.tabs[i].dyn_active = 0;
            return;
        }
        // A command may describe its step explicitly via `dyn_spec()` — that
        // takes full control of the boxes, guide and anchor. Otherwise fall
        // back to the legacy `dyn_field()` shaping below.
        if let Some(spec) = self.tabs[i].active_cmd.as_ref().and_then(|c| c.dyn_spec()) {
            self.apply_dyn_spec(i, spec);
            return;
        }
        let field = self.tabs[i]
            .active_cmd
            .as_ref()
            .map(|c| c.dyn_field())
            .unwrap_or(crate::command::DynField::Point);
        // A text-input step reads a single scalar. The overlay shows one box
        // the user types into (or, when the command supplies a live value,
        // sets by moving the cursor); on commit the host routes it to
        // `on_text_input` instead of `on_point`. A distance prompt keeps the
        // `Distance` box (so a perpendicular-distance live value reads
        // naturally); everything else uses the typed-only `Scalar` box.
        let wants_text = self.tabs[i]
            .active_cmd
            .as_ref()
            .map(|c| c.wants_text_input())
            .unwrap_or(false);
        // A point step that also accepts keyword letters (PLINE A/L/C…) keeps
        // its polar boxes: only letters reach the command line, digits stay
        // coordinates. So such a step is NOT treated as a text-only prompt.
        let point_keywords = self.tabs[i]
            .active_cmd
            .as_ref()
            .map(|c| c.point_step_accepts_keywords())
            .unwrap_or(false);
        let wants_text = wants_text && !point_keywords;
        // A step that hit-tests for an object (entity / structure pick) has no
        // coordinate to enter — clicks select, they don't place a point. Show
        // no coordinate box so the cursor stays clean and typed option keywords
        // (e.g. FILLET's "R") reach the command line instead of an X/Y field.
        let picks_object = self.tabs[i]
            .active_cmd
            .as_ref()
            .map(|c| c.needs_entity_pick() || c.needs_structure_point_pick())
            .unwrap_or(false);
        let has_base = self.last_point.is_some();
        // While aligned to an OTRACK ray, the point step reads a single
        // distance along the ray (issue #69) — show one Distance box.
        let otrack_dist = self.otrack_active.is_some()
            && !wants_text
            && matches!(field, crate::command::DynField::Point);
        let default: Vec<DynComponent> = match field {
            _ if otrack_dist => vec![DynComponent::Distance],
            crate::command::DynField::Distance => vec![DynComponent::Distance],
            crate::command::DynField::Angle => vec![DynComponent::Angle],
            crate::command::DynField::Scalar => vec![DynComponent::Scalar],
            // A text prompt with the default `Point` field reads free text /
            // a name / a keyword (which may itself contain digits) from the
            // command line — show no scalar box, so digits reach the command
            // line instead of being captured into a dyn buffer.
            crate::command::DynField::Point if wants_text || picks_object => vec![],
            crate::command::DynField::Point if has_base => {
                vec![DynComponent::Distance, DynComponent::Angle]
            }
            crate::command::DynField::Point => vec![DynComponent::X, DynComponent::Y],
        };
        // Multiple shapes can satisfy the same command request — e.g. a
        // `Point` is happy with either `[Distance, Angle]` (polar) or
        // `[X, Y]` / `[X, Y, Z]` (cartesian). If the user already
        // reshaped via `,` (see #35) the existing set is still a valid
        // Point configuration and must not be reverted on every mouse
        // move.
        let current: Vec<DynComponent> = self.tabs[i]
            .dyn_fields
            .iter()
            .map(|f| f.component)
            .collect();
        // Only treat a cartesian / polar variant as "good enough to keep"
        // when the user explicitly reshaped via `,`. Otherwise we follow
        // the command's default so e.g. clicking the first point of LINE
        // flips a stale `[X, Y]` (from before there was a base) over to
        // the polar `[Distance, Angle]` the prompt actually wants.
        let current_is_acceptable = if self.dyn_user_reshaped && !wants_text && !picks_object {
            match field {
                crate::command::DynField::Distance => {
                    matches!(current.as_slice(), [DynComponent::Distance])
                }
                crate::command::DynField::Angle => {
                    matches!(current.as_slice(), [DynComponent::Angle])
                }
                crate::command::DynField::Scalar => {
                    matches!(current.as_slice(), [DynComponent::Scalar])
                }
                crate::command::DynField::Point => matches!(
                    current.as_slice(),
                    [DynComponent::Distance, DynComponent::Angle]
                        | [DynComponent::X, DynComponent::Y]
                        | [DynComponent::X, DynComponent::Y, DynComponent::Z]
                ),
            }
        } else {
            current == default
        };
        if !current_is_acceptable {
            self.tabs[i].dyn_fields = default.into_iter().map(DynFieldEntry::new).collect();
            self.tabs[i].dyn_active = 0;
        }
        // Derive the guide + anchor for the legacy field set so the overlay
        // draws the right construction without each command opting in.
        let comps: Vec<DynComponent> =
            self.tabs[i].dyn_fields.iter().map(|f| f.component).collect();
        self.tabs[i].dyn_guide = match comps.as_slice() {
            [DynComponent::Distance, DynComponent::Angle] | [DynComponent::Angle] => {
                crate::command::DynGuide::Polar
            }
            [DynComponent::Distance] => crate::command::DynGuide::Radius,
            _ => crate::command::DynGuide::None,
        };
        self.tabs[i].dyn_anchor = self.last_point;
        self.tabs[i].dyn_ref = None;
    }

    /// Apply an explicit per-step [`DynSpec`](crate::command::DynSpec): rebuild
    /// the boxes from its roles (preserving typed buffers when the role set is
    /// unchanged), and set the guide + anchor.
    fn apply_dyn_spec(&mut self, i: usize, spec: crate::command::DynSpec) {
        use super::document::DynFieldEntry;
        let new_roles: Vec<crate::command::DynRole> =
            spec.fields.iter().map(|f| f.role).collect();
        let cur_roles: Vec<crate::command::DynRole> =
            self.tabs[i].dyn_fields.iter().map(|f| f.role).collect();
        if cur_roles != new_roles {
            self.tabs[i].dyn_fields =
                spec.fields.iter().map(|f| DynFieldEntry::from_role(f.role)).collect();
            self.tabs[i].dyn_active = 0;
        }
        self.tabs[i].dyn_guide = spec.guide;
        self.tabs[i].dyn_anchor = match spec.anchor {
            crate::command::DynAnchor::LastPoint => self.last_point,
            crate::command::DynAnchor::Point(p) => Some(p),
        };
        self.tabs[i].dyn_ref = spec.ref_point;
    }

    /// Track cursor dwell over a selected entity's grip. Sets
    /// `grip_hover` while the cursor sits within `GRIP_THRESHOLD_PX` of
    /// a grip and opens `grip_popup` once the dwell exceeds the
    /// threshold. Cursor drift clears both.
    /// After the active Model tile changes, mirror its stored visual style
    /// into the tab so the picker shows it and the tile renders with it
    /// (the active tile draws with the tab's live render mode).
    fn sync_render_mode_to_active_tile(&mut self, i: usize) {
        use acadrust::entities::ViewportRenderMode as M;
        if self.tabs[i].scene.current_layout != "Model" {
            return;
        }
        let mode = self.tabs[i].scene.active_model_tile_render_mode();
        if self.tabs[i].render_mode == mode {
            return;
        }
        let label = match mode {
            M::Wireframe2D => "Wireframe 2D",
            M::Wireframe3D => "Wireframe 3D",
            M::HiddenLine => "Hidden Line",
            M::FlatShaded => "Flat Shaded",
            M::GouraudShaded => "Gouraud Shaded",
            M::FlatShadedWithEdges => "Flat Shaded + Edges",
            M::GouraudShadedWithEdges => "Gouraud Shaded + Edges",
        };
        self.tabs[i].render_mode = mode;
        let wf = matches!(mode, M::Wireframe2D | M::Wireframe3D);
        self.tabs[i].wireframe = wf;
        self.ribbon.set_wireframe(wf);
        self.tabs[i].visual_style = label.into();
        self.tabs[i].scene.bump_geometry();
    }

    fn update_grip_hover(&mut self, i: usize, p: iced::Point) {
        const HOVER_OPEN_MS: u128 = 600;
        const POPUP_DISMISS_PX: f32 = 80.0;
        if self.tabs[i].active_cmd.is_some()
            || self.tabs[i].active_grip.is_some()
            || self.tabs[i].selected_grips.is_empty()
        {
            self.grip_hover = None;
            self.grip_popup = None;
            return;
        }
        let Some(handle) = self.tabs[i].selected_handle else {
            self.grip_hover = None;
            self.grip_popup = None;
            return;
        };
        let (vw, vh) = self.tabs[i].scene.selection.borrow().vp_size;
        let bounds = iced::Rectangle {
            x: 0.0,
            y: 0.0,
            width: vw,
            height: vh,
        };
        let is_paper = self.tabs[i].scene.current_layout != "Model";
        let hit = if is_paper {
            let cam = self.tabs[i].scene.camera.borrow();
            let aspect = if vh > 0.0 { vw / vh } else { 1.0 };
            let half_h = cam.ortho_size();
            let half_w = half_h * aspect;
            let tx = cam.target.x;
            let ty = cam.target.y;
            drop(cam);
            find_hit_grip_paper(
                p,
                &self.tabs[i].selected_grips,
                tx,
                ty,
                half_w,
                half_h,
                bounds,
            )
        } else {
            let vp_mat = self.tabs[i].scene.camera.borrow().view_proj(bounds);
            find_hit_grip(p, &self.tabs[i].selected_grips, vp_mat, bounds)
        };
        match hit {
            Some((grip_id, _, _)) => {
                let same = self
                    .grip_hover
                    .as_ref()
                    .map_or(false, |h| h.handle == handle && h.grip_id == grip_id);
                if !same {
                    self.grip_hover = Some(super::GripHover {
                        handle,
                        grip_id,
                        screen: p,
                        started: iced::time::Instant::now(),
                    });
                    self.grip_popup = None;
                } else if let Some(h) = self.grip_hover.as_mut() {
                    h.screen = p;
                }
                // Open popup once dwell crosses the threshold. The visibility
                // grip has its own click-to-open dropdown, so it gets no
                // hover grip-menu.
                if self.grip_popup.is_none()
                    && grip_id != super::visibility::VIS_GRIP_ID
                    && self
                        .grip_hover
                        .as_ref()
                        .map_or(false, |h| h.started.elapsed().as_millis() >= HOVER_OPEN_MS)
                {
                    let entity_opt = self.tabs[i].scene.document.get_entity(handle);
                    if let Some(e) = entity_opt {
                        use crate::entities::traits::EntityTypeOps;
                        let items = e.grip_menu(grip_id);
                        if !items.is_empty() {
                            self.grip_popup = Some(super::GripPopup {
                                handle,
                                grip_id,
                                anchor: p,
                                items,
                                selected: 0,
                            });
                        }
                    }
                }
            }
            None => {
                self.grip_hover = None;
                if let Some(popup) = &self.grip_popup {
                    let dx = p.x - popup.anchor.x;
                    let dy = p.y - popup.anchor.y;
                    if (dx * dx + dy * dy).sqrt() > POPUP_DISMISS_PX {
                        self.grip_popup = None;
                    }
                }
            }
        }
    }

    /// Re-run the active command's preview hook against the current
    /// cursor world position. Keyboard-driven point commits (typed
    /// coordinates in the command line or dynamic input) don't fire a
    /// mouse-move event, so without this the rubber-band preview keeps
    /// dangling from the previous `last_point` until the user actually
    /// moves the mouse. See #32.
    fn refresh_active_cmd_preview(&mut self, i: usize) {
        if self.tabs[i].active_cmd.is_none() {
            return;
        }
        let cur = self.tabs[i].last_cursor_world;
        let previews = self.tabs[i]
            .active_cmd
            .as_mut()
            .map(|c| c.on_preview_wires(cur))
            .unwrap_or_default();
        self.tabs[i].scene.set_preview_wires(previews);
    }

    /// Resolve the world point implied by the current dynamic-input field
    /// values. Locked fields use their typed buffer; the rest fall back to
    /// the live cursor-derived value. Returns `None` when the field set
    /// isn't one we know how to turn into a point.
    fn dyn_resolve_point(&self) -> Option<glam::Vec3> {
        use super::document::DynComponent;
        let i = self.active_tab;
        let fields = &self.tabs[i].dyn_fields;
        if fields.is_empty() {
            return None;
        }
        let w = self.tabs[i].last_cursor_world;
        let base = self.tabs[i]
            .dyn_anchor
            .or(self.last_point)
            .unwrap_or(glam::Vec3::ZERO);
        // Buffer value parsed as f32 (de-scaled by the role so a typed diameter
        // becomes a radius), or the supplied geometric live value. Width/Height
        // are shown unsigned, so a typed value takes the sign of the cursor's
        // delta on that axis (`live` is the signed delta in the cartesian arms).
        let val = |idx: usize, live: f32| -> f32 {
            match fields[idx]
                .buffer
                .as_ref()
                .map(|s| s.trim().replace(',', "."))
                .and_then(|s| crate::app::expr_eval::eval_number(&s).map(|v| v as f32))
                .map(|v| v / fields[idx].role.value_scale())
            {
                Some(v) => {
                    if matches!(
                        fields[idx].role,
                        crate::command::DynRole::Width | crate::command::DynRole::Height
                    ) {
                        v.abs().copysign(live)
                    } else {
                        v
                    }
                }
                None => live,
            }
        };
        let dx = w.x - base.x;
        let dy = w.y - base.y;
        let live_d = (dx * dx + dy * dy).sqrt();
        let live_a = dy.atan2(dx); // radians
        // A typed angle is shown unsigned (0..180); give it the sign of the
        // cursor's current side so an entry made below the X axis sweeps
        // downward to match the arc instead of mirroring up. Untyped → live.
        let angle_rad = |idx: usize| -> f32 {
            match fields[idx]
                .buffer
                .as_ref()
                .map(|s| s.trim().replace(',', "."))
                .and_then(|s| crate::app::expr_eval::eval_number(&s).map(|v| v as f32))
            {
                Some(mag) => mag.abs().to_radians().copysign(dy),
                None => live_a,
            }
        };
        let comps: Vec<DynComponent> = fields.iter().map(|f| f.component).collect();
        // Perpendicular offset: a single distance measured square to the
        // reference line (anchor → dyn_ref). The committed point lies on the
        // perpendicular through the anchor at that offset; the command projects
        // it. Untyped tracks the cursor's signed offset; typed takes the
        // cursor's side.
        if let (Some(ref_pt), [DynComponent::Distance]) =
            (self.tabs[i].dyn_ref, comps.as_slice())
        {
            let axis = (ref_pt - base).normalize_or_zero();
            let perp = glam::Vec3::new(-axis.y, axis.x, 0.0);
            let signed = (w - base).dot(perp);
            let typed = fields[0]
                .buffer
                .as_ref()
                .map(|s| s.trim().replace(',', "."))
                .and_then(|s| crate::app::expr_eval::eval_number(&s).map(|v| v as f32));
            let h = match typed {
                Some(v) => v.abs().copysign(signed),
                None => signed,
            };
            return Some(base + perp * h);
        }
        // DYN-on defaults to RELATIVE coordinates when a base point is set
        // (see #26 / #35). The live cartesian fallback is the cursor
        // position relative to base; typed values are relative deltas.
        let has_base = self.last_point.is_some();
        match comps.as_slice() {
            [DynComponent::X, DynComponent::Y] if has_base => Some(glam::Vec3::new(
                base.x + val(0, dx),
                base.y + val(1, dy),
                base.z,
            )),
            [DynComponent::X, DynComponent::Y] => {
                Some(glam::Vec3::new(val(0, w.x), val(1, w.y), base.z))
            }
            [DynComponent::X, DynComponent::Y, DynComponent::Z] if has_base => {
                Some(glam::Vec3::new(
                    base.x + val(0, dx),
                    base.y + val(1, dy),
                    base.z + val(2, 0.0),
                ))
            }
            [DynComponent::X, DynComponent::Y, DynComponent::Z] => {
                Some(glam::Vec3::new(val(0, w.x), val(1, w.y), val(2, base.z)))
            }
            [DynComponent::Distance, DynComponent::Angle] => {
                let d = val(0, live_d);
                let a = angle_rad(1);
                Some(glam::Vec3::new(
                    base.x + d * a.cos(),
                    base.y + d * a.sin(),
                    base.z,
                ))
            }
            [DynComponent::Distance] => {
                // Keep the cursor's direction, override the magnitude.
                let dir = glam::Vec3::new(dx, dy, 0.0).normalize_or(glam::Vec3::X);
                Some(base + dir * val(0, live_d))
            }
            [DynComponent::Angle] => {
                // Standalone angle (e.g. ROTATE): the typed value is an
                // absolute CCW angle, not a cursor-signed magnitude — keep it
                // literal. Only the polar Distance+Angle pair uses the
                // cursor-signed `angle_rad`.
                let a = val(0, live_a.to_degrees()).to_radians();
                Some(glam::Vec3::new(
                    base.x + live_d * a.cos(),
                    base.y + live_d * a.sin(),
                    base.z,
                ))
            }
            _ => None,
        }
    }

    /// Handle `,` while a dynamic-input field set is showing. Locks the
    /// current field's buffer if it has one, then either advances within
    /// the existing field set or reshapes it: a polar `[Distance, Angle]`
    /// configuration becomes cartesian `[X(buf), Y]`, and a cartesian
    /// `[X, Y]` configuration extends to `[X, Y, Z]`. Default fallthrough
    /// is "advance to next field", matching `Tab`. See #35.
    fn dyn_comma_advance(&mut self) {
        use super::document::{DynComponent, DynFieldEntry};
        let i = self.active_tab;
        if self.tabs[i].dyn_fields.is_empty() {
            return;
        }
        // The user picked a shape — `sync_dyn_fields` preserves it until
        // the next commit / command-start clears the flag.
        self.dyn_user_reshaped = true;
        let active = self.tabs[i]
            .dyn_active
            .min(self.tabs[i].dyn_fields.len() - 1);
        let comps: Vec<DynComponent> = self.tabs[i]
            .dyn_fields
            .iter()
            .map(|f| f.component)
            .collect();
        let cur_buf = self.tabs[i].dyn_fields[active].buffer.clone();
        match (comps.as_slice(), active) {
            // First polar field — `,` switches to cartesian, locking the
            // typed value as X.
            ([DynComponent::Distance, DynComponent::Angle], 0) | ([DynComponent::Distance], 0) => {
                let mut x_field = DynFieldEntry::new(DynComponent::X);
                x_field.buffer = cur_buf;
                self.tabs[i].dyn_fields = vec![x_field, DynFieldEntry::new(DynComponent::Y)];
                self.tabs[i].dyn_active = 1;
            }
            // Already cartesian X (first field) — just advance to Y.
            ([DynComponent::X, DynComponent::Y], 0)
            | ([DynComponent::X, DynComponent::Y, DynComponent::Z], 0) => {
                self.tabs[i].dyn_active = 1;
            }
            // Cartesian Y — extend to 3-D by appending Z.
            ([DynComponent::X, DynComponent::Y], 1) => {
                self.tabs[i]
                    .dyn_fields
                    .push(DynFieldEntry::new(DynComponent::Z));
                self.tabs[i].dyn_active = 2;
            }
            // Cartesian Y in the 3-D set — advance to Z.
            ([DynComponent::X, DynComponent::Y, DynComponent::Z], 1) => {
                self.tabs[i].dyn_active = 2;
            }
            // Z, Angle, or any singleton: nothing further to advance to.
            _ => {}
        }
    }

    /// If dynamic input has at least one locked (typed) field, resolve the
    /// implied point, feed it to the active command as a point pick, reset
    /// the field buffers, and return the resulting task. Returns `None`
    /// when there is nothing typed, so the caller falls back to its normal
    /// Enter handling.
    /// Give a bare typed angle the sign of the cursor's side relative to the
    /// step's reference direction, so a commit-as-text angle rotates/sweeps the
    /// way the cursor is dragging. Only applies to steps with an `Angle` field;
    /// an explicit `+`/`-` is left untouched. Returns the (possibly re-signed)
    /// text to feed `on_text_input`.
    fn dyn_sign_angle_text(&self, i: usize, text: String) -> String {
        let has_angle = self.tabs[i]
            .dyn_fields
            .iter()
            .any(|f| f.role == crate::command::DynRole::Angle);
        let t = text.trim();
        if !has_angle || t.is_empty() || t.starts_with('-') || t.starts_with('+') {
            return text;
        }
        if t.parse::<f32>().is_err() {
            return text;
        }
        let anchor = self.tabs[i]
            .dyn_anchor
            .or(self.last_point)
            .unwrap_or(glam::Vec3::ZERO);
        let cur = self.tabs[i].last_cursor_world;
        let a_cur = (cur.y - anchor.y).atan2(cur.x - anchor.x);
        let a_ref = self.tabs[i]
            .dyn_ref
            .map(|r| (r.y - anchor.y).atan2(r.x - anchor.x))
            .unwrap_or(0.0);
        let mut d = a_cur - a_ref;
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d <= -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        if d < 0.0 {
            format!("-{t}")
        } else {
            text
        }
    }

    fn try_dyn_commit(&mut self) -> Option<Task<Message>> {
        let i = self.active_tab;
        if !self.dyn_input
            || self.tabs[i].active_cmd.is_none()
            || self.tabs[i].dyn_fields.is_empty()
            || !self.tabs[i].dyn_fields.iter().any(|f| f.locked())
        {
            return None;
        }
        // OTRACK: while aligned to a tracking ray, a typed value is a distance
        // along the ray from the tracking point (issue #69).
        if let Some((base, dir)) = self.otrack_active {
            let wants_text = self.tabs[i]
                .active_cmd
                .as_ref()
                .map(|c| c.wants_text_input())
                .unwrap_or(false);
            if !wants_text {
                if let Some(text) = self.tabs[i]
                    .dyn_fields
                    .iter()
                    .find_map(|f| f.buffer.clone())
                {
                    if let Some(dist) = super::expr_eval::eval_number(text.trim()) {
                        let pt = base + dir * dist as f32;
                        self.last_point = Some(pt);
                        for f in self.tabs[i].dyn_fields.iter_mut() {
                            f.buffer = None;
                        }
                        self.tabs[i].dyn_active = 0;
                        self.dyn_user_reshaped = false;
                        self.sync_dyn_fields();
                        self.reset_tracking_after_point();
                        let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_point(pt));
                        let task = result.map(|r| self.apply_cmd_result(r))?;
                        self.refresh_active_cmd_preview(i);
                        return Some(task);
                    }
                }
            }
        }
        // A text-input step reads its single box as a string and commits via
        // `on_text_input` (a count, radius, distance) rather than resolving a
        // point. Only the typed buffer matters here — a mouse-driven live
        // value commits through the viewport click, not Enter.
        // A point-with-keywords step (PLINE) commits a typed distance/angle as
        // a point, not as text, so it is excluded here.
        let wants_text = self.tabs[i]
            .active_cmd
            .as_ref()
            .map(|c| {
                (c.wants_text_input() && !c.point_step_accepts_keywords())
                    || c.dyn_commit_as_text()
            })
            .unwrap_or(false);
        if wants_text {
            let text = self.tabs[i]
                .dyn_fields
                .iter()
                .find_map(|f| f.buffer.clone())
                .unwrap_or_default();
            let text = super::expr_eval::eval_to_string(text.trim());
            // Shared rule: a bare angle typed into a commit-as-text step takes
            // the sign of the cursor's side relative to the reference, so the
            // committed direction matches the drag (the box shows magnitude
            // only). Commands receive an already-signed string.
            let text = self.dyn_sign_angle_text(i, text);
            let result = self.tabs[i]
                .active_cmd
                .as_mut()
                .and_then(|c| c.on_text_input(&text));
            for f in self.tabs[i].dyn_fields.iter_mut() {
                f.buffer = None;
            }
            self.tabs[i].dyn_active = 0;
            self.sync_dyn_fields();
            let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
            if let Some(p) = prompt {
                self.command_line.push_info(&p);
            }
            self.refresh_active_cmd_preview(i);
            return Some(match result {
                Some(r) => self.apply_cmd_result(r),
                None => self.focus_cmd_input(),
            });
        }
        let pt = self.dyn_resolve_point()?;
        self.last_point = Some(pt);
        self.dyn_user_reshaped = false;
        self.sync_dyn_fields();
        self.reset_tracking_after_point();
        let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_point(pt));
        for f in self.tabs[i].dyn_fields.iter_mut() {
            f.buffer = None;
        }
        self.tabs[i].dyn_active = 0;
        let task = result.map(|r| self.apply_cmd_result(r))?;
        // Match the command-line path: refresh the rubber-band preview
        // so the next segment immediately starts from the new
        // last_point even though no mouse-move fires after a typed
        // coordinate. See #32.
        self.refresh_active_cmd_preview(i);
        Some(task)
    }

    /// Mutable access to the currently selected table style.
    fn tablestyle_mut(&mut self, tab: usize) -> Option<&mut acadrust::objects::TableStyle> {
        use acadrust::objects::ObjectType;
        let name = self.tablestyle_selected.clone();
        self.tabs[tab]
            .scene
            .document
            .objects
            .values_mut()
            .find_map(|o| match o {
                ObjectType::TableStyle(s) if s.name == name => Some(s),
                _ => None,
            })
    }

    /// Mutable access to a table style's cell style by row (0=Data,1=Header,2=Title).
    fn ts_cell_of(
        s: &mut acadrust::objects::TableStyle,
        row: u8,
    ) -> Option<&mut acadrust::objects::RowCellStyle> {
        match row {
            0 => Some(&mut s.data_row_style),
            1 => Some(&mut s.header_row_style),
            2 => Some(&mut s.title_row_style),
            _ => None,
        }
    }

    /// Mutable access to a cell's border by index
    /// (0=left 1=right 2=top 3=bottom 4=horizontal-inside 5=vertical-inside).
    fn ts_border_of(
        c: &mut acadrust::objects::RowCellStyle,
        border: u8,
    ) -> Option<&mut acadrust::objects::TableCellBorder> {
        match border {
            0 => Some(&mut c.left_border),
            1 => Some(&mut c.right_border),
            2 => Some(&mut c.top_border),
            3 => Some(&mut c.bottom_border),
            4 => Some(&mut c.horizontal_inside_border),
            5 => Some(&mut c.vertical_inside_border),
            _ => None,
        }
    }

    /// Populate margin + per-cell edit buffers from the selected table style.
    fn load_tablestyle_bufs(&mut self, tab: usize) {
        use acadrust::objects::ObjectType;
        let name = self.tablestyle_selected.clone();
        let Some(s) = self.tabs[tab]
            .scene
            .document
            .objects
            .values()
            .find_map(|o| match o {
                ObjectType::TableStyle(s) if s.name == name => Some(s),
                _ => None,
            })
        else {
            return;
        };
        self.ts_hmargin = format!("{:.4}", s.horizontal_margin);
        self.ts_vmargin = format!("{:.4}", s.vertical_margin);
        self.ts_description = s.description.clone();
        for (r, c) in [&s.data_row_style, &s.header_row_style, &s.title_row_style]
            .into_iter()
            .enumerate()
        {
            self.ts_cell_textstyle[r] = c.text_style_name.clone();
            self.ts_cell_height[r] = format!("{:.4}", c.text_height);
            self.ts_cell_textcolor[r] = c
                .text_color
                .index()
                .map(|v| v.to_string())
                .unwrap_or_default();
            self.ts_cell_fillcolor[r] = c
                .fill_color
                .index()
                .map(|v| v.to_string())
                .unwrap_or_default();
            self.ts_cell_datatype[r] = c.data_type.to_string();
            self.ts_cell_unittype[r] = c.unit_type.to_string();
            self.ts_cell_format[r] = c.format_string.clone();
            let borders = [
                &c.left_border,
                &c.right_border,
                &c.top_border,
                &c.bottom_border,
                &c.horizontal_inside_border,
                &c.vertical_inside_border,
            ];
            for (b, bd) in borders.into_iter().enumerate() {
                self.ts_border_lw[r][b] = bd.line_weight.value().to_string();
                self.ts_border_color[r][b] =
                    bd.color.index().map(|v| v.to_string()).unwrap_or_default();
                self.ts_border_spacing[r][b] = format!("{:.4}", bd.double_line_spacing);
            }
        }
    }

    /// Mutable access to the currently selected multileader style.
    fn mleaderstyle_mut(&mut self, tab: usize) -> Option<&mut acadrust::objects::MultiLeaderStyle> {
        use acadrust::objects::ObjectType;
        let name = self.mleaderstyle_selected.clone();
        self.tabs[tab]
            .scene
            .document
            .objects
            .values_mut()
            .find_map(|o| match o {
                ObjectType::MultiLeaderStyle(s) if s.name == name => Some(s),
                _ => None,
            })
    }

    /// Populate all edit buffers from the currently selected multileader style.
    pub(super) fn load_mleaderstyle_bufs(&mut self, tab: usize) {
        use acadrust::objects::ObjectType;
        let name = self.mleaderstyle_selected.clone();
        let Some(s) = self.tabs[tab]
            .scene
            .document
            .objects
            .values()
            .find_map(|o| match o {
                ObjectType::MultiLeaderStyle(s) if s.name == name => Some(s),
                _ => None,
            })
        else {
            return;
        };
        self.mls_landing_distance = format!("{:.4}", s.landing_distance);
        self.mls_landing_gap = format!("{:.4}", s.landing_gap);
        self.mls_arrowhead_size = format!("{:.4}", s.arrowhead_size);
        self.mls_text_height = format!("{:.4}", s.text_height);
        self.mls_scale_factor = format!("{:.4}", s.scale_factor);
        self.mls_break_gap = format!("{:.4}", s.break_gap_size);
        self.mls_first_seg_angle = format!("{:.4}", s.first_segment_angle);
        self.mls_second_seg_angle = format!("{:.4}", s.second_segment_angle);
        self.mls_max_points = s.max_leader_points.to_string();
        self.mls_default_text = s.default_text.clone();
        self.mls_line_color = s
            .line_color
            .index()
            .map(|c| c.to_string())
            .unwrap_or_default();
        self.mls_text_color = s
            .text_color
            .index()
            .map(|c| c.to_string())
            .unwrap_or_default();
        self.mls_description = s.description.clone();
        self.mls_line_weight = s.line_weight.value().to_string();
        self.mls_align_space = format!("{:.4}", s.align_space);
        self.mls_block_color = s
            .block_content_color
            .index()
            .map(|c| c.to_string())
            .unwrap_or_default();
        self.mls_block_rotation = format!("{:.4}", s.block_content_rotation);
        self.mls_block_scale_x = format!("{:.4}", s.block_content_scale_x);
        self.mls_block_scale_y = format!("{:.4}", s.block_content_scale_y);
        self.mls_block_scale_z = format!("{:.4}", s.block_content_scale_z);
    }

    /// Populate all edit buffers from the currently selected dim style.
    pub(super) fn load_dimstyle_bufs(&mut self, tab: usize) {
        let doc = &self.tabs[tab].scene.document;
        let Some(ds) = doc.dim_styles.get(&self.dimstyle_selected) else {
            return;
        };
        self.ds_dimdle = format!("{}", ds.dimdle);
        self.ds_dimdli = format!("{}", ds.dimdli);
        self.ds_dimgap = format!("{}", ds.dimgap);
        self.ds_dimexe = format!("{}", ds.dimexe);
        self.ds_dimexo = format!("{}", ds.dimexo);
        self.ds_dimsd1 = ds.dimsd1;
        self.ds_dimsd2 = ds.dimsd2;
        self.ds_dimse1 = ds.dimse1;
        self.ds_dimse2 = ds.dimse2;
        self.ds_dimasz = format!("{}", ds.dimasz);
        self.ds_dimcen = format!("{}", ds.dimcen);
        self.ds_dimtsz = format!("{}", ds.dimtsz);
        self.ds_dimtxt = format!("{}", ds.dimtxt);
        self.ds_dimtxsty = ds.dimtxsty.clone();
        self.ds_dimtad = format!("{}", ds.dimtad);
        self.ds_dimtih = ds.dimtih;
        self.ds_dimtoh = ds.dimtoh;
        self.ds_dimscale = format!("{}", ds.dimscale);
        self.ds_dimlfac = format!("{}", ds.dimlfac);
        self.ds_dimlunit = format!("{}", ds.dimlunit);
        self.ds_dimdec = format!("{}", ds.dimdec);
        self.ds_dimpost = ds.dimpost.clone();
        self.ds_dimtol = ds.dimtol;
        self.ds_dimlim = ds.dimlim;
        self.ds_dimtp = format!("{}", ds.dimtp);
        self.ds_dimtm = format!("{}", ds.dimtm);
        self.ds_dimtdec = format!("{}", ds.dimtdec);
        self.ds_dimtfac = format!("{}", ds.dimtfac);
        self.ds_annotative = ds.annotative;
        self.ds_dimclrd = format!("{}", ds.dimclrd);
        self.ds_dimlwd = format!("{}", ds.dimlwd);
        self.ds_dimclre = format!("{}", ds.dimclre);
        self.ds_dimlwe = format!("{}", ds.dimlwe);
        self.ds_dimfxl = format!("{}", ds.dimfxl);
        self.ds_dimfxlon = ds.dimfxlon;
        self.ds_dimsah = ds.dimsah;
        self.ds_dimarcsym = format!("{}", ds.dimarcsym);
        self.ds_dimjogang = format!("{}", ds.dimjogang.to_degrees());
        self.ds_dimclrt = format!("{}", ds.dimclrt);
        self.ds_dimjust = format!("{}", ds.dimjust);
        self.ds_dimtvp = format!("{}", ds.dimtvp);
        self.ds_dimtfill = format!("{}", ds.dimtfill);
        self.ds_dimtfillclr = format!("{}", ds.dimtfillclr);
        self.ds_dimtxtdirection = ds.dimtxtdirection;
        self.ds_dimatfit = format!("{}", ds.dimatfit);
        self.ds_dimtix = ds.dimtix;
        self.ds_dimsoxd = ds.dimsoxd;
        self.ds_dimtmove = format!("{}", ds.dimtmove);
        self.ds_dimupt = ds.dimupt;
        self.ds_dimtofl = ds.dimtofl;
        self.ds_dimfit = format!("{}", ds.dimfit);
        self.ds_dimdsep = format!("{}", ds.dimdsep);
        self.ds_dimrnd = format!("{}", ds.dimrnd);
        self.ds_dimzin = format!("{}", ds.dimzin);
        self.ds_dimfrac = format!("{}", ds.dimfrac);
        self.ds_dimaunit = format!("{}", ds.dimaunit);
        self.ds_dimadec = format!("{}", ds.dimadec);
        self.ds_dimunit = format!("{}", ds.dimunit);
        self.ds_dimazin = format!("{}", ds.dimazin);
        self.ds_dimalt = ds.dimalt;
        self.ds_dimaltf = format!("{}", ds.dimaltf);
        self.ds_dimaltd = format!("{}", ds.dimaltd);
        self.ds_dimaltu = format!("{}", ds.dimaltu);
        self.ds_dimalttd = format!("{}", ds.dimalttd);
        self.ds_dimaltrnd = format!("{}", ds.dimaltrnd);
        self.ds_dimapost = ds.dimapost.clone();
        self.ds_dimaltz = format!("{}", ds.dimaltz);
        self.ds_dimalttz = format!("{}", ds.dimalttz);
        self.ds_dimtolj = format!("{}", ds.dimtolj);
        self.ds_dimtzin = format!("{}", ds.dimtzin);
    }

    /// Write edit buffers back into the selected dim style document entry.
    fn apply_dimstyle_bufs(&mut self, tab: usize) {
        let doc = &mut self.tabs[tab].scene.document;
        let Some(ds) = doc.dim_styles.get_mut(&self.dimstyle_selected) else {
            return;
        };
        macro_rules! set_f64 {
            ($field:ident, $buf:expr) => {
                if let Ok(v) = $buf.trim().parse::<f64>() {
                    ds.$field = v;
                }
            };
        }
        macro_rules! set_i16 {
            ($field:ident, $buf:expr) => {
                if let Ok(v) = $buf.trim().parse::<i16>() {
                    ds.$field = v;
                }
            };
        }
        set_f64!(dimdle, self.ds_dimdle);
        set_f64!(dimdli, self.ds_dimdli);
        set_f64!(dimgap, self.ds_dimgap);
        set_f64!(dimexe, self.ds_dimexe);
        set_f64!(dimexo, self.ds_dimexo);
        set_f64!(dimasz, self.ds_dimasz);
        set_f64!(dimcen, self.ds_dimcen);
        set_f64!(dimtsz, self.ds_dimtsz);
        set_f64!(dimtxt, self.ds_dimtxt);
        set_f64!(dimscale, self.ds_dimscale);
        set_f64!(dimlfac, self.ds_dimlfac);
        set_f64!(dimtp, self.ds_dimtp);
        set_f64!(dimtm, self.ds_dimtm);
        set_f64!(dimtfac, self.ds_dimtfac);
        set_i16!(dimtad, self.ds_dimtad);
        set_i16!(dimlunit, self.ds_dimlunit);
        set_i16!(dimdec, self.ds_dimdec);
        set_i16!(dimtdec, self.ds_dimtdec);
        ds.dimsd1 = self.ds_dimsd1;
        ds.dimsd2 = self.ds_dimsd2;
        ds.dimse1 = self.ds_dimse1;
        ds.dimse2 = self.ds_dimse2;
        ds.dimtih = self.ds_dimtih;
        ds.dimtoh = self.ds_dimtoh;
        ds.dimtol = self.ds_dimtol;
        ds.dimlim = self.ds_dimlim;
        ds.dimpost = self.ds_dimpost.clone();
        ds.dimtxsty = self.ds_dimtxsty.clone();
        ds.annotative = self.ds_annotative;
        set_i16!(dimclrd, self.ds_dimclrd);
        set_i16!(dimlwd, self.ds_dimlwd);
        set_i16!(dimclre, self.ds_dimclre);
        set_i16!(dimlwe, self.ds_dimlwe);
        set_f64!(dimfxl, self.ds_dimfxl);
        set_i16!(dimarcsym, self.ds_dimarcsym);
        set_i16!(dimclrt, self.ds_dimclrt);
        set_i16!(dimjust, self.ds_dimjust);
        set_f64!(dimtvp, self.ds_dimtvp);
        set_i16!(dimtfill, self.ds_dimtfill);
        set_i16!(dimtfillclr, self.ds_dimtfillclr);
        set_i16!(dimatfit, self.ds_dimatfit);
        set_i16!(dimtmove, self.ds_dimtmove);
        set_i16!(dimfit, self.ds_dimfit);
        set_i16!(dimdsep, self.ds_dimdsep);
        set_f64!(dimrnd, self.ds_dimrnd);
        set_i16!(dimzin, self.ds_dimzin);
        set_i16!(dimfrac, self.ds_dimfrac);
        set_i16!(dimaunit, self.ds_dimaunit);
        set_i16!(dimadec, self.ds_dimadec);
        set_i16!(dimunit, self.ds_dimunit);
        set_i16!(dimazin, self.ds_dimazin);
        set_f64!(dimaltf, self.ds_dimaltf);
        set_i16!(dimaltd, self.ds_dimaltd);
        set_i16!(dimaltu, self.ds_dimaltu);
        set_i16!(dimalttd, self.ds_dimalttd);
        set_f64!(dimaltrnd, self.ds_dimaltrnd);
        set_i16!(dimaltz, self.ds_dimaltz);
        set_i16!(dimalttz, self.ds_dimalttz);
        set_i16!(dimtolj, self.ds_dimtolj);
        set_i16!(dimtzin, self.ds_dimtzin);
        if let Ok(v) = self.ds_dimjogang.trim().parse::<f64>() {
            ds.dimjogang = v.to_radians();
        }
        ds.dimfxlon = self.ds_dimfxlon;
        ds.dimsah = self.ds_dimsah;
        ds.dimtxtdirection = self.ds_dimtxtdirection;
        ds.dimtix = self.ds_dimtix;
        ds.dimsoxd = self.ds_dimsoxd;
        ds.dimupt = self.ds_dimupt;
        ds.dimtofl = self.ds_dimtofl;
        ds.dimalt = self.ds_dimalt;
        ds.dimapost = self.ds_dimapost.clone();
        self.command_line
            .push_output(&format!("DimStyle '{}' updated.", self.dimstyle_selected));
    }

    /// Update a single string buffer field.
    fn apply_ds_edit(&mut self, field: super::DsField, val: String) {
        use super::DsField::*;
        match field {
            Dimdle => self.ds_dimdle = val,
            Dimdli => self.ds_dimdli = val,
            Dimgap => self.ds_dimgap = val,
            Dimexe => self.ds_dimexe = val,
            Dimexo => self.ds_dimexo = val,
            Dimasz => self.ds_dimasz = val,
            Dimcen => self.ds_dimcen = val,
            Dimtsz => self.ds_dimtsz = val,
            Dimtxt => self.ds_dimtxt = val,
            Dimtxsty => self.ds_dimtxsty = val,
            Dimtad => self.ds_dimtad = val,
            Dimscale => self.ds_dimscale = val,
            Dimlfac => self.ds_dimlfac = val,
            Dimlunit => self.ds_dimlunit = val,
            Dimdec => self.ds_dimdec = val,
            Dimpost => self.ds_dimpost = val,
            Dimtp => self.ds_dimtp = val,
            Dimtm => self.ds_dimtm = val,
            Dimtdec => self.ds_dimtdec = val,
            Dimtfac => self.ds_dimtfac = val,
            Dimclrd => self.ds_dimclrd = val,
            Dimlwd => self.ds_dimlwd = val,
            Dimclre => self.ds_dimclre = val,
            Dimlwe => self.ds_dimlwe = val,
            Dimfxl => self.ds_dimfxl = val,
            Dimarcsym => self.ds_dimarcsym = val,
            Dimjogang => self.ds_dimjogang = val,
            Dimclrt => self.ds_dimclrt = val,
            Dimjust => self.ds_dimjust = val,
            Dimtvp => self.ds_dimtvp = val,
            Dimtfill => self.ds_dimtfill = val,
            Dimtfillclr => self.ds_dimtfillclr = val,
            Dimatfit => self.ds_dimatfit = val,
            Dimtmove => self.ds_dimtmove = val,
            Dimfit => self.ds_dimfit = val,
            Dimdsep => self.ds_dimdsep = val,
            Dimrnd => self.ds_dimrnd = val,
            Dimzin => self.ds_dimzin = val,
            Dimfrac => self.ds_dimfrac = val,
            Dimaunit => self.ds_dimaunit = val,
            Dimadec => self.ds_dimadec = val,
            Dimunit => self.ds_dimunit = val,
            Dimazin => self.ds_dimazin = val,
            Dimaltf => self.ds_dimaltf = val,
            Dimaltd => self.ds_dimaltd = val,
            Dimaltu => self.ds_dimaltu = val,
            Dimalttd => self.ds_dimalttd = val,
            Dimaltrnd => self.ds_dimaltrnd = val,
            Dimapost => self.ds_dimapost = val,
            Dimaltz => self.ds_dimaltz = val,
            Dimalttz => self.ds_dimalttz = val,
            Dimtolj => self.ds_dimtolj = val,
            Dimtzin => self.ds_dimtzin = val,
            // Bool fields — no-op for string edit
            _ => {}
        }
    }

    /// Toggle a boolean buffer field.
    fn apply_ds_toggle(&mut self, field: super::DsField) {
        use super::DsField::*;
        match field {
            Dimsd1 => self.ds_dimsd1 = !self.ds_dimsd1,
            Dimsd2 => self.ds_dimsd2 = !self.ds_dimsd2,
            Dimse1 => self.ds_dimse1 = !self.ds_dimse1,
            Dimse2 => self.ds_dimse2 = !self.ds_dimse2,
            Dimtih => self.ds_dimtih = !self.ds_dimtih,
            Dimtoh => self.ds_dimtoh = !self.ds_dimtoh,
            Dimtol => self.ds_dimtol = !self.ds_dimtol,
            Dimlim => self.ds_dimlim = !self.ds_dimlim,
            Annotative => self.ds_annotative = !self.ds_annotative,
            Dimfxlon => self.ds_dimfxlon = !self.ds_dimfxlon,
            Dimsah => self.ds_dimsah = !self.ds_dimsah,
            Dimtxtdirection => self.ds_dimtxtdirection = !self.ds_dimtxtdirection,
            Dimtix => self.ds_dimtix = !self.ds_dimtix,
            Dimsoxd => self.ds_dimsoxd = !self.ds_dimsoxd,
            Dimupt => self.ds_dimupt = !self.ds_dimupt,
            Dimtofl => self.ds_dimtofl = !self.ds_dimtofl,
            Dimalt => self.ds_dimalt = !self.ds_dimalt,
            _ => {}
        }
    }

    /// Populate edit buffers from the currently selected text style.
    fn open_save_dialog_window(&mut self, tab_idx: usize) -> Task<Message> {
        // Pre-fill filename and folder from current path or defaults.
        if let Some(p) = &self.tabs[tab_idx].current_path.clone() {
            if let Some(name) = p.file_name() {
                self.save_dialog_filename = name.to_string_lossy().into_owned();
            }
            if let Some(dir) = p.parent() {
                self.save_dialog_folder = dir.to_path_buf();
            }
        } else {
            let (ext, _) = crate::io::parse_save_format(&self.save_dialog_format);
            self.save_dialog_filename = format!("{}.{ext}", self.tabs[tab_idx].tab_display_name());
        }
        self.save_dialog_entries = crate::io::read_dir_entries(&self.save_dialog_folder.clone());
        self.active_modal = Some(super::ModalKind::SaveDialog);
        Task::none()
    }

    fn close_save_dialog_window(&mut self) -> Task<Message> {
        if self.active_modal == Some(super::ModalKind::SaveDialog) {
            self.active_modal = None;
        }
        Task::none()
    }

    fn open_unsaved_dialog_window(&mut self) -> Task<Message> {
        self.active_modal = Some(super::ModalKind::Unsaved);
        Task::none()
    }

    fn close_unsaved_dialog_window(&mut self) -> Task<Message> {
        if self.active_modal == Some(super::ModalKind::Unsaved) {
            self.active_modal = None;
        }
        Task::none()
    }

    pub(super) fn load_textstyle_bufs(&mut self, tab: usize) {
        let doc = &self.tabs[tab].scene.document;
        if let Some(s) = doc.text_styles.get(&self.textstyle_selected) {
            self.textstyle_font = s.font_file.clone();
            self.textstyle_width = format!("{:.4}", s.width_factor);
            self.textstyle_oblique = format!("{:.2}", s.oblique_angle.to_degrees());
            self.textstyle_height = format!("{:.4}", s.height);
            self.textstyle_bigfont = s.big_font_file.clone();
            self.textstyle_ttf = s.true_type_font.clone();
        }
    }
}

/// Parse a scale string like "1:50" or "2:1" into (numerator, denominator).
/// Returns (1.0, 1.0) for "Fit" or unknown formats.
/// Sync the model-space annotation scale into the standard CANNOSCALE /
/// CANNOSCALEVALUE header variables before a save, so the scale round-trips
/// through the file (and is read correctly by other CAD applications).
fn sync_annotation_scale_header(scene: &mut Scene) {
    let anno = scene.annotation_scale;
    let value = if anno.abs() > 1e-9 {
        1.0 / anno as f64
    } else {
        1.0
    };
    // Prefer the name of a matching scale already in the drawing's list;
    // fall back to a formatted ratio when none matches.
    let name = scene
        .scale_list()
        .into_iter()
        .find(|(_, a, _)| (a - anno).abs() < 0.001 * anno.max(0.001))
        .map(|(n, _, _)| n)
        .unwrap_or_else(|| format_annotation_scale_name(anno));
    let hdr = &mut scene.document.header;
    hdr.current_annotation_scale = name;
    hdr.annotation_scale_value = value;
}

/// Format an annotation-scale multiplier as a ratio name: 50.0 -> "1:50",
/// 0.5 -> "2:1", 1.0 -> "1:1".
fn format_annotation_scale_name(anno: f32) -> String {
    if anno >= 1.0 {
        format!("1:{}", anno.round() as i64)
    } else if anno > 0.0 {
        format!("{}:1", (1.0 / anno).round() as i64)
    } else {
        "1:1".to_string()
    }
}

fn parse_plot_scale(s: &str) -> (f64, f64) {
    if s == "Fit" {
        return (1.0, 1.0);
    }
    if let Some((a, b)) = s.split_once(':') {
        let num: f64 = a.trim().parse().unwrap_or(1.0);
        let den: f64 = b.trim().parse().unwrap_or(1.0);
        if den > 0.0 {
            return (num, den);
        }
    }
    (1.0, 1.0)
}
