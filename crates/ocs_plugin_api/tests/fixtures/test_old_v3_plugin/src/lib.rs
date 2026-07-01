//! Fixture plugin that reports API v3 with an outdated ABI revision.
//!
//! This is used to verify that the runner rejects stale v3 cdylibs before it
//! constructs a plugin object with a potentially incompatible trait layout.

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

struct OldV3Plugin;

impl OldV3Plugin {
    fn new() -> Self {
        Self
    }
}

static MANIFEST: PluginManifest = PluginManifest {
    id: "ocs.test.old_v3_plugin",
    name: "Test Old V3 Plugin",
    version: "0.1.0",
    description: "Fixture plugin with a stale v3 ABI revision.",
    api_version: ApiVersion { major: 3 },
    ribbon_order: 100,
    xdata_apps: &[],
    command_prefixes: &[],
};

struct OldModule;

impl CadModule for OldModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        MANIFEST.name
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        Box::leak(Box::new(vec![RibbonGroup {
            title: "Old V3",
            tools: vec![RibbonItem::Tool(ToolDef {
                id: "OLD_V3_HELLO",
                label: "Hello",
                icon: IconKind::Glyph("O"),
                event: ModuleEvent::Command("OLD_V3_HELLO".to_string()),
            })],
        }])).as_slice()
    }
}

impl BuiltinPlugin for OldV3Plugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(OldModule)
    }

    fn dispatch(&self, _host: &mut dyn HostApi, _cmd: &str) -> bool {
        false
    }
}

#[no_mangle]
pub extern "C" fn ocs_plugin_api_version() -> u32 {
    3
}

#[no_mangle]
pub extern "C" fn ocs_plugin_abi_revision() -> u64 {
    // Deliberately stale revision; the current host expects ABI_REVISION.
    999
}

#[no_mangle]
pub extern "C" fn ocs_plugin_register() -> *mut Box<dyn BuiltinPlugin> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let plugin: Box<dyn BuiltinPlugin> = Box::new(OldV3Plugin::new());
        Box::into_raw(Box::new(plugin))
    })) {
        Ok(ptr) => ptr,
        Err(_) => std::ptr::null_mut(),
    }
}
