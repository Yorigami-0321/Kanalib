# 私网组网备选方案对比（v2）

> 上级：[pixiv-integration.md](pixiv-integration.md) · **主方案：[easytier-connectivity.md](easytier-connectivity.md)** · 入库服务：[ingest-service.md](ingest-service.md) · 网页后端：[web-dynamic-backend.md](web-dynamic-backend.md)
>
> 2026-11-22 用户问题：除了 Tailscale，还有什么私网组建方案？要求用 open_websearch 实际调研。
> 2026-11-22 决策：**换 EasyTier**。本文件保留候选对比与淘汰理由，主方案细节见 [easytier-connectivity.md](easytier-connectivity.md)。
>
> 状态：**调研稿，未实现、未实测**。本文件只做选型对比与回退设计，不改代码。

## 0. 结论

| 问题 | 结论 |
|---|---|
| 是否只有 Tailscale | 不是。至少还有 Headscale、EasyTier、NetBird、Nebula、ZeroTier、Cloudflare Mesh、frp、原生 WireGuard |
| 本项目首选 | **EasyTier + 自建共享节点**：无账号、无控制面、Apache-2.0、可走 TCP/WSS |
| 次选 | **Tailscale / Headscale**：体验最成熟，但需账号（Headscale 可自托管控制面）；仅作对照与回退 |
| 最小依赖回退 | **frp over TCP 443**，只承载控制面与小元数据；原图仍应尽量直连 box |
| 不建议首选 | ZeroTier（根服务器可达性 + 控制器运维）、Nebula（UDP 4242 + 手工 PKI）、NetBird（三节点过重）、Cloudflare Mesh（非自托管、国内可达性波动） |
| 关键约束 | 公网访客**不装任何客户端**；私网方案只服务 web/box/桌面三端；原图不能绕经 2c2g |

一句话：**连接层可替换，业务层不变。** 双 listener、HMAC、设备令牌、原图只走私网的边界继续有效；本文件回答“底层用哪张网”，主方案细节在 [easytier-connectivity.md](easytier-connectivity.md)。

## 1. 需求约束

选型必须同时满足以下条件，缺一即淘汰或降级：

| # | 约束 | 影响 |
|---|---|---|
| N1 | 三端：2c2g 公网 web、box、Windows 桌面 | 节点很少，不需要企业级控制面 |
| N2 | 公网浏览器只读，不装客户端 | 私网方案不能替代公网 HTTPS |
| N3 | 写接口、同步、原图只走私网 | 需要设备身份 + 端口级 ACL |
| N4 | 桌面直连 box 拉原图，绕开 2c2g 出口 | 必须支持 P2P 或直连，不能强制全量中继 |
| N5 | box 默认零公网入站 | 优先出站建连 / NAT 穿透 |
| N6 | 中国网络，UDP 可能被限速或封锁 | 需要 TCP/WS/WSS 回退路径 |
| N7 | 服务后续要迁移 | 配置只写稳定名字，不写死虚拟 IP 或公网 IP |
| N8 | 只允许改文档 | 本轮不装客户端、不跑隧道、不改代码 |

## 2. 候选速览

