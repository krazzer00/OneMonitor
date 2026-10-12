//! OpenCode free models (Zen). OpenCode publishes neither the free-tier limits
//! nor an endpoint with the remaining quota, and the free tier only works from
//! inside OpenCode. So the usage is read from the local OpenCode history
//! (`~/.local/share/opencode/opencode*.db`): requests to free models and the
//! "free limit reached" answers with their Retry-After.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use reqwest::Client;
use serde_json::Value;

use super::send;
use crate::model::{Account, FreeModel, FreeUsage, Service, Snapshot};
use crate::util::{now, Res};

const MODELS_URL: &str = "https://opencode.ai/zen/v1/models";
const HISTORY_SECS: i64 = 7 * 86_400;
/// Without a Retry-After a limit error counts as active for this long, unless
/// a successful answer from the same model follows.
const LIMIT_ASSUMED_SECS: i64 = 5 * 3600;

/// Free Zen models: "*-free" plus the free stealth model.
pub fn is_free(id: &str) -> bool {
    id.ends_with("-free") || id == "big-pickle"
}

pub async fn fetch(http: &Client, _acc: &mut Account, snap: &mut Snapshot) -> Res<()> {
    snap.link = Some("https://opencode.ai/zen".into());
    let probe = send(http.get(MODELS_URL)).await;
    let mut available = vec![];
    snap.service = Some(match &probe {
        Ok(r) => {
            if let Ok(v) = r.json() {
                available = v
                    .get("data")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m.get("id").and_then(Value::as_str))
                    .filter(|id| is_free(id))
                    .map(str::to_owned)
                    .collect();
            }
            Service {
                up: r.status < 500,
                latency_ms: Some(r.ms),
                code: Some(r.status),
                message: if r.ok() {
                    format!("Zen работает · бесплатных моделей: {}", available.len())
                } else {
                    format!("Zen: HTTP {}", r.status)
                },
                ..Default::default()
            }
        }
        Err(e) => Service {
            up: false,
            message: e.clone(),
            ..Default::default()
        },
    });

    let dbs = db_files();
    let t = now();
    let (events, sources) = tokio::task::spawn_blocking(move || read_history(&dbs, t - HISTORY_SECS))
        .await
        .map_err(|e| e.to_string())?;
    let free = summarize(&events, &available, t);

    if sources.is_empty() {
        snap.note(
            "История OpenCode",
            "не найдена — запустите OpenCode хотя бы раз (~/.local/share/opencode)",
        );
    }
    let limited: Vec<_> = free.models.iter().filter(|m| m.limited).collect();
    if let Some(m) = limited.first() {
        let until = m
            .limited_until
            .map(|u| format!(", сброс {}", crate::util::fmt_local(u)))
            .unwrap_or_default();
        snap.warning = Some(if limited.len() == 1 {
            format!("Исчерпан бесплатный лимит {}{until}", m.id)
        } else {
            format!("Исчерпан бесплатный лимит у {} моделей{until}", limited.len())
        });
    }
    snap.free = Some(FreeUsage { sources, ..free });
    Ok(())
}

/// Where OpenCode keeps its database: `$XDG_DATA_HOME/opencode` or
/// `~/.local/share/opencode` (also on Windows); channel builds use
/// `opencode-<channel>.db`.
fn db_files() -> Vec<PathBuf> {
    let mut dirs = vec![];
    if let Some(x) = std::env::var_os("XDG_DATA_HOME").filter(|x| !x.is_empty()) {
        dirs.push(PathBuf::from(x).join("opencode"));
    }
    if let Some(h) = dirs::home_dir() {
        dirs.push(h.join(".local").join("share").join("opencode"));
    }
    let mut out = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("opencode") && name.ends_with(".db") && !out.contains(&e.path()) {
                out.push(e.path());
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Ok,
    /// Free limit reached; when it resets, if the provider said so.
    Limit(Option<i64>),
}

#[derive(Debug, Clone)]
pub struct Event {
    pub model: String,
    pub at: i64,
    pub outcome: Outcome,
}

fn secs(t: i64) -> i64 {
    if t > 100_000_000_000 {
        t / 1000
    } else {
        t
    }
}

/// Reads assistant messages of the last days from every OpenCode database.
/// Returns the events and the files that could be read.
fn read_history(dbs: &[PathBuf], since: i64) -> (Vec<Event>, Vec<String>) {
    use rusqlite::{Connection, OpenFlags};
    let mut events = vec![];
    let mut sources = vec![];
    for path in dbs {
        let Ok(conn) = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            continue;
        };
        let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
        let mut read_any = false;
        // `message` holds OpenCode 1.x history, `session_message` the newer one
        for table in ["message", "session_message"] {
            let exists: bool = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            if !exists {
                continue;
            }
            // the newer table keeps the entry type in its own column
            let kind = if table == "session_message" { "type" } else { "NULL" };
            // time_created is in milliseconds; compare in both units to be safe
            let sql = format!(
                "SELECT time_created, data, {kind} FROM {table} WHERE time_created >= ?1 OR (time_created < 100000000000 AND time_created >= ?2)"
            );
            let Ok(mut stmt) = conn.prepare(&sql) else { continue };
            let rows = stmt.query_map([since * 1000, since], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
            });
            let Ok(rows) = rows else { continue };
            read_any = true;
            for (t, data, kind) in rows.flatten() {
                // cheap prefilter before parsing JSON
                if !data.contains("opencode") {
                    continue;
                }
                if let Ok(mut v) = serde_json::from_str::<Value>(&data) {
                    if let (Some(k), Some(o)) = (kind, v.as_object_mut()) {
                        o.entry("type").or_insert(Value::String(k));
                    }
                    if let Some(e) = parse_event(&v, secs(t)) {
                        events.push(e);
                    }
                }
            }
        }
        if read_any {
            sources.push(path.display().to_string());
        }
    }
    (events, sources)
}

