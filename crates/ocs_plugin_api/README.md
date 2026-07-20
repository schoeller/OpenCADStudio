# ocs_plugin_api

Dual-use crate that defines the OpenCAD Studio plugin contract and the
out-of-process plugin runtime.

- **Plugin side:** stable API surface (`BuiltinPlugin`, `HostApi`, `CadModule`,
  `export_plugin!`) that cdylib plugins compile against.
- **Host side:** IPC protocol, transport, and process management used by the
  host and the plugin runner.

## API versions

### V2 (`BuiltinPlugin`)

The legacy API surface (the first three methods of `BuiltinPlugin`):

- `manifest()` — static plugin metadata.
- `ribbon()` — ribbon tab/group/tool definitions.
- `dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool` — command
  handler.

V2 plugins have no panels, async events, document reader, interactive object
pick, or entity removal. They are still supported through `V2ToV3Adapter`. A v2
cdylib reports `ocs_plugin_api_version() == 2` and exports `*mut Box<dyn
BuiltinPlugin>`; the runner loads it through the adapter so the v3 `BuiltinPlugin`
methods that a v2 cdylib does not implement are safely masked as no-ops.

The original v2 `CadModule` returned `Vec<RibbonGroup>` from `ribbon_groups()`. The
runner uses the `CadModuleV2` trait and `V2CadModuleAdapter` to call that legacy
convention, then exposes the data as the current `CadModule` slice API. Existing
v2 cdylibs that were built with the old `Vec` ABI can therefore be loaded without
recompilation.

### V3 (`BuiltinPlugin`)

The current API surface. It extends V2 with:

- `panels()` — dockable/floating plugin panels.
- `on_async_event(&mut self, host: &mut dyn HostApi, event: HostAsync)` —
  host→plugin async events (panel events, document lifecycle, coordinate picks).
- `send_async(&mut self, event: PluginAsync)` — plugin→host async events
  (panel updates).
- `document_reader(&self)` — zero-copy read-only document view.
- `document_view(&mut self)` — shared-memory document view for out-of-process
  plugins.
- `request_point_pick(&mut self, panel_id: &str)` — start interactive point
  pick.
- `set_active_tab(&mut self, tab: usize)` — switch the host's active document
  tab (mainly for out-of-process plugins).
- `remove_entity(&mut self, handle: Handle) -> Option<EntityType>` — remove an
  entity from the active document.

## Core traits

### `BuiltinPlugin`

The main plugin entry point. Implement this and export it with
`export_plugin!(PluginType::new())`.

```rust
pub trait BuiltinPlugin: Send + Sync {
    fn manifest(&self) -> &'static PluginManifest;
    fn ribbon(&self) -> Box<dyn CadModule>;
    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool;
    fn panels(&self) -> Vec<PanelDef> { Vec::new() }
    fn on_async_event(&mut self, host: &mut dyn HostApi, event: HostAsync);
}
```

### `HostApi`

The runtime surface the plugin uses while dispatching commands or handling
async events. It covers document access, entity creation/removal, XDATA,
undo/dirty, command-line output, panels, and async communication.

See `src/host.rs` for the full trait definition.

### `CadModule`

Defines ribbon tabs, groups, and tools. Use `export_plugin!` to register the
plugin; the host queries `ribbon()` once at load time.

## IPC protocol

When a plugin is loaded out-of-process, all `HostApi` calls are serialized over
`interprocess::local_socket` (named pipes on Windows, Unix domain sockets
elsewhere) as length-framed `bincode` messages.

Key message enums:

- `HostRequest` / `HostResponse` — host asks the plugin for manifest, ribbon,
  dispatch, panels, etc.
- `PluginRequest` / `PluginResponse` — plugin asks the host to mutate the
  document or UI.
- `HostAsync` / `PluginAsync` — async events in both directions.

See `src/ipc/protocol.rs` for all variants.

## Out-of-process runtime

- `process::PluginProcess` — spawns a plugin process and manages its socket.
- `process::PluginManager` — discovers, loads, supervises, and unloads plugins.
- `ipc::client::PluginHostApi` — plugin-side `HostApi` proxy.
- `ipc::server::serve_plugin_connection` — host-side request handler.
- `runner` — plugin runner invoked by the host executable in runner mode.

## V2 to V3 adapter

`V2ToV3Adapter` wraps a v2 cdylib's `Box<dyn BuiltinPlugin>` so it satisfies
`BuiltinPlugin`. V2-only methods (`panels`, `on_async_event`) are no-ops, which
masks the incomplete v2 vtable without recompiling the plugin. `V2CadModuleAdapter`
wraps the old `CadModuleV2` `Vec<RibbonGroup>` return convention so it satisfies
the current `CadModule` slice contract.

## Version constants

- `API_VERSION` — major API version; checked before loading a cdylib.
- `ABI_REVISION` — ABI revision within the major version; checked after the
  major version so stale v3 cdylibs are rejected.

## Testing

```powershell
$env:CARGO_TARGET_DIR = 'C:\tmp\ocs_target'
cargo test -p ocs_plugin_api --features host --lib
```

The crate uses the `host` feature to gate IPC, process management, and the
`HostApi` runtime surface.
