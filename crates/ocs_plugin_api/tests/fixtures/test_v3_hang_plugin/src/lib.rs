//! Malfunctioning API v3 fixture plugin that hangs forever on `HANG`.
//!
//! Used to verify that the host times out and marks the process dead instead of
//! blocking indefinitely.

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

struct TestV3HangPlugin;

impl TestV3HangPlugin {
    fn new() -> Self {
        Self
    }
}

static MANIFEST: ocs_plugin_api::manifest::PluginManifest = ocs_plugin_api::manifest::PluginManifest {
    id: "ocs.test.v3_hang_plugin",
    name: "Test V3 Hang Plugin",
    version: "0.1.0",
    description: "Malfunctioning API v3 plugin that hangs on dispatch.",
    api_version: ocs_plugin_api::manifest::ApiVersion { major: 3 },
    ribbon_order: 100,
    xdata_apps: &[],
    command_prefixes: &[],
};

struct TestModule;

impl CadModule for TestModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        MANIFEST.name
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        Box::leak(Box::new(vec![RibbonGroup {
            title: "V3 Hang",
            tools: vec![RibbonItem::Tool(ToolDef {
                id: "HANG",
                label: "Hang",
                icon: IconKind::Glyph("H"),
                event: ModuleEvent::Command("HANG".to_string()),
            })],
        }])).as_slice()
    }
}

impl BuiltinPlugin for TestV3HangPlugin {
    fn manifest(&self) -> &'static ocs_plugin_api::manifest::PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(TestModule)
    }

    fn dispatch(&self, _host: &mut dyn HostApi, cmd: &str) -> bool {
        if cmd == "HANG" {
            // Malfunction: never return.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        false
    }
}

ocs_plugin_api::export_plugin!(TestV3HangPlugin::new());
