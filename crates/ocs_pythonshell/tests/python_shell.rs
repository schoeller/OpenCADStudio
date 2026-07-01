//! Integration tests for the Python shell plugin.
//!
//! The tests exercise the plugin by constructing it directly and driving it
//! through a mock [`HostApi`].  A real Python interpreter is required; if none
//! is found the tests skip gracefully.

use std::collections::HashMap;
use std::sync::Mutex;

use acadrust::xdata::ExtendedDataRecord;
use acadrust::{CadDocument, EntityType, Handle};
use ocs_plugin_api::host::{BuiltinPlugin, DocumentReader, HostApi, ReaderEntity};
use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync};
use ocs_plugin_api::panel::{PanelEvent, Widget};
use ocs_pythonshell::PythonShellPlugin;

const PANEL_ID: &str = "python_repl";

/// Serialize access to environment variables so tests that manipulate
/// `OCS_PYTHON_EXE` do not race with tests that rely on Python discovery.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

struct MockEntity {
    handle: Handle,
    kind: ocs_plugin_api::host::ReaderEntityKind,
    layer_name: String,
    point: Option<ocs_plugin_api::host::ReaderPoint>,
}

struct MockReader<'a> {
    entities: &'a [MockEntity],
    layers: &'a [(Handle, String)],
}

impl<'a> DocumentReader for MockReader<'a> {
    fn entity_count(&self) -> usize {
        self.entities.len()
    }

    fn for_each_entity(&self, f: &mut dyn FnMut(ReaderEntity<'_>)) {
        for e in self.entities {
            f(ReaderEntity {
                handle: e.handle,
                kind: e.kind,
                layer_name: &e.layer_name,
                point: e.point,
            });
        }
    }

    fn layer_name(&self, handle: Handle) -> Option<&str> {
        self.layers
            .iter()
            .find(|(h, _)| *h == handle)
            .map(|(_, n)| n.as_str())
    }

    fn app_id_name(&self, _handle: Handle) -> Option<&str> {
        None
    }
}

struct MockHost {
    doc: CadDocument,
    reader_entities: Vec<MockEntity>,
    reader_layers: Vec<(Handle, String)>,
    panels_opened: Vec<ocs_plugin_api::panel::PanelDef>,
    async_events: Vec<PluginAsync>,
    infos: Vec<String>,
    errors: Vec<String>,
    added_entities: Vec<EntityType>,
    next_handle: u64,
    records: HashMap<(Handle, String), ExtendedDataRecord>,
}

impl MockHost {
    fn new() -> Self {
        Self {
            doc: CadDocument::default(),
            reader_entities: Vec::new(),
            reader_layers: Vec::new(),
            panels_opened: Vec::new(),
            async_events: Vec::new(),
            infos: Vec::new(),
            errors: Vec::new(),
            added_entities: Vec::new(),
            next_handle: 1,
            records: HashMap::new(),
        }
    }

    fn with_point(mut self, x: f64, y: f64, z: f64, layer: &str) -> Self {
        let handle = Handle::new(self.next_handle);
        self.next_handle += 1;
        self.reader_entities.push(MockEntity {
            handle,
            kind: ocs_plugin_api::host::ReaderEntityKind::Point,
            layer_name: layer.to_string(),
            point: Some(ocs_plugin_api::host::ReaderPoint { x, y, z }),
        });
        self
    }
}

impl HostApi for MockHost {
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
        Box::new(MockReader {
            entities: &self.reader_entities,
            layers: &self.reader_layers,
        })
    }

    fn add_entity(&mut self, entity: EntityType) -> Handle {
        let handle = Handle::new(self.next_handle);
        self.next_handle += 1;
        self.added_entities.push(entity);
        handle
    }

    fn bump_geometry(&mut self) {}

    fn read_record(&self, handle: Handle, app_name: &str) -> Option<&ExtendedDataRecord> {
        self.records.get(&(handle, app_name.to_string()))
    }

    fn write_record(&mut self, handle: Handle, record: ExtendedDataRecord) -> bool {
        let key = (handle, record.application_name.clone());
        self.records.insert(key, record);
        true
    }

    fn remove_record(&mut self, handle: Handle, app_name: &str) -> bool {
        self.records
            .remove(&(handle, app_name.to_string()))
            .is_some()
    }

    fn push_undo(&mut self, _label: &str) {}

    fn set_dirty(&mut self) {}

    fn push_info(&mut self, msg: &str) {
        self.infos.push(msg.to_string());
    }

    fn push_output(&mut self, _msg: &str) {}

    fn push_error(&mut self, msg: &str) {
        self.errors.push(msg.to_string());
    }

    fn start_interactive(&mut self, _command: Box<dyn ocs_plugin_api::host::InteractiveCommand>) {}

