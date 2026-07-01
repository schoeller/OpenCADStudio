//! Standalone `cdylib` plugin that hosts an interactive Python REPL panel.
//!
//! The plugin spawns a Python interpreter (`python -u` by default) with an
//! embedded bootstrap script.  Python `stdout` is shown in a host-rendered
//! multiline output widget, `stderr` carries JSON host API requests, and
//! `stdin` carries code to execute plus `__ocs_resp__` JSON replies.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use ocs_plugin_api::host::{BuiltinPlugin, HostApi, ReaderEntityKind};
use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::panel::{DockStyle, DockZone, PanelDef, PanelEvent, Widget};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

use acadrust::entities::{Circle, Line, Point, Text};
use acadrust::types::Vector3;
use acadrust::xdata::{ExtendedDataRecord, XDataValue};
use acadrust::{EntityType, Handle};

const PANEL_ID: &str = "python_repl";
const OUTPUT_WIDGET_ID: &str = "py_output";
const INPUT_WIDGET_ID: &str = "py_input";
const RUN_BUTTON_ID: &str = "py_run";
const CLEAR_BUTTON_ID: &str = "py_clear";
const DONE_MARKER: &str = "__ocs_done__";
const MAX_OUTPUT_LINES: usize = 500;
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Requests sent from the embedded Python interpreter to the Rust plugin over
/// the child process' `stderr` as JSON lines.  (`stdout` is reserved for REPL
/// output and `stdin` carries code to execute plus `__ocs_resp__` RPC replies.)
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
enum PyRequest {
    PushInfo(String),
    PushOutput(String),
    PushError(String),
    Exit,
    // Document reads
    GetEntities,
    GetLayers,
    LayerName(u64),
    AppIdName(u64),
    // Entity writes
    AddPoint {
        x: f64,
        y: f64,
        z: f64,
        layer: String,
    },
    AddLine {
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
        layer: String,
    },
    AddCircle {
        x: f64,
        y: f64,
        z: f64,
        radius: f64,
        layer: String,
    },
    AddText {
        x: f64,
        y: f64,
        z: f64,
        text: String,
        height: f64,
        layer: String,
    },
    // XDATA
    ReadRecord {
        handle: u64,
        app_name: String,
    },
    WriteRecord {
        handle: u64,
        record: PyXDataRecord,
    },
    RemoveRecord {
        handle: u64,
        app_name: String,
    },
    // Scene state
    BumpGeometry,
    SetDirty,
    PushUndo(String),
}

/// Responses sent from the Rust plugin back to the embedded Python interpreter
/// on `stdin`, prefixed with `__ocs_resp__ ` and encoded as JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
enum PyResponse {
    Ok,
    Entities(Vec<PyEntity>),
    Layers(Vec<PyLayer>),
    OptionalString(Option<String>),
    Handle(u64),
    Record(Option<PyXDataRecord>),
    Bool(bool),
    Error(String),
}

/// Transportable entity view returned to Python.
#[derive(Debug, Serialize, Deserialize)]
struct PyEntity {
    handle: u64,
    kind: u8,
    layer_name: String,
    point: Option<[f64; 3]>,
}

/// Transportable layer view returned to Python.
#[derive(Debug, Serialize, Deserialize)]
struct PyLayer {
    handle: u64,
    name: String,
}

/// XDATA record in a shape that round-trips through JSON without depending on
/// acadrust's internal serialization details.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PyXDataRecord {
    application_name: String,
    values: Vec<PyXDataValue>,
}

/// XDATA value in a shape that Python can construct and inspect directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
enum PyXDataValue {
    String(String),
    ControlString(String),
    LayerName(String),
    Real(f64),
    Distance(f64),
    ScaleFactor(f64),
    Integer16(i16),
    Integer32(i32),
    Point3D { x: f64, y: f64, z: f64 },
}

