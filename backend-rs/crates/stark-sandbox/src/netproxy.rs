//! Loopback egress proxy for model-spawned processes.
//!
//! Children receive `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` pointing at this
//! listener. CONNECT tunnels are checked against the harness network policy
//! before the upstream socket is opened; plain HTTP absolute-form requests are
//! rewritten to origin-form and forwarded the same way.
//!
//! This is a policy gate, not an OS firewall. A child that ignores proxy
//! variables can still open sockets unless the platform sandbox is enforcing
//! network isolation. The proxy is the enforcement point for tools that honor
//! the standard environment, and it blocks private/link-local destinations so
//! an allowed public host cannot be used for SSRF.
//!
//! Bind address is always loopback; the server never exposes it to the LAN.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use stark_policy::{Decision, NetworkRuleProtocol};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, warn};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_CONNECTIONS: usize = 128;

static PROXY_URL: OnceLock<String> = OnceLock::new();

/// Proxy URL for child processes, if the server started one.
pub fn url() -> Option<&'static str> {
    PROXY_URL.get().map(String::as_str)
}

/// Whether model-spawned processes may reach the network. Defaults to off.
pub fn network_enabled() -> bool {
    std::env::var("STARK_SANDBOX_NETWORK")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "on" | "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

fn allow_local() -> bool {
    std::env::var("STARK_NETWORK_ALLOW_LOCAL")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "on" | "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

/// Starts the loopback proxy and returns its URL. The task lives for the
/// process lifetime; failures are reported to the caller so the server can
/// keep running without a proxy rather than refusing to boot.
pub async fn start_background(bind: &str) -> io::Result<String> {
    let listener = TcpListener::bind(bind).await?;
    let address = listener.local_addr()?;
    let url = format!("http://{address}");
    let _ = PROXY_URL.set(url.clone());
    tokio::spawn(async move { serve(listener).await });
    Ok(url)
}

async fn serve(listener: TcpListener) {
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(value) => value,
            Err(error) => {
                warn!("network proxy accept failed: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = permits.clone().acquire_owned().await else {
            return;
        };
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = handle_client(stream).await {
                debug!("network proxy connection from {peer} ended: {error}");
            }
        });
    }
}

struct RequestLine {
    method: String,
    target: String,
    version: String,
}

fn parse_request_line(header: &[u8]) -> Option<RequestLine> {
    let text = std::str::from_utf8(header).ok()?;
    let first = text.lines().next()?.trim_end_matches('\r');
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let version = parts.next()?.to_string();
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return None;
    }
    Some(RequestLine {
        method,
        target,
        version,
    })
}

fn header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn response(status: u16, reason: &str, body: &str) -> Vec<u8> {
    let body = body.as_bytes();
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

async fn deny(stream: &mut TcpStream, status: u16, reason: &str, message: &str) -> io::Result<()> {
    stream.write_all(&response(status, reason, message)).await?;
    stream.shutdown().await
}

fn normalized_host(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn parse_authority(target: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = target.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail
            .strip_prefix(':')
            .filter(|port| !port.is_empty())
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(default_port);
        return Some((normalized_host(host), port));
    }
    let (host, port) = match target.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => (host, port),
            Err(_) => (target, default_port),
        },
        None => (target, default_port),
    };
    if host.is_empty() {
        return None;
    }
    Some((normalized_host(host), port))
}

fn ipv4_non_public(ip: Ipv4Addr) -> bool {
    let value = u32::from(ip);
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || value & 0xff00_0000 == 0 // 0.0.0.0/8
        || value & 0xffc0_0000 == 0x6440_0000 // 100.64.0.0/10 CGNAT
        || value & 0xffff_ff00 == 0xc000_0000 // 192.0.0.0/24
        || value & 0xffff_ff00 == 0xc000_0200 // 192.0.2.0/24
        || value & 0xfffe_0000 == 0xc612_0000 // 198.18.0.0/15
        || value & 0xffff_ff00 == 0xc633_6400 // 198.51.100.0/24
        || value & 0xffff_ff00 == 0xcb00_7100 // 203.0.113.0/24
        || value & 0xf000_0000 == 0xf000_0000 // 240.0.0.0/4
}