fn str_at<'a>(v: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths.iter().find_map(|p| v.pointer(p).and_then(Value::as_str))
}

/// One assistant answer from a free OpenCode model, or None.
pub fn parse_event(v: &Value, row_time: i64) -> Option<Event> {
    let role = str_at(v, &["/role", "/type"])?;
    if role != "assistant" {
        return None;
    }
    let provider = str_at(v, &["/providerID", "/model/providerID", "/provider"])?;
    let model = str_at(v, &["/modelID", "/model/modelID", "/model/id"])?;
    if provider != "opencode" || !is_free(model) {
        return None;
    }
    let at = v
        .pointer("/time/created")
        .and_then(Value::as_i64)
        .map(secs)
        .unwrap_or(row_time);
    let outcome = match v.get("error").filter(|e| !e.is_null()) {
        Some(err) if err.to_string().contains("FreeUsageLimitError") => {
            Outcome::Limit(retry_after(err).map(|s| at + s))
        }
        _ => Outcome::Ok,
    };
    Some(Event {
        model: model.to_owned(),
        at,
        outcome,
    })
}

/// Seconds from a Retry-After header stored with the API error.
fn retry_after(err: &Value) -> Option<i64> {
    let headers = err
        .pointer("/data/responseHeaders")
        .or_else(|| err.get("responseHeaders"))?
        .as_object()?;
    let raw = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, v)| v.as_str().map(str::to_owned).or_else(|| v.as_i64().map(|n| n.to_string())))?;
    let raw = raw.trim();
    if let Ok(n) = raw.parse::<f64>() {
        return Some(n.max(0.0) as i64);
    }
    chrono::DateTime::parse_from_rfc2822(raw)
        .ok()
        .map(|d| (d.timestamp() - now()).max(0))
}

