"""One-step operator setup writes exactly the strict configs the installed commands parse, and refuses drift."""

import hashlib
import importlib.util
import json
import re
import stat
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("metropolis_operator_init", ROOT / "mcp-server/src/erebus_mcp/metropolis_setup/operator_init.py")
init = importlib.util.module_from_spec(spec)
spec.loader.exec_module(init)

PIN = "0" * 63 + "1"
POOLS = {"wmon": ("0x" + "88" * 20, "0x" + "fb" * 20), "usdc": ("0x" + "86" * 20, "0x" + "53" * 20)}

FAKE_NEGOTIATE = r'''#!PYTHON
import json, os, sys
from pathlib import Path
request = json.loads(sys.stdin.read())
def private(path, data):
    Path(path).parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    Path(path).write_bytes(data); os.chmod(path, 0o600)
method = request["method"]
if method == "prepare_operator":
    d = Path(request["directory"]); reply = {"status": "ok", "suite_id": 2}
    for name in ("agreement", "wallet", "discovery", "transport") + (("spend",) if request["role"] == "seller" else ()):
        private(d / f"{name}.key", os.urandom(32))
    private(d / "agreement.public", os.urandom(64))
    (d / f"{request['role']}.descriptor.json").write_text(json.dumps({"role": request["role"], "endpoints": ["tcp://" + request["endpoint"]]}))
    reply.update(agreement_key_file=str(d / "agreement.key"), wallet_agreement_key_file=str(d / "agreement.key"),
                 wallet_key_file=str(d / "wallet.key"), discovery_key_file=str(d / "discovery.key"),
                 transport_key_file=str(d / "transport.key"), descriptor_file=str(d / f"{request['role']}.descriptor.json"))
    if request["role"] == "seller":
        reply["seller_spend_secret_file"] = str(d / "spend.key")
elif method == "prepare_binding":
    private(request["output"], json.dumps(request).encode()); reply = {"status": "ok"}
elif method == "prepare_terms":
    private(request["output"], json.dumps(request).encode()); reply = {"status": "ok"}
else:
    reply = {"status": "error", "error": "unknown"}
print(json.dumps(reply)); sys.exit(0 if reply["status"] == "ok" else 1)
'''
FAKE_DISCLOSURE = r'''#!PYTHON
import json, os, sys
from pathlib import Path
request = json.loads(sys.stdin.read())
if request["method"] == "checkpoint":
    root = Path(request["deployment"]["cache_root"])
    record = root / "history-checkpoint.json"
    if request.get("confirm_new_cache") is not True or (not record.exists() and (root / "primary").exists()):
        print(json.dumps({"status": "error", "error": "refused"})); sys.exit(1)
    created = "existing" if record.exists() else "created"
    if not record.exists():
        fd = os.open(record, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.write(fd, json.dumps({"version": 1, "checkpoint": {"block_number": 41}}).encode()); os.close(fd)
    print(json.dumps({"status": "ok", "checkpoint": created, "checkpoint_block": 41, "history_from_block": 42,
                      "payment_verified": False}))
    sys.exit(0)
fd = os.open(request["key_file"], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600); os.write(fd, os.urandom(32)); os.close(fd)
print(json.dumps({"status": "ok", "public_key": "00" * 32}))
'''


def rust_optional(relative: str, opener: str, within: str | None = None) -> set[str]:
    """Fields marked `#[serde(default)]`: the binary accepts them absent."""
    source = (ROOT / relative).read_text()
    if within is not None:
        source = source[source.index(within):]
    match = re.search(re.escape(opener) + r"\s*\{(.*?)\n\s*\}", source, re.S)
    return set(re.findall(r"#\[serde\(default[^\]]*\)\]\s*(?:///[^\n]*\s*)*(?:pub\s+)?([a-z_][a-z0-9_]*)\s*:", match.group(1)))


def rust_fields(relative: str, opener: str, within: str | None = None) -> set[str]:
    """Field names of a strict Rust struct or struct-like enum variant, read from the source."""
    source = (ROOT / relative).read_text()
    if within is not None:
        source = source[source.index(within):]
    match = re.search(re.escape(opener) + r"\s*\{(.*?)\n\s*\}", source, re.S)
    assert match, f"{opener} not found in {relative}"
    return set(re.findall(r"^\s+(?:pub\s+)?([a-z_][a-z0-9_]*)\s*:", match.group(1), re.M))


@pytest.fixture
def world(tmp_path, monkeypatch):
    monkeypatch.setattr(init, "wheel_tag", lambda: "macosx_11_0_arm64")
    bins = tmp_path / "bin"
    bins.mkdir()
    for name in init.COMMANDS:
        path = bins / name
        body = {"erebus-negotiate": FAKE_NEGOTIATE, "erebus-shielded-disclosure": FAKE_DISCLOSURE}.get(name, "#!/bin/sh\n")
        path.write_text(body.replace("#!PYTHON", f"#!{sys.executable}"))
        path.chmod(0o755)
    pins = tmp_path / "pins"
    pins.mkdir()
    manifests = {}
    for name, (pool, _) in POOLS.items():
        manifest = {"version": 2, "chain_id": 10143, "settlement_contract": pool, "verifier_version": 2, "test_only": True,
                    "circuits": [{"kind": kind, "identity": {"verifier_runtime_keccak256": PIN},
                                  **{field: {"sha256": hashlib.sha256(f"{kind}.{field}".encode()).hexdigest(), "bytes": 1}
                                     for field in ("wasm", "r1cs", "zkey")}} for kind in ("deposit", "transfer", "withdraw")]}
        (pins / f"manifest-{name}.json").write_text(json.dumps(manifest))
        manifests[name] = hashlib.sha256((pins / f"manifest-{name}.json").read_bytes()).hexdigest()
        (pins / f"manifest-{name}.signature.json").write_text(json.dumps({"version": 1, "signature_hex": "ab",
                                                                         "manifest_sha256": manifests[name]}))
    binaries = {name: {"sha256": hashlib.sha256((bins / name).read_bytes()).hexdigest()} for name in init.COMMANDS}
    (pins / "release-manifest.json").write_text(json.dumps({"version": "0.3.0.dev2026101202", "source_commit": "07e6949",
        "platforms": {"macosx-11.0-arm64": {"platform_qualification": {"wheel_tag": "macosx_11_0_arm64"}, "binaries": binaries}}}))
    (pins / "testnet-release.json").write_text(json.dumps({"package_version": "0.3.0.dev2026101202",
        "publisher": "0x" + "cd" * 20, "hosting_manifests": manifests}))
    deployment = tmp_path / "deployment.json"
    deployment.write_text(json.dumps({"chain_id": 10143, "test_only": True, "pools": [
        {"name": name, "pool": pool, "asset": asset, "namespace": "eip155:10143", "verifier_version": 2,
         "deployment_block": 7, "deployment_hash": "0x" + "ab" * 32,
         "runtime_pins": {k: PIN for k in ("pool", "poseidon", "deposit", "transfer", "withdraw")}}
        for name, (pool, asset) in POOLS.items()]}))
    return SimpleNamespace(root=tmp_path, bins=bins, pins=pins, deployment=deployment)


def init_args(world, role, **changes):
    values = dict(role=role, pool="wmon", directory=world.root / role, bin=world.bins, pins=world.pins,
                  deployment=world.deployment, endpoint=f"127.0.0.1:{9101 if role == 'buyer' else 9102}",
                  rpc_url="https://a.example.net/", peer_rpc_url="https://b.example.net/", signer_address="0x" + "12" * 20,
                  transaction_key_file=None, maximum_price="800" if role == "buyer" else None,
                  relay_url=None, relay_identity=None, relay_sponsor=None, relay_auth_key_file=None, relay_timeout_seconds=60,
                  descriptor_lifetime_seconds=86400, binding_lifetime_seconds=21600)
    values.update(changes)
    return SimpleNamespace(**values)


def connect_args(world, role, **changes):
    payload = world.root / "payload.json"
    payload.write_text("{}")
    values = dict(directory=world.root / role, peer_share=world.root / ("seller" if role == "buyer" else "buyer") / "share",
                  resource="dataset.snapshot.v1", amount="700" if role == "seller" else "600", minimum_price=None,
                  unit="snapshot", quantity="1", payload_file=payload if role == "seller" else None,
                  port=18732 if role == "seller" else None, delivery_seconds=21600, service_url=None,
                  allow_loopback_access_http=False)
    values.update(changes)
    return SimpleNamespace(**values)


def owner_only(path: Path) -> bool:
    return not stat.S_IMODE(path.stat().st_mode) & 0o077


