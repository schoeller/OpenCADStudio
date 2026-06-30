//! Plugin-side IPC client and `HostApi` proxy.

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use acadrust::xdata::ExtendedDataRecord;
use acadrust::{CadDocument, EntityType, Handle};
use interprocess::local_socket::traits::Stream as StreamTrait;
use interprocess::local_socket::{GenericNamespaced, Stream, ToNsName};

use crate::host::{AsyncSessionError, AsyncSessionHandle, DocumentReader, HostApi, InteractiveCommand, ReaderEntity};
use crate::ipc::protocol::{
    HostRequest, HostResponse, HostToPlugin, PluginRequest, PluginResponse, PluginToHost, RunnerHandshake,
};
use crate::ipc::transport::{recv, send};
use crate::shm::{DocumentViewInfo, SharedDocumentReader};

/// Shared registry of active interactive commands, keyed by host-assigned id.
pub type InteractiveRegistry = Rc<RefCell<HashMap<u64, Box<dyn InteractiveCommand>>>>;

/// Thread-safe variant used by API V3 plugins.
pub type InteractiveRegistryV3 = Arc<Mutex<HashMap<u64, Box<dyn InteractiveCommand>>>>;

/// Plugin-side connection to the host.
#[derive(Clone)]
pub struct IpcClient {
    stream: Rc<RefCell<Stream>>,
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
            stream: Rc::new(RefCell::new(stream)),
        }
    }

    pub fn stream_ref(&self) -> std::cell::RefMut<'_, Stream> {
        self.stream.borrow_mut()
    }

    /// Send the initial runner handshake presenting the pre-shared token.
    pub fn send_handshake(&self, token: &str) -> Result<(), crate::ipc::transport::TransportError> {
        send(
            &mut self.stream.borrow_mut(),
            &RunnerHandshake::Token(token.to_string()),
        )
    }

    /// Send a plugin request and wait for the matching response. Any nested
    /// host requests that arrive while we are waiting are treated as errors.
    pub fn request(
        &self,
        req: PluginRequest,
    ) -> Result<PluginResponse, crate::ipc::transport::TransportError> {
        send(&mut self.stream.borrow_mut(), &PluginToHost::Request(req))?;
        loop {
            match recv::<HostToPlugin>(&mut self.stream.borrow_mut())? {
                HostToPlugin::Response(resp) => return Ok(resp),
                HostToPlugin::Request(host_req) => {
                    let resp = HostResponse::Error(format!(
                        "unexpected nested host request: {host_req:?}"
                    ));
                    send(&mut self.stream.borrow_mut(), &PluginToHost::Response(resp))?;
                }
                HostToPlugin::RequestV3 { .. } | HostToPlugin::ResponseV3 { .. } => {
                    eprintln!("[plugin v2] unexpected V3 message on V2 client");
                    return Err(crate::ipc::transport::TransportError::Io(
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "unexpected V3 message on V2 client",
                        ),
                    ));
                }
            }
        }
    }
}

/// `HostApi` implementation used inside the plugin process. Every host-mutating
/// method is an RPC; `document()` / `document_mut()` return a local cached copy.
pub struct PluginHostApi {
    client: IpcClient,
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
    pub fn new(client: IpcClient, tab_index: usize, interactive: InteractiveRegistry) -> Self {
        Self {
            client,
            tab_index,
            document_cache: OnceCell::new(),
            interactive,
            next_command_id: Cell::new(1),
            record_cache: RefCell::new(HashMap::new()),
            doc_view: RefCell::new(None),
        }
    }

