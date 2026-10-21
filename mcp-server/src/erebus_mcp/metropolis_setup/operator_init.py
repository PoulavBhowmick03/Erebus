#!/usr/bin/env python3
"""One-step operator setup for a published Metropolis release. Never broadcasts; never reads key contents.

    operator_init.py pins    --version V --directory NEW
    operator_init.py pins    --directory NEW --rehearsal-release FILE --rehearsal-release-sha256 HEX --authorized-pins DIR
    operator_init.py pins    --directory NEW --prepublication-release FILE --prepublication-release-sha256 HEX
                             --authorization FILE --authorized-pins DIR
    operator_init.py init    --role buyer|seller --pool wmon|usdc --directory NEW --bin DIR --pins DIR
                             --deployment FILE --endpoint HOST:PORT --rpc-url URL --peer-rpc-url URL
                             --signer-address 0x.. [--transaction-key-file PATH] [--maximum-price N]
    operator_init.py connect --directory DIR --peer-share DIR --resource NAME --amount N
                             [--payload-file FILE --port N] (seller)   [--minimum-price N]
    operator_init.py doctor  --directory DIR [--offline]
    operator_init.py wallet-policy --directory DIR --maximum-amount N [--allow METHOD ...]
                             [--withdrawal-recipient ADDR ...] [--replacement-fees MAX PRIORITY]
    operator_init.py auditor --pool wmon|usdc --directory NEW --bin DIR --pins DIR --deployment FILE
                             --rpc-url URL --peer-rpc-url URL [--history full|checkpoint]
    operator_init.py quickstart up|start|status|renew|down --directory DIR [--pins DIR] [...]
                             (Monad testnet buyer, seller and auditor on one machine; see quickstart.py)

`pins` downloads, over HTTPS only, the package release's `release-manifest.json`, the owner authorization and
deployment record at that release's source commit, and every authorized pool's hosting manifest and signature,
and checks the manifests against the authorization. Its output directory is the `--pins` input of `init`.
Two offline forms never download anything. `--rehearsal-release` qualifies an unpublished candidate's onboarding
against an existing published authorization; everything made from it is labelled as a rehearsal and is never
publication evidence. `--prepublication-release` is for native qualification of a clean candidate whose version
the committed owner authorization already names.

`init` checks the installed commands against the release manifest's hashes for this platform and pins the
publisher-signed hosting manifest of the chosen pool (from a `--pins` directory holding
`release-manifest.json`, `manifest-<pool>.json`, its `.signature.json` and `testnet-release.json`). It then
asks the installed `erebus-negotiate` to create the suite-2 operator keys and identity binding, and writes
owner-only `wallet.json` (and `payment.json` for a buyer) with the pool's five runtime pins. Key files are
created by Rust; this script only passes paths. The gas key is the operator's own: give its address and,
optionally, its key file path (omit the file to sign externally). A `share/` directory receives the public
descriptor and binding for the counterparty, plus the seller's offer after `connect`.

`connect` runs once the counterparty's `share/` directory has arrived. It asks `erebus-negotiate` to prepare
the terms template and writes `negotiate.json`, the seller's `access.json`, and `mcp.json` (environment for
`erebus-mcp-server` with `EREBUS_BACKEND=metropolis`).

`doctor` is read-only and prints no secrets. It reports whether the installed commands still match the hashes
recorded at `init`, whether every configured key file, config and state directory is owner-only (key files
are never opened), whether the pinned manifest is unchanged, how much of it is cached, whether the wallet
opens (note count only), and, unless `--offline`, whether both RPC endpoints pass the installed network check
on chain 10143 and the gas signer's native balance on each.

`auditor` creates an auditor key with the installed `erebus-shielded-disclosure` and renders the disclosure
deployment config (see `disclosure_config.py`). By default (`--history full`) it makes no RPC call and
verification replays from the pool deployment, so a deal that settled before the auditor was set up (the usual
case: the grant arrives after settlement) verifies. `--history checkpoint` is the faster opt-in for an auditor
set up before the deals it will audit: it asks the same command to record a paired finalized checkpoint in the
empty cache, so verification scans only later blocks and the reply reports `history_from_block`; a deal settled
at or before the checkpoint verifies as `settled_before_checkpoint`, never as paid or unpaid. Either way
`disclosure.json` is the same.

Amounts are base units of the pool's asset. Everything is test-only, single-operator and unaudited, like the
deployment it names.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import time
import urllib.parse
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
NAMESPACE_CHAIN = 10143
COMMANDS = ("erebus-negotiate", "erebus-payment", "erebus-wallet", "erebus-access", "erebus-access-service",
            "erebus-shielded-disclosure", "erebus-artifacts", "erebus-local-prove", "erebus-mcp-server")
ADDRESS = re.compile(r"0x[0-9a-f]{40}", re.ASCII)
HEX64 = re.compile(r"[0-9a-f]{64}", re.ASCII)


def user_agent() -> str:
    """Explicit User-Agent for every operator HTTPS/JSON-RPC request; some providers reject library defaults."""
    try:
        from importlib.metadata import version
        return f"erebus-init/{version('erebus-mcp-server')}"
    except Exception:  # running from a checkout without the installed distribution
        return "erebus-init/source"


def request(url: str, body: bytes | None = None, headers: dict | None = None):
    import urllib.request
    return urllib.request.Request(url, body, {"user-agent": user_agent(), **(headers or {})})
UNSIGNED = re.compile(r"[0-9]{1,78}", re.ASCII)
GAS = {"gas_limit": 3000000, "max_fee_per_gas": "300000000000", "max_priority_fee_per_gas": "2000000000"}


def load(name: str):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def private_json(path: Path, data: dict) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        stream.write(json.dumps(data, indent=2) + "\n")


def wheel_tag() -> str:
    system, machine = platform.system(), platform.machine()
    if (system, machine) == ("Darwin", "arm64"):
        return "macosx_11_0_arm64"
    if (system, machine) == ("Linux", "x86_64"):
        return "linux_x86_64"
    raise ValueError("published releases support macOS arm64 and Linux x86_64 only")


REHEARSAL_MARKER = "rehearsal.json"
REHEARSAL_LABELS = ["rehearsal-pins", "not-publication-evidence"]


def authorized_pool_files(pins: Path, authorization: dict) -> dict:
    """Every authorized pool's hosting manifest and signature envelope in a pins directory, checked by hash."""
    manifests = authorization.get("hosting_manifests") or {}
    if not manifests or not ADDRESS.fullmatch(str(authorization.get("publisher", ""))):
        raise ValueError("the release authorization must pin a publisher and every hosting manifest")
    files = {}
    for pool, pin in sorted(manifests.items()):
        manifest, signature = pins / f"manifest-{pool}.json", pins / f"manifest-{pool}.signature.json"
        if manifest.is_symlink() or signature.is_symlink() or sha256(manifest) != pin:
            raise ValueError(f"{pool}: the hosting manifest does not match the release authorization")
        envelope = json.loads(signature.read_text())
        if not isinstance(envelope, dict) or str(envelope.get("manifest_sha256", "")).removeprefix("0x") != pin:
            raise ValueError(f"{pool}: the signature envelope is not for its pinned hosting manifest")
        files[manifest.name], files[signature.name] = manifest.read_bytes(), signature.read_bytes()
    return files


def rehearsal_release(pins: Path, authorization: dict) -> tuple[dict, dict]:
    """A rehearsal pins directory's candidate release, checked against its marker and the authorization.

    The candidate's `release.json` stays byte-exact; it is wrapped here in the published manifest's shape so the
    same installed-binary check applies. The publisher, signatures and hosting manifests are the authorization's."""
    marker = json.loads((pins / REHEARSAL_MARKER).read_text())
    candidate_bytes = (pins / "candidate-release.json").read_bytes()
    candidate = json.loads(candidate_bytes)
    if (marker.get("rehearsal") is not True or marker.get("publication_evidence") is not False
            or hashlib.sha256(candidate_bytes).hexdigest() != marker.get("candidate_release_sha256")
            or candidate.get("version") != marker.get("candidate_version")
            or authorization.get("package_version") != marker.get("authorization_package_version")):
        raise ValueError("the rehearsal pins do not match their marker; create them again with `pins --rehearsal-release`")
    domains = {str(domain.get("manifest_sha256")) for domain in candidate.get("proving_domains") or []}
    if domains != set((authorization.get("hosting_manifests") or {}).values()):
        raise ValueError("the candidate release does not record exactly the authorized hosting manifests")
    release = {"version": candidate["version"], "source_commit": candidate.get("source_commit"),
               "platforms": {candidate.get("platform", "candidate"): candidate}}
    return release, marker


