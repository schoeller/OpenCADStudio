# Plan: `ocs_python_repl` — fast, Pythonic CAD scripting plugin

**Date:** 2026-07-20  
**Scope:** Create `crates/ocs_python_repl`; delete `crates/ocs_python_lsp` after the new plugin is stable.  
**Allowed external changes:** `crates/ocs_plugin_api` only.  
**Constraint:** No API V2. Use API V3 and any other coding approach (shared memory, PyO3, external editors).  
**Goal:** One-click Python editor/debugging plugin that runs on the user's locally installed Python + PIP, exposes the current document through a Pythonic `acadifc` object model, and gives fast read/write/erase access to the host document (target: add and remove 1000 points in under 1 second).

---

## 1. Problem statement

`ocs_python_lsp` is built around an LSP bridge: editor → bridge → TCP server → per-edit thread → Python worker → host queue. It is over-engineered for the actual goal — open a Python editor and script the document — and exposes only a tiny, ad-hoc `ocs` API. It also serializes every host mutation through a slow JSON-RPC loop.

`ocs_python_repl` replaces it with a simpler, faster architecture: a normal API V3 plugin that spawns a local Python REPL, gives it full access to the document via a shared-memory snapshot, and collects mutations through a shared-memory queue.

---

## 2. Key decisions

1. **API V3 is the foundation.** `ocs_python_repl` is a standard `cdylib` plugin. It uses the existing plugin runner for process lifecycle, health monitoring, and control-plane IPC.
2. **API V3 is bypassed for the data path.** The simplified `DocumentReader` and per-entity `AddEntity`/`UpdateEntity`/`RemoveEntity` requests are too limited and too slow for a Pythonic REPL. Full document access and batched mutations go through shared memory.
3. **Full document snapshot is serialized with `serde` + `bincode`.** `acadrust` (the `acadifc` crate) supports `serde` but not `rkyv`. The host serializes the full `CadDocument` into a memory-mapped file; the Python extension deserializes it once per snapshot. This is one copy per refresh, not strict zero-copy, but it avoids per-entity IPC.
4. **Mutations are batched through a shared-memory queue.** `EntityOp` records (Add/Update/Remove) are serialized with `bincode` and written to a lock-free ring buffer. The host drains and applies them on a `PluginAsync` signal.
5. **PyO3 exposes real `acadifc` types to Python.** The user writes normal Python against `ocs.Line`, `ocs.Circle`, `ocs.doc.layers`, etc.
6. **Platform-independent protection.** The Python child is a separate process with a heartbeat, timeout, graceful cancellation, and stderr capture. No OS-specific sandboxing in v1.
7. **External editors only in v1.** The host panel system only supports abstract widgets (`Label`, `Button`, `TextInput`, `MultilineOutput`, `List`), so an embedded code editor is deferred. Supported editors: Zed (`zed <workspace>`), Gram (`gram <workspace>` / `gram -n <workspace>`), Lite XL (`lite-xl <workspace>`), VS Code (`code <workspace>`), Lapce (`lapce <workspace>`).

---

## 3. Architecture

```text
User clicks PYTHONEDIT
        │
        ▼
┌─────────────────────────────────────┐
│  ocs_python_repl (API V3 cdylib)    │
│  - verify Python + PIP              │
│  - create workspace                 │
│  - launch external editor           │
│  - spawn Python REPL process          │
└─────────────────────────────────────┘
        │ passes SHM paths + tab id + control socket
        ▼
Python REPL process (local interpreter)
        │
        ├─ loads `ocs_acadifc` extension (PyO3)
        ├─ reads full `CadDocument` from shared-memory snapshot
        ├─ writes/erases via shared-memory mutation queue
        ├─ sends control messages via local socket
        └─ debug via `debugpy`
        │
        ▼
Host document (OpenCAD Studio)
```

The Python session is tied to the plugin runner / host tab, not to the external editor. The editor is the primary interface; the plugin panel is for status and output. When the tab is closed, the host exits, or the user clicks Stop, the runner kills the Python child and removes the temp workspace. Each tab gets its own Python process, workspace, and document snapshot when PYTHONEDIT is invoked.

---

## 4. API V3 usage

