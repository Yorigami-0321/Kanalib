# 三端互联与 EasyTier 方案（v2）

> 上级：[pixiv-integration.md](pixiv-integration.md) · 网页后端：[web-dynamic-backend.md](web-dynamic-backend.md) · 入库服务：[ingest-service.md](ingest-service.md) · 备选对比：[private-networking-options.md](private-networking-options.md)
>
> 2026-11-22 决策：三端私网主方案由 Tailscale 改为 **EasyTier**。Tailscale 旧稿已删除，选型理由保留在 [private-networking-options.md](private-networking-options.md)。
>
> 状态：**设计稿，未安装、未实测、未改代码**。本文件只定拓扑、地址、监听、ACL、迁移与验收。

## 0. 结论

| 问题 | 结论 |
|---|---|
| 主方案 | **EasyTier 2.6.x + 自建共享节点**；不注册账号、不依赖任何控制面 |
| 为什么换 | 无账号/无控制面、Apache-2.0、中文文档、可走 TCP/WSS、共享节点可自建；比 Tailscale 更适合“三台自有机器 + 中国网络 + 后续迁移” |
| 公网浏览器 | 继续**公网 HTTPS 只读**；访客不装任何客户端 |
| 写接口放哪 | 只挂**私网 listener**；公网 listener 不注册 `/internal/*`、`/api/desktop/*` |
| 原图放哪 | 只走私网；桌面优先**直连 box**，不经过 2c2g 云 |
| 私网名字 | EasyTier 魔法 DNS：`<hostname>.et.net`；Linux 需手工配置 split DNS |
| 加密 | `--secure-mode` 端到端加密 + ACL 默认拒绝 + 应用层令牌/HMAC |
| TLS | 公网由 Nginx 终止；私网 v1 用 **secure mode 加密 + 明文 HTTP**，不引入私有 CA |
| 回退 | 多共享节点 / frp over TCP 443 / 公网 HMAC；公网只读始终不受影响 |
| 不做什么 | **不做数据迁移**；不导入旧库/旧缩略图；**不拆分桌面现有功能**、不升级桌面 schema |

一句话：**公网只读走普通 HTTPS，三台机器之间的写入、同步、原图传输走 EasyTier 虚拟网。**

## 1. 为什么是 EasyTier，以及和 Tailscale 的差异

### 1.1 选它的理由

| 需求 | EasyTier 的表现 |
|---|---|
| 不想注册账号 | 开源核心**不需要账号**；没有 Tailscale 式的 tailnet 归属 |
| 不想被控制面卡住 | 去中心化，没有中心控制面数据库；共享节点可自建 |
| 中国网络 UDP 不稳 | 支持 TCP / UDP / WS / WSS / QUIC / KCP / FakeTCP；默认显式走 TCP |
| box 默认零公网入站 | 节点主动连共享节点，NAT 穿透；失败时共享节点中转 |
| 后续要迁移 | 配置只写 `network_name` + `network_secret` + 共享节点发现地址；换机器只改发现地址 |
| 许可 | Apache-2.0，和本项目 MIT 兼容 |

EasyTier 的 Web 控制台（`easytier-web`）和 EasyTier Pro 都是**可选**的集中管理/商业服务；本项目不使用，也不需要任何在线账号。

### 1.2 不是等价替换：三个必须改的设计点

| Tailscale 做法 | EasyTier 对应 | 影响 |
|---|---|---|
| `tailscale serve` 把 `127.0.0.1:8788` 暴露到 tailnet | **没有 Serve**；必须让服务监听虚拟网 IP，或在本机做反代 | private listener 不能再只绑回环 |
| Serve 自动提供 HTTPS + MagicDNS | 只有魔法 DNS，没有自动 HTTPS 反代 | 私网 TLS 要应用层做，或接受 secure mode 加密下的明文 HTTP |
| MagicDNS 三平台自动 | Windows/macOS 自动；**Linux 要手工把 `et.net` 指到 `100.100.100.101`** | web/box 若是 Linux，必须配 split DNS 或 hosts |

