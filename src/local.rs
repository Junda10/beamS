//! Small conveniences that talk to the local machine: finding the dev server,
//! checking it is actually up, and handing the public URL to the clipboard or
//! the browser.

use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::task::JoinSet;

/// Ports we probe when no target is given — the defaults of the dev servers
/// people run most often (CRA/Next, Vite, Tomcat-ish, Django, Angular, Flask,
/// Hugo, Astro).
pub const COMMON_PORTS: [u16; 8] = [3000, 5173, 8080, 8000, 4200, 5000, 1313, 4321];

/// True if something accepts TCP connections on `host:port`.
pub async fn is_listening(host: &str, port: u16) -> bool {
    listening_addr(host, port).await.is_some()
}

/// The first address `host:port` resolves to that accepts TCP connections.
/// Every resolved address is probed at once with its own timeout: Windows puts
/// `::1` ahead of `127.0.0.1` for `localhost` and takes seconds to refuse a
/// closed port, which would otherwise eat the budget of an IPv4-only server.
pub async fn listening_addr(host: &str, port: u16) -> Option<SocketAddr> {
    let mut probes = JoinSet::new();
    for addr in tokio::net::lookup_host((host, port)).await.ok()? {
        probes.spawn(async move {
            let connect = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr));
            matches!(connect.await, Ok(Ok(_))).then_some(addr)
        });
    }
    while let Some(probe) = probes.join_next().await {
        if let Ok(Some(addr)) = probe {
            return Some(addr);
        }
    }
    None
}

/// Common dev ports with something listening on them, in `COMMON_PORTS` order.
/// Probed concurrently — a closed localhost port can take seconds to refuse on
/// Windows, so a sequential scan would stall.
pub async fn detect_ports() -> Vec<u16> {
    let probes: Vec<_> = COMMON_PORTS
        .into_iter()
        .map(|port| tokio::spawn(async move { (port, is_listening("localhost", port).await) }))
        .collect();
    let mut live = Vec::new();
    for probe in probes {
        if let Ok((port, true)) = probe.await {
            live.push(port);
        }
    }
    live
}

/// Copy text to the system clipboard using whatever tool the platform ships.
/// Best-effort — returns false when no clipboard tool is available.
pub async fn copy_to_clipboard(text: &str) -> bool {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };
    for (cmd, args) in candidates {
        if pipe_to(cmd, args, text).await {
            return true;
        }
    }
    false
}

async fn pipe_to(cmd: &str, args: &[&str], text: &str) -> bool {
    use tokio::io::AsyncWriteExt;
    let Ok(mut child) = tokio::process::Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let Some(mut stdin) = child.stdin.take() else {
        return false;
    };
    if stdin.write_all(text.as_bytes()).await.is_err() {
        return false;
    }
    drop(stdin); // let the tool see EOF, otherwise it never exits
    matches!(child.wait().await, Ok(status) if status.success())
}

/// Open a URL in the default browser. Best-effort; failure is silent.
pub fn open_in_browser(url: &str) {
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(target_os = "windows") {
        ("cmd", &["/C", "start", ""])
    } else {
        ("xdg-open", &[])
    };
    let _ = std::process::Command::new(cmd)
        .args(args)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detects_a_live_listener_and_a_dead_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = listener.local_addr().unwrap().port();
        assert!(is_listening("127.0.0.1", live).await);

        // Bind then release a second port so we have one nothing is on.
        let dead = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(!is_listening("127.0.0.1", dead).await);
    }

    // Windows resolves `localhost` to ::1 first and takes seconds to refuse a
    // closed port there; an IPv4-only server must still be found, by its IPv4 address.
    #[tokio::test]
    async fn localhost_finds_an_ipv4_only_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        assert_eq!(listening_addr("localhost", addr.port()).await, Some(addr));
    }
}
