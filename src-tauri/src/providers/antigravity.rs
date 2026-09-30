//! Google Antigravity model quotas.
//! Login is the Google OAuth flow used by the Antigravity IDE; quotas come from the
//! Cloud Code Assist `fetchAvailableModels` endpoint (remainingFraction / resetTime).

use std::collections::BTreeMap;

use reqwest::Client;
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use super::{pretty_plan, send};
use crate::model::{Account, Kind, Limit, Snapshot, Source, Tokens};
use crate::oauth::{build_url, pkce, random_state, Loopback};
use crate::util::{now, num, parse_time, Res};

const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";
const ENDPOINTS: [&str; 2] = [
    "https://cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
];
const UA: &str = "antigravity/1.15.8 windows/amd64";

pub async fn login(
    http: &Client,
    open: impl Fn(&str),
    cancel: oneshot::Receiver<()>,
) -> Res<Account> {
    let lb = Loopback::bind(0).await?;
    let redirect = format!("http://localhost:{}/oauth-callback", lb.port);
    let pk = pkce();
    let state = random_state();
    let url = build_url(
        "https://accounts.google.com/o/oauth2/v2/auth",
        &[
            ("client_id", CLIENT_ID),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", SCOPES),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("code_challenge", &pk.challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
        ],
    );
    open(&url);
    let code = lb.wait_code("/oauth-callback", &state, cancel).await?;

    let v = send(http.post("https://oauth2.googleapis.com/token").form(&[
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("code", code.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect.as_str()),
        ("code_verifier", pk.verifier.as_str()),
    ]))
    .await?
    .json()?;
    let tokens = tokens_from(&v, None)?;
    if tokens.refresh_token.is_empty() {
        return Err("Google не выдал refresh_token — повторите вход".into());
    }

    let mut acc = Account {
        id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::Antigravity,
        label: String::new(),
        email: None,
        source: Source::OAuth,
        meta: Map::new(),
        created_at: now(),
        secret: Default::default(),
    };
    if let Ok(info) = send(
        http.get("https://www.googleapis.com/oauth2/v2/userinfo")
            .bearer_auth(&tokens.access_token),
    )
    .await
    .and_then(|r| r.json())
    {
        acc.email = info.get("email").and_then(Value::as_str).map(str::to_owned);
    }
    let _ = load_code_assist(http, &mut acc, &tokens).await;
    acc.label = acc.email.clone().unwrap_or_else(|| "Antigravity".into());
    acc.secret.tokens = Some(tokens);
    Ok(acc)
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
        id_token: s("id_token"),
    })
}

async fn refresh(http: &Client, t: &Tokens) -> Res<Tokens> {
    let r = send(http.post("https://oauth2.googleapis.com/token").form(&[
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("refresh_token", t.refresh_token.as_str()),
        ("grant_type", "refresh_token"),
    ]))
    .await?;
    if r.status == 400 || r.status == 401 {
        return Err("Сессия Google истекла — войдите заново".into());
    }
    tokens_from(&r.json()?, Some(t))
}

