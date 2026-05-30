#!/bin/sh
set -eu

TARGET="${TARGET:-aarch64-unknown-linux-gnu}"
PROFILE_DIR="${PROFILE_DIR:-release}"
BIN="target/$TARGET/$PROFILE_DIR/mujina-minerd"
OUT_DIR="${OUT_DIR:-target/s19xp-amlogic}"

if [ ! -x "$BIN" ]; then
    echo "Missing binary: $BIN" >&2
    echo "Build it first, for example:" >&2
    echo "  cargo build -p mujina-miner --bin mujina-minerd --target $TARGET --release --no-default-features" >&2
    exit 2
fi

mkdir -p "$OUT_DIR/scripts"
cp "$BIN" "$OUT_DIR/mujina-minerd"
cp scripts/s19xp-preflight.sh "$OUT_DIR/scripts/"
cp scripts/s19xp-run-mujina.sh "$OUT_DIR/scripts/"
cp scripts/s19xp-soak.sh "$OUT_DIR/scripts/"
cp scripts/s19xp-collect-soak.sh "$OUT_DIR/scripts/"
cp scripts/s19xp-rollback-luxos.sh "$OUT_DIR/scripts/"
chmod +x "$OUT_DIR/mujina-minerd" "$OUT_DIR"/scripts/*.sh

tarball="$OUT_DIR.tar.gz"
tar -C "$(dirname "$OUT_DIR")" -czf "$tarball" "$(basename "$OUT_DIR")"

echo "$tarball"
