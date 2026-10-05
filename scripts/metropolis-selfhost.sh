#!/usr/bin/env bash
# Self-host the Metropolis services from installed binaries (the same ones a hosted deployment
# runs). Everything lives under one owner-only directory. See docs/metropolis-operations.md.
#
#   metropolis-selfhost.sh init  DIR   create the layout and a relay token
#   metropolis-selfhost.sh up    DIR   start each configured HTTP service and wait for health
#   metropolis-selfhost.sh check DIR   probe every configured service
#   metropolis-selfhost.sh down  DIR   stop what `up` started
#
# A service is configured when its file exists: relay.env, access.json, indexer.env,
# relayer.env. Binaries are resolved from PATH only.
set -euo pipefail
umask 077

usage() { echo "usage: $0 init|up|check|down DIR" >&2; exit 2; }
[[ $# -eq 2 ]] || usage
command=$1
root=$2
[[ "$root" = /* ]] || { echo "DIR must be absolute" >&2; exit 2; }

load_env() {
  local line
  service_env=()
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" = \#* ]] && continue
    [[ "$line" =~ ^[A-Z_][A-Z_0-9]*= ]] || { echo "invalid environment entry" >&2; return 1; }
    service_env+=("$line")
  done < "$1"
}

value() { # value FILE NAME DEFAULT: one variable from an env file, without sourcing it
  local found
  found=$(grep -E "^$2=" "$1" 2>/dev/null | tail -1 | cut -d= -f2-) || true
  echo "${found:-$3}"
}

health() { # health NAME URL [TOKEN]
  local args=(-fsS -m 5 -o /dev/null)
  [[ -n "${3:-}" ]] && args+=(-H "Authorization: Bearer $3")
  if curl "${args[@]}" "$2"; then echo "$1: healthy"; else echo "$1: unhealthy" >&2; return 1; fi
}

start() { # start NAME ENV_FILE_OR_EMPTY BINARY [ARGS...] with extra env already exported
  local name=$1 envfile=$2 binary=$3
  shift 3
  if [[ -f "$root/run/$name.pid" ]] && kill -0 "$(cat "$root/run/$name.pid")" 2>/dev/null; then
    echo "$name: already running"; return
  fi
  command -v "$binary" >/dev/null || { echo "$name: $binary not on PATH" >&2; return 1; }
  if [[ -n "$envfile" ]]; then
    load_env "$envfile"
    env "${service_env[@]}" "$binary" "$@" >>"$root/logs/$name.log" 2>&1 &
  else
    "$binary" "$@" >>"$root/logs/$name.log" 2>&1 &
  fi
  echo $! >"$root/run/$name.pid"
}

wait_healthy() { # wait_healthy NAME URL [TOKEN]
  for _ in $(seq 1 50); do
    health "$@" 2>/dev/null && return
    kill -0 "$(cat "$root/run/$1.pid")" 2>/dev/null || { echo "$1: exited; see $root/logs/$1.log" >&2; return 1; }
    sleep 0.2
  done
  echo "$1: no health response; see $root/logs/$1.log" >&2
  return 1
}

case $command in
  init)
    mkdir -p "$root"/{relay/data,access/evidence,access/issuance,indexer/data,relayer/state,run,logs}
    chmod -R go-rwx "$root"
    if [[ ! -f "$root/relay.env" ]]; then
      cat >"$root/relay.env" <<EOF
EREBUS_RELAY_ROOT=$root/relay/data
EREBUS_RELAY_PORT=8080
EREBUS_RELAY_TOKEN=$(openssl rand -hex 32)
EREBUS_RELAY_RETENTION=604800
EOF
    fi
    echo "initialized $root; add access.json, indexer.env, or relayer.env to enable those services"
    ;;
  up)
    [[ -d "$root/run" ]] || { echo "run init first" >&2; exit 1; }
    if [[ -f "$root/relay.env" ]]; then
      start relay "$root/relay.env" erebus-relay
      wait_healthy relay "http://127.0.0.1:$(value "$root/relay.env" EREBUS_RELAY_PORT 8080)/healthz" "$(value "$root/relay.env" EREBUS_RELAY_TOKEN "")"
    fi
    if [[ -f "$root/access.json" ]]; then
      EREBUS_ACCESS_CONFIG="$root/access.json" start access "" erebus-access-service
      port=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["port"])' "$root/access.json")
      wait_healthy access "http://127.0.0.1:$port/healthz"
    fi
    if [[ -f "$root/indexer.env" ]]; then
      start indexer "$root/indexer.env" erebus_pool_indexer
      sleep 1
      kill -0 "$(cat "$root/run/indexer.pid")" || { echo "indexer: exited; see $root/logs/indexer.log" >&2; exit 1; }
      echo "indexer: running"
    fi
    # erebus-tx-relayer has no listener: a trusted host process runs it per request or
    # with --serve over stdin. `check` verifies its configuration.
    ;;
  check)
    status=0
    if [[ -f "$root/relay.env" ]]; then
      health relay "http://127.0.0.1:$(value "$root/relay.env" EREBUS_RELAY_PORT 8080)/healthz" "$(value "$root/relay.env" EREBUS_RELAY_TOKEN "")" || status=1
    fi
    if [[ -f "$root/access.json" ]]; then
      port=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["port"])' "$root/access.json")
      health access "http://127.0.0.1:$port/healthz" || status=1
    fi
    if [[ -f "$root/indexer.env" ]]; then
      kill -0 "$(cat "$root/run/indexer.pid" 2>/dev/null)" 2>/dev/null && echo "indexer: running" || { echo "indexer: not running" >&2; status=1; }
    fi
    if [[ -f "$root/relayer.env" ]]; then
      load_env "$root/relayer.env"
      reply=$(echo '{"method":"health"}' | env "${service_env[@]}" erebus-tx-relayer 2>/dev/null) || true
      if [[ "$reply" == *'"status":"ok"'* ]]; then echo "relayer: configured"; else echo "relayer: configuration rejected" >&2; status=1; fi
    fi
    exit $status
    ;;
  down)
    for pidfile in "$root"/run/*.pid; do
      [[ -f "$pidfile" ]] || continue
      kill "$(cat "$pidfile")" 2>/dev/null || true
      rm -f "$pidfile"
      echo "$(basename "$pidfile" .pid): stopped"
    done
    ;;
  *) usage ;;
esac
