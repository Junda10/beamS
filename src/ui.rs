//! `beams --ui`: a small local web page to pick a port and share it.
//! Hand-rolled HTTP on 127.0.0.1 — four routes don't justify a web framework.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::binary::{self, Tool};
use crate::local;
use crate::ready;
use crate::tunnel::{CloudflareBackend, Tunnel, TunnelHandle};

const PAGE: &str = include_str!("ui.html");

struct State {
    /// `Host` values a request may carry: our own address, nothing else.
    hosts: [String; 2],
    protocol: Option<String>,
    /// Port -> live tunnel. Removing an entry drops `stop`, which ends it.
    tunnels: Mutex<HashMap<u16, Active>>,
}

struct Active {
    url: String,
    /// False while the tunnel is being redialed.
    up: bool,
    _stop: oneshot::Sender<()>,
}

impl State {
    /// Edit `port`'s entry if it is still the tunnel at `url`; false once that
    /// tunnel was stopped or replaced, so its task should give up.
    fn update(&self, port: u16, url: &str, f: impl FnOnce(&mut Active)) -> bool {
        match self.tunnels.lock().unwrap().get_mut(&port) {
            Some(a) if a.url == url => {
                f(a);
                true
            }
            _ => false,
        }
    }
}

type Reply = (&'static str, &'static str, String);

/// Ports the page lists: the CLI's usual ports plus the neighbours dev servers
/// fall back to when their default is taken.
fn scan_ports() -> Vec<u16> {
    let mut ports: Vec<u16> = local::COMMON_PORTS
        .into_iter()
        .chain(3001..=3010)
        .chain(4000..=4010)
        .chain(5001..=5010)
        .chain(5174..=5180)
        .chain([6006, 8888])
        .chain(8001..=8100)
        .chain(9000..=9010)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Serve the UI on a random localhost port and open it in the browser.
pub async fn run(protocol: Option<String>) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    println!("  ✓ beams UI: {url}  (Ctrl+C to stop)");
    local::open_in_browser(&url);
    serve(listener, protocol).await
}

pub async fn serve(listener: TcpListener, protocol: Option<String>) -> anyhow::Result<()> {
    let port = listener.local_addr()?.port();
    let state = Arc::new(State {
        hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")],
        protocol,
        tunnels: Mutex::default(),
    });
    loop {
        let (sock, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle(sock, state).await;
        });
    }
}

async fn handle(mut sock: TcpStream, state: Arc<State>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = sock.read(&mut chunk).await?;
        if n == 0 || buf.len() > 16 * 1024 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let (status, ctype, body) = route(&String::from_utf8_lossy(&buf), &state).await;
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body.as_bytes()).await
}

fn error(status: &'static str, msg: impl Into<String>) -> Reply {
    (
        status,
        "application/json",
        json!({ "error": msg.into() }).to_string(),
    )
}

async fn route(req: &str, state: &Arc<State>) -> Reply {
    let mut lines = req.lines();
    let mut start = lines.next().unwrap_or("").split(' ');
    let (method, target) = (start.next().unwrap_or(""), start.next().unwrap_or(""));
    let header = |name: &str| {
        lines.clone().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    };

    // DNS rebinding: a hostile site can point its own name at 127.0.0.1, but
    // the browser still sends that name as Host.
    if !header("host").is_some_and(|h| state.hosts.iter().any(|ok| ok == h)) {
        return error("403 Forbidden", "wrong host");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    // Any web page can fire a plain POST at localhost. A custom header forces a
    // CORS preflight, which this server never approves.
    if path.starts_with("/api/") && header("x-beams").is_none() {
        return error("403 Forbidden", "missing X-Beams header");
    }
    let port = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("port="))
        .and_then(|p| p.parse::<u16>().ok())
        .filter(|&p| p != 0);

    match (method, path, port) {
        ("GET", "/", _) => ("200 OK", "text/html; charset=utf-8", PAGE.to_string()),
        ("GET", "/api/ports", _) => {
            let ports = local::live_ports(&scan_ports()).await;
            let tunnels: Vec<_> = state
                .tunnels
                .lock()
                .unwrap()
                .iter()
                .map(|(port, a)| json!({ "port": port, "url": a.url, "up": a.up }))
                .collect();
            let body = json!({ "ports": ports, "tunnels": tunnels });
            ("200 OK", "application/json", body.to_string())
        }
        ("POST", "/api/share", Some(port)) => match share(port, state).await {
            Ok((url, ready)) => (
                "200 OK",
                "application/json",
                json!({ "url": url, "ready": ready }).to_string(),
            ),
            Err(e) => error("400 Bad Request", e),
        },
        ("POST", "/api/stop", Some(port)) => {
            if state.tunnels.lock().unwrap().remove(&port).is_some() {
                println!("  ✓ Stopped sharing port {port}");
            }
            ("200 OK", "application/json", "{}".to_string())
        }
        ("POST", _, None) => error("400 Bad Request", "pass a port between 1 and 65535"),
        _ => error("404 Not Found", "not found"),
    }
}

