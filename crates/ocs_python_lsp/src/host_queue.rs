//! In-process request queue between LSP server threads and `on_async_event`.
//!
//! LSP server threads push `(tab, PluginRequest, reply_tx)`; the plugin's
//! `on_async_event` drains the queue, temporarily switches to `tab`, applies
//! the request via `HostApi`, and sends the `PluginResponse` back.

use crossbeam_channel::{bounded, Receiver, Sender};
use ocs_plugin_api::ipc::protocol::{PluginRequest, PluginResponse};

/// One queued host request plus the tab it targets and a oneshot reply channel.
pub type QueueItem = (usize, PluginRequest, Sender<PluginResponse>);

/// Thread-safe queue shared by all LSP server threads.
#[derive(Clone)]
pub struct HostQueue {
    sender: Sender<QueueItem>,
    receiver: Receiver<QueueItem>,
}

impl HostQueue {
    /// Create a new bounded queue.
    pub fn new() -> Self {
        let (sender, receiver) = bounded(1024);
        Self {
            sender,
            receiver,
        }
    }

    /// Push a request targeting `tab` and block until `on_async_event` replies.
    pub fn request(&self, tab: usize, req: PluginRequest) -> Result<PluginResponse, String> {
        let (tx, rx) = bounded(1);
        self.sender
            .send((tab, req, tx))
            .map_err(|e| format!("host queue send failed: {e}"))?;
        rx.recv()
            .map_err(|e| format!("host queue reply channel closed: {e}"))
    }

    /// Non-blocking drain of all currently queued items.
    pub fn drain(&self) -> Vec<QueueItem> {
        let mut items = Vec::new();
        while let Ok(item) = self.receiver.try_recv() {
            items.push(item);
        }
        items
    }
}

impl Default for HostQueue {
    fn default() -> Self {
        Self::new()
    }
}
