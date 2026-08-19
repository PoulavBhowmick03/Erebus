"""Compatibility shim: the server moved into the package as `erebus_mcp.server`.

Kept so existing launch paths stay valid — `uv run mcp dev mcp-server/src/server.py`
(mcp dev inspects a file for a module-level server object, so this shim builds one at
import), agent configs pointing stdio at this path, and `uv run python` invocations.
New code should prefer the installed entry point, `erebus-mcp-server`.
"""

from erebus_mcp.server import build_server

server = build_server()

if __name__ == "__main__":
    server.run()