| 方案 | 控制面 | NAT 穿透 | 中继 | 自托管 | 许可/生态 | 适合本项目 |
|---|---|---|---|---|---|---|
| **Tailscale** | 官方云（SaaS） | 有，UDP 打洞 | DERP，可自建 | 控制面不可自托管（用 Headscale） | 客户端开源；生态最成熟 | 体验最好，但必须登录账号；降为次选 |
| **Headscale** | 自托管，单 tailnet | 复用 Tailscale 客户端 | 可内置 DERP | 是 | 开源；官方仅支持二进制/DEB，Docker 非官方 | 想要 Tailscale 体验又不想用官方账号时的替代 |
| **EasyTier** | 无中心控制面 | 有，P2P 优先 | 共享节点，可自建 | 是 | Apache-2.0；Rust；中文文档 | **本项目主方案** |
| **NetBird** | 自托管 Management | 有，ICE/WireGuard | Signal + Relay（Coturn/WS） | 是 | 开源；组件多 | 功能强但三节点偏重 |
| **Nebula** | 无中心控制面，PKI | Lighthouse 辅助 | Lighthouse 可转发 | 是 | 开源；Go；Slack 生产验证 | 安全模型好，但需公网 UDP + 手工证书 |
| **ZeroTier** | Central 或自托管 controller | 有 | Root 服务器 | 部分（controller 可自建） | 客户端开源；controller 生态较弱 | 国内根服务器可达性是风险 |
| **Cloudflare Mesh** | Cloudflare 云 | 由 Cloudflare 编排 | Cloudflare 网络 | 否 | 商业服务 | 不作为私网主选 |
| **Cloudflare Tunnel** | Cloudflare 云 | 出站建连，无需入站 | Cloudflare 网络 | 否 | 商业服务 | 只适合暴露 HTTP，不适合原图直连 |
| **frp** | 自建 frps | 有 P2P 模式 | frps 中转 | 是 | 开源；Go；国内常用 | 最小回退，需公网 frps |
| **原生 WireGuard** | 无 | 无（需公网/端口映射） | 无 | 是 | 内核级；极简 | 有公网 IP 时的最小方案 |

> “许可/生态”只写已核实或公开可见的信息；落地前仍以各仓库 `LICENSE` 为准。

## 3. 各方案要点

### 3.1 Tailscale

已核实：

- 网状网络；任意两台已授权设备可直连，打洞失败时走 DERP 中继。
- `tailscale serve` 可把回环服务暴露到 tailnet；`tailscale funnel` 可暴露到公网，但为 beta 且有带宽/端口限制。
- MagicDNS 提供稳定私网名字；Services 可在换机时保持稳定名字（需较新版本）。
- 控制面与 DERP 在部分中国网络下可能不稳定。

对本项目：

- 最适合“私有机器互联”，不是公网图墙的替代品。
- 桌面 ↔ box 直连时，原图不经过 2c2g，满足 N4。
- 风险集中在 control plane / DERP 可达性；必须做真实网络实测。

### 3.2 Headscale

已核实（官方文档）：

- Tailscale control server 的开源、自托管实现；目标是自托管者和小型组织。
- 只实现**单个 tailnet**，适合个人或小型开源组织；不是企业级软件。
- 需要公网 IP；推荐 IPv4 + IPv6 双栈；HTTPS 443；启用内置 DERP 时还需要 UDP 3478；gRPC 远程控制用 TCP 50443。
- 官方支持二进制与 DEB；Docker 镜像有，但**官方不支持 Docker 部署**。
- 推荐 SQLite；PostgreSQL 仍在维护模式。
- 默认关闭客户端向 Tailscale Inc. 的日志上报（`logtail.enabled`）。

对本项目：

- 换控制面不换客户端：桌面/box/web 仍用 `tailscale` 命令与 Serve，业务代码不需要改。
- 适合“Tailscale SaaS 不可达，但想保留同一套客户端体验”。
- 代价：多一个需要备份、升级、监控的服务；升级不能跳小版本。
- 可部署在 2c2g 或 box 上；但 Headscale 与 tailnet 节点同机可能导致 subnet router / DERP / MagicDNS 问题，官方明确“不推荐/不支持”，所以更稳妥是独立小机器或至少独立网络命名空间。

### 3.3 EasyTier

已核实（官方文档）：

- Rust + Tokio；去中心化，没有也不需要类似 ZeroTier/Tailscale 的中心控制面板。
- 节点间自动尝试 NAT 穿透并建立 P2P；P2P 失败时由共享节点中转。
- 共享节点可不带参数启动，不需要 root；支持共享节点集群。
- 传输支持 TCP / UDP / WG / WS / WSS / QUIC / KCP / FakeTCP 等；适合 UDP 受限网络。
- `--private-mode true` 只允许同网络名 + 同密钥的节点接入。
- 2.4.0 起支持基于 IP 的 ACL，2.4.3 起支持基于身份组（Group）的零信任 ACL；规则可匹配源/目标组、协议、端口、速率。
- Apache-2.0；官网有中文文档与 DaoCloud 镜像；Windows/Linux/macOS/FreeBSD 可用。

