# beam — 一条命令把 localhost 分享到公网

> 设计文档 · 2026-06-14 · 状态：已通过，待实现

## 1. 目标与理念

`beam` 是一个用 Rust 编写、**对所有人永久免费**的命令行工具。它把"将本地服务暴露到公网"这件事，从一堆复杂的隧道配置，简化成一条命令：

```
beam 3000
```

核心理念：

- **永久免费且可持续**：不自建、不维护需要付费的中转服务器。通过封装成熟的免费后端（首选 Cloudflare Quick Tunnel），让流量成本由大厂承担，项目自身零运营成本。
- **用户友好**：零手动安装依赖、零注册、清晰的人话提示、贴心的细节（如二维码）。
- **Benefit 所有人**：开源、跨平台、文档友好。

### 为什么是"封装现有免费服务"而非"自建中转"

本地机器位于 NAT/防火墙之后，公网无法直接连入，因此必须有一台公网 IP 的机器中转流量。该中转服务器需要持续付费运行。对开源项目而言，长期自养中转服务器不可持续。Cloudflare Quick Tunnel 由 Cloudflare 免费提供、自带 HTTPS、无需注册即可获得 `*.trycloudflare.com` 随机网址，是 MVP 最现实的"真免费"路径。

## 2. MVP 范围（v0.1）

单条命令、前台运行：

```
$ beam 3000

  ✓ 正在准备隧道...
  ✓ 你的本地服务已上线！

  🌐 公网地址:  https://happy-cat-42.trycloudflare.com
  📍 转发到:    http://localhost:3000

  [二维码]   ← 手机扫码即可访问

  按 Ctrl+C 停止
```

MVP 明确包含：

- `beam <port>` 与 `beam <url>`（如 `beam http://localhost:3000`）两种调用形式
- 仅 HTTP/HTTPS 服务（占 90% 需求）
- 随机临时网址（`*.trycloudflare.com`）
- 首次运行**自动下载并缓存** `cloudflared` 二进制，用户零手动安装
- 终端打印公网网址的**二维码**，手机可直接扫码访问
- 前台运行，实时显示状态；`Ctrl+C` 干净退出（自动终止子进程）
- 彩色、友好的输出与错误提示

MVP 明确**不**包含（见路线图）：固定子域名、自定义域名、TCP（SSH/数据库）、配置文件多隧道、后台守护进程。

## 3. 架构

分层设计，关键在于用 `Tunnel` trait 抽象后端，使后续扩展不影响上层。

```
┌─────────────────────────────────────────────┐
│  CLI 层 (clap)                                │
│  解析参数、彩色输出、二维码渲染、信号处理         │
├─────────────────────────────────────────────┤
│  Tunnel trait        后端统一接口              │  ← 核心抽象
│   ├─ CloudflareBackend   (v0.1, HTTP)        │
│   ├─ BoreBackend         (v0.2, TCP)         │
│   └─ SelfHostedBackend   (远期, 可选)         │
├─────────────────────────────────────────────┤
│  BinaryManager                                │
│  检测 OS/arch、下载、缓存、校验、赋可执行权限     │
├─────────────────────────────────────────────┤
│  ProcessRunner                                │
│  spawn 子进程、流式读取 stdout、解析分配的 URL    │
└─────────────────────────────────────────────┘
```

### 3.1 `Tunnel` trait（核心抽象）

定义所有后端的统一接口。MVP 仅实现 `CloudflareBackend`，但接口预留，后续加 bore（TCP）、固定子域名、自定义域名时上层无需改动。

接口职责（概念性，非最终签名）：

- `start(local_target) -> Result<TunnelHandle>`：启动隧道，返回包含公网 URL 与生命周期控制的句柄
- `TunnelHandle`：持有公网 URL、子进程句柄，提供 `public_url()` 与 `shutdown()`

### 3.2 BinaryManager

- 检测当前 OS（macOS/Linux/Windows）与架构（x86_64/aarch64）
- 从 Cloudflare 官方发行渠道下载对应平台的 `cloudflared`
- 缓存到平台标准目录（用 `directories` crate，如 `~/.cache/beam/`）
- 校验下载完整性，赋予可执行权限
- 缓存命中时跳过下载

### 3.3 ProcessRunner

- 用 `tokio::process` 启动 `cloudflared tunnel --url http://localhost:<port>`
- 流式读取其 stdout/stderr
- 解析输出，抓取分配的 `https://*.trycloudflare.com` 网址
- 监控子进程存活；进程异常退出时上报友好错误

## 4. 技术栈

| 用途 | Crate |
|------|-------|
| 异步运行时 | `tokio`（含 `process`、`signal`） |
| CLI 解析 | `clap` |
| 下载二进制 | `reqwest` |
| 跨平台缓存路径 | `directories` |
| 二维码 | `qrcode` |
| 彩色输出 | `colored` 或 `owo-colors` |
| 错误处理 | `anyhow`（应用层）/ `thiserror`（库层错误类型） |

## 5. 错误处理

每类错误都给**人话提示 + 解决建议**，绝不直接抛栈：

- **本地端口无服务**：提示"localhost:3000 上似乎没有运行的服务，请先启动你的应用"
- **二进制下载失败**：提示网络问题，给出手动安装的备用指引
- **网络不通**：提示检查网络连接
- **cloudflared 异常退出**：捕获并展示其错误摘要，给出排查方向
- **URL 解析超时**：若一定时间内未抓到公网网址，提示并给出建议

## 6. 测试策略

- **BinaryManager 单测**：mock 下载、缓存命中、OS/arch 检测
- **URL 解析器单测**：用真实 `cloudflared` 输出样本作为 fixture
- **集成测试**：启动一个本地 HTTP server，跑完整流程，断言能拿到可访问的公网 URL（标记为需要网络的测试）

## 7. 路线图

| 版本 | 内容 |
|------|------|
| **v0.1 (MVP)** | `beam <port>`、Cloudflare 随机网址、HTTP、自动下载二进制、二维码、前台运行 |
| **v0.2** | 固定自选子域名；TCP 支持（引入 `BoreBackend`，可暴露 SSH/数据库等） |
| **v0.3** | 自定义域名（用户自有域名）；配置文件支持多隧道 |
| **远期** | 后台守护进程、开机自启、多隧道管理 |

## 8. 开源项目基础设施（实现时一并建立）

- `LICENSE`（建议 MIT 或 Apache-2.0，最大化"benefit 所有人"）
- `README.md`：安装、用法、示例、理念
- `CONTRIBUTING.md`
- CI：`cargo fmt --check`、`cargo clippy`、`cargo test`、跨平台构建