def test_full_setup_writes_exactly_the_strict_rust_schemas(world):
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    init.connect(connect_args(world, "seller"))
    init.connect(connect_args(world, "buyer"))
    buyer, seller = world.root / "buyer", world.root / "seller"
    shielded = "sdk/shielded/src/bin"
    expected = {
        buyer / "wallet.json": rust_fields(f"{shielded}/erebus_wallet.rs", "struct Config"),
        buyer / "payment.json": rust_fields(f"{shielded}/payment/shielded.rs", "struct ShieldedConfig"),
        buyer / "negotiate.json": rust_fields(f"{shielded}/negotiate/peers.rs", "struct ConfigV1")
        - {"seller_spend_secret_file", "seller_wallet_file", "seller_wallet_key_file", "access_evidence_root"},
        seller / "negotiate.json": rust_fields(f"{shielded}/negotiate/peers.rs", "struct ConfigV2"),
        seller / "peers" / "default" / "peer.json": rust_fields(f"{shielded}/negotiate/peers.rs", "struct PeerFile"),
        seller / "access.json": rust_fields(f"{shielded}/erebus_access_service.rs", "struct Config")
        - {"profile", "max_response_bytes", "feed_file"},
    }
    optional = {buyer / "wallet.json": (f"{shielded}/erebus_wallet.rs", "struct Config"),
                buyer / "payment.json": (f"{shielded}/payment/shielded.rs", "struct ShieldedConfig"),
                buyer / "negotiate.json": (f"{shielded}/negotiate/peers.rs", "struct ConfigV1"),
                seller / "negotiate.json": (f"{shielded}/negotiate/peers.rs", "struct ConfigV2"),
                seller / "peers" / "default" / "peer.json": (f"{shielded}/negotiate/peers.rs", "struct PeerFile"),
                seller / "access.json": (f"{shielded}/erebus_access_service.rs", "struct Config")}
    for path, fields in expected.items():
        keys = set(json.loads(path.read_text()))
        accepted_absent = rust_optional(*optional[path])
        assert fields - accepted_absent <= keys <= fields, (path.name, sorted(fields - keys - accepted_absent), sorted(keys - fields))
        assert owner_only(path), path.name
    backend = json.loads((seller / "access.json").read_text())["backend"]
    assert set(backend) - {"mode"} == rust_fields(f"{shielded}/erebus_access_service.rs", "Shielded", within="enum Backend")
    wallet = json.loads((buyer / "wallet.json").read_text())
    payment = json.loads((buyer / "payment.json").read_text())
    assert wallet["wrapper"] == POOLS["wmon"][1] and wallet["runtime_pins"]["withdraw"] == PIN
    assert wallet["artifact_manifest_authorization"]["publisher"] == "0x" + "cd" * 20
    assert payment["withdraw_verifier_keccak256"] == "0x" + PIN and payment["transaction_key_file"] is None
    assert len(payment["buyer_key_hex"]) == 128
    environment = json.loads((buyer / "mcp.json").read_text())["mcpServers"]["erebus-monad-buyer"]["env"]
    assert environment["EREBUS_BACKEND"] == "metropolis" and environment["EREBUS_PAYMENT_CONFIG"].endswith("payment.json")
    assert sorted(p.name for p in (seller / "share").iterdir()) == ["identity.binding", "offer.json", "seller.descriptor.json",
                                                                    "service.json"]
    assert not list((buyer / "share").glob("*.key"))
    # The seller advertises routed negotiation, and its first buyer is the peer table's `default`.
    assert json.loads((seller / "share" / "service.json").read_text())["negotiation_transport"] == 2
    assert json.loads((buyer / "negotiate.json").read_text())["negotiation_transport"] == 2
    negotiate = json.loads((seller / "negotiate.json").read_text())
    assert negotiate["version"] == 2 and negotiate["peers_root"] == str(seller / "peers")
    peer = json.loads((seller / "peers" / "default" / "peer.json").read_text())
    assert (peer["id"], peer["enabled"], peer["legacy_transport"]) == ("default", True, False)
    assert sorted(p.name for p in (seller / "peers" / "default").iterdir()) == [
        "connect.json", "descriptor.json", "identity.binding", "peer.json", "service.terms"]
    assert all(owner_only(path) for path in (seller / "peers").rglob("*"))


def init_second_buyer(world, name="buyer2", port=9103):
    init.init(init_args(world, "buyer", directory=world.root / name, endpoint=f"127.0.0.1:{port}"))
    return world.root / name


def tree(directory: Path) -> dict:
    return {str(path.relative_to(directory)): path.read_bytes() for path in sorted(directory.rglob("*")) if path.is_file()}


def test_a_second_buyer_adds_a_peer_and_an_outbox_without_touching_the_service(world):
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    init.connect(connect_args(world, "seller"))
    init.connect(connect_args(world, "buyer"))
    seller = world.root / "seller"
    second = init_second_buyer(world)
    kept = {name: (seller / name).read_bytes() for name in ("access.json", "payload", "share/service.json", "share/offer.json")}
    default = tree(seller / "peers" / "default")
    with pytest.raises(ValueError, match="--peer-id"):
        init.connect(connect_args(world, "seller", peer_share=second / "share"))
    added = init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b2", amount="650"))
    assert added["written"] == ["peers/b2/", "outbox/b2/"]
    assert {name: (seller / name).read_bytes() for name in kept} == kept
    assert tree(seller / "peers" / "default") == default
    outbox = seller / "outbox" / "b2"
    assert sorted(p.name for p in outbox.iterdir()) == ["identity.binding", "offer.json", "seller.descriptor.json",
                                                       "service.json"]
    assert json.loads((outbox / "offer.json").read_text())["price"] == "650"
    assert (outbox / "service.json").read_bytes() == kept["share/service.json"]
    peer = json.loads((seller / "peers" / "b2" / "peer.json").read_text())
    assert peer["minimum_price"] == "650" and peer["legacy_transport"] is False
    # The same buyer again under the same id changes nothing; different inputs or a reused buyer are refused.
    before = tree(seller)
    again = init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b2", amount="650"))
    assert again["repeated"] is True and tree(seller) == before
    with pytest.raises(ValueError, match="different inputs"):
        init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b2", amount="651"))
    with pytest.raises(ValueError, match="another --peer-id"):
        init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b3"))
    with pytest.raises(ValueError, match="another resource"):
        init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b4", resource="other"))
    with pytest.raises(ValueError, match="--peer-id must be"):
        init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="B_4"))
    assert tree(seller) == before
    assert not list((seller / "peers").glob(".staging-*"))
    # The second buyer connects against its outbox and dials with the routed handshake.
    init.connect(connect_args(world, "buyer", directory=second, peer_share=outbox, amount="650"))
    negotiate = json.loads((second / "negotiate.json").read_text())
    assert negotiate["negotiation_transport"] == 2


def test_a_version_1_seller_migrates_its_buyer_to_a_legacy_default_peer(world):
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    init.connect(connect_args(world, "seller"))
    init.connect(connect_args(world, "buyer"))
    seller = world.root / "seller"
    # Rewrite the seller as a version 1 connection, as an earlier release left it.
    peer = json.loads((seller / "peers" / "default" / "peer.json").read_text())
    v2 = json.loads((seller / "negotiate.json").read_text())
    v1 = {key: value for key, value in v2.items() if key != "peers_root"} | {
        "version": 1, "peer_descriptor_file": str(world.root / "buyer" / "share" / "buyer.descriptor.json"),
        "terms_template_file": str(seller / "peers" / "default" / "service.terms"), "minimum_price": peer["minimum_price"],
        "max_deal_lifetime_seconds": peer["max_deal_lifetime_seconds"]}
    import shutil
    terms = (seller / "peers" / "default" / "service.terms").read_bytes()
    shutil.rmtree(seller / "peers")
    (seller / "keys" / "service.terms").write_bytes(terms)
    v1["terms_template_file"] = str(seller / "keys" / "service.terms")
    (seller / "negotiate.json").unlink()
    init.private_json(seller / "negotiate.json", v1)
    access = (seller / "access.json").read_bytes()
    second = init_second_buyer(world)
    init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b2"))
    migrated = json.loads((seller / "negotiate.json").read_text())
    assert migrated["version"] == 2 and set(migrated) == rust_fields(
        "sdk/shielded/src/bin/negotiate/peers.rs", "struct ConfigV2")
    default = json.loads((seller / "peers" / "default" / "peer.json").read_text())
    assert default["legacy_transport"] is True and default["binding_file"] is None
    assert (seller / "peers" / "default" / "service.terms").read_bytes() == terms
    assert (seller / "access.json").read_bytes() == access
    assert sorted(p.name for p in (seller / "peers").iterdir()) == ["b2", "default"]


def test_init_refuses_tampered_binaries_unpinned_manifests_and_bad_inputs(world):
    (world.bins / "erebus-payment").write_text("#!/bin/sh\necho changed\n")
    with pytest.raises(ValueError, match="erebus-payment is not the published binary"):
        init.init(init_args(world, "buyer"))
    (world.bins / "erebus-payment").write_text("#!/bin/sh\n")
    (world.pins / "manifest-wmon.json").write_text("{}")
    with pytest.raises(ValueError, match="does not match the release authorization"):
        init.init(init_args(world, "buyer"))
    for change, message in ((dict(signer_address="0x" + "0" * 40), "own gas address"),
                            (dict(peer_rpc_url="https://a.example.net/"), "must differ"),
                            (dict(maximum_price=None), "--maximum-price"),
                            (dict(pool="usdc", directory=world.root / "u", rpc_url="http://u:p@x/"), "credentials")):
        with pytest.raises(ValueError, match=message):
            init.init(init_args(world, "buyer", **change))


