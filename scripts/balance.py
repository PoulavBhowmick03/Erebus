#!/usr/bin/env python3
"""Print amounts that an identity can pay.

Settlement spends an exact subset of notes and mints no change note, so the spendable
amounts are subset sums, not every amount below the total. One 1 STRK note can pay 1 STRK
but not 0.9 STRK. The output gives an agent the payable set before negotiation.

Reads the `result` object of a `balance` request on stdin.
"""

import json
import sys

# Limit output when the number of subset sums grows.
MAX_LISTED = 24


def subset_sums(notes: list[int]) -> list[int]:
    """Every amount an exact-subset spend can produce, excluding the empty set."""
    reachable = {0}
    for note in notes:
        reachable |= {existing + note for existing in reachable}
    reachable.discard(0)
    return sorted(reachable)


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

    payable = subset_sums(notes)
    shown = ", ".join(f"{amount / 1e18:.4f}" for amount in payable[:MAX_LISTED])
    suffix = "" if len(payable) <= MAX_LISTED else f", … ({len(payable)} total)"
    print(f"payable exactly: {shown}{suffix}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