def release_pins(pins: Path, pool: str) -> dict:
    """The authorized hosting manifest, signature and publisher for one pool, checked against each other."""
    authorization = json.loads((pins / "testnet-release.json").read_text())
    rehearsal = (pins / REHEARSAL_MARKER).exists()
    if rehearsal and (pins / "release-manifest.json").exists():
        raise ValueError("a pins directory is either published or rehearsal, never both")
    manifest = pins / f"manifest-{pool}.json"
    signature = pins / f"manifest-{pool}.signature.json"
    expected = (authorization.get("hosting_manifests") or {}).get(pool)
    if expected is None or sha256(manifest) != expected:
        raise ValueError("the pool's hosting manifest does not match the release authorization")
    publisher = authorization.get("publisher", "")
    if not ADDRESS.fullmatch(publisher):
        raise ValueError("the release authorization does not pin a publisher")
    envelope = json.loads(signature.read_text())
    if not isinstance(envelope, dict) or str(envelope.get("manifest_sha256", "")).removeprefix("0x") != expected:
        # The installed erebus-artifacts verifies the signature itself before accepting any artifact byte; this only
        # catches a pins directory whose signature belongs to another manifest.
        raise ValueError("the pool's signature envelope is not for its pinned hosting manifest")
    marker = None
    if rehearsal:
        release, marker = rehearsal_release(pins, authorization)
    else:
        release = json.loads((pins / "release-manifest.json").read_text())
        if release.get("version") != authorization.get("package_version"):
            raise ValueError("the release manifest is not the authorized package version")
    return {"manifest": manifest, "manifest_sha256": expected, "signature": json.loads(signature.read_text()),
            "publisher": publisher, "release": release, "test_only": json.loads(manifest.read_text()).get("test_only"),
            "rehearsal": marker}


def verify_installed(bins: Path, release: dict) -> dict:
    """Every installed command must be the release's byte-for-byte; returns the checked hashes."""
    tag = wheel_tag()
    platforms = [entry for entry in release["platforms"].values() if entry["platform_qualification"]["wheel_tag"] == tag]
    if len(platforms) != 1:
        raise ValueError("the release has no qualified build for this platform")
    expected = {**platforms[0]["binaries"], **platforms[0].get("launchers", {})}
    checked = {}
    for name, record in expected.items():
        path = bins / name
        if path.is_symlink() or not path.is_file() or sha256(path) != record["sha256"]:
            raise ValueError(f"installed {name} is not the published binary")
        checked[name] = record["sha256"]
    return checked


NATIVE_STATUSES = ("ok", "error", "pending")


def command(bins: Path, name: str, request: dict, cwd: Path) -> dict:
    result = subprocess.run([str(bins / name)], input=json.dumps(request), capture_output=True, text=True,
                            cwd=cwd, timeout=120, check=False,
                            env={"PATH": f"{bins}{os.pathsep}/usr/bin:/bin", "HOME": os.environ.get("HOME", str(cwd))})
    try:
        reply = json.loads(result.stdout)
    except ValueError:
        reply = {}
    if result.returncode != 0 or reply.get("status") != "ok":
        # Fixed text only: the native reply's own fields are never echoed.
        status = reply.get("status") if reply.get("status") in NATIVE_STATUSES else "unreadable"
        raise ValueError(f"{name} {request.get('method')} refused (exit {result.returncode}, status {status}); "
                         f"run the installed {name} directly to see its error")
    return reply


def pool_entry(deployment: Path, name: str) -> tuple[dict, dict]:
    record = json.loads(deployment.read_text())
    matches = [entry for entry in record.get("pools", []) if entry.get("name") == name]
    if len(matches) != 1 or record.get("chain_id") != NAMESPACE_CHAIN:
        raise ValueError("the deployment record must list this pool on chain 10143")
    return record, matches[0]


def require_unsigned(value: str, label: str) -> str:
    if not UNSIGNED.fullmatch(value):
        raise ValueError(f"{label} must be a base-unit integer")
    return value


REGISTRY = "https://github.com/PoulavBhowmick03/erebus-metropolis/releases/download"
SOURCE = "https://raw.githubusercontent.com/PoulavBhowmick03/Erebus"


def https(url: str, opener=None) -> bytes:
    import urllib.request
    if not url.startswith("https://"):
        raise ValueError("pins are downloaded over https only")
    with (opener or urllib.request.urlopen)(request(url), timeout=120) as response:
        data = response.read((8 << 20) + 1)
    if len(data) > 8 << 20:
        raise ValueError(f"unexpectedly large download: {url}")
    return data


def rehearsal_pins(args) -> dict:
    """Offline pins for qualifying an unpublished candidate's installed onboarding. Never publication evidence.

    Requires the candidate's exact `release.json` (by SHA-256) and an existing published pins directory; the
    authorization, publisher, signed hosting manifests and deployment record are copied from it unchanged, never
    substituted. Every file written and every operator initialized from it is labelled as a rehearsal."""
    directory = args.directory.resolve()
    if directory.exists():
        raise ValueError("use a new pins directory")
    if args.rehearsal_release_sha256 is None or not HEX64.fullmatch(args.rehearsal_release_sha256):
        raise ValueError("--rehearsal-release needs --rehearsal-release-sha256 (the candidate release.json digest)")
    if args.authorized_pins is None:
        raise ValueError("--rehearsal-release needs --authorized-pins: a directory from `pins --version` of a published release")
    source = args.authorized_pins.resolve()
    candidate_bytes = args.rehearsal_release.read_bytes()
    if hashlib.sha256(candidate_bytes).hexdigest() != args.rehearsal_release_sha256:
        raise ValueError("the candidate release.json does not have the given SHA-256")
    candidate = json.loads(candidate_bytes)
    authorization_bytes = (source / "testnet-release.json").read_bytes()
    authorization = json.loads(authorization_bytes)
    if (source / REHEARSAL_MARKER).exists():
        raise ValueError("--authorized-pins must come from a published release, not a rehearsal")
    published = json.loads((source / "release-manifest.json").read_text())
    if published.get("version") != authorization.get("package_version"):
        raise ValueError("the authorized pins' release manifest is not its authorization's package version")
    files = authorized_pool_files(source, authorization)
    if candidate.get("published") is not False or not isinstance(candidate.get("version"), str):
        raise ValueError("the candidate must be an unpublished registry release.json")
    if {str(domain.get("manifest_sha256")) for domain in candidate.get("proving_domains") or []} != set(
            (authorization.get("hosting_manifests") or {}).values()):
        raise ValueError("the candidate release does not record exactly the authorized hosting manifests")
    marker = {"version": 1, "rehearsal": True, "publication_evidence": False,
              "candidate_version": candidate["version"], "candidate_release_sha256": args.rehearsal_release_sha256,
              "candidate_source_tree_sha256": (candidate.get("candidate") or {}).get("source_tree_sha256"),
              "authorization_package_version": authorization.get("package_version"),
              "authorization_sha256": hashlib.sha256(authorization_bytes).hexdigest(),
              "publisher": authorization.get("publisher"),
              "labels": ["test-only", "single-operator", "unaudited", *REHEARSAL_LABELS]}
    files.update({"testnet-release.json": authorization_bytes, "candidate-release.json": candidate_bytes,
                  "monad-testnet-shielded-v2.json": (source / "monad-testnet-shielded-v2.json").read_bytes(),
                  REHEARSAL_MARKER: json.dumps(marker, indent=2).encode()})
    directory.mkdir(mode=0o700)
    try:
        for name, data in files.items():
            (directory / name).write_bytes(data)
        # Re-read through the same check init uses, so a mismatched candidate fails here and not at init.
        for pool in sorted(authorization.get("hosting_manifests") or {}):
            release_pins(directory, pool)
    except (OSError, ValueError, KeyError, TypeError):
        shutil.rmtree(directory)
        raise
    return {"status": "pinned", "rehearsal": True, "publication_evidence": False, "directory": str(directory),
            "candidate_version": candidate["version"], "authorization_package_version": authorization.get("package_version"),
            "publisher": authorization.get("publisher"),
            "files": {name: hashlib.sha256(data).hexdigest() for name, data in sorted(files.items())},
            "note": "rehearsal pins qualify an unpublished candidate's onboarding only; they are not publication evidence"}


def prepublication_pins(args) -> dict:
    """Offline pins for the native qualification of a version the owner has already authorized, before upload.

    The authorization file must name exactly the candidate's version (it is the one committed at the release commit),
    the candidate must come from a clean, release-eligible tree, and it must record exactly the authorized hosting
    manifests. Signed manifests are taken from --authorized-pins and checked against this authorization by hash.
    These are ordinary pins: `init` checks installed binaries against the candidate exactly as against a published
    release. Publication still requires its own authorization, native runs and every other gate."""
    directory = args.directory.resolve()
    if directory.exists():
        raise ValueError("use a new pins directory")
    if args.prepublication_release_sha256 is None or not HEX64.fullmatch(args.prepublication_release_sha256):
        raise ValueError("--prepublication-release needs --prepublication-release-sha256")
    if args.authorization is None or args.authorized_pins is None:
        raise ValueError("--prepublication-release needs --authorization and --authorized-pins")
    candidate_bytes = args.prepublication_release.read_bytes()
    if hashlib.sha256(candidate_bytes).hexdigest() != args.prepublication_release_sha256:
        raise ValueError("the candidate release.json does not have the given SHA-256")
    candidate = json.loads(candidate_bytes)
    authorization_bytes = args.authorization.read_bytes()
    authorization = json.loads(authorization_bytes)
    if authorization.get("package_version") != candidate.get("version"):
        raise ValueError("the authorization does not name this candidate's version; use --rehearsal-release instead")
    if candidate.get("published") is not False or (candidate.get("candidate") or {}).get("release_eligible") is not True:
        raise ValueError("prepublication pins need an unpublished candidate from a clean, release-eligible tree")
    if {str(domain.get("manifest_sha256")) for domain in candidate.get("proving_domains") or []} != set(
            (authorization.get("hosting_manifests") or {}).values()):
        raise ValueError("the candidate release does not record exactly the authorized hosting manifests")
    files = authorized_pool_files(args.authorized_pins.resolve(), authorization)
    release = {"version": candidate["version"], "source_commit": candidate.get("source_commit"),
               "prepublication": True, "platforms": {candidate.get("platform", "candidate"): candidate}}
    files.update({"testnet-release.json": authorization_bytes, "candidate-release.json": candidate_bytes,
                  "release-manifest.json": json.dumps(release, indent=2).encode(),
                  "monad-testnet-shielded-v2.json": (args.authorized_pins.resolve() / "monad-testnet-shielded-v2.json").read_bytes()})
    directory.mkdir(mode=0o700)
    try:
        for name, data in files.items():
            (directory / name).write_bytes(data)
        for pool in sorted(authorization.get("hosting_manifests") or {}):
            release_pins(directory, pool)
    except (OSError, ValueError, KeyError, TypeError):
        shutil.rmtree(directory)
        raise
    return {"status": "pinned", "prepublication": True, "rehearsal": False, "directory": str(directory),
            "version": candidate["version"], "publisher": authorization.get("publisher"),
            "files": {name: hashlib.sha256(data).hexdigest() for name, data in sorted(files.items())},
            "note": "prepublication pins for the authorized version; publication still needs every gate"}


