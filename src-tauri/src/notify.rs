//! Windows notifications about state changes between two refreshes.

use std::collections::HashSet;

use crate::model::{Settings, Snapshot};
use crate::util::fmt_usd;

pub struct Toast {
    pub title: String,
    pub body: String,
}

/// Compares the previous and the new snapshot of one account and returns the
/// notifications to show. `fired` remembers one-shot alerts so they are not
/// repeated on every refresh.
pub fn diff(
    old: Option<&Snapshot>,
    new: &Snapshot,
    settings: &Settings,
    fired: &mut HashSet<String>,
) -> Vec<Toast> {
    let mut out = vec![];
    if !settings.notify {
        return out;
    }
    let name = match new.kind {
        Some(k) if new.label != k.title() && !new.label.starts_with(k.title()) => {
            format!("{} · {}", k.title(), new.label)
        }
        Some(k) => k.title().to_owned(),
        None => new.label.clone(),
    };

    // --- balance below the threshold (once per crossing)
    if settings.notify_balance {
        let key = format!("low:{}", new.id);
        match &new.balance {
            Some(b) if !b.stale && b.amount < settings.low_balance => {
                if fired.insert(key) {
                    out.push(Toast {
                        title: format!("{name}: мало средств"),
                        body: format!(
                            "Баланс {} — ниже порога {}",
                            fmt_usd(b.amount),
                            fmt_usd(settings.low_balance)
                        ),
                    });
                }
            }
            Some(b) if !b.stale => {
                fired.remove(&key);
            }
            _ => {}
        }
    }

    // --- subscription limits
    if settings.notify_limits {
        for l in &new.limits {
            let window = l.resets_at.unwrap_or(0);
            let key = format!("lim:{}:{}:{}", new.id, l.key, window);
            if l.used_percent >= settings.warn_percent && fired.insert(key) {
                let left = (100.0 - l.used_percent).max(0.0);
                out.push(Toast {
                    title: format!("{name}: {}", l.name),
                    body: if left <= 0.0 {
                        "Лимит исчерпан".to_owned()
                    } else {
                        format!("Осталось {left:.0}%")
                    },
                });
            }
            // reset: a limit that was high is low again
            if let Some(prev) = old.and_then(|o| o.limits.iter().find(|p| p.key == l.key)) {
                let was_high = prev.used_percent >= settings.warn_percent;
                let reset = prev.resets_at.is_some()
                    && l.resets_at.is_some()
                    && l.resets_at > prev.resets_at
                    && l.used_percent + 20.0 < prev.used_percent;
                if was_high && (reset || l.used_percent + 50.0 < prev.used_percent) {
                    out.push(Toast {
                        title: format!("{name}: лимит сброшен"),
                        body: format!("«{}» снова доступен — использовано {:.0}%", l.name, l.used_percent),
                    });
                }
            }
        }
    }

    // --- availability and errors (only transitions, never on the first check)
    if settings.notify_service {
        if let Some(old) = old.filter(|o| o.updated_at > 0) {
            let was_up = old.service.as_ref().map(|s| s.up);
            let is_up = new.service.as_ref().map(|s| s.up);
            match (was_up, is_up) {
                (Some(true), Some(false)) => out.push(Toast {
                    title: format!("{name}: API недоступен"),
                    body: new
                        .service
                        .as_ref()
                        .map(|s| s.message.clone())
                        .unwrap_or_default(),
                }),
                (Some(false), Some(true)) => out.push(Toast {
                    title: format!("{name}: API снова работает"),
                    body: "Доступность восстановлена".into(),
                }),
                _ => {}
            }
            if old.error.is_none() && new.service.as_ref().map_or(true, |s| s.up) {
                if let Some(e) = &new.error {
                    out.push(Toast {
                        title: format!("{name}: ошибка"),
                        body: e.clone(),
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Balance, Kind, Limit, Service};

    fn snap() -> Snapshot {
        Snapshot {
            id: "a".into(),
            kind: Some(Kind::Claude),
            label: "Claude".into(),
            updated_at: 1,
            ..Default::default()
        }
    }

    #[test]
    fn low_balance_fires_once_per_crossing() {
        let s = Settings::default();
        let mut fired = HashSet::new();
        let mut low = snap();
        low.balance = Some(Balance { amount: 0.5, ..Default::default() });
        assert_eq!(diff(None, &low, &s, &mut fired).len(), 1);
        assert_eq!(diff(Some(&low), &low, &s, &mut fired).len(), 0, "no repeat");
        let mut ok = snap();
        ok.balance = Some(Balance { amount: 10.0, ..Default::default() });
        diff(Some(&low), &ok, &s, &mut fired);
        assert_eq!(diff(Some(&ok), &low, &s, &mut fired).len(), 1, "fires again after recovery");
    }

    #[test]
    fn limit_high_then_reset() {
        let s = Settings::default();
        let mut fired = HashSet::new();
        let mut high = snap();
        high.limits = vec![Limit { key: "five_hour".into(), name: "5 ч".into(), used_percent: 90.0, resets_at: Some(100), ..Default::default() }];
        assert_eq!(diff(None, &high, &s, &mut fired).len(), 1);
        let mut reset = snap();
        reset.limits = vec![Limit { key: "five_hour".into(), name: "5 ч".into(), used_percent: 2.0, resets_at: Some(200), ..Default::default() }];
        let t = diff(Some(&high), &reset, &s, &mut fired);
        assert_eq!(t.len(), 1);
        assert!(t[0].title.contains("сброшен"));
    }

    #[test]
    fn service_transitions_only() {
        let s = Settings::default();
        let mut fired = HashSet::new();
        let mut up = snap();
        up.service = Some(Service { up: true, ..Default::default() });
        let mut down = snap();
        down.service = Some(Service { up: false, message: "503".into(), ..Default::default() });
        assert!(diff(None, &down, &s, &mut fired).is_empty(), "first check is silent");
        assert_eq!(diff(Some(&up), &down, &s, &mut fired).len(), 1);
        assert!(diff(Some(&down), &down, &s, &mut fired).is_empty());
        assert_eq!(diff(Some(&down), &up, &s, &mut fired).len(), 1);
    }

    #[test]
    fn master_switch_silences_everything() {
        let s = Settings { notify: false, ..Default::default() };
        let mut low = snap();
        low.balance = Some(Balance { amount: 0.1, ..Default::default() });
        assert!(diff(None, &low, &s, &mut HashSet::new()).is_empty());
    }
}
