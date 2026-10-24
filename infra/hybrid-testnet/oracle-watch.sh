#!/bin/bash
# Restart the Oracle validator when its committed round stops advancing for
# three consecutive checks (timer: every minute). systemd Restart=always only
# covers a crash, not a hang. The round lives in /run so a reboot resets it.
set -u
f=/run/maya2c-watch.last; n=/run/maya2c-watch.stuck
now=$(curl -s -m5 -X POST localhost:8545 -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"get_bft_status","params":[]}' | grep -o '"committed_round":[0-9]*' | cut -d: -f2)
last=$(cat "$f" 2>/dev/null || echo -1); stuck=$(cat "$n" 2>/dev/null || echo 0)
if [ -n "$now" ] && [ "$now" != "$last" ]; then stuck=0; else stuck=$((stuck + 1)); fi
echo "${now:--1}" > "$f"; echo "$stuck" > "$n"
if [ "$stuck" -ge 3 ]; then
  logger -t maya2c-watch "committed_round stuck at ${now:-unreachable} for $stuck checks: restarting v1"
  systemctl restart maya2c-validator@1; echo 0 > "$n"
fi
