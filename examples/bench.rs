// Tunnel success-rate benchmark against the real relays (needs network).
//
//   cargo run --release --example bench -- cloudflare 20
//   cargo run --release --example bench -- cloudflare 20 http2
//   cargo run --release --example bench -- bore 20
//   cargo run --release --example bench -- localtunnel 20
//
// Each run opens a fresh tunnel to a local "hello" server and records three
// stages, so a failure says where it happened:
//   url   - the backend handed back a public address
//   ready - beams' own readiness check said yes (what the user is told)
//   serve - a fresh visitor actually got "hello" back through the tunnel
use std::time::{Duration, Instant};

use beams::binary::{ensure_binary, Tool};
use beams::tunnel::{BoreBackend, CloudflareBackend, LocaltunnelBackend, Tunnel};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
struct Run {
    url: Option<f64>,
    ready: Option<bool>,
    ready_s: f64,
    serve: Option<f64>,
    err: String,
}

async fn hello_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                // Drain the request first: on Windows, closing with unread input
                // sends an RST that discards the response.
                let _ = sock.read(&mut [0u8; 4096]).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .await;
                let _ = sock.shutdown().await;
            });
        }
    });
    port
}

/// One visitor request through the tunnel, as someone who just got the link.
async fn visit(url: &str, tcp: bool) -> bool {
    if tcp {
        let fut = async {
            let mut s = tokio::net::TcpStream::connect(url).await.ok()?;
            s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .ok()?;
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await.ok()?;
            Some(String::from_utf8_lossy(&buf).contains("hello"))
        };
        return matches!(
            tokio::time::timeout(Duration::from_secs(10), fut).await,
            Ok(Some(true))
        );
    }
    // Fresh client each time: no pooled connection or warm DNS from beams' probes.
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    else {
        return false;
    };
    match client
        .get(url)
        .header("bypass-tunnel-reminder", "1") // loca.lt interstitial
        .send()
        .await
    {
        Ok(r) => r.text().await.is_ok_and(|b| b.contains("hello")),
        Err(_) => false,
    }
}

async fn one_run(kind: &str, i: usize, protocol: Option<String>) -> Run {
    let mut run = Run::default();
    let port = hello_server().await;
    let tcp = kind == "bore";
    let backend: Box<dyn Tunnel + Send + Sync> = match kind {
        "cloudflare" => Box::new(CloudflareBackend {
            binary: ensure_binary(Tool::Cloudflared).await.expect("cloudflared"),
            target: format!("http://127.0.0.1:{port}"),
            protocol,
        }),
        "bore" => Box::new(BoreBackend {
            binary: ensure_binary(Tool::Bore).await.expect("bore"),
            local_port: port,
        }),
        "localtunnel" => Box::new(LocaltunnelBackend {
            subdomain: format!("beams-bench-{}-{i}", std::process::id()),
            local_host: "127.0.0.1".into(),
            local_port: port,
        }),
        _ => panic!("backend must be cloudflare | bore | localtunnel"),
    };

    let t0 = Instant::now();
    let mut handle = match backend.start().await {
        Ok(h) => h,
        Err(e) => {
            run.err = e.to_string();
            return run;
        }
    };
    run.url = Some(t0.elapsed().as_secs_f64());
    let url = handle.public_url().to_string();

    let ready = beams::ready::wait_until_ready(&url, tcp).await;
    run.ready = Some(ready);
    run.ready_s = t0.elapsed().as_secs_f64();

    // The user clicks right after the banner; allow a couple of quick retries
    // for an edge blip, but no long wait — that would hide real failures.
    for _ in 0..3 {
        if visit(&url, tcp).await {
            run.serve = Some(t0.elapsed().as_secs_f64());
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if run.serve.is_none() {
        run.err = format!("visitor could not load {url}");
    }
    handle.shutdown().await;
    run
}

fn pct(n: usize, of: usize) -> String {
    format!("{n}/{of} ({:.0}%)", 100.0 * n as f64 / of.max(1) as f64)
}

fn stats(mut v: Vec<f64>) -> String {
    if v.is_empty() {
        return "-".into();
    }
    v.sort_by(f64::total_cmp);
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    format!(
        "p50 {:.1}s  p90 {:.1}s  max {:.1}s",
        p(0.5),
        p(0.9),
        v[v.len() - 1]
    )
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let kind = args
        .first()
        .map_or("cloudflare", String::as_str)
        .to_string();
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let protocol = args.get(2).cloned();

    let mut runs = Vec::new();
    for i in 0..n {
        let r = one_run(&kind, i, protocol.clone()).await;
        let mark = |b: bool| if b { "ok" } else { "FAIL" };
        println!(
            "[{:>3}/{n}] url {:<4} ready {:<4} serve {:<4} {:>6.1}s  {}",
            i + 1,
            mark(r.url.is_some()),
            mark(r.ready == Some(true)),
            mark(r.serve.is_some()),
            r.serve.unwrap_or(r.ready_s),
            r.err
        );
        runs.push(r);
        // Space the dials out so we measure the relay, not its rate limiter.
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    let got_url = runs.iter().filter(|r| r.url.is_some()).count();
    let ready = runs.iter().filter(|r| r.ready == Some(true)).count();
    let served = runs.iter().filter(|r| r.serve.is_some()).count();
    // ready=true but the visitor failed: beams told the user a broken link works.
    let lied = runs
        .iter()
        .filter(|r| r.ready == Some(true) && r.serve.is_none())
        .count();

    println!(
        "\n== {kind} x{n}{} ==",
        protocol.map(|p| format!(" ({p})")).unwrap_or_default()
    );
    println!("got URL        {}", pct(got_url, n));
    println!("beams ready    {}", pct(ready, n));
    println!("visitor served {}   <- success rate", pct(served, n));
    println!("ready but broken {lied}");
    println!(
        "time to URL    {}",
        stats(runs.iter().filter_map(|r| r.url).collect())
    );
    println!(
        "time to serve  {}",
        stats(runs.iter().filter_map(|r| r.serve).collect())
    );
}