def test_init_refuses_a_manifest_bound_to_another_pool(world):
    other = world.pins / "manifest-usdc.json"
    (world.pins / "manifest-wmon.json").write_bytes(other.read_bytes())
    (world.pins / "manifest-wmon.signature.json").write_bytes((world.pins / "manifest-usdc.signature.json").read_bytes())
    authorization = json.loads((world.pins / "testnet-release.json").read_text())
    authorization["hosting_manifests"]["wmon"] = authorization["hosting_manifests"]["usdc"]
    (world.pins / "testnet-release.json").write_text(json.dumps(authorization))
    with pytest.raises(ValueError, match="names a different pool"):
        init.init(init_args(world, "buyer"))
    assert not (world.root / "buyer").exists()


def test_connect_requires_the_sellers_offer_and_unchanged_binaries(world):
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    with pytest.raises(ValueError, match="offer.json"):
        init.connect(connect_args(world, "buyer"))
    init.connect(connect_args(world, "seller"))
    with pytest.raises(ValueError, match="different resource"):
        init.connect(connect_args(world, "buyer", resource="other"))
    (world.bins / "erebus-negotiate").write_text((world.bins / "erebus-negotiate").read_text() + "\n# changed\n")
    with pytest.raises(ValueError, match="erebus-negotiate is not the published binary"):
        init.connect(connect_args(world, "buyer"))


def test_auditor_setup_uses_the_installed_keygen_and_the_strict_disclosure_schema(world):
    init.auditor(SimpleNamespace(pool="usdc", directory=world.root / "auditor", bin=world.bins, pins=world.pins, deployment=world.deployment,
                                 rpc_url="https://a.example.net/", peer_rpc_url="https://b.example.net/"))
    config = json.loads((world.root / "auditor/disclosure.json").read_text())
    assert set(config) == rust_fields("sdk/shielded/src/bin/erebus_shielded_disclosure.rs", "struct Deployment")
    assert config["settlement_contract"] == POOLS["usdc"][0] and owner_only(world.root / "auditor/auditor.key")


def auditor_args(world, **changes):
    values = dict(pool="wmon", directory=world.root / "auditor", bin=world.bins, pins=world.pins,
                  deployment=world.deployment, rpc_url="https://a.example.net/", peer_rpc_url="https://b.example.net/")
    values.update(changes)
    return SimpleNamespace(**values)


def recording(monkeypatch):
    calls, real = [], init.command

    def record(bins, name, request, cwd):
        calls.append((name, request))
        return real(bins, name, request, cwd)
    monkeypatch.setattr(init, "command", record)
    return calls


def test_auditor_checkpoint_opt_in_is_a_native_checkpoint_of_its_empty_cache(world, monkeypatch):
    calls = recording(monkeypatch)
    result = init.auditor(auditor_args(world, history="checkpoint"))
    directory = world.root / "auditor"
    config = json.loads((directory / "disclosure.json").read_text())
    assert [request["method"] for _, request in calls] == ["keygen", "checkpoint"]
    # The checkpoint is asked of the installed Rust command with the very config it will verify with.
    assert calls[1] == ("erebus-shielded-disclosure",
                        {"method": "checkpoint", "deployment": config, "confirm_new_cache": True})
    assert (directory / "cache/history-checkpoint.json").is_file()
    assert result["history"] == "checkpoint" and result["checkpoint_block"] == 41
    assert result["history_from_block"] == 42 and "settled_before_checkpoint" in result["scope"]
    # The disclosure schema is unchanged by the checkpoint.
    assert set(config) == rust_fields("sdk/shielded/src/bin/erebus_shielded_disclosure.rs", "struct Deployment")


def test_auditor_defaults_to_full_history_and_stays_offline(world, monkeypatch):
    # A standalone auditor is usually set up after the grant, i.e. after settlement: full history verifies it.
    calls = recording(monkeypatch)
    result = init.auditor(auditor_args(world))
    assert [request["method"] for _, request in calls] == ["keygen"]
    assert result["history"] == "full" and result["history_from_block"] == 7
    assert not (world.root / "auditor/cache/history-checkpoint.json").exists()
    explicit = init.auditor(auditor_args(world, directory=world.root / "explicit", history="full"))
    assert explicit["history"] == "full" and not (world.root / "explicit/cache/history-checkpoint.json").exists()
    with pytest.raises(ValueError, match="--history"):
        init.auditor(auditor_args(world, directory=world.root / "a2", history="later"))


def test_auditor_checkpoint_failure_names_the_offline_alternative(world, monkeypatch):
    real = init.command

    def refuse(bins, name, request, cwd):
        if request["method"] == "checkpoint":
            raise ValueError("refused")
        return real(bins, name, request, cwd)
    monkeypatch.setattr(init, "command", refuse)
    with pytest.raises(ValueError, match="omit --history checkpoint"):
        init.auditor(auditor_args(world, history="checkpoint"))


def test_auditor_history_flag_parses(world, capsys):
    common = ["auditor", "--pool", "wmon", "--bin", str(world.bins), "--pins", str(world.pins),
              "--deployment", str(world.deployment), "--rpc-url", "https://a.example.net/",
              "--peer-rpc-url", "https://b.example.net/"]
    assert init.main([*common, "--directory", str(world.root / "default")]) == 0
    default = json.loads(capsys.readouterr().out)
    assert default["history"] == "full" and "checkpoint_block" not in default
    assert init.main([*common, "--directory", str(world.root / "opt-in"), "--history", "checkpoint"]) == 0
    assert json.loads(capsys.readouterr().out)["checkpoint_block"] == 41


def test_pins_downloads_over_https_from_the_release_commit_and_checks_manifests(tmp_path):
    import io
    commit = "07e6949" + "0" * 33
    manifest = b'{"settlement_contract": "0x88"}'
    authorization = {"package_version": "0.3.0.dev1", "artifact_tag": "artifacts-x", "publisher": "0x" + "cd" * 20,
                     "hosting_manifests": {"wmon": hashlib.sha256(manifest).hexdigest()}}
    served = {
        f"{init.REGISTRY}/v0.3.0.dev1/release-manifest.json": json.dumps({"version": "0.3.0.dev1", "source_commit": commit}).encode(),
        f"{init.SOURCE}/{commit}/packaging/metropolis/testnet-release.json": json.dumps(authorization).encode(),
        f"{init.SOURCE}/{commit}/contracts/evm/deployments/monad-testnet-shielded-v2.json": b"{}",
        f"{init.REGISTRY}/artifacts-x/manifest-wmon.json": manifest,
        f"{init.REGISTRY}/artifacts-x/manifest-wmon.signature.json": b"{}",
    }
    def opener(url, timeout=120):
        assert url.get_header("User-agent", "").startswith("erebus-init/")
        return io.BytesIO(served[url.full_url])
    result = init.pins(SimpleNamespace(version="0.3.0.dev1", directory=tmp_path / "pins"), opener)
    assert result["source_commit"] == commit and sorted(result["files"]) == sorted(p.name for p in (tmp_path / "pins").iterdir())
    served[f"{init.REGISTRY}/artifacts-x/manifest-wmon.json"] = b'{"settlement_contract": "0x99"}'
    with pytest.raises(ValueError, match="differs from the authorization"):
        init.pins(SimpleNamespace(version="0.3.0.dev1", directory=tmp_path / "pins-2"), opener)
    with pytest.raises(ValueError, match="https"):
        init.https("http://example.net/x", opener)


def test_doctor_is_read_only_and_flags_drift_without_printing_secrets(world, monkeypatch):
    init.init(init_args(world, "buyer"))
    monkeypatch.setattr(init, "command", lambda bins, name, request, cwd: {"status": "ok", "result": {"notes": [], "operations": []}})
    healthy = init.doctor(SimpleNamespace(directory=world.root / "buyer", offline=True))
    assert healthy["status"] == "healthy" and healthy["secrets_printed"] is False
    assert healthy["checks"]["owner_only"]["problems"] == [] and healthy["checks"]["artifact_cache"]["of"] == 9
    rendered = json.dumps(healthy)
    for key in (world.root / "buyer/keys").glob("*.key"):
        assert key.read_bytes().hex() not in rendered
    (world.root / "buyer/wallet.json").chmod(0o644)
    (world.bins / "erebus-wallet").write_text("#!/bin/sh\necho changed\n")
    problems = " ".join(init.doctor(SimpleNamespace(directory=world.root / "buyer", offline=True))["problems"])
    assert "erebus-wallet is not the published binary" in problems and "wallet.json" in problems


def wallet_capabilities(monkeypatch, deposit=True, funding=True):
    real = init.command

    def fake(bins, name, request, cwd):
        if name == "erebus-wallet" and request == {"method": "version"}:
            return {"status": "ok", "result": {"protocol": 1, "methods": ["notes", "sync", *init.WALLET_WRITES],
                    "deposit_operation_references": deposit, "funding_operation_references": funding}}
        return real(bins, name, request, cwd)
    monkeypatch.setattr(init, "command", fake)


def policy_args(world, role="buyer", **changes):
    values = dict(directory=world.root / role, maximum_amount="1000", allow=[], withdrawal_recipient=[], replacement_fees=None)
    values.update(changes)
    return SimpleNamespace(**values)


