//! Pythonic document wrapper.

use std::cell::RefCell;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use acadrust::{CadDocument, Handle};
use ocs_plugin_api::shm::{DocumentFullSnapshotReader, DocumentMutationView, EntityOp};

use crate::entities::entity_to_py;
use crate::{runtime_paths, send_control_message};

#[pyclass(name = "Layer")]
#[derive(Clone)]
pub struct PyLayer {
    pub name: String,
}

#[pymethods]
impl PyLayer {
    #[new]
    #[pyo3(signature = (name=None))]
    fn new(name: Option<String>) -> Self {
        Self {
            name: name.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn name(&self) -> String {
        self.name.clone()
    }
}

#[pyclass(name = "Document")]
pub struct PyDocument {
    cached_doc: RefCell<Option<CadDocument>>,
    cached_version: RefCell<u64>,
}

#[pymethods]
impl PyDocument {
    #[getter]
    fn version(&self) -> PyResult<u64> {
        let (snapshot_path, _, _) = runtime_paths()?;
        let reader = DocumentFullSnapshotReader::open(&snapshot_path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("snapshot open: {e}")))?;
        Ok(reader.version())
    }

    fn refresh(&self) -> PyResult<()> {
        let (snapshot_path, _, _) = runtime_paths()?;
        let mut reader = DocumentFullSnapshotReader::open(&snapshot_path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("snapshot open: {e}")))?;
        let (doc, version) = reader
            .refresh()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("snapshot refresh: {e}")))?;
        *self.cached_doc.borrow_mut() = Some(doc);
        *self.cached_version.borrow_mut() = version;
        Ok(())
    }

    #[getter]
    fn layers(&self, py: Python) -> PyResult<Py<PyList>> {
        self.ensure_current(py)?;
        let doc = self.cached_doc.borrow();
        let doc = doc.as_ref().unwrap();
        let list = PyList::empty_bound(py);
        for layer in doc.layers.iter() {
            let py_layer = PyLayer {
                name: layer.name.clone(),
            };
            list.append(py_layer.into_py(py))?;
        }
        Ok(list.into())
    }

    #[getter]
    fn text_styles(&self, py: Python) -> PyResult<Py<PyList>> {
        self.ensure_current(py)?;
        let doc = self.cached_doc.borrow();
        let doc = doc.as_ref().unwrap();
        let list = PyList::empty_bound(py);
        for style in doc.text_styles.iter() {
            list.append(style.name.clone())?;
        }
        Ok(list.into())
    }

    #[getter]
    fn dim_styles(&self, py: Python) -> PyResult<Py<PyList>> {
        self.ensure_current(py)?;
        let doc = self.cached_doc.borrow();
        let doc = doc.as_ref().unwrap();
        let list = PyList::empty_bound(py);
        for style in doc.dim_styles.iter() {
            list.append(style.name.clone())?;
        }
        Ok(list.into())
    }

    #[getter]
    fn styles(&self, py: Python) -> PyResult<Py<PyDict>> {
        self.ensure_current(py)?;
        let doc = self.cached_doc.borrow();
        let doc = doc.as_ref().unwrap();
        let dict = PyDict::new_bound(py);
        let text_styles = PyList::empty_bound(py);
        for style in doc.text_styles.iter() {
            text_styles.append(style.name.clone())?;
        }
        let dim_styles = PyList::empty_bound(py);
        for style in doc.dim_styles.iter() {
            dim_styles.append(style.name.clone())?;
        }
        dict.set_item("text_styles", text_styles)?;
        dict.set_item("dim_styles", dim_styles)?;
        Ok(dict.into())
    }

    #[getter]
    fn blocks(&self, py: Python) -> PyResult<Py<PyList>> {
        self.ensure_current(py)?;
        let doc = self.cached_doc.borrow();
        let doc = doc.as_ref().unwrap();
        let list = PyList::empty_bound(py);
        for block in doc.block_records.iter() {
            list.append(block.name.clone())?;
        }
        Ok(list.into())
    }

    #[getter]
    fn entities(&self, py: Python) -> PyResult<Py<PyList>> {
        self.ensure_current(py)?;
        let doc = self.cached_doc.borrow();
        let doc = doc.as_ref().unwrap();
        let list = PyList::empty_bound(py);
        for entity in doc.entities() {
            let py_entity = entity_to_py(py, entity)?;
            list.append(py_entity)?;
        }
        Ok(list.into())
    }

    fn __len__(&self, py: Python) -> PyResult<usize> {
        self.ensure_current(py)?;
        let doc = self.cached_doc.borrow();
        let doc = doc.as_ref().unwrap();
        Ok(doc.entities().count())
    }

    fn add(&self, entity: &Bound<'_, PyAny>) -> PyResult<()> {
        let op = crate::entities::py_to_entity_op(entity)?;
        let mut queue = open_queue()?;
        if queue.push(&op).map_err(queue_err)? {
            Ok(())
        } else {
            Err(pyo3::exceptions::PyRuntimeError::new_err(
                "mutation queue full; call commit() more often",
            ))
        }
    }

    fn add_many(&self, entities: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut ops = Vec::new();
        for item in entities.iter()? {
            let item = item?;
            ops.push(crate::entities::py_to_entity_op(&item)?);
        }
        let expected = ops.len();
        let mut queue = open_queue()?;
        let queued = queue
            .push_many(ops.into_iter())
            .map_err(queue_err)?;
        if queued < expected {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "mutation queue overflow: queued {queued} of {expected} operations; call commit() more often"
            )));
        }
        Ok(())
    }

    /// Fast vectorized path for adding a large number of points. Avoids the
    /// per-entity Python object creation overhead of `add_many` for homogeneous
    /// point batches.
    fn add_points(&self, points: Vec<(f64, f64, f64)>, layer: Option<String>) -> PyResult<()> {
        let ops = crate::entities::point_ops(points, layer.unwrap_or_else(|| "0".to_string()));
        let expected = ops.len();
        let mut queue = open_queue()?;
        let queued = queue.push_many(ops.into_iter()).map_err(queue_err)?;
        if queued < expected {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "mutation queue overflow: queued {queued} of {expected} point operations; call commit() more often"
            )));
        }
        Ok(())
    }

    /// Fast vectorized path for adding a large number of lines. Avoids the
    /// per-entity Python object creation overhead of `add_many` for homogeneous
    /// line batches.
    fn add_lines(
        &self,
        lines: Vec<((f64, f64, f64), (f64, f64, f64))>,
        layer: Option<String>,
    ) -> PyResult<()> {
        let ops = crate::entities::line_ops(lines, layer.unwrap_or_else(|| "0".to_string()));
        let expected = ops.len();
        let mut queue = open_queue()?;
        let queued = queue.push_many(ops.into_iter()).map_err(queue_err)?;
        if queued < expected {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "mutation queue overflow: queued {queued} of {expected} line operations; call commit() more often"
            )));
        }
        Ok(())
    }

    fn remove_all(&self) -> PyResult<()> {
        self.ensure_current_no_py()?;
        let doc = self.cached_doc.borrow();
        let doc = doc.as_ref().unwrap();
        let handles: Vec<Handle> = doc.entities().map(|e| e.common().handle).collect();
        let expected = handles.len();
        let ops = handles.into_iter().map(EntityOp::Remove);
        let mut queue = open_queue()?;
        let queued = queue.push_many(ops).map_err(queue_err)?;
        if queued < expected {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "mutation queue overflow: queued {queued} of {expected} remove operations; call commit() more often"
            )));
        }
        Ok(())
    }

    fn commit(&self) -> PyResult<()> {
        send_control_message("REFRESH")
    }
}

