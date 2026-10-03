"""The Python seam forwards paths and rejects private or malformed binary output."""

import json
import subprocess

import pytest

from erebus import LocalProvingError, LocalProvingSeam


def request():
    return dict(manifest_file="manifest.json", manifest_sha256="ab" * 32, circuit="deposit",
                cache_root="cache", witness_file="witness.json", expected_public=["1"] * 6)


def response():
    return {"status": "ok", "calldata": [["1", "2"], [["3", "4"], ["5", "6"]], ["7", "8"], ["1"] * 6],
            "downloaded_bytes": 123, "cache_hits": 0, "installation_ms": 10, "proving_ms": 100}


def fake(monkeypatch, value, returncode=0, stderr=""):
    seen = []
    def run(args, **kwargs):
        seen.append((args, kwargs))
        return subprocess.CompletedProcess(args, returncode, json.dumps(value), stderr)
    monkeypatch.setattr(subprocess, "run", run)
    return seen


def test_paths_only_with_development_exceptions_off_by_default(monkeypatch):
    seen = fake(monkeypatch, response())
    result = LocalProvingSeam(binary="local-prove").prove(**request())
    assert result == response()
    args, params = seen[0]
    assert args == ["local-prove"]
    payload = json.loads(params["input"])
    assert payload["witness_file"] == "witness.json"
    assert "witness" not in payload and "key" not in payload
    assert payload["allow_test_artifacts"] is False and payload["allow_loopback_http"] is False


def test_version_checks_protocol_without_reading_a_witness(monkeypatch):
    seen = fake(monkeypatch, {"status": "ok", "protocol": 1, "methods": ["prove"]})
    assert LocalProvingSeam(binary="local-prove").version()["protocol"] == 1
    assert seen[0][0] == ["local-prove", "--version"]
    assert seen[0][1]["input"] is None


@pytest.mark.parametrize("field,value", [
    ("circuit", {}), ("circuit", "invalid"), ("expected_public", ["1"]),
    ("expected_public", ["01"] * 6), ("manifest_sha256", "not-a-hash"),
    ("allow_test_artifacts", 1), ("witness_file", ""),
])
def test_invalid_requests_do_not_spawn_a_process(monkeypatch, field, value):
    seen = fake(monkeypatch, response())
    params = request()
    params[field] = value
    with pytest.raises(LocalProvingError):
        LocalProvingSeam(binary="local-prove").prove(**params)
    assert seen == []


@pytest.mark.parametrize("mutation", [
    lambda output: output.update(witness="private"),
    lambda output: output.update(downloaded_bytes=True),
    lambda output: output.update(cache_hits=4),
    lambda output: output.update(proving_ms=-1),
    lambda output: output.update(calldata=[]),
    lambda output: output["calldata"].__setitem__(0, ["secret", "2"]),
    lambda output: output["calldata"].__setitem__(3, ["2"] * 6),
])
def test_private_or_malformed_response_is_never_returned(monkeypatch, mutation):
    output = response()
    mutation(output)
    fake(monkeypatch, output)
    with pytest.raises(LocalProvingError):
        LocalProvingSeam(binary="local-prove").prove(**request())


@pytest.mark.parametrize("code,stderr", [(1, ""), (0, "private provider detail")])
def test_failed_process_and_stderr_are_redacted(monkeypatch, code, stderr):
    fake(monkeypatch, {"status": "error", "error": "private provider detail"}, code, stderr)
    with pytest.raises(LocalProvingError) as error:
        LocalProvingSeam(binary="local-prove").prove(**request())
    assert "private provider detail" not in str(error.value)