    fn fetch_document(&self) -> CadDocument {
        match self.client.request(PluginRequest::DocumentSnapshot) {
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
        match self.client.request(PluginRequest::AddEntity(entity)) {
            Ok(PluginResponse::Handle(h)) => h,
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

    fn bump_geometry(&mut self) {
        let _ = self.client.request(PluginRequest::BumpGeometry);
    }

    fn read_record(&self, handle: Handle, app_name: &str) -> Option<&ExtendedDataRecord> {
        let key = (handle, app_name.to_string());
        {
            let cache = self.record_cache.borrow();
            if let Some(&r) = cache.get(&key) {
                return Some(r);
            }
        }
        match self.client.request(PluginRequest::ReadRecord {
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
            .client
            .request(PluginRequest::WriteRecord { handle, record })
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
        match self.client.request(PluginRequest::RemoveRecord {
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
        if let Err(e) = self.client.request(PluginRequest::PushUndo {
            label: label.to_string(),
        }) {
            eprintln!("[plugin] push_undo failed: {e}");
        }
    }

    fn set_dirty(&mut self) {
        if let Err(e) = self.client.request(PluginRequest::SetDirty) {
            eprintln!("[plugin] set_dirty failed: {e}");
        }
    }

    fn push_info(&mut self, msg: &str) {
        if let Err(e) = self
            .client
            .request(PluginRequest::PushInfo(msg.to_string()))
        {
            eprintln!("[plugin] push_info failed: {e}");
        }
    }

    fn push_output(&mut self, msg: &str) {
        if let Err(e) = self
            .client
            .request(PluginRequest::PushOutput(msg.to_string()))
        {
            eprintln!("[plugin] push_output failed: {e}");
        }
    }

    fn push_error(&mut self, msg: &str) {
        if let Err(e) = self
            .client
            .request(PluginRequest::PushError(msg.to_string()))
        {
            eprintln!("[plugin] push_error failed: {e}");
        }
    }

    fn start_interactive(&mut self, command: Box<dyn InteractiveCommand>) {
        let id = self.next_command_id.get();
        self.next_command_id.set(id + 1);
        self.interactive.borrow_mut().insert(id, command);
        if let Err(e) = self
            .client
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
                match self.client.request(PluginRequest::OpenDocumentView) {
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
            client,
            0,
            std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
        );
        (api, client_stream)
    }

    #[test]
    fn push_info_emits_request() {
        let (mut api, mut peer) = make_client();
        let peer_handle = thread::spawn(move || {
            let msg = recv::<PluginToHost>(&mut peer).unwrap();
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
            let msg = recv::<PluginToHost>(&mut peer).unwrap();
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
}

// ─────────────────────────────────────────────────────────────────────────────
// V3 async-IPC client and HostApi proxy
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Thread-safe plugin-side connection to the host for API V3.
///
/// Multiple threads can share a single `IpcClientV3`. Requests are serialized
/// internally so only one request/response exchange is in flight at a time;
/// nested host requests received while waiting are dispatched through the
/// supplied callback.
#[derive(Clone)]
pub struct IpcClientV3 {
    pub(crate) stream: Arc<Mutex<Stream>>,
    next_id: Arc<AtomicU64>,
}

impl IpcClientV3 {
    pub fn connect(name: &str) -> std::io::Result<Self> {
        let name = name
            .to_ns_name::<GenericNamespaced>()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let stream = StreamTrait::connect(name)?;
        Ok(Self::from_stream(stream))
    }

    pub(crate) fn from_stream(stream: Stream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(stream)),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn send_handshake(&self, token: &str) -> Result<(), crate::ipc::transport::TransportError> {
        let mut guard = self.stream.lock().unwrap();
        send(&mut *guard, &RunnerHandshake::Token(token.to_string()))
    }

    /// Send a plugin request and wait for the matching host response.
    ///
        /// `on_host_request` is called for any nested host→plugin request that
    /// arrives while we are waiting; its return value is sent back as the
    /// response.
    pub fn request(
        &self,
        session_id: &str,
        req: PluginRequest,
        on_host_request: &mut dyn FnMut(HostRequest) -> HostResponse,
    ) -> Result<PluginResponse, crate::ipc::transport::TransportError> {
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut guard = self.stream.lock().unwrap();
            send(
                &mut *guard,
                &PluginToHost::RequestV3 {
                    request_id,
                    session_id: session_id.to_string(),
                    request: req,
                },
            )?;
        }
        loop {
            let mut guard = self.stream.lock().unwrap();
            match recv::<HostToPlugin>(&mut *guard)? {
                HostToPlugin::ResponseV3 {
                    request_id: rid,
                    response,
                } if rid == request_id => return Ok(response),
                HostToPlugin::ResponseV3 { request_id: rid, .. } => {
                    eprintln!("[ipc v3] unexpected response id {rid} (expected {request_id})");
                }
                HostToPlugin::RequestV3 {
                    request_id: rid,
                    session_id: _,
                    request,
                } => {
                    let response = on_host_request(request);
                    send(
                        &mut *guard,
                        &PluginToHost::ResponseV3 {
                            request_id: rid,
                            response,
                        },
                    )?;
                }
                other => {
                    eprintln!("[ipc v3] unexpected V2 message on V3 path: {other:?}");
                }
            }
        }
    }

    /// Send a response to a host request that was delivered through the
    /// callback above.
    pub fn send_response(
        &self,
        request_id: u64,
        response: HostResponse,
    ) -> Result<(), crate::ipc::transport::TransportError> {
        let mut guard = self.stream.lock().unwrap();
        send(
            &mut *guard,
            &PluginToHost::ResponseV3 {
                request_id,
                response,
            },
        )
    }
}

/// `HostApi` implementation for API V3 plugins. Like `PluginHostApi` it proxies
/// every mutating call to the host, but it uses the thread-safe V3 client so
/// calls can continue after `dispatch` returns.
pub struct PluginHostApiV3 {
    client: IpcClientV3,
    tab_index: usize,
    document_cache: OnceCell<CadDocument>,
    interactive: InteractiveRegistryV3,
    next_command_id: Cell<u64>,
    record_cache: RefCell<HashMap<(Handle, String), &'static ExtendedDataRecord>>,
    doc_view: RefCell<Option<DocumentViewInfo>>,
    async_session_id: RefCell<Option<String>>,
}

impl PluginHostApiV3 {
    pub fn new(client: IpcClientV3, tab_index: usize, interactive: InteractiveRegistryV3) -> Self {
        Self {
            client,
            tab_index,
            document_cache: OnceCell::new(),
            interactive,
            next_command_id: Cell::new(1),
            record_cache: RefCell::new(HashMap::new()),
            doc_view: RefCell::new(None),
            async_session_id: RefCell::new(None),
        }
    }

    fn next_command_id(&self) -> u64 {
        let id = self.next_command_id.get();
        self.next_command_id.set(id + 1);
        id
    }

    fn request(&self, req: PluginRequest) -> PluginResponse {
        match self.client.request("", req, &mut |host_req| {
            // V3 async sessions do not expect nested host→plugin requests.
            // The only expected lifecycle message is EndAsyncSession, which is
            // handled by the dedicated `on_host_request` path in the runner.
            HostResponse::Error(format!(
                "unexpected host request on V3 async path: {host_req:?}"
            ))
        }) {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("[plugin v3] request failed: {e}");
                PluginResponse::Error(e.to_string())
            }
        }
    }

    fn fetch_document(&self) -> CadDocument {
        match self.request(PluginRequest::DocumentSnapshot) {
            PluginResponse::Document(doc) => doc,
            other => {
                eprintln!("[plugin v3] unexpected DocumentSnapshot response: {other:?}");
                CadDocument::default()
            }
        }
    }
}

// Placeholder removed: V3 async sessions do not support nested interactive
// commands from the host.

impl HostApi for PluginHostApiV3 {
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
        match self.request(PluginRequest::AddEntity(entity)) {
            PluginResponse::Handle(h) => h,
            other => {
                eprintln!("[plugin v3] unexpected AddEntity response: {other:?}");
                Handle::default()
            }
        }
    }

    fn bump_geometry(&mut self) {
        let _ = self.request(PluginRequest::BumpGeometry);
    }

    fn read_record(&self, handle: Handle, app_name: &str) -> Option<&ExtendedDataRecord> {
        let key = (handle, app_name.to_string());
        {
            let cache = self.record_cache.borrow();
            if let Some(&r) = cache.get(&key) {
                return Some(r);
            }
        }
        match self.request(PluginRequest::ReadRecord {
            handle,
            app_name: app_name.to_string(),
        }) {
            PluginResponse::Record(rec) => rec.map(|r| {
                let leaked: &'static ExtendedDataRecord = Box::leak(Box::new(r));
                self.record_cache.borrow_mut().insert(key, leaked);
                leaked
            }),
            other => {
                eprintln!("[plugin v3] unexpected ReadRecord response: {other:?}");
                None
            }
        }
    }

    fn write_record(&mut self, handle: Handle, record: ExtendedDataRecord) -> bool {
        let app = record.application_name.clone();
        match self.request(PluginRequest::WriteRecord { handle, record }) {
            PluginResponse::Bool(b) => {
                if b {
                    self.record_cache.borrow_mut().remove(&(handle, app));
                }
                b
            }
            other => {
                eprintln!("[plugin v3] unexpected WriteRecord response: {other:?}");
                false
            }
        }
    }

    fn remove_record(&mut self, handle: Handle, app_name: &str) -> bool {
        match self.request(PluginRequest::RemoveRecord {
            handle,
            app_name: app_name.to_string(),
        }) {
            PluginResponse::Bool(b) => {
                if b {
                    self.record_cache.borrow_mut().remove(&(handle, app_name.to_string()));
                }
                b
            }
            other => {
                eprintln!("[plugin v3] unexpected RemoveRecord response: {other:?}");
                false
            }
        }
    }

    fn push_undo(&mut self, label: &str) {
        let _ = self.request(PluginRequest::PushUndo {
            label: label.to_string(),
        });
    }

    fn set_dirty(&mut self) {
        let _ = self.request(PluginRequest::SetDirty);
    }

    fn push_info(&mut self, msg: &str) {
        let _ = self.request(PluginRequest::PushInfo(msg.to_string()));
    }

    fn push_output(&mut self, msg: &str) {
        let _ = self.request(PluginRequest::PushOutput(msg.to_string()));
    }

    fn push_error(&mut self, msg: &str) {
        let _ = self.request(PluginRequest::PushError(msg.to_string()));
    }

    fn start_interactive(&mut self, command: Box<dyn InteractiveCommand>) {
        let id = self.next_command_id();
        self.interactive.lock().unwrap().insert(id, command);
        let _ = self.request(PluginRequest::StartInteractive { command_id: id });
    }

    fn plugin_state_any(&self, _plugin_id: &str) -> Option<&(dyn Any + Send + Sync)> {
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
        panic!("ensure_plugin_state is not supported for API V3 out-of-process plugins; keep state in the plugin crate")
    }

    fn document_reader(&self) -> Box<dyn DocumentReader + '_> {
        if let Some(session_id) = self.async_session_id.borrow().as_ref() {
            let handle = AsyncSessionHandleV3 {
                client: self.client.clone(),
                tab_index: self.tab_index,
                session_id: session_id.clone(),
            };
            return handle.document_reader();
        }
        let info = self
            .doc_view
            .borrow_mut()
            .get_or_insert_with(|| match self.request(PluginRequest::OpenDocumentView) {
                PluginResponse::DocumentView { path, version } => DocumentViewInfo { path, version },
                other => {
                    eprintln!("[plugin v3] unexpected OpenDocumentView response: {other:?}");
                    DocumentViewInfo {
                        path: String::new(),
                        version: 0,
                    }
                }
            })
            .clone();
        if info.path.is_empty() {
            return Box::new(EmptyDocumentReader);
        }
        match SharedDocumentReader::open(Path::new(&info.path)) {
            Ok(reader) => Box::new(reader),
            Err(e) => {
                eprintln!("[plugin v3] failed to open shared document reader: {e}");
                Box::new(EmptyDocumentReader)
            }
        }
    }

    fn start_async_session(&mut self, session_id: &str) -> Option<Box<dyn AsyncSessionHandle>> {
        match self.request(PluginRequest::StartAsyncSession {
            session_id: session_id.to_string(),
        }) {
            PluginResponse::Ok => {
                *self.async_session_id.borrow_mut() = Some(session_id.to_string());
                Some(Box::new(AsyncSessionHandleV3 {
                    client: self.client.clone(),
                    tab_index: self.tab_index,
                    session_id: session_id.to_string(),
                }))
            }
            other => {
                eprintln!("[plugin v3] start_async_session failed: {other:?}");
                None
            }
        }
    }
}

/// Plugin-side async session handle used after `dispatch` returns.
struct AsyncSessionHandleV3 {
    client: IpcClientV3,
    tab_index: usize,
    #[allow(dead_code)]
    session_id: String,
}

impl AsyncSessionHandle for AsyncSessionHandleV3 {
    fn tab_index(&self) -> usize {
        self.tab_index
    }

    fn request(&self, req: PluginRequest) -> Result<PluginResponse, AsyncSessionError> {
        self.client
            .request(&self.session_id, req, &mut |_host_req| {
                HostResponse::Error(
                    "nested host requests not supported from async session".to_string(),
                )
            })
            .map_err(|e| AsyncSessionError::Transport(e.to_string()))
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
                        eprintln!("[plugin v3] shared reader open failed: {e}");
                        Box::new(EmptyDocumentReader)
                    }
                }
            }
            other => {
                eprintln!("[plugin v3] document_reader failed: {other:?}");
                Box::new(EmptyDocumentReader)
            }
        }
    }

    fn document_view(&self) -> Option<DocumentViewInfo> {
        match self.request(PluginRequest::OpenDocumentView) {
            Ok(PluginResponse::DocumentView { path, version }) => Some(DocumentViewInfo { path, version }),
            _ => None,
        }
    }
}

