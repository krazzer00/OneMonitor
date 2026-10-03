//! OneProvider gateway: liveness probe, public model observations and key balance.
//! Docs: https://oneprovider.dev/docs/llms.txt (§1, §10.1) and https://oneprovider.dev/status

use std::sync::atomic::{AtomicUsize, Ordering};

use reqwest::Client;
use serde_json::Value;

use super::{send, Resp};
use crate::model::{Account, Balance, Component, DayCost, ModelCost, Service, Snapshot, Spend};
use crate::util::{num, parse_time, Res};

const BASE: &str = "https://api.oneprovider.dev";

/// Where the key balance can be read. The documented endpoint has moved between
/// hosts and at times disappeared (404 everywhere), so try every known location
/// and remember the one that answered.
const BALANCE_URLS: [&str; 2] = [
    "https://api.oneprovider.dev/v1/dashboard/balance",
    "https://dashboard.oneprovider.dev/v1/dashboard/balance",
];
/// Per-key usage statistics: quota, daily spend, per-model totals.
const USAGE_URL: &str = "https://api.oneprovider.dev/v1/usage";
static BALANCE_HOST: AtomicUsize = AtomicUsize::new(0);

async fn fetch_balance(http: &Client, key: &str) -> Res<Resp> {
    let first = BALANCE_HOST.load(Ordering::Relaxed) % BALANCE_URLS.len();
    let mut last = None;
    for i in 0..BALANCE_URLS.len() {
        let idx = (first + i) % BALANCE_URLS.len();
        let resp = send(http.get(BALANCE_URLS[idx]).bearer_auth(key)).await?;
        // 404/405: the endpoint does not live there (any more), try the next one.
        if resp.status == 404 || resp.status == 405 {
            last = Some(resp);
            continue;
        }
        BALANCE_HOST.store(idx, Ordering::Relaxed);
        return Ok(resp);
    }
    Ok(last.expect("at least one balance url"))
}

pub async fn fetch(http: &Client, acc: &mut Account, snap: &mut Snapshot) -> Res<()> {
    snap.link = Some("https://oneprovider.dev/status".into());
    let key = acc
        .secret
        .api_key
        .clone()
        .filter(|k| !k.trim().is_empty());

    let probe = {
        let mut rb = http.get(format!("{BASE}/v1/models"));
        if let Some(k) = &key {
            rb = rb.bearer_auth(k.trim());
        }
        send(rb)
    };
    let families = send(
        http.get(format!("{BASE}/_model_status.json"))
            .header("Cache-Control", "no-cache"),
    );
    let balance = async {
        match &key {
            Some(k) => Some(fetch_balance(http, k.trim()).await),
            None => None,
        }
    };
    let usage = async {
        match &key {
            Some(k) => send(http.get(USAGE_URL).bearer_auth(k.trim()))
                .await
                .ok()
                .filter(Resp::ok)
                .and_then(|r| r.json().ok()),
            None => None,
        }
    };
    let (probe, families, balance, usage) = tokio::join!(probe, families, balance, usage);
    snap.spend = usage.as_ref().map(parse_usage);

    let mut service = service_from_probe(&probe, key.is_some());
    service.models_status = match families.as_ref().map(|r| r.json()) {
        Ok(Ok(v)) => {
            service.components = parse_families(&v, crate::util::now());
            if service.components.is_empty() { "empty" } else { "ok" }
        }
        _ => "unavailable",
    }
    .to_owned();
    apply_family_issues(&mut service);
    if !service.issues.is_empty() {
        snap.warning = Some(format!("Перебои у моделей: {}", service.issues.join(", ")));
    }
    let service_up = service.up;
    let key_rejected = matches!(&probe, Ok(r) if r.status == 401) && key.is_some();
    snap.service = Some(service);

    if key.is_none() {
        snap.note("Режим", "только статус (без ключа)");
        return Ok(());
    }
    if key_rejected {
        return Err("API-ключ не принят (401). Проверьте ключ.".into());
    }

    match balance {
        Some(Ok(resp)) if resp.ok() => {
            let v = resp.json()?;
            let Some(amount) = find_num(&v, AMOUNT_KEYS, 3) else {
                let keys = v
                    .as_object()
                    .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                snap.warning = Some(format!("Неизвестный формат ответа баланса (поля: {keys})"));
                restore_last_balance(acc, snap);
                return Ok(());
            };
            let expires_at = find_val(&v, &["expires_at"], 3).and_then(parse_time);
            let active = find_val(&v, &["is_active", "active"], 3).and_then(Value::as_bool);
            snap.balance = Some(Balance {
                amount,
                currency: "USD".into(),
                expires_at,
                active,
                used: find_num(&v, &["used_usd", "total_used", "total_usage", "spent_usd"], 3),
                ..Default::default()
            });
            remember_balance(acc, amount);
            if let Some(t) = v.get("last_synced_at").and_then(parse_time) {
                snap.note("Синхронизация баланса", crate::util::fmt_local(t));
            }
            if active == Some(false) {
                snap.warning = Some("Ключ приостановлен или отключён".into());
            } else if let Some(exp) = expires_at {
                let left = exp - crate::util::now();
                if left <= 0 {
                    snap.warning = Some("Срок действия ключа истёк".into());
                } else if left < 3 * 86400 {
                    snap.warning = Some("Ключ скоро истекает".into());
                }
            }
        }
        Some(Ok(resp)) if resp.status == 429 => {
            snap.warning = Some("Слишком частые запросы баланса (лимит 30/мин)".into());
            restore_last_balance(acc, snap);
        }
        Some(Ok(resp)) if resp.status == 401 || resp.status == 403 => {
            return Err(format!(
                "Ключ не принят сервисом баланса: {}",
                crate::util::api_error(resp.status, &resp.body)
            ));
        }
        Some(Ok(resp)) => {
            let msg = if resp.status == 404 {
                "OneProvider сейчас не отдаёт баланс (эндпоинт отвечает 404 — проблема на их стороне)"
                    .to_owned()
            } else {
                format!("Баланс недоступен: {}", crate::util::api_error(resp.status, &resp.body))
            };
            restore_last_balance(acc, snap);
            if service_up {
                snap.warning = Some(msg);
            } else {
                return Err(msg);
            }
        }
        Some(Err(e)) => {
            restore_last_balance(acc, snap);
            if service_up {
                snap.warning = Some(format!("Баланс недоступен: {e}"));
            } else {
                return Err(e);
            }
        }
        None => {}
    }

    if let (Some(spend), Some(b)) = (snap.spend.as_mut(), snap.balance.as_ref()) {
        let per_day = spend.week / 7.0;
        if per_day > 0.01 && !b.stale {
            spend.forecast_days = Some(b.amount / per_day);
        }
    }
    Ok(())
}

