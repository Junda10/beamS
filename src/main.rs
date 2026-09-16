use std::io::{BufRead, IsTerminal, Write};
use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use owo_colors::OwoColorize;

use beams::binary::{self, Tool};
use beams::cli;
use beams::error::BeamsError;
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

    /// Pin the cloudflared edge transport. `quic` is fast but needs UDP/7844;
    /// use `http2` on networks that throttle or block UDP.
    #[arg(long, value_parser = ["quic", "http2", "auto"])]
    protocol: Option<String>,
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

/// List the live ports and ask which one to share. Re-asks on a bad number.
fn prompt_for_port(ports: &[u16]) -> anyhow::Result<String> {
    println!("  {} Found local servers on several ports:", "✓".green());
    for (i, port) in ports.iter().enumerate() {
        println!(
            "    {}  http://localhost:{port}",
            format!("{})", i + 1).bold()
        );
    }
    loop {
        print!("  Which one? [1-{}, or type a port] (1): ", ports.len());
        std::io::stdout().flush()?;
        let mut answer = String::new();
        if std::io::stdin().lock().read_line(&mut answer)? == 0 {
            anyhow::bail!("no port chosen");
        }
        match cli::pick_target(&answer, ports) {
            Some(target) => return Ok(target),
            None => println!("  {} pick a number from the list", "✗".red()),
        }
    }
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

    // No target given? Use whichever common dev port is actually serving, and
    // let the user choose when more than one is.
    let mut target = match args.target.clone() {
        Some(t) => t,
        None => {
            let live = local::detect_ports().await;
            match live.as_slice() {
                [] => anyhow::bail!(
                    "no local server found on the usual ports ({}) — pass one explicitly, e.g. `beams 3000`",
                    local::COMMON_PORTS
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                [port] => {
                    println!("  {} Found a local server on port {port}", "✓".green());
                    port.to_string()
                }
                // Piped / CI: nobody to ask, keep the old first-match behaviour.
                [port, ..] if !std::io::stdin().is_terminal() => {
                    println!("  {} Found a local server on port {port}", "✓".green());
                    port.to_string()
                }
                _ => prompt_for_port(&live)?,
            }
        }
    };

    // Check the local side first: a tunnel to nothing just moves the failure to
    // whoever opens the link.
    let (mut host, port) = cli::parse_host_port(&target)?;
    let Some(addr) = local::listening_addr(&host, port).await else {
        anyhow::bail!(
            "nothing is listening on {host}:{port} — start your server, then run beams again"
        );
    };
    // Forward to the address that answered. `localhost` may resolve to an empty
    // ::1 first, and the tunnel client would then 502 against a working server.
    if host == "localhost" {
        host = match addr {
            SocketAddr::V4(a) => a.ip().to_string(),
            SocketAddr::V6(a) => format!("[{}]", a.ip()),
        };
        target = cli::parse_target(&target)?.replacen("://localhost:", &format!("://{host}:"), 1);
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
            protocol: args.protocol.clone(),
        })
    };

    let forward = format!("{host}:{port}");

    // From outside, a dead local server just looks like a broken tunnel (the
    // public URL answers 502), so say so here where the user can act on it.
    tokio::spawn(watch_local(addr));

    // Race the whole session against Ctrl+C. Once a signal handler exists the
    // default "kill the process" is gone, so dialing, readiness and backoff must
    // be interruptible too, not just the steady state. Dropping the session
    // drops the handle, and `kill_on_drop` stops the tunnel process.
    let session = async {
        let mut first_run = true;
        let mut failures: u32 = 0;

        loop {
            let started = match backend.start().await {
                Ok(mut handle) => {
                    let public_url = handle.public_url().to_string();
                    // Wait until the tunnel is actually reachable before showing
                    // the address, so it works the moment the user opens it
                    // (quick tunnels need a few seconds for DNS/edge propagation).
                    // If the process dies meanwhile, stop waiting on a dead tunnel.
                    println!("  {} Waiting for the tunnel to come online...", "…".cyan());
                    let ready = tokio::select! {
                        ready = beams::ready::wait_until_ready(&public_url, args.tcp) => Some(ready),
                        _ = handle.wait() => None,
                    };
                    match ready {
                        Some(ready) => Ok((handle, public_url, ready)),
                        None => Err(BeamsError::TunnelStart(
                            "the tunnel process exited while coming online".to_string(),
                        )),
                    }
                }
                Err(e) => Err(e),
            };

            let (mut handle, public_url, ready) = match started {
                Ok(started) => {
                    failures = 0;
                    started
                }
                Err(e) => {
                    failures += 1;
                    if failures >= 5 {
                        return anyhow::Result::<()>::Err(e.into());
                    }
                    // Back off so a relay that is refusing everyone right now gets
                    // room to recover, instead of five dials inside ten seconds.
                    // failures is 1..=4 here, so this is 2s, 4s, 8s, 16s.
                    let wait = Duration::from_secs(1u64 << failures);
                    eprintln!(
                        "  {} {e} — retrying in {}s ({failures}/5)",
                        "!".yellow(),
                        wait.as_secs()
                    );
                    tokio::time::sleep(wait).await;
                    continue;
                }
            };

            let copied = local::copy_to_clipboard(&public_url).await;
            if args.tcp {
                output::print_tcp_banner(&public_url, port, copied, ready);
            } else {
                output::print_banner(&public_url, &forward, copied, ready)?;
                if args.open && first_run {
                    local::open_in_browser(&public_url);
                }
            }
            if !ready {
                // Almost always a stale negative DNS entry: something looked the
                // hostname up before it was registered, and quick-tunnel zones
                // cache NXDOMAIN for 30 minutes.
                println!(
                    "\n  {} If it won't open, flush this machine's DNS cache:\n      {}",
                    "!".yellow(),
                    if cfg!(target_os = "macos") {
                        "sudo killall -HUP mDNSResponder"
                    } else if cfg!(windows) {
                        "ipconfig /flushdns"
                    } else {
                        "sudo resolvectl flush-caches"
                    }
                    .bold()
                );
            }
            first_run = false;

            tokio::select! {
                // The process is still alive but the edge stopped routing to it.
                // Nothing else ends the wait below, so tear it down ourselves.
                _ = beams::ready::watch_until_dead(&public_url, args.tcp) => {
                    println!("\n  {} Tunnel stopped responding — reconnecting...", "…".yellow());
                    handle.shutdown().await;
                }
                // The relay dropped us (quick tunnels do this on long runs) — dial
                // again instead of making the user re-run the command. The public
                // URL changes, so the banner is reprinted.
                _ = handle.wait() => {
                    println!("\n  {} Tunnel dropped — reconnecting...", "…".yellow());
                }
            }
        }
    };

    tokio::select! {
        result = session => result,
        _ = shutdown_signal() => {
            println!("\n  Stopping...");
            Ok(())
        }
    }
}

/// Poll the local server and report when it stops answering or comes back.
async fn watch_local(addr: SocketAddr) {
    let host = addr.ip().to_string();
    let mut up = true;
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if local::is_listening(&host, addr.port()).await == up {
            continue;
        }
        up = !up;
        if up {
            println!("  {} Local server on {addr} is back", "✓".green());
        } else {
            println!(
                "  {} Local server on {addr} stopped responding — visitors get errors until it is back",
                "!".yellow()
            );
        }
    }
}
