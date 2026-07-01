//! Plugin-side IPC client and `HostApi` proxy.

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use acadrust::xdata::ExtendedDataRecord;
use acadrust::{CadDocument, EntityType, Handle};
use interprocess::local_socket::traits::Stream as StreamTrait;
use interprocess::local_socket::{GenericNamespaced, RecvHalf, SendHalf, Stream, ToNsName};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::host::{DocumentReader, HostApi, InteractiveCommand, ReaderEntity};
use crate::ipc::protocol::{
    HostResponse, HostToPlugin, PluginAsync, PluginRequest, PluginResponse, PluginToHost,
    RunnerHandshake,
};
use crate::ipc::transport::{recv, send};
use crate::shm::{DocumentViewInfo, SharedDocumentReader};

/// Shared registry of active interactive commands, keyed by host-assigned id.
pub type InteractiveRegistry = Arc<Mutex<HashMap<u64, Box<dyn InteractiveCommand>>>>;

/// Internal stream storage for `IpcClient`. A client may hold a full
/// bidirectional stream (sync socket), only the send half (async socket in
/// the runner when no responses are expected), or both halves of a split
/// stream (async socket when the async event thread needs to perform
/// synchronous request/response calls).
#[derive(Clone)]
enum ClientStream {
    Full(Arc<Mutex<Stream>>),
    Split {
        send: Arc<Mutex<SendHalf>>,
        recv: Arc<Mutex<RecvHalf>>,
    },
}

/// Plugin-side connection to the host for one direction of traffic.
///
/// The runner keeps two independent clients: one for the sync socket and one
/// for the async socket. `PluginHostApi` routes requests accordingly.
#[derive(Clone)]
pub struct IpcClient {
    stream: ClientStream,
    /// When `true`, synchronous host requests are sent fire-and-forget instead
    /// of blocking for a response. Used by the runner while executing
    /// `on_async_event` so the handler never blocks waiting for the host.
    async_mode: Cell<bool>,
}

impl IpcClient {
    pub fn connect(name: &str) -> std::io::Result<Self> {
        let name = name
            .to_ns_name::<GenericNamespaced>()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let stream = StreamTrait::connect(name)?;
        Ok(Self::from_stream(stream))
    }

    pub(crate) fn from_stream(stream: Stream) -> Self {
        Self {
            stream: ClientStream::Full(Arc::new(Mutex::new(stream))),
            async_mode: Cell::new(false),
        }
    }

    /// Consume this full-stream client and split it into a full-duplex client
    /// that owns both halves of the async socket. This lets the async event
    /// thread perform synchronous request/response calls over the async socket.
    pub(crate) fn split(self) -> Self {
        match self.stream {
            ClientStream::Full(stream) => {
                let stream = Arc::try_unwrap(stream)
                    .expect("IpcClient has no clones when split")
                    .into_inner()
                    .expect("stream mutex not poisoned");
                let (recv, send) = stream.split();
                Self {
                    stream: ClientStream::Split {
                        send: Arc::new(Mutex::new(send)),
                        recv: Arc::new(Mutex::new(recv)),
                    },
                    async_mode: Cell::new(self.async_mode.get()),
                }
            }
            ClientStream::Split { .. } => {
                panic!("cannot split a split IPC client")
            }
        }
    }

    /// Set whether this client runs in async event mode. In async mode,
    /// requests are sent fire-and-forget and a default response is returned.
    pub(crate) fn set_async_mode(&self, enabled: bool) {
        self.async_mode.set(enabled);
    }

    fn with_writer<F, T>(&self, f: F) -> Result<T, crate::ipc::transport::TransportError>
    where
        F: FnOnce(&mut dyn Write) -> Result<T, crate::ipc::transport::TransportError>,
    {
        match &self.stream {
            ClientStream::Full(stream) => {
                let mut guard = stream.lock().unwrap_or_else(|e| e.into_inner());
                f(&mut *guard)
            }
            ClientStream::Split { send, .. } => {
                let mut guard = send.lock().unwrap_or_else(|e| e.into_inner());
                f(&mut *guard)
            }
        }
    }

