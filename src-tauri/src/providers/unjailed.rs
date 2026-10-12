//! Unjailed AI: OpenAI-compatible API (key check), live per-model status and,
//! through a website session, the wallet (balance, plan, spending).
//! Docs: https://unjailedai.com/docs and https://unjailedai.com/status

use std::collections::BTreeMap;

use reqwest::Client;
use serde_json::Value;

use super::{add_forecast, remember_balance, restore_last_balance, send, spend_from_days, Resp};
use crate::model::{Account, Balance, Component, Service, Snapshot, Spend};
use crate::util::{api_error, fmt_local, num, parse_time, Res};

pub const SITE: &str = "https://unjailedai.com";
pub const LOGIN_URL: &str = "https://unjailedai.com/login";
const STATUS_URL: &str = "https://unjailedai.com/api/status";
const WALLET_URL: &str = "https://unjailedai.com/api/wallet";
pub const ME_URL: &str = "https://unjailedai.com/api/me";

/// Plans as published on the site: (id, name, requests per minute).
const PLANS: [(&str, &str, u32); 4] = [
    ("weekly", "Starter", 30),
    ("monthly", "Pro", 60),
    ("power", "Max 5×", 120),
    ("max20", "Max 20×", 200),
];
const FREE_RATE_LIMIT: u32 = 20;

pub async fn fetch(http: &Client, acc: &mut Account, snap: &mut Snapshot) -> Res<()> {
    snap.link = Some(format!("{SITE}/status"));
    let key = acc
        .secret
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned);
    let cookie = acc.secret.cookie.clone().filter(|c| !c.is_empty());

    let probe = {
        let mut rb = http.get(format!("{SITE}/v1/models"));
        if let Some(k) = &key {
            rb = rb.bearer_auth(k);
        }
        send(rb)
    };
    let status = send(http.get(STATUS_URL).header("Cache-Control", "no-cache"));
    let wallet = async {
        match &cookie {
            Some(c) => Some(send(http.get(WALLET_URL).header("Cookie", c)).await),
            None => None,
        }
    };
    let (probe, status, wallet) = tokio::join!(probe, status, wallet);

    let mut service = service_from_probe(&probe, key.is_some());
    match status.as_ref().map(|r| r.json()) {
        Ok(Ok(v)) => apply_model_status(&mut service, &v),
        _ => service.models_status = "unavailable".into(),
    }
    if !service.issues.is_empty() {
        snap.warning = Some(format!("Перебои у моделей: {}", service.issues.join(", ")));
    }
    let key_rejected = key.is_some() && matches!(&probe, Ok(r) if r.status == 401 || r.status == 403);
    let service_up = service.up;
    snap.service = Some(service);

    // ---- wallet through the website session
    snap.session = "none".into();
    match wallet {
        Some(Ok(resp)) if resp.ok() => {
            snap.session = "ok".into();
            apply_wallet(acc, snap, &resp.json()?);
        }
        Some(Ok(resp)) if resp.status == 401 || resp.status == 403 => {
            snap.session = "expired".into();
            restore_last_balance(acc, snap);
            snap.warning.get_or_insert(
                "Сессия на сайте закончилась — нажмите «Войти на сайт», чтобы снова видеть баланс".into(),
            );
        }
        Some(Ok(resp)) => {
            snap.session = "ok".into();
            restore_last_balance(acc, snap);
            snap.warning
                .get_or_insert(format!("Баланс недоступен: {}", api_error(resp.status, &resp.body)));
        }
        Some(Err(e)) => {
            snap.session = "ok".into();
            restore_last_balance(acc, snap);
            if service_up {
                snap.warning.get_or_insert(format!("Баланс недоступен: {e}"));
            }
        }
        None => {}
    }
    if let Some(user) = acc.meta_str("site_user") {
        snap.note("Аккаунт", user);
    }
    if key.is_none() {
        snap.note("API-ключ", "не указан — проверяется только доступность");
    }
    if key_rejected {
        return Err("API-ключ не принят (401). Проверьте ключ.".into());
    }
    if !service_up && snap.balance.is_none() {
        return Err(snap.service.as_ref().map(|s| s.message.clone()).unwrap_or_default());
    }
    add_forecast(snap);
    Ok(())
}

