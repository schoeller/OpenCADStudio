//! Process management for out-of-process plugins.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::{Listener, Stream as StreamTrait};
use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, RecvHalf, SendHalf, Stream, ToNsName,
};

use crate::host::{CommandStep, HostApi};
use crate::ipc::protocol::{
    HostAsync, HostRequest, HostResponse, HostToPlugin, InteractiveEvent, PluginAsync,
    PluginRequest, PluginResponse, PluginToHost, RunnerHandshake, PLUGIN_TOKEN_ENV,
};
use crate::ipc::server::handle_plugin_request;
use crate::ipc::transport::{recv, send};
use crate::ribbon::owned::{OwnedPluginManifest, OwnedRibbonGroup as OwnedRibbonGroupAlias};

use serde::de::DeserializeOwned;

mod manager;
pub use manager::{DispatchResult, PluginManager};

/// Whether verbose plugin-IPC logging is on. Off by default so a normal run
/// only prints the one-line "Loaded plugin" notice; set `OCS_PLUGIN_VERBOSE=1`
/// (any value) to see the spawn / handshake / per-dispatch request-response
/// trace.
pub(crate) fn verbose() -> bool {
    use std::sync::OnceLock;
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var_os("OCS_PLUGIN_VERBOSE").is_some())
}

/// `eprintln!` that only fires in verbose mode (see [`verbose`]).
macro_rules! vlog {
    ($($arg:tt)*) => {{
        if crate::process::verbose() {
            eprintln!($($arg)*);
        }
    }};
}

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

/// Maximum number of out-of-band plugin async events queued per process.
const ASYNC_QUEUE_BOUND: usize = 256;

/// Inbound async message from a plugin process, delivered on the async socket.
#[derive(Debug)]
pub enum AsyncInbound {
    Event(PluginAsync),
    Request(PluginRequest),
}

/// Outbound async message to a plugin process, sent on the async socket.
enum AsyncOutbound {
    Event(HostAsync),
    Response(PluginResponse),
}

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
    // Tests can lower the floor via OCS_PLUGIN_TEST_FLOOR_SECS so the suite
    // does not wait out the real 10 s Dispatch minimum. The variable is
    // intentionally undocumented and only expected to be set by tests.
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
        HostRequest::GetPanels => "GetPanels",
        HostRequest::Shutdown => "Shutdown",
    }
}

/// Read a handshake from `stream` and verify it matches `expected_token`.
/// Returns the stream on success so the caller can keep using it.
fn verify_handshake_on_stream(
    stream: Stream,
    child: &Mutex<Option<Child>>,
    expected_token: &str,
    label: &'static str,
) -> Result<Stream, PluginError> {
    let stream = Mutex::new(Some(stream));
    let timeout = spawn_timeout();
    let handshake = recv_with_deadline::<RunnerHandshake>(
        &stream,
        child,
        Instant::now() + timeout,
        timeout,
        label,
    )?;
    let result = verify_runner_handshake(handshake, expected_token);
    let mut guard = stream.lock().unwrap_or_else(|e| e.into_inner());
    let stream = guard.take().expect("stream returned by recv_with_deadline");
    result.map(|_| stream)
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
    #[error("ABI revision mismatch: plugin built for revision {plugin}, host revision is {host}")]
    AbiRevisionMismatch { plugin: u64, host: u64 },
}

/// Shared inbox for async messages arriving from a plugin process.
struct AsyncInbox {
    queue: Mutex<VecDeque<AsyncInbound>>,
    dropped: AtomicU64,
}

