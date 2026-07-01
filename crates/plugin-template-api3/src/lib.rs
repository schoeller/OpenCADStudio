//! API v3 panel + async communication template plugin.
//!
//! This crate is a reference implementation of a modern API v3 plugin. It
//! demonstrates panels, async host→plugin events, async plugin→host events,
//! ribbon contribution, dirty/undo, and command-line output — all without
//! relying on any API v2 compatibility paths.

use std::collections::VecDeque;
use std::sync::Mutex;

use ocs_plugin_api::export_plugin;
use acadrust::EntityType;
use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::ipc::protocol::{HostAsync, PluginAsync};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::panel::{DockStyle, DockZone, PanelDef, PanelEvent, PanelHandle, Widget};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

pub static MANIFEST: PluginManifest = PluginManifest {
    id: "ocs.template.api3",
    name: "API v3 Panel Template",
    version: "0.1.0",
    description: "Reference plugin showing API v3 panels and async IPC.",
    api_version: ApiVersion { major: 3 },
    ribbon_order: 200,
    xdata_apps: &["API3TMPL"],
    command_prefixes: &["API3_"],
};

pub struct Api3TemplatePlugin {
    click_count: u64,
    last_input: String,
    selected_item: Option<usize>,
    log_lines: VecDeque<String>,
    panel_handle: Mutex<Option<PanelHandle>>,
    pending_pick: PickMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickMode {
    None,
    Log,
    AddPoint,
}

impl Default for Api3TemplatePlugin {
    fn default() -> Self {
        Self::new()
    }
}

fn log(_msg: &str) {
    // Disabled by default to avoid synchronous file I/O on the async event
    // path. Uncomment the body when debugging the plugin locally.
    /*
    use std::io::Write;
    let path = std::path::PathBuf::from("C:\\tmp\\plugin-template-api3.log");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    let _ = writeln!(file, "{} - {msg}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
    */
}

impl Api3TemplatePlugin {
    pub fn new() -> Self {
        let mut log_lines = VecDeque::new();
        log_lines.push_back("Plugin loaded".to_string());
        Self {
            click_count: 0,
            last_input: String::new(),
            selected_item: None,
            log_lines,
            panel_handle: Mutex::new(None),
            pending_pick: PickMode::None,
        }
    }

    fn panel_def() -> PanelDef {
        PanelDef {
            id: "api3_panel".to_string(),
            title: "API v3 Template".to_string(),
            icon: None,
            dock: DockZone::Floating,
            initial_x: Some(120.0),
            initial_y: Some(80.0),
            initial_width: 280.0,
            initial_height: 420.0,
            min_width: 160.0,
            min_height: 120.0,
            dockable_zones: vec![DockZone::Floating, DockZone::Left, DockZone::Right],
            allow_undock: true,
            resizable: true,
            draggable: true,
            dock_style: DockStyle::Tabs,
        }
    }

    fn render(&self) -> Vec<Widget> {
        vec![
            Widget::Label("API v3 template panel".to_string()),
            Widget::Label("Use tab switch only when loaded out-of-process".to_string()),
            Widget::Button {
                id: "inc".to_string(),
                label: format!("Clicked {} times", self.click_count),
            },
            Widget::Button {
                id: "pick_point".to_string(),
                label: "Pick point".to_string(),
            },
            Widget::Button {
                id: "add_point".to_string(),
                label: "Add point".to_string(),
            },
            Widget::Button {
                id: "remove_last".to_string(),
                label: "Remove last".to_string(),
            },
            Widget::Button {
                id: "switch_tab".to_string(),
                label: "Switch to tab 0".to_string(),
            },
            Widget::Button {
                id: "dock_left".to_string(),
                label: "Dock left".to_string(),
            },
            Widget::Button {
                id: "dock_right".to_string(),
                label: "Dock right".to_string(),
            },
            Widget::Button {
                id: "undock".to_string(),
                label: "Undock".to_string(),
            },
            Widget::TextInput {
                id: "input".to_string(),
                // Keep the text input uncontrolled on the plugin side so rapid
                // keystrokes are not overwritten by stale PanelUpdate messages.
                value: String::new(),
            },
            Widget::Button {
                id: "send_cmd".to_string(),
                label: "Send to CMD".to_string(),
            },
            Widget::List {
                id: "list".to_string(),
                items: vec![
                    "Item 0".to_string(),
                    "Item 1".to_string(),
                    "Item 2".to_string(),
                ]
                .into_iter()
                .enumerate()
                .map(|(i, label)| {
                    if self.selected_item == Some(i) {
                        format!("> {label}")
                    } else {
                        label
                    }
                })
                .collect(),
            },
            Widget::MultilineOutput {
                id: "log".to_string(),
                lines: self.log_lines.iter().cloned().collect(),
            },
        ]
    }

