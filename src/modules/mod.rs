// Module system — CadModule, ToolDef, RibbonGroup.
//
// To add a **core** ribbon tab (Home, View, …):
//   1. Create `src/modules/my_name/` directory (no `plugin.toml`)
//   2. Add `src/modules/my_name/mod.rs` implementing `CadModule` as `MyNameModule`
//   3. Add `pub mod my_name;` below
//   4. `cargo build` — tab appears via build.rs registry
//
// To add an **add-on plugin** (Storm Sewer, …):
//   See `docs/plugin-architecture.md` and copy `docs/plugin-template/`.
//
// Each module folder contains:
//   - mod.rs       : module definition (ribbon groups + tool layout)
//   - <tool>.rs    : one file per tool (ribbon def + future command logic)

// ── Events ────────────────────────────────────────────────────────────────

/// Events a module tool can emit to the host application.
#[derive(Debug, Clone)]
pub enum ModuleEvent {
    /// Fire a named CAD command (e.g. "LINE", "CIRCLE").
    Command(String),
    /// Open the OS file dialog.
    OpenFileDialog,
    /// Remove all loaded models from the scene.
    #[allow(dead_code)]
    ClearModels,
    /// Toggle wireframe rendering.
    SetWireframe(bool),
    /// Toggle the layer manager panel.
    ToggleLayers,
}

// ── Data types ────────────────────────────────────────────────────────────

/// Icon source for a ribbon tool button.
#[derive(Clone, Copy)]
pub enum IconKind {
    /// Unicode glyph rendered as text (fast, no file needed).
    Glyph(&'static str),
    /// Raw SVG bytes embedded at compile time via `include_bytes!`.
    Svg(&'static [u8]),
}

/// A single tool button shown in the ribbon.
#[derive(Clone)]
pub struct ToolDef {
    /// Unique command id, e.g. "LINE".
    pub id: &'static str,
    /// Short label shown under the icon.
    pub label: &'static str,
    /// Icon — either a unicode glyph or embedded SVG bytes.
    pub icon: IconKind,
    /// Event emitted when the tool is clicked.
    pub event: ModuleEvent,
}

/// One item in a ribbon group — plain button or dropdown, in small (1-row) or large (3-row) size.
#[derive(Clone)]
pub enum RibbonItem {
    /// 1-row button — icon only, no label.
    Tool(ToolDef),
    /// 3-row button — icon + label below; full ribbon height.
    LargeTool(ToolDef),
    /// 1-row dropdown — icon + ▾ on right, no label.
    Dropdown {
        id: &'static str,
        icon: IconKind,
        items: Vec<(&'static str, &'static str, IconKind)>,
        default: &'static str,
    },
    /// 3-row dropdown — icon + label + ▾ below label; full ribbon height.
    LargeDropdown {
        id: &'static str,
        label: &'static str,
        icon: IconKind,
        items: Vec<(&'static str, &'static str, IconKind)>,
        default: &'static str,
    },
    /// Layer combo + two rows of small tools below.
    /// row2: operates on the layer of a selected object (off/freeze/lock/make-current)
    /// row3: all-layers operations + match (on/thaw/unlock/match)
    LayerComboGroup {
        row2: Vec<ToolDef>,
        row3: Vec<ToolDef>,
    },
    /// Match Properties (large button) + Color / Linetype / Lineweight combos on the right.
    PropertiesGroup { match_prop: ToolDef },
    /// A style selector combobox (text / dim / mleader / table style) with
    /// optional small tool rows below it.
    StyleComboGroup {
        /// Which style domain this combo controls.
        style_key: StyleKey,
        /// Unique dropdown id (must be unique across the ribbon).
        combo_id: &'static str,
        /// Optional command to run when the user opens the style manager.
        manager_cmd: Option<&'static str>,
        /// Small tool rows rendered below the combo (0–2 rows).
        rows: Vec<Vec<ToolDef>>,
    },
}

/// Identifies which style list a `StyleComboGroup` refers to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StyleKey {
    TextStyle,
    DimStyle,
    MLeaderStyle,
    TableStyle,
}

impl From<ToolDef> for RibbonItem {
    fn from(t: ToolDef) -> Self {
        RibbonItem::Tool(t)
    }
}

/// A named group of tool buttons shown together in the ribbon.
pub struct RibbonGroup {
    pub title: &'static str,
    pub tools: Vec<RibbonItem>,
}

// ── Trait ─────────────────────────────────────────────────────────────────

/// A CAD module owns a set of ribbon groups shown when its tab is active.
/// Each module is a stateless unit struct — all UI state lives in Ribbon.
pub trait CadModule: Send + Sync {
    #[allow(dead_code)]
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn ribbon_groups(&self) -> Vec<RibbonGroup>;
}

// ── Module declarations ───────────────────────────────────────────────────

pub mod annotate;
pub mod home;
pub mod insert;
pub mod model;
pub mod layout;
pub mod manage;
pub mod view;

// ── Auto-generated module registry ───────────────────────────────────────
// build.rs writes this file; it contains only `all_modules()`.
pub mod registry;