/// One spawned plugin process.
pub struct PluginProcess {
    sync_stream: Arc<Mutex<Option<Stream>>>,
    /// Receive half of the async socket, owned by the async reader thread.
    /// Kept in the struct so the stream resource is released with the process.
    #[allow(dead_code)]
    async_recv: Mutex<Option<RecvHalf>>,
    /// Send half of the async socket, owned by the async writer thread.
    /// Kept in the struct so the stream resource is released with the process.
    #[allow(dead_code)]
    async_send: Mutex<Option<SendHalf>>,
    child: Mutex<Option<Child>>,
    id: String,
    manifest: OwnedPluginManifest,
    ribbon: Vec<OwnedRibbonGroupAlias>,
    /// Panels declared by the plugin, fetched at load time.
    panels: Vec<crate::panel::PanelDef>,
    async_inbox: Arc<AsyncInbox>,
    /// Channel used by the UI thread to feed async host events and request
    /// responses to the writer thread. `None` after shutdown.
    async_writer_tx: Mutex<Option<mpsc::SyncSender<AsyncOutbound>>>,
    async_writer_alive: Arc<AtomicBool>,
    async_reader_alive: Arc<AtomicBool>,
    async_writer_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    async_reader_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Set to true when the host is deliberately shutting this plugin down so
    /// the async reader can suppress the expected socket-close error message.
    shutting_down: Arc<AtomicBool>,
    /// Path to a temp file capturing the runner's stderr, used for crash
    /// diagnostics when the runner exits unexpectedly.
    stderr_path: PathBuf,
}

fn tail(path: &Path, max_lines: usize) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&data);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    Some(lines[start..].join("\n"))
}

fn stderr_tail(path: &Path, max_lines: usize) -> String {
    tail(path, max_lines).unwrap_or_else(|| "(stderr unavailable)".into())
}

/// Wrap a spawn-time failure so the runner's stderr is surfaced in the host
/// error message. This is the only way to diagnose runner crashes during the
/// initial handshake / manifest / ribbon / panels sequence.
fn fail_spawn(
    err: PluginError,
    stderr_path: &Path,
    child: &Mutex<Option<Child>>,
) -> PluginError {
    if let Some(child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
        reap(child);
    }
    let tail = stderr_tail(stderr_path, 200);
    eprintln!(
        "[plugin] spawn failed; runner stderr tail:\n{}\n(full stderr: {})",
        tail,
        stderr_path.display()
    );
    let mut detail = format!("{err}");
    if !tail.is_empty() && tail != "(stderr unavailable)" {
        detail.push_str(&format!("\nrunner stderr tail:\n{tail}"));
    }
    PluginError::Runner(detail)
}

impl PluginProcess {
    /// Spawn the plugin cdylib in a separate process and connect to it.
    pub fn spawn(cdylib_path: &Path, host: &mut dyn HostApi) -> Result<Self, PluginError> {
        let sync_socket_name = generate_socket_name();
        let async_socket_name = generate_socket_name();
        let sync_name_ref: interprocess::local_socket::Name = sync_socket_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("valid namespaced name");
        let async_name_ref: interprocess::local_socket::Name = async_socket_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("valid namespaced name");
        let runner_path = runner_executable()?;
        vlog!(
            "[plugin] spawning runner {} for {}",
            runner_path.display(),
            cdylib_path.display()
        );

        let token = generate_token()?;
        let stderr_path = generate_stderr_path();
        let stderr_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&stderr_path)?;

        // Create both listeners before spawning so the runner can connect immediately.
        let sync_listener = ListenerOptions::new().name(sync_name_ref).create_sync()?;
        let async_listener = ListenerOptions::new().name(async_name_ref).create_sync()?;

