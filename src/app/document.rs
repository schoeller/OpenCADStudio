use crate::command::CadCommand;
use crate::io::linetypes;
use crate::modules::draw::modify::block_edit::BlockEditSession;
use crate::modules::draw::modify::refedit::RefEditSession;
use crate::scene::pick::grip::GripEdit;
use crate::scene::GripDef;
use crate::scene::Scene;
use crate::snap::SnapResult;
use crate::ui::{LayerPanel, PropertiesPanel};
use acadrust::tables::Ucs;
use acadrust::{CadDocument, EntityType, Handle};
use iced;
use ocs_plugin_api::shm::{DocumentShmResources, DocumentSnapshotStore};
use std::any::Any;
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

/// One layer's display state, captured and restored by LAYERSTATE.
#[derive(Clone)]
pub(super) struct LayerSnap {
    pub name: String,
    pub off: bool,
    pub frozen: bool,
    pub locked: bool,
    pub color: acadrust::types::Color,
    pub line_type: String,
    pub line_weight: acadrust::types::LineWeight,
}

impl DocumentTab {
    /// Snapshot every layer's current display state under `name` (overwrites
    /// any existing state of that name).
    pub(super) fn save_layer_state(&mut self, name: &str) {
        let snaps: Vec<LayerSnap> = self
            .scene
            .document
            .layers
            .iter()
            .map(|l| LayerSnap {
                name: l.name.clone(),
                off: l.flags.off,
                frozen: l.flags.frozen,
                locked: l.flags.locked,
                color: l.color.clone(),
                line_type: l.line_type.clone(),
                line_weight: l.line_weight.clone(),
            })
            .collect();
        self.layer_states.insert(name.to_string(), snaps);
    }

    /// Reapply the saved state `name` to the matching layers; returns the number
    /// of layers updated, or `None` if no such state exists.
    pub(super) fn restore_layer_state(&mut self, name: &str) -> Option<usize> {
        let snaps = self.layer_states.get(name)?.clone();
        let mut applied = 0usize;
        for s in &snaps {
            if let Some(l) = self.scene.document.layers.get_mut(&s.name) {
                l.flags.off = s.off;
                l.flags.frozen = s.frozen;
                l.flags.locked = s.locked;
                l.color = s.color.clone();
                l.line_type = s.line_type.clone();
                l.line_weight = s.line_weight.clone();
                applied += 1;
            }
        }
        Some(applied)
    }
}

