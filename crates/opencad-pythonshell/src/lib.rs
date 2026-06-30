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

#[cfg(test)]
fn clear_session_map() {
    SESSIONS.lock().unwrap().clear();
    SESSION_COUNTER.store(1, Ordering::SeqCst);
}

#[cfg(test)]
fn set_controller_tx(tx: Sender<UiRequest>) {
    *UI_TX.lock().unwrap() = Some(tx);
}

/// Remove the session entry whose id matches `session_id` from the global session map.
///
/// Called by the controller when the plugin itself initiates the close.
pub(crate) fn remove_session_by_id(session_id: &str) {
    let mut sessions = SESSIONS.lock().unwrap();
    sessions.retain(|_, id| id != session_id);
}

/// Returns true if the global session map contains `tab`.
#[cfg(test)]
pub(crate) fn has_session_for_tab(tab: usize) -> bool {
    SESSIONS.lock().unwrap().contains_key(&tab)
}

#[cfg(test)]
mod tests {
    use super::*;

    use ocs_plugin_api::host::{AsyncSessionError, AsyncSessionHandle, DocumentReader, ReaderEntity};
    use ocs_plugin_api::ipc::protocol::{PluginRequest, PluginResponse};
    use ocs_plugin_api::shm::DocumentViewInfo;

    // Tests mutate global static state (UI_TX and SESSIONS); run them serially.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EmptyReader;
    impl DocumentReader for EmptyReader {
        fn entity_count(&self) -> usize { 0 }
        fn for_each_entity(&self, _f: &mut dyn FnMut(ReaderEntity<'_>)) {}
        fn layer_name(&self, _handle: acadrust::Handle) -> Option<&str> { None }
        fn app_id_name(&self, _handle: acadrust::Handle) -> Option<&str> { None }
    }

    struct MockHost {
        tab: usize,
        infos: Mutex<Vec<String>>,
        errors: Mutex<Vec<String>>,
        sessions: Mutex<Vec<String>>,
        accept_session: bool,
    }

    impl MockHost {
        fn new(tab: usize, accept_session: bool) -> Self {
            Self {
                tab,
                infos: Mutex::new(Vec::new()),
                errors: Mutex::new(Vec::new()),
                sessions: Mutex::new(Vec::new()),
                accept_session,
            }
        }
    }

    impl HostApi for MockHost {
        fn tab_index(&self) -> usize {
            self.tab
        }
        fn push_info(&mut self, msg: &str) {
            self.infos.lock().unwrap().push(msg.to_string());
        }
        fn push_error(&mut self, msg: &str) {
            self.errors.lock().unwrap().push(msg.to_string());
        }
        fn start_async_session(&mut self, session_id: &str) -> Option<Box<dyn AsyncSessionHandle>> {
            self.sessions.lock().unwrap().push(session_id.to_string());
            if !self.accept_session {
                return None;
            }
            struct Dummy;
            impl AsyncSessionHandle for Dummy {
                fn tab_index(&self) -> usize {
                    0
                }
                fn request(
                    &self,
                    _req: PluginRequest,
                ) -> Result<PluginResponse, AsyncSessionError> {
                    Ok(PluginResponse::Ok)
                }
                fn document_reader(&self) -> Box<dyn DocumentReader + 'static> {
                    Box::new(EmptyReader)
                }
                fn document_view(&self) -> Option<DocumentViewInfo> {
                    None
                }
            }

            Some(Box::new(Dummy))
        }
        fn document(&self) -> &acadrust::CadDocument {
            panic!("not used")
        }
        fn document_mut(&mut self) -> &mut acadrust::CadDocument {
            panic!("not used")
        }
        fn add_entity(&mut self, _entity: acadrust::EntityType) -> acadrust::Handle {
            panic!("not used")
        }
        fn bump_geometry(&mut self) {}
        fn read_record(
            &self,
            _handle: acadrust::Handle,
            _app_name: &str,
        ) -> Option<&acadrust::xdata::ExtendedDataRecord> {
            None
        }
        fn write_record(
            &mut self,
            _handle: acadrust::Handle,
            _record: acadrust::xdata::ExtendedDataRecord,
        ) -> bool {
            false
        }
        fn remove_record(&mut self, _handle: acadrust::Handle, _app_name: &str) -> bool {
            false
        }
        fn push_undo(&mut self, _label: &str) {}
        fn set_dirty(&mut self) {}
        fn push_output(&mut self, _msg: &str) {}
        fn start_interactive(
            &mut self,
            _command: Box<dyn ocs_plugin_api::host::InteractiveCommand>,
        ) {
        }
        fn plugin_state_any(&self, _plugin_id: &str) -> Option<&(dyn std::any::Any + Send + Sync)> {
            None
        }
        fn plugin_state_any_mut(
            &mut self,
            _plugin_id: &str,
        ) -> Option<&mut (dyn std::any::Any + Send + Sync)> {
            None
        }
        fn ensure_plugin_state_any(
            &mut self,
            _plugin_id: &'static str,
            _init: &mut dyn FnMut() -> Box<dyn std::any::Any + Send + Sync>,
        ) -> &mut (dyn std::any::Any + Send + Sync) {
            panic!("not used")
        }
        fn document_reader(&self) -> Box<dyn DocumentReader + '_> {
            Box::new(EmptyReader)
        }
    }

