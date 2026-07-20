//! Runtime host surface (`host` feature).
//!
//! [`HostApi`] is the `acadrust`-typed adapter a plugin uses at *dispatch* time
//! — document access, entity creation, XDATA, undo, and the command line. It is
//! the stable counterpart to the dependency-free manifest/ribbon contract: a
//! plugin's `dispatch` receives `&mut dyn HostApi` rather than the host's
//! concrete session type, so an out-of-tree add-on compiles against this crate
//! alone.
//!
//! Per-tab plugin state is keyed by `manifest.id`. The trait exposes it in an
//! object-safe `Any` form; use the [`plugin_state`], [`plugin_state_mut`] and
//! [`ensure_plugin_state`] helpers for the ergonomic typed access.

use std::any::Any;

use acadrust::xdata::ExtendedDataRecord;
use acadrust::{CadDocument, EntityType, Handle};

use crate::ipc::protocol::PluginAsync;
use crate::manifest::PluginManifest;
use crate::panel::{DockZone, PanelDef, PanelError, PanelEvent, PanelHandle};
use crate::ribbon::{CadModule, RibbonGroup};

/// An add-on package's entry point: its manifest, optional ribbon tab, and
/// command dispatch. Built-in (in-tree) and dynamically-loaded (cdylib) plugins
/// implement the same trait from this crate, so an out-of-tree add-on targets
/// the stable contract rather than the host binary.
pub trait BuiltinPlugin: Send + Sync {
    fn manifest(&self) -> &'static PluginManifest;
    fn ribbon(&self) -> Box<dyn CadModule>;
    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool;

    /// Panels declared by this plugin (API v3). Default: none.
    fn panels(&self) -> Vec<PanelDef> {
        Vec::new()
    }
    /// Asynchronous host event delivered to the plugin (API v3). Default: no-op.
    fn on_async_event(&mut self, _host: &mut dyn HostApi, _event: crate::ipc::protocol::HostAsync) {
    }
}

/// API v2 plugin surface. The v2 subset is the first three methods of
/// `BuiltinPlugin` (manifest, ribbon, dispatch). V2 cdylibs report
/// `ocs_plugin_api_version() == 2` and export `*mut Box<dyn BuiltinPlugin>`. The
/// runner loads them through `V2ToV3Adapter`, which supplies the default no-op
/// implementations for v3-only methods (`panels`, `on_async_event`) so that old
/// plugins do not need to be recompiled. Plugins may also use this trait
/// explicitly if they prefer a separate v2-only contract.
pub trait BuiltinPluginV2: Send + Sync {
    fn manifest(&self) -> &'static PluginManifest;
    fn ribbon(&self) -> Box<dyn CadModuleV2>;
    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool;
}

/// Legacy API v2 `CadModule` surface. Before API v3, `CadModule::ribbon_groups`
/// returned `Vec<RibbonGroup>` by value. V2 cdylibs compiled against that ABI
/// return `Box<dyn CadModule>` whose vtable uses the old signature. The runner
/// transmutes that trait object to this v2-compatible trait so it can call the
/// plugin's real `ribbon_groups()` without crashing.
pub trait CadModuleV2: Send + Sync {
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn ribbon_groups(&self) -> Vec<RibbonGroup>;
}

/// Wraps the `Vec<RibbonGroup>` produced by an old v2 `CadModule` so it
/// satisfies the current `CadModule` contract (`ribbon_groups` returns a slice).
/// The data is leaked so the returned slice is valid for the adapter's lifetime.
pub struct V2CadModuleAdapter {
    id: &'static str,
    title: &'static str,
    groups: &'static [RibbonGroup],
}

impl V2CadModuleAdapter {
    pub fn from_v2(v2: Box<dyn CadModuleV2>) -> Box<dyn CadModule> {
        // Safe because the concrete type behind the v2 trait object is also
        // Send + Sync (CadModule requires it) and we leak the data to give the
        // slice a static lifetime.
        let id = v2.id();
        let title = v2.title();
        let groups: &'static [RibbonGroup] = Box::leak(v2.ribbon_groups().into_boxed_slice());
        Box::new(Self { id, title, groups })
    }
}

impl CadModule for V2CadModuleAdapter {
    fn id(&self) -> &'static str {
        self.id
    }
    fn title(&self) -> &'static str {
        self.title
    }
    fn ribbon_groups(&self) -> &[RibbonGroup] {
        self.groups
    }
}

