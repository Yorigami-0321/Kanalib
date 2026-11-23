#!/usr/bin/env bash
# 单次健康检查：HTTP、磁盘、推送积压、下载/审核队列。
# 退出码：0 健康，1 有硬失败，2 配置缺失。
# 可由 systemd timer 每 5 分钟运行；失败时可 POST 到 KANALIB_ALERT_WEBHOOK。
set -uo pipefail

PUBLIC_HEALTH="${KANALIB_PUBLIC_HEALTH-http://127.0.0.1:8787/api/health}"
PRIVATE_HEALTH="${KANALIB_PRIVATE_HEALTH-}"
INGEST_HEALTH="${KANALIB_INGEST_HEALTH-}"
KANALIB_INGEST_METRICS="${KANALIB_INGEST_METRICS:-}"
MAX_MODEL_ERROR_STREAK="${KANALIB_MAX_MODEL_ERROR_STREAK:-10}"
WEB_DB="${KANALIB_WEB_DB-/var/lib/kanalib-web/kanalib-web.db}"
INGEST_DB="${KANALIB_INGEST_DB-/var/lib/kanalib-ingest/ingest.db}"
DISK_PATH="${KANALIB_DISK_PATH:-/var/lib/kanalib-web}"
MAX_DISK_PCT="${KANALIB_MAX_DISK_PCT:-90}"
MAX_PUSH_BACKLOG="${KANALIB_MAX_PUSH_BACKLOG:-100}"
MAX_QUEUE="${KANALIB_MAX_QUEUE:-500}"
MAX_REVIEW="${KANALIB_MAX_REVIEW:-500}"
TLS_HOST="${KANALIB_TLS_HOST:-}"
TLS_MIN_DAYS="${KANALIB_TLS_MIN_DAYS:-14}"
ALERT_WEBHOOK="${KANALIB_ALERT_WEBHOOK:-}"

fail=0
REPORT=""
emit() { printf '%s\n' "$*"; REPORT+="$*"$'\n'; }
ok() { emit "ok   $*"; }
bad() { emit "FAIL $*"; fail=1; }
warn() { emit "warn $*"; }

check_http() {
    local name="$1" url="$2"
    if [[ -z "$url" ]]; then
        return 0
    fi
    if curl -fsS --max-time 5 "$url" | grep -q '"ok":true'; then
        ok "$name health"
    else
        bad "$name health: $url"
    fi
}

check_http public "$PUBLIC_HEALTH"
check_http private "$PRIVATE_HEALTH"
check_http ingest "$INGEST_HEALTH"

# 打标后端连续失败：model_error_streak 是 gauge，任一次成功即归零，
# 所以超阈值说明后端持续不可用，而不是历史累计。未配 URL 时跳过。
metrics_url="$KANALIB_INGEST_METRICS"
if [[ -z "$metrics_url" && -n "$INGEST_HEALTH" ]]; then
    metrics_url="${INGEST_HEALTH%/admin/api/health}/admin/api/metrics"
fi
if [[ -n "$metrics_url" ]]; then
    if streak="$(curl -fsS --max-time 5 "$metrics_url" \
            | sed -n 's/^model_error_streak \([0-9][0-9]*\)$/\1/p')"; then
        if [[ -z "$streak" ]]; then
            bad "model_error_streak missing from $metrics_url"
        elif (( streak > MAX_MODEL_ERROR_STREAK )); then
            bad "tagger consecutive failures ${streak} > ${MAX_MODEL_ERROR_STREAK}"
        else
            ok "tagger consecutive failures ${streak}"
        fi
    else
        bad "ingest metrics fetch failed: $metrics_url"
    fi
fi

