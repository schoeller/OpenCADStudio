//! Ribbon tab for the Python Shell plugin.

use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

pub struct PythonShellModule;

impl CadModule for PythonShellModule {
    fn id(&self) -> &'static str {
        "opencad_pythonshell"
    }

    fn title(&self) -> &'static str {
        "Python Shell"
    }

    fn ribbon_groups(&self) -> Vec<RibbonGroup> {
        vec![RibbonGroup {
            title: "Scripting",
            tools: vec![RibbonItem::LargeTool(ToolDef {
                id: "PYSHELL",
                label: "Python Shell",
                icon: IconKind::Glyph("Py"),
                event: ModuleEvent::Command("PYSHELL".to_string()),
            })],
        }]
    }
}
