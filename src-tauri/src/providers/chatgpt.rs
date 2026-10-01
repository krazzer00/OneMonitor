//! ChatGPT (Codex) subscription limits.
//! Login uses the same OAuth PKCE flow as the official Codex CLI; limits come from
//! the endpoint Codex uses for `/status` (`/backend-api/wham/usage`).

use std::path::PathBuf;

use reqwest::Client;
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use super::{pretty_plan, send};
use crate::model::{Account, Kind, Limit, Snapshot, Source, Tokens};
use crate::oauth::{build_url, pkce, random_state, Loopback};
use crate::util::{jwt_claims, jwt_exp, now, num, Res};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
const PORT: u16 = 1455;
const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

pub async fn login(
    http: &Client,
    open: impl Fn(&str),
    cancel: oneshot::Receiver<()>,
) -> Res<Account> {
    let lb = Loopback::bind(PORT).await?;
    let redirect = format!("http://localhost:{PORT}/auth/callback");
    let pk = pkce();
    let state = random_state();
    let url = build_url(
        &format!("{ISSUER}/oauth/authorize"),
        &[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", &redirect),
            ("scope", "openid profile email offline_access"),
            ("code_challenge", &pk.challenge),
            ("code_challenge_method", "S256"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("state", &state),
            ("originator", "codex_cli_rs"),
        ],
    );
    open(&url);
    let code = lb.wait_code("/auth/callback", &state, cancel).await?;

    let resp = send(http.post(format!("{ISSUER}/oauth/token")).form(&[
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("client_id", CLIENT_ID),
        ("code_verifier", pk.verifier.as_str()),
    ]))
    .await?;
    let v = resp.json()?;
    let tokens = tokens_from(&v, None)?;
    let mut acc = Account {
        id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::ChatGpt,
        label: String::new(),
        email: None,
        source: Source::OAuth,
        meta: Map::new(),
        created_at: now(),
        secret: Default::default(),
    };
    apply_identity(&mut acc, &tokens);
    acc.label = acc.email.clone().unwrap_or_else(|| "ChatGPT".into());
    acc.secret.tokens = Some(tokens);
    Ok(acc)
}

fn tokens_from(v: &Value, old: Option<&Tokens>) -> Res<Tokens> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let access = s("access_token")
        .or_else(|| old.map(|o| o.access_token.clone()))
        .ok_or("В ответе нет access_token")?;
    Ok(Tokens {
        expires_at: v
            .get("expires_in")
            .and_then(Value::as_i64)
            .map(|e| now() + e)
            .unwrap_or_else(|| jwt_exp(&access)),
        refresh_token: s("refresh_token")
            .or_else(|| old.map(|o| o.refresh_token.clone()))
            .unwrap_or_default(),
        id_token: s("id_token").or_else(|| old.and_then(|o| o.id_token.clone())),
        access_token: access,
    })
}

/// Pulls email / account id / plan out of the id token (or the access token).
fn apply_identity(acc: &mut Account, t: &Tokens) {
    let claims = [t.id_token.as_deref(), Some(t.access_token.as_str())]
        .into_iter()
        .flatten()
        .filter_map(jwt_claims)
        .collect::<Vec<_>>();
    for c in &claims {
        let email = c
            .get("email")
            .or_else(|| c.pointer("/https:~1~1api.openai.com~1profile/email"))
            .and_then(Value::as_str);
        if let (None, Some(e)) = (&acc.email, email) {
            acc.email = Some(e.to_owned());
        }
        if let Some(auth) = c.get("https://api.openai.com/auth") {
            if let Some(id) = auth.get("chatgpt_account_id").and_then(Value::as_str) {
                if acc.meta_str("account_id").is_none() {
                    acc.set_meta("account_id", id);
                }
            }
            if let Some(p) = auth.get("chatgpt_plan_type").and_then(Value::as_str) {
                acc.set_meta("plan", pretty_plan(p));
            }
        }
    }
}

async fn refresh(http: &Client, t: &Tokens) -> Res<Tokens> {
    if t.refresh_token.is_empty() {
        return Err("Сессия истекла — войдите заново".into());
    }
    let resp = send(http.post(format!("{ISSUER}/oauth/token")).json(&json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": t.refresh_token,
        "scope": "openid profile email",
    })))
    .await?;
    if resp.status == 400 || resp.status == 401 {
        return Err("Сессия ChatGPT истекла — войдите заново".into());
    }
    tokens_from(&resp.json()?, Some(t))
}

// ---- Codex CLI credentials (~/.codex/auth.json) -------------------------------------

fn cli_path() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))?;
    Some(home.join("auth.json"))
}

fn read_cli() -> Res<(Tokens, Value)> {
    let path = cli_path().ok_or("Не найден домашний каталог")?;
    let raw = std::fs::read(&path)
        .map_err(|_| format!("Не найден {} — выполните вход в Codex CLI", path.display()))?;
    let v: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let t = v.get("tokens").ok_or("В auth.json нет ChatGPT-токенов (используется API-ключ?)")?;
    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let access = s("access_token");
    if access.is_empty() {
        return Err("В auth.json нет access_token".into());
    }
    Ok((
        Tokens {
            expires_at: jwt_exp(&access),
            access_token: access,
            refresh_token: s("refresh_token"),
            id_token: Some(s("id_token")).filter(|x| !x.is_empty()),
        },
        v,
    ))
}

fn write_cli(t: &Tokens) -> Res<()> {
    let path = cli_path().ok_or("Не найден домашний каталог")?;
    let (_, mut v) = read_cli()?;
    if let Some(obj) = v.get_mut("tokens").and_then(Value::as_object_mut) {
        obj.insert("access_token".into(), t.access_token.clone().into());
        obj.insert("refresh_token".into(), t.refresh_token.clone().into());
        if let Some(id) = &t.id_token {
            obj.insert("id_token".into(), id.clone().into());
        }
    }
    v["last_refresh"] = chrono::Utc::now().to_rfc3339().into();
    let data = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
    crate::store::write_atomic(&path, &data)
}