Used for:
- Plugin lifecycle (spawn, health, shutdown, stderr capture, timeouts).
- Control-plane messages: `push_info`, `push_error`, `open_panel`, `request_point_pick`, `set_dirty`, `push_undo`.
- Async notification: `PluginAsync::DocumentRefreshRequested` tells the host to drain the mutation queue.

Bypassed for:
- Full document reads (too limited through `DocumentReader`).
- Per-entity mutations (too slow through IPC).

This keeps the API V3 surface stable while adding only:
- `HostApi::document_full_snapshot()` — returns the path to the full `CadDocument` snapshot.
- `HostApi::document_mutation_queue()` — returns the path to the mutation queue.
- `PluginAsync::DocumentRefreshRequested` — fire-and-forget queue-drain signal.
- `PluginRequest::EntityBatch` / `PluginResponse::BatchResult` — IPC fallback only.

---

## 5. Shared-memory data path

### 5.1 Full document snapshot

Add to `crates/ocs_plugin_api/src/shm.rs`:

```rust
pub struct DocumentFullSnapshotStore { ... }   // host side
pub struct DocumentFullSnapshotReader { ... }   // Python side

pub struct DocumentFullSnapshotInfo {
    pub path: String,
    pub version: u64,
}
```

- The host serializes the full `CadDocument` with `bincode` (requires `acadrust/serde`).
- The file is sized dynamically or starts at a large default (e.g., 64 MB) and grows on overflow.
- The Python extension maps the file, deserializes it once, and wraps the `acadifc` objects in PyO3 classes.
- `ocs.doc` is a fresh snapshot every time the host publishes a new version (e.g., after mutations are applied or on user request).

### 5.2 Mutation queue

Add to `crates/ocs_plugin_api/src/shm.rs`:

```rust
pub struct DocumentMutationQueue { ... }     // host side
pub struct DocumentMutationView { ... }       // Python side

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EntityOp {
    Add(EntityType),
    Update(EntityType),
    Remove(Handle),
}
```

- Fixed-size ring buffer with atomic read/write indices.
- Default capacity: 65,536 records. This allows a single `commit()` for at least 10,000 entities without backpressure.
- Records are serialized with `bincode`.
- Host validates every record before applying: entity type is valid, handle exists for Update/Remove, target layer is not locked, no duplicate handles on Add.
- Invalid records are skipped and reported as errors via `push_error`.

### 5.3 Mutation flow

1. User script calls `ocs.doc.add(...)`, `entity.update()`, or `entity.delete()`.
2. Python extension appends `EntityOp` records to the shared-memory queue.
3. On `ocs.doc.commit()` the Python extension writes a `DocumentRefreshRequested` control message to the local socket.
4. The plugin runner receives the message and forwards it to the host as `PluginAsync::DocumentRefreshRequested`.
5. The host drains the queue, applies the batch, pushes an undo snapshot, sets dirty, bumps geometry, and publishes a new full snapshot.
6. The Python extension sees the new version on next access to `ocs.doc`.

---

## 6. Pythonic `acadifc` object model

`acadifc` is the GitHub org/repo; the published crate is `acadrust`. The PyO3 extension wraps `acadrust` types.

### 6.1 Core Python API

```python
import ocs

# Read
for e in ocs.doc.entities:
    if e.layer.name == "0":
        print(e.handle, e.kind, e.point)

# Bulk add (preferred for performance)
points = [ocs.Point(x, y, 0, layer="POINTS") for x, y in zip(xs, ys)]
ocs.doc.add_many(points)
ocs.doc.commit()

# Single add / modify / erase
line = ocs.Line(start=(0, 0, 0), end=(100, 100, 0), layer="LINES")
ocs.doc.add(line)
e.delete()
ocs.doc.commit()

# Debug
ocs.debug.start(port=5678)
ocs.debug.wait_for_client()
```

### 6.2 Performance-oriented helpers

To keep the 1000-point roundtrip fast, expose bulk operations that cross the Python/Rust boundary once:

- `ocs.doc.add_many(iterable)` — append all records to the mutation queue in one Rust call.
- `ocs.doc.remove_many(handles)` — stage `Remove` records for many handles at once.
- `ocs.doc.clear_layer(name)` / `ocs.doc.remove_all()` — bulk erase.
- `ocs.doc.refresh()` — explicitly reload the document snapshot; otherwise the extension lazily checks the version on each access.
- `ocs.doc.version` — expose the current snapshot version so users can decide when to refresh.

