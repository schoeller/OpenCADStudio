//! V3 test plugin for OpenCADStudio plugin lifecycle tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

static MANIFEST: PluginManifest = PluginManifest {
    id: "opencad.plugin_template_api3",
    name: "Plugin Template API V3",
    version: "0.1.0",
    description: "V3 plugin used in lifecycle tests.",
    api_version: ApiVersion { major: 3 },
    ribbon_order: 91,
    xdata_apps: &["PT3_RECORD"],
    command_prefixes: &["PT3_"],
};

static SHOULD_SHUTDOWN: AtomicBool = AtomicBool::new(false);

struct Api3Module;

impl CadModule for Api3Module {
    fn id(&self) -> &'static str {
        "plugin_template_api3"
    }
    fn title(&self) -> &'static str {
        "API V3 Test"
    }
    fn ribbon_groups(&self) -> Vec<RibbonGroup> {
        vec![RibbonGroup {
            title: "Tools",
            tools: vec![
                RibbonItem::LargeTool(ToolDef {
                    id: "PT3_START_SESSION",
                    label: "Start Session",
                    icon: IconKind::Glyph("3"),
                    event: ModuleEvent::Command("PT3_START_SESSION".to_string()),
                }),
                RibbonItem::LargeTool(ToolDef {
                    id: "PT3_NO_SESSION",
                    label: "No Session",
                    icon: IconKind::Glyph("N"),
                    event: ModuleEvent::Command("PT3_NO_SESSION".to_string()),
                }),
            ],
        }]
    }
}

struct Api3Plugin;

impl BuiltinPlugin for Api3Plugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(Api3Module)
    }
    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        match cmd {
            "PT3_START_SESSION" => {
                host.push_info("api3 plugin starting async session");
                host.start_async_session("pt3-session").is_some()
            }
            "PT3_NO_SESSION" => {
                host.push_info("api3 plugin without session");
                true
            }
            _ => false,
        }
    }

    fn run_on_main_thread(&self) -> Result<(), Box<dyn std::error::Error>> {
        SHOULD_SHUTDOWN.store(false, Ordering::SeqCst);
        while !SHOULD_SHUTDOWN.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn on_host_request(
        &self,
        req: &ocs_plugin_api::ipc::protocol::HostRequest,
    ) -> Option<ocs_plugin_api::ipc::protocol::HostResponse> {
        match req {
            ocs_plugin_api::ipc::protocol::HostRequest::Shutdown
            | ocs_plugin_api::ipc::protocol::HostRequest::EndAsyncSession { .. } => {
                SHOULD_SHUTDOWN.store(true, Ordering::SeqCst);
                Some(ocs_plugin_api::ipc::protocol::HostResponse::Bool(true))
            }
            _ => None,
        }
    }
}

ocs_plugin_api::export_plugin!(Api3Plugin);
