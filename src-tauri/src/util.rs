use base64::engine::general_purpose::{URL_SAFE_NO_PAD, STANDARD};
use base64::Engine;
use serde_json::Value;

pub type Res<T> = Result<T, String>;

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn b64(data: &[u8]) -> String {
    STANDARD.encode(data)
}

pub fn unb64(data: &str) -> Res<Vec<u8>> {
    STANDARD.decode(data.trim()).map_err(|e| e.to_string())
}

pub fn b64url(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

/// Decodes the (unverified) payload of a JWT.
pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn jwt_exp(token: &str) -> i64 {
    jwt_claims(token)
        .and_then(|c| c.get("exp").and_then(Value::as_i64))
        .unwrap_or(0)
}

/// Parses an RFC 3339 timestamp into unix seconds.
pub fn parse_time(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) if !s.is_empty() => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.timestamp()),
        Value::Number(n) => n.as_f64().map(|f| {
            // treat millisecond timestamps transparently
            if f > 1e12 {
                (f / 1000.0) as i64
            } else {
                f as i64
            }
        }),
        _ => None,
    }
}

pub fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Shortens an error body so it fits into the UI.
pub fn short(s: &str) -> String {
    let s = s.trim();
    let mut out: String = s.chars().take(160).collect();
    if s.chars().count() > 160 {
        out.push('…');
    }
    out
}

/// Extracts a human readable message from an API error body.
pub fn api_error(status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            let e = v.get("error").cloned().unwrap_or(v.clone());
            e.get("message")
                .or_else(|| e.get("error_description"))
                .or_else(|| v.get("detail"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| e.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| short(body));
    if msg.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {}", short(&msg))
    }
}

pub fn net_error(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "Таймаут соединения".into()
    } else if e.is_connect() {
        "Нет соединения с сервером".into()
    } else {
        short(&e.to_string())
    }
}

/// "5 ч", "7 дн" and similar short window names.
pub fn window_name(secs: i64) -> String {
    match secs {
        s if s <= 0 => "Лимит".into(),
        s if s % 86400 == 0 && s >= 86400 * 7 && s % (86400 * 7) == 0 => {
            let w = s / (86400 * 7);
            if w == 1 {
                "Недельный лимит".into()
            } else {
                format!("Лимит на {w} нед.")
            }
        }
        s if s % 86400 == 0 => format!("Лимит на {} дн.", s / 86400),
        s if s % 3600 == 0 => format!("{}-часовой лимит", s / 3600),
        s => format!("Лимит на {} мин", s / 60),
    }
}

/// Formats unix seconds in local time, e.g. "29.07 19:00".
pub fn fmt_local(t: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(t, 0)
        .single()
        .map(|d| d.format("%d.%m %H:%M").to_string())
        .unwrap_or_default()
}

pub fn fmt_usd(v: f64) -> String {
    format!("${v:.2}")
}
