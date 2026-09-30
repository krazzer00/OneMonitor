use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    OneProvider,
    OpenRouter,
    ChatGpt,
    Claude,
    Antigravity,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::OneProvider => "OneProvider",
            Kind::OpenRouter => "OpenRouter",
            Kind::ChatGpt => "ChatGPT",
            Kind::Claude => "Claude",
            Kind::Antigravity => "Antigravity",
        }
    }

    pub fn is_gateway(self) -> bool {
        matches!(self, Kind::OneProvider | Kind::OpenRouter)
    }
}

/// Where the credentials of an account come from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    ApiKey,
    /// Our own OAuth session, tokens are stored (encrypted) by the app.
    OAuth,
    /// Tokens are read from (and refreshed into) the official CLI credentials file.
    Cli,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Unix seconds, 0 when unknown.
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default)]
    pub id_token: Option<String>,
}

impl Tokens {
    pub fn needs_refresh(&self) -> bool {
        self.expires_at > 0 && self.expires_at - crate::util::now() < 120
    }
}

/// Secret part of an account. Persisted encrypted (DPAPI on Windows).
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Secret {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub tokens: Option<Tokens>,
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    pub email: Option<String>,
    pub source: Source,
    pub meta: Map<String, Value>,
    pub created_at: i64,
    pub secret: Secret,
}

impl Account {
    pub fn meta_str(&self, key: &str) -> Option<String> {
        self.meta
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    pub fn set_meta(&mut self, key: &str, value: impl Into<Value>) {
        self.meta.insert(key.to_owned(), value.into());
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub refresh_secs: u64,
    pub popup_on_hover: bool,
    pub hide_on_blur: bool,
    /// acrylic | blur | mica | none
    pub effect: String,
    /// Opacity of the dark glass tint, 0.2 .. 0.95
    pub tint: f64,
    /// Balance (USD) under which a gateway account is highlighted.
    pub low_balance: f64,
    /// Used-percent above which a subscription limit is highlighted.
    pub warn_percent: f64,
    pub active_tab: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_secs: 120,
            popup_on_hover: true,
            hide_on_blur: true,
            effect: "acrylic".into(),
            tint: 0.62,
            low_balance: 1.0,
            warn_percent: 85.0,
            active_tab: None,
        }
    }
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    #[default]
    Pending,
    Ok,
    Warn,
    Error,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Balance {
    pub amount: f64,
    pub currency: String,
    pub total: Option<f64>,
    pub used: Option<f64>,
    pub expires_at: Option<i64>,
    pub active: Option<bool>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Component {
    pub name: String,
    pub uptime: f64,
    pub series: Vec<f64>,
    pub last_probe_at: Option<i64>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Service {
    pub up: bool,
    pub latency_ms: Option<u64>,
    pub code: Option<u16>,
    pub message: String,
    pub components: Vec<Component>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Limit {
    pub key: String,
    pub name: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
    pub window_secs: Option<i64>,
    pub detail: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Note {
    pub label: String,
    pub value: String,
}

#[derive(Serialize, Clone, Copy, Debug)]
pub struct Probe {
    pub t: i64,
    pub ms: Option<u64>,
    pub ok: bool,
}

/// Everything the UI needs to render one account tab.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Snapshot {
    pub id: String,
    pub kind: Option<Kind>,
    pub label: String,
    pub email: Option<String>,
    pub source: Source,
    pub state: Health,
    /// Hard failure: data could not be fetched / service is down.
    pub error: Option<String>,
    /// Soft problem: data is shown, but something needs attention.
    pub warning: Option<String>,
    pub updated_at: i64,
    pub plan: Option<String>,
    pub balance: Option<Balance>,
    pub service: Option<Service>,
    pub limits: Vec<Limit>,
    pub notes: Vec<Note>,
    pub history: Vec<Probe>,
    pub link: Option<String>,
}

impl Snapshot {
    pub fn for_account(acc: &Account) -> Self {
        Snapshot {
            id: acc.id.clone(),
            kind: Some(acc.kind),
            label: acc.label.clone(),
            email: acc.email.clone(),
            source: acc.source,
            plan: acc.meta_str("plan"),
            ..Default::default()
        }
    }

    pub fn note(&mut self, label: impl Into<String>, value: impl Into<String>) {
        self.notes.push(Note {
            label: label.into(),
            value: value.into(),
        });
    }
}