    fn with_reader<F, T>(&self, f: F) -> Result<T, crate::ipc::transport::TransportError>
    where
        F: FnOnce(&mut dyn Read) -> Result<T, crate::ipc::transport::TransportError>,
    {
        match &self.stream {
            ClientStream::Full(stream) => {
                let mut guard = stream.lock().unwrap_or_else(|e| e.into_inner());
                f(&mut *guard)
            }
            ClientStream::Split { recv, .. } => {
                let mut guard = recv.lock().unwrap_or_else(|e| e.into_inner());
                f(&mut *guard)
            }
        }
    }

    /// Send the initial runner handshake presenting the pre-shared token.
    pub fn send_handshake(&self, token: &str) -> Result<(), crate::ipc::transport::TransportError> {
        self.with_writer(|w| send(w, &RunnerHandshake::Token(token.to_string())))
    }

    /// Send a plugin request and return immediately without waiting for a
    /// response. On the full sync socket this still performs a normal
    /// request/response round-trip because the peer is guaranteed to be
    /// listening. On the split async socket the request is sent
    /// fire-and-forget and `PluginResponse::Ok` is returned; this keeps
    /// one-way notifications from blocking the async event thread when the
    /// test host (or a busy UI frame) has not yet drained the request queue.
    pub fn request(
        &self,
        req: PluginRequest,
    ) -> Result<PluginResponse, crate::ipc::transport::TransportError> {
        match &self.stream {
            ClientStream::Full(_) => self.request_response(req),
            ClientStream::Split { .. } => self
                .with_writer(|w| send(w, &PluginToHost::Request(req)))
                .map(|_| PluginResponse::Ok),
        }
    }

    /// Send a plugin request and block until the matching response arrives.
    /// Used for calls that need a real return value (entity handles, document
    /// snapshots, shared-memory view path, XDATA records, panel results) even
    /// from the async event thread.
    pub fn request_response(
        &self,
        req: PluginRequest,
    ) -> Result<PluginResponse, crate::ipc::transport::TransportError> {
        self.with_writer(|w| send(w, &PluginToHost::Request(req)))?;
        loop {
            match self.with_reader(|r| recv(r))? {
                HostToPlugin::Response(resp) => return Ok(resp),
                HostToPlugin::Request(host_req) => {
                    let resp = HostResponse::Error(format!(
                        "unexpected nested host request: {host_req:?}"
                    ));
                    self.with_writer(|w| send(w, &PluginToHost::Response(resp)))?;
                }
                HostToPlugin::Async(event) => {
                    eprintln!("[plugin] dropping async event during sync request: {event:?}");
                }
            }
        }
    }

    /// Send an asynchronous plugin event to the host on this client's socket.
    /// Thread-safe.
    pub fn send_async(
        &self,
        event: PluginAsync,
    ) -> Result<(), crate::ipc::transport::TransportError> {
        self.with_writer(|w| send(w, &PluginToHost::Async(event)))
    }

    /// Receive one length-framed message from the host. Used by the runner main
    /// thread, which acts as the server side of the sync socket.
    pub(crate) fn recv<T: DeserializeOwned>(
        &self,
    ) -> Result<T, crate::ipc::transport::TransportError> {
        self.with_reader(|r| recv(r))
    }

    /// Send one length-framed message to the host. Used by the runner main
    /// thread, which acts as the server side of the sync socket.
    pub(crate) fn send<T: Serialize>(
        &self,
        msg: &T,
    ) -> Result<(), crate::ipc::transport::TransportError> {
        self.with_writer(|w| send(w, msg))
    }
}