def test_wallet_policy_defaults_to_reads_and_adds_the_wallet_to_mcp_settings(world, monkeypatch):
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    init.connect(connect_args(world, "seller"))
    init.connect(connect_args(world, "buyer"))
    wallet_capabilities(monkeypatch)
    result = init.wallet_policy(policy_args(world))
    policy = json.loads((world.root / "buyer/wallet-policy.json").read_text())
    assert policy == {"version": 1, "maximum_amount": "1000", "allowed_methods": ["notes", "sync"], "withdrawal_recipients": []}
    assert result["funding_enabled"] == [] and owner_only(world.root / "buyer/wallet-policy.json")
    environment = json.loads((world.root / "buyer/mcp.json").read_text())["mcpServers"]["erebus-monad-buyer"]["env"]
    assert environment["EREBUS_WALLET_POLICY"].endswith("wallet-policy.json") and environment["EREBUS_WALLET_CLI"].endswith("erebus-wallet")
    assert environment["EREBUS_BACKEND"] == "metropolis" and owner_only(world.root / "buyer/mcp.json")
    init.wallet_policy(policy_args(world, allow=["deposit", "wrap"], maximum_amount="5"))
    assert json.loads((world.root / "buyer/wallet-policy.json").read_text())["allowed_methods"] == ["deposit", "notes", "sync", "wrap"]


def test_wallet_policy_refuses_unsafe_or_unsupported_permissions(world, monkeypatch):
    init.init(init_args(world, "buyer"))
    wallet_capabilities(monkeypatch, deposit=False, funding=False)
    for changes, message in ((dict(allow=["deposit"]), "durable deposit references"),
                             (dict(allow=["approve"]), "durable funding references"),
                             (dict(allow=["withdraw"]), "requires at least one --withdrawal-recipient"),
                             (dict(withdrawal_recipient=["0x" + "ab" * 20]), "applies only when withdraw"),
                             (dict(allow=["replace"]), "--replacement-fees"),
                             (dict(allow=["replace"], replacement_fees=["1", "2"]), "exceeds its cap"),
                             (dict(allow=["withdraw"], withdrawal_recipient=["0x" + "0" * 40]), "canonical nonzero"),
                             (dict(maximum_amount="0"), "positive")):
        with pytest.raises(ValueError, match=message):
            init.wallet_policy(policy_args(world, **changes))
    assert not (world.root / "buyer/wallet-policy.json").exists()


def test_wallet_policy_refuses_wrap_on_the_usdc_pool_and_changed_binaries(world, monkeypatch):
    init.init(init_args(world, "buyer", pool="usdc"))
    wallet_capabilities(monkeypatch)
    with pytest.raises(ValueError, match="wmon"):
        init.wallet_policy(policy_args(world, allow=["wrap"]))
    (world.bins / "erebus-wallet").write_text("#!/bin/sh\necho changed\n")
    with pytest.raises(ValueError, match="erebus-wallet is not the published binary"):
        init.wallet_policy(policy_args(world))


def test_wallet_status_is_optional_and_gated_on_paired_diagnostics(world, monkeypatch):
    init.init(init_args(world, "buyer"))
    real = init.command

    def capable(diagnostics):
        def fake(bins, name, request, cwd):
            if name == "erebus-wallet" and request == {"method": "version"}:
                return {"status": "ok", "result": {"protocol": 1, "methods": ["notes", "sync", "status", *init.WALLET_WRITES],
                        "paired_account_diagnostics": diagnostics}}
            return real(bins, name, request, cwd)
        return fake
    monkeypatch.setattr(init, "command", capable(False))
    assert "status" not in init.wallet_policy(policy_args(world))["allowed_methods"]
    with pytest.raises(ValueError, match="paired_account_diagnostics"):
        init.wallet_policy(policy_args(world, allow=["status"]))
    monkeypatch.setattr(init, "command", capable(True))
    assert init.wallet_policy(policy_args(world, allow=["status"]))["allowed_methods"] == ["notes", "status", "sync"]



def sponsored(world, **changes):
    key = world.root / "relay.token"
    if not key.exists():
        key.write_bytes(b"\0" * 32)
        key.chmod(0o600)
    values = dict(relay_url="https://relay.example.net/v1/relay", relay_identity="ab" * 32,
                  relay_sponsor="0x" + "5a" * 20, relay_auth_key_file=key)
    values.update(changes)
    return values


def payment_capability(monkeypatch, advertised):
    real = init.command

    def fake(bins, name, request, cwd):
        if name == "erebus-payment" and request == {"method": "version"}:
            return {"status": "ok", "protocol_version": 1, **({"sponsored_relay": True} if advertised else {})}
        return real(bins, name, request, cwd)
    monkeypatch.setattr(init, "command", fake)


def test_direct_buyers_never_get_a_relay_field(world):
    init.init(init_args(world, "buyer"))
    assert "relay" not in json.loads((world.root / "buyer/payment.json").read_text())


def test_sponsored_setup_is_refused_without_the_advertised_payment_capability(world, monkeypatch):
    payment_capability(monkeypatch, advertised=False)
    with pytest.raises(ValueError, match="sponsored_relay: true"):
        init.init(init_args(world, "buyer", **sponsored(world)))
    assert not (world.root / "buyer").exists()


@pytest.mark.parametrize("changes,message", [
    (dict(transaction_key_file=Path("/etc/hosts")), "must not hold a transaction key"),
    (dict(relay_url="http://relay.example.net/v1/relay"), "https"),
    (dict(relay_url="https://relay.example.net/v1/other"), "/v1/relay"),
    (dict(relay_identity="00" * 32), "nonzero 32-byte"),
    (dict(relay_sponsor="0x" + "0" * 40), "nonzero gas address"),
    (dict(relay_identity=None), "needs --relay-url"),
])
def test_sponsored_setup_validates_every_pinned_value(world, monkeypatch, changes, message):
    payment_capability(monkeypatch, advertised=True)
    values = {**sponsored(world), **changes}
    with pytest.raises(ValueError, match=message):
        init.init(init_args(world, "buyer", **values))
    with pytest.raises(ValueError, match="only a buyer"):
        init.init(init_args(world, "seller", **sponsored(world)))


def test_sponsored_buyer_pins_the_relay_and_holds_no_transaction_key(world, monkeypatch):
    payment_capability(monkeypatch, advertised=True)
    init.init(init_args(world, "buyer", **sponsored(world)))
    payment = json.loads((world.root / "buyer/payment.json").read_text())
    assert payment["transaction_key_file"] is None
    assert payment["relay"] == {"url": "https://relay.example.net/v1/relay", "identity": "ab" * 32,
                                "authentication_key_file": str(world.root / "relay.token"),
                                "sponsor_address": "0x" + "5a" * 20, "timeout_seconds": 60}
    if (ROOT / "sdk/shielded/src/relay_client.rs").exists():
        assert set(payment["relay"]) <= rust_fields("sdk/shielded/src/relay_client.rs", "pub struct RelayEndpoint")


def connected_buyer_and_seller(world):
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    init.connect(connect_args(world, "seller"))
    init.connect(connect_args(world, "buyer"))


def offline_chain(monkeypatch, passed=True, chain=10143):
    monkeypatch.setattr(init, "network_check", lambda bins, url, cwd: {"passed": passed, "chain_id": chain})
    real = init.command

    def fake(bins, name, request, cwd):
        if name == "erebus-wallet" and request.get("method") == "notes":
            json.loads(Path(request["config_file"]).read_text())
            return {"status": "ok", "result": {"notes": [], "operations": []}}
        return real(bins, name, request, cwd)
    monkeypatch.setattr(init, "command", fake)


def endpoint_args(world, role, **changes):
    values = dict(directory=world.root / role, rpc_url="https://c.example.net/", peer_rpc_url="https://d.example.net/",
                  bin=None, resume=False, abort=False)
    values.update(changes)
    return SimpleNamespace(**values)


def test_endpoint_change_updates_every_config_consistently_and_preserves_everything_else(world, monkeypatch):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    seller = world.root / "seller"
    before_access = json.loads((seller / "access.json").read_text())
    state_files = {p: p.read_bytes() for p in (seller / "state").rglob("*") if p.is_file()}
    result = init.endpoints(endpoint_args(world, "seller"))
    assert result["status"] == "changed" and result["configs"] == ["wallet.json", "access.json"] and result["transactions_sent"] == 0
    wallet, access = json.loads((seller / "wallet.json").read_text()), json.loads((seller / "access.json").read_text())
    assert (wallet["rpc_url"], wallet["peer_rpc_url"]) == ("https://c.example.net/", "https://d.example.net/")
    assert (access["backend"]["rpc_url"], access["backend"]["peer_rpc_url"]) == ("https://c.example.net/", "https://d.example.net/")
    assert {k: v for k, v in access.items() if k != "backend"} == {k: v for k, v in before_access.items() if k != "backend"}
    assert {p: p.read_bytes() for p in (seller / "state").rglob("*") if p.is_file()} == state_files
    assert all(owner_only(seller / name) for name in ("wallet.json", "access.json")) and not (seller / ".endpoint-change").exists()
    buyer = init.endpoints(endpoint_args(world, "buyer"))
    assert buyer["configs"] == ["wallet.json", "payment.json"]
    assert "relay" not in json.loads((world.root / "buyer/payment.json").read_text())