if [[ -n "$TLS_HOST" ]]; then
    if command -v openssl >/dev/null 2>&1; then
        end_date="$(echo | openssl s_client -servername "$TLS_HOST" -connect "$TLS_HOST:443" 2>/dev/null \
            | openssl x509 -noout -enddate 2>/dev/null | cut -d= -f2)"
        end_epoch="$(date -d "$end_date" +%s 2>/dev/null || true)"
        if [[ -n "$end_epoch" ]]; then
            days=$(( (end_epoch - $(date +%s)) / 86400 ))
            if (( days < TLS_MIN_DAYS )); then
                bad "tls cert ${days}d < ${TLS_MIN_DAYS}d ($TLS_HOST)"
            else
                ok "tls cert ${days}d ($TLS_HOST)"
            fi
        else
            bad "tls cert check failed: $TLS_HOST"
        fi
    else
        warn "openssl missing; tls check skipped"
    fi
fi

if [[ -d "$DISK_PATH" ]]; then
    pct="$(df -P "$DISK_PATH" | awk 'NR==2 { gsub("%", "", $5); print $5 }')"
    if [[ -n "${pct:-}" ]] && (( pct > MAX_DISK_PCT )); then
        bad "disk ${pct}% > ${MAX_DISK_PCT}% at $DISK_PATH"
    else
        ok "disk ${pct:-?}% at $DISK_PATH"
    fi
else
    warn "disk path missing: $DISK_PATH"
fi

if [[ -n "$INGEST_DB" || -n "$WEB_DB" ]]; then
    if command -v sqlite3 >/dev/null 2>&1; then
        if [[ -n "$INGEST_DB" ]]; then
            if [[ -f "$INGEST_DB" ]]; then
                due=0 failed=0 rejected=0 review=0 queued=0
                if stats="$(sqlite3 -readonly "$INGEST_DB" "
                        SELECT
                          (SELECT COUNT(*) FROM push_batches
                            WHERE state='pending' AND (next_retry_at IS NULL OR next_retry_at <= strftime('%s','now'))),
                          (SELECT COUNT(*) FROM push_batches WHERE state='failed'),
                          (SELECT COUNT(*) FROM push_batches WHERE state='rejected'),
                          (SELECT COUNT(*) FROM items WHERE decision='review'),
                          (SELECT COUNT(*) FROM download_queue WHERE state IN ('queued','running','failed','paused'));
                    " 2>/dev/null)"; then
                    IFS='|' read -r due failed rejected review queued <<< "$stats"
                    (( due > MAX_PUSH_BACKLOG )) && bad "push backlog ${due} > ${MAX_PUSH_BACKLOG}" || ok "push backlog ${due}"
                    (( failed > 0 )) && bad "failed push batches ${failed}" || ok "failed push batches 0"
                    (( rejected > 0 )) && warn "rejected push batches ${rejected} (需要人工处理)"
                    (( review > MAX_REVIEW )) && bad "review queue ${review} > ${MAX_REVIEW}" || ok "review queue ${review}"
                    (( queued > MAX_QUEUE )) && bad "download queue ${queued} > ${MAX_QUEUE}" || ok "download queue ${queued}"
                else
                    bad "ingest db query failed: $INGEST_DB"
                fi
            else
                warn "ingest db missing: $INGEST_DB"
            fi
        fi

        if [[ -n "$WEB_DB" ]]; then
            if [[ -f "$WEB_DB" ]]; then
                items="$(sqlite3 -readonly "$WEB_DB" 'SELECT COUNT(*) FROM items WHERE deleted=0;' 2>/dev/null || echo '?')"
                ok "web live items ${items}"
            else
                warn "web db missing: $WEB_DB"
            fi
        fi
    else
        warn "sqlite3 not installed; queue/database checks skipped"
    fi
fi

if (( fail != 0 )) && [[ -n "$ALERT_WEBHOOK" ]]; then
    if printf '%s' "$REPORT" | curl -fsS --max-time 5 -X POST \
        -H 'Content-Type: text/plain' --data-binary @- "$ALERT_WEBHOOK" >/dev/null; then
        ok "alert webhook delivered"
    else
        warn "alert webhook failed"
    fi
fi

exit "$fail"
