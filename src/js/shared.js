// Shared helpers for the panel and the tray popup.

const T = window.__TAURI__;

/** Tauri bridge; falls back to a demo backend when opened in a normal browser. */
export const api = await (async () => {
  if (T && T.core) {
    return {
      invoke: (cmd, args) => T.core.invoke(cmd, args),
      listen: (evt, cb) => T.event.listen(evt, (e) => cb(e.payload)),
      demo: false,
    };
  }
  const { mockApi } = await import("./mock.js");
  return mockApi;
})();

export const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

// ---------------------------------------------------------------- providers

export const KINDS = {
  oneprovider: {
    title: "OneProvider",
    desc: "Статус шлюза и баланс ключа",
    auth: "key",
    keyHelp: { text: "Ключ выдаёт @oneprovider_robot", url: "https://t.me/oneprovider_robot" },
  },
  chatgpt: {
    title: "ChatGPT",
    desc: "Лимиты подписки (Codex)",
    auth: "oauth",
    cli: "Импорт из Codex CLI",
    cliHint: "Берёт сессию из ~/.codex/auth.json и обновляет её там же — Codex продолжит работать.",
  },
  claude: {
    title: "Claude",
    desc: "Лимиты Pro / Max: 5 ч и неделя",
    auth: "oauth",
    cli: "Импорт из Claude Code",
    cliHint: "Берёт сессию из ~/.claude/.credentials.json и обновляет её там же.",
  },
  antigravity: {
    title: "Antigravity",
    desc: "Квоты моделей Google",
    auth: "oauth",
  },
};
export const KIND_ORDER = ["oneprovider", "chatgpt", "claude", "antigravity"];
export const isGateway = (k) => k === "oneprovider";

const svg = (body, sw = 2) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

export const PROVIDER_ICONS = {
  oneprovider: svg('<path d="M9.5 8.5 13 6v12"/><path d="M9.5 18h7"/>', 2.3),
  chatgpt: svg('<path d="M12 2.8 20 7.4v9.2l-8 4.6-8-4.6V7.4z"/><path d="M12 7.5v9M8.1 9.75l7.8 4.5M15.9 9.75l-7.8 4.5"/>', 1.9),
  claude: svg('<path d="M12 3.5v17M3.5 12h17M6 6l12 12M18 6 6 18"/>', 2.2),
  antigravity: svg('<path d="M3.5 20C5.5 10.5 8.3 4.5 12 4.5S18.5 10.5 20.5 20"/><path d="M8.5 20c1-3.6 2.1-5.5 3.5-5.5s2.5 1.9 3.5 5.5"/>', 2.1),
};

export const pi = (kind) => `<span class="pi ${esc(kind)}">${PROVIDER_ICONS[kind] || ""}</span>`;

export const ICONS = {
  refresh: svg('<path d="M20 11a8 8 0 0 0-14.7-4.4L4 8"/><path d="M4 4v4h4"/><path d="M4 13a8 8 0 0 0 14.7 4.4L20 16"/><path d="M20 20v-4h-4"/>'),
  gear: svg('<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>', 1.7),
  pin: svg('<path d="M9 4h6l-1 5 3 3v2H7v-2l3-3z"/><path d="M12 14v6"/>', 1.8),
  close: svg('<path d="M6 6l12 12M18 6 6 18"/>'),
  left: svg('<path d="m15 18-6-6 6-6"/>'),
  right: svg('<path d="m9 18 6-6-6-6"/>'),
  plus: svg('<path d="M12 5v14M5 12h14"/>'),
  external: svg('<path d="M14 4h6v6"/><path d="M20 4 11 13"/><path d="M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/>', 1.8),
  edit: svg('<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="m13.5 6.5 4 4"/>', 1.8),
  trash: svg('<path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 13h10l1-13"/><path d="M9 7V4h6v3"/>', 1.8),
  eye: svg('<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>', 1.8),
  alert: svg('<path d="M12 3 2 20h20z"/><path d="M12 10v4M12 17.5v.01"/>', 2),
  info: svg('<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5v.01"/>', 2),
  moveL: svg('<path d="M19 12H5"/><path d="m11 18-6-6 6-6"/>'),
  moveR: svg('<path d="M5 12h14"/><path d="m13 6 6 6-6 6"/>'),
  folder: svg('<path d="M3 6a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z"/>', 1.8),
  power: svg('<path d="M12 3v8"/><path d="M6.3 7.2a8 8 0 1 0 11.4 0"/>', 1.9),
  terminal: svg('<path d="m5 8 4 4-4 4"/><path d="M12 17h7"/>', 2),
  list: svg('<path d="M8 6h12M8 12h12M8 18h12"/><circle cx="4" cy="6" r="1"/><circle cx="4" cy="12" r="1"/><circle cx="4" cy="18" r="1"/>', 1.9),
  cards: svg('<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M3 9h18"/>', 1.8),
  download: svg('<path d="M12 4v11"/><path d="m7 10 5 5 5-5"/><path d="M5 20h14"/>', 1.9),
  bell: svg('<path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15z"/><path d="M10 20a2 2 0 0 0 4 0"/>', 1.8),
  login: svg('<path d="M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3"/><path d="M10 17l5-5-5-5"/><path d="M15 12H4"/>', 1.9),
};

