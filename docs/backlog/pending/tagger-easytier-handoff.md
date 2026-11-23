# 打标链路交接（EasyTier 直连，2026-11-23）

> 上级：[ingest-service.md](../ingest-service.md) · 部署说明：[deploy/tagger/README.md](../../../deploy/tagger/README.md) · 登记：F-089
>
> 本文交接「box → 本机 GPU 打标」这条链路的拓扑、配置与遗留问题。原 SSH 反向隧道 + 云端 TCP 转发方案已废弃。

## 1. 状态摘要

- 本机已加入 EasyTier `kanalib` 网络，地址 `10.126.126.1`，云端看到 p2p（`ws` 隧道）。
- 打标服务 `sidecar/tagger/http_server.py` 只绑 `10.126.126.1:8793`，云端 `curl /health` 已通。
- 云端 `kanalib-tagger-forward.service` 已 `disable --now`，SSH 反向隧道进程已停。
- **未完成**：box 当前离线，`/etc/kanalib/ingest.toml` 仍指向旧的 `10.126.126.2:8793`，尚未切换到本机。

## 2. 拓扑

```text
kanalib-ingest (box 10.126.126.3)
  └─ http://10.126.126.1:8793/tag
       └─ EasyTier kanalib 网络（本机 ↔ kanalib-web，wss 经 Cloudflare）
            └─ 本机 Windows：sidecar/tagger/http_server.py（CUDA/FP16，只绑 10.126.126.1）
```

开关语义：打标服务只绑 EasyTier 地址。本机 EasyTier 服务停止 → 地址消失 → box 连接失败 → 条目落 `review`，不会误放行。启动脚本也会先检查该地址是否存在。

## 3. 本机 EasyTier 客户端

| 项 | 值 |
|---|---|
| 程序目录 | `D:\APP\Easytier` |
| 运行方式 | Windows 服务 `easytier-gui`（`StartType=Automatic`，服务模式） |
| 服务配置目录 | `C:\Users\Administrator\AppData\Roaming\com.kkrainbow.easytier\config.d` |
| 网络配置 | `config.d\kanalib.toml`（备份 `kanalib.toml.bak-20261123`） |
| 网络名 / 地址 | `kanalib` / `10.126.126.1/24` |
| 对端 | `wss://patchouli.us.ci/easytier/` |
| RPC | `127.0.0.1:15999`（服务启动参数） |

`kanalib.toml` 关键字段（密钥不在此展开）：

```toml
instance_name = "kanalib-pc"
hostname = "kanalib-pc"
ipv4 = "10.126.126.1"
listeners = []

[network_identity]
network_name = "kanalib"
network_secret = "<云端 /etc/kanalib/easytier.env 的 ET_NETWORK_SECRET>"

[secure_mode]
enabled = true
local_private_key = "<Base64 raw 32B Ed25519 seed>"
local_public_key = "<同上私钥推导的公钥>"

[[peer]]
uri = "wss://patchouli.us.ci/easytier/"

[flags]
private_mode = true
enable_ipv6 = false
accept_dns = false
```

## 4. 踩过的坑（排障必读）

1. **`secure_mode` 是表，不是布尔值。** 写 `secure_mode = true` 会报 `invalid type: boolean, expected struct SecureModeConfig`。
2. **config-dir 实例不继承命令行参数。** `easytier-core --config-dir X --secure-mode` 不会把 secure 模式应用到 X 里的实例；只有 `--config-file` 单实例会。
3. **secure 模式必须自带密钥。** 只写 `enabled = true` 时连接一直 `Connecting`，日志报 `local private key is not set`。密钥是 32 字节 Ed25519 原始种子序列化成的 ASCII 字符串，公钥由私钥推导；缺公钥报 `local public key is not set`。
4. **非管理员建不了 wintun。** 手工运行 `easytier-core` 会报 `Failed to create adapter` / `os error 5`；用 GUI 服务模式（管理员）才行。
5. **不要同时跑 GUI 与独立 core。** 两者会抢 `11010/11012`，日志出现 `AddrInUse`。
6. **`listeners = []` 表示不监听入站端口。** 当前依赖本机主动连 `kanalib-web` 后复用该隧道，实测云端可达；若需要其他节点直连/打洞，去掉该行并重启服务。
7. **重启服务需要管理员。** 非管理员执行 `Restart-Service easytier-gui` 会 `OpenService FAILED 5`。

