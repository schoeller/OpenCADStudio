//! Process management for out-of-process plugins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Listener;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, ToNsName};

use crate::host::{AsyncSessionError, AsyncSessionHandle, CommandStep, DocumentReader, HostApi};
use crate::ipc::protocol::{
    Handle, HostRequest, HostResponse, HostToPlugin, InteractiveEvent, PluginRequest, PluginResponse,
    PluginToHost, RunnerHandshake, PLUGIN_TOKEN_ENV,
};
use crate::ipc::server::handle_plugin_request;
use crate::ipc::transport::{recv, send};
use crate::ribbon::owned::{OwnedPluginManifest, OwnedRibbonGroup as OwnedRibbonGroupAlias};
use crate::shm::{DocumentViewInfo, SharedDocumentReader};

use serde::de::DeserializeOwned;

mod manager;
pub use manager::{DispatchResult, PluginManager};

/// Maximum time to wait for the plugin runner to connect back to the host.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);

fn spawn_timeout() -> Duration {
    std::env::var("OCS_PLUGIN_SPAWN_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(SPAWN_TIMEOUT)
}

/// Default maximum time to wait for a plugin call to respond.
const CALL_TIMEOUT_DEFAULT: Duration = Duration::from_secs(30);

/// Length of the random pre-shared token used to authenticate the runner.
const PLUGIN_TOKEN_LEN: usize = 32;

fn call_timeout() -> Duration {
    std::env::var("OCS_PLUGIN_CALL_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(CALL_TIMEOUT_DEFAULT)
}

/// Per-request-kind timeout floors. The user-configured default is raised to
/// these minima so that no request kind can be configured into an unsafe value.
fn request_timeout(kind: &'static str) -> Duration {
    base_max_floor(call_timeout(), kind)
}

fn base_max_floor(base: Duration, kind: &'static str) -> Duration {
    // Tests lower the floor via OCS_PLUGIN_TEST_FLOOR_SECS so the suite does not
    // wait out the real 10 s Dispatch minimum. The seam is compiled in only
    // under cfg(test); production always enforces the safety floors below.
    #[cfg(test)]
    if let Some(secs) = std::env::var("OCS_PLUGIN_TEST_FLOOR_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        return base.max(Duration::from_secs(secs));
    }
    let floor = match kind {
        "GetManifest" | "GetRibbon" => Duration::from_secs(5),
        "Dispatch" => Duration::from_secs(10),
        "InteractiveEvent" | "GetPrompt" | "NeedsEntityPick" => Duration::from_secs(2),
        _ => Duration::from_secs(1),
    };
    base.max(floor)
}

fn request_kind(req: &HostRequest) -> &'static str {
    match req {
        HostRequest::GetManifest => "GetManifest",
        HostRequest::GetRibbon => "GetRibbon",
        HostRequest::Dispatch { .. } => "Dispatch",
        HostRequest::InteractiveEvent { .. } => "InteractiveEvent",
        HostRequest::GetPrompt { .. } => "GetPrompt",
        HostRequest::NeedsEntityPick { .. } => "NeedsEntityPick",
        HostRequest::EndAsyncSession { .. } => "EndAsyncSession",
        HostRequest::AsyncSessionRequest { .. } => "AsyncSessionRequest",
        HostRequest::Shutdown => "Shutdown",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport error: {0}")]
    Transport(#[from] crate::ipc::transport::TransportError),
    #[error("plugin runner error: {0}")]
    Runner(String),
    #[error("spawn timeout: runner did not connect within {0:?}")]
    SpawnTimeout(Duration),
    #[error("call timeout: {request} did not respond within {duration:?}")]
    CallTimeout {
        request: &'static str,
        duration: Duration,
    },
    #[error("runner exited before connecting")]
    RunnerExited,
    #[error("unexpected response: {0:?}")]
    UnexpectedResponse(HostResponse),
}

/// Host-side state for API V3 async sessions.
struct V3State {
    /// Per-session queues of plugin requests waiting for the host adapter.
    /// Each entry carries the V3 `request_id` the adapter must use when sending
    /// the response.
    queues: Arc<Mutex<HashMap<String, Vec<(u64, crate::ipc::protocol::PluginRequest)>>>>,
    /// Pending response channels for host-initiated V3 requests.
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<crate::ipc::protocol::HostResponse>>>>,
}

/// One spawned plugin process.
pub struct PluginProcess {
    stream: Arc<Mutex<Option<Stream>>>,
    child: Arc<Mutex<Option<Child>>>,
    id: String,
    manifest: OwnedPluginManifest,
    ribbon: Vec<OwnedRibbonGroupAlias>,
    next_request_id: AtomicU64,
    v3_state: Mutex<Option<V3State>>,
}

impl PluginProcess {
    /// Spawn the plugin cdylib in a separate process and connect to it.
    pub fn spawn(cdylib_path: &Path, host: &mut dyn HostApi) -> Result<Self, PluginError> {
        let socket_name = generate_socket_name();
        let socket_name_ref: interprocess::local_socket::Name = socket_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("valid namespaced name");
        let runner_path = runner_executable()?;
        eprintln!(
            "[plugin] spawning runner {} for {}",
            runner_path.display(),
            cdylib_path.display()
        );

        let token = generate_token()?;

        // Create the listener before spawning so the runner can connect immediately.
        let listener = ListenerOptions::new().name(socket_name_ref).create_sync()?;

        let child = Command::new(&runner_path)
            .arg("--ocs-plugin-runner")
            .arg(&socket_name)
            .arg(cdylib_path)
            .env(PLUGIN_TOKEN_ENV, &token)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let child = Arc::new(Mutex::new(Some(child)));

        // Accept the runner connection with a timeout so a hung/crashed runner
        // does not block the host indefinitely.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(listener.accept());
        });
        let stream = match rx.recv_timeout(spawn_timeout()) {
            Ok(Ok(stream)) => {
                eprintln!("[plugin] runner connected");
                Arc::new(Mutex::new(Some(stream)))
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    reap(child);
                }
                return Err(PluginError::SpawnTimeout(spawn_timeout()));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    reap(child);
                }
                return Err(PluginError::RunnerExited);
            }
        };

        // Verify the runner presented the token it received through the
        // environment before allowing any host→runner requests. The read is
        // bounded by a deadline: `accept` above guards the connect, and this
        // guards the first frame, so a process that connects but never sends —
        // or a runner that dies mid-handshake — cannot block the host forever.
        let handshake_timeout = spawn_timeout();
        let handshake_deadline = Instant::now() + handshake_timeout;
        let handshake = recv_with_deadline::<RunnerHandshake>(
            &stream,
            &child,
            handshake_deadline,
            handshake_timeout,
            "Handshake",
        )?;
        if let Err(e) = verify_runner_handshake(handshake, &token) {
            mark_dead(&stream, &child);
            return Err(e);
        }

        // The runner first answers GetManifest and GetRibbon so the host can
        // build the UI without keeping the plugin object alive.
        let no_op = &mut |_| {};
        let manifest = match call(&stream, &child, host, HostRequest::GetManifest, no_op)? {
            HostResponse::Manifest(m) => m,
            other => return Err(PluginError::UnexpectedResponse(other)),
        };
        let ribbon = match call(&stream, &child, host, HostRequest::GetRibbon, no_op)? {
            HostResponse::Ribbon(r) => r,
            other => return Err(PluginError::UnexpectedResponse(other)),
        };

        let id = manifest.id.clone();
        Ok(Self {
            stream,
            child,
            id,
            manifest,
            ribbon,
            next_request_id: AtomicU64::new(1),
            v3_state: Mutex::new(None),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn manifest(&self) -> &OwnedPluginManifest {
        &self.manifest
    }

    pub fn ribbon(&self) -> &[OwnedRibbonGroupAlias] {
        &self.ribbon
    }

    pub fn dispatch(
        &self,
        host: &mut dyn HostApi,
        cmd: &str,
        on_start_interactive: &mut dyn FnMut(u64),
    ) -> Result<bool, PluginError> {
        eprintln!("[plugin] dispatching {cmd}");
        let result = match call(
            &self.stream,
            &self.child,
            host,
            HostRequest::Dispatch {
                cmd: cmd.to_string(),
            },
            on_start_interactive,
        )? {
            HostResponse::Bool(b) => Ok(b),
            other => Err(PluginError::UnexpectedResponse(other)),
        };
        eprintln!("[plugin] dispatch {cmd} result: {result:?}");
        result
    }

    /// Dispatch a command to an API V3 plugin. Returns whether the command was
    /// handled and, if the plugin started an async session, the session ID.
    pub fn dispatch_v3(
        &self,
        host: &mut dyn HostApi,
        cmd: &str,
    ) -> Result<(bool, Option<String>), PluginError> {
        eprintln!("[plugin v3] dispatching {cmd}");
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
            let stream = guard.as_mut().ok_or_else(shutdown_error)?;
            send(
                stream,
                &HostToPlugin::RequestV3 {
                    request_id,
                    session_id: String::new(),
                    request: HostRequest::Dispatch {
                        cmd: cmd.to_string(),
                    },
                },
            )?;
        }

        let mut started_session: Option<String> = None;
        let kind = "Dispatch";
        let timeout = request_timeout(kind);
        let deadline = Instant::now() + timeout;
        let result = loop {
            let msg = recv_with_deadline::<PluginToHost>(
                &self.stream,
                &self.child,
                deadline,
                timeout,
                kind,
            )?;
            match msg {
                PluginToHost::ResponseV3 {
                    request_id: rid,
                    response,
                } if rid == request_id => match response {
                    HostResponse::Bool(b) => {
                        eprintln!("[plugin v3] dispatch {cmd} result: {b} async={started_session:?}");
                        break Ok((b, started_session.clone()));
                    }
                    other => break Err(PluginError::UnexpectedResponse(other)),
                },
                PluginToHost::ResponseV3 { .. } => continue,
                PluginToHost::RequestV3 {
                    request_id: rid,
                    session_id: _,
                    request: PluginRequest::StartAsyncSession { session_id },
                } => {
                    let resp = match host.start_async_session(&session_id) {
                        Some(_) => {
                            started_session = Some(session_id);
                            PluginResponse::Ok
                        }
                        None => PluginResponse::Error(
                            "host rejected async session".to_string(),
                        ),
                    };
                    self.send_async_response_v3(rid, resp)?;
                }
                PluginToHost::RequestV3 {
                    request_id: rid,
                    session_id: _,
                    request,
                } => {
                    let mut noop = |_id: u64| {};
                    let resp = handle_plugin_request(host, request, &mut noop);
                    self.send_async_response_v3(rid, resp)?;
                }
                _ => {}
            }
        };

        // Start the long-lived host-side reader only after the synchronous
        // dispatch exchange has finished, to avoid races over the stream.
        if started_session.is_some() {
            self.ensure_v3_reader();
        }
        result
    }

    fn ensure_v3_reader(&self) {
        let mut state_guard = self.v3_state.lock().unwrap_or_else(|e| e.into_inner());
        if state_guard.is_some() {
            return;
        }
        let queues: Arc<Mutex<HashMap<String, Vec<(u64, PluginRequest)>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<HostResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let state = V3State {
            queues: Arc::clone(&queues),
            pending: Arc::clone(&pending),
        };
        let stream = Arc::clone(&self.stream);
        let child = Arc::clone(&self.child);
        std::thread::spawn(move || {
            loop {
                let msg = match recv_one::<PluginToHost>(&stream) {
                    Ok(m) => m,
                    Err(_) => break,
                };
                match msg {
                    PluginToHost::RequestV3 {
                        request_id: rid,
                        session_id,
                        request,
                    } => {
                        let mut q = queues.lock().unwrap();
                        q.entry(session_id).or_default().push((rid, request));
                    }
                    PluginToHost::ResponseV3 {
                        request_id: rid,
                        response,
                    } => {
                        let tx = pending.lock().unwrap().remove(&rid);
                        if let Some(tx) = tx {
                            let _ = tx.send(response);
                        }
                    }
                    _ => {}
                }
            }
            // Process died or stream closed; clear pending so waiters wake.
            let _ = child.lock().unwrap_or_else(|e| e.into_inner()).take();
            let txs = std::mem::take(&mut *pending.lock().unwrap());
            for (_, tx) in txs {
                let _ = tx.send(HostResponse::Error("session closed".to_string()));
            }
        });
        *state_guard = Some(state);
    }

    pub fn send_async_response_v3(
        &self,
        request_id: u64,
        response: PluginResponse,
    ) -> Result<(), PluginError> {
        let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream = guard.as_mut().ok_or_else(shutdown_error)?;
        send(
            stream,
            &HostToPlugin::ResponseV3 {
                request_id,
                response,
            },
        )
        .map_err(Into::into)
    }

    /// Send a host request within an async session and wait for the response.
    pub fn request_v3(
        &self,
        session_id: &str,
        req: HostRequest,
    ) -> Result<HostResponse, PluginError> {
        // Make sure the host-side V3 reader thread is running so responses can
        // be routed back to this call.
        self.ensure_v3_reader();

        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut guard = self.v3_state.lock().unwrap_or_else(|e| e.into_inner());
            let state = guard.as_mut().expect("ensure_v3_reader initialized state");
            state.pending.lock().unwrap().insert(request_id, tx);
        }
        {
            let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
            let stream = guard.as_mut().ok_or_else(shutdown_error)?;
            send(
                stream,
                &HostToPlugin::RequestV3 {
                    request_id,
                    session_id: session_id.to_string(),
                    request: req,
                },
            )?;
        }
        match rx.recv_timeout(request_timeout("AsyncRequest")) {
            Ok(resp) => Ok(resp),
            Err(_) => Err(PluginError::CallTimeout {
                request: "AsyncRequest",
                duration: request_timeout("AsyncRequest"),
            }),
        }
    }

    /// Drain pending plugin requests for `session_id`.
    pub fn drain_async_requests(&self, session_id: &str) -> Vec<(u64, PluginRequest)> {
        let guard = self.v3_state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(state) = guard.as_ref() else {
            return Vec::new();
        };
        let mut q = state.queues.lock().unwrap();
        q.remove(session_id).unwrap_or_default()
    }

    /// Create a host-side async session handle for `session_id`.
    pub fn async_session_handle(
        self: &Arc<Self>,
        tab: usize,
        session_id: String,
    ) -> Box<dyn AsyncSessionHandle> {
        Box::new(PluginAsyncSessionHandle {
            process: Arc::clone(self),
            tab,
            session_id,
        })
    }

    /// Send an async session-end notification to the plugin runner.
    pub fn send_end_session(&self, session_id: &str) -> Result<(), PluginError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream = guard.as_mut().ok_or_else(shutdown_error)?;
        send(
            stream,
            &HostToPlugin::RequestV3 {
                request_id,
                session_id: session_id.to_string(),
                request: HostRequest::EndAsyncSession {
                    session_id: session_id.to_string(),
                },
            },
        )
        .map_err(Into::into)
    }

    /// Send an interactive event for `command_id` and return the step the
    /// plugin command produces. Interactive events are not expected to trigger
    /// nested host API calls, so this path does not supply a `HostApi`.
    pub fn interactive_event(
        &self,
        command_id: u64,
        event: InteractiveEvent,
    ) -> Result<CommandStep, PluginError> {
        self.send_request(HostRequest::InteractiveEvent { command_id, event })?;
        let kind = "InteractiveEvent";
        let timeout = request_timeout(kind);
        let deadline = Instant::now() + timeout;
        loop {
            match recv_with_deadline::<PluginToHost>(
                &self.stream,
                &self.child,
                deadline,
                timeout,
                kind,
            )? {
                PluginToHost::Response(HostResponse::CommandStep(s)) => return Ok(s),
                PluginToHost::Response(other) => {
                    return Err(PluginError::UnexpectedResponse(other))
                }
                PluginToHost::Request(req) => {
                    let resp = crate::ipc::protocol::PluginResponse::Error(format!(
                        "unexpected nested request during interactive event: {req:?}"
                    ));
                    self.send_response(resp)?;
                }
                PluginToHost::RequestV3 { .. } | PluginToHost::ResponseV3 { .. } => {
                    let resp = crate::ipc::protocol::PluginResponse::Error(
                        "unexpected API v3 message during interactive event".to_string(),
                    );
                    self.send_response(resp)?;
                }
            }
        }
    }

    /// Ask the plugin process for the current prompt of an interactive command.
    pub fn get_prompt(&self, command_id: u64) -> Result<String, PluginError> {
        self.send_request(HostRequest::GetPrompt { command_id })?;
        let kind = "GetPrompt";
        let timeout = request_timeout(kind);
        let deadline = Instant::now() + timeout;
        loop {
            match recv_with_deadline::<PluginToHost>(
                &self.stream,
                &self.child,
                deadline,
                timeout,
                kind,
            )? {
                PluginToHost::Response(HostResponse::Text(s)) => return Ok(s),
                PluginToHost::Response(other) => {
                    return Err(PluginError::UnexpectedResponse(other))
                }
                PluginToHost::Request(req) => {
                    let resp = crate::ipc::protocol::PluginResponse::Error(format!(
                        "unexpected nested request during get_prompt: {req:?}"
                    ));
                    self.send_response(resp)?;
                }
                PluginToHost::RequestV3 { .. } | PluginToHost::ResponseV3 { .. } => {
                    let resp = crate::ipc::protocol::PluginResponse::Error(
                        "unexpected API v3 message during get_prompt".to_string(),
                    );
                    self.send_response(resp)?;
                }
            }
        }
    }

    /// Ask the plugin process whether an interactive command wants object picks.
    pub fn needs_entity_pick(&self, command_id: u64) -> Result<bool, PluginError> {
        self.send_request(HostRequest::NeedsEntityPick { command_id })?;
        let kind = "NeedsEntityPick";
        let timeout = request_timeout(kind);
        let deadline = Instant::now() + timeout;
        loop {
            match recv_with_deadline::<PluginToHost>(
                &self.stream,
                &self.child,
                deadline,
                timeout,
                kind,
            )? {
                PluginToHost::Response(HostResponse::Bool(b)) => return Ok(b),
                PluginToHost::Response(other) => {
                    return Err(PluginError::UnexpectedResponse(other))
                }
                PluginToHost::Request(req) => {
                    let resp = crate::ipc::protocol::PluginResponse::Error(format!(
                        "unexpected nested request during needs_entity_pick: {req:?}"
                    ));
                    self.send_response(resp)?;
                }
                PluginToHost::RequestV3 { .. } | PluginToHost::ResponseV3 { .. } => {
                    let resp = crate::ipc::protocol::PluginResponse::Error(
                        "unexpected API v3 message during needs_entity_pick".to_string(),
                    );
                    self.send_response(resp)?;
                }
            }
        }
    }

    /// Return the OS process id for the plugin child process, if still held.
    pub fn process_id(&self) -> Option<u32> {
        let guard = self.child.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(|child| child.id())
    }

    /// Kill the plugin child process and take its resources.
    pub fn shutdown_all(&self) {
        let (stream, mut child) = self.take_resources();
        drop(stream);
        if let Some(mut child) = child.take() {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    pub fn is_alive(&self) -> bool {
        let mut guard = self.child.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                Ok(Some(_)) | Err(_) => false,
            },
            None => false,
        }
    }

    /// Tear down the plugin process without blocking the caller. The stream is
    /// closed and the child is killed synchronously; the blocking `wait()` is
    /// done in a detached background thread so the host never waits on a plugin.
    pub fn shutdown(&self) {
        let (stream, child) = self.take_resources();
        drop(stream);
        if let Some(child) = child {
            reap(child);
        }
    }

    /// Take the stream and child handles out of the process. After this the
    /// process is considered shut down and any further IPC will fail.
    fn take_resources(&self) -> (Option<Stream>, Option<Child>) {
        let stream = self.stream.lock().unwrap_or_else(|e| e.into_inner()).take();
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        (stream, child)
    }
}

