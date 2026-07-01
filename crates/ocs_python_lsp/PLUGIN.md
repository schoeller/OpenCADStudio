# ocs_python_lsp — Plugin Architecture

## API V3 additions used

- `HostApi::set_active_tab` + `PluginRequest::SetActiveTab` — switches the host
  target tab before each editor-driven request and restores the original tab
  afterwards.
- `HostApi::remove_entity` + `PluginRequest::RemoveEntity` / `RemoveEntities` —
  removes entities from the active document. The plugin routes `ocs.erase` to
  `RemoveEntity`.
- `PluginResponse::Count` — returns the number of entities removed by
  `RemoveEntities`.

## Out-of-process plugin model

`ocs_python_lsp` is a `cdylib` plugin. When loaded out-of-process, the host
spawns it in the plugin runner and passes a `PluginHostApi` proxy. The plugin
opens one shared Python worker and one LSP server thread per `PYTHONEDIT`
invocation. Host mutations are serialized through the in-process `HostQueue`
and drained by `on_async_event`, which the host pumps while the status panel is
open.

## Threading

- One `PythonLspPlugin` instance (loaded by the host runner).
- One shared `Worker` protected by a `Mutex`; all LSP server threads acquire it
  before sending code.
- One `LspServer` thread per `PYTHONEDIT` invocation.
- One `HostQueue` shared between all LSP server threads and the host's async
  event pump.

## Request flow for `ocs.run`

1. Editor sends `workspace/executeCommand` with `command: "ocs.run"` and a
   base64-encoded `code` argument.
2. `LspServer` locks the worker, sends `CODE <base64>`, and waits.
3. The worker executes the code. Any `ocs.*` call emits a `PyRequest` JSON line
   on stderr.
4. `LspServer::flush_worker_requests` reads each `PyRequest`, converts it to a
   `PluginRequest`, and sends it through `HostQueue::request(tab, ...)`,
   blocking until `on_async_event` replies.
5. `on_async_event` saves the original tab, calls `host.set_active_tab(tab)`,
   applies the request, restores the original tab, and sends the reply.
6. `LspServer` converts the `PluginResponse` back to `PyResponse` and writes
   `__ocs_resp__` JSON to the worker's stdin.
7. When the Python code finishes, the worker prints `__ocs_done__`.
8. `LspServer` returns `{ done: true, output: [...] }` to the editor.

## Layout

- `src/lib.rs` — cdylib plugin entry point, ribbon, `PYTHONEDIT` dispatch.
- `src/host_queue.rs` — in-process request queue.
- `src/lsp_server.rs` — per-`PYTHONEDIT` LSP server thread.
- `src/editor.rs` — editor detection + launch.
- `src/workspace.rs` — temp workspace + editor config generation.
- `src/worker.rs` — Python child process.
- `src/host_api.rs` — `PyRequest` <-> `PluginRequest` routing.
- `src/bootstrap.rs` — embedded Python `ocs` module.
- `src/debugger.rs` — debugpy stub/placeholder.
- `assets/ocs_lsp_bridge.py` — stdio <-> TCP bridge.
- `editors/vscode/` — minimal bundled VS Code extension notes.

## Known limitations

- `ocs.erase_by_layer` and `ocs.erase_all` are not implemented on the host
  side; calling them returns an error.
- `ocs.counts()` returns local counters (currently zeros) until host-side
  write/erase tracking is added.
- The VS Code extension is a README placeholder; no TypeScript code is bundled
  yet.
- Temporary workspaces are created under `%TEMP%` but are not cleaned up
  automatically when the panel closes.
