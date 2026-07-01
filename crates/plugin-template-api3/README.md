# API v3 Panel Template Plugin

This crate is a reference implementation of a modern Open CAD Studio plugin
built against API v3. It demonstrates panels, asynchronous host→plugin events,
asynchronous plugin→host events, ribbon contribution, the full dispatch
surface, and the new API v3 entity/tab operations — without relying on any API
v2 compatibility paths.

## What it shows

| Feature | Location |
|---|---|
| API v3 registration with `export_plugin!` | `src/lib.rs` |
| Ribbon group and tool definition | `src/lib.rs` (`TemplateModule`) |
| Panel declaration | `BuiltinPlugin::panels()` |
| Opening a panel from `dispatch()` | `API3_OPEN` handler |
| Updating panel widgets asynchronously | `PluginAsync::PanelUpdate` in `refresh_panel()` |
| Reacting to user input | `HostAsync::PanelEvent` handling |
| Document lifecycle events | `DocumentActivated/Changed/TabClosed` handlers |
| Command-line output | `push_info` / `push_output` |
| Dirty + undo | `set_dirty` and `push_undo` |
| Closing a panel | `PanelEvent::Closed` handling |
| Coordinate pick round-trip | `request_point_pick` + `CoordinatesPicked` |
| Text input to command line | `TextInput` widget + `push_output` |
| Add entity via `HostApi::add_entity` | `API3_ADD_POINT` handler / panel button |
| Remove entity via `HostApi::remove_entity` | `API3_REMOVE_LAST` handler / panel button |
| Switch active tab via `HostApi::set_active_tab` | `API3_SWITCH_TAB` handler / panel button |

## New API v3 methods demonstrated

### `HostApi::add_entity`

Creates a new entity in the active document and returns its handle. The
template adds a point at the origin.

### `HostApi::remove_entity`

Removes an entity by handle. The template removes the last entity in the
document.

### `HostApi::set_active_tab`

Switches the host's active document tab. This is mainly useful for
out-of-process plugins that queue host requests for a specific tab; the
template shows a simple switch to tab 0.

## Files

| File | Purpose |
|---|---|
| `src/lib.rs` | Plugin implementation and `MANIFEST` |
| `plugin.toml` | Metadata read by the host installer/discovery (keep in sync with `MANIFEST`) |
| `tests/integration.rs` | End-to-end integration tests |

## Building

```powershell
cargo build -p plugin-template-api3
```

The artifact is a `cdylib` named `plugin_template_api3.dll` (Windows) or
`libplugin_template_api3.so` (Linux).

## Running the tests

The integration tests build the `ocs_plugin_runner` binary, spawn the plugin
process, and exercise every RPC and async step end-to-end.

```powershell
$env:CARGO_TARGET_DIR = 'C:\tmp\ocs_target'
cargo test -p plugin-template-api3 --test integration
```

Tests run with the `host` feature of `ocs_plugin_api` enabled.

## Manual loading in the host

1. Build the crate: `cargo build -p plugin-template-api3`.
2. Copy `target/debug/plugin_template_api3.dll` to
   `%APPDATA%\OpenCADStudio\plugins\ocs.template.api3\plugin-template-api3-windows-x86_64.dll`.
3. Launch the host and the plugin runner in separate terminals:
   - `cargo run -p OpenCADStudio`
   - `cargo run -p ocs_plugin_runner`
4. Click **Template → Open Template** in the ribbon.
5. Use the panel buttons to add/remove points or switch tabs.
