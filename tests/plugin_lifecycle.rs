//! PluginManager lifecycle tests for V2 and V3 out-of-process plugins.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use acadrust::xdata::ExtendedDataRecord;
use acadrust::{CadDocument, EntityType, Handle};
use ocs_plugin_api::host::{
    AsyncSessionError, AsyncSessionHandle, DocumentReader, HostApi, InteractiveCommand,
    ReaderEntity,
};
use ocs_plugin_api::ipc::protocol::HostRequest;
use ocs_plugin_api::process::{PluginManager, PluginProcess};
use ocs_plugin_api::CadModule;

struct EmptyReader;

impl DocumentReader for EmptyReader {
    fn entity_count(&self) -> usize {
        0
    }
    fn for_each_entity(&self, _f: &mut dyn FnMut(ReaderEntity<'_>)) {}
    fn layer_name(&self, _handle: Handle) -> Option<&str> {
        None
    }
    fn app_id_name(&self, _handle: Handle) -> Option<&str> {
        None
    }
}

struct DummyAsyncHandle {
    #[allow(dead_code)]
    session_id: String,
}

impl AsyncSessionHandle for DummyAsyncHandle {
    fn tab_index(&self) -> usize {
        0
    }
    fn request(
        &self,
        _req: ocs_plugin_api::ipc::protocol::PluginRequest,
    ) -> Result<ocs_plugin_api::ipc::protocol::PluginResponse, AsyncSessionError> {
        Err(AsyncSessionError::Closed)
    }
    fn document_reader(&self) -> Box<dyn DocumentReader + 'static> {
        Box::new(EmptyReader)
    }
    fn document_view(&self) -> Option<ocs_plugin_api::shm::DocumentViewInfo> {
        None
    }
}

struct DummyHost {
    document: CadDocument,
    accepted_session: Option<String>,
}

impl HostApi for DummyHost {
    fn tab_index(&self) -> usize {
        0
    }
    fn document(&self) -> &CadDocument {
        &self.document
    }
    fn document_mut(&mut self) -> &mut CadDocument {
        &mut self.document
    }
    fn add_entity(&mut self, _entity: EntityType) -> Handle {
        Handle::NULL
    }
    fn bump_geometry(&mut self) {}
    fn read_record(
        &self,
        _handle: Handle,
        _app_name: &str,
    ) -> Option<&ExtendedDataRecord> {
        None
    }
    fn write_record(&mut self, _handle: Handle, _record: ExtendedDataRecord) -> bool {
        false
    }
    fn remove_record(&mut self, _handle: Handle, _app_name: &str) -> bool {
        false
    }
    fn push_undo(&mut self, _label: &str) {}
    fn set_dirty(&mut self) {}
    fn push_info(&mut self, _msg: &str) {}
    fn push_output(&mut self, _msg: &str) {}
    fn push_error(&mut self, _msg: &str) {}
    fn start_interactive(&mut self, _command: Box<dyn InteractiveCommand>) {}
    fn plugin_state_any(&self, _plugin_id: &str) -> Option<&(dyn Any + Send + Sync)> {
        None
    }
    fn plugin_state_any_mut(
        &mut self,
        _plugin_id: &str,
    ) -> Option<&mut (dyn Any + Send + Sync)> {
        None
    }
    fn ensure_plugin_state_any(
        &mut self,
        _plugin_id: &'static str,
        _init: &mut dyn FnMut() -> Box<dyn Any + Send + Sync>,
    ) -> &mut (dyn Any + Send + Sync) {
        panic!("not used")
    }
    fn document_reader(&self) -> Box<dyn DocumentReader + '_> {
        Box::new(EmptyReader)
    }
    fn start_async_session(&mut self, session_id: &str) -> Option<Box<dyn AsyncSessionHandle>> {
        self.accepted_session = Some(session_id.to_string());
        Some(Box::new(DummyAsyncHandle {
            session_id: session_id.to_string(),
        }))
    }
}

fn runner_exe() -> Option<PathBuf> {
    std::env::var_os("CARGO_BIN_EXE_OpenCADStudio").map(PathBuf::from)
}

fn target_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().expect("current exe");
    // current_exe is target/<profile>/deps/test_name.exe
    exe.parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("target"))
}

