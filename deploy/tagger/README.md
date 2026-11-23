# 打标后端（本机 GPU 过渡方案）

box 只负责入库与规则；标签不足 `min_tags` 时通过 HTTP 调本机 GPU 打标。
链路：

```text
kanalib-ingest (box)
  └─ http://10.126.126.2:8793/tag
       └─ cloud: kanalib-tagger-forward  (systemd, 监听 EasyTier 地址)
            └─ 127.0.0.1:8793  (SSH -R 反向隧道)
                 └─ Windows: sidecar/tagger/http_server.py (CUDA)
```

## 本机启动

```powershell
.\deploy\tagger\start-local-tagger.ps1
```

脚本会启动两个后台进程（均隐藏窗口）：

- `dist/Kanalib/runtime/python/python.exe sidecar/tagger/http_server.py --device cuda --port 8793`
- `ssh -N -R 127.0.0.1:8793:127.0.0.1:8793 <user>@<cloud>`

默认 SSH 目标 `yorigami@128.1.132.31`、密钥 `%USERPROFILE%\.ssh\kanalib_deploy`，可用参数覆盖。
日志写到 `%TEMP%\kanalib-tagger.log`、`%TEMP%\kanalib-tagger-tunnel.err`。

## 云端

```bash
sudo install -m 0755 deploy/tagger/kanalib-tagger-forward.py /usr/local/bin/
sudo install -m 0644 deploy/systemd/kanalib-tagger-forward.service.example \
  /etc/systemd/system/kanalib-tagger-forward.service
sudo systemctl daemon-reload
sudo systemctl enable --now kanalib-tagger-forward.service
```

## box 配置

```toml
[tagging]
backend = "http"
endpoint = "http://10.126.126.2:8793/tag"
concurrency = 2
timeout_secs = 30
```

改完重启 `kanalib-ingest`；规则文件里的 `min_tags` 与此无关，保持 5。

## 验证

```bash
# 云端
curl -s http://127.0.0.1:8793/health

# box
curl -s --noproxy '*' http://10.126.126.2:8793/health
sudo curl -s --noproxy '*' -F "image=@/var/lib/kanalib-ingest/media/originals/<id>.png" \
  http://10.126.126.2:8793/tag
```

单张约 1–4 秒（RTX 4060 Laptop，FP16）。本机休眠、重启或断网后需要重新执行启动脚本；
长期应把该服务迁到常驻 GPU 主机。
