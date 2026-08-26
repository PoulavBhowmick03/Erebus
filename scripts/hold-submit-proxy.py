#!/usr/bin/env python3
"""Forward Starknet JSON-RPC but hold transaction submission for recovery canaries."""

from __future__ import annotations

import argparse
import json
import os
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def env_value(path: Path, name: str) -> str:
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith(name + "="):
            return line.split("=", 1)[1]
    raise SystemExit(f"{name} is missing from {path}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("env", type=Path)
    parser.add_argument("capture", type=Path)
    parser.add_argument("--port", type=int, default=18765)
    args = parser.parse_args()
    upstream = env_value(args.env, "STARKNET_RPC_URL")

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:  # noqa: N802
            length = int(self.headers.get("content-length", "0"))
            body = self.rfile.read(length)
            request = json.loads(body)
            if request.get("method") == "starknet_addInvokeTransaction":
                args.capture.parent.mkdir(parents=True, exist_ok=True)
                descriptor = os.open(args.capture, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(body)
                    stream.flush()
                    os.fsync(stream.fileno())
                # Keep the CLI inside submission until the canary kills it. Returning an
                # error here would advance the journal through an ordinary failure path.
                while True:
                    self.connection.settimeout(60)
                    try:
                        self.connection.recv(1)
                    except TimeoutError:
                        continue
                    return
            upstream_request = urllib.request.Request(
                upstream,
                data=body,
                headers={"content-type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(upstream_request, timeout=180) as response:
                payload = response.read()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *values: object) -> None:
            return

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"holding submissions on http://127.0.0.1:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
