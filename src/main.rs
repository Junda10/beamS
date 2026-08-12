use std::time::Duration;

use clap::Parser;
use owo_colors::OwoColorize;

use beams::binary::{self, Tool};
use beams::cli;
use beams::local;
use beams::output;
use beams::tunnel::{BoreBackend, CloudflareBackend, LocaltunnelBackend, Tunnel};

#[derive(Parser)]
#[command(
    name = "beams",
    version,
    about = "Share your localhost with the world — free, friendly, for everyone"
)]
struct Args {
    /// Port or local address, e.g. 3000 or http://localhost:3000.
    /// Omit it and beams looks for a dev server on the usual ports.
    target: Option<String>,

    /// Request a fixed subdomain over localtunnel, e.g. --subdomain myapp -> https://myapp.loca.lt
    #[arg(long, conflicts_with = "tcp")]
    subdomain: Option<String>,

    /// Expose a raw TCP port (SSH, databases, …) over bore.pub
    #[arg(long)]
    tcp: bool,

    /// Open the public URL in your browser once the tunnel is up
    #[arg(long)]
    open: bool,
}

/// Resolve when the process is asked to terminate: Ctrl+C (SIGINT), SIGTERM, or
/// SIGHUP (closing the terminal). Catching these lets us kill the child tunnel
/// process instead of orphaning it — `kill_on_drop` does not run on a signal.
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut hup = signal(SignalKind::hangup()).expect("install SIGHUP handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
        _ = hup.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Say which build this is — first thing anyone needs when a tunnel misbehaves.
    println!(
        "  {} {}",
        "beams".bold(),
        concat!("v", env!("CARGO_PKG_VERSION")).dimmed()
    );

    // No target given? Use whichever common dev port is actually serving.
    let target = match args.target.clone() {
        Some(t) => t,
        None => match local::detect_port().await {
            Some(port) => {
                println!("  {} Found a local server on port {port}", "✓".green());
                port.to_string()
            }
            None => anyhow::bail!(
                "no local server found on the usual ports ({}) — pass one explicitly, e.g. `beams 3000`",
                local::COMMON_PORTS
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
    };

    // Check the local side first: a tunnel to nothing just moves the failure to
    // whoever opens the link.
    let (host, port) = cli::parse_host_port(&target)?;
    if !local::is_listening(&host, port).await {
        anyhow::bail!(
            "nothing is listening on {host}:{port} — start your server, then run beams again"
        );
    }

    println!("  {} Setting up tunnel...", "✓".green());

    let backend: Box<dyn Tunnel + Send + Sync> = if args.tcp {
        let bin = binary::ensure_binary(Tool::Bore).await?;
        Box::new(BoreBackend {
            binary: bin,
            local_port: port,
        })
    } else if let Some(subdomain) = args.subdomain.clone() {
        Box::new(LocaltunnelBackend {
            subdomain,
            local_host: host.clone(),
            local_port: port,
        })
    } else {
        let bin = binary::ensure_binary(Tool::Cloudflared).await?;
        Box::new(CloudflareBackend {
            binary: bin,
            target: cli::parse_target(&target)?,
        })
    };

    let forward = format!("{host}:{port}");
    let mut first_run = true;
    let mut failures = 0;

    loop {
        let mut handle = match backend.start().await {
            Ok(handle) => {
                failures = 0;
                handle
            }
            Err(e) => {
                failures += 1;
                if failures >= 5 {
                    return Err(e.into());
                }
                eprintln!("  {} {e} — retrying in 2s ({failures}/5)", "!".yellow());
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        // Wait until the tunnel is actually reachable before showing the address,
        // so it works the moment the user opens it (quick tunnels need a few seconds
        // for DNS/edge propagation).
        println!("  {} Waiting for the tunnel to come online...", "…".cyan());
        let ready = beams::ready::wait_until_ready(handle.public_url(), args.tcp).await;

        let copied = local::copy_to_clipboard(handle.public_url()).await;
        if args.tcp {
            output::print_tcp_banner(handle.public_url(), port, copied, ready);
        } else {
            output::print_banner(handle.public_url(), &forward, copied, ready)?;
            if args.open && first_run {
                local::open_in_browser(handle.public_url());
            }
        }
        if !ready {
            // Almost always a stale negative DNS entry: something looked the
            // hostname up before it was registered, and quick-tunnel zones cache
            // NXDOMAIN for 30 minutes.
            println!(
                "\n  {} If it won't open, flush this machine's DNS cache:\n      {}",
                "!".yellow(),
                if cfg!(target_os = "macos") {
                    "sudo killall -HUP mDNSResponder"
                } else {
                    "sudo resolvectl flush-caches"
                }
                .bold()
            );
        }
        first_run = false;

        tokio::select! {
            _ = shutdown_signal() => {
                println!("\n  Stopping...");
                handle.shutdown().await;
                return Ok(());
            }
            // The relay dropped us (quick tunnels do this on long runs) — dial
            // again instead of making the user re-run the command. The public
            // URL changes, so the banner is reprinted.
            _ = handle.wait() => {
                println!("\n  {} Tunnel dropped — reconnecting...", "…".yellow());
            }
        }
    }
}
