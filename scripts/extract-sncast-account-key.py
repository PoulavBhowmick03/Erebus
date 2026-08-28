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
    parser.add_argument(
        "--accounts-file",
        default="~/.starknet_accounts/starknet_open_zeppelin_accounts.json",
        help="sncast accounts registry (default: %(default)s)",
    )
    parser.add_argument(
        "--network",
        default="alpha-sepolia",
        help="registry network key (default: %(default)s)",
    )
    parser.add_argument(
        "--filename",
        default="account.key",
        help="destination filename inside directory (default: %(default)s)",
    )
    args = parser.parse_args()

    registry = Path(args.accounts_file).expanduser()
    networks = json.loads(registry.read_text())
    if args.network not in networks:
        available = ", ".join(sorted(networks)) or "<none>"
        raise SystemExit(
            f"unknown network {args.network!r}; available: {available}"
        )
    accounts = networks[args.network]
    if args.name not in accounts:
        available = ", ".join(sorted(accounts)) or "<none>"
        raise SystemExit(
            f"unknown {args.network} account {args.name!r}; available: {available}"
        )

    filename = Path(args.filename)
    if filename.name != args.filename or args.filename in {"", ".", ".."}:
        raise SystemExit("filename must be one plain file name")

    directory = Path(args.directory).expanduser().resolve()
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(directory, 0o700)
    destination = directory / filename

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