/// Adapter that wraps an API v2 plugin (exported as `Box<dyn BuiltinPlugin>`)
/// so it satisfies the API v3 `BuiltinPlugin` trait. New v3 methods (`panels`,
/// `on_async_event`) are left as the built-in no-op defaults, masking any
/// missing/incomplete entries in the v2 vtable.
pub struct V2ToV3Adapter(pub Box<dyn BuiltinPlugin>);

impl BuiltinPlugin for V2ToV3Adapter {
    fn manifest(&self) -> &'static PluginManifest {
        self.0.manifest()
    }
    fn ribbon(&self) -> Box<dyn CadModule> {
        // V2 cdylibs were compiled when `CadModule::ribbon_groups` returned
        // `Vec<RibbonGroup>`. Treat the returned trait object as `dyn
        // CadModuleV2` (same vtable layout) so the call uses the old ABI, then
        // convert the result to the current `CadModule` contract.
        let v2: Box<dyn CadModuleV2> = unsafe { std::mem::transmute(self.0.ribbon()) };
        V2CadModuleAdapter::from_v2(v2)
    }
    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        self.0.dispatch(host, cmd)
    }
}

/// A point-driven interactive command a plugin starts via
/// [`HostApi::start_interactive`]. The host shows the prompt, collects points —
/// clicked in the viewport, or fed as coordinates over the `--serve` automation
/// API — and commits the entities the command yields, exactly like a built-in
/// tool. This is the plugin-facing slice of the host's command machinery; it
/// covers click-to-place placement without exposing the host's internal command
/// trait.
pub trait InteractiveCommand: Send {
    /// Prompt for the next point.
    fn prompt(&self) -> String;
    /// A point was supplied (clicked or typed `x,y[,z]`). Returns the next step.
    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep;
    /// Enter pressed with no point — e.g. to finish a multi-point command.
    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }

    /// When `true`, the next input picks an existing **entity** (the user clicks
    /// on it; over `--serve`, a handle is supplied) rather than a free point —
    /// the host then calls [`on_object_pick`](Self::on_object_pick). Use this to
    /// reference existing geometry (e.g. connect a pipe between two structures).
    fn needs_object_pick(&self) -> bool {
        false
    }
    /// An existing entity was picked: its `handle` and the pick point. Read the
    /// entity's data (XDATA / geometry) via `HostApi`, keyed by the handle.
    fn on_object_pick(&mut self, _handle: Handle, _pt: [f64; 3]) -> CommandStep {
        CommandStep::Cancel
    }
}

/// The outcome of an [`InteractiveCommand`] step.
#[derive(Debug)]
#[cfg_attr(feature = "host", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandStep {
    /// Need another point; keep the command active.
    NeedPoint,
    /// Commit an entity to the document and keep collecting points.
    Commit(EntityType),
    /// Commit an entity and end the command.
    CommitAndEnd(EntityType),
    /// End the command without committing.
    Done,
    /// Cancel the command.
    Cancel,
}

/// Export a `BuiltinPlugin` from a `cdylib` so the host can load it at runtime.
///
/// Emits the C symbols the loader looks for:
/// - `ocs_plugin_api_version` (checked before anything else, so an ABI-incompatible
///   build is rejected without running its code),
/// - `ocs_plugin_abi_revision` (for API v3, checked after the major version so
///   old v3 cdylibs with a stale trait layout are rejected),
/// - `ocs_plugin_register` (constructs the plugin and hands ownership to the host
///   as a boxed trait object).
///
/// ```ignore
/// ocs_plugin_api::export_plugin!(MyPlugin::new());
/// ```
#[macro_export]
macro_rules! export_plugin {
    ($ctor:expr) => {
        #[no_mangle]
        pub extern "C" fn ocs_plugin_api_version() -> u32 {
            $crate::API_VERSION
        }

        #[no_mangle]
        pub extern "C" fn ocs_plugin_abi_revision() -> u64 {
            $crate::ABI_REVISION
        }

        #[no_mangle]
        pub extern "C" fn ocs_plugin_register(
        ) -> *mut ::std::boxed::Box<dyn $crate::host::BuiltinPlugin> {
            // The constructor runs across a C ABI boundary; a panic unwinding
            // past it is undefined behavior. Contain it and report failure as a
            // null pointer, which the host loader treats as "registration
            // failed" rather than crashing the runner process.
            match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                let plugin: ::std::boxed::Box<dyn $crate::host::BuiltinPlugin> =
                    ::std::boxed::Box::new($ctor);
                ::std::boxed::Box::into_raw(::std::boxed::Box::new(plugin))
            })) {
                ::std::result::Result::Ok(ptr) => ptr,
                ::std::result::Result::Err(_) => ::std::ptr::null_mut(),
            }
        }
    };
}