    fn open_panel(
        &mut self,
        def: &ocs_plugin_api::panel::PanelDef,
    ) -> Result<ocs_plugin_api::panel::PanelHandle, ocs_plugin_api::panel::PanelError> {
        self.panels_opened.push(def.clone());
        Ok(ocs_plugin_api::panel::PanelHandle(1))
    }

    fn close_panel(
        &mut self,
        _handle: ocs_plugin_api::panel::PanelHandle,
    ) -> Result<(), ocs_plugin_api::panel::PanelError> {
        Ok(())
    }

    fn post_panel_event(
        &mut self,
        _handle: ocs_plugin_api::panel::PanelHandle,
        _event: PanelEvent,
    ) -> Result<(), ocs_plugin_api::panel::PanelError> {
        Ok(())
    }

    fn send_async(&mut self, event: PluginAsync) {
        self.async_events.push(event);
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

fn output_contains(events: &[PluginAsync], needle: &str) -> bool {
    events.iter().any(|e| {
        if let PluginAsync::PanelUpdate { widgets, .. } = e {
            widgets.iter().any(|w| {
                if let Widget::MultilineOutput { lines, .. } = w {
                    lines.iter().any(|l| l.contains(needle))
                } else {
                    false
                }
            })
        } else {
            false
        }
    })
}

fn output_lines(events: &[PluginAsync]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| {
            if let PluginAsync::PanelUpdate { widgets, .. } = e {
                widgets.iter().find_map(|w| {
                    if let Widget::MultilineOutput { lines, .. } = w {
                        Some(lines.clone())
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        })
        .last()
        .unwrap_or_default()
}

fn send_input(plugin: &mut PythonShellPlugin, value: &str) {
    plugin.on_async_event(
        &mut MockHost::new(),
        HostAsync::PanelEvent {
            panel_id: PANEL_ID.to_string(),
            event: PanelEvent::TextChanged {
                id: "py_input".to_string(),
                value: value.to_string(),
            },
        },
    );
}

fn click_run(plugin: &mut PythonShellPlugin, host: &mut MockHost) {
    plugin.on_async_event(
        host,
        HostAsync::PanelEvent {
            panel_id: PANEL_ID.to_string(),
            event: PanelEvent::Clicked("py_run".to_string()),
        },
    );
}

fn click_clear(plugin: &mut PythonShellPlugin, host: &mut MockHost) {
    plugin.on_async_event(
        host,
        HostAsync::PanelEvent {
            panel_id: PANEL_ID.to_string(),
            event: PanelEvent::Clicked("py_clear".to_string()),
        },
    );
}

#[test]
fn python_shell_eval() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if !ocs_pythonshell::python_available() {
        eprintln!("Python interpreter not found, skipping python_shell_eval");
        return;
    }

    let mut host = MockHost::new();
    let mut plugin = PythonShellPlugin::new();

    assert!(plugin.dispatch(&mut host, "PY_OPEN_SHELL"));
    assert_eq!(host.panels_opened.len(), 1);
    assert!(host
        .async_events
        .iter()
        .any(|e| matches!(e, PluginAsync::PanelUpdate { .. })));

    send_input(&mut plugin, "2+3");
    click_run(&mut plugin, &mut host);

    assert!(
        output_contains(&host.async_events, "5"),
        "expected Python output containing 5, got {:?}",
        host.async_events
    );
}

#[test]
fn python_host_api_call() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if !ocs_pythonshell::python_available() {
        eprintln!("Python interpreter not found, skipping python_host_api_call");
        return;
    }

    let mut host = MockHost::new();
    let mut plugin = PythonShellPlugin::new();

    assert!(plugin.dispatch(&mut host, "PY_OPEN_SHELL"));
    send_input(
        &mut plugin,
        "print('A'); import ocs; ocs.push_info('hello from python'); print('C')",
    );
    click_run(&mut plugin, &mut host);

    assert!(
        host.infos.iter().any(|s| s.contains("hello from python")),
        "expected push_info to reach host, got infos {:?}",
        host.infos
    );
}

#[test]
fn python_crash_isolated() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if !ocs_pythonshell::python_available() {
        eprintln!("Python interpreter not found, skipping python_crash_isolated");
        return;
    }

    let mut host = MockHost::new();
    let mut plugin = PythonShellPlugin::new();

    assert!(plugin.dispatch(&mut host, "PY_OPEN_SHELL"));
    // Abruptly kill the Python worker process.
    send_input(&mut plugin, "import os; os._exit(1)");
    click_run(&mut plugin, &mut host);

    assert!(
        host.async_events
            .iter()
            .any(|e| matches!(e, PluginAsync::PanelClosed { panel_id } if panel_id == PANEL_ID)),
        "expected PanelClosed after Python crash, got {:?}",
        host.async_events
    );
}

#[test]
fn python_reads_entities() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if !ocs_pythonshell::python_available() {
        eprintln!("Python interpreter not found, skipping python_reads_entities");
        return;
    }

    let mut host = MockHost::new().with_point(1.0, 2.0, 0.0, "test_layer");
    let mut plugin = PythonShellPlugin::new();

    assert!(plugin.dispatch(&mut host, "PY_OPEN_SHELL"));
    send_input(&mut plugin, "ocs.doc.entities()");
    click_run(&mut plugin, &mut host);

    assert!(
        output_contains(&host.async_events, "test_layer"),
        "expected output to contain layer name, got {:?}",
        host.async_events
    );
}

#[test]
fn python_adds_entities() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if !ocs_pythonshell::python_available() {
        eprintln!("Python interpreter not found, skipping python_adds_entities");
        return;
    }

