# ocs_plugin_runner

Out-of-process plugin runner for OpenCAD Studio.

## Purpose

The host executable can run in two modes:

1. **Normal mode** — the full OpenCAD Studio GUI application.
2. **Plugin-runner mode** — loads one cdylib plugin, connects back to the host
   over a local socket, and forwards all `HostApi` calls.

`ocs_plugin_runner` is the crate that implements the runner mode. Because the
runner is built from the same source tree as the host, the two can never drift
out of sync at deployment time.

## How the host spawns a plugin

1. The host discovers a plugin cdylib (e.g. in `%APPDATA%\OpenCADStudio\plugins`).
2. It creates a local socket name and spawns itself in runner mode:
   ```text
   OpenCADStudio.exe --ocs-plugin-runner <socket_name> <cdylib_path>
   ```
3. The runner loads the cdylib, validates `ocs_plugin_api_version` and
   `ocs_plugin_abi_revision`, and calls `ocs_plugin_register` to obtain the
   `Box<dyn BuiltinPlugin>`.
4. The runner connects to the host socket and answers `GetManifest` and
   `GetRibbon`.
5. The runner enters a request loop: dispatch commands, forward interactive
   events, and proxy `HostApi` calls via `PluginHostApi`.

## Entry point

The runner is invoked through the `--ocs-plugin-runner` CLI argument handled in
the main OpenCAD Studio binary. The `ocs_plugin_runner` crate provides the
`run` function consumed by the host.

## Failure handling

- Plugin panic — caught inside the runner; an error response is returned to the
  host.
- Plugin crash / hang / malformed message — the host marks the plugin dead,
  drops its ribbon tab, logs the error, and continues running.
- Spawn failure — reported through `PluginManager` and shown in the host's
  Plugin Manager.

## Security model

Running plugins in separate processes gives the host:

- Memory isolation — a buggy or malicious plugin cannot corrupt host memory.
- Crash containment — a plugin crash does not crash the host.
- UI thread protection — long-running plugin requests time out instead of
  freezing the host.

## Building

```powershell
cargo build -p ocs_plugin_runner
```

The runner is normally not executed directly; the host binary spawns it
automatically.

## Testing

Integration tests for out-of-process plugins live in plugin crates such as
`plugin-template-api3/tests/integration.rs`. Those tests build the runner and
the plugin cdylib, spawn the runner, and exercise the full RPC and async flow.
