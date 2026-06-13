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
    about = "Share your localhost with the world — free, friendly, for everyone"
)]
struct Args {
    /// Port or local address, e.g. 3000 or http://localhost:3000
    target: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let target = cli::parse_target(&args.target)?;

    println!("  {} Setting up tunnel...", "✓".green());
    let bin = binary::ensure_binary().await?;

    let backend = CloudflareBackend { binary: bin };
    let mut handle = backend.start(&target).await?;

    output::print_banner(handle.public_url(), &target)?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\n  Stopping...");
            let _ = handle.shutdown().await;
        }
        result = handle.wait() => {
            match result {
                Ok(status) => eprintln!("  cloudflared exited ({status})"),
                Err(e) => eprintln!("  error while monitoring cloudflared: {e}"),
            }
        }
    }
    Ok(())
}