async fn cloudcode(http: &Client, method: &str, t: &Tokens, body: &Value) -> Res<Value> {
    let mut last = String::new();
    for base in ENDPOINTS {
        let r = send(
            http.post(format!("{base}/v1internal:{method}"))
                .bearer_auth(&t.access_token)
                .header("User-Agent", UA)
                .header("X-Goog-Api-Client", "google-cloud-sdk vscode_cloudshelleditor/0.1")
                .header(
                    "Client-Metadata",
                    r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#,
                )
                .json(body),
        )
        .await;
        match r {
            Ok(r) if r.ok() => return r.json(),
            Ok(r) if r.status == 401 => return Err("401".into()),
            Ok(r) => last = crate::util::api_error(r.status, &r.body),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Resolves the Cloud Code project and the subscription tier.
async fn load_code_assist(http: &Client, acc: &mut Account, t: &Tokens) -> Res<()> {
    let v = cloudcode(
        http,
        "loadCodeAssist",
        t,
        &json!({ "metadata": { "ideType": "ANTIGRAVITY", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI" } }),
    )
    .await?;
    let project = match v.get("cloudaicompanionProject") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(o) => o.get("id").and_then(Value::as_str).map(str::to_owned),
        None => None,
    };
    if let Some(p) = project {
        acc.set_meta("project_id", p);
    }
    let tier = v
        .get("paidTier")
        .or_else(|| v.get("currentTier"))
        .and_then(|t| t.get("name").or_else(|| t.get("id")))
        .and_then(Value::as_str);
    if let Some(t) = tier {
        acc.set_meta("plan", pretty_plan(t.trim_end_matches("-tier")));
    }
    Ok(())
}

pub async fn fetch(http: &Client, acc: &mut Account, snap: &mut Snapshot) -> Res<()> {
    snap.link = Some("https://antigravity.google/".into());
    let mut tokens = acc
        .secret
        .tokens
        .clone()
        .ok_or("Нет токенов — войдите заново")?;
    let mut refreshed = false;
    if tokens.needs_refresh() {
        tokens = refresh(http, &tokens).await?;
        acc.secret.tokens = Some(tokens.clone());
        refreshed = true;
    }
    if acc.meta_str("project_id").is_none() {
        let _ = load_code_assist(http, acc, &tokens).await;
    }
    let body = match acc.meta_str("project_id") {
        Some(p) => json!({ "project": p }),
        None => json!({}),
    };
    let v = match cloudcode(http, "fetchAvailableModels", &tokens, &body).await {
        Err(e) if e == "401" && !refreshed => {
            tokens = refresh(http, &tokens).await?;
            acc.secret.tokens = Some(tokens.clone());
            cloudcode(http, "fetchAvailableModels", &tokens, &body).await
        }
        other => other,
    }
    .map_err(|e| {
        if e == "401" {
            "Сессия Google недействительна — войдите заново".to_owned()
        } else {
            e
        }
    })?;
    parse_models(&v, snap);
    if snap.limits.is_empty() {
        snap.warning = Some("Сервер не вернул квоты моделей".into());
    }
    Ok(())
}

/// Buckets models into quota families so the panel stays compact.
fn family(id: &str, name: &str) -> (u8, String) {
    let s = format!("{id} {name}").to_lowercase();
    if s.contains("image") {
        (4, "Gemini Image".into())
    } else if s.contains("claude") {
        (0, "Claude".into())
    } else if s.contains("gpt-oss") || s.contains("gpt_oss") {
        (3, "GPT-OSS".into())
    } else if s.contains("gemini") && s.contains("pro") {
        (1, "Gemini Pro".into())
    } else if s.contains("gemini") && s.contains("flash") {
        (2, "Gemini Flash".into())
    } else {
        (5, name.to_owned())
    }
}

pub(crate) fn parse_models(v: &Value, snap: &mut Snapshot) {
    let Some(models) = v.get("models").and_then(Value::as_object) else {
        return;
    };
    // family -> (order, min remaining, earliest reset, model names)
    let mut groups: BTreeMap<String, (u8, f64, Option<i64>, Vec<String>)> = BTreeMap::new();
    for (id, m) in models {
        let Some(q) = m.get("quotaInfo") else { continue };
        let lid = id.to_lowercase();
        if lid.starts_with("chat_") || lid.starts_with("tab_") || lid.contains("autocomplete") {
            continue;
        }
        let name = m
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned();
        let remaining = num(q.get("remainingFraction")).unwrap_or(0.0).clamp(0.0, 1.0);
        let reset = q.get("resetTime").and_then(parse_time);
        let (order, fam) = family(id, &name);
        let e = groups.entry(fam).or_insert((order, 1.0, None, vec![]));
        e.1 = e.1.min(remaining);
        e.2 = match (e.2, reset) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if !e.3.contains(&name) {
            e.3.push(name);
        }
    }
    let mut list: Vec<_> = groups.into_iter().collect();
    list.sort_by(|a, b| a.1 .0.cmp(&b.1 .0).then(a.0.cmp(&b.0)));
    snap.limits = list
        .into_iter()
        .map(|(fam, (_, rem, reset, mut names))| {
            names.sort();
            Limit {
                key: fam.clone(),
                name: fam,
                used_percent: ((1.0 - rem) * 100.0).clamp(0.0, 100.0),
                resets_at: reset,
                window_secs: None,
                detail: Some(names.join(", ")),
            }
        })
        .collect();
}
