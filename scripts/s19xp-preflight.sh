#!/bin/sh
set -eu

OUT_DIR="${OUT_DIR:-/tmp/mujina-s19xp-preflight}"
mkdir -p "$OUT_DIR"

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out="$OUT_DIR/preflight-$stamp.txt"

{
    echo "== date =="
    date -u
    echo

    echo "== uname =="
    uname -a
    echo

    echo "== cmdline =="
    cat /proc/cmdline
    echo

    echo "== model =="
    tr -d '\0' </proc/device-tree/model 2>/dev/null || true
    echo
    echo

    echo "== processes =="
    ps w
    echo

    echo "== devices =="
    ls -l /dev/ttyS* /dev/i2c-* /dev/gpio* 2>/dev/null || true
    echo

    echo "== luxminer fds =="
    for pid in $(pgrep -f '^/luxminer$' 2>/dev/null || true); do
        echo "-- pid $pid --"
        ls -l "/proc/$pid/fd" 2>/dev/null || true
    done
    echo

    echo "== dmesg uart/gpio/i2c =="
    dmesg | grep -Ei 'uart|tty|gpio|i2c|hashboard|asic|bm|fan|pwm' | tail -n 240 || true
    echo

    echo "== gpio sysfs =="
    for g in /sys/class/gpio/gpio[0-9]*; do
        [ -d "$g" ] || continue
        echo "$g"
        for f in direction value active_low edge; do
            [ -r "$g/$f" ] || continue
            printf '  %s=' "$f"
            cat "$g/$f"
        done
    done
    echo

    echo "== luxos config summary =="
    sed -n '1,220p' /config/luxminer.conf.d/profile.toml 2>/dev/null || true
    sed -n '1,180p' /config/luxminer.conf.d/hashboard.toml 2>/dev/null || true
    sed -n '1,180p' /config/luxminer.conf.d/fancontrol.toml 2>/dev/null || true
    sed -n '1,180p' /config/luxminer.conf.d/tempcontrol.toml 2>/dev/null || true
    echo

    echo "== latest luxminer log tail =="
    latest_log="$(ls -1t /persistent/miner/luxminer-*.log 2>/dev/null | head -n 1 || true)"
    if [ -n "$latest_log" ]; then
        echo "$latest_log"
        tail -n 220 "$latest_log" 2>/dev/null || true
    fi
} >"$out" 2>&1

echo "$out"
