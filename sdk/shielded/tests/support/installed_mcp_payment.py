"""Call the installed Metropolis MCP server in the loopback sponsored-payment fixture."""

import asyncio
import json
import os
import sys
from pathlib import Path

import erebus
import erebus_mcp
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client


async def main():
    bins, negotiation, payment, method, operation = sys.argv[1:]
    bins = Path(bins)
    assert Path(erebus.__file__).resolve().is_relative_to(bins.parent.resolve())
    assert Path(erebus_mcp.__file__).resolve().is_relative_to(bins.parent.resolve())
    config = json.loads(Path(payment).read_text())
    sponsored = config.get("relay") is not None
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith("EREBUS_") and key not in {"PYTHONPATH", "VIRTUAL_ENV"}}
    environment.update({
        "EREBUS_BACKEND": "metropolis",
        "EREBUS_NEGOTIATION_CONFIG": negotiation,
        "EREBUS_PAYMENT_CONFIG": payment,
        "EREBUS_NEGOTIATION_CLI": str(bins / "erebus-negotiate"),
        "EREBUS_PAYMENT_CLI": str(bins / "erebus-payment"),
        "EREBUS_NATIVE_TIMEOUT_SECONDS": "120",
    })
    params = StdioServerParameters(command=str(bins / "erebus-mcp-server"), env=environment)
    log = Path(payment).with_suffix(".mcp.stderr.log")
    descriptor = os.open(log, os.O_CREAT | os.O_WRONLY | os.O_APPEND, 0o600)
    with os.fdopen(descriptor, "a") as errors:
        async with stdio_client(params, errlog=errors) as (read, write):
            async with ClientSession(read, write, read_timeout_seconds=150.0) as session:
                await session.initialize()
                tools = {tool.name for tool in (await session.list_tools()).tools}
                submit = "sponsor_shielded_deal" if sponsored else "settle_deal"
                excluded = "settle_deal" if sponsored else "sponsor_shielded_deal"
                assert submit in tools and excluded not in tools
                assert "recover_deal" in tools
                tool = "recover_deal" if method == "observe" else submit
                result = await session.call_tool(tool, {"operation_ref": operation})
                assert not result.is_error
                envelope = result.structured_content
                if envelope is None:
                    envelope = json.loads(result.content[0].text)
                if "result" in envelope:
                    print(json.dumps(envelope["result"]))
                else:
                    assert envelope["error"]["category"] == "METROPOLIS_UNAVAILABLE"
                    print(json.dumps({"status": "error"}))


if __name__ == "__main__":
    asyncio.run(main())