`ocs.doc.entities` remains a lazy view that does not build a full Python list until iterated.

### 6.3 Phased type coverage

- **Phase 1:** `Vector3`, `Color`, `Handle`, `Point`, `Line`, `Circle`, `Arc`, `Text`, `Layer`, `EntityCollection`, `LayerCollection`, `CadDocument`.
- **Phase 2:** `Polyline`, `LwPolyline`, `Spline`, `Hatch`, `Dimension`, `Leader`, `Insert`, `Viewport`, `MText`.
- **Phase 3:** `BlockRecord`, `Block`, `Layout`, `TextStyle`, `DimStyle`, `Dictionary`, `Group`, `Material`, `PlotSettings`.
- **Phase 4:** `DxfReader`, `DwgReader`, `DxfWriter`, `DwgWriter`, `doc.save()`, `ocs.open()`.

### 6.4 Mutability model

- `ocs.doc` is a read-only snapshot.
- To modify, the user creates a new entity object or copies an existing one, stages it, and calls `commit()`.
- All mutations are applied as a batch on the host side.

---

## 7. Performance targets and optimization

### 7.1 Target

The roundtrip test must add and then remove 1000 random `Point` entities in **under 1 second total** on a typical developer machine, measured from the Python script.

Breakdown target:
- Add 1000 points and commit: < 300 ms
- Host apply + snapshot publish: < 200 ms
- Remove 1000 points and commit: < 300 ms
- Host apply + snapshot publish: < 200 ms

### 7.2 Optimization strategies

1. **Bulk Python/Rust boundary.** `add_many` / `remove_many` pass whole iterators across PyO3 once instead of once per entity.
2. **Lazy entity view.** `ocs.doc.entities` is a PyO3 view; it does not allocate a Python list until iterated or collected.
3. **Snapshot caching.** The Python extension caches the deserialized `CadDocument` and only reloads when the version counter changes.
4. **Large mutation queue.** 65,536 slots eliminate backpressure for the target test.
5. **One control message per commit.** A single `DocumentRefreshRequested` is sent for the whole batch, not per entity.
6. **Batch host apply.** The host applies all queued `EntityOp` records in one pass, pushing a single undo snapshot, setting dirty once, and bumping geometry once.
7. **Avoid extra snapshots.** Do not publish a new document snapshot until the batch is fully applied.

### 7.3 Roundtrip test

Add a test script and CI test in `crates/ocs_python_repl/tests/` that exercises the full add/remove path:

```python
import ocs, random, time

def roundtrip_1000_points():
    random.seed(42)
    pts = [(random.uniform(0, 1000), random.uniform(0, 1000), 0.0) for _ in range(1000)]

    t0 = time.perf_counter()
    ocs.doc.add_many(ocs.Point(x, y, z, layer="PTS") for x, y, z in pts)
    ocs.doc.commit()
    t1 = time.perf_counter()
    add_time = t1 - t0

    assert len(ocs.doc.entities) == 1000, f"expected 1000 points, got {len(ocs.doc.entities)}"

    t0 = time.perf_counter()
    for e in ocs.doc.entities:
        e.delete()
    ocs.doc.commit()
    t1 = time.perf_counter()
    remove_time = t1 - t0

    assert len(ocs.doc.entities) == 0, f"expected 0 points, got {len(ocs.doc.entities)}"
    return add_time, remove_time

if __name__ == "__main__":
    add_time, remove_time = roundtrip_1000_points()
    print(f"add 1000 points: {add_time:.3f}s")
    print(f"remove 1000 points: {remove_time:.3f}s")
    assert add_time + remove_time < 1.0, "roundtrip too slow"
```

The Rust integration test in `crates/ocs_python_repl/tests/ocs_python_repl.rs` runs the above script against a real host document, records the timings, and fails if the target is missed.

---

## 8. Protection measures