/// `HostApi` implementation used inside the plugin process. Every host-mutating
/// method is an RPC; `document()` / `document_mut()` return a local cached copy.
pub struct PluginHostApi {
    sync_client: IpcClient,
    pub(crate) async_client: IpcClient,
    tab_index: usize,
    document_cache: OnceCell<CadDocument>,
    interactive: InteractiveRegistry,
    next_command_id: Cell<u64>,
    /// Cache XDATA records so repeated reads for the same (handle, app) return
    /// stable references without leaking on every call. Each distinct record is
    /// leaked once per plugin dispatch/interactive session.
    record_cache: RefCell<HashMap<(Handle, String), &'static ExtendedDataRecord>>,
    /// Shared-memory document view information, lazily fetched on first
    /// `document_reader()` access.
    doc_view: RefCell<Option<DocumentViewInfo>>,
}

impl PluginHostApi {
    pub fn new(
        sync_client: IpcClient,
        async_client: IpcClient,
        tab_index: usize,
        interactive: InteractiveRegistry,
    ) -> Self {
        Self {
            sync_client,
            async_client,
            tab_index,
            document_cache: OnceCell::new(),
            interactive,
            next_command_id: Cell::new(1),
            record_cache: RefCell::new(HashMap::new()),
            doc_view: RefCell::new(None),
        }
    }

    /// Set whether this proxy runs in async event mode. In async mode,
    /// synchronous host requests are sent fire-and-forget on the async socket
    /// so the plugin's `on_async_event` handler never blocks waiting for the host.
    pub(crate) fn set_async_mode(&self, enabled: bool) {
        self.sync_client.set_async_mode(enabled);
        self.async_client.set_async_mode(enabled);
    }

    fn active_client(&self) -> &IpcClient {
        if self.async_client.async_mode.get() {
            &self.async_client
        } else {
            &self.sync_client
        }
    }

    fn fetch_document(&self) -> CadDocument {
        match self
            .active_client()
            .request_response(PluginRequest::DocumentSnapshot)
        {
            Ok(PluginResponse::Document(doc)) => doc,
            Ok(other) => {
                eprintln!("[plugin] unexpected DocumentSnapshot response: {other:?}");
                CadDocument::default()
            }
            Err(e) => {
                eprintln!("[plugin] failed to fetch document snapshot: {e}");
                CadDocument::default()
            }
        }
    }
}

impl HostApi for PluginHostApi {
    fn tab_index(&self) -> usize {
        self.tab_index
    }

    fn document(&self) -> &CadDocument {
        self.document_cache.get_or_init(|| self.fetch_document())
    }

    fn document_mut(&mut self) -> &mut CadDocument {
        if self.document_cache.get().is_none() {
            let doc = self.fetch_document();
            let _ = self.document_cache.set(doc);
        }
        self.document_cache.get_mut().expect("document initialized")
    }

    fn add_entity(&mut self, entity: EntityType) -> Handle {
        match self
            .active_client()
            .request_response(PluginRequest::AddEntity(entity))
        {
            Ok(PluginResponse::Handle(h)) => {
                self.document_cache = OnceCell::new();
                h
            }
            Ok(other) => {
                eprintln!("[plugin] unexpected AddEntity response: {other:?}");
                Handle::default()
            }
            Err(e) => {
                eprintln!("[plugin] AddEntity failed: {e}");
                Handle::default()
            }
        }
    }

    fn update_entity(&mut self, entity: EntityType) -> bool {
        match self
            .active_client()
            .request_response(PluginRequest::UpdateEntity(entity))
        {
            Ok(PluginResponse::Bool(b)) => {
                if b {
                    // The cached snapshot is now stale; drop it so a later
                    // document() re-fetches the host's post-edit truth.
                    self.document_cache = OnceCell::new();
                }
                b
            }
            Ok(other) => {
                eprintln!("[plugin] unexpected UpdateEntity response: {other:?}");
                false
            }
            Err(e) => {
                eprintln!("[plugin] UpdateEntity failed: {e}");
                false
            }
        }
    }

    fn bump_geometry(&mut self) {
        let _ = self.active_client().request(PluginRequest::BumpGeometry);
    }

