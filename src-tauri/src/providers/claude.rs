//! Claude (claude.ai subscription) limits.
//! Login uses the OAuth PKCE flow of Claude Code; limits come from `/api/oauth/usage`
//! (the data behind `/usage` in Claude Code).

use std::path::PathBuf;

use reqwest::Client;
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use super::{pretty_plan, send};
use crate::model::{Account, Kind, Limit, Snapshot, Source, Tokens};
use crate::oauth::{build_url, pkce, random_state, Loopback};
use crate::util::{now, num, parse_time, Res};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URLS: [&str; 2] = [
    "https://platform.claude.com/v1/oauth/token",
    "https://console.anthropic.com/v1/oauth/token",
];
const SCOPES: &str = "org:create_api_key user:profile user:inference";
const API: &str = "https://api.anthropic.com";
const BETA: &str = "oauth-2025-04-20";

pub async fn login(
    http: &Client,
    open: impl Fn(&str),
    cancel: oneshot::Receiver<()>,
) -> Res<Account> {
    let lb = Loopback::bind(0).await?;
    let redirect = format!("http://localhost:{}/callback", lb.port);
    let pk = pkce();
    let state = random_state();
    let url = build_url(
        AUTHORIZE_URL,
        &[
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("scope", SCOPES),
            ("code_challenge", &pk.challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
        ],
    );
    open(&url);
    let code = lb.wait_code("/callback", &state, cancel).await?;

    let body = json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect,
        "client_id": CLIENT_ID,
        "code_verifier": pk.verifier,
        "state": state,
    });
    let v = token_request(http, &body).await?;
    let tokens = tokens_from(&v, None)?;

    let mut acc = Account {
        id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::Claude,
        label: String::new(),
        email: v
            .pointer("/account/email_address")
            .and_then(Value::as_str)
            .map(str::to_owned),
        source: Source::OAuth,
        meta: Map::new(),
        created_at: now(),
        secret: Default::default(),
    };
    let _ = load_profile(http, &mut acc, &tokens).await;
    acc.label = acc.email.clone().unwrap_or_else(|| "Claude".into());
    acc.secret.tokens = Some(tokens);
    Ok(acc)
}

