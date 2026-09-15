"""Drive the actual Rust account path against a local RPC, including response loss."""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
from erebus import ErebusError, Seam

CLI = Path(__file__).resolve().parents[3] / "sdk/rs/target/debug/erebus-cli"
pytestmark = pytest.mark.skipif(not CLI.exists(), reason="build erebus-cli first")


@pytest.fixture
def chain():
    state = {
        "chain_id": "0x534e5f5345504f4c4941",
        "deployed": False,
        "submissions": [],
        "public_key": "0x0",
    }

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            method = request["method"]
            body = {"jsonrpc": "2.0", "id": request["id"]}
            if method == "starknet_chainId":
                body["result"] = state["chain_id"]
            elif method == "starknet_getClassHashAt":
                if state["deployed"]:
                    body["result"] = "0x123"
                else:
                    body["error"] = {"code": 20, "message": "Contract not found"}
            elif method == "starknet_estimateFee":
                body["result"] = [
                    {
                        "l1_gas_consumed": "0x1",
                        "l1_gas_price": "0x1",
                        "l2_gas_consumed": "0x1",
                        "l2_gas_price": "0x1",
                        "l1_data_gas_consumed": "0x1",
                        "l1_data_gas_price": "0x1",
                        "overall_fee": "0x3",
                        "unit": "FRI",
                    }
                ]
            elif method == "starknet_addDeployAccountTransaction":
                state["submissions"].append(request["params"])
                body["error"] = {
                    "code": -32603,
                    "message": "response lost after submission",
                }
            elif method == "starknet_call":
                body["result"] = [state["public_key"]]
            else:
                body["error"] = {"code": -32601, "message": method}
            encoded = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}", state
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_deployment_response_loss_replays_saved_request_byte_for_byte(chain, tmp_path):
    url, state = chain
    seam = Seam(binary=CLI)
    args = {"directory": str(tmp_path), "chain_id": state["chain_id"], "rpc_url": url}
    account = seam.call("onboarding", {"action": "create", "args": args})
    for _ in range(2):
        with pytest.raises(ErebusError, match="submission uncertain"):
            seam.call("onboarding", {"action": "deploy", "args": args})
    assert len(state["submissions"]) == 2
    assert state["submissions"][0] == state["submissions"][1]
    saved = json.loads((tmp_path / "deployment.json").read_text())
    assert saved["transaction_hash"].startswith("0x")
    assert "private_key" not in saved
    assert account["address"] != "0x0"
    state["deployed"] = True
    result = seam.call("onboarding", {"action": "deploy", "args": args})
    assert result["deployed"] is True
    assert len(state["submissions"]) == 2


def test_wrong_chain_is_rejected_before_deployment(chain, tmp_path):
    url, state = chain
    seam = Seam(binary=CLI)
    args = {"directory": str(tmp_path), "chain_id": "0x534e5f4d41494e", "rpc_url": url}
    seam.call("onboarding", {"action": "create", "args": args})
    with pytest.raises(ErebusError, match="chain does not match"):
        seam.call("onboarding", {"action": "deploy", "args": args})
    assert state["submissions"] == []
    assert not (tmp_path / "deployment.json").exists()


def test_existing_address_requires_matching_signer(chain, tmp_path):
    url, state = chain
    seam = Seam(binary=CLI)
    args = {"directory": str(tmp_path), "chain_id": state["chain_id"], "rpc_url": url}
    account = seam.call("onboarding", {"action": "create", "args": args})
    args.update(
        address=account["address"], account_key_file=account["account_key_file"]
    )
    with pytest.raises(ErebusError, match="does not match"):
        seam.call("onboarding", {"action": "verify_signer", "args": args})
    state["public_key"] = account["public_key"]
    assert seam.call("onboarding", {"action": "verify_signer", "args": args}) == {
        "verified": True
    }
    assert not state["submissions"]
