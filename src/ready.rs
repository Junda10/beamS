use std::time::Duration;

/// Seconds to wait for the hostname to appear in public DNS, then for the edge
/// to actually route to us. Quick tunnels normally clear both in a few seconds.
const DNS_ATTEMPTS: u32 = 20;
const REACH_ATTEMPTS: u32 = 15;

/// Poll until the public endpoint is actually reachable, so the URL we show the
/// user works immediately. Returns `true` once reachable, or `false` if it gives
/// up (the caller shows the URL anyway, with a warning).
///
/// Quick-tunnel hostnames are registered on demand, so for the first few seconds
/// they do not resolve. Asking the OS resolver during that window is worse than
/// not checking at all: the NXDOMAIN is cached for the zone's SOA minimum —
/// 1800s for trycloudflare.com — so the browser keeps failing with
/// ERR_NAME_NOT_RESOLVED for half an hour after the tunnel came up. So we
/// confirm the record exists over DNS-over-HTTPS first, which never touches the
/// system resolver, and only then let anything look the name up normally.
pub async fn wait_until_ready(public_url: &str, is_tcp: bool) -> bool {
    let Ok(client) = http_client() else {
        return false;
    };
    let host = hostname_of(public_url);

    if !retry(DNS_ATTEMPTS, || dns_has_record(&client, host)).await {
        return false;
    }

    // The name resolves upstream now, so the OS lookup below returns — and
    // caches — a positive answer.
    retry(REACH_ATTEMPTS, || async {
        if is_tcp {
            tcp_reachable(public_url).await
        } else {
            http_reachable(&client, public_url).await
        }
    })
    .await
}

/// Watch a live tunnel and resolve once it stops answering, so the caller can
/// restart it instead of waiting for the tunnel process to notice and exit.
///
/// A tunnel can stay "up" — the child process alive, no error logged — while the
/// edge has already stopped routing to it. That half-dead window is what reads as
/// an unstable connection, so probe the public URL rather than trusting liveness
/// of the process. One failed probe is normal (a blip, a slow edge); only a run
/// of `TOLERANCE` failures counts as dead.
pub async fn watch_until_dead(public_url: &str, is_tcp: bool) {
    const INTERVAL: Duration = Duration::from_secs(15);
    const TOLERANCE: u32 = 3;

    let Ok(client) = http_client() else {
        // No prober, no opinion: never claim the tunnel died.
        return std::future::pending::<()>().await;
    };

    let mut failures: u32 = 0;
    loop {
        tokio::time::sleep(INTERVAL).await;

        let alive = if is_tcp {
            tcp_reachable(public_url).await
        } else {
            tunnel_alive(&client, public_url).await
        };

        if alive {
            failures = 0;
        } else {
            failures += 1;
            if failures >= TOLERANCE {
                return;
            }
        }
    }
}

/// Run `check` up to `attempts` times, one second apart, until it succeeds.
async fn retry<F, Fut>(attempts: u32, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..attempts {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    false
}

/// The bare hostname of a public URL or a `host:port` address.
fn hostname_of(public_url: &str) -> &str {
    let after_scheme = public_url
        .split_once("://")
        .map_or(public_url, |(_, rest)| rest);
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    authority.split(':').next().unwrap_or(authority)
}

/// True once `host` has an A record in public DNS. Queries Cloudflare's
/// DNS-over-HTTPS endpoint by IP, so the lookup neither goes through nor
/// poisons the machine's own resolver cache.
async fn dns_has_record(client: &reqwest::Client, host: &str) -> bool {
    let Ok(resp) = client
        .get(format!("https://1.1.1.1/dns-query?name={host}&type=A"))
        .header("accept", "application/dns-json")
        .send()
        .await
    else {
        return false;
    };
    let Ok(body) = resp.text().await else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return false;
    };
    json["Answer"].as_array().is_some_and(|a| !a.is_empty())
}

fn http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
}

/// Reachable if an HTTP request to the URL completes at the transport level
/// (DNS resolved, TLS + connection succeeded, any response received).
async fn http_reachable(client: &reqwest::Client, url: &str) -> bool {
    client.get(url).send().await.is_ok()
}

/// Alive if the request completed *and* the answer came from our server rather
/// than from the edge reporting a dead tunnel.
///
/// `http_reachable` is deliberately transport-only, which is right for waiting on
/// a tunnel to come up but wrong for liveness: once cloudflared stops serving a
/// hostname the Cloudflare edge still answers, with 530 (error 1033). Treating
/// that as reachable would mean never noticing the tunnel died.
async fn tunnel_alive(client: &reqwest::Client, url: &str) -> bool {
    match client.get(url).send().await {
        Ok(resp) => resp.status().as_u16() != 530,
        Err(_) => false,
    }
}

/// Reachable if a TCP connection to `host:port` succeeds.
async fn tcp_reachable(addr: &str) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_of_strips_scheme_port_and_path() {
        assert_eq!(
            hostname_of("https://happy-cat-42.trycloudflare.com"),
            "happy-cat-42.trycloudflare.com"
        );
        assert_eq!(
            hostname_of("http://myapp.loca.lt/some/path"),
            "myapp.loca.lt"
        );
        // bore hands back a bare host:port, with no scheme.
        assert_eq!(hostname_of("bore.pub:41234"), "bore.pub");
    }

    /// A server that answers every connection with one canned status line.
    async fn serve_status(status: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                // Drain the request before answering: closing a socket that still
                // has unread incoming data sends an RST, which on Windows throws
                // away the response we just wrote.
                let _ = sock.read(&mut [0u8; 1024]).await;
                let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{addr}/")
    }

    /// The distinction `watch_until_dead` rests on: a completed request is not
    /// proof of life, because the Cloudflare edge answers 530 for a hostname
    /// whose tunnel is gone.
    #[tokio::test]
    async fn tunnel_alive_rejects_edge_error_and_accepts_a_real_response() {
        let client = http_client().unwrap();

        let live = serve_status("200 OK").await;
        assert!(tunnel_alive(&client, &live).await);

        let gone = serve_status("530 Origin DNS Error").await;
        assert!(!tunnel_alive(&client, &gone).await);
        // The same URL still looks fine to the transport-only check — which is
        // exactly why liveness needs its own probe.
        assert!(http_reachable(&client, &gone).await);
    }

    #[tokio::test]
    async fn retry_stops_at_first_success() {
        let mut calls = 0;
        let ok = retry(5, || {
            calls += 1;
            async move { true }
        })
        .await;
        assert!(ok);
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn retry_gives_up_after_attempts() {
        let mut calls = 0;
        // Attempts sleep 1s each; keep the count low so the test stays quick.
        let ok = retry(2, || {
            calls += 1;
            async move { false }
        })
        .await;
        assert!(!ok);
        assert_eq!(calls, 2);
    }
}