pub(super) struct DocumentTab {
    pub(super) scene: Scene,
    /// Shared-memory snapshot store for out-of-process plugins. Kept on the tab
    /// so the backing file survives individual HostSession instances.
    pub(super) doc_store: Option<DocumentSnapshotStore>,
    /// Shared-memory resources (full snapshot + mutation queue) for the Python
    /// REPL and similar batch-style out-of-process plugins.
    pub(super) doc_shm: Option<DocumentShmResources>,
    /// Last `geometry_epoch` value for which the full snapshot was published.
    /// Used to publish the Python REPL snapshot on manual document edits.
    pub(super) last_published_geometry_epoch: u64,
    pub(super) current_path: Option<PathBuf>,
    pub(super) dirty: bool,
    pub(super) tab_title: String,
    pub(super) properties: PropertiesPanel,
    pub(super) layers: LayerPanel,
    pub(super) active_cmd: Option<Box<dyn CadCommand>>,
    /// The selection set the most recent command worked on, captured when a
    /// finishing command drops the live selection — re-selectable with the
    /// "Previous" keyword at any Select objects prompt (#426).
    pub(super) prev_selection: Vec<acadrust::Handle>,
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
    pub(super) last_cursor_world: glam::DVec3,
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
    pub(super) dyn_anchor: Option<glam::DVec3>,
    /// Far end of a reference line through `dyn_anchor` (for the `Perp` guide:
    /// the base edge / major axis the offset is measured square to).
    pub(super) dyn_ref: Option<glam::DVec3>,
    /// `dyn_ref` projected to viewport pixels.
    pub(super) dyn_ref_screen: Option<iced::Point>,
    /// Index of the field that TAB has focused (the one keystrokes edit).
    pub(super) dyn_active: usize,
    pub(super) history: HistoryState,
    pub(super) active_layer: String,
    /// Named layer-state snapshots saved by LAYERSTATE (name → per-layer state).
    pub(super) layer_states: std::collections::HashMap<String, Vec<LayerSnap>>,
    /// Currently active UCS. `None` means WCS (identity transform).
    pub(super) active_ucs: Option<Ucs>,
    /// Custom model-space background color.  `None` = default dark grey.
    pub(super) bg_color: Option<[f32; 4]>,
    /// Custom paper-space background color.  `None` = default off-white grey.
    pub(super) paper_bg_color: Option<[f32; 4]>,
    /// Active REFEDIT session, if any.
    pub(super) refedit_session: Option<RefEditSession>,
    /// Active BEDIT block-editor space session, if any (issue #261).
    pub(super) block_edit: Option<BlockEditSession>,
    /// Currently active MLeader style name.
    pub(super) active_mleader_style: String,
    /// Last camera_generation value written back to the document.
    pub(super) last_synced_camera_gen: u64,
    /// Sentinel "Welcome / Start" tab. Always at index 0 when present.
    /// Cannot be closed; the viewport area renders a welcome page instead
    /// of the model-space shader. The scene is still constructed so the
    /// rest of the code can treat it as a normal tab when reading.
    pub(super) is_start: bool,
    /// Interactive PAN mode (the PAN command / tool). While active, a left-
    /// button drag pans the view instead of selecting — the only pan path on a
    /// device with no middle mouse button (a trackpad / web client). Exited
    /// with Esc or by starting another command.
    pub(super) pan_mode: bool,
    /// Per-plugin document state (`plugin::BuiltinPlugin` manifest id → state).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) plugin_state: HashMap<&'static str, Box<dyn Any + Send + Sync>>,
    pub(super) suspended_cmd: Option<Box<dyn CadCommand>>,
}

impl DocumentTab {
    /// Publish the full serde/bincode snapshot if the document's geometry epoch
    /// has changed since the last publish. This keeps the Python REPL file in
    /// sync with manual edits made through the host UI.
    pub(super) fn publish_full_snapshot(&mut self) {
        let epoch = self.scene.geometry_epoch;
        if epoch == self.last_published_geometry_epoch {
            return;
        }
        let mut shm_opt = self.doc_shm.take();
        let doc = &self.scene.document;
        if let Some(ref mut shm) = shm_opt {
            shm.publish_full_snapshot(doc);
        }
        self.doc_shm = shm_opt;
        self.last_published_geometry_epoch = epoch;
    }

    /// The active WCS↔UCS converter for this tab — identity when no UCS is set.
    /// Every consumer that needs UCS-relative coordinates goes through this.
    pub(super) fn ucs_xform(&self) -> super::helpers::UcsXform {
        super::helpers::UcsXform::from_active(self.active_ucs.as_ref())
    }

    /// World-space UCS origin in full f64 precision (stored f64 in the header /
    /// viewport). Used to anchor the UCS icon and as the cursor-pick plane point
    /// — `UcsXform`'s origin is f32 and would quantize the anchor at UTM scale.
    pub(super) fn ucs_origin_world(&self) -> glam::DVec3 {
        self.active_ucs
            .as_ref()
            .map(|u| glam::dvec3(u.origin.x, u.origin.y, u.origin.z))
            .unwrap_or(glam::DVec3::ZERO)
    }

    /// True when the active pane edits **model-space** geometry: the Model tab,
    /// or inside a floating viewport (MSPACE). The UCS applies in both; plain
    /// paper space (no active viewport) is excluded. Single predicate so every
    /// UCS-aware system shares one rule. [[feedback_shared_infra]]
    pub(super) fn editing_model_space(&self) -> bool {
        self.scene.current_layout == "Model" || self.scene.active_viewport.is_some()
    }

    /// The document's saved model-space UCS (header), as a `Ucs`. `None` when it
    /// is identity (plain WCS).
    fn model_ucs_from_header(&self) -> Option<Ucs> {
        let h = &self.scene.document.header;
        let mut u = Ucs::new(h.model_space_ucs_name.clone());
        u.origin = h.model_space_ucs_origin;
        u.x_axis = h.model_space_ucs_x_axis;
        u.y_axis = h.model_space_ucs_y_axis;
        if super::helpers::UcsXform::from_ucs(&u).is_identity() {
            None
        } else {
            Some(u)
        }
    }

