//! Malfunctioning API v2 fixture plugin that hangs forever on `V2TEST_HANG`.
//!
//! Used to verify that the host times out and marks the process dead instead of
//! blocking indefinitely.

use ocs_plugin_api::host::{BuiltinPlugin, CadModuleV2, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

struct TestV2HangPlugin;

impl TestV2HangPlugin {
    fn new() -> Self {
        Self
    }
}

static MANIFEST: PluginManifest = PluginManifest {
    id: "ocs.test.v2_hang_plugin",
    name: "Test V2 Hang Plugin",
    version: "0.1.0",
    description: "Malfunctioning API v2 plugin that hangs on dispatch.",
    api_version: ApiVersion { major: 2 },
    ribbon_order: 100,
    xdata_apps: &[],
    command_prefixes: &["V2TEST"],
};

struct TestModule;

impl CadModuleV2 for TestModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        MANIFEST.name
    }

    fn ribbon_groups(&self) -> Vec<RibbonGroup> {
        vec![RibbonGroup {
            title: "V2 Hang",
            tools: vec![RibbonItem::Tool(ToolDef {
                id: "V2TEST_HANG",
                label: "Hang",
                icon: IconKind::Glyph("H"),
                event: ModuleEvent::Command("V2TEST_HANG".to_string()),
            })],
        }]
    }
}

impl BuiltinPlugin for TestV2HangPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        unsafe { std::mem::transmute(Box::new(TestModule) as Box<dyn CadModuleV2>) }
    }

    fn dispatch(&self, _host: &mut dyn HostApi, cmd: &str) -> bool {
        if cmd == "V2TEST_HANG" {
            // Malfunction: never return.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        false
    }
}

#[no_mangle]
pub extern "C" fn ocs_plugin_api_version() -> u32 {
    2
}

#[no_mangle]
pub extern "C" fn ocs_plugin_register() -> *mut Box<dyn BuiltinPlugin> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let plugin: Box<dyn BuiltinPlugin> = Box::new(TestV2HangPlugin::new());
        Box::into_raw(Box::new(plugin))
    })) {
        Ok(ptr) => ptr,
        Err(_) => std::ptr::null_mut(),
    }
}