fn is_non_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_non_public(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4() {
                return ipv4_non_public(v4) || ip.is_loopback();
            }
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
        }
    }
}

/// Policy check for host-side tools that do not go through the proxy (browser
/// URL validation, future web_fetch). `https` selects the rule protocol.
pub fn denial_for_host(host: &str, https: bool) -> Option<String> {
    policy_denial(
        &normalized_host(host),
        if https {
            NetworkRuleProtocol::Https
        } else {
            NetworkRuleProtocol::Http
        },
    )
}

fn policy_denial(host: &str, protocol: NetworkRuleProtocol) -> Option<String> {
    if !network_enabled() {
        return Some(format!(
            "network access to {host} is disabled for this sandbox (set STARK_SANDBOX_NETWORK=on to enable)"
        ));
    }
    let decision = stark_policy::evaluate_network(host, protocol, Decision::Allow);
    match decision.decision {
        Decision::Allow => None,
        Decision::Prompt => Some(format!(
            "network access to {host} requires approval; run the request in the foreground or add a network_rule to .deep_agents/execpolicy.star"
        )),
        Decision::Forbidden => Some(
            decision
                .reason
                .unwrap_or_else(|| format!("network access to {host} is denied by policy")),
        ),
    }
}

async fn resolve_target(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let addresses: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| format!("DNS lookup for {host} timed out"))?
            .map_err(|error| format!("DNS lookup for {host} failed: {error}"))?
            .collect(),
    };
    if addresses.is_empty() {
        return Err(format!("DNS lookup for {host} returned no addresses"));
    }
    let allow_local = allow_local();
    let allowed: Vec<SocketAddr> = addresses
        .into_iter()
        .filter(|address| allow_local || !is_non_public(address.ip()))
        .collect();
    if allowed.is_empty() {
        return Err(format!(
            "network access to {host} is blocked: local/private addresses are not allowed"
        ));
    }
    Ok(allowed)
}

async fn connect_target(addresses: &[SocketAddr]) -> Result<TcpStream, String> {
    let mut last = None;
    for address in addresses {
        match timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Ok(Err(error)) => last = Some(error.to_string()),
            Err(_) => last = Some(format!("connect to {address} timed out")),
        }
    }
    Err(last.unwrap_or_else(|| "no address could be reached".to_string()))
}

