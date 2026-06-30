//! V2 test plugin for OpenCADStudio plugin lifecycle tests.

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

static MANIFEST: PluginManifest = PluginManifest {
    id: "opencad.plugin_template_api2",
    name: "Plugin Template API V2",
    version: "0.1.0",
    description: "V2 plugin used in lifecycle tests.",
    api_version: ApiVersion { major: 2 },
    ribbon_order: 90,
    xdata_apps: &["PT2_RECORD"],
    command_prefixes: &["PT2_"],
};

struct Api2Module;

impl CadModule for Api2Module {
    fn id(&self) -> &'static str {
        "plugin_template_api2"
    }
    fn title(&self) -> &'static str {
        "API V2 Test"
    }
    fn ribbon_groups(&self) -> Vec<RibbonGroup> {
        vec![RibbonGroup {
            title: "Tools",
            tools: vec![RibbonItem::LargeTool(ToolDef {
                id: "PT2_HELLO",
                label: "Hello",
                icon: IconKind::Glyph("2"),
                event: ModuleEvent::Command("PT2_HELLO".to_string()),
            })],
        }]
    }
}

struct Api2Plugin;

impl BuiltinPlugin for Api2Plugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(Api2Module)
    }
    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        match cmd {
            "PT2_HELLO" => {
                host.push_info("hello from api2 plugin");
                true
            }
            _ => false,
        }
    }
}

ocs_plugin_api::export_plugin!(Api2Plugin);