    let mut host = MockHost::new();
    let mut plugin = PythonShellPlugin::new();

    assert!(plugin.dispatch(&mut host, "PY_OPEN_SHELL"));
    send_input(
        &mut plugin,
        "ocs.add_point(1, 2); ocs.add_line(0, 0, 0, 1, 1, 1); ocs.add_circle(5, 5, 0, 2); ocs.add_text(0, 0, 0, 'A', 5)",
    );
    click_run(&mut plugin, &mut host);

    assert_eq!(
        host.added_entities.len(),
        4,
        "expected 4 added entities, got {:?}",
        host.added_entities
    );
    assert!(matches!(&host.added_entities[0], EntityType::Point(_)));
    assert!(matches!(&host.added_entities[1], EntityType::Line(_)));
    assert!(matches!(&host.added_entities[2], EntityType::Circle(_)));
    assert!(matches!(&host.added_entities[3], EntityType::Text(_)));
}

#[test]
fn python_xdata_roundtrip() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if !ocs_pythonshell::python_available() {
        eprintln!("Python interpreter not found, skipping python_xdata_roundtrip");
        return;
    }

    let mut host = MockHost::new();
    let mut plugin = PythonShellPlugin::new();

    assert!(plugin.dispatch(&mut host, "PY_OPEN_SHELL"));
    send_input(
        &mut plugin,
        "h = ocs.add_point(1, 2)\n\
ocs.write_xdata(h, 'PY_SHELL', {'values': [{'type': 'String', 'value': 'hello'}]})\n\
print(ocs.read_xdata(h, 'PY_SHELL'))\n\
ocs.remove_xdata(h, 'PY_SHELL')\n\
print(ocs.read_xdata(h, 'PY_SHELL'))",
    );
    click_run(&mut plugin, &mut host);

    assert!(
        output_contains(&host.async_events, "hello"),
        "expected output to contain written xdata, got {:?}",
        host.async_events
    );
    assert!(
        output_contains(&host.async_events, "None"),
        "expected output to contain None after removal, got {:?}",
        host.async_events
    );
}

#[test]
fn clear_output_button_empties_buffer() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if !ocs_pythonshell::python_available() {
        eprintln!("Python interpreter not found, skipping clear_output_button_empties_buffer");
        return;
    }

    let mut host = MockHost::new();
    let mut plugin = PythonShellPlugin::new();

    assert!(plugin.dispatch(&mut host, "PY_OPEN_SHELL"));
    send_input(&mut plugin, "print('hello')");
    click_run(&mut plugin, &mut host);
    assert!(
        output_contains(&host.async_events, "hello"),
        "expected output to contain hello before clear, got {:?}",
        host.async_events
    );

    click_clear(&mut plugin, &mut host);
    let lines = output_lines(&host.async_events);
    assert!(
        lines.iter().all(|l| l.is_empty()),
        "expected empty output after clear, got {:?}",
        lines
    );
}

#[test]
fn missing_python_shows_error() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let original = std::env::var("OCS_PYTHON_EXE").ok();
    unsafe {
        std::env::set_var("OCS_PYTHON_EXE", "");
    }

    let mut host = MockHost::new();
    let plugin = PythonShellPlugin::new();

    assert!(plugin.dispatch(&mut host, "PY_OPEN_SHELL"));
    assert!(
        host.errors
            .iter()
            .any(|e| e.contains("Python interpreter not found")),
        "expected push_error about missing Python, got {:?}",
        host.errors
    );
    assert!(host.async_events.iter().any(|e| {
        if let PluginAsync::PanelUpdate { widgets, .. } = e {
            widgets.iter().any(|w| matches!(w, Widget::Label(_)))
        } else {
            false
        }
    }));

    unsafe {
        match original {
            Some(v) => std::env::set_var("OCS_PYTHON_EXE", v),
            None => std::env::remove_var("OCS_PYTHON_EXE"),
        }
    }
}
