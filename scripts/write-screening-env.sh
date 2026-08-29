#!/usr/bin/env bash
# Write the proof-interceptor credentials without putting a secret in shell history.

set -euo pipefail

OUTPUT_ENV="${1:-$HOME/.erebus/screening.env}"
case "$OUTPUT_ENV" in
    /*) ;;
    *) echo "output env path must be absolute" >&2; exit 2 ;;
esac
[ ! -e "$OUTPUT_ENV" ] || {
    echo "refusing to overwrite: $OUTPUT_ENV" >&2
    exit 1
}

printf 'Screening proxy URL: '
IFS= read -r SCREENING_URL
printf 'Partner name: '
IFS= read -r SCREENING_PARTNER_NAME
printf 'Partner secret: '
IFS= read -rs SCREENING_PARTNER_SECRET
echo

case "$SCREENING_URL" in
    https://*) ;;
    *) echo "screening proxy URL must use https://" >&2; exit 2 ;;
esac
[ -n "$SCREENING_PARTNER_NAME" ] || { echo "partner name is empty" >&2; exit 2; }
[ -n "$SCREENING_PARTNER_SECRET" ] || { echo "partner secret is empty" >&2; exit 2; }

OUTPUT_DIR=$(dirname "$OUTPUT_ENV")
mkdir -p "$OUTPUT_DIR"
chmod 700 "$OUTPUT_DIR"
TEMP_ENV=$(mktemp "$OUTPUT_ENV.tmp.XXXXXX")
trap 'rm -f "$TEMP_ENV"' EXIT
chmod 600 "$TEMP_ENV"
printf '%s\n' \
    "SCREENING_URL=$SCREENING_URL" \
    "SCREENING_PARTNER_NAME=$SCREENING_PARTNER_NAME" \
    "SCREENING_PARTNER_SECRET=$SCREENING_PARTNER_SECRET" \
    'SCREENING_POOL_ADDRESS=0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a' \
    > "$TEMP_ENV"
mv "$TEMP_ENV" "$OUTPUT_ENV"
trap - EXIT

unset SCREENING_PARTNER_SECRET
echo "screening env written: $OUTPUT_ENV"
