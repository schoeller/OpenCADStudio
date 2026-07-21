//! Request/response envelopes exchanged between the host and a plugin process.
//!
//! Two independent local sockets are used:
//! - The **sync** socket carries host→plugin `Request`s and plugin→host
//!   `Response`s (plus nested plugin→host `Request`s handled inline).
//! - The **async** socket carries host→plugin `Async` events and plugin→host
//!   `Async` events and fire-and-forget `Request`s.
//!
//! Splitting the traffic lets the host read plugin-initiated events on a
//! background thread instead of only while blocked waiting for a synchronous
//! response.

use serde::{Deserialize, Serialize};

use crate::host::CommandStep;
use crate::manifest::ApiVersion;
use crate::panel::{DockZone, PanelDef, PanelError, PanelEvent, PanelHandle, Widget};
use crate::ribbon::owned::{OwnedPluginManifest, OwnedRibbonGroup};
use crate::shm::EntityOp;

pub use acadrust::xdata::ExtendedDataRecord;
pub use acadrust::{CadDocument, EntityType, Handle};

/// Events the host forwards to an active plugin `InteractiveCommand`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InteractiveEvent {
    Point([f64; 3]),
    Enter,
    ObjectPick { handle: Handle, pt: [f64; 3] },
}

/// Initial handshake sent by the plugin runner immediately after connecting.
///
/// The runner proves it was spawned by this host by presenting a pre-shared
/// token delivered through the `OCS_PLUGIN_TOKEN` environment variable. A
/// mismatch causes the host to close the connection.
#[derive(Debug, Serialize, Deserialize)]
pub enum RunnerHandshake {
    Token(String),
}

/// Environment variable through which the host passes the pre-shared
/// authentication token to the plugin runner child process.
pub const PLUGIN_TOKEN_ENV: &str = "OCS_PLUGIN_TOKEN";

/// Events the host forwards to an active plugin asynchronously.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostAsync {
    DocumentActivated {
        tab: usize,
    },
    DocumentChanged {
        tab: usize,
        version: u64,
    },
    TabClosed {
        tab: usize,
    },
    PanelEvent {
        panel_id: String,
        event: PanelEvent,
    },
    /// Result of a `RequestPointPick` round-trip. The host started the point
    /// pick after the plugin requested it; when the user clicks, the picked
    /// coordinates are delivered here.
    CoordinatesPicked {
        panel_id: String,
        point: [f64; 3],
    },
}

/// Events a plugin forwards to the host asynchronously.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginAsync {
    PanelUpdate {
        panel_id: String,
        widgets: Vec<Widget>,
    },
    PanelClosed {
        panel_id: String,
    },
    /// Signal the host to drain the shared mutation queue and apply the
    /// batched entity operations. Used by the Python REPL plugin.
    DocumentRefreshRequested,
}

/// Requests the host sends to the plugin runner.
#[derive(Debug, Serialize, Deserialize)]
pub enum HostRequest {
    GetManifest,
    GetRibbon,
    Dispatch {
        cmd: String,
    },
    InteractiveEvent {
        command_id: u64,
        event: InteractiveEvent,
    },
    GetPrompt {
        command_id: u64,
    },
    NeedsEntityPick {
        command_id: u64,
    },
    /// Fetch the panels declared by this plugin (API v3).
    GetPanels,
    Shutdown,
}

/// Responses the plugin runner sends back for `HostRequest`.
#[derive(Debug, Serialize, Deserialize)]
pub enum HostResponse {
    Bool(bool),
    CommandStep(CommandStep),
    Text(String),
    Ribbon(Vec<OwnedRibbonGroup>),
    Manifest(OwnedPluginManifest),
    /// Panel declarations returned by `GetPanels` (API v3).
    Panels(Vec<PanelDef>),
    Error(String),
}

