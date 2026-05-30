#!/bin/sh
set -eu

echo "Stopping Mujina if it is running..."
pkill -TERM -f 'mujina-minerd' 2>/dev/null || true
sleep 3
pkill -KILL -f 'mujina-minerd' 2>/dev/null || true

echo "Starting LuxOS miner/watchdog..."
/etc/init.d/luxminer-init start || /etc/init.d/luxminer-init restart

echo "Current miner processes:"
ps w | grep -E 'luxminer|luxupdate|mujina' | grep -v grep || true