这三条决定了本文件后面的 listener、TLS 和迁移写法。业务接口、HMAC、设备令牌、双 listener 的**边界不变**，只换连接层。

## 2. 角色与链路

| 链路 | 方向 | 走哪 | 内容 | 鉴权 |
|---|---|---|---|---|
| 浏览器 → 网页 | 公网 | 普通 HTTPS | 只读 API、缩略图、SPA | 无（公开只读） |
| 桌面 → 网页 | 私网 | EasyTier 虚拟网 | 上传图片/数据、拉取目录、回传标签 | 设备令牌 + ACL |
| box → 网页 | 私网 | EasyTier 虚拟网 | 只推 `allow` 批次、删除事件 | HMAC + ACL |
| 桌面 → box | 私网 | EasyTier 虚拟网，优先 P2P | 拉原图、批量下载、补页 | 设备令牌/短期签名 + ACL |
| 网页 → box | 私网 | EasyTier 虚拟网 | 健康检查、元数据、必要的原图回源 | 共享密钥 + ACL |
| 管理员 → 网页/box | 私网 | EasyTier 虚拟网 + SSH | 审核、日志、维护 | SSH key + ACL |

EasyTier 是网状网：任意两台已入网节点都可尝试直连；业务上仍按最小权限只开必要端口，不因为“同一个网络”就默认全通。

## 3. 推荐拓扑

```text
                        公网
  Browser ──HTTPS──► Nginx :443 ──► kanalib-web public 127.0.0.1:8787
                                      （只读：index/items/media）
                                            │
                                            │ 同一进程
                                            ▼
                                    kanalib-web private 10.126.126.2:8788
                                      （ingest / desktop / original）
                                            ▲
                                            │ EasyTier 虚拟网 10.126.126.0/24
        ┌───────────────────────────────────┴──────────────────────────┐
        │                                                              │
        │   2c2g: kanalib-web + EasyTier 节点（兼共享节点/兜底中转）      │
        │   box : kanalib-ingest 10.126.126.3:8790 + 原图目录            │
        │   桌面: 工作台 10.126.126.4                                    │
        │                                                              │
        └──────────────────────────────────────────────────────────────┘
```

要点：

1. **公网只读**：Nginx 只反代 public listener；`/internal/*`、`/api/desktop/*` 在公网直接 404。
2. **私网写入**：private listener 绑 EasyTier 虚拟 IP，不绑 `0.0.0.0`，也不依赖 Serve。
3. **原图不出私网**：桌面直接从 box 拉；公网浏览器最多拿 t2048。
4. **box 零公网入站**：管理口、原图、审核页都只走虚拟网；除非明确要公网原图，才单开签名媒体端口。
5. **共享节点放 2c2g**：它本来就有公网入口；EasyTier 进程同时作为网络成员和兜底中转。共享节点只做发现/兜底，不长期承载原图。

## 4. 地址与名字

### 4.1 网段与静态地址

默认虚拟网段是 `10.126.126.0/24`。本项目**显式指定静态 IP**，不用 `-d`：

| 节点 | 虚拟 IP | hostname | FQDN |
|---|---|---|---|
| 2c2g web | `10.126.126.2` | `kanalib-web` | `kanalib-web.et.net` |
| box | `10.126.126.3` | `kanalib-box` | `kanalib-box.et.net` |
| 桌面 | `10.126.126.4` | `kanalib-desktop` | `kanalib-desktop.et.net` |
| 共享节点（若独立） | 不分配 | `kanalib-relay` | `kanalib-relay.et.net` |

为什么不用 `-d`（DHCP）：官方明确“出现 IP 冲突时 IP 会自动更改”。迁移、ACL、防火墙规则都依赖地址稳定，所以用 `-i` 固定；冲突风险由三台机器的小规模人工分配承担。

### 4.2 魔法 DNS

