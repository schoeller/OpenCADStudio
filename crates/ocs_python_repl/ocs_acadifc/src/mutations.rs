//! Mutation queue wrapper (exposed for advanced use; most users use ocs.doc).

use pyo3::prelude::*;

#[pyclass]
pub struct PyMutationQueue;

#[pymethods]
impl PyMutationQueue {}