/// Per-model counters: requests in 5 h / 24 h / 7 days, the latest limit error
/// and how many requests preceded it.
pub fn summarize(events: &[Event], available: &[String], now: i64) -> FreeUsage {
    let mut by_model: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for e in events {
        by_model.entry(e.model.as_str()).or_default().push(e);
    }
    let avail: HashSet<&str> = available.iter().map(String::as_str).collect();
    let mut models: Vec<FreeModel> = by_model
        .into_iter()
        .map(|(id, mut list)| {
            list.sort_by_key(|e| e.at);
            let ok: Vec<i64> = list.iter().filter(|e| e.outcome == Outcome::Ok).map(|e| e.at).collect();
            let count = |from: i64, to: i64| ok.iter().filter(|t| **t >= from && **t <= to).count() as u64;
            let last_ok = ok.last().copied();
            let last_limit = list.iter().rev().find_map(|e| match e.outcome {
                Outcome::Limit(until) => Some((e.at, until)),
                Outcome::Ok => None,
            });
            let (limited, limited_until) = match last_limit {
                Some((at, until)) if last_ok.map_or(true, |ok| ok < at) => match until {
                    Some(u) => (u > now, Some(u).filter(|u| *u > now)),
                    None => (now - at < LIMIT_ASSUMED_SECS, None),
                },
                _ => (false, None),
            };
            FreeModel {
                id: id.to_owned(),
                available: avail.contains(id),
                requests_5h: count(now - 5 * 3600, now),
                requests_24h: count(now - 86_400, now),
                requests_7d: ok.len() as u64,
                last_used_at: list.last().map(|e| e.at),
                limit_at: last_limit.map(|l| l.0),
                limited,
                limited_until,
                limit_after_5h: last_limit.map(|(at, _)| count(at - 5 * 3600, at)),
            }
        })
        .collect();
    for id in available {
        if !models.iter().any(|m| &m.id == id) {
            models.push(FreeModel {
                id: id.clone(),
                available: true,
                ..Default::default()
            });
        }
    }
    // limited first, then recently used, then the rest by name
    models.sort_by(|a, b| {
        b.limited
            .cmp(&a.limited)
            .then(b.last_used_at.cmp(&a.last_used_at))
            .then(a.id.cmp(&b.id))
    });
    FreeUsage {
        sources: vec![],
        models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn free_model_names() {
        assert!(is_free("big-pickle"));
        assert!(is_free("step-5-preview-free"));
        assert!(!is_free("claude-opus-5-5"));
    }

    #[test]
    fn parses_assistant_messages() {
        // OpenCode 1.x message.data
        let ok = json!({"id": "msg_1", "role": "assistant", "providerID": "opencode", "modelID": "big-pickle",
            "time": {"created": 1_800_000_000_000i64, "completed": 1_800_000_005_000i64}, "cost": 0, "finish": "stop"});
        let e = parse_event(&ok, 0).unwrap();
        assert_eq!((e.model.as_str(), e.at, e.outcome.clone()), ("big-pickle", 1_800_000_000, Outcome::Ok));

        let limit = json!({"role": "assistant", "providerID": "opencode", "modelID": "space-bunny-free",
            "time": {"created": 1_800_000_100_000i64},
            "error": {"name": "APIError", "data": {"message": "Free limit reached", "statusCode": 429,
                "isRetryable": false, "responseHeaders": {"Retry-After": "3600"},
                "responseBody": "{\"type\":\"error\",\"error\":{\"type\":\"FreeUsageLimitError\"}}"}}});
        let e = parse_event(&limit, 0).unwrap();
        assert_eq!(e.outcome, Outcome::Limit(Some(1_800_000_100 + 3600)));

        // newer layout: model object, type column style
        let v2 = json!({"type": "assistant", "model": {"providerID": "opencode", "modelID": "jev-1.13-free"}});
        assert_eq!(parse_event(&v2, 42).unwrap().at, 42);

        let paid = json!({"role": "assistant", "providerID": "opencode", "modelID": "claude-opus-5-5"});
        assert!(parse_event(&paid, 0).is_none());
        let user = json!({"role": "user", "model": {"providerID": "opencode", "modelID": "big-pickle"}});
        assert!(parse_event(&user, 0).is_none());
    }

    fn ev(model: &str, at: i64, outcome: Outcome) -> Event {
        Event { model: model.into(), at, outcome }
    }

    #[test]
    fn summary_counts_and_limits() {
        let now = 1_800_000_000;
        let h = 3600;
        let events = vec![
            ev("big-pickle", now - 30 * h, Outcome::Ok),
            ev("big-pickle", now - 4 * h, Outcome::Ok),
            ev("big-pickle", now - 2 * h, Outcome::Ok),
            ev("big-pickle", now - h, Outcome::Limit(Some(now + h))),
            ev("space-bunny-free", now - 3 * h, Outcome::Limit(None)),
            ev("space-bunny-free", now - 2 * h, Outcome::Ok),
            ev("old-free", now - 6 * h, Outcome::Limit(None)),
        ];
        let available = vec!["big-pickle".to_owned(), "step-5-preview-free".to_owned()];
        let s = summarize(&events, &available, now);
        let m = |id: &str| s.models.iter().find(|m| m.id == id).unwrap().clone();

        let bp = m("big-pickle");
        assert!(bp.limited && bp.available);
        assert_eq!(bp.limited_until, Some(now + h));
        assert_eq!((bp.requests_5h, bp.requests_24h, bp.requests_7d), (2, 2, 3));
        assert_eq!(bp.limit_after_5h, Some(2));

        assert!(!m("space-bunny-free").limited, "a successful answer after the error lifts it");
        assert!(!m("old-free").limited, "an old limit without Retry-After expires");
        assert!(!m("old-free").available);
        assert_eq!(m("step-5-preview-free").requests_7d, 0);
        assert_eq!(s.models[0].id, "big-pickle", "limited models first");
    }

    #[test]
    fn reads_a_real_sqlite_history() {
        let dir = std::env::temp_dir().join(format!("om-oc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id text PRIMARY KEY, session_id text NOT NULL, time_created integer NOT NULL,
               time_updated integer NOT NULL, data text NOT NULL);",
        )
        .unwrap();
        let t = now();
        let rows = [
            (t - 60, json!({"role": "assistant", "providerID": "opencode", "modelID": "big-pickle"})),
            (t - 10 * 86_400, json!({"role": "assistant", "providerID": "opencode", "modelID": "big-pickle"})),
            (t - 60, json!({"role": "assistant", "providerID": "anthropic", "modelID": "claude-opus-5-5"})),
        ];
        for (i, (at, data)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO message VALUES (?1, 's', ?2, ?2, ?3)",
                rusqlite::params![format!("m{i}"), at * 1000, data.to_string()],
            )
            .unwrap();
        }
        conn.execute_batch(
            "CREATE TABLE session_message (id text PRIMARY KEY, session_id text NOT NULL, type text NOT NULL,
               seq integer NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_message VALUES ('e1', 's', 'assistant', 1, ?1, ?1, ?2)",
            rusqlite::params![t * 1000, json!({"model": {"providerID": "opencode", "modelID": "space-bunny-free"}}).to_string()],
        )
        .unwrap();
        drop(conn);
        let (events, sources) = read_history(&[path], t - HISTORY_SECS);
        assert_eq!(sources.len(), 1);
        let models: Vec<_> = events.iter().map(|e| e.model.as_str()).collect();
        assert_eq!(models, ["big-pickle", "space-bunny-free"], "old and paid messages are skipped");
        let _ = std::fs::remove_dir_all(dir);
    }
}
