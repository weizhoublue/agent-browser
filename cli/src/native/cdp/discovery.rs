use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::types::BrowserVersionInfo;

/// Default timeout for CDP discovery HTTP requests.
const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

type HttpHeaders<'a> = Option<&'a [(String, String)]>;

/// Discover the CDP WebSocket URL for the given host and port (root `/json/version`).
pub async fn discover_cdp_url(
    host: &str,
    port: u16,
    query: Option<&str>,
) -> Result<String, String> {
    discover_cdp_url_with_auth(host, port, query, None, DEFAULT_DISCOVERY_TIMEOUT).await
}

/// Like [`discover_cdp_url`] but with a custom timeout and no auth headers.
pub async fn discover_cdp_url_with_timeout(
    host: &str,
    port: u16,
    query: Option<&str>,
    timeout: Duration,
) -> Result<String, String> {
    discover_cdp_url_with_auth(host, port, query, None, timeout).await
}

/// Like [`discover_cdp_url`] but with optional auth headers and custom timeout.
pub async fn discover_cdp_url_with_auth(
    host: &str,
    port: u16,
    query: Option<&str>,
    headers: HttpHeaders<'_>,
    timeout: Duration,
) -> Result<String, String> {
    let base = format!("http://{}:{}", bracket_ipv6(host), port);
    discover_cdp_at_http_base(&base, query, headers, timeout, host, port).await
}

/// Discover CDP WebSocket URL from a full HTTP base (e.g. CloakBrowser Manager
/// `http://host:9223/api/profiles/<uuid>/cdp`).
pub async fn discover_cdp_http_endpoint(
    http_base: &str,
    query: Option<&str>,
    headers: HttpHeaders<'_>,
    timeout: Duration,
) -> Result<String, String> {
    let parsed = url::Url::parse(http_base)
        .map_err(|e| format!("Invalid CDP HTTP base URL: {}", e))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("No host in CDP URL: {}", http_base))?;
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let scheme = parsed.scheme();
    let base = if http_base.contains("://") {
        http_base.trim_end_matches('/').to_string()
    } else {
        format!("{}://{}:{}", scheme, bracket_ipv6(host), port)
    };
    discover_cdp_at_http_base(&base, query, headers, timeout, host, port).await
}

async fn discover_cdp_at_http_base(
    http_base: &str,
    query: Option<&str>,
    headers: HttpHeaders<'_>,
    timeout: Duration,
    rewrite_host: &str,
    rewrite_port: u16,
) -> Result<String, String> {
    let base = http_base.trim_end_matches('/');

    let version_err = match fetch_cdp_info_at(base, headers, timeout).await {
        Ok(info) => {
            if let Some(ws_url) = info.web_socket_debugger_url {
                return Ok(append_query(
                    &rewrite_ws_host(&ws_url, rewrite_host, rewrite_port),
                    query,
                ));
            }
            format!("No webSocketDebuggerUrl in /json/version at {}", base)
        }
        Err(e) => e,
    };

    let list_err = match fetch_cdp_list_at(base, headers, timeout).await {
        Ok(ws_url) => {
            return Ok(append_query(
                &rewrite_ws_host(&ws_url, rewrite_host, rewrite_port),
                query,
            ))
        }
        Err(e) => e,
    };

    match discover_cdp_ws(rewrite_host, rewrite_port, timeout).await {
        Ok(ws_url) => Ok(append_query(&ws_url, query)),
        Err(ws_err) => Err(format!(
            "All CDP discovery methods failed for {}: /json/version: {}; /json/list: {}; WebSocket: {}",
            base, version_err, list_err, ws_err
        )),
    }
}

/// Bracket an IPv6 address for use in URLs. No-op for IPv4 or already-bracketed addresses.
fn bracket_ipv6(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]", host)
    } else {
        host.to_string()
    }
}

async fn fetch_cdp_info_at(
    http_base: &str,
    headers: HttpHeaders<'_>,
    timeout: Duration,
) -> Result<BrowserVersionInfo, String> {
    let url = format!("{}/json/version", http_base.trim_end_matches('/'));
    let body = tokio::time::timeout(timeout, reqwest_get_string(&url, headers))
        .await
        .map_err(|_| format!("Timeout connecting to CDP at {}", url))?
        .map_err(|e| format!("Failed to connect to CDP at {}: {}", url, e))?;
    serde_json::from_str(&body).map_err(|e| format!("Invalid /json/version response from {}: {}", url, e))
}

/// Rewrite the host and port in a WebSocket URL to match the target we
/// actually connected to. Chrome's `/json/version` always returns
/// `ws://127.0.0.1:<local-port>/...` which is unreachable when the
/// browser is on a remote machine or behind a port-forward.
fn rewrite_ws_host(ws_url: &str, host: &str, port: u16) -> String {
    if let Ok(mut parsed) = url::Url::parse(ws_url) {
        let _ = parsed.set_host(Some(&bracket_ipv6(host)));
        let _ = parsed.set_port(Some(port));
        parsed.to_string()
    } else {
        ws_url.to_string()
    }
}

