# OpenCADStudio Python Shell Plugin

This plugin adds an interactive Python shell to OpenCADStudio. It is an API V3
plugin that runs in a separate process and communicates with the host via the
`ocs_plugin_api` V3 IPC protocol.

## Ribbon

A "Python Shell" tab is added to the ribbon. Click the **Python Shell** tool (or
run `PYSHELL` on the command line) to open/raise the REPL window for the active
tab.

## Commands

| Command   | Description                                              |
|-----------|----------------------------------------------------------|
| `PYSHELL` | Open or raise the Python Shell for the current document. |

## Python API (planned)

- `ocs.active_document` — document proxy for the invocation tab.
- `doc.entities` — iterable over the document's entities (zero-copy reads).
- `doc.add_point(...)`, `doc.add_line(...)` — add entities via async RPC.
- `entity.xdata["APP"]` — read/write XDATA records.

## Building

```bash
cargo build --release -p opencad-pythonshell
```

Ship the resulting cdylib together with `plugin.toml` in the host plugins
directory.
