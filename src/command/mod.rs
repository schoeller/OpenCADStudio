// Command system — trait for interactive CAD commands.
//
// Each tool that requires user interaction (point picks, object selection,
// numeric input) implements `CadCommand`.  The active command receives
// viewport events from main.rs and returns `CmdResult` tokens that tell
// the host what to do next.

use crate::scene::model::hatch_model::HatchModel;
use crate::scene::model::wire_model::WireModel;
use crate::scene::Scene;
use acadrust::{EntityType, Handle};
use glam::DVec3;

/// Domain object resolved under the cursor for ObjectPick snapping.
#[derive(Clone, Copy, Debug)]
pub struct ObjectPickHit {
    pub handle: Handle,
    pub x: f64,
    pub y: f64,
    pub label: &'static str,
}

// ── Transform ─────────────────────────────────────────────────────────────

/// A geometric transformation applied to existing entities.
#[derive(Clone)]
pub enum EntityTransform {
    /// Move every point by the given world-space delta (world XY plane).
    Translate(DVec3),
    /// Rotate around `center` by `angle_rad` in the world XY plane.
    Rotate { center: DVec3, angle_rad: f32 },
    /// Uniform scale from `center` by `factor`.
    Scale { center: DVec3, factor: f32 },
    /// Mirror across the line through `p1`→`p2` in the world XY plane.
    Mirror { p1: DVec3, p2: DVec3 },
}

// ── Tangent object ─────────────────────────────────────────────────────────

/// Geometric representation of a tangent-snap target.
#[derive(Clone, Copy, Debug)]
pub enum TangentObject {
    /// Infinite line through two world-space XZ-plane points.
    Line { p1: DVec3, p2: DVec3 },
    /// Circle in the world XY plane.
    Circle { center: DVec3, radius: f64 },
}

// ── Result token ──────────────────────────────────────────────────────────