/// Host-side handle for a V3 async session.
struct PluginAsyncSessionHandle {
    process: Arc<PluginProcess>,
    tab: usize,
    session_id: String,
}

impl AsyncSessionHandle for PluginAsyncSessionHandle {
    fn tab_index(&self) -> usize {
        self.tab
    }

    fn request(&self, req: PluginRequest) -> Result<PluginResponse, AsyncSessionError> {
        match self
            .process
            .request_v3(&self.session_id, HostRequest::AsyncSessionRequest { request: req })
        {
            Ok(HostResponse::AsyncSessionResponse(resp)) => Ok(resp),
            Ok(other) => Err(AsyncSessionError::Transport(format!(
                "unexpected async session response: {other:?}"
            ))),
            Err(e) => Err(AsyncSessionError::Transport(e.to_string())),
        }
    }

    fn document_reader(&self) -> Box<dyn DocumentReader + 'static> {
        match self.request(PluginRequest::OpenDocumentView) {
            Ok(PluginResponse::DocumentView { path, version: _ }) => {
                if path.is_empty() {
                    return Box::new(EmptyDocumentReader);
                }
                match SharedDocumentReader::open(Path::new(&path)) {
                    Ok(reader) => Box::new(reader),
                    Err(e) => {
                        eprintln!("[plugin] shared reader open failed: {e}");
                        Box::new(EmptyDocumentReader)
                    }
                }
            }
            other => {
                eprintln!("[plugin] document_reader failed: {other:?}");
                Box::new(EmptyDocumentReader)
            }
        }
    }

    fn document_view(&self) -> Option<DocumentViewInfo> {
        match self.request(PluginRequest::OpenDocumentView) {
            Ok(PluginResponse::DocumentView { path, version }) => {
                Some(DocumentViewInfo { path, version })
            }
            _ => None,
        }
    }
}

