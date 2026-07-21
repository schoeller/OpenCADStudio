//! Phase-2 external plugin discovery.
//!
//! Scans the per-user plugins directory for installed add-on packages and
//! reads their `plugin.toml` so the host can list them and gate them on the
//! API version — *before* any native code is loaded. Actually loading the
//! `cdylib` is a separate step; this module only inspects what is on disk.
//!
//! Layout (mirrors the spec in `docs/plugin-architecture.md`):
//! ```text
//! <config>/OpenCADStudio/plugins/
//!   <plugin-id>/
//!     plugin.toml
//!     <lib<name>.so | .dll | .dylib>
//! ```

use std::path::PathBuf;

/// One entry in the curated plugin registry (`plugins/registry.json`).
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub repo: String,
    pub name: String,
    pub description: String,
}

/// An add-on package found on disk (not necessarily loaded or compatible).
#[derive(Debug, Clone)]
pub struct ExternalPlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub api_version: u32,
    pub ribbon_order: i32,
    pub command_prefixes: Vec<String>,
    /// The package directory under the plugins folder.
    pub dir: PathBuf,
    /// Whether a native library for this platform sits beside `plugin.toml`.
    pub lib_present: bool,
}

impl ExternalPlugin {
    /// True when the package's API version is supported by this host.
    pub fn api_compatible(&self) -> bool {
        ocs_plugin_api::host_accepts_plugin_version(self.api_version)
    }

    /// True when the package can be loaded today: compatible API *and* a native
    /// library present for this platform.
    #[allow(dead_code)] // plugin-host surface (issue #100); not yet wired
    pub fn loadable(&self) -> bool {
        self.api_compatible() && self.lib_present
    }
}

/// `<config>/OpenCADStudio/plugins`, matching the settings/recent-files store.
/// Overridable via `OCS_PLUGINS_DIR` for tests.
pub fn plugins_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OCS_PLUGINS_DIR") {
        return Some(PathBuf::from(p));
    }
    let base: PathBuf = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)?
    } else if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")?;
        let mut p = PathBuf::from(home);
        p.push("Library");
        p.push("Application Support");
        p
    } else if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(d)
    } else {
        let home = std::env::var_os("HOME")?;
        let mut p = PathBuf::from(home);
        p.push(".config");
        p
    };
    let mut p = base;
    p.push("OpenCADStudio");
    p.push("plugins");
    Some(p)
}

/// Delete an installed package's folder. It stays loaded for the current
/// session (the library is resident); the removal takes effect on next start.
#[cfg(not(target_arch = "wasm32"))]
pub fn uninstall(id: &str) -> Result<(), String> {
    let dir = plugins_dir()
        .ok_or("cannot locate the plugins folder")?
        .join(id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Native dynamic-library extension for the current platform (no dot).
fn lib_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// Discover every package under the plugins directory, sorted by `ribbon_order`
/// then id. Missing directory → empty list (not an error).
pub fn discover() -> Vec<ExternalPlugin> {
    let Some(root) = plugins_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let toml_path = dir.join("plugin.toml");
        let Ok(text) = std::fs::read_to_string(&toml_path) else {
            continue;
        };
        if let Some(mut p) = parse_plugin_toml(&text) {
            p.lib_present = lib_present_in(&dir);
            p.dir = dir;
            found.push(p);
        }
    }
    found.sort_by(|a, b| a.ribbon_order.cmp(&b.ribbon_order).then(a.id.cmp(&b.id)));
    found
}

/// True when a file with this platform's dynamic-library extension exists in
/// `dir` (any name — the package owns its lib naming).
fn lib_present_in(dir: &std::path::Path) -> bool {
    let ext = lib_extension();
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some(ext))
        })
        .unwrap_or(false)
}

/// Minimal `plugin.toml` reader for the documented `[plugin]` / `[opencad]`
/// keys. Deliberately small (string / integer / string-array values) so the
/// host doesn't pull in a full TOML parser for a fixed, host-defined schema.
/// Returns `None` when the required `id` is missing. `dir` / `lib_present` are
/// filled in by the caller.
pub(crate) fn parse_plugin_toml(text: &str) -> Option<ExternalPlugin> {
    let mut id = None;
    let mut name = String::new();
    let mut version = String::new();
    let mut description = String::new();
    let mut api_version: u32 = 0;
    let mut ribbon_order: i32 = 0;
    let mut command_prefixes: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "id" => id = Some(unquote(value)),
            "name" => name = unquote(value),
            "version" => version = unquote(value),
            "description" => description = unquote(value),
            "api_version" => api_version = value.parse().unwrap_or(0),
            "ribbon_order" => ribbon_order = value.parse().unwrap_or(0),
            "command_prefixes" => command_prefixes = parse_string_array(value),
            _ => {}
        }
    }

    Some(ExternalPlugin {
        id: id?,
        name,
        version,
        description,
        api_version,
        ribbon_order,
        command_prefixes,
        dir: PathBuf::new(),
        lib_present: false,
    })
}

