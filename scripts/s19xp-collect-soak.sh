#!/bin/sh
set -eu

BASE_LOG_DIR="${LOG_DIR:-/tmp/mujina-s19xp-soak}"
OUT_DIR="${OUT_DIR:-/tmp}"

if [ "${1:-}" = "" ]; then
    RUN_DIR="$(ls -1dt "$BASE_LOG_DIR"/* 2>/dev/null | head -n 1 || true)"
else
    RUN_DIR="$1"
fi

if [ -z "$RUN_DIR" ] || [ ! -d "$RUN_DIR" ]; then
    echo "No soak run directory found. Pass one explicitly, for example:" >&2
    echo "  $0 /tmp/mujina-s19xp-soak/20260529T220000Z" >&2
    exit 2
fi

name="$(basename "$RUN_DIR")"
archive="$OUT_DIR/mujina-s19xp-soak-$name.tar.gz"

tar -C "$(dirname "$RUN_DIR")" -czf "$archive" "$(basename "$RUN_DIR")"
sha256sum "$archive" >"$archive.sha256"

echo "$archive"
echo "$archive.sha256"
