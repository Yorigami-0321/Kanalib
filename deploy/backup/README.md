# Backups

两份脚本都做在线 SQLite 备份（`sqlite3 .backup`），不会阻塞读写；备份后跑
`PRAGMA integrity_check`，失败即删除该份并返回非 0。

| 脚本 | 内容 | 默认保留 |
|---|---|---|
| `kanalib-web-backup.sh` | 网页 `kanalib-web.db` | 7 天 |
| `kanalib-ingest-backup.sh` | `ingest.db`（含 rules/review/push_batches）、媒体硬链接快照、`rules.toml` | 7 天 |

环境变量：

```text
KANALIB_WEB_DB=/var/lib/kanalib-web/kanalib-web.db
KANALIB_INGEST_DB=/var/lib/kanalib-ingest/ingest.db
KANALIB_INGEST_MEDIA=/var/lib/kanalib-ingest/media
KANALIB_RULES_FILE=/etc/kanalib/rules.toml
KANALIB_BACKUP_DIR=/var/backups/kanalib
KANALIB_BACKUP_KEEP_DAYS=7
```

恢复：

1. 停服务。
2. `gunzip -c <backup>.db.gz > /var/lib/kanalib-web/kanalib-web.db`（ingest 同理）。
3. 确认属主与权限（`chown kanalib:kanalib`，`chmod 600`）。
4. 需要重放推送批次时，用 `kanalib-ingest --replay-push <BATCH_ID>`；批次内容已持久化在
   `push_batches.payload_json`，不会因重放改变。
5. 启动服务，先看 `/api/health` 与 `kanalib-health.sh`。

媒体快照用 `rsync --link-dest`，未变化的文件只占硬链接，不会每天复制整库。恢复媒体时直接
把对应 `media-<stamp>/` 同步回 `KANALIB_INGEST_MEDIA`。
