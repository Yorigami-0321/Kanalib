# 打标链路交接（EasyTier + 云端代理，2026-11-23）

> 上级：[ingest-service.md](../ingest-service.md) · 部署说明：[deploy/tagger/README.md](../../../deploy/tagger/README.md) · 登记：F-089
>
> 本文交接「box → 本机 GPU 打标」这条链路的拓扑、配置与遗留问题。SSH 反向隧道已废弃，现为纯 EasyTier 链路加一跳云端 TCP 代理。

## 1. 状态摘要

- 本机已加入 EasyTier `kanalib` 网络，地址 `10.126.126.1`；云端看到 p2p（`wss` 经 Cloudflare），延迟约 0.4 秒。
- 打标服务 `sidecar/tagger/http_server.py` 只绑 `10.126.126.1:8793`（CUDA/FP16），当前 pid 与 health 正常。
- 云端 `kanalib-tagger-proxy.service` 已上线：监听 `10.126.126.2:8793`，把请求再发起给 `10.126.126.1:8793`。旧的 `kanalib-tagger-forward.service` 与脚本已删除。
- box 的 `[tagging].endpoint = "http://10.126.126.2:8793/tag"`，`kanalib-ingest` 已部署到 `9fd683d` 并运行。
- 端到端已闭环：health 约 1.2 秒，真实打标 4.9–7.1 秒；`2087cc90…` 重跑后 `tagged=1`、`tag_source=merged`、51 个标签、`push_pending=0`。
- 2026-11-24 Nodaroo CPU 打标已部署：EasyTier `10.126.126.5`，打标 `http://10.126.126.5:8793`，bootstrap health `http://10.126.126.5:18793/health`；公网 `se2.nodaroo.fun:25926` 改为 EasyTier TCP 监听，不再提供 HTTP health。
- 本机已添加并持久化直连 peer `tcp://se2.nodaroo.fun:25926`，peer 为 `p2p`、`tunnel_proto=tcp`、路由 `DIRECT`；真实打标返回 39 个标签，耗时 206 秒，ingest 默认超时改为 1800 秒。
- 云端新增 `kanalib-nodaroo-tagger-proxy.service`：`10.126.126.2:18793 → 10.126.126.5:8793`；box 已切到该端点、并发 1、超时 1800，并重启 `kanalib-ingest`。box 经云端代理的真实打标返回 26 个标签，耗时 74.5 秒；24.6MB 大图重跑返回 51 个标签，条目转 `allow` 且已推送，端到端约 20 分 30 秒。

## 2. 拓扑

```text
kanalib-ingest (box 10.126.126.3)
  └─ http://10.126.126.2:8793/tag      ← EasyTier 直连，box ↔ cloud
       └─ cloud: kanalib-tagger-proxy   监听 10.126.126.2:8793 → 再发起 10.126.126.1:8793
            └─ 10.126.126.1:8793        ← EasyTier 直连，cloud ↔ 本机（wss 经 Cloudflare）
                 └─ 本机 Windows：sidecar/tagger/http_server.py（CUDA/FP16，只绑 10.126.126.1）
```

为什么需要云端代理：当前拓扑下 box 与 PC 在 EasyTier 里显示 `relay(2)`，但 relay 不转发负载（`rx/tx` 恒为 `0 B`），box 直连 `10.126.126.1:8793` 不通。box ↔ cloud、cloud ↔ PC 各自都是 p2p，所以由云端做一次 TCP 终结与再发起。代理不是 SSH 隧道，走 EasyTier 虚拟网络。

开关语义：打标服务只绑 EasyTier 地址 `10.126.126.1`。本机 EasyTier 服务停止 → 该地址消失 → 代理上游连接失败 → box 条目落 `review`，不会误放行。启动脚本也会先检查该地址是否存在。

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
listeners = ["tcp://0.0.0.0:11010", "udp://0.0.0.0:11010"]

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
5. **不要同时跑 GUI 与独立 core。** 两者会抢 `11010/11012`，日志出现 `AddrInUse`，peer 表也会异常。当前只保留会话 0 的服务实例；桌面 GUI 实例已停。
6. **relay 不转发负载。** box ↔ PC 的 `relay(2)` 通路 `rx/tx` 恒为 0；加 `--relay-all-peer-rpc`、显式 `listeners` 都无效。云端 easytier 脚本已回滚，备份 `kanalib-easytier.bak-20261123`。需要 box → 本机时走云端代理。
7. **重启服务需要管理员。** 非管理员执行 `Restart-Service easytier-gui` 会 `OpenService FAILED 5`。
8. **升权方式。** 本机沙箱账号无法操作服务；需用提权 exec 以 `win-patchouli\administrator` 执行。UAC 的 `Start-Process -Verb RunAs` 在当前组策略下被拦截，不要依赖它。