/// Sentinel reader used when the shared-memory view could not be initialized.
struct EmptyDocumentReader;

impl DocumentReader for EmptyDocumentReader {
    fn entity_count(&self) -> usize {
        0
    }
    fn for_each_entity(&self, _f: &mut dyn FnMut(crate::host::ReaderEntity<'_>)) {}
    fn layer_name(&self, _handle: Handle) -> Option<&str> {
        None
    }
    fn app_id_name(&self, _handle: Handle) -> Option<&str> {
        None
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl PluginProcess {
    fn send_request(&self, req: HostRequest) -> Result<(), PluginError> {
        let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream = guard.as_mut().ok_or_else(shutdown_error)?;
        send(stream, &HostToPlugin::Request(req)).map_err(Into::into)
    }

    fn send_response(&self, resp: crate::ipc::protocol::PluginResponse) -> Result<(), PluginError> {
        let mut guard = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream = guard.as_mut().ok_or_else(shutdown_error)?;
        send(stream, &HostToPlugin::Response(resp)).map_err(Into::into)
    }
}

/// Kill a child process and reap it without blocking the caller. The blocking
/// `wait()` runs in a detached thread so the host never stalls on a plugin, and
/// the child is reaped rather than left as a zombie on Unix.
fn reap(mut child: Child) {
    let _ = child.kill();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

fn shutdown_error() -> PluginError {
    PluginError::Io(std::io::Error::new(
        std::io::ErrorKind::NotConnected,
        "plugin process has been shut down",
    ))
}

/// Take the stream and child away from a process and kill the child without
/// blocking the caller. After this the process is considered dead and any
/// further IPC will fail.
fn mark_dead(stream: &Mutex<Option<Stream>>, child: &Mutex<Option<Child>>) {
    // On a timeout the live `Stream` is owned by the detached reader thread, so
    // this `take()` usually clears an already-`None` slot. The host end is not
    // closed here directly: killing the child below shuts its socket end, which
    // unblocks the reader thread's `recv` and lets it drop the `Stream`. If the
    // kill fails the reader thread can stay parked until the OS tears the socket
    // down, but the process is still treated as dead for all further IPC.
    let _ = stream.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
        reap(child);
    }
}

/// Receive one message from the plugin runner with a deadline.
///
/// A short-lived reader thread performs the blocking `recv` so that the main
/// thread can time it out. If the deadline passes, the process is marked dead
/// (stream closed, child killed) so that subsequent dispatch attempts are
/// skipped.
/// Receive one message from `stream` without holding the mutex while blocking.
/// The stream is taken out, read on a helper thread, and then restored.
fn recv_one<T: DeserializeOwned + Send + 'static>(
    stream: &Arc<Mutex<Option<Stream>>>,
) -> Result<T, PluginError> {
    let (tx, rx) = mpsc::channel();
    let stream_to_thread = stream.lock().unwrap_or_else(|e| e.into_inner()).take();
    std::thread::spawn(move || {
        let result = match stream_to_thread {
            Some(mut s) => match recv::<T>(&mut s) {
                Ok(m) => (Ok(m), Some(s)),
                Err(e) => (Err(PluginError::from(e)), Some(s)),
            },
            None => (Err(shutdown_error()), None),
        };
        let _ = tx.send(result);
    });
    match rx.recv() {
        Ok((Ok(msg), stream_opt)) => {
            *stream.lock().unwrap_or_else(|e| e.into_inner()) = stream_opt;
            Ok(msg)
        }
        Ok((Err(e), stream_opt)) => {
            *stream.lock().unwrap_or_else(|e| e.into_inner()) = stream_opt;
            Err(e)
        }
        Err(_) => Err(shutdown_error()),
    }
}

fn recv_with_deadline<T: DeserializeOwned + Send + 'static>(
    stream: &Mutex<Option<Stream>>,
    child: &Mutex<Option<Child>>,
    deadline: Instant,
    timeout: Duration,
    request: &'static str,
) -> Result<T, PluginError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        mark_dead(stream, child);
        return Err(PluginError::CallTimeout {
            request,
            duration: timeout,
        });
    }

