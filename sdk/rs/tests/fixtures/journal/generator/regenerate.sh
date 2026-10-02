#!/usr/bin/env bash
# Regenerates the historical operation-journal fixtures from the commits that defined each
# schema. Provenance tooling only: cargo never compiles anything in this directory and CI
# never runs this script.
#
# For each commit it extracts `sdk/rs` at that commit into a scratch directory outside the
# repository (`git archive`, no worktree metadata), appends a throwaway `#[cfg(test)]`
# generator module to that commit's `src/journal.rs`, runs it, and copies the records and
# stored transactions it wrote (not the runtime `.lock` files) byte for byte.
#
# Usage: regenerate.sh [DEST]   (DEST defaults to the fixtures directory above this one)
# Needs the crates of each historical Cargo.lock in the local registry cache (`--offline`);
# drop `--offline` below to fetch them.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="${1:-$(dirname "$HERE")}"
REPO="$(git -C "$HERE" rev-parse --show-toplevel)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/erebus-journal-fixtures.XXXXXX")"
export CARGO_TARGET_DIR="$WORK/target"

# generate <commit> <era byte> <T0> <template> <markers kept> <markers dropped> <v3+ accepted_at> <fixture dir>
generate() {
  local commit="$1" era="$2" t0="$3" template="$4" keep="$5" drop="$6" block_ts="$7" dir="$8"
  mkdir -p "$WORK/$commit"
  git -C "$REPO" archive --format=tar "$commit" sdk/rs | tar -x -C "$WORK/$commit"

  local gen
  gen="$(sed -e "s/@COMMIT@/$commit/" -e "s/@ERA@/$era/" -e "s/@T0@/$t0/" \
    -e "s/@BLOCK_TS@/$block_ts/" "$HERE/$template")"
  for marker in $drop; do
    gen="$(printf '%s\n' "$gen" | grep -vF "/*$marker*/")"
  done
  for marker in $keep; do
    gen="$(printf '%s\n' "$gen" | sed -e "s|/\*$marker\*/||")"
  done
  printf '%s\n' "$gen" >>"$WORK/$commit/sdk/rs/src/journal.rs"

  mkdir -p "$WORK/out/$commit"
  (cd "$WORK/$commit/sdk/rs" && FIXTURE_OUT="$WORK/out/$commit" \
    cargo test --locked --offline --lib fixture_gen::generate -- --ignored --quiet)

  rm -rf "${DEST:?}/$dir"
  mkdir -p "$DEST/$dir"
  cp "$WORK/out/$commit/operations/"*.json "$DEST/$dir/"
  if compgen -G "$WORK/out/$commit/operations/*.tx" >/dev/null; then
    cp "$WORK/out/$commit/operations/"*.tx "$DEST/$dir/"
  fi
  chmod 0644 "$DEST/$dir/"*
  echo "$dir: $(find "$DEST/$dir" -type f | wc -l | tr -d ' ') files from $commit"
}

#        commit  era  T0 (commit date, 00:00 UTC)   template           keep     drop     v3+  dir
generate b784f3f 0x1a 1787443200 gen_v1.rs.tmpl     "A AB"  "BC C"  false v1-b784f3f
generate 7b7b4c8 0x1b 1787446800 gen_v1.rs.tmpl     "BC AB" "A C"   false v1-7b7b4c8
generate 448146e 0x1c 1787450400 gen_v1.rs.tmpl     "BC C"  "A AB"  false v1-448146e
generate b1fe0e3 0x20 1787529600 gen_v2plus.rs.tmpl ""      "SIM"   false v2-b1fe0e3
generate d3dab59 0x30 1787875200 gen_v2plus.rs.tmpl ""      "SIM"   true  v3-d3dab59
generate 93593ed 0x40 1788220800 gen_v2plus.rs.tmpl "SIM"   ""      true  v4-93593ed

rm -rf "$WORK"
