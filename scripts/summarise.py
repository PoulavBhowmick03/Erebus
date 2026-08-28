#!/usr/bin/env python3
"""Print one line per offer in a channel transcript.

The compact form keeps the fields needed for a threshold rule and removes repeated channel
ids from the full `read_channel_state` result.

Reads the `result` object on stdin.
"""

import json
import sys


def main() -> int:
    result = json.load(sys.stdin)
    participants = result.get("participants") or [None]
    me = participants[0]

    for offer in result.get("offers", []):
        who = "me" if offer["proposer"] == me else "them"
        amount = int(offer["terms"]["amount"]) / 1e18
        # Offer ids are channel-scoped, so the channel half is noise once you know the channel.
        short = offer["offer_id"].split(":", 1)[1]
        reply = offer["reply_to"].split(":", 1)[1] if offer.get("reply_to") else "-"
        print(f"{short:<10} {who:<5} {amount:>12.4f}  {offer['status']:<10} reply_to={reply}")

    print(f"settled={result.get('settled')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
