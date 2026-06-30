//! Parse CDP connection auth headers for HTTP discovery and WebSocket handshake.

use serde_json::{json, Value};

/// Build optional HTTP headers for CDP discovery and WebSocket connect.
///
/// `token` becomes `Authorization: Bearer <token>`.
/// `headers_json` must be a JSON object of string keys/values; merged after Bearer.
pub fn parse_cdp_auth_headers(
    token: Option<&str>,
    headers_json: Option<&str>,
) -> Result<Option<Vec<(String, String)>>, String> {
    let mut out: Vec<(String, String)> = Vec::new();

    if let Some(t) = token.map(str::trim).filter(|s| !s.is_empty()) {
        out.push(("Authorization".to_string(), format!("Bearer {}", t)));
    }

    if let Some(raw) = headers_json.map(str::trim).filter(|s| !s.is_empty()) {
        let v: Value = serde_json::from_str(raw)
            .map_err(|e| format!("Invalid JSON for CDP headers: {}", e))?;
        let obj = v
            .as_object()
            .ok_or_else(|| "CDP headers must be a JSON object".to_string())?;
        for (k, val) in obj {
            let s = val
                .as_str()
                .ok_or_else(|| format!("CDP header {:?} must be a string", k))?;
            if k.eq_ignore_ascii_case("authorization") {
                if let Some(pos) = out.iter().position(|(key, _)| {
                    key.eq_ignore_ascii_case("authorization")
                }) {
                    out[pos] = (k.clone(), s.to_string());
                    continue;
                }
            }
            out.push((k.clone(), s.to_string()));
        }
    }

    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

/// Parse CDP auth from a daemon `launch` command (`cdpToken`, `cdpHeaders`).
pub fn parse_cdp_auth_from_launch_cmd(cmd: &Value) -> Result<Option<Vec<(String, String)>>, String> {
    let token = cmd.get("cdpToken").and_then(|v| v.as_str());
    let headers_json = cmd.get("cdpHeaders").and_then(|v| match v {
        Value::Object(_) => Some(v.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    });
    parse_cdp_auth_headers(token, headers_json.as_deref())
}

/// Attach `cdpToken` / `cdpHeaders` to a `launch` command when connecting via CDP.
pub fn apply_launch_cdp_auth(cmd: &mut Value, token: Option<&str>, headers_json: Option<&str>) {
    if cmd.get("action").and_then(|v| v.as_str()) != Some("launch") {
        return;
    }
    if cmd.get("cdpUrl").is_none() && cmd.get("cdpPort").is_none() {
        return;
    }
    if let Some(t) = token.map(str::trim).filter(|s| !s.is_empty()) {
        cmd["cdpToken"] = json!(t);
    }
    if let Some(raw) = headers_json.map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            cmd["cdpHeaders"] = v;
        } else {
            cmd["cdpHeaders"] = json!(raw);
        }
    }
}

#[cfg(test)]
#[path = "cdp_auth_tests.rs"]
mod tests;