pub fn import_cli() -> Res<Account> {
    let (tokens, v) = read_cli()?;
    let mut acc = Account {
        id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::ChatGpt,
        label: String::new(),
        email: None,
        source: Source::Cli,
        meta: Map::new(),
        created_at: now(),
        secret: Default::default(),
    };
    if let Some(id) = v.pointer("/tokens/account_id").and_then(Value::as_str) {
        acc.set_meta("account_id", id);
    }
    apply_identity(&mut acc, &tokens);
    acc.label = acc.email.clone().unwrap_or_else(|| "ChatGPT (Codex CLI)".into());
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
    apply_identity(acc, t);
    match acc.source {
        Source::Cli => write_cli(t),
        _ => {
            acc.secret.tokens = Some(t.clone());
            Ok(())
        }
    }
}

pub async fn fetch(http: &Client, acc: &mut Account, snap: &mut Snapshot) -> Res<()> {
    snap.link = Some("https://chatgpt.com/codex/settings/usage".into());
    let mut tokens = load_tokens(acc)?;
    let mut refreshed = false;
    if tokens.needs_refresh() {
        tokens = refresh(http, &tokens).await?;
        store_tokens(acc, &tokens)?;
        refreshed = true;
    }
    if acc.meta_str("account_id").is_none() {
        apply_identity(acc, &tokens);
    }

    let mut resp = usage(http, acc, &tokens).await?;
    if (resp.status == 401 || resp.status == 403) && !refreshed {
        tokens = refresh(http, &tokens).await?;
        store_tokens(acc, &tokens)?;
        resp = usage(http, acc, &tokens).await?;
    }
    if resp.status == 401 {
        return Err("Сессия ChatGPT недействительна — войдите заново".into());
    }
    let v = resp.json()?;
    parse_usage(&v, snap);
    if let Some(p) = &snap.plan {
        acc.set_meta("plan", p.clone());
    }
    Ok(())
}

async fn usage(http: &Client, acc: &Account, t: &Tokens) -> Res<super::Resp> {
    let mut rb = http
        .get(USAGE_URL)
        .bearer_auth(&t.access_token)
        .header("User-Agent", "codex_cli_rs/0.50.0 (Windows 10.0; x86_64) OneMonitor")
        .header("originator", "codex_cli_rs")
        .header("Accept", "application/json");
    if let Some(id) = acc.meta_str("account_id") {
        rb = rb.header("ChatGPT-Account-Id", id);
    }
    send(rb).await
}

fn window_limit(key: &str, prefix: &str, w: &Value) -> Option<Limit> {
    let used = num(w.get("used_percent"))?;
    let window = w.get("limit_window_seconds").and_then(Value::as_i64);
    let resets_at = w
        .get("reset_at")
        .and_then(Value::as_i64)
        .filter(|t| *t > 0)
        .or_else(|| {
            w.get("reset_after_seconds")
                .and_then(Value::as_i64)
                .map(|s| now() + s)
        });
    let base = crate::util::window_name(window.unwrap_or(0));
    Some(Limit {
        key: key.to_owned(),
        name: if prefix.is_empty() {
            base
        } else {
            format!("{prefix} · {base}")
        },
        used_percent: used.clamp(0.0, 100.0),
        resets_at,
        window_secs: window,
        detail: None,
        ..Default::default()
    })
}

fn push_rate_limit(snap: &mut Snapshot, key: &str, prefix: &str, rl: &Value) {
    for (w, suffix) in [("primary_window", "primary"), ("secondary_window", "secondary")] {
        if let Some(win) = rl.get(w).filter(|x| x.is_object()) {
            if let Some(l) = window_limit(&format!("{key}.{suffix}"), prefix, win) {
                snap.limits.push(l);
            }
        }
    }
    if rl.get("limit_reached").and_then(Value::as_bool) == Some(true) && snap.warning.is_none() {
        snap.warning = Some(if prefix.is_empty() {
            "Лимит исчерпан".to_owned()
        } else {
            format!("{prefix}: лимит исчерпан")
        });
    }
}

pub(crate) fn parse_usage(v: &Value, snap: &mut Snapshot) {
    if let Some(p) = v.get("plan_type").and_then(Value::as_str) {
        snap.plan = Some(pretty_plan(p));
    }
    if let Some(rl) = v.get("rate_limit").filter(|x| x.is_object()) {
        push_rate_limit(snap, "codex", "", rl);
    }
    if let Some(rl) = v.get("code_review_rate_limit").filter(|x| x.is_object()) {
        push_rate_limit(snap, "review", "Code review", rl);
    }
    if let Some(extra) = v.get("additional_rate_limits").and_then(Value::as_array) {
        for (i, item) in extra.iter().enumerate() {
            let name = item
                .get("limit_name")
                .or_else(|| item.get("metered_feature"))
                .and_then(Value::as_str)
                .unwrap_or("Доп. лимит")
                .to_owned();
            if let Some(rl) = item.get("rate_limit").filter(|x| x.is_object()) {
                push_rate_limit(snap, &format!("extra{i}"), &name, rl);
            }
        }
    }
    if let Some(c) = v.get("credits").filter(|x| x.is_object()) {
        if c.get("unlimited").and_then(Value::as_bool) == Some(true) {
            snap.note("Кредиты", "без ограничений");
        } else if c.get("has_credits").and_then(Value::as_bool) == Some(true) {
            if let Some(b) = num(c.get("balance")) {
                snap.note("Кредиты", format!("{b:.0}"));
            }
        }
    }
}
