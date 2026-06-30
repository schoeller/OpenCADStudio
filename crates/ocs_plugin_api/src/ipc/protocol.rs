//! Request/response envelopes exchanged between the host and a plugin process.
//!
//! A single bidirectional socket is used. Each side sends either a request
//! (expecting a response) or a response (to a previous request). This lets the
//! host handle plugin RPCs inline while it waits for the result of a host→plugin
//! request such as `Dispatch`, avoiding the need for two sockets or threads.

use serde::{Deserialize, Serialize};

use crate::host::CommandStep;
use crate::manifest::ApiVersion;
use crate::ribbon::owned::{OwnedPluginManifest, OwnedRibbonGroup};

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
    /// Ask the plugin runner to close/end an async session. Sent by the host
    /// when the host-side async adapter is dropped or the pinned tab is closed.
    EndAsyncSession {
        session_id: String,
    },
    /// Forward a plugin request from the host-side async session handle to the
    /// plugin runner. This is an implementation detail of V3 async sessions.
    AsyncSessionRequest {
        request: PluginRequest,
    },
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
    Error(String),
    /// Response to a forwarded async-session plugin request.
    AsyncSessionResponse(PluginResponse),
}

/// Requests the plugin runner sends to the host.
#[derive(Debug, Serialize, Deserialize)]
pub enum PluginRequest {
    PushInfo(String),
    PushOutput(String),
    PushError(String),
    AddEntity(EntityType),
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
    /// Ask the host to create an async session for out-of-process plugins that
    /// need to keep talking to the host after `dispatch` returns.
    StartAsyncSession {
        session_id: String,
    },
    /// Tell the host an async session is ending (e.g. the plugin window closed).
    EndAsyncSession {
        session_id: String,
    },
}

/// Responses the host sends back for `PluginRequest`.
#[derive(Debug, Serialize, Deserialize)]
pub enum PluginResponse {
    Ok,
    Bool(bool),
    Handle(Handle),
    Record(Option<ExtendedDataRecord>),
    Document(CadDocument),
    Error(String),
    /// Path to the memory-mapped file and the current snapshot version.
    DocumentView {
        path: String,
        version: u64,
    },
}

/// Messages sent from the host to the plugin runner.
#[derive(Debug, Serialize, Deserialize)]
pub enum HostToPlugin {
    Request(HostRequest),
    Response(PluginResponse),
    /// V3 request/response pair. The `request_id` lets multiple in-flight
    /// requests share the same bidirectional socket. `session_id` routes the
    /// request to the correct async session on the plugin side.
    RequestV3 {
        request_id: u64,
        session_id: String,
        request: HostRequest,
    },
    ResponseV3 {
        request_id: u64,
        response: PluginResponse,
    },
}

/// Messages sent from the plugin runner to the host.
#[derive(Debug, Serialize, Deserialize)]
pub enum PluginToHost {
    Request(PluginRequest),
    Response(HostResponse),
    /// V3 request/response pair. The `request_id` lets multiple in-flight
    /// requests share the same bidirectional socket. `session_id` routes the
    /// request to the correct async session on the host side.
    RequestV3 {
        request_id: u64,
        session_id: String,
        request: PluginRequest,
    },
    ResponseV3 {
        request_id: u64,
        response: HostResponse,
    },
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

    #[test]
    fn api_version_orders_by_major() {
        assert!(ApiVersion { major: 3 } >= ApiVersion { major: 2 });
        assert!(ApiVersion { major: 3 } >= ApiVersion { major: 3 });
        assert!(ApiVersion { major: 2 } < ApiVersion { major: 3 });
    }

    #[test]
    fn v3_envelopes_round_trip_through_bincode() {
        let host_to_plugin = HostToPlugin::RequestV3 {
            request_id: 7,
            session_id: "session-42".to_string(),
            request: HostRequest::EndAsyncSession {
                session_id: "session-42".to_string(),
            },
        };
        let encoded = bincode::serialize(&host_to_plugin).expect("serialize HostToPlugin");
        let decoded: HostToPlugin =
            bincode::deserialize(&encoded).expect("deserialize HostToPlugin");
        assert!(
            std::mem::discriminant(&decoded) == std::mem::discriminant(&host_to_plugin),
            "HostToPlugin V3 variant round-tripped"
        );

        let plugin_to_host = PluginToHost::RequestV3 {
            request_id: 9,
            session_id: "session-42".to_string(),
            request: PluginRequest::StartAsyncSession {
                session_id: "session-42".to_string(),
            },
        };
        let encoded = bincode::serialize(&plugin_to_host).expect("serialize PluginToHost");
        let decoded: PluginToHost =
            bincode::deserialize(&encoded).expect("deserialize PluginToHost");
        assert!(
            std::mem::discriminant(&decoded) == std::mem::discriminant(&plugin_to_host),
            "PluginToHost V3 variant round-tripped"
        );
    }
}
