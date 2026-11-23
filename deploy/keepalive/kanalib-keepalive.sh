#!/usr/bin/env bash
# box 侧链路保活：每分钟检查云端私网健康；连续失败时重启 EasyTier 实例。
# 只统计 peer 失败，本地 ingest 健康仅用于日志，不触发网络重启。
set -uo pipefail

PEER_HEALTH="${KANALIB_KEEPALIVE_URL:-http://10.126.126.2:8788/api/health}"
LOCAL_HEALTH="${KANALIB_KEEPALIVE_LOCAL:-http://10.126.126.3:8792/admin/api/health}"
STATE="${KANALIB_KEEPALIVE_STATE:-/run/kanalib-keepalive.failures}"
THRESHOLD="${KANALIB_KEEPALIVE_THRESHOLD:-3}"

peer_ok=1
local_ok=1
curl -fsS --max-time 5 "$PEER_HEALTH" 2>/dev/null | grep -q '"ok":true' && peer_ok=0
curl -fsS --max-time 5 "$LOCAL_HEALTH" 2>/dev/null | grep -q '"ok":true' && local_ok=0

if (( peer_ok == 0 && local_ok == 0 )); then
    rm -f "$STATE"
    exit 0
fi

count=0
if [[ -f "$STATE" ]]; then
    count="$(cat "$STATE" 2>/dev/null || echo 0)"
fi
case "$count" in
    ''|*[!0-9]*) count=0 ;;
esac

echo "keepalive peer=$peer_ok local=$local_ok" >&2

if (( peer_ok == 0 )); then
    # 本地正常但云端不可达，按连续失败计数；本地异常已由 systemd 负责。
    count=$((count + 1))
    printf '%s\n' "$count" > "$STATE"
    echo "peer failure ${count}/${THRESHOLD}" >&2
    if (( count >= THRESHOLD )); then
        echo "restarting kanalib-easytier after ${count} peer failures" >&2
        systemctl restart kanalib-easytier || true
        rm -f "$STATE"
    fi
fi

exit 1