/// Strip surrounding single or double quotes from a TOML scalar.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse `["a", "b"]` into `["a", "b"]`. Tolerant of spacing and missing
/// brackets; ignores empty entries.
fn parse_string_array(s: &str) -> Vec<String> {
    s.trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(unquote)
        .filter(|e| !e.is_empty())
        .collect()
}

// ── Runtime loading (desktop only) ──────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
    pub(crate) use loader::{shutdown_plugins, with_manager};

#[cfg(all(not(target_arch = "wasm32"), not(test)))]
pub(crate) use loader::{load_at_startup, loaded_ids};

#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(test, allow(dead_code))]
mod loader {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use ocs_plugin_api::ipc::protocol::HostAsync;
    use ocs_plugin_api::panel::{PanelDef, PanelError, PanelEvent, PanelHandle};
    use ocs_plugin_api::process::PluginManager;

    fn panel_log(_msg: &str) {
        // Disabled in release builds to avoid synchronous file I/O on the hot
        // panel-event path. Re-enable locally when debugging async IPC.
    }

    use super::lib_extension;
    use crate::plugin::panels::{DocumentEvent, PanelManager};

    /// Host-side wrapper around the crate-level plugin manager that also owns
    /// the panel manager for API v3 panels.
    pub struct HostPluginManager {
        manager: PluginManager,
        /// `RefCell` allows `HostApi` callbacks dispatched from inside
        /// `with_manager` (which holds an immutable borrow of the manager) to
        /// still mutate the panel state (e.g. `open_panel`).
        panels: RefCell<PanelManager>,
    }

    impl HostPluginManager {
        pub fn new() -> Self {
            Self {
                manager: PluginManager::new(),
                panels: RefCell::new(PanelManager::new()),
            }
        }

        /// Returns whether any plugin panel is currently open.
        pub fn has_open_panels(&self) -> bool {
            self.panels.borrow().has_panels()
        }

        /// Returns whether `cursor` lies inside any open plugin panel's bounds.
        pub fn cursor_over_panel(&self, cursor: iced::Point) -> bool {
            self.panels.borrow().cursor_over_panel(cursor)
        }

        /// Returns the bounding rectangles of all open panels in window coordinates.
        pub fn panel_rects(&self) -> Vec<iced::Rectangle> {
            self.panels.borrow().panel_rects()
        }

        /// Open (or refresh) a panel for `def` owned by `process`.
        pub fn open_panel(
            &self,
            process: Arc<ocs_plugin_api::process::PluginProcess>,
            def: &PanelDef,
        ) -> Result<PanelHandle, PanelError> {
            self.panels.borrow_mut().open(process, def)
        }

        /// Update the logical window size used for clamping and edge snapping.
        pub fn set_window_size(&self, width: f32, height: f32) {
            self.panels.borrow_mut().set_window_size(width, height);
        }

        /// Close an open panel.
        pub fn close_panel(&self, handle: PanelHandle) -> Result<(), PanelError> {
            self.panels.borrow_mut().close(handle)
        }

        /// Move an open panel to logical window coordinates.
        pub fn move_panel(&self, handle: PanelHandle, x: f32, y: f32) -> Result<(), PanelError> {
            self.panels.borrow_mut().move_panel(handle, x, y)
        }

        /// Resize an open panel. Values are clamped to the panel's minimum size.
        pub fn resize_panel(
            &self,
            handle: PanelHandle,
            width: f32,
            height: f32,
        ) -> Result<(), PanelError> {
            self.panels.borrow_mut().resize_panel(handle, width, height)
        }

        /// Dock an open panel to `zone`.
        pub fn dock_panel(
            &self,
            handle: PanelHandle,
            zone: ocs_plugin_api::panel::DockZone,
        ) -> Result<(), PanelError> {
            self.panels.borrow_mut().dock_panel(handle, zone)
        }

        /// Undock an open panel and place it at logical window coordinates.
        pub fn undock_panel(
            &self,
            handle: PanelHandle,
            x: f32,
            y: f32,
        ) -> Result<(), PanelError> {
            self.panels.borrow_mut().undock_panel(handle, x, y)
        }

        /// Update the widgets of the panel identified by `panel_id`.
        #[allow(dead_code)]
        pub fn update_panel(&self, panel_id: &str, widgets: Vec<ocs_plugin_api::panel::Widget>) {
            self.panels.borrow_mut().update(panel_id, widgets);
        }