impl PyDocument {
    fn ensure_current(&self, _py: Python) -> PyResult<()> {
        let (snapshot_path, _, _) = runtime_paths()?;
        let mut reader = DocumentFullSnapshotReader::open(&snapshot_path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("snapshot open: {e}")))?;
        if reader.has_new_version() || self.cached_doc.borrow().is_none() {
            let (doc, version) = reader
                .refresh()
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("snapshot refresh: {e}")))?;
            *self.cached_doc.borrow_mut() = Some(doc);
            *self.cached_version.borrow_mut() = version;
        }
        Ok(())
    }

    fn ensure_current_no_py(&self) -> PyResult<()> {
        let (snapshot_path, _, _) = runtime_paths()?;
        let mut reader = DocumentFullSnapshotReader::open(&snapshot_path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("snapshot open: {e}")))?;
        if reader.has_new_version() || self.cached_doc.borrow().is_none() {
            let (doc, version) = reader
                .refresh()
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("snapshot refresh: {e}")))?;
            *self.cached_doc.borrow_mut() = Some(doc);
            *self.cached_version.borrow_mut() = version;
        }
        Ok(())
    }
}

pub(crate) fn open_queue() -> PyResult<DocumentMutationView> {
    let (_, queue_path, _) = runtime_paths()?;
    DocumentMutationView::open(&queue_path)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("queue open: {e}")))
}

pub(crate) fn queue_err(e: ocs_plugin_api::shm::QueueError) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(format!("mutation queue: {e}"))
}

/// Return the singleton `ocs.doc` object.
#[pyfunction]
pub fn get_doc(py: Python) -> PyResult<Py<PyDocument>> {
    let doc = PyDocument {
        cached_doc: RefCell::new(None),
        cached_version: RefCell::new(0),
    };
    doc.refresh()?;
    Py::new(py, doc)
}