export const LOGO = `<svg class="logo" viewBox="0 0 24 24" fill="none"><circle cx="12" cy="12" r="8.2" stroke="#eceef3" stroke-width="2.4"/><path d="M2.8 12h5l1.6-3.2 2.6 6.4 1.7-3.2h7.5" stroke="#34d399" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

// ---------------------------------------------------------------- formatting

const usd = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2 });
export const money = (v, cur = "USD") =>
  cur === "USD" ? usd.format(v) : `${Number(v).toFixed(2)} ${cur}`;

export const nowSec = () => Math.floor(Date.now() / 1000);

/** "2 ч 14 мин", "3 дн 4 ч", "45 с" */
export function span(sec) {
  sec = Math.max(0, Math.round(sec));
  const d = Math.floor(sec / 86400);
  const h = Math.floor((sec % 86400) / 3600);
  const m = Math.floor((sec % 3600) / 60);
  if (d > 0) return h ? `${d} дн ${h} ч` : `${d} дн`;
  if (h > 0) return m ? `${h} ч ${m} мин` : `${h} ч`;
  if (m > 0) return `${m} мин`;
  return `${sec} с`;
}

export function until(ts) {
  const left = ts - nowSec();
  if (left <= 0) return "сейчас";
  return "через " + span(left);
}

export function ago(ts) {
  if (!ts) return "ещё не обновлялось";
  const d = nowSec() - ts;
  if (d < 5) return "только что";
  if (d < 60) return `${d} с назад`;
  if (d < 3600) return `${Math.floor(d / 60)} мин назад`;
  return `${Math.floor(d / 3600)} ч назад`;
}

const WD = ["вс", "пн", "вт", "ср", "чт", "пт", "сб"];
export function clock(ts) {
  const d = new Date(ts * 1000);
  const now = new Date();
  const hm = d.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return hm;
  const diffDays = (d - now) / 86400000;
  if (diffDays > 0 && diffDays < 6) return `${WD[d.getDay()]} ${hm}`;
  return `${d.toLocaleDateString("ru-RU", { day: "2-digit", month: "2-digit" })} ${hm}`;
}

export function dateShort(ts) {
  return new Date(ts * 1000).toLocaleDateString("ru-RU", { day: "numeric", month: "short" });
}

export function level(used, warnAt = 85) {
  if (used >= 100) return "error";
  if (used >= warnAt) return "warn";
  return "ok";
}

/** The limit that is closest to exhaustion. */
export function bindingLimit(limits) {
  if (!limits || !limits.length) return null;
  return limits.reduce((a, b) => (b.used_percent > a.used_percent ? b : a));
}

export const STATE_TEXT = {
  ok: "Всё в порядке",
  warn: "Нужно внимание",
  error: "Есть сбои",
  pending: "Проверка…",
  none: "Нет аккаунтов",
};

export function overall(snaps) {
  if (!snaps.length) return "none";
  const rank = { pending: 0, ok: 1, warn: 2, error: 3 };
  let best = "pending";
  for (const s of snaps) if (rank[s.state] > rank[best]) best = s.state;
  return best;
}

/** Live countdowns / "ago" labels inside `root`. */
export function tick(root = document) {
  root.querySelectorAll("[data-until]").forEach((el) => (el.textContent = until(+el.dataset.until)));
  root.querySelectorAll("[data-ago]").forEach((el) => (el.textContent = ago(+el.dataset.ago)));
}

/** Applies the glass tint / platform classes. */
export function applyLook(settings, info) {
  const supported = info.supported_effects || (info.win11 ? ["acrylic", "blur", "mica"] : []);
  const fx = supported.includes(settings.effect);
  // Without a backdrop effect the desktop shows through unblurred: keep it denser.
  const tint = fx ? settings.tint : Math.max(settings.tint, 0.85);
  document.documentElement.style.setProperty("--tint", String(tint));
  document.documentElement.classList.toggle("w11", !!info.win11);
  document.documentElement.classList.toggle("fx", fx);
}

// ---------------------------------------------------------------- animation helpers

const memory = new Map();

function tween(from, to, ms, fn) {
  const t0 = performance.now();
  const step = (t) => {
    const k = Math.min(1, (t - t0) / ms);
    const e = 1 - Math.pow(1 - k, 3);
    fn(from + (to - from) * e);
    if (k < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

/** Animates bars, gauges and numbers from their previously rendered values. */
export function animateIn(root) {
  root.querySelectorAll("[data-bar]").forEach((el) => {
    const key = "bar:" + el.dataset.bar;
    const to = +el.dataset.to;
    const from = memory.has(key) ? memory.get(key) : 0;
    memory.set(key, to);
    el.style.width = from + "%";
    el.getBoundingClientRect();
    requestAnimationFrame(() => (el.style.width = to + "%"));
  });
  root.querySelectorAll("[data-gauge]").forEach((el) => {
    const key = "g:" + el.dataset.gauge;
    const len = +el.dataset.len;
    const to = +el.dataset.to; // fraction 0..1 shown
    const from = memory.has(key) ? memory.get(key) : 0;
    memory.set(key, to);
    el.style.strokeDashoffset = String(len * (1 - from));
    el.getBoundingClientRect();
    requestAnimationFrame(() => (el.style.strokeDashoffset = String(len * (1 - to))));
  });
  root.querySelectorAll("[data-count]").forEach((el) => {
    const key = "n:" + el.dataset.key;
    const to = +el.dataset.count;
    const from = memory.has(key) ? memory.get(key) : to * 0.6;
    memory.set(key, to);
    const f = el.dataset.fmt;
    const render = (v) => {
      if (f === "usd") {
        const s = money(v);
        const i = s.lastIndexOf(".");
        el.innerHTML = i > 0 ? `${esc(s.slice(0, i))}<small>${esc(s.slice(i))}</small>` : esc(s);
      } else el.textContent = Math.round(v) + (f === "pct" ? "%" : "");
    };
    if (Math.abs(from - to) < 1e-9) render(to);
    else tween(from, to, 800, render);
  });
}
