//! NanoGPT: API liveness, the public status page, key balance, subscription
//! quotas and spending. Docs: https://docs.nano-gpt.com/llms.txt

use std::collections::BTreeMap;

use reqwest::Client;
use serde_json::Value;

use super::{add_forecast, remember_balance, restore_last_balance, send, spend_from_days, Resp};
use crate::model::{Account, Balance, Component, Limit, ModelCost, Service, Snapshot, Spend};
use crate::util::{api_error, fmt_local, num, parse_time, Res};

const BASE: &str = "https://api.nano-gpt.com/api";
/// Better Stack status page; `index.json` is its public machine-readable form.
const STATUS_URL: &str = "https://status.nano-gpt.com/index.json";
const STATUS_PAGE: &str = "https://status.nano-gpt.com";
/// Subscription quotas for a "Usage only" management token.
const MGMT_USAGE_URL: &str = "https://nano-gpt.com/api/management/v1/subscription/usage";
const MGMT_PREFIX: &str = "sk-nano-mgmt-";
/// Days of the status page history shown per service.
const HISTORY_DAYS: usize = 30;

pub async fn fetch(http: &Client, acc: &mut Account, snap: &mut Snapshot) -> Res<()> {
    snap.link = Some(STATUS_PAGE.into());
    let key = acc
        .secret
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned);
    // A management token can only read subscription quotas.
    let mgmt = key.as_deref().is_some_and(|k| k.starts_with(MGMT_PREFIX));
    let inference_key = key.as_deref().filter(|_| !mgmt);

    let probe = send(http.get(format!("{BASE}/v1/models")));
    let status = send(http.get(STATUS_URL).header("Cache-Control", "no-cache"));
    let balance = async {
        match inference_key {
            Some(k) => Some(send(http.post(format!("{BASE}/check-balance")).header("x-api-key", k)).await),
            None => None,
        }
    };
    let subscription = async {
        match (&key, mgmt) {
            (Some(k), true) => Some(send(http.get(MGMT_USAGE_URL).bearer_auth(k)).await),
            (Some(k), false) => {
                Some(send(http.get(format!("{BASE}/subscription/v1/usage")).bearer_auth(k)).await)
            }
            (None, _) => None,
        }
    };
    let usage = async {
        match inference_key {
            Some(k) => send(http.get(format!("{BASE}/v1/usage")).bearer_auth(k))
                .await
                .ok()
                .filter(Resp::ok)
                .and_then(|r| r.json().ok()),
            None => None,
        }
    };
    let (probe, status, balance, subscription, usage) =
        tokio::join!(probe, status, balance, subscription, usage);

    let mut service = service_from_probe(&probe);
    match status.as_ref().map(|r| r.json()) {
        Ok(Ok(v)) => apply_status_page(&mut service, &v, crate::util::now()),
        _ => service.models_status = "unavailable".into(),
    }
    if !service.issues.is_empty() {
        snap.warning = Some(format!("По данным статус-страницы: {}", service.issues.join(", ")));
    }
    let service_up = service.up;
    snap.service = Some(service);

    let Some(_) = key else {
        snap.note("Режим", "только статус (без ключа)");
        return Ok(());
    };
    if mgmt {
        snap.note("Режим", "только квоты подписки (management-токен)");
    }

    // ---- subscription quotas
    match subscription {
        Some(Ok(resp)) if resp.ok() => apply_subscription(snap, &resp.json()?),
        Some(Ok(resp)) if resp.status == 401 || resp.status == 403 => {
            return Err(key_error(&resp, mgmt));
        }
        Some(Ok(resp)) => {
            snap.note("Подписка", format!("нет данных ({})", api_error(resp.status, &resp.body)));
        }
        Some(Err(e)) if !service_up => return Err(e),
        Some(Err(e)) => snap.note("Подписка", format!("нет данных ({e})")),
        None => {}
    }

    // ---- balance and spending
    snap.spend = usage.as_ref().map(parse_usage);
    match balance {
        Some(Ok(resp)) if resp.ok() => {
            let v = resp.json()?;
            match num(v.get("usd_balance")) {
                Some(amount) => {
                    snap.balance = Some(Balance {
                        amount,
                        currency: "USD".into(),
                        ..Default::default()
                    });
                    remember_balance(acc, amount);
                    if let Some(nano) = num(v.get("nano_balance")).filter(|n| *n > 0.0) {
                        snap.note("Баланс в Nano", format!("{nano:.4} XNO"));
                    }
                }
                None => {
                    restore_last_balance(acc, snap);
                    snap.warning = Some("Неизвестный формат ответа баланса".into());
                }
            }
        }
        Some(Ok(resp)) if resp.status == 401 || resp.status == 403 => {
            return Err(key_error(&resp, false));
        }
        Some(Ok(resp)) => {
            restore_last_balance(acc, snap);
            let msg = format!("Баланс недоступен: {}", api_error(resp.status, &resp.body));
            if !service_up {
                return Err(msg);
            }
            snap.warning.get_or_insert(msg);
        }
        Some(Err(e)) => {
            restore_last_balance(acc, snap);
            if !service_up {
                return Err(e);
            }
            snap.warning.get_or_insert(format!("Баланс недоступен: {e}"));
        }
        None => {}
    }
    add_forecast(snap);
    Ok(())
}

