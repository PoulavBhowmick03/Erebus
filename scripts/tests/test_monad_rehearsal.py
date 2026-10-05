"""The live Monad rehearsal refuses unsafe plans and never broadcasts without explicit authorization."""

import importlib.util
import json
import subprocess
import sys
import time
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


@pytest.mark.parametrize("value", [0, -1, 3601, True, "600", 1.5])
def test_phase_timeout_rejects_invalid_bounds(value):
    with pytest.raises(rehearsal.Rehearsal, match="between 1 and 3600"):
        rehearsal.phase_timeout({"verification_timeout_seconds": value}, "verification_timeout_seconds")


def test_verification_phase_allows_a_longer_bounded_window():
    plan = {"verification_timeout_seconds": 7200, "negotiation_timeout_seconds": 7200}
    assert rehearsal.phase_timeout(plan, "verification_timeout_seconds", rehearsal.VERIFICATION_SECONDS) == 7200
    with pytest.raises(rehearsal.Rehearsal, match="between 1 and 86400"):
        rehearsal.phase_timeout({"verification_timeout_seconds": rehearsal.VERIFICATION_SECONDS + 1},
                                "verification_timeout_seconds", rehearsal.VERIFICATION_SECONDS)
    with pytest.raises(rehearsal.Rehearsal, match="between 1 and 3600"):
        rehearsal.phase_timeout(plan, "negotiation_timeout_seconds")


def test_poll_only_retries_pending_observations(monkeypatch):
    replies = iter([(2, {"status": "pending"}), (0, {"status": "ok", "payment_verified": True})])
    timeouts = []
    monkeypatch.setattr(rehearsal.time, "sleep", lambda _: None)
    def observe(timeout):
        timeouts.append(timeout)
        return next(replies)
    reply = rehearsal.poll_reply(observe, lambda reply: reply.get("payment_verified") is True, 5, "observation")
    assert reply["payment_verified"] is True
    assert len(timeouts) == 2 and all(0 < timeout <= 5 for timeout in timeouts)