- 所有节点加 `--accept-dns`（布尔开关，不带值）。
- 默认 DNS 服务器是 `100.100.100.101`，域名后缀默认 `et.net`；可用 `--tld-dns-zone` 改。
- Windows/macOS 自动配置系统 DNS；**Linux 必须手工配置**。
- 业务配置只写 FQDN，不写 `10.126.126.x`；这样换 IP 时只改 EasyTier 配置。

Linux（web / box）用 systemd-resolved 做 split DNS，避免影响公网解析：

```bash
# 网卡名由 --dev-name 决定，建议显式设为 et0
sudo resolvectl dns et0 100.100.100.101
sudo resolvectl domain et0 '~et.net'
resolvectl query kanalib-box.et.net
```

如果目标机器没有 systemd-resolved，用 `/etc/hosts` 兜底：

```text
10.126.126.2 kanalib-web.et.net kanalib-web
10.126.126.3 kanalib-box.et.net kanalib-box
```

> hosts 兜底会失去“换 IP 只改一处”的好处；它只用于 DNS 故障时的临时恢复。

### 4.3 发现地址（迁移的关键）

不要让节点写死共享节点 IP。用发现地址，把入口收敛到一个可替换的文本文件：

```text
https://<公网域名>/easytier/peers.txt
```

文件内容每行一个候选地址：

```text
tcp://<公网域名>:11010
tcp://<备用共享节点>:11010
```

所有节点用同一个 `-p https://<公网域名>/easytier/peers.txt`。换共享节点、加备用节点、切 IP，只改这个文件，不改三台机器的配置。

EasyTier 也支持 `txt://` 和 `srv://` 发现；本项目先用 HTTPS 文本，原因是它最容易由现有 Nginx 托管、最容易人工核对。

## 5. 两个 listener（网页）

`kanalib-web` 仍从一个进程开两个监听，但绑定地址变了：

```text
--bind-public  127.0.0.1:8787   # 只给 Nginx；公网不直接访问
--bind-private 10.126.126.2:8788 # 只给 EasyTier 虚拟网
```

| listener | 路由 | 谁能访问 |
|---|---|---|
| public `127.0.0.1:8787` | `/api/health`、`/api/index`、`/api/items`、`/api/works`、`/api/authors`、`/api/collections`、`/api/tags`、`/media/*` | Nginx，经公网 HTTPS 只读 |
| private `10.126.126.2:8788` | `/internal/ingest/*`、`/api/desktop/*`、`/api/items/{id}/original`、`/original/{id}` | 仅 EasyTier 虚拟网 |

约束：

1. public listener **不注册**写路由；即使请求打进来也是 404，而不是 401。
2. Nginx 额外显式拒绝 `/internal/*`、`/api/desktop/*`、`/original/*`，作为第二道保险。
3. private listener 绑虚拟 IP，不绑 `0.0.0.0`；公网扫描不到 8788。
4. 两个 listener 共享同一个 SQLite 和媒体目录；写路由只挂在 private router。
5. 主机防火墙再兜一层：只允许 `et0` 入站到 8788。

> 网页前端只调用 public listener 的只读 API；private listener 只给 box 与桌面原生客户端。不要让浏览器跨到 `http://*.et.net:8788`，否则会触发混合内容拦截。

### 5.1 为什么不能继续绑 `127.0.0.1`

Tailscale Serve 能在本机把回环端口代理到 tailnet；EasyTier 没有 Serve。若 private listener 仍绑回环，远端节点无法访问。

两种替代：

| 方案 | 做法 | 评价 |
|---|---|---|
| **绑虚拟 IP（推荐）** | `--bind-private 10.126.126.2:8788` | 最简单；公网天然不可达；IP 变则改配置 |
| 绑 `0.0.0.0` + 防火墙 | `--bind-private 0.0.0.0:8788`，nftables 只放行 `et0` | 不依赖启动顺序，但多一层配置，漏配即暴露 |

本项目选前者；systemd unit 里用 `After=easytier.service` + `Restart=always` 处理启动顺序。

更稳的做法是加一个启动前等待，避免 EasyTier 还没把虚拟 IP 配上就绑定失败：