fn service_from_probe(probe: &Res<Resp>, has_key: bool) -> Service {
    match probe {
        Ok(r) => Service {
            up: r.status < 500,
            latency_ms: Some(r.ms),
            code: Some(r.status),
            message: match r.status {
                200..=299 => "API работает".to_owned(),
                401 if !has_key => "API доступен".to_owned(),
                401 | 403 => "API доступен, ключ отклонён".to_owned(),
                402 => "API доступен, на балансе нет средств".to_owned(),
                429 => "API доступен, слишком много запросов".to_owned(),
                s if s < 500 => format!("API доступен (HTTP {s})"),
                s => format!("Сбой API (HTTP {s})"),
            },
            ..Default::default()
        },
        Err(e) => Service {
            up: false,
            message: e.clone(),
            ..Default::default()
        },
    }
}

/// `/api/status`: observed health of every model.
fn apply_model_status(service: &mut Service, v: &Value) {
    service.components = v
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let name = m.get("model").and_then(Value::as_str)?.to_owned();
            let state = match m.get("status").and_then(Value::as_str) {
                Some("up") => "ok",
                Some("degraded") => "degraded",
                Some("down") => "down",
                _ => "unknown",
            };
            Some(Component {
                name,
                uptime: num(m.get("availability_pct")).unwrap_or(0.0).clamp(0.0, 100.0),
                state: state.into(),
                granularity: "now".into(),
                latency_s: num(m.get("avg_latency_s")),
                last_probe_at: v.get("generated_at").and_then(parse_time),
                ..Default::default()
            })
        })
        .collect();
    service.models_status = if service.components.is_empty() { "empty" } else { "ok" }.into();

    let mut names = vec![];
    for c in &service.components {
        let what = match c.state.as_str() {
            "down" => "сбой",
            "degraded" => "перебои",
            _ => continue,
        };
        service.issues.push(format!("{}: {what}", c.name));
        names.push(c.name.clone());
    }
    if service.up && !names.is_empty() {
        service.message = if names.len() == service.components.len() {
            "Все модели недоступны".to_owned()
        } else {
            format!("{} · перебои: {}", service.message, names.join(", "))
        };
    }
}

fn cents(v: Option<&Value>) -> Option<f64> {
    num(v).map(|c| c / 100.0)
}

