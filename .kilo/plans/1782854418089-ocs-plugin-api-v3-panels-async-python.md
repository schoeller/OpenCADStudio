# Plan: OCS Plugin API v3 — Panels, Async IPC, Autonomous Python Shell Plugin, Lightweight Runner

**Date:** 2026-07-01  
**Scope:**
- `crates/ocs_plugin_api` — API additions for panels, async events, ABI safety.
- `crates/ocs_plugin_runner` — new lightweight binary crate that replaces the host-as-runner spawn model.
- `crates/ocs_pythonshell` — new standalone `cdylib` plugin that hosts a Python REPL.
- `src/plugin` — concentrate all new host-side plugin code here; keep `src/app` changes to thin hooks.

**Goal:** Extend API v3, keep v2 plugins loadable, implement Python shell as an autonomous plugin, and reduce runner binary size by splitting it into a separate minimal binary crate.

---

## 1. Constraints

- All new protocol/trait work lives in `crates/ocs_plugin_api`.
- All new host-side plugin panel/runtime logic lives in `src/plugin`.
- `src/app` changes are limited to thin delegation hooks.
- `crates/ocs_pythonshell` is a standalone `cdylib` plugin; no Python code in `src/`.
- `ocs_plugin_api` compiles and works without `ocs_pythonshell` or `ocs_plugin_runner`.
- v2 plugins continue to load unchanged.
- Old v3 cdylibs compiled against the previous trait layout are rejected safely.
- Runner binary must be significantly smaller than the host binary (no iced/wgpu).

---

## 2. Decisions

| Topic | Decision |
|---|---|
| Runner | Separate `crates/ocs_plugin_runner` binary crate; host spawns it instead of itself. |
| Host CLI | Remove the hidden `--ocs-plugin-runner` argument and branch. |
| Runner lookup | `ocs_plugin_api::process::runner_executable()` finds `ocs_plugin_runner` next to the host exe, or uses `OCS_PLUGIN_RUNNER_EXE`. |
| Python shell | Standalone `cdylib` plugin in `crates/ocs_pythonshell`. |
| Panels | Host-rendered abstract widgets (label, button, text input, multiline output, list). |
| Panel manager | Owned by the thread-local `PluginManager` in `src/plugin/external.rs`. |
| HostSession | Stays in `src/app/plugin_host.rs`; panel methods delegate to `PluginManager`. |
| App update loop | Calls thin `plugin::on_document_event(...)` and `plugin::on_message(...)` hooks. |
| Zero-copy | Reads via existing shared-memory `DocumentReader`; writes remain validated RPCs. |
| Async IPC | Sync RPC kept; out-of-band `HostAsync`/`PluginAsync` events on the same socket. |
| v2 loading | `BuiltinPluginV2` adapter in `ocs_plugin_api`. |
| Old-v3 safety | `ocs_plugin_abi_revision()` C export; runner rejects mismatched v3 cdylibs. |
| Feature gating | API-major gating: v2 gets nothing new; v3 plugins implement the full new surface. |
| Python process | External `python`/`python3`/`python.exe`; no `pyo3`. |
| Python ↔ host | Python `ocs` module serializes HostApi requests to stderr; host handles them like plugin requests. |

---

## 3. Architecture

```text
OpenCADStudio host binary
  ├─ src/plugin/PluginManager (owns PanelManager)
  ├─ src/app/plugin_host.rs HostSession (delegates panels to PluginManager)
  └─ spawns crates/ocs_plugin_runner

ocs_plugin_runner binary
  └─ crates/ocs_plugin_api::runner::run(socket, cdylib)

ocs_pythonshell cdylib
  └─ loaded by runner · declares panel · spawns python
```

---

## 4. `crates/ocs_plugin_runner` (new)

### 4.1 `Cargo.toml`

```toml
[package]
name = "ocs_plugin_runner"
version = "0.1.0"
edition = "2021"

[dependencies]
ocs_plugin_api = { path = "../ocs_plugin_api", features = ["host"] }

[profile.release]
opt-level = "z"
lto = true
strip = true
```

No `iced`, `wgpu`, `rfd`, `cosmic-text`, or other GUI dependencies.

### 4.2 `src/main.rs`

```rust
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: ocs_plugin_runner <socket> <cdylib>");
        std::process::exit(1);
    }
    if let Err(e) = ocs_plugin_api::runner::run(&args[1], std::path::Path::new(&args[2])) {
        eprintln!("[runner] fatal: {e}");
        std::process::exit(1);
    }
}
```

