# Kanalib deployment runbook

这些文件是**模板**，不会自动安装。目标形态：

```text
公网浏览器 ──443──> Nginx ──> kanalib-web 127.0.0.1:8787（只读）
                                      │
box ──EasyTier 私网──> kanalib-web 10.126.126.2:8788（ingest / desktop / 原图）
                  └──> kanalib-ingest 10.126.126.3:8792（审核 / 规则草稿 / 指标）
kanalib-ingest ──HTTP──> tagger 10.126.126.1:8793（标签不足时补标，走 EasyTier，见 tagger/README.md）
桌面 ──EasyTier 私网──> 同上
```

## Layout

| 路径 | 用途 |
|---|---|
| `nginx/kanalib.conf.example` | TLS、静态站、公开只读反代，显式 404 私网路由 |
| `systemd/kanalib-web.service.example` | 网页后端 public + private 双 listener |
| `systemd/kanalib-ingest.service.example` | box 入库服务（Pixiv/mock + 规则 + 审核页） |
| `systemd/*backup*` | 每日 SQLite/媒体备份 |
| `systemd/kanalib-health.*` | 每 5 分钟健康检查 |
| `backup/*.sh` | 在线备份脚本 |
| `monitor/kanalib-health.sh` | HTTP/磁盘/队列检查 |
| `tagger/` | 本机 GPU 打标服务，直接绑 EasyTier 地址（过渡方案） |

## Install order

1. 建用户与目录：

   ```bash
   useradd --system --home /var/lib/kanalib --shell /usr/sbin/nologin kanalib
   install -d -o kanalib -g kanalib -m 0750 /var/lib/kanalib-web /var/lib/kanalib-ingest
   install -d -o root -g root -m 0755 /usr/local/sbin /etc/kanalib
   install -d -o kanalib -g kanalib -m 0700 /var/backups/kanalib
   ```

2. 部署二进制与静态包：

   ```bash
   install -m 0755 kanalib-web /usr/local/bin/kanalib-web
   install -m 0755 kanalib-ingest /usr/local/bin/kanalib-ingest
   install -m 0755 deploy/backup/kanalib-web-backup.sh /usr/local/sbin/
   install -m 0755 deploy/backup/kanalib-ingest-backup.sh /usr/local/sbin/
   install -m 0755 deploy/monitor/kanalib-health.sh /usr/local/sbin/
   # web/dist 解到 /var/lib/kanalib-web/dist，属主 kanalib
   ```

3. 写密钥文件（**0600，属主 kanalib**）：

   ```text
   /etc/kanalib/web.env
     KANALIB_INGEST_SECRET=<box 与网页共用；轮换时可写 new,old 逗号分隔>
     KANALIB_MEDIA_SIGNING_KEY=<原图签名密钥>

   /etc/kanalib/ingest.env
     KANALIB_INGEST_PUSH_SECRET=<与 web.env 的 ingest secret 相同>
   ```

   `KANALIB_INGEST_SECRET` 与 `KANALIB_MEDIA_SIGNING_KEY` 由程序直接读环境；不要写进 unit。
   轮换顺序：`new,old` → 重启 web → 切 box 到 `new` → 去掉 `old` → 再重启 web。

4. 安装 unit/timer，替换 `<...>`：

   ```bash
   cp deploy/systemd/kanalib-web.service.example /etc/systemd/system/kanalib-web.service
   cp deploy/systemd/kanalib-ingest.service.example /etc/systemd/system/kanalib-ingest.service
   cp deploy/systemd/kanalib-*-backup.{service,timer}.example /etc/systemd/system/
   cp deploy/systemd/kanalib-health.{service,timer}.example /etc/systemd/system/
   systemctl daemon-reload
   systemctl enable --now kanalib-web kanalib-ingest
   systemctl enable --now kanalib-web-backup.timer kanalib-ingest-backup.timer kanalib-health.timer
   systemctl enable --now kanalib-keepalive.timer   # 仅 box
   ```

5. Nginx 只反代 public listener；`/internal/`、`/api/desktop/`、
   `/api/items/<id>/original` 必须显式 404（模板已写）。

