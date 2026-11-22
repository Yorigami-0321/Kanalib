#!/usr/bin/env bash
# box 备份：ingest.db 在线备份 + 媒体目录硬链接快照 + 可选 rules.toml。
# 依赖：sqlite3, gzip, rsync, find, date。
set -euo pipefail

DB="${KANALIB_INGEST_DB:-/var/lib/kanalib-ingest/ingest.db}"
MEDIA="${KANALIB_INGEST_MEDIA:-/var/lib/kanalib-ingest/media}"
RULES="${KANALIB_RULES_FILE:-/etc/kanalib/rules.toml}"
DEST="${KANALIB_BACKUP_DIR:-/var/backups/kanalib}"
KEEP_DAYS="${KANALIB_BACKUP_KEEP_DAYS:-7}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"

if [[ ! -f "$DB" ]]; then
    echo "kanalib-ingest-backup: database not found: $DB" >&2
    exit 2
fi
if [[ "$DEST" == "/" || "$DEST" != /* ]]; then
    echo "kanalib-ingest-backup: refusing unsafe backup dir: $DEST" >&2
    exit 2
fi

mkdir -p "$DEST/ingest"
TARGET="$DEST/ingest/kanalib-ingest-$STAMP.db"
sqlite3 "$DB" ".backup '$TARGET'"
CHECK="$(sqlite3 "$TARGET" 'PRAGMA integrity_check;')"
if [[ "$CHECK" != "ok" ]]; then
    echo "kanalib-ingest-backup: integrity_check failed: $CHECK" >&2
    rm -f "$TARGET"
    exit 1
fi
gzip -9 "$TARGET"

if [[ -d "$MEDIA" ]]; then
    SNAPSHOT="$DEST/ingest/media-$STAMP"
    LINK_DEST="$DEST/ingest/media-latest"
    mkdir -p "$SNAPSHOT"
    if [[ -d "$LINK_DEST" ]]; then
        rsync -a --delete --link-dest="$LINK_DEST" "$MEDIA/" "$SNAPSHOT/"
    else
        rsync -a "$MEDIA/" "$SNAPSHOT/"
    fi
    ln -sfn "media-$STAMP" "$LINK_DEST"
else
    echo "kanalib-ingest-backup: media directory missing, skipped: $MEDIA" >&2
fi

if [[ -f "$RULES" ]]; then
    cp -p "$RULES" "$DEST/ingest/rules-$STAMP.toml"
fi

find "$DEST/ingest" -maxdepth 1 -type f -name 'kanalib-ingest-*.db.gz' \
    -mtime +"$KEEP_DAYS" -print -delete
find "$DEST/ingest" -maxdepth 1 -type d -name 'media-*' \
    -mtime +"$KEEP_DAYS" -print -exec rm -rf -- {} +

echo "kanalib-ingest-backup: wrote $TARGET.gz and media snapshot"
