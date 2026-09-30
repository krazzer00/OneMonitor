//! OpenRouter: availability probe of the API plus credits / key limits.
//! https://openrouter.ai/docs/api-reference/get-current-api-key , /credits

use reqwest::Client;
use serde_json::Value;

use super::send;
use crate::model::{Account, Balance, Service, Snapshot};
use crate::util::{api_error, fmt_usd, num, Res};

const BASE: &str = "https://openrouter.ai/api/v1";

pub async fn fetch(http: &Client, acc: &Account, snap: &mut Snapshot) -> Res<()> {
    snap.link = Some("https://openrouter.ai/settings/credits".into());
    let key = acc
        .secret
        .api_key
        .clone()
        .filter(|k| !k.trim().is_empty());

    let probe = {
        let mut rb = http.get(format!("{BASE}/key"));
        if let Some(k) = &key {
            rb = rb.bearer_auth(k.trim());
        }
        send(rb)
    };
    let credits = async {
        match &key {
            Some(k) => Some(send(http.get(format!("{BASE}/credits")).bearer_auth(k.trim())).await),
            None => None,
        }
    };
    let (probe, credits) = tokio::join!(probe, credits);

    let key_data = match &probe {
        Ok(r) => {
            snap.service = Some(Service {
                up: r.status < 500,
                latency_ms: Some(r.ms),
                code: Some(r.status),
                message: match r.status {
                    200..=299 => "API работает".into(),
                    401 if key.is_none() => "API доступен".into(),
                    401 => "API доступен, ключ отклонён".into(),
                    s if s < 500 => format!("API доступен (HTTP {s})"),
                    s => format!("Сбой API (HTTP {s})"),
                },
                components: vec![],
            });
            if r.ok() {
                r.json().ok().and_then(|v| v.get("data").cloned())
            } else {
                None
            }
        }
        Err(e) => {
            snap.service = Some(Service {
                up: false,
                message: e.clone(),
                ..Default::default()
            });
            None
        }
    };

    let Some(_) = key else {
        snap.note("Режим", "только статус (без ключа)");
        return Ok(());
    };
    if matches!(&probe, Ok(r) if r.status == 401) {
        return Err("API-ключ не принят (401). Проверьте ключ.".into());
    }

    if let Some(d) = &key_data {
        if d.get("is_free_tier").and_then(Value::as_bool) == Some(true) {
            snap.plan = Some("Free tier".into());
        }
        if let Some(l) = d.get("label").and_then(Value::as_str) {
            if !l.is_empty() {
                snap.note("Ключ", l);
            }
        }
        for (field, label) in [
            ("usage_daily", "Расход сегодня"),
            ("usage_weekly", "Расход за неделю"),
            ("usage_monthly", "Расход за месяц"),
        ] {
            if let Some(v) = num(d.get(field)) {
                snap.note(label, fmt_usd(v));
            }
        }
    }

    let mut balance = None;
    match credits {
        Some(Ok(r)) if r.ok() => {
            let v = r.json()?;
            let d = v.get("data").unwrap_or(&v);
            if let (Some(total), Some(used)) = (num(d.get("total_credits")), num(d.get("total_usage"))) {
                balance = Some(Balance {
                    amount: total - used,
                    currency: "USD".into(),
                    total: Some(total),
                    used: Some(used),
                    ..Default::default()
                });
            }
        }
        Some(Ok(r)) => snap.warning = Some(format!("Кредиты недоступны: {}", api_error(r.status, &r.body))),
        Some(Err(e)) => snap.warning = Some(format!("Кредиты недоступны: {e}")),
        None => {}
    }
    // Fall back to the per-key limit when account credits are not readable.
    if balance.is_none() {
        if let Some(d) = &key_data {
            if let Some(rem) = num(d.get("limit_remaining")) {
                balance = Some(Balance {
                    amount: rem,
                    currency: "USD".into(),
                    total: num(d.get("limit")),
                    used: num(d.get("usage")),
                    ..Default::default()
                });
                snap.warning = None;
                snap.note("Источник", "лимит ключа");
            }
        }
    }
    snap.balance = balance;
    Ok(())
}
