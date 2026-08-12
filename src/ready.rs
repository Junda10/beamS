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
