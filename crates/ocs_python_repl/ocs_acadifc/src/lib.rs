//! PyO3 extension exposing the `acadifc` document model to Python.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use interprocess::local_socket::traits::Stream as StreamTrait;
use interprocess::local_socket::{GenericNamespaced, Stream, ToNsName};
use pyo3::prelude::*;
use pyo3::types::PyModule;

mod document;
mod entities;
mod geometry;
mod mutations;

use document::{PyDocument, PyLayer};
use entities::{PyArc, PyCircle, PyEntity, PyLine, PyPoint, PyText};
use geometry::{PyColor, PyVector3};
use mutations::PyMutationQueue;

/// Shared runtime state initialized from the bootstrap script.
struct Runtime {
    snapshot_path: PathBuf,
    queue_path: PathBuf,
    control_socket: String,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// Initialize the extension from the bootstrap script. Called before any
/// Python user code runs.
#[pyfunction]
fn _init(
    py: Python,
    snapshot_path: String,
    queue_path: String,
    control_socket: String,
) -> PyResult<()> {
    init_runtime(py, snapshot_path, queue_path, control_socket)
}

fn init_runtime(
    py: Python,
    snapshot_path: String,
    queue_path: String,
    control_socket: String,
) -> PyResult<()> {
    {
        let mut rt = RUNTIME.lock().unwrap();
        *rt = Some(Runtime {
            snapshot_path: PathBuf::from(snapshot_path),
            queue_path: PathBuf::from(queue_path),
            control_socket,
        });
    }

    // Create the singleton `ocs.doc` object. It is created lazily here so the
    // shared-memory paths are already set.
    let doc = document::get_doc(py)?;
    let module = py.import_bound("ocs")?;
    module.setattr("doc", doc)?;
    Ok(())
}

/// Try to auto-initialize the extension from `_ocs_config.json` next to the
/// module binary or in the current working directory. This lets a simple
/// `import ocs` in Zed/debugpy work without explicit self-initialization boilerplate.
fn try_auto_init(py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    {
        let rt = RUNTIME.lock().unwrap();
        if rt.is_some() {
            return Ok(());
        }
    }

    // Candidate directories: module's directory, then cwd.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(file) = m.getattr("__file__") {
        if let Ok(path) = file.extract::<String>() {
            if let Some(parent) = PathBuf::from(path).parent() {
                candidates.push(parent.to_path_buf());
            }
        }
    }
    candidates.push(std::env::current_dir().unwrap_or_default());

    for dir in &candidates {
        let config_path = dir.join("_ocs_config.json");
        if config_path.exists() {
            return init_from_config_file(py, m, &config_path);
        }
    }

    Ok(())
}

fn init_from_config_file(
    py: Python,
    m: &Bound<'_, PyModule>,
    config_path: &std::path::Path,
) -> PyResult<()> {
    let contents = std::fs::read_to_string(config_path)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("read config: {e}")))?;
    let cfg: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("parse config: {e}")))?;

    let snapshot_path = cfg["snapshot_path"].as_str().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("config missing snapshot_path")
    })?;
    let queue_path = cfg["queue_path"].as_str().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("config missing queue_path")
    })?;
    let control_socket = cfg["control_socket"].as_str().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("config missing control_socket")
    })?;

    {
        let mut rt = RUNTIME.lock().unwrap();
        *rt = Some(Runtime {
            snapshot_path: PathBuf::from(snapshot_path),
            queue_path: PathBuf::from(queue_path),
            control_socket: control_socket.to_string(),
        });
    }

    let doc = document::get_doc(py)?;
    m.setattr("doc", doc)?;
    Ok(())
}

fn runtime() -> PyResult<std::sync::MutexGuard<'static, Option<Runtime>>> {
    let rt = RUNTIME.lock().unwrap();
    if rt.is_none() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "ocs extension not initialized; call ocs._init()",
        ));
    }
    Ok(rt)
}

pub(crate) fn runtime_paths() -> PyResult<(PathBuf, PathBuf, String)> {
    let rt = runtime()?;
    let rt = rt.as_ref().unwrap();
    Ok((rt.snapshot_path.clone(), rt.queue_path.clone(), rt.control_socket.clone()))
}

pub(crate) fn send_control_message(msg: &str) -> PyResult<()> {
    let rt = runtime()?;
    let rt = rt.as_ref().unwrap();
    let name = rt
        .control_socket
        .clone()
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("socket name: {e}")))?;

    // Run the control-socket exchange on a background thread so we can enforce a
    // timeout. Zed/debugpy sometimes launches the Python process in a context
    // where the host listener cannot be reached, and we want to fail fast with
    // a diagnostic message instead of hanging forever.
    let msg = msg.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> PyResult<String> {
            let mut stream = Stream::connect(name).map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("control socket connect: {e}"))
            })?;
            stream
                .write_all(format!("{}\n", msg).as_bytes())
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("control socket write: {e}")))?;
            stream
                .flush()
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("control socket flush: {e}")))?;
            let mut reader = std::io::BufReader::new(stream);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("control socket read ack: {e}")))?;
            Ok(line)
        })();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(Ok(line)) => {
            if line.trim() == "OK" {
                Ok(())
            } else if line.trim().is_empty() {
                Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "control socket closed without ack",
                ))
            } else {
                Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "control socket unexpected ack: {line:?}"
                )))
            }
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err(pyo3::exceptions::PyRuntimeError::new_err(
            "control socket timeout waiting for ack (host listener not reachable)",
        )),
    }
}

/// `ocs` module entry.
#[pymodule(name = "ocs")]
fn ocs_acadifc(py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(_init, m)?)?;
    m.add_function(wrap_pyfunction!(document::get_doc, m)?)?;
    m.add_function(wrap_pyfunction!(entities::make_point, m)?)?;
    m.add_function(wrap_pyfunction!(entities::make_line, m)?)?;
    m.add_function(wrap_pyfunction!(entities::make_arc, m)?)?;
    m.add_function(wrap_pyfunction!(entities::make_circle, m)?)?;
    m.add_function(wrap_pyfunction!(entities::make_text, m)?)?;
    m.add_class::<PyDocument>()?;
    m.add_class::<PyLayer>()?;
    m.add_class::<PyEntity>()?;
    m.add_class::<PyPoint>()?;
    m.add_class::<PyLine>()?;
    m.add_class::<PyArc>()?;
    m.add_class::<PyCircle>()?;
    m.add_class::<PyText>()?;

    m.add_class::<PyVector3>()?;
    m.add_class::<PyColor>()?;
    m.add_class::<PyMutationQueue>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    try_auto_init(py, m)?;
    Ok(())
}