```ini
[Service]
ExecStartPre=/usr/local/bin/wait-for-ip.sh 10.126.126.2
ExecStart=/usr/local/bin/kanalib-web --bind-private 10.126.126.2:8788 ...
Restart=always
```

`wait-for-ip.sh` 只做一件事：循环 `ip -4 addr show dev et0 | grep -q 10.126.126.2`，超时 30 秒后退出非零。

### 5.2 私网 TLS

v1 不引入私有 CA：

- EasyTier `--secure-mode` 已提供节点间端到端加密（Noise 握手、会话密钥轮换），中继节点也无法解密。
- 私网 HTTP 上仍有应用层设备令牌 / HMAC / 短期签名；不把“进了虚拟网”当作授权。
- 若未来有浏览器形态的桌面客户端，再补应用层 TLS 或本机反代；当前桌面是原生客户端，不受混合内容限制。

注意：如果桌面端用浏览器内核访问 `https://` 网页、再被 302 到 `http://kanalib-box.et.net:8790/...`，会被混合内容策略拦截。当前设计里原图接口只给原生桌面客户端，公网浏览器拿不到，所以不触发。

## 6. EasyTier 配置

> 以下命令按官方文档的 CLI 形式给出；TOML 仅作形状示意。落地前用目标版本的 `easytier-core --help` 或官方配置助手核对字段名。

### 6.1 2c2g：web 节点 + 共享节点

```bash
sudo easytier-core \
  --hostname kanalib-web \
  -i 10.126.126.2 \
  --network-name kanalib \
  --network-secret '<强随机>' \
  --private-mode true \
  --secure-mode \
  --accept-dns \
  --dev-name et0 \
  -l tcp://0.0.0.0:11010 \
  -l udp://0.0.0.0:11010 \
  --rpc-portal 15888 \
  --relay-network-whitelist kanalib \
  --foreign-relay-bps-limit 2000000
```

- `--private-mode true`：只允许同网络名 + 同密钥的节点接入。
- `--relay-network-whitelist kanalib`：只为本网络转发，不为陌生网络做公共中继。
- `--foreign-relay-bps-limit`：给兜底中转限速，保护 2c2g 出口（单位以 `--help` 为准）。
- `--rpc-portal 15888`：管理口只回环，不暴露公网。
- 不分配 `-i` 的独立共享节点只做转发；本节点因为要托管 8788，所以必须有自己的虚拟 IP。
- `wss://` 监听先不加：证书来源与路径复用都未验证（见 §10.2、§14 E-Q3）。确认可用后再作为 TCP 被封时的补充入口。
- web/box/桌面都要创建 TUN，需要 root 或 `CAP_NET_ADMIN` + `/dev/net/tun`；容器部署时加 `--cap-add NET_ADMIN --device /dev/net/tun`。

### 6.2 box：入库服务

```bash
sudo easytier-core \
  --hostname kanalib-box \
  -i 10.126.126.3 \
  --network-name kanalib \
  --network-secret '<强随机>' \
  --private-mode true \
  --secure-mode \
  --accept-dns \
  --dev-name et0 \
  -p https://<公网域名>/easytier/peers.txt \
  --rpc-portal 15888
```

`kanalib-ingest` 只监听 `10.126.126.3:8790`：

```toml
[service]
bind = "10.126.126.3:8790"
```

box 不需要任何公网入站；出站只允许 Pixiv、模型服务、网页私网地址。

### 6.3 桌面：Windows 服务

```powershell
.\easytier-core.exe `
  --hostname kanalib-desktop `
  -i 10.126.126.4 `
  --network-name kanalib `
  --network-secret '<强随机>' `
  --private-mode true `
  --secure-mode `
  --accept-dns `
  -p https://<公网域名>/easytier/peers.txt
```

用官方 `install.cmd` 注册为 Windows 服务（`File` 模式加载配置文件）；服务会随系统启动。桌面必须 `--accept-dns` 才能解析 `*.et.net`。

### 6.4 配置文件形状（示意）

