pub mod antigravity;
pub mod chatgpt;
pub mod claude;
pub mod oneprovider;
pub mod openrouter;

use std::time::{Duration, Instant};

use reqwest::{Client, RequestBuilder};
use serde_json::Value;

use crate::model::{Account, Health, Kind, Settings, Snapshot};
use crate::util::{api_error, net_error, now, Res};

pub const UA: &str = concat!("OneMonitor/", env!("CARGO_PKG_VERSION"));

pub fn client() -> Client {
    Client::builder()
        .user_agent(UA)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()
        .expect("http client")
}

pub struct Resp {
    pub status: u16,
    pub body: String,
    pub ms: u64,
}

impl Resp {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Res<Value> {
        if !self.ok() {
            return Err(api_error(self.status, &self.body));
        }
        serde_json::from_str(&self.body).map_err(|e| format!("Некорректный ответ сервера: {e}"))
    }
}

pub async fn send(rb: RequestBuilder) -> Res<Resp> {
    let t0 = Instant::now();
    let resp = rb.send().await.map_err(net_error)?;
    let ms = t0.elapsed().as_millis() as u64;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(net_error)?;
    Ok(Resp { status, body, ms })
}

/// Result of one refresh of one account.
pub struct Outcome {
    pub snap: Snapshot,
    /// Present when the account itself changed (refreshed tokens, discovered metadata).
    pub account: Option<Account>,
}

pub async fn fetch(http: &Client, acc: Account, settings: &Settings) -> Outcome {
    let mut acc = acc;
    let before = format!("{:?}{:?}{:?}", acc.secret, acc.meta, acc.email);
    let mut snap = Snapshot::for_account(&acc);
    let result = match acc.kind {
        Kind::OneProvider => oneprovider::fetch(http, &mut acc, &mut snap).await,
        Kind::OpenRouter => openrouter::fetch(http, &acc, &mut snap).await,
        Kind::ChatGpt => chatgpt::fetch(http, &mut acc, &mut snap).await,
        Kind::Claude => claude::fetch(http, &mut acc, &mut snap).await,
        Kind::Antigravity => antigravity::fetch(http, &mut acc, &mut snap).await,
    };
    if let Err(e) = result {
        snap.error = Some(e);
    }
    snap.updated_at = now();
    snap.plan = snap.plan.take().or_else(|| acc.meta_str("plan"));
    snap.email = acc.email.clone();
    evaluate(&mut snap, settings);
    let after = format!("{:?}{:?}{:?}", acc.secret, acc.meta, acc.email);
    Outcome {
        snap,
        account: (before != after).then_some(acc),
    }
}

pub fn evaluate(snap: &mut Snapshot, settings: &Settings) {
    let mut h = Health::Ok;
    if snap.warning.is_some() {
        h = Health::Warn;
    }
    if let Some(b) = &snap.balance {
        if b.amount < settings.low_balance || b.active == Some(false) {
            h = h.max(Health::Warn);
        }
    }
    if snap
        .limits
        .iter()
        .any(|l| l.used_percent >= settings.warn_percent)
    {
        h = h.max(Health::Warn);
    }
    if let Some(s) = &snap.service {
        if !s.up {
            h = Health::Error;
        }
    }
    if snap.error.is_some() {
        h = Health::Error;
    }
    snap.state = h;
}

/// Human friendly plan names.
pub fn pretty_plan(raw: &str) -> String {
    let r = raw.trim().to_lowercase();
    let known = [
        ("free", "Free"),
        ("go", "Go"),
        ("plus", "Plus"),
        ("pro", "Pro"),
        ("team", "Team"),
        ("business", "Business"),
        ("enterprise", "Enterprise"),
        ("edu", "Edu"),
        ("max", "Max"),
    ];
    for (k, v) in known {
        if r == k {
            return v.into();
        }
    }
    let mut out = String::new();
    for (i, w) in r.split(['_', '-', ' ']).filter(|w| !w.is_empty()).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut c = w.chars();
        if let Some(f) = c.next() {
            out.extend(f.to_uppercase());
            out.push_str(c.as_str());
        }
    }
    out
}