@pytest.mark.parametrize("changes,message", [
    (dict(peer_rpc_url="https://c.example.net"), "must differ"),
    (dict(rpc_url="http://c.example.net/"), "https"),
    (dict(rpc_url="https://user:pw@c.example.net/"), "credentials"),
    (dict(rpc_url=None), "needs --rpc-url"),
])
def test_endpoint_change_refuses_bad_endpoints_without_touching_configs(world, monkeypatch, changes, message):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    before = (world.root / "buyer/wallet.json").read_bytes()
    with pytest.raises(ValueError, match=message):
        init.endpoints(endpoint_args(world, "buyer", **changes))
    assert (world.root / "buyer/wallet.json").read_bytes() == before and not (world.root / "buyer/.endpoint-change").exists()


def test_endpoint_change_refuses_endpoints_failing_the_chain_check(world, monkeypatch):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch, passed=True, chain=1)
    before = (world.root / "buyer/payment.json").read_bytes()
    with pytest.raises(ValueError, match="chain 10143; nothing changed"):
        init.endpoints(endpoint_args(world, "buyer"))
    assert (world.root / "buyer/payment.json").read_bytes() == before


def test_an_interrupted_endpoint_change_resumes_or_aborts_exactly(world, monkeypatch):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    buyer = world.root / "buyer"
    originals = {name: (buyer / name).read_bytes() for name in ("wallet.json", "payment.json")}
    real_replace, calls = init.os.replace, []

    def crash_after_first(source, target):
        calls.append(Path(target).name)
        if Path(target).name == "payment.json":
            raise OSError("power lost")
        return real_replace(source, target)
    monkeypatch.setattr(init.os, "replace", crash_after_first)
    with pytest.raises(OSError):
        init.endpoints(endpoint_args(world, "buyer"))
    monkeypatch.setattr(init.os, "replace", real_replace)
    assert json.loads((buyer / "wallet.json").read_text())["rpc_url"] == "https://c.example.net/"
    assert (buyer / "payment.json").read_bytes() == originals["payment.json"]
    assert "endpoint change is incomplete" in " ".join(init.doctor(SimpleNamespace(directory=buyer, offline=True))["problems"])
    with pytest.raises(ValueError, match="already staged"):
        init.endpoints(endpoint_args(world, "buyer"))
    aborted = init.endpoints(endpoint_args(world, "buyer", rpc_url=None, peer_rpc_url=None, abort=True))
    assert aborted["status"] == "aborted" and {name: (buyer / name).read_bytes() for name in originals} == originals
    monkeypatch.setattr(init.os, "replace", crash_after_first)
    with pytest.raises(OSError):
        init.endpoints(endpoint_args(world, "buyer"))
    monkeypatch.setattr(init.os, "replace", real_replace)
    resumed = init.endpoints(endpoint_args(world, "buyer", rpc_url=None, peer_rpc_url=None, resume=True))
    assert resumed["status"] == "resumed" and resumed["applied_now"] == ["payment.json"]
    assert json.loads((buyer / "payment.json").read_text())["peer_rpc_url"] == "https://d.example.net/"
    assert not (buyer / ".endpoint-change").exists() and owner_only(buyer / "payment.json")


def test_doctor_reports_damaged_configs_without_crashing_or_echoing_contents(world, monkeypatch):
    init.init(init_args(world, "buyer"))
    offline_chain(monkeypatch)
    buyer = world.root / "buyer"
    (buyer / "wallet.json").write_text("{not json: secret-ish-content")
    report = init.doctor(SimpleNamespace(directory=buyer, offline=True))
    assert report["status"] == "attention" and "secret-ish-content" not in json.dumps(report)
    assert any("wallet.json is missing or damaged" in problem for problem in report["problems"])
    (buyer / "operator.json").write_text("[]")
    assert init.doctor(SimpleNamespace(directory=buyer, offline=True))["status"] == "attention"


def test_doctor_separates_balance_failures_from_the_network_verdict(world, monkeypatch):
    init.init(init_args(world, "buyer"))
    offline_chain(monkeypatch)

    def refused(url, address, opener=None):
        raise OSError("403")
    monkeypatch.setattr(init, "rpc_balance", refused)
    report = init.doctor(SimpleNamespace(directory=world.root / "buyer", offline=False))
    endpoints = report["checks"]["endpoints"]
    assert all(entry["passed"] is True and entry["signer_balance_wei"] is None for entry in endpoints.values())
    assert sum("balance read failed" in problem for problem in report["problems"]) == 2


def test_operator_requests_carry_an_explicit_user_agent():
    request = init.request("https://rpc.example.net/", b"{}", {"content-type": "application/json"})
    assert request.get_header("User-agent").startswith("erebus-init/")


# ---- endpoint-change recovery matrix -------------------------------------------------------------------------

ENDPOINT_FILES = ("wallet.json", "payment.json")  # a buyer's endpoint-bearing configs, in replacement order


def snapshot(directory):
    return {name: (directory / name).read_bytes() for name in ENDPOINT_FILES}


def crash_on(monkeypatch, target_name, after):
    """Make os.replace fail when it touches `target_name`, either before or after the real replacement."""
    real = init.os.replace

    def replace(source, target):
        if Path(target).name == target_name:
            if after:
                real(source, target)
            raise OSError(f"injected crash {'after' if after else 'before'} {target_name}")
        return real(source, target)
    monkeypatch.setattr(init.os, "replace", replace)
    return real


def change(world, **changes):
    return init.endpoints(endpoint_args(world, "buyer", **changes))


@pytest.mark.parametrize("target_name", ENDPOINT_FILES)
@pytest.mark.parametrize("after", [False, True])
@pytest.mark.parametrize("recovery", ["resume", "abort"])
def test_every_interruption_point_recovers_to_one_consistent_pair(world, monkeypatch, target_name, after, recovery):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    buyer = world.root / "buyer"
    before = snapshot(buyer)
    real = crash_on(monkeypatch, target_name, after)
    with pytest.raises(OSError, match="injected crash"):
        change(world)
    monkeypatch.setattr(init.os, "replace", real)
    report = init.doctor(SimpleNamespace(directory=buyer, offline=True))
    assert report["status"] == "attention" and any("incomplete" in p for p in report["problems"])
    result = change(world, rpc_url=None, peer_rpc_url=None, **{recovery: True})
    assert result["status"] == ("resumed" if recovery == "resume" else "aborted")
    pairs = {(json.loads((buyer / n).read_text())["rpc_url"], json.loads((buyer / n).read_text())["peer_rpc_url"]) for n in ENDPOINT_FILES}
    expected = ("https://c.example.net/", "https://d.example.net/") if recovery == "resume" else ("https://a.example.net/", "https://b.example.net/")
    assert pairs == {expected}
    if recovery == "abort":
        assert snapshot(buyer) == before
    assert all(owner_only(buyer / n) for n in ENDPOINT_FILES) and not (buyer / ".endpoint-change").exists()
    assert init.doctor(SimpleNamespace(directory=buyer, offline=True))["status"] == "healthy"
    for again in ("resume", "abort"):
        with pytest.raises(ValueError, match="no interrupted endpoint change"):
            change(world, rpc_url=None, peer_rpc_url=None, **{again: True})


def test_an_interruption_before_the_journal_commits_changes_nothing(world, monkeypatch):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    buyer = world.root / "buyer"
    before = snapshot(buyer)
    crash_on(monkeypatch, "journal.json", after=False)
    with pytest.raises(OSError):
        change(world)
    monkeypatch.undo()
    offline_chain(monkeypatch)
    assert snapshot(buyer) == before and (buyer / ".endpoint-change").exists()
    with pytest.raises(ValueError, match="before it was committed"):
        change(world, rpc_url=None, peer_rpc_url=None, resume=True)
    assert change(world, rpc_url=None, peer_rpc_url=None, abort=True)["status"] == "aborted"
    assert snapshot(buyer) == before and not (buyer / ".endpoint-change").exists()


def interrupted_after_first(world, monkeypatch):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    real = crash_on(monkeypatch, "payment.json", after=False)
    with pytest.raises(OSError):
        change(world)
    monkeypatch.setattr(init.os, "replace", real)
    return world.root / "buyer"


@pytest.mark.parametrize("damage", ["truncate", "garbage", "drop_files", "bad_hash"])
def test_a_damaged_journal_is_refused_without_touching_any_config(world, monkeypatch, damage):
    buyer = interrupted_after_first(world, monkeypatch)
    journal = buyer / ".endpoint-change/journal.json"
    data = json.loads(journal.read_text())
    journal.write_text({"truncate": journal.read_text()[:40], "garbage": "\0\0not json",
                        "drop_files": json.dumps({**data, "files": []}),
                        "bad_hash": json.dumps({**data, "files": [{**data["files"][0], "new_sha256": "zz"}]})}[damage])
    current = snapshot(buyer)
    for recovery in ("resume", "abort"):
        with pytest.raises(ValueError, match="journal is damaged"):
            change(world, rpc_url=None, peer_rpc_url=None, **{recovery: True})
    assert snapshot(buyer) == current
    assert init.doctor(SimpleNamespace(directory=buyer, offline=True))["status"] == "attention"