/// Returned by every `CadCommand` method to tell main.rs what to do.
#[allow(dead_code)]
pub enum CmdResult {
    /// Command is still waiting for the next point; show updated prompt.
    NeedPoint,
    /// Update the committed-segment wire (normal colour) and keep collecting points.
    InterimWire(WireModel),
    /// Update the in-progress (cyan) preview wire in the viewport.
    Preview(WireModel),
    /// Commit an acadrust entity to the document; keep the command active.
    CommitEntity(EntityType),
    /// Commit an acadrust entity to the document and end the command.
    CommitAndExit(EntityType),
    /// Commit a Model-tab 3D solid: the acadrust entity (for selection /
    /// persistence) plus its truck B-rep (cached for boolean ops + shaded
    /// rendering). Ends the command.
    CommitSolid {
        entity: EntityType,
        solid: Box<truck_modeling::Solid>,
    },
    /// Commit an acadrust entity, end the command, and open the in-place text
    /// editor on it (used by MLEADER to type the annotation after placement).
    CommitAndEditText(EntityType),
    /// Commit several entities, end the command, and open the in-place text
    /// editor on the one at `edit_index` (used by LEADER to place the leader
    /// line plus an empty MText annotation, then type into the MText).
    CommitManyAndEditText {
        entities: Vec<EntityType>,
        edit_index: usize,
    },
    /// Create a block definition from existing entities and insert one reference.
    CreateBlock {
        handles: Vec<Handle>,
        name: String,
        base: DVec3,
    },
    /// Apply a transform to selected entities and end the command.
    TransformSelected(Vec<Handle>, EntityTransform),
    /// Copy selected entities with a transform; command stays active for more copies.
    CopySelected(Vec<Handle>, EntityTransform),
    /// Commit a hatch fill (stored in Scene::hatches, not the DXF document).
    CommitHatch(HatchModel),
    /// Copy selected entities with multiple transforms (e.g. rectangular array); end command.
    BatchCopy(Vec<Handle>, Vec<EntityTransform>),
    /// Erase `handle` and replace with new entities; command stays active.
    ReplaceEntity(Handle, Vec<EntityType>),
    /// Replace / delete multiple entities and add new ones; command ends.
    /// Each pair: (handle_to_erase, replacement_entities) — empty vec = delete only.
    ReplaceMany(Vec<(Handle, Vec<EntityType>)>, Vec<EntityType>),
    /// Cancel: discard any preview and end the command.
    Cancel,
    /// End the selection-gather phase and re-dispatch the named command
    /// with the gathered handles installed as the active scene selection.
    Relaunch(String, Vec<Handle>),
    /// Move `dest` entities to the layer of the `src` entity; end command.
    MatchEntityLayer { dest: Vec<Handle>, src: Handle },
    /// Copy all visual properties (layer/color/linetype/lineweight) from `src` to `dest`; end command.
    MatchProperties { dest: Vec<Handle>, src: Handle },
    /// Create a named group from the given entity handles; end command.
    CreateGroup { handles: Vec<Handle>, name: String },
    /// Dissolve all groups that contain any of the given handles; end command.
    DeleteGroups { handles: Vec<Handle> },
    /// Freeze or thaw layers by name in the given viewport; command stays active.
    VpLayerUpdate {
        vp_handle: Handle,
        freeze: Vec<String>,
        thaw: Vec<String>,
    },
    /// Paste clipboard entities translated so their centroid lands at `base_pt`; end command.
    PasteClipboard { base_pt: DVec3 },
    /// Zoom the model-space camera to fit the given corner points; end command.
    ZoomToWindow { p1: DVec3, p2: DVec3 },
    /// Print a measurement result to the command line and end the command.
    Measurement(String),
    /// Break `handle` at points `p1` and `p2`; replace with computed fragments.
    BreakEntity { handle: Handle, p1: DVec3, p2: DVec3 },
    /// Attempt to join the given entities into fewer merged entities.
    JoinEntities(Vec<Handle>),
    /// Apply a polyline-edit operation to one entity; keep command active.
    PeditOp {
        handle: Handle,
        op: crate::modules::draw::modify::pedit::PeditOp,
    },
    /// Place Point entities at N equal intervals along the entity.
    DivideEntity { handle: Handle, n: usize },
    /// Place Point entities at `segment_length` intervals along the entity.
    MeasureEntity { handle: Handle, segment_length: f64 },
    /// Extend/trim a Line or Arc by the given mode; end command.
    LengthenEntity {
        handle: Handle,
        pick_pt: DVec3,
        mode: crate::modules::draw::modify::lengthen::LenMode,
    },
    /// Align selected entities: translate to dst1, rotate by angle_rad, optional scale.
    AlignSelected {
        handles: Vec<Handle>,
        src1: DVec3,
        dst1: DVec3,
        angle_rad: f32,
        scale: f32,
    },
    /// Set the plot window on the active layout's PlotSettings.
    SetPlotWindow { p1: DVec3, p2: DVec3 },
    /// Replace the text content of a Text/MText entity in-place.
    DdeditEntity { handle: Handle, new_text: String },
    /// Open the in-place editor (plain box or rich MText editor, per type) for
    /// a text-bearing entity picked by a command such as DDEDIT.
    EditTextEntity { handle: Handle },
    /// Open the in-place MText editor (formatting toolbar + multi-line text
    /// area with live viewport preview). `handle` is `Some` when editing an
    /// existing MText, `None` when creating a new one at `pos`.
    OpenMTextEditor {
        pos: DVec3,
        handle: Option<Handle>,
        initial: String,
        height: f64,
    },
    /// Open the in-place single-line TEXT editor (a plain text-entry box, no
    /// formatting toolbar). `handle` is `Some` when editing an existing Text,
    /// `None` when creating a new one at `pos`.
    OpenTextEditor {
        pos: DVec3,
        handle: Option<Handle>,
        initial: String,
        height: f64,
    },
    /// Apply new pattern/scale/angle to an existing hatch entity.
    HatcheditApply {
        handle: Handle,
        name: String,
        scale: f32,
        angle: f32,
    },
    /// Stretch entities: move only vertices/endpoints inside the crossing window.
    StretchEntities {
        handles: Vec<Handle>,
        /// Min corner of the crossing window in world XZ (= DXF XY).
        win_min: DVec3,
        /// Max corner of the crossing window in world XZ (= DXF XY).
        win_max: DVec3,
        /// Translation vector to apply to vertices inside the window.
        delta: DVec3,
    },
    /// Create a Solid3D placeholder entity + associated MeshModel.
    /// `mesh_fn` is called with the entity's handle string to build the mesh.
    CommitSolid3D {
        mesh_fn: Box<dyn FnOnce(String) -> Option<crate::scene::model::mesh_model::MeshModel> + Send>,
    },
    /// Extrude the profile entity `handle` by `height` along Z.
    ExtrudeEntity {
        handle: Handle,
        height: f32,
        color: [f32; 4],
    },
    /// Revolve the profile entity `handle` around the given axis by `angle_deg`.
    RevolveEntity {
        handle: Handle,
        axis_start: glam::DVec3,
        axis_end: glam::DVec3,
        angle_deg: f32,
        color: [f32; 4],
    },
    /// Sweep the profile entity `profile_handle` along `path_handle`.
    SweepEntity {
        profile_handle: Handle,
        path_handle: Handle,
        color: [f32; 4],
    },
    /// Loft through a series of profile entities.
    LoftEntities {
        handles: Vec<Handle>,
        color: [f32; 4],
    },
    /// INSERT landed on a block that has AttributeDefinitions.
    /// The host should look up the attdefs for `block_name` from the document
    /// and call `attreq_set_attdefs()` on the command, then loop on text input.
    AttreqNeeded { block_name: String },
    /// Add a command-owned "live" entity to the document mid-command and hand
    /// its assigned handle back to the active command via `set_live_handle()`.
    /// One undo snapshot is pushed here, so the whole in-progress object reverts
    /// as a single unit. The command stays active. Used by PLINE so the partial
    /// polyline is a real, snappable entity while later vertices are placed.
    CommitLiveEntity(EntityType),
    /// Replace the geometry of the live entity `handle` in place — preserving
    /// its layer — without pushing a new undo snapshot. When `finish` is true
    /// the command also exits (the entity is already committed, so no separate
    /// commit is needed).
    UpdateLiveEntity {
        handle: Handle,
        entity: EntityType,
        finish: bool,
    },
    /// Suspends command execution, moves it to suspended_cmd, and opens the text editor for the given handle.
    SuspendForTextEdit { handle: Handle },
    /// Requests a standard document-level undo while keeping the command active.
    UndoDocument,
    /// Sets the TEXTEDITMODE system variable and ends the command.
    SetTexteditMode(bool),
}

