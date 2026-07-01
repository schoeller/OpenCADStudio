//! API v3 fixture plugin that exercises asynchronous host↔plugin events.
//!
//! - `on_async_event` records the last received `HostAsync` in a static mutex
//!   so a later sync dispatch can verify delivery.
//! - The `SEND_ASYNC` dispatch command emits a `PluginAsync::PanelUpdate` to
//!   test that plugin-to-host async messages are enqueued while a sync RPC is
//!   in flight.

use std::sync::Mutex;

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync};
use ocs_plugin_api::panel::Widget;
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

struct AsyncV3Plugin;

impl AsyncV3Plugin {
    fn new() -> Self {
        Self
    }
}

static LAST_EVENT: Mutex<String> = Mutex::new(String::new());

static MANIFEST: ocs_plugin_api::manifest::PluginManifest = ocs_plugin_api::manifest::PluginManifest {
    id: "ocs.test.v3_async_plugin",
    name: "Test V3 Async Plugin",
    version: "0.1.0",
    description: "Fixture plugin for async event round-trips.",
    api_version: ocs_plugin_api::manifest::ApiVersion { major: 3 },
    ribbon_order: 100,
    xdata_apps: &[],
    command_prefixes: &[],
};

struct AsyncModule;

impl CadModule for AsyncModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        MANIFEST.name
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        Box::leak(Box::new(vec![RibbonGroup {
            title: "Async V3",
            tools: vec![RibbonItem::Tool(ToolDef {
                id: "ASYNC_STATUS",
                label: "Status",
                icon: IconKind::Glyph("A"),
                event: ModuleEvent::Command("ASYNC_STATUS".to_string()),
            })],
        }])).as_slice()
    }
}

impl BuiltinPlugin for AsyncV3Plugin {
    fn manifest(&self) -> &'static ocs_plugin_api::manifest::PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(AsyncModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        match cmd {
            "ASYNC_STATUS" => {
                let event = LAST_EVENT.lock().unwrap_or_else(|e| e.into_inner()).clone();
                host.push_info(&event);
                true
            }
            "SEND_ASYNC" => {
                host.send_async(PluginAsync::PanelUpdate {
                    panel_id: "test.panel".to_string(),
                    widgets: vec![Widget::Label("async hello".to_string())],
                });
                true
            }
            _ => false,
        }
    }

    fn on_async_event(&mut self, host: &mut dyn HostApi, event: HostAsync) {
        let mut guard = LAST_EVENT.lock().unwrap_or_else(|e| e.into_inner());
        *guard = match event {
            HostAsync::DocumentActivated { tab } => format!("DocumentActivated:{tab}"),
            HostAsync::DocumentChanged { tab, version } => {
                format!("DocumentChanged:{tab}:{version}")
            }
            HostAsync::TabClosed { tab } => format!("TabClosed:{tab}"),
            HostAsync::PanelEvent { panel_id, event } => {
                format!("PanelEvent:{panel_id}:{event:?}")
            }
            HostAsync::CoordinatesPicked { panel_id, point } => {
                format!("CoordinatesPicked:{panel_id}:{:?}", point)
            }
        };
        // Echo back an async event so tests can observe delivery without a ping.
        host.send_async(PluginAsync::PanelUpdate {
            panel_id: "test.panel".to_string(),
            widgets: vec![Widget::Label("async delivered".to_string())],
        });
    }
}

ocs_plugin_api::export_plugin!(AsyncV3Plugin::new());