        let child = Command::new(&runner_path)
            .arg(&sync_socket_name)
            .arg(&async_socket_name)
            .arg(cdylib_path)
            .env(PLUGIN_TOKEN_ENV, &token)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file))
            .spawn()?;
        let child = Mutex::new(Some(child));

        // Accept both connections with a timeout. Each accept runs in its own
        // thread so a hung/crashed runner cannot block the host indefinitely.
        let sync_stream = {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(sync_listener.accept());
            });
            match rx.recv_timeout(spawn_timeout()) {
                Ok(Ok(stream)) => stream,
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
            }
        };
        let async_stream = {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(async_listener.accept());
            });
            match rx.recv_timeout(spawn_timeout()) {
                Ok(Ok(stream)) => stream,
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
            }
        };
        vlog!("[plugin] runner connected (sync + async)");

        // Verify the runner presented the token on both sockets before wrapping them.
        let sync_stream =
            match verify_handshake_on_stream(sync_stream, &child, &token, "SyncHandshake") {
                Ok(s) => s,
                Err(e) => {
                    if let Some(child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
                        reap(child);
                    }
                    return Err(e);
                }
            };
        let async_stream =
            match verify_handshake_on_stream(async_stream, &child, &token, "AsyncHandshake") {
                Ok(s) => s,
                Err(e) => {
                    if let Some(child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
                        reap(child);
                    }
                    return Err(e);
                }
            };

        let sync_stream = Arc::new(Mutex::new(Some(sync_stream)));
        let async_stream = Arc::new(Mutex::new(Some(async_stream)));

        // The runner first answers GetManifest and GetRibbon so the host can
        // build the UI without keeping the plugin object alive.
        let no_op = &mut |_| {};
        let drop_async = |_event: PluginAsync| {};
        let manifest = match call(
            &sync_stream,
            &child,
            host,
            HostRequest::GetManifest,
            no_op,
            drop_async,
        )
        .map_err(|e| fail_spawn(e, &stderr_path, &child))? {
            HostResponse::Manifest(m) => m,
            other => return Err(fail_spawn(
                PluginError::UnexpectedResponse(other),
                &stderr_path,
                &child,
            )),
        };
        eprintln!(
            "Loaded plugin: {} ({} {})",
            manifest.name, manifest.id, manifest.version
        );
        let ribbon = match call(
            &sync_stream,
            &child,
            host,
            HostRequest::GetRibbon,
            no_op,
            drop_async,
        )
        .map_err(|e| fail_spawn(e, &stderr_path, &child))? {
            HostResponse::Ribbon(r) => r,
            other => return Err(fail_spawn(
                PluginError::UnexpectedResponse(other),
                &stderr_path,
                &child,
            )),
        };
        let panels = match call(
            &sync_stream,
            &child,
            host,
            HostRequest::GetPanels,
            no_op,
            drop_async,
        )
        .map_err(|e| fail_spawn(e, &stderr_path, &child))? {
            HostResponse::Panels(p) => p,
            other => return Err(fail_spawn(
                PluginError::UnexpectedResponse(other),
                &stderr_path,
                &child,
            )),
        };

        let id = manifest.id.clone();
        let (async_writer_tx, async_writer_rx) = mpsc::sync_channel::<AsyncOutbound>(ASYNC_QUEUE_BOUND);
        let async_inbox = Arc::new(AsyncInbox {
            queue: Mutex::new(VecDeque::new()),
            dropped: AtomicU64::new(0),
        });
        let writer_alive = Arc::new(AtomicBool::new(true));
        let reader_alive = Arc::new(AtomicBool::new(true));
        let shutting_down = Arc::new(AtomicBool::new(false));

        // Split the async socket so the reader and writer threads can operate
        // concurrently without contending for a single stream lock.
        let async_stream = Arc::try_unwrap(async_stream)
            .expect("async stream has no other refs")
            .into_inner()
            .expect("async stream mutex not poisoned")
            .expect("async stream present");
        let (async_recv, async_send) = async_stream.split();

        let writer_alive_for_thread = Arc::clone(&writer_alive);
        let async_writer_handle = std::thread::spawn(move || {
            async_writer(async_send, async_writer_rx, writer_alive_for_thread);
        });

        let reader_alive_for_thread = Arc::clone(&reader_alive);
        let reader_inbox = Arc::clone(&async_inbox);
        let shutting_down_for_reader = Arc::clone(&shutting_down);
        let async_reader_handle = std::thread::spawn(move || {
            async_reader(async_recv, reader_inbox, reader_alive_for_thread, shutting_down_for_reader);
        });

        Ok(Self {
            sync_stream,
            async_recv: Mutex::new(None),
            async_send: Mutex::new(None),
            child,
            id,
            manifest,
            ribbon,
            panels,
            async_inbox,
            async_writer_tx: Mutex::new(Some(async_writer_tx)),
            async_writer_alive: writer_alive,
            async_reader_alive: reader_alive,
            async_writer_handle: Mutex::new(Some(async_writer_handle)),
            async_reader_handle: Mutex::new(Some(async_reader_handle)),
            shutting_down,
            stderr_path,
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

    pub fn panels(&self) -> &[crate::panel::PanelDef] {
        &self.panels
    }

    pub fn dispatch(
        &self,
        host: &mut dyn HostApi,
        cmd: &str,
        on_start_interactive: &mut dyn FnMut(u64),
    ) -> Result<bool, PluginError> {
        vlog!("[plugin] dispatching {cmd}");
        let result = match call(
            &self.sync_stream,
            &self.child,
            host,
            HostRequest::Dispatch {
                cmd: cmd.to_string(),
            },
            on_start_interactive,
            |event| self.enqueue_async(event),
        ) {
            Ok(HostResponse::Bool(b)) => Ok(b),
            Ok(other) => Err(PluginError::UnexpectedResponse(other)),
            Err(e) => {
                if is_connection_failure(&e) {
                    let tail = self.stderr_tail(200);
                    eprintln!(
                        "[plugin] {} runner died; stderr tail:\n{}\n(full stderr: {})",
                        self.id,
                        tail,
                        self.stderr_path.display()
                    );
                    let mut detail = format!("{e}");
                    if !tail.is_empty() && tail != "(stderr unavailable)" {
                        detail.push_str(&format!("\nrunner stderr tail:\n{tail}"));
                    }
                    Err(PluginError::Runner(detail))
                } else {
                    Err(e)
                }
            }
        };
        vlog!("[plugin] dispatch {cmd} result: {result:?}");
        result
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
                &self.sync_stream,
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
                PluginToHost::Async(event) => self.enqueue_async(event),
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
                &self.sync_stream,
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
                PluginToHost::Async(event) => self.enqueue_async(event),
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
                &self.sync_stream,
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
                PluginToHost::Async(event) => self.enqueue_async(event),
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        let mut guard = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let child_alive = match guard.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        };
        let writer_alive = self.async_writer_alive.load(Ordering::SeqCst);
        let reader_alive = self.async_reader_alive.load(Ordering::SeqCst);
        child_alive && writer_alive && reader_alive
    }

    /// Read the last `max_lines` lines from the runner's stderr capture file.
    /// Used for crash diagnostics when a plugin process exits unexpectedly.
    pub fn stderr_tail(&self, max_lines: usize) -> String {
        stderr_tail(&self.stderr_path, max_lines)
    }

    /// Send an asynchronous host event to the plugin process. Never blocks the
    /// caller. If the writer channel is full, the oldest event is dropped and
    /// the drop counter is incremented.
    pub fn send_async(&self, event: HostAsync) -> Result<(), PluginError> {
        let mut guard = self
            .async_writer_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let sender = guard.as_mut().ok_or_else(shutdown_error)?;
        match sender.try_send(AsyncOutbound::Event(event)) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => {
                self.async_inbox.dropped.fetch_add(1, Ordering::Relaxed);
                Err(PluginError::Runner(
                    "async writer channel full; event dropped".to_string(),
                ))
            }
            Err(mpsc::TrySendError::Disconnected(_)) => Err(shutdown_error()),
        }
    }

    /// Send a synchronous response to a plugin request that arrived on the
    /// async socket. The plugin's async event thread is waiting for this
    /// response so it can complete a host API call made from `on_async_event`.
    pub fn send_async_response(&self, resp: PluginResponse) -> Result<(), PluginError> {
        let mut guard = self
            .async_writer_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let sender = guard.as_mut().ok_or_else(shutdown_error)?;
        match sender.try_send(AsyncOutbound::Response(resp)) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => {
                self.async_inbox.dropped.fetch_add(1, Ordering::Relaxed);
                Err(PluginError::Runner(
                    "async response channel full; response dropped".to_string(),
                ))
            }
            Err(mpsc::TrySendError::Disconnected(_)) => Err(shutdown_error()),
        }
    }

    /// Take all queued async messages from the plugin process.
    pub fn drain_async(&self) -> Vec<AsyncInbound> {
        let mut guard = self
            .async_inbox
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard).into_iter().collect()
    }

    /// Number of async messages dropped because the inbound queue or outbound
    /// channel was full.
    pub fn dropped_async_count(&self) -> u64 {
        self.async_inbox.dropped.load(Ordering::Relaxed)
    }

    fn enqueue_async(&self, event: PluginAsync) {
        let mut guard = self
            .async_inbox
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if guard.len() >= ASYNC_QUEUE_BOUND {
            drop(guard);
            self.async_inbox.dropped.fetch_add(1, Ordering::Relaxed);
            eprintln!("[plugin] async event dropped for {} (queue full)", self.id);
        } else {
            guard.push_back(AsyncInbound::Event(event));
        }
    }

    /// Tear down the plugin process without blocking the caller. Drops the
    /// writer channel, kills the child, and joins the async threads with a short
    /// timeout. The child is killed before taking streams so that any thread
    /// blocked on the async socket is unblocked first.
    pub fn shutdown(&self) {
        // Let the async reader know this shutdown is intentional so it can
        // suppress the expected socket-close error message.
        self.shutting_down.store(true, Ordering::SeqCst);
        // Dropping the sender wakes the writer thread so it can exit cleanly.
        drop(
            self.async_writer_tx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take(),
        );
        // Killing the child closes the runner's socket ends, which unblocks any
        // reader/writer thread currently parked on recv/send.
        if let Some(child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            reap(child);
        }
        // Now the threads are (or will soon be) unblocked; take the sync stream.
        let (_sync_stream, _child) = self.take_resources();
        self.join_async_threads(Duration::from_millis(500));
    }

    /// Take the sync stream and child handles out of the process. After this
    /// the process is considered shut down and any further IPC will fail. The
    /// async socket halves are owned by the reader/writer threads and are not
    /// accessible here.
    fn take_resources(&self) -> (Option<Stream>, Option<Child>) {
        let sync_stream = self
            .sync_stream
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        (sync_stream, child)
    }

    fn join_async_threads(&self, timeout: Duration) {
        let writer = self
            .async_writer_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let reader = self
            .async_reader_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        fn join_with_timeout<T: 'static>(
            handle: Option<std::thread::JoinHandle<T>>,
            timeout: Duration,
        ) {
            let Some(handle) = handle else {
                return;
            };
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = handle.join();
                let _ = tx.send(());
            });
            let _ = rx.recv_timeout(timeout);
        }

        join_with_timeout(writer, timeout);
        join_with_timeout(reader, timeout);
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        self.shutdown();
        let _ = std::fs::remove_file(&self.stderr_path);
    }
}