```toml
instance_name = "kanalib-web"
hostname = "kanalib-web"
ipv4 = "10.126.126.2"
listeners = [
  "tcp://0.0.0.0:11010",
  "udp://0.0.0.0:11010",
]
rpc_portal = "15888"

[network_identity]
network_name = "kanalib"
network_secret = "<强随机>"

[secure_mode]
enabled = true

[flags]
private_mode = true
accept_dns = true
dev_name = "et0"
relay_network_whitelist = "kanalib"
foreign_relay_bps_limit = 2000000

[[peer]]
uri = "https://<公网域名>/easytier/peers.txt"
```

### 6.5 诊断

```bash
easytier-cli node      # 本节点虚拟 IP、hostname、listener、NAT 类型
easytier-cli peer      # 对端、cost、lat_ms、loss_rate、tunnel_proto、nat_type
easytier-cli route     # next_hop 是 DIRECT 还是经共享节点
easytier-cli proxy     # KCP/QUIC 代理连接
easytier-cli acl stats # ACL 命中统计
```

验收时以 `route` 的 `next_hop` 和 `peer` 的 `tunnel_proto` 为准，不看“能 ping 通”就结束。

## 7. ACL 与安全模式

### 7.1 身份组

| 组 | 成员 | 用途 |
|---|---|---|
| `web` | `kanalib-web` | 只读站与私网写入口 |
| `box` | `kanalib-box` | 入库、原图、审核 |
| `desktop` | `kanalib-desktop` | 工作台桥接 |
| `admin` | 管理机 | SSH/维护 |

每个节点在 `members` 里声明自己属于哪些组；`declares` 必须包含所有组的 `group_secret`，且**所有节点完全一致**。`group_secret` 与 `network_secret` 分开，泄露一个不连带另一个。

### 7.2 入站规则

每台机器只配自己的入站链，默认拒绝：

| 目标节点 | 允许来源 | 协议/端口 | 用途 |
|---|---|---|---|
| web | box、desktop | TCP 8788 | ingest / desktop 桥接 |
| box | desktop、web | TCP 8790 | 原图、媒体、管理 |
| box | admin | TCP 22 | 维护 |
| desktop | web、box | 仅已建立连接的返回流量 | 桌面不主动开入站 |

规则顺序：先放行必要端口，再 `default_action = 2`（拒绝）。不要用 `tcp:*` 图省事。

### 7.3 ACL 形状（示意）

```toml
[acl.acl_v1.group]
members = ["web"]

[[acl.acl_v1.group.declares]]
group_name = "web"
group_secret = "<web-secret>"

[[acl.acl_v1.group.declares]]
group_name = "box"
group_secret = "<box-secret>"

[[acl.acl_v1.group.declares]]
group_name = "desktop"
group_secret = "<desktop-secret>"

[[acl.acl_v1.chains]]
name = "web_inbound"
chain_type = 1
enabled = true
default_action = 2

[[acl.acl_v1.chains.rules]]
name = "allow_box_ingest"
priority = 1000
action = 1
source_groups = ["box"]
destination_groups = ["web"]
protocol = 1
ports = ["8788"]
enabled = true

[[acl.acl_v1.chains.rules]]
name = "allow_desktop_bridge"
priority = 900
action = 1
source_groups = ["desktop"]
destination_groups = ["web"]
protocol = 1
ports = ["8788"]
enabled = true
```

注意：

- ACL 从 2.4.0 起支持 IP、2.4.3 起支持身份组；**固定使用 2.6.x**，不要混用旧版本。
- 改 ACL 后通常要重启 EasyTier 进程。
- `group_secret` 放 0600 配置文件，不进仓库；轮换时三端一起改。

### 7.4 安全模式

`--secure-mode` 提供：

- 节点间独立协商密钥，经共享节点中转也无法解密；
- Noise 握手 + 防重放 + 会话密钥轮换；
- 可给共享节点固定 `local_private_key`，客户端用 `peer_public_key` 锁定，防止连到假冒共享节点；
- 可签发带 TTL 的临时凭据（本项目只给临时设备用）。

