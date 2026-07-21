# `ocs_python_repl` — Python scripting for OpenCAD Studio

The Python REPL plugin lets users write, run, and debug Python scripts that read and modify the active OpenCAD Studio document. It is implemented as a normal API V3 plugin: the host shares a full document snapshot and a mutation queue through shared memory, and a separate Python process reads the snapshot and writes entity operations back.

## Features

- One-click external editor launch via `PYTHONEDIT`.
- Fast read/write access through a shared-memory document snapshot and lock-free mutation queue.
- Pythonic wrapper for `acadifc` entities: `Point`, `Line`, `Circle`, `Arc`, `Text`, `MText`, `LwPolyline`, `Insert`, `Hatch`, `Dimension`, `Leader`, `Viewport`, `Spline`.
- Document collections: `ocs.doc.layers`, `ocs.doc.blocks`, `ocs.doc.text_styles`, `ocs.doc.dim_styles`, `ocs.doc.styles`.
- Bulk operations: `ocs.doc.add_many()`, `ocs.doc.remove_all()` for sub-second 1000-point roundtrips.
- Built-in debugging support via `debugpy` and editor attach configs for VS Code and Zed.
- No embedded Python runtime — the plugin uses the user's system Python.

## Architecture

```text
┌──────────────────────────────────────────────────────────────┐
│  OpenCAD Studio host (tab document + DocumentShmResources)    │
│  - DocumentFullSnapshotStore (serde+bincode)                │
│  - DocumentMutationQueue (lock-free ring buffer)            │
└──────────────────────┬───────────────────────────────────────┘
│                       │ shared memory paths + control socket
│                       ▼
│              ┌────────────────────┐
│              │  ocs_python_repl   │  cdylib plugin
│              │  - spawns Python   │
│              │  - forwards async  │
│              └────────┬───────────┘
│                       │
│                       ▼
│              ┌────────────────────┐
│              │  Python process    │
│              │  _ocs_bootstrap.py │
│              └────────┬───────────┘
│                       │
│                       ▼
│              ┌────────────────────┐
│              │  ocs_acadifc       │  PyO3 extension
│              │  - ocs.doc         │
│              │  - ocs.debug       │
│              └──────────────────────┘
└──────────────────────────────────────────────────────────────┘
```

### Data path

1. **Full document snapshot** — the host serializes the active `CadDocument` with `serde`/`bincode` into a file-backed `DocumentFullSnapshotStore`. The Python extension opens the same file read-only and caches the deserialized document, refreshing only when the version counter changes.
2. **Mutation queue** — the Python extension writes lightweight `EntityOp` records (`Add`, `Update`, `Remove`) into a `DocumentMutationQueue` backed by a memory-mapped ring buffer.
3. **Refresh** — when the script calls `ocs.doc.commit()`, the extension sends a `REFRESH` message over a local control socket. The plugin forwards a `DocumentRefreshRequested` async event to the host, which drains the queue and applies the batch as a single undoable action.

### Why this design

- **Decoupling**: Python is not linked into the host process, so a buggy script cannot crash the editor.
- **Performance**: bulk `add_many`/`remove_all` cross the Python/Rust boundary once, and the 1000-point roundtrip completes in about **0.03 s** (Python side) / **0.10 s** (host side).
- **Windows compatibility**: the full snapshot uses open/write/close file I/O instead of cross-process memory mapping, avoiding the hangs seen with `mmap` on Windows.

### Crate layout

```text
crates/ocs_python_repl/
├── Cargo.toml              # plugin cdylib
├── plugin.toml             # API V3 manifest
├── src/
│   ├── lib.rs              # plugin entry, PYTHONEDIT dispatch
│   ├── repl.rs             # Python child process + control socket
│   ├── editor.rs           # editor launcher (Zed, VS Code, ...)
│   ├── workspace.rs        # temp workspace creation
│   └── python_env.rs       # Python / pip / debugpy detection
├── ocs_acadifc/            # PyO3 extension
│   └── src/
│       ├── lib.rs          # module init
│       ├── document.rs     # ocs.doc
│       ├── entities.rs     # entity constructors/conversions
│       ├── geometry.rs     # Vector3, Color
│       └── debug.rs        # ocs.debug
├── assets/                 # main.py, ocs.pyi, editor configs
└── tests/                # roundtrip benchmark
```

## Installation

### Requirements

- OpenCAD Studio built from source.
- Python 3 with `pip` installed and on `PATH`.
- A supported editor: Zed, Gram, VS Code, Lite XL, or Lapce.

### Build

```bash
cargo build --workspace
```

The build produces:

- `target/debug/ocs_python_repl.dll` (or `.so`/`.dylib`) — the plugin.
- `target/debug/ocs_acadifc.dll` (or `.so`/`.dylib`) — the PyO3 extension.

### Install debugpy

The plugin will attempt to install `debugpy` automatically, but you can pre-install it:

```bash
python -m pip install debugpy
```

### First run

1. Launch OpenCAD Studio.
2. Type `PYTHONEDIT` in the command line or click the ribbon button.
3. The plugin opens a temp workspace in your system temp directory and launches your editor with `main.py`.