/// Requests the plugin runner sends to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginRequest {
    PushInfo(String),
    PushOutput(String),
    PushError(String),
    AddEntity(EntityType),
    /// Replace the existing entity carrying this entity's handle in place.
    UpdateEntity(EntityType),
    BumpGeometry,
    ReadRecord {
        handle: Handle,
        app_name: String,
    },
    WriteRecord {
        handle: Handle,
        record: ExtendedDataRecord,
    },
    RemoveRecord {
        handle: Handle,
        app_name: String,
    },
    PushUndo {
        label: String,
    },
    SetDirty,
    StartInteractive {
        command_id: u64,
    },
    DocumentSnapshot,
    /// Ask the host to create/refresh a shared-memory document view and return
    /// the file path + current version.
    OpenDocumentView,
    /// Ask the host to create/refresh a shared-memory full document snapshot
    /// and return the file path + current version.
    OpenDocumentFullSnapshot,
    /// Ask the host to create a shared-memory mutation queue and return the
    /// file path.
    OpenMutationQueue,
    /// Open a plugin-declared panel.
    OpenPanel {
        def: PanelDef,
    },
    /// Close an open plugin panel.
    ClosePanel {
        handle: PanelHandle,
    },
    /// Move an open plugin panel to logical window coordinates.
    MovePanel {
        handle: PanelHandle,
        x: f32,
        y: f32,
    },
    /// Resize an open plugin panel.
    ResizePanel {
        handle: PanelHandle,
        width: f32,
        height: f32,
    },
    /// Dock an open plugin panel to a dock zone.
    DockPanel {
        handle: PanelHandle,
        zone: DockZone,
    },
    /// Undock an open plugin panel and place it at logical window coordinates.
    UndockPanel {
        handle: PanelHandle,
        x: f32,
        y: f32,
    },
    /// Forward a user-generated panel event to the plugin.
    PostPanelEvent {
        handle: PanelHandle,
        event: PanelEvent,
    },
    /// Ask the host to start a point pick and deliver the result back to the
    /// plugin via `HostAsync::CoordinatesPicked`. The `panel_id` is echoed so
    /// the plugin knows which panel initiated the pick.
    RequestPointPick {
        panel_id: String,
    },
    /// Ask the host to switch the active document tab (API v3).
    SetActiveTab(usize),
    /// Remove a single entity by handle (API v3).
    RemoveEntity {
        handle: Handle,
    },
    /// Remove multiple entities by handle (API v3).
    RemoveEntities {
        handles: Vec<Handle>,
    },
    /// Apply a batch of entity operations. Used as an IPC fallback when the
    /// shared-memory queue is not available (e.g. in-process plugins).
    EntityBatch {
        ops: Vec<EntityOp>,
    },
}

/// Responses the host sends back for `PluginRequest`.
#[derive(Debug, Serialize, Deserialize)]
pub enum PluginResponse {
    Ok,
    Bool(bool),
    Handle(Handle),
    Count(usize),
    Entity(EntityType),
    Record(Option<ExtendedDataRecord>),
    Document(CadDocument),
    Error(String),
    /// Path to the memory-mapped file and the current snapshot version.
    DocumentView {
        path: String,
        version: u64,
    },
    /// Path and version of the full document snapshot.
    DocumentFullSnapshot {
        path: String,
        version: u64,
    },
    /// Path of the mutation queue.
    MutationQueue {
        path: String,
    },
    /// Result of opening a panel.
    PanelHandleResult(Result<PanelHandle, PanelError>),
    /// Result of a panel operation that returns nothing.
    PanelResult(Result<(), PanelError>),
    /// Result of applying a batch of entity operations (API v3 fallback).
    BatchResult {
        applied: usize,
        failed: usize,
    },
}

/// Messages sent from the host to the plugin runner.
#[derive(Debug, Serialize, Deserialize)]
pub enum HostToPlugin {
    Request(HostRequest),
    Response(PluginResponse),
    Async(HostAsync),
}

/// Messages sent from the plugin runner to the host.
#[derive(Debug, Serialize, Deserialize)]
pub enum PluginToHost {
    Request(PluginRequest),
    Response(HostResponse),
    Async(PluginAsync),
}

