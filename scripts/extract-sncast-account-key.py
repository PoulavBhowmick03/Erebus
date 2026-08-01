#!/usr/bin/env python3
"""Copy one sncast account key into an Erebus-owned mode-0600 file.

The private key is never printed and an existing destination is never overwritten.
"""

import argparse
import json
import os
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("name", help="sncast account name")
    parser.add_argument("directory", help="Erebus identity directory")
    args = parser.parse_args()

    registry = Path(
        "~/.starknet_accounts/starknet_open_zeppelin_accounts.json"
    ).expanduser()
    accounts = json.loads(registry.read_text())["alpha-sepolia"]
    if args.name not in accounts:
        available = ", ".join(sorted(accounts)) or "<none>"
        raise SystemExit(
            f"unknown alpha-sepolia account {args.name!r}; available: {available}"
        )

    directory = Path(args.directory).expanduser().resolve()
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(directory, 0o700)
    destination = directory / "account.key"

    descriptor = os.open(
        destination,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL,
        0o600,
    )
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w") as output:
            output.write(accounts[args.name]["private_key"] + "\n")
    except BaseException:
        destination.unlink(missing_ok=True)
        raise

    print(f"address: {accounts[args.name]['address']}")
    print(f"account key: {destination}")


if __name__ == "__main__":
    main()
