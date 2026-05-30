#!/bin/sh
set -eu

BIN="${MUJINA_BIN:-./mujina-minerd}"
BASE_LOG_DIR="${LOG_DIR:-/tmp/mujina-s19xp-soak}"
ASIC_TTY="${MUJINA_S19XP_ASIC_TTY:-/dev/ttyS1}"
STOP_LUXOS="${STOP_LUXOS:-yes}"
STOP_METHOD="${STOP_METHOD:-graceful}"
RESTORE_LUXOS="${RESTORE_LUXOS:-yes}"
CONFIRM="${CONFIRM_S19XP_RUN:-no}"
SNAPSHOT_INTERVAL="${SNAPSHOT_INTERVAL:-30}"
API_URL="${MUJINA_API_URL:-http://127.0.0.1:7785/api/v0/miner}"

if [ "$CONFIRM" != "yes" ]; then
    echo "Refusing to run: set CONFIRM_S19XP_RUN=yes when physically monitoring the miner." >&2
    exit 2
fi

if [ ! -x "$BIN" ]; then
    echo "Mujina binary is not executable: $BIN" >&2
    exit 2
fi

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="$BASE_LOG_DIR/$stamp"
mkdir -p "$RUN_DIR"

MUJINA_LOG="$RUN_DIR/mujina.log"
SNAPSHOT_LOG="$RUN_DIR/snapshots.log"
ENV_LOG="$RUN_DIR/environment.txt"
PID_FILE="$RUN_DIR/mujina.pid"

luxos_was_stopped="no"
mujina_pid=""
monitor_pid=""
stopping="no"

stop_luxos() {
    if [ "$STOP_LUXOS" != "yes" ]; then
        return
    fi

    if [ "$STOP_METHOD" = "graceful" ] && [ -x /etc/init.d/luxminer-init ]; then
        echo "Stopping LuxOS miner/watchdog gracefully..."
        /etc/init.d/luxminer-init stop >>"$RUN_DIR/luxos-stop.log" 2>&1 || true
        sleep 5
    fi

    if pgrep -f '^/luxminer$' >/dev/null 2>&1 || pgrep -f '^/luxupdate watch /luxminer$' >/dev/null 2>&1; then
        echo "Stopping remaining LuxOS miner/watchdog processes by PID..."
        lux_pids="$(pgrep -f '^/luxminer$' || true) $(pgrep -f '^/luxupdate watch /luxminer$' || true)"
        for pid in $lux_pids; do
            kill -TERM "$pid" 2>/dev/null || true
        done
        sleep 3
        lux_pids="$(pgrep -f '^/luxminer$' || true) $(pgrep -f '^/luxupdate watch /luxminer$' || true)"
        for pid in $lux_pids; do
            kill -KILL "$pid" 2>/dev/null || true
        done
        sleep 2
    fi

    if pgrep -f '^/luxminer$' >/dev/null 2>&1 || pgrep -f '^/luxupdate watch /luxminer$' >/dev/null 2>&1; then
        echo "LuxOS miner/watchdog is still running; refusing soak." >&2
        exit 2
    fi

    luxos_was_stopped="yes"
}

restore_luxos() {
    if [ "$RESTORE_LUXOS" != "yes" ] || [ "$luxos_was_stopped" != "yes" ]; then
        return
    fi

    echo "Restarting LuxOS miner/watchdog..."
    if [ -x /etc/init.d/luxminer-init ]; then
        /etc/init.d/luxminer-init start >>"$RUN_DIR/luxos-restore.log" 2>&1 || true
    fi
    sleep 3
    if ! pgrep -f '^/luxminer$' >/dev/null 2>&1; then
        /luxupdate watch /luxminer >>"$RUN_DIR/luxos-restore.log" 2>&1 &
    fi
}

snapshot_once() {
    now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    {
        echo "===== $now ====="
        echo "[processes]"
        ps w | grep -E 'luxminer|luxupdate|mujina' | grep -v grep || true
        echo "[gpio]"
        for gpio in 437 456; do
            if [ -e "/sys/class/gpio/gpio$gpio/value" ]; then
                printf 'gpio%s=' "$gpio"
                cat "/sys/class/gpio/gpio$gpio/value" || true
            else
                echo "gpio$gpio=unexported"
            fi
        done
        echo "[thermal]"
        for zone in /sys/class/thermal/thermal_zone*; do
            [ -e "$zone/temp" ] || continue
            name="$(cat "$zone/type" 2>/dev/null || basename "$zone")"
            temp="$(cat "$zone/temp" 2>/dev/null || true)"
            echo "$name=$temp"
        done
        echo "[api]"
        if command -v curl >/dev/null 2>&1; then
            curl -fsS --max-time 3 "$API_URL" || true
            echo
        else
            echo "curl unavailable"
        fi
        echo
    } >>"$SNAPSHOT_LOG" 2>&1
}