def test_damaged_staged_files_are_refused_and_abort_still_restores(world, monkeypatch):
    buyer = interrupted_after_first(world, monkeypatch)
    (buyer / ".endpoint-change/payment.json.new").write_text("{}")
    with pytest.raises(ValueError, match="staged payment.json is missing or damaged"):
        change(world, rpc_url=None, peer_rpc_url=None, resume=True)
    (buyer / ".endpoint-change/wallet.json.orig").write_text("{}")
    with pytest.raises(ValueError, match="saved original of wallet.json"):
        change(world, rpc_url=None, peer_rpc_url=None, abort=True)


def test_a_concurrent_edit_after_staging_is_never_overwritten(world, monkeypatch):
    buyer = interrupted_after_first(world, monkeypatch)
    edited = json.loads((buyer / "payment.json").read_text())
    edited["maximum_price"] = "1"
    (buyer / "payment.json").write_text(json.dumps(edited))
    with pytest.raises(ValueError, match="payment.json was changed by something else"):
        change(world, rpc_url=None, peer_rpc_url=None, resume=True)
    assert json.loads((buyer / "payment.json").read_text())["maximum_price"] == "1"
    with pytest.raises(ValueError, match="changed by something else"):
        change(world, rpc_url=None, peer_rpc_url=None, abort=True)
    assert json.loads((buyer / "payment.json").read_text())["maximum_price"] == "1"
    assert init.doctor(SimpleNamespace(directory=buyer, offline=True))["status"] == "attention"


def test_symlinks_and_unsafe_permissions_are_refused_before_staging(world, monkeypatch, tmp_path):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    buyer = world.root / "buyer"
    outside = tmp_path / "elsewhere.json"
    outside.write_bytes((buyer / "payment.json").read_bytes())
    (buyer / "payment.json").unlink()
    (buyer / "payment.json").symlink_to(outside)
    with pytest.raises(ValueError, match="payment.json must be a regular owner-only file"):
        change(world)
    (buyer / "payment.json").unlink()
    (buyer / "payment.json").write_bytes(outside.read_bytes())
    (buyer / "payment.json").chmod(0o644)
    with pytest.raises(ValueError, match="regular owner-only"):
        change(world)
    (buyer / "payment.json").chmod(0o600)
    (buyer / ".endpoint-change").symlink_to(tmp_path)
    with pytest.raises(ValueError, match="not a real directory"):
        change(world)
    (buyer / ".endpoint-change").unlink()
    buyer.chmod(0o770)
    try:
        with pytest.raises(ValueError, match="only its owner can write"):
            change(world)
    finally:
        buyer.chmod(0o700)
    assert not (buyer / ".endpoint-change").exists() and outside.read_bytes() == (buyer / "payment.json").read_bytes()


