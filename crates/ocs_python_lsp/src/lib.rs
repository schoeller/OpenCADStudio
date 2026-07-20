//! LSP-bridged Python editor plugin (`ocs.python_lsp`).
//!
//! Contributes a `Python` ribbon tab with an `Editor LSP` tool. Each
//! `PYTHONEDIT` command launches an external editor bound to the currently
//! active document tab via a per-call LSP server and a small Python stdio
//! bridge copied into a temporary workspace.

use std::sync::{Arc, Mutex};

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync, PluginRequest, PluginResponse};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::panel::{DockStyle, DockZone, PanelDef, PanelEvent, PanelHandle, Widget};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

mod bootstrap;
mod debugger;
mod editor;
pub mod host_api;
pub mod host_queue;
pub mod lsp_server;
pub mod worker;
mod workspace;

use editor::launch_editor;

use host_queue::HostQueue;
use lsp_server::LspServer;
use worker::{spawn_python_worker, Worker};
use workspace::Workspace;

const PANEL_ID: &str = "python_lsp_status";
const SERVERS_WIDGET_ID: &str = "py_lsp_servers";

static MANIFEST: PluginManifest = PluginManifest {
    id: "ocs.python_lsp",
    name: "Python LSP Editor",
    version: "0.1.0",
    description: "LSP-bridged Python editor integration for OpenCAD Studio.",
    api_version: ApiVersion { major: 3 },
    ribbon_order: 210,
    xdata_apps: &["PY_LSP"],
    command_prefixes: &["PYTHONEDIT"],
};

struct PythonLspModule;

impl CadModule for PythonLspModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        MANIFEST.name
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        Box::leak(Box::new(vec![RibbonGroup {
            title: "Python",
            tools: vec![RibbonItem::Tool(ToolDef {
                id: "PYTHONEDIT",
                label: "Editor LSP",
                icon: IconKind::Glyph("Editor LSP"),
                event: ModuleEvent::Command("PYTHONEDIT".to_string()),
            })],
        }])).as_slice()
    }
}

fn panel_def() -> PanelDef {
    PanelDef {
        id: PANEL_ID.to_string(),
        title: "Python LSP".to_string(),
        icon: None,
        dock: DockZone::Floating,
        initial_x: Some(120.0),
        initial_y: Some(120.0),
        initial_width: 350.0,
        initial_height: 200.0,
        min_width: 200.0,
        min_height: 120.0,
        dockable_zones: vec![DockZone::Floating, DockZone::Left, DockZone::Right],
        allow_undock: true,
        resizable: true,
        draggable: true,
        dock_style: DockStyle::Tabs,
    }
}

fn build_widgets(server_count: usize) -> Vec<Widget> {
    vec![
        Widget::Label(format!("Active LSP servers: {server_count}")),
        Widget::MultilineOutput {
            id: SERVERS_WIDGET_ID.to_string(),
            lines: Vec::new(),
        },
    ]
}

struct PluginState {
    queue: HostQueue,
    servers: Vec<LspServer>,
    panel_handle: Option<PanelHandle>,
    worker: Option<Arc<Mutex<Worker>>>,
    python_missing_notified: bool,
}

pub struct PythonLspPlugin {
    state: Mutex<PluginState>,
}

impl PythonLspPlugin {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(PluginState {
                queue: HostQueue::new(),
                servers: Vec::new(),
                panel_handle: None,
                worker: None,
                python_missing_notified: false,
            }),
        }
    }

    fn ensure_panel_open(&self, host: &mut dyn HostApi) -> Result<PanelHandle, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(handle) = state.panel_handle {
            return Ok(handle);
        }
        match host.open_panel(&panel_def()) {
            Ok(handle) => {
                state.panel_handle = Some(handle);
                Ok(handle)
            }
            Err(e) => Err(format!("failed to open status panel: {e}")),
        }
    }

    fn update_panel(&self, host: &mut dyn HostApi) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let count = state.servers.len();
        drop(state);
        host.send_async(PluginAsync::PanelUpdate {
            panel_id: PANEL_ID.to_string(),
            widgets: build_widgets(count),
        });
    }
}