/// Convenience helper for manifest serialization.
impl From<&'static crate::manifest::PluginManifest> for OwnedPluginManifest {
    fn from(m: &'static crate::manifest::PluginManifest) -> Self {
        Self {
            id: m.id.to_string(),
            name: m.name.to_string(),
            version: m.version.to_string(),
            description: m.description.to_string(),
            api_version: m.api_version.major,
            ribbon_order: m.ribbon_order,
            xdata_apps: m.xdata_apps.iter().map(|s| s.to_string()).collect(),
            command_prefixes: m.command_prefixes.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl OwnedPluginManifest {
    pub fn api_version(&self) -> ApiVersion {
        ApiVersion {
            major: self.api_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::DockStyle;

    #[test]
    fn dock_panel_request_round_trips() {
        let req = PluginRequest::DockPanel {
            handle: PanelHandle(7),
            zone: DockZone::Left,
        };
        let bytes = bincode::serialize(&req).expect("serialize DockPanel");
        let decoded: PluginRequest = bincode::deserialize(&bytes).expect("deserialize DockPanel");
        match decoded {
            PluginRequest::DockPanel { handle, zone } => {
                assert_eq!(handle, PanelHandle(7));
                assert_eq!(zone, DockZone::Left);
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn undock_panel_request_round_trips() {
        let req = PluginRequest::UndockPanel {
            handle: PanelHandle(9),
            x: 120.0,
            y: 80.0,
        };
        let bytes = bincode::serialize(&req).expect("serialize UndockPanel");
        let decoded: PluginRequest = bincode::deserialize(&bytes).expect("deserialize UndockPanel");
        match decoded {
            PluginRequest::UndockPanel { handle, x, y } => {
                assert_eq!(handle, PanelHandle(9));
                assert_eq!(x, 120.0);
                assert_eq!(y, 80.0);
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn set_active_tab_request_round_trips() {
        let req = PluginRequest::SetActiveTab(7);
        let bytes = bincode::serialize(&req).expect("serialize SetActiveTab");
        let decoded: PluginRequest = bincode::deserialize(&bytes).expect("deserialize SetActiveTab");
        match decoded {
            PluginRequest::SetActiveTab(tab) => assert_eq!(tab, 7),
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn remove_entity_request_round_trips() {
        let req = PluginRequest::RemoveEntity {
            handle: Handle::new(42),
        };
        let bytes = bincode::serialize(&req).expect("serialize RemoveEntity");
        let decoded: PluginRequest = bincode::deserialize(&bytes).expect("deserialize RemoveEntity");
        match decoded {
            PluginRequest::RemoveEntity { handle } => assert_eq!(handle.value(), 42),
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn remove_entities_request_round_trips() {
        let req = PluginRequest::RemoveEntities {
            handles: vec![Handle::new(1), Handle::new(2)],
        };
        let bytes = bincode::serialize(&req).expect("serialize RemoveEntities");
        let decoded: PluginRequest =
            bincode::deserialize(&bytes).expect("deserialize RemoveEntities");
        match decoded {
            PluginRequest::RemoveEntities { handles } => {
                assert_eq!(handles.len(), 2);
                assert_eq!(handles[0].value(), 1);
                assert_eq!(handles[1].value(), 2);
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn panel_def_policy_round_trips() {
        let def = PanelDef {
            id: "policy_test".to_string(),
            title: "Policy Test".to_string(),
            icon: None,
            dock: DockZone::Floating,
            initial_x: Some(10.0),
            initial_y: Some(20.0),
            initial_width: 300.0,
            initial_height: 500.0,
            min_width: 200.0,
            min_height: 150.0,
            dockable_zones: vec![DockZone::Left, DockZone::Right],
            allow_undock: false,
            resizable: false,
            draggable: false,
            dock_style: DockStyle::Stack,
        };
        let bytes = bincode::serialize(&def).expect("serialize PanelDef");
        let decoded: PanelDef = bincode::deserialize(&bytes).expect("deserialize PanelDef");
        assert_eq!(decoded.dockable_zones, def.dockable_zones);
        assert_eq!(decoded.allow_undock, def.allow_undock);
        assert_eq!(decoded.resizable, def.resizable);
        assert_eq!(decoded.draggable, def.draggable);
        assert_eq!(decoded.dock_style, def.dock_style);
    }
}
