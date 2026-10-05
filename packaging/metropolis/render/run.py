"""Single-instance gateway and service supervisor. Never resets durable payment state."""

import json
import os
from pathlib import Path
import signal
import subprocess
import time


def configuration(environment, root):
    mode = environment.get("EREBUS_HOSTED_SERVICE")
    port = environment.get("PORT", "10000")
    if mode not in {"relay", "access"} or not port.isascii() or not port.isdecimal() or not 1 <= int(port) <= 65535:
        raise ValueError("configure a service and a valid gateway port")
    if mode == "relay":
        token = environment.get("EREBUS_RELAY_TOKEN", "")
        if not token or len(token) > 256 or any(character.isspace() for character in token):
            raise ValueError("configure a bounded relay bearer token")
        environment.update(EREBUS_RELAY_ROOT=str(root / "relay"), EREBUS_RELAY_PORT="8081")
        environment.pop("EREBUS_RELAY_INSECURE", None)
        # Health alone injects the internal token. Mailbox callers still authenticate themselves.
        routes = "handle /healthz {\n reverse_proxy 127.0.0.1:8081 {\n header_up Authorization \"Bearer {$EREBUS_RELAY_TOKEN}\"\n }\n}\nhandle {\n reverse_proxy 127.0.0.1:8081\n}"
        binary = "erebus-relay"
    else:
        path = root / "access.json"
        if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o077:
            raise ValueError("provision an owner-only /data/access.json before starting access")
        config = json.loads(path.read_text())
        if config.get("port") != 8081:
            raise ValueError("access must use loopback port 8081 behind the gateway")
        for field in ("state_root", "evidence_root", "payload_file"):
            candidate = Path(config[field]).resolve()
            if not candidate.is_relative_to(root.resolve()):
                raise ValueError("access paths must use the persistent disk")
        if config.get("backend", {}).get("mode") == "x402_exact":
            backend = config["backend"]
            for field in ("transaction_key_file", "signer_journal_root"):
                candidate = Path(backend[field]).resolve()
                if not candidate.is_relative_to(root.resolve()):
                    raise ValueError("x402 keys and signer journals must use the persistent disk")
            key = Path(backend["transaction_key_file"])
            if key.is_symlink() or not key.is_file() or key.stat().st_mode & 0o077 or key.stat().st_size != 32:
                raise ValueError("x402 requires an owner-only regular 32-byte transaction key")
        environment["EREBUS_ACCESS_CONFIG"] = str(path)
        routes = "reverse_proxy 127.0.0.1:8081"
        binary = "erebus-access-service"
    return binary, f"{{\n admin off\n auto_https off\n}}\n:{port} {{\n{routes}\n}}\n"


def main():
    os.umask(0o077)
    environment = dict(os.environ)
    root = Path("/data")
    root.mkdir(mode=0o700, exist_ok=True)
    binary, gateway = configuration(environment, root)
    config = Path("/tmp/erebus-gateway.conf")
    config.write_text(gateway)
    children = []
    stopped = False

    def stop(*_):
        nonlocal stopped
        stopped = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        children.append(subprocess.Popen([binary], env=environment))
        children.append(subprocess.Popen(["caddy", "run", "--adapter", "caddyfile", "--config", str(config)], env=environment))
        while not stopped:
            if any(child.poll() is not None for child in children):
                raise RuntimeError("hosted service stopped; persistent state retained")
            time.sleep(0.2)
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
        for child in children:
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError):
        raise SystemExit("hosted service configuration or process failed; inspect private configuration and retain state") from None
