#!/bin/sh
set -eu

BIN="${MUJINA_BIN:-./mujina-minerd}"
LOG_DIR="${LOG_DIR:-/tmp/mujina-s19xp-run}"
RUN_SECONDS="${RUN_SECONDS:-120}"
ASIC_TTY="${MUJINA_S19XP_ASIC_TTY:-/dev/ttyS1}"
STOP_LUXOS="${STOP_LUXOS:-no}"
CONFIRM="${CONFIRM_S19XP_RUN:-no}"

mkdir -p "$LOG_DIR"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
log="$LOG_DIR/mujina-$stamp.log"

if [ "$CONFIRM" != "yes" ]; then
    echo "Refusing to run: set CONFIRM_S19XP_RUN=yes when physically monitoring the miner." >&2
    exit 2
fi

if [ ! -x "$BIN" ]; then
    echo "Mujina binary is not executable: $BIN" >&2
    exit 2
fi

if pgrep -f '^/luxminer$' >/dev/null 2>&1 && [ "$STOP_LUXOS" != "yes" ]; then
    echo "LuxOS miner is running. Set STOP_LUXOS=yes to stop it for this timed handoff." >&2
    exit 2
fi

if [ "$STOP_LUXOS" = "yes" ]; then
    echo "Stopping LuxOS miner/watchdog by PID..."
    lux_pids="$(pgrep -f '^/luxminer$' || true) $(pgrep -f '^/luxupdate watch /luxminer$' || true)"
    for pid in $lux_pids; do
        kill -9 "$pid" 2>/dev/null || true
    done
    sleep 2
    if pgrep -f '^/luxminer$' >/dev/null 2>&1 || pgrep -f '^/luxupdate watch /luxminer$' >/dev/null 2>&1; then
        echo "LuxOS miner/watchdog is still running after PID kill; refusing handoff." >&2
        exit 2
    fi
fi

export MUJINA_USB_DISABLE="${MUJINA_USB_DISABLE:-1}"
export MUJINA_LOCAL_BOARD="${MUJINA_LOCAL_BOARD:-s19xp-amlogic}"
export MUJINA_S19XP_ASIC_TTY="$ASIC_TTY"
export MUJINA_S19XP_FREQUENCY_MHZ="${MUJINA_S19XP_FREQUENCY_MHZ:-110}"
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

echo "Starting timed Mujina handoff for ${RUN_SECONDS}s. Log: $log"
set +e
if command -v timeout >/dev/null 2>&1; then
    timeout "$RUN_SECONDS" "$BIN" >"$log" 2>&1
    status=$?
else
    "$BIN" >"$log" 2>&1 &
    mujina_pid=$!
    (
        sleep "$RUN_SECONDS"
        kill -TERM "$mujina_pid" 2>/dev/null
        sleep 5
        kill -KILL "$mujina_pid" 2>/dev/null
    ) &
    timer_pid=$!
    wait "$mujina_pid"
    status=$?
    kill "$timer_pid" 2>/dev/null
    wait "$timer_pid" 2>/dev/null
    if [ "$status" -gt 128 ]; then
        status=124
    fi
fi
set -e

echo "Mujina exited with status $status"
echo "Log: $log"

if [ "$STOP_LUXOS" = "yes" ]; then
    echo "Restarting LuxOS miner/watchdog..."
    if [ -x /etc/init.d/luxminer-init ]; then
        /etc/init.d/luxminer-init start || true
    fi
    sleep 2
    if ! pgrep -f '^/luxminer$' >/dev/null 2>&1; then
        /luxupdate watch /luxminer >/tmp/luxupdate-restart.log 2>&1 &
    fi
fi

exit "$status"
