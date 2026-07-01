//! End-to-end integration tests for the API v3 panel template plugin.
//!
//! These tests build the `ocs_plugin_runner` binary and the template cdylib,
//! spawn the plugin process, and verify every RPC and async communication step.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;
use std::time::Duration;

use acadrust::xdata::ExtendedDataRecord;
use acadrust::{CadDocument, EntityType, Handle};
use ocs_plugin_api::host::{DocumentReader, HostApi, ReaderEntity};
use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync};
use ocs_plugin_api::panel::{DockZone, PanelDef, PanelError, PanelEvent, PanelHandle, Widget};
use ocs_plugin_api::process::{AsyncInbound, PluginProcess};
use ocs_plugin_api::shm::DocumentViewInfo;

/// Directory where Cargo places build artifacts.
fn target_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        PathBuf::from(dir)
    } else {
        // `CARGO_MANIFEST_DIR` is `crates/plugin-template-api3`; the workspace
        // target directory lives two levels up.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
    }
}

/// Build a workspace package and return the path to its artifact.
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
        // Cargo normalizes hyphens in package names to underscores for cdylib
        // artifact file names.
        let lib_name = package.replace('-', "_");
        path.push(format!("{prefix}{lib_name}{suffix}"));
    }
    assert!(path.exists(), "artifact not found at {}", path.display());
    path
}

