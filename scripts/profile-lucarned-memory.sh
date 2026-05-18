#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

OUT_DIR=${LUCARNE_MEMORY_PROFILE_DIR:-"$ROOT/target/memory-profiles/lucarned-$(date +%Y%m%d-%H%M%S)"}
RUN_DIR="$OUT_DIR/run"
PROFILE_TARGET_DIR=${LUCARNE_MEMORY_PROFILE_TARGET_DIR:-"$OUT_DIR/cargo-target"}
BIN=${LUCARNE_PROFILE_BIN:-"$PROFILE_TARGET_DIR/release/lucarned"}
mkdir -p "$RUN_DIR"

if [[ "${LUCARNE_PROFILE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo +nightly build \
    -Zbuild-dir-new-layout \
    --release \
    -p lucarned \
    --features memory-profiling \
    --target-dir "$PROFILE_TARGET_DIR"
elif [[ ! -x "$BIN" ]]; then
  echo "profiling binary not found: $BIN" >&2
  echo "set LUCARNE_PROFILE_BIN or unset LUCARNE_PROFILE_SKIP_BUILD" >&2
  exit 1
fi

PROFILE_MODE=${LUCARNE_MEMORY_PROFILE_MODE:-real}
case "$PROFILE_MODE" in
  real)
    # Real mode intentionally leaves daemon configuration alone. lucarned may load
    # .env and production defaults exactly like normal startup.
    ;;
  synthetic)
    if [[ -z "${LUCARNE_CONFIG:-}" && -z "${LUCARNED_CONFIG:-}" ]]; then
      cat >"$RUN_DIR/lucarned.yaml" <<'YAML'
channels:
  telegram:
    enabled: true
  wechat:
    enabled: false
YAML
      export LUCARNE_CONFIG="$RUN_DIR/lucarned.yaml"
    fi

    export LUCARNE_STATE_DB=${LUCARNE_STATE_DB:-"$RUN_DIR/state.sqlite3"}
    export LUCARNE_LOG_FILE=${LUCARNE_LOG_FILE:-"$RUN_DIR/lucarned.log"}
    export RUST_LOG=${RUST_LOG:-error}
    export TELEGRAM_BOT_TOKEN=${TELEGRAM_BOT_TOKEN:-"000000:invalid"}
    export TELEGRAM_CHAT_ID=${TELEGRAM_CHAT_ID:-"0"}
    ;;
  *)
    echo "invalid LUCARNE_MEMORY_PROFILE_MODE=$PROFILE_MODE (expected real or synthetic)" >&2
    exit 1
    ;;
esac
export LUCARNE_MEMORY_PROFILE_PAUSE_MS=${LUCARNE_MEMORY_PROFILE_PAUSE_MS:-750}

STDOUT_LOG="$OUT_DIR/stdout.log"
STDERR_LOG="$OUT_DIR/stderr.log"
SUMMARY_CSV="$OUT_DIR/summary.csv"
printf 'label,rss_kb,heap_live_bytes\n' >"$SUMMARY_CSV"

labels=(
  lucarned.main.start
  lucarned.main.after_dotenv
  lucarned.init_tracing.start
  lucarned.init_tracing.after_filters
  lucarned.init_tracing.before_file_appender
  lucarned.init_tracing.after_file_appender
  lucarned.init_tracing.after_nonblocking
  lucarned.init_tracing.after_layers
  lucarned.init_tracing.after_try_init
  lucarned.main.after_tracing
  lucarned.main.after_config_load
  lucarned.main.after_register_adapters
  lucarned.main.before_open_sqlite
  lucarne.core.open_sqlite.start
  lucarne.core.open_sqlite.after_runtime_new
  lucarne.core.open_sqlite.after_register_defaults
  lucarne.core.open_sqlite.after_store_open
  lucarne.core.from_runtime_and_store.start
  lucarne.core.from_runtime_and_store.after_load_control_plane
  lucarne.core.from_runtime_and_store.after_provider_ids
  lucarned.main.after_open_sqlite
  lucarned.main.before_supervise_enabled
  lucarne_adapter.history_watch.wait_start
  lucarne_wechat.adapter.spawn.start
  lucarne_wechat.adapter.spawn.before_transport_new
  lucarne_wechat.adapter.spawn.after_transport_new
  lucarne_wechat.adapter.spawn.after_login
  lucarne_wechat.adapter.spawn.after_known_user_ids
  lucarne_wechat.adapter.spawn.before_task_spawn
  lucarne_telegram.adapter.spawn.start
  lucarne_telegram.adapter.spawn.before_task_spawn
  lucarned.main.after_supervise_enabled
  lucarne_telegram.adapter.run.start
  lucarne_telegram.channel.start.start
  lucarne_telegram.channel.start.after_bot_new
  lucarne_telegram.channel.start.after_poll_spawn
  lucarne_telegram.adapter.run.after_channel_start
  lucarne_telegram.channel.poll_updates.start
  lucarned.main.before_wait
  lucarne_telegram.adapter.run.after_sync_commands
  lucarne_telegram.adapter.run.after_state_new
  lucarne_telegram.adapter.run.after_bot_new
  lucarne_telegram.adapter.run.before_bot_run
  lucarne_telegram.bot.run.start
  lucarne_telegram.bot.run.after_watch_events_subscribe
  lucarne_telegram.bot.run.after_core_watcher_spawn
  lucarne_adapter.history_watch.after_subscriber
  lucarne.core.start_history_session_watch.start
  lucarne.core.start_history_session_watch.after_started_flag
  lucarne.core.start_history_watcher_once.start
  agent_sessions.watch.start
  agent_sessions.watch.after_recommended_watcher
  agent_sessions.watch.after_roots
  agent_sessions.watch.after_initialize_baselines
  lucarne.core.start_history_watcher_once.after_watcher_start
  lucarne.core.start_history_session_watch.after_initial_watcher
  lucarne.core.start_history_session_watch.after_spawn_loop
  lucarne_telegram.bot.run.after_channel_subscribe
)

