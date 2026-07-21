import json
import os
import sys
import ocs

# Starter script for the OpenCAD Studio Python REPL.
# Use ocs.doc to read and modify the host document.

print("OpenCAD Studio Python REPL ready.")
print(f"Entities: {len(ocs.doc.entities)}")
print(f"Snapshot version: {ocs.doc.version}")
