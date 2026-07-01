# ocs_python_lsp

LSP-bridged Python editor plugin for OpenCAD Studio.

## What it does

`ocs.python_lsp` contributes a `Python` ribbon tab with an `Editor LSP` tool.
When the user runs `PYTHONEDIT` (from the ribbon or the command line), the
plugin:

1. Starts a per-document-tab LSP server on a free localhost TCP port.
2. Creates a temporary workspace containing `main.py`, `ocs_lsp.json`, and
   `ocs_lsp_bridge.py`.
3. Launches the first available external editor: Lapce → Zed → VS Code
   (`code` / `code.cmd` on Windows).
4. Opens a shared status panel that the host pumps at 60 Hz for async work.

The editor talks LSP JSON-RPC to the bridge, the bridge forwards the stream to
the Rust LSP server, and the server delegates Python execution to a shared
Python worker. Python scripts can read and write the active CAD document
through the embedded `ocs` module.

## Quick start

1. Install Python 3 and (optionally) `debugpy` for debugging:
   ```powershell
   pip install debugpy
   ```
2. Install an external editor such as VS Code.
3. Load the plugin in OpenCAD Studio.
4. Click **Python → Editor LSP** in the ribbon.

## Architecture

```text
External editor (VS Code / Zed / Lapce)
        │ LSP JSON-RPC over stdio
        ▼
ocs_lsp_bridge.py  (stdio ↔ TCP)
        │ TCP localhost:<port>
        ▼
LspServer thread  (crates/ocs_python_lsp/src/lsp_server.rs)
        │ workspace/executeCommand
        ├─ ocs.run          -> Python worker
        ├─ ocs.read         -> Python worker + host document
        ├─ ocs.erase        -> PluginRequest::RemoveEntity
        ├─ ocs.erase_*      -> host queue (partial)
        ├─ ocs.debug.start  -> debugpy in Python
        └─ ocs.stats        -> local counters
        │
        ├─ Python worker stdin/stdout/stderr
        │   (shared by all LSP server threads)
        ▼
Python worker with embedded `ocs` module
        │
        │ PyRequest on stderr, __ocs_resp__ on stdin
        ▼
HostQueue  (tab, PluginRequest, reply_tx)
        │
        ▼
BuiltinPlugin::on_async_event  -> HostApi
```

## File layout

| File | Purpose |
|---|---|
| `src/lib.rs` | Plugin entry point, ribbon, `PYTHONEDIT` dispatch, panel handling, queue drain |
| `src/host_queue.rs` | Thread-safe queue between LSP server threads and `on_async_event` |
| `src/lsp_server.rs` | Per-`PYTHONEDIT` TCP LSP server |
| `src/worker.rs` | Shared Python child process |
| `src/bootstrap.rs` | Embedded `ocs` Python module |
| `src/host_api.rs` | `PyRequest`/`PyResponse` types and routing to `PluginRequest` |
| `src/editor.rs` | Editor detection and launch |
| `src/workspace.rs` | Temporary workspace generation |
| `src/debugger.rs` | Debugpy placeholder/stub |
| `assets/ocs_lsp_bridge.py` | Python stdio ↔ TCP bridge |
| `plugin.toml` | Plugin metadata |
| `tests/ocs_python_lsp.rs` | Integration tests |
| `editors/vscode/README.md` | Bundled VS Code extension notes |

## Supported `ocs` module methods

| Python | Behavior |
|---|---|
| `ocs.doc.entities()` | List entities in active document |
| `ocs.doc.layers()` | List layers |
| `ocs.add_point(x, y, z=0, layer='0')` | Add a point entity |
| `ocs.add_line(...)` | Add a line entity |
| `ocs.add_circle(...)` | Add a circle entity |
| `ocs.add_text(...)` | Add a text entity |
| `ocs.read_xdata(handle, app_name)` | Read XDATA record |
| `ocs.write_xdata(handle, app_name, data)` | Write XDATA record |
| `ocs.remove_xdata(handle, app_name)` | Remove XDATA record |
| `ocs.erase(handle)` | Remove entity by handle |
| `ocs.erase_by_layer(layer)` | **Not implemented** (returns error) |
| `ocs.erase_all()` | **Not implemented** (returns error) |
| `ocs.counts()` | Returns `{ written, erased }` (currently local stubs) |
| `ocs.debug.start(port=5678)` | Start `debugpy` listener and wait for client |

## Environment variables

| Variable | Meaning |
|---|---|
| `OCS_PYTHON_EXE` | Path to Python interpreter; overrides discovery |
| `OCS_PYTHON_TIMEOUT_SECS` | Timeout for `ocs.run` commands (default 30) |

## Testing

```powershell
$env:CARGO_TARGET_DIR = 'C:\tmp\ocs_target'
cargo test -p ocs_python_lsp
```

Tests that require a Python interpreter are marked `#[ignore]` and skip
gracefully when Python is unavailable.

## See also

- `PLUGIN.md` — architecture details and API V3 additions used by this plugin.
- `crates/ocs_pythonshell` — the interactive Python REPL plugin.