对本项目：

- **已选为主方案**：三节点场景下运维最轻，没有控制面数据库，只有一个共享节点（放在 2c2g）。
- 中国网络优势：可显式用 `tcp://` / `wss://` 443 连共享节点，不依赖 UDP 打洞成功。
- 迁移：配置里写共享节点域名 + 网络名 + 密钥；换共享节点只改 `-p`，不涉及控制面迁移。
- 风险：项目比 Tailscale 年轻；ACL 的 `group_secret` 需要所有节点一致，泄露即失守；需要自己保证共享节点高可用。
- 不建议把共享节点放在 2c2g 上承载原图中转；共享节点只做发现/兜底，原图仍应 P2P 直连 box。

### 3.4 NetBird

已核实（官方文档）：

- 开源 Zero Trust 组网；可自托管；点对点 WireGuard overlay；没有集中式 VPN 服务器。
- 组件：Client、Management、Signal、Relay；Relay 是 Coturn/WS，仅在无法直连时兜底。
- 自托管 quickstart：Linux VM 至少 1 CPU / 2 GB；公网 TCP 443 + UDP 3478；需要公网域名；Docker Compose；默认内置 Traefik。
- 支持 groups / access control policies / private DNS / SSO（内置 Dex）。

对本项目：

- 功能覆盖最全，但三节点属于“杀鸡用牛刀”：Management + Signal + Relay + Traefik + 数据库，备份面明显大于 Headscale。
- 若未来要多用户、SSO、细粒度策略，可重新评估。
- 国内使用同样要验证云服务/自托管入口可达性。

### 3.5 Nebula

已核实（官方文档）：

- 自托管 overlay；PKI 证书身份；主机证书包含名字、IP、组；组用于防火墙规则。
- Lighthouse 负责节点发现与 NAT 穿透辅助；默认 UDP 4242，需要 lighthouse 有固定公网地址且入站 UDP 放行。
- 单二进制；Linux/macOS/Windows/iOS/Android/FreeBSD；Slack 曾用于 5 万台生产主机。
- 加密：Curve25519 ECDH + AES-256-GCM；可选 FIPS 模式。

对本项目：

- 安全模型最干净：不依赖共享密钥，证书即身份；适合“不想有控制面”的场景。
- 代价：证书签发/轮换/吊销要自己管；lighthouse 需要公网 UDP；没有内置 MagicDNS 式名字服务（可用 `static_host_map` 的 DNS 名，会周期重解析）。
- 如果 box 或 2c2g 的 UDP 4242 被封，需要额外方案；国内网络下不如 EasyTier 的 TCP/WSS 灵活。

### 3.6 ZeroTier

已核实（官方文档）：

- 端到端加密；设备 Curve25519 身份；直连失败时由 root 服务器中继；root 能看到路由元数据，但不能解密。
- 网络 controller 决定谁能加入、下发规则/路由/DNS；Central 是托管 controller。
- 本地 controller 可通过 `http://localhost:9993/controller/...` API 管理；controller 本身也是一个节点，网络 ID 基于 controller 的 Node ID。
- `zerotier-cli peers` 可看到 `DIRECT` / `RELAY`。

对本项目：

- 概念与 Tailscale 接近，但国内依赖 ZeroTier 根服务器（PLANET）发现；根服务器可达性不佳时体验会明显下降。
- 自托管 controller 只解决“谁管理网络”，不自动解决根服务器/中继可达性；要完全自主还需自建 moon/root，运维复杂度上升。
- 不作为首选；可作为“已有 ZeroTier 账号”的临时方案。

### 3.7 Cloudflare Mesh / Tunnel

已核实（官方文档）：

