use crate::command::CadCommand;
use crate::linetypes;
use crate::modules::home::modify::refedit::RefEditSession;
use crate::scene::pick::grip::GripEdit;
use crate::scene::GripDef;
use crate::scene::Scene;
use crate::snap::SnapResult;
use crate::ui::{LayerPanel, PropertiesPanel};
use acadrust::tables::Ucs;
use acadrust::{CadDocument, Handle};
use iced;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::PathBuf;

// ── Dynamic input ──────────────────────────────────────────────────────────

/// One quantity shown in the dynamic-input overlay near the cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DynComponent {
    /// Absolute X ordinate.
    X,
    /// Absolute Y ordinate.
    Y,
    /// Absolute Z ordinate (only visible after the user types a second
    /// `,` separator from a cartesian X/Y configuration).
    Z,
    /// Linear distance from the last point.
    Distance,
    /// Angle from the last point, in degrees.
    Angle,
    /// A scalar the command reads from the command line (a count, a radius,
    /// a delta). Typed-only — it has no geometric live value derived from
    /// the cursor unless the command supplies one via `dyn_live_value`.
    Scalar,
}

/// A single editable dynamic-input field. `buffer == None` means the box
/// tracks the cursor live; once the user types, the typed text is held in
/// `buffer` and the box stops following the cursor (it is "locked").
#[derive(Clone, Debug)]
pub(super) struct DynFieldEntry {
    pub(super) component: DynComponent,
    /// Semantic role — drives the label and value scaling (e.g. diameter).
    /// Defaults to the role matching `component` on the legacy path.
    pub(super) role: crate::command::DynRole,
    pub(super) buffer: Option<String>,
}

impl DynFieldEntry {
    pub(super) fn new(component: DynComponent) -> Self {
        Self {
            component,
            role: default_role_for(component),
            buffer: None,
        }
    }
    /// Build from an explicit role (spec-driven path); the resolution
    /// component is derived from the role.
    pub(super) fn from_role(role: crate::command::DynRole) -> Self {
        Self {
            component: component_for_role(role),
            role,
            buffer: None,
        }
    }
    pub(super) fn locked(&self) -> bool {
        self.buffer.is_some()
    }
}

/// Map a [`DynRole`](crate::command::DynRole) to the ordinate/distance/angle
/// component used by point resolution.
pub(super) fn component_for_role(role: crate::command::DynRole) -> DynComponent {
    use crate::command::DynRole;
    match role {
        DynRole::X | DynRole::Width => DynComponent::X,
        DynRole::Y | DynRole::Height => DynComponent::Y,
        DynRole::Z => DynComponent::Z,
        DynRole::Distance | DynRole::Radius | DynRole::Diameter => DynComponent::Distance,
        DynRole::Angle => DynComponent::Angle,
        DynRole::Factor | DynRole::Count => DynComponent::Scalar,
    }
}

fn default_role_for(component: DynComponent) -> crate::command::DynRole {
    use crate::command::DynRole;
    match component {
        DynComponent::X => DynRole::X,
        DynComponent::Y => DynRole::Y,
        DynComponent::Z => DynRole::Z,
        DynComponent::Distance => DynRole::Distance,
        DynComponent::Angle => DynRole::Angle,
        DynComponent::Scalar => DynRole::Factor,
    }
}

// ── Per-document tab state ─────────────────────────────────────────────────

pub(super) struct DocumentTab {
    pub(super) scene: Scene,
    pub(super) current_path: Option<PathBuf>,
    pub(super) dirty: bool,
    pub(super) tab_title: String,
    pub(super) properties: PropertiesPanel,
    pub(super) layers: LayerPanel,
    pub(super) active_cmd: Option<Box<dyn CadCommand>>,
    pub(super) last_cmd: Option<String>,
    pub(super) snap_result: Option<SnapResult>,
    pub(super) active_grip: Option<GripEdit>,
    pub(super) selected_grips: Vec<GripDef>,
    pub(super) selected_handle: Option<Handle>,
    /// Dynamic-block visibility grip for the current single selection.
    pub(super) visibility_grip: Option<super::visibility::VisibilityGrip>,
    pub(super) wireframe: bool,
    pub(super) render_mode: acadrust::entities::ViewportRenderMode,
    pub(super) visual_style: String,
    pub(super) last_cursor_world: glam::Vec3,
    pub(super) last_cursor_screen: iced::Point,
    /// Base point (`App::last_point`) projected to viewport pixels, refreshed
    /// on cursor move. Lets the dynamic-input overlay place the distance label
    /// along the rubber-band line and the angle label at its end.
    pub(super) last_point_screen: Option<iced::Point>,
    /// Dynamic-input fields shown near the cursor while a command waits
    /// for a point/distance/angle. Rebuilt whenever the active command's
    /// `dyn_field()` or the presence of a base point changes. Empty when
    /// dynamic input is not active.
    pub(super) dyn_fields: Vec<DynFieldEntry>,
    /// Guide geometry the overlay draws for the current step (set alongside
    /// `dyn_fields`). Polar arc, radius line, axis-delta projections, etc.
    pub(super) dyn_guide: crate::command::DynGuide,
    /// World-space anchor the current step's values are measured from. `None`
    /// falls back to `App::last_point`.
    pub(super) dyn_anchor: Option<glam::Vec3>,
    /// Far end of a reference line through `dyn_anchor` (for the `Perp` guide:
    /// the base edge / major axis the offset is measured square to).
    pub(super) dyn_ref: Option<glam::Vec3>,
    /// `dyn_ref` projected to viewport pixels.
    pub(super) dyn_ref_screen: Option<iced::Point>,
    /// Index of the field that TAB has focused (the one keystrokes edit).
    pub(super) dyn_active: usize,
    pub(super) history: HistoryState,
    pub(super) active_layer: String,
    /// Currently active UCS. `None` means WCS (identity transform).
    pub(super) active_ucs: Option<Ucs>,
    /// Custom model-space background color.  `None` = default dark grey.
    pub(super) bg_color: Option<[f32; 4]>,
    /// Custom paper-space background color.  `None` = default off-white grey.
    pub(super) paper_bg_color: Option<[f32; 4]>,
    /// Active REFEDIT session, if any.
    pub(super) refedit_session: Option<RefEditSession>,
    /// Currently active MLeader style name.
    pub(super) active_mleader_style: String,
    /// Last camera_generation value written back to the document.
    pub(super) last_synced_camera_gen: u64,
    /// Sentinel "Welcome / Start" tab. Always at index 0 when present.
    /// Cannot be closed; the viewport area renders a welcome page instead
    /// of the model-space shader. The scene is still constructed so the
    /// rest of the code can treat it as a normal tab when reading.
    pub(super) is_start: bool,
    /// Per-plugin document state (`plugin::BuiltinPlugin` manifest id → state).
    pub(super) plugin_state: HashMap<&'static str, Box<dyn Any + Send + Sync>>,
}

