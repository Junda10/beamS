//! `beams ./dist`: serve a folder of static files on localhost so a built
//! project can be shared without starting a server first.

use std::path::{Component, Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Serve `root` on a free localhost port and return that port.
pub async fn spawn(root: &Path) -> std::io::Result<u16> {
    let root = root.canonicalize()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let root = root.clone();
            tokio::spawn(async move {
                let _ = handle(sock, &root).await;
            });
        }
    });
    Ok(port)
}

async fn handle(mut sock: TcpStream, root: &Path) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = sock.read(&mut chunk).await?;
        if n == 0 || buf.len() > 16 * 1024 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut start = head.lines().next().unwrap_or("").split(' ');
    let (method, target) = (start.next().unwrap_or(""), start.next().unwrap_or(""));

    let file = match method {
        "GET" | "HEAD" => resolve(root, target),
        _ => None,
    };
    // ponytail: whole file in memory, no Range requests. Fine for a built site;
    // stream the file if people start sharing large videos.
    let (status, ctype, body) = match file {
        Some(path) => match tokio::fs::read(&path).await {
            Ok(body) => ("200 OK", content_type(&path), body),
            Err(_) => ("404 Not Found", "text/plain", b"not found".to_vec()),
        },
        None => ("404 Not Found", "text/plain", b"not found".to_vec()),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    if method != "HEAD" {
        sock.write_all(&body).await?;
    }
    sock.shutdown().await
}

/// Map a request target to a file under `root`, or `None` for a 404.
/// The public internet reaches this, so nothing outside `root` may resolve:
/// only plain path components are accepted, and the final path is checked again
/// after symlinks are followed.
fn resolve(root: &Path, target: &str) -> Option<PathBuf> {
    let path = target.split(['?', '#']).next().unwrap_or("");
    let rel = PathBuf::from(percent_decode(path)?.trim_start_matches('/'));
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let mut full = root.join(&rel);
    if full.is_dir() {
        full.push("index.html");
    }
    // Single-page apps route in the browser: a path with no file extension that
    // matches nothing gets the app shell, like every static host does.
    if !full.is_file() && rel.extension().is_none() {
        full = root.join("index.html");
    }
    let full = full.canonicalize().ok()?;
    (full.starts_with(root) && full.is_file()).then_some(full)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // A decoded NUL or backslash is never part of an honest URL path.
    let s = String::from_utf8(out).ok()?;
    (!s.contains(['\0', '\\'])).then_some(s)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_inside_the_folder_only() {
        let base = std::env::temp_dir().join(format!("beams-serve-{}", std::process::id()));
        let root = base.join("site");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("index.html"), "home").unwrap();
        std::fs::write(root.join("assets/a b.js"), "js").unwrap();
        std::fs::write(base.join("secret.txt"), "secret").unwrap();
        let root = root.canonicalize().unwrap();
        let name = |t: &str| resolve(&root, t).map(|p| p.strip_prefix(&root).unwrap().to_owned());

        assert_eq!(name("/"), Some("index.html".into()));
        assert_eq!(name("/assets/a%20b.js?v=1"), Some("assets/a b.js".into()));
        // Client-side route falls back to the app shell; a missing asset is a 404.
        assert_eq!(name("/about/team"), Some("index.html".into()));
        assert_eq!(name("/missing.png"), None);
        // Nothing above the folder, however it is spelled.
        for escape in [
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/assets/../../secret.txt",
            "/..%5csecret.txt",
            "//etc/passwd",
            "/C:/Windows/win.ini",
        ] {
            let got = name(escape);
            assert!(
                got.is_none() || got == Some("index.html".into()),
                "{escape}"
            );
        }
        std::fs::remove_dir_all(base).unwrap();
    }
}
