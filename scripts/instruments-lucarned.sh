#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-normal}"              # normal | attach | sign | isolated
DURATION="${2:-}"                # optional seconds for attach/isolated only
TEMPLATE="${TEMPLATE:-Allocations}" # Allocations | Leaks | Time Profiler

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT"

mkdir -p data/tmp data/instruments

ENT="data/tmp/get-task-allow.entitlements"
BIN="target/debug/lucarned"
SIGNED="data/tmp/lucarned-instruments"
TRACE="data/instruments/lucarned-${MODE}-${TEMPLATE// /-}-$(date +%Y%m%d-%H%M%S).trace"
LUCARNED_PID=""
XCTRACE_PID=""

cat > "$ENT" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
 "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.get-task-allow</key>
  <true/>
</dict>
</plist>
PLIST

build() {
  cargo +nightly build -p lucarned -Zbuild-dir-new-layout
}

sign_bin() {
  local path="$1"
  codesign --force --sign - --entitlements "$ENT" "$path"
}

time_args() {
  if [[ -n "$DURATION" ]]; then
    printf '%s\0%s\0' --time-limit "${DURATION}s"
  fi
}

print_saved() {
  if [[ -d "$TRACE" ]]; then
    echo "saved: $TRACE"
  fi
}

stop_xctrace() {
  if [[ -z "${XCTRACE_PID:-}" ]]; then
    print_saved
    return 0
  fi

  if kill -0 "$XCTRACE_PID" 2>/dev/null; then
    echo "stopping instruments pid=$XCTRACE_PID"
    kill -INT "$XCTRACE_PID" 2>/dev/null || true
  fi

  set +e
  wait "$XCTRACE_PID"
  local status=$?
  set -e
  if [[ $status -ne 0 && -d "$TRACE" ]]; then
    echo "xctrace exit=$status; trace exists"
  elif [[ $status -ne 0 ]]; then
    echo "xctrace exit=$status" >&2
  fi
  print_saved
}

on_signal() {
  trap - INT TERM
  echo
  echo "stopping lucarned pid=$LUCARNED_PID"
  if [[ -n "${LUCARNED_PID:-}" ]] && kill -0 "$LUCARNED_PID" 2>/dev/null; then
    kill -INT "$LUCARNED_PID" 2>/dev/null || true
  fi
  stop_xctrace
  if [[ -n "${LUCARNED_PID:-}" ]]; then
    wait "$LUCARNED_PID" 2>/dev/null || true
  fi
  exit 130
}

start_xctrace_attach_background() {
  local pid="$1"
  echo "instruments attach pid=$pid"
  echo "trace=$TRACE"
  xcrun xctrace record \
    --quiet \
    --template "$TEMPLATE" \
    --attach "$pid" \
    --output "$TRACE" \
    --no-prompt &
  XCTRACE_PID=$!

  sleep "${ATTACH_DELAY:-1}"
  if ! kill -0 "$XCTRACE_PID" 2>/dev/null; then
    set +e
    wait "$XCTRACE_PID"
    local status=$?
    set -e
    echo "xctrace exited early=$status; lucarned continues" >&2
    XCTRACE_PID=""
  fi
}

record_attach_foreground() {
  local pid="$1"
  local args=(--quiet --template "$TEMPLATE" --attach "$pid" --output "$TRACE" --no-prompt)
  if [[ -n "$DURATION" ]]; then
    args=(--quiet --template "$TEMPLATE" --attach "$pid" --time-limit "${DURATION}s" --output "$TRACE" --no-prompt)
  fi
  echo "attach pid=$pid"
  echo "trace=$TRACE"
  set +e
  xcrun xctrace record "${args[@]}"
  local status=$?
  set -e
  if [[ $status -ne 0 && -d "$TRACE" ]]; then
    echo "xctrace exit=$status; trace exists"
  elif [[ $status -ne 0 ]]; then
    exit "$status"
  fi
  print_saved
}

case "$MODE" in
  normal|run)
    build
    sign_bin "$BIN"

    echo "run signed lucarned in foreground"
    echo "binary=$BIN"
    "$PWD/$BIN" &
    LUCARNED_PID=$!
    trap on_signal INT TERM

    start_xctrace_attach_background "$LUCARNED_PID"

    set +e
    wait "$LUCARNED_PID"
    lucarned_status=$?
    set -e

    stop_xctrace
    echo "lucarned exited status=$lucarned_status"
    exit "$lucarned_status"
    ;;

  sign)
    build
    sign_bin "$BIN"
    echo "signed: $BIN"
    ;;

  attach)
    build
    sign_bin "$BIN"
    PID_TO_ATTACH="${PID:-$(pgrep -n lucarned || true)}"
    if [[ -z "$PID_TO_ATTACH" ]]; then
      echo "no running lucarned found" >&2
      exit 1
    fi
    echo "if entitlement error appears: restart lucarned from signed $BIN"
    record_attach_foreground "$PID_TO_ATTACH"
    ;;

  isolated|launch-isolated)
    build
    cp "$BIN" "$SIGNED"
    sign_bin "$SIGNED"

    CFG="data/tmp/lucarned-instruments-disabled.yaml"
    cat > "$CFG" <<'YAML'
channels:
  telegram:
    enabled: false
  wechat:
    enabled: false
YAML

    args=(--quiet --template "$TEMPLATE" --output "$TRACE" --env LUCARNE_CONFIG="$PWD/$CFG" --env LUCARNE_STATE_DB="$PWD/data/tmp/lucarned-instruments.sqlite3" --env LUCARNE_LOG_FILE="$PWD/data/tmp/lucarned-instruments.log" --env RUST_LOG="${RUST_LOG:-info}" --no-prompt --launch -- "$PWD/$SIGNED")
    if [[ -n "$DURATION" ]]; then
      args=(--quiet --template "$TEMPLATE" --time-limit "${DURATION}s" --output "$TRACE" --env LUCARNE_CONFIG="$PWD/$CFG" --env LUCARNE_STATE_DB="$PWD/data/tmp/lucarned-instruments.sqlite3" --env LUCARNE_LOG_FILE="$PWD/data/tmp/lucarned-instruments.log" --env RUST_LOG="${RUST_LOG:-info}" --no-prompt --launch -- "$PWD/$SIGNED")
    fi
    echo "launch signed isolated lucarned"
    echo "trace=$TRACE"
    set +e
    xcrun xctrace record "${args[@]}"
    status=$?
    set -e
    if [[ $status -ne 0 && -d "$TRACE" ]]; then
      echo "xctrace exit=$status; trace exists"
    elif [[ $status -ne 0 ]]; then
      exit "$status"
    fi
    print_saved
    ;;

  *)
    cat >&2 <<EOF
usage:
  $0                      # cargo-run-like: foreground lucarned, background Instruments, no time limit
  $0 attach [seconds]     # attach existing lucarned; optional xctrace time limit, target keeps running
  $0 sign                 # sign target/debug/lucarned with get-task-allow
  $0 isolated [seconds]   # isolated launch with Telegram/WeChat disabled

examples:
  $0
  TEMPLATE=Leaks $0
  PID=12345 $0 attach
EOF
    exit 2
    ;;
esac