/// Cost actually charged for a usage record (falls back to the list price).
fn cost_of(v: &Value) -> f64 {
    num(v.get("actual_cost")).or_else(|| num(v.get("cost"))).unwrap_or(0.0)
}

fn parse_usage(v: &Value) -> Spend {
    use chrono::{Duration, NaiveDate};

    let mut by_day: std::collections::BTreeMap<NaiveDate, (f64, u64)> = Default::default();
    for d in v.get("daily_usage").and_then(Value::as_array).into_iter().flatten() {
        let Some(date) = d
            .get("date")
            .and_then(Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        else {
            continue;
        };
        let e = by_day.entry(date).or_default();
        e.0 += cost_of(d);
        e.1 += d.get("requests").and_then(Value::as_u64).unwrap_or(0);
    }

    let today = chrono::Local::now().date_naive();
    let today_v = v.pointer("/usage/today");
    if let Some(t) = today_v {
        // the "today" block is fresher than the daily list
        by_day.insert(
            today,
            (cost_of(t), t.get("requests").and_then(Value::as_u64).unwrap_or(0)),
        );
    }
    let sum_since = |days: i64| -> f64 {
        let from = today - Duration::days(days - 1);
        by_day.range(from..).map(|(_, (c, _))| c).sum()
    };

    let daily = (0..14)
        .rev()
        .map(|i| {
            let date = today - Duration::days(i);
            let (cost, requests) = by_day.get(&date).copied().unwrap_or_default();
            DayCost {
                date: date.format("%Y-%m-%d").to_string(),
                cost,
                requests,
            }
        })
        .collect();

    let mut top_models: Vec<ModelCost> = v
        .get("model_stats")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ModelCost {
                name: m.get("model").and_then(Value::as_str)?.to_owned(),
                cost: cost_of(m),
                requests: m.get("requests").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect();
    top_models.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    top_models.truncate(3);

    let (today_cost, today_requests) = by_day.get(&today).copied().unwrap_or_default();
    Spend {
        currency: v
            .get("unit")
            .and_then(Value::as_str)
            .unwrap_or("USD")
            .to_owned(),
        today: today_cost,
        today_requests,
        week: sum_since(7),
        month: sum_since(30),
        daily,
        top_models,
        forecast_days: None,
        quota_limit: num(v.pointer("/quota/limit")),
        quota_used: num(v.pointer("/quota/used")),
        quota_remaining: num(v.pointer("/quota/remaining")).or_else(|| num(v.get("remaining"))),
    }
}

fn service_from_probe(probe: &Res<Resp>, has_key: bool) -> Service {
    match probe {
        Ok(r) => {
            let up = r.status < 500;
            let message = match r.status {
                200..=299 => "API работает".to_owned(),
                401 if !has_key => "API доступен".to_owned(),
                401 => "API доступен, ключ отклонён".to_owned(),
                429 => "API доступен, баланс исчерпан".to_owned(),
                s if s < 500 => format!("API доступен (HTTP {s})"),
                502 => "Ошибка апстрима (502)".to_owned(),
                503 => "Пул апстримов перегружен (503)".to_owned(),
                504 => "Таймаут апстрима (504)".to_owned(),
                s => format!("Сбой шлюза (HTTP {s})"),
            };
            Service {
                up,
                latency_ms: Some(r.ms),
                code: Some(r.status),
                message,
                components: vec![],
                issues: vec![],
                models_status: String::new(),
            }
        }
        Err(e) => Service {
            up: false,
            latency_ms: None,
            code: None,
            message: e.clone(),
            components: vec![],
            issues: vec![],
            models_status: String::new(),
        },
    }
}

/// Upper bound for how old the last probe of a family may be before its state
/// is considered unknown.
const PROBE_STALE_SECS: i64 = 45 * 60;

/// The published `uptime_series` is the cumulative 30-day uptime sampled over
/// time. Each step therefore reveals the latest probe: a success nudges it up
/// by about `(100 - U) / n`, a failure drops it by about `U / n`. Steps that
/// fall by more than half the failure drop count as failed probes.
fn recent_outcomes(series: &[f64], uptime: f64, samples: f64) -> Vec<bool> {
    if series.len() < 2 || samples < 1.0 {
        return vec![];
    }
    let fail_drop = (uptime / samples).max(0.004);
    series
        .windows(2)
        .map(|w| w[1] - w[0] > -fail_drop * 0.5)
        .collect()
}

fn family_state(recent: &[bool], last_probe_at: Option<i64>, now: i64) -> &'static str {
    if last_probe_at.is_some_and(|t| now - t > PROBE_STALE_SECS) {
        return "stale";
    }
    if recent.is_empty() {
        return "unknown";
    }
    let tail = &recent[recent.len().saturating_sub(6)..];
    let last3 = &recent[recent.len().saturating_sub(3)..];
    if last3.len() == 3 && last3.iter().all(|ok| !ok) {
        "down"
    } else if tail.iter().any(|ok| !ok) {
        "degraded"
    } else {
        "ok"
    }
}

/// Short family name: "DeepSeek upstream" -> "DeepSeek".
fn short_name(name: &str) -> &str {
    name.strip_suffix(" upstream").unwrap_or(name)
}

/// Summarises family problems into the service message and `issues`.
fn apply_family_issues(service: &mut Service) {
    service.issues = service
        .components
        .iter()
        .filter_map(|c| match c.state.as_str() {
            "down" => Some(format!("{}: сбой", short_name(&c.name))),
            "degraded" => Some(format!("{}: перебои", short_name(&c.name))),
            _ => None,
        })
        .collect();
    let all_down = !service.components.is_empty()
        && service.components.iter().all(|c| c.state == "down");
    if service.up && !service.issues.is_empty() {
        let names: Vec<_> = service
            .components
            .iter()
            .filter(|c| c.state == "down" || c.state == "degraded")
            .map(|c| short_name(&c.name))
            .collect();
        service.message = if all_down {
            "Все модели недоступны".to_owned()
        } else {
            format!("{} · перебои: {}", service.message, names.join(", "))
        };
    }
}

fn parse_families(v: &Value, now: i64) -> Vec<Component> {
    let Some(list) = v.get("families").and_then(Value::as_array) else {
        return vec![];
    };
    list.iter()
        .filter_map(|f| {
            let name = f
                .get("display_name")
                .or_else(|| f.get("id"))
                .and_then(Value::as_str)?
                .to_owned();
            let uptime = num(f.get("uptime_percent"))?.clamp(0.0, 100.0);
            let series: Vec<f64> = f
                .get("uptime_series")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
                .unwrap_or_default();
            let samples = num(f.get("eligible_samples")).unwrap_or(0.0);
            let recent = recent_outcomes(&series, uptime, samples);
            let last_probe_at = f.get("last_probe_at").and_then(parse_time);
            let state = family_state(&recent, last_probe_at, now).to_owned();
            Some(Component {
                name,
                uptime,
                series,
                last_probe_at,
                state,
                recent,
            })
        })
        .collect()
}

const AMOUNT_KEYS: &[&str] = &[
    "balance_usd",
    "total_available",
    "available_usd",
    "remaining_usd",
    "balance",
    "remaining",
    "credits",
];

/// Breadth-first search for the first of `keys` (in priority order) in `v`,
/// descending into nested objects up to `depth` levels.
fn find_val<'a>(v: &'a Value, keys: &[&str], depth: usize) -> Option<&'a Value> {
    let mut level = vec![v];
    for _ in 0..=depth {
        for key in keys {
            if let Some(found) = level.iter().find_map(|o| o.get(*key).filter(|x| !x.is_null())) {
                return Some(found);
            }
        }
        level = level
            .iter()
            .filter_map(|o| o.as_object())
            .flat_map(|o| o.values().filter(|x| x.is_object()))
            .collect();
        if level.is_empty() {
            break;
        }
    }
    None
}