impl DocumentTab {
    pub(super) fn new_drawing(n: usize) -> Self {
        let mut scene = Scene::new();
        linetypes::populate_document(&mut scene.document);
        // Override acadrust's imperial default limits (12×9) with A4 landscape.
        for obj in scene.document.objects.values_mut() {
            if let acadrust::objects::ObjectType::Layout(l) = obj {
                if l.name != "Model" {
                    l.min_limits = (0.0, 0.0);
                    l.max_limits = (297.0, 210.0);
                    l.min_extents = (0.0, 0.0, 0.0);
                    l.max_extents = (297.0, 210.0, 0.0);
                }
            }
        }
        Self {
            scene,
            current_path: None,
            dirty: false,
            tab_title: format!("Drawing{}", n),
            properties: PropertiesPanel::empty(),
            layers: LayerPanel::default(),
            active_cmd: None,
            last_cmd: None,
            snap_result: None,
            active_grip: None,
            selected_grips: vec![],
            selected_handle: None,
            visibility_grip: None,
            wireframe: false,
            render_mode: acadrust::entities::ViewportRenderMode::Wireframe2D,
            visual_style: "Wireframe 2D".into(),
            last_cursor_world: glam::Vec3::ZERO,
            last_cursor_screen: iced::Point::ORIGIN,
            last_point_screen: None,
            dyn_fields: Vec::new(),
            dyn_guide: crate::command::DynGuide::Polar,
            dyn_anchor: None,
            dyn_ref: None,
            dyn_ref_screen: None,
            dyn_active: 0,
            history: HistoryState::default(),
            active_layer: "0".to_string(),
            active_ucs: None,
            bg_color: None,
            paper_bg_color: None,
            refedit_session: None,
            active_mleader_style: "Standard".to_string(),
            last_synced_camera_gen: 0,
            is_start: false,
            plugin_state: HashMap::new(),
        }
    }

    pub(super) fn plugin_state<T: Any + Send + Sync + 'static>(
        &self,
        plugin_id: &'static str,
        _type_id: TypeId,
    ) -> Option<&T> {
        self.plugin_state.get(plugin_id)?.downcast_ref::<T>()
    }

    pub(super) fn plugin_state_mut<T: Any + Send + Sync + 'static>(
        &mut self,
        plugin_id: &'static str,
        _type_id: TypeId,
    ) -> Option<&mut T> {
        self.plugin_state.get_mut(plugin_id)?.downcast_mut::<T>()
    }

    pub(super) fn ensure_plugin_state<T: Any + Send + Sync + 'static>(
        &mut self,
        plugin_id: &'static str,
        init: impl FnOnce() -> T,
    ) -> &mut T {
        if !self.plugin_state.contains_key(plugin_id) {
            self.plugin_state.insert(plugin_id, Box::new(init()));
        }
        self.plugin_state
            .get_mut(plugin_id)
            .expect("just inserted")
            .downcast_mut::<T>()
            .expect("plugin_id type mismatch")
    }

    /// Welcome / Start tab. Carries a dummy Scene so the rest of the app
    /// can read tab state uniformly; the viewport renderer detects
    /// `is_start` and shows a welcome page instead.
    pub(super) fn new_start() -> Self {
        let mut t = Self::new_drawing(0);
        t.tab_title = "Start".to_string();
        t.is_start = true;
        t
    }

    pub(super) fn tab_display_name(&self) -> String {
        match &self.current_path {
            Some(p) => p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            None => self.tab_title.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct HistorySnapshot {
    pub(super) document: CadDocument,
    pub(super) current_layout: String,
    pub(super) selected: Vec<Handle>,
    pub(super) dirty: bool,
    pub(super) label: String,
}

#[derive(Default)]
pub(super) struct HistoryState {
    pub(super) undo_stack: Vec<HistorySnapshot>,
    pub(super) redo_stack: Vec<HistorySnapshot>,
}