impl PluginProcess {
    fn send_request(&self, req: HostRequest) -> Result<(), PluginError> {
        let mut guard = self.sync_stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream = guard.as_mut().ok_or_else(shutdown_error)?;
        send(stream, &HostToPlugin::Request(req)).map_err(|e| {
            drop(guard);
            let err: PluginError = e.into();
            if is_connection_failure(&err) {
                mark_dead(&self.sync_stream, &self.child);
            }
            err
        })
    }

    fn send_response(&self, resp: crate::ipc::protocol::PluginResponse) -> Result<(), PluginError> {
        let mut guard = self.sync_stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream = guard.as_mut().ok_or_else(shutdown_error)?;
        send(stream, &HostToPlugin::Response(resp)).map_err(|e| {
            drop(guard);
            let err: PluginError = e.into();
            if is_connection_failure(&err) {
                mark_dead(&self.sync_stream, &self.child);
            }
            err
        })
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
/// skipped. IO/transport errors also mark the process dead so the host does not
/// keep trying to talk to a runner whose pipe has closed.
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
            Some(mut stream) => match recv(&mut stream) {
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
            if is_connection_failure(&e) {
                mark_dead(stream, child);
            }
            Err(e)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            mark_dead(stream, child);
            Err(PluginError::CallTimeout {
                request,
                duration: timeout,
            })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            mark_dead(stream, child);
            Err(shutdown_error())
        }
    }
}