def pins(args, opener=None) -> dict:
    if getattr(args, "prepublication_release", None) is not None:
        if args.version is not None or getattr(args, "rehearsal_release", None) is not None:
            raise ValueError("--prepublication-release excludes --version and --rehearsal-release")
        return prepublication_pins(args)
    if getattr(args, "rehearsal_release", None) is not None:
        if args.version is not None:
            raise ValueError("--version and --rehearsal-release are exclusive")
        return rehearsal_pins(args)
    if args.version is None:
        raise ValueError("pins needs --version (published) or --rehearsal-release (offline rehearsal)")
    if any(getattr(args, name, None) for name in ("rehearsal_release_sha256", "authorized_pins", "authorization",
                                                  "prepublication_release_sha256")):
        raise ValueError("offline pin options apply only with --rehearsal-release or --prepublication-release")
    directory = args.directory.resolve()
    if directory.exists():
        raise ValueError("use a new pins directory")
    release_bytes = https(f"{REGISTRY}/v{args.version}/release-manifest.json", opener)
    release = json.loads(release_bytes)
    commit = str(release.get("source_commit", ""))
    if release.get("version") != args.version or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("the release manifest does not name this version and a source commit")
    authorization_bytes = https(f"{SOURCE}/{commit}/packaging/metropolis/testnet-release.json", opener)
    authorization = json.loads(authorization_bytes)
    deployment_bytes = https(f"{SOURCE}/{commit}/contracts/evm/deployments/monad-testnet-shielded-v2.json", opener)
    if authorization.get("package_version") != args.version:
        raise ValueError("the authorization at the release commit names a different version")
    files = {"release-manifest.json": release_bytes, "testnet-release.json": authorization_bytes,
             "monad-testnet-shielded-v2.json": deployment_bytes}
    for pool, pin in sorted((authorization.get("hosting_manifests") or {}).items()):
        manifest = https(f"{REGISTRY}/{authorization['artifact_tag']}/manifest-{pool}.json", opener)
        if hashlib.sha256(manifest).hexdigest() != pin:
            raise ValueError(f"{pool}: hosted manifest differs from the authorization")
        files[f"manifest-{pool}.json"] = manifest
        files[f"manifest-{pool}.signature.json"] = https(f"{REGISTRY}/{authorization['artifact_tag']}/manifest-{pool}.signature.json", opener)
    directory.mkdir(mode=0o700)
    for name, data in files.items():
        (directory / name).write_bytes(data)
    return {"status": "pinned", "directory": str(directory), "source_commit": commit, "publisher": authorization.get("publisher"),
            "files": {name: hashlib.sha256(data).hexdigest() for name, data in sorted(files.items())},
            "note": "publisher and digests come from the release's own commit; confirm them through a channel you trust"}


PAYMENT_RELAY_CAPABILITY = "sponsored_relay"


def sponsored_relay(args, bins: Path, cwd: Path) -> dict | None:
    """Validate an explicitly requested sponsored relay, before anything is written.

    Direct buyers get no `relay` field at all, which keeps configs valid for published payment binaries. A sponsored
    buyer pins the relay URL, identity, sponsor and authentication key file, never holds a transaction key, and is
    refused unless the installed erebus-payment advertises `sponsored_relay: true` in its version reply."""
    values = [args.relay_url, args.relay_identity, args.relay_sponsor, args.relay_auth_key_file]
    if not any(values):
        return None
    if not all(values):
        raise ValueError("sponsored setup needs --relay-url, --relay-identity, --relay-sponsor and --relay-auth-key-file")
    if args.role != "buyer":
        raise ValueError("only a buyer submits through a sponsored relay")
    if args.transaction_key_file is not None:
        raise ValueError("a sponsored buyer must not hold a transaction key; omit --transaction-key-file")
    url = urllib.parse.urlsplit(args.relay_url)
    if url.scheme != "https" or url.path != "/v1/relay" or url.username or url.password or url.query or url.fragment or not url.hostname:
        raise ValueError("--relay-url must be https://<host>/v1/relay without credentials, query or fragment")
    identity = args.relay_identity.lower().removeprefix("0x")
    if not HEX64.fullmatch(identity) or identity == "0" * 64:
        raise ValueError("--relay-identity must be a nonzero 32-byte hex identity")
    sponsor = args.relay_sponsor.lower()
    if not ADDRESS.fullmatch(sponsor) or sponsor == "0x" + "0" * 40:
        raise ValueError("--relay-sponsor must be the relay's canonical nonzero gas address")
    key = args.relay_auth_key_file
    if not key.is_absolute() or not key.is_file():
        raise ValueError("--relay-auth-key-file must be an existing absolute owner-only file (it is not read here)")
    if stat.S_IMODE(key.stat().st_mode) & 0o077:
        raise ValueError("--relay-auth-key-file must be owner-only")
    if not 1 <= args.relay_timeout_seconds <= 900:
        raise ValueError("--relay-timeout-seconds must be between 1 and 900")
    reply = command(bins, "erebus-payment", {"method": "version"}, cwd)
    if reply.get(PAYMENT_RELAY_CAPABILITY) is not True:
        raise ValueError(f"the installed erebus-payment does not advertise `{PAYMENT_RELAY_CAPABILITY}: true` in its version "
                         "reply; sponsored setup refused and nothing was written. Direct submission is not used instead.")
    return {"url": args.relay_url, "identity": identity, "authentication_key_file": str(key),
            "sponsor_address": sponsor, "timeout_seconds": args.relay_timeout_seconds}