impl Default for PythonLspPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinPlugin for PythonLspPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(PythonLspModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        if cmd != "PYTHONEDIT" {
            return false;
        }

        let tab = host.tab_index();
        let (queue, worker) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.worker.is_none() {
                match spawn_python_worker() {
                    Ok(w) => state.worker = Some(Arc::new(Mutex::new(w))),
                    Err(e) => {
                        state.python_missing_notified = true;
                        drop(state);
                        host.push_error(&format!("Python interpreter not found: {e}. Install Python or set OCS_PYTHON_EXE."));
                        let _ = self.ensure_panel_open(host);
                        self.update_panel(host);
                        return true;
                    }
                }
            }
            (state.queue.clone(), state.worker.clone().unwrap())
        };

        let server = match LspServer::start(tab, queue, worker) {
            Ok(s) => s,
            Err(e) => {
                host.push_error(&format!("Failed to start LSP server: {e}"));
                return true;
            }
        };

        {
            let port = server.port;
            let workspace = match Workspace::create(tab, port) {
                Ok(ws) => ws,
                Err(e) => {
                    host.push_error(&format!("Failed to create workspace: {e}"));
                    return true;
                }
            };

            match launch_editor(workspace.root()) {
                Ok(editor) => host.push_info(&format!("Launched {editor} for tab {tab} on port {port}.")),
                Err(e) => host.push_error(&e),
            }

            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.servers.push(server);
        }

        match self.ensure_panel_open(host) {
            Ok(_) => self.update_panel(host),
            Err(e) => host.push_error(&e),
        }

        true
    }

    fn panels(&self) -> Vec<PanelDef> {
        vec![panel_def()]
    }

    fn on_async_event(&mut self, host: &mut dyn HostApi, event: HostAsync) {
        let HostAsync::PanelEvent { panel_id, event } = event else {
            return;
        };
        if panel_id != PANEL_ID {
            return;
        }

        match event {
            PanelEvent::Closed => {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                for server in state.servers.drain(..) {
                    server.cancel();
                }
                if let Some(worker) = state.worker.take() {
                    if let Ok(w) = worker.lock() {
                        w.close();
                    }
                }
                state.panel_handle = None;
            }
            _ => {}
        }

        // Drain any queued host requests regardless of which panel event fired.
        self.drain_queue(host);
        self.update_panel(host);
    }
}

impl PythonLspPlugin {
    fn drain_queue(&self, host: &mut dyn HostApi) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let items = state.queue.drain();
        drop(state);

        let original_tab = host.tab_index();
        for (tab, req, reply_tx) in items {
            let result = if let Err(e) = host.set_active_tab(tab) {
                PluginResponse::Error(e)
            } else {
                // Apply the request. For now only a tiny subset is wired; most
                // commands still go through the Python worker (Phase 7).
                apply_request(host, req)
            };
            // Restore the original tab before sending the reply so concurrent
            // requests for other tabs are not affected.
            if let Err(e) = host.set_active_tab(original_tab) {
                eprintln!("[ocs_python_lsp] failed to restore tab {original_tab}: {e}");
            }
            let _ = reply_tx.send(result);
        }
    }
}

fn apply_request(host: &mut dyn HostApi, req: PluginRequest) -> PluginResponse {
    use PluginRequest::*;
    match req {
        PushInfo(msg) => {
            host.push_info(&msg);
            PluginResponse::Ok
        }
        PushOutput(msg) => {
            host.push_output(&msg);
            PluginResponse::Ok
        }
        PushError(msg) => {
            host.push_error(&msg);
            PluginResponse::Ok
        }
        AddEntity(entity) => PluginResponse::Handle(host.add_entity(entity)),
        UpdateEntity(entity) => PluginResponse::Bool(host.update_entity(entity)),
        BumpGeometry => {
            host.bump_geometry();
            PluginResponse::Ok
        }
        ReadRecord { handle, app_name } => {
            PluginResponse::Record(host.read_record(handle, &app_name).cloned())
        }
        WriteRecord { handle, record } => PluginResponse::Bool(host.write_record(handle, record)),
        RemoveRecord { handle, app_name } => {
            PluginResponse::Bool(host.remove_record(handle, &app_name))
        }
        RemoveEntity { handle } => match host.remove_entity(handle) {
            true => PluginResponse::Bool(true),
            false => PluginResponse::Error(format!("entity {} not found", handle.value())),
        },
        RemoveEntities { handles } => {
            let mut removed = 0usize;
            for handle in handles {
                if host.remove_entity(handle) {
                    removed += 1;
                }
            }
            PluginResponse::Count(removed)
        }
        PushUndo { label } => {
            host.push_undo(&label);
            PluginResponse::Ok
        }
        SetDirty => {
            host.set_dirty();
            PluginResponse::Ok
        }
        StartInteractive { .. } => PluginResponse::Error("interactive commands not supported".to_string()),
        DocumentSnapshot => PluginResponse::Document(host.document().clone()),
        OpenDocumentView => match host.document_view() {
            Some(info) => PluginResponse::DocumentView {
                path: info.path,
                version: info.version,
            },
            None => PluginResponse::Error("shared document view unavailable".to_string()),
        },
        // Panel ops are handled by the host directly; they should not arrive here.
        OpenPanel { .. }
        | ClosePanel { .. }
        | MovePanel { .. }
        | ResizePanel { .. }
        | DockPanel { .. }
        | UndockPanel { .. }
        | PostPanelEvent { .. }
        | RequestPointPick { .. }
        | SetActiveTab(_) => PluginResponse::Error("unexpected request in LSP queue".to_string()),
    }
}

ocs_plugin_api::export_plugin!(PythonLspPlugin::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_and_ribbon_are_valid() {
        let plugin = PythonLspPlugin::new();
        assert_eq!(plugin.manifest().id, "ocs.python_lsp");
        assert_eq!(plugin.manifest().api_version.major, 3);
        let ribbon = plugin.ribbon();
        let groups = ribbon.ribbon_groups();
        assert!(!groups.is_empty());
        assert_eq!(groups[0].title, "Python");
    }
}
