#!/usr/bin/env python3
"""Print amounts that an identity can pay.

Settlement spends notes covering the price and returns any excess as a new change note, so
any positive amount up to the total is payable.

Reads the `result` object of a `balance` request on stdin.
"""

import json
import sys


def main() -> int:
    result = json.load(sys.stdin)
    notes = sorted((int(n) for n in result.get("notes", [])), reverse=True)
    total = int(result.get("total", 0))
    pending = sorted((int(n) for n in result.get("pending", [])), reverse=True)

    if not notes:
        if pending:
            # Show pending funds separately so the agent does not deposit again.
            print("no spendable notes yet, but a deposit has landed and is maturing:")
            for amount in pending:
                print(f"  pending {amount:>20}  = {amount / 1e18:.4f}")
            print("it becomes spendable once it is proving_block_lag blocks deep")
            return 0
        print("no unspent notes; this identity cannot settle anything yet")
        print("fund it with: agent.sh <env> fund <amount_wei>")
        return 0

    for amount in notes:
        print(f"note  {amount:>22}  = {amount / 1e18:.4f}")
    print(f"total {total:>22}  = {total / 1e18:.4f}")

    for amount in pending:
        print(f"pend  {amount:>22}  = {amount / 1e18:.4f}  (maturing)")

    print(f"payable: 0 < amount <= {total / 1e18:.4f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
