"""The live Monad rehearsal refuses unsafe plans and never broadcasts without explicit authorization."""

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/metropolis-monad-rehearsal.py"
spec = importlib.util.spec_from_file_location("monad_rehearsal", SCRIPT)
rehearsal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rehearsal)


@pytest.mark.parametrize("data,digest", [
    (b"", "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"),
    (b"Transfer(address,address,uint256)", "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"),
    (b"x" * 200, None),
])
def test_keccak_matches_ethereum_vectors(data, digest):
    value = rehearsal.keccak256(data).hex()
    if digest:
        assert value == digest
    else:  # crosses the 136-byte rate boundary; compared with Foundry's cast during development
        assert value.startswith("3c3800defb6a25a7")


def test_run_refuses_without_live_authorization(tmp_path):
    done = subprocess.run([sys.executable, str(SCRIPT), "run", "--workdir", str(tmp_path)], capture_output=True, text=True)
    assert done.returncode != 0 and "--authorize-live-transactions" in done.stderr


@pytest.mark.parametrize("change,message", [({"rail": "upto"}, "rail"), ({"peer_rpc_url": "https://testnet-rpc.monad.xyz/"}, "distinct")])
def test_init_rejects_unsupported_rails_and_unpaired_observation(tmp_path, change, message):
    plan = {"rail": "public-bound", "rpc_url": "https://testnet-rpc.monad.xyz", "peer_rpc_url": "https://rpc-testnet.monadinfra.com",
            "bin_dir": str(tmp_path), **change}
    (tmp_path / "plan.json").write_text(json.dumps(plan))
    with pytest.raises(rehearsal.Rehearsal, match=message):
        rehearsal.init(tmp_path / "plan.json", tmp_path / "work")
    assert not (tmp_path / "work").exists()
