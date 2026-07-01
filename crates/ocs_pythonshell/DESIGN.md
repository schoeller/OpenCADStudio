# Design: ocs_pythonshell

## Overview

The Python Shell plugin embeds a Python 3 REPL inside an OpenCAD Studio panel. The interpreter runs in a separate OS process and communicates with the Rust plugin over `stdin`, `stdout`, and `stderr`.

## Architecture

```
+--------------------------------------------------+
|  OpenCAD Studio host                             |
|  +---------------------------------------------+ |
|  | PythonShellPlugin (Rust)                    | |
|  |  - panel state                              | |
|  |  - Worker handle                            | |
|  +----------------------+----------------------+ |
|           | stdin / stdout / stderr             |
+-----------|--------------------------------------+
            v
+--------------------------------------------------+
|  Python child process (-u -c <bootstrap>)        |
|  - reads CODE lines from stdin                   |
|  - prints REPL output to stdout                  |
|  - emits JSON requests on stderr                 |
|  - reads __ocs_resp__ replies from stdin         |
+--------------------------------------------------+
```

## Streams

| Stream | Direction | Content |
|--------|-----------|---------|
| `stdin` | Rust → Python | `CODE <base64>` blocks and `__ocs_resp__ <json>` replies |
| `stdout` | Python → Rust | REPL output plus `__ocs_done__` completion marker |
| `stderr` | Python → Rust | JSON host API requests |

## Completion signaling

After each `CODE` block finishes, the bootstrap prints `__ocs_done__` on `stdout`. Because the marker is on the same stream as the REPL output, ordering is guaranteed: all output lines arrive before the marker. The Rust stdout reader filters the marker and sets `OutputBuffer.done = true`.

## Request / response protocol

Python requests are JSON objects emitted on `stderr`. Rust replies with `__ocs_resp__ <json>` on `stdin`.

Requests and replies are defined by the `PyRequest` / `PyResponse` enums in `src/lib.rs`.

Example request:

```json
{"type":"AddPoint","value":{"x":1.0,"y":2.0,"z":0.0,"layer":"0"}}
```

Example reply:

```json
{"type":"Handle","value":42}
```

## Bootstrap

A Python bootstrap script is embedded as a string constant. It:

1. Creates an `ocs` module with methods for each host API request.
2. Enters a loop reading `CODE <base64>` lines.
3. Decodes and compiles the code, using `eval` for expressions and `exec` for statements.
4. Prints exceptions to stdout.
5. Prints `__ocs_done__` after each block.

## Lifecycle

- `dispatch("PY_OPEN_SHELL")` opens the panel and spawns a `Worker`.
- `Worker::new` starts stdout/stderr reader threads.
- Each Run event sends the code, then loops until `__ocs_done__`, process exit, or idle timeout.
- `Worker::close` closes stdin, kills the child, waits for it, and joins the reader threads to drain remaining output.

## Concurrency

- The plugin state is protected by a `Mutex<PluginState>`.
- Reader threads push lines/requests into a shared `Mutex<OutputBuffer>`.
- The plugin thread polls the shared buffer during command execution.
- `set_dirty` / `bump_geometry` are batched per command batch to avoid O(N) host notifications.

## Security model

The shell intentionally evaluates arbitrary user code. It runs in the same user context as the host, with the same filesystem and network access. There is no sandbox. The panel should only be exposed to trusted users.

The plugin validates the configured interpreter by checking `python --version` output, rejecting non-Python executables.

## Known limitations

- The JSON wire format is hand-written on the Python side and mirrored by serde enums on the Rust side. Renaming a variant or field on one side silently breaks the other until an integration test catches it.
- XDATA conversion is implemented as two mirrored match arms (`xdata_to_py` and `py_to_xdata`).
- No sandboxing; arbitrary code execution is by design.
- Output buffer is capped at 500 lines.