/// Dedicated writer thread that sends host async events to the runner on the
/// async socket send half. Exits when the channel is disconnected or any send
/// fails.
fn async_writer(
    mut send_half: SendHalf,
    rx: mpsc::Receiver<AsyncOutbound>,
    alive: Arc<AtomicBool>,
) {
    while let Ok(outbound) = rx.recv() {
        let result = match outbound {
            AsyncOutbound::Event(event) => send(&mut send_half, &HostToPlugin::Async(event)),
            AsyncOutbound::Response(resp) => send(&mut send_half, &HostToPlugin::Response(resp)),
        };
        if let Err(e) = result {
            eprintln!("[plugin] async writer send failed: {e}");
            alive.store(false, Ordering::SeqCst);
            break;
        }
    }
    vlog!("[plugin] async writer thread exiting");
}

/// Dedicated reader thread that receives plugin async events and fire-and-forget
/// requests on the async socket receive half. Enqueues them for the host UI
/// thread and sets the liveness flag to false on any recv error.
fn async_reader(
    mut recv_half: RecvHalf,
    inbox: Arc<AsyncInbox>,
    alive: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
) {
    loop {
        match recv(&mut recv_half) {
            Ok(PluginToHost::Async(event)) => {
                let mut queue = inbox.queue.lock().unwrap_or_else(|e| e.into_inner());
                if queue.len() >= ASYNC_QUEUE_BOUND {
                    drop(queue);
                    inbox.dropped.fetch_add(1, Ordering::Relaxed);
                    eprintln!("[plugin] async event dropped (queue full)");
                } else {
                    queue.push_back(AsyncInbound::Event(event));
                }
            }
            Ok(PluginToHost::Request(req)) => {
                let mut queue = inbox.queue.lock().unwrap_or_else(|e| e.into_inner());
                if queue.len() >= ASYNC_QUEUE_BOUND {
                    drop(queue);
                    inbox.dropped.fetch_add(1, Ordering::Relaxed);
                    eprintln!("[plugin] async request dropped (queue full)");
                } else {
                    queue.push_back(AsyncInbound::Request(req));
                }
            }
            Ok(PluginToHost::Response(resp)) => {
                eprintln!("[plugin] unexpected PluginToHost::Response on async socket: {resp:?}");
            }
            Err(e) => {
                if !shutting_down.load(Ordering::SeqCst) {
                    eprintln!("[plugin] async reader recv error: {e}");
                }
                alive.store(false, Ordering::SeqCst);
                break;
            }
        }
    }
    vlog!("[plugin] async reader thread exiting");
}

