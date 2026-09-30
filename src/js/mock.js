// Demo backend used only when the UI is opened in a plain browser (design preview).

const now = () => Math.floor(Date.now() / 1000);
const listeners = {};

const history = (base, n = 40, fail = []) =>
  Array.from({ length: n }, (_, i) => ({
    t: now() - (n - i) * 120,
    ms: fail.includes(i) ? null : Math.round(base + Math.sin(i / 2.3) * base * 0.25 + (i % 7) * 9),
    ok: !fail.includes(i),
  }));

let settings = {
  refresh_secs: 120,
  popup_on_hover: true,
  hide_on_blur: true,
  effect: "acrylic",
  tint: 0.62,
  low_balance: 1,
  warn_percent: 85,
  active_tab: null,
};

let snapshots = [
  {
    id: "a1", kind: "oneprovider", label: "OneProvider", email: null, source: "apikey", state: "ok",
    error: null, warning: null, updated_at: now() - 14, plan: null,
    balance: { amount: 54.77, currency: "USD", total: null, used: null, expires_at: now() + 86400 * 41, active: true },
    service: {
      up: true, latency_ms: 182, code: 200, message: "API работает",
      components: [
        { name: "Claude", uptime: 99.6, series: [100, 100, 99, 100, 100, 100, 97, 100, 100, 100, 100, 100], last_probe_at: now() - 90 },
        { name: "GPT", uptime: 98.9, series: [100, 100, 100, 92, 100, 100, 100, 100, 99, 100, 100, 100], last_probe_at: now() - 90 },
      ],
    },
    limits: [], notes: [{ label: "Синхронизация баланса", value: "30.09 16:02" }],
    history: history(180, 40, [12]), link: "https://oneprovider.dev/status",
  },
  {
    id: "a2", kind: "chatgpt", label: "work@example.com", email: "work@example.com", source: "oauth", state: "ok",
    error: null, warning: null, updated_at: now() - 14, plan: "Plus", balance: null, service: null,
    limits: [
      { key: "codex.primary", name: "5-часовой лимит", used_percent: 37, resets_at: now() + 2 * 3600 + 14 * 60, window_secs: 18000, detail: null },
      { key: "codex.secondary", name: "Недельный лимит", used_percent: 58, resets_at: now() + 3 * 86400 + 5 * 3600, window_secs: 604800, detail: null },
    ],
    notes: [], history: [], link: "https://chatgpt.com/codex/settings/usage",
  },
  {
    id: "a3", kind: "claude", label: "me@example.com", email: "me@example.com", source: "cli", state: "warn",
    error: null, warning: null, updated_at: now() - 14, plan: "Max 5x", balance: null, service: null,
    limits: [
      { key: "five_hour", name: "5-часовое окно", used_percent: 88, resets_at: now() + 47 * 60, window_secs: 18000, detail: null },
      { key: "seven_day", name: "Неделя · все модели", used_percent: 41, resets_at: now() + 4 * 86400, window_secs: 604800, detail: null },
      { key: "seven_day_opus", name: "Неделя · Opus", used_percent: 12, resets_at: now() + 4 * 86400, window_secs: 604800, detail: null },
    ],
    notes: [], history: [], link: "https://claude.ai/settings/usage",
  },
  {
    id: "a4", kind: "antigravity", label: "me@gmail.com", email: "me@gmail.com", source: "oauth", state: "ok",
    error: null, warning: null, updated_at: now() - 14, plan: "Google AI Pro", balance: null, service: null,
    limits: [
      { key: "Claude", name: "Claude", used_percent: 20, resets_at: now() + 3 * 3600, detail: "Claude Opus 4.5 (Thinking), Claude Sonnet 4.5" },
      { key: "Gemini Pro", name: "Gemini Pro", used_percent: 0, resets_at: now() + 5 * 3600, detail: "Gemini 3 Pro (High), Gemini 3 Pro (Low)" },
      { key: "Gemini Flash", name: "Gemini Flash", used_percent: 64, resets_at: now() + 1 * 3600, detail: "Gemini 3 Flash" },
    ],
    notes: [], history: [], link: "https://antigravity.google/",
  },
  {
    id: "a5", kind: "openrouter", label: "OpenRouter ···9f2c", email: null, source: "apikey", state: "error",
    error: null, warning: null, updated_at: now() - 14, plan: null,
    balance: { amount: 3.12, currency: "USD", total: 20, used: 16.88 },
    service: { up: false, latency_ms: null, code: 503, message: "Сбой API (HTTP 503)", components: [] },
    limits: [], notes: [{ label: "Расход сегодня", value: "$0.42" }, { label: "Расход за месяц", value: "$9.10" }],
    history: history(240, 40, [30, 38, 39]), link: "https://openrouter.ai/settings/credits",
  },
];

const emit = (evt, payload) => (listeners[evt] || []).forEach((cb) => cb(payload));

export const mockApi = {
  demo: true,
  listen: async (evt, cb) => {
    (listeners[evt] ||= []).push(cb);
    return () => {};
  },
  invoke: async (cmd, args = {}) => {
    switch (cmd) {
      case "get_state":
        return {
          snapshots, settings,
          info: { version: "0.1.0", data_dir: "C:\\Tools\\OneMonitor\\data", win11: true, effect_active: true, autostart: true },
          refreshing: false, pinned: false,
        };
      case "refresh_now":
        emit("refreshing", true);
        setTimeout(() => {
          snapshots = snapshots.map((s) => ({ ...s, updated_at: now() }));
          emit("snapshots", snapshots);
          emit("refreshing", false);
        }, 900);
        return;
      case "save_settings":
        settings = args.settings;
        emit("settings", settings);
        return;
      case "remove_account":
        snapshots = snapshots.filter((s) => s.id !== args.id);
        emit("snapshots", snapshots);
        return;
      case "rename_account":
        snapshots = snapshots.map((s) => (s.id === args.id ? { ...s, label: args.label } : s));
        emit("snapshots", snapshots);
        return;
      case "login":
        await new Promise((r) => setTimeout(r, 2500));
        throw "Демо-режим: вход недоступен";
      case "set_autostart":
        return args.enabled;
      default:
        return null;
    }
  },
};
