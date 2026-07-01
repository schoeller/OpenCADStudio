//! Per-`PYTHONEDIT` LSP server thread.
//!
//! Each call spins up a TCP listener on `127.0.0.1:0`, accepts a single
//! connection from `ocs_lsp_bridge.py`, then runs a minimal LSP loop.
//! `workspace/executeCommand` requests are routed to the command table; any
//! host mutation is forwarded through the shared `HostQueue`.

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use crossbeam_channel::bounded;
use lsp_server::{Message, Request, RequestId, Response};
use lsp_types::{
    ExecuteCommandOptions, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
};
use serde_json::{json, Value};

use crate::host_api::{plugin_response_to_py_response, py_request_to_plugin_request};
use crate::host_queue::HostQueue;
use crate::worker::Worker;

const COMMAND_RUN: &str = "ocs.run";
const COMMAND_READ: &str = "ocs.read";
const COMMAND_ERASE: &str = "ocs.erase";
const COMMAND_ERASE_BY_LAYER: &str = "ocs.erase_by_layer";
const COMMAND_ERASE_ALL: &str = "ocs.erase_all";
const COMMAND_DEBUG_START: &str = "ocs.debug.start";
const COMMAND_STATS: &str = "ocs.stats";

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const IDLE_TIMEOUT: Duration = Duration::from_millis(500);