/// Append a query string to a URL, preserving any existing query parameters.
fn append_query(url: &str, query: Option<&str>) -> String {
    match query {
        Some(q) if !q.is_empty() => {
            if let Ok(mut parsed) = url::Url::parse(url) {
                {
                    let mut pairs = parsed.query_pairs_mut();
                    pairs.extend_pairs(url::form_urlencoded::parse(q.as_bytes()));
                }
                parsed.to_string()
            } else if url.contains('?') {
                format!("{}&{}", url, q)
            } else {
                format!("{}?{}", url, q)
            }
        }
        _ => url.to_string(),
    }
}

async fn fetch_cdp_list_at(
    http_base: &str,
    headers: HttpHeaders<'_>,
    timeout: Duration,
) -> Result<String, String> {
    let url = format!("{}/json/list", http_base.trim_end_matches('/'));
    let body = tokio::time::timeout(timeout, reqwest_get_string(&url, headers))
        .await
        .map_err(|_| format!("Timeout connecting to /json/list at {}", url))?
        .map_err(|e| format!("Failed to connect to /json/list at {}: {}", url, e))?;

    let targets: Vec<serde_json::Value> =
        serde_json::from_str(&body).map_err(|e| format!("Invalid /json/list response from {}: {}", url, e))?;

    let browser_target = targets
        .iter()
        .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("browser"));

    let target = browser_target.or_else(|| targets.first());

    target
        .and_then(|t| t.get("webSocketDebuggerUrl"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("No webSocketDebuggerUrl found in /json/list at {}", url))
}

async fn discover_cdp_ws(host: &str, port: u16, timeout: Duration) -> Result<String, String> {
    let ws_url = format!("ws://{}:{}/devtools/browser", bracket_ipv6(host), port);

    tokio::time::timeout(timeout, async {
        let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| format!("WebSocket connect failed at {}: {}", ws_url, e))?;

        let cmd = r#"{"id":1,"method":"Browser.getVersion"}"#;
        ws_stream
            .send(Message::Text(cmd.into()))
            .await
            .map_err(|e| format!("Failed to send command: {}", e))?;

        #[derive(serde::Deserialize)]
        struct CdpReply {
            id: u64,
        }

        let mut result: Result<(), String> = Err("No valid CDP response received".to_string());
        while let Some(msg) = ws_stream.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if serde_json::from_str::<CdpReply>(&text).is_ok_and(|r| r.id == 1) {
                        result = Ok(());
                        break;
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            }
        }

        let _ = ws_stream.close(None).await;
        result
    })
    .await
    .map_err(|_| format!("Timeout connecting to WebSocket at {}", ws_url))?
    .map(|()| ws_url)
}

async fn reqwest_get_string(url: &str, headers: HttpHeaders<'_>) -> Result<String, String> {
    let client = reqwest::Client::new();
    let mut req = client.get(url);
    if let Some(hs) = headers {
        for (k, v) in hs {
            req = req.header(k.as_str(), v.as_str());
        }
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let snippet: String = body.chars().take(200).collect();
        return Err(format!("HTTP {} from {}: {}", status, url, snippet));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const HTTP_404: &str =
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    fn http_200(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{}",
            body.len(),
            body
        )
    }

    async fn accept_http(listener: &TcpListener, response: &str) {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = s.read(&mut buf).await;
        s.write_all(response.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn discovers_ws_url_from_json_version() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            accept_http(
                &listener,
                &http_200(r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:1234/"}"#),
            )
            .await;
        });

        let ws_url = discover_cdp_url("127.0.0.1", port, None).await.unwrap();
        assert_eq!(ws_url, format!("ws://127.0.0.1:{}/", port));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn discovers_ws_url_from_prefixed_json_version() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{}/api/profiles/test-id/cdp", port);
        let server = tokio::spawn(async move {
            accept_http(
                &listener,
                &http_200(
                    r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:1234/api/profiles/test-id/cdp"}"#,
                ),
            )
            .await;
        });

        let ws_url = discover_cdp_http_endpoint(&base, None, None, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(
            ws_url,
            format!("ws://127.0.0.1:{}/api/profiles/test-id/cdp", port)
        );
        server.await.unwrap();
    }

    #[test]
    fn rewrite_ws_host_replaces_host_and_port() {
        let original = "ws://127.0.0.1:9222/devtools/browser/abc";
        let rewritten = rewrite_ws_host(original, "10.211.55.12", 9223);
        assert_eq!(rewritten, "ws://10.211.55.12:9223/devtools/browser/abc");
    }

    #[test]
    fn append_query_adds_params_to_url_without_query() {
        let url = "ws://127.0.0.1:9222/devtools/browser/abc";
        let result = append_query(url, Some("mode=Hello"));
        assert_eq!(
            result,
            "ws://127.0.0.1:9222/devtools/browser/abc?mode=Hello"
        );
    }
}