/// The plugin-facing runtime surface for one active document tab.
pub trait HostApi {
    /// Index of the tab this session targets.
    fn tab_index(&self) -> usize;

    // ── Document ────────────────────────────────────────────────────────────
    fn document(&self) -> &CadDocument;
    /// Mutable access to the active document.
    ///
    /// For an **out-of-process** plugin this borrows a *local snapshot*: edits
    /// to existing entities made through it are NOT sent back to the host and
    /// are silently discarded. To modify or delete entities from any plugin,
    /// use [`add_entity`](Self::add_entity), [`update_entity`](Self::update_entity)
    /// and [`remove_entity`](Self::remove_entity), which are committed to the
    /// host document over IPC.
    fn document_mut(&mut self) -> &mut CadDocument;

    /// Add an entity to the active document, returning its handle.
    fn add_entity(&mut self, entity: EntityType) -> Handle;
    /// Replace the existing entity that carries `entity`'s handle, preserving
    /// its identity (handle and owning block). Returns `false` when no entity
    /// has that handle. This is the sanctioned way to commit in-place edits
    /// from an out-of-process plugin — mutating `document_mut()` does not work
    /// across the process boundary.
    fn update_entity(&mut self, entity: EntityType) -> bool {
        let handle = entity.common().handle;
        match self.document_mut().get_entity_mut(handle) {
            Some(slot) => {
                *slot = entity;
                true
            }
            None => false,
        }
    }
    /// Delete the entity with `handle` (and any derived render caches). Returns
    /// `true` when an entity was removed.
    fn remove_entity(&mut self, handle: Handle) -> bool {
        self.document_mut().remove_entity(handle).is_some()
    }

    // ── XDATA ───────────────────────────────────────────────────────────────
    /// Mark the scene geometry dirty so it is re-tessellated next frame.
    fn bump_geometry(&mut self);
    /// Read the XDATA record for `app_name` on entity `handle`, if any.
    fn read_record(&self, handle: Handle, app_name: &str) -> Option<&ExtendedDataRecord>;
    /// Attach `record` to entity `handle`, replacing any existing record for the
    /// same application and registering the APPID. Returns `false` if the entity
    /// does not exist.
    fn write_record(&mut self, handle: Handle, record: ExtendedDataRecord) -> bool;
    /// Remove the XDATA record for `app_name` from entity `handle`. Returns
    /// `true` if a record was removed.
    fn remove_record(&mut self, handle: Handle, app_name: &str) -> bool;

    // ── Undo / dirty ────────────────────────────────────────────────────────
    fn push_undo(&mut self, label: &str);
    fn set_dirty(&mut self);

    // ── Command line ────────────────────────────────────────────────────────
    fn push_info(&mut self, msg: &str);
    fn push_output(&mut self, msg: &str);
    fn push_error(&mut self, msg: &str);

    /// Start a plugin-defined interactive (click-to-place) command on the active
    /// tab. The host drives it through its normal point-collection flow.
    fn start_interactive(&mut self, command: Box<dyn InteractiveCommand>);
    // ── Per-tab plugin state (object-safe; use the typed helpers below) ──────
    fn plugin_state_any(&self, plugin_id: &str) -> Option<&(dyn Any + Send + Sync)>;
    fn plugin_state_any_mut(&mut self, plugin_id: &str) -> Option<&mut (dyn Any + Send + Sync)>;
    /// Get the state for `plugin_id`, inserting `init()`'s result if absent.
    fn ensure_plugin_state_any(
        &mut self,
        plugin_id: &'static str,
        init: &mut dyn FnMut() -> Box<dyn Any + Send + Sync>,
    ) -> &mut (dyn Any + Send + Sync);
    // ── DocumentReader (added in API v3; appended at the end to keep vtable
    // indices stable for API v2 plugins) ─────────────────────────────────────

