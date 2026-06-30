//! Proxy around a V3 async session handle for host-side CAD operations.

use acadrust::entities::{Line, Point};
use acadrust::{EntityType, Handle};
use ocs_plugin_api::host::{AsyncSessionError, AsyncSessionHandle, DocumentReader};
use ocs_plugin_api::ipc::protocol::{PluginRequest, PluginResponse};
use ocs_plugin_api::shm::DocumentViewInfo;

/// Thread-safe wrapper around the async session handle returned by the host.
///
/// The UI and interpreter live on the plugin main thread, but the handle is
/// `Send + Sync` so it can be shared with background worker threads if needed.
pub struct HostProxy {
    handle: Box<dyn AsyncSessionHandle>,
}

impl std::fmt::Debug for HostProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostProxy")
            .field("tab_index", &self.handle.tab_index())
            .finish_non_exhaustive()
    }
}

impl HostProxy {
    /// Wrap a host-provided async session handle.
    pub fn new(handle: Box<dyn AsyncSessionHandle>) -> Self {
        Self { handle }
    }

    /// Tab index this session targets.
    pub fn tab_index(&self) -> usize {
        self.handle.tab_index()
    }

    fn request(&self, req: PluginRequest) -> Result<PluginResponse, AsyncSessionError> {
        self.handle.request(req)
    }

    /// Add a point entity to the active document.
    pub fn add_point(&self, x: f64, y: f64, z: f64) -> Result<Handle, AsyncSessionError> {
        let mut pt = Point::from_coords(x, y, z);
        // The host assigns a real handle when it commits the entity.
        pt.common.handle = Handle::new(0);
        match self.request(PluginRequest::AddEntity(EntityType::Point(pt)))? {
            PluginResponse::Handle(h) => Ok(h),
            other => Err(AsyncSessionError::Transport(format!(
                "unexpected AddEntity response: {other:?}"
            ))),
        }
    }

    /// Add a line entity between two points.
    pub fn add_line(
        &self,
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
    ) -> Result<Handle, AsyncSessionError> {
        let mut line = Line::from_coords(x1, y1, z1, x2, y2, z2);
        line.common.handle = Handle::new(0);
        match self.request(PluginRequest::AddEntity(EntityType::Line(line)))? {
            PluginResponse::Handle(h) => Ok(h),
            other => Err(AsyncSessionError::Transport(format!(
                "unexpected AddEntity response: {other:?}"
            ))),
        }
    }

    /// Push an undo marker onto the host undo stack.
    pub fn push_undo(&self, label: &str) -> Result<(), AsyncSessionError> {
        match self.request(PluginRequest::PushUndo {
            label: label.to_string(),
        })? {
            PluginResponse::Ok => Ok(()),
            other => Err(AsyncSessionError::Transport(format!(
                "unexpected PushUndo response: {other:?}"
            ))),
        }
    }

    /// Ask the host to open/refresh a shared-memory document view.
    pub fn open_document_view(&self) -> Option<DocumentViewInfo> {
        self.handle.document_view()
    }

    /// Return a read-only view of the active document.
    pub fn document_reader(&self) -> Box<dyn DocumentReader + 'static> {
        self.handle.document_reader()
    }

    /// Helper: write a string to the host output console.
    pub fn push_output(&self, msg: &str) -> Result<(), AsyncSessionError> {
        match self.request(PluginRequest::PushOutput(msg.to_string()))? {
            PluginResponse::Ok => Ok(()),
            other => Err(AsyncSessionError::Transport(format!(
                "unexpected PushOutput response: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acadrust::Handle;
    use ocs_plugin_api::host::{AsyncSessionError, AsyncSessionHandle, DocumentReader, ReaderEntity};

    struct MockHandle {
        fail_next: std::sync::Mutex<bool>,
    }

    impl AsyncSessionHandle for MockHandle {
        fn tab_index(&self) -> usize {
            7
        }

        fn request(
            &self,
            req: PluginRequest,
        ) -> Result<PluginResponse, AsyncSessionError> {
            if *self.fail_next.lock().unwrap() {
                return Err(AsyncSessionError::Closed);
            }
            match req {
                PluginRequest::AddEntity(_) => Ok(PluginResponse::Handle(Handle::new(42))),
                PluginRequest::PushUndo { .. } | PluginRequest::PushOutput { .. } => Ok(PluginResponse::Ok),
                _ => Ok(PluginResponse::Error(format!("unmocked: {req:?}"))),
            }
        }

        fn document_reader(&self) -> Box<dyn DocumentReader + 'static> {
            Box::new(EmptyReader)
        }

        fn document_view(&self) -> Option<DocumentViewInfo> {
            None
        }
    }

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

    fn mock() -> MockHandle {
        MockHandle {
            fail_next: std::sync::Mutex::new(false),
        }
    }

    #[test]
    fn proxy_forwards_push_undo() {
        let proxy = HostProxy::new(Box::new(mock()));
        proxy.push_undo("test-undo").unwrap();
    }

    #[test]
    fn proxy_add_point_returns_handle() {
        let proxy = HostProxy::new(Box::new(mock()));
        let h = proxy.add_point(1.0, 2.0, 3.0).unwrap();
        assert_eq!(h, Handle::new(42));
    }

    #[test]
    fn proxy_add_line_returns_handle() {
        let proxy = HostProxy::new(Box::new(mock()));
        let h = proxy.add_line(0.0, 0.0, 0.0, 1.0, 1.0, 1.0).unwrap();
        assert_eq!(h, Handle::new(42));
    }

    #[test]
    fn proxy_propagates_errors() {
        let handle = mock();
        *handle.fail_next.lock().unwrap() = true;
        let proxy = HostProxy::new(Box::new(handle));
        let err = proxy.push_undo("fail").unwrap_err();
        assert!(matches!(err, AsyncSessionError::Closed));
    }
}