fn xdata_to_py(record: &ExtendedDataRecord) -> Result<PyXDataRecord, String> {
    let mut values = Vec::with_capacity(record.values.len());
    for v in &record.values {
        values.push(match v {
            XDataValue::String(s) => PyXDataValue::String(s.clone()),
            XDataValue::ControlString(s) => PyXDataValue::ControlString(s.clone()),
            XDataValue::LayerName(s) => PyXDataValue::LayerName(s.clone()),
            XDataValue::Real(x) => PyXDataValue::Real(*x),
            XDataValue::Distance(x) => PyXDataValue::Distance(*x),
            XDataValue::ScaleFactor(x) => PyXDataValue::ScaleFactor(*x),
            XDataValue::Integer16(x) => PyXDataValue::Integer16(*x),
            XDataValue::Integer32(x) => PyXDataValue::Integer32(*x),
            XDataValue::Point3D(p) => PyXDataValue::Point3D {
                x: p.x,
                y: p.y,
                z: p.z,
            },
            other => return Err(format!("unsupported XDATA value type: {other:?}")),
        });
    }
    Ok(PyXDataRecord {
        application_name: record.application_name.clone(),
        values,
    })
}

fn py_to_xdata(record: &PyXDataRecord) -> Result<ExtendedDataRecord, String> {
    let mut ext = ExtendedDataRecord::new(&record.application_name);
    for v in &record.values {
        ext.add_value(match v {
            PyXDataValue::String(s) => XDataValue::String(s.clone()),
            PyXDataValue::ControlString(s) => XDataValue::ControlString(s.clone()),
            PyXDataValue::LayerName(s) => XDataValue::LayerName(s.clone()),
            PyXDataValue::Real(x) => XDataValue::Real(*x),
            PyXDataValue::Distance(x) => XDataValue::Distance(*x),
            PyXDataValue::ScaleFactor(x) => XDataValue::ScaleFactor(*x),
            PyXDataValue::Integer16(x) => XDataValue::Integer16(*x),
            PyXDataValue::Integer32(x) => XDataValue::Integer32(*x),
            PyXDataValue::Point3D { x, y, z } => XDataValue::Point3D(Vector3::new(*x, *y, *z)),
        });
    }
    Ok(ext)
}

/// Shared state between the Python reader threads and the plugin thread.
struct SharedState {
    output: Mutex<OutputBuffer>,
}

struct OutputBuffer {
    lines: VecDeque<String>,
    requests: Vec<PyRequest>,
    alive: bool,
    done: bool,
}

