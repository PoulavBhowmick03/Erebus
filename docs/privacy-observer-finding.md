# Adversarial calldata observer finding

> **Not yet line-reviewed.** This harness is evidence about the supplied fixtures,
> not an external cryptographic review or fresh live wire-v2 settlement.

> The overall privacy boundary lives in [privacy-model.md](./privacy-model.md). This file is
> the harness result only.
>
The no-key observer in `scripts/observer.py` has a positive control: it reconstructs the
known wire-v1 acceptance from four public salt halves. Against the static wire-v2 fixture,
the same public recovery attack finds no plausible transcript. The observer therefore does
not learn the message type, reply target, timestamps, amount, deadline, or memo hash from
wire-v2 calldata by applying the attack that breaks wire v1. This result relies on the
security of AES-256-GCM-SIV and on the channel key remaining secret; the harness does not
prove those assumptions.

Traffic privacy fails independently. Wire v2 fills only 536 of 595 payload bits, so its
fifth salt always has bit 119 set and bits 60 through 118 clear. The harness detects that
shape without decrypting anything and classifies the transaction as likely Erebus traffic.
Because an individual wire-v1 salt can match that shape by chance, successful wire-v1
content recovery takes precedence over the shape classifier. The harness still reports the
shape collision explicitly.
An unrelated uniform 120-bit salt has that shape with probability 2^-60, or 2^-59 after
conditioning on the format flag. An observer can therefore identify likely Erebus pool
interactions and, from public transaction metadata, count and time them and associate each
transaction with its submitting account. The encrypted negotiation contents remain a
separate finding from that traffic fingerprint.