    fn read_record(&self, handle: Handle, app_name: &str) -> Option<&ExtendedDataRecord> {
        let key = (handle, app_name.to_string());
        {
            let cache = self.record_cache.borrow();
            if let Some(&r) = cache.get(&key) {
                return Some(r);
            }
        }
        match self.active_client().request_response(PluginRequest::ReadRecord {
            handle,
            app_name: app_name.to_string(),
        }) {
            Ok(PluginResponse::Record(rec)) => rec.map(|r| {
                // Leak once per distinct (handle, app_name) and reuse the
                // reference for the lifetime of this PluginHostApi.
                let leaked: &'static ExtendedDataRecord = Box::leak(Box::new(r));
                self.record_cache.borrow_mut().insert(key, leaked);
                leaked
            }),
            Ok(other) => {
                eprintln!("[plugin] unexpected ReadRecord response: {other:?}");
                None
            }
            Err(e) => {
                eprintln!("[plugin] ReadRecord failed: {e}");
                None
            }
        }
    }

    fn write_record(&mut self, handle: Handle, record: ExtendedDataRecord) -> bool {
        let app = record.application_name.clone();
        match self
            .active_client()
            .request_response(PluginRequest::WriteRecord { handle, record })
        {
            Ok(PluginResponse::Bool(b)) => {
                if b {
                    self.record_cache.borrow_mut().remove(&(handle, app));
                }
                b
            }
            Ok(other) => {
                eprintln!("[plugin] unexpected WriteRecord response: {other:?}");
                false
            }
            Err(e) => {
                eprintln!("[plugin] WriteRecord failed: {e}");
                false
            }
        }
    }

    fn remove_record(&mut self, handle: Handle, app_name: &str) -> bool {
        match self.active_client().request_response(PluginRequest::RemoveRecord {
            handle,
            app_name: app_name.to_string(),
        }) {
            Ok(PluginResponse::Bool(b)) => {
                if b {
                    self.record_cache
                        .borrow_mut()
                        .remove(&(handle, app_name.to_string()));
                }
                b
            }
            Ok(other) => {
                eprintln!("[plugin] unexpected RemoveRecord response: {other:?}");
                false
            }
            Err(e) => {
                eprintln!("[plugin] RemoveRecord failed: {e}");
                false
            }
        }
    }

    fn push_undo(&mut self, label: &str) {
        if let Err(e) = self.active_client().request(PluginRequest::PushUndo {
            label: label.to_string(),
        }) {
            eprintln!("[plugin] push_undo failed: {e}");
        }
    }

    fn set_dirty(&mut self) {
        if let Err(e) = self.active_client().request(PluginRequest::SetDirty) {
            eprintln!("[plugin] set_dirty failed: {e}");
        }
    }

    fn push_info(&mut self, msg: &str) {
        if let Err(e) = self
            .active_client()
            .request(PluginRequest::PushInfo(msg.to_string()))
        {
            eprintln!("[plugin] push_info failed: {e}");
        }
    }

    fn push_output(&mut self, msg: &str) {
        if let Err(e) = self
            .active_client()
            .request(PluginRequest::PushOutput(msg.to_string()))
        {
            eprintln!("[plugin] push_output failed: {e}");
        }
    }

    fn push_error(&mut self, msg: &str) {
        if let Err(e) = self
            .active_client()
            .request(PluginRequest::PushError(msg.to_string()))
        {
            eprintln!("[plugin] push_error failed: {e}");
        }
    }