def init(args) -> dict:
    directory = args.directory.resolve()
    if directory.exists():
        raise ValueError("use a new operator directory")
    signer = args.signer_address.lower()
    if not ADDRESS.fullmatch(signer) or signer == "0x" + "0" * 40:
        raise ValueError("--signer-address must be the operator's own gas address")
    if args.transaction_key_file is not None and not args.transaction_key_file.is_file():
        raise ValueError("--transaction-key-file must name an existing owner-only key file")
    if args.role == "buyer":
        require_unsigned(args.maximum_price or "", "--maximum-price")
    disclosure = load("disclosure_config")
    disclosure.endpoint(args.rpc_url)
    disclosure.endpoint(args.peer_rpc_url)
    if args.rpc_url == args.peer_rpc_url:
        raise ValueError("the primary and peer RPC endpoints must differ")
    bins = args.bin.resolve()
    record, pool = pool_entry(args.deployment, args.pool)
    pins = release_pins(args.pins.resolve(), args.pool)
    checked = verify_installed(bins, pins["release"])
    sponsored = sponsored_relay(args, bins, directory.parent)
    manifest = json.loads(pins["manifest"].read_text())
    if str(manifest.get("settlement_contract")).lower() != pool["pool"].lower() or manifest.get("chain_id") != NAMESPACE_CHAIN:
        raise ValueError("the hosting manifest names a different pool")
    staging = load("stage_artifacts")
    identity = staging.deployment_identity(args.deployment, args.pool)
    for circuit in manifest["circuits"]:
        if circuit["identity"]["verifier_runtime_keccak256"] != identity["runtime_pins"][circuit["kind"]]:
            raise ValueError("the hosting manifest's verifiers differ from the deployment pins")

    directory.mkdir(mode=0o700, parents=False)
    (directory / "share").mkdir(mode=0o700)
    state = directory / "state"
    state.mkdir(mode=0o700)
    (state / "artifacts").mkdir(mode=0o700)
    artifacts = directory / "release"
    artifacts.mkdir(mode=0o700)
    shutil.copyfile(pins["manifest"], artifacts / "manifest.json")
    # erebus-payment reads the manifest as a local input and refuses one others can read.
    (artifacts / "manifest.json").chmod(0o600)
    namespace = f"eip155:{NAMESPACE_CHAIN}"
    asset = pool["asset"].lower()
    operator = command(bins, "erebus-negotiate", {
        "method": "prepare_operator", "suite_id": 2, "directory": str(directory / "keys"), "role": args.role,
        "endpoint": args.endpoint, "namespace": namespace, "assets": [f"{namespace}/erc20:{asset}"],
        "descriptor_lifetime_seconds": args.descriptor_lifetime_seconds}, directory)
    binding = directory / "keys" / "identity.binding"
    command(bins, "erebus-negotiate", {
        "method": "prepare_binding", "output": str(binding), "descriptor_file": operator["descriptor_file"],
        "discovery_key_file": operator["discovery_key_file"], "agreement_key_file": operator["agreement_key_file"],
        "settlement_contract": pool["pool"].lower(), "verifier_version": pool["verifier_version"],
        "expires_at": int(time.time()) + args.binding_lifetime_seconds}, directory)
    for source in (Path(operator["descriptor_file"]), binding):
        shutil.copyfile(source, directory / "share" / source.name)
        (directory / "share" / source.name).chmod(0o600)
    authorization = {"publisher": pins["publisher"], "signature": pins["signature"]}
    transaction_key = str(args.transaction_key_file.resolve()) if args.transaction_key_file else None
    wallet = {
        "version": 1, "state_root": str(state), "namespace": namespace, "pool": pool["pool"].lower(),
        "verifier_version": pool["verifier_version"], "asset": asset,
        "wrapper": asset if args.pool == "wmon" else None, "rpc_url": args.rpc_url, "peer_rpc_url": args.peer_rpc_url,
        "first_block": pool["deployment_block"], "first_hash": pool["deployment_hash"].lower(),
        "wallet_file": str(state / "wallet" / "notes.enc"), "wallet_key_file": operator["wallet_key_file"],
        "agreement_key_file": operator["wallet_agreement_key_file"], "signer_address": signer,
        "signer_journal_root": str(state / "signer-journal"), "transaction_key_file": transaction_key,
        "artifact_manifest_file": str(artifacts / "manifest.json"), "artifact_manifest_sha256": pins["manifest_sha256"],
        "artifact_manifest_authorization": authorization, "artifact_cache_root": str(state / "artifacts"),
        "runtime_pins": {name: value for name, value in identity["runtime_pins"].items()},
        "allow_test_artifacts": pins["test_only"] is True, "allow_loopback_artifact_http": False,
        **GAS, "timeout_seconds": 30, "max_scan_blocks": 1000, "artifact_timeout_seconds": 300,
    }
    private_json(directory / "wallet.json", wallet)
    written = ["wallet.json"]
    if args.role == "buyer":
        public = Path(operator["agreement_key_file"]).with_name("agreement.public")
        if not public.is_file():
            raise ValueError("erebus-negotiate did not write the public agreement key")
        pins_hex = {field: "0x" + identity["runtime_pins"][pin] for field, pin in load("disclosure_config").PINS.items()}
        private_json(directory / "payment.json", {
            "version": 1, "mode": "shielded", "state_root": str(state), "namespace": namespace,
            "pool": pool["pool"].lower(), "verifier_version": pool["verifier_version"], **pins_hex,
            "first_block": pool["deployment_block"], "first_hash": pool["deployment_hash"].lower(),
            "rpc_url": args.rpc_url, "peer_rpc_url": args.peer_rpc_url, "buyer_key_hex": public.read_bytes().hex(),
            "asset": f"{namespace}/erc20:{asset}", "signer_address": signer,
            "signer_journal_root": wallet["signer_journal_root"], "transaction_key_file": transaction_key,
            "wallet_file": wallet["wallet_file"], "wallet_key_file": wallet["wallet_key_file"],
            "manifest_file": wallet["artifact_manifest_file"], "manifest_sha256": pins["manifest_sha256"],
            "manifest_authorization": authorization, "artifact_cache": wallet["artifact_cache_root"],
            "allow_test_artifacts": wallet["allow_test_artifacts"], "allow_loopback_http": False,
            "maximum_price": args.maximum_price, **({"relay": sponsored} if sponsored else {}), **GAS,
            "timeout_seconds": 30, "max_scan_blocks": 1000})
        written.append("payment.json")
    summary = {"version": 1, "role": args.role, "pool": args.pool, "settlement_contract": pool["pool"].lower(),
               "asset": asset, "chain_id": NAMESPACE_CHAIN, "endpoint": args.endpoint, "signer_address": signer,
               "bin": str(bins), "release_version": pins["release"]["version"],
               "release_source_commit": pins["release"]["source_commit"], "binaries_sha256": checked,
               "hosting_manifest_sha256": pins["manifest_sha256"], "publisher": pins["publisher"],
               "deployment_sha256": sha256(args.deployment), "operator": {k: v for k, v in operator.items() if k.endswith("_file")},
               "labels": ["test-only", "single-operator", "unaudited"], "transactions_sent": 0}
    if pins["release"].get("prepublication") is True:
        summary["prepublication"] = True
    if pins["rehearsal"] is not None:
        summary["rehearsal"] = rehearsal_record(pins["rehearsal"])
        summary["labels"] += REHEARSAL_LABELS
    private_json(directory / "operator.json", summary)
    return {"status": "initialized", "directory": str(directory), "written": written + ["operator.json", "share/"],
            **({"rehearsal": True, "publication_evidence": False} if pins["rehearsal"] is not None else {}),
            "next": "fund the signer address with testnet MON, send share/ to the counterparty, then run connect"}


def rehearsal_record(marker: dict) -> dict:
    return {"publication_evidence": False, "candidate_version": marker["candidate_version"],
            "candidate_release_sha256": marker["candidate_release_sha256"],
            "authorization_package_version": marker["authorization_package_version"]}


def connect(args) -> dict:
    directory = args.directory.resolve()
    operator = json.loads((directory / "operator.json").read_text())
    bins = Path(operator["bin"])
    verify_installed(bins, release_pins_from(operator))
    peer = args.peer_share.resolve()
    descriptors = sorted(peer.glob("*.descriptor.json"))
    if len(descriptors) != 1 or not (peer / "identity.binding").is_file():
        raise ValueError("the peer share must hold exactly one descriptor and identity.binding")
    role, other = operator["role"], ("seller" if operator["role"] == "buyer" else "buyer")
    args.role_checked = role
    if role == "seller" and (args.service_url is not None or args.allow_loopback_access_http):
        raise ValueError("--service-url and --allow-loopback-access-http apply to a buyer")
    require_unsigned(args.amount, "--amount")
    namespace = f"eip155:{NAMESPACE_CHAIN}"
    offer_file = peer / "offer.json"
    if role == "seller":
        if args.payload_file is None or args.port is None:
            raise ValueError("a seller connects with --payload-file and --port")
        digest = sha256(args.payload_file)
        deadline = int(time.time()) + args.delivery_seconds
        offer = {"resource": args.resource, "unit": args.unit, "quantity": args.quantity, "price": args.amount,
                 "fulfillment_method": "http-access-v1", "fulfillment_digest": digest, "delivery_deadline": deadline}
    else:
        if not offer_file.is_file():
            raise ValueError("a buyer connects after the seller's share includes offer.json")
        offer = json.loads(offer_file.read_text())
        if offer.get("resource") != args.resource:
            raise ValueError("the seller's offer names a different resource")
        digest, deadline = offer["fulfillment_digest"], offer["delivery_deadline"]
    files = operator["operator"]
    # Validated before anything is written.
    access = access_environment(args, directory, bins, peer, files) if args.service_url is not None else None
    # The buyer dials the seller: erebus-negotiate requires the buyer's endpoint to be one the seller's signed
    # descriptor advertises, and the seller's to be its own.
    endpoint = peer_endpoint(descriptors[0]) if role == "buyer" else operator["endpoint"]
    terms = directory / "keys" / "service.terms"
    command(bins, "erebus-negotiate", {
        "method": "prepare_terms", "suite_id": 2, "output": str(terms), "role": role,
        "local_descriptor_file": files["descriptor_file"], "peer_descriptor_file": str(descriptors[0]),
        "local_binding_file": str(directory / "keys" / "identity.binding"), "peer_binding_file": str(peer / "identity.binding"),
        "settlement_contract": operator["settlement_contract"], "verifier_version": 2,
        "asset": f"{namespace}/erc20:{operator['asset']}", "amount": args.amount, "resource": args.resource,
        "unit": offer["unit"], "quantity": offer["quantity"], "fulfillment_method": offer["fulfillment_method"],
        "fulfillment_digest": digest, "delivery_deadline": deadline}, directory)
    maximum = json.loads((directory / "payment.json").read_text())["maximum_price"] if role == "buyer" else args.amount
    negotiate = {
        "version": 1, "role": role, "state_root": str(directory / "state"),
        "transport_key_file": files["transport_key_file"], "agreement_key_file": files["agreement_key_file"],
        "discovery_key_file": files["discovery_key_file"], "local_descriptor_file": files["descriptor_file"],
        "peer_descriptor_file": str(descriptors[0]), "terms_template_file": str(terms), "endpoint": endpoint,
        "maximum_price": maximum, "minimum_price": require_unsigned(args.minimum_price or args.amount, "--minimum-price"),
        "max_deal_lifetime_seconds": args.delivery_seconds, "timeout_seconds": 30}
    written = ["negotiate.json", "mcp.json"]
    environment = {"EREBUS_BACKEND": "metropolis", "EREBUS_NEGOTIATION_CONFIG": str(directory / "negotiate.json"),
                   "EREBUS_NEGOTIATION_CLI": str(bins / "erebus-negotiate")}
    if role == "seller":
        wallet = json.loads((directory / "wallet.json").read_text())
        evidence = directory / "access-evidence"
        evidence.mkdir(mode=0o700)
        negotiate.update({"seller_spend_secret_file": files["seller_spend_secret_file"],
                          "seller_wallet_file": wallet["wallet_file"], "seller_wallet_key_file": wallet["wallet_key_file"],
                          "access_evidence_root": str(evidence)})
        public = Path(files["agreement_key_file"]).with_name("agreement.public")
        pins = wallet["runtime_pins"]
        payload = directory / "payload"
        shutil.copyfile(args.payload_file, payload)
        payload.chmod(0o600)
        service_id = os.urandom(32)
        private_json(directory / "access.json", {
            "service_id": list(service_id), "seller_key": list(public.read_bytes()), "suite_id": 2,
            "resource": args.resource, "payload_file": str(payload), "evidence_root": str(evidence),
            "state_root": str(directory / "access-state"), "port": args.port,
            "backend": {"mode": "shielded", "chain_id": NAMESPACE_CHAIN, "pool": operator["settlement_contract"],
                        "rpc_url": wallet["rpc_url"], "peer_rpc_url": wallet["peer_rpc_url"],
                        "first_block": wallet["first_block"], "first_hash": list(bytes.fromhex(wallet["first_hash"][2:])),
                        "runtime_keccak256": pins["pool"], "poseidon_keccak256": pins["poseidon"],
                        "deposit_verifier_keccak256": pins["deposit"], "transfer_verifier_keccak256": pins["transfer"],
                        "withdraw_verifier_keccak256": pins["withdraw"], "max_concurrent_queries": 4,
                        "requests_per_second": 20}})
        written.append("access.json")
        offer_out = directory / "share" / "offer.json"
        private_json(offer_out, offer)
        # Public: the buyer pins this service identity; the URL is the buyer's own choice (`--service-url`).
        private_json(directory / "share" / "service.json", {"version": 1, "service_id": service_id.hex(),
                                                             "resource": args.resource, "port": args.port})
    else:
        environment.update({"EREBUS_PAYMENT_CONFIG": str(directory / "payment.json"),
                            "EREBUS_PAYMENT_CLI": str(bins / "erebus-payment")})
        if access is not None:
            (directory / "access-cache").mkdir(mode=0o700)
            environment.update(access)
            written.append("access-cache/")
    private_json(directory / "negotiate.json", negotiate)
    private_json(directory / "mcp.json", {"mcpServers": {f"erebus-{role}": {"command": str(bins / "erebus-mcp-server"),
                                                                          "env": environment}}})
    return {"status": "connected", "role": role, "peer": other, "written": written,
            "next": ("start erebus-access-service with access.json, then the MCP server" if role == "seller"
                     else "add mcp.json to the agent, fund the wallet (wrap/approve/deposit with erebus-wallet), then negotiate"
                     if operator["pool"] == "wmon" else
                     "add mcp.json to the agent, send the signer the pool's asset, fund the wallet (approve/deposit with "
                     "erebus-wallet), then negotiate")}


