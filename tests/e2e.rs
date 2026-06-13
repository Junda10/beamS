// Full path: starts a local HTTP server, downloads cloudflared, opens a tunnel,
// and asserts the public URL serves the local content. Requires network +
// outbound access, so it is #[ignore]d by default. Run with:
//   cargo test --test e2e -- --ignored --nocapture
use std::time::Duration;

use pharos::binary;
use pharos::tunnel::{CloudflareBackend, Tunnel};

#[tokio::test]
#[ignore]
async fn tunnel_serves_local_http() {
    // Start a tiny local server on an ephemeral port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
                    .await;
            }
        }
    });

    let bin = binary::ensure_binary().await.expect("download cloudflared");
    let backend = CloudflareBackend { binary: bin };
    let mut handle = backend
        .start(&format!("http://localhost:{port}"))
        .await
        .expect("start tunnel");

    let url = handle.public_url().to_string();
    assert!(url.ends_with(".trycloudflare.com"), "got: {url}");

    // Quick tunnels take a few seconds to become reachable; retry briefly.
    let client = reqwest::Client::new();
    let mut last = String::new();
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(body) = resp.text().await {
                last = body;
                if last.contains("hello") {
                    break;
                }
            }
        }
    }
    let _ = handle.shutdown().await;
    assert!(
        last.contains("hello"),
        "tunnel did not serve local content: {last:?}"
    );
}