    fn push_log(&mut self, host: &mut dyn HostApi, msg: String) {
        self.log_lines.push_back(msg);
        if self.log_lines.len() > 50 {
            self.log_lines.pop_front();
        }
        self.refresh_panel(host);
    }

    fn refresh_panel(&self, host: &mut dyn HostApi) {
        host.send_async(PluginAsync::PanelUpdate {
            panel_id: "api3_panel".to_string(),
            widgets: self.render(),
        });
    }
}

struct TemplateModule;

impl CadModule for TemplateModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        MANIFEST.name
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        Box::leak(Box::new(vec![RibbonGroup {
            title: "Template",
            tools: vec![RibbonItem::LargeTool(ToolDef {
                id: "API3_OPEN",
                label: "Open\nTemplate",
                icon: IconKind::Glyph("◆"),
                event: ModuleEvent::Command("API3_OPEN".to_string()),
            })],
        }])).as_slice()
    }
}

impl BuiltinPlugin for Api3TemplatePlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(TemplateModule)
    }

    fn panels(&self) -> Vec<PanelDef> {
        vec![Self::panel_def()]
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        log(&format!("dispatch: {cmd}"));
        let result = match cmd {
            "API3_ADD_POINT" => {
                use acadrust::entities::Point;
                use acadrust::types::Vector3;
                let mut p = Point::at(Vector3::new(0.0, 0.0, 0.0));
                p.common.layer = "0".to_string();
                let handle = host.add_entity(EntityType::Point(p));
                host.push_info(&format!("Added point {handle:?}"));
                true
            }
            "API3_REMOVE_LAST" => {
                let last_handle = {
                    let doc = host.document();
                    doc.entities().last().map(|e| e.common().handle)
                };
                if let Some(handle) = last_handle {
                    match host.remove_entity(handle) {
                        Some(_) => host.push_info(&format!("Removed {handle:?}")),
                        None => host.push_error(&format!("Could not remove {handle:?}")),
                    }
                } else {
                    host.push_error("No entity to remove");
                }
                true
            }
            "API3_SWITCH_TAB" => {
                match host.set_active_tab(0) {
                    Ok(()) => host.push_info("Switched to tab 0"),
                    Err(e) => host.push_error(&format!("Switch tab failed: {e}")),
                }
                true
            }
            "API3_OPEN" => {
                log("API3_OPEN: calling open_panel");
                let def = Self::panel_def();
                match host.open_panel(&def) {
                    Ok(handle) => {
                        log(&format!("API3_OPEN: open_panel returned {handle:?}"));
                        self.panel_handle.lock().unwrap().replace(handle);
                        log("API3_OPEN: calling set_dirty");
                        host.set_dirty();
                        log("API3_OPEN: set_dirty returned");
                        log("API3_OPEN: calling push_info");
                        host.push_info("API3 panel opened");
                        log("API3_OPEN: push_info returned");
                        log("API3_OPEN: calling send_async PanelUpdate");
                        host.send_async(PluginAsync::PanelUpdate {
                            panel_id: "api3_panel".to_string(),
                            widgets: self.render(),
                        });
                        log("API3_OPEN: send_async returned");
                        true
                    }
                    Err(e) => {
                        log(&format!("API3_OPEN: open_panel error {e:?}"));
                        host.push_error(&format!("API3 open_panel failed: {e}"));
                        true
                    }
                }
            }
            _ => false,
        };
        log(&format!("dispatch: {cmd} returning {result}"));
        result
    }

    fn on_async_event(&mut self, host: &mut dyn HostApi, event: HostAsync) {
        log(&format!("on_async_event: {event:?}"));
        match event {
            HostAsync::DocumentActivated { tab } => {
                host.push_info(&format!("DocumentActivated tab={tab}"));
                self.push_log(host, format!("DocumentActivated tab={tab}"));
            }
            HostAsync::DocumentChanged { tab, version } => {
                self.push_log(host, format!("DocumentChanged tab={tab} version={version}"));
            }
            HostAsync::TabClosed { tab } => {
                self.push_log(host, format!("TabClosed tab={tab}"));
            }
            HostAsync::PanelEvent { panel_id, event } if panel_id == "api3_panel" => match event {
                PanelEvent::Clicked(id) if id == "inc" => {
                    self.click_count += 1;
                    self.push_log(host, format!("Button clicked: count={}", self.click_count));
                }
                PanelEvent::Clicked(id) if id == "pick_point" => {
                    self.pending_pick = PickMode::Log;
                    match host.request_point_pick("api3_panel") {
                        Ok(()) => self.push_log(host, "Point pick requested".to_string()),
                        Err(e) => self.push_log(host, format!("Point pick failed: {e}")),
                    }
                }
                PanelEvent::Clicked(id) if id == "add_point" => {
                    self.pending_pick = PickMode::AddPoint;
                    match host.request_point_pick("api3_panel") {
                        Ok(()) => self.push_log(host, "Click in the viewport to place the point".to_string()),
                        Err(e) => self.push_log(host, format!("Point pick failed: {e}")),
                    }
                }
                PanelEvent::Clicked(id) if id == "remove_last" => {
                    // Use the highest handle instead of the iterator's last item, because the
                    // document snapshot order does not guarantee that the most recently added
                    // entity is last.
                    let last_handle = {
                        let doc = host.document();
                        doc.entities().map(|e| e.common().handle).max()
                    };
                    if let Some(handle) = last_handle {
                        match host.remove_entity(handle) {
                            Some(_) => {
                                host.set_dirty();
                                self.push_log(host, format!("Removed {handle:?}"));
                            }
                            None => self.push_log(host, format!("Could not remove {handle:?}")),
                        }
                    } else {
                        self.push_log(host, "No entity to remove".to_string());
                    }
                }
                PanelEvent::Clicked(id) if id == "switch_tab" => {
                    match host.set_active_tab(0) {
                        Ok(()) => self.push_log(host, "Switched to tab 0".to_string()),
                        Err(e) => self.push_log(host, format!("Switch tab failed: {e}")),
                    }
                }
                PanelEvent::Clicked(id) if id == "dock_left" => {
                    let handle = *self.panel_handle.lock().unwrap();
                    if let Some(handle) = handle {
                        match host.dock_panel(handle, DockZone::Left) {
                            Ok(()) => self.push_log(host, "Dock left requested".to_string()),
                            Err(e) => self.push_log(host, format!("Dock left failed: {e}")),
                        }
                    }
                }
                PanelEvent::Clicked(id) if id == "dock_right" => {
                    let handle = *self.panel_handle.lock().unwrap();
                    if let Some(handle) = handle {
                        match host.dock_panel(handle, DockZone::Right) {
                            Ok(()) => self.push_log(host, "Dock right requested".to_string()),
                            Err(e) => self.push_log(host, format!("Dock right failed: {e}")),
                        }
                    }
                }
                PanelEvent::Clicked(id) if id == "undock" => {
                    let handle = *self.panel_handle.lock().unwrap();
                    if let Some(handle) = handle {
                        match host.undock_panel(handle, 120.0, 80.0) {
                            Ok(()) => self.push_log(host, "Undock requested".to_string()),
                            Err(e) => self.push_log(host, format!("Undock failed: {e}")),
                        }
                    }
                }
                PanelEvent::Clicked(id) if id == "send_cmd" => {
                    let value = self.last_input.clone();
                    if !value.is_empty() {
                        host.push_output(&value);
                        self.push_log(host, format!("Sent to CMD: {value}"));
                    }
                }
                PanelEvent::TextChanged { id, value } if id == "input" => {
                    self.push_log(host, format!("Input changed: {value}"));
                    self.last_input = value;
                }
                PanelEvent::ItemSelected { id, index } if id == "list" => {
                    self.selected_item = Some(index);
                    let msg = format!("List item selected: {index}");
                    host.push_output(&msg);
                    self.push_log(host, msg);
                }
                PanelEvent::Closed => {
                    self.panel_handle.lock().unwrap().take();
                    self.push_log(host, "API3 panel closed".to_string());
                }
                PanelEvent::Moved { x, y } => {
                    self.push_log(host, format!("Panel moved: x={x:.1}, y={y:.1}"));
                }
                PanelEvent::Resized { width, height } => {
                    self.push_log(host, format!("Panel resized: {width:.1} x {height:.1}"));
                }
                PanelEvent::Focused => {
                    self.push_log(host, "Panel focused".to_string());
                }
                PanelEvent::Docked { zone } => {
                    self.push_log(host, format!("Panel docked: {zone:?}"));
                }
                PanelEvent::Undocked => {
                    self.push_log(host, "Panel undocked".to_string());
                }
                _ => {}
            },
            HostAsync::CoordinatesPicked { panel_id, point } if panel_id == "api3_panel" => {
                match self.pending_pick {
                    PickMode::AddPoint => {
                        use acadrust::entities::Point;
                        use acadrust::types::Vector3;
                        let mut p = Point::at(Vector3::new(point[0], point[1], point[2]));
                        p.common.layer = "0".to_string();
                        let handle = host.add_entity(EntityType::Point(p));
                        host.set_dirty();
                        self.push_log(host, format!("Added point {handle:?} at {:.3}, {:.3}, {:.3}", point[0], point[1], point[2]));
                    }
                    _ => {
                        let msg = format!(
                            "Picked point: {:.3}, {:.3}, {:.3}",
                            point[0], point[1], point[2]
                        );
                        host.push_output(&msg);
                        self.push_log(host, msg);
                    }
                }
                self.pending_pick = PickMode::None;
            }
            _ => {}
        }
    }
}

export_plugin!(Api3TemplatePlugin::new());
