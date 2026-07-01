//! `debugpy` integration for the embedded Python worker.

/// Attempt to start a debugpy listener on `port` from inside the Python worker.
#[allow(dead_code)]
pub fn start(port: u16) -> Result<(), String> {
    // This function is invoked by the Python worker via `ocs.debug.start(port)`.
    // It returns Ok once the Python side has arranged for debugpy to listen.
    // The actual implementation is in the bootstrap script; this Rust helper is
    // a placeholder so the command table can report success/failure uniformly.
    Err(format!(
        "debugpy is not installed or not configured. Install it with `pip install debugpy` and retry on port {port}."
    ))
}
