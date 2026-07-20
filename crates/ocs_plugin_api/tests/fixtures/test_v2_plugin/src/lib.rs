//! Minimal API v2-compatible fixture plugin for runner validation.
//!
//! This plugin deliberately reports API major version 2 via the
//! `ocs_plugin_api_version` C symbol while implementing the current
//! `BuiltinPlugin` trait (the first three methods are the API v2 surface). That
//! lets Stage 1 tests verify that the host still loads v2 plugins without
//! requiring them to be rebuilt for the V2-specific trait.

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
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

impl CadModule for TestModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        MANIFEST.name
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        Box::leak(Box::new(vec![RibbonGroup {
            title: "V2 Test",
            tools: vec![RibbonItem::Tool(ToolDef {
                id: "V2TEST_HELLO",
                label: "Hello",
                icon: IconKind::Glyph("H"),
                event: ModuleEvent::Command("V2TEST_HELLO".to_string()),
            })],
        }])).as_slice()
    }
}

impl BuiltinPlugin for TestV2Plugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(TestModule)
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