    /// Read-only, zero-copy view of the active document. For out-of-process
    /// plugins this is backed by host-owned shared memory; for in-process
    /// plugins it wraps `document()`.
    fn document_reader(&self) -> Box<dyn DocumentReader + '_>;

    /// Open (or refresh) the host-side shared document view and return the
    /// information the plugin needs to map it. In-process hosts implement this;
    /// out-of-process plugin proxies return `None`.
    fn document_view(&mut self) -> Option<crate::shm::DocumentViewInfo> {
        None
    }
    /// Set the active document tab for subsequent host operations (API v3).
    /// Out-of-process hosts route this as a `PluginRequest::SetActiveTab`;
    /// in-process hosts that do not support tab switching can leave the default
    /// error implementation. V2 hosts therefore keep compiling without changes.
    fn set_active_tab(&mut self, _tab: usize) -> Result<(), String> {
        Err("set_active_tab requires an out-of-process plugin host".to_string())
    }
    // ── Panels (API v3; default implementations return Unsupported for v2 hosts) ─
    /// Open (or refresh) a plugin panel and return a host-allocated handle.
    fn open_panel(&mut self, _def: &PanelDef) -> Result<PanelHandle, PanelError> {
        Err(PanelError::Unsupported)
    }
    /// Close a previously opened plugin panel.
    fn close_panel(&mut self, _handle: PanelHandle) -> Result<(), PanelError> {
        Err(PanelError::Unsupported)
    }
    /// Move an open panel to logical window coordinates `(x, y)`.
    fn move_panel(&mut self, _handle: PanelHandle, _x: f32, _y: f32) -> Result<(), PanelError> {
        Err(PanelError::NotImplemented)
    }
    /// Resize an open panel. Values are clamped to the panel's minimum size.
    fn resize_panel(
        &mut self,
        _handle: PanelHandle,
        _width: f32,
        _height: f32,
    ) -> Result<(), PanelError> {
        Err(PanelError::NotImplemented)
    }
    /// Dock an open panel to `zone`.
    fn dock_panel(&mut self, _handle: PanelHandle, _zone: DockZone) -> Result<(), PanelError> {
        Err(PanelError::Unsupported)
    }
    /// Undock an open panel and place it at logical window coordinates `(x, y)`.
    fn undock_panel(&mut self, _handle: PanelHandle, _x: f32, _y: f32) -> Result<(), PanelError> {
        Err(PanelError::Unsupported)
    }
    /// Forward a user-generated panel event to the plugin.
    fn post_panel_event(
        &mut self,
        _handle: PanelHandle,
        _event: PanelEvent,
    ) -> Result<(), PanelError> {
        Err(PanelError::Unsupported)
    }

    /// Send an asynchronous plugin event to the host (API v3). In-process hosts
    /// and hosts that do not support panels can leave this as a no-op.
    fn send_async(&mut self, _event: PluginAsync) {}

    /// Request a point pick from the host (API v3). The host will start its
    /// normal point-collection flow and deliver the result back to the plugin
    /// via `HostAsync::CoordinatesPicked`. Out-of-process hosts route this as a
    /// `PluginRequest::RequestPointPick`; in-process hosts that do not support
    /// point picks can leave the default error implementation.
    fn request_point_pick(&mut self, _panel_id: &str) -> Result<(), String> {
        Err("point pick requires an out-of-process plugin host".to_string())
    }

    /// Set the plugin process currently dispatching a request on this host.
    /// Used by the host to route panel open/close/event operations back to the
    /// owning process. Default no-op.
    fn set_current_process(
        &mut self,
        _process: Option<std::sync::Arc<crate::process::PluginProcess>>,
    ) {
    }

    /// Current plugin process dispatching on this host, if any.
    fn current_process(&self) -> Option<std::sync::Arc<crate::process::PluginProcess>> {
        None
    }
}

/// Simplified, read-only entity kind exposed by [`DocumentReader`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderEntityKind {
    Point,
    Line,
    Circle,
    Arc,
    Polyline,
    Text,
    Other,
}

/// A 3D point returned by [`DocumentReader`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReaderPoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A read-only view of one entity, borrowed from a [`DocumentReader`].
pub struct ReaderEntity<'a> {
    /// Entity handle in the host document.
    pub handle: Handle,
    /// Simplified entity type.
    pub kind: ReaderEntityKind,
    /// Name of the layer the entity lives on.
    pub layer_name: &'a str,
    /// If the entity is a point, its coordinates.
    pub point: Option<ReaderPoint>,
}