/// What kind of value the active command is currently asking for. Drives
/// the dynamic-input overlay so the tooltip shows the relevant quantity
/// (coordinates for a point pick, a single length for a radius/distance
/// prompt, degrees for an angle prompt).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DynField {
    /// A position — X/Y coordinates (or distance+angle relative to the
    /// last point). The default for every command step.
    #[default]
    Point,
    /// A single linear distance (radius, length, offset) measured from
    /// the last point.
    Distance,
    /// An angle, shown in degrees, measured from the last point.
    Angle,
    /// A typed scalar with no geometric meaning at the cursor — a count
    /// (number of sides / segments) or any value the command reads purely
    /// from the keyboard. Shown as a single typed box.
    Scalar,
}

// ── Per-step dynamic-input specification ───────────────────────────────────
//
// `DynField` only says "this step wants a point / distance / angle". `DynSpec`
// lets a command describe its step precisely: which value boxes to show (with
// roles + labels), what guide geometry to draw, and where it is measured from.
// A command returns `Some(DynSpec)` from `dyn_spec()` to take explicit control;
// returning `None` (the default) keeps the legacy `dyn_field()` behaviour.

/// Semantic role of a dynamic-input box. Resolution maps each role to a base
/// ordinate/distance/angle; the role additionally drives the label and any
/// value scaling (e.g. a diameter shows/accepts twice the geometric radius).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DynRole {
    X,
    Y,
    Z,
    /// Linear distance from the anchor.
    Distance,
    /// Angle from the anchor, degrees.
    Angle,
    /// Distance shown labelled `R` (circle/arc radius).
    Radius,
    /// Distance shown labelled `⌀`; displayed/typed value is twice the radius.
    Diameter,
    /// Cartesian X-delta shown labelled `W` (rectangle width).
    Width,
    /// Cartesian Y-delta shown labelled `H` (rectangle height).
    Height,
    /// Typed-only scale factor.
    Factor,
    /// Typed-only integer count. Reserved for upcoming command migrations.
    #[allow(dead_code)]
    Count,
}