    /// A floating viewport's own per-viewport UCS, if it has one set. `None`
    /// when the viewport uses world coordinates or the handle is not a viewport.
    pub(super) fn ucs_from_viewport(&self, h: Handle) -> Option<Ucs> {
        let vp = match self.scene.document.get_entity(h) {
            Some(acadrust::EntityType::Viewport(vp)) => vp,
            _ => return None,
        };
        if !vp.ucs_per_viewport {
            return None;
        }
        let mut u = Ucs::new("*VPUCS*");
        u.origin = vp.ucs_origin;
        u.x_axis = vp.ucs_x_axis;
        u.y_axis = vp.ucs_y_axis;
        if super::helpers::UcsXform::from_ucs(&u).is_identity() {
            None
        } else {
            Some(u)
        }
    }

    /// Set `active_ucs` to the UCS of the *current pane*: the entered viewport's
    /// own per-viewport UCS, the model header UCS in the Model tab, or none in
    /// plain paper space. Keeps the ViewCube in lock-step. Call on every pane
    /// change (enter/exit viewport, layout / tab switch, load) so one field
    /// drives all UCS-aware systems regardless of where editing happens.
    pub(super) fn refresh_active_ucs(&mut self) {
        self.active_ucs = if let Some(h) = self.scene.active_viewport {
            self.ucs_from_viewport(h)
        } else if self.scene.current_layout == "Model" {
            self.model_ucs_from_header()
        } else {
            None
        };
        self.sync_ucs_to_scene();
    }

    /// Persist `active_ucs` back to its pane's storage so it round-trips: the
    /// entered viewport's per-viewport UCS fields, or the document header's
    /// model-space UCS in the Model tab. No-op in plain paper space. Call after
    /// any UCS change.
    pub(super) fn persist_active_ucs(&mut self) {
        use acadrust::types::Vector3;
        if let Some(h) = self.scene.active_viewport {
            let (o, x, y, per) = match &self.active_ucs {
                Some(u) => (u.origin, u.x_axis, u.y_axis, true),
                None => (Vector3::ZERO, Vector3::UNIT_X, Vector3::UNIT_Y, false),
            };
            if let Some(acadrust::EntityType::Viewport(vp)) = self.scene.document.get_entity_mut(h)
            {
                vp.ucs_origin = o;
                vp.ucs_x_axis = x;
                vp.ucs_y_axis = y;
                vp.ucs_per_viewport = per;
            }
        } else if self.scene.current_layout == "Model" {
            let h = &mut self.scene.document.header;
            match &self.active_ucs {
                Some(u) => {
                    h.model_space_ucs_origin = u.origin;
                    h.model_space_ucs_x_axis = u.x_axis;
                    h.model_space_ucs_y_axis = u.y_axis;
                }
                None => {
                    h.model_space_ucs_origin = Vector3::ZERO;
                    h.model_space_ucs_x_axis = Vector3::UNIT_X;
                    h.model_space_ucs_y_axis = Vector3::UNIT_Y;
                }
            }
        }
    }

    /// Adopt the active pane's UCS on load (the file's saved model-space UCS in
    /// the Model tab). Thin wrapper over [`refresh_active_ucs`] kept for the
    /// load call sites.
    pub(super) fn adopt_active_ucs_from_header(&mut self) {
        self.refresh_active_ucs();
    }

    /// Push the active UCS rotation into the scene so the ViewCube composes with
    /// it. Call after any change to `active_ucs`.
    pub(super) fn sync_ucs_to_scene(&mut self) {
        self.scene.viewcube_ucs = self.ucs_xform().rotation_mat();
    }

