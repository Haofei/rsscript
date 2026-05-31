#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
DEMO_DIR="$ROOT_DIR/demos/s3-iam-reir"
LOG_DIR="$DEMO_DIR/review"
SERVER_LOG="$LOG_DIR/mock-s3-server.log"
RUN_OUT="$LOG_DIR/runtime-run"

OBJECTS="${RSS_S3_DEMO_OBJECTS:-6}"
PAYLOAD_BYTES="${RSS_S3_DEMO_PAYLOAD_BYTES:-262144}"
SERVER_DELAY_MS="${RSS_S3_DEMO_SERVER_DELAY_MS:-250}"

mkdir -p "$LOG_DIR"
rm -rf "$RUN_OUT"

echo "1. REIR preflight with missing IAM: expect failure"
cargo run --bin rss -- pkg review --reir "$DEMO_DIR" > "$LOG_DIR/rsscript.json"
"$DEMO_DIR/scripts/mock-iam-to-reir.sh" missing > "$LOG_DIR/iam-missing.json"
cargo run -p reir -- merge \
  "$LOG_DIR/rsscript.json" \
  "$LOG_DIR/iam-missing.json" \
  --out "$LOG_DIR/system-missing.json" >/dev/null

set +e
cargo run -p reir -- reconcile \
  --target prod \
  --out "$LOG_DIR/system-missing-reconciled.json" \
  "$LOG_DIR/system-missing.json" > "$LOG_DIR/reconcile-missing.log" 2>&1
MISSING_RC=$?
set -e
if [ "$MISSING_RC" -eq 0 ]; then
  echo "missing IAM unexpectedly passed" >&2
  cat "$LOG_DIR/reconcile-missing.log" >&2
  exit 1
fi
grep -E "status: fail|missing capability|s3:PutObject|required by:" "$LOG_DIR/reconcile-missing.log"

echo
echo "2. REIR preflight after IAM fix: expect pass"
"$DEMO_DIR/scripts/mock-iam-to-reir.sh" fixed > "$LOG_DIR/iam-fixed.json"
cargo run -p reir -- merge \
  "$LOG_DIR/rsscript.json" \
  "$LOG_DIR/iam-fixed.json" \
  --out "$LOG_DIR/system-fixed.json" >/dev/null
cargo run -p reir -- reconcile \
  --target prod \
  --out "$LOG_DIR/system-fixed-reconciled.json" \
  "$LOG_DIR/system-fixed.json" > "$LOG_DIR/reconcile-fixed.log"
grep -E "status: pass|covered capability|s3:PutObject|required by:" "$LOG_DIR/reconcile-fixed.log"

echo
echo "3. Start high-concurrency mock S3 server"
RSS_S3_DEMO_SERVER_DELAY_MS="$SERVER_DELAY_MS" \
  cargo run --manifest-path "$DEMO_DIR/native/rust/Cargo.toml" --bin mock_s3_server \
  > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

cleanup() {
  kill "$SERVER_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in {1..80}; do
  if grep -q "mock s3 server listening" "$SERVER_LOG"; then
    break
  fi
  sleep 0.1
done

echo "objects=$OBJECTS payload_bytes=$PAYLOAD_BYTES server_delay_ms=$SERVER_DELAY_MS"

echo
echo "4. Build and warm RSS async client backed by rsscript-runtime Tokio"
RSS_S3_DEMO_ENDPOINT=127.0.0.1:39090 \
RSS_S3_DEMO_OBJECTS="$OBJECTS" \
RSS_S3_DEMO_PAYLOAD_BYTES="$PAYLOAD_BYTES" \
  cargo run --bin rss -- run "$DEMO_DIR" --out-dir "$RUN_OUT" >/dev/null
cargo build --quiet --manifest-path "$RUN_OUT/Cargo.toml"

echo "5. Upload with warmed RSS async client"
ASYNC_START="$(python3 - <<'PY'
import time
print(time.perf_counter_ns())
PY
)"
RSS_S3_DEMO_ENDPOINT=127.0.0.1:39090 \
RSS_S3_DEMO_OBJECTS="$OBJECTS" \
RSS_S3_DEMO_PAYLOAD_BYTES="$PAYLOAD_BYTES" \
  cargo run --quiet --manifest-path "$RUN_OUT/Cargo.toml" >/dev/null
ASYNC_END="$(python3 - <<'PY'
import time
print(time.perf_counter_ns())
PY
)"
ASYNC_MS="$(( (ASYNC_END - ASYNC_START) / 1000000 ))"
echo "async rss client: ${ASYNC_MS} ms"

echo
echo "6. Upload with sync client using blocking TCP"
cargo build --quiet --manifest-path "$DEMO_DIR/native/rust/Cargo.toml" --bin sync_s3_client
SYNC_OUTPUT="$(
  RSS_S3_DEMO_ENDPOINT=127.0.0.1:39090 \
  RSS_S3_DEMO_OBJECTS="$OBJECTS" \
  RSS_S3_DEMO_PAYLOAD_BYTES="$PAYLOAD_BYTES" \
    cargo run --manifest-path "$DEMO_DIR/native/rust/Cargo.toml" --bin sync_s3_client
)"
echo "$SYNC_OUTPUT"

echo
echo "mock s3 log: $SERVER_LOG"
grep "stored object" "$SERVER_LOG" | tail -n "$(( OBJECTS * 3 ))"
