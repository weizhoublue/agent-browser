//! Unit tests for `cdp_auth` — kept in a dedicated file so the implementation
//! module stays focused on logic only.

use super::*;

// ---------------------------------------------------------------------------
// parse_cdp_auth_headers
// ---------------------------------------------------------------------------

#[test]
fn token_only() {
    let h = parse_cdp_auth_headers(Some("secret"), None).unwrap().unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].0, "Authorization");
    assert_eq!(h[0].1, "Bearer secret");
}

#[test]
fn headers_json_only() {
    let h = parse_cdp_auth_headers(None, Some(r#"{"X-Custom":"1"}"#))
        .unwrap()
        .unwrap();
    assert_eq!(h[0].0, "X-Custom");
    assert_eq!(h[0].1, "1");
}

#[test]
fn both_none_returns_none() {
    assert!(parse_cdp_auth_headers(None, None).unwrap().is_none());
}

#[test]
fn empty_token_ignored() {
    assert!(parse_cdp_auth_headers(Some("  "), None).unwrap().is_none());
}

#[test]
fn token_and_extra_headers_merged() {
    let h = parse_cdp_auth_headers(Some("tok"), Some(r#"{"X-Org":"acme"}"#))
        .unwrap()
        .unwrap();
    assert_eq!(h.len(), 2);
    assert!(h.iter().any(|(k, v)| k == "Authorization" && v == "Bearer tok"));
    assert!(h.iter().any(|(k, v)| k == "X-Org" && v == "acme"));
}

#[test]
fn headers_json_authorization_overrides_token() {
    let h = parse_cdp_auth_headers(Some("tok"), Some(r#"{"Authorization":"Basic xyz"}"#))
        .unwrap()
        .unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].1, "Basic xyz");
}

#[test]
fn invalid_headers_json_returns_error() {
    assert!(parse_cdp_auth_headers(None, Some("{not json}")).is_err());
}

#[test]
fn headers_json_non_object_returns_error() {
    assert!(parse_cdp_auth_headers(None, Some("[1,2,3]")).is_err());
}

#[test]
fn headers_json_non_string_value_returns_error() {
    assert!(parse_cdp_auth_headers(None, Some(r#"{"X-Num":42}"#)).is_err());
}

// ---------------------------------------------------------------------------
// parse_cdp_auth_from_launch_cmd
// ---------------------------------------------------------------------------

#[test]
fn from_cmd_token_only() {
    let cmd = serde_json::json!({ "cdpToken": "mytoken" });
    let h = parse_cdp_auth_from_launch_cmd(&cmd).unwrap().unwrap();
    assert_eq!(h[0].1, "Bearer mytoken");
}

#[test]
fn from_cmd_headers_as_object() {
    let cmd = serde_json::json!({ "cdpHeaders": { "X-Tenant": "t1" } });
    let h = parse_cdp_auth_from_launch_cmd(&cmd).unwrap().unwrap();
    assert_eq!(h[0].0, "X-Tenant");
    assert_eq!(h[0].1, "t1");
}

#[test]
fn from_cmd_headers_as_string() {
    let cmd = serde_json::json!({ "cdpHeaders": r#"{"X-Env":"prod"}"# });
    let h = parse_cdp_auth_from_launch_cmd(&cmd).unwrap().unwrap();
    assert_eq!(h[0].0, "X-Env");
    assert_eq!(h[0].1, "prod");
}

#[test]
fn from_cmd_no_auth_fields_returns_none() {
    let cmd = serde_json::json!({ "action": "launch", "cdpUrl": "http://localhost/cdp" });
    assert!(parse_cdp_auth_from_launch_cmd(&cmd).unwrap().is_none());
}

// ---------------------------------------------------------------------------
// apply_launch_cdp_auth
// ---------------------------------------------------------------------------

#[test]
fn apply_attaches_token_to_cdp_url_launch() {
    let mut cmd = serde_json::json!({ "action": "launch", "cdpUrl": "http://localhost/cdp" });
    apply_launch_cdp_auth(&mut cmd, Some("tok"), None);
    assert_eq!(cmd["cdpToken"], serde_json::json!("tok"));
}

#[test]
fn apply_attaches_token_to_cdp_port_launch() {
    let mut cmd = serde_json::json!({ "action": "launch", "cdpPort": 9222 });
    apply_launch_cdp_auth(&mut cmd, Some("tok"), None);
    assert_eq!(cmd["cdpToken"], serde_json::json!("tok"));
}

#[test]
fn apply_attaches_headers_json() {
    let mut cmd = serde_json::json!({ "action": "launch", "cdpUrl": "http://localhost/cdp" });
    apply_launch_cdp_auth(&mut cmd, None, Some(r#"{"X-App":"1"}"#));
    assert_eq!(cmd["cdpHeaders"]["X-App"], serde_json::json!("1"));
}

#[test]
fn apply_skipped_if_no_cdp_url_or_port() {
    let mut cmd = serde_json::json!({ "action": "launch" });
    apply_launch_cdp_auth(&mut cmd, Some("tok"), None);
    assert!(cmd.get("cdpToken").is_none());
}

#[test]
fn apply_skipped_if_not_launch_action() {
    let mut cmd = serde_json::json!({ "action": "navigate", "cdpUrl": "http://localhost/cdp" });
    apply_launch_cdp_auth(&mut cmd, Some("tok"), None);
    assert!(cmd.get("cdpToken").is_none());
}

#[test]
fn apply_empty_token_not_attached() {
    let mut cmd = serde_json::json!({ "action": "launch", "cdpUrl": "http://localhost/cdp" });
    apply_launch_cdp_auth(&mut cmd, Some("  "), None);
    assert!(cmd.get("cdpToken").is_none());
}

#[test]
fn apply_invalid_headers_json_stored_as_raw_string() {
    let mut cmd = serde_json::json!({ "action": "launch", "cdpUrl": "http://localhost/cdp" });
    apply_launch_cdp_auth(&mut cmd, None, Some("{invalid}"));
    assert_eq!(cmd["cdpHeaders"], serde_json::json!("{invalid}"));
}
