//! Python request/response types and routing to `PluginRequest`.

use acadrust::xdata::{ExtendedDataRecord, XDataValue};
use acadrust::{EntityType, Handle};
use ocs_plugin_api::host::{CadDocumentReader, HostApi};
use ocs_plugin_api::ipc::protocol::{PluginRequest, PluginResponse};
use serde::{Deserialize, Serialize};

/// Requests sent from the embedded Python interpreter to the Rust plugin over
/// the child process' `stderr` as JSON lines. `stdout` is reserved for REPL
/// output and `stdin` carries `__ocs_resp__` RPC replies.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum PyRequest {
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
    // Entity removal
    Erase(u64),
    EraseByLayer(String),
    EraseAll,
    // Debug
    DebugStart {
        port: u16,
    },
    // Stats
    GetStats,
}

/// Responses sent from the Rust plugin back to the embedded Python interpreter
/// on `stdin`, prefixed with `__ocs_resp__ ` and encoded as JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum PyResponse {
    Ok,
    Entities(Vec<PyEntity>),
    Layers(Vec<PyLayer>),
    OptionalString(Option<String>),
    Handle(u64),
    Record(Option<PyXDataRecord>),
    Bool(bool),
    Count(usize),
    DebugStarted { port: u16 },
    Stats { written: usize, erased: usize },
    Error(String),
}

/// Transportable entity view returned to Python.
#[derive(Debug, Serialize, Deserialize)]
pub struct PyEntity {
    pub handle: u64,
    pub kind: u8,
    pub layer_name: String,
    pub point: Option<[f64; 3]>,
}

/// Transportable layer view returned to Python.
#[derive(Debug, Serialize, Deserialize)]
pub struct PyLayer {
    pub handle: u64,
    pub name: String,
}

/// XDATA record in a shape that round-trips through JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PyXDataRecord {
    pub application_name: String,
    pub values: Vec<PyXDataValue>,
}

/// XDATA value in a shape that Python can construct and inspect directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum PyXDataValue {
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

/// Convert an `ExtendedDataRecord` to the Python representation.
pub fn xdata_to_py(record: &ExtendedDataRecord) -> Result<PyXDataRecord, String> {
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

/// Convert the Python XDATA representation to an `ExtendedDataRecord`.
pub fn py_to_xdata(record: &PyXDataRecord) -> Result<ExtendedDataRecord, String> {
    use acadrust::types::Vector3;
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

/// Convert a Python request to the `PluginRequest` sent through the host queue.
pub fn py_request_to_plugin_request(req: PyRequest) -> Result<PluginRequest, String> {
    use acadrust::entities::{Circle, Line, Point, Text};
    use acadrust::types::Vector3;
    match req {
        PyRequest::PushInfo(m) => Ok(PluginRequest::PushInfo(m)),
        PyRequest::PushOutput(m) => Ok(PluginRequest::PushOutput(m)),
        PyRequest::PushError(m) => Ok(PluginRequest::PushError(m)),
        PyRequest::Exit => Ok(PluginRequest::PushInfo("Python worker exited".to_string())),
        PyRequest::GetEntities => Ok(PluginRequest::DocumentSnapshot),
        PyRequest::GetLayers => Ok(PluginRequest::DocumentSnapshot),
        PyRequest::LayerName(_) => Ok(PluginRequest::DocumentSnapshot),
        PyRequest::AppIdName(_) => Ok(PluginRequest::DocumentSnapshot),
        PyRequest::AddPoint { x, y, z, layer } => {
            let mut p = Point::at(Vector3::new(x, y, z));
            p.common.layer = layer;
            Ok(PluginRequest::AddEntity(EntityType::Point(p)))
        }
        PyRequest::AddLine { x1, y1, z1, x2, y2, z2, layer } => {
            let mut l = Line::from_points(Vector3::new(x1, y1, z1), Vector3::new(x2, y2, z2));
            l.common.layer = layer;
            Ok(PluginRequest::AddEntity(EntityType::Line(l)))
        }
        PyRequest::AddCircle { x, y, z, radius, layer } => {
            let mut c = Circle {
                center: Vector3::new(x, y, z),
                radius,
                ..Default::default()
            };
            c.common.layer = layer;
            Ok(PluginRequest::AddEntity(EntityType::Circle(c)))
        }
        PyRequest::AddText { x, y, z, text, height, layer } => {
            let mut t = Text::with_value(&text, Vector3::new(x, y, z)).with_height(height);
            t.common.layer = layer;
            Ok(PluginRequest::AddEntity(EntityType::Text(t)))
        }
        PyRequest::ReadRecord { handle, app_name } => Ok(PluginRequest::ReadRecord {
            handle: Handle::new(handle),
            app_name,
        }),
        PyRequest::WriteRecord { handle, record } => match py_to_xdata(&record) {
            Ok(mut ext) => {
                ext.application_name = record.application_name;
                Ok(PluginRequest::WriteRecord {
                    handle: Handle::new(handle),
                    record: ext,
                })
            }
            Err(e) => Err(e),
        },
        PyRequest::RemoveRecord { handle, app_name } => Ok(PluginRequest::RemoveRecord {
            handle: Handle::new(handle),
            app_name,
        }),
        PyRequest::BumpGeometry => Ok(PluginRequest::BumpGeometry),
        PyRequest::SetDirty => Ok(PluginRequest::SetDirty),
        PyRequest::PushUndo(label) => Ok(PluginRequest::PushUndo { label }),
        PyRequest::Erase(handle) => Ok(PluginRequest::RemoveEntity {
            handle: Handle::new(handle),
        }),
        PyRequest::EraseByLayer(layer) => {
            Err(format!("erase by layer not implemented for layer: {layer}"))
        }
        PyRequest::EraseAll => Err("erase all not implemented".to_string()),
        PyRequest::DebugStart { port } => Ok(PluginRequest::PushInfo(format!(
            "debugpy start requested on port {port}; install debugpy on the Python side"
        ))),
        PyRequest::GetStats => Ok(PluginRequest::DocumentSnapshot),
    }
}

/// Convert a host `PluginResponse` back to the Python response shape.
pub fn plugin_response_to_py_response(
    resp: PluginResponse,
    _written: usize,
    _erased: usize,
) -> Result<PyResponse, String> {
    use ocs_plugin_api::host::{DocumentReader, ReaderEntityKind};
    match resp {
        PluginResponse::Ok => Ok(PyResponse::Ok),
        PluginResponse::Bool(b) => Ok(PyResponse::Bool(b)),
        PluginResponse::Count(n) => Ok(PyResponse::Count(n)),
        PluginResponse::Entity(_) => Ok(PyResponse::Ok),
        PluginResponse::Handle(h) => Ok(PyResponse::Handle(h.value())),
        PluginResponse::Record(r) => match r {
            Some(record) => match xdata_to_py(&record) {
                Ok(py) => Ok(PyResponse::Record(Some(py))),
                Err(e) => Err(e),
            },
            None => Ok(PyResponse::Record(None)),
        },
        PluginResponse::Document(doc) => {
            let reader = CadDocumentReader(&doc);
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
                entities.push(PyEntity {
                    handle: e.handle.value(),
                    kind,
                    layer_name: e.layer_name.to_string(),
                    point: e.point.map(|p| [p.x, p.y, p.z]),
                });
            });
            Ok(PyResponse::Entities(entities))
        }
        PluginResponse::DocumentView { .. } => Ok(PyResponse::Ok),
        PluginResponse::Error(e) => Err(e),
        _ => Err(format!("unexpected PluginResponse: {resp:?}")),
    }
}