落地顺序：**先升级共享节点，再升级客户端**；所有节点统一 2.6.x。桌面是长期可信节点，直接用 `network_secret`，不引入凭据。

### 7.5 密钥清单

| 密钥 | 存在哪 | 轮换 |
|---|---|---|
| `network_secret` | 三端 EasyTier 配置（0600） | 泄露即全网重配 |
| `group_secret` | 三端 ACL 配置（0600） | 可与 network_secret 分开轮换 |
| `local_private_key` | 共享节点配置 | 固定，不随重启变化 |
| HMAC shared secret | web/box 的 `secret_file` | 独立轮换 |
| 桌面设备令牌 | 只在 web 存哈希 | 可单独撤销 |

## 8. 流量账

| 流量 | 走哪 | EasyTier 是否省 | 说明 |
|---|---|---|---|
| 公网浏览器看缩略图 | 网页云 → 浏览器 | **不省** | 仍是云出口；靠 CDN/缓存/只存缩略图 |
| 桌面上传图片/数据到网页 | 桌面 → 网页云 | **不省字节** | 仍要进云；省的是“不用暴露公网写接口” |
| box 推送 allow 批次 | box → 网页云 | **不省字节** | 元数据 + 缩略图，量小；省的是公网攻击面 |
| 桌面从 box 拉原图 | 桌面 ↔ box P2P | **省云出口** | 不经过 2c2g；这是最大的流量收益 |
| P2P 失败，经共享节点中转 | 桌面 ↔ 2c2g ↔ box | **不省，且更贵** | 云出口翻倍；用 `route` 监控，长期 relay 要查 NAT |
| 网页代理原图给浏览器 | box → 网页云 → 浏览器 | **反而更贵** | 默认不做 |
| KCP/QUIC 代理 | 仍走两端 | **不省字节** | 只改善丢包下的延迟/吞吐 |
| `--compression zstd` | 两端 | **基本不省** | JPEG/PNG 已压缩；不要为图片开启 |

EasyTier 不做“把图片变小”；**“省流量”应理解为“绕开云出口”，不是压缩。**

## 9. 迁移

### 9.1 稳定名字

- 业务配置只写 `kanalib-web.et.net` / `kanalib-box.et.net`，不写 `10.126.126.x`。
- 新机器接管时复用同一个 `--hostname`；先下线旧节点，避免同名冲突。
- 虚拟 IP 变化时，只改该节点的 `-i` 与绑定地址；FQDN 不变。

### 9.2 共享节点迁移

- 所有节点都通过 `peers.txt` 发现入口；换共享节点只改这个文件。
- 迁移窗口内 `peers.txt` 同时列旧、新地址；EasyTier 会随机挑一个可用地址。
- 新共享节点验证 `easytier-cli peer` 出现 `p2p` 后，再删旧地址。

### 9.3 版本升级

- 共享节点 → web → box → 桌面，按这个顺序升级。
- 安全模式的服务端可接受旧客户端；反过来不行，所以不能先升客户端。
- 固定小版本（如 2.6.4），不使用 nightly；升级前备份配置。

### 9.4 公网 DNS

- EasyTier 只解决私网名字；浏览器访问的公网域名仍要自己切。
- 用低 TTL + 稳定域名；迁移窗口内保留旧站只读，切完再下线。

## 10. 中国网络与回退

### 10.1 首选路径

1. `tcp://<域名>:11010`：最容易穿过只放行 TCP 的网络。
2. `udp://<域名>:11010`：UDP 可用时优先，延迟最低。
3. `wss://<域名>:11012`：**待验证**；确认证书与路径可用后，作为 11010 被封时的补充。
4. 多个 `-p` 同时配置，EasyTier 会挑可用地址。

### 10.2 443 复用（待实测）

2c2g 的 443 已被 Nginx 占用。若 11010/11012 都被封，可尝试：