/// Path to the built template cdylib.
fn plugin_dll() -> PathBuf {
    cargo_build("plugin-template-api3", false)
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

#[derive(Default)]
struct RecordingHost {
    opened: Vec<PanelDef>,
    infos: Vec<String>,
    outputs: Vec<String>,
    errors: Vec<String>,
    dirty: bool,
    undos: Vec<String>,
}

impl HostApi for RecordingHost {
    fn tab_index(&self) -> usize {
        0
    }
    fn document(&self) -> &CadDocument {
        unimplemented!()
    }
    fn document_mut(&mut self) -> &mut CadDocument {
        unimplemented!()
    }
    fn add_entity(&mut self, _e: EntityType) -> Handle {
        unimplemented!()
    }
    fn bump_geometry(&mut self) {}
    fn read_record(&self, _h: Handle, _app: &str) -> Option<&ExtendedDataRecord> {
        None
    }
    fn write_record(&mut self, _h: Handle, _r: ExtendedDataRecord) -> bool {
        false
    }
    fn remove_record(&mut self, _h: Handle, _app: &str) -> bool {
        false
    }
    fn push_undo(&mut self, label: &str) {
        self.undos.push(label.to_string());
    }
    fn set_dirty(&mut self) {
        self.dirty = true;
    }
    fn push_info(&mut self, msg: &str) {
        self.infos.push(msg.to_string());
    }
    fn push_output(&mut self, msg: &str) {
        self.outputs.push(msg.to_string());
    }
    fn push_error(&mut self, msg: &str) {
        self.errors.push(msg.to_string());
    }
    fn start_interactive(&mut self, _c: Box<dyn ocs_plugin_api::host::InteractiveCommand>) {}
    fn open_panel(&mut self, def: &PanelDef) -> Result<PanelHandle, PanelError> {
        self.opened.push(def.clone());
        Ok(PanelHandle(42))
    }
    fn close_panel(&mut self, _h: PanelHandle) -> Result<(), PanelError> {
        Ok(())
    }
    fn move_panel(&mut self, _h: PanelHandle, _x: f32, _y: f32) -> Result<(), PanelError> {
        Ok(())
    }
    fn resize_panel(
        &mut self,
        _h: PanelHandle,
        _width: f32,
        _height: f32,
    ) -> Result<(), PanelError> {
        Ok(())
    }
    fn dock_panel(&mut self, _handle: PanelHandle, _zone: DockZone) -> Result<(), PanelError> {
        Ok(())
    }
    fn undock_panel(&mut self, _handle: PanelHandle, _x: f32, _y: f32) -> Result<(), PanelError> {
        Ok(())
    }
    fn post_panel_event(&mut self, _h: PanelHandle, _e: PanelEvent) -> Result<(), PanelError> {
        Ok(())
    }
    fn document_reader(&self) -> Box<dyn DocumentReader + '_> {
        Box::new(EmptyReader)
    }
    fn document_view(&mut self) -> Option<DocumentViewInfo> {
        None
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

fn drain_async_inbound(process: &PluginProcess) -> Vec<AsyncInbound> {
    let deadline = std::time::Instant::now() + Duration::from_millis(2000);
    let mut all = Vec::new();
    while std::time::Instant::now() < deadline {
        let msgs = process.drain_async();
        if !msgs.is_empty() {
            all.extend(msgs);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    all
}

fn panel_updates(msgs: &[AsyncInbound]) -> Vec<Vec<Widget>> {
    msgs.iter()
        .filter_map(|e| match e {
            AsyncInbound::Event(PluginAsync::PanelUpdate { panel_id, widgets })
                if panel_id == "api3_panel" =>
            {
                Some(widgets.clone())
            }
            _ => None,
        })
        .collect()
}

/// Format the async message stream for test-failure debugging.
fn format_async(msgs: &[AsyncInbound]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    writeln!(s, "=== async messages ({} total) ===", msgs.len()).unwrap();
    for (i, m) in msgs.iter().enumerate() {
        match m {
            AsyncInbound::Event(PluginAsync::PanelUpdate { panel_id, widgets }) => {
                writeln!(s, "{i}: PanelUpdate({panel_id}, {} widgets)", widgets.len()).unwrap();
            }
            AsyncInbound::Event(PluginAsync::PanelClosed { panel_id }) => {
                writeln!(s, "{i}: PanelClosed({panel_id})").unwrap();
            }
            AsyncInbound::Request(req) => {
                writeln!(s, "{i}: Request({req:?})").unwrap();
            }
        }
    }
    s
}

/// Format the extracted panel update stream for test-failure debugging.
fn format_updates(updates: &[Vec<Widget>]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    writeln!(s, "=== panel updates ({} total) ===", updates.len()).unwrap();
    for (i, widgets) in updates.iter().enumerate() {
        writeln!(s, "--- update {i} ({} widgets) ---", widgets.len()).unwrap();
        for w in widgets {
            match w {
                Widget::Label(v) => writeln!(s, "  Label({v})").unwrap(),
                Widget::Button { id, label } => writeln!(s, "  Button({id}, {label})").unwrap(),
                Widget::TextInput { id, value } => {
                    writeln!(s, "  TextInput({id}, {value})").unwrap()
                }
                Widget::MultilineOutput { id, lines } => {
                    writeln!(s, "  MultilineOutput({id}, {lines:?})").unwrap()
                }
                Widget::List { id, items } => writeln!(s, "  List({id}, {items:?})").unwrap(),
            }
        }
    }
    s
}

static INIT_RUNNER: Once = Once::new();

fn ensure_runner_env() {
    INIT_RUNNER.call_once(|| {
        let runner = cargo_build("ocs_plugin_runner", false);
        std::env::set_var("OCS_PLUGIN_RUNNER_EXE", &runner);
    });
}

fn spawn_with_runner() -> (PluginProcess, RecordingHost) {
    ensure_runner_env();
    let plugin = plugin_dll();
    let mut host = RecordingHost::default();
    let process = PluginProcess::spawn(&plugin, &mut host).unwrap();
    (process, host)
}

#[test]
fn manifest_and_panels_declared() {
    let (process, _host) = spawn_with_runner();

    assert_eq!(process.manifest().id, "ocs.template.api3");
    assert_eq!(process.panels().len(), 1);
    assert_eq!(process.panels()[0].id, "api3_panel");

    process.shutdown();
}

#[test]
fn plugin_toml_matches_compiled_manifest() {
    use plugin_template_api3::MANIFEST;

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let toml_text = std::fs::read_to_string(manifest_dir.join("plugin.toml"))
        .expect("plugin.toml should exist");
    let table: toml::Table = toml_text.parse().expect("plugin.toml should be valid TOML");

    let plugin = table
        .get("plugin")
        .expect("plugin section")
        .as_table()
        .unwrap();
    let opencad = table
        .get("opencad")
        .expect("opencad section")
        .as_table()
        .unwrap();

    assert_eq!(plugin.get("id").unwrap().as_str().unwrap(), MANIFEST.id);
    assert_eq!(plugin.get("name").unwrap().as_str().unwrap(), MANIFEST.name);
    assert_eq!(
        plugin.get("version").unwrap().as_str().unwrap(),
        MANIFEST.version
    );
    assert_eq!(
        plugin.get("description").unwrap().as_str().unwrap(),
        MANIFEST.description
    );

    assert_eq!(
        opencad.get("api_version").unwrap().as_integer().unwrap() as u32,
        MANIFEST.api_version.major
    );
    assert_eq!(
        opencad.get("ribbon_order").unwrap().as_integer().unwrap() as i32,
        MANIFEST.ribbon_order
    );

    let toml_prefixes: Vec<&str> = opencad
        .get("command_prefixes")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(toml_prefixes, MANIFEST.command_prefixes);

    let toml_apps: Vec<&str> = opencad
        .get("xdata_apps")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(toml_apps, MANIFEST.xdata_apps);
}

#[test]
fn button_click_reaches_plugin_and_updates_label() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Clicked("inc".to_string()),
        })
        .unwrap();
    let updates = panel_updates(&drain_async_inbound(&process));
    let label = updates
        .last()
        .unwrap()
        .iter()
        .find_map(|w| match w {
            Widget::Button { id, label } if id == "inc" => Some(label.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        label.contains("Clicked 1 times"),
        "label: {label}; {}",
        format_updates(&updates)
    );

    process.shutdown();
}

#[test]
fn text_input_change_is_reflected() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::TextChanged {
                id: "input".to_string(),
                value: "hello world".to_string(),
            },
        })
        .unwrap();
    let updates = panel_updates(&drain_async_inbound(&process));
    let has_log = updates.last().unwrap().iter().any(|w| match w {
        Widget::MultilineOutput { lines, .. } => lines
            .iter()
            .any(|l| l.contains("Input changed: hello world")),
        _ => false,
    });
    assert!(
        has_log,
        "missing input-change log; {}",
        format_updates(&updates)
    );

    process.shutdown();
}