/// Map a Python request to a host operation and return the synchronous reply.
#[allow(dead_code)]
pub fn handle_py_request(
    host: &mut dyn HostApi,
    req: PyRequest,
    needs_dirty: &mut bool,
    needs_bump: &mut bool,
    written: &mut usize,
    erased: &mut usize,
) -> PyResponse {
    use ocs_plugin_api::host::{ReaderEntityKind};
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
            use acadrust::entities::Point;
            use acadrust::types::Vector3;
            let mut p = Point::at(Vector3::new(x, y, z));
            p.common.layer = layer;
            let handle = host.add_entity(EntityType::Point(p));
            *needs_dirty = true;
            *needs_bump = true;
            *written += 1;
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
            use acadrust::entities::Line;
            use acadrust::types::Vector3;
            let mut l = Line::from_points(Vector3::new(x1, y1, z1), Vector3::new(x2, y2, z2));
            l.common.layer = layer;
            let handle = host.add_entity(EntityType::Line(l));
            *needs_dirty = true;
            *needs_bump = true;
            *written += 1;
            PyResponse::Handle(handle.value())
        }
        PyRequest::AddCircle {
            x,
            y,
            z,
            radius,
            layer,
        } => {
            use acadrust::entities::Circle;
            use acadrust::types::Vector3;
            let mut c = Circle {
                center: Vector3::new(x, y, z),
                radius,
                ..Default::default()
            };
            c.common.layer = layer;
            let handle = host.add_entity(EntityType::Circle(c));
            *needs_dirty = true;
            *needs_bump = true;
            *written += 1;
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
            use acadrust::entities::Text;
            use acadrust::types::Vector3;
            let mut t = Text::with_value(&text, Vector3::new(x, y, z)).with_height(height);
            t.common.layer = layer;
            let handle = host.add_entity(EntityType::Text(t));
            *needs_dirty = true;
            *needs_bump = true;
            *written += 1;
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
        PyRequest::WriteRecord { handle, record } => match py_to_xdata(&record) {
            Ok(mut ext) => {
                ext.application_name = record.application_name;
                host.write_record(Handle::new(handle), ext);
                *needs_dirty = true;
                PyResponse::Bool(true)
            }
            Err(e) => PyResponse::Error(e),
        },
        PyRequest::RemoveRecord { handle, app_name } => {
            PyResponse::Bool(host.remove_record(Handle::new(handle), &app_name))
        }
        PyRequest::Erase(handle) => match host.remove_entity(Handle::new(handle)) {
            true => {
                *erased += 1;
                PyResponse::Ok
            }
            false => PyResponse::Error(format!("entity {handle} not found")),
        },
        PyRequest::EraseByLayer(_) | PyRequest::EraseAll => {
            PyResponse::Error("erase by layer/all not implemented".to_string())
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
        PyRequest::DebugStart { port } => PyResponse::DebugStarted { port },
        PyRequest::GetStats => PyResponse::Stats {
            written: *written,
            erased: *erased,
        },
    }
}