/// Handle to the spawned Python worker.
struct Worker {
    panel_id: String,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    shared: Arc<SharedState>,
    readers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl Worker {
    fn new(
        panel_id: &str,
        child: Child,
        stdin: ChildStdin,
        stdout: ChildStdout,
        stderr: ChildStderr,
    ) -> Self {
        let shared = Arc::new(SharedState {
            output: Mutex::new(OutputBuffer {
                lines: VecDeque::new(),
                requests: Vec::new(),
                alive: true,
                done: false,
            }),
        });

        // stdout reader: line-oriented REPL output and completion markers.
        let st = shared.clone();
        let stdout_reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = String::new();
            loop {
                buf.clear();
                match reader.read_line(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = buf
                            .trim_end_matches('\n')
                            .trim_end_matches('\r')
                            .to_string();
                        let mut out = st.output.lock().unwrap_or_else(|e| e.into_inner());
                        if line == DONE_MARKER {
                            out.done = true;
                        } else {
                            push_line(&mut out.lines, line);
                        }
                    }
                    Err(_) => break,
                }
            }
            let mut out = st.output.lock().unwrap_or_else(|e| e.into_inner());
            out.alive = false;
        });

        // stderr reader: JSON-encoded host API requests.
        let st = shared.clone();
        let stderr_reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut buf = String::new();
            loop {
                buf.clear();
                match reader.read_line(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = buf.trim_end_matches('\n').trim_end_matches('\r');
                        if line.is_empty() {
                            continue;
                        }
                        if let Ok(req) = serde_json::from_str::<PyRequest>(line) {
                            let mut out = st.output.lock().unwrap_or_else(|e| e.into_inner());
                            if matches!(req, PyRequest::Exit) {
                                out.alive = false;
                            }
                            out.requests.push(req);
                        }
                    }
                    Err(_) => break,
                }
            }
            let mut out = st.output.lock().unwrap_or_else(|e| e.into_inner());
            out.alive = false;
        });

        Self {
            panel_id: panel_id.to_string(),
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            shared,
            readers: Mutex::new(vec![stdout_reader, stderr_reader]),
        }
    }

    fn append_output(&self, line: &str) {
        let mut out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        push_line(&mut out.lines, line.to_string());
    }

    fn send_code(&self, code: &str) -> std::io::Result<()> {
        self.reset_done();
        let encoded = base64::engine::general_purpose::STANDARD.encode(code.as_bytes());
        let mut guard = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(stdin) = guard.as_mut() {
            writeln!(stdin, "CODE {encoded}")?;
            stdin.flush()?;
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "stdin closed",
            ))
        }
    }

    fn send_response(&self, resp: &PyResponse) -> std::io::Result<()> {
        let json = serde_json::to_string(resp)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let mut guard = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(stdin) = guard.as_mut() {
            writeln!(stdin, "__ocs_resp__ {json}")?;
            stdin.flush()?;
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "stdin closed",
            ))
        }
    }

    fn is_alive(&self) -> bool {
        let out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        if !out.alive {
            return false;
        }
        let mut guard = self.child.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    fn is_done(&self) -> bool {
        let out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.done
    }

    fn reset_done(&self) {
        let mut out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.done = false;
    }

    fn close(&self) {
        let _ = self.stdin.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let readers = std::mem::take(&mut *self.readers.lock().unwrap_or_else(|e| e.into_inner()));
        for handle in readers {
            let _ = handle.join();
        }
        let mut out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.alive = false;
    }

    fn output_lines(&self) -> Vec<String> {
        let out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.lines.iter().cloned().collect()
    }

    fn clear_output(&self) {
        let mut out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.lines.clear();
    }

    fn take_requests(&self) -> Vec<PyRequest> {
        let mut out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut out.requests)
    }

    fn wait_for_activity(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (prev_lines, prev_reqs, prev_done, prev_alive) = {
            let out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
            (out.lines.len(), out.requests.len(), out.done, out.alive)
        };
        loop {
            {
                let out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
                if out.lines.len() != prev_lines
                    || out.requests.len() != prev_reqs
                    || out.done != prev_done
                    || out.alive != prev_alive
                {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn push_line(lines: &mut VecDeque<String>, line: String) {
    if lines.len() >= MAX_OUTPUT_LINES {
        lines.pop_front();
    }
    lines.push_back(line);
}

/// Embedded Python bootstrap.  It exposes a high-level `ocs` object whose
/// methods issue JSON-RPC-like requests on `stderr` and read matching
/// `__ocs_resp__` replies from `stdin`.  Code to execute is delivered as
/// `__ocs_code__ <base64>` lines on `stdin` so multi-line statements work.
const BOOTSTRAP: &str = r#"
import sys, json, traceback, base64

class OcsError(Exception):
    pass

def _call(req):
    sys.stderr.write(json.dumps(req, separators=(',', ':')) + '\n')
    sys.stderr.flush()
    while True:
        line = sys.stdin.readline()
        if not line:
            raise EOFError('lost connection to OpenCAD Studio host')
        line = line.rstrip('\n').rstrip('\r')
        if line.startswith('__ocs_resp__ '):
            payload = line[len('__ocs_resp__ '):]
            resp = json.loads(payload)
            if resp.get('type') == 'Error':
                raise OcsError(resp.get('value'))
            return resp

class Doc:
    def entities(self):
        return _call({'type': 'GetEntities'})['value']

    def layers(self):
        return _call({'type': 'GetLayers'})['value']

    def layer_name(self, handle):
        return _call({'type': 'LayerName', 'value': handle})['value']

    def app_id_name(self, handle):
        return _call({'type': 'AppIdName', 'value': handle})['value']

class Ocs:
    doc = Doc()

    def push_info(self, msg):
        _call({'type': 'PushInfo', 'value': str(msg)})

    def push_output(self, msg):
        _call({'type': 'PushOutput', 'value': str(msg)})

    def push_error(self, msg):
        _call({'type': 'PushError', 'value': str(msg)})

    def exit(self):
        _call({'type': 'Exit'})
        sys.exit(0)

    def add_point(self, x, y, z=0.0, layer='0'):
        return _call({'type': 'AddPoint', 'value': {'x': x, 'y': y, 'z': z, 'layer': layer}})['value']

    def add_line(self, x1, y1, z1, x2, y2, z2, layer='0'):
        return _call({'type': 'AddLine', 'value': {
            'x1': x1, 'y1': y1, 'z1': z1,
            'x2': x2, 'y2': y2, 'z2': z2,
            'layer': layer}})['value']

    def add_circle(self, x, y, z, radius, layer='0'):
        return _call({'type': 'AddCircle', 'value': {
            'x': x, 'y': y, 'z': z, 'radius': radius, 'layer': layer}})['value']

    def add_text(self, x, y, z, text, height=10.0, layer='0'):
        return _call({'type': 'AddText', 'value': {
            'x': x, 'y': y, 'z': z, 'text': text,
            'height': height, 'layer': layer}})['value']

    def read_xdata(self, handle, app_name):
        return _call({'type': 'ReadRecord', 'value': {
            'handle': handle, 'app_name': app_name}})['value']

    def write_xdata(self, handle, app_name, data):
        record = dict(data)
        record.setdefault('application_name', app_name)
        return _call({'type': 'WriteRecord', 'value': {
            'handle': handle, 'record': record}})['value']

    def remove_xdata(self, handle, app_name):
        return _call({'type': 'RemoveRecord', 'value': {
            'handle': handle, 'app_name': app_name}})['value']

    def bump_geometry(self):
        _call({'type': 'BumpGeometry'})

    def set_dirty(self):
        _call({'type': 'SetDirty'})

    def push_undo(self, label):
        _call({'type': 'PushUndo', 'value': str(label)})

_ocs = Ocs()
sys.modules['ocs'] = _ocs

print('Python REPL ready', flush=True)
_globals = {'ocs': _ocs, '__name__': '__main__'}
_locals = {}
while True:
    line = sys.stdin.readline()
    if not line:
        break
    line = line.rstrip('\n').rstrip('\r')
    if line.startswith('CODE '):
        payload = line[len('CODE '):]
        try:
            code = base64.b64decode(payload).decode('utf-8')
        except Exception:
            print(traceback.format_exc().strip(), flush=True)
        else:
            try:
                try:
                    # Single expressions (e.g. `2+3` or `ocs.doc.entities()`)
                    # are evaluated so their value is printed like a REPL.
                    result = eval(compile(code, '<stdin>', 'eval'), _globals, _locals)
                except SyntaxError:
                    # Statements, definitions and multi-line blocks run with exec.
                    exec(compile(code, '<stdin>', 'exec'), _globals, _locals)
                else:
                    if result is not None:
                        print(repr(result), flush=True)
            except Exception:
                print(traceback.format_exc().strip(), flush=True)
        print('__ocs_done__', flush=True)
"#;

/// Verify that a command speaks Python 3.
fn verify_python3(name: &str, extra_args: &[&str]) -> bool {
    let mut probe = Command::new(name);
    for arg in extra_args {
        probe.arg(arg);
    }
    probe.arg("--version");
    probe.stdout(Stdio::piped());
    probe.stderr(Stdio::piped());
    let mut child = match probe.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let timeout = Duration::from_millis(2000);
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                let mut stdout = String::new();
                if let Some(out) = child.stdout.take() {
                    let _ = BufReader::new(out).read_to_string(&mut stdout);
                }
                let mut stderr = String::new();
                if let Some(err) = child.stderr.take() {
                    let _ = BufReader::new(err).read_to_string(&mut stderr);
                }
                let output = if stdout.trim().is_empty() {
                    stderr.trim()
                } else {
                    stdout.trim()
                };
                return output.starts_with("Python 3");
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return false,
        }
    }
}

fn python_command() -> Option<Command> {
    if let Ok(p) = std::env::var("OCS_PYTHON_EXE") {
        if p.is_empty() {
            return None;
        }
        // Accept both absolute paths and command names resolvable through PATH.
        if verify_python3(&p, &[]) {
            return Some(Command::new(p));
        }
        return None;
    }
    for name in ["python3", "python"] {
        if verify_python3(name, &[]) {
            return Some(Command::new(name));
        }
    }
    if cfg!(windows) {
        if verify_python3("py", &["-3"]) {
            let mut cmd = Command::new("py");
            cmd.arg("-3");
            return Some(cmd);
        }
    }
    None
}

pub fn python_available() -> bool {
    python_command().is_some()
}

fn spawn_python_worker_with_command(panel_id: &str, cmd: &mut Command) -> Result<Worker, String> {
    let mut child = cmd
        .arg("-u")
        .arg("-c")
        .arg(BOOTSTRAP)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn Python: {e}"))?;

    let stdin = child.stdin.take().ok_or("missing stdin")?;
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    Ok(Worker::new(panel_id, child, stdin, stdout, stderr))
}

fn build_widgets(lines: Vec<String>, input: &str, worker_alive: bool) -> Vec<Widget> {
    if worker_alive {
        vec![
            Widget::MultilineOutput {
                id: OUTPUT_WIDGET_ID.to_string(),
                lines,
            },
            Widget::TextInput {
                id: INPUT_WIDGET_ID.to_string(),
                value: input.to_string(),
            },
            Widget::Button {
                id: RUN_BUTTON_ID.to_string(),
                label: "Run".to_string(),
            },
            Widget::Button {
                id: CLEAR_BUTTON_ID.to_string(),
                label: "Clear Output".to_string(),
            },
        ]
    } else {
        vec![
            Widget::MultilineOutput {
                id: OUTPUT_WIDGET_ID.to_string(),
                lines,
            },
            Widget::Label(
                "Python interpreter not available. Install Python or set OCS_PYTHON_EXE."
                    .to_string(),
            ),
        ]
    }
}

fn panel_def() -> PanelDef {
    PanelDef {
        id: PANEL_ID.to_string(),
        title: "Python Shell".to_string(),
        icon: None,
        dock: DockZone::Floating,
        initial_x: Some(100.0),
        initial_y: Some(100.0),
        initial_width: 400.0,
        initial_height: 300.0,
        min_width: 200.0,
        min_height: 150.0,
        dockable_zones: vec![DockZone::Floating, DockZone::Left, DockZone::Right],
        allow_undock: true,
        resizable: true,
        draggable: true,
        dock_style: DockStyle::Tabs,
    }
}

struct PluginState {
    worker: Option<Worker>,
    input: String,
}

pub struct PythonShellPlugin {
    state: Mutex<PluginState>,
}

impl PythonShellPlugin {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(PluginState {
                worker: None,
                input: String::new(),
            }),
        }
    }
}