## 5. 打标服务

- 代码：`sidecar/tagger/http_server.py`（提交 `cfb2adf`），协议 `POST /tag`（multipart `image`）→ `{"tags": [...]}`，`GET /health`。
- 启动：`.\deploy\tagger\start-local-tagger.ps1`（可选 `-BindAddress`、`-Port`）。
  - 先校验 `10.126.126.1` 存在，否则直接报错。
  - 用 `%TEMP%\kanalib-tagger.pid` 判重，已在跑只做健康检查。
  - 绑定 `10.126.126.1:8793`，日志 `%TEMP%\kanalib-tagger.log|.err`。
- 依赖 `dist/Kanalib/runtime/python`（torch/transformers）与 `models/pixai-tagger-v1.0`。
- 单张推理 1–4 秒；实际请求耗时受网络影响更大。
- 打标结果与来源标签合并；`tag_source` 为 `merged`。标签预算按来源标签行数计数，译名不再重复计入（`9fd683d`）。

## 6. box 侧状态

box 在线，`kanalib-ingest` active，配置 `[tagging].endpoint = "http://10.126.126.2:8793/tag"`。
当前 10 allow / 4 block / 0 review / `push_pending=0`，规则版本 `2026-11-23.3`。

更新与部署：

```bash
export PATH=/home/user/.cargo/bin:$PATH
cd /home/user/Kanalib && git fetch origin main && git reset --hard origin/main
cargo build --release -p kanalib-ingest
sudo install -m 0755 target/release/kanalib-ingest /usr/local/bin/kanalib-ingest
sudo systemctl restart kanalib-ingest
```

box 拉 GitHub 需 `HTTPS_PROXY=http://127.0.0.1:3128`，`NO_PROXY=10.126.126.0/24,.et.net,127.0.0.1,localhost`。

补标重跑：

```bash
curl -sS --noproxy '*' -X POST -H 'content-type: application/json' -d '{"force":true}' \
  http://10.126.126.3:8792/admin/api/items/<item-id>/recheck
```

等约 60 秒，日志应出现 `tagged=1`，该条目标签数增加且 `push_pending` 归零。

## 7. 清理

- 本机 `%TEMP%` 排障残留：`et-*.toml`、`p-*.log`、`etkeys` venv、`etgen.py`、`kanalib-et.env`（含网络密钥，已删）。
- 云端临时文件：`/tmp/taghealth*`、`/tmp/b2p.log`（已删）。
- 旧转发服务已清理：`kanalib-tagger-forward.service` 与 `/usr/local/bin/kanalib-tagger-forward.py`。

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
curl -s --noproxy '*' http://10.126.126.2:8793/health

# box
curl -s --noproxy '*' http://10.126.126.2:8793/health
```

期望：本机 peer 看到 `10.126.126.2`（p2p）与 `10.126.126.3`（relay 即可）；health 都返回 `"ok": true`。

## 9. 回滚

1. 临时停用打标：把 box `[tagging]` 的 `backend`/`endpoint` 注释掉并重启，所有标签不足条目进 `review`。
2. 回滚代理：`sudo systemctl disable --now kanalib-tagger-proxy`。恢复时可从仓库 `deploy/tagger/kanalib-tagger-proxy.py` 与 `deploy/systemd/kanalib-tagger-proxy.service.example` 重新安装。
3. 停本机服务：`Stop-Process` 掉 tagger，或停 `easytier-gui` 服务。

## 10. 风险与限制

- EasyTier 走 `wss://patchouli.us.ci/easytier/`，当前实测延迟约 0.4–1 秒、丢包 3%–5%；大图上传可能接近 30 秒超时，必要时调大 `[tagging].timeout_secs`。
- 本机休眠/重启/退出 EasyTier 后，打标不可用，review 会堆积；监控已有 `model_error_streak` 告警。
- box ↔ PC 的 relay 通路不可用，不能把 endpoint 直接指向 `10.126.126.1`。
- 这是过渡方案，长期应迁到常驻 GPU 主机。
- `config.d\kanalib.toml` 含网络密钥与 secure-mode 私钥，不要入库、不要外发。

## 11. 相关提交

- `cfb2adf` feat(tagger): 接入 HTTP 打标后端并覆盖黑名单优先拦截
- `fa99d36` fix(tagger): 用 PID 文件判断本机进程，避免误判隧道
- `851d695` fix(ingest): 已推送作品被 block 时生成删除批次
- `9fd683d` fix(ingest): 标签预算按标签计数，不再把译名重复计入
