//! Host-side V4 snapshot and notification support for the Python REPL plugin.

use std::sync::Arc;

use ocs_plugin_api::host::{HostNotification, PluginNotification};
use ocs_plugin_api::host_v4::{manager, HostV4SnapshotManager};
use ocs_plugin_api::process::NotificationHandler;
use ocs_plugin_api::shm::DocumentViewInfo;

fn mgr() -> std::sync::MutexGuard<'static, HostV4SnapshotManager> {
    manager().lock().unwrap()
}

/// Open (or refresh) the V4 shared document view for `tab_id`.
pub fn open_document_view_v4(tab_id: u64, doc: &codec::CadDocument) -> Option<DocumentViewInfo> {
    mgr().open(tab_id, doc)
}

/// Close the V4 shared document view for `tab_id`.
pub fn close_document_view_v4(tab_id: u64) {
    mgr().close(tab_id);
}

/// Publish the latest document to the V4 shared view for `tab_id` and broadcast
/// a change notification when successful.
pub fn publish_document_view_v4(tab_id: u64, doc: &codec::CadDocument) {
    let info = mgr().publish(tab_id, doc);
    if let Some(info) = info {
        broadcast(HostNotification::DocumentChangedV4 {
            tab_id,
            version: info.version,
        });
    }
}

/// Notify V4 plugins that a tab has been closed and clean up its snapshot.
pub fn on_tab_closed(tab_id: u64) {
    close_document_view_v4(tab_id);
    broadcast(HostNotification::DocumentTabClosed { tab_id });
}

/// Broadcast a selection change to V4 plugins.
///
/// `HostNotification::SelectionChangedV4` carries the active tab id and the
/// current selection set. The caller is responsible for only calling this when
/// the selection actually changed; see `OpenCADStudio::notify_plugins_selection_changed`.
pub fn publish_selection_changed_v4(tab_id: u64, handles: Vec<codec::Handle>) {
    broadcast(HostNotification::SelectionChangedV4 { tab_id, handles });
}

/// Broadcast the tab-independent `HostNotification::SelectionChanged` to every
/// loaded native plugin (API v9): the plain-handle variant of the selection
/// event, dispatched at the same fingerprint-guarded message boundary as the
/// tab-scoped one above (`OpenCADStudio::notify_plugins_selection_changed`).
pub fn publish_selection_changed(handles: Vec<codec::Handle>) {
    broadcast(HostNotification::SelectionChanged { handles });
}

/// Broadcast the tab-independent `HostNotification::DocumentChanged` to every
/// loaded native plugin (API v9): the geometry epoch of the active tab's
/// document after a mutation settled, dispatched at the fingerprint-guarded
/// message boundary (`OpenCADStudio::notify_plugins_document_changed`).
pub fn publish_document_changed(version: u64) {
    broadcast(HostNotification::DocumentChanged { version });
}

/// Broadcast `HostNotification::DocumentOpened` to every loaded native plugin
/// (API v9): a whole document became resident in a tab — opened from file,
/// created from a template, or loaded by a system open. The counterpart of
/// [`HostNotification::DocumentTabClosed`], so a plugin can re-seed runtime
/// per-document state (e.g. reference-only designations) from load to close.
pub fn publish_document_opened(tab_id: u64) {
    broadcast(HostNotification::DocumentOpened { tab_id });
}

/// Broadcast a change in a tab's active interactive command.
pub fn publish_command_state_changed(tab_id: u64, command: Option<String>) {
    broadcast(HostNotification::CommandStateChanged { tab_id, command });
}

/// Broadcast drawing changes independently of shared-view subscriptions.
pub fn publish_drawing_changed(tab_id: u64, epoch: u64) {
    broadcast(HostNotification::DrawingChanged { tab_id, epoch });
}

/// V4 notification handler installed on the plugin manager.
///
/// Forwards REPL status messages to the log; other notifications are handled
/// by the manager's per-plugin routing.
pub fn notification_handler() -> NotificationHandler {
    Arc::new(|plugin_id, _command_id, notification| {
        if let PluginNotification::ReplStatus { status, message } = notification {
            eprintln!("[{plugin_id}] REPL status {status}: {message}");
        }
    })
}

fn broadcast(notification: HostNotification) {
    #[cfg(test)]
    record_test_dispatch(&notification);
    crate::plugin::external::with_manager(|mgr| {
        mgr.broadcast_notification(None, notification);
    });
}

// ── Test-only dispatch observation ──────────────────────────────────────────
//
// App-level tests assert boundary dispatch behaviour through this per-thread
// log: `cargo test` runs each test on its own thread, so a thread-local log
// records exactly the notifications this test's app instance broadcast,
// without cross-test interference. One recorded entry stands for one fan-out
// to every loaded native plugin (the manager's broadcast), so counts here are
// counts per plugin.

#[cfg(test)]
thread_local! {
    static TEST_DISPATCH_LOG: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn record_test_dispatch(notification: &HostNotification) {
    let kind = match notification {
        HostNotification::DocumentChanged { version } => {
            format!("DocumentChanged({version})")
        }
        HostNotification::SelectionChanged { handles } => {
            format!("SelectionChanged({} handles)", handles.len())
        }
        HostNotification::DocumentOpened { tab_id } => format!("DocumentOpened({tab_id})"),
        HostNotification::DocumentTabClosed { tab_id } => format!("DocumentTabClosed({tab_id})"),
        HostNotification::DrawingChanged { tab_id, epoch } => {
            format!("DrawingChanged({tab_id}, {epoch})")
        }
        HostNotification::SelectionChangedV4 { tab_id, .. } => {
            format!("SelectionChangedV4({tab_id})")
        }
        HostNotification::DocumentChangedV4 { tab_id, version } => {
            format!("DocumentChangedV4({tab_id}, {version})")
        }
        HostNotification::CommandStateChanged { tab_id, .. } => {
            format!("CommandStateChanged({tab_id})")
        }
        _ => "Other".to_string(),
    };
    TEST_DISPATCH_LOG.with(|log| log.borrow_mut().push(kind));
}

/// Drain this thread's recorded plugin-notification broadcasts (test-only).
#[cfg(test)]
pub fn take_test_dispatch_log() -> Vec<String> {
    TEST_DISPATCH_LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}