- Tunnel：`cloudflared` 从源站**出站**连接 Cloudflare，无需公网入站；适合把 HTTP/SSH/RDP 等暴露到 Cloudflare 网络。
- Mesh：给服务器/笔记本/手机分配私有 Mesh IP，支持 TCP/UDP/ICMP 与设备到设备；节点以 headless 模式运行 Cloudflare One Client；可宣告子网路由。
- 两者都依赖 Cloudflare 账号与控制面，不是自托管。

对本项目：

- Tunnel 适合“公网只读站点”或临时管理入口，不适合桌面 ↔ box 的原图直连（流量仍经 Cloudflare）。
- Mesh 能提供私网 IP，但国内到 Cloudflare 的可达性与带宽波动是现实风险；且后续迁移受服务商约束。
- 不作为主选；可作为“没有公网 IP、只想快速暴露一个 HTTP 管理页”的临时手段。

### 3.8 frp

已核实（官方文档）：

- 反向代理/内网穿透；支持 TCP、UDP、HTTP、HTTPS；客户端-服务端支持 TCP、QUIC、KCP、WebSocket；支持 P2P 通信（流量可不经服务器）。
- 需要一台有公网 IP 的 frps。

对本项目：

- 最小回退：box 跑 frpc 连 2c2g 的 frps，只暴露 `kanalib-ingest` 的私网端口；桌面再连 2c2g 的映射端口。
- 优点：只依赖 TCP 443/7000，穿 NAT 成功率高；实现简单。
- 缺点：默认所有流量经 2c2g，原图会吃云出口；P2P 模式可缓解，但仍需 frps 做信令。
- 适合“先跑通，再换 mesh”的过渡，不适合作为长期原图通道。

### 3.9 原生 WireGuard

- 最简：box 有公网 IP 时，桌面直接 WireGuard 到 box；2c2g 只做公网只读。
- 没有 NAT 穿透；box 无公网入站时，需要 2c2g 做 hub，原图就会绕云。
- 配置静态、无控制面、无 DNS/ACL 生态；适合作为“最后兜底”而不是日常方案。

## 4. 对比矩阵

评分口径：5 = 最适合本项目，1 = 明显不适合。分数是工程判断，不是 benchmark。

| 维度 | Tailscale | Headscale | EasyTier | NetBird | Nebula | ZeroTier | CF Mesh | frp | WireGuard |
|---|---|---|---|---|---|---|---|---|---|
| 三节点上手成本 | 5 | 3 | 4 | 2 | 3 | 3 | 3 | 4 | 3 |
| NAT 穿透 | 5 | 5 | 4 | 5 | 4 | 4 | 4 | 3 | 1 |
| 中国网络可控性 | 3 | 4 | 5 | 3 | 3 | 2 | 2 | 4 | 4 |
| 桌面直连 box | 5 | 5 | 5 | 5 | 4 | 4 | 3 | 3 | 5（有公网时） |
| 自托管/数据自主 | 2 | 5 | 5 | 5 | 5 | 3 | 1 | 5 | 5 |
| ACL/身份 | 5 | 5 | 4 | 5 | 5 | 3 | 4 | 2 | 2 |
| 运维面 | 5 | 2 | 4 | 2 | 3 | 3 | 4 | 4 | 3 |
| 迁移友好 | 5 | 4 | 4 | 4 | 3 | 2 | 2 | 4 | 3 |
| 生态/成熟度 | 5 | 3 | 3 | 4 | 4 | 4 | 4 | 4 | 5 |
| 许可/可控性 | 4 | 5 | 5 | 5 | 5 | 3 | 1 | 5 | 5 |
| 免账号 | 2 | 5 | 5 | 4 | 5 | 4 | 2 | 5 | 5 |
| **结论** | 次选 | 次选 | **主选** | 淘汰 | 淘汰 | 淘汰 | 淘汰 | 回退 | 兜底 |

## 5. 中国网络与 UDP 现实

这不是“哪个协议更先进”，而是“哪条路能通”：