```nginx
location /easytier/ {
    proxy_pass http://127.0.0.1:11012;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

再让节点连 `wss://<域名>/easytier/`。**这条未验证**：需要确认当前版本 wss 监听是否接受路径、TLS 证书如何配置。验证前不要把 443 复用写进生产配置。

### 10.3 回退阶梯

| 方案 | 适用 | 代价 |
|---|---|---|
| 多共享节点 | 单个共享节点故障/被墙 | 多一台小机器 |
| frp over TCP 443 | 只需要让固定端口可达 | 流量经 2c2g，原图会吃云出口 |
| 公网 HTTPS + HMAC + IP allowlist | 最保守 | box 需要一个受控公网入口 |
| 仅公网只读 + 桌面手动导入 | 先不做自动同步 | 功能不受阻 |

不建议把公网图墙整体塞进 EasyTier：访客不可能都装客户端，公网只读也不应该依赖虚拟网。

## 11. 失败模式

| 故障 | 影响 | 处理 |
|---|---|---|
| 共享节点不可用 | 新连接/新 P2P 受影响；已建立直连可能继续 | `peers.txt` 配多个入口；公网只读不受影响 |
| 长期只能 relay | 大文件慢、2c2g 出口涨 | `easytier-cli route` 告警；查 NAT/UDP；必要时改 TCP 或 frp |
| 魔法 DNS 不解析 | 私网请求按名字失败 | Linux 检查 `resolvectl`；hosts 兜底 |
| 虚拟 IP 冲突 | 节点掉线/路由异常 | 三台机器静态分配；冲突时改 `-i` 并改绑定 |
| box 离线 | 新图停止入库；原图不可取 | 网页保留已有目录；桌面显示明确错误 |
| web 离线 | 公网只读不可用；桌面推送失败 | 静态站兜底；桌面队列重试 |
| 版本不一致 | 安全模式握手失败 | 共享节点先升级；固定版本 |
| TUN 未加载（Linux） | 启动报 `TunError` | `modprobe tun`；容器加 `NET_ADMIN` 与 `/dev/net/tun` |
| RPC 口暴露 | 本机管理面被远程访问 | `--rpc-portal 15888`；防火墙拒绝 15888 |

## 12. 验收

### 12.1 公网

- 不安装 EasyTier 的浏览器能正常看首页、标签、详情、缩略图。
- 公网请求 `/internal/ingest/batch`、`/api/desktop/push` 返回 404，不是 401/403。
- 公网不能下载原图；详情页回退到 t2048 或显示“仅私网”。
- 公网扫描不到 8788、8790、15888。

### 12.2 私网

- 三端 `easytier-cli peer` 都能看到彼此，`route` 的 `next_hop` 为 `DIRECT`（至少桌面 ↔ box）。
- 桌面能 push/pull；退出 EasyTier 后立即失败，不静默回退公网。
- box 能推送 allow 批次；公网无法伪造。
- `kanalib-box.et.net` 在三端都能解析；Linux 用 `resolvectl query` 验证。

### 12.3 流量

- 桌面从 box 拉 100MB 原图，记录耗时、`tunnel_proto`、是否 relay。
- 对比 2c2g 网卡计数：原图传输期间出口增量接近 0。
- 强制 UDP 不可用，验证 `tcp://11010` 仍能建链（WSS 待验证）。

### 12.4 迁移

- 改 `peers.txt` 指向新共享节点，三端不改配置即可重连。
- 新 web/box 节点复用 hostname 接管，业务只改部署配置。
- 旧节点下线不影响网页已有目录。

### 12.5 故障

- 停掉 EasyTier：公网只读仍可用；桌面/box 的私网操作有明确错误与重试。
- 强制 relay：大文件仍能传，但记录 relay 状态；不把它当成正常性能。

## 13. 已定决策