async fn token_request(http: &Client, body: &Value) -> Res<Value> {
    let mut last = String::new();
    for url in TOKEN_URLS {
        match send(http.post(url).json(body)).await {
            Ok(r) if r.ok() => return r.json(),
            Ok(r) if r.status == 400 || r.status == 401 => {
                return Err(format!(
                    "Сервер авторизации отклонил запрос: {}",
                    crate::util::api_error(r.status, &r.body)
                ))
            }
            Ok(r) => last = crate::util::api_error(r.status, &r.body),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn tokens_from(v: &Value, old: Option<&Tokens>) -> Res<Tokens> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Ok(Tokens {
        access_token: s("access_token").ok_or("В ответе нет access_token")?,
        refresh_token: s("refresh_token")
            .or_else(|| old.map(|o| o.refresh_token.clone()))
            .unwrap_or_default(),
        expires_at: v
            .get("expires_in")
            .and_then(Value::as_i64)
            .map(|e| now() + e)
            .unwrap_or(0),
        id_token: None,
    })
}

async fn refresh(http: &Client, t: &Tokens) -> Res<Tokens> {
    if t.refresh_token.is_empty() {
        return Err("Сессия истекла — войдите заново".into());
    }
    let v = token_request(
        http,
        &json!({
            "grant_type": "refresh_token",
            "refresh_token": t.refresh_token,
            "client_id": CLIENT_ID,
        }),
    )
    .await
    .map_err(|e| format!("Не удалось обновить сессию Claude: {e}"))?;
    tokens_from(&v, Some(t))
}

async fn load_profile(http: &Client, acc: &mut Account, t: &Tokens) -> Res<()> {
    let r = send(
        http.get(format!("{API}/api/oauth/profile"))
            .bearer_auth(&t.access_token)
            .header("anthropic-beta", BETA),
    )
    .await?;
    let v = r.json()?;
    if let Some(e) = v.pointer("/account/email").and_then(Value::as_str) {
        acc.email = Some(e.to_owned());
    }
    let tier = v
        .pointer("/organization/rate_limit_tier")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let plan = if tier.contains("max_20x") {
        Some("Max 20x".to_owned())
    } else if tier.contains("max_5x") {
        Some("Max 5x".to_owned())
    } else if v.pointer("/account/has_claude_max").and_then(Value::as_bool) == Some(true) {
        Some("Max".to_owned())
    } else if v.pointer("/account/has_claude_pro").and_then(Value::as_bool) == Some(true) {
        Some("Pro".to_owned())
    } else {
        v.pointer("/organization/organization_type")
            .and_then(Value::as_str)
            .map(|s| pretty_plan(s.trim_start_matches("claude_")))
    };
    if let Some(p) = plan {
        acc.set_meta("plan", p);
    }
    Ok(())
}

// ---- Claude Code credentials (~/.claude/.credentials.json) --------------------------

fn cli_path() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?;
    Some(dir.join(".credentials.json"))
}

fn read_cli() -> Res<(Tokens, Value)> {
    let path = cli_path().ok_or("Не найден домашний каталог")?;
    let raw = std::fs::read(&path)
        .map_err(|_| format!("Не найден {} — выполните вход в Claude Code", path.display()))?;
    let v: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let o = v
        .get("claudeAiOauth")
        .ok_or("В файле нет OAuth-сессии claude.ai")?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let access = s("accessToken");
    if access.is_empty() {
        return Err("В файле нет accessToken".into());
    }
    Ok((
        Tokens {
            access_token: access,
            refresh_token: s("refreshToken"),
            expires_at: o.get("expiresAt").and_then(parse_time).unwrap_or(0),
            id_token: None,
        },
        v,
    ))
}

fn write_cli(t: &Tokens) -> Res<()> {
    let path = cli_path().ok_or("Не найден домашний каталог")?;
    let (_, mut v) = read_cli()?;
    if let Some(o) = v.get_mut("claudeAiOauth").and_then(Value::as_object_mut) {
        o.insert("accessToken".into(), t.access_token.clone().into());
        o.insert("refreshToken".into(), t.refresh_token.clone().into());
        if t.expires_at > 0 {
            o.insert("expiresAt".into(), (t.expires_at * 1000).into());
        }
    }
    let data = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
    crate::store::write_atomic(&path, &data)
}

pub async fn import_cli(http: &Client) -> Res<Account> {
    let (tokens, v) = read_cli()?;
    let mut acc = Account {
        id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::Claude,
        label: String::new(),
        email: None,
        source: Source::Cli,
        meta: Map::new(),
        created_at: now(),
        secret: Default::default(),
    };
    if let Some(p) = v
        .pointer("/claudeAiOauth/subscriptionType")
        .and_then(Value::as_str)
    {
        acc.set_meta("plan", pretty_plan(p));
    }
    if !tokens.needs_refresh() {
        let _ = load_profile(http, &mut acc, &tokens).await;
    }
    acc.label = acc
        .email
        .clone()
        .unwrap_or_else(|| "Claude (Claude Code)".into());
    Ok(acc)
}

// ---- Fetch ------------------------------------------------------------------------------

fn load_tokens(acc: &Account) -> Res<Tokens> {
    match acc.source {
        Source::Cli => read_cli().map(|(t, _)| t),
        _ => acc
            .secret
            .tokens
            .clone()
            .ok_or_else(|| "Нет токенов — войдите заново".into()),
    }
}

fn store_tokens(acc: &mut Account, t: &Tokens) -> Res<()> {
    match acc.source {
        Source::Cli => write_cli(t),
        _ => {
            acc.secret.tokens = Some(t.clone());
            Ok(())
        }
    }
}

async fn usage(http: &Client, t: &Tokens) -> Res<super::Resp> {
    send(
        http.get(format!("{API}/api/oauth/usage"))
            .bearer_auth(&t.access_token)
            .header("anthropic-beta", BETA)
            .header("User-Agent", "claude-code/2.0.0 (OneMonitor)")
            .header("Accept", "application/json"),
    )
    .await
}

pub async fn fetch(http: &Client, acc: &mut Account, snap: &mut Snapshot) -> Res<()> {
    snap.link = Some("https://claude.ai/settings/usage".into());
    let mut tokens = load_tokens(acc)?;
    let mut refreshed = false;
    if tokens.needs_refresh() {
        tokens = refresh(http, &tokens).await?;
        store_tokens(acc, &tokens)?;
        refreshed = true;
    }
    let mut resp = usage(http, &tokens).await?;
    if resp.status == 401 && !refreshed {
        tokens = refresh(http, &tokens).await?;
        store_tokens(acc, &tokens)?;
        resp = usage(http, &tokens).await?;
    }
    if resp.status == 401 {
        return Err("Сессия Claude недействительна — войдите заново".into());
    }
    if resp.status == 429 {
        return Err("Слишком частые запросы к Claude — увеличьте интервал обновления".into());
    }
    let v = resp.json()?;
    parse_usage(&v, snap);

    if acc.email.is_none() || acc.meta_str("plan").is_none() {
        let _ = load_profile(http, acc, &tokens).await;
    }
    Ok(())
}

/// Display name and order of a usage bucket. The usage endpoint also returns
/// internal buckets under code names (e.g. `seven_day_iguana`); those mean
/// nothing to the user and are skipped.
fn limit_name(key: &str) -> Option<(u8, String)> {
    Some(match key {
        "five_hour" => (0, "5-часовое окно".into()),
        "seven_day" => (1, "Неделя · все модели".into()),
        "seven_day_oauth_apps" => (8, "Неделя · OAuth-приложения".into()),
        other => {
            let model = other.strip_prefix("seven_day_")?;
            let (order, name) = match model {
                "opus" => (2, "Opus"),
                "sonnet" => (3, "Sonnet"),
                "haiku" => (4, "Haiku"),
                "fable" => (5, "Fable"),
                _ => return None,
            };
            (order, format!("Неделя · {name}"))
        }
    })
}

pub(crate) fn parse_usage(v: &Value, snap: &mut Snapshot) {
    let Some(obj) = v.as_object() else { return };
    let mut limits: Vec<(u8, Limit)> = vec![];
    for (key, val) in obj {
        if key == "extra_usage" {
            continue;
        }
        let Some(util) = num(val.get("utilization")) else {
            continue;
        };
        let Some((order, name)) = limit_name(key) else {
            continue;
        };
        limits.push((
            order,
            Limit {
                key: key.clone(),
                name,
                used_percent: util.clamp(0.0, 100.0),
                resets_at: val.get("resets_at").and_then(parse_time),
                window_secs: match key.as_str() {
                    "five_hour" => Some(5 * 3600),
                    k if k.starts_with("seven_day") => Some(7 * 86400),
                    _ => None,
                },
                detail: None,
                ..Default::default()
            },
        ));
    }
    if let Some(extra) = obj.get("extra_usage").filter(|x| x.is_object()) {
        if extra.get("is_enabled").and_then(Value::as_bool) == Some(true) {
            if let Some(util) = num(extra.get("utilization")) {
                limits.push((
                    9,
                    Limit {
                        key: "extra_usage".into(),
                        name: "Доп. использование · месяц".into(),
                        used_percent: util.clamp(0.0, 100.0),
                        ..Default::default()
                    },
                ));
            }
        }
    }
    limits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
    snap.limits = limits.into_iter().map(|(_, l)| l).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn internal_code_name_buckets_are_hidden() {
        let mut snap = Snapshot::default();
        parse_usage(
            &json!({
                "five_hour": {"utilization": 20.0, "resets_at": "2026-10-01T12:00:00Z"},
                "seven_day": {"utilization": 41.0, "resets_at": "2026-10-04T10:00:00Z"},
                "seven_day_opus": {"utilization": 12.0, "resets_at": null},
                "seven_day_iguana": {"utilization": 99.0, "resets_at": null},
                "iguana_necktie": {"utilization": 100.0},
                "seven_day_oauth_apps": null,
                "extra_usage": {"is_enabled": false}
            }),
            &mut snap,
        );
        let names: Vec<_> = snap.limits.iter().map(|l| l.key.as_str()).collect();
        assert_eq!(names, ["five_hour", "seven_day", "seven_day_opus"]);
    }
}