/// True for errors that mean the runner process has gone away or the IPC
/// stream is unusable. Used to mark a plugin dead immediately instead of
/// generating cascading failures on the next dispatch.
fn is_connection_failure(err: &PluginError) -> bool {
    matches!(
        err,
        PluginError::Io(_) | PluginError::Transport(_) | PluginError::RunnerExited
    )
}

/// Send a host request and wait for the response, handling any nested plugin
/// requests inline using the supplied `HostApi`. Out-of-band plugin async events
/// are enqueued via `on_async` instead of being treated as errors.
fn call(
    stream: &Mutex<Option<Stream>>,
    child: &Mutex<Option<Child>>,
    host: &mut dyn HostApi,
    req: HostRequest,
    on_start_interactive: &mut dyn FnMut(u64),
    mut on_async: impl FnMut(PluginAsync),
) -> Result<HostResponse, PluginError> {
    let kind = request_kind(&req);
    let timeout = request_timeout(kind);
    let deadline = Instant::now() + timeout;
    vlog!("[plugin] host -> runner: {req:?}");
    {
        let mut guard = stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream_ref = guard.as_mut().ok_or_else(shutdown_error)?;
        if let Err(e) = send(stream_ref, &HostToPlugin::Request(req)) {
            drop(guard);
            mark_dead(stream, child);
            return Err(e.into());
        }
    }
    loop {
        let msg = recv_with_deadline::<PluginToHost>(stream, child, deadline, timeout, kind)?;
        vlog!("[plugin] runner -> host: {msg:?}");
        match msg {
            PluginToHost::Response(resp) => return Ok(resp),
            PluginToHost::Request(plugin_req) => {
                let resp = handle_plugin_request(host, plugin_req, on_start_interactive);
                vlog!("[plugin] host -> runner response: {resp:?}");
                let mut guard = stream.lock().unwrap_or_else(|e| e.into_inner());
                let stream_ref = guard.as_mut().ok_or_else(shutdown_error)?;
                if let Err(e) = send(stream_ref, &HostToPlugin::Response(resp)) {
                    drop(guard);
                    mark_dead(stream, child);
                    return Err(e.into());
                }
            }
            PluginToHost::Async(event) => on_async(event),
        }
    }
}