| # | 问题 | 结论 |
|---|---|---|
| E1 | 私网主方案 | **EasyTier 2.6.x + 自建共享节点** |
| E2 | 是否需要账号 | **不需要**；不使用 Web 控制台/EasyTier Pro |
| E3 | 公网浏览器是否装客户端 | **不装**；公网 HTTPS 只读 |
| E4 | 网页监听 | **public 回环 8787 + private 虚拟 IP 8788** |
| E5 | 原图默认给谁 | **仅私网**；公网最多 t2048 |
| E6 | 私网 TLS | v1 用 **secure mode + 明文 HTTP + 应用层鉴权**；暂不建私有 CA |
| E7 | 私网名字 | **魔法 DNS `*.et.net`**；Linux 配 split DNS |
| E8 | 地址分配 | **静态 `-i`**，不用 `-d` |
| E9 | 迁移入口 | **`peers.txt` 发现地址** + 固定 hostname |
| E10 | Tailscale/Headscale | 降为备选；不作为默认路径 |
| E11 | 共享节点位置 | **2c2g**（已有公网入口）；独立小机可选 |
| E12 | 回退 | 多共享节点 / frp / 公网 HMAC；公网只读不受影响 |

## 14. 待实测

2026-11-22 桌面本机检查：`D:\APP\Easytier\easytier-cli.exe` 为 2.6.4-8428a89d，GUI 在运行但 core RPC（默认 `127.0.0.1:15888`）不可达，无已启动网络；未执行联网测试或变更网络配置。可运行 `scripts/verify_private_net.ps1` 并显式传入端点参数。部署草案见 `deploy/systemd/`、`deploy/nginx/`、`deploy/easytier/`。

| # | 问题 | 建议 |
|---|---|---|
| E-Q1 | 2c2g 上 EasyTier 常驻的内存/CPU 占用 | 先跑空载 24h；目标 <50MB 常驻 |
| E-Q2 | `tcp://11010` 在目标运营商的可达性 | 三端各测一次；不通再评估 WSS 或 frp |
| E-Q3 | wss 监听是否需要证书、是否支持路径 | 先直连 11012；443 复用验证后再上 |
| E-Q4 | Linux 魔法 DNS 与 systemd-resolved 的兼容性 | 用 `resolvectl` 配 `~et.net`；记录重启后是否保留 |
| E-Q5 | 桌面 ↔ box 的 NAT 类型与 P2P 成功率 | `easytier-cli peer` 记录 `nat_type`；失败时加共享节点 |
| E-Q6 | ACL 规则在 2.6.x 的实际生效顺序 | 用 `easytier-cli acl stats` 验证；先宽松后收紧 |
| E-Q7 | 共享节点被滥用/扫描的风险 | `private-mode` + 限速 + fail2ban；观察连接数 |
| E-Q8 | 2c2g 重启后 TUN 与绑定顺序 | systemd `After=easytier.service`；`Restart=always` |

## 15. 参考

> 本轮通过 open_websearch 抓取 EasyTier 官方中文文档；GitHub 原始页面受本机代理影响，版本号以官网下载页为准。

- EasyTier 简介：<https://easytier.cn/guide/introduction.html>
- 快速组网：<https://easytier.cn/guide/network/quick-networking.html>
- 去中心组网：<https://easytier.cn/guide/network/decentralized-networking.html>
- 共享节点：<https://easytier.cn/guide/network/host-public-server.html>
- 完整配置项：<https://easytier.cn/guide/network/configurations.html>
- 配置文件：<https://easytier.cn/guide/network/config-file.html>
- 魔法 DNS：<https://easytier.cn/guide/network/magic-dns.html>
- 发现地址：<https://easytier.cn/guide/network/discovery-url.html>
- 安全模式：<https://easytier.cn/guide/network/secure-mode.html>
- ACL：<https://easytier.cn/guide/config/acl.html>
- KCP/QUIC 代理：<https://easytier.cn/guide/network/kcp-proxy.html>
- systemd：<https://easytier.cn/guide/network/install-as-a-systemd-service.html>
- Windows 服务：<https://easytier.cn/guide/network/install-as-a-windows-service.html>
- FAQ：<https://easytier.cn/guide/faq.html>
- 下载与版本：<https://easytier.cn/guide/download.html>