/// Shared rule for how an angle reads in the dynamic-input box: the unsigned
/// magnitude of the short signed angle, so a clockwise angle (cursor below the
/// reference axis) shows as a positive value rather than a negative or a
/// CCW 300-something. Callers keep the *signed* radian for the actual
/// computation/commit; this is display-only. `signed_rad` is the angle from
/// the reference to the cursor.
pub fn dyn_display_angle_deg(signed_rad: f32) -> f32 {
    let mut a = signed_rad % std::f32::consts::TAU;
    if a > std::f32::consts::PI {
        a -= std::f32::consts::TAU;
    }
    if a <= -std::f32::consts::PI {
        a += std::f32::consts::TAU;
    }
    a.to_degrees().abs()
}

impl DynRole {
    /// Default label shown before the value (empty = value only).
    pub fn label(self) -> &'static str {
        match self {
            DynRole::X => "X",
            DynRole::Y => "Y",
            DynRole::Z => "Z",
            DynRole::Distance | DynRole::Angle | DynRole::Factor => "",
            DynRole::Radius => "R",
            DynRole::Diameter => "\u{2300}",
            DynRole::Width => "W",
            DynRole::Height => "H",
            DynRole::Count => "#",
        }
    }

    /// Multiplier between the geometric value and the displayed/typed value.
    /// A diameter box shows and accepts twice the underlying radius.
    pub fn value_scale(self) -> f32 {
        match self {
            DynRole::Diameter => 2.0,
            _ => 1.0,
        }
    }
}

/// Guide geometry the overlay draws for a step, anchored at the step's base.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DynGuide {
    /// No guide lines.
    None,
    /// +X reference line and the angle arc (polar point entry).
    Polar,
    /// Dotted projections from the cursor down to the anchor's X and Y axes.
    AxisDelta,
    /// A line from the anchor to the cursor (radius / single distance).
    Radius,
    /// The two rectangle sides (width × height) from the anchor corner.
    RectSides,
    /// A line from the anchor, perpendicular to the reference line (anchor →
    /// `DynSpec::ref_point`), reaching the cursor's perpendicular offset — the
    /// measured semi-axis (ellipse minor). The value is that offset.
    Perp,
    /// Like `Perp` but drawn as a dimension: the measured segment is offset off
    /// the edge with extension lines back to its endpoints (rectangle height).
    PerpDim,
}

/// Where a step's values are measured from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DynAnchor {
    /// The previous committed point (`App::last_point`). Reserved — current
    /// specs pass the anchor explicitly via `Point`.
    #[allow(dead_code)]
    LastPoint,
    /// An explicit world point.
    Point(DVec3),
}

/// One value box in a [`DynSpec`].
#[derive(Clone, Debug)]
pub struct DynFieldSpec {
    pub role: DynRole,
    /// Label override; `None` uses the role's default label.
    #[allow(dead_code)] // dyn-spec framework field; not yet consumed
    pub label: Option<&'static str>,
}

