> **Read this before installing.** These statements apply to every Erebus release before
> `v1.0` and are prepended to the generated notes automatically, so no release can quietly
> omit them.

**This software is unaudited.** No independent cryptographic or security review has been
performed on the wire, the settlement path, or the disclosure design. Do not put value you
care about through it.

**Mainnet readiness is a release gate, not a forecast.** Current source completed three
mainnet registrations and two directional channel opens by 2026-08-30. Shielding, offers,
settlement, recovery, and disclosure remain Sepolia-only. The `v0.2.0` tag is blocked until
a packaged full mainnet canary is recorded and this paragraph is updated to link that
evidence. Until then, this is not a mainnet-verified install.

**Relationship privacy is not complete.** Erebus hides the terms, not the relationship.
Negotiation content and settlement amounts are confidential, and that is demonstrated: an
observer with no key recovers the full terms from wire v1 and nothing from wire v2. But
opening a channel writes the counterparty's address to public calldata (F38), and a fixed
fifth-salt shape lets an observer count and time Erebus traffic without reading it (F31).
Never describe this release as private without qualification. The full boundary is in
[docs/privacy-model.md](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/privacy-model.md).

**Supported platforms are Linux x86-64 and macOS arm64.** Intel macOS is not built: its CI
runner is no longer available, and cross-compiling would ship a binary that was never
executed on its own architecture. There is no Intel macOS wheel or raw binary in this
release.

---