monitor_loop() {
    while :; do
        snapshot_once
        sleep "$SNAPSHOT_INTERVAL" || true
    done
}

cleanup() {
    if [ "$stopping" = "yes" ]; then
        return
    fi
    stopping="yes"

    echo "Stopping soak and collecting final snapshot..."
    snapshot_once || true

    if [ -n "$monitor_pid" ]; then
        kill "$monitor_pid" 2>/dev/null || true
        wait "$monitor_pid" 2>/dev/null || true
    fi

    if [ -n "$mujina_pid" ] && kill -0 "$mujina_pid" 2>/dev/null; then
        kill -TERM "$mujina_pid" 2>/dev/null || true
        sleep 8
        kill -KILL "$mujina_pid" 2>/dev/null || true
        wait "$mujina_pid" 2>/dev/null || true
    fi

    restore_luxos

    {
        echo "ended_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo "run_dir=$RUN_DIR"
    } >>"$ENV_LOG"

    echo "Soak logs are in: $RUN_DIR"
}

trap cleanup INT TERM EXIT

export MUJINA_USB_DISABLE="${MUJINA_USB_DISABLE:-1}"
export MUJINA_LOCAL_BOARD="${MUJINA_LOCAL_BOARD:-s19xp-amlogic}"
export MUJINA_S19XP_ASIC_TTY="$ASIC_TTY"
export MUJINA_S19XP_FREQUENCY_MHZ="${MUJINA_S19XP_FREQUENCY_MHZ:-100}"
export MUJINA_S19XP_EXPECTED_CHIPS="${MUJINA_S19XP_EXPECTED_CHIPS:-110}"
export MUJINA_S19XP_STARTUP_BAUD="${MUJINA_S19XP_STARTUP_BAUD:-115200}"
export MUJINA_S19XP_BAUD="${MUJINA_S19XP_BAUD:-115200}"
export MUJINA_S19XP_APW12_STARTUP="${MUJINA_S19XP_APW12_STARTUP:-true}"
export MUJINA_S19XP_APW12_I2C_DEVICE="${MUJINA_S19XP_APW12_I2C_DEVICE:-/dev/i2c-1}"
export MUJINA_S19XP_APW12_I2C_ADDRESS="${MUJINA_S19XP_APW12_I2C_ADDRESS:-0x10}"
export MUJINA_S19XP_APW12_TARGET_VOLTAGE="${MUJINA_S19XP_APW12_TARGET_VOLTAGE:-12.0}"
export MUJINA_S19XP_PS_ENABLE_GPIO="${MUJINA_S19XP_PS_ENABLE_GPIO:-437}"
export MUJINA_S19XP_PS_ENABLE_ACTIVE_HIGH="${MUJINA_S19XP_PS_ENABLE_ACTIVE_HIGH:-false}"
export MUJINA_S19XP_PS_ENABLE_DELAY_MS="${MUJINA_S19XP_PS_ENABLE_DELAY_MS:-2000}"
export MUJINA_S19XP_CHAIN_RESET_GPIO="${MUJINA_S19XP_CHAIN_RESET_GPIO:-456}"
export MUJINA_POOL_URL="${MUJINA_POOL_URL:-stratum+tcp://pool.256foundation.org:3333}"
export MUJINA_POOL_USER="${MUJINA_POOL_USER:-AgentP.Mujina_on_S19XP}"
export MUJINA_POOL_PASS="${MUJINA_POOL_PASS:-x}"
export MUJINA_API_LISTEN="${MUJINA_API_LISTEN:-127.0.0.1:7785}"
export RUST_LOG="${RUST_LOG:-mujina_miner=debug,mujina_miner::asic::bm13xx=trace}"

{
    echo "started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "run_dir=$RUN_DIR"
    echo "bin=$BIN"
    echo "stop_luxos=$STOP_LUXOS"
    echo "stop_method=$STOP_METHOD"
    echo "restore_luxos=$RESTORE_LUXOS"
    echo "snapshot_interval=$SNAPSHOT_INTERVAL"
    env | sort | grep -E '^(MUJINA|RUST_LOG|STOP_|RESTORE_|SNAPSHOT_)' || true
} >"$ENV_LOG"

stop_luxos
snapshot_once

echo "Starting Mujina soak. Press Ctrl-C to stop and restore LuxOS."
echo "Run directory: $RUN_DIR"

monitor_loop &
monitor_pid=$!

"$BIN" >"$MUJINA_LOG" 2>&1 &
mujina_pid=$!
echo "$mujina_pid" >"$PID_FILE"

set +e
wait "$mujina_pid"
mujina_status=$?
set -e
mujina_pid=""
echo "Mujina exited with status $mujina_status"
exit "$mujina_status"