| Threat | Mitigation |
|--------|------------|
| Python crash | Python runs in a separate child process. Crash kills only the child. Runner detects death and restarts if configured. |
| Infinite loop | Heartbeat thread in the Python extension; runner kills child if heartbeat stops. Per-script timeout also applies. |
| High CPU | Python bootstrap calls `os.nice(5)` if available (Unix). |
| High memory | No hard cap in v1 (not cross-platform). Rely on process isolation and watchdog. |
| Malformed mutations | Host validates every `EntityOp` before applying; invalid records are skipped and reported. |
| Queue overflow | Queue is bounded; full queue returns `queue_full` error to the script. |
| File-system access | Python runs as the same OS user as the host. Workspace is in a temp directory. User is warned. |
| Stale resources | Runner kills Python child, removes workspace temp dir, and closes sockets on shutdown. Host removes shared-memory files on tab close. |
| Cancellation | UI `Stop` button sends a `cancel` control message to the Python extension, which raises `KeyboardInterrupt`. If no response within 2 s, runner calls `Child::kill()`. |

---

## 8.5 Build integration

The `ocs_python_repl` crate contains the API V3 plugin. The PyO3 extension lives in a sub-crate inside the same directory so all build artifacts stay within `crates/ocs_python_repl`:

```text
crates/ocs_python_repl/
├── Cargo.toml                  # V3 cdylib plugin
├── plugin.toml
├── src/                        # plugin source
│   ├── lib.rs
│   ├── python_env.rs
│   ├── repl.rs
│   ├── editor.rs
│   ├── workspace.rs
│   └── ...
└── ocs_acadifc/                # PyO3 extension crate
    ├── Cargo.toml              # crate-type = ["cdylib"]
    └── src/
        ├── lib.rs              # PyO3 module definition
        ├── document.rs
        ├── entities.rs
        ├── geometry.rs
        ├── tables.rs
        └── mutations.rs
```

Both crates are added to the workspace `Cargo.toml`. The CI build produces:
- `ocs_python_repl.dll/.so` (plugin)
- `ocs_acadifc.pyd/.so` (Python extension)

At runtime the plugin runner locates the extension next to the plugin binary (or in a known build directory) and copies it into the temp workspace. If the extension is missing, the runner falls back to running `cargo build -p ocs_acadifc` from the project root (requires the project source tree and a Rust toolchain). The user only needs a local Python interpreter; building OpenCAD Studio itself is not required for normal use.

---

## 9. Implementation phases

### Phase 1 — Skeleton + Python check
- Create `crates/ocs_python_repl` workspace member with `plugin.toml`, `Cargo.toml`, and `export_plugin!` entry.
- Implement `python_env::ensure_python()` and `ensure_package("debugpy")`.
- On `PYTHONEDIT`, open a status panel; if Python is missing, show a clear error.
- **Acceptance:** Missing Python shows an error; present Python shows the path in the panel.

### Phase 2 — External editor launch
- Create a temp workspace folder per session under the system temp directory. Include a `main.py` starter script and editor-specific config files.
- Detect and launch Zed / Gram / Lite XL / VS Code / Lapce with the workspace folder.
- Generate editor-specific settings (Python interpreter, debug launch for `debugpy` on port 5678).
- **Acceptance:** The selected editor opens the workspace folder with `main.py`.
- *Cleanup:* The runner removes the workspace when the session ends.

### Phase 3 — Full document snapshot + PyO3 skeleton
- Add `DocumentFullSnapshotStore` / `DocumentFullSnapshotReader` to `ocs_plugin_api::shm` using `bincode`.
- Add `HostApi::document_full_snapshot()`.
- Build a minimal PyO3 extension that exposes `ocs.doc.entities` and basic entity attributes.
- **Acceptance:** `print(len(ocs.doc.entities))` in the editor matches the host document entity count.

### Phase 4 — Mutation queue + basic write/erase
- Add `DocumentMutationQueue` / `DocumentMutationView` to `ocs_plugin_api::shm`.
- Add `HostApi::document_mutation_queue()`, `PluginAsync::DocumentRefreshRequested`, and `apply_entity_batch()`.
- Implement `ocs.doc.add()`, `entity.delete()`, `ocs.doc.commit()` in the PyO3 extension.
- **Acceptance:** Adding a line and committing creates it in the host; deleting removes it.

### Phase 5 — Expanded `acadifc` coverage
- Bind Phase 1 and Phase 2 entity types.
- Expose `doc.layers`, `doc.blocks`, `doc.styles`, and entity constructors.
- **Acceptance:** User scripts can read layers, add polylines/circles/text, and read block references.

