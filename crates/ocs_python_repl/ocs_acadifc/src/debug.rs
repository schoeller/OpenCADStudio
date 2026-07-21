//! Python debugging helpers (`ocs.debug`).
//!
//! Wraps `debugpy` so user scripts can block and wait for the editor debugger
//! to attach before continuing.

use pyo3::prelude::*;

/// Start the debugpy listener on `localhost:port`.
///
/// Typical usage in `main.py`:
///
/// ```python
/// import ocs
/// ocs.debug.start()
/// ocs.debug.wait_for_client()
/// # set breakpoints below this line
/// ```
#[pyfunction]
#[pyo3(signature = (port=5678))]
fn start(port: u16) -> PyResult<()> {
    Python::with_gil(|py| {
        let debugpy = py
            .import_bound("debugpy")
            .map_err(|e| {
                pyo3::exceptions::PyModuleNotFoundError::new_err(format!(
                    "debugpy is not installed or not importable: {e}"
                ))
            })?;
        debugpy
            .call_method1("listen", (("localhost", port),))
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "debugpy.listen(localhost:{port}) failed: {e}"
                ))
            })?;
        Ok(())
    })
}

/// Block until a debugger attaches to the listening port.
#[pyfunction]
fn wait_for_client() -> PyResult<()> {
    Python::with_gil(|py| {
        let debugpy = py
            .import_bound("debugpy")
            .map_err(|e| {
                pyo3::exceptions::PyModuleNotFoundError::new_err(format!(
                    "debugpy is not installed or not importable: {e}"
                ))
            })?;
        debugpy.call_method0("wait_for_client").map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "debugpy.wait_for_client() failed: {e}"
            ))
        })?;
        Ok(())
    })
}

/// Register the `debug` submodule functions.
pub(crate) fn init_module(py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    let debug_mod = PyModule::new_bound(py, "debug")?;
    debug_mod.add_function(wrap_pyfunction!(start, &debug_mod)?)?;
    debug_mod.add_function(wrap_pyfunction!(wait_for_client, &debug_mod)?)?;
    m.add("debug", &debug_mod)?;
    Ok(())
}
