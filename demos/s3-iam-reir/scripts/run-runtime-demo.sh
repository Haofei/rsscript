#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
DEMO_DIR="$ROOT_DIR/demos/s3-iam-reir"
LOG_DIR="$DEMO_DIR/review"
SERVER_LOG="$LOG_DIR/mock-s3-server.log"
RUN_OUT="$LOG_DIR/runtime-run"

mkdir -p "$LOG_DIR"
rm -rf "$RUN_OUT"

cargo run --manifest-path "$DEMO_DIR/native/rust/Cargo.toml" --bin mock_s3_server \
  > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

cleanup() {
  kill "$SERVER_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in {1..40}; do
  if grep -q "mock s3 server listening" "$SERVER_LOG"; then
    break
  fi
  sleep 0.1
done

RSS_S3_DEMO_ENDPOINT=127.0.0.1:39090 \
  cargo run --bin rss -- run "$DEMO_DIR" --out-dir "$RUN_OUT"

echo "mock s3 log: $SERVER_LOG"
