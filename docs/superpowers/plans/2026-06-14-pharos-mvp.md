# pharos v0.1 MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `pharos`, a Rust CLI that exposes a local HTTP service to a public `*.trycloudflare.com` URL with one command, auto-downloading `cloudflared` and showing a QR code.

**Architecture:** A small Rust library crate (`pharos`) of focused, individually-testable modules — pure logic (target parsing, URL extraction, platform→asset mapping, QR rendering) plus an async `Tunnel` trait whose only MVP implementation, `CloudflareBackend`, spawns and supervises `cloudflared`. A thin binary (`main.rs`) wires them together and handles Ctrl+C.

**Tech Stack:** Rust 2021, `tokio` (async + process + signal), `clap` (CLI), `reqwest` (binary download), `directories` (cache path), `flate2` + `tar` (macOS `.tgz` extraction), `qrcode` (QR), `owo-colors` (color), `anyhow` + `thiserror` (errors), `async-trait`.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `Cargo.toml` | Crate metadata + dependencies; declares both lib and bin |
| `src/lib.rs` | Declares public modules |
| `src/error.rs` | `PharosError` enum + `Result` alias |
| `src/cli.rs` | `parse_target` — normalize port/URL input to a forwarding URL |
| `src/parser.rs` | `extract_public_url` — pull the `*.trycloudflare.com` URL out of a log line |
| `src/binary.rs` | `cloudflared_asset`, `download_url`, `cache_dir`, `binary_path`, `ensure_binary` |
| `src/output.rs` | `render_qr`, `print_banner` — friendly terminal output |
| `src/tunnel.rs` | `Tunnel` trait, `TunnelHandle`, `CloudflareBackend` |
| `src/main.rs` | CLI entry: parse → ensure binary → start tunnel → print → wait for Ctrl+C |
| `tests/e2e.rs` | Network-gated end-to-end test (ignored by default) |

Each pure-logic module is unit-tested inline with `#[cfg(test)]`. The spawning/download paths are exercised by the ignored e2e test.

---

## Task 1: Project scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/error.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "pharos"
version = "0.1.0"
edition = "2021"
description = "Share your localhost to the world — free, friendly, for everyone."
license = "MIT"
repository = "https://github.com/Junda10/beamS"

[lib]
name = "pharos"
path = "src/lib.rs"

[[bin]]
name = "pharos"
path = "src/main.rs"

[dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread", "process", "io-util", "signal", "time"] }
clap = { version = "4", features = ["derive"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
directories = "5"
qrcode = "0.14"
owo-colors = "4"
anyhow = "1"
thiserror = "1"
async-trait = "0.1"
flate2 = "1"
tar = "0.4"
```

- [ ] **Step 2: Write `src/error.rs`**

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PharosError {
    #[error("无法识别的输入 \"{0}\"，请给出端口号(如 3000)或地址(如 http://localhost:3000)")]
    InvalidTarget(String),
    #[error("下载 cloudflared 失败: {0}")]
    Download(String),
    #[error("启动隧道失败: {0}")]
    TunnelStart(String),
    #[error("等待公网地址超时（30 秒内 cloudflared 未返回地址）")]
    UrlTimeout,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, PharosError>;
```

- [ ] **Step 3: Write `src/lib.rs`**

```rust
pub mod error;
pub mod cli;
pub mod parser;
pub mod binary;
pub mod output;
pub mod tunnel;
```

- [ ] **Step 4: Write a temporary `src/main.rs` so the crate builds**

```rust
fn main() {
    println!("pharos");
}
```

- [ ] **Step 5: Create empty module files so `lib.rs` compiles**

Create `src/cli.rs`, `src/parser.rs`, `src/binary.rs`, `src/output.rs`, `src/tunnel.rs`, each containing only:

```rust
// filled in by a later task
```

- [ ] **Step 6: Verify it builds**

Run: `cargo build`
Expected: compiles (empty modules are valid). Warnings about unused crates are fine.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/
git commit -m "chore: scaffold pharos crate (lib + bin + error type)"
```

---

## Task 2: Target normalization (`cli::parse_target`)

**Files:**
- Modify: `src/cli.rs`

- [ ] **Step 1: Write the failing tests in `src/cli.rs`**

```rust
use crate::error::{PharosError, Result};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_port_becomes_localhost_url() {
        assert_eq!(parse_target("3000").unwrap(), "http://localhost:3000");
    }

    #[test]
    fn full_http_url_is_unchanged() {
        assert_eq!(parse_target("http://localhost:8080").unwrap(), "http://localhost:8080");
    }

    #[test]
    fn full_https_url_is_unchanged() {
        assert_eq!(parse_target("https://127.0.0.1:8080").unwrap(), "https://127.0.0.1:8080");
    }

    #[test]
    fn host_port_without_scheme_gets_http() {
        assert_eq!(parse_target("localhost:3000").unwrap(), "http://localhost:3000");
    }

    #[test]
    fn port_zero_is_invalid() {
        assert!(matches!(parse_target("0"), Err(PharosError::InvalidTarget(_))));
    }

    #[test]
    fn garbage_is_invalid() {
        assert!(matches!(parse_target("not a target"), Err(PharosError::InvalidTarget(_))));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib cli`
Expected: FAIL — `parse_target` not found.

- [ ] **Step 3: Implement `parse_target` above the `#[cfg(test)]` block**

```rust
/// Normalize user input into a URL `cloudflared` can forward to.
///
/// - `"3000"`              -> `"http://localhost:3000"`
/// - `"http://..."`        -> unchanged
/// - `"localhost:3000"`    -> `"http://localhost:3000"`
pub fn parse_target(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(PharosError::InvalidTarget(input.to_string()));
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        return Ok(input.to_string());
    }
    // bare port: all digits, 1..=65535
    if input.chars().all(|c| c.is_ascii_digit()) {
        match input.parse::<u32>() {
            Ok(p) if (1..=65535).contains(&p) => return Ok(format!("http://localhost:{p}")),
            _ => return Err(PharosError::InvalidTarget(input.to_string())),
        }
    }
    // host:port without scheme -> assume http
    if let Some((host, port)) = input.rsplit_once(':') {
        if !host.is_empty() && port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
            return Ok(format!("http://{input}"));
        }
    }
    Err(PharosError::InvalidTarget(input.to_string()))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib cli`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs
git commit -m "feat: normalize port/url input into a forwarding target"
```

---

## Task 3: Public URL extraction (`parser::extract_public_url`)

**Files:**
- Modify: `src/parser.rs`

- [ ] **Step 1: Write the failing tests in `src/parser.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_url_from_boxed_log_line() {
        let line = "2024-01-01T00:00:00Z INF |  https://happy-cat-42.trycloudflare.com   |";
        assert_eq!(
            extract_public_url(line).as_deref(),
            Some("https://happy-cat-42.trycloudflare.com")
        );
    }

    #[test]
    fn extracts_plain_url() {
        let line = "https://blue-tree-7.trycloudflare.com";
        assert_eq!(
            extract_public_url(line).as_deref(),
            Some("https://blue-tree-7.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_non_trycloudflare_urls() {
        let line = "INF connecting to https://api.cloudflare.com/foo";
        assert_eq!(extract_public_url(line), None);
    }

    #[test]
    fn ignores_lines_without_url() {
        assert_eq!(extract_public_url("INF starting tunnel"), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib parser`
Expected: FAIL — `extract_public_url` not found.

- [ ] **Step 3: Implement `extract_public_url`**

```rust
/// Pull a `https://<sub>.trycloudflare.com` URL out of a cloudflared log line.
/// Returns `None` if the line contains no such URL.
pub fn extract_public_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.ends_with(".trycloudflare.com") {
        Some(url.to_string())
    } else {
        None
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib parser`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/parser.rs
git commit -m "feat: extract trycloudflare public url from log lines"
```

---

## Task 4: Platform → cloudflared asset mapping (`binary` pure parts)

**Files:**
- Modify: `src/binary.rs`

- [ ] **Step 1: Write the failing tests in `src/binary.rs`**

```rust
use crate::error::{PharosError, Result};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_arm_asset() {
        assert_eq!(cloudflared_asset("macos", "aarch64").unwrap(), "cloudflared-darwin-arm64.tgz");
    }

    #[test]
    fn macos_intel_asset() {
        assert_eq!(cloudflared_asset("macos", "x86_64").unwrap(), "cloudflared-darwin-amd64.tgz");
    }

    #[test]
    fn linux_amd64_asset() {
        assert_eq!(cloudflared_asset("linux", "x86_64").unwrap(), "cloudflared-linux-amd64");
    }

    #[test]
    fn windows_asset() {
        assert_eq!(cloudflared_asset("windows", "x86_64").unwrap(), "cloudflared-windows-amd64.exe");
    }

    #[test]
    fn unsupported_platform_errors() {
        assert!(matches!(cloudflared_asset("plan9", "sparc"), Err(PharosError::Download(_))));
    }

    #[test]
    fn download_url_points_at_latest_release() {
        assert_eq!(
            download_url("cloudflared-linux-amd64"),
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64"
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib binary`
Expected: FAIL — functions not found.

- [ ] **Step 3: Implement the pure functions**

```rust
/// Map (OS, arch) — as given by `std::env::consts::{OS, ARCH}` — to the
/// cloudflared release asset filename.
pub fn cloudflared_asset(os: &str, arch: &str) -> Result<&'static str> {
    Ok(match (os, arch) {
        ("macos", "x86_64") => "cloudflared-darwin-amd64.tgz",
        ("macos", "aarch64") => "cloudflared-darwin-arm64.tgz",
        ("linux", "x86_64") => "cloudflared-linux-amd64",
        ("linux", "aarch64") => "cloudflared-linux-arm64",
        ("windows", "x86_64") => "cloudflared-windows-amd64.exe",
        _ => return Err(PharosError::Download(format!("暂不支持的平台: {os}/{arch}"))),
    })
}

/// Build the download URL for a given asset from cloudflared's latest release.
pub fn download_url(asset: &str) -> String {
    format!("https://github.com/cloudflare/cloudflared/releases/latest/download/{asset}")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib binary`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/binary.rs
git commit -m "feat: map platform to cloudflared release asset"
```

---

## Task 5: Cache paths + binary download (`binary::ensure_binary`)

**Files:**
- Modify: `src/binary.rs`

- [ ] **Step 1: Write the failing test for cache paths (append inside the existing `tests` mod in `src/binary.rs`)**

```rust
    #[test]
    fn binary_path_lives_under_cache_dir_and_is_named_cloudflared() {
        let dir = cache_dir().unwrap();
        let path = binary_path().unwrap();
        assert!(path.starts_with(&dir));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name == "cloudflared" || name == "cloudflared.exe");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib binary::tests::binary_path`
Expected: FAIL — `cache_dir` / `binary_path` not found.

- [ ] **Step 3: Add imports at the top of `src/binary.rs` (above the pure functions from Task 4)**

```rust
use std::path::{Path, PathBuf};
```

- [ ] **Step 4: Implement cache path helpers**

```rust
/// Directory where pharos caches the downloaded cloudflared binary.
pub fn cache_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "pharos", "pharos")
        .ok_or_else(|| PharosError::Download("无法定位缓存目录".to_string()))?;
    Ok(dirs.cache_dir().to_path_buf())
}

/// Full path to the cached cloudflared binary.
pub fn binary_path() -> Result<PathBuf> {
    let name = if cfg!(windows) { "cloudflared.exe" } else { "cloudflared" };
    Ok(cache_dir()?.join(name))
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib binary::tests::binary_path`
Expected: PASS.

- [ ] **Step 6: Implement `ensure_binary` + tgz extraction (no unit test — covered by the e2e test in Task 9; this is I/O against the network)**

```rust
/// Ensure cloudflared exists in the cache, downloading it on first use.
/// Returns the path to the executable.
pub async fn ensure_binary() -> Result<PathBuf> {
    let path = binary_path()?;
    if path.exists() {
        return Ok(path);
    }
    let asset = cloudflared_asset(std::env::consts::OS, std::env::consts::ARCH)?;
    let url = download_url(asset);
    std::fs::create_dir_all(cache_dir()?)?;

    let bytes = reqwest::get(&url)
        .await
        .map_err(|e| PharosError::Download(e.to_string()))?
        .error_for_status()
        .map_err(|e| PharosError::Download(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| PharosError::Download(e.to_string()))?;

    if asset.ends_with(".tgz") {
        extract_cloudflared_from_tgz(&bytes, &path)?;
    } else {
        std::fs::write(&path, &bytes)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(path)
}

/// Extract the `cloudflared` entry from a `.tgz` archive into `dest`.
fn extract_cloudflared_from_tgz(bytes: &[u8], dest: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;
    let mut archive = Archive::new(GzDecoder::new(bytes));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let is_cloudflared = entry
            .path()?
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == "cloudflared")
            .unwrap_or(false);
        if is_cloudflared {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    Err(PharosError::Download("压缩包中未找到 cloudflared".to_string()))
}
```

- [ ] **Step 7: Verify the whole crate still builds**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 8: Commit**

```bash
git add src/binary.rs
git commit -m "feat: auto-download and cache cloudflared binary"
```

---

## Task 6: Friendly output (`output::render_qr`, `output::print_banner`)

**Files:**
- Modify: `src/output.rs`

- [ ] **Step 1: Write the failing test in `src/output.rs`**

```rust
use crate::error::{PharosError, Result};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_qr_returns_non_empty_block() {
        let qr = render_qr("https://happy-cat-42.trycloudflare.com").unwrap();
        assert!(!qr.trim().is_empty());
        // Unicode QR uses block glyphs; the output should contain newlines (multi-row).
        assert!(qr.contains('\n'));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib output`
Expected: FAIL — `render_qr` not found.

- [ ] **Step 3: Implement `render_qr` and `print_banner`**

```rust
/// Render a URL as a scannable Unicode QR code string.
pub fn render_qr(url: &str) -> Result<String> {
    use qrcode::render::unicode;
    use qrcode::QrCode;
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| PharosError::Download(format!("生成二维码失败: {e}")))?;
    Ok(code
        .render::<unicode::Dense1x2>()
        .quiet_zone(true)
        .build())
}

/// Print the success banner with the public URL, forwarding target, and QR code.
pub fn print_banner(public_url: &str, target: &str) -> Result<()> {
    use owo_colors::OwoColorize;
    println!("  {} 你的本地服务已上线！\n", "✓".green());
    println!("  🌐 公网地址:  {}", public_url.bold().cyan());
    println!("  📍 转发到:    {}\n", target);
    println!("{}", render_qr(public_url)?);
    println!("  手机扫码即可访问 · 按 {} 停止", "Ctrl+C".yellow());
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib output`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/output.rs
git commit -m "feat: friendly banner output with QR code"
```

---

## Task 7: Tunnel trait + CloudflareBackend (`src/tunnel.rs`)

**Files:**
- Modify: `src/tunnel.rs`

- [ ] **Step 1: Implement the trait, handle, and backend**

No unit test here — spawning a real `cloudflared` process is exercised by the e2e test in Task 9. Write the implementation directly:

```rust
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::error::{PharosError, Result};
use crate::parser::extract_public_url;

/// A running tunnel: owns the child process and exposes its public URL.
pub struct TunnelHandle {
    public_url: String,
    child: Child,
}

impl TunnelHandle {
    pub fn public_url(&self) -> &str {
        &self.public_url
    }

    /// Wait for the underlying process to exit.
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    /// Terminate the tunnel.
    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        self.child.kill().await
    }
}

/// A tunnel backend. The MVP ships only `CloudflareBackend`; this trait is the
/// extension point for future backends (bore/TCP, self-hosted, etc.).
#[async_trait::async_trait]
pub trait Tunnel {
    async fn start(&self, target: &str) -> Result<TunnelHandle>;
}

/// Backend that wraps the `cloudflared` quick-tunnel command.
pub struct CloudflareBackend {
    pub binary: PathBuf,
}

#[async_trait::async_trait]
impl Tunnel for CloudflareBackend {
    async fn start(&self, target: &str) -> Result<TunnelHandle> {
        let mut child = Command::new(&self.binary)
            .args(["tunnel", "--no-autoupdate", "--url", target])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| PharosError::TunnelStart(e.to_string()))?;

        // cloudflared prints the assigned URL to stderr.
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| PharosError::TunnelStart("无法读取 cloudflared 输出".to_string()))?;
        let mut lines = BufReader::new(stderr).lines();

        let found = tokio::time::timeout(Duration::from_secs(30), async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(url) = extract_public_url(&line) {
                    return Some(url);
                }
            }
            None
        })
        .await
        .map_err(|_| PharosError::UrlTimeout)?;

        match found {
            Some(url) => Ok(TunnelHandle { public_url: url, child }),
            None => Err(PharosError::TunnelStart(
                "cloudflared 退出但未提供公网地址".to_string(),
            )),
        }
    }
}
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add src/tunnel.rs
git commit -m "feat: Tunnel trait and CloudflareBackend (spawn + url capture)"
```

---

## Task 8: CLI entry point (`src/main.rs`)

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace the placeholder `main.rs` with the real entry point**

```rust
use clap::Parser;
use owo_colors::OwoColorize;

use pharos::binary;
use pharos::cli;
use pharos::output;
use pharos::tunnel::{CloudflareBackend, Tunnel};

#[derive(Parser)]
#[command(
    name = "pharos",
    version,
    about = "把你的 localhost 分享到全世界 — 免费、友好、属于每个人"
)]
struct Args {
    /// 端口号或本地地址，例如 3000 或 http://localhost:3000
    target: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let target = cli::parse_target(&args.target)?;

    println!("  {} 正在准备隧道...", "✓".green());
    let bin = binary::ensure_binary().await?;

    let backend = CloudflareBackend { binary: bin };
    let mut handle = backend.start(&target).await?;

    output::print_banner(handle.public_url(), &target)?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\n  正在停止...");
            let _ = handle.shutdown().await;
        }
        result = handle.wait() => {
            match result {
                Ok(status) => eprintln!("  cloudflared 已退出 ({status})"),
                Err(e) => eprintln!("  cloudflared 监控出错: {e}"),
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Verify it builds and the help renders**

Run: `cargo run -- --help`
Expected: prints usage with `把你的 localhost 分享到全世界` and a `target` argument.

- [ ] **Step 3: Verify invalid input gives a friendly error**

Run: `cargo run -- "not a target"`
Expected: exits non-zero printing `无法识别的输入 "not a target"...`.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire up CLI entry point with Ctrl+C handling"
```

---

## Task 9: End-to-end test, docs, and CI

**Files:**
- Create: `tests/e2e.rs`
- Create: `LICENSE`
- Modify: `README.md`
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write the network-gated e2e test**

```rust
// tests/e2e.rs
//
// Full path: starts a local HTTP server, downloads cloudflared, opens a tunnel,
// and asserts the public URL serves the local content. Requires network +
// outbound access, so it is #[ignore]d by default. Run with:
//   cargo test --test e2e -- --ignored --nocapture
use std::time::Duration;

use pharos::binary;
use pharos::tunnel::{CloudflareBackend, Tunnel};

#[tokio::test]
#[ignore]
async fn tunnel_serves_local_http() {
    // Start a tiny local server on an ephemeral port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
                    .await;
            }
        }
    });

    let bin = binary::ensure_binary().await.expect("download cloudflared");
    let backend = CloudflareBackend { binary: bin };
    let mut handle = backend
        .start(&format!("http://localhost:{port}"))
        .await
        .expect("start tunnel");

    let url = handle.public_url().to_string();
    assert!(url.ends_with(".trycloudflare.com"), "got: {url}");

    // Quick tunnels take a few seconds to become reachable; retry briefly.
    let client = reqwest::Client::new();
    let mut last = String::new();
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(body) = resp.text().await {
                last = body;
                if last.contains("hello") {
                    break;
                }
            }
        }
    }
    let _ = handle.shutdown().await;
    assert!(last.contains("hello"), "tunnel did not serve local content: {last:?}");
}
```

- [ ] **Step 2: Add `reqwest` as a dev-dependency for the test client**

Add to `Cargo.toml`:

```toml
[dev-dependencies]
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
```

- [ ] **Step 3: Run the e2e test (requires network)**

Run: `cargo test --test e2e -- --ignored --nocapture`
Expected: PASS — downloads cloudflared, prints a `*.trycloudflare.com` URL, fetches `hello` through it. (If the network blocks outbound, skip and note it.)

- [ ] **Step 4: Confirm the default test suite stays green and fast**

Run: `cargo test`
Expected: PASS — all unit tests pass; the e2e test is ignored.

- [ ] **Step 5: Write `LICENSE` (MIT)**

```
MIT License

Copyright (c) 2026 pharos contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 6: Rewrite `README.md` with real usage**

````markdown
# pharos

> Like the lighthouse of Alexandria — make your localhost visible to the world.

`pharos` shares a local HTTP service to a public `https://*.trycloudflare.com`
URL with one command. Free forever, no signup. It auto-downloads `cloudflared`
on first run and prints a QR code so you can open the URL on your phone.

## Install

```bash
cargo install --path .
```

## Usage

```bash
pharos 3000                       # forwards http://localhost:3000
pharos http://localhost:8080      # explicit URL
```

Press `Ctrl+C` to stop. The public URL is temporary and changes each run.

## How it works

`pharos` wraps Cloudflare Quick Tunnel: your machine dials out to Cloudflare,
which assigns a public HTTPS URL and relays traffic back to your localhost. No
inbound ports, no account, no cost.

## Roadmap

- v0.2 — fixed custom subdomain; TCP support (SSH/databases)
- v0.3 — bring-your-own domain; config file for multiple tunnels
- later — background daemon

## License

MIT
````

- [ ] **Step 7: Write the CI workflow**

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
```

- [ ] **Step 8: Run formatting and lint locally before committing**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: no formatting diff; clippy passes with no warnings.

- [ ] **Step 9: Commit**

```bash
git add tests/e2e.rs Cargo.toml LICENSE README.md .github/
git commit -m "test: e2e tunnel test; docs: README + MIT license; ci: fmt/clippy/test"
```

---

## Self-Review

**Spec coverage:**
- Free/sustainable via Cloudflare wrapping → Tasks 5, 7 ✓
- `pharos <port>` / `pharos <url>` → Task 2 (`parse_target`) + Task 8 ✓
- HTTP only (MVP) → `cloudflared tunnel --url` in Task 7 ✓
- Random `*.trycloudflare.com` URL → Task 3 (parse) + Task 7 (capture) ✓
- Auto-download + cache cloudflared, zero manual install → Task 5 ✓
- QR code in MVP → Task 6 ✓
- Foreground run + clean Ctrl+C shutdown → Task 8 ✓
- Friendly colored output + human error messages → Task 1 (`PharosError`), Task 6, Task 8 ✓
- `Tunnel` trait abstraction for future backends → Task 7 ✓
- Testing strategy (unit for pure logic, network-gated e2e) → Tasks 2,3,4,5,6,9 ✓
- Open-source infra (LICENSE, README, CI) → Task 9 ✓

No spec requirement is left without a task.

**Placeholder scan:** No TBD/TODO/"handle edge cases" — every code step contains complete code. (`src/main.rs` and module files start as deliberate placeholders in Task 1 and are filled by later tasks.)

**Type consistency:** `parse_target -> Result<String>`, `extract_public_url -> Option<String>`, `cloudflared_asset -> Result<&'static str>`, `download_url -> String`, `cache_dir/binary_path -> Result<PathBuf>`, `ensure_binary -> Result<PathBuf>`, `render_qr -> Result<String>`, `print_banner -> Result<()>`, `Tunnel::start -> Result<TunnelHandle>`, `TunnelHandle::{public_url, wait, shutdown}` — all consistent across Tasks 7, 8, 9. `Result` is `pharos::error::Result` in the lib; `main.rs` and `tests/e2e.rs` use `anyhow::Result` / `expect`, converting via `PharosError: std::error::Error`.