        /// Send a panel event to the plugin process owning the panel with
        /// `handle`. Used when the plugin addresses a panel by handle rather
        /// than by id.
        pub fn send_panel_event(
            &self,
            handle: PanelHandle,
            event: PanelEvent,
        ) -> Result<(), PanelError> {
            // Find the panel by handle so we can route to the right process/id.
            let panels = self.panels.borrow();
            let (process_id, panel_id) = panels
                .panel_by_handle(handle)
                .map(|(pid, id)| (pid.to_string(), id.to_string()))
                .ok_or(PanelError::UnknownHandle)?;
            drop(panels);
            self.panels
                .borrow_mut()
                .send_panel_event_by_ids(&process_id, &panel_id, event);
            Ok(())
        }

        /// Handle an asynchronous plugin event from an in-process plugin.
        pub fn handle_async(&self, event: ocs_plugin_api::ipc::protocol::PluginAsync) {
            panel_log(&format!("handle_async: {event:?}"));
            let mut panels = self.panels.borrow_mut();
            match event {
                ocs_plugin_api::ipc::protocol::PluginAsync::PanelUpdate { panel_id, widgets } => {
                    panels.update(&panel_id, widgets);
                }
                ocs_plugin_api::ipc::protocol::PluginAsync::PanelClosed { panel_id } => {
                    if let Some(handle) = panels.handle_by_panel_id(&panel_id) {
                        let _ = panels.close(handle);
                    }
                }
                ocs_plugin_api::ipc::protocol::PluginAsync::DocumentRefreshRequested => {}
            }
            panel_log("handle_async done");
        }

        /// Drain all queued async events from plugin processes and apply them.
        pub fn drain_and_handle_async(&self, host: &mut dyn ocs_plugin_api::host::HostApi) {
            let events = self.manager.drain_async_events(host);
            panel_log(&format!("drain_and_handle_async: {} events", events.len()));
            for event in events {
                self.handle_async(event);
            }
        }

        /// Broadcast a document lifecycle event to every panel-owning plugin.
        pub fn broadcast_document_event(&self, tab: usize, event: DocumentEvent) {
            panel_log(&format!("broadcast_document_event tab={tab} event={event:?}"));
            self.panels.borrow().broadcast_document_event(tab, event);
        }

        /// Handle a host UI message that may target a plugin panel.
        pub fn handle_message(
            &self,
            msg: &crate::app::Message,
        ) -> Option<iced::Task<crate::app::Message>> {
            panel_log(&format!("handle_message: {msg:?}"));
            let result = self.panels.borrow_mut().handle_message(msg);
            panel_log("handle_message done");
            result
        }

        /// Render the open plugin panels as floating overlays.
        pub fn view(&self) -> iced::Element<'static, crate::app::Message> {
            panel_log("view start");
            let result = self.panels.borrow().view();
            panel_log("view done");
            result
        }

        /// Returns whether the user is currently dragging or resizing a panel.
        pub fn is_dragging_or_resizing(&self) -> bool {
            self.panels.borrow().is_dragging_or_resizing()
        }

        /// Ribbon modules for alive, non-disabled plugins.
        pub fn ribbon_modules<F: Fn(&str) -> bool>(
            &self,
            is_disabled: F,
        ) -> Vec<(i32, ocs_plugin_api::ribbon::owned::SharedCadModule)> {
            self.manager.ribbon_modules(is_disabled)
        }

        /// Command names advertised by every alive, non-disabled plugin.
        pub fn command_names<F: Fn(&str) -> bool>(&self, is_disabled: F) -> Vec<String> {
            self.manager.command_names(is_disabled)
        }

        /// Dispatch `cmd` to each plugin until one handles it.
        pub fn dispatch<F: Fn(&str) -> bool>(
            &self,
            host: &mut dyn ocs_plugin_api::host::HostApi,
            cmd: &str,
            is_disabled: F,
        ) -> ocs_plugin_api::process::DispatchResult {
            self.manager.dispatch(host, cmd, is_disabled)
        }

        /// Plugin ids currently loaded.
        pub fn ids(&self) -> Vec<String> {
            self.manager.ids()
        }