6. 首次设备令牌：

   ```bash
   sudo -u kanalib kanalib-web \
     --db /var/lib/kanalib-web/kanalib-web.db \
     --media /var/lib/kanalib-web/media \
     --issue-device desktop
   ```

   令牌只显示一次，数据库只存 SHA-256。

## Private listener

- `--bind-private` 必须是 EasyTier 虚拟 IP（如 `10.126.126.2:8788`），不要绑 `0.0.0.0`。
- 程序启动时检测到未指定地址会打 warning；生产日志里看到就说明配置错了。
- 公网 Nginx 不反代 private listener；EasyTier ACL 只放行 box/桌面到 8788。

## Box 入站防火墙

box 的 `cocoonbox` nftables 默认 `policy drop`，重启后会挡掉 EasyTier 入站。持久化放行写在
`/etc/nftables.conf` 的 `inet cocoonbox` input 链：

```nft
iifname "et0" ip protocol icmp accept
iifname "et0" tcp dport { 8790, 8792 } accept
```

改完 `sudo nft -f /etc/nftables.conf`，重启后从云端验证 `ping 10.126.126.3` 与
`curl http://10.126.126.3:8792/admin/api/health`。8790 是原图回源，8792 是管理口。
完整模板见 `deploy/box/nftables-cocoonbox.conf.example`。
必须用 `iifname`：`nftables.service` 在 EasyTier 建出 `et0` 之前加载，`iif "et0"` 会因
接口不存在导致整份规则加载失败。

## Box keepalive

`kanalib-keepalive.timer` 每分钟 ping 云端 `10.126.126.2`，连续 3 次失败重启
`kanalib-easytier`；本地 ingest 健康只记录，不触发网络重启，避免网页/代理短停误判。
同时屏蔽 OS 级休眠：

```bash
sudo systemctl mask sleep.target suspend.target hibernate.target hybrid-sleep.target
```

## Backup / restore / rollback

- 备份：`kanalib-web-backup.sh` 与 `kanalib-ingest-backup.sh` 默认每天 03:30/04:00，
  保留 7 天；媒体用 `rsync --link-dest` 硬链接快照。
- 恢复：停服务 → gunzip 覆盖 DB → `chown kanalib:kanalib && chmod 600` → 启动 →
  `kanalib-health.sh`。
- 推送重放：`kanalib-ingest --replay-push <BATCH_ID>`；批次内容来自
  `push_batches.payload_json`，重放不改内容。
- 回滚：保留上一版 `kanalib-web`/`kanalib-ingest` 二进制与 `web/dist`；数据库迁移前先备份，
  回滚二进制不自动回滚 schema（`user_version` 更高时旧二进制会拒绝启动）。

## Health

```bash
KANALIB_PUBLIC_HEALTH=http://127.0.0.1:8787/api/health \
KANALIB_PRIVATE_HEALTH=http://10.126.126.2:8788/api/health \
KANALIB_INGEST_HEALTH=http://10.126.126.3:8792/admin/api/health \
/usr/local/sbin/kanalib-health.sh
```

非 0 退出即需要处理。阈值见 `monitor/README.md`；可选
`KANALIB_TLS_HOST` 检查证书剩余天数，`KANALIB_ALERT_WEBHOOK` 在失败时 POST 报告。
webhook 只放 `/etc/kanalib/health.env`，不写入仓库。
`/admin/api/health` 不需要 token；`/admin/api/metrics` 需要，脚本会读取
`KANALIB_INGEST_ADMIN_TOKEN` 或默认的 `/etc/kanalib/admin-token`。

告警收件箱由 `kanalib-alert-sink.service` 提供：公网
`https://patchouli.us.ci/kanalib-alert/<token>`，box 用私网
`http://10.126.126.2:8795/kanalib-alert`。安装见 `deploy/monitor/README.md`。

## Current state

`kanalib-ingest.service.example` 通过 `--config /etc/kanalib/ingest.toml` 启动；来源由
`[source.pixiv].enabled` 或 `--source` 决定。规则热重载、审核页、规则草稿与 Prometheus
指标已接入。真实 Pixiv 令牌只放在 `/etc/kanalib`（0600），首次上线仍需要在 box 上做
一次手动联调（见 `docs/backlog/ingest-service.md` §12.3）。
