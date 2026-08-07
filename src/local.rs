//! Small conveniences that talk to the local machine: finding the dev server,
//! checking it is actually up, and handing the public URL to the clipboard or
//! the browser.

use std::process::Stdio;
use std::time::Duration;

use tokio::net::TcpStream;

/// Ports we probe when no target is given — the defaults of the dev servers
/// people run most often (CRA/Next, Vite, Tomcat-ish, Django, Angular, Flask,
/// Hugo, Astro).
pub const COMMON_PORTS: [u16; 8] = [3000, 5173, 8080, 8000, 4200, 5000, 1313, 4321];

/// True if something accepts TCP connections on `host:port`.
pub async fn is_listening(host: &str, port: u16) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(2), TcpStream::connect((host, port))).await,
        Ok(Ok(_))
    )
}

/// First common dev port with something listening on it, if any.
pub async fn detect_port() -> Option<u16> {
    for port in COMMON_PORTS {
        if is_listening("localhost", port).await {
            return Some(port);
        }
    }
    None
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
}