#[test]
fn list_selection_is_reflected() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::ItemSelected {
                id: "list".to_string(),
                index: 2,
            },
        })
        .unwrap();
    let msgs = drain_async_inbound(&process);
    let updates = panel_updates(&msgs);
    let has_log = updates.last().unwrap().iter().any(|w| match w {
        Widget::MultilineOutput { lines, .. } => {
            lines.iter().any(|l| l.contains("List item selected: 2"))
        }
        _ => false,
    });
    assert!(
        has_log,
        "missing list-selection log; {}",
        format_updates(&updates)
    );

    let list_has_selection = updates.last().unwrap().iter().any(|w| match w {
        Widget::List { items, .. } => items.iter().any(|i| i.contains("> Item 2")),
        _ => false,
    });
    assert!(
        list_has_selection,
        "selected item should be highlighted; {}",
        format_updates(&updates)
    );

    let requests: Vec<_> = msgs
        .iter()
        .filter_map(|m| match m {
            AsyncInbound::Request(ocs_plugin_api::ipc::protocol::PluginRequest::PushOutput(
                msg,
            )) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert!(
        requests.iter().any(|m| m.contains("List item selected: 2")),
        "missing push_output for list selection; requests: {requests:?}\n{}",
        format_async(&msgs)
    );

    process.shutdown();
}

#[test]
fn document_lifecycle_events_are_logged() {
    let (process, _host) = spawn_with_runner();

    process
        .send_async(HostAsync::DocumentActivated { tab: 3 })
        .unwrap();
    process
        .send_async(HostAsync::DocumentChanged { tab: 3, version: 7 })
        .unwrap();
    process.send_async(HostAsync::TabClosed { tab: 3 }).unwrap();
    let has_logs = |updates: &[Vec<Widget>], text: &str| {
        updates.iter().any(|widgets| {
            widgets.iter().any(|w| match w {
                Widget::MultilineOutput { lines, .. } => lines.iter().any(|l| l.contains(text)),
                _ => false,
            })
        })
    };
    let updates = panel_updates(&drain_async_inbound(&process));
    assert!(
        has_logs(&updates, "DocumentActivated tab=3"),
        "missing DocumentActivated log; updates: {updates:?}"
    );
    assert!(
        has_logs(&updates, "DocumentChanged tab=3 version=7"),
        "missing DocumentChanged log; updates: {updates:?}"
    );
    assert!(
        has_logs(&updates, "TabClosed tab=3"),
        "missing TabClosed log; updates: {updates:?}"
    );

    process.shutdown();
}

#[test]
fn panel_close_outputs_message() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();
    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Closed,
        })
        .unwrap();
    let updates = panel_updates(&drain_async_inbound(&process));
    let has_close_log = updates.iter().any(|widgets| {
        widgets.iter().any(|w| match w {
            Widget::MultilineOutput { lines, .. } => {
                lines.iter().any(|l| l.contains("API3 panel closed"))
            }
            _ => false,
        })
    });
    assert!(
        has_close_log,
        "missing panel closed log; updates: {updates:?}"
    );

    process.shutdown();
}

