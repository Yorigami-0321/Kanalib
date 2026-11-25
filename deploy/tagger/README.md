# 打标后端（本机 GPU，经 EasyTier 直连）

box 只负责入库与规则；标签不足 `min_tags` 时通过 HTTP 调本机 GPU 打标。
链路全部走 EasyTier，没有 SSH 隧道。EasyTier 的 relay 在本拓扑下不转发负载，
所以云端做一次 TCP 终结与再发起（不是 SSH 隧道，也不经过公网明文）：

```text
kanalib-ingest (box 10.126.126.3)
  └─ http://10.126.126.2:8793/tag      ← EasyTier 直连（box ↔ cloud）
       └─ cloud: kanalib-tagger-proxy   监听 10.126.126.2:8793，转发到 10.126.126.1:8793
            └─ 10.126.126.1:8793        ← EasyTier 直连（cloud ↔ 本机，wss 经 Cloudflare）
                 └─ Windows: sidecar/tagger/http_server.py（CUDA/FP16）
```

## 为什么能当开关

打标服务只绑定 EasyTier 虚拟地址 `10.126.126.1`，不监听 `127.0.0.1` 或 `0.0.0.0`。
本地 EasyTier 客户端（Windows 服务 `easytier-gui`）停掉后该地址消失，box 的连接直接失败，
条目落到 `review`，不会误放行。启动脚本也会先检查该地址，缺失就拒绝启动。

## 本机启动

```powershell
.\deploy\tagger\start-local-tagger.ps1
```

依赖：

- `dist/Kanalib/runtime/python/python.exe`（带 torch/transformers）
- `models/pixai-tagger-v1.0`
- EasyTier 已连上 `kanalib` 网络并拿到 `10.126.126.1`

日志：`%TEMP%\kanalib-tagger.log`、`%TEMP%\kanalib-tagger.err`；PID：`%TEMP%\kanalib-tagger.pid`。
脚本可重复执行，已在跑时只做健康检查。

## 鉴权与配额

打标服务不再接受匿名请求。启动脚本首次运行会生成
`C:\ProgramData\Kanalib\tagger-token`（32 字节 hex），并作为
`--token-file` 传给 `http_server.py`；把同一份内容放到 box 的
`/etc/kanalib/tagger-token`，让 ingest 的 `[tagging].token_file` 指向它。

- `/tag` 与 `/health` 都要求 `x-tagger-token`（或 `Authorization: Bearer`）。
- 请求体默认上限 32 MiB、并发 4，超出分别返回 413/503；
  可用 `KANALIB_TAGGER_MAX_BODY`、`KANALIB_TAGGER_MAX_INFLIGHT` 调整。
- 云端代理可用 `KANALIB_TAGGER_PROXY_ALLOW=10.126.126.3` 限制来源；
  它只做 TCP 转发，鉴权仍由 tagger 校验。
- 模型目录的 `*.py`/`*.json` 会算出 guard hash；设置
  `KANALIB_TAGGER_MODEL_SHA256` 后目录被替换会拒绝启动。

## box 配置

```toml
[tagging]
backend = "http"
endpoint = "http://10.126.126.2:8793/tag"
token_file = "/etc/kanalib/tagger-token"
concurrency = 2
timeout_secs = 1800
```

改完 `sudo systemctl restart kanalib-ingest`。`min_tags` 保持 5，和打标后端配置无关。

Nodaroo CPU 部署时，Nodaroo 使用 EasyTier `10.126.126.5`，云端 `kanalib-nodaroo-tagger-proxy.service` 监听 `10.126.126.2:18793` 并转发到 `10.126.126.5:8793`。box 使用：

```toml
[tagging]
backend = "http"
endpoint = "http://10.126.126.2:18793/tag"
concurrency = 1
timeout_secs = 1800
```

## 验证

```bash
# 本机
curl -H "x-tagger-token: $(cat /etc/kanalib/tagger-token)" http://10.126.126.1:8793/health

# 云端（同网络任意节点）
curl -s --noproxy '*' -H "x-tagger-token: $(cat /etc/kanalib/tagger-token)" \
  http://10.126.126.1:8793/health

# box（经云端代理）
sudo curl -s --noproxy '*' -F "image=@/var/lib/kanalib-ingest/media/originals/<id>.png" \
  -H "x-tagger-token: $(cat /etc/kanalib/tagger-token)" \
  http://10.126.126.2:8793/tag

# Nodaroo CPU（经云端第二代理）
curl -s --noproxy '*' -H "x-tagger-token: $(cat /etc/kanalib/tagger-token)" \
  http://10.126.126.2:18793/health
sudo curl -s --noproxy '*' -F "image=@/var/lib/kanalib-ingest/media/originals/<id>.png" \
  -H "x-tagger-token: $(cat /etc/kanalib/tagger-token)" \
  http://10.126.126.2:18793/tag
```

单张推理约 1–4 秒（RTX 4060 Laptop，FP16）。实测 box 端到端约 7 秒（含两跳 EasyTier 传输）。
Nodaroo CPU 实测单张 206 秒；24.6MB 原图经 EasyTier 上传加推理端到端约 20 分 30 秒，因此 CPU 部署使用 `timeout_secs = 1800`。

## 限制

- 本机必须在线；休眠、重启或退出 EasyTier 后，box 只能进 `review`。
- 这是过渡方案。长期应迁到常驻 GPU 主机，或给 box/云端换足算力。
- 网络密钥与 secure-mode 私钥都在本机 EasyTier 配置里，见 `docs/backlog/pending/tagger-easytier-handoff.md`。
