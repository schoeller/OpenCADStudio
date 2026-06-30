//! `dialog` arms and helpers, split out of the original `update.rs` (#mechanical decomposition).

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
    pub(in crate::app) fn open_save_dialog_window(&mut self, tab_idx: usize) -> Task<Message> {
        // Default the format dropdown to the latest version (DWG/DXF 2018),
        // regardless of the loaded file's version. The user can pick any older
        // version explicitly; the selected version is written as-is.
        let is_dxf = self.tabs[tab_idx]
            .current_path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("dxf"))
            .unwrap_or(false);
        self.save_dialog_format =
            crate::io::format_for_version(acadrust::DxfVersion::AC1032, is_dxf);

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
        self.aec_drop_acknowledged = false;
        self.overwrite_acknowledged = false;
        self.active_modal = Some(crate::app::ModalKind::SaveDialog);
        Task::none()
    }


    pub(in crate::app) fn close_save_dialog_window(&mut self) -> Task<Message> {
        self.aec_drop_acknowledged = false;
        self.overwrite_acknowledged = false;
        if self.active_modal == Some(crate::app::ModalKind::SaveDialog) {
            self.active_modal = None;
        }
        Task::none()
    }


    pub(in crate::app) fn open_unsaved_dialog_window(&mut self) -> Task<Message> {
        self.active_modal = Some(crate::app::ModalKind::Unsaved);
        // The unsaved-changes prompt renders inside the main window, so bring
        // that window to the foreground — a close signal can arrive while the
        // app is backgrounded, leaving the prompt unseen behind other windows.
        // `gain_focus` alone is ignored by most Linux WMs (focus-stealing
        // prevention), so pair it with an urgency hint so the window is at
        // least flagged for attention when the compositor blocks the raise.
        match self.main_window {
            Some(id) => Task::batch([
                iced::window::gain_focus(id),
                iced::window::request_user_attention(
                    id,
                    Some(iced::window::UserAttention::Critical),
                ),
            ]),
            None => Task::none(),
        }
    }


    pub(in crate::app) fn close_unsaved_dialog_window(&mut self) -> Task<Message> {
        if self.active_modal == Some(crate::app::ModalKind::Unsaved) {
            self.active_modal = None;
        }
        Task::none()
    }