## Usage

### Edit and run scripts

The editor workspace contains:

- `main.py` — your script.
- `ocs.pyi` — type stubs for autocompletion.
- `pyrightconfig.json` — language server configuration.
- `.vscode/launch.json` or `.zed/debug.json` — debugger attach config.

Modify `main.py` and save. The REPL process runs the script automatically on startup; for debugging, see the Debugging section below.

### `ocs.doc` API

```python
import ocs
from ocs import Vector3

# Read document metadata
print(len(ocs.doc.entities))
print(ocs.doc.version)

# Read layers and blocks
for layer in ocs.doc.layers:
    print(layer.name)
print(ocs.doc.blocks)
print(ocs.doc.styles)  # dict with text_styles and dim_styles
```

### Adding entities

```python
import ocs
from ocs import Vector3

ocs.doc.add(ocs.Point(10, 20, 0))
ocs.doc.add(ocs.Line(Vector3(0, 0, 0), Vector3(10, 10, 0)))
ocs.doc.add(ocs.Circle(Vector3(5, 5, 0), radius=3))
ocs.doc.add(ocs.Text("Hello", x=0, y=0, height=2.5))
ocs.doc.add(ocs.MText("Multi-line text", x=10, y=10, height=2.5))
ocs.doc.add(ocs.LwPolyline([(0, 0, 0), (10, 0, 0), (10, 10, 0.5)], is_closed=True))
ocs.doc.add(ocs.Insert("MyBlock", x=5, y=5, rotation=0.5))
ocs.doc.add(ocs.Hatch([(0, 0), (10, 0), (10, 10), (0, 10)], is_solid=True))
ocs.doc.add(ocs.Dimension(Vector3(0, 0, 0), Vector3(10, 0, 0)))
ocs.doc.add(ocs.Leader([Vector3(0, 0, 0), Vector3(5, 5, 0)]))
ocs.doc.add(ocs.Viewport(Vector3(100, 100, 0), width=200, height=150, id=2))
ocs.doc.add(ocs.Spline(
    [Vector3(0, 0, 0), Vector3(5, 10, 0), Vector3(10, 0, 0)],
    knots=[0, 0, 0, 1, 1, 1],
    degree=2,
))

ocs.doc.commit()
```

### Bulk operations

```python
import ocs

points = [ocs.Point(i, i, 0) for i in range(1000)]
ocs.doc.add_many(points)
ocs.doc.commit()
```

### Removing entities

```python
# Remove one entity from an iterator
for e in ocs.doc.entities:
    if e.kind == "Point":
        e.delete()
ocs.doc.commit()

# Remove all entities
ocs.doc.remove_all()
ocs.doc.commit()
```

### Reading entities

```python
for e in ocs.doc.entities:
    print(e.handle, e.kind, e.layer)
```

Entity types are converted to Python classes when supported; unknown or not-yet-bound types fall back to the generic `Entity` class.

## Debugging

The editor configs let you attach to the running Python process on `localhost:5678`.

### Manual debugging in `main.py`

```python
import ocs

ocs.debug.start()          # start debugpy listener on localhost:5678
ocs.debug.wait_for_client()  # block until debugger attaches

# Set breakpoints below this line.
```

Start the debugger attach config in your editor before the script continues. In VS Code, select **"Attach to OpenCAD Studio Python REPL"** and press F5. In Zed, select the same label from the debugger panel.

The bootstrap also starts `debugpy` on port 5678 automatically, so you can attach without adding `ocs.debug.start()` to the script.

## Development

### Run the benchmark

```bash
cargo test -p ocs_python_repl --test ocs_python_repl roundtrip_1000_points -- --nocapture
```

Expected output (debug build):

```text
add 1000 points: 0.018s
remove 1000 points: 0.008s
roundtrip wall time: 0.104s
```

### Run all REPL tests

```bash
cargo test -p ocs_python_repl -p ocs_acadifc -p ocs_plugin_api
```

### Crate tests

- `ocs_plugin_api::shm` tests cover full snapshot and mutation queue roundtrips.
- `ocs_python_repl::workspace` tests verify starter files are written.
- `ocs_python_repl::ocs_python_repl` integration test runs the full Python benchmark.

## Troubleshooting

| Problem | Cause | Fix |
|---------|-------|-----|
| "Python check failed" | Python not on `PATH` | Install Python 3 and ensure `python`/`python3`/`py` is available. |
| "debugpy installation failed" | `pip` unavailable or no network | Pre-install `debugpy` with `python -m pip install debugpy`. |
| "cannot find ocs_acadifc.dll" | Extension not built | Build the workspace with `cargo build --workspace`. |
| `ocs.doc` raises "not initialized" | `import ocs` ran outside the REPL | Run the script through the REPL bootstrap; `_ocs_config.json` is required for standalone execution. |
| "mutation queue full" | Too many operations between commits | Call `ocs.doc.commit()` more often. |

## License

GPL-3.0-only — see the main OpenCAD Studio repository license.