| 现象 | 受影响方案 | 应对 |
|---|---|---|
| UDP 打洞失败 / 被 QoS | Tailscale、ZeroTier、Nebula、NetBird、EasyTier 的 UDP 模式 | 走 TCP 11010 / WSS 11012；或接受中继 |
| 控制面域名不可达 | Tailscale SaaS、ZeroTier Central、NetBird Cloud | Headscale / EasyTier / 自托管 NetBird |
| DERP/Relay 慢 | Tailscale、ZeroTier、NetBird | 自建 DERP/共享节点；或改用直连 |
| 公网 443 被占用 | Headscale、NetBird、frp | 独立小机器；或反代 + 端口复用（EasyTier 走 11010/11012，不抢 443） |
| 家宽无公网 IP | WireGuard、Nebula lighthouse | 用有公网 IP 的中继/共享节点；或 frp |

结论：

1. **不要假设 UDP 一定可用。** 所有方案都要准备 TCP 11010 / WSS 11012 的回退。
2. **不要假设控制面一定可达。** 至少准备一个“无控制面”方案（EasyTier）或自托管控制面（Headscale）。
3. **不要把所有流量压到 2c2g。** 原图必须尽量 P2P；中继只做兜底。

## 6. 推荐决策

### 6.1 主方案：EasyTier + 自建共享节点

适用条件：

- 不想注册任何账号，也不想依赖第三方控制面；
- 中国网络下 UDP 打洞成功率不确定，需要 TCP/WSS 回退；
- 三端都是自有机器，规模小，不需要企业级身份系统。

落地方式：

- 完整拓扑、地址、监听、ACL、迁移与验收见 [easytier-connectivity.md](easytier-connectivity.md)。
- 业务配置只写 `*.et.net` 名字；不写 `10.126.126.x`。
- 共享节点放 2c2g，只做发现/兜底；原图仍走桌面 ↔ box P2P。

EasyTier 建议拓扑：

```text
                    ┌──────────────────────────┐
                    │ 共享节点（2c2g 或独立小机）│
                    │  tcp 11010 / wss 11012     │
                    └────────────┬─────────────┘
                                 │ 发现 / 兜底中继
          ┌──────────────────────┼──────────────────────┐
          │                      │                      │
     ┌────▼────┐            ┌────▼────┐            ┌────▼────┐
     │  web    │            │  box    │            │ desktop │
     │ .2      │            │ .3      │            │ .4      │
     └─────────┘            └─────────┘            └─────────┘
          └──────────── P2P 优先，原图直连 ────────────┘
```

配置要点：

```text
# 所有节点一致
--network-name kanalib
--network-secret <强随机>
--secure-mode
--accept-dns
-p https://<公网域名>/easytier/peers.txt

# 共享节点
easytier-core --private-mode true \
  --network-name kanalib \
  --network-secret <强随机> \
  --secure-mode
```

注意：

- `--private-mode true` 只允许同网络名 + 同密钥节点接入。
- ACL 用 group + `group_secret`；所有节点的 `declares` 必须一致。
- 共享节点只做发现/兜底；不要把原图长期压到共享节点。
- 共享节点必须有一个可入站的 TCP 端口；box 默认零公网入站，所以共享节点放 2c2g 或独立小机。
- 虚拟网段固定 `10.126.126.0/24`，三端静态分配，不用 `-d`。

### 6.2 次选：Tailscale，Headscale 兜底

适用条件：

- 能接受登录账号（或愿意自建 Headscale）；
- 更看重开箱即用的 Serve/MagicDNS/ACL；
- EasyTier 在目标网络实测不达标，需要成熟方案对照。

落地方式：

- 拓扑与双 listener 边界同主方案，但 private listener 可回到 `127.0.0.1` + `tailscale serve`。
- 业务配置只写 MagicDNS 名字；不写 `100.x`。
- 若 control plane 不可达：部署 Headscale，客户端仍用 `tailscale`，只改 `--login-server`；业务代码零改动。

### 6.3 最小回退：frp over TCP 443

