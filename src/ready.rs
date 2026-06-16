use std::time::Duration;

/// Poll until the public endpoint is actually reachable, so the URL we show the
/// user works immediately. Quick tunnels need a few seconds for DNS + edge
/// propagation; showing the URL before then leads to ERR_NAME_NOT_RESOLVED or
/// transient gateway errors. Returns `true` once reachable, or `false` if it
/// gives up after ~20s (the caller shows the URL anyway).
pub async fn wait_until_ready(public_url: &str, is_tcp: bool) -> bool {
    for _ in 0..20 {
        let ready = if is_tcp {
            tcp_reachable(public_url).await
        } else {
            http_reachable(public_url).await
        };
        if ready {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    false
}

/// Reachable if an HTTP request to the URL completes at the transport level
/// (DNS resolved, TLS + connection succeeded, any response received).
async fn http_reachable(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    else {
        return false;
    };
    client.get(url).send().await.is_ok()
}

/// Reachable if a TCP connection to `host:port` succeeds.
async fn tcp_reachable(addr: &str) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}
