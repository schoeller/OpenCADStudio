# Open CAD Studio — Plugin Architecture

**Status:** Accepted (phase 1)  
**Author:** Open CAD Studio contributors  
**Date:** June 2026

This document is the **authoritative spec** for how add-on packages integrate with Open CAD Studio. It follows patterns familiar from [QGIS](https://plugins.qgis.org/) and other open-source extensibility models: a small metadata file, a single entry-point registration, optional separate engine crate, and user-installable packages in a later phase.

> **Scope:** Generic host runtime only (`src/plugin/`, `src/app/plugin_host.rs`). Domain plugins (Storm Sewer, future sanitary/geotech) live under `src/modules/<name>/` and optional `crates/<engine>/`. They **consume** this API; they are **not** part of the framework source.

---

## Design goals

| Goal | Rationale |
|------|-----------|
| **One package, one registration** | Ribbon tab, commands, and manifest ship together — no duplicate hooks in `build.rs` and `commands.rs`. |
| **Stable host surface** | Plugin authors target the semver-versioned `ocs_plugin_api` crate (manifest + ribbon today) and `HostSession`, not `OpenCADStudio` internals. |
| **Open-source add-on ergonomics** | Separate git repo + workspace crate is supported; in-tree built-ins use the same layout. |
| **DWG round-trip** | Domain data on entities (XDATA), not opaque plugin databases. |
| **Engine reuse** | Headless crates (`stormsewer`, …) run in WASM/CLI without the CAD host. |

## Non-goals (phase 1)

- Sandboxed scripting (Python/Lua).
- Replacing the `acadrust` entity model.
- Full Autodesk CUI XML import.

---

## Three layers (do not mix)

```
┌────────────────────────────────────────────────────────────────────┐
│  Layer A — Host core                                                │
│  iced UI · Scene · Document · Undo · Command line                   │
│  Built-in ribbon tabs: Home, Model, View, … (NOT plugins)           │
└───────────────────────────────┬────────────────────────────────────┘
                                │ HostSession (stable adapter)
┌───────────────────────────────▼────────────────────────────────────┐
│  Layer B — Add-on plugin package                                    │
│  plugin.toml · manifest.rs · register.rs · plugin.rs · dispatch.rs │
│  Optional ribbon (CadModule) · per-tab state · XDATA schemas        │
└───────────────────────────────┬────────────────────────────────────┘
                                │ pure Rust API
┌───────────────────────────────▼────────────────────────────────────┐
│  Layer C — Domain engine crate (optional)                           │
│  crates/stormsewer — hydraulics, IO, no iced/acadrust dependency    │
└────────────────────────────────────────────────────────────────────┘
```

| Layer | Examples | May depend on |
|-------|----------|---------------|
| **A — Host core** | `src/app/`, `src/ui/`, `src/modules/home/` | Everything in the app |
| **B — Plugin package** | `src/modules/storm_sewer/` | Host + optional engine crate |
| **C — Engine** | `crates/stormsewer/` | `std` only (target: WASM/CLI too) |

**Hard rules**

1. `src/plugin/` must **not** import any domain module (`storm_sewer`, …).
2. Engine crates must **not** import `iced`, `acadrust`, or `OpenCADStudio`.
3. Add-on plugins must **not** edit `src/app/commands.rs` for new commands.

---

## Comparison to QGIS

| QGIS | Open CAD Studio |
|------|-----------------|
| `metadata.txt` (name, version, author, …) | `plugin.toml` beside the package |
| `classFactory(iface)` in `__init__.py` | `inventory::submit!(PluginRegistration { construct })` in `register.rs` |
| `iface` stable API | `ocs_plugin_api` crate (manifest + ribbon) + `HostSession` |
| User folder `…/python/plugins/<id>/` | Phase 2: `%APPDATA%/OpenCADStudio/plugins/<id>/` |
| Plugin repository (plugins.qgis.org) | Future: curated index; today = git + in-tree |
| `qgisMinimumVersion` | `api_version` in manifest (host ABI major) |
| Processing algorithms | Headless engine crates + `SS_ANALYZE`-style commands |
| Vector layer provider | XDATA on DWG entities (`STORMSEWER_*`) |

QGIS separates **core application** from **Python plugins** loaded at runtime. Open CAD Studio phase 1 compiles add-ons **in-tree** (same ergonomics, static linking). Phase 2 adds dynamic `.dll`/`.so` with the **same** `plugin.toml` and C ABI entry point.

---

## Add-on package layout

Every add-on — whether in-tree or external — uses this directory shape:

```
<plugin_id>/                    # e.g. storm_sewer or opencad-storm-sewer repo
  plugin.toml                   # human metadata (mirrors manifest.rs)
  PLUGIN.md                     # XDATA schemas, command reference
  register.rs                   # ONLY inventory::submit! — no domain logic
  plugin.rs                     # thin BuiltinPlugin impl
  manifest.rs                   # static PluginManifest (compile-time truth)
  dispatch.rs                   # command routing for this plugin
  state.rs                      # per-document tab state (optional)
  mod.rs                        # CadModule ribbon (if the plugin has a tab)
  icons/                        # SVG assets
  …                             # domain modules (data.rs, preview.rs, …)

crates/<engine>/                # optional, separate workspace member
  Cargo.toml
  src/
```

### `plugin.toml` (metadata file)

Source of truth for **humans and phase-2 loader**. Values must match `manifest.rs`.

```toml
[plugin]
id = "opencad.storm_sewer"
name = "Storm Sewer"
version = "0.2.0"
description = "Gravity storm-drain network design and analysis"
author = "Open CAD Studio contributors"
license = "GPL-3.0-only"
homepage = "https://github.com/…/storm-sewer"

[opencad]
api_version = 1
ribbon_order = 50
command_prefixes = ["SS_"]
xdata_apps = ["STORMSEWER_STRUCT", "STORMSEWER_PIPE", "STORMSEWER_CATCHMENT"]
```

**Discovery rule:** If `src/modules/<dir>/plugin.toml` exists, `build.rs` **excludes** that directory from the auto-generated ribbon registry. The tab is registered only via `BuiltinPlugin::ribbon()`.

---

## Host runtime API (phase 1)

### `PluginManifest`

```rust
pub struct PluginManifest {
    pub id: &'static str,              // reverse-DNS: "opencad.storm_sewer"
    pub name: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    pub api_version: ApiVersion,       // host ABI major; must match host
    pub ribbon_order: i32,             // sort key among add-on tabs
    pub xdata_apps: &'static [&'static str],
    pub command_prefixes: &'static [&'static str],
}
```

### `BuiltinPlugin`

```rust
pub trait BuiltinPlugin: Send + Sync {
    fn manifest(&self) -> &'static PluginManifest;
    fn ribbon(&self) -> Box<dyn CadModule>;
    fn dispatch(&self, host: &mut HostSession<'_>, cmd: &str) -> bool;
}
```

### Registration (single entry point)

```rust
// register.rs — keep this file free of domain logic
inventory::submit! {
    crate::plugin::registry::PluginRegistration {
        construct: || Box::new(MyPlugin),
    }
}
```

Host startup:

1. `inventory::iter::<PluginRegistration>` constructs all plugins.
2. `try_dispatch` routes commands before the legacy `commands.rs` match.
3. `all_ribbon_modules()` = core tabs from `build.rs` + plugin tabs sorted by `ribbon_order`.

### `HostSession` — plugin-facing surface

Plugins use `HostSession`, not `OpenCADStudio`:

| Category | Methods |
|----------|---------|
| Document | `document()`, `document_mut()`, `entities()`, `entities_mut()`, `add_entity()`, `bump_geometry()` |
| XDATA | `read_record(handle, app)`, `write_record(handle, record)`, `remove_record(handle, app)` — keyed by entity handle; `write_record` registers the APPID so the file stays standard |
| Tab state | `plugin_state()`, `plugin_state_mut()`, `ensure_plugin_state()` keyed by `manifest.id` |
| Command line | `push_info`, `push_output`, `push_error`, `set_active_command` |
| Undo / dirty | `push_undo`, `set_dirty` |

**Status:** The dependency-free half of the contract — `PluginManifest` /
`ApiVersion` (manifest) and `CadModule` + the ribbon types (`ToolDef`,
`RibbonGroup`, …) — now lives in the standalone, semver-versioned
[`crates/ocs_plugin_api`](../crates/ocs_plugin_api) crate. The host re-exports it
(`crate::plugin::manifest`, `crate::modules`) so in-tree paths are unchanged.
The `acadrust`-typed runtime surface in the table above (`document_mut`,
`add_entity`, `set_active_command`, …) stays in the host binary for now; lifting
it behind a `HostApi` trait in the same crate is the remaining phase-1b step.

### Command routing

```rust
// app/commands.rs — plugins run first
if crate::plugin::try_dispatch(self, tab_index, cmd) {
    return Task::none();
}
// … legacy core commands …
```

Plugins own:

- One-shot commands (`SS_ANALYZE`)
- Interactive acquisition (`SS_PIPE` → `PlacePipe`)
- Subcommands (`SS_PARAMS RP 25`)

Autocomplete: each plugin submits `inventory::submit!(CommandRegistration { names: &[…] })` in `mod.rs` or `register.rs`.

Interactive acquisition (C3D-style orange ObjectPick) stays in the **host** via generic `CadCommand` hooks — `resolve_object_pick`, `object_pick_hover_previews`, `entity_pick_acquire_previews` — so `app/update.rs` never imports domain modules.

### Per-document state

```rust
DocumentTab {
    plugin_state: HashMap<&'static str, Box<dyn Any + Send + Sync>>,
}
```

Store under `manifest.id` (e.g. `opencad.storm_sewer`), not ad hoc globals.

### XDATA contract

Domain persistence lives on entities. Document schemas in `PLUGIN.md`:

| App id | Owner | Purpose |
|--------|-------|---------|
| `STORMSEWER_STRUCT` | `opencad.storm_sewer` | Inlet / junction / outfall |
| `STORMSEWER_PIPE` | `opencad.storm_sewer` | Pipe link between structures |
| `STORMSEWER_CATCHMENT` | `opencad.storm_sewer` | Catchment boundary + hydrology |

`HostSession` provides `read_record` / `write_record` / `remove_record` helpers (keyed by entity handle) over the `acadrust` XDATA API; `write_record` also registers the application in the APPID table so the data survives a DWG/DXF round-trip. Plugins may still use the raw `acadrust` XDATA APIs directly.

---

## Core ribbon vs add-on ribbon

| Kind | Location | Registration |
|------|----------|--------------|
| **Core tab** | `src/modules/home/`, `view/`, … | `build.rs` auto-discovers `mod.rs` (no `plugin.toml`) |
| **Add-on tab** | `src/modules/storm_sewer/`, … | `plugin.toml` + `BuiltinPlugin::ribbon()` |

This mirrors QGIS: the application ships core menus; plugins add tabs/tools without patching the host binary.

---

## Phased rollout

### Phase 1 — Built-in add-ons (current)

- [x] `src/plugin/` runtime + `try_dispatch`
- [x] Per-tab `plugin_state`
- [x] Storm Sewer off `commands.rs` monolith
- [x] Single registration (`plugin.toml` + `BuiltinPlugin::ribbon`)
- [~] Extract `ocs_plugin_api` crate — manifest + ribbon/`CadModule` done; `acadrust`-typed host surface pending
- [x] Plugin manager UI (list installed, versions) — `PLUGINS` / `PLUGINMANAGER` command, or the Start-page "Plugins" button
- [x] Enable/disable plugins from the manager — a disabled plugin drops its ribbon tab and command dispatch; persisted in `settings.txt` (`disabled_plugins=`)
- [x] `ModuleEvent::PluginFileDialog` — a plugin tool requests a native file picker; the host opens it and dispatches `"<command> <path>"` back to the plugin with original case preserved (bypasses the command-line upper-casing)
- [x] XDATA convenience on `HostSession` — `read_record` / `write_record` / `remove_record`; `write_record` registers the APPID so plugin data round-trips through DWG/DXF

### Phase 2 — Dynamic loading (desktop)

```
%APPDATA%/OpenCADStudio/plugins/
  opencad.storm_sewer/
    plugin.toml
    opencad_storm_sewer.dll    # cdylib
```

- `libloading` + `#[no_mangle] extern "C" fn ocs_plugin_register() -> *const PluginVTable`
- `api_version` compatibility gate at load time
- [x] Enable/disable in settings (like QGIS plugin manager) — landed early in phase 1
- Enable/disable in settings (like QGIS plugin manager)

### Phase 3 — Interchange & QA

- LandXML / SWMM export as plugins or engine features
- Golden-file tests per plugin
- Public plugin index (optional)

### Phase 4 — Live analysis & WASM

- `on_entity_committed` hooks
- WASM-hosted engines on hydrocomplete.com

---

## Authoring a new add-on (checklist)

1. Copy `docs/plugin-template/` into `src/modules/<name>/`.
2. Fill `plugin.toml` and `manifest.rs` (keep in sync).
3. Implement `CadModule` in `mod.rs` (ribbon).
4. Implement `dispatch.rs` (all commands for your prefixes).
5. Add `plugin.rs` + `register.rs`.
6. Add `pub mod <name>;` to `src/modules/mod.rs`.
7. Document XDATA in `PLUGIN.md`.
8. Optional: add `crates/<engine>/` and depend from the plugin package only.
9. `cargo build` — tab appears via plugin registry; `commands.rs` untouched.

**External repo:** Publish the engine crate to crates.io; depend on `ocs_plugin_api` (when extracted) + ship a `cdylib` for phase 2. In-tree path: add as a git submodule under `src/modules/<name>/` or `plugins/`.

---

## Reference implementation: Storm Sewer

| Piece | Path |
|-------|------|
| Metadata | `storm_sewer/plugin.toml`, `manifest.rs` |
| Registration | `storm_sewer/register.rs` |
| Adapter | `storm_sewer/plugin.rs` |
| Commands | `storm_sewer/dispatch.rs` |
| Ribbon | `storm_sewer/mod.rs` |
| Tab state | `storm_sewer/state.rs` |
| XDATA | `storm_sewer/data.rs`, `PLUGIN.md` |
| Engine | `crates/stormsewer/` |

---

## Workspace layout

```
OpenCADStudio/
  docs/
    plugin-architecture.md      # this file
    plugin-template/            # scaffold for new add-ons
  src/
    plugin/                     # Layer A runtime (generic)
    modules/
      home/                     # core ribbon (no plugin.toml)
      storm_sewer/              # add-on (has plugin.toml)
  crates/
    stormsewer/                 # Layer C engine
    ocs_plugin_api/             # stable contract: manifest + ribbon (host API: phase 1b)
  plugins/                      # (phase 2) third-party cdylibs
```

---

## Appendix: Civil 3D / SSA contrast

| SSA / Civil 3D | Open CAD Studio add-on |
|----------------|------------------------|
| Proprietary project DB | DWG + XDATA |
| Vendor-only hydraulics | Pluggable `stormsewer` engine |
| Monolithic install | QGIS-style optional packages |
| Closed API | Documented `HostSession` + `PLUGIN.md` |

This positions Open CAD Studio as an **open, inspectable** civil CAD platform rather than a single-vendor clone.