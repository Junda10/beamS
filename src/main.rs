use clap::Parser;
use owo_colors::OwoColorize;

use beams::binary::{self, Tool};
use beams::cli;
use beams::output;
use beams::tunnel::{BoreBackend, CloudflareBackend, LocaltunnelBackend, Tunnel, TunnelHandle};

#[derive(Parser)]
#[command(
    name = "beams",
    version,
    about = "Share your localhost with the world — free, friendly, for everyone"
)]
struct Args {
    /// Port or local address, e.g. 3000 or http://localhost:3000
    target: String,

    /// Request a fixed subdomain over localtunnel, e.g. --subdomain myapp -> https://myapp.loca.lt
    #[arg(long, conflicts_with = "tcp")]
    subdomain: Option<String>,

    /// Expose a raw TCP port (SSH, databases, …) over bore.pub
    #[arg(long)]
    tcp: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("  {} Setting up tunnel...", "✓".green());

    // Build the backend from flags. `tcp_port` is Some(port) for TCP tunnels
    // (used for the TCP banner), None for HTTP tunnels.
    let (mut handle, tcp_port): (TunnelHandle, Option<u16>) = if args.tcp {
        let (_host, port) = cli::parse_host_port(&args.target)?;
        let bin = binary::ensure_binary(Tool::Bore).await?;
        let backend = BoreBackend {
            binary: bin,
            local_port: port,
        };
        (backend.start().await?, Some(port))
    } else if let Some(subdomain) = args.subdomain.clone() {
        let (host, port) = cli::parse_host_port(&args.target)?;
        let backend = LocaltunnelBackend {
            subdomain,
            local_host: host,
            local_port: port,
        };
        (backend.start().await?, None)
    } else {
        let target = cli::parse_target(&args.target)?;
        let bin = binary::ensure_binary(Tool::Cloudflared).await?;
        let backend = CloudflareBackend {
            binary: bin,
            target,
        };
        (backend.start().await?, None)
    };

    match tcp_port {
        Some(port) => output::print_tcp_banner(handle.public_url(), port),
        None => {
            // Show a tidy "localhost:PORT" forwarding line for HTTP tunnels.
            let forward = cli::parse_host_port(&args.target)
                .map(|(h, p)| format!("{h}:{p}"))
                .unwrap_or_else(|_| args.target.clone());
            output::print_banner(handle.public_url(), &forward)?;
        }
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\n  Stopping...");
            handle.shutdown().await;
        }
        _ = handle.wait() => {
            eprintln!("  tunnel closed");
        }
    }
    Ok(())
}