fn find_num(v: &Value, keys: &[&str], depth: usize) -> Option<f64> {
    let mut level = vec![v];
    for _ in 0..=depth {
        for key in keys {
            if let Some(n) = level.iter().find_map(|o| num(o.get(*key))) {
                return Some(n);
            }
        }
        level = level
            .iter()
            .filter_map(|o| o.as_object())
            .flat_map(|o| o.values().filter(|x| x.is_object()))
            .collect();
        if level.is_empty() {
            break;
        }
    }
    None
}

/// Keeps the last successfully read balance in the account metadata so it can
/// be shown (marked as stale) while the provider's endpoint is unavailable.
fn remember_balance(acc: &mut Account, amount: f64) {
    let changed = acc
        .meta
        .get("last_balance")
        .and_then(Value::as_f64)
        .map_or(true, |old| (old - amount).abs() > 1e-9);
    let at = acc.meta.get("last_balance_at").and_then(Value::as_i64).unwrap_or(0);
    // write at most every 30 minutes when the value is unchanged
    if changed || crate::util::now() - at > 1800 {
        acc.set_meta("last_balance", amount);
        acc.set_meta("last_balance_at", crate::util::now());
    }
}

fn restore_last_balance(acc: &Account, snap: &mut Snapshot) {
    let Some(amount) = acc.meta.get("last_balance").and_then(Value::as_f64) else {
        return;
    };
    snap.balance = Some(Balance {
        amount,
        currency: "USD".into(),
        as_of: acc.meta.get("last_balance_at").and_then(Value::as_i64),
        stale: true,
        ..Default::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> Account {
        Account {
            id: "t".into(),
            kind: crate::model::Kind::OneProvider,
            label: "t".into(),
            email: None,
            source: crate::model::Source::ApiKey,
            meta: Default::default(),
            created_at: 0,
            secret: Default::default(),
        }
    }

    #[test]
    fn finds_documented_and_nested_amounts() {
        let documented = json!({"object": "balance", "balance_usd": 54.77, "is_active": true});
        assert_eq!(find_num(&documented, AMOUNT_KEYS, 3), Some(54.77));

        let credit_summary = json!({"object": "credit_summary", "total_available": "12.5"});
        assert_eq!(find_num(&credit_summary, AMOUNT_KEYS, 3), Some(12.5));

        let nested = json!({"data": {"key": {"balance": 3.0, "expires_at": "2026-12-01T00:00:00Z"}}});
        assert_eq!(find_num(&nested, AMOUNT_KEYS, 3), Some(3.0));
        assert!(find_val(&nested, &["expires_at"], 3).and_then(parse_time).is_some());

        assert_eq!(find_num(&json!({"input_tokens": 10}), AMOUNT_KEYS, 3), None);
    }

    #[test]
    fn parses_usage_statistics() {
        let today = chrono::Local::now().date_naive();
        let day = |back: i64| (today - chrono::Duration::days(back)).format("%Y-%m-%d").to_string();
        let v = json!({
            "daily_usage": [
                {"date": day(20), "requests": 5, "cost": 1.0, "actual_cost": 2.0},
                {"date": day(3), "requests": 10, "cost": 1.0, "actual_cost": 4.0},
                {"date": day(0), "requests": 1, "cost": 0.1, "actual_cost": 0.2}
            ],
            "usage": {"today": {"requests": 31, "cost": 0.07, "actual_cost": 0.38}},
            "model_stats": [
                {"model": "cheap", "requests": 1, "actual_cost": 1.0},
                {"model": "pricey", "requests": 9, "actual_cost": 99.0},
                {"model": "mid", "requests": 3, "actual_cost": 10.0},
                {"model": "tiny", "requests": 3, "actual_cost": 0.1}
            ],
            "quota": {"limit": 100.0, "used": 40.0, "remaining": 60.0, "unit": "USD"},
            "unit": "USD"
        });
        let s = parse_usage(&v);
        assert!((s.today - 0.38).abs() < 1e-9, "today block wins over the daily list");
        assert_eq!(s.today_requests, 31);
        assert!((s.week - 4.38).abs() < 1e-9);
        assert!((s.month - 6.38).abs() < 1e-9);
        assert_eq!(s.daily.len(), 14);
        assert_eq!(s.daily.last().unwrap().date, day(0));
        assert_eq!(s.daily[13 - 3].requests, 10);
        assert_eq!(s.top_models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["pricey", "mid", "cheap"]);
        assert_eq!(s.quota_remaining, Some(60.0));
    }

    fn family(id: &str, uptime: f64, n: u64, series: &[f64], probe_age: i64, now: i64) -> Value {
        let t = chrono::DateTime::from_timestamp(now - probe_age, 0).unwrap().to_rfc3339();
        json!({"id": id, "display_name": format!("{id} upstream"), "uptime_percent": uptime,
               "eligible_samples": n, "ignored_4xx": 0, "last_probe_at": t, "uptime_series": series})
    }

    #[test]
    fn family_state_from_cumulative_series() {
        let now = 1_800_000_000;
        // shapes taken from the live endpoint during an incident
        let falling = [94.35, 94.35, 94.31, 94.27, 94.27, 94.27, 94.28, 94.24, 94.24, 94.2, 94.2,
            94.16, 94.16, 94.12, 94.12, 94.13, 94.13, 94.13, 94.13, 94.09, 94.09, 94.05, 94.01,
            93.97, 93.93];
        let steady = [92.77, 92.78, 92.78, 92.79, 92.79, 92.8, 92.8, 92.81, 92.81];
        let one_blip = [94.7, 94.71, 94.71, 94.71, 94.72, 94.72, 94.68];
        let v = json!({"families": [
            family("deepseek", 93.93, 2207, &falling, 300, now),
            family("claude", 92.81, 4477, &steady, 300, now),
            family("glm", 94.72, 2190, &one_blip, 300, now),
            family("old", 99.0, 1000, &steady, 3 * 3600, now),
        ]});
        let comps = parse_families(&v, now);
        let state = |id: &str| comps.iter().find(|c| c.name.starts_with(id)).unwrap().state.clone();
        assert_eq!(state("deepseek"), "down", "three failed probes in a row");
        assert_eq!(state("claude"), "ok");
        assert_eq!(state("glm"), "degraded", "a recent failed probe");
        assert_eq!(state("old"), "stale");
        let ds = comps.iter().find(|c| c.name.starts_with("deepseek")).unwrap();
        assert_eq!(ds.recent.len(), falling.len() - 1);
        assert!(!ds.recent[1] && ds.recent[5], "decline = failure, rise = success");

        let mut svc = Service { up: true, message: "API работает".into(), components: comps, ..Default::default() };
        apply_family_issues(&mut svc);
        assert_eq!(svc.issues, ["deepseek: сбой", "glm: перебои"]);
        assert_eq!(svc.message, "API работает · перебои: deepseek, glm");
    }

    #[test]
    fn empty_family_list_has_no_components() {
        // what the endpoint returns while the provider's observations are off
        let v = json!({"families": [], "generated_at": "2026-10-03T01:39:55Z", "schema_version": 1});
        assert!(parse_families(&v, 1_800_000_000).is_empty());
    }

    #[test]
    fn keeps_last_known_balance_when_source_fails() {
        let mut acc = account();
        let mut snap = Snapshot::for_account(&acc);
        restore_last_balance(&acc, &mut snap);
        assert!(snap.balance.is_none(), "nothing to restore yet");

        remember_balance(&mut acc, 54.77);
        let mut snap = Snapshot::for_account(&acc);
        restore_last_balance(&acc, &mut snap);
        let b = snap.balance.expect("restored");
        assert!(b.stale);
        assert_eq!(b.amount, 54.77);
        assert!(b.as_of.is_some());
    }
}