    fn start_interactive(&mut self, command: Box<dyn InteractiveCommand>) {
        let id = self.next_command_id.get();
        self.next_command_id.set(id + 1);
        self.interactive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, command);
        if let Err(e) = self
            .active_client()
            .request(PluginRequest::StartInteractive { command_id: id })
        {
            eprintln!("[plugin] start_interactive failed: {e}");
        }
    }

    fn plugin_state_any(&self, _plugin_id: &str) -> Option<&(dyn Any + Send + Sync)> {
        // Per-tab plugin state stored in the host cannot cross the process
        // boundary because `dyn Any` is not serializable. Plugins should keep
        // their own state inside the plugin process.
        None
    }

    fn plugin_state_any_mut(&mut self, _plugin_id: &str) -> Option<&mut (dyn Any + Send + Sync)> {
        None
    }

    fn ensure_plugin_state_any(
        &mut self,
        _plugin_id: &'static str,
        _init: &mut dyn FnMut() -> Box<dyn Any + Send + Sync>,
    ) -> &mut (dyn Any + Send + Sync) {
        // Same limitation as `plugin_state_any`. This would need a serializable
        // state contract to work across processes.
        panic!("ensure_plugin_state is not supported for out-of-process plugins; keep state in the plugin crate")
    }

    fn document_reader(&self) -> Box<dyn DocumentReader + '_> {
        {
            let mut view = self.doc_view.borrow_mut();
            if view.is_none() {
                match self
                    .active_client()
                    .request_response(PluginRequest::OpenDocumentView)
                {
                    Ok(PluginResponse::DocumentView { path, version }) => {
                        *view = Some(DocumentViewInfo { path, version });
                    }
                    Ok(other) => {
                        eprintln!("[plugin] unexpected OpenDocumentView response: {other:?}");
                    }
                    Err(e) => {
                        eprintln!("[plugin] OpenDocumentView request failed: {e}");
                    }
                }
            }
        }
        match self.doc_view.borrow().as_ref() {
            Some(info) => match SharedDocumentReader::open(Path::new(&info.path)) {
                Ok(reader) => Box::new(reader),
                Err(e) => {
                    eprintln!(
                        "[plugin] failed to open document view at {}: {e}",
                        info.path
                    );
                    Box::new(EmptyDocumentReader)
                }
            },
            None => Box::new(EmptyDocumentReader),
        }
    }

    fn open_panel(
        &mut self,
        def: &crate::panel::PanelDef,
    ) -> Result<crate::panel::PanelHandle, crate::panel::PanelError> {
        match self
            .active_client()
            .request_response(PluginRequest::OpenPanel { def: def.clone() })
        {
            Ok(PluginResponse::PanelHandleResult(result)) => result,
            Ok(other) => {
                eprintln!("[plugin] unexpected OpenPanel response: {other:?}");
                Err(crate::panel::PanelError::Io(
                    "unexpected response".to_string(),
                ))
            }
            Err(e) => {
                eprintln!("[plugin] OpenPanel failed: {e}");
                Err(crate::panel::PanelError::Io(e.to_string()))
            }
        }
    }

    fn close_panel(
        &mut self,
        handle: crate::panel::PanelHandle,
    ) -> Result<(), crate::panel::PanelError> {
        match self
            .active_client()
            .request_response(PluginRequest::ClosePanel { handle })
        {
            Ok(PluginResponse::PanelResult(result)) => result,
            Ok(other) => {
                eprintln!("[plugin] unexpected ClosePanel response: {other:?}");
                Err(crate::panel::PanelError::Io(
                    "unexpected response".to_string(),
                ))
            }
            Err(e) => {
                eprintln!("[plugin] ClosePanel failed: {e}");
                Err(crate::panel::PanelError::Io(e.to_string()))
            }
        }
    }

    fn move_panel(
        &mut self,
        handle: crate::panel::PanelHandle,
        x: f32,
        y: f32,
    ) -> Result<(), crate::panel::PanelError> {
        match self
            .active_client()
            .request_response(PluginRequest::MovePanel { handle, x, y })
        {
            Ok(PluginResponse::PanelResult(result)) => result,
            Ok(other) => {
                eprintln!("[plugin] unexpected MovePanel response: {other:?}");
                Err(crate::panel::PanelError::Io(
                    "unexpected response".to_string(),
                ))
            }
            Err(e) => {
                eprintln!("[plugin] MovePanel failed: {e}");
                Err(crate::panel::PanelError::Io(e.to_string()))
            }
        }
    }

    fn resize_panel(
        &mut self,
        handle: crate::panel::PanelHandle,
        width: f32,
        height: f32,
    ) -> Result<(), crate::panel::PanelError> {
        match self.active_client().request_response(PluginRequest::ResizePanel {
            handle,
            width,
            height,
        }) {
            Ok(PluginResponse::PanelResult(result)) => result,
            Ok(other) => {
                eprintln!("[plugin] unexpected ResizePanel response: {other:?}");
                Err(crate::panel::PanelError::Io(
                    "unexpected response".to_string(),
                ))
            }
            Err(e) => {
                eprintln!("[plugin] ResizePanel failed: {e}");
                Err(crate::panel::PanelError::Io(e.to_string()))
            }
        }
    }

    fn dock_panel(
        &mut self,
        handle: crate::panel::PanelHandle,
        zone: crate::panel::DockZone,
    ) -> Result<(), crate::panel::PanelError> {
        match self
            .active_client()
            .request_response(PluginRequest::DockPanel { handle, zone })
        {
            Ok(PluginResponse::PanelResult(result)) => result,
            Ok(other) => {
                eprintln!("[plugin] unexpected DockPanel response: {other:?}");
                Err(crate::panel::PanelError::Io(
                    "unexpected response".to_string(),
                ))
            }
            Err(e) => {
                eprintln!("[plugin] DockPanel failed: {e}");
                Err(crate::panel::PanelError::Io(e.to_string()))
            }
        }
    }

    fn undock_panel(
        &mut self,
        handle: crate::panel::PanelHandle,
        x: f32,
        y: f32,
    ) -> Result<(), crate::panel::PanelError> {
        match self
            .active_client()
            .request_response(PluginRequest::UndockPanel { handle, x, y })
        {
            Ok(PluginResponse::PanelResult(result)) => result,
            Ok(other) => {
                eprintln!("[plugin] unexpected UndockPanel response: {other:?}");
                Err(crate::panel::PanelError::Io(
                    "unexpected response".to_string(),
                ))
            }
            Err(e) => {
                eprintln!("[plugin] UndockPanel failed: {e}");
                Err(crate::panel::PanelError::Io(e.to_string()))
            }
        }
    }

    fn post_panel_event(
        &mut self,
        handle: crate::panel::PanelHandle,
        event: crate::panel::PanelEvent,
    ) -> Result<(), crate::panel::PanelError> {
        match self
            .active_client()
            .request_response(PluginRequest::PostPanelEvent { handle, event })
        {
            Ok(PluginResponse::PanelResult(result)) => result,
            Ok(other) => {
                eprintln!("[plugin] unexpected PostPanelEvent response: {other:?}");
                Err(crate::panel::PanelError::Io(
                    "unexpected response".to_string(),
                ))
            }
            Err(e) => {
                eprintln!("[plugin] PostPanelEvent failed: {e}");
                Err(crate::panel::PanelError::Io(e.to_string()))
            }
        }
    }

    fn send_async(&mut self, event: PluginAsync) {
        if let Err(e) = self.async_client.send_async(event) {
            eprintln!("[plugin] send_async failed: {e}");
        }
    }

    fn request_point_pick(&mut self, panel_id: &str) -> Result<(), String> {
        match self
            .active_client()
            .request(PluginRequest::RequestPointPick {
                panel_id: panel_id.to_string(),
            }) {
            Ok(PluginResponse::Ok) => Ok(()),
            Ok(other) => Err(format!("unexpected response: {other:?}")),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set_active_tab(&mut self, tab: usize) -> Result<(), String> {
        match self
            .active_client()
            .request_response(PluginRequest::SetActiveTab(tab))
        {
            Ok(PluginResponse::Ok) => Ok(()),
            Ok(PluginResponse::Error(e)) => Err(e),
            Ok(other) => Err(format!("unexpected SetActiveTab response: {other:?}")),
            Err(e) => Err(e.to_string()),
        }
    }

    fn remove_entity(&mut self, handle: Handle) -> Option<EntityType> {
        match self
            .active_client()
            .request_response(PluginRequest::RemoveEntity { handle })
        {
            Ok(PluginResponse::Entity(entity)) => Some(entity),
            Ok(other) => {
                eprintln!("[plugin] unexpected RemoveEntity response: {other:?}");
                None
            }
            Err(e) => {
                eprintln!("[plugin] RemoveEntity failed: {e}");
                None
            }
        }
    }
}

/// Sentinel reader used when the shared-memory view could not be initialized.
struct EmptyDocumentReader;

impl DocumentReader for EmptyDocumentReader {
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

#[cfg(all(test, feature = "host"))]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use acadrust::entities::Point;
    use acadrust::{EntityType, Handle};
    use interprocess::local_socket::{
        traits::{Listener, Stream as StreamTrait},
        GenericNamespaced, ListenerOptions, Stream, ToNsName,
    };

    use crate::host::HostApi;
    use crate::ipc::client::{IpcClient, PluginHostApi};
    use crate::ipc::protocol::{HostToPlugin, PluginRequest, PluginResponse, PluginToHost};
    use crate::ipc::transport::{recv, send};

    fn unique_socket_name() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("ocs_plugin_client_test_{}_{}", std::process::id(), n)
    }

    fn make_client() -> (PluginHostApi, Stream) {
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
        let client_stream = client_thread.join().expect("client thread");
        let client = IpcClient::from_stream(server);
        let api = PluginHostApi::new(
            client.clone(),
            client,
            0,
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        );
        (api, client_stream)
    }

    #[test]
    fn push_info_emits_request() {
        let (mut api, mut peer) = make_client();
        let peer_handle = thread::spawn(move || {
            let msg = recv(&mut peer).unwrap();
            match msg {
                PluginToHost::Request(PluginRequest::PushInfo(s)) => assert_eq!(s, "hello host"),
                other => panic!("unexpected: {other:?}"),
            }
            send(&mut peer, &HostToPlugin::Response(PluginResponse::Ok)).unwrap();
        });
        api.push_info("hello host");
        peer_handle.join().unwrap();
    }

    #[test]
    fn add_entity_awaits_handle_response() {
        let (mut api, mut peer) = make_client();
        let peer_handle = thread::spawn(move || {
            let msg = recv(&mut peer).unwrap();
            match msg {
                PluginToHost::Request(PluginRequest::AddEntity(_)) => {}
                other => panic!("unexpected: {other:?}"),
            }
            send(
                &mut peer,
                &HostToPlugin::Response(PluginResponse::Handle(Handle::new(42))),
            )
            .unwrap();
        });
        let handle = api.add_entity(EntityType::Point(Point::new()));
        peer_handle.join().unwrap();
        assert_eq!(handle, Handle::new(42));
    }

    #[test]
    fn update_entity_awaits_bool_response() {
        let (mut api, mut peer) = make_client();
        let peer_handle = thread::spawn(move || {
            let msg = recv(&mut peer).unwrap();
            match msg {
                PluginToHost::Request(PluginRequest::UpdateEntity(_)) => {}
                other => panic!("unexpected: {other:?}"),
            }
            send(&mut peer, &HostToPlugin::Response(PluginResponse::Bool(true))).unwrap();
        });
        assert!(api.update_entity(EntityType::Point(Point::new())));
        peer_handle.join().unwrap();
    }

    #[test]
    fn remove_entity_awaits_entity_response() {
        let (mut api, mut peer) = make_client();
        let peer_handle = thread::spawn(move || {
            let msg = recv(&mut peer).unwrap();
            match msg {
                PluginToHost::Request(PluginRequest::RemoveEntity { handle }) => {
                    assert_eq!(handle, Handle::new(7));
                }
                other => panic!("unexpected: {other:?}"),
            }
            send(
                &mut peer,
                &HostToPlugin::Response(PluginResponse::Entity(EntityType::Point(Point::new()))),
            )
            .unwrap();
        });
        assert!(api.remove_entity(Handle::new(7)).is_some());
        peer_handle.join().unwrap();
    }
}