    let (tx, rx) = mpsc::channel::<(Result<T, PluginError>, Option<Stream>)>();
    let stream_to_thread = stream.lock().unwrap_or_else(|e| e.into_inner()).take();

    std::thread::spawn(move || {
        let result = match stream_to_thread {
            Some(mut stream) => match recv::<T>(&mut stream) {
                Ok(msg) => (Ok(msg), Some(stream)),
                Err(e) => (Err(PluginError::from(e)), Some(stream)),
            },
            None => (Err(shutdown_error()), None),
        };
        let _ = tx.send(result);
    });

    match rx.recv_timeout(remaining) {
        Ok((Ok(msg), stream_opt)) => {
            *stream.lock().unwrap_or_else(|e| e.into_inner()) = stream_opt;
            Ok(msg)
        }
        Ok((Err(e), stream_opt)) => {
            *stream.lock().unwrap_or_else(|e| e.into_inner()) = stream_opt;
            Err(e)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            mark_dead(stream, child);
            Err(PluginError::CallTimeout {
                request,
                duration: timeout,
            })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(shutdown_error()),
    }
}

/// Send a host request and wait for the response, handling any nested plugin
/// requests inline using the supplied `HostApi`.
fn call(
    stream: &Mutex<Option<Stream>>,
    child: &Mutex<Option<Child>>,
    host: &mut dyn HostApi,
    req: HostRequest,
    on_start_interactive: &mut dyn FnMut(u64),
) -> Result<HostResponse, PluginError> {
    let kind = request_kind(&req);
    let timeout = request_timeout(kind);
    let deadline = Instant::now() + timeout;
    eprintln!("[plugin] host -> runner: {req:?}");
    {
        let mut guard = stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream = guard.as_mut().ok_or_else(shutdown_error)?;
        send(stream, &HostToPlugin::Request(req))?;
    }
    loop {
        let msg = recv_with_deadline::<PluginToHost>(stream, child, deadline, timeout, kind)?;
        eprintln!("[plugin] runner -> host: {msg:?}");
        match msg {
            PluginToHost::Response(resp) => return Ok(resp),
            PluginToHost::Request(plugin_req) => {
                let resp = handle_plugin_request(host, plugin_req, on_start_interactive);
                eprintln!("[plugin] host -> runner response: {resp:?}");
                let mut guard = stream.lock().unwrap_or_else(|e| e.into_inner());
                let stream = guard.as_mut().ok_or_else(shutdown_error)?;
                send(stream, &HostToPlugin::Response(resp))?;
            }
            PluginToHost::RequestV3 { .. } | PluginToHost::ResponseV3 { .. } => {
                return Err(PluginError::Runner(
                    "unexpected API v3 message on v2 call path".to_string(),
                ));
            }
        }
    }
}

/// Locate the executable to spawn for running a plugin.
///
/// The host spawns *itself* in runner mode (`--ocs-plugin-runner`), so the
/// runner is always available and stays in sync with the host binary. This
/// avoids shipping a separate `ocs_plugin_runner` binary and works the same on
/// Windows, macOS, and Linux.
///
/// For testing or unusual deployment layouts, set `OCS_PLUGIN_RUNNER_EXE` to
/// the host executable path.
fn runner_executable() -> Result<PathBuf, PluginError> {
    static RUNNER: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut cached = RUNNER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref path) = *cached {
        return Ok(path.clone());
    }

