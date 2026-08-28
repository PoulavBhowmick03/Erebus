#!/usr/bin/env bash
set -euo pipefail

readonly SNAPSHOT_URL="https://juno-snapshots.nethermind.io/files/mainnet-pruned/latest"
readonly DATA_PARENT="${JUNO_DATA_PARENT:-/srv/erebus}"
readonly DATA_DIR="${DATA_PARENT}/juno_mainnet_pruned"
readonly ARCHIVE="${DATA_PARENT}/juno_mainnet_pruned.tar.zst"

for command in docker wget tar unzstd; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "missing required command: ${command}" >&2
    exit 1
  fi
done

if ! docker compose version >/dev/null 2>&1; then
  echo "Docker Compose v2 is required" >&2
  exit 1
fi

if [[ ! -f juno.env ]]; then
  echo "copy juno.env.example to juno.env and set ETHEREUM_WS_URL first" >&2
  exit 1
fi

mkdir -p "${DATA_PARENT}"

if [[ -d "${DATA_DIR}" ]] && [[ -n "$(find "${DATA_DIR}" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "using existing Juno database at ${DATA_DIR}"
else
  available_kib="$(df -Pk "${DATA_PARENT}" | awk 'NR == 2 {print $4}')"
  required_kib=$((300 * 1024 * 1024))
  if (( available_kib < required_kib )); then
    echo "at least 300 GiB free is required while the snapshot archive and database coexist" >&2
    exit 1
  fi

  echo "downloading the resumable pruned mainnet snapshot"
  wget --continue --output-document="${ARCHIVE}" "${SNAPSHOT_URL}"
  echo "extracting the snapshot"
  tar --use-compress-program=unzstd -xf "${ARCHIVE}" -C "${DATA_PARENT}"
fi

docker compose --env-file juno.env pull juno
docker compose --env-file juno.env up -d juno

echo "Juno started on remote loopback port 6060"
echo "Keep ${ARCHIVE} until verify.sh reports that Juno is ready"