impl Default for PythonShellPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PythonShellPlugin {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = state.worker.take() {
            w.close();
        }
    }
}

static MANIFEST: PluginManifest = PluginManifest {
    id: "ocs.pythonshell",
    name: "Python Shell",
    version: "0.1.0",
    description: "Interactive Python REPL panel.",
    api_version: ApiVersion { major: 3 },
    ribbon_order: 200,
    xdata_apps: &["PY_SHELL"],
    command_prefixes: &[],
};

struct PythonModule;

impl CadModule for PythonModule {
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
                id: "PY_OPEN_SHELL",
                label: "Python Shell",
                icon: IconKind::Glyph(">_"),
                event: ModuleEvent::Command("PY_OPEN_SHELL".to_string()),
            })],
        }])).as_slice()
    }
}

impl BuiltinPlugin for PythonShellPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(PythonModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        if cmd != "PY_OPEN_SHELL" {
            return false;
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let def = panel_def();

        match host.open_panel(&def) {
            Ok(_handle) => {
                let Some(mut cmd) = python_command() else {
                    host.send_async(PluginAsync::PanelUpdate {
                        panel_id: PANEL_ID.to_string(),
                        widgets: build_widgets(Vec::new(), "", false),
                    });
                    host.push_error(
                        "Python interpreter not found. Install Python or set OCS_PYTHON_EXE.",
                    );
                    return true;
                };
                match spawn_python_worker_with_command(PANEL_ID, &mut cmd) {
                    Ok(worker) => {
                        worker.append_output("Python REPL ready");
                        let lines = worker.output_lines();
                        host.send_async(PluginAsync::PanelUpdate {
                            panel_id: PANEL_ID.to_string(),
                            widgets: build_widgets(lines, "", true),
                        });
                        state.worker = Some(worker);
                    }
                    Err(e) => {
                        host.send_async(PluginAsync::PanelUpdate {
                            panel_id: PANEL_ID.to_string(),
                            widgets: build_widgets(Vec::new(), "", false),
                        });
                        host.push_error(&format!("Could not start Python: {e}"));
                    }
                }
            }
            Err(e) => {
                host.push_error(&format!("Failed to open Python panel: {e}"));
            }
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

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            PanelEvent::Closed => {
                if let Some(w) = state.worker.take() {
                    w.close();
                }
                state.input.clear();
            }
            PanelEvent::TextChanged { id, value } if id == INPUT_WIDGET_ID => {
                state.input = value;
            }
            PanelEvent::Clicked(id) if id == CLEAR_BUTTON_ID => {
                if let Some(worker) = state.worker.as_ref() {
                    worker.clear_output();
                    let lines = worker.output_lines();
                    host.send_async(PluginAsync::PanelUpdate {
                        panel_id: PANEL_ID.to_string(),
                        widgets: build_widgets(lines, &state.input, true),
                    });
                }
            }
            PanelEvent::Clicked(id) if id == RUN_BUTTON_ID => {
                let code = state.input.clone();
                if code.is_empty() {
                    return;
                }
                state.input.clear();

                let worker = match state.worker.as_ref() {
                    Some(w) => w,
                    None => return,
                };

                worker.append_output(&format!(">>> {code}"));
                if let Err(e) = worker.send_code(&code) {
                    worker.append_output(&format!("Error: {e}"));
                    flush_to_host(host, worker, "");
                    return;
                }

                let timeout_secs: u64 = std::env::var("OCS_PYTHON_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(DEFAULT_TIMEOUT_SECS);
                const IDLE_TIMEOUT: Duration = Duration::from_millis(2000);
                let total_timeout = Duration::from_secs(timeout_secs);
                let deadline = Instant::now() + total_timeout;
                let mut keep_worker = true;
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    if !worker.is_alive() {
                        break;
                    }
                    if worker.is_done() {
                        // The Done marker is printed on stdout after all REPL
                        // output, so all stdout is already in the buffer. Do one
                        // final request flush and update.
                        if !flush_to_host(host, worker, "") {
                            keep_worker = false;
                        }
                        break;
                    }
                    if !worker.wait_for_activity(IDLE_TIMEOUT) {
                        break;
                    }
                    if !flush_to_host(host, worker, "") {
                        keep_worker = false;
                        break;
                    }
                }

                if keep_worker && !flush_to_host(host, worker, "") {
                    keep_worker = false;
                }

                if keep_worker && !worker.is_alive() {
                    worker.close();
                    host.send_async(PluginAsync::PanelClosed {
                        panel_id: PANEL_ID.to_string(),
                    });
                    keep_worker = false;
                }
                if !keep_worker {
                    state.worker = None;
                }
            }
            _ => {}
        }
    }
}

