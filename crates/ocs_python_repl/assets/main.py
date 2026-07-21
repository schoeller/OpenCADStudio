import json
import os
import sys
import ocs

# Starter script for the OpenCAD Studio Python REPL.
# Use ocs.doc to read and modify the host document.
#
# To debug, uncomment the lines below and start the debugger attach config in
# your editor (VS Code or Zed) before the script continues:
#
#   ocs.debug.start()
#   ocs.debug.wait_for_client()

print("OpenCAD Studio Python REPL ready.")
print(f"Entities: {len(ocs.doc.entities)}")
print(f"Snapshot version: {ocs.doc.version}")