fn key_error(resp: &Resp, mgmt: bool) -> String {
    let what = if mgmt { "Management-токен" } else { "API-ключ" };
    format!("{what} не принят: {}", api_error(resp.status, &resp.body))
}

fn service_from_probe(probe: &Res<Resp>) -> Service {
    match probe {
        Ok(r) => Service {
            up: r.status < 500,
            latency_ms: Some(r.ms),
            code: Some(r.status),
            message: match r.status {
                200..=299 => "API работает".to_owned(),
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

fn resource_state(status: &str) -> &'static str {
    match status {
        "operational" => "ok",
        "degraded" => "degraded",
        "downtime" => "down",
        "maintenance" => "maintenance",
        _ => "unknown",
    }
}

/// Reads the Better Stack status page: one component per monitored service
/// (with its daily history) plus open incidents.
fn apply_status_page(service: &mut Service, v: &Value, now: i64) {
    let included = v.get("included").and_then(Value::as_array);
    let of_type = |t: &'static str| {
        included
            .into_iter()
            .flatten()
            .filter(move |x| x.get("type").and_then(Value::as_str) == Some(t))
            .filter_map(|x| x.get("attributes"))
    };

    service.components = of_type("status_page_resource")
        .filter_map(|a| {
            let name = a.get("public_name").and_then(Value::as_str)?.to_owned();
            let state = resource_state(a.get("status").and_then(Value::as_str).unwrap_or(""));
            let history = a.get("status_history").and_then(Value::as_array);
            let recent: Vec<bool> = history
                .map(|h| {
                    h[h.len().saturating_sub(HISTORY_DAYS)..]
                        .iter()
                        .map(|d| {
                            !matches!(
                                d.get("status").and_then(Value::as_str),
                                Some("downtime" | "degraded")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Component {
                name,
                uptime: (num(a.get("availability")).unwrap_or(1.0) * 100.0).clamp(0.0, 100.0),
                state: state.to_owned(),
                recent,
                granularity: "days".into(),
                ..Default::default()
            })
        })
        .collect();
    service.models_status = if service.components.is_empty() { "empty" } else { "ok" }.into();

    let mut issues = vec![];
    let mut names = vec![];
    for c in &service.components {
        let what = match c.state.as_str() {
            "down" => "сбой",
            "degraded" => "перебои",
            _ => continue,
        };
        issues.push(format!("{}: {what}", c.name));
        names.push(c.name.clone());
    }
    for r in of_type("status_report") {
        let title = r.get("title").and_then(Value::as_str).unwrap_or("инцидент");
        let resolved = r.get("aggregate_state").and_then(Value::as_str) == Some("resolved")
            || r.get("ends_at").and_then(parse_time).is_some_and(|t| t <= now);
        let started = r.get("starts_at").and_then(parse_time).map_or(true, |t| t <= now);
        let maintenance = r.get("report_type").and_then(Value::as_str) == Some("maintenance");
        if !resolved && started && !maintenance {
            issues.push(format!("Инцидент: {title}"));
            names.push(title.to_owned());
        }
    }
    if service.up && !issues.is_empty() {
        service.message = format!("{} · перебои: {}", service.message, names.join(", "));
    }
    service.issues = issues;
}

const QUOTAS: [(&str, &str, Option<i64>, &str); 4] = [
    ("dailyInputTokens", "Входные токены · день", Some(86_400), "токенов"),
    ("weeklyInputTokens", "Входные токены · неделя", Some(604_800), "токенов"),
    ("dailyImages", "Изображения · день", Some(86_400), "изображений"),
    ("tokens", "Токены пробного периода", None, "токенов"),
];

/// "250 тыс.", "1.5 млн", "120".
fn compact(n: f64) -> String {
    let trim = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s
        }
    };
    if n >= 1e9 {
        format!("{} млрд", trim(format!("{:.1}", n / 1e9)))
    } else if n >= 1e6 {
        format!("{} млн", trim(format!("{:.1}", n / 1e6)))
    } else if n >= 1e4 {
        format!("{} тыс.", trim(format!("{:.0}", n / 1e3)))
    } else {
        format!("{n:.0}")
    }
}

fn apply_subscription(snap: &mut Snapshot, v: &Value) {
    let active = v.get("active").and_then(Value::as_bool).unwrap_or(false);
    let state = v.get("state").and_then(Value::as_str).unwrap_or("");
    if !active && state != "grace" {
        snap.note("Подписка", "нет — только оплата с баланса");
        return;
    }
    let trial = v.get("usageUnits").and_then(Value::as_str) == Some("tokens");
    snap.plan = Some(if trial { "Пробный период" } else { "Подписка" }.into());
    if state == "grace" {
        snap.warning = Some("Подписка в льготном периоде — продлите оплату".into());
    }

    for (key, name, window, unit) in QUOTAS {
        let Some(q) = v.get(key).filter(|q| q.is_object()) else { continue };
        let degraded = q.get("degraded").and_then(Value::as_bool).unwrap_or(false);
        let Some(fraction) = num(q.get("percentUsed")).filter(|_| !degraded) else {
            // the provider asks to show "unknown", never a full allowance
            snap.note(name, "нет данных");
            continue;
        };
        let used = num(q.get("used"));
        let total = num(v.pointer(&format!("/limits/{key}")))
            .or_else(|| (key == "tokens").then(|| num(v.pointer("/tokenLimits/total"))).flatten())
            .or_else(|| Some(used? + num(q.get("remaining"))?));
        let detail = match (used, total) {
            (Some(u), Some(t)) => Some(format!("{} из {} {unit}", compact(u), compact(t))),
            _ => None,
        };
        snap.limits.push(Limit {
            key: key.into(),
            name: name.into(),
            used_percent: (fraction * 100.0).clamp(0.0, 100.0),
            resets_at: q.get("resetAt").and_then(parse_time),
            window_secs: window,
            detail,
            ..Default::default()
        });
    }

    if let Some(end) = v.pointer("/period/currentPeriodEnd").and_then(parse_time) {
        snap.note("Подписка до", fmt_local(end));
    }
    match v.pointer("/routing/recommendedMode").and_then(Value::as_str) {
        Some("subscription") => snap.note("Оплата запросов", "из подписки"),
        Some("paygo") => snap.note("Оплата запросов", "с баланса — квота подписки недоступна"),
        Some("unavailable") => {
            let reason = v
                .pointer("/routing/reason")
                .and_then(Value::as_str)
                .unwrap_or("квота исчерпана, оплата с баланса запрещена");
            snap.warning.get_or_insert(format!("Запросы сейчас не пройдут: {reason}"));
        }
        _ => {}
    }
}

/// `/v1/usage`: the key's last 30 UTC days grouped by day and model.
fn parse_usage(v: &Value) -> Spend {
    let cost = |x: &Value| num(x.get("netCostUsd")).or_else(|| num(x.get("costUsd"))).unwrap_or(0.0);
    let requests = |x: &Value| x.get("requests").and_then(Value::as_u64).unwrap_or(0);

    let mut by_day: BTreeMap<chrono::NaiveDate, (f64, u64)> = BTreeMap::new();
    for d in v.get("byDay").and_then(Value::as_array).into_iter().flatten() {
        let Some(date) = d
            .get("date")
            .and_then(Value::as_str)
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        else {
            continue;
        };
        let e = by_day.entry(date).or_default();
        e.0 += cost(d);
        e.1 += requests(d);
    }
    let mut top: Vec<ModelCost> = v
        .get("byModel")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ModelCost {
                name: m.get("model").and_then(Value::as_str)?.to_owned(),
                cost: cost(m),
                requests: requests(m),
            })
        })
        .collect();
    top.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    top.truncate(3);

    // the endpoint works in UTC days
    let mut spend = spend_from_days(&by_day, chrono::Utc::now().date_naive(), top);
    spend.top_period = "30 дней".into();
    spend
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subscription_quotas_become_limits() {
        // shape from https://docs.nano-gpt.com/api-reference/management-api
        let v = json!({
            "active": true, "state": "active",
            "limits": {"dailyInputTokens": 1000000, "weeklyInputTokens": 6000000, "dailyImages": null},
            "dailyInputTokens": {"used": 250000, "remaining": 750000, "percentUsed": 0.25, "resetAt": 1788652800000u64},
            "weeklyInputTokens": {"used": null, "remaining": null, "percentUsed": null, "resetAt": null, "degraded": true},
            "dailyImages": null,
            "period": {"currentPeriodEnd": "2026-10-05T00:00:00.000Z"},
            "routing": {"recommendedMode": "subscription", "subscriptionQuotaAvailable": true}
        });
        let mut snap = Snapshot::default();
        apply_subscription(&mut snap, &v);
        assert_eq!(snap.plan.as_deref(), Some("Подписка"));
        assert_eq!(snap.limits.len(), 1, "degraded and absent quotas are not shown as limits");
        let l = &snap.limits[0];
        assert_eq!(l.used_percent, 25.0);
        assert_eq!(l.resets_at, Some(1_788_652_800));
        assert_eq!(l.window_secs, Some(86_400));
        assert_eq!(l.detail.as_deref(), Some("250 тыс. из 1 млн токенов"));
        let note = |label: &str| snap.notes.iter().find(|n| n.label == label).map(|n| n.value.clone());
        assert_eq!(note("Входные токены · неделя").as_deref(), Some("нет данных"));
        assert!(note("Подписка до").is_some());
        assert_eq!(note("Оплата запросов").as_deref(), Some("из подписки"));
        assert!(snap.warning.is_none());
    }

    #[test]
    fn inactive_and_grace_subscriptions() {
        let mut snap = Snapshot::default();
        apply_subscription(&mut snap, &json!({"active": false, "state": "inactive", "limits": {}}));
        assert!(snap.limits.is_empty() && snap.plan.is_none());

        let mut snap = Snapshot::default();
        apply_subscription(&mut snap, &json!({
            "active": false, "state": "grace", "limits": {"dailyInputTokens": 100},
            "dailyInputTokens": {"used": 150, "remaining": 0, "percentUsed": 1.5, "resetAt": null},
            "routing": {"recommendedMode": "unavailable", "reason": "Daily quota exhausted"}
        }));
        assert!(snap.warning.as_deref().unwrap().contains("льготном"));
        assert_eq!(snap.limits[0].used_percent, 100.0, "overage is capped");
    }

    #[test]
    fn status_page_resources_and_incidents() {
        let day = |s: &str| json!({"day": "2026-10-01", "status": s, "downtime_duration": 0});
        let v = json!({
            "data": {"attributes": {"aggregate_state": "degraded"}},
            "included": [
                {"type": "status_page_section", "attributes": {"name": "Current status by service"}},
                {"type": "status_page_resource", "attributes": {"public_name": "nano-gpt.com", "status": "degraded",
                    "availability": 0.9987, "status_history": [day("operational"), day("downtime"), day("maintenance")]}},
                {"type": "status_page_resource", "attributes": {"public_name": "Zano deposits", "status": "operational",
                    "availability": 1.0, "status_history": [day("operational")]}},
                {"type": "status_report", "attributes": {"title": "Elevated errors", "aggregate_state": "degraded",
                    "starts_at": "2026-10-01T10:00:00Z", "ends_at": null}},
                {"type": "status_report", "attributes": {"title": "Old outage", "aggregate_state": "resolved",
                    "starts_at": "2026-09-01T10:00:00Z", "ends_at": "2026-09-01T11:00:00Z"}}
            ]
        });
        let mut svc = Service { up: true, message: "API работает".into(), ..Default::default() };
        apply_status_page(&mut svc, &v, 1_800_000_000);
        assert_eq!(svc.components.len(), 2);
        let site = &svc.components[0];
        assert_eq!(site.state, "degraded");
        assert!((site.uptime - 99.87).abs() < 1e-9);
        assert_eq!(site.recent, [true, false, true], "maintenance days are not failures");
        assert_eq!(svc.issues, ["nano-gpt.com: перебои", "Инцидент: Elevated errors"]);
        assert_eq!(svc.message, "API работает · перебои: nano-gpt.com, Elevated errors");
        assert_eq!(svc.models_status, "ok");
    }

    #[test]
    fn usage_by_utc_day_and_model() {
        let today = chrono::Utc::now().date_naive();
        let day = |back: i64| (today - chrono::Duration::days(back)).format("%Y-%m-%d").to_string();
        let v = json!({
            "object": "usage",
            "byDay": [
                {"date": day(0), "requests": 3, "costUsd": 1.0, "refundedUsd": 0.5, "netCostUsd": 0.5},
                {"date": day(5), "requests": 10, "costUsd": 2.0, "netCostUsd": 2.0},
                {"date": day(20), "requests": 1, "costUsd": 4.0, "netCostUsd": 4.0}
            ],
            "byModel": [
                {"model": "a", "requests": 1, "netCostUsd": 0.1},
                {"model": "b", "requests": 1, "netCostUsd": 5.0},
                {"model": "c", "requests": 1, "netCostUsd": 1.0},
                {"model": "d", "requests": 1, "netCostUsd": 0.4}
            ]
        });
        let s = parse_usage(&v);
        assert_eq!(s.today, 0.5);
        assert_eq!(s.today_requests, 3);
        assert_eq!(s.week, 2.5);
        assert_eq!(s.month, 6.5);
        assert_eq!(s.top_models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["b", "c", "d"]);
        assert_eq!(s.top_period, "30 дней");
    }

    #[test]
    fn compact_numbers() {
        assert_eq!(compact(120.0), "120");
        assert_eq!(compact(250_000.0), "250 тыс.");
        assert_eq!(compact(1_000_000.0), "1 млн");
        assert_eq!(compact(6_500_000.0), "6.5 млн");
    }
}