/// Locate the executable to spawn for running a plugin.
///
/// The host spawns the separate `ocs_plugin_runner` binary that lives next to
/// the host executable. This keeps the runner lightweight (no iced/wgpu) while
/// the host stays unchanged. The `OCS_PLUGIN_RUNNER_EXE` environment variable
/// overrides the lookup for tests or unusual deployment layouts.
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

        // Look for the separate runner binary next to the host executable.
        let runner = runner_path_next_to_host(&host);
        if runner.exists() {
            runner
        } else if let Some(alt) = runner_path_in_sibling_profile(&host) {
            // Development convenience: if the host is in a Cargo target profile
            // directory (debug/release), try the sibling profile too. This lets a
            // debug host find a release-built runner (or vice versa) without
            // requiring OCS_PLUGIN_RUNNER_EXE.
            alt
        } else if let Some(alt) = runner_path_in_sibling_target_dir(&host) {
            // Another development convenience: the host and runner may have been
            // built in different Cargo target directories (e.g. one clean dir for
            // tests, another for the host). Search sibling target dirs for the
            // runner in both debug and release profiles.
            alt
        } else {
            return Err(PluginError::Runner(format!(
                "cannot find runner executable at {}. Build it with `cargo build -p ocs_plugin_runner{}` (or set OCS_PLUGIN_RUNNER_EXE to override)",
                runner.display(),
                if is_target_profile_dir(&runner) {
                    ""
                } else {
                    " --release"
                }
            )));
        }
    };

    *cached = Some(path.clone());
    Ok(path)
}

/// Build the expected runner path next to the host binary.
fn runner_path_next_to_host(host: &Path) -> PathBuf {
    let dir = host.parent().unwrap_or_else(|| Path::new("."));
    let mut runner = dir.to_path_buf();
    runner.push(runner_exe_name());
    runner
}

