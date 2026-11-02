use std::net::SocketAddr;
use std::time::Duration;

/// Seconds to wait for the hostname to appear in public DNS, then for the edge
/// to actually route to us. Quick tunnels normally clear both in a few seconds.
const DNS_ATTEMPTS: u32 = 30;
const REACH_ATTEMPTS: u32 = 15;
/// Positive answers in a row before the name counts as published. Right after a
/// quick-tunnel name appears, the zone's anycast nameservers still disagree for
/// a few seconds (yes, no, yes, no...); a resolver that hits a stale one then
/// caches the NXDOMAIN.
const DNS_SETTLED: u32 = 3;
/// Authoritative misses in a row before falling back to DoH for good.
const NS_MISSES: u32 = 3;

/// Poll until the public endpoint is actually reachable, so the URL we show the
/// user works immediately. Returns `true` once reachable, or `false` if it gives
/// up (the caller shows the URL anyway, with a warning).
///
/// Quick-tunnel hostnames are registered on demand, so for the first few seconds
/// they do not resolve. Any caching resolver asked during that window caches
/// the NXDOMAIN for the zone's SOA minimum (60s for trycloudflare.com when
/// checked, 1800s earlier) and keeps answering it after the name exists. For
/// the OS resolver that means ERR_NAME_NOT_RESOLVED in the browser. Polling a
/// public resolver instead only moves the problem: 1.1.1.1 caches the early
/// NXDOMAIN too, so the poll keeps seeing "no such name" after the name exists.
/// So we ask the zone's authoritative nameserver directly, which has no cache,
/// and only then let anything look the name up normally.
pub async fn wait_until_ready(public_url: &str, is_tcp: bool) -> bool {
    let Ok(client) = http_client() else {
        return false;
    };
    let host = hostname_of(public_url);

    let mut nameserver = zone_nameserver(&client, host).await;
    let (mut streak, mut misses) = (0, 0);
    for _ in 0..DNS_ATTEMPTS {
        let exists = match nameserver {
            Some(ns) => match authoritative_answer(ns, host).await {
                Some(found) => {
                    misses = 0;
                    found
                }
                // No authoritative answer this time (lost packet, referral at a
                // zone apex like bore.pub, UDP/53 blocked): use DoH for this
                // attempt, and only give up on the nameserver if it keeps failing.
                None => {
                    misses += 1;
                    if misses == NS_MISSES {
                        nameserver = None;
                    }
                    dns_has_record(&client, host).await
                }
            },
            None => dns_has_record(&client, host).await,
        };
        streak = if exists { streak + 1 } else { 0 };
        if streak == DNS_SETTLED {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if streak < DNS_SETTLED {
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
///
/// Only used when no authoritative nameserver answers: 1.1.1.1 caches an early
/// NXDOMAIN, so this can miss a fresh name for up to the SOA minimum.
async fn dns_has_record(client: &reqwest::Client, host: &str) -> bool {
    doh(client, host, "A")
        .await
        .is_some_and(|json| json["Answer"].as_array().is_some_and(|a| !a.is_empty()))
}

/// One DNS-over-HTTPS query to Cloudflare's resolver, by IP.
async fn doh(client: &reqwest::Client, name: &str, kind: &str) -> Option<serde_json::Value> {
    let resp = client
        .get(format!("https://1.1.1.1/dns-query?name={name}&type={kind}"))
        .header("accept", "application/dns-json")
        .send()
        .await
        .ok()?;
    serde_json::from_str(&resp.text().await.ok()?).ok()
}

/// An IPv4 address of a nameserver for the zone `host` lives in (its parent
/// domain). Every name looked up here already exists, so no resolver gets to
/// cache a negative answer for the tunnel's hostname.
async fn zone_nameserver(client: &reqwest::Client, host: &str) -> Option<SocketAddr> {
    let zone = host.split_once('.')?.1;
    let json = doh(client, zone, "NS").await?;
    let ns = json["Answer"]
        .as_array()?
        .iter()
        .filter(|a| a["type"] == 2)
        .find_map(|a| a["data"].as_str())?
        .trim_end_matches('.')
        .to_owned();
    tokio::net::lookup_host((ns, 53))
        .await
        .ok()?
        .find(SocketAddr::is_ipv4)
}

/// Ask an authoritative nameserver for `host`'s A record over plain UDP. It
/// answers from its own zone data, never from a cache. `None` when there is no
/// authoritative answer to be had (timeout, error, or a referral).
async fn authoritative_answer(server: SocketAddr, host: &str) -> Option<bool> {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .subsec_nanos() as u16;
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.ok()?;
    socket.send_to(&dns_query(id, host), server).await.ok()?;
    let mut buf = [0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(2), socket.recv(&mut buf))
        .await
        .ok()?
        .ok()?;
    parse_answer(&buf[..n], id)
}

/// A DNS query for `host`'s A record, recursion not requested.
fn dns_query(id: u16, host: &str) -> Vec<u8> {
    let mut q = id.to_be_bytes().to_vec();
    // flags 0, one question, no other records
    q.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in host.split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]); // root label, type A, class IN
    q
}

/// `Some(true)` if an authoritative reply to query `id` carries an answer,
/// `Some(false)` if it says the name (or its A record) does not exist, `None`
/// if it is not an authoritative reply to our query at all.
fn parse_answer(resp: &[u8], id: u16) -> Option<bool> {
    if resp.len() < 12 || resp[..2] != id.to_be_bytes() {
        return None;
    }
    let is_reply = resp[2] & 0x80 != 0;
    let authoritative = resp[2] & 0x04 != 0;
    if !is_reply || !authoritative {
        return None;
    }
    match resp[3] & 0x0f {
        0 => Some(u16::from_be_bytes([resp[6], resp[7]]) > 0),
        3 => Some(false),
        _ => None,
    }
}

fn http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        // Lets the request log tell our own probes apart from real visitors.
        .user_agent(concat!("beams/", env!("CARGO_PKG_VERSION")))
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

    #[test]
    fn parse_answer_trusts_only_an_authoritative_reply_to_our_query() {
        let mut resp = dns_query(7, "happy-cat-42.trycloudflare.com");
        resp[2] = 0x84; // reply + authoritative, NOERROR, no answer yet
        assert_eq!(parse_answer(&resp, 7), Some(false));
        resp[7] = 1; // one answer record
        assert_eq!(parse_answer(&resp, 7), Some(true));
        resp[3] = 3; // NXDOMAIN
        resp[7] = 0;
        assert_eq!(parse_answer(&resp, 7), Some(false));
        // A stray reply to some other query proves nothing.
        assert_eq!(parse_answer(&resp, 8), None);
        // A referral is not authoritative: fall back rather than wait it out.
        resp[2] = 0x80;
        assert_eq!(parse_answer(&resp, 7), None);
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