    let path = if let Ok(path) = std::env::var("OCS_PLUGIN_RUNNER_EXE") {
        let path = PathBuf::from(path);
        if path.exists() {
            path
        } else {
            return Err(PluginError::Runner(format!(
                "OCS_PLUGIN_RUNNER_EXE does not exist: {}",
                path.display()
            )));
        }
    } else {
        let host = std::env::current_exe()?;
        if !host.exists() {
            return Err(PluginError::Runner(format!(
                "cannot find current executable at {}",
                host.display()
            )));
        }

        // Create a hard link with a distinct name next to the host binary. This
        // makes runner processes visible as separate sub-processes in task
        // managers / ps, while keeping the runner the exact same binary as the
        // host so they can never drift out of sync.
        let runner = distinct_runner_path(&host);
        let _ = std::fs::remove_file(&runner);
        match std::fs::hard_link(&host, &runner) {
            Ok(()) => runner,
            Err(_) => host,
        }
    };

    *cached = Some(path.clone());
    Ok(path)
}

/// Build a runner path like `<host>-plugin-runner<ext>` in the same directory.
/// Using a distinct image name lets task managers show plugin processes as
/// children/sub-processes of the host instead of collapsing them into one row.
fn distinct_runner_path(host: &Path) -> PathBuf {
    let mut runner = host.as_os_str().to_owned();
    if let Some(ext) = host.extension().and_then(|s| s.to_str()) {
        let base = host.file_stem().unwrap_or_default();
        runner =
            std::ffi::OsString::from(format!("{}-plugin-runner.{}", base.to_string_lossy(), ext));
    } else {
        runner.push("-plugin-runner");
    }
    let mut path = host
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    path.push(runner);
    path
}