#[test]
fn text_input_reaches_command_line() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::TextChanged {
                id: "input".to_string(),
                value: "hello world".to_string(),
            },
        })
        .unwrap();
    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Clicked("send_cmd".to_string()),
        })
        .unwrap();

    let msgs = drain_async_inbound(&process);
    let updates = panel_updates(&msgs);
    let has_log = updates.iter().any(|widgets| {
        widgets.iter().any(|w| match w {
            Widget::MultilineOutput { lines, .. } => {
                lines.iter().any(|l| l.contains("Sent to CMD: hello world"))
            }
            _ => false,
        })
    });
    assert!(has_log, "missing 'Sent to CMD' log; updates: {updates:?}");

    let requests: Vec<_> = msgs
        .iter()
        .filter_map(|m| match m {
            AsyncInbound::Request(ocs_plugin_api::ipc::protocol::PluginRequest::PushOutput(
                msg,
            )) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert!(
        requests.contains(&"hello world".to_string()),
        "push_output request not found; requests: {requests:?}"
    );

    process.shutdown();
}

#[test]
fn coordinate_pick_round_trip() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Clicked("pick_point".to_string()),
        })
        .unwrap();

    // Wait for the plugin to request a point pick from the host.
    let deadline = std::time::Instant::now() + Duration::from_millis(2000);
    let mut found_request = false;
    while std::time::Instant::now() < deadline && !found_request {
        for msg in process.drain_async() {
            if let AsyncInbound::Request(
                ocs_plugin_api::ipc::protocol::PluginRequest::RequestPointPick { panel_id },
            ) = msg
            {
                assert_eq!(panel_id, "api3_panel");
                found_request = true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(found_request, "plugin did not send RequestPointPick");

    // Simulate the host delivering a picked coordinate back to the plugin.
    process
        .send_async(HostAsync::CoordinatesPicked {
            panel_id: "api3_panel".to_string(),
            point: [1.0, 2.0, 3.0],
        })
        .unwrap();

    let msgs = drain_async_inbound(&process);
    let updates = panel_updates(&msgs);
    let has_log = updates.iter().any(|widgets| {
        widgets.iter().any(|w| match w {
            Widget::MultilineOutput { lines, .. } => lines
                .iter()
                .any(|l| l.contains("Picked point: 1.000, 2.000, 3.000")),
            _ => false,
        })
    });
    assert!(has_log, "missing picked point log; updates: {updates:?}");

    let requests: Vec<_> = msgs
        .iter()
        .filter_map(|m| match m {
            AsyncInbound::Request(ocs_plugin_api::ipc::protocol::PluginRequest::PushOutput(
                msg,
            )) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert!(
        requests
            .iter()
            .any(|m| m.contains("Picked point: 1.000, 2.000, 3.000")),
        "missing push_output for picked point; requests: {requests:?}"
    );

    process.shutdown();
}

#[test]
fn add_point_picks_and_creates_entity() {
    use ocs_plugin_api::ipc::protocol::PluginRequest;
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Clicked("add_point".to_string()),
        })
        .unwrap();

    // Wait for the plugin to request a point pick from the host.
    let deadline = std::time::Instant::now() + Duration::from_millis(2000);
    let mut found_request = false;
    while std::time::Instant::now() < deadline && !found_request {
        for msg in process.drain_async() {
            if let AsyncInbound::Request(PluginRequest::RequestPointPick { panel_id }) = msg {
                assert_eq!(panel_id, "api3_panel");
                found_request = true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(found_request, "plugin did not send RequestPointPick for add_point");

    // Simulate the host delivering a picked coordinate back to the plugin.
    process
        .send_async(HostAsync::CoordinatesPicked {
            panel_id: "api3_panel".to_string(),
            point: [4.0, 5.0, 6.0],
        })
        .unwrap();

    // The plugin should now add a point at the picked coordinate.
    let deadline = std::time::Instant::now() + Duration::from_millis(2000);
    let mut found_add = false;
    while std::time::Instant::now() < deadline && !found_add {
        for msg in process.drain_async() {
            if let AsyncInbound::Request(PluginRequest::AddEntity(acadrust::EntityType::Point(p))) = msg {
                assert!((p.location.x - 4.0).abs() < 1e-9);
                assert!((p.location.y - 5.0).abs() < 1e-9);
                assert!((p.location.z - 6.0).abs() < 1e-9);
                found_add = true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(found_add, "plugin did not send AddEntity(Point) after picking");

    process.shutdown();
}

#[test]
fn async_push_info_from_event() {
    let (process, _host) = spawn_with_runner();

    process
        .send_async(HostAsync::DocumentActivated { tab: 5 })
        .unwrap();

    // The plugin both pushes info to the host and updates its panel log.
    let msgs = drain_async_inbound(&process);
    let requests: Vec<_> = msgs
        .iter()
        .filter_map(|m| match m {
            AsyncInbound::Request(ocs_plugin_api::ipc::protocol::PluginRequest::PushInfo(msg)) => {
                Some(msg.clone())
            }
            _ => None,
        })
        .collect();
    assert!(
        requests
            .iter()
            .any(|m| m.contains("DocumentActivated tab=5")),
        "async push_info request not found; requests: {requests:?}"
    );

    let updates = panel_updates(&msgs);
    let has_log = updates.iter().any(|widgets| {
        widgets.iter().any(|w| match w {
            Widget::MultilineOutput { lines, .. } => {
                lines.iter().any(|l| l.contains("DocumentActivated tab=5"))
            }
            _ => false,
        })
    });
    assert!(
        has_log,
        "missing DocumentActivated panel log; updates: {updates:?}"
    );

    process.shutdown();
}

#[test]
fn panel_move_resize_events_are_logged() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Moved { x: 150.0, y: 200.0 },
        })
        .unwrap();
    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Resized {
                width: 300.0,
                height: 500.0,
            },
        })
        .unwrap();

    let updates = panel_updates(&drain_async_inbound(&process));
    let has_log = |text: &str| {
        updates.iter().any(|widgets| {
            widgets.iter().any(|w| match w {
                Widget::MultilineOutput { lines, .. } => lines.iter().any(|l| l.contains(text)),
                _ => false,
            })
        })
    };
    assert!(
        has_log("Panel moved: x=150.0, y=200.0"),
        "missing move log; {}",
        format_updates(&updates)
    );
    assert!(
        has_log("Panel resized: 300.0 x 500.0"),
        "missing resize log; {}",
        format_updates(&updates)
    );

    process.shutdown();
}

#[test]
fn panel_dock_undock_events_are_logged() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Docked {
                zone: DockZone::Left,
            },
        })
        .unwrap();
    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Undocked,
        })
        .unwrap();

    let updates = panel_updates(&drain_async_inbound(&process));
    let has_log = |text: &str| {
        updates.iter().any(|widgets| {
            widgets.iter().any(|w| match w {
                Widget::MultilineOutput { lines, .. } => lines.iter().any(|l| l.contains(text)),
                _ => false,
            })
        })
    };
    assert!(
        has_log("Panel docked: Left"),
        "missing docked log; {}",
        format_updates(&updates)
    );
    assert!(
        has_log("Panel undocked"),
        "missing undocked log; {}",
        format_updates(&updates)
    );

    process.shutdown();
}