        /// Eagerly shut down all plugin runner processes.
        pub fn shutdown_all(&mut self) {
            self.manager.shutdown_all();
        }
    }

    // Process-wide plugin manager. Drop kills every runner process asynchronously
    // so host shutdown is never delayed by a plugin.
    thread_local! {
        static MANAGER: RefCell<Option<HostPluginManager>> = const { RefCell::new(None) };
    }

    /// Discover packages and spawn every API-compatible one as a separate
    /// process. Call once at startup. Returns per-id results so the host can
    /// report load failures.
    pub(crate) fn load_at_startup(
        app: &mut crate::app::OpenCADStudio,
    ) -> Vec<(String, Result<(), String>)> {
        let discovered = super::discover();
        let mut host_manager = HostPluginManager::new();
        let mut out = Vec::new();
        for d in &discovered {
            if !d.api_compatible() || !d.lib_present {
                continue;
            }
            let Some(path) = lib_file(&d.dir) else {
                out.push((
                    d.id.clone(),
                    Err("no native library in package".to_string()),
                ));
                continue;
            };
            let mut host = crate::app::plugin_host::HostSession::new(app, 0);
            match host_manager.manager.load(&path, &mut host) {
                Ok(id) => {
                    // Tell the plugin about the initially active tab and
                    // register any panels it declares.
                    if let Some(process) = host_manager.manager.process(&id) {
                        let _ = process.send_async(HostAsync::DocumentActivated { tab: 0 });
                        let mut panels = host_manager.panels.borrow_mut();
                        for def in process.panels() {
                            panels.register_def(def);
                        }
                    }
                    out.push((id, Ok(())));
                }
                Err(e) => out.push((d.id.clone(), Err(e.to_string()))),
            }
        }
        MANAGER.with(|m| *m.borrow_mut() = Some(host_manager));
        out
    }

    /// Ids of the plugins currently loaded in the process store.
    pub fn loaded_ids() -> Vec<String> {
        MANAGER.with(|m| m.borrow().as_ref().map(|mgr| mgr.ids()).unwrap_or_default())
    }

    /// Run `f` with a reference to the loaded host plugin manager.
    pub fn with_manager<R>(f: impl FnOnce(&HostPluginManager) -> R) -> R {
        MANAGER.with(|m| {
            let guard = m.borrow();
            if let Some(manager) = guard.as_ref() {
                return f(manager);
            }
            drop(guard);
            let empty = HostPluginManager::new();
            f(&empty)
        })
    }

    /// Eagerly shut down all plugin runner processes.
    pub fn shutdown_plugins() {
        MANAGER.with(|m| {
            if let Some(mut manager) = m.borrow_mut().take() {
                manager.shutdown_all();
            }
        });
    }

    /// Path to the native library beside `plugin.toml`, if any.
    fn lib_file(dir: &Path) -> Option<PathBuf> {
        let ext = lib_extension();
        std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let p = e.path();
            (p.extension().and_then(|s| s.to_str()) == Some(ext)).then_some(p)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_v2_plugin_from_template_is_compatible() {
        let toml = r#"
[plugin]
id = "opencad.my_plugin"
name = "My Plugin"
version = "0.1.0"
description = "Template plugin"

[opencad]
api_version = 2
ribbon_order = 60
command_prefixes = ["MP_"]
xdata_apps = ["MYPLUGIN_RECORD"]
"#;
        let p = parse_plugin_toml(toml).expect("parsed");
        assert_eq!(p.api_version, 2);
        assert!(p.command_prefixes.contains(&"MP_".to_string()));
        assert!(p.api_compatible(), "API v2 plugins must run on API v3 host");
    }

    #[test]
    fn missing_id_is_rejected() {
        assert!(parse_plugin_toml("name = \"x\"").is_none());
    }

    #[test]
    fn incompatible_api_flagged() {
        let p = parse_plugin_toml("id=\"a\"\napi_version = 9999").unwrap();
        assert!(!p.api_compatible());
        assert!(!p.loadable());
    }

    /// Integration smoke test for the out-of-process plugin path.
    /// Set `OCS_TEST_PLUGIN` to the built cdylib path and make sure the
    /// `OpenCADStudio` binary is built; the test uses it as the runner host.
    #[test]
    fn spawn_and_dispatch_test_plugin() {
        let path = match std::env::var_os("OCS_TEST_PLUGIN") {
            Some(p) => std::path::PathBuf::from(p),
            None => return,
        };
        if !path.exists() {
            eprintln!("OCS_TEST_PLUGIN does not exist: {}", path.display());
            return;
        }
        let host_exe = std::path::PathBuf::from(
            std::env::var_os("OCS_PLUGIN_RUNNER_EXE")
                .unwrap_or_else(|| std::env::current_exe().unwrap().into_os_string()),
        );
        assert!(
            host_exe.exists(),
            "host exe not found: {}",
            host_exe.display()
        );
        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", &host_exe);

        let mut app = crate::app::OpenCADStudio::new_for_test();
        let mut host = crate::app::plugin_host::HostSession::new(&mut app, 0);
        let process = ocs_plugin_api::process::PluginProcess::spawn(&path, &mut host)
            .expect("spawn test plugin");
        assert_eq!(process.id(), "opencad.my_plugin");
        let mut started = false;
        let handled = process
            .dispatch(&mut host, "MP_HELLO", &mut |_id| {
                started = true;
            })
            .expect("dispatch MP_HELLO");
        assert!(handled, "plugin should handle MP_HELLO");
        assert!(!started, "MP_HELLO is not interactive");
    }
}