### Phase 6 — Debugging
- Install `debugpy` via PIP if missing.
- Generate editor debug configs for VS Code / Zed / Gram.
- Provide `ocs.debug.start()`, `ocs.debug.wait_for_client()`.
- **Acceptance:** User can set a breakpoint and attach the editor debugger.

### Phase 7 — Performance roundtrip test
- Add `crates/ocs_python_repl/tests/bench_roundtrip_1000_points.py` and the Rust harness in `crates/ocs_python_repl/tests/ocs_python_repl.rs`.
- Implement `ocs.doc.add_many()`, `ocs.doc.remove_all()`, and snapshot caching so the test hits the performance target.
- Run the test: add 1000 random points, commit, assert count, remove all, commit, assert count is 0.
- **Target:** total wall time < 1 s. **Acceptance:** Test passes and prints timing breakdown.

### Phase 8 — Embedded editor (future)
- Requires a new `Widget::CodeEditor` variant in `ocs_plugin_api`. Out of scope for v1.

### Phase 9 — Remove `ocs_python_lsp`
- After `ocs_python_repl` has feature parity, remove `crates/ocs_python_lsp` from the workspace.

---

## 10. Risks and trade-offs

| Risk | Mitigation |
|------|------------|
| `acadrust` does not support `rkyv`; true zero-copy is not possible. | Use `bincode` + `serde`. Accept one copy per snapshot. This is still much faster than per-entity IPC. |
| Full `CadDocument` snapshot may exceed the default shared-memory size. | Start with a large default (64 MB) and dynamically grow the file. |
| PyO3 extension build is complex. | Keep the extension as a separate Cargo sub-crate inside `crates/ocs_python_repl`. CI builds both artifacts; the runner locates the pre-built extension at runtime. |
| Python environment varies across users. | Check Python version and PIP at startup; install `debugpy` automatically; fail with a clear message. |
| User scripts can access the file system. | Document the security model; run Python as the same OS user. |
| Debugging across processes is tricky. | Use `debugpy` and generate editor-specific launch configs. |

---

## 11. Resolved questions

- **Crate dependency:** `acadrust = "0.4"` from crates.io (or the GitHub patch); `acadifc` is the repo name.
- **Full document serialization:** `serde` + `bincode` (shared memory), not `rkyv`, because `acadrust` does not implement `rkyv::Archive`.
- **Mutation queue ownership:** Host creates and owns the queue; exposed via `HostApi::document_mutation_queue()`.
- **Host queue triggering:** `PluginAsync::DocumentRefreshRequested` from Python → runner → host.
- **Workspace:** Temp folder per session under the system temp directory; removed when the session ends.
- **Python session lifetime:** Tied to the plugin runner / host tab, not the external editor. Killed when the tab is closed, host exits, or user clicks Stop.
- **Multi-tab:** One Python process, workspace, and document snapshot per tab.
- **PyO3 extension build:** Separate Cargo sub-crate inside `crates/ocs_python_repl/ocs_acadifc`; built alongside the plugin.
- **Embedded editor:** Deferred to Phase 7; current panel system only supports abstract widgets.
- **Editor CLI commands:** Zed (`zed`), Gram (`gram` / `gram -n`), Lite XL (`lite-xl`), VS Code (`code`), Lapce (`lapce`).

---

## 12. Deliverables

- `crates/ocs_python_repl/` — new plugin crate with PyO3 extension build.
- Updated `crates/ocs_plugin_api`:
  - `DocumentFullSnapshotStore` / `DocumentFullSnapshotReader`.
  - `DocumentMutationQueue` / `DocumentMutationView`.
  - `HostApi::document_full_snapshot()` and `document_mutation_queue()`.
  - `PluginAsync::DocumentRefreshRequested`.
  - `PluginRequest::EntityBatch` / `PluginResponse::BatchResult` (fallback).
- New documentation: `crates/ocs_python_repl/README.md`, `crates/ocs_python_repl/PLUGIN.md`.
- New performance tests: `crates/ocs_python_repl/tests/bench_roundtrip_1000_points.py` and the Rust harness.
- Deprecation and eventual removal of `crates/ocs_python_lsp`.
