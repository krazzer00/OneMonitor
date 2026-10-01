//! OneProvider gateway: liveness probe, public model observations and key balance.
//! Docs: https://oneprovider.dev/docs/llms.txt (§1, §10.1) and https://oneprovider.dev/status

use std::sync::atomic::{AtomicUsize, Ordering};

use reqwest::Client;
use serde_json::Value;

use super::{send, Resp};
use crate::model::{Account, Balance, Component, Service, Snapshot};
use crate::util::{num, parse_time, Res};

const BASE: &str = "https://api.oneprovider.dev";

/// Where the key balance can be read. The documented endpoint has moved between
/// hosts and at times disappeared (404 everywhere), so try every known location
/// and remember the one that answered.
const BALANCE_URLS: [&str; 3] = [
    "https://api.oneprovider.dev/v1/dashboard/balance",
    "https://dashboard.oneprovider.dev/v1/dashboard/balance",
    "https://api.oneprovider.dev/v1/usage",
];
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
    let (probe, families, balance) = tokio::join!(probe, families, balance);

    let mut service = service_from_probe(&probe, key.is_some());
    if let Ok(resp) = &families {
        if let Ok(v) = resp.json() {
            service.components = parse_families(&v);
        }
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
    Ok(())
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
            }
        }
        Err(e) => Service {
            up: false,
            latency_ms: None,
            code: None,
            message: e.clone(),
            components: vec![],
        },
    }
}

fn parse_families(v: &Value) -> Vec<Component> {
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
            let series = f
                .get("uptime_series")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
                .unwrap_or_default();
            Some(Component {
                name,
                uptime,
                series,
                last_probe_at: f.get("last_probe_at").and_then(parse_time),
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