### 4.3 Workspace registration

Add `crates/ocs_plugin_runner` to `[workspace].members` in the root `Cargo.toml`.

---

## 5. `crates/ocs_plugin_api` changes

### 5.1 ABI safety

Add C export beside `ocs_plugin_api_version`:

```rust
#[no_mangle]
pub extern "C" fn ocs_plugin_abi_revision() -> u64;
```

`export_plugin!` emits it. `runner::load_plugin`:

- `version == 2` → load with `BuiltinPluginV2`, wrap in `V2ToV3Adapter`.
- `version == 3` → check `abi_revision`; mismatch → `PluginError::AbiRevisionMismatch`; else load with current `BuiltinPlugin`.
- other → version error.

### 5.2 Panel vocabulary (`src/panel.rs`)

```rust
pub struct PanelDef { pub id: String, pub title: String, pub icon: Option<OwnedIconKind> }
pub struct PanelHandle(pub u64);
pub enum Widget { Label(String), Button { id: String, label: String }, TextInput { id: String, value: String }, MultilineOutput { id: String, lines: Vec<String> }, List { id: String, items: Vec<String> } }
pub enum PanelEvent { Clicked(String), TextChanged { id: String, value: String }, ItemSelected { id: String, index: usize }, Closed }
pub enum PanelError { Unsupported, UnknownHandle, Io(String) }
```

### 5.3 `BuiltinPlugin` additions (v3 ABI break)

```rust
fn panels(&self) -> Vec<PanelDef>;
fn on_async_event(&mut self, host: &mut dyn HostApi, event: HostAsync);
```

Default no-op bodies only for test convenience; real v3 plugins override them.

### 5.4 `HostApi` additions (appended)

```rust
fn open_panel(&mut self, def: &PanelDef) -> Result<PanelHandle, PanelError>;
fn close_panel(&mut self, handle: PanelHandle) -> Result<(), PanelError>;
fn post_panel_event(&mut self, handle: PanelHandle, event: PanelEvent) -> Result<(), PanelError>;
```

Default implementations return `PanelError::Unsupported`.

### 5.5 Protocol additions

```rust
pub enum HostAsync {
    DocumentActivated { tab: usize },
    DocumentChanged { tab: usize, version: u64 },
    TabClosed { tab: usize },
    PanelEvent { panel_id: String, event: PanelEvent },
}

pub enum PluginAsync {
    PanelUpdate { panel_id: String, widgets: Vec<Widget> },
    PanelClosed { panel_id: String },
}
```

Add to `PluginRequest`:

```rust
OpenPanel { def: PanelDef },
ClosePanel { handle: PanelHandle },
PostPanelEvent { handle: PanelHandle, event: PanelEvent },
```

Add matching `PluginResponse` variants.

### 5.6 Process/runtime changes

- `PluginProcess`: bounded inbound `PluginAsync` queue + dropped-event counter.
- `PluginProcess`: `send_async(&self, HostAsync)`.
- Sync RPC loops enqueue `PluginToHost::Async` instead of erroring.
- `IpcClient` async sending becomes thread-safe.
- `runner_executable()` returns the path to `ocs_plugin_runner` next to the host exe, or `OCS_PLUGIN_RUNNER_EXE` override.

### 5.7 Runner changes

- Load v2 plugins through the adapter.
- Forward `HostAsync` to `plugin.on_async_event`.

---

## 6. `src/plugin` concentration

### 6.1 `src/plugin/panels.rs` (new)

`PanelManager`:

- Owns open plugin panels keyed by `PanelHandle`.
- Stores widgets per panel and renders with iced.
- Maps each panel to owning `PluginProcess` and panel id.
- On user action, calls `PluginProcess::send_async(HostAsync::PanelEvent)`.
- Exposes `open`, `close`, `update`, and `broadcast_document_event(tab, event)`.

### 6.2 `src/plugin/external.rs`

- Add `panel_manager: PanelManager` to `PluginManager`.
- Add methods: `open_panel`, `close_panel`, `update_panel`, `broadcast_document_event`, `handle_message`.
- During `load_at_startup`, register plugin `PanelDef`s and broadcast `DocumentActivated` for the current tab.

### 6.3 `src/plugin/registry.rs`

- No new state; use `external::with_manager` for panel operations.