async fn handle_client(mut client: TcpStream) -> io::Result<()> {
    let _ = client.set_nodelay(true);
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let end = loop {
        let read = timeout(IO_TIMEOUT, client.read(&mut chunk)).await;
        let read = match read {
            Ok(Ok(0)) => return Ok(()),
            Ok(Ok(read)) => read,
            Ok(Err(error)) => return Err(error),
            Err(_) => return Ok(()),
        };
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_HEADER_BYTES {
            return deny(
                &mut client,
                431,
                "Request Header Fields Too Large",
                "request headers are too large",
            )
            .await;
        }
        if let Some(end) = header_end(&buffer) {
            break end;
        }
    };

    let Some(request) = parse_request_line(&buffer[..end]) else {
        return deny(&mut client, 400, "Bad Request", "invalid proxy request").await;
    };

    if request.method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = parse_authority(&request.target, 443) else {
            return deny(&mut client, 400, "Bad Request", "invalid CONNECT target").await;
        };
        if let Some(message) = policy_denial(&host, NetworkRuleProtocol::Https) {
            return deny(&mut client, 403, "Forbidden", &message).await;
        }
        let addresses = match resolve_target(&host, port).await {
            Ok(addresses) => addresses,
            Err(message) => return deny(&mut client, 403, "Forbidden", &message).await,
        };
        let mut upstream = match connect_target(&addresses).await {
            Ok(upstream) => upstream,
            Err(message) => return deny(&mut client, 502, "Bad Gateway", &message).await,
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        client.flush().await?;
        if end < buffer.len() {
            upstream.write_all(&buffer[end..]).await?;
        }
        debug!("network proxy CONNECT {host}:{port}");
        let _ = timeout(
            IO_TIMEOUT,
            tokio::io::copy_bidirectional(&mut client, &mut upstream),
        )
        .await;
        return Ok(());
    }

    // Plain HTTP proxying: absolute-form request target -> origin-form.
    let Ok(uri) = request.target.parse::<http::Uri>() else {
        return deny(&mut client, 400, "Bad Request", "invalid proxy target").await;
    };
    if uri.scheme_str() != Some("http") {
        return deny(
            &mut client,
            400,
            "Bad Request",
            "only http:// absolute targets and CONNECT are supported",
        )
        .await;
    }
    let Some(host) = uri.host() else {
        return deny(&mut client, 400, "Bad Request", "proxy target has no host").await;
    };
    let host = normalized_host(host);
    let port = uri.port_u16().unwrap_or(80);
    if let Some(message) = policy_denial(&host, NetworkRuleProtocol::Http) {
        return deny(&mut client, 403, "Forbidden", &message).await;
    }
    let addresses = match resolve_target(&host, port).await {
        Ok(addresses) => addresses,
        Err(message) => return deny(&mut client, 403, "Forbidden", &message).await,
    };
    let mut upstream = match connect_target(&addresses).await {
        Ok(upstream) => upstream,
        Err(message) => return deny(&mut client, 502, "Bad Gateway", &message).await,
    };

    let path = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let mut forwarded = Vec::new();
    forwarded.extend_from_slice(
        format!("{} {} {}\r\n", request.method, path, request.version).as_bytes(),
    );
    let header_text = String::from_utf8_lossy(&buffer[..end]);
    for line in header_text.lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("proxy-connection:") || lower.starts_with("proxy-authorization:") {
            continue;
        }
        forwarded.extend_from_slice(line.as_bytes());
        forwarded.extend_from_slice(b"\r\n");
    }
    forwarded.extend_from_slice(b"\r\n");
    forwarded.extend_from_slice(&buffer[end..]);
    upstream.write_all(&forwarded).await?;
    upstream.flush().await?;
    debug!("network proxy HTTP {host}:{port}{path}");
    let _ = timeout(
        IO_TIMEOUT,
        tokio::io::copy_bidirectional(&mut client, &mut upstream),
    )
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_authority_forms() {
        assert_eq!(
            parse_authority("example.com:443", 443),
            Some(("example.com".to_string(), 443))
        );
        assert_eq!(
            parse_authority("example.com", 443),
            Some(("example.com".to_string(), 443))
        );
        assert_eq!(
            parse_authority("[2606:4700:4700::1111]:443", 443),
            Some(("2606:4700:4700::1111".to_string(), 443))
        );
        assert_eq!(parse_authority(":443", 443), None);
    }

    #[test]
    fn private_and_link_local_addresses_are_non_public() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
        ] {
            assert!(
                is_non_public(address.parse().unwrap()),
                "{address} must be non-public"
            );
        }
        assert!(!is_non_public("1.1.1.1".parse().unwrap()));
        assert!(!is_non_public("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn header_end_finds_double_newline() {
        assert_eq!(
            header_end(b"GET / HTTP/1.1\r\nHost: x\r\n\r\nbody"),
            Some(27)
        );
        assert_eq!(header_end(b"GET / HTTP/1.1\r\nHost: x\r\n"), None);
    }

    #[tokio::test]
    async fn private_connect_targets_are_denied() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(serve(listener));
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"CONNECT 127.0.0.1:80 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut response = vec![0u8; 512];
        let read = client.read(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response[..read]);
        assert!(text.starts_with("HTTP/1.1 403"), "{text}");
    }
}