/// Drain pending host API requests, send RPC replies back to Python, and emit a
/// panel update.  Returns `false` if the Python side requested an exit (in which
/// case the panel has been closed).
fn flush_to_host(host: &mut dyn HostApi, worker: &Worker, input: &str) -> bool {
    let mut needs_dirty = false;
    let mut needs_bump = false;
    for req in worker.take_requests() {
        let close = matches!(req, PyRequest::Exit);
        let resp = handle_py_request(host, req, &mut needs_dirty, &mut needs_bump);
        let _ = worker.send_response(&resp);
        if close {
            worker.close();
            host.send_async(PluginAsync::PanelClosed {
                panel_id: worker.panel_id.clone(),
            });
            return false;
        }
    }
    if needs_dirty {
        host.set_dirty();
    }
    if needs_bump {
        host.bump_geometry();
    }
    let alive = worker.is_alive();
    let lines = worker.output_lines();
    host.send_async(PluginAsync::PanelUpdate {
        panel_id: worker.panel_id.clone(),
        widgets: build_widgets(lines, input, alive),
    });
    true
}

/// Map a Python request to a host operation and return the synchronous reply.
fn handle_py_request(
    host: &mut dyn HostApi,
    req: PyRequest,
    needs_dirty: &mut bool,
    needs_bump: &mut bool,
) -> PyResponse {
    match req {
        PyRequest::PushInfo(m) => {
            host.push_info(&m);
            PyResponse::Ok
        }
        PyRequest::PushOutput(m) => {
            host.push_output(&m);
            PyResponse::Ok
        }
        PyRequest::PushError(m) => {
            host.push_error(&m);
            PyResponse::Ok
        }
        PyRequest::Exit => PyResponse::Ok,
        PyRequest::GetEntities => {
            let reader = host.document_reader();
            let mut entities = Vec::new();
            reader.for_each_entity(&mut |e| {
                let kind = match e.kind {
                    ReaderEntityKind::Point => 0,
                    ReaderEntityKind::Line => 1,
                    ReaderEntityKind::Circle => 2,
                    ReaderEntityKind::Arc => 3,
                    ReaderEntityKind::Polyline => 4,
                    ReaderEntityKind::Text => 5,
                    ReaderEntityKind::Other => 6,
                };
                let point = e.point.map(|p| [p.x, p.y, p.z]);
                entities.push(PyEntity {
                    handle: e.handle.value(),
                    kind,
                    layer_name: e.layer_name.to_string(),
                    point,
                });
            });
            PyResponse::Entities(entities)
        }
        PyRequest::GetLayers => {
            let layers: Vec<PyLayer> = host
                .document()
                .layers
                .iter()
                .map(|l| PyLayer {
                    handle: l.handle.value(),
                    name: l.name.clone(),
                })
                .collect();
            PyResponse::Layers(layers)
        }
        PyRequest::LayerName(handle) => {
            let reader = host.document_reader();
            PyResponse::OptionalString(
                reader
                    .layer_name(Handle::new(handle))
                    .map(|s| s.to_string()),
            )
        }
        PyRequest::AppIdName(handle) => {
            let reader = host.document_reader();
            PyResponse::OptionalString(
                reader
                    .app_id_name(Handle::new(handle))
                    .map(|s| s.to_string()),
            )
        }
        PyRequest::AddPoint { x, y, z, layer } => {
            let mut p = Point::at(Vector3::new(x, y, z));
            p.common.layer = layer;
            let handle = host.add_entity(EntityType::Point(p));
            *needs_dirty = true;
            *needs_bump = true;
            PyResponse::Handle(handle.value())
        }
        PyRequest::AddLine {
            x1,
            y1,
            z1,
            x2,
            y2,
            z2,
            layer,
        } => {
            let mut l = Line::from_points(Vector3::new(x1, y1, z1), Vector3::new(x2, y2, z2));
            l.common.layer = layer;
            let handle = host.add_entity(EntityType::Line(l));
            *needs_dirty = true;
            *needs_bump = true;
            PyResponse::Handle(handle.value())
        }
        PyRequest::AddCircle {
            x,
            y,
            z,
            radius,
            layer,
        } => {
            let mut c = Circle {
                center: Vector3::new(x, y, z),
                radius,
                ..Default::default()
            };
            c.common.layer = layer;
            let handle = host.add_entity(EntityType::Circle(c));
            *needs_dirty = true;
            *needs_bump = true;
            PyResponse::Handle(handle.value())
        }
        PyRequest::AddText {
            x,
            y,
            z,
            text,
            height,
            layer,
        } => {
            let mut t = Text::with_value(&text, Vector3::new(x, y, z)).with_height(height);
            t.common.layer = layer;
            let handle = host.add_entity(EntityType::Text(t));
            *needs_dirty = true;
            *needs_bump = true;
            PyResponse::Handle(handle.value())
        }
        PyRequest::ReadRecord { handle, app_name } => {
            let record = host.read_record(Handle::new(handle), &app_name);
            match record {
                Some(r) => match xdata_to_py(r) {
                    Ok(py) => PyResponse::Record(Some(py)),
                    Err(e) => PyResponse::Error(e),
                },
                None => PyResponse::Record(None),
            }
        }
        PyRequest::WriteRecord { handle, record } => {
            match py_to_xdata(&record) {
                Ok(mut ext) => {
                    // Ensure the record uses the requested application name.
                    ext.application_name = record.application_name;
                    host.write_record(Handle::new(handle), ext);
                    *needs_dirty = true;
                    PyResponse::Bool(true)
                }
                Err(e) => PyResponse::Error(e),
            }
        }
        PyRequest::RemoveRecord { handle, app_name } => {
            PyResponse::Bool(host.remove_record(Handle::new(handle), &app_name))
        }
        PyRequest::BumpGeometry => {
            *needs_bump = true;
            PyResponse::Ok
        }
        PyRequest::SetDirty => {
            *needs_dirty = true;
            PyResponse::Ok
        }
        PyRequest::PushUndo(label) => {
            host.push_undo(&label);
            PyResponse::Ok
        }
    }
}

ocs_plugin_api::export_plugin!(PythonShellPlugin::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_and_ribbon_are_valid() {
        let plugin = PythonShellPlugin::new();
        assert_eq!(plugin.manifest().id, "ocs.pythonshell");
        assert_eq!(plugin.manifest().api_version.major, 3);
        assert_eq!(plugin.panels().len(), 1);
        assert_eq!(plugin.panels()[0].id, PANEL_ID);
        let ribbon = plugin.ribbon();
        assert!(!ribbon.ribbon_groups().is_empty());
    }
}
