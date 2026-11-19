//! Shared public-URL guard for browser automation.
//!
//! `browser_act` routes every navigation and page read through
//! [`validate_public_url`]: only `http(s)` destinations that resolve to
//! public addresses are allowed, so an agent cannot be steered onto
//! localhost, intranet hosts, or cloud metadata endpoints.

use std::net::IpAddr;

use reqwest::Url;

fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || octets[0] == 100 && (octets[1] & 0b1100_0000) == 64
                || octets[0] == 192 && octets[1] == 0 && octets[2] == 2
                || octets[0] == 198 && octets[1] == 51 && octets[2] == 100
                || octets[0] == 203 && octets[1] == 0 && octets[2] == 113
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| private_ip(IpAddr::V4(mapped)))
        }
    }
}

pub(crate) async fn validate_public_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https URLs are allowed".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("localhost destinations are blocked".to_string());
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("cannot resolve {host}: {error}"))?;
    let mut any = false;
    for address in addresses {
        any = true;
        if private_ip(address.ip()) {
            return Err(format!("blocked private destination: {}", address.ip()));
        }
    }
    if !any {
        return Err(format!("cannot resolve {host}"));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blocks_loopback_and_private_destinations() {
        assert!(validate_public_url("http://127.0.0.1/").await.is_err());
        assert!(validate_public_url("http://localhost/").await.is_err());
        assert!(validate_public_url("http://10.0.0.1/").await.is_err());
        assert!(validate_public_url("file:///etc/passwd").await.is_err());
    }
}