/// Open a Cloudflare quick tunnel to `localhost:port`; returns `(url, ready)`.
async fn share(port: u16, state: &Arc<State>) -> Result<(String, bool), String> {
    if let Some(a) = state.tunnels.lock().unwrap().get(&port) {
        return Ok((a.url.clone(), true));
    }
    let addr = local::listening_addr("localhost", port)
        .await
        .ok_or(format!(
            "nothing is listening on port {port} — start your server first"
        ))?;
    // Forward to the address that answered; `localhost` may resolve to an empty ::1.
    let host = match addr {
        SocketAddr::V4(a) => a.ip().to_string(),
        SocketAddr::V6(a) => format!("[{}]", a.ip()),
    };
    let binary = binary::ensure_binary(Tool::Cloudflared)
        .await
        .map_err(|e| e.to_string())?;
    let backend = CloudflareBackend {
        binary,
        target: format!("http://{host}:{port}"),
        protocol: state.protocol.clone(),
    };
    let (handle, url, ready) = dial(&backend).await?;

    let (stop, stopped) = oneshot::channel();
    let previous = state.tunnels.lock().unwrap().insert(
        port,
        Active {
            url: url.clone(),
            up: true,
            _stop: stop,
        },
    );
    drop(previous); // a racing second share of the same port replaces the first
    println!("  ✓ Sharing {host}:{port} at {url}");

    tokio::spawn(keep_alive(
        port,
        backend,
        handle,
        url.clone(),
        stopped,
        state.clone(),
    ));
    Ok((url, ready))
}

/// Start a tunnel and wait until its public URL answers. `ready` is false when
/// the wait gave up; the link usually starts working shortly after.
async fn dial(backend: &CloudflareBackend) -> Result<(TunnelHandle, String, bool), String> {
    let mut handle = backend.start().await.map_err(|e| e.to_string())?;
    let url = handle.public_url().to_string();
    let ready = tokio::select! {
        ready = ready::wait_until_ready(&url, false) => Some(ready),
        _ = handle.wait() => None,
    };
    match ready {
        Some(ready) => Ok((handle, url, ready)),
        None => Err("the tunnel process exited while coming online".into()),
    }
}

/// Own one shared port's tunnel: redial when it drops or stops answering, the
/// way the CLI does, until the user stops it. Returning drops the handle, and
/// `kill_on_drop` ends cloudflared.
async fn keep_alive(
    port: u16,
    backend: CloudflareBackend,
    mut handle: TunnelHandle,
    mut url: String,
    mut stopped: oneshot::Receiver<()>,
    state: Arc<State>,
) {
    loop {
        tokio::select! {
            _ = handle.wait() => println!("  … Tunnel for port {port} dropped — reconnecting..."),
            _ = ready::watch_until_dead(&url, false) => {
                println!("  … Tunnel for port {port} stopped responding — reconnecting...");
                handle.shutdown().await;
            }
            _ = &mut stopped => return,
        }
        if !state.update(port, &url, |a| a.up = false) {
            return;
        }

        let mut failures: u32 = 0;
        let (next, next_url) = loop {
            let attempt = tokio::select! {
                attempt = dial(&backend) => attempt,
                _ = &mut stopped => return,
            };
            match attempt {
                Ok((next, next_url, _)) => break (next, next_url),
                Err(e) => {
                    failures += 1;
                    if failures >= 5 {
                        println!("  ✗ Gave up on port {port}: {e}");
                        let mut tunnels = state.tunnels.lock().unwrap();
                        if tunnels.get(&port).is_some_and(|a| a.url == url) {
                            tunnels.remove(&port);
                        }
                        return;
                    }
                    // Same backoff as the CLI: 2s, 4s, 8s, 16s.
                    let wait = Duration::from_secs(1u64 << failures);
                    println!("  ! {e} — retrying in {}s ({failures}/5)", wait.as_secs());
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = &mut stopped => return,
                    }
                }
            }
        };

        let moved = state.update(port, &url, |a| {
            a.url = next_url.clone();
            a.up = true;
        });
        if !moved {
            return;
        }
        println!("  ✓ Port {port} is back at {next_url}");
        (handle, url) = (next, next_url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_only_touches_the_tunnel_it_owns() {
        let state = State {
            hosts: Default::default(),
            protocol: None,
            tunnels: Mutex::default(),
        };
        let (stop, _stopped) = oneshot::channel();
        let active = Active {
            url: "https://a".into(),
            up: true,
            _stop: stop,
        };
        state.tunnels.lock().unwrap().insert(3000, active);

        assert!(state.update(3000, "https://a", |a| a.url = "https://b".into()));
        // The old URL's task no longer owns the entry, and nobody owns port 4000.
        assert!(!state.update(3000, "https://a", |a| a.up = false));
        assert!(!state.update(4000, "https://b", |a| a.up = false));
        assert!(state.tunnels.lock().unwrap()[&3000].up);
    }

    #[tokio::test]
    async fn serves_page_and_guards_the_api() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(serve(listener, None));
        let c = reqwest::Client::new();
        let status = |r: reqwest::Response| r.status().as_u16();

        assert_eq!(status(c.get(&base).send().await.unwrap()), 200);
        // Cross-site POST without the custom header.
        let r = c
            .post(format!("{base}/api/stop?port=1"))
            .send()
            .await
            .unwrap();
        assert_eq!(status(r), 403);
        // DNS rebinding: Host names some other site.
        let r = c
            .get(format!("{base}/api/ports"))
            .header("x-beams", "1")
            .header("host", "evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(status(r), 403);

        let r = c.get(format!("{base}/api/ports")).header("x-beams", "1");
        let body: serde_json::Value =
            serde_json::from_str(&r.send().await.unwrap().text().await.unwrap()).unwrap();
        assert!(body["ports"].is_array() && body["tunnels"].is_array());

        // Sharing a dead port fails before any tunnel is dialed.
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let r = c
            .post(format!("{base}/api/share?port={dead}"))
            .header("x-beams", "1")
            .send()
            .await
            .unwrap();
        assert_eq!(status(r), 400);
    }
}
