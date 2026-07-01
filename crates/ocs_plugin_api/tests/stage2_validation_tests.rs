//! Stage 2 validation tests for API v3 ABI safety and async IPC.
//!
//! Run with the `host` feature enabled:
//!
//! ```text
//! $env:CARGO_TARGET_DIR='C:\tmp\ocs_target'
//! cargo test -p ocs_plugin_api --features host --test stage2_validation_tests -- --nocapture
//! ```

#[cfg(feature = "host")]
mod stage2_tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;

    use acadrust::xdata::ExtendedDataRecord;
    use acadrust::{CadDocument, EntityType, Handle};
    use ocs_plugin_api::host::{DocumentReader, HostApi, ReaderEntity};
    use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync};
    use ocs_plugin_api::process::{AsyncInbound, PluginProcess};

    fn target_dir() -> PathBuf {
        PathBuf::from(
            std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| r"C:\tmp\ocs_target".to_string()),
        )
    }

    fn cargo_build(package: &str, release: bool) -> PathBuf {
        let target_dir = target_dir();
        let mut cmd = Command::new("cargo");
        cmd.arg("build")
            .arg("-p")
            .arg(package)
            .env("CARGO_TARGET_DIR", &target_dir);
        if release {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("failed to run cargo build");
        assert!(status.success(), "cargo build -p {package} failed");

        let profile = if release { "release" } else { "debug" };
        let mut path = target_dir.join(profile);
        if package == "ocs_plugin_runner" {
            path.push(format!("ocs_plugin_runner{}", std::env::consts::EXE_SUFFIX));
        } else {
            let prefix = std::env::consts::DLL_PREFIX;
            let suffix = std::env::consts::DLL_SUFFIX;
            path.push(format!("{prefix}{package}{suffix}"));
        }
        assert!(path.exists(), "artifact not found at {}", path.display());
        path
    }

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

    struct DummyHost {
        doc: CadDocument,
        infos: Mutex<Vec<String>>,
    }

    impl DummyHost {
        fn new() -> Self {
            Self {
                doc: CadDocument::default(),
                infos: Mutex::new(Vec::new()),
            }
        }
    }

    impl HostApi for DummyHost {
        fn tab_index(&self) -> usize {
            0
        }
        fn document(&self) -> &CadDocument {
            &self.doc
        }
        fn document_mut(&mut self) -> &mut CadDocument {
            &mut self.doc
        }
        fn document_reader(&self) -> Box<dyn DocumentReader + '_> {
            Box::new(EmptyReader)
        }
        fn add_entity(&mut self, _entity: EntityType) -> Handle {
            Handle::default()
        }
        fn bump_geometry(&mut self) {}
        fn read_record(&self, _handle: Handle, _app_name: &str) -> Option<&ExtendedDataRecord> {
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
        fn push_info(&mut self, msg: &str) {
            self.infos
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(msg.to_string());
        }
        fn push_output(&mut self, _msg: &str) {}
        fn push_error(&mut self, _msg: &str) {}
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
    }

    /// A plugin that reports API v3 with a stale ABI revision is rejected by
    /// the runner before its code runs.
    #[test]
    fn old_v3_abi_revision_rejected() {
        let runner = cargo_build("ocs_plugin_runner", true);
        let plugin = cargo_build("test_old_v3_plugin", false);

        let output = Command::new(&runner)
            .arg("dummy_sync_socket")
            .arg("dummy_async_socket")
            .arg(&plugin)
            .output()
            .expect("failed to run runner");

        assert!(
            !output.status.success(),
            "runner should exit with an error for a stale v3 ABI"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("ABI revision mismatch"),
            "expected ABI mismatch error, got: {stderr}"
        );
    }

    /// A host async event is delivered to the plugin and can be observed via a
    /// plugin-to-host async echo.
    #[test]
    fn async_event_roundtrip() {
        let runner = cargo_build("ocs_plugin_runner", true);
        let plugin = cargo_build("test_v3_async_plugin", false);

        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", &runner);
        let mut host = DummyHost::new();
        let process = PluginProcess::spawn(&plugin, &mut host).expect("spawn plugin process");

        assert_eq!(process.id(), "ocs.test.v3_async_plugin");
        assert_eq!(process.manifest().api_version, 3);

        process
            .send_async(HostAsync::DocumentActivated { tab: 7 })
            .expect("send async event");

        // Poll for the plugin's echo event; delivery is asynchronous.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut found = false;
        while std::time::Instant::now() < deadline && !found {
            for msg in process.drain_async() {
                if let AsyncInbound::Event(PluginAsync::PanelUpdate { panel_id, widgets }) = msg {
                    if panel_id == "test.panel"
                        && widgets.iter().any(|w| {
                            matches!(
                                w,
                                ocs_plugin_api::panel::Widget::Label(s) if s == "async delivered"
                            )
                        })
                    {
                        found = true;
                    }
                }
            }
            if !found {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        assert!(found, "async event was not delivered to plugin");

        process.shutdown();
    }

    /// A plugin can send a `PluginAsync` while the host is waiting for a sync
    /// RPC response; the event is enqueued and available via `drain_async`.
    #[test]
    fn plugin_async_during_rpc() {
        let runner = cargo_build("ocs_plugin_runner", true);
        let plugin = cargo_build("test_v3_async_plugin", false);

        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", &runner);
        let mut host = DummyHost::new();
        let process = PluginProcess::spawn(&plugin, &mut host).expect("spawn plugin process");

        let handled = process
            .dispatch(&mut host, "SEND_ASYNC", &mut |_| {})
            .expect("dispatch succeeded");
        assert!(handled, "plugin should handle SEND_ASYNC");

        use ocs_plugin_api::process::AsyncInbound;

        // Poll for the async event; delivery is asynchronous and may arrive
        // shortly after the sync response.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut events = Vec::new();
        while std::time::Instant::now() < deadline && events.is_empty() {
            events = process.drain_async();
            if events.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        assert_eq!(events.len(), 1, "expected one async event");
        match &events[0] {
            AsyncInbound::Event(PluginAsync::PanelUpdate { panel_id, widgets }) => {
                assert_eq!(panel_id, "test.panel");
                assert_eq!(widgets.len(), 1);
                match &widgets[0] {
                    ocs_plugin_api::panel::Widget::Label(text) => {
                        assert_eq!(text, "async hello");
                    }
                    other => panic!("expected Label widget, got {other:?}"),
                }
            }
            other => panic!("expected PanelUpdate event, got {other:?}"),
        }
        assert_eq!(process.dropped_async_count(), 0);

        process.shutdown();
    }
}