impl DynFieldSpec {
    pub fn new(role: DynRole) -> Self {
        Self { role, label: None }
    }
}

/// A full per-step dynamic-input description.
#[derive(Clone, Debug)]
pub struct DynSpec {
    pub anchor: DynAnchor,
    pub fields: Vec<DynFieldSpec>,
    pub guide: DynGuide,
    /// Far end of a reference line through `anchor` (only used by
    /// [`DynGuide::Perp`]); `None` otherwise.
    pub ref_point: Option<DVec3>,
}

// ── Trait ─────────────────────────────────────────────────────────────────

/// An interactive CAD command that collects user input step-by-step.
pub trait CadCommand: Send {
    /// Short name shown in the command line prompt, e.g. `"LINE"`.
    #[allow(dead_code)]
    fn name(&self) -> &'static str;
    /// Current prompt string to display in the command line.
    fn prompt(&self) -> String;

    /// Push the active UCS into the command as a UCS→render(wire)-space affine
    /// (identity = plain WCS). Commands that build axis-aligned geometry (RECT,
    /// rectangular ARRAY, …) override this to store it and rotate their implicit
    /// axes into the UCS; most commands work purely from picked points and
    /// ignore it. Called before each point / preview dispatch.
    fn set_ucs(&mut self, _ucs: glam::Mat4) {}

    /// Called when the user left-clicks in the viewport (point pick).
    fn on_point(&mut self, pt: DVec3) -> CmdResult;

    /// Called when the user presses Enter (finalize / next option).
    fn on_enter(&mut self) -> CmdResult;

    /// Called when the user presses Escape (cancel).
    #[allow(dead_code)]
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    /// Returns `true` when the command needs entity picking (hit-test) instead of point picking.
    fn needs_entity_pick(&self) -> bool {
        false
    }

    /// Called when the text editor closes, either because the user committed or cancelled the edit.
    fn on_editor_closed(&mut self, _committed: bool) -> CmdResult {
        CmdResult::Cancel
    }

    /// Called when the user clicks and `needs_entity_pick()` is true.
    /// `handle` is the nearest wire's entity handle (Handle::NULL if nothing found).
    fn on_entity_pick(&mut self, _handle: Handle, _pt: DVec3) -> CmdResult {
        CmdResult::Cancel
    }

    /// Host callback after `CmdResult::CommitLiveEntity`: records the handle the
    /// new live entity was assigned so later `UpdateLiveEntity` results can
    /// target it.
    fn set_live_handle(&mut self, _handle: Handle) {}

    /// Point-click pick of domain objects (wire hit-test often misses small markers).
    fn needs_structure_point_pick(&self) -> bool {
        false
    }

    /// Resolve a domain object near `(x, y)` while `needs_structure_point_pick()` is active.
    fn resolve_object_pick(&self, _scene: &Scene, _x: f64, _y: f64) -> Option<ObjectPickHit> {
        None
    }

    /// Preview wires while hovering during object-point pick.
    fn object_pick_hover_previews(&self, _scene: &Scene, _cursor: DVec3) -> Vec<WireModel> {
        vec![]
    }