/// If the host binary sits inside a Cargo `debug` or `release` folder, also try
/// the sibling folder. This avoids forcing developers to build the runner in the
/// same profile as the host.
fn runner_path_in_sibling_profile(host: &Path) -> Option<PathBuf> {
    let dir = host.parent()?;
    let dir_name = dir.file_name()?.to_str()?;
    let sibling = match dir_name {
        "debug" => "release",
        "release" => "debug",
        _ => return None,
    };
    let mut candidate = dir.to_path_buf();
    candidate.pop();
    candidate.push(sibling);
    candidate.push(runner_exe_name());
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

/// If the host binary sits inside `<target_dir>/debug` or `<target_dir>/release`,
/// also try `<sibling_target_dir>/debug` and `<sibling_target_dir>/release` for
/// every sibling of `<target_dir>`. This handles the common case where the host
/// was built in one Cargo target directory (e.g. a clean dir for tests) and the
/// runner in another.
fn runner_path_in_sibling_target_dir(host: &Path) -> Option<PathBuf> {
    let profile_dir = host.parent()?;
    let profile = profile_dir.file_name()?.to_str()?;
    if profile != "debug" && profile != "release" {
        return None;
    }
    let target_dir = profile_dir.parent()?;
    let parent = target_dir.parent()?;
    let entries = std::fs::read_dir(parent).ok()?;
    for entry in entries.flatten() {
        let sibling_target = entry.path();
        if sibling_target == target_dir || !sibling_target.is_dir() {
            continue;
        }
        for profile in ["debug", "release"] {
            let candidate = sibling_target.join(profile).join(runner_exe_name());
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn runner_exe_name() -> &'static str {
    if cfg!(windows) {
        "ocs_plugin_runner.exe"
    } else {
        "ocs_plugin_runner"
    }
}

/// True when `path` looks like it lives inside a Cargo `debug` or `release`
/// profile folder (used to tailor the build command in error messages).
fn is_target_profile_dir(path: &Path) -> bool {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|n| n == "debug" || n == "release")
        .unwrap_or(false)
}

/// Verify that the runner's `handshake` presents `expected_token`.
fn verify_runner_handshake(
    handshake: RunnerHandshake,
    expected_token: &str,
) -> Result<(), PluginError> {
    match handshake {
        RunnerHandshake::Token(ref presented) if presented == expected_token => {
            vlog!("[plugin] runner authenticated");
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

fn generate_stderr_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "ocs_plugin_runner_{}_{}.stderr",
        std::process::id(),
        n
    ));
    path
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
    fn runner_path_next_to_host_uses_platform_name() {
        let host = PathBuf::from("/app/OpenCADStudio.exe");
        let runner = runner_path_next_to_host(&host);
        let expected = if cfg!(windows) {
            "/app/ocs_plugin_runner.exe"
        } else {
            "/app/ocs_plugin_runner"
        };
        assert_eq!(runner, PathBuf::from(expected));
    }

    #[test]
    fn runner_path_next_to_host_handles_no_extension() {
        let host = PathBuf::from("/app/OpenCADStudio");
        let runner = runner_path_next_to_host(&host);
        let expected = if cfg!(windows) {
            "/app/ocs_plugin_runner.exe"
        } else {
            "/app/ocs_plugin_runner"
        };
        assert_eq!(runner, PathBuf::from(expected));
    }

    #[test]
    fn runner_path_in_sibling_profile_finds_release_when_host_in_debug() {
        // The helper only returns a path when the sibling file exists; create a
        // temporary directory tree to exercise the happy path.
        let tmp =
            std::env::temp_dir().join(format!("ocs_runner_sibling_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("debug")).unwrap();
        std::fs::create_dir_all(tmp.join("release")).unwrap();
        let runner = tmp.join("release").join(runner_exe_name());
        std::fs::write(&runner, b"").unwrap();

        let host = tmp.join("debug").join("OpenCADStudio");
        let found = runner_path_in_sibling_profile(&host);
        assert_eq!(found, Some(runner));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_target_profile_dir_recognizes_cargo_profiles() {
        assert!(is_target_profile_dir(Path::new(
            "/target/debug/ocs_plugin_runner"
        )));
        assert!(is_target_profile_dir(Path::new(
            "/target/release/ocs_plugin_runner"
        )));
        assert!(!is_target_profile_dir(Path::new("/app/ocs_plugin_runner")));
    }

    #[test]
    fn runner_path_in_sibling_target_dir_finds_runner_across_target_dirs() {
        let tmp = std::env::temp_dir().join(format!(
            "ocs_runner_target_sibling_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);

        // Host lives in one target directory (debug profile).
        let host_target = tmp.join("ocs_target_clean");
        std::fs::create_dir_all(host_target.join("debug")).unwrap();
        let host = host_target.join("debug").join("OpenCADStudio");

        // Runner lives in a sibling target directory (release profile).
        let runner_target = tmp.join("ocs_target");
        let runner = runner_target.join("release").join(runner_exe_name());
        std::fs::create_dir_all(runner_target.join("release")).unwrap();
        std::fs::write(&runner, b"").unwrap();

        let found = runner_path_in_sibling_target_dir(&host);
        assert_eq!(found, Some(runner));

        let _ = std::fs::remove_dir_all(&tmp);
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
            sync_stream: Arc::new(Mutex::new(Some(host_stream))),
            async_recv: Mutex::new(None),
            async_send: Mutex::new(None),
            child: Mutex::new(Some(sleepy_child())),
            id: "test.plugin".to_string(),
            manifest: fake_manifest(),
            ribbon: vec![],
            panels: vec![],
            async_inbox: Arc::new(AsyncInbox {
                queue: Mutex::new(VecDeque::new()),
                dropped: AtomicU64::new(0),
            }),
            async_writer_tx: Mutex::new(None),
            async_writer_alive: Arc::new(AtomicBool::new(true)),
            async_reader_alive: Arc::new(AtomicBool::new(true)),
            async_writer_handle: Mutex::new(None),
            async_reader_handle: Mutex::new(None),
            shutting_down: Arc::new(AtomicBool::new(false)),
            stderr_path: generate_stderr_path(),
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
            let req = recv(&mut peer).expect("read dispatch");
            assert!(
                matches!(req, HostToPlugin::Request(HostRequest::Dispatch { ref cmd }) if cmd == "HANG")
            );
            // Block until the host closes the connection after the timeout.
            let _: Result<HostToPlugin, _> = recv(&mut peer);
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
            let req = recv(&mut peer).expect("read dispatch");
            assert!(
                matches!(req, HostToPlugin::Request(HostRequest::Dispatch { ref cmd }) if cmd == "NESTED")
            );
            send(
                &mut peer,
                &PluginToHost::Request(PluginRequest::PushInfo("hello".to_string())),
            )
            .expect("send nested request");
            let resp = recv(&mut peer).expect("read nested response");
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
        assert!(
            result.is_ok(),
            "expected authentication success, got {result:?}"
        );
    }
}