    /// UCS→render(wire)-space affine for commands that build axis-aligned
    /// geometry. Columns are the UCS axes; translation is the UCS origin in wire
    /// space. Identity outside model space (no UCS there).
    pub(super) fn ucs_wire_affine(&self) -> glam::Mat4 {
        if !self.editing_model_space() {
            return glam::Mat4::IDENTITY;
        }
        let (o, x, y, z) = self.ucs_xform().axes();
        let origin = glam::Vec3::new(o.x as f32, o.y as f32, o.z as f32);
        glam::Mat4::from_cols(
            x.as_vec3().extend(0.0),
            y.as_vec3().extend(0.0),
            z.as_vec3().extend(0.0),
            origin.extend(1.0),
        )
    }

    /// World-space rotation angle (radians) of the active UCS X axis — the
    /// default rotation for new text-bearing objects so their text aligns to
    /// the user's coordinate system. Zero outside model space / with no UCS.
    pub(super) fn ucs_rotation_angle(&self) -> f64 {
        if !self.editing_model_space() {
            return 0.0;
        }
        let (_, x, ..) = self.ucs_xform().axes();
        (x.y as f64).atan2(x.x as f64)
    }

    /// Grid origin (render/wire space) and UCS→world rotation for grid snap and
    /// the grid overlay. Identity / origin-at-zero outside model space.
    pub(super) fn ucs_grid_basis(&self) -> (glam::Vec3, glam::Mat4) {
        if !self.editing_model_space() {
            return (glam::Vec3::ZERO, glam::Mat4::IDENTITY);
        }
        let xf = self.ucs_xform();
        let (o, ..) = xf.axes();
        let origin = glam::Vec3::new(o.x as f32, o.y as f32, o.z as f32);
        (origin, xf.rotation_mat())
    }

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
            doc_store: None,
            doc_shm: None,
            last_published_geometry_epoch: 0,
            current_path: None,
            dirty: false,
            prev_selection: Vec::new(),
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
            last_cursor_world: glam::DVec3::ZERO,
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
            layer_states: std::collections::HashMap::new(),
            active_ucs: None,
            bg_color: None,
            paper_bg_color: None,
            refedit_session: None,
            block_edit: None,
            active_mleader_style: "Standard".to_string(),
            last_synced_camera_gen: 0,
            is_start: false,
            pan_mode: false,
            plugin_state: HashMap::new(),
            suspended_cmd: None,
        }
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

/// One undo/redo entry. Most edits touch the whole drawing's state and store a
/// [`FullSnapshot`] (a document clone); frequent entity-only edits (move / copy /
/// draw / erase) instead store a cheap [`DeltaSnapshot`] naming just the entities
/// they changed, so an 800k-entity drawing doesn't pay a ~46 ms document clone
/// per edit.
#[derive(Clone)]
pub(super) enum HistorySnapshot {
    Full(FullSnapshot),
    Delta(DeltaSnapshot),
}

impl HistorySnapshot {
    pub(super) fn label(&self) -> &str {
        match self {
            HistorySnapshot::Full(f) => &f.label,
            HistorySnapshot::Delta(d) => &d.label,
        }
    }
}

/// A whole-document undo entry: the safe fallback for any command that may touch
/// layers, objects, block records, styles or tables.
#[derive(Clone)]
pub(super) struct FullSnapshot {
    pub(super) document: CadDocument,
    pub(super) current_layout: String,
    pub(super) selected: Vec<Handle>,
    pub(super) dirty: bool,
    pub(super) label: String,
}

/// An entity-only undo entry: for each touched handle, its before-image and
/// after-image (`None` = the entity was absent on that side, i.e. an add or an
/// erase). Symmetric — undo applies the before side, redo the after side — so
/// one delta moves between the undo and redo stacks without re-capturing. The
/// command by construction changes no layers / objects / blocks, so only the
/// cheap scalars (`current_layout`, selection, `dirty`) ride alongside.
#[derive(Clone)]
pub(super) struct DeltaSnapshot {
    pub(super) entities: Vec<(Handle, Option<EntityType>, Option<EntityType>)>,
    pub(super) current_layout: String,
    pub(super) selected_before: Vec<Handle>,
    pub(super) selected_after: Vec<Handle>,
    pub(super) dirty_before: bool,
    pub(super) dirty_after: bool,
    pub(super) label: String,
}

#[derive(Default)]
pub(super) struct HistoryState {
    pub(super) undo_stack: Vec<HistorySnapshot>,
    pub(super) redo_stack: Vec<HistorySnapshot>,
}
