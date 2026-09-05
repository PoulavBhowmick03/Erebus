# web

The public Erebus site. Next.js, static export, no server.

```bash
pnpm install --ignore-workspace   # this package is deliberately outside the repo workspace
pnpm dev                          # http://localhost:4000
pnpm build                        # static export to ./out
pnpm typecheck
```

`--ignore-workspace` matters: `pnpm-workspace.yaml` lists only `sdk/ts`, and adding `web`
to it would pull this package into the root `pnpm -r typecheck`/`build` that CI runs against
the differential-test oracle. It stays standalone on purpose.

## This does not replace `demo/`

`demo/` is still the sprint's published evidence page. `strk20.json` pins
`demo_url`/`demo_video` to it, and `scripts/check-demo.py` runs in CI against its three files
verbatim. Nothing here touches it. Deploy this as a separate Vercel project with the root
directory set to `web/`.

## Design rules

Three rules carry the whole page. Breaking any one of them makes it an ordinary site.

1. **Cinnabar means "a public chain reader can already read this."** Never a button, never a
   link, never decoration. `--color-cinnabar` appears on the counterparty address, the
   submitting account, block, timestamp, note count, the Public column of the leak ledger, and
   the metrics that came out badly. Grep for it before committing and check every use is a leak.
2. **Structure comes from hairlines and space.** No cards, no fills, no shadows, no radii, no
   gradients — the one gradient in the tree is the legibility veil over the void section.
3. **The plaintext is always in the DOM.** Redaction is an ink bar drawn over readable markup,
   and the ciphertext substitution happens client-side after mount. No-JS readers, crawlers and
   link previews get the complete page. It is a demonstration of the disclosure model, not a
   security boundary.

## The two states

`<html data-key="held|dropped">` is the only global state. The document ships `dropped` and is
handed a key ~900ms after mount; the header pill hands it back. The argument the page is making
is that **the cinnabar facts are identical in both states** — that is F38, rendered as an
interface. If a redesign ever makes a red value change when the key drops, the page has started
lying.

Three distinct levels of disclosure, and they must stay distinct:

| | what you see |
|---|---|
| barred | an ink bar. a value exists here, nothing more |
| peeked (hover, focus, or press) | what a chain reader actually gets: ciphertext |
| decrypted (key held) | the record |

## WebGL

`NoteLattice` is the only three-dimensional element and it is a diagram, not an atmosphere:
orthographic camera, unlit constant-size marks, no lighting model, no post-processing. It draws
the anonymity set — thousands of pool notes with the settlement's seven in cinnabar, because
that count is public. `highlight={false}` drops the seven where the lattice is used as a
backdrop and they would read as confetti.

It degrades to nothing if `WebGLRenderer` throws, pauses when off-screen, and renders a single
static frame under `prefers-reduced-motion`.

## Content

Every fact on the page is sourced in `lib/content.ts`, which names the document each value came
from. `docs/status.md` is the tiebreaker and `docs/privacy-model.md` is the only source allowed
to make a privacy claim. Do not add a claim that is not already written down in the repo.