/// Verify that the runner's `handshake` presents `expected_token`.
fn verify_runner_handshake(
    handshake: RunnerHandshake,
    expected_token: &str,
) -> Result<(), PluginError> {
    match handshake {
        RunnerHandshake::Token(ref presented) if presented == expected_token => {
            eprintln!("[plugin] runner authenticated");
            Ok(())
        }
        RunnerHandshake::Token(_) => Err(PluginError::Runner("authentication failed".into())),
    }
}

/// Generate a unique local socket name.
fn generate_socket_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ocs_plugin_{}_{}", std::process::id(), n)
}

/// Generate a 32-byte random token for runner authentication.
fn generate_token() -> Result<String, PluginError> {
    let mut bytes = [0u8; PLUGIN_TOKEN_LEN];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| PluginError::Runner(format!("token generation failed: {e}")))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_runner_path_appends_suffix() {
        let host = PathBuf::from("/app/OpenCADStudio.exe");
        let runner = distinct_runner_path(&host);
        assert_eq!(
            runner,
            PathBuf::from("/app/OpenCADStudio-plugin-runner.exe")
        );
    }

    #[test]
    fn distinct_runner_path_handles_no_extension() {
        let host = PathBuf::from("/app/OpenCADStudio");
        let runner = distinct_runner_path(&host);
        assert_eq!(runner, PathBuf::from("/app/OpenCADStudio-plugin-runner"));
    }
}