/// Read-only, zero-copy view of a CAD document.
///
/// For out-of-process plugins this is backed by host-owned shared memory. The
/// plugin receives only references into that mapping, so the document model is
/// not copied into the plugin's heap.
pub trait DocumentReader {
    /// Total number of entities in the document.
    fn entity_count(&self) -> usize;

    /// Iterate over all entities without allocating a full `CadDocument`.
    fn for_each_entity(&self, f: &mut dyn FnMut(ReaderEntity<'_>));

    /// Look up a layer name by handle.
    fn layer_name(&self, handle: Handle) -> Option<&str>;

    /// Look up an APPID name by handle.
    fn app_id_name(&self, handle: Handle) -> Option<&str>;
}

impl ReaderEntityKind {
    /// Map a concrete `EntityType` to the simplified reader kind.
    pub fn from_entity(entity: &EntityType) -> Self {
        match entity {
            EntityType::Point(_) => ReaderEntityKind::Point,
            EntityType::Line(_) => ReaderEntityKind::Line,
            EntityType::Circle(_) => ReaderEntityKind::Circle,
            EntityType::Arc(_) => ReaderEntityKind::Arc,
            EntityType::Polyline(_)
            | EntityType::Polyline2D(_)
            | EntityType::Polyline3D(_)
            | EntityType::LwPolyline(_) => ReaderEntityKind::Polyline,
            EntityType::Text(_) | EntityType::MText(_) => ReaderEntityKind::Text,
            _ => ReaderEntityKind::Other,
        }
    }
}

/// In-process `DocumentReader` implementation that wraps a borrowed `CadDocument`.
pub struct CadDocumentReader<'a>(pub &'a CadDocument);

impl<'a> DocumentReader for CadDocumentReader<'a> {
    fn entity_count(&self) -> usize {
        self.0.entities().count()
    }

    fn for_each_entity(&self, f: &mut dyn FnMut(ReaderEntity<'_>)) {
        for entity in self.0.entities() {
            let kind = ReaderEntityKind::from_entity(entity);
            let layer_name = entity.common().layer.as_str();
            let point = match entity {
                EntityType::Point(p) => Some(ReaderPoint {
                    x: p.location.x,
                    y: p.location.y,
                    z: p.location.z,
                }),
                _ => None,
            };
            f(ReaderEntity {
                handle: entity.common().handle,
                kind,
                layer_name,
                point,
            });
        }
    }

    fn layer_name(&self, handle: Handle) -> Option<&str> {
        self.0
            .layers
            .iter()
            .find(|layer| layer.handle == handle)
            .map(|layer| layer.name.as_str())
    }

    fn app_id_name(&self, handle: Handle) -> Option<&str> {
        self.0
            .app_ids
            .iter()
            .find(|app_id| app_id.handle == handle)
            .map(|app_id| app_id.name.as_str())
    }
}

/// Typed read of per-tab plugin state stored under `plugin_id`.
pub fn plugin_state<'a, T: Any + Send + Sync>(
    host: &'a dyn HostApi,
    plugin_id: &str,
) -> Option<&'a T> {
    host.plugin_state_any(plugin_id)?.downcast_ref::<T>()
}

/// Typed mutable access to per-tab plugin state stored under `plugin_id`.
pub fn plugin_state_mut<'a, T: Any + Send + Sync>(
    host: &'a mut dyn HostApi,
    plugin_id: &str,
) -> Option<&'a mut T> {
    host.plugin_state_any_mut(plugin_id)?.downcast_mut::<T>()
}

/// Typed get-or-insert of per-tab plugin state stored under `plugin_id`.
pub fn ensure_plugin_state<'a, T: Any + Send + Sync>(
    host: &'a mut dyn HostApi,
    plugin_id: &'static str,
    init: impl FnOnce() -> T,
) -> &'a mut T {
    let mut init = Some(init);
    let any = host.ensure_plugin_state_any(plugin_id, &mut || {
        Box::new((init.take().expect("init called once"))())
    });
    any.downcast_mut::<T>()
        .expect("plugin state type mismatch for plugin_id")
}