sanitize_label() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9_.-' '_'
}

heap_live_bytes() {
  sed -n 's/^All zones: .* nodes (\([0-9][0-9]*\) bytes).*/\1/p' "$1" | head -1
}

collect_snapshot() {
  local label=$1
  local safe
  safe=$(sanitize_label "$label")
  local stopped=0
  local rss

  if [[ "${LUCARNE_PROFILE_STOP_THE_WORLD:-1}" == "1" ]] && kill -0 "$PID" 2>/dev/null; then
    kill -STOP "$PID" 2>/dev/null && stopped=1 || stopped=0
  fi

  rss=$(ps -o rss= -p "$PID" 2>/dev/null | awk '{print $1}' || true)

  ps -o pid,rss,vsz,command -p "$PID" >"$OUT_DIR/$safe.ps.txt" 2>&1 || true
  vmmap -summary "$PID" >"$OUT_DIR/$safe.vmmap.txt" 2>&1 || true
  heap -s "$PID" >"$OUT_DIR/$safe.heap.txt" 2>&1 || true

  if [[ "$stopped" == "1" ]]; then
    kill -CONT "$PID" 2>/dev/null || true
  fi

  local heap_live
  heap_live=$(heap_live_bytes "$OUT_DIR/$safe.heap.txt" || true)
  printf '%s,%s,%s\n' "$label" "${rss:-}" "${heap_live:-}" >>"$SUMMARY_CSV"
  printf '%-48s rss=%sKB heap_live=%s\n' "$label" "${rss:-?}" "${heap_live:-?}"
}

wait_for_label() {
  local label=$1
  local timeout=${LUCARNE_PROFILE_LABEL_TIMEOUT_SECS:-20}
  local deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    if grep -q "label=$label" "$STDERR_LOG" 2>/dev/null; then
      return 0
    fi
    if ! kill -0 "$PID" 2>/dev/null; then
      echo "lucarned exited before label=$label" >&2
      return 1
    fi
    sleep 0.1
  done
  echo "timeout waiting for label=$label" >&2
  return 1
}

"$BIN" >"$STDOUT_LOG" 2>"$STDERR_LOG" &
PID=$!
trap 'kill "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true' EXIT

echo "profile_dir=$OUT_DIR"
echo "mode=$PROFILE_MODE"
echo "binary=$BIN"
echo "pid=$PID"

for label in "${labels[@]}"; do
  if wait_for_label "$label"; then
    collect_snapshot "$label"
  else
    break
  fi
done

sleep "${LUCARNE_PROFILE_SETTLE_SECS:-5}"
if kill -0 "$PID" 2>/dev/null; then
  collect_snapshot "lucarned.settled.${LUCARNE_PROFILE_SETTLE_SECS:-5}s"
fi

if [[ "${LUCARNE_PROFILE_LEAKS:-0}" == "1" ]] && kill -0 "$PID" 2>/dev/null; then
  leaks "$PID" --quiet >"$OUT_DIR/leaks.txt" 2>&1 || true
fi

echo "summary=$SUMMARY_CSV"