#[test]
fn panel_focus_event_is_logged() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Focused,
        })
        .unwrap();

    let updates = panel_updates(&drain_async_inbound(&process));
    let has_log = updates.iter().any(|widgets| {
        widgets.iter().any(|w| match w {
            Widget::MultilineOutput { lines, .. } => {
                lines.iter().any(|l| l.contains("Panel focused"))
            }
            _ => false,
        })
    });
    assert!(has_log, "missing focus log; updates: {updates:?}");

    process.shutdown();
}

#[test]
fn dock_left_button_sends_dock_request() {
    let (process, _host) = spawn_with_runner();

    process
        .dispatch(&mut RecordingHost::default(), "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Clicked("dock_left".to_string()),
        })
        .unwrap();

    let requests: Vec<_> = drain_async_inbound(&process)
        .into_iter()
        .filter_map(|m| match m {
            AsyncInbound::Request(ocs_plugin_api::ipc::protocol::PluginRequest::DockPanel {
                handle,
                zone,
            }) => Some((handle, zone)),
            _ => None,
        })
        .collect();
    assert_eq!(requests, vec![(PanelHandle(42), DockZone::Left)]);

    process.shutdown();
}

#[test]
fn undock_button_sends_undock_request() {
    let (process, _host) = spawn_with_runner();

    process
        .dispatch(&mut RecordingHost::default(), "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Clicked("undock".to_string()),
        })
        .unwrap();

    let requests: Vec<_> =
        drain_async_inbound(&process)
            .into_iter()
            .filter_map(|m| match m {
                AsyncInbound::Request(
                    ocs_plugin_api::ipc::protocol::PluginRequest::UndockPanel { handle, x, y },
                ) => Some((handle, x, y)),
                _ => None,
            })
            .collect();
    assert_eq!(requests, vec![(PanelHandle(42), 120.0, 80.0)]);

    process.shutdown();
}

#[test]
fn host_focus_event_signals_active_tab() {
    let (process, mut host) = spawn_with_runner();

    process
        .dispatch(&mut host, "API3_OPEN", &mut |_| {})
        .unwrap();

    process
        .send_async(HostAsync::PanelEvent {
            panel_id: "api3_panel".to_string(),
            event: PanelEvent::Focused,
        })
        .unwrap();

    let updates = panel_updates(&drain_async_inbound(&process));
    let has_focus_log = updates.iter().any(|widgets| {
        widgets.iter().any(|w| match w {
            Widget::MultilineOutput { lines, .. } => {
                lines.iter().any(|l| l.contains("Panel focused"))
            }
            _ => false,
        })
    });
    assert!(
        has_focus_log,
        "Focused event should be logged as active tab; updates: {updates:?}"
    );

    process.shutdown();
}