    /// Message when `resolve_object_pick` returns none on click.
    fn object_pick_miss_message(&self) -> &'static str {
        "No object near click."
    }

    /// Called when `needs_structure_point_pick()` is true and a structure is found near the click.
    fn on_structure_pick(&mut self, _handle: Handle, _pt: DVec3) -> CmdResult {
        CmdResult::Cancel
    }

    /// Extra acquisition previews during entity pick (besides `on_hover_entity`).
    fn entity_pick_acquire_previews(&self, _scene: &Scene, _handle: Handle) -> Vec<WireModel> {
        vec![]
    }

    /// Acquisition hint label during entity pick hover.
    fn entity_pick_acquire_hint(&self, _handle: Handle) -> Option<&'static str> {
        None
    }

    /// Hover label for object acquisition (e.g. "Inlet" under cursor).
    fn set_acquisition_hint(&mut self, _hint: Option<&str>) {}

    /// Called after `CmdResult::ReplaceEntity` is applied to the document.
    /// `old` is the erased handle; `new_handles` are the handles assigned to the replacement entities.
    /// Commands that stay active across replaces should update their internal snapshots here.
    fn on_entity_replaced(&mut self, _old: Handle, _new_handles: &[Handle]) {}

    /// Called on every mouse-move when `needs_entity_pick()` is true.
    /// Return preview wires showing the operation result under the cursor.
    /// Default: empty (no preview).
    fn on_hover_entity(&mut self, _handle: Handle, _pt: DVec3) -> Vec<WireModel> {
        vec![]
    }

    /// Called on every mouse-move in the viewport.
    /// Return `Some(WireModel)` to update the rubber-band preview, `None` to skip.
    fn on_mouse_move(&mut self, _pt: DVec3) -> Option<WireModel> {
        None
    }

    /// Called on every mouse-move; return all preview wires to show (object ghosts + rubber-band).
    /// Default: forwards to `on_mouse_move` for backwards compatibility.
    fn on_preview_wires(&mut self, pt: DVec3) -> Vec<WireModel> {
        self.on_mouse_move(pt).into_iter().collect()
    }

    /// Returns `true` when the command is waiting for text typed in the command line.
    fn wants_text_input(&self) -> bool {
        false
    }

    /// Returns `true` when the current step is a point pick that *also* accepts
    /// optional keyword letters (e.g. PLINE's A/L/C/U). Such a step keeps the
    /// polar dynamic-input boxes: typed digits become coordinates while letters
    /// still reach the command line as keywords. Without this, a command that
    /// returns `wants_text_input() == true` for its keywords would suppress the
    /// dynamic-input distance/angle entirely. Default `false`.
    fn point_step_accepts_keywords(&self) -> bool {
        false
    }

    /// Returns `true` when the active text prompt expects free-form prose
    /// that can legitimately contain whitespace (the body of a TEXT /
    /// MTEXT / DDEDIT entity, an attribute default value, etc.). For
    /// these prompts the command-line input must let `Space` be typed as
    /// a literal character; for every other prompt `Space` submits the
    /// input the same way `Enter` does.
    ///
    /// Default `false` — single-token prompts (option letters, numeric
    /// radius, block name) do not embed spaces.
    fn wants_text_with_spaces(&self) -> bool {
        false
    }

    /// Called when the user submits text via the command line while `wants_text_input` is true.
    fn on_text_input(&mut self, _text: &str) -> Option<CmdResult> {
        None
    }

    /// Returns `true` when the command is in a selection-gathering phase.
    /// While true, viewport clicks are routed through the normal selection
    /// system (single / box / polygon) instead of the command's point-pick path.
    /// After each completed selection action the host calls `on_selection_complete`.
    fn is_selection_gathering(&self) -> bool {
        false
    }

    /// Called after a selection action completes while `is_selection_gathering` is true.
    /// `handles` is the full set of currently selected entities.
    /// Return `Relaunch` to fire the pending command, or `NeedPoint` to keep gathering.
    fn on_selection_complete(&mut self, _handles: Vec<Handle>) -> CmdResult {
        CmdResult::Cancel
    }

    /// Returns `true` when the command wants object picks via Tangent snap.
    fn needs_tangent_pick(&self) -> bool {
        false
    }

    /// If this command is XATTACH, returns the file path to attach.
    /// Default: None.
    fn xattach_path(&self) -> Option<String> {
        None
    }

    /// If this command needs attribute data injected (ATTEDIT), returns the
    /// INSERT handle awaiting attr initialization; else None.
    fn attedit_pending_handle(&self) -> Option<acadrust::Handle> {
        None
    }

    /// Inject attribute (tag, value) pairs into the command after entity pick.
    fn attedit_set_attrs(&mut self, _attrs: Vec<(String, String)>) {}

    /// Inject attribute definitions (tag, prompt, default_value) for ATTREQ
    /// attr-filling after INSERT point is picked.
    fn attreq_set_attdefs(&mut self, _attdefs: Vec<(String, String, String)>) {}

    /// Returns the INSERT entity built so far (pending attr fill) if this is an
    /// ATTREQ-aware INSERT command waiting for attdef injection.
    /// Called by the host after `AttreqNeeded` to commit the completed Insert.
    fn attreq_take_insert(&mut self) -> Option<acadrust::EntityType> {
        None
    }

    /// Called instead of `on_point` when the command needs a tangent pick
    /// and the snap system found a tangent object.
    fn on_tangent_point(&mut self, obj: TangentObject, hit: DVec3) -> CmdResult {
        let _ = obj;
        self.on_point(hit)
    }

    /// When true, `update.rs` injects the picked entity before calling
    /// `on_entity_pick` (required when the pick handler reads injected state).
    fn inject_before_entity_pick(&self) -> bool {
        false
    }

    /// Called by update.rs to inject the cloned entity into commands
    /// that need to read/modify it (e.g. DIMTEDIT, MLEADERADD, MLEADERREMOVE).
    /// Default: no-op.
    fn inject_picked_entity(&mut self, _entity: acadrust::EntityType) {}

    /// What the command is asking for at this step, used to label the
    /// dynamic-input overlay. Default is a point pick; commands waiting
    /// on a radius/length return `Distance` and angle prompts return
    /// `Angle`.
    fn dyn_field(&self) -> DynField {
        DynField::Point
    }

    /// When true, a value typed into the dynamic-input box for this step is
    /// committed via `on_text_input` (as a string the command parses) rather
    /// than resolved into a point. Used by steps whose typed value is a span /
    /// included angle / length the command interprets itself (e.g. ARC angle
    /// modes), while the box still previews a live value from the cursor.
    fn dyn_commit_as_text(&self) -> bool {
        false
    }

    /// Explicit per-step dynamic-input description. `Some(spec)` takes full
    /// control of the boxes, guide geometry and anchor for this step; `None`
    /// (the default) falls back to the legacy `dyn_field()` behaviour so
    /// commands that haven't migrated keep working unchanged.
    fn dyn_spec(&self) -> Option<DynSpec> {
        None
    }

    /// Live value for the dynamic-input scalar box, derived from the cursor
    /// world position. Lets a command drive a typed prompt by mouse — e.g.
    /// OFFSET returns the perpendicular distance from the cursor to the
    /// object being offset, so moving the cursor fills in the distance.
    /// Returns `None` when the value can only be typed (a count, or a
    /// distance with no reference yet). The string the host commits is this
    /// value formatted; the command's own `on_text_input` parses it back.
    fn dyn_live_value(&self, _cursor: DVec3) -> Option<f64> {
        None
    }
}

// ── Autocomplete registry ─────────────────────────────────────────────────
//
// Every `impl CadCommand for Foo` module submits the names it answers to
// at compile time via `inventory::submit!`. The command-line autocomplete
// then iterates the resulting collection at runtime — no central list to
// keep in sync.
//
// Non-interactive one-shot dispatch arms (NEW, OPEN, SAVE, …) live in
// `app/commands.rs` and don't have a `CadCommand` impl; they're absent
// from autocomplete by design. Add an explicit `inventory::submit!` next
// to their dispatch arm if you want them surfaced.

pub struct CommandRegistration {
    pub names: &'static [&'static str],
}

inventory::collect!(CommandRegistration);

/// All registered command names, including aliases.
pub fn all_registered_command_names() -> Vec<&'static str> {
    inventory::iter::<CommandRegistration>
        .into_iter()
        .flat_map(|r| r.names.iter().copied())
        .collect()
}