适用条件：

- 只想尽快让桌面/box 的私网端口可达；
- 能接受流量经 2c2g；
- 不需要 mesh、DNS、复杂 ACL。

建议：

- 只映射 `kanalib-ingest` 的私网端口与 web private listener；
- 原图仍走“桌面直连 box”的独立通道；frp 只做控制/元数据；
- 用 TLS + token + 端口白名单；不要暴露整个内网。

### 6.4 不推荐作为主选

| 方案 | 原因 |
|---|---|
| ZeroTier | 国内根服务器可达性不可控；自托管 controller 仍需处理 root/moon |
| Nebula | 需要公网 UDP 4242；证书轮换与 lighthouse 运维成本高于收益 |
| NetBird | 三节点要维护 Management/Signal/Relay/Traefik/DB，过重 |
| Cloudflare Mesh | 非自托管，国内可达性与迁移自由度不足 |
| 纯 WireGuard | 无 NAT 穿透；box 无公网入站时原图绕云 |

## 7. 与现有文档的衔接

本文件不推翻主方案 [easytier-connectivity.md](easytier-connectivity.md)，而是把“连接层”抽象出来：

| 层 | 现有文档 | 可替换性 |
|---|---|---|
| 业务拓扑 | 公网只读 + 私网写入 + 桌面直连 box | **不变** |
| 鉴权 | 设备令牌 / HMAC / ACL | **不变** |
| 双 listener | public `:8787` / private `:8788` | **不变** |
| 连接层 | EasyTier 虚拟网 + 魔法 DNS | 可换 Tailscale / Headscale / frp |
| 原图通道 | 桌面 ↔ box 直连 | 尽量保持直连；中继只兜底 |

实现建议：

- 配置里只出现“服务名 + 端口”，不出现具体组网产品：

```toml
[connect]
web_private = "http://kanalib-web.et.net:8788"
box_private  = "http://kanalib-box.et.net:8790"
```

- 由部署层决定 `web.internal` 是 EasyTier 魔法 DNS、虚拟 IP 还是 frp 映射；业务代码不改。
- 私网用明文 HTTP + secure mode 加密；若换 Tailscale/Headscale，则改成 `https://...:8443`，业务代码仍不改。
- 健康检查统一探测 `GET /api/health`；连接层切换不影响业务验收。

## 8. 验证清单

本轮只做文档，不执行；后续落地时按此清单实测。

2026-11-22 桌面本机检查：本地安装于 `D:\APP\Easytier`，`easytier-cli 2.6.4-8428a89d` 可用；GUI 进程在运行，但 EasyTier core RPC（默认 `127.0.0.1:15888`）不可达，无已启动网络。`*.et.net` 目前解析到公网 wildcard 地址，不代表私网可达。8787/8788/8790/11010/11012 均无监听；未提供公网域名、web/box 私网端点或共享节点，因此下方条目仍未实测。已准备 `scripts/verify_private_net.ps1` 与 `deploy/` 配置草稿。缺少的先决条件：启动 EasyTier core 并加入 kanalib 网络，且提供三个真实端点。

### 8.1 通用

- [ ] 三端都能解析稳定名字（EasyTier `*.et.net` / hosts / DNS）。
- [ ] 桌面 → web private、桌面 → box、box → web private 三条链路都通。
- [ ] 公网请求 `/internal/*`、`/api/desktop/*` 返回 404。
- [ ] 桌面退出组网后，私网请求立即失败，不静默回退公网。

### 8.2 P2P 与流量

- [ ] `easytier-cli route` 的 `next_hop` 为 `DIRECT`，或明确记录 relay 及原因。
- [ ] 传 100MB 文件，记录耗时、是否中继、2c2g 出口增量。
- [ ] 确认原图传输不经过 2c2g（对比网卡计数）。

### 8.3 中国网络

- [ ] 在目标运营商/时段测试共享节点可达性（TCP 11010 / WSS 11012）。
- [ ] 强制 UDP 不可用，验证 TCP 11010 / WSS 11012 回退。
- [ ] 断网/重连后自动恢复，不需要手工重启服务。