/// `/api/wallet`: balance, plan and the ledger of credits and spends.
fn apply_wallet(acc: &mut Account, snap: &mut Snapshot, v: &Value) {
    if let Some(amount) = cents(v.get("balanceCents")) {
        snap.balance = Some(Balance {
            amount,
            currency: "USD".into(),
            ..Default::default()
        });
        remember_balance(acc, amount);
    }

    let plan_id = v.pointer("/plan/id").and_then(Value::as_str).filter(|p| *p != "none");
    let known = plan_id.and_then(|id| PLANS.iter().find(|p| p.0 == id));
    let rate = match plan_id {
        Some(_) => known.map(|p| p.2),
        None => Some(FREE_RATE_LIMIT),
    };
    snap.plan = plan_id.map(|id| {
        v.pointer("/plan/name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| known.map(|p| p.1.to_owned()))
            .unwrap_or_else(|| super::pretty_plan(id))
    });
    if plan_id.is_none() {
        snap.note("Тариф", "без подписки — оплата с баланса");
    }
    if let Some(t) = v.pointer("/plan/renewsAt").and_then(parse_time) {
        snap.note("Тариф продлится", fmt_local(t));
    }
    if let Some(r) = rate {
        snap.note("Лимит запросов", format!("{r} в минуту"));
    }
    if let (Some(amount), Some(t)) = (
        cents(v.pointer("/latestPlanCredit/amountCents")),
        v.pointer("/latestPlanCredit/grantedAt").and_then(parse_time),
    ) {
        snap.note("Кредит по тарифу", format!("+{} · {}", crate::util::fmt_usd(amount), fmt_local(t)));
    }
    snap.spend = v.get("ledger").and_then(Value::as_array).map(|l| parse_ledger(l));
}

/// Spends from the wallet ledger, by local day.
fn parse_ledger(ledger: &[Value]) -> Spend {
    use chrono::TimeZone;
    let mut by_day: BTreeMap<chrono::NaiveDate, (f64, u64)> = BTreeMap::new();
    for e in ledger {
        if e.get("kind").and_then(Value::as_str) != Some("spend") {
            continue;
        }
        let (Some(amount), Some(t)) = (cents(e.get("amountCents")), e.get("createdAt").and_then(parse_time))
        else {
            continue;
        };
        let Some(date) = chrono::Local.timestamp_opt(t, 0).single().map(|d| d.date_naive()) else {
            continue;
        };
        let d = by_day.entry(date).or_default();
        d.0 += amount.abs();
        d.1 += 1;
    }
    spend_from_days(&by_day, chrono::Local::now().date_naive(), vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> Account {
        Account {
            id: "t".into(),
            kind: crate::model::Kind::Unjailed,
            label: "t".into(),
            email: None,
            source: crate::model::Source::ApiKey,
            meta: Default::default(),
            created_at: 0,
            secret: Default::default(),
        }
    }

    #[test]
    fn model_status_from_live_shape() {
        // shape of https://unjailedai.com/api/status
        let v = json!({"object": "status", "generated_at": "2026-10-12T14:25:12.159Z",
            "summary": {"total": 3, "up": 1, "degraded": 1, "down": 1},
            "models": [
                {"model": "glm-5.3", "status": "up", "availability_pct": 100, "avg_latency_s": 14.9},
                {"model": "kimi-k3", "status": "degraded", "availability_pct": 91.5, "avg_latency_s": 30.2},
                {"model": "claude-opus-5-5", "status": "down", "availability_pct": 40, "avg_latency_s": null}
            ]});
        let mut svc = Service { up: true, message: "API работает".into(), ..Default::default() };
        apply_model_status(&mut svc, &v);
        assert_eq!(svc.components.len(), 3);
        assert_eq!(svc.components[0].latency_s, Some(14.9));
        assert_eq!(svc.components[0].granularity, "now");
        assert_eq!(svc.issues, ["kimi-k3: перебои", "claude-opus-5-5: сбой"]);
        assert_eq!(svc.message, "API работает · перебои: kimi-k3, claude-opus-5-5");
    }

    #[test]
    fn wallet_balance_plan_and_ledger() {
        let now = chrono::Utc::now();
        let ago = |h: i64| (now - chrono::Duration::hours(h)).to_rfc3339();
        let v = json!({
            "balanceCents": 1234,
            "plan": {"id": "monthly", "name": "Pro", "renewsAt": "2026-11-01T00:00:00Z"},
            "latestPlanCredit": {"amountCents": 3000, "grantedAt": "2026-10-01T00:00:00Z"},
            "ledger": [
                {"kind": "spend", "amountCents": -150, "createdAt": ago(0)},
                {"kind": "spend", "amountCents": -50, "createdAt": ago(0)},
                {"kind": "plan_grant", "amountCents": 3000, "createdAt": ago(1)},
                {"kind": "spend", "amountCents": -400, "createdAt": ago(24 * 10)}
            ],
            "payments": []
        });
        let mut acc = account();
        let mut snap = Snapshot::default();
        apply_wallet(&mut acc, &mut snap, &v);
        assert_eq!(snap.balance.as_ref().unwrap().amount, 12.34);
        assert_eq!(snap.plan.as_deref(), Some("Pro"));
        let note = |l: &str| snap.notes.iter().find(|n| n.label == l).map(|n| n.value.clone());
        assert_eq!(note("Лимит запросов").as_deref(), Some("60 в минуту"));
        assert!(note("Тариф продлится").is_some());
        let s = snap.spend.unwrap();
        assert_eq!(s.today_requests, 2);
        assert!((s.today - 2.0).abs() < 1e-9, "spends are positive");
        assert!((s.month - 6.0).abs() < 1e-9);
        assert_eq!(acc.meta.get("last_balance").and_then(Value::as_f64), Some(12.34));
    }

    #[test]
    fn wallet_without_plan() {
        let mut snap = Snapshot::default();
        apply_wallet(&mut account(), &mut snap, &json!({"balanceCents": 0, "plan": null, "ledger": []}));
        assert!(snap.plan.is_none());
        assert!(snap.notes.iter().any(|n| n.value == "20 в минуту"));
    }
}