/// Handle to a running LSP server thread bound to one document tab.
pub struct LspServer {
    pub tab: usize,
    pub port: u16,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LspServer {
    /// Bind a listener on localhost, spawn the accept + LSP loop thread, and
    /// return the server handle together with the chosen port.
    pub fn start(
        tab: usize,
        queue: HostQueue,
        worker: Arc<Mutex<Worker>>,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("failed to bind LSP listener: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("failed to get local address: {e}"))?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("failed to set nonblocking listener: {e}"))?;

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel2 = cancel.clone();
        let thread = std::thread::spawn(move || {
            if let Err(e) = run_server(tab, listener, queue, worker, cancel2) {
                eprintln!("[ocs_python_lsp] server for tab {tab} ended: {e}");
            }
        });

        Ok(Self {
            tab,
            port,
            cancel,
            thread: Some(thread),
        })
    }

    /// Signal the server thread to stop and unblock the listener.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        // Connecting to the listener unblocks `accept`.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

impl Drop for LspServer {
    fn drop(&mut self) {
        self.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run_server(
    tab: usize,
    listener: TcpListener,
    queue: HostQueue,
    worker: Arc<Mutex<Worker>>,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let stream = accept_with_cancel(&listener, cancel.clone())?;
    let mut read_stream =
        BufReader::new(stream.try_clone().map_err(|e| format!("stream clone: {e}"))?);
    let mut write_stream = stream;
    let (out_tx, out_rx) = bounded::<Message>(64);

    let write_cancel = cancel.clone();
    let writer = std::thread::spawn(move || {
        while let Ok(msg) = out_rx.recv() {
            if write_cancel.load(Ordering::SeqCst) {
                break;
            }
            if let Err(e) = msg.write(&mut write_stream) {
                eprintln!("[ocs_python_lsp] LSP write error: {e}");
                break;
            }
            let _ = write_stream.flush();
        }
    });

    let mut shutdown = false;
    while !cancel.load(Ordering::SeqCst) {
        match Message::read(&mut read_stream) {
            Ok(Some(Message::Request(req))) => {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let response = handle_request(tab, &req, &queue, &worker);
                if let Some(resp) = response {
                    let _ = out_tx.send(Message::Response(resp));
                }
                if is_shutdown(&req) {
                    shutdown = true;
                }
            }
            Ok(Some(Message::Notification(note))) => {
                if note.method == "exit" {
                    break;
                }
            }
            Ok(Some(Message::Response(_))) => {
                // We never send requests to the client, so responses are ignored.
            }
            Ok(None) => {
                // EOF
                break;
            }
            Err(e) => {
                eprintln!("[ocs_python_lsp] LSP read error: {e}");
                break;
            }
        }
        if shutdown {
            break;
        }
    }

    drop(out_tx);
    let _ = writer.join();
    Ok(())
}

fn accept_with_cancel(listener: &TcpListener, cancel: Arc<AtomicBool>) -> Result<TcpStream, String> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if cancel.load(Ordering::SeqCst) {
                    return Err("listener cancelled".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("accept failed: {e}")),
        }
    }
}

fn is_shutdown(req: &Request) -> bool {
    req.method == "shutdown"
}

fn handle_request(tab: usize, req: &Request, queue: &HostQueue, worker: &Arc<Mutex<Worker>>) -> Option<Response> {
    match req.method.as_str() {
        "initialize" => Some(initialize_response(&req.id)),
        "workspace/executeCommand" => Some(execute_command(tab, &req.id, req.params.clone(), queue, worker)),
        "shutdown" => Some(Response::new_ok(req.id.clone(), Value::Null)),
        _ => Some(Response::new_ok(req.id.clone(), Value::Null)),
    }
}

fn initialize_response(id: &RequestId) -> Response {
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::INCREMENTAL)),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![
                COMMAND_RUN.to_string(),
                COMMAND_READ.to_string(),
                COMMAND_ERASE.to_string(),
                COMMAND_ERASE_BY_LAYER.to_string(),
                COMMAND_ERASE_ALL.to_string(),
                COMMAND_DEBUG_START.to_string(),
                COMMAND_STATS.to_string(),
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = json!({
        "capabilities": capabilities,
        "serverInfo": { "name": "ocs_python_lsp", "version": "0.1.0" }
    });
    Response::new_ok(id.clone(), result)
}

fn execute_command(
    tab: usize,
    id: &RequestId,
    params: Value,
    queue: &HostQueue,
    worker: &Arc<Mutex<Worker>>,
) -> Response {
    let command = params
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);

    let result = match command {
        COMMAND_RUN => run_code(tab, &args, queue, worker),
        COMMAND_READ => read_entities(tab, queue, worker),
        COMMAND_ERASE => erase_entity(tab, &args, queue, worker),
        COMMAND_ERASE_BY_LAYER => erase_by_layer(tab, &args, queue, worker),
        COMMAND_ERASE_ALL => erase_all(tab, queue, worker),
        COMMAND_DEBUG_START => start_debug(tab, &args, queue, worker),
        COMMAND_STATS => get_stats(worker),
        _ => json!({ "error": format!("unknown command: {command}") }),
    };
    Response::new_ok(id.clone(), result)
}

fn run_code(tab: usize, args: &Value, queue: &HostQueue, worker: &Arc<Mutex<Worker>>) -> Value {
    let code_b64 = args
        .get(0)
        .and_then(|a| a.get("code"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let code = match base64::engine::general_purpose::STANDARD.decode(code_b64) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => return json!({ "error": format!("invalid utf-8: {e}") }),
        },
        Err(e) => return json!({ "error": format!("invalid base64: {e}") }),
    };

    let Ok(mut guard) = worker.lock() else {
        return json!({ "error": "worker mutex poisoned" });
    };

    if let Err(e) = guard.send_code(&code) {
        return json!({ "error": format!("failed to send code: {e}") });
    }

    let timeout_secs: u64 = std::env::var("OCS_PYTHON_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    while Instant::now() < deadline {
        if !guard.is_alive() {
            break;
        }
        if let Err(e) = flush_worker_requests(tab, &mut guard, queue) {
            return json!({ "error": e });
        }
        if guard.is_done() {
            let _ = flush_worker_requests(tab, &mut guard, queue);
            let output = guard.output_lines();
            return json!({ "done": true, "output": output });
        }
        if !guard.wait_for_activity(IDLE_TIMEOUT) {
            // No activity in IDLE_TIMEOUT; check once more for done before continuing.
            if guard.is_done() {
                let _ = flush_worker_requests(tab, &mut guard, queue);
                let output = guard.output_lines();
                return json!({ "done": true, "output": output });
            }
        }
    }

    json!({ "error": "Python execution timed out" })
}

fn read_entities(tab: usize, queue: &HostQueue, worker: &Arc<Mutex<Worker>>) -> Value {
    let code = r#"import json; print(json.dumps(list(ocs.doc.entities())))"#;
    let result = run_code(
        tab,
        &json!([{ "code": base64::engine::general_purpose::STANDARD.encode(code) }]),
        queue,
        worker,
    );
    match result.get("output") {
        Some(Value::Array(lines)) if !lines.is_empty() => {
            let last = lines.last().unwrap().as_str().unwrap_or("[]");
            match serde_json::from_str::<Value>(last) {
                Ok(entities) => json!({ "entities": entities, "version": 0 }),
                Err(_) => json!({ "error": "failed to parse entity list" }),
            }
        }
        _ => json!({ "error": "no output from ocs.read" }),
    }
}

fn erase_entity(tab: usize, args: &Value, queue: &HostQueue, worker: &Arc<Mutex<Worker>>) -> Value {
    let handle = args
        .get(0)
        .and_then(|a| a.get("handle"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let code = format!("ocs.erase({handle})");
    run_code(tab,
        &json!([{ "code": base64::engine::general_purpose::STANDARD.encode(&code) }]),
        queue,
        worker,
    )
}

fn erase_by_layer(tab: usize, args: &Value, queue: &HostQueue, worker: &Arc<Mutex<Worker>>) -> Value {
    let layer = args
        .get(0)
        .and_then(|a| a.get("layer"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let code = format!("ocs.erase_by_layer({layer:?})");
    run_code(tab,
        &json!([{ "code": base64::engine::general_purpose::STANDARD.encode(&code) }]),
        queue,
        worker,
    )
}

fn erase_all(tab: usize, queue: &HostQueue, worker: &Arc<Mutex<Worker>>) -> Value {
    run_code(tab,
        &json!([{ "code": base64::engine::general_purpose::STANDARD.encode("ocs.erase_all()") }]),
        queue,
        worker,
    )
}

fn start_debug(tab: usize, args: &Value, queue: &HostQueue, worker: &Arc<Mutex<Worker>>) -> Value {
    let port = args
        .get(0)
        .and_then(|a| a.get("port"))
        .and_then(|v| v.as_u64())
        .unwrap_or(5678) as u16;
    let code = format!("ocs.debug.start({port})");
    run_code(tab,
        &json!([{ "code": base64::engine::general_purpose::STANDARD.encode(&code) }]),
        queue,
        worker,
    )
}

fn get_stats(worker: &Arc<Mutex<Worker>>) -> Value {
    let Ok(guard) = worker.lock() else {
        return json!({ "error": "worker mutex poisoned" });
    };
    let code = r#"import json; print(json.dumps(ocs.counts()))"#;
    if let Err(e) = guard.send_code(&base64::engine::general_purpose::STANDARD.encode(code)) {
        return json!({ "error": format!("failed to send stats request: {e}") });
    }
    // Allow a short moment for the worker to respond.
    let _ = guard.wait_for_activity(Duration::from_millis(200));
    let lines = guard.output_lines();
    match lines.last().and_then(|l| serde_json::from_str::<Value>(l).ok()) {
        Some(stats) => stats,
        None => json!({ "written": 0, "erased": 0 }),
    }
}

fn flush_worker_requests(tab: usize, worker: &mut Worker, queue: &HostQueue) -> Result<(), String> {
    for req in worker.take_requests() {
        let plugin_req = py_request_to_plugin_request(req)?;
        let resp = queue.request(tab, plugin_req)?;
        let py_resp = plugin_response_to_py_response(resp, 0, 0)?;
        worker.send_response(&py_resp).map_err(|e| e.to_string())?;
    }
    Ok(())
}