### 8.4 迁移

- [ ] 替换共享节点后，只改 `peers.txt`，不改业务代码。
- [ ] 旧节点下线不影响网页已有目录。
- [ ] 备份/恢复共享节点配置与三端 EasyTier 配置（EasyTier 没有控制面数据库）。

## 9. 决策记录

| # | 问题 | 结论 |
|---|---|---|
| P1 | 私网主方案 | **EasyTier + 自建共享节点**；无账号、无控制面 |
| P2 | Tailscale/Headscale 定位 | **次选/对照**；需要成熟 Serve/MagicDNS 时再评估 |
| P3 | 是否用 ZeroTier | 不作为主选；已有账号可临时用 |
| P4 | 是否用 Nebula | 不作为主选；UDP 4242 与 PKI 成本高 |
| P5 | 是否用 NetBird | 不作为主选；三节点过重 |
| P6 | 是否用 Cloudflare Mesh/Tunnel | 仅临时管理入口；不做原图通道 |
| P7 | 最小回退 | **frp over TCP 443**，只承载控制/元数据 |
| P8 | 业务代码是否绑定组网产品 | **否**；只写稳定名字 + 端口 |

## 10. 待确认

| # | 问题 | 推荐 |
|---|---|---|
| P-Q1 | box 是否有可入站 TCP 端口 | 没有；共享节点放 2c2g 或独立小机 |
| P-Q2 | 是否接受自建 Headscale 的升级/备份成本 | 主方案不需要；仅作对照 |
| P-Q3 | 是否需要多用户/SSO | 不需要则 EasyTier/Headscale 足够 |
| P-Q4 | 是否要求完全无第三方控制面 | **是**；这是选 EasyTier 的主因 |
| P-Q5 | 原图是否允许经 2c2g 中转 | 不允许；只做兜底 |

## 11. 参考

> 本轮通过 open_websearch 抓取官方文档；GitHub 原始页面受本机代理影响未能直接读取，落地前应在仓库核对 `LICENSE` 与最新版本。

- Tailscale DERP：<https://tailscale.com/kb/1232/derp-servers>
- Tailscale Serve：<https://tailscale.com/kb/1242/tailscale-serve>
- Tailscale Funnel：<https://tailscale.com/kb/1223/funnel>
- Tailscale Services：<https://tailscale.com/kb/1552/tailscale-services>
- Headscale 文档：<https://headscale.net/stable/>
- Headscale FAQ：<https://headscale.net/stable/about/faq/>
- Headscale 部署要求：<https://headscale.net/stable/setup/requirements/>
- EasyTier 快速组网：<https://easytier.cn/guide/network/quick-networking.html>
- EasyTier 共享节点：<https://easytier.cn/guide/network/host-public-server.html>
- EasyTier ACL：<https://easytier.cn/guide/config/acl.html>
- EasyTier FAQ：<https://easytier.cn/guide/faq.html>
- NetBird 介绍：<https://docs.netbird.io/>
- NetBird 架构：<https://docs.netbird.io/about-netbird/how-netbird-works>
- NetBird 自托管：<https://docs.netbird.io/selfhosted/selfhosted-quickstart>
- Nebula 文档：<https://nebula.defined.net/docs/>
- Nebula 快速开始：<https://nebula.defined.net/docs/guides/quick-start/>
- Nebula 节点发现：<https://nebula.defined.net/docs/guides/host-discovery/>
- ZeroTier 文档：<https://docs.zerotier.com/>
- ZeroTier 安全：<https://docs.zerotier.com/security/>
- ZeroTier Controller：<https://docs.zerotier.com/controller/>
- ZeroTier 中继排查：<https://docs.zerotier.com/faq/relaying/>
- Cloudflare Tunnel：<https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/>
- Cloudflare Mesh：<https://developers.cloudflare.com/mesh/>
- frp 概览：<https://gofrp.org/en/docs/overview/>
