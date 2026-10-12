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
  notify: true,
  notify_balance: true,
  notify_limits: true,
  notify_service: true,
  tray_badge: "balance",
  hotkey: "Ctrl+Alt+O",
  auto_update: true,
};

let snapshots = [
  {
    id: "a1", kind: "oneprovider", label: "OneProvider", email: null, source: "apikey", state: "warn",
    error: null, updated_at: now() - 14, plan: null,
    balance: { amount: 1317.05, currency: "USD", total: null, used: null, expires_at: now() + 86400 * 89, active: true },
    warning: "Перебои у моделей: DeepSeek: сбой, GLM: перебои",
    spend: {
      currency: "USD", today: 0.38, today_requests: 31, week: 320.1, month: 634.77, forecast_days: 28.8,
      quota_limit: 1820.11, quota_used: 634.77, quota_remaining: 1185.34,
      daily: [2.3, 0.78, 1.62, 127.47, 60.16, 40.0, 4.19, 4.74, 301.93, 0.97, 9.09, 0, 3.18, 0.38].map((cost, i) => ({
        date: new Date(Date.now() - (13 - i) * 86400000).toISOString().slice(0, 10), cost, requests: Math.round(cost * 9) + 3,
      })),
      top_models: [{ name: "deepseek-v4-pro", cost: 276.89, requests: 1348 }, { name: "claude-opus-5-5", cost: 151.2, requests: 402 }, { name: "gpt-6-sol", cost: 64.5, requests: 610 }],
    },
    service: {
      up: true, latency_ms: 182, code: 200, message: "API работает · перебои: DeepSeek, GLM",
      issues: ["DeepSeek: сбой", "GLM: перебои"],
      components: [
        { name: "Claude upstream", uptime: 92.81, state: "ok", recent: Array(24).fill(true), last_probe_at: now() - 600 },
        { name: "ChatGPT upstream", uptime: 88.41, state: "ok", recent: Array(24).fill(true), last_probe_at: now() - 500 },
        { name: "DeepSeek upstream", uptime: 93.93, state: "down", recent: [true, false, false, true, true, true, false, true, false, true, false, true, false, true, true, true, true, false, true, false, false, false, false, false], last_probe_at: now() - 200 },
        { name: "GLM upstream", uptime: 94.72, state: "degraded", recent: [...Array(23).fill(true), false], last_probe_at: now() - 200 },
        { name: "Qwen upstream", uptime: 52.12, state: "ok", recent: Array(24).fill(true), last_probe_at: now() - 400 },
      ],
    },
    limits: [], notes: [{ label: "Синхронизация баланса", value: "30.09 16:02" }],
    history: history(180, 40, [12]), link: "https://oneprovider.dev/status",
  },
  {
    id: "a5", kind: "nanogpt", label: "NanoGPT", email: null, source: "apikey", state: "ok",
    error: null, warning: null, updated_at: now() - 14, plan: "Подписка",
    balance: { amount: 12.48, currency: "USD", total: null, used: null, expires_at: null, active: null },
    limits: [
      { key: "weeklyInputTokens", name: "Входные токены · неделя", used_percent: 46, resets_at: now() + 3 * 86400 + 7 * 3600, window_secs: 604800, detail: "27.6 млн из 60 млн токенов", pace_ok: true },
      { key: "dailyInputTokens", name: "Входные токены · день", used_percent: 31, resets_at: now() + 9 * 3600, window_secs: 86400, detail: "3.1 млн из 10 млн токенов" },
    ],
    spend: {
      currency: "USD", today: 0.12, today_requests: 214, week: 1.9, month: 6.4, forecast_days: 46, top_period: "30 дней",
      daily: [0.4, 0.1, 0.3, 0, 0.9, 0.2, 0.5, 0.3, 0.1, 0.2, 0.6, 0.4, 0.2, 0.12].map((cost, i) => ({
        date: new Date(Date.now() - (13 - i) * 86400000).toISOString().slice(0, 10), cost, requests: Math.round(cost * 900) + 20,
      })),
      top_models: [{ name: "GLM 5.1", cost: 3.1, requests: 2210 }, { name: "Kimi K3", cost: 2.2, requests: 1480 }, { name: "DeepSeek V4", cost: 0.7, requests: 960 }],
    },
    service: {
      up: true, latency_ms: 236, code: 200, message: "API работает", issues: [], models_status: "ok",
      components: [
        { name: "nano-gpt.com", uptime: 99.87, state: "ok", granularity: "days", recent: [...Array(17).fill(true), false, ...Array(12).fill(true)] },
        { name: "Zano deposits", uptime: 100, state: "ok", granularity: "days", recent: Array(30).fill(true) },
      ],
    },
    notes: [{ label: "Подписка до", value: "05.11 03:00" }, { label: "Оплата запросов", value: "из подписки" }],
    history: history(240, 40), link: "https://status.nano-gpt.com",
  },
  {
    id: "a2", kind: "chatgpt", label: "work@example.com", email: "work@example.com", source: "oauth", state: "ok",
    error: null, warning: null, updated_at: now() - 14, plan: "Plus", balance: null, service: null,
    limits: [
      { key: "codex.primary", name: "5-часовой лимит", used_percent: 37, resets_at: now() + 2 * 3600 + 14 * 60, window_secs: 18000, detail: null, pace_ok: true },
      { key: "codex.secondary", name: "Недельный лимит", used_percent: 58, resets_at: now() + 3 * 86400 + 5 * 3600, window_secs: 604800, detail: null },
    ],
    notes: [], history: [], link: "https://chatgpt.com/codex/settings/usage",
  },
  {
    id: "a3", kind: "claude", label: "me@example.com", email: "me@example.com", source: "cli", state: "warn",
    error: null, warning: null, updated_at: now() - 14, plan: "Max 5x", balance: null, service: null,
    limits: [
      { key: "five_hour", name: "5-часовое окно", used_percent: 88, resets_at: now() + 47 * 60, window_secs: 18000, detail: null, eta_secs: 25 * 60 },
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
          info: {
            version: "0.2.0", data_dir: "C:\\Tools\\OneMonitor\\data", win11: true, effect_active: true,
            supported_effects: ["blur", "acrylic", "mica"], autostart: true, dev_build: false,
          },
          refreshing: false, pinned: false,
          update: { available: { version: "0.2.1", url: "https://github.com/krazzer00/OneMonitor/releases" }, checking: false, installing: false, error: null, checked_at: now() - 600 },
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
      case "set_hotkey":
        settings = { ...settings, hotkey: args.hotkey };
        return args.hotkey;
      case "check_update":
        await new Promise((r) => setTimeout(r, 700));
        return { available: null, checking: false, installing: false, error: null, checked_at: now() };
      case "install_update":
        await new Promise((r) => setTimeout(r, 1500));
        throw "Демо-режим: обновление недоступно";
      default:
        return null;
    }
  },
};
