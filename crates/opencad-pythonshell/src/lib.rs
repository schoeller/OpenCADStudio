//! OpenCADStudio Python Shell plugin (API V3).
//!
//! Registers a ribbon tab, starts an async session when `PYSHELL` is invoked,
//! and runs a minimal REPL UI in a deferred egui viewport on the plugin main
//! thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crossbeam_channel::Sender;

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::ipc::protocol::{HostRequest, HostResponse};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::CadModule;

mod host_proxy;
mod interpreter;
mod ocs_module;
mod ribbon;
mod shell;

use host_proxy::HostProxy;

static MANIFEST: PluginManifest = PluginManifest {
    id: "opencad.pythonshell",
    name: "Python Shell",
    version: "0.1.0",
    description: "Interactive Python shell for OpenCADStudio.",
    api_version: ApiVersion { major: 3 },
    ribbon_order: 80,
    xdata_apps: &["PYSHELL_RECORD"],
    command_prefixes: &["PYSHELL"],
};

/// Requests sent from the dispatch thread to the main-thread controller.
#[derive(Debug)]
pub enum UiRequest {
    /// Open a new Python shell viewport for `tab` using `session_id`.
    Open {
        tab: usize,
        session_id: String,
        proxy: HostProxy,
    },
    /// Raise the existing Python shell viewport for `tab`.
    Raise { tab: usize },
    /// Close the viewport for `session_id`.
    Close { session_id: String },
    /// Shut down the controller and exit the runner.
    Shutdown,
}

static UI_TX: Mutex<Option<Sender<UiRequest>>> = Mutex::new(None);
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);
static SESSIONS: std::sync::LazyLock<Mutex<HashMap<usize, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// V3 plugin entry point.
pub struct PythonShellPlugin;

impl BuiltinPlugin for PythonShellPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(ribbon::PythonShellModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        if cmd != "PYSHELL" {
            return false;
        }

        let tab = host.tab_index();
        let tx = UI_TX.lock().unwrap().clone();
        let Some(tx) = tx else {
            host.push_error("Python Shell controller is not running");
            return false;
        };

        let mut sessions = SESSIONS.lock().unwrap();
        if let Some(session_id) = sessions.get(&tab).cloned() {
            host.push_info(&format!("Raising Python Shell for tab {tab}"));
            let _ = tx.send(UiRequest::Raise { tab });
            // Notify the runner that the session is still active; the host
            // adapter was already created for this tab.
            let _ = host.start_async_session(&session_id);
            return true;
        }

        let session_id = format!("pyshell-{tab}-{}", SESSION_COUNTER.fetch_add(1, Ordering::Relaxed));
        let handle = match host.start_async_session(&session_id) {
            Some(h) => h,
            None => {
                host.push_error("Host rejected Python Shell async session");
                return false;
            }
        };

        sessions.insert(tab, session_id.clone());
        host.push_info(&format!("Opening Python Shell {session_id}"));

        let proxy = HostProxy::new(handle);
        let _ = tx.send(UiRequest::Open {
            tab,
            session_id,
            proxy,
        });
        true
    }

    fn run_on_main_thread(&self) -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = crossbeam_channel::unbounded::<UiRequest>();
        *UI_TX.lock().unwrap() = Some(tx);

        shell::run_controller(rx)
    }

    fn shutdown(&self) {
        if let Some(tx) = UI_TX.lock().unwrap().take() {
            let _ = tx.send(UiRequest::Shutdown);
        }
    }

    fn on_host_request(&self, req: &HostRequest) -> Option<HostResponse> {
        match req {
            HostRequest::Shutdown => {
                if let Some(tx) = UI_TX.lock().unwrap().as_ref() {
                    let _ = tx.send(UiRequest::Shutdown);
                }
                Some(HostResponse::Bool(true))
            }
            HostRequest::EndAsyncSession { session_id } => {
                let mut sessions = SESSIONS.lock().unwrap();
                sessions.retain(|_, id| id != session_id);
                if let Some(tx) = UI_TX.lock().unwrap().as_ref() {
                    let _ = tx.send(UiRequest::Close {
                        session_id: session_id.clone(),
                    });
                }
                Some(HostResponse::Bool(true))
            }
            _ => None,
        }
    }
}

ocs_plugin_api::export_plugin!(PythonShellPlugin);