def peer_endpoint(descriptor: Path) -> str:
    """The single TCP endpoint a peer's descriptor advertises, as host:port (the descriptor's signature is
    verified by erebus-negotiate itself)."""
    endpoints = json.loads(descriptor.read_text()).get("endpoints")
    if not isinstance(endpoints, list) or len(endpoints) != 1 or not str(endpoints[0]).startswith("tcp://"):
        raise ValueError("the peer descriptor must advertise exactly one tcp:// endpoint")
    return str(endpoints[0]).removeprefix("tcp://")


LOOPBACK_HOSTS = {"127.0.0.1", "::1", "localhost"}


def access_environment(args, directory: Path, bins: Path, peer: Path, files: dict) -> dict:
    """Buyer retrieval settings for the MCP server: the seller's shared service identity and the buyer's own URL.

    HTTPS only; cleartext HTTP is accepted only for a loopback host and only with --allow-loopback-access-http."""
    if args.role_checked != "buyer":
        raise ValueError("--service-url applies to a buyer")
    service = json.loads((peer / "service.json").read_text())
    if service.get("version") != 1 or not HEX64.fullmatch(str(service.get("service_id", ""))) or service["service_id"] == "0" * 64:
        raise ValueError("the seller's share/service.json is missing or invalid; reconnect the seller first")
    if service.get("resource") != args.resource:
        raise ValueError("the seller's service names a different resource")
    url = urllib.parse.urlsplit(args.service_url)
    if url.path != "/v1/access" or url.username or url.password or url.query or url.fragment or not url.hostname:
        raise ValueError("--service-url must be <scheme>://<host>[:port]/v1/access without credentials, query or fragment")
    loopback = url.hostname in LOOPBACK_HOSTS
    if url.scheme == "http" and not (loopback and args.allow_loopback_access_http):
        raise ValueError("--service-url must use https; plain http is allowed only to loopback with --allow-loopback-access-http")
    if url.scheme not in {"https", "http"}:
        raise ValueError("--service-url must use https")
    if args.allow_loopback_access_http and not (loopback and url.scheme == "http"):
        raise ValueError("--allow-loopback-access-http applies only to an http loopback --service-url")
    cache = directory / "access-cache"
    return {"EREBUS_ACCESS_SERVICE_URL": args.service_url, "EREBUS_ACCESS_SERVICE_ID": service["service_id"],
            "EREBUS_ACCESS_BUYER_KEY_FILE": files["agreement_key_file"], "EREBUS_ACCESS_CACHE": str(cache),
            "EREBUS_ACCESS_CLI": str(bins / "erebus-access"),
            **({"EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP": "1"} if url.scheme == "http" else {})}


def release_pins_from(operator: dict) -> dict:
    """The binary hashes checked at init time; connect refuses commands that changed since."""
    return {"platforms": {"local": {"platform_qualification": {"wheel_tag": wheel_tag()},
                                    "binaries": {name: {"sha256": digest} for name, digest in operator["binaries_sha256"].items()}}}}


def rpc_balance(url: str, address: str, opener=None) -> int:
    import urllib.request
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_getBalance", "params": [address, "finalized"]}).encode()
    with (opener or urllib.request.urlopen)(request(url, body, {"content-type": "application/json"}), timeout=30) as response:
        return int(json.loads(response.read(1 << 16))["result"], 16)


# ERC-20 `balanceOf(address)`.
BALANCE_OF = "0x70a08231"


def rpc_token_balance(url: str, token: str, address: str, opener=None) -> int:
    """A finalized ERC-20 balance read; nothing is signed or sent."""
    import urllib.request
    token, address = token.lower(), address.lower()
    if not ADDRESS.fullmatch(token) or not ADDRESS.fullmatch(address):
        raise ValueError("token and holder must be 0x-prefixed lowercase addresses")
    call = {"to": token, "data": BALANCE_OF + "0" * 24 + address[2:]}
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [call, "finalized"]}).encode()
    with (opener or urllib.request.urlopen)(request(url, body, {"content-type": "application/json"}), timeout=30) as response:
        word = json.loads(response.read(1 << 16))["result"]
    if not isinstance(word, str) or not re.fullmatch(r"0x[0-9a-fA-F]{64}", word):
        raise ValueError("balanceOf did not return one word")
    return int(word, 16)


def network_check(bins: Path, url: str, cwd: Path) -> dict:
    """The installed read-only network check; its reply carries `passed`, not `status`."""
    result = subprocess.run([str(bins / "erebus-network-check")], capture_output=True, text=True, timeout=120, cwd=cwd,
                            input=json.dumps({"chain_id": NAMESPACE_CHAIN, "rpc_url": url, "timeout_ms": 15000}),
                            env={"PATH": f"{bins}{os.pathsep}/usr/bin:/bin", "HOME": str(cwd)})
    return json.loads(result.stdout)


ENDPOINT_STAGING = ".endpoint-change"
ENDPOINT_JOURNAL = f"{ENDPOINT_STAGING}/journal.json"
ENDPOINT_LOCK = ".endpoint-change.lock"
# Generated configs that name RPC endpoints, and where the pair lives inside each.
ENDPOINT_CONFIGS = {"wallet.json": (), "payment.json": (), "access.json": ("backend",), "disclosure.json": ()}


def endpoint_url(url: str) -> str:
    parts = urllib.parse.urlsplit(url)
    loopback = parts.hostname in {"127.0.0.1", "localhost", "::1"}
    if not parts.hostname or parts.username or parts.password or parts.query or parts.fragment \
            or not (parts.scheme == "https" or (parts.scheme == "http" and loopback)):
        raise ValueError("RPC endpoints must be https (http only on loopback) without credentials, query or fragment")
    return url


def file_digest(path: Path) -> str | None:
    """Digest of a regular, non-symlink file; None if it is missing or anything else."""
    try:
        info = path.lstat()
    except OSError:
        return None
    return sha256(path) if stat.S_ISREG(info.st_mode) else None


def endpoint_pair(data: dict, nesting: tuple) -> tuple | None:
    holder = data
    for key in nesting:
        holder = holder.get(key) if isinstance(holder, dict) else None
    if not isinstance(holder, dict) or "rpc_url" not in holder or "peer_rpc_url" not in holder:
        return None
    return holder["rpc_url"], holder["peer_rpc_url"]


def endpoint_state(directory: Path) -> dict:
    """What doctor needs: an incomplete change, and whether every config names the same endpoint pair."""
    pairs = {}
    for name, nesting in ENDPOINT_CONFIGS.items():
        data = read_config(directory / name)
        pair = endpoint_pair(data, nesting) if data is not None else None
        if pair is not None:
            pairs[name] = pair
    staging = directory / ENDPOINT_STAGING
    return {"incomplete": staging.exists() or staging.is_symlink(), "mixed": len(set(pairs.values())) > 1,
            "configs": sorted(pairs)}


