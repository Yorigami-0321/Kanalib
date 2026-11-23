# 打标后端（本机 GPU，经 EasyTier 直连）

box 只负责入库与规则；标签不足 `min_tags` 时通过 HTTP 调本机 GPU 打标。
链路全部走 EasyTier，没有 SSH 隧道，也没有云端转发：

```text
kanalib-ingest (box, 10.126.126.3)
  └─ http://10.126.126.1:8793/tag
       └─ EasyTier 私网（kanalib 网络，wss 经 Cloudflare）
            └─ Windows: sidecar/tagger/http_server.py，只绑 10.126.126.1（CUDA/FP16）
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

## box 配置

```toml
[tagging]
backend = "http"
endpoint = "http://10.126.126.1:8793/tag"
concurrency = 2
timeout_secs = 30
```

改完 `sudo systemctl restart kanalib-ingest`。`min_tags` 保持 5，和打标后端配置无关。

## 验证

```bash
# 本机
curl http://10.126.126.1:8793/health

# 云端（同网络任意节点）
curl -s --noproxy '*' http://10.126.126.1:8793/health

# box
sudo curl -s --noproxy '*' -F "image=@/var/lib/kanalib-ingest/media/originals/<id>.png" \
  http://10.126.126.1:8793/tag
```

单张推理约 1–4 秒（RTX 4060 Laptop，FP16），实际耗时主要受 EasyTier 路径影响：
当前 wss 路径延迟约 0.4–1 秒、丢包 3%–5%，大图上传会明显慢于本地。

## 限制

- 本机必须在线；休眠、重启或退出 EasyTier 后，box 只能进 `review`。
- 这是过渡方案。长期应迁到常驻 GPU 主机，或给 box/云端换足算力。
- 网络密钥与 secure-mode 私钥都在本机 EasyTier 配置里，见 `docs/backlog/pending/tagger-easytier-handoff.md`。
