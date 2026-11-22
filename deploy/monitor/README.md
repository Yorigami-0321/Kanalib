# Monitoring

`kanalib-health.sh` 是一次性检查，适合 systemd timer / cron / 外部探活：

- public `/api/health`（默认 `127.0.0.1:8787`）。
- 可选 private `/api/health`（例如 EasyTier 地址 `10.126.126.2:8788`），只配
  `KANALIB_PRIVATE_HEALTH` 时才检查。
- 磁盘占用、推送积压/失败/拒绝、review 队列、下载队列。

非 0 退出表示需要处理。阈值全部走环境变量：

```text
KANALIB_PUBLIC_HEALTH=http://127.0.0.1:8787/api/health
KANALIB_PRIVATE_HEALTH=http://10.126.126.2:8788/api/health
KANALIB_WEB_DB=/var/lib/kanalib-web/kanalib-web.db
KANALIB_INGEST_DB=/var/lib/kanalib-ingest/ingest.db
KANALIB_DISK_PATH=/var/lib/kanalib-web
KANALIB_MAX_DISK_PCT=90
KANALIB_MAX_PUSH_BACKLOG=100
KANALIB_MAX_QUEUE=500
KANALIB_MAX_REVIEW=500
KANALIB_TLS_HOST=kanalib.example.com
KANALIB_TLS_MIN_DAYS=14
KANALIB_ALERT_WEBHOOK=
```

示例 timer 在 `deploy/systemd/kanalib-health.timer.example`。

- 配 `KANALIB_TLS_HOST` 时用 `openssl s_client` 检查证书剩余天数，低于
  `KANALIB_TLS_MIN_DAYS` 失败。
- 配 `KANALIB_ALERT_WEBHOOK` 时，只有检查失败才 POST 纯文本报告；webhook 地址只来自
  `/etc/kanalib/health.env`（0600），不写入仓库。没配就只靠 systemd 日志/`OnFailure=`。