#[cfg(all(test, feature = "host"))]
mod timeout_tests {
    use super::*;
    use crate::host::{DocumentReader, HostApi, ReaderEntity};
    use crate::ipc::protocol::{
        HostRequest, HostResponse, HostToPlugin, PluginRequest, PluginResponse, PluginToHost,
        RunnerHandshake,
    };
    use crate::ipc::transport::{recv, send};
    use crate::ribbon::owned::OwnedPluginManifest;
    use acadrust::xdata::ExtendedDataRecord;
    use acadrust::{CadDocument, EntityType, Handle};
    use interprocess::local_socket::{
        traits::{Listener, Stream as StreamTrait},
        GenericNamespaced, ListenerOptions, Stream, ToNsName,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::Instant;

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    struct EmptyReader;

    impl DocumentReader for EmptyReader {
        fn entity_count(&self) -> usize {
            0
        }
        fn for_each_entity(&self, _f: &mut dyn FnMut(ReaderEntity<'_>)) {}
        fn layer_name(&self, _handle: Handle) -> Option<&str> {
            None
        }
        fn app_id_name(&self, _handle: Handle) -> Option<&str> {
            None
        }
    }

    struct DummyHost {
        doc: CadDocument,
    }

    impl HostApi for DummyHost {
        fn tab_index(&self) -> usize {
            0
        }
        fn document(&self) -> &CadDocument {
            &self.doc
        }
        fn document_mut(&mut self) -> &mut CadDocument {
            &mut self.doc
        }
        fn document_reader(&self) -> Box<dyn DocumentReader + '_> {
            Box::new(EmptyReader)
        }
        fn add_entity(&mut self, _entity: EntityType) -> Handle {
            panic!("not used")
        }
        fn bump_geometry(&mut self) {}
        fn read_record(&self, _handle: Handle, _app_name: &str) -> Option<&ExtendedDataRecord> {
            None
        }
        fn write_record(&mut self, _handle: Handle, _record: ExtendedDataRecord) -> bool {
            false
        }
        fn remove_record(&mut self, _handle: Handle, _app_name: &str) -> bool {
            false
        }
        fn push_undo(&mut self, _label: &str) {}
        fn set_dirty(&mut self) {}
        fn push_info(&mut self, _msg: &str) {}
        fn push_output(&mut self, _msg: &str) {}
        fn push_error(&mut self, _msg: &str) {}
        fn start_interactive(&mut self, _command: Box<dyn crate::host::InteractiveCommand>) {}
        fn plugin_state_any(&self, _plugin_id: &str) -> Option<&(dyn std::any::Any + Send + Sync)> {
            None
        }
        fn plugin_state_any_mut(
            &mut self,
            _plugin_id: &str,
        ) -> Option<&mut (dyn std::any::Any + Send + Sync)> {
            None
        }
        fn ensure_plugin_state_any(
            &mut self,
            _plugin_id: &'static str,
            _init: &mut dyn FnMut() -> Box<dyn std::any::Any + Send + Sync>,
        ) -> &mut (dyn std::any::Any + Send + Sync) {
            panic!("not used")
        }
    }

    fn unique_socket_name() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("ocs_plugin_timeout_test_{}_{}", std::process::id(), n)
    }

    fn connected_pair() -> (Stream, Stream) {
        let name = unique_socket_name();
        let name_ref = name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("valid name");
        let listener = ListenerOptions::new()
            .name(name_ref)
            .create_sync()
            .expect("listener");
        let client_name = name.clone();
        let client_thread = thread::spawn(move || {
            StreamTrait::connect(client_name.to_ns_name::<GenericNamespaced>().unwrap())
                .expect("connect")
        });
        let server = listener.accept().expect("accept");
        let client = client_thread.join().expect("client thread");
        (server, client)
    }

    fn sleepy_child() -> Child {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .arg("/c")
                .arg("ping -n 30 127.0.0.1")
                .stdout(std::process::Stdio::null())
                .spawn()
                .expect("spawn sleep")
        }
        #[cfg(not(windows))]
        {
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn sleep")
        }
    }

    fn fake_manifest() -> OwnedPluginManifest {
        OwnedPluginManifest {
            id: "test.plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: "0.1.0".to_string(),
            description: "test".to_string(),
            api_version: 1,
            ribbon_order: 0,
            xdata_apps: vec![],
            command_prefixes: vec![],
        }
    }