## 5. 打标服务

- 代码：`sidecar/tagger/http_server.py`（提交 `cfb2adf`），协议 `POST /tag`（multipart `image`）→ `{"tags": [...]}`，`GET /health`。
- 启动：`.\deploy\tagger\start-local-tagger.ps1`（可选 `-BindAddress`、`-Port`）。
  - 先校验 `10.126.126.1` 存在，否则直接报错。
  - 用 `%TEMP%\kanalib-tagger.pid` 判重，已在跑只做健康检查。
  - 绑定 `10.126.126.1:8793`，日志 `%TEMP%\kanalib-tagger.log|.err`。
- 依赖 `dist/Kanalib/runtime/python`（torch/transformers）与 `models/pixai-tagger-v1.0`。
- 单张推理 1–4 秒；实际请求耗时受网络影响更大。

## 6. box 侧待办（唯一阻塞）

box 目前离线：云端与本机的 `easytier-cli peer` 都没有 `10.126.126.3`，box MCP 返回
`HTTP 502: magpie's sign-in to the MCP server box has run out`。

恢复后执行：

```bash
sudo sed -i 's#endpoint = "http://10.126.126.2:8793/tag"#endpoint = "http://10.126.126.1:8793/tag"#' /etc/kanalib/ingest.toml
sudo grep -A3 '^\[tagging\]' /etc/kanalib/ingest.toml
sudo systemctl restart kanalib-ingest
curl -s --noproxy '*' http://10.126.126.1:8793/health
```

然后挑一条 `tag budget short` 的条目走管理页「按规则重跑」，确认日志出现 `tagged>=1` 且 `push_pending=0`。

## 7. 云端清理（可选）

转发服务已停用，但文件仍在：

```bash
sudo rm -f /usr/local/bin/kanalib-tagger-forward.py \
           /etc/systemd/system/kanalib-tagger-forward.service
sudo systemctl daemon-reload
```

本机 `%TEMP%` 下还有排障残留：`et-*.toml`、`p-*.log`、`etkeys` venv、`etgen.py`、`kanalib-et.env`（含网络密钥，用完即删）。

## 8. 验证清单

```powershell
# 本机
ipconfig | Select-String 10.126.126.1
& 'D:\APP\Easytier\easytier-cli.exe' -p 127.0.0.1:15999 peer
curl http://10.126.126.1:8793/health
```

```bash
# 云端
curl -s --noproxy '*' http://10.126.126.1:8793/health

# box
curl -s --noproxy '*' http://10.126.126.1:8793/health
```

期望：本机 peer 看到 `10.126.126.2`、`10.126.126.3`；三处 health 都返回 `"ok": true`。

## 9. 回滚

1. 临时停用打标：把 box `[tagging]` 的 `backend`/`endpoint` 注释掉并重启，所有标签不足条目进 `review`。
2. 回到 SSH 隧道方案：从提交 `cfb2adf` 取回 `deploy/tagger/kanalib-tagger-forward.py` 与 unit 模板，恢复云端服务与本机 `ssh -R`，endpoint 改回 `10.126.126.2:8793`。

## 10. 风险与限制

- EasyTier 走 `wss://patchouli.us.ci/easytier/`，当前实测延迟约 0.4–1 秒、丢包 3%–5%；大图上传可能接近 30 秒超时，必要时调大 `[tagging].timeout_secs`。
- 本机休眠/重启/退出 EasyTier 后，打标不可用，review 会堆积；监控已有 `model_error_streak` 告警。
- 这是过渡方案，长期应迁到常驻 GPU 主机。
- `config.d\kanalib.toml` 含网络密钥与 secure-mode 私钥，不要入库、不要外发。

## 11. 相关提交

- `cfb2adf` feat(tagger): 接入 HTTP 打标后端并覆盖黑名单优先拦截
- `fa99d36` fix(tagger): 用 PID 文件判断本机进程，避免误判隧道
- `851d695` fix(ingest): 已推送作品被 block 时生成删除批次
