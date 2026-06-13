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
