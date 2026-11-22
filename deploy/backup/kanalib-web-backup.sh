#!/usr/bin/env bash
# 网页 SQLite 在线备份：sqlite3 .backup + integrity_check + gzip + 保留 N 天。
# 依赖：sqlite3, gzip, find, date。建议由 systemd timer 每日运行。
set -euo pipefail

DB="${KANALIB_WEB_DB:-/var/lib/kanalib-web/kanalib-web.db}"
DEST="${KANALIB_BACKUP_DIR:-/var/backups/kanalib}"
KEEP_DAYS="${KANALIB_BACKUP_KEEP_DAYS:-7}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"

if [[ ! -f "$DB" ]]; then
    echo "kanalib-web-backup: database not found: $DB" >&2
    exit 2
fi
if [[ "$DEST" == "/" || "$DEST" != /* ]]; then
    echo "kanalib-web-backup: refusing unsafe backup dir: $DEST" >&2
    exit 2
fi

mkdir -p "$DEST/web"
TARGET="$DEST/web/kanalib-web-$STAMP.db"

sqlite3 "$DB" ".backup '$TARGET'"
CHECK="$(sqlite3 "$TARGET" 'PRAGMA integrity_check;')"
if [[ "$CHECK" != "ok" ]]; then
    echo "kanalib-web-backup: integrity_check failed: $CHECK" >&2
    rm -f "$TARGET"
    exit 1
fi

gzip -9 "$TARGET"
find "$DEST/web" -maxdepth 1 -type f -name 'kanalib-web-*.db.gz' \
    -mtime +"$KEEP_DAYS" -print -delete

echo "kanalib-web-backup: wrote $TARGET.gz (schema $(sqlite3 "$DB" 'PRAGMA user_version;'))"