def valid_journal(journal: dict | None) -> bool:
    if not isinstance(journal, dict) or journal.get("version") != 1 or not isinstance(journal.get("files"), list) \
            or not journal["files"]:
        return False
    for key in ("rpc_url", "peer_rpc_url", "old_rpc_url", "old_peer_rpc_url"):
        if not isinstance(journal.get(key), str):
            return False
    names = []
    for entry in journal["files"]:
        if not isinstance(entry, dict) or entry.get("name") not in ENDPOINT_CONFIGS \
                or not all(isinstance(entry.get(k), str) and HEX64.fullmatch(entry[k]) for k in ("old_sha256", "new_sha256")):
            return False
        names.append(entry["name"])
    return len(names) == len(set(names))


def endpoints(args) -> dict:
    """Change the primary and peer RPC endpoints of every generated config in one operator or auditor directory.

    This is a journaled, recoverable multi-file update, not a collectively atomic one: each file is replaced
    atomically, but a process reading several configs during the update could see the old pair in one and the new
    pair in another. Readers are not fenced; `doctor` reports an incomplete or mixed configuration until the change
    is finished with --resume or undone with --abort. A file edited by anything else after staging is never
    overwritten: the update stops and both copies stay for the operator. Endpoint invocations on one directory are
    serialized by an exclusive lock. Wallet notes, checkpoints, keys and every other setting are untouched, and
    nothing is signed or sent."""
    import fcntl
    directory = args.directory.resolve()
    info = directory.lstat()
    if not stat.S_ISDIR(info.st_mode) or stat.S_IMODE(info.st_mode) & 0o022:
        raise ValueError("the operator directory must be a real directory that only its owner can write")
    lock_path = directory / ENDPOINT_LOCK
    if lock_path.is_symlink():
        raise ValueError(f"{ENDPOINT_LOCK} must not be a symlink")
    descriptor = os.open(lock_path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError("another endpoint change is running in this directory") from None
        return _endpoints_locked(args, directory)
    finally:
        os.close(descriptor)


def _endpoints_locked(args, directory: Path) -> dict:
    staging = directory / ENDPOINT_STAGING
    journal_path = directory / ENDPOINT_JOURNAL
    operator = read_config(directory / "operator.json")
    bins = Path(operator["bin"]) if operator and "bin" in operator else (args.bin.resolve() if args.bin else None)
    if bins is None:
        raise ValueError("an auditor directory needs --bin (the installed release bin directory)")
    if operator is not None:
        verify_installed(bins, release_pins_from(operator))
    if staging.is_symlink() or (staging.exists() and not staging.is_dir()):
        raise ValueError(f"{ENDPOINT_STAGING} is not a real directory; refusing to touch it")

    def staged_journal() -> dict:
        journal = read_config(journal_path)
        if not valid_journal(journal):
            raise ValueError("the endpoint journal is damaged; no file was changed by this command. Inspect "
                             f"{ENDPOINT_STAGING}/ (the .orig files are the configs from before the change)")
        for entry in journal["files"]:
            if file_digest(staging / f"{entry['name']}.orig") != entry["old_sha256"]:
                raise ValueError(f"the saved original of {entry['name']} is missing or changed")
        return journal

    def replace_checked(entry: dict, source: Path, want: str, have_ok: str) -> bool:
        """Replace one target only if it still holds `have_ok`; True if replaced, False if already `want`."""
        target = directory / entry["name"]
        current = file_digest(target)
        if current == want:
            return False
        if target.is_symlink() or current != have_ok:
            raise ValueError(f"{entry['name']} was changed by something else after staging; nothing more was replaced. "
                             f"Reconcile it with {ENDPOINT_STAGING}/{entry['name']}.orig and .new, then run --abort or --resume")
        os.replace(source, target)
        return True

    def finish(journal: dict, action: str, changed: list[str]) -> dict:
        wallet_opens = None
        if (directory / "wallet.json").exists():
            notes = command(bins, "erebus-wallet", {"method": "notes", "config_file": str(directory / "wallet.json")}, directory)
            wallet_opens = notes.get("status") == "ok"
        shutil.rmtree(staging)
        state = endpoint_state(directory)
        return {"status": action, "directory": str(directory), "rpc_url": journal["rpc_url"],
                "peer_rpc_url": journal["peer_rpc_url"], "configs": [entry["name"] for entry in journal["files"]],
                "applied_now": changed, "wallet_opens": wallet_opens, "mixed_endpoints": state["mixed"],
                "transactions_sent": 0}

    if args.resume or args.abort:
        if not staging.exists():
            raise ValueError("no interrupted endpoint change to resume or abort")
        if not journal_path.exists():
            # The journal is the commit point and is written after all staged copies; without it no config was
            # replaced, so the staged copies are leftovers.
            if args.resume:
                raise ValueError("the change was interrupted before it was committed; run --abort to discard it")
            shutil.rmtree(staging)
            return {"status": "aborted", "directory": str(directory), "applied_now": [], "transactions_sent": 0,
                    "note": "interrupted before commit; no config had been changed"}
        journal = staged_journal()
        if args.abort:
            restored = []
            for entry in journal["files"]:
                temporary = staging / f"{entry['name']}.restore"
                shutil.copyfile(staging / f"{entry['name']}.orig", temporary)
                temporary.chmod(0o600)
                if replace_checked(entry, temporary, want=entry["old_sha256"], have_ok=entry["new_sha256"]):
                    restored.append(entry["name"])
            journal = {**journal, "rpc_url": journal["old_rpc_url"], "peer_rpc_url": journal["old_peer_rpc_url"]}
            return finish(journal, "aborted", restored)
        applied = []
        for entry in journal["files"]:
            staged = staging / f"{entry['name']}.new"
            if file_digest(directory / entry["name"]) != entry["new_sha256"] and file_digest(staged) != entry["new_sha256"]:
                raise ValueError(f"the staged {entry['name']} is missing or damaged; run --abort")
            if replace_checked(entry, staged, want=entry["new_sha256"], have_ok=entry["old_sha256"]):
                applied.append(entry["name"])
        return finish(journal, "resumed", applied)

    if staging.exists():
        raise ValueError("an endpoint change is already staged; run --resume or --abort first")
    if not args.rpc_url or not args.peer_rpc_url:
        raise ValueError("a new endpoint change needs --rpc-url and --peer-rpc-url")
    primary, peer = endpoint_url(args.rpc_url), endpoint_url(args.peer_rpc_url)
    if primary.rstrip("/") == peer.rstrip("/"):
        raise ValueError("the primary and peer RPC endpoints must differ")
    changes, pairs = [], set()
    for name, nesting in ENDPOINT_CONFIGS.items():
        path = directory / name
        if not path.exists() and not path.is_symlink():
            continue
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) & 0o077:
            raise ValueError(f"{name} must be a regular owner-only file (not a symlink); nothing changed")
        data = read_config(path)
        pair = endpoint_pair(data, nesting) if data is not None else None
        if pair is None:
            raise ValueError(f"{name} is damaged or carries no endpoint pair; restore it before changing endpoints")
        pairs.add(pair)
        holder = data
        for key in nesting:
            holder = holder[key]
        holder["rpc_url"], holder["peer_rpc_url"] = primary, peer
        changes.append((name, path, data))
    if not changes:
        raise ValueError("no generated config with RPC endpoints in this directory")
    for label, url in (("--rpc-url", primary), ("--peer-rpc-url", peer)):
        check = network_check(bins, url, directory)
        if check.get("passed") is not True or check.get("chain_id") not in (None, NAMESPACE_CHAIN):
            raise ValueError(f"{label} did not pass the installed network check for chain {NAMESPACE_CHAIN}; nothing changed")
    old_pair = sorted(pairs)[0]
    staging.mkdir(mode=0o700)
    entries = []
    for name, path, data in changes:
        shutil.copyfile(path, staging / f"{name}.orig")
        (staging / f"{name}.orig").chmod(0o600)
        private_json(staging / f"{name}.new", data)
        entries.append({"name": name, "old_sha256": sha256(staging / f"{name}.orig"), "new_sha256": sha256(staging / f"{name}.new")})
    journal = {"version": 1, "rpc_url": primary, "peer_rpc_url": peer, "old_rpc_url": old_pair[0],
               "old_peer_rpc_url": old_pair[1], "mixed_before": len(pairs) > 1, "files": entries}
    private_json(staging / "journal.tmp", journal)
    os.replace(staging / "journal.tmp", journal_path)
    applied = []
    for entry in entries:
        if replace_checked(entry, staging / f"{entry['name']}.new", want=entry["new_sha256"], have_ok=entry["old_sha256"]):
            applied.append(entry["name"])
    return finish(journal, "changed", applied)


def read_config(path: Path) -> dict | None:
    """A generated JSON config, or None when it is missing, unreadable or not a JSON object (contents never echoed)."""
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError, UnicodeDecodeError):
        return None
    return data if isinstance(data, dict) else None