    fn fake_process() -> (PluginProcess, Stream) {
        let (host_stream, runner_stream) = connected_pair();
        let process = PluginProcess {
            stream: Arc::new(Mutex::new(Some(host_stream))),
            child: Arc::new(Mutex::new(Some(sleepy_child()))),
            id: "test.plugin".to_string(),
            manifest: fake_manifest(),
            ribbon: vec![],
            next_request_id: AtomicU64::new(1),
            v3_state: Mutex::new(None),
        };
        (process, runner_stream)
    }

    #[test]
    fn dispatch_call_timeout_marks_process_dead() {
        let _env_guard = ENV_LOCK.lock().expect("env lock");
        let prev = std::env::var("OCS_PLUGIN_CALL_TIMEOUT_SECS").ok();
        let prev_floor = std::env::var("OCS_PLUGIN_TEST_FLOOR_SECS").ok();
        std::env::set_var("OCS_PLUGIN_CALL_TIMEOUT_SECS", "1");
        // Drop the Dispatch floor to 0 so the test fires at the 1 s base instead
        // of waiting out the real 10 s safety floor.
        std::env::set_var("OCS_PLUGIN_TEST_FLOOR_SECS", "0");
        let (process, runner_stream) = fake_process();

        let _runner = thread::spawn(move || {
            let mut peer = runner_stream;
            let req = recv::<HostToPlugin>(&mut peer).expect("read dispatch");
            assert!(
                matches!(req, HostToPlugin::Request(HostRequest::Dispatch { ref cmd }) if cmd == "HANG")
            );
            // Block until the host closes the connection after the timeout.
            let _ = recv::<HostToPlugin>(&mut peer);
        });

        let mut host = DummyHost {
            doc: CadDocument::default(),
        };
        let start = Instant::now();
        let result = process.dispatch(&mut host, "HANG", &mut |_| {});
        let elapsed = start.elapsed();

        assert!(
            matches!(
                result,
                Err(PluginError::CallTimeout {
                    request: "Dispatch",
                    ..
                })
            ),
            "expected Dispatch timeout, got {result:?}"
        );
        assert!(
            elapsed >= Duration::from_secs(1),
            "timeout should respect the 1 s base: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "timed out too slowly: {elapsed:?}"
        );
        assert!(!process.is_alive(), "process should be marked dead");

        // Do not join the fake runner thread: it blocks until the host closes
        // the socket. In production the killed child process closes its end of
        // the socket and the reader thread exits; this test uses a local thread
        // instead, so we let it be reaped with the test process.
        match prev {
            Some(v) => std::env::set_var("OCS_PLUGIN_CALL_TIMEOUT_SECS", v),
            None => std::env::remove_var("OCS_PLUGIN_CALL_TIMEOUT_SECS"),
        }
        match prev_floor {
            Some(v) => std::env::set_var("OCS_PLUGIN_TEST_FLOOR_SECS", v),
            None => std::env::remove_var("OCS_PLUGIN_TEST_FLOOR_SECS"),
        }
    }

    #[test]
    fn dispatch_succeeds_with_nested_request_within_deadline() {
        let _env_guard = ENV_LOCK.lock().expect("env lock");
        let prev = std::env::var("OCS_PLUGIN_CALL_TIMEOUT_SECS").ok();
        std::env::set_var("OCS_PLUGIN_CALL_TIMEOUT_SECS", "2");
        let (process, runner_stream) = fake_process();

        let runner = thread::spawn(move || {
            let mut peer = runner_stream;
            let req = recv::<HostToPlugin>(&mut peer).expect("read dispatch");
            assert!(
                matches!(req, HostToPlugin::Request(HostRequest::Dispatch { ref cmd }) if cmd == "NESTED")
            );
            send(
                &mut peer,
                &PluginToHost::Request(PluginRequest::PushInfo("hello".to_string())),
            )
            .expect("send nested request");
            let resp = recv::<HostToPlugin>(&mut peer).expect("read nested response");
            assert!(matches!(resp, HostToPlugin::Response(PluginResponse::Ok)));
            send(&mut peer, &PluginToHost::Response(HostResponse::Bool(true)))
                .expect("send final response");
        });

        let mut host = DummyHost {
            doc: CadDocument::default(),
        };
        let result = process.dispatch(&mut host, "NESTED", &mut |_| {});
        assert!(result.expect("dispatch succeeds"));
        assert!(process.is_alive(), "process should still be alive");

        // Clean up the helper child so it does not outlive the test.
        if let Some(mut child) = process
            .child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = child.kill();
        }

        runner.join().expect("runner thread");
        match prev {
            Some(v) => std::env::set_var("OCS_PLUGIN_CALL_TIMEOUT_SECS", v),
            None => std::env::remove_var("OCS_PLUGIN_CALL_TIMEOUT_SECS"),
        }
    }

    #[test]
    fn runner_handshake_wrong_token_is_rejected() {
        let result = verify_runner_handshake(
            RunnerHandshake::Token("wrong-token".to_string()),
            "expected-token",
        );
        assert!(
            matches!(result, Err(PluginError::Runner(ref s)) if s == "authentication failed"),
            "expected authentication failure, got {result:?}"
        );
    }

    #[test]
    fn runner_handshake_correct_token_is_accepted() {
        let result = verify_runner_handshake(
            RunnerHandshake::Token("correct-token".to_string()),
            "correct-token",
        );
        assert!(result.is_ok(), "expected authentication success, got {result:?}");
    }
}
