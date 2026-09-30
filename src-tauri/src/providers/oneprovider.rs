//! OneProvider gateway: liveness probe, public model observations and key balance.
//! Docs: https://oneprovider.dev/docs/llms.txt (§1, §10.1) and https://oneprovider.dev/status

use reqwest::Client;
use serde_json::Value;

use super::{send, Resp};
use crate::model::{Account, Balance, Component, Service, Snapshot};
use crate::util::{num, parse_time, Res};

const BASE: &str = "https://api.oneprovider.dev";

pub async fn fetch(http: &Client, acc: &Account, snap: &mut Snapshot) -> Res<()> {
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
            Some(k) => Some(
                send(
                    http.get(format!("{BASE}/v1/dashboard/balance"))
                        .bearer_auth(k.trim()),
                )
                .await,
            ),
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
            let expires_at = v.get("expires_at").and_then(parse_time);
            let active = v.get("is_active").and_then(Value::as_bool);
            snap.balance = Some(Balance {
                amount: num(v.get("balance_usd")).unwrap_or(0.0),
                currency: "USD".into(),
                expires_at,
                active,
                ..Default::default()
            });
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
        }
        Some(Ok(resp)) => {
            let msg = crate::util::api_error(resp.status, &resp.body);
            if service_up {
                snap.warning = Some(format!("Баланс недоступен: {msg}"));
            } else {
                return Err(msg);
            }
        }
        Some(Err(e)) => {
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
