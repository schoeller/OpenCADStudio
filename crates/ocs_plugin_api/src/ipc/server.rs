//! Host-side IPC request handler.

use std::sync::Arc;

use crate::host::{CommandStep, HostApi, InteractiveCommand};
use crate::ipc::protocol::{HostAsync, PluginRequest, PluginResponse};
use crate::process::PluginProcess;

/// Apply one plugin request to the host's `HostApi` implementation.
///
/// `on_start_interactive` is called when the plugin starts an interactive
/// command; the host should install an adapter that sends
/// `HostRequest::InteractiveEvent` back to the plugin process.
pub fn handle_plugin_request(
    host: &mut dyn HostApi,
    req: PluginRequest,
    on_start_interactive: &mut dyn FnMut(u64),
) -> PluginResponse {
    use PluginRequest::*;
    match req {
        PushInfo(msg) => {
            host.push_info(&msg);
            PluginResponse::Ok
        }
        PushOutput(msg) => {
            host.push_output(&msg);
            PluginResponse::Ok
        }
        PushError(msg) => {
            host.push_error(&msg);
            PluginResponse::Ok
        }
        AddEntity(entity) => PluginResponse::Handle(host.add_entity(entity)),
        UpdateEntity(entity) => PluginResponse::Bool(host.update_entity(entity)),
        BumpGeometry => {
            host.bump_geometry();
            PluginResponse::Ok
        }
        ReadRecord { handle, app_name } => {
            PluginResponse::Record(host.read_record(handle, &app_name).cloned())
        }
        WriteRecord { handle, record } => PluginResponse::Bool(host.write_record(handle, record)),
        RemoveRecord { handle, app_name } => {
            PluginResponse::Bool(host.remove_record(handle, &app_name))
        }
        PushUndo { label } => {
            host.push_undo(&label);
            PluginResponse::Ok
        }
        SetDirty => {
            host.set_dirty();
            PluginResponse::Ok
        }
        StartInteractive { command_id } => {
            on_start_interactive(command_id);
            PluginResponse::Ok
        }
        DocumentSnapshot => PluginResponse::Document(host.document().clone()),
        OpenDocumentView => match host.document_view() {
            Some(info) => PluginResponse::DocumentView {
                path: info.path,
                version: info.version,
            },
            None => PluginResponse::Error("shared document view unavailable".to_string()),
        },
        OpenPanel { def } => PluginResponse::PanelHandleResult(host.open_panel(&def)),
        ClosePanel { handle } => PluginResponse::PanelResult(host.close_panel(handle)),
        MovePanel { handle, x, y } => PluginResponse::PanelResult(host.move_panel(handle, x, y)),
        ResizePanel {
            handle,
            width,
            height,
        } => PluginResponse::PanelResult(host.resize_panel(handle, width, height)),
        DockPanel { handle, zone } => PluginResponse::PanelResult(host.dock_panel(handle, zone)),
        UndockPanel { handle, x, y } => {
            PluginResponse::PanelResult(host.undock_panel(handle, x, y))
        }
        PostPanelEvent { handle, event } => {
            PluginResponse::PanelResult(host.post_panel_event(handle, event))
        }
        RequestPointPick { panel_id } => match host.current_process() {
            Some(process) => {
                host.start_interactive(Box::new(PointPickAdapter { process, panel_id }));
                PluginResponse::Ok
            }
            None => PluginResponse::Error("point pick requires out-of-process plugin".to_string()),
        },
        SetActiveTab(tab) => match host.set_active_tab(tab) {
            Ok(()) => PluginResponse::Ok,
            Err(e) => PluginResponse::Error(e),
        },
        RemoveEntity { handle } => match host.remove_entity(handle) {
            true => PluginResponse::Bool(true),
            false => PluginResponse::Error(format!("entity {} not found", handle.value())),
        },
        RemoveEntities { handles } => {
            let mut removed = 0usize;
            for handle in handles {
                if host.remove_entity(handle) {
                    removed += 1;
                }
            }
            PluginResponse::Count(removed)
        }
    }
}

/// Interactive command that sends picked coordinates back to the originating
/// plugin process as a `HostAsync::CoordinatesPicked` event.
struct PointPickAdapter {
    process: Arc<PluginProcess>,
    panel_id: String,
}

impl InteractiveCommand for PointPickAdapter {
    fn prompt(&self) -> String {
        "Pick a point".to_string()
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        let _ = self.process.send_async(HostAsync::CoordinatesPicked {
            panel_id: self.panel_id.clone(),
            point: pt,
        });
        CommandStep::Done
    }
}