### 6.4 `src/plugin/mod.rs`

Add public hooks:

```rust
pub fn on_document_activated(app: &mut OpenCADStudio, tab: usize);
pub fn on_document_changed(app: &mut OpenCADStudio, tab: usize);
pub fn on_tab_closed(app: &mut OpenCADStudio, tab: usize);
pub fn on_message(app: &mut OpenCADStudio, msg: &Message) -> Option<Task<Message>>;
```

These delegate to `PluginManager`.

---

## 7. Minimal `src/app` hooks

### 7.1 `src/app/plugin_host.rs`

Implement new `HostApi` panel methods by delegating to `crate::plugin::external::with_manager`:

```rust
fn open_panel(&mut self, def: &PanelDef) -> Result<PanelHandle, PanelError> {
    crate::plugin::external::with_manager(|mgr| mgr.open_panel(def))
}
// ... close_panel, post_panel_event
```

### 7.2 `src/app/mod.rs`

No new fields; plugin panel state lives inside `src/plugin/external.rs` thread-local `PluginManager`.

### 7.3 `src/app/update/mod.rs`

Insert thin calls:

- On tab switch/change/close: `crate::plugin::on_document_activated/...`.
- In the main message dispatch match, add one arm or modify the catch-all to call `crate::plugin::on_message(self, msg)` before default handling.

### 7.4 `src/cli.rs` and `src/main.rs`

- Remove `ocs_plugin_runner` field from `Cli`.
- Remove the `--ocs-plugin-runner` branch from `main`.

---

## 8. `crates/ocs_pythonshell` (new)

### 8.1 `Cargo.toml`

```toml
[package]
name = "ocs_pythonshell"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
ocs_plugin_api = { path = "../ocs_plugin_api", features = ["host"] }
acadrust = "0.4"
```

Add `crates/ocs_pythonshell` to `[workspace].members`.

### 8.2 `src/lib.rs`

- Static `PluginManifest` with API v3.
- `CadModule` ribbon with one tool: `PY_OPEN_SHELL`.
- `BuiltinPlugin`:
  - `panels()` returns a single Python REPL `PanelDef`.
  - `dispatch()` handles `PY_OPEN_SHELL` with `host.open_panel(...)`.
  - `on_async_event()` handles `PanelEvent` for input field and run button.
- Spawns `python -u` with an embedded bootstrap script.
- Forwards code to Python stdin; reads stdout into `MultilineOutput` updates; reads stderr framed HostApi requests and dispatches them.

### 8.3 Python bootstrap

Embedded Python script installs an `ocs` module whose methods serialize HostApi requests as length-prefixed bincode to `sys.stderr`. The Rust plugin reads stderr and forwards calls through its `PluginHostApi`.

---

## 9. Implementation stages

### Stage 1 — Runner split

1. Create `crates/ocs_plugin_runner` binary crate.
2. Add it to workspace members.
3. Remove `--ocs-plugin-runner` from `src/cli.rs` and `src/main.rs`.
4. Change `ocs_plugin_api::process::runner_executable()` to locate `ocs_plugin_runner`.
5. Build and verify host spawns the new runner.
6. Test: v2 plugin loads through the new runner.

### Stage 2 — ABI & async foundation

1. Add `ocs_plugin_abi_revision()` to `export_plugin!` and the v2 fixture.
2. Define `BuiltinPluginV2` and `V2ToV3Adapter`.
3. Update `runner::load_plugin` version/revision logic.
4. Add `HostAsync`/`PluginAsync` to protocol.
5. Make `IpcClient` async-send thread-safe; add inbound queue to `PluginProcess`.
6. Update sync RPC loops to enqueue async messages.
7. Tests: v2 init/close, old-v3 rejection, async event round-trip.

### Stage 3 — Panel API

1. Add `src/panel.rs` with panel vocabulary.
2. Add `BuiltinPlugin::panels` and `on_async_event`.
3. Add `HostApi` panel methods.
4. Add `PluginRequest`/`PluginResponse` panel variants and implement plugin-side methods.
5. Tests: plugin declares panel, host opens it, plugin updates widgets.

### Stage 4 — Host panel manager in `src/plugin`

1. Create `src/plugin/panels.rs` with `PanelManager` and iced rendering.
2. Add `PanelManager` to `PluginManager` in `src/plugin/external.rs`.
3. Implement `HostSession` panel delegation in `src/app/plugin_host.rs`.
4. Add `plugin::on_document_event` / `plugin::on_message` hooks and wire them in `src/app/update/mod.rs`.
5. Tests: document lifecycle events reach plugin; button click reaches plugin.

