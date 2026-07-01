//! Python child process shared by all LSP server threads.
//!
//! Adapted from `ocs_pythonshell`; the worker runs a single Python interpreter
//! with the embedded `ocs` bootstrap. `stdout` carries REPL output, `stderr`
//! carries JSON host API requests, and `stdin` carries code to execute plus
//! `__ocs_resp__` JSON replies.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json;

use crate::host_api::{PyRequest, PyResponse};

const DONE_MARKER: &str = "__ocs_done__";
const MAX_OUTPUT_LINES: usize = 500;
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Shared state between the Python reader threads and the server threads.
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
pub struct Worker {
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    shared: Arc<SharedState>,
    readers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl Worker {
    fn new(
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
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            shared,
            readers: Mutex::new(vec![stdout_reader, stderr_reader]),
        }
    }

    pub fn is_alive(&self) -> bool {
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

    pub fn send_code(&self, code: &str) -> std::io::Result<()> {
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

    pub fn send_response(&self, resp: &PyResponse) -> std::io::Result<()> {
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

    pub fn output_lines(&self) -> Vec<String> {
        let out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.lines.iter().cloned().collect()
    }

    pub fn take_requests(&self) -> Vec<PyRequest> {
        let mut out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut out.requests)
    }

    pub fn is_done(&self) -> bool {
        let out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.done
    }

    fn reset_done(&self) {
        let mut out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.done = false;
    }

    pub fn wait_for_activity(&self, timeout: Duration) -> bool {
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

    pub fn close(&self) {
        let _ = self.stdin.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let readers = std::mem::take(&mut *self.readers.lock().unwrap_or_else(|e| e.into_inner()));
        for handle in readers {
            // Join the reader threads with a short timeout. If the child process
            // (or a grandchild it spawned) keeps the stdout/stderr pipes open,
            // `read_line` can block forever; detaching the thread prevents
            // `close()` from hanging the host/test.
            let deadline = Instant::now() + Duration::from_secs(2);
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = handle.join();
        }
        let mut out = self.shared.output.lock().unwrap_or_else(|e| e.into_inner());
        out.alive = false;
    }
}

fn push_line(lines: &mut VecDeque<String>, line: String) {
    if lines.len() >= MAX_OUTPUT_LINES {
        lines.pop_front();
    }
    lines.push_back(line);
}

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

pub fn python_command() -> Option<Command> {
    if let Ok(p) = std::env::var("OCS_PYTHON_EXE") {
        if p.is_empty() {
            return None;
        }
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

#[allow(dead_code)]
pub fn python_available() -> bool {
    python_command().is_some()
}

pub fn spawn_python_worker() -> Result<Worker, String> {
    let mut cmd = python_command().ok_or("Python interpreter not found")?;
    let mut child = cmd
        .arg("-u")
        .arg("-c")
        .arg(crate::bootstrap::BOOTSTRAP)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn Python: {e}"))?;

    let stdin = child.stdin.take().ok_or("missing stdin")?;
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    Ok(Worker::new(child, stdin, stdout, stderr))
}
