//! Stage 1 validation tests for the split `ocs_plugin_runner` binary.
//!
//! These tests build the runner and a minimal API v2 fixture plugin, then
//! exercise the full host→runner→plugin→runner→host spawn path.
//!
//! Run with the `host` feature enabled:
//!
//! ```text
//! $env:CARGO_TARGET_DIR='C:\tmp\ocs_target'
//! cargo test -p ocs_plugin_api --features host --test runner_spawns_plugin -- --nocapture
//! ```

#[cfg(feature = "host")]
mod stage1_tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use acadrust::xdata::ExtendedDataRecord;
    use acadrust::{CadDocument, EntityType, Handle};
    use ocs_plugin_api::host::{DocumentReader, HostApi, ReaderEntity};
    use ocs_plugin_api::process::PluginProcess;

    /// Locate the shared target directory used for all builds.
    fn target_dir() -> PathBuf {
        PathBuf::from(
            std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| r"C:\tmp\ocs_target".to_string()),
        )
    }

    /// Run `cargo build` for a workspace package and return the artifact path.
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
        } else if package == "test_v2_plugin"
            || package == "test_v2_hang_plugin"
            || package == "test_v3_hang_plugin"
        {
            let prefix = std::env::consts::DLL_PREFIX;
            let suffix = std::env::consts::DLL_SUFFIX;
            path.push(format!("{prefix}{package}{suffix}"));
        } else {
            panic!("unknown package {package}");
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

        fn infos(&self) -> Vec<String> {
            self.infos.lock().unwrap_or_else(|e| e.into_inner()).clone()
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

    /// Build the runner release binary and the host release binary, then verify the
    /// runner is significantly smaller because it does not include iced/wgpu.
    #[test]
    fn runner_binary_smaller() {
        let target = target_dir();
        let runner = cargo_build("ocs_plugin_runner", true);
        let host = {
            let mut cmd = Command::new("cargo");
            cmd.arg("build")
                .arg("--release")
                .env("CARGO_TARGET_DIR", &target);
            let status = cmd.status().expect("failed to build host");
            assert!(status.success(), "host release build failed");
            target
                .join("release")
                .join(format!("OpenCADStudio{}", std::env::consts::EXE_SUFFIX))
        };
        assert!(host.exists(), "host binary not found at {}", host.display());

        let runner_size = std::fs::metadata(&runner).unwrap().len();
        let host_size = std::fs::metadata(&host).unwrap().len();
        eprintln!(
            "runner size: {} bytes, host size: {} bytes",
            runner_size, host_size
        );
        // The runner should be at least an order of magnitude smaller than the
        // GUI host. Use a conservative ratio so the test is not flaky on small
        // absolute differences.
        assert!(
            runner_size * 10 < host_size,
            "runner binary ({runner_size} bytes) is not significantly smaller than host ({host_size} bytes)"
        );
    }

    /// Build the runner and a minimal v2 plugin, spawn the plugin process, and
    /// verify the handshake plus GetManifest/GetRibbon succeed.
    #[test]
    fn runner_spawns_plugin() {
        let runner = cargo_build("ocs_plugin_runner", true);
        let plugin = cargo_build("test_v2_plugin", false);

        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", &runner);
        let mut host = DummyHost::new();
        let process = PluginProcess::spawn(&plugin, &mut host).expect("spawn plugin process");

        assert_eq!(process.id(), "ocs.test.v2_plugin");
        let manifest = process.manifest();
        assert_eq!(manifest.name, "Test V2 Plugin");
        assert_eq!(manifest.api_version, 2);

        let ribbon = process.ribbon();
        assert_eq!(ribbon.len(), 1);
        assert_eq!(ribbon[0].title, "V2 Test");
        assert_eq!(ribbon[0].tools.len(), 1);

        process.shutdown();
    }

    /// Extend `runner_spawns_plugin` by dispatching a command and then shutting the
    /// plugin process down cleanly.
    #[test]
    fn v2_plugin_init_close() {
        let runner = cargo_build("ocs_plugin_runner", true);
        let plugin = cargo_build("test_v2_plugin", false);

        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", &runner);
        let mut host = DummyHost::new();
        let process = PluginProcess::spawn(&plugin, &mut host).expect("spawn plugin process");

        let handled = process
            .dispatch(&mut host, "V2TEST_HELLO", &mut |_| {})
            .expect("dispatch succeeded");
        assert!(handled, "plugin should handle V2TEST_HELLO");
        assert_eq!(host.infos(), vec!["hello from v2 plugin"]);

        assert!(process.is_alive(), "process should be alive after dispatch");
        process.shutdown();

        // Give the OS a moment to reap the child, then assert it is no longer alive.
        let deadline = Instant::now() + Duration::from_secs(5);
        while process.is_alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!process.is_alive(), "process should be dead after shutdown");
    }

    /// A malfunctioning API v2 plugin that never returns from dispatch must not
    /// block the host for more than one second.
    #[test]
    fn v2_plugin_hang_times_out() {
        let runner = cargo_build("ocs_plugin_runner", true);
        let plugin = cargo_build("test_v2_hang_plugin", false);

        // Force a 1 s Dispatch timeout for this test.
        let prev_timeout = std::env::var("OCS_PLUGIN_CALL_TIMEOUT_SECS").ok();
        let prev_floor = std::env::var("OCS_PLUGIN_TEST_FLOOR_SECS").ok();
        std::env::set_var("OCS_PLUGIN_CALL_TIMEOUT_SECS", "1");
        std::env::set_var("OCS_PLUGIN_TEST_FLOOR_SECS", "0");

        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", &runner);
        let mut host = DummyHost::new();
        let process = PluginProcess::spawn(&plugin, &mut host).expect("spawn plugin process");

        let start = Instant::now();
        let result = process.dispatch(&mut host, "V2TEST_HANG", &mut |_| {});
        let elapsed = start.elapsed();

        // Restore environment before assertions so other tests are not affected.
        match prev_timeout {
            Some(v) => std::env::set_var("OCS_PLUGIN_CALL_TIMEOUT_SECS", v),
            None => std::env::remove_var("OCS_PLUGIN_CALL_TIMEOUT_SECS"),
        }
        match prev_floor {
            Some(v) => std::env::set_var("OCS_PLUGIN_TEST_FLOOR_SECS", v),
            None => std::env::remove_var("OCS_PLUGIN_TEST_FLOOR_SECS"),
        }

        assert!(
            matches!(
                result,
                Err(ocs_plugin_api::process::PluginError::CallTimeout {
                    request: "Dispatch",
                    ..
                })
            ),
            "expected Dispatch timeout, got {result:?}"
        );
        assert!(
            elapsed >= Duration::from_secs(1),
            "timeout fired too quickly: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "host blocked for too long: {elapsed:?}"
        );
        assert!(
            !process.is_alive(),
            "process should be marked dead after timeout"
        );
    }

    /// A malfunctioning API v3 plugin that never returns from dispatch must not
    /// block the host for more than one second.
    #[test]
    fn v3_plugin_hang_times_out() {
        let runner = cargo_build("ocs_plugin_runner", true);
        let plugin = cargo_build("test_v3_hang_plugin", false);

        let prev_timeout = std::env::var("OCS_PLUGIN_CALL_TIMEOUT_SECS").ok();
        let prev_floor = std::env::var("OCS_PLUGIN_TEST_FLOOR_SECS").ok();
        std::env::set_var("OCS_PLUGIN_CALL_TIMEOUT_SECS", "1");
        std::env::set_var("OCS_PLUGIN_TEST_FLOOR_SECS", "0");

        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", &runner);
        let mut host = DummyHost::new();
        let process = PluginProcess::spawn(&plugin, &mut host).expect("spawn plugin process");

        let start = Instant::now();
        let result = process.dispatch(&mut host, "HANG", &mut |_| {});
        let elapsed = start.elapsed();

        match prev_timeout {
            Some(v) => std::env::set_var("OCS_PLUGIN_CALL_TIMEOUT_SECS", v),
            None => std::env::remove_var("OCS_PLUGIN_CALL_TIMEOUT_SECS"),
        }
        match prev_floor {
            Some(v) => std::env::set_var("OCS_PLUGIN_TEST_FLOOR_SECS", v),
            None => std::env::remove_var("OCS_PLUGIN_TEST_FLOOR_SECS"),
        }

        assert!(
            matches!(
                result,
                Err(ocs_plugin_api::process::PluginError::CallTimeout {
                    request: "Dispatch",
                    ..
                })
            ),
            "expected Dispatch timeout, got {result:?}"
        );
        assert!(
            elapsed >= Duration::from_secs(1),
            "timeout fired too quickly: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "host blocked for too long: {elapsed:?}"
        );
        assert!(
            !process.is_alive(),
            "process should be marked dead after timeout"
        );
    }
}