def doctor(args) -> dict:
    """Read-only, secret-free health report. It never raises on damaged state and never reports a damaged
    operator as healthy; file contents are never printed, only fixed problem descriptions."""
    directory = args.directory.resolve()
    checks, problems = {}, []
    operator = read_config(directory / "operator.json")
    required = ("bin", "role", "pool", "release_version", "signer_address", "binaries_sha256", "operator", "hosting_manifest_sha256")
    if operator is None or any(key not in operator for key in required):
        return {"status": "attention", "role": None, "pool": None, "checks": {},
                "problems": ["operator.json is missing or damaged; restore it from backup or re-run init in a new directory"],
                "secrets_printed": False}
    bins = Path(operator["bin"])

    def owner_only(path: Path, directory_expected: bool = False) -> bool:
        try:
            info = path.lstat()
        except OSError:
            return False
        kind_ok = stat.S_ISDIR(info.st_mode) if directory_expected else stat.S_ISREG(info.st_mode)
        return kind_ok and not stat.S_IMODE(info.st_mode) & 0o077

    try:
        verify_installed(bins, release_pins_from(operator))
        checks["installed_commands"] = "match the release"
    except (OSError, ValueError, KeyError) as error:
        problems.append(str(error) if isinstance(error, ValueError) else "installed commands could not be checked")
    configs = [name for name in ("wallet.json", "payment.json", "negotiate.json", "access.json", "mcp.json", "operator.json",
                                 "wallet-policy.json") if (directory / name).exists()]
    keys = [Path(path) for name, path in operator["operator"].items() if name.endswith("_file") and "descriptor" not in name]
    wallet = read_config(directory / "wallet.json")
    damaged = [name for name in configs if read_config(directory / name) is None]
    if damaged:
        problems.append(f"damaged or unreadable configs (contents not shown): {damaged}")
    if wallet is not None and wallet.get("transaction_key_file"):
        keys.append(Path(wallet["transaction_key_file"]))
    loose = [str(path.relative_to(directory)) if path.is_relative_to(directory) else path.name
             for path in [directory / name for name in configs] + keys if not owner_only(path)]
    loose += [name for name in ("state", "keys", "share") if not owner_only(directory / name, True)]
    if loose:
        problems.append(f"not owner-only or missing: {loose}")
    checks["owner_only"] = {"configs": len(configs), "key_files": len(keys), "problems": loose}
    endpoint = endpoint_state(directory)
    if endpoint["incomplete"]:
        problems.append("an endpoint change is incomplete; run `endpoints --resume` or `endpoints --abort`")
    if endpoint["mixed"]:
        problems.append(f"configs name different RPC endpoint pairs: {endpoint['configs']}")
    wallet_fields = ("artifact_manifest_file", "artifact_manifest_sha256", "artifact_cache_root", "rpc_url", "peer_rpc_url",
                     "signer_address")
    if wallet is None or any(key not in wallet for key in wallet_fields):
        problems.append("wallet.json is missing or damaged; the wallet, cache and endpoint checks were skipped")
        return {"status": "attention", "role": operator["role"], "pool": operator["pool"],
                "release_version": operator["release_version"], "signer_address": operator["signer_address"],
                "checks": checks, "problems": problems, "secrets_printed": False}
    try:
        manifest = Path(wallet["artifact_manifest_file"])
        if sha256(manifest) != wallet["artifact_manifest_sha256"] or wallet["artifact_manifest_sha256"] != operator["hosting_manifest_sha256"]:
            problems.append("the pinned artifact manifest changed")
        circuits = json.loads(manifest.read_text())["circuits"]
        wanted = [circuit[field]["sha256"] for circuit in circuits for field in ("wasm", "r1cs", "zkey")]
        cache = Path(wallet["artifact_cache_root"])
        checks["artifact_cache"] = {"cached": sum((cache / f"{digest}.artifact").exists() for digest in wanted), "of": len(wanted),
                                    "note": "missing artifacts download over HTTPS on first proof; "
                                            "`erebus-init --metropolis quickstart prefetch` (or erebus-artifacts) installs them now"}
    except (OSError, ValueError, KeyError, TypeError):
        problems.append("the pinned artifact manifest is missing or unreadable")
    try:
        notes = command(bins, "erebus-wallet", {"method": "notes", "config_file": str(directory / "wallet.json")}, directory)
        checks["wallet"] = {"opens": True, "notes": len(notes["result"]["notes"]), "operations": len(notes["result"]["operations"])}
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        problems.append("the installed wallet does not open this configuration")
    if not args.offline:
        endpoints = {}
        for label in ("rpc_url", "peer_rpc_url"):
            url = wallet[label]
            entry = {}
            try:
                check = network_check(bins, url, directory)
                finalized = (check.get("finalized") or {}).get("number")
                entry.update(passed=check.get("passed") is True,
                             finalized=finalized if type(finalized) is int else None)
                if not entry["passed"]:
                    problems.append(f"{label} failed the installed network check")
            except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
                entry["passed"] = False
                problems.append(f"{label} network check unavailable: {type(error).__name__}")
            try:
                entry["signer_balance_wei"] = rpc_balance(url, wallet["signer_address"])
            except (OSError, ValueError, KeyError, TypeError) as error:
                entry["signer_balance_wei"] = None
                problems.append(f"{label} balance read failed: {type(error).__name__}")
            endpoints[label] = entry
        checks["endpoints"] = endpoints
        if endpoints and all(entry.get("signer_balance_wei") == 0 for entry in endpoints.values()):
            problems.append("the gas signer has no testnet MON yet")
    return {"status": "healthy" if not problems else "attention", "role": operator["role"], "pool": operator["pool"],
            "release_version": operator["release_version"], "signer_address": operator["signer_address"],
            **({"rehearsal": operator["rehearsal"]} if "rehearsal" in operator else {}),
            "checks": checks, "problems": problems, "secrets_printed": False}


WALLET_READS = ("notes", "sync")
WALLET_WRITES = ("approve", "wrap", "unwrap", "deposit", "withdraw", "resume", "replace")
# Optional read actions: never enabled by default, each gated on an installed capability.
WALLET_OPTIONAL_READS = {"status": "paired_account_diagnostics"}


def replace_private_json(path: Path, data: dict) -> None:
    """Rewrite an owner-only file atomically; the new file is created 0600 before any bytes are written."""
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    private_json(temporary, data)
    os.replace(temporary, path)


def wallet_policy(args) -> dict:
    """Write an explicit wallet action policy and add the wallet to this operator's MCP settings.

    Reads (notes, sync) are always allowed; every write method must be named with --allow. Funding methods
    are enabled only when the installed erebus-wallet advertises durable operation references."""
    directory = args.directory.resolve()
    operator = json.loads((directory / "operator.json").read_text())
    bins = Path(operator["bin"])
    verify_installed(bins, release_pins_from(operator))
    maximum = require_unsigned(args.maximum_amount, "--maximum-amount")
    if int(maximum) == 0:
        raise ValueError("--maximum-amount must be positive")
    allowed = set(args.allow)
    unknown = allowed - set(WALLET_WRITES) - set(WALLET_OPTIONAL_READS)
    if unknown:
        raise ValueError(f"unsupported wallet methods: {sorted(unknown)}")
    if {"wrap", "unwrap"} & allowed and operator["pool"] != "wmon":
        raise ValueError("wrap and unwrap apply only to the wrapped-native (wmon) pool")
    recipients = sorted({value.lower() for value in args.withdrawal_recipient})
    if any(not ADDRESS.fullmatch(value) or value == "0x" + "0" * 40 for value in recipients):
        raise ValueError("withdrawal recipients must be canonical nonzero addresses")
    if "withdraw" in allowed and not recipients:
        raise ValueError("allowing withdraw requires at least one --withdrawal-recipient")
    if recipients and "withdraw" not in allowed:
        raise ValueError("--withdrawal-recipient applies only when withdraw is allowed")
    fees = None
    if "replace" in allowed:
        if args.replacement_fees is None:
            raise ValueError("allowing replace requires --replacement-fees MAX_FEE PRIORITY_FEE")
        fees = [require_unsigned(value, "--replacement-fees") for value in args.replacement_fees]
        if int(fees[1]) > int(fees[0]):
            raise ValueError("the replacement priority fee exceeds its cap")
    elif args.replacement_fees is not None:
        raise ValueError("--replacement-fees applies only when replace is allowed")
    # Same reading as the SDK's MetropolisWallet.version(): the envelope's result advertises protocol, methods
    # and durable-reference capabilities.
    capabilities = command(bins, "erebus-wallet", {"method": "version"}, directory).get("result") or {}
    if capabilities.get("protocol") != 1 or not set(WALLET_READS) | allowed <= set(capabilities.get("methods") or []):
        raise ValueError("the installed erebus-wallet does not advertise the requested methods")
    if "deposit" in allowed and capabilities.get("deposit_operation_references") is not True:
        raise ValueError("the installed erebus-wallet lacks durable deposit references; deposit stays disabled")
    if {"approve", "wrap", "unwrap"} & allowed and capabilities.get("funding_operation_references") is not True:
        raise ValueError("the installed erebus-wallet lacks durable funding references; approve/wrap/unwrap stay disabled")
    for method, capability in WALLET_OPTIONAL_READS.items():
        if method in allowed and capabilities.get(capability) is not True:
            raise ValueError(f"the installed erebus-wallet lacks {capability}; {method} stays disabled")
    policy = {"version": 1, "maximum_amount": maximum, "allowed_methods": sorted(set(WALLET_READS) | allowed),
              "withdrawal_recipients": recipients}
    if fees is not None:
        policy["replacement_fees"] = fees
    target = directory / "wallet-policy.json"
    (replace_private_json if target.exists() else private_json)(target, policy)
    written = ["wallet-policy.json"]
    mcp = directory / "mcp.json"
    if mcp.exists():
        settings = json.loads(mcp.read_text())
        entry = next(iter(settings["mcpServers"].values()))
        entry["env"].update({"EREBUS_WALLET_CONFIG": str(directory / "wallet.json"), "EREBUS_WALLET_POLICY": str(target),
                             "EREBUS_WALLET_CLI": str(bins / "erebus-wallet")})
        replace_private_json(mcp, settings)
        written.append("mcp.json")
    return {"status": "wallet-policy-written", "directory": str(directory), "written": written,
            "allowed_methods": policy["allowed_methods"], "maximum_amount": maximum,
            "funding_enabled": sorted(allowed & {"approve", "wrap", "unwrap", "deposit"}),
            "next": "restart the MCP server; agents can call only the methods above, each capped at maximum_amount"
            if mcp.exists() else "run connect to generate mcp.json, then re-run wallet-policy"}


