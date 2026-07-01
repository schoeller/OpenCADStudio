# ocs_pythonshell

Interactive Python REPL panel for OpenCAD Studio.

## What it does

`ocs_pythonshell` is a plugin that adds a dockable **Python Shell** panel to OpenCAD Studio. It spawns a real Python 3 interpreter and exposes a small `ocs` object so users can:

- Inspect and modify the CAD document (`ocs.doc.entities()`, `ocs.add_point(...)`, ...)
- Push info/output/error messages to the host UI
- Read and write XDATA records
- Trigger host-side geometry/dirty updates

The plugin is built as a `cdylib` and loaded through the API v3 plugin system.

## Requirements

- Rust toolchain
- Python 3 interpreter available on `PATH` as `python3`, `python`, or `py -3` (Windows)
- Alternatively, set the environment variable `OCS_PYTHON_EXE` to a Python 3 executable

## Building

```bash
cargo build -p ocs_pythonshell
```

On Windows, if the default `target` directory has build-script issues, use a temporary target dir:

```powershell
$env:CARGO_TARGET_DIR = 'C:\temp\ocs-target'
cargo build -p ocs_pythonshell
```

## Testing

```bash
cargo test -p ocs_pythonshell
```

The integration tests require a real Python interpreter. If none is found, the Python-dependent tests skip gracefully.

## Configuration

| Environment variable | Purpose |
|----------------------|---------|
| `OCS_PYTHON_EXE`     | Path or command name of the Python 3 executable to use |
| `OCS_PYTHON_TIMEOUT_SECS` | Maximum time (in seconds) a running Python command is allowed before the host gives up. Default: 30 |

## Usage in OpenCAD Studio

1. Open the **Python Shell** panel from the ribbon.
2. Type Python code in the input box.
3. Click **Run** to execute the code.
4. Use **Clear Output** to empty the output buffer.

Example:

```python
print(ocs.doc.entities())
ocs.add_point(10, 20)
ocs.push_info("Added a point")
```

## Security notice

The Python Shell executes arbitrary code with the privileges of the host process. It is intended for trusted users. Do not expose the panel in untrusted environments.

The plugin honors `OCS_PYTHON_EXE` but verifies that the executable reports `Python 3.x` before using it.