def test_a_second_concurrent_endpoint_change_is_refused(world, monkeypatch):
    import fcntl
    import os
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    held = os.open(world.root / "buyer/.endpoint-change.lock", os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(held, fcntl.LOCK_EX)
        with pytest.raises(ValueError, match="another endpoint change is running"):
            change(world)
    finally:
        os.close(held)
    assert change(world)["status"] == "changed"


def test_doctor_reports_a_mixed_endpoint_configuration(world, monkeypatch):
    connected_buyer_and_seller(world)
    offline_chain(monkeypatch)
    payment = world.root / "buyer/payment.json"
    data = json.loads(payment.read_text())
    data["rpc_url"] = "https://other.example.net/"
    payment.write_text(json.dumps(data))
    report = init.doctor(SimpleNamespace(directory=world.root / "buyer", offline=True))
    assert report["status"] == "attention" and any("different RPC endpoint pairs" in p for p in report["problems"])


def test_init_refuses_a_signature_envelope_for_another_manifest(world):
    other = json.loads((world.pins / "manifest-usdc.signature.json").read_text())
    (world.pins / "manifest-wmon.signature.json").write_text(json.dumps(other))
    with pytest.raises(ValueError, match="signature envelope is not for its pinned hosting manifest"):
        init.init(init_args(world, "buyer"))
    assert not (world.root / "buyer").exists()


def test_auditor_rechecks_installed_binaries_before_keygen(world):
    (world.bins / "erebus-shielded-disclosure").write_text("#!/bin/sh\necho changed\n")
    with pytest.raises(ValueError, match="erebus-shielded-disclosure is not the published binary"):
        init.auditor(SimpleNamespace(pool="wmon", directory=world.root / "auditor", bin=world.bins, pins=world.pins,
                                     deployment=world.deployment, rpc_url="https://a.example.net/", peer_rpc_url="https://b.example.net/"))
    assert not (world.root / "auditor").exists()


def test_no_command_output_contains_key_material_and_the_relay_key_is_never_read(world, monkeypatch, capsys):
    payment_capability(monkeypatch, advertised=True)
    values = sponsored(world)
    values["relay_auth_key_file"].write_bytes(b"RELAY-SECRET-" + b"7" * 19)
    values["relay_auth_key_file"].chmod(0o000)  # unreadable: any attempt to open it would fail the command
    outputs = [init.init(init_args(world, "buyer", **values)), init.init(init_args(world, "seller"))]
    values["relay_auth_key_file"].chmod(0o600)
    outputs += [init.connect(connect_args(world, "seller")), init.connect(connect_args(world, "buyer"))]
    wallet_capabilities(monkeypatch)
    outputs.append(init.wallet_policy(policy_args(world)))
    offline_chain(monkeypatch)
    outputs.append(init.doctor(SimpleNamespace(directory=world.root / "buyer", offline=True)))
    outputs.append(init.endpoints(endpoint_args(world, "seller")))
    rendered = json.dumps(outputs) + capsys.readouterr().out
    secrets = [p.read_bytes() for p in world.root.rglob("*.key")] + [values["relay_auth_key_file"].read_bytes()]
    assert secrets and not any(secret.hex() in rendered or secret.decode("latin-1") in rendered for secret in secrets)
    configs = "".join(p.read_text() for p in world.root.rglob("*.json") if "share" not in p.parts)
    assert "RELAY-SECRET" not in configs and all(secret.hex() not in configs for secret in secrets)


def candidate_release(world, tmp_path, **changes):
    """An unpublished single-platform registry release.json recording the authorized hosting manifests."""
    authorization = json.loads((world.pins / "testnet-release.json").read_text())
    if not (world.pins / "monad-testnet-shielded-v2.json").exists():
        (world.pins / "monad-testnet-shielded-v2.json").write_bytes(world.deployment.read_bytes())
    binaries = {name: {"sha256": hashlib.sha256((world.bins / name).read_bytes()).hexdigest()} for name in init.COMMANDS}
    release = {"version": "0.3.0.dev2026101399", "source_commit": "482d912", "published": False,
               "platform": "macosx-11.0-arm64", "platform_qualification": {"wheel_tag": "macosx_11_0_arm64"},
               "binaries": binaries, "candidate": {"source_tree_sha256": "ab" * 32, "release_eligible": False},
               "proving_domains": [{"manifest_sha256": pin} for pin in authorization["hosting_manifests"].values()]}
    release.update(changes)
    path = tmp_path / "candidate-release.json"
    path.write_text(json.dumps(release))
    return path, hashlib.sha256(path.read_bytes()).hexdigest()


def rehearsal_args(world, path, digest, directory=None, **changes):
    values = dict(version=None, directory=directory or world.root / "rehearsal-pins", rehearsal_release=path,
                  rehearsal_release_sha256=digest, authorized_pins=world.pins)
    values.update(changes)
    return SimpleNamespace(**values)


def test_rehearsal_pins_bind_the_exact_candidate_to_the_published_authorization(world, tmp_path):
    path, digest = candidate_release(world, tmp_path)
    result = init.pins(rehearsal_args(world, path, digest))
    pins = world.root / "rehearsal-pins"
    assert result["rehearsal"] is True and result["publication_evidence"] is False
    assert not (pins / "release-manifest.json").exists(), "a rehearsal must not look like published pins"
    for name in ("testnet-release.json", "manifest-wmon.json", "manifest-wmon.signature.json", "manifest-usdc.json"):
        assert (pins / name).read_bytes() == (world.pins / name).read_bytes(), "authorizations are copied, never substituted"
    assert (pins / "candidate-release.json").read_bytes() == path.read_bytes()
    marker = json.loads((pins / "rehearsal.json").read_text())
    assert marker["publication_evidence"] is False and "not-publication-evidence" in marker["labels"]
    # Installed candidate binaries differ from the published ones: the candidate's own hashes are checked.
    (world.bins / "erebus-wallet").write_text("#!/bin/sh\n# candidate build\n")
    path, digest = candidate_release(world, tmp_path)
    init.pins(rehearsal_args(world, path, digest, directory=world.root / "candidate-pins"))
    with pytest.raises(ValueError, match="erebus-wallet is not the published binary"):
        init.init(init_args(world, "buyer"))
    init.init(init_args(world, "buyer", pins=world.root / "candidate-pins"))
    operator = json.loads((world.root / "buyer/operator.json").read_text())
    assert operator["rehearsal"]["publication_evidence"] is False and operator["release_version"] == "0.3.0.dev2026101399"
    assert {"rehearsal-pins", "not-publication-evidence"} <= set(operator["labels"])
    assert init.doctor(SimpleNamespace(directory=world.root / "buyer", offline=True))["rehearsal"]["publication_evidence"] is False
    auditor = init.auditor(SimpleNamespace(pool="wmon", directory=world.root / "auditor", bin=world.bins,
                                           pins=world.root / "candidate-pins", deployment=world.deployment,
                                           rpc_url="https://a.example.net/", peer_rpc_url="https://b.example.net/"))
    assert auditor["rehearsal"]["publication_evidence"] is False and (world.root / "auditor/rehearsal.json").is_file()


@pytest.mark.parametrize("change,message", [
    (lambda world, path, digest: {"rehearsal_release_sha256": "0" * 64}, "does not have the given SHA-256"),
    (lambda world, path, digest: {"rehearsal_release_sha256": None}, "--rehearsal-release-sha256"),
    (lambda world, path, digest: {"authorized_pins": None}, "--authorized-pins"),
    (lambda world, path, digest: {"version": "0.3.0.dev2026101202"}, "exclusive"),
])
def test_rehearsal_pins_refuse_incomplete_or_mismatched_inputs(world, tmp_path, change, message):
    path, digest = candidate_release(world, tmp_path)
    with pytest.raises(ValueError, match=message):
        init.pins(rehearsal_args(world, path, digest, **change(world, path, digest)))
    assert not (world.root / "rehearsal-pins").exists()


def test_rehearsal_pins_never_substitute_an_authorization_or_a_publisher(world, tmp_path):
    path, digest = candidate_release(world, tmp_path, proving_domains=[{"manifest_sha256": "f" * 64}])
    with pytest.raises(ValueError, match="exactly the authorized hosting manifests"):
        init.pins(rehearsal_args(world, path, digest))
    path, digest = candidate_release(world, tmp_path, published=True)
    with pytest.raises(ValueError, match="unpublished"):
        init.pins(rehearsal_args(world, path, digest, directory=world.root / "p2"))
    path, digest = candidate_release(world, tmp_path)
    init.pins(rehearsal_args(world, path, digest))
    with pytest.raises(ValueError, match="not a rehearsal"):
        init.pins(rehearsal_args(world, path, digest, directory=world.root / "p3", authorized_pins=world.root / "rehearsal-pins"))
    (world.pins / "manifest-usdc.json").write_text("{}")
    with pytest.raises(ValueError, match="does not match the release authorization"):
        init.pins(rehearsal_args(world, path, digest, directory=world.root / "p4"))
    for name in ("p2", "p3", "p4"):
        assert not (world.root / name).exists()


def test_a_tampered_rehearsal_marker_or_mixed_pins_are_refused_at_init(world, tmp_path):
    path, digest = candidate_release(world, tmp_path)
    init.pins(rehearsal_args(world, path, digest))
    pins = world.root / "rehearsal-pins"
    marker = json.loads((pins / "rehearsal.json").read_text())
    (pins / "rehearsal.json").write_text(json.dumps({**marker, "publication_evidence": True}))
    with pytest.raises(ValueError, match="do not match their marker"):
        init.init(init_args(world, "buyer", pins=pins))
    (pins / "rehearsal.json").write_text(json.dumps(marker))
    (pins / "release-manifest.json").write_bytes((world.pins / "release-manifest.json").read_bytes())
    with pytest.raises(ValueError, match="published or rehearsal, never both"):
        init.init(init_args(world, "buyer", pins=pins))
    assert not (world.root / "buyer").exists()


def test_buyer_retrieval_settings_come_from_the_sellers_share_and_require_tls_or_explicit_loopback(world):
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    init.connect(connect_args(world, "seller"))
    service = json.loads((world.root / "seller/share/service.json").read_text())
    access = json.loads((world.root / "seller/access.json").read_text())
    assert bytes(access["service_id"]).hex() == service["service_id"] and service["port"] == 18732
    for changes, message in ((dict(service_url="http://seller.example.net/v1/access"), "must use https"),
                             (dict(service_url="http://127.0.0.1:18732/v1/access"), "must use https"),
                             (dict(service_url="https://seller.example.net/v1/other"), "/v1/access"),
                             (dict(service_url="https://u:p@seller.example.net/v1/access"), "without credentials"),
                             (dict(service_url="https://seller.example.net/v1/access", allow_loopback_access_http=True), "applies only"),
                             (dict(service_url="https://seller.example.net/v1/access", resource="other"), "different resource")):
        with pytest.raises(ValueError, match=message):
            init.connect(connect_args(world, "buyer", **changes))
        assert not (world.root / "buyer/mcp.json").exists() and not (world.root / "buyer/access-cache").exists()
    with pytest.raises(ValueError, match="apply to a buyer"):
        init.connect(connect_args(world, "seller", service_url="https://x.example.net/v1/access"))
    init.connect(connect_args(world, "buyer", service_url="http://127.0.0.1:18732/v1/access", allow_loopback_access_http=True))
    environment = json.loads((world.root / "buyer/mcp.json").read_text())["mcpServers"]["erebus-monad-buyer"]["env"]
    buyer = json.loads((world.root / "buyer/operator.json").read_text())
    assert environment["EREBUS_ACCESS_SERVICE_ID"] == service["service_id"]
    assert environment["EREBUS_ACCESS_BUYER_KEY_FILE"] == buyer["operator"]["agreement_key_file"]
    assert environment["EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP"] == "1" and owner_only(world.root / "buyer/access-cache")
    assert environment["EREBUS_ACCESS_CLI"] == str(world.bins / "erebus-access")


def test_the_buyer_dials_the_endpoint_the_sellers_descriptor_advertises(world):
    connected_buyer_and_seller(world)
    buyer = json.loads((world.root / "buyer/negotiate.json").read_text())
    seller = json.loads((world.root / "seller/negotiate.json").read_text())
    assert seller["endpoint"] == "127.0.0.1:9102" and buyer["endpoint"] == "127.0.0.1:9102"


def test_a_peer_descriptor_without_exactly_one_tcp_endpoint_is_refused(world):
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    init.connect(connect_args(world, "seller"))
    (world.root / "seller/share/seller.descriptor.json").write_text(json.dumps({"endpoints": ["tcp://a:1", "tcp://b:2"]}))
    with pytest.raises(ValueError, match="exactly one tcp:// endpoint"):
        init.connect(connect_args(world, "buyer"))
    assert not (world.root / "buyer/negotiate.json").exists()


SENTINEL = "LEAKAGE_REVIEW_SENTINEL"


def test_native_replies_are_never_echoed_through_any_wrapper_channel(world, monkeypatch, capsys):
    """Every error and report channel prints fixed text, whatever a native command puts in its reply."""
    real = init.subprocess.run

    def hostile(command, **kwargs):
        name = Path(command[0]).name
        if name in {"erebus-negotiate", "erebus-shielded-disclosure", "erebus-wallet", "erebus-payment"}:
            return SimpleNamespace(returncode=1, stdout=json.dumps({"status": "error", "error": SENTINEL, "detail": SENTINEL}),
                                   stderr=SENTINEL)
        if name == "erebus-network-check":
            return SimpleNamespace(returncode=0, stdout=json.dumps({"passed": True, "finalized": {"number": SENTINEL},
                                                                   "chain_id": 10143, "error": SENTINEL}), stderr=SENTINEL)
        return real(command, **kwargs)

    connected_buyer_and_seller(world)
    monkeypatch.setattr(init.subprocess, "run", hostile)
    monkeypatch.setattr(init, "rpc_balance", lambda url, address, opener=None: 1)
    calls = [["init", "--role", "buyer", "--pool", "wmon", "--directory", str(world.root / "b2"), "--bin", str(world.bins),
              "--pins", str(world.pins), "--deployment", str(world.deployment), "--endpoint", "127.0.0.1:1",
              "--rpc-url", "https://a.example.net/", "--peer-rpc-url", "https://b.example.net/",
              "--signer-address", "0x" + "12" * 20, "--maximum-price", "5"],
             ["auditor", "--pool", "wmon", "--directory", str(world.root / "a2"), "--bin", str(world.bins),
              "--pins", str(world.pins), "--deployment", str(world.deployment), "--rpc-url", "https://a.example.net/",
              "--peer-rpc-url", "https://b.example.net/"],
             ["wallet-policy", "--directory", str(world.root / "buyer"), "--maximum-amount", "5"],
             ["doctor", "--directory", str(world.root / "buyer")]]
    for arguments in calls:
        init.main(arguments)
        printed = capsys.readouterr()
        assert SENTINEL not in printed.out + printed.err, arguments[0]


def prepublication_args(world, path, digest, authorization, directory=None, **changes):
    values = dict(version=None, rehearsal_release=None, directory=directory or world.root / "pre-pins",
                  prepublication_release=path, prepublication_release_sha256=digest, authorization=authorization,
                  authorized_pins=world.pins)
    values.update(changes)
    return SimpleNamespace(**values)


def test_prepublication_pins_need_an_authorization_naming_the_clean_candidate(world, tmp_path):
    authorization = json.loads((world.pins / "testnet-release.json").read_text())
    named = tmp_path / "authorization-v.json"
    named.write_text(json.dumps({**authorization, "package_version": "0.3.0.dev2026101399"}))
    clean = {"candidate": {"source_tree_sha256": "ab" * 32, "release_eligible": True}}
    path, digest = candidate_release(world, tmp_path, **clean)
    result = init.pins(prepublication_args(world, path, digest, named))
    assert result["prepublication"] is True and result["rehearsal"] is False
    pins = world.root / "pre-pins"
    assert not (pins / "rehearsal.json").exists() and (pins / "testnet-release.json").read_bytes() == named.read_bytes()
    init.init(init_args(world, "buyer", pins=pins))
    operator = json.loads((world.root / "buyer/operator.json").read_text())
    assert operator["prepublication"] is True and "rehearsal" not in operator and "rehearsal-pins" not in operator["labels"]
    # The old authorization (another version), a dirty tree or other manifests are refused, and nothing is written.
    for changes, authorization_file, message in (
            ({}, world.pins / "testnet-release.json", "does not name this candidate's version"),
            ({"candidate": {"release_eligible": False}}, named, "clean, release-eligible"),
            ({**clean, "proving_domains": [{"manifest_sha256": "f" * 64}]}, named, "exactly the authorized hosting manifests")):
        path, digest = candidate_release(world, tmp_path, **changes)
        with pytest.raises(ValueError, match=message):
            init.pins(prepublication_args(world, path, digest, authorization_file, directory=world.root / "refused"))
        assert not (world.root / "refused").exists()
    with pytest.raises(ValueError, match="excludes"):
        init.pins(prepublication_args(world, path, digest, named, directory=world.root / "x", version="1"))


def test_every_file_a_native_command_reads_as_a_local_input_is_owner_only(world):
    """erebus-payment refuses a hosting-manifest copy that others can read, so settlement would fail after funding."""
    connected_buyer_and_seller(world)
    for role in ("buyer", "seller"):
        config = json.loads((world.root / role / "wallet.json").read_text())
        assert owner_only(Path(config["artifact_manifest_file"])), role
    assert owner_only(Path(json.loads((world.root / "buyer/payment.json").read_text())["manifest_file"]))


NATIVE_NEGOTIATE = ROOT / "sdk/shielded/target/debug/erebus-negotiate"


@pytest.mark.skipif(not NATIVE_NEGOTIATE.is_file(), reason="build sdk/shielded erebus-negotiate")
def test_the_native_loader_accepts_the_written_peer_table_with_a_buyer_priced_above_the_first(world):
    """The real erebus-negotiate prepares keys and terms and then loads what connect wrote, so the Python writer
    and the native peer-table checks are tested against each other, not against a fake."""
    import shutil
    real = world.bins / "erebus-negotiate"
    shutil.copyfile(NATIVE_NEGOTIATE, real)
    real.chmod(0o755)
    release = json.loads((world.pins / "release-manifest.json").read_text())
    platform, = release["platforms"].values()
    platform["binaries"]["erebus-negotiate"]["sha256"] = init.sha256(real)
    (world.pins / "release-manifest.json").write_text(json.dumps(release))
    for role in ("buyer", "seller"):
        init.init(init_args(world, role))
    init.connect(connect_args(world, "seller", amount="1000"))
    init.connect(connect_args(world, "buyer", amount="1000"))
    seller = world.root / "seller"

    def listed() -> dict:
        return init.command(world.bins, "erebus-negotiate",
                            {"method": "list_peers", "config_file": str(seller / "negotiate.json")}, seller)
    assert [(peer["peer"], peer["live"]) for peer in listed()["peers"]] == [("default", True)]
    # A second buyer at twice the first's price: the seller's ceiling follows it.
    second = init_second_buyer(world)
    added = init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b2", amount="2000"))
    assert "negotiate.json" in added["written"]
    assert json.loads((seller / "negotiate.json").read_text())["maximum_price"] == "2000"
    peers = listed()["peers"]
    assert {peer["peer"]: (peer["live"], peer["price"]) for peer in peers} == {
        "default": (True, "1000"), "b2": (True, "2000")}
    # A cheaper third buyer leaves the ceiling alone.
    third = init_second_buyer(world, name="buyer3", port=9104)
    init.connect(connect_args(world, "seller", peer_share=third / "share", peer_id="b3", amount="500"))
    assert json.loads((seller / "negotiate.json").read_text())["maximum_price"] == "2000"
    assert len(listed()["peers"]) == 3
    # A directory written by the earlier release (ceiling below b2) is repaired by repeating b2's connect.
    config = json.loads((seller / "negotiate.json").read_text())
    init.replace_private_json(seller / "negotiate.json", {**config, "maximum_price": "1000"})
    import subprocess
    refused = subprocess.run([str(real)], input=json.dumps({"method": "list_peers",
                             "config_file": str(seller / "negotiate.json")}), capture_output=True, text=True)
    assert json.loads(refused.stdout)["error"] == "invalid price policy"
    repaired = init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b2", amount="2000"))
    assert repaired["repeated"] is True and repaired["written"] == ["negotiate.json"]
    assert len(listed()["peers"]) == 3


NATIVE_ACCESS = ROOT / "sdk/shielded/target/debug/erebus-access"


def free_port() -> int:
    import socket
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def install_native(world, *names):
    import shutil
    release = json.loads((world.pins / "release-manifest.json").read_text())
    platform, = release["platforms"].values()
    for name in names:
        real = world.bins / name
        shutil.copyfile(ROOT / "sdk/shielded/target/debug" / name, real)
        real.chmod(0o755)
        platform["binaries"][name]["sha256"] = init.sha256(real)
    (world.pins / "release-manifest.json").write_text(json.dumps(release))


def native(binary: Path, request: dict) -> dict:
    import subprocess
    completed = subprocess.run([str(binary)], input=json.dumps(request), capture_output=True, text=True, timeout=120)
    return json.loads(completed.stdout)


@pytest.mark.skipif(not (NATIVE_NEGOTIATE.is_file() and NATIVE_ACCESS.is_file()), reason="build sdk/shielded binaries")
def test_written_configs_negotiate_two_buyers_whose_evidence_passes_the_retrieval_profile(world):
    """Real writer -> real negotiation -> real erebus-access: each buyer's evidence passes the client's snapshot
    profile and reaches the network. Only the operation record (not evidence) is refused as a profile."""
    import subprocess
    install_native(world, "erebus-negotiate", "erebus-access")
    seller_port = free_port()
    init.init(init_args(world, "seller", endpoint=f"127.0.0.1:{seller_port}"))
    init.init(init_args(world, "buyer", maximum_price="5000"))
    init.connect(connect_args(world, "seller", amount="1000"))
    init.connect(connect_args(world, "buyer", amount="1000"))
    second = world.root / "buyer2"
    init.init(init_args(world, "buyer", directory=second, endpoint="127.0.0.1:9103", maximum_price="5000"))
    init.connect(connect_args(world, "seller", peer_share=second / "share", peer_id="b2", amount="2000"))
    init.connect(connect_args(world, "buyer", directory=second, peer_share=world.root / "seller" / "outbox" / "b2",
                              amount="2000"))
    seller = world.root / "seller"
    negotiate = world.bins / "erebus-negotiate"

    def run(config: Path, operation: str) -> subprocess.Popen:
        process = subprocess.Popen([str(negotiate)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        process.stdin.write(json.dumps({"method": "negotiate", "config_file": str(config), "operation_ref": operation}))
        process.stdin.close()
        return process

    service = json.loads((seller / "share" / "service.json").read_text())
    for buyer, peer, operation in ((world.root / "buyer", "default", "a1" * 32), (second, "b2", "b2" * 32)):
        selling = run(seller / "negotiate.json", "5" + operation[1:])
        buying = run(buyer / "negotiate.json", operation)
        bought = json.loads(buying.stdout.read())
        sold = json.loads(selling.stdout.read())
        buying.wait(timeout=120)
        selling.wait(timeout=120)
        assert bought["status"] == "authorized" and sold["status"] == "authorized", (bought, sold)
        assert sold["peer"] == peer and sold["deal_commitment"] == bought["deal_commitment"]
        # The seller published the access evidence its service reads.
        assert (seller / "access-evidence" / f"{sold['deal_commitment']}.evidence").is_file()
        files = json.loads((buyer / "operator.json").read_text())["operator"]
        cache = world.root / f"cache-{peer}"
        cache.mkdir(mode=0o700)
        request = {"method": "retrieve", "buyer_key_file": files["agreement_key_file"],
                   "service_url": f"http://127.0.0.1:{free_port()}/v1/access", "service_id": service["service_id"],
                   "cache_root": str(cache), "allow_loopback_http": True}
        # The evidence blob passes every client profile check; only the closed endpoint fails.
        reached = native(NATIVE_ACCESS, {**request, "evidence_file": bought["evidence_file"]})
        assert reached["error"].startswith("access service unavailable"), reached
        # The operation record next to it is not evidence: that, and only that, is a profile refusal.
        record = Path(bought["evidence_file"]).with_name(f"{operation}.json")
        refused = native(NATIVE_ACCESS, {**request, "evidence_file": str(record)})
        assert refused["error"] == "agreement does not support this retrieval profile"
