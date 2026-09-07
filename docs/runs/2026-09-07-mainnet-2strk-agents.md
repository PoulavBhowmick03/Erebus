# Mainnet 2 STRK run, two agent frameworks — 2026-09-07

The third complete bounded Erebus workflow on Starknet mainnet, and the first driven by **two
different agent frameworks** rather than two processes of the same one. Claude Code held the
payer identity and Codex held the payee. Neither could see the other's session: the channel
was the only medium between them.

This is definition-of-done 4 — an external agent framework driving the whole loop through the
MCP server without touching Erebus internals — demonstrated across frameworks.

## Frozen inputs

| Item | Value |
| --- | --- |
| Network | `SN_MAIN` |
| Source | `2ce8289` |
| Client binary | `sdk/rs/target/debug/erebus-cli` (see friction F41) |
| Prover | Starkscan asynchronous mainnet relay |
| Pool | `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a` |
| Token | STRK |
| Pool fee | 6 STRK per `apply_actions` |
| Shield | 2.5 STRK from Account A |
| Opening offer | 1.6 STRK, buyer-authored (Claude Code) |
| Agreement | 2.0 STRK from A to B, seller-authored (Codex) |
| Change | 0.5 STRK to A |
| Deal ID | `18017819677971087339` |

Both existing directional channels were reused; no channel was opened. Allowances were sized
to exactly the writes planned — A 20.5 STRK (2.5 deposit plus three 6 STRK pool fees), B
6 STRK (one fee) — and both read back `0` afterwards, leaving no standing allowance.

## Readiness

Both identities passed `doctor` on `0x534e5f4d41494e`: RPC, prover, chain id, pool version 2.0,
registration, key files, and public funding. A's largest existing note was 0.4 STRK, so the
2.5 STRK shield was required rather than optional; this was confirmed with `get_note_balance`
before any write.

## Mainnet transactions

| Action | Transaction | Block | Driver |
| --- | --- | ---: | --- |
| Allowance, A | `0x3db8c21448cae46b6694053cdb989eb83503b7930a45ba12b00f68ca12e4220` | 14501535 | operator |
| Allowance, B | `0x1b1b7928926cebcb9fec10a24f89615e0106eea9beadf99adf53690950478b7` | 14501547 | operator |
| Screened shield, 2.5 | `0x67b6e2e69dad6018fb2cdf7d81b900fc89de050397a3ecae6c943407609a2a` | 14501798 | operator |
| Buyer proposal, 1.6 | `0x18c7fb7340ec0df8f22fb1ff8af8ba604590df95679156ded296c18a2e40645` | 14502327 | **Claude Code** |
| Seller counter, 2.0 | `0x5a7a24b55a256602698ec37dd38e87707d43e5da50b28b9725d981e98798cff` | 14502531 | **Codex** |
| Atomic settlement, 2.0 | `0x2582f34a10f6a3c9f1fbfdad4622c2f8a79398a1782c7d425eac13698e7f5a7` | 14502618 | **Claude Code** |

The two allowance transactions are ordinary ERC-20 approvals on the account key. They carry no
proof and touch no pool state, so only the four `apply_actions` writes are listed in
`strk20.json`.

Total cost: Account A went from 52.680807 to 25.202741 STRK, Account B from 19.645868 to
11.293062 STRK — about 35.8 STRK in pool and network fees combined.

## Checks

- **Payment and change conserved.** Before: A held 2.5 / 0.4 / 0.2. After: A holds 0.5 / 0.4 /
  0.2 and B holds 2.0 / 0.8 / 0.6. The 2.5 note was spent, 2.0 went to B, 0.5 returned as change.
- **Allowances exact.** Both read back `0`, confirming the sizing was neither short nor loose.
- **Scoped disclosure verified.** A granted a wire-v3 viewing key for deal
  `18017819677971087339` only, to Account C (`0x6b293619b447480677ee2da22dbb7c442c4ae0251de9889f9cbb375ef51bad6`),
  with a one-hour expiry. C — which was not party to the deal — reconstructed all three offers
  (1.6 proposed, 2.0 countered, 2.0 accepted) and the settlement record, `agreed 2.0 / paid 2.0`.
  The two earlier deals in the same channel, from the 2026-08-31 canaries, stayed unreadable to C.

## What this run does not establish

It is a third bounded workflow, not evidence of capacity, uptime, independent security review,
or safe use with real value. The relationship, transaction timing, action shape, and note count
remain public exactly as [privacy-model.md](../privacy-model.md) describes. Nothing here
changes the privacy claim.
