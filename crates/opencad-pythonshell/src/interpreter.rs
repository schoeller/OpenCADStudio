//! Python interpreter backend for the Python Shell plugin.
//!
//! This module spawns a real Python subprocess and forwards REPL input to it.
//! If no Python interpreter can be found, a cheap stub interpreter is provided
//! so the UI still starts and shows a friendly message.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender};

use crate::ocs_module::ocs_module_source;

/// Something that can evaluate Python-like input lines and produce text output.
pub trait Interpreter: Send + Sync {
    /// Send one line of input to the interpreter.
    fn eval(&self, line: &str);

    /// Drain pending output lines produced by the interpreter.
    fn drain_output(&self) -> Vec<String>;
}

/// Error returned when a real Python interpreter cannot be located or started.
#[derive(Debug)]
pub struct InterpreterError {
    pub message: String,
}

impl std::fmt::Display for InterpreterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for InterpreterError {}

/// Real Python interpreter backed by a subprocess.
pub struct PythonInterpreter {
    stdin: Mutex<std::process::ChildStdin>,
    output_rx: Mutex<Receiver<String>>,
    output_tx: Sender<String>,
    python_path: PathBuf,
}

impl PythonInterpreter {
    /// Try to locate a Python executable.
    pub fn find_python() -> Option<PathBuf> {
        for name in ["python3", "python"] {
            if let Ok(path) = which::which(name) {
                return Some(path);
            }
        }
        if cfg!(target_os = "windows") {
            if let Ok(path) = which::which("py") {
                return Some(path);
            }
        }
        None
    }

    /// Spawn a Python subprocess, returning the interpreter and the output receiver.
    pub fn new() -> Result<(Self, Receiver<String>), InterpreterError> {
        let python_path = Self::find_python().ok_or_else(|| InterpreterError {
            message: "No Python interpreter found (tried python3, python, py on Windows)".to_string(),
        })?;
        Self::with_python(&python_path)
    }

    /// Spawn a Python subprocess using the given executable.
    pub fn with_python(python_path: &Path) -> Result<(Self, Receiver<String>), InterpreterError> {
        let module_code = ocs_module_source();
        let startup = format!(
            "exec({}); __name__ = '__interactive__'",
            serde_json::to_string(&module_code)
                .map_err(|e| InterpreterError { message: e.to_string() })?
        );

        let mut child = Command::new(python_path)
            .arg("-u")
            .arg("-i")
            .arg("-c")
            .arg(&startup)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| InterpreterError {
                message: format!("failed to spawn Python: {e}"),
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| InterpreterError {
                message: "failed to open Python stdin".to_string(),
            })?;

        let (output_tx, output_rx) = crossbeam_channel::unbounded::<String>();
        let stdout_tx = output_tx.clone();
        let stderr_tx = output_tx.clone();

        if let Some(stdout) = child.stdout.take() {
            spawn_reader(stdout, stdout_tx);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader(stderr, stderr_tx);
        }

        // Detach a waiter so the child is reaped when it exits.
        std::thread::spawn(move || {
            let _ = child.wait();
        });

        let interpreter = Self {
            stdin: Mutex::new(stdin),
            output_rx: Mutex::new(output_rx.clone()),
            output_tx,
            python_path: python_path.to_path_buf(),
        };
        Ok((interpreter, output_rx))
    }

    fn run_pip(&self, args: &str) {
        let args: Vec<&str> = args.split_whitespace().collect();
        let output = Command::new(&self.python_path)
            .arg("-m")
            .arg("pip")
            .args(&args)
            .output();

        match output {
            Ok(out) => {
                if !out.stdout.is_empty() {
                    let text = String::from_utf8_lossy(&out.stdout);
                    let _ = self.output_tx.send(text.to_string());
                }
                if !out.stderr.is_empty() {
                    let text = String::from_utf8_lossy(&out.stderr);
                    let _ = self.output_tx.send(text.to_string());
                }
                if out.status.success() {
                    let _ = self.output_tx.send("[pip] done.".to_string());
                } else {
                    let _ = self
                        .output_tx
                        .send(format!("[pip] exited with {:?}.", out.status.code()));
                }
            }
            Err(e) => {
                let _ = self.output_tx.send(format!("[pip] failed: {e}"));
            }
        }
    }
}

impl Interpreter for PythonInterpreter {
    fn eval(&self, line: &str) {
        let trimmed = line.trim();
        if trimmed.starts_with("%pip") {
            let rest = trimmed.strip_prefix("%pip").unwrap_or("").trim();
            self.run_pip(rest);
            return;
        }

        if let Ok(mut stdin) = self.stdin.lock() {
            let _ = writeln!(stdin, "{}", line);
        } else {
            let _ = self.output_tx.send("[pyshell] interpreter stdin is closed".to_string());
        }
    }

    fn drain_output(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rx) = self.output_rx.lock() {
            while let Ok(line) = rx.try_recv() {
                out.push(line);
            }
        }
        out
    }
}

fn spawn_reader<R: std::io::Read + Send + 'static>(stream: R, tx: Sender<String>) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(text) => {
                    let _ = tx.send(text);
                }
                Err(_) => break,
            }
        }
    });
}

/// Cheap stub used when no Python interpreter is available.
pub struct StubInterpreter {
    output_rx: Mutex<Receiver<String>>,
    output_tx: Sender<String>,
}

impl StubInterpreter {
    /// Create a stub interpreter. Returns the interpreter and the output receiver.
    pub fn new() -> (Self, Receiver<String>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (
            Self {
                output_rx: Mutex::new(rx.clone()),
                output_tx: tx,
            },
            rx,
        )
    }
}

impl Interpreter for StubInterpreter {
    fn eval(&self, _line: &str) {
        let _ = self
            .output_tx
            .send("Python not found; running in stub mode.".to_string());
    }

    fn drain_output(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rx) = self.output_rx.lock() {
            while let Ok(line) = rx.try_recv() {
                out.push(line);
            }
        }
        out
    }
}

/// Create a real interpreter if Python is available, otherwise a stub.
pub fn create_interpreter() -> (Arc<dyn Interpreter>, Receiver<String>) {
    match PythonInterpreter::new() {
        Ok((interp, rx)) => {
            // PythonInterpreter already owns a clone of rx internally for
            // drain_output; we return the original rx to the caller.
            (Arc::new(interp), rx)
        }
        Err(e) => {
            let (tx, rx) = crossbeam_channel::unbounded();
            let _ = tx.send(format!("Warning: {e}"));
            (Arc::new(StubInterpreter {
                output_rx: Mutex::new(rx.clone()),
                output_tx: tx,
            }), rx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_echoes_stub_message() {
        let (stub, rx) = StubInterpreter::new();
        stub.eval("1 + 1");
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            "Python not found; running in stub mode."
        );
    }

    #[test]
    fn stub_drain_output_works() {
        let (stub, _rx) = StubInterpreter::new();
        stub.eval("a");
        stub.eval("b");
        let out = stub.drain_output();
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|s| s == "Python not found; running in stub mode."));
    }
}