pub(super) fn on_ribbon_tool_click(&mut self, tool_id: String, event: ModuleEvent) -> Task<Message> {
                // Ribbon tools should not activate while a modal dialog has the
                // focus. The modal's own dispatch path will close itself first.
                if self.active_modal.is_some() {
                    return Task::none();
                }
                match event {
                    ModuleEvent::Command(cmd) => {
                        let task = self.dispatch_command(&cmd);
                        // Only highlight the tool when the command was actually
                        // accepted and installed as the active command.
                        if self.tabs[self.active_tab].active_cmd.is_some() {
                            self.ribbon.activate_tool(&tool_id);
                        } else {
                            self.ribbon.deactivate_tool_if(&tool_id);
                        }
                        return task;
                    }                    ModuleEvent::OpenFileDialog => {
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

    pub(super) fn on_unsaved_dialog_discard(&mut self) -> Task<Message> {
                match self.pending_close.take() {
                    Some(crate::app::PendingClose::Tab(idx)) => {
                        let close_win = self.close_unsaved_dialog_window();
                        // Discarded — drop this tab's autosave recovery copy.
                        #[cfg(not(target_arch = "wasm32"))]
                        let _ = std::fs::remove_file(self.autosave_target(idx));
                        if self.tabs.len() == 1 {
                            self.tab_counter += 1;
                            self.tabs[0] =
                                crate::app::document::DocumentTab::new_drawing(self.tab_counter);
                            self.active_tab = 0;
                            self.apply_bg_default(0);
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
                    Some(crate::app::PendingClose::Quit) => {
                        if let Some(idx) = self.tabs.iter().position(|t| t.dirty) {
                            self.tabs[idx].dirty = false;
                        }
                        if self.tabs.iter().any(|t| t.dirty) {
                            // More dirty tabs remain — keep window open.
                            self.pending_close = Some(crate::app::PendingClose::Quit);
                        } else {
                            let close_win = self.close_unsaved_dialog_window();
                            return Task::batch(vec![close_win, self.exit_app()]);
                        }
                    }
                    None => {}
                }
                Task::none()
    }

    pub(super) fn on_unsaved_dialog_save(&mut self) -> Task<Message> {
                match self.pending_close.take() {
                    Some(crate::app::PendingClose::Tab(idx)) => {
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
                                    self.pending_close = Some(crate::app::PendingClose::Tab(idx));
                                }
                            }
                        } else {
                            // No path — close unsaved dialog, open custom Save As dialog.
                            self.pending_close = Some(crate::app::PendingClose::Tab(idx));
                            self.save_dialog_for_unsaved = true;
                            let close_win = self.close_unsaved_dialog_window();
                            let open_save = self.open_save_dialog_window(idx);
                            return Task::batch([close_win, open_save]);
                        }
                    }
                    Some(crate::app::PendingClose::Quit) => {
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
                                        self.pending_close = Some(crate::app::PendingClose::Quit);
                                        return Task::none();
                                    }
                                }
                            } else {
                                // No path — close unsaved dialog, open custom Save As dialog.
                                self.active_tab = idx;
                                self.pending_close = Some(crate::app::PendingClose::Quit);
                                self.save_dialog_for_unsaved = true;
                                let close_win = self.close_unsaved_dialog_window();
                                let open_save = self.open_save_dialog_window(idx);
                                return Task::batch([close_win, open_save]);
                            }
                        }
                        if self.tabs.iter().any(|t| t.dirty) {
                            // More dirty tabs — keep window open.
                            self.pending_close = Some(crate::app::PendingClose::Quit);
                        } else {
                            let close_win = self.close_unsaved_dialog_window();
                            return Task::batch(vec![close_win, self.exit_app()]);
                        }
                    }
                    None => {}
                }
                Task::none()
    }

    pub(super) fn on_unsaved_picked_save_path_some(&mut self, path: std::path::PathBuf) -> Task<Message> {
                let (_ext, version) = crate::io::parse_save_format(&self.save_dialog_format);
                match self.pending_close.take() {
                    Some(crate::app::PendingClose::Tab(idx)) => {
                        match crate::io::save_as_version(
                            &self.tabs[idx].scene.document,
                            &path,
                            version,
                        ) {
                            Ok(()) => {
                                self.command_line
                                    .push_output(&format!("Saved: {}", path.display()));
                                #[cfg(not(target_arch = "wasm32"))]
                                let _ = std::fs::remove_file(self.autosave_target(idx));
                                self.tabs[idx].current_path = Some(path);
                                self.tabs[idx].dirty = false;
                                return self.update(Message::TabClose(idx));
                            }
                            Err(e) => {
                                self.command_line.push_error(&format!("Save failed: {e}"));
                                self.pending_close = Some(crate::app::PendingClose::Tab(idx));
                                return self.open_unsaved_dialog_window();
                            }
                        }
                    }
                    Some(crate::app::PendingClose::Quit) => {
                        let i = self.active_tab;
                        match crate::io::save_as_version(
                            &self.tabs[i].scene.document,
                            &path,
                            version,
                        ) {
                            Ok(()) => {
                                self.command_line
                                    .push_output(&format!("Saved: {}", path.display()));
                                #[cfg(not(target_arch = "wasm32"))]
                                let _ = std::fs::remove_file(self.autosave_target(i));
                                self.tabs[i].current_path = Some(path);
                                self.tabs[i].dirty = false;
                                if self.tabs.iter().any(|t| t.dirty) {
                                    self.pending_close = Some(crate::app::PendingClose::Quit);
                                    return self.open_unsaved_dialog_window();
                                } else {
                                    return self.exit_app();
                                }
                            }
                            Err(e) => {
                                self.command_line.push_error(&format!("Save failed: {e}"));
                                self.pending_close = Some(crate::app::PendingClose::Quit);
                                return self.open_unsaved_dialog_window();
                            }
                        }
                    }
                    None => {}
                }
                Task::none()
    }
}