AUDITOR_HISTORY = ("checkpoint", "full")


def auditor_checkpoint(bins: Path, directory: Path) -> dict:
    """Start the auditor's empty cache from a paired finalized checkpoint (native, idempotent, no log reads).

    The record changes which blocks the auditor scans, not what a verified payment means: a deal settled at
    or before the checkpoint block verifies as `settled_before_checkpoint`, never as paid or unpaid.
    """
    deployment = json.loads((directory / "disclosure.json").read_text())
    try:
        reply = command(bins, "erebus-shielded-disclosure",
                        {"method": "checkpoint", "deployment": deployment, "confirm_new_cache": True}, directory)
    except ValueError:
        raise ValueError("the auditor checkpoint failed: both RPC providers must answer and agree on the same "
                         "finalized block, and the cache must hold no full history; retry, or omit --history checkpoint") from None
    return {"checkpoint": reply["checkpoint"], "checkpoint_block": reply["checkpoint_block"],
            "history_from_block": reply["history_from_block"]}


def auditor(args) -> dict:
    history = getattr(args, "history", "full")
    if history not in AUDITOR_HISTORY:
        raise ValueError("--history must be checkpoint or full")
    bins = args.bin.resolve()
    pinned = release_pins(args.pins.resolve(), args.pool)
    verify_installed(bins, pinned["release"])
    directory = args.directory.resolve()
    if directory.exists():
        raise ValueError("use a new auditor directory")
    disclosure = load("disclosure_config")
    directory.mkdir(mode=0o700)
    command(bins, "erebus-shielded-disclosure", {"method": "keygen", "key_file": str(directory / "auditor.key")}, directory)
    (directory / "cache").mkdir(mode=0o700)
    private_json(directory / "disclosure.json", disclosure.render(args.deployment, args.pool, args.rpc_url,
                                                                  args.peer_rpc_url, str(directory / "cache")))
    written = ["auditor.key (Rust)", "disclosure.json"]
    rehearsal = pinned["rehearsal"]
    if rehearsal is not None:
        private_json(directory / REHEARSAL_MARKER, {**rehearsal_record(rehearsal),
                                                    "labels": ["test-only", "single-operator", "unaudited", *REHEARSAL_LABELS]})
        written.append(REHEARSAL_MARKER)
    result = {"status": "auditor-ready", "directory": str(directory), "written": written, "history": history,
              **({"rehearsal": rehearsal_record(rehearsal)} if rehearsal is not None else {})}
    if history == "checkpoint":
        checkpoint = auditor_checkpoint(bins, directory)
        written.append("cache/history-checkpoint.json (Rust)")
        result.update(checkpoint_block=checkpoint["checkpoint_block"],
                      history_from_block=checkpoint["history_from_block"],
                      scope="deals settled at or before checkpoint_block verify as settled_before_checkpoint; "
                            "verify those with a full-history auditor directory (the default --history full)")
    else:
        result["history_from_block"] = json.loads((directory / "disclosure.json").read_text())["first_block"]
    return result


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    pinning = commands.add_parser("pins")
    pinning.add_argument("--version", help="published package version (HTTPS download)")
    pinning.add_argument("--directory", type=Path, required=True)
    pinning.add_argument("--rehearsal-release", type=Path,
                         help="offline rehearsal: an unpublished candidate's release.json (never publication evidence)")
    pinning.add_argument("--rehearsal-release-sha256", help="rehearsal: the exact SHA-256 of --rehearsal-release")
    pinning.add_argument("--authorized-pins", type=Path,
                         help="offline pins: an existing `pins --version` directory supplying the signed pool manifests")
    pinning.add_argument("--prepublication-release", type=Path,
                         help="native qualification: a clean candidate's release.json for the version --authorization names")
    pinning.add_argument("--prepublication-release-sha256", help="prepublication: the exact SHA-256 of the release.json")
    pinning.add_argument("--authorization", type=Path,
                         help="prepublication: the owner authorization committed at the candidate's source commit")
    first = commands.add_parser("init")
    first.add_argument("--role", choices=("buyer", "seller"), required=True)
    first.add_argument("--pool", choices=("wmon", "usdc"), required=True)
    first.add_argument("--directory", type=Path, required=True)
    first.add_argument("--bin", type=Path, required=True, help="installed release bin directory")
    first.add_argument("--pins", type=Path, required=True, help="release pins directory")
    first.add_argument("--deployment", type=Path, required=True)
    first.add_argument("--endpoint", required=True, help="host:port this operator's negotiation listens on")
    first.add_argument("--rpc-url", required=True)
    first.add_argument("--peer-rpc-url", required=True)
    first.add_argument("--signer-address", required=True)
    first.add_argument("--transaction-key-file", type=Path)
    first.add_argument("--maximum-price", help="buyer: highest price in base units")
    first.add_argument("--relay-url", help="sponsored buyer: https://<host>/v1/relay (needs an advertised payment capability)")
    first.add_argument("--relay-identity", help="sponsored buyer: the relay's 32-byte service identity")
    first.add_argument("--relay-sponsor", help="sponsored buyer: the relay's public gas address")
    first.add_argument("--relay-auth-key-file", type=Path, help="sponsored buyer: owner-only bearer key file (path only)")
    first.add_argument("--relay-timeout-seconds", type=int, default=60)
    first.add_argument("--descriptor-lifetime-seconds", type=int, default=86400)
    first.add_argument("--binding-lifetime-seconds", type=int, default=21600)
    second = commands.add_parser("connect")
    second.add_argument("--directory", type=Path, required=True)
    second.add_argument("--peer-share", type=Path, required=True)
    second.add_argument("--resource", required=True)
    second.add_argument("--amount", required=True, help="this side's opening price in base units")
    second.add_argument("--minimum-price")
    second.add_argument("--unit", default="snapshot")
    second.add_argument("--quantity", default="1")
    second.add_argument("--payload-file", type=Path)
    second.add_argument("--port", type=int)
    second.add_argument("--delivery-seconds", type=int, default=21600)
    second.add_argument("--service-url", help="buyer: the seller's access URL, https://<host>/v1/access; adds retrieval to mcp.json")
    second.add_argument("--allow-loopback-access-http", action="store_true",
                        help="buyer: accept a plain-http --service-url on a loopback host (local rehearsal only)")
    checkup = commands.add_parser("doctor")
    checkup.add_argument("--directory", type=Path, required=True)
    checkup.add_argument("--offline", action="store_true", help="skip RPC endpoint and balance checks")
    policy = commands.add_parser("wallet-policy", help="explicit agent wallet permissions (default: notes and sync only)")
    policy.add_argument("--directory", type=Path, required=True)
    policy.add_argument("--maximum-amount", required=True, help="per-action cap in base units")
    policy.add_argument("--allow", action="append", default=[], choices=(*WALLET_WRITES, *WALLET_OPTIONAL_READS),
                        help="repeatable: a write method, or the optional status read, agents may call; none by default")
    policy.add_argument("--withdrawal-recipient", action="append", default=[], help="repeatable; required with --allow withdraw")
    policy.add_argument("--replacement-fees", nargs=2, metavar=("MAX_FEE", "PRIORITY_FEE"), help="required with --allow replace")
    change = commands.add_parser("endpoints", help="change the primary and peer RPC endpoints of generated configs")
    change.add_argument("--directory", type=Path, required=True)
    change.add_argument("--rpc-url")
    change.add_argument("--peer-rpc-url")
    change.add_argument("--bin", type=Path, help="auditor directories only: the installed release bin directory")
    mode = change.add_mutually_exclusive_group()
    mode.add_argument("--resume", action="store_true", help="finish an interrupted change")
    mode.add_argument("--abort", action="store_true", help="restore the configs saved before an interrupted change")
    third = commands.add_parser("auditor")
    third.add_argument("--pool", choices=("wmon", "usdc"), required=True)
    third.add_argument("--directory", type=Path, required=True)
    third.add_argument("--bin", type=Path, required=True)
    third.add_argument("--pins", type=Path, required=True, help="release pins directory; installed binaries are checked against it")
    third.add_argument("--deployment", type=Path, required=True)
    third.add_argument("--rpc-url", required=True)
    third.add_argument("--peer-rpc-url", required=True)
    third.add_argument("--history", choices=AUDITOR_HISTORY, default="full",
                       help="full (default): replay from the pool deployment, no RPC at setup; verifies deals that "
                            "settled before this auditor existed. checkpoint: faster opt-in for an auditor set up "
                            "before its deals; scans from a paired finalized block taken now, and deals settled at "
                            "or before it verify as settled_before_checkpoint")
    quickstart = load("quickstart")
    quickstart.add_parser(commands)
    args = parser.parse_args(argv)
    try:
        if args.command == "quickstart":
            result = quickstart.run(args, SimpleNamespace(**globals()))
        else:
            result = {"pins": pins, "init": init, "connect": connect, "doctor": doctor, "auditor": auditor,
                      "wallet-policy": wallet_policy, "endpoints": endpoints}[args.command](args)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(json.dumps({"status": "error", "error": str(error) if isinstance(error, ValueError) else type(error).__name__}))
        return 2
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