fn cdylib_path(crate_name: &str) -> Option<PathBuf> {
    let target = target_dir();
    let normalized = crate_name.replace('-', "_");
    let candidates: Vec<(PathBuf, String)> = {
        let mut v = Vec::new();
        #[cfg(target_os = "windows")]
        {
            v.push((target.join("debug"), format!("{normalized}.dll")));
            v.push((target.join("debug").join("deps"), format!("{normalized}.dll")));
        }
        #[cfg(target_os = "linux")]
        {
            v.push((target.join("debug"), format!("lib{normalized}.so")));
            v.push((target.join("debug").join("deps"), format!("lib{normalized}.so")));
        }
        #[cfg(target_os = "macos")]
        {
            v.push((target.join("debug"), format!("lib{normalized}.dylib")));
            v.push((target.join("debug").join("deps"), format!("lib{normalized}.dylib")));
        }
        v
    };
    for (dir, name) in candidates {
        let path = dir.join(&name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn wait_for<F: FnMut() -> bool>(mut predicate: F, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("wait_for timed out after {timeout:?}");
}

fn setup_runner() {
    if let Some(exe) = runner_exe() {
        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", exe);
    }
}

#[test]
fn v2_plugin_lifecycle() {
    setup_runner();
    let path = match cdylib_path("plugin-template-api2") {
        Some(p) => p,
        None => {
            eprintln!("plugin-template-api2 cdylib not built; skipping");
            return;
        }
    };

    let mut manager = PluginManager::new();
    let mut host = DummyHost {
        document: CadDocument::new(),
        accepted_session: None,
    };

    let id = manager.load(&path, &mut host).expect("load V2 plugin");
    assert_eq!(id, "opencad.plugin_template_api2");
    assert_eq!(manager.ids(), vec![id.clone()]);

    let modules = manager.ribbon_modules(|_| false);
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].1.title(), "Plugin Template API V2");

    let result = manager.dispatch(&mut host, "PT2_HELLO", |_| false);
    assert!(result.handled, "PT2_HELLO should be handled");
    assert!(result.async_session.is_none(), "V2 plugin should not start async session");

    let result = manager.dispatch(&mut host, "PT2_UNKNOWN", |_| false);
    assert!(!result.handled, "unknown command should not be handled");
    assert!(result.dead_plugins.is_empty(), "process should be alive");

    assert!(manager.is_alive(&id), "plugin process should be alive before shutdown");

    manager.shutdown_all();
    wait_for(|| !manager.is_alive(&id), Duration::from_secs(5));
    assert!(!manager.is_alive(&id), "plugin process should be dead after shutdown");
}

#[test]
fn v3_plugin_lifecycle() {
    setup_runner();
    let path = match cdylib_path("plugin-template-api3") {
        Some(p) => p,
        None => {
            eprintln!("plugin-template-api3 cdylib not built; skipping");
            return;
        }
    };

    let mut manager = PluginManager::new();
    let mut host = DummyHost {
        document: CadDocument::new(),
        accepted_session: None,
    };

    let id = manager.load(&path, &mut host).expect("load V3 plugin");
    assert_eq!(id, "opencad.plugin_template_api3");
    assert_eq!(manager.ids(), vec![id.clone()]);

    let modules = manager.ribbon_modules(|_| false);
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].1.title(), "Plugin Template API V3");

    let result = manager.dispatch(&mut host, "PT3_NO_SESSION", |_| false);
    assert!(result.handled, "PT3_NO_SESSION should be handled");
    assert!(result.async_session.is_none(), "PT3_NO_SESSION should not start async session");

    let result = manager.dispatch(&mut host, "PT3_START_SESSION", |_| false);
    assert!(result.handled, "PT3_START_SESSION should be handled");
    let (_, session_id) = result
        .async_session
        .expect("PT3_START_SESSION should start an async session");
    assert_eq!(session_id, "pt3-session");
    assert_eq!(host.accepted_session.as_deref(), Some("pt3-session"));

    wait_for(|| manager.is_alive(&id), Duration::from_secs(5));
    assert!(manager.is_alive(&id), "V3 plugin process should be alive before shutdown");

    manager.shutdown_all();
    wait_for(|| !manager.is_alive(&id), Duration::from_secs(5));
    assert!(!manager.is_alive(&id), "V3 plugin process should be dead after shutdown");
}

#[test]
fn v3_request_fails_when_process_is_killed() {
    setup_runner();
    let path = match cdylib_path("plugin-template-api3") {
        Some(p) => p,
        None => {
            eprintln!("plugin-template-api3 cdylib not built; skipping");
            return;
        }
    };

    let mut host = DummyHost {
        document: CadDocument::new(),
        accepted_session: None,
    };

    let process = PluginProcess::spawn(&path, &mut host).expect("spawn V3 plugin");
    assert!(process.is_alive(), "process should be alive");

    process.shutdown_all();
    wait_for(|| !process.is_alive(), Duration::from_secs(5));
    assert!(!process.is_alive(), "process should be dead after kill");

    // After the runner is gone, a V3 request must not hang indefinitely.
    let err = process
        .request_v3("", HostRequest::GetManifest)
        .expect_err("request on dead process should fail");
    let err_string = err.to_string();
    assert!(
        err_string.contains("session closed")
            || err_string.contains("CallTimeout")
            || err_string.contains("timed out")
            || err_string.contains("shut down"),
        "expected session-closed, timeout, or shutdown error, got: {err_string}"
    );
}

