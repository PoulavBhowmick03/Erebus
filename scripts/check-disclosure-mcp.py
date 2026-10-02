"""Fresh auditor MCP process: only a grant, key file, and public expected issuer."""

import asyncio
import json
import sys
from pathlib import Path

from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client


async def check(binary: str, directory: str, issuer: str, deployment: str | None = None) -> None:
    parameters = StdioServerParameters(
        command=sys.executable, args=["-m", "erebus_mcp.server"], cwd=directory,
        env={"EREBUS_BACKEND": "disclosure", "EREBUS_DISCLOSURE_DIR": directory,
             "EREBUS_DISCLOSURE_CLI": binary},
    )
    if deployment is not None:
        parameters.env["EREBUS_DISCLOSURE_DEPLOYMENT"] = deployment
    async with stdio_client(parameters) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            response = await session.call_tool("verify_disclosed_payment" if deployment else "verify_disclosed_agreement", {
                "grant_name": "deal.grant", "expected_issuer": issuer,
            })
            result = response.structured_content or json.loads(response.content[0].text)
            assert result["ok"] is True, "MCP auditor rejected the grant"
            facts = result["result"]
            assert facts["agreement_verified"] is True
            assert facts["payment_verified"] is bool(deployment)
            assert facts["delivery_verified"] is False
            print(json.dumps(facts))


if __name__ == "__main__":
    binary, directory, issuer, *deployment = sys.argv[1:]
    expected = {"auditor.key", "deal.grant"}
    if deployment:
        expected.add("public-cache")
    assert set(path.name for path in Path(directory).iterdir()) == expected
    asyncio.run(check(binary, directory, issuer, deployment[0] if deployment else None))
