//! Minimal API v2-compatible fixture plugin for runner validation.
//!
//! This plugin deliberately reports API major version 2 via the
//! `ocs_plugin_api_version` C symbol while implementing the current
//! `BuiltinPlugin` trait (the first three methods are the API v2 surface). It also
//! emulates the original v2 `CadModule` ABI where `ribbon_groups` returned
//! `Vec<RibbonGroup>` by value, so the runner's v2 compatibility path is tested.

use ocs_plugin_api::host::{BuiltinPlugin, CadModuleV2, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

struct TestV2Plugin;

impl TestV2Plugin {
    fn new() -> Self {
        Self
    }
}

static MANIFEST: PluginManifest = PluginManifest {
    id: "ocs.test.v2_plugin",
    name: "Test V2 Plugin",
    version: "0.1.0",
    description: "Minimal API v2-compatible fixture plugin for runner validation.",
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
            title: "V2 Test",
            tools: vec![RibbonItem::Tool(ToolDef {
                id: "V2TEST_HELLO",
                label: "Hello",
                icon: IconKind::Glyph("H"),
                event: ModuleEvent::Command("V2TEST_HELLO".to_string()),
            })],
        }]
    }
}

impl BuiltinPlugin for TestV2Plugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        // V2 cdylibs compiled against the original ABI returned `Box<dyn CadModule>`
        // whose `ribbon_groups` used the `Vec<RibbonGroup>` return convention. This
        // transmute reproduces that trait-object layout so the runner tests the
        // same path used by real existing V2 plugins.
        unsafe { std::mem::transmute(Box::new(TestModule) as Box<dyn CadModuleV2>) }
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        if cmd == "V2TEST_HELLO" {
            host.push_info("hello from v2 plugin");
            return true;
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
        let plugin: Box<dyn BuiltinPlugin> = Box::new(TestV2Plugin::new());
        Box::into_raw(Box::new(plugin))
    })) {
        Ok(ptr) => ptr,
        Err(_) => std::ptr::null_mut(),
    }
}
