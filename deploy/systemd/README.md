# Deployment drafts

These files are prepared, not installed. Replace every `<...>` placeholder, copy the `.example`
files without the suffix, and keep secrets in protected local files. The private listener and
network identity are intentionally supplied by deployment configuration; application code and
business configuration contain no virtual-network IP addresses.

## Web write routes (W4)

`kanalib-web` 的写路由只存在于 private listener。生产上：

- `--bind-private` 必须是 EasyTier 虚拟 IP（例如 `10.126.126.2:8788`），**不要绑 `0.0.0.0`**；
  启动时检测到未指定地址会打 warning。
- 共享密钥放 `/etc/kanalib/web.env`（`0600`，属主 `kanalib`）：

  ```text
  KANALIB_INGEST_SECRET=<box 与网页共用的 HMAC 密钥>
  KANALIB_MEDIA_SIGNING_KEY=<原图签名密钥>
  ```

  两项都由进程环境读取，不写进 unit 文件或仓库。
- 原图回源是可选项；需要时在 `ExecStart` 追加
  `--box-origin http://10.126.126.3:8789`。未配置时 `/api/items/{id}/original` 返回 404，
  前端回退 t2048。
- 桌面设备令牌只生成一次：

  ```bash
  sudo -u kanalib kanalib-web \
    --db /var/lib/kanalib-web/kanalib-web.db \
    --media /var/lib/kanalib-web/media \
    --issue-device desktop
  ```

  命令打印 `device_token` 后退出；数据库只存 SHA-256。撤销用
  `UPDATE desktop_devices SET revoked = 1 WHERE id = '<device_id>'`。
- Nginx 必须显式拒绝 `/internal/`、`/api/desktop/` 与公开的
  `/api/items/<id>/original`；`deploy/nginx/kanalib.conf.example` 已包含这三条。

## Backups and health

- 备份脚本在 `deploy/backup/`，timer 模板为 `kanalib-*-backup.{service,timer}.example`。
- 健康检查脚本在 `deploy/monitor/kanalib-health.sh`，timer 为
  `kanalib-health.{service,timer}.example`。
- 完整安装/恢复/回滚步骤见 `deploy/README.md`。