def test_poll_times_out_without_signing_or_retrying_payment(monkeypatch):
    clock = [0.0]
    calls = []
    monkeypatch.setattr(rehearsal.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(rehearsal.time, "sleep", lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    def observe(timeout):
        calls.append(timeout)
        return 2, {"status": "pending"}
    with pytest.raises(rehearsal.Rehearsal, match="recover without a new payment"):
        rehearsal.poll_reply(observe, lambda reply: False, 3, "finality")
    assert calls == [3, 1]


@pytest.mark.parametrize("code,status", [(1, "error"), (0, "closed_unpaid"), (2, "funding_required")])
def test_poll_fails_closed_on_permanent_errors(code, status):
    calls = []
    def observe(timeout):
        calls.append(timeout)
        return code, {"status": status, "error": "private details must not escape"}
    with pytest.raises(rehearsal.Rehearsal, match="recover without a new payment") as error:
        rehearsal.poll_reply(observe, lambda reply: True, 1, "observation")
    assert len(calls) == 1
    assert "private details" not in str(error.value)


def test_poll_retries_a_killed_call_without_losing_durable_progress(monkeypatch):
    clock = [0.0]
    calls = []
    monkeypatch.setattr(rehearsal.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(rehearsal.time, "sleep", lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    def observe(timeout):
        calls.append(timeout)
        if len(calls) == 1:
            raise rehearsal.CommandTimeout("erebus-payment timed out")
        return 2, {"status": "pending", "history_pending": True, "next_log_block": 11, "ancestry_block": 11}
    with pytest.raises(rehearsal.Rehearsal, match="timed out"):
        rehearsal.poll_reply(observe, lambda reply: False, 5, "history")
    assert calls == [5, 3, 1]


def test_poll_fails_closed_when_history_never_advances(monkeypatch):
    clock = [0.0]
    calls = []
    monkeypatch.setattr(rehearsal.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(rehearsal.time, "sleep", lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    def observe(timeout):
        calls.append(timeout)
        return 2, {"status": "pending", "history_pending": True, "next_log_block": 7, "ancestry_block": 7}
    with pytest.raises(rehearsal.Rehearsal, match="made no progress") as error:
        rehearsal.poll_reply(observe, lambda reply: False, 600, "funding and retained history")
    assert len(calls) == rehearsal.STALL_LIMIT + 1
    assert "next_log_block" not in str(error.value)


def test_poll_keeps_polling_a_completed_scan_waiting_for_finality(monkeypatch):
    clock = [0.0]
    calls = []
    monkeypatch.setattr(rehearsal.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(rehearsal.time, "sleep", lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    def observe(timeout):
        calls.append(timeout)
        if len(calls) < 3:
            return 2, {"status": "pending", "payment_verified": False}
        return 0, {"status": "ok", "payment_verified": True}
    reply = rehearsal.poll_reply(observe, lambda reply: reply.get("payment_verified") is True, 30, "finality observation")
    assert reply["payment_verified"] is True
    assert len(calls) == 3


def test_killed_calls_fail_closed_after_the_stall_window(monkeypatch):
    clock = [0.0]
    calls = []
    monkeypatch.setattr(rehearsal.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(rehearsal.time, "sleep", lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    monkeypatch.setattr(rehearsal, "STALL_SECONDS", 4)
    def observe(timeout):
        calls.append(timeout)
        raise rehearsal.CommandTimeout("erebus-payment timed out")
    with pytest.raises(rehearsal.Rehearsal, match="made no progress"):
        rehearsal.poll_reply(observe, lambda reply: False, 600, "history")
    assert len(calls) == 3


def test_read_only_poll_recovers_after_a_transient_error(monkeypatch):
    replies = iter([(1, {"status": "error", "error": "private"}),
                    (2, {"status": "pending", "history_pending": True, "next_log_block": 5, "ancestry_block": 5}),
                    (0, {"status": "ok", "payment_verified": True})])
    monkeypatch.setattr(rehearsal.time, "sleep", lambda _: None)
    reply = rehearsal.poll_reply(lambda timeout: next(replies),
                                 lambda reply: reply.get("payment_verified") is True,
                                 60, "finality", retry_errors=True)
    assert reply["payment_verified"] is True


def test_read_only_poll_fails_closed_when_errors_never_clear(monkeypatch):
    clock = [0.0]
    calls = []
    monkeypatch.setattr(rehearsal.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(rehearsal.time, "sleep", lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    monkeypatch.setattr(rehearsal, "STALL_SECONDS", 4)
    def observe(timeout):
        calls.append(timeout)
        return 1, {"status": "error", "error": "private details must not escape"}
    with pytest.raises(rehearsal.Rehearsal, match="recover without a new payment") as error:
        rehearsal.poll_reply(observe, lambda reply: True, 600, "funding and retained history", retry_errors=True)
    assert len(calls) == 3
    assert "private details" not in str(error.value)


def test_read_only_poll_does_not_retry_errors_without_opt_in(monkeypatch):
    calls = []
    def observe(timeout):
        calls.append(timeout)
        return 1, {"status": "error"}
    with pytest.raises(rehearsal.Rehearsal, match="recover without a new payment"):
        rehearsal.poll_reply(observe, lambda reply: True, 60, "auditor verification")
    assert len(calls) == 1


def test_command_timeout_is_a_rehearsal_failure():
    assert issubclass(rehearsal.CommandTimeout, rehearsal.Rehearsal)


def settle_setup(monkeypatch, calls, outcome):
    def run(bin_dir, name, request, cwd, timeout):
        calls.append((name, request["method"], timeout))
        if isinstance(outcome, Exception):
            raise outcome
        return outcome
    monkeypatch.setattr(rehearsal, "command", run)
    return lambda method: {"method": method}


def test_settle_timeout_never_retries_the_submission(monkeypatch, tmp_path):
    calls = []
    request = settle_setup(monkeypatch, calls, rehearsal.CommandTimeout("timed out"))
    reply = rehearsal.settle_once(tmp_path, request, tmp_path, 600)
    assert reply == {"status": "pending", "payment_verified": False}
    assert calls == [("erebus-payment", "settle", 600)]


@pytest.mark.parametrize("code", [1, 3])
def test_settle_error_reply_becomes_observe_only(monkeypatch, tmp_path, code):
    calls = []
    request = settle_setup(monkeypatch, calls, (code, {"status": "error", "error": "private"}))
    reply = rehearsal.settle_once(tmp_path, request, tmp_path, 600)
    assert reply["status"] == "pending" and reply["payment_verified"] is False
    assert len(calls) == 1 and "private" not in str(reply)


@pytest.mark.parametrize("code,reply", [
    (0, {"status": "ok", "payment_verified": True, "stage": "Finalized"}),
    (2, {"status": "pending", "payment_verified": False, "history_pending": True}),
])
def test_settle_returns_a_real_reply_unchanged(monkeypatch, tmp_path, code, reply):
    calls = []
    request = settle_setup(monkeypatch, calls, (code, reply))
    assert rehearsal.settle_once(tmp_path, request, tmp_path, 600) == reply
    assert len(calls) == 1


def test_settle_catch_up_resumes_only_before_any_attempt(monkeypatch, tmp_path):
    calls = []

    def run(bin_dir, name, request, cwd, timeout=900):
        method = request["method"]
        calls.append(method)
        if method == "settle":
            if calls.count("settle") == 1:
                return 2, {"status": "pending", "payment_verified": False, "history_pending": True,
                           "broadcast_attempts": 0, "stage": "Authorized"}
            return 0, {"status": "ok", "payment_verified": True, "broadcast_attempts": 1, "stage": "Finalized"}
        if method == "observe":
            return 2, {"status": "pending", "payment_verified": False, "history_pending": False,
                       "broadcast_attempts": 0, "stage": "Authorized"}
        raise AssertionError(method)

    monkeypatch.setattr(rehearsal, "command", run)
    monkeypatch.setattr(rehearsal.time, "sleep", lambda _: None)
    reply = rehearsal.settle_with_catch_up(tmp_path, lambda method: {"method": method}, tmp_path, 60)
    assert reply["payment_verified"] is True
    assert calls.count("settle") == 2
    assert calls.count("observe") == 1


@pytest.mark.parametrize("attempts,stage", [(1, "Finalized"), (None, "Authorized")])
def test_settle_catch_up_never_resumes_after_an_attempt_or_without_proof(monkeypatch, tmp_path, attempts, stage):
    calls = []

    def run(bin_dir, name, request, cwd, timeout=900):
        calls.append(request["method"])
        reply = {"status": "pending", "payment_verified": False, "history_pending": True, "stage": stage}
        if attempts is not None:
            reply["broadcast_attempts"] = attempts
        return 2, reply

    monkeypatch.setattr(rehearsal, "command", run)
    monkeypatch.setattr(rehearsal.time, "sleep", lambda _: None)
    reply = rehearsal.settle_with_catch_up(tmp_path, lambda method: {"method": method}, tmp_path, 60)
    assert reply["history_pending"] is True
    assert calls == ["settle"]


def test_settle_catch_up_times_out_without_submitting(monkeypatch, tmp_path):
    clock = [0.0]
    calls = []
    monkeypatch.setattr(rehearsal.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(rehearsal.time, "sleep", lambda seconds: clock.__setitem__(0, clock[0] + seconds))

    def run(bin_dir, name, request, cwd, timeout=900):
        calls.append(request["method"])
        if request["method"] == "settle":
            return 2, {"status": "pending", "payment_verified": False, "history_pending": True,
                       "broadcast_attempts": 0, "stage": "Authorized"}
        return 2, {"status": "pending", "payment_verified": False, "history_pending": True,
                   "next_log_block": 5, "ancestry_block": 5, "broadcast_attempts": 0}

    monkeypatch.setattr(rehearsal, "command", run)
    with pytest.raises(rehearsal.Rehearsal, match="recover without a new payment"):
        rehearsal.settle_with_catch_up(tmp_path, lambda method: {"method": method}, tmp_path, 4)
    assert "settle" in calls and "observe" in calls


@pytest.mark.parametrize("failed_peer", [False, True])
def test_negotiation_timeout_or_failed_peer_cleans_up_both_processes(tmp_path, monkeypatch, failed_peer):
    popen = subprocess.Popen
    children = []
    for role in ("buyer", "seller"):
        (tmp_path / role).mkdir()
    def launch(args, **kwargs):
        role = Path(kwargs["cwd"]).name
        code = "import sys,time; sys.stdin.read(); time.sleep(60)"
        if failed_peer and role == "seller":
            code = 'import sys; sys.stdin.read(); print(\'{"status":"error","error":"private"}\'); sys.exit(1)'
        kwargs["env"] = None
        child = popen([sys.executable, "-c", code], **kwargs)
        children.append(child)
        return child
    monkeypatch.setattr(rehearsal.subprocess, "Popen", launch)
    with pytest.raises(rehearsal.Rehearsal) as error:
        rehearsal.negotiate_participants(tmp_path, tmp_path, "ab" * 32, 0.2)
    assert len(children) == 2 and all(child.poll() is not None for child in children)
    assert "private" not in str(error.value)


def test_negotiation_sends_both_requests_and_collects_both_replies(tmp_path, monkeypatch):
    popen = subprocess.Popen
    for role in ("buyer", "seller"):
        (tmp_path / role).mkdir()
    def launch(args, **kwargs):
        code = 'import sys,json; request=json.load(sys.stdin); print(json.dumps({"status":"ok","operation_ref":request["operation_ref"]}))'
        kwargs["env"] = None
        return popen([sys.executable, "-c", code], **kwargs)
    monkeypatch.setattr(rehearsal.subprocess, "Popen", launch)
    replies = rehearsal.negotiate_participants(tmp_path, tmp_path, "ab" * 32, 2)
    assert set(replies) == {"buyer", "seller"}
    assert all(reply["operation_ref"] == "ab" * 32 for reply in replies.values())


def test_command_timeout_does_not_print_partial_private_output(tmp_path, monkeypatch):
    (tmp_path / "erebus-payment").touch()
    def timeout(*args, **kwargs):
        raise subprocess.TimeoutExpired("erebus-payment", 1, output="private", stderr="private")
    monkeypatch.setattr(rehearsal.subprocess, "run", timeout)
    with pytest.raises(rehearsal.Rehearsal, match="recover without a new payment") as error:
        rehearsal.command(tmp_path, "erebus-payment", {"method": "observe"}, tmp_path, 1)
    assert "private" not in str(error.value)


@pytest.mark.parametrize("encoded", [bytes(range(32)), bytes(range(32)).hex().encode(), b"0x" + bytes(range(32)).hex().encode() + b"\n"])
def test_testnet_gas_key_accepts_raw_or_hex_owner_only_files(tmp_path, encoded):
    path = tmp_path / "gas.key"
    path.write_bytes(encoded)
    path.chmod(0o600)
    assert rehearsal.gas_seed(path) == bytes(range(32))


def test_gas_key_rejects_public_or_symlinked_files(tmp_path):
    path = tmp_path / "gas.key"
    path.write_bytes(bytes(range(32)))
    path.chmod(0o644)
    with pytest.raises(rehearsal.Rehearsal, match="owner-only"):
        rehearsal.gas_seed(path)
    path.chmod(0o600)
    link = tmp_path / "link"
    link.symlink_to(path)
    with pytest.raises(rehearsal.Rehearsal, match="owner-only"):
        rehearsal.gas_seed(link)


def test_agreement_lifetime_must_outlast_the_scan_window():
    assert rehearsal.agreement_lifetime({"agreement_lifetime_seconds": 7200}, 3600) == 7200
    assert rehearsal.agreement_lifetime({"delivery_window_seconds": 7200}, 3600) == 7200
    with pytest.raises(rehearsal.Rehearsal, match="settlement and delivery"):
        rehearsal.agreement_lifetime({"agreement_lifetime_seconds": 7199}, 3600)
    with pytest.raises(rehearsal.Rehearsal, match="between 1 and 86400"):
        rehearsal.agreement_lifetime({"agreement_lifetime_seconds": 0}, 3600)
    with pytest.raises(rehearsal.Rehearsal, match="between 1 and 86400"):
        rehearsal.agreement_lifetime({"agreement_lifetime_seconds": rehearsal.MAX_AGREEMENT_LIFETIME_SECONDS + 1}, 3600)
    with pytest.raises(rehearsal.Rehearsal, match="between 1 and 86400"):
        rehearsal.agreement_lifetime({"agreement_lifetime_seconds": "7200"}, 3600)


def test_validate_agreement_lifetimes_rejects_a_shortened_participant_config(tmp_path):
    for role in ("buyer", "seller"):
        (tmp_path / role).mkdir()
        (tmp_path / role / "config.json").write_text(json.dumps({"max_deal_lifetime_seconds": 7200}))
    plan = {"agreement_lifetime_seconds": 7200}
    assert rehearsal.validate_agreement_lifetimes(tmp_path, plan, 3600) == 7200
    (tmp_path / "seller" / "config.json").write_text(json.dumps({"max_deal_lifetime_seconds": 3600}))
    with pytest.raises(rehearsal.Rehearsal, match="initialize a replacement agreement"):
        rehearsal.validate_agreement_lifetimes(tmp_path, plan, 3600)
    (tmp_path / "buyer" / "config.json").unlink()
    with pytest.raises(rehearsal.Rehearsal, match="configuration unavailable"):
        rehearsal.validate_agreement_lifetimes(tmp_path, plan, 3600)


def rehearsal_plan(tmp_path, **extra):
    payload = tmp_path / "payload.bin"
    payload.write_bytes(b"snapshot-bytes")
    gas = tmp_path / "gas.key"
    gas.write_bytes(bytes(range(32)))
    gas.chmod(0o600)
    return {"rail": "public-bound", "chain_id": 10143,
            "rpc_url": "https://testnet-rpc.monad.xyz", "peer_rpc_url": "https://rpc-testnet.monadinfra.com",
            "bin_dir": str(tmp_path), "settlement_contract": "0x" + "aa" * 20, "deployment_block": 1,
            "asset_contract": "0x" + "bb" * 20, "negotiation_endpoint": "127.0.0.1:19431", "access_port": 19432,
            "payload_file": str(payload), "resource": "erebus.release-manifest.v1",
            "buyer_start_price": 60, "seller_price": 70, "buyer_maximum_price": 75,
            "gas_key_file": str(gas), "verification_timeout_seconds": 3600,
            "agreement_lifetime_seconds": 10800, **extra}


def install_fake_commands(monkeypatch, calls):
    def run(bin_dir, name, request, cwd, timeout=900):
        calls.append((name, request["method"]))
        if name == "erebus-negotiate" and request["method"] == "prepare_operator":
            directory = Path(request["directory"])
            directory.mkdir(mode=0o700)
            role = request["role"]
            for key in ("agreement.key", "transport.key"):
                (directory / key).write_bytes(bytes(range(32)))
                (directory / key).chmod(0o600)
            address = ("11" if role == "buyer" else "22") * 20
            (directory / f"{role}.descriptor.json").write_text(
                json.dumps({"seller_address": address, "expires": int(time.time()) + 86400}))
            return 0, {"status": "ok", "agreement_address": "0x" + address,
                       "descriptor_file": str(directory / f"{role}.descriptor.json")}
        if name == "erebus-negotiate" and request["method"] == "prepare_terms":
            Path(request["output"]).write_text("terms")
            return 0, {"status": "ok"}
        if name == "erebus-shielded-disclosure" and request["method"] == "keygen":
            Path(request["key_file"]).write_bytes(bytes(range(32)))
            Path(request["key_file"]).chmod(0o600)
            return 0, {"status": "ok", "recipient_public_key": "ab" * 32}
        raise AssertionError((name, request))
    monkeypatch.setattr(rehearsal, "command", run)


def test_init_writes_the_validated_lifetime_to_both_participants(tmp_path, monkeypatch):
    calls = []
    install_fake_commands(monkeypatch, calls)
    plan = rehearsal_plan(tmp_path)
    (tmp_path / "plan.json").write_text(json.dumps(plan))
    before = int(time.time())
    result = rehearsal.init(tmp_path / "plan.json", tmp_path / "work")
    after = int(time.time())
    assert result["status"] == "initialized"
    for role in ("buyer", "seller"):
        config = json.loads((tmp_path / "work" / role / "config.json").read_text())
        assert config["max_deal_lifetime_seconds"] == 10800
    record = json.loads((tmp_path / "work" / "rehearsal.json").read_text())
    assert before + 10800 <= record["delivery_deadline"] <= after + 10800
    assert ("erebus-negotiate", "prepare_operator") in calls
    assert ("erebus-shielded-disclosure", "keygen") in calls


def test_init_rejects_a_lifetime_that_cannot_outlast_the_scan_window(tmp_path):
    plan = rehearsal_plan(tmp_path, agreement_lifetime_seconds=3600, verification_timeout_seconds=3600)
    (tmp_path / "plan.json").write_text(json.dumps(plan))
    with pytest.raises(rehearsal.Rehearsal, match="settlement and delivery"):
        rehearsal.init(tmp_path / "plan.json", tmp_path / "work")
    assert not (tmp_path / "work").exists()


def test_observer_budget_bounds_and_defaults():
    assert rehearsal.observer_budget({}) == {"log_block_range": 100, "max_log_queries": 256,
                                             "max_concurrent_queries": 8, "max_ancestry": 8192}
    assert rehearsal.observer_budget({"max_log_queries": 32, "max_concurrent_queries": 4})["max_log_queries"] == 32
    with pytest.raises(rehearsal.Rehearsal, match="max_log_queries"):
        rehearsal.observer_budget({"max_log_queries": 0})
    with pytest.raises(rehearsal.Rehearsal, match="max_log_queries"):
        rehearsal.observer_budget({"max_log_queries": 1_025})
    with pytest.raises(rehearsal.Rehearsal, match="max_concurrent_queries"):
        rehearsal.observer_budget({"max_concurrent_queries": 33})
    with pytest.raises(rehearsal.Rehearsal, match="max_ancestry"):
        rehearsal.observer_budget({"max_ancestry": 0})
    with pytest.raises(rehearsal.Rehearsal, match="max_ancestry"):
        rehearsal.observer_budget({"max_ancestry": 8_193})


def test_scan_budget_is_conservative_and_concurrency_scaled():
    assert rehearsal.scan_budget(0, 100, 16) == 0
    assert rehearsal.scan_budget(1_000, 100, 1) == 10 * rehearsal.SCAN_QUERY_SECONDS
    assert rehearsal.scan_budget(1_000, 100, 4) == 5


def test_observation_slice_must_fit_the_access_request():
    assert rehearsal.validate_observation_slice(
        {"max_log_queries": 256, "max_concurrent_queries": 8}) == 64
    with pytest.raises(rehearsal.Rehearsal, match="access request"):
        rehearsal.validate_observation_slice({"max_log_queries": 512, "max_concurrent_queries": 4})


def test_init_rejects_an_observer_budget_that_cannot_fit_a_request(tmp_path):
    plan = rehearsal_plan(tmp_path, max_log_queries=1_024, max_concurrent_queries=1)
    (tmp_path / "plan.json").write_text(json.dumps(plan))
    with pytest.raises(rehearsal.Rehearsal, match="access request"):
        rehearsal.init(tmp_path / "plan.json", tmp_path / "work")
    assert not (tmp_path / "work").exists()


def test_grant_lifetime_must_cover_the_auditor_scan():
    assert rehearsal.grant_lifetime({"grant_lifetime_seconds": 3_600}, 2_000) == 3_600
    with pytest.raises(rehearsal.Rehearsal, match="auditor scan"):
        rehearsal.grant_lifetime({"grant_lifetime_seconds": 1_200}, 2_000)
    with pytest.raises(rehearsal.Rehearsal, match="between 1 and 86400"):
        rehearsal.grant_lifetime({"grant_lifetime_seconds": "3600"}, 0)


def test_live_budget_rejects_a_stale_delivery_deadline(tmp_path):
    plan = rehearsal_plan(tmp_path, agreement_lifetime_seconds=18_000, verification_timeout_seconds=3_600)
    now = int(time.time())
    record = {"plan": plan, "delivery_deadline": now + 18_000}
    head = int(plan["deployment_block"]) + 100_000
    checks = rehearsal.validate_live_budget(record, head)
    assert checks["estimated_scan_seconds"] > 0
    assert checks["grant_lifetime_seconds"] >= checks["estimated_scan_seconds"]
    stale = {"plan": plan, "delivery_deadline": now + 100}
    with pytest.raises(rehearsal.Rehearsal, match="delivery deadline"):
        rehearsal.validate_live_budget(stale, head)
    with pytest.raises(rehearsal.Rehearsal, match="cover the estimated"):
        rehearsal.validate_live_budget(record, int(plan["deployment_block"]) + 20_000_000)


def retained_participants(tmp_path, expires=None):
    source = tmp_path / "source"
    source.mkdir(parents=True)
    buyer, seller = "0x" + "11" * 20, "0x" + "22" * 20
    record = {"version": 1, "rail": "public-bound", "namespace": "eip155:10143", "buyer": buyer, "seller": seller,
              "gas_role": "buyer", "auditor_public_key": "ab" * 32, "operation_ref": "cd" * 32, "service_id": "ef" * 32,
              "contract": "0x" + "aa" * 20, "delivery_deadline": 0, "fulfillment_digest": "00" * 32, "plan": {}}
    (source / "rehearsal.json").write_text(json.dumps(record))
    expires = expires if expires is not None else int(time.time()) + 86400
    for role, address, peer in (("buyer", buyer, seller), ("seller", seller, buyer)):
        directory = source / role
        directory.mkdir()
        for key in ("agreement.key", "transport.key"):
            (directory / key).write_bytes(bytes(range(32)))
            (directory / key).chmod(0o600)
        for name, owner in ((role, address), ("seller" if role == "buyer" else "buyer", peer)):
            (directory / f"{name}.descriptor.json").write_text(
                json.dumps({"seller_address": owner.removeprefix("0x"), "expires": expires}))
    (source / "buyer" / "gas.key").write_bytes(bytes(range(32)))
    (source / "buyer" / "gas.key").chmod(0o600)
    (source / "auditor").mkdir()
    (source / "auditor" / "auditor.key").write_bytes(bytes(range(32)))
    (source / "auditor" / "auditor.key").chmod(0o600)
    return source, record


def test_init_reuse_keeps_funded_participants_and_never_generates_keys(tmp_path, monkeypatch):
    source, record = retained_participants(tmp_path)
    calls = []
    install_fake_commands(monkeypatch, calls)
    plan = rehearsal_plan(tmp_path, reuse_from=str(source))
    (tmp_path / "plan.json").write_text(json.dumps(plan))
    result = rehearsal.init(tmp_path / "plan.json", tmp_path / "work")
    assert result["buyer"] == record["buyer"] and result["seller"] == record["seller"]
    assert [call for call in calls if call[1] in {"prepare_operator", "keygen"}] == []
    work = tmp_path / "work"
    for role in ("buyer", "seller"):
        assert (work / role / "agreement.key").read_bytes() == (source / role / "agreement.key").read_bytes()
        assert (work / role / "agreement.key").stat().st_mode & 0o077 == 0
        config = json.loads((work / role / "config.json").read_text())
        assert config["agreement_key_file"] == str(work / role / "agreement.key")
        assert config["max_deal_lifetime_seconds"] == 10800
    rehearsal_record = json.loads((work / "rehearsal.json").read_text())
    assert rehearsal_record["auditor_public_key"] == "ab" * 32
    assert (work / "auditor" / "auditor.key").read_bytes() == (source / "auditor" / "auditor.key").read_bytes()
    assert json.loads((source / "rehearsal.json").read_text()) == record


def test_reuse_rejects_another_deployment_or_address_mismatch(tmp_path):
    source, _ = retained_participants(tmp_path)
    work = tmp_path / "work"
    work.mkdir()
    with pytest.raises(rehearsal.Rehearsal, match="different rail or deployment"):
        rehearsal.reuse_participants(source, work, "public-bound", {"chain_id": 10143}, "eip155:1")
    (source / "buyer" / "buyer.descriptor.json").write_text(
        json.dumps({"seller_address": "99" * 20, "expires": int(time.time()) + 86400}))
    work2 = tmp_path / "work2"
    work2.mkdir()
    with pytest.raises(rehearsal.Rehearsal, match="does not match"):
        rehearsal.reuse_participants(source, work2, "public-bound", {"chain_id": 10143}, "eip155:10143")


def test_reuse_rejects_expired_or_public_key_material(tmp_path):
    expired, _ = retained_participants(tmp_path / "expired", expires=int(time.time()) - 1)
    work = tmp_path / "expired-work"
    work.mkdir()
    with pytest.raises(rehearsal.Rehearsal, match="expired"):
        rehearsal.reuse_participants(expired, work, "public-bound", {"chain_id": 10143}, "eip155:10143")
    source, _ = retained_participants(tmp_path / "public")
    (source / "seller" / "agreement.key").chmod(0o644)
    work = tmp_path / "public-work"
    work.mkdir()
    with pytest.raises(rehearsal.Rehearsal, match="owner-only"):
        rehearsal.reuse_participants(source, work, "public-bound", {"chain_id": 10143}, "eip155:10143")
