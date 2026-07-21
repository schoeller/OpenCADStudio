# Plan: Python Shell for OpenCAD Studio

**Scope:** `crates/ocs_pythonshell` is the interactive Python REPL plugin.

**Goal:** Provide a dockable Python REPL panel that can read and write the
active document through the API v3 host surface.

## Architecture

- `src/lib.rs` — plugin entry point, REPL panel, command dispatch.
- Embedded Python bootstrap exposes an `ocs` object (`ocs.doc.entities()`,
  `ocs.add_point(...)`, XDATA helpers, etc.).
- Host API calls travel as JSON on the child `stderr`; replies and code are
  sent on `stdin`; user output appears on `stdout`.

## Out of scope

Editor integration is now provided by `crates/ocs_python_repl`; the old `crates/ocs_python_lsp` LSP-based approach has been removed.
