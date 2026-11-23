#!/usr/bin/env bash
# box 侧链路保活：每分钟 ping 云端私网地址；连续失败时重启 EasyTier 实例。
# 网络用 ICMP 判断，本地 ingest 健康仅用于日志，不触发网络重启；
# 云端网页/代理短停不会导致误重启。
set -uo pipefail

PEER="${KANALIB_KEEPALIVE_PEER:-10.126.126.2}"
PEER_TIMEOUT="${KANALIB_KEEPALIVE_TIMEOUT:-3}"
LOCAL_HEALTH="${KANALIB_KEEPALIVE_LOCAL:-http://10.126.126.3:8792/admin/api/health}"
STATE="${KANALIB_KEEPALIVE_STATE:-/run/kanalib-keepalive.failures}"
THRESHOLD="${KANALIB_KEEPALIVE_THRESHOLD:-3}"

peer_ok=1
local_ok=1
ping -c 1 -W "$PEER_TIMEOUT" "$PEER" >/dev/null 2>&1 && peer_ok=0
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

echo "keepalive peer=$PEER peer_ok=$peer_ok local_ok=$local_ok" >&2

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
