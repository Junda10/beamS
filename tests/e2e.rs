// Network-gated end-to-end tests (require outbound access), #[ignore]d by default.
// Run with: cargo test --test e2e -- --ignored --nocapture
use std::time::Duration;

use beams::binary::{ensure_binary, Tool};
use beams::tunnel::{BoreBackend, CloudflareBackend, LocaltunnelBackend, Tunnel};

async fn spawn_hello_server() -> u16 {
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
    port
}

#[tokio::test]
#[ignore]
async fn cloudflare_serves_local_http() {
    let port = spawn_hello_server().await;
    let bin = ensure_binary(Tool::Cloudflared)
        .await
        .expect("download cloudflared");
    let backend = CloudflareBackend {
        binary: bin,
        target: format!("http://localhost:{port}"),
    };
    let mut handle = backend.start().await.expect("start tunnel");

    let url = handle.public_url().to_string();
    assert!(url.ends_with(".trycloudflare.com"), "got: {url}");

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
    handle.shutdown().await;
    assert!(last.contains("hello"), "tunnel did not serve: {last:?}");
}

#[tokio::test]
#[ignore]
async fn bore_tcp_tunnel_opens() {
    let port = spawn_hello_server().await;
    let bin = ensure_binary(Tool::Bore).await.expect("download bore");
    let backend = BoreBackend {
        binary: bin,
        local_port: port,
    };
    let mut handle = backend.start().await.expect("start bore");
    assert!(
        handle.public_url().starts_with("bore.pub:"),
        "got {}",
        handle.public_url()
    );
    handle.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn localtunnel_subdomain_opens() {
    let port = spawn_hello_server().await;
    let backend = LocaltunnelBackend {
        subdomain: format!("beams-test-{port}"),
        local_host: "localhost".to_string(),
        local_port: port,
    };
    let mut handle = backend.start().await.expect("open localtunnel");
    assert!(
        handle.public_url().contains("loca.lt"),
        "got {}",
        handle.public_url()
    );
    handle.shutdown().await;
}
