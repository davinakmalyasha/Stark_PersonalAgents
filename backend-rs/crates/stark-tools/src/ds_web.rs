//! DeepSeek Harness-compatible web tools.
//!
//! Search uses DeepSeek's `/v1/tools/search` API when a key is configured.
//! Fetch is a local HTTP GET with a strict public-destination guard, matching
//! the DeepSeek Harness default `fetchProvider: http` composition.

use std::net::IpAddr;
use std::time::Duration;

use reqwest::Url;
use serde_json::{Value, json};

use crate::{ToolContext, ToolOutcome};

const MAX_FETCH_BYTES: usize = 5 * 1024 * 1024;
const MAX_FETCH_CHARS: usize = 200_000;

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

async fn validate_public_url(raw: &str) -> Result<Url, String> {
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

fn strip_html(html: &str) -> String {
    let mut text = html.to_string();
    for pattern in [
        r"(?is)<script[^>]*>.*?</script>",
        r"(?is)<style[^>]*>.*?</style>",
        r"(?is)<noscript[^>]*>.*?</noscript>",
        r"(?is)<!--.*?-->",
    ] {
        if let Ok(regex) = regex::Regex::new(pattern) {
            text = regex.replace_all(&text, " ").to_string();
        }
    }
    if let Ok(regex) = regex::Regex::new(r"(?is)<[^>]+>") {
        text = regex.replace_all(&text, " ").to_string();
    }
    for (entity, replacement) in [
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
    ] {
        text = text.replace(entity, replacement);
    }
    if let Ok(regex) = regex::Regex::new(r"[ \t\r\f\v]+") {
        text = regex.replace_all(&text, " ").to_string();
    }
    text = text.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    if let Ok(regex) = regex::Regex::new(r"\n{3,}") {
        text = regex.replace_all(&text, "\n\n").to_string();
    }
    text.chars().take(MAX_FETCH_CHARS).collect()
}

pub async fn web_fetch(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(raw) = args.get("url").and_then(Value::as_str) else {
        return ToolOutcome::err("Error: web_fetch requires url");
    };
    if let Some(blocked) = crate::permissions::gate(ctx, "webfetch", &[raw.to_string()], raw).await
    {
        return blocked;
    }
    let url = match validate_public_url(raw).await {
        Ok(url) => url,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        let next = attempt.url().clone();
        if next.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost")
        }) {
            attempt.error("redirect to localhost is blocked")
        } else if next
            .host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .is_some_and(private_ip)
        {
            attempt.error("redirect to a private destination is blocked")
        } else if attempt.previous().len() >= 5 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    });
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(policy)
        .user_agent("Stark/0.1 (+https://github.com/davinakmalyasha/Stark_PersonalAgents)")
        .build()
    {
        Ok(client) => client,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let mut response = match client.get(url.clone()).send().await {
        Ok(response) => response,
        Err(error) => return ToolOutcome::err(format!("Error: unable to fetch {url}: {error}")),
    };
    if !response.status().is_success() {
        return ToolOutcome::err(format!("Error: {url} returned HTTP {}", response.status()));
    }
    if let Some(length) = response.content_length()
        && length as usize > MAX_FETCH_BYTES
    {
        return ToolOutcome::err(format!(
            "Error: response is too large ({length} bytes, max {MAX_FETCH_BYTES})"
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_FETCH_BYTES {
                    return ToolOutcome::err(format!(
                        "Error: response is too large (max {MAX_FETCH_BYTES} bytes)"
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => {
                return ToolOutcome::err(format!("Error: unable to read {url}: {error}"));
            }
        }
    }
    let body = String::from_utf8_lossy(&bytes).to_string();
    let text = if content_type.contains("html")
        || body.trim_start().starts_with("<!doctype")
        || body.trim_start().starts_with("<html")
    {
        strip_html(&body)
    } else {
        body.chars().take(MAX_FETCH_CHARS).collect()
    };
    ToolOutcome::ok(format!(
        "[URL] {url}\n[CONTENT-TYPE] {content_type}\n\n{text}"
    ))
}

fn search_base() -> String {
    std::env::var("STARK_SEARCH_BASE_URL")
        .ok()
        .or_else(|| std::env::var("DEEPSEEK_SEARCH_BASE_URL").ok())
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "https://api.deepseek.com".to_string())
}

fn search_key() -> Option<String> {
    std::env::var("STARK_SEARCH_API_KEY")
        .ok()
        .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub async fn web_search(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let Some(queries) = args.get("queries").and_then(Value::as_array) else {
        return ToolOutcome::err("Error: web_search requires queries");
    };
    let queries: Vec<String> = queries
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .take(4)
        .map(str::to_string)
        .collect();
    if queries.is_empty() {
        return ToolOutcome::err("Error: web_search requires 1-4 non-empty queries");
    }
    if let Some(blocked) =
        crate::permissions::gate(ctx, "websearch", &[queries.join(" | ")], "web search").await
    {
        return blocked;
    }
    let Some(key) = search_key() else {
        return ToolOutcome::err(
            "Error: web_search is not configured (set DEEPSEEK_API_KEY or STARK_SEARCH_API_KEY)",
        );
    };
    let endpoint = format!("{}/v1/tools/search", search_base());
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(client) => client,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let mut merged: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for query in queries {
        let response = match client
            .post(&endpoint)
            .bearer_auth(&key)
            .json(&json!({"query": query, "count": 10}))
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => return ToolOutcome::err(format!("Error: web_search failed: {error}")),
        };
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return ToolOutcome::err(format!(
                "Error: web_search returned HTTP {status}: {}",
                body.chars().take(500).collect::<String>()
            ));
        }
        let value: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
        if let Some(results) = value.get("results").and_then(Value::as_array) {
            for result in results {
                let url = result.get("url").and_then(Value::as_str).unwrap_or("");
                if url.is_empty() || !seen.insert(url.to_string()) {
                    continue;
                }
                merged.push(json!({
                    "title": result.get("title").cloned().unwrap_or(Value::Null),
                    "url": url,
                    "snippet": result.get("snippet").cloned().unwrap_or(Value::Null),
                    "summary": result.get("summary").cloned().unwrap_or(Value::Null),
                    "published_at": result.get("published_at").cloned().unwrap_or(Value::Null),
                    "authority": result.get("authority").cloned().unwrap_or(Value::Null),
                    "rerank_score": result.get("rerank_score").cloned().unwrap_or(Value::Null),
                    "query": query,
                }));
            }
        }
    }
    ToolOutcome::ok(serde_json::to_string_pretty(&json!({"results": merged})).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_and_tags() {
        let text = strip_html("<html><script>bad()</script><h1>Hello</h1><p>world</p></html>");
        assert!(text.contains("Hello"), "{text}");
        assert!(text.contains("world"), "{text}");
        assert!(!text.contains("bad()"), "{text}");
    }

    #[tokio::test]
    async fn blocks_loopback_and_private_destinations() {
        assert!(validate_public_url("http://127.0.0.1/").await.is_err());
        assert!(validate_public_url("http://localhost/").await.is_err());
        assert!(validate_public_url("http://10.0.0.1/").await.is_err());
        assert!(validate_public_url("file:///etc/passwd").await.is_err());
    }
}
