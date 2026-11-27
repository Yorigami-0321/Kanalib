# Monitoring

`kanalib-health.sh` 是一次性检查，适合 systemd timer / cron / 外部探活：

- public `/api/health`（默认 `127.0.0.1:8787`）。
- 可选 private `/api/health`（例如 EasyTier 地址 `10.126.126.2:8788`），只配
  `KANALIB_PRIVATE_HEALTH` 时才检查。
- 可选 ingest 管理口 `/admin/api/health`（例如 EasyTier 地址 `10.126.126.3:8792`），
  只配 `KANALIB_INGEST_HEALTH` 时才检查。
- ingest `/admin/api/metrics` 的 `model_error_streak`（打标后端连续失败次数）；
  未配 `KANALIB_INGEST_METRICS` 时由 `KANALIB_INGEST_HEALTH` 推导，两者都没配则跳过。
  该值是 gauge，任一次打标成功即归零，所以超阈值代表后端持续不可用，不是历史累计。
  metrics 需要管理 token；脚本优先读 `KANALIB_INGEST_ADMIN_TOKEN`，否则读
  `KANALIB_INGEST_ADMIN_TOKEN_FILE`（默认 `/etc/kanalib/admin-token`）。
  `/admin/api/health` 是不带 token 的存活探针。
- 磁盘占用、推送积压/失败/拒绝、review 队列、下载队列。

非 0 退出表示需要处理。阈值全部走环境变量：

```text
KANALIB_PUBLIC_HEALTH=http://127.0.0.1:8787/api/health
KANALIB_PRIVATE_HEALTH=http://10.126.126.2:8788/api/health
KANALIB_INGEST_HEALTH=http://10.126.126.3:8792/admin/api/health
KANALIB_INGEST_METRICS=http://10.126.126.3:8792/admin/api/metrics
KANALIB_INGEST_ADMIN_TOKEN_FILE=/etc/kanalib/admin-token
KANALIB_MAX_MODEL_ERROR_STREAK=10
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

## 告警收件箱

`kanalib-alert-sink.py` 是一个只接收纯文本报告的小收件箱：

- 公网：`https://patchouli.us.ci/kanalib-alert/<token>`，Nginx 反代 `127.0.0.1:8794`。
- 私网：`http://10.126.126.2:8795/kanalib-alert`，只给 box 用，不需要 token。
- `POST text/plain` 追加到 `/var/lib/kanalib-alert/alerts.jsonl`，保留最近 500 条。
- 浏览器 GET 同一地址查看最近 50 条；公网必须带 token。
- token 放 `/etc/kanalib/alert.env`（0600），不写入仓库。

云端 `health.env` 填公网 URL，box 填私网 URL；两边共用同一个收件箱。
box 有全局 HTTP 代理，`health.env` 需加
`NO_PROXY=10.126.126.0/24,127.0.0.1,localhost`，否则私网 POST 会被代理返回 403。
