#!/usr/bin/env python3
"""Stdio <-> TCP bridge for the OpenCAD Studio Python LSP server.

The external editor launches this script as its Python language server. We
read `ocs_lsp.json` from the script's directory, connect to the LSP server
listening on localhost:port, and forward LSP JSON-RPC messages in both
directions until EOF.
"""

import json
import os
import socket
import sys
import threading


def forward(source, sink, name):
    """Copy bytes from source to sink until source returns EOF."""
    try:
        while True:
            chunk = source.read(4096)
            if not chunk:
                break
            sink.write(chunk)
            sink.flush()
    except (OSError, ValueError):
        pass
    finally:
        try:
            sink.close()
        except Exception:
            pass


def main():
    script_dir = os.path.dirname(os.path.abspath(__file__))
    config_path = os.path.join(script_dir, "ocs_lsp.json")
    with open(config_path, "r", encoding="utf-8") as f:
        config = json.load(f)

    port = config["port"]
    # tab is informational for diagnostics; the server already recorded it.
    _tab = config.get("tab", 0)

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    # Retry briefly in case the server is still starting up.
    for _ in range(50):
        try:
            sock.connect(("127.0.0.1", port))
            break
        except ConnectionRefusedError:
            import time
            time.sleep(0.1)
    else:
        print(f"Failed to connect to LSP server on port {port}", file=sys.stderr)
        sys.exit(1)

    # Use binary buffered I/O to preserve LSP Content-Length framing.
    sock_file = sock.makefile("rwb", buffering=0)
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer

    t1 = threading.Thread(target=forward, args=(stdin, sock_file, "stdin->socket"))
    t2 = threading.Thread(target=forward, args=(sock_file, stdout, "socket->stdout"))
    t1.start()
    t2.start()
    t1.join()
    t2.join()


if __name__ == "__main__":
    main()
