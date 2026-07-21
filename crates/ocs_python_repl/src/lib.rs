//! API V3 Python REPL plugin.
//!
//! One-click Python editor/debugging plugin that runs on the user's locally
//! installed Python + PIP, exposes the current document through a Pythonic
//! `acadifc` object model, and gives fast read/write/erase access to the host
//! document via shared memory.

use std::sync::{Arc, Mutex};

use ocs_plugin_api::export_plugin;
use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::panel::{DockStyle, DockZone, PanelDef, PanelHandle, PanelEvent, Widget};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

mod editor;
mod python_env;
mod repl;
mod workspace;

pub static MANIFEST: PluginManifest = PluginManifest {
    id: "ocs.python.repl",
    name: "Python REPL",
    version: "0.1.0",
    description: "Python scripting and debugging for OpenCAD Studio.",
    api_version: ApiVersion { major: 3 },
    ribbon_order: 300,
    xdata_apps: &["OCS_PYTHON"],
    command_prefixes: &["PYTHONEDIT"],
};

#[derive(Default)]
struct PluginState {
    panel_handle: Option<PanelHandle>,
    status: String,
    session: Option<repl::ReplSession>,
}

pub struct PythonReplPlugin {
    state: Arc<Mutex<PluginState>>,
}

impl PythonReplPlugin {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(PluginState::default())),
        }
    }

    fn panel_def() -> PanelDef {
        PanelDef {
            id: "ocs_python_repl".to_string(),
            title: "Python REPL".to_string(),
            icon: None,
            dock: DockZone::Floating,
            initial_x: Some(120.0),
            initial_y: Some(80.0),
            initial_width: 320.0,
            initial_height: 240.0,
            min_width: 200.0,
            min_height: 120.0,
            dockable_zones: vec![DockZone::Floating, DockZone::Left, DockZone::Right],
            allow_undock: true,
            resizable: true,
            draggable: true,
            dock_style: DockStyle::Stack,
        }
    }

    fn render_status(status: &str) -> Vec<Widget> {
        vec![
            Widget::Label("Python REPL".to_string()),
            Widget::MultilineOutput {
                id: "status".to_string(),
                lines: status.lines().map(|s| s.to_string()).collect(),
            },
        ]
    }

    fn update_panel(&self, host: &mut dyn HostApi, status: String) {
        let mut state = self.state.lock().unwrap();
        state.status = status;
        host.send_async(PluginAsync::PanelUpdate {
            panel_id: "ocs_python_repl".to_string(),
            widgets: Self::render_status(&state.status),
        });
    }

    fn set_error(&self, host: &mut dyn HostApi, msg: String) {
        host.push_error(&msg);
        self.update_panel(host, msg);
    }
}

struct ReplModule;

impl CadModule for ReplModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        MANIFEST.name
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        Box::leak(Box::new(vec![RibbonGroup {
            title: "Script",
            tools: vec![RibbonItem::LargeTool(ToolDef {
                id: "PYTHONEDIT",
                label: "Python\nEdit",
                icon: IconKind::Glyph("py"),
                event: ModuleEvent::Command("PYTHONEDIT".to_string()),
            })],
        }]))
        .as_slice()
    }
}

impl BuiltinPlugin for PythonReplPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(ReplModule)
    }

    fn panels(&self) -> Vec<PanelDef> {
        vec![Self::panel_def()]
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        if cmd != "PYTHONEDIT" {
            return false;
        }
        let mut state = self.state.lock().unwrap();

        // Open / refresh the status panel.
        if state.panel_handle.is_none() {
            match host.open_panel(&Self::panel_def()) {
                Ok(handle) => state.panel_handle = Some(handle),
                Err(e) => {
                    host.push_error(&format!("Python REPL panel failed: {e}"));
                    return true;
                }
            }
        }

        // Verify Python + PIP and install debugpy if missing.
        let (python, pip) = match python_env::ensure_python() {
            Ok(p) => p,
            Err(e) => {
                drop(state);
                self.set_error(host, format!("Python check failed: {e}"));
                return true;
            }
        };

        if let Err(e) = python_env::ensure_package(&pip, "debugpy") {
            drop(state);
            self.set_error(host, format!("debugpy installation failed: {e}"));
            return true;
        }

        // Create a temp workspace for the script and editor.
        let workspace = match workspace::create() {
            Ok(w) => w,
            Err(e) => {
                drop(state);
                self.set_error(host, format!("workspace creation failed: {e}"));
                return true;
            }
        };

        // Launch the editor with the workspace.
        if let Err(e) = editor::launch(&workspace) {
            drop(state);
            self.set_error(host, format!("editor launch failed: {e}"));
            return true;
        }

        // Spawn the Python REPL / child process.
        match repl::ReplSession::spawn(&python, &workspace, host) {
            Ok(session) => {
                let msg = format!(
                    "Python REPL started.\nInterpreter: {}\nWorkspace: {}",
                    python.display(),
                    workspace.display()
                );
                state.session = Some(session);
                drop(state);
                self.update_panel(host, msg);
                host.push_info("Python REPL started");
            }
            Err(e) => {
                drop(state);
                self.set_error(host, format!("Python REPL spawn failed: {e}"));
            }
        }
        true
    }

    fn on_async_event(&mut self, _host: &mut dyn HostApi, event: HostAsync) {
        match event {
            HostAsync::PanelEvent { panel_id, event } if panel_id == "ocs_python_repl" => {
                if let PanelEvent::Closed = event {
                    let mut state = self.state.lock().unwrap();
                    state.panel_handle = None;
                }
            }
            _ => {}
        }
    }
}

export_plugin!(PythonReplPlugin::new());