    #[test]
    fn dispatch_opens_new_session_and_sends_open_request() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_session_map();
        let (tx, rx) = crossbeam_channel::unbounded();
        set_controller_tx(tx);

        let plugin = PythonShellPlugin;
        let mut host = MockHost::new(7, true);
        let handled = plugin.dispatch(&mut host, "PYSHELL");

        assert!(handled, "PYSHELL should be handled");
        assert_eq!(host.sessions.lock().unwrap().len(), 1);
        assert!(host.sessions.lock().unwrap()[0].starts_with("pyshell-7-"));
        let req = rx.try_recv().expect("Open request expected");
        assert!(matches!(req, UiRequest::Open { tab: 7, .. }));
    }

    #[test]
    fn dispatch_without_running_controller_errors() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_session_map();
        *UI_TX.lock().unwrap() = None;

        let plugin = PythonShellPlugin;
        let mut host = MockHost::new(3, true);
        let handled = plugin.dispatch(&mut host, "PYSHELL");
        assert!(!handled);
        assert_eq!(host.errors.lock().unwrap().len(), 1);
    }

    #[test]
    fn dispatch_unknown_command_returns_false() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_session_map();
        let plugin = PythonShellPlugin;
        let mut host = MockHost::new(0, true);
        assert!(!plugin.dispatch(&mut host, "UNKNOWN"));
    }

    #[test]
    fn dispatch_on_existing_tab_sends_raise_only() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_session_map();
        let (tx, rx) = crossbeam_channel::unbounded();
        set_controller_tx(tx);
        SESSIONS.lock().unwrap().insert(4, "pyshell-4-old".to_string());

        let plugin = PythonShellPlugin;
        let mut host = MockHost::new(4, true);
        let handled = plugin.dispatch(&mut host, "PYSHELL");

        assert!(handled);
        // Should re-use existing session ID and not create a new one.
        assert_eq!(
            host.sessions.lock().unwrap().clone(),
            vec!["pyshell-4-old".to_string()]
        );
        let req = rx.try_recv().expect("Raise expected");
        assert!(matches!(req, UiRequest::Raise { tab: 4 }));
    }

    #[test]
    fn on_host_request_end_async_session_sends_close() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_session_map();
        let (tx, rx) = crossbeam_channel::unbounded();
        set_controller_tx(tx);
        SESSIONS.lock().unwrap().insert(2, "s-2".to_string());

        let plugin = PythonShellPlugin;
        let resp = plugin.on_host_request(&HostRequest::EndAsyncSession {
            session_id: "s-2".to_string(),
        });

        assert!(matches!(resp, Some(HostResponse::Bool(true))));
        assert!(!SESSIONS.lock().unwrap().contains_key(&2));
        let req = rx.try_recv().expect("Close expected");
        assert!(matches!(req, UiRequest::Close { session_id } if session_id == "s-2"));
    }

    #[test]
    fn on_host_request_shutdown_sends_shutdown() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_session_map();
        let (tx, rx) = crossbeam_channel::unbounded();
        set_controller_tx(tx);

        let plugin = PythonShellPlugin;
        let resp = plugin.on_host_request(&HostRequest::Shutdown);

        assert!(matches!(resp, Some(HostResponse::Bool(true))));
        let req = rx.try_recv().expect("Shutdown expected");
        assert!(matches!(req, UiRequest::Shutdown));
    }

    #[test]
    fn on_host_request_unknown_returns_none() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_session_map();

        let plugin = PythonShellPlugin;
        let resp = plugin.on_host_request(&HostRequest::GetManifest);
        assert!(resp.is_none());
    }

    #[test]
    fn dispatch_when_host_rejects_session_errors() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear_session_map();
        let (tx, _rx) = crossbeam_channel::unbounded();
        set_controller_tx(tx);

        let plugin = PythonShellPlugin;
        let mut host = MockHost::new(5, false);
        let handled = plugin.dispatch(&mut host, "PYSHELL");

        assert!(!handled);
        assert_eq!(host.errors.lock().unwrap().len(), 1);
        assert!(!SESSIONS.lock().unwrap().contains_key(&5));
    }
}

ocs_plugin_api::export_plugin!(PythonShellPlugin);
