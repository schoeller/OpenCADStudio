import json
import os
import sys

import ocs

# Self-initialize when this script is run directly (e.g. by Zed's debugpy
# runner) instead of through the REPL bootstrap.
if not getattr(ocs, "doc", None):
    # Zed's debugpy runner sometimes executes the script in a context where
    # __file__ is not defined. Fall back to sys.argv[0] or the current directory.
    script_path = globals().get("__file__") or sys.argv[0]
    if script_path:
        script_dir = os.path.dirname(os.path.abspath(script_path))
    else:
        script_dir = os.getcwd()
    config_path = os.path.join(script_dir, "_ocs_config.json")
    with open(config_path) as f:
        cfg = json.load(f)
    ocs._init(
        cfg["snapshot_path"],
        cfg["queue_path"],
        cfg["control_socket"],
    )

# Starter script for the OpenCAD Studio Python REPL.
# Use ocs.doc to read and modify the host document.

print("OpenCAD Studio Python REPL ready.")
print(f"Entities: {len(ocs.doc.entities)}")
print(f"Snapshot version: {ocs.doc.version}")
