//! Open CAD Studio add-on template.
//!
//! Rename the crate (`Cargo.toml`), the ids/strings below, and `plugin.toml` to
//! match. Build with `cargo build --release` and ship the resulting cdylib plus
//! `plugin.toml` as GitHub Release assets (see `.github/workflows/release.yml`).

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::panel::{DockStyle, DockZone, PanelDef, PanelEvent, Widget};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

// Keep these fields in sync with `plugin.toml`.
static MANIFEST: PluginManifest = PluginManifest {
    id: "opencad.my_plugin",
    name: "My Plugin",
    version: "0.1.0",
    description: "What this plugin does.",
    api_version: ApiVersion::CURRENT,
    ribbon_order: 50,
    xdata_apps: &[],
    command_prefixes: &["MP_"],
};

/// The ribbon tab.
struct MyModule;

impl CadModule for MyModule {
    fn id(&self) -> &'static str {
        "my_plugin"
    }
    fn title(&self) -> &'static str {
        "My Plugin"
    }
    fn ribbon_groups(&self) -> Vec<RibbonGroup> {
        vec![RibbonGroup {
            title: "Tools",
            tools: vec![RibbonItem::LargeTool(ToolDef {
                id: "MP_HELLO",
                label: "Hello",
                icon: IconKind::Glyph("★"),
                event: ModuleEvent::Command("MP_HELLO".to_string()),
            })],
        }]
    }
}

/// The plugin entry point.
struct MyPlugin;

fn panel_def() -> PanelDef {
    PanelDef {
        id: "my_plugin.panel".to_string(),
        title: "My Plugin Panel".to_string(),
        icon: None,
        dock: DockZone::Floating,
        initial_x: Some(100.0),
        initial_y: Some(100.0),
        initial_width: 260.0,
        initial_height: 400.0,
        min_width: 160.0,
        min_height: 120.0,
        dockable_zones: vec![DockZone::Floating, DockZone::Left, DockZone::Right],
        allow_undock: true,
        resizable: true,
        draggable: true,
        dock_style: DockStyle::Tabs,
    }
}

impl BuiltinPlugin for MyPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(MyModule)
    }
    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        match cmd {
            "MP_HELLO" => {
                host.push_info("Hello from My Plugin");
                true
            }
            // Open the optional panel when the ribbon tool is clicked.
            "MP_OPEN_PANEL" => {
                let _ = host.open_panel(&panel_def());
                true
            }
            _ => false,
        }
    }

    /// Optional API v3 panel declaration. Remove if your add-on has no panels.
    fn panels(&self) -> Vec<PanelDef> {
        vec![panel_def()]
    }

    /// Handle host async events (panel interactions, document lifecycle).
    /// Remove if your add-on has no panels.
    fn on_async_event(&mut self, host: &mut dyn HostApi, event: ocs_plugin_api::ipc::protocol::HostAsync) {
        use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync};
        match event {
            HostAsync::PanelEvent { panel_id, event } => {
                if panel_id == "my_plugin.panel" {
                    match event {
                        PanelEvent::Clicked(id) if id == "run" => {
                            host.push_info("Run clicked");
                            let _ = host.send_async(PluginAsync::PanelUpdate {
                                panel_id: "my_plugin.panel".to_string(),
                                widgets: vec![Widget::Label("Hello from the panel".to_string())],
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

// Emit the C-ABI symbols the host loader looks for.
ocs_plugin_api::export_plugin!(MyPlugin);
