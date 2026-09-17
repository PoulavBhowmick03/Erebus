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

## This is what the demo URL serves

`strk20.json` pins `demo_url` and `demo_video` to
`https://erebus-private-agents.vercel.app`, and **this package is what is published there.**
It replaced the old `demo/` page on 2026-09-06.

`erebusagents.live` is the custom domain; the `.vercel.app` host redirects to it.

Deploys are git-driven. The Vercel project's root directory is the repository root, so it reads
the root `vercel.json`, which installs and builds only `web/`:

```json
"installCommand": "pnpm --dir web install --frozen-lockfile --ignore-workspace",
"buildCommand": "pnpm --dir web build",
"outputDirectory": "web/out"
```

`--ignore-workspace` is load-bearing: the root workspace lists `sdk/ts`, whose
`@starkware-libs/starknet-privacy-sdk` dependency is a sibling checkout that does not exist on
the builder. Without it, `pnpm install` fails before the page is ever built. Push to `main` and
Vercel rebuilds.

**`web/public/erebus-final-cut.mp4` must stay.** The pinned `demo_video_mp4` URL resolves
to it. Delete it and a URL in `strk20.json` and the README 404s. The archived `demo/` page keeps
its own three-minute `erebus-private-sprint.mp4`.

`demo/` is still in the repo on purpose. `scripts/check-demo.py` and
`scripts/tests/test_demo.py` both run against it in CI, and it is the archived sprint
artifact. Do not delete it to tidy up.

## Design rules

Three rules carry the whole page. Breaking any one of them makes it an ordinary site.

1. **Two oranges, two jobs.** `--color-ember` (`#FB4020`, the mark's own orange) is the brand: the
   hero's atmospheric floor, the ember tick opening each `.section-head` rule, the glow under the
   footer mark, and the one filled CTA. Atmosphere and action only — never a data value.
   `--color-cinnabar` (`#FF3B1F`, a touch redder) means "a public chain reader can already read
   this." Never a button, never decoration. It appears on the counterparty address, the
   submitting account, block, timestamp, note count, the public side of the replay, the observer
   metrics that came out badly, and the `leak-tag`. Grep for it before committing and check every
   use is a leak. If ember ever lands on a data value, or cinnabar on a control, the system has
   collapsed back into one orange and the page is lying.
2. **Warm obsidian, separated by hairlines.** The canvas is `#0a0908`, not a cold near-black, and
   surfaces step up to `--color-panel` on warm neutrals. No cards, no fills, no shadows, no radii.
   **One gradient, hero only:** `.hero-floor` is an ember bloom off the top edge plus a dotted
   measure, masked to fade out. Do not add a second.
3. **The plaintext is always in the DOM.** Redaction is an ink bar drawn over readable markup,
   and the ciphertext substitution happens client-side after mount. No-JS readers, crawlers and
   link previews get the complete page. It is a demonstration of the disclosure model, not a
   security boundary.

The brand lockup (`web/public/erebus-lockup.svg`) carries the same `#FB4020` mark in the header
and the ghosted footer mark, which is why ember is the right atmosphere color here: the glow and
the logo are the same orange, so the page reads as one material.

## Routes

One static route:

- `/` — the landing page. Four sections: hero, evidence, the pool band, the replay, the boundary.

The docs are **not** in this package any more. They moved to their own repo,
`ishitab02/erebus-docs`, on 2026-09-17 and are served at the root of that Vercel project
(`erebus-docs-ishita02b-3383s-projects.vercel.app`, no `/docs` path). The hero CTA and the
header nav link straight there.

Root `vercel.json` redirects `/docs`, `/docs/*`, and the `docs.erebusagents.live` host to that
URL so old links keep working. The `docs.erebusagents.live` alias is still attached to **this**
project because `erebusagents.live` lives in a different Vercel account and can't be moved from
here; the host redirect is what keeps it usable in the meantime. When the alias is finally
moved to the docs project, remove that redirect block and point the links at the custom domain.

## Type

Mono is for data: labels, hashes, block numbers, tool names, code. Sentences are set in the
grotesque via `.prose` (and `.lead` for the one-line intro). Mono body copy at 13px was a
readability tax at exactly the moment the page wanted to be read; if you add a paragraph, give it
`.prose`, not a mono utility.

## Social

`web/public/og.png` (1200×630) is the social card, wired through `openGraph.images` and
`twitter`. Regenerate it if the tagline or the lockup changes.

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