### Stage 5 — Python shell plugin

1. Create `crates/ocs_pythonshell` crate.
2. Implement manifest, ribbon, panel, dispatch.
3. Implement Python process spawn and bootstrap script.
4. Implement `ocs` Python module backed by stderr framing.
5. Tests: Python eval, `ocs.push_info`, Python crash does not kill host.

### Stage 6 — Integration & docs

1. Update `docs/plugin-template` with optional panel example.
2. Update `docs/plugin-architecture.md`.
3. Build runner and compare binary sizes.
4. Run full test suite.

---

## 10. Validation

| Test | What it proves |
|---|---|
| `runner_binary_smaller` | `ocs_plugin_runner` release binary is significantly smaller than the host binary. |
| `runner_spawns_plugin` | Host discovers and spawns `ocs_plugin_runner` correctly. |
| `v2_plugin_init_close` | v2 plugins still load and shut down. |
| `old_v3_abi_revision_rejected` | Old v3 cdylibs are refused without crashing. |
| `async_event_roundtrip` | Host→plugin async events are delivered. |
| `plugin_async_during_rpc` | `PluginAsync` messages are enqueued while a sync call is in flight. |
| `zerocopy_read_then_write` | Handle read from shared memory; XDATA written via RPC and read back. |
| `panel_open_update_close` | Plugin panel lifecycle works end-to-end. |
| `document_lifecycle_to_panel` | Tab switch/change events reach the plugin panel. |
| `python_shell_eval` | Python expression output appears in the panel. |
| `python_host_api_call` | Python `ocs.add_entity` reaches the host document. |
| `python_crash_isolated` | Python worker crash closes only the panel, not the host. |

---

## 11. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Runner binary not found after deployment. | Host looks next to current exe; `OCS_PLUGIN_RUNNER_EXE` override for tests; CI verifies both binaries are packaged. |
| Old v3 cdylib crashes runner due to trait mismatch. | ABI revision export rejects mismatched plugins before trait calls. |
| Plugin async sender races with runner RPC thread. | `IpcClient` uses thread-safe stream access. |
| Rapid panel updates flood the UI. | Host coalesces `PanelUpdate` messages per frame. |
| Panel event queue overflows. | Bounded queue + dropped-event counter per plugin process. |
| Python executable not found. | Detection order: `OCS_PYTHON_EXE` → `python3` → `python` → `py -3` (Windows) + error panel. |
| Python code hangs. | Read timeouts on stdout/stderr; host kills the worker process. |
| `ocs_plugin_api` accidentally depends on runner/Python. | Both are separate workspace crates; CI builds `ocs_plugin_api` alone. |

---

## 12. Files expected to change

### `crates/ocs_plugin_api`
- `src/manifest.rs`
- `src/host.rs`
- `src/panel.rs` (new)
- `src/lib.rs`
- `src/ipc/protocol.rs`
- `src/ipc/client.rs`
- `src/ipc/server.rs`
- `src/ipc/transport.rs`
- `src/process.rs`
- `src/runner.rs`

### `crates/ocs_plugin_runner` (new)
- `Cargo.toml`
- `src/main.rs`

### `crates/ocs_pythonshell` (new)
- `Cargo.toml`
- `src/lib.rs`
- embedded bootstrap script

### `src/plugin`
- `src/plugin/panels.rs` (new)
- `src/plugin/external.rs`
- `src/plugin/registry.rs`
- `src/plugin/mod.rs`

### `src/app`
- `src/app/plugin_host.rs` (panel delegation only)
- `src/app/update/mod.rs` (thin hooks only)
- `src/cli.rs` (remove runner arg)
- `src/main.rs` (remove runner branch)

### Root
- `Cargo.toml` (workspace members)

### Docs
- `docs/plugin-template/src/lib.rs`
- `docs/plugin-template/plugin.toml`
- `docs/plugin-architecture.md`

---

## 13. Out of scope

- WebAssembly plugin support (unchanged no-op).
- Rich widgets beyond the minimal core set.
- Incremental/delta shared-memory snapshots.
- Plugin code signing or sandboxing beyond process isolation.
- Further splitting `ocs_plugin_api` features to shrink the runner beyond removing iced/wgpu.
