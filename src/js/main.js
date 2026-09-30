import {
  api, esc, KINDS, KIND_ORDER, isGateway, pi, ICONS, LOGO, money, clock, dateShort, level,
  bindingLimit, STATE_TEXT, overall, tick, applyLook, animateIn,
} from "./shared.js";

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];

const S = {
  snaps: [],
  settings: {},
  info: {},
  idx: 0,
  refreshing: false,
  pinned: false,
  loginKind: null,
};

const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* ignore */ } },
};

// ============================================================ bootstrap

$("#logo").innerHTML = LOGO;
$("#btn-refresh").innerHTML = ICONS.refresh;
$("#btn-pin").innerHTML = ICONS.pin;
$("#btn-settings").innerHTML = ICONS.gear;
$("#btn-close").innerHTML = ICONS.close;
$("#prev").innerHTML = ICONS.left;
$("#next").innerHTML = ICONS.right;
$("#sheet-close").innerHTML = ICONS.close;

const initial = await api.invoke("get_state");
S.snaps = initial.snapshots;
S.settings = initial.settings;
S.info = initial.info;
S.refreshing = initial.refreshing;
S.pinned = initial.pinned;
applyLook(S.settings, S.info);

const savedTab = store.get("tab");
S.idx = Math.max(0, S.snaps.findIndex((s) => s.id === savedTab));
render();
goTo(S.idx, true);
setRefreshing(S.refreshing);
setPinned(S.pinned, false);
$("#shell").classList.add("enter");

api.listen("snapshots", (snaps) => {
  const currentId = pageIds()[S.idx];
  S.snaps = snaps;
  render();
  const i = pageIds().indexOf(currentId);
  goTo(i >= 0 ? i : Math.min(S.idx, pageIds().length - 1), true);
});
api.listen("refreshing", setRefreshing);
api.listen("settings", (s) => {
  S.settings = s;
  applyLook(S.settings, S.info);
});
api.listen("autostart", (v) => {
  S.info.autostart = v;
  const cb = $("#set-autostart");
  if (cb) cb.checked = v;
});
api.listen("panel-shown", (tab) => {
  const shell = $("#shell");
  shell.classList.remove("enter");
  void shell.offsetWidth;
  shell.classList.add("enter");
  if (tab) {
    const i = pageIds().indexOf(tab);
    if (i >= 0) goTo(i);
  }
  tick();
});

setInterval(() => {
  tick();
  renderUpdated();
}, 1000);

// ============================================================ rendering

function pageIds() {
  return [...S.snaps.map((s) => s.id), "__add"];
}

function render() {
  renderOverall();
  renderTabs();
  renderPages();
  renderUpdated();
}

function renderOverall() {
  const o = overall(S.snaps);
  const pill = $("#overall");
  pill.innerHTML = `<i class="dot ${o === "none" ? "pending" : o}"></i><span>${STATE_TEXT[o]}</span>`;
  pill.firstElementChild.style.animation = o === "none" ? "none" : "";
}

function renderUpdated() {
  const ts = Math.max(0, ...S.snaps.map((s) => s.updated_at || 0));
  const el = $("#updated");
  if (S.refreshing) el.textContent = "Обновление…";
  else if (!S.snaps.length) el.textContent = "";
  else el.textContent = ts ? "Обновлено " + agoShort(ts) : "Ожидание данных…";
}

function agoShort(ts) {
  const d = Math.floor(Date.now() / 1000) - ts;
  if (d < 5) return "только что";
  if (d < 60) return `${d} с назад`;
  if (d < 3600) return `${Math.floor(d / 60)} мин назад`;
  return `${Math.floor(d / 3600)} ч назад`;
}

function renderTabs() {
  const strip = $("#strip");
  const ink = $("#ink");
  const html = S.snaps
    .map(
      (s) => `<button class="tab" data-id="${esc(s.id)}" title="${esc(s.label)}">
        ${pi(s.kind)}<span class="t">${esc(shortLabel(s))}</span><i class="dot ${s.state}"></i></button>`,
    )
    .join("") + `<button class="tab add" data-id="__add" title="Добавить аккаунт">${ICONS.plus}${S.snaps.length ? "" : '<span class="t">Добавить</span>'}</button>`;
  if (strip.dataset.html !== html) {
    strip.dataset.html = html;
    strip.innerHTML = "";
    strip.appendChild(ink);
    strip.insertAdjacentHTML("beforeend", html);
    $$(".tab", strip).forEach((b, i) => b.addEventListener("click", () => goTo(i)));
  }
  const pager = $("#pager");
  pager.innerHTML = pageIds().map(() => "<i></i>").join("");
}

function shortLabel(s) {
  const title = KINDS[s.kind]?.title || "";
  const l = s.label || title;
  const at = l.indexOf("@");
  if (at <= 0) return l;
  // e-mail labels: the provider name reads better, unless there are several of a kind
  const same = S.snaps.filter((x) => x.kind === s.kind).length;
  return same > 1 ? `${title} · ${l.slice(0, at)}` : title;
}

function renderPages() {
  const track = $("#track");
  const existing = new Map($$(".page", track).map((p) => [p.dataset.id, p]));
  const ids = pageIds();
  ids.forEach((id, i) => {
    let page = existing.get(id);
    if (!page) {
      page = document.createElement("section");
      page.className = "page";
      page.dataset.id = id;
    }
    existing.delete(id);
    if (track.children[i] !== page) track.insertBefore(page, track.children[i] || null);
    const html = id === "__add" ? addPageHtml() : accountHtml(S.snaps.find((s) => s.id === id));
    if (page.dataset.html !== html) {
      page.dataset.html = html;
      page.innerHTML = html;
      bindPage(page, id);
      animateIn(page);
      tick(page);
    }
  });
  existing.forEach((p) => p.remove());
}

function banner(kind, text) {
  return `<div class="banner ${kind}">${kind === "error" ? ICONS.alert : ICONS.info}<div>${esc(text)}</div></div>`;
}

function accountHtml(s) {
  const k = KINDS[s.kind] || { title: s.kind };
  const subParts = [];
  if (s.email && s.email !== s.label) subParts.push(s.email);
  else if (s.label === k.title || s.label.startsWith(k.title + " ")) {
    subParts.push(isGateway(s.kind) ? (s.notes || []).some((n) => n.label === "Режим") ? "Только статус" : "API-ключ" : k.title);
  } else subParts.push(k.title);
  if (s.source === "cli") subParts.push(s.kind === "chatgpt" ? "Codex CLI" : "Claude Code");
  const stateTitle = { ok: "Работает", warn: "Нужно внимание", error: "Ошибка", pending: "Проверка…" }[s.state];

  let h = `<div class="acc-head">${pi(s.kind)}
    <div class="who"><div class="l">${esc(s.label)}</div>
    <div class="s"><span>${esc(subParts.join(" · "))}</span>${s.plan ? `<span class="badge">${esc(s.plan)}</span>` : ""}</div></div>
    <i class="dot ${s.state}" title="${stateTitle}"></i></div>`;

  if (s.error) h += banner("error", s.error);
  else if (s.warning) h += banner("warn", s.warning);

  if (!s.updated_at) {
    h += `<div class="card"><div class="skeleton" style="width:40%"></div><div class="skeleton" style="width:75%"></div><div class="skeleton" style="width:60%"></div></div>`;
  } else if (isGateway(s.kind)) {
    h += gatewayHtml(s);
  } else {
    h += limitsHtml(s);
  }

  if (s.notes && s.notes.length) {
    h += `<div class="card"><h4>Подробности</h4><dl class="kv">${s.notes
      .map((n) => `<dt>${esc(n.label)}</dt><dd title="${esc(n.value)}">${esc(n.value)}</dd>`)
      .join("")}</dl></div>`;
  }

  h += `<div class="acts">
    ${s.link ? `<button class="btn" data-act="open" data-url="${esc(s.link)}">${ICONS.external}Открыть</button>` : ""}
    <button class="btn ghost" data-act="rename" title="Переименовать">${ICONS.edit}</button>
    <button class="btn ghost" data-act="left" title="Переместить влево">${ICONS.moveL}</button>
    <button class="btn ghost" data-act="right" title="Переместить вправо">${ICONS.moveR}</button>
    <span style="flex:1"></span>
    <button class="btn ghost danger" data-act="remove" title="Удалить">${ICONS.trash}</button>
  </div>`;
  return h;
}

function gatewayHtml(s) {
  let h = "";
  const b = s.balance;
  if (b) {
    const low = b.amount < (S.settings.low_balance ?? 1);
    let sub = [];
    if (b.total != null && b.used != null) sub.push(`Потрачено <b>${esc(money(b.used))}</b> из ${esc(money(b.total))}`);
    if (b.expires_at) sub.push(`Ключ до <b>${esc(dateShort(b.expires_at))}</b>`);
    if (b.active === false) sub.push(`<b style="color:var(--warn)">ключ отключён</b>`);
    h += `<div class="card"><h4>Баланс ${low ? '<em style="color:var(--warn)">мало средств</em>' : ""}</h4>
      <div class="hero"><div><div class="v" data-count="${b.amount}" data-fmt="usd" data-key="${esc(s.id)}-bal">${esc(money(b.amount))}</div>
      ${sub.length ? `<div class="sub">${sub.join(" · ")}</div>` : ""}</div></div></div>`;
  }
  const svc = s.service;
  if (svc) {
    const st = svc.up ? (svc.code && svc.code >= 400 && svc.code !== 401 ? "warn" : "ok") : "error";
    h += `<div class="card"><h4>Доступность API <em class="num">${svc.latency_ms != null ? svc.latency_ms + " ms" : ""}</em></h4>
      <div class="svc"><i class="dot ${st}"></i><span class="m" title="${esc(svc.message)}">${esc(svc.message)}</span></div>
      ${sparkline(s.history || [])}
      ${componentsHtml(svc.components || [])}</div>`;
  }
  return h;
}

function sparkline(hist) {
  if (hist.length < 2) return `<div class="hint">История задержек появится после нескольких проверок.</div>`;
  const W = 340, H = 34, pad = 3;
  const vals = hist.map((p) => (p.ok && p.ms != null ? p.ms : null));
  const good = vals.filter((v) => v != null);
  const max = Math.max(...good, 1) * 1.15;
  const min = Math.min(...good, max) * 0.85;
  const x = (i) => pad + (i * (W - pad * 2)) / (hist.length - 1);
  const y = (v) => H - pad - ((v - min) / Math.max(1, max - min)) * (H - pad * 2);
  // split into continuous runs so gaps (failed probes) stay empty
  const runs = [];
  vals.forEach((v, i) => {
    if (v == null) return;
    const run = runs[runs.length - 1];
    if (run && run[run.length - 1] === i - 1) run.push(i);
    else runs.push([i]);
  });
  const pt = (i) => `${x(i).toFixed(1)},${y(vals[i]).toFixed(1)}`;
  const line = runs.map((r) => "M" + r.map(pt).join(" L")).join(" ");
  const area = runs
    .filter((r) => r.length > 1)
    .map((r) => `M${x(r[0]).toFixed(1)},${H} L${r.map(pt).join(" L")} L${x(r[r.length - 1]).toFixed(1)},${H} Z`)
    .join(" ");
  const fails = hist.map((p, i) => (!p.ok ? `<rect class="fail" x="${(x(i) - 1.5).toFixed(1)}" y="${H - 8}" width="3" height="8" rx="1"/>` : "")).join("");
  return `<svg class="spark" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none">
    <defs><linearGradient id="sparkfill" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#9db8ff" stop-opacity=".28"/><stop offset="1" stop-color="#9db8ff" stop-opacity="0"/></linearGradient></defs>
    <path class="area" d="${area}"/><path class="line" d="${line}"/>${fails}</svg>`;
}

function componentsHtml(list) {
  if (!list.length) return "";
  return `<div class="comp">${list
    .map((c) => {
      const series = (c.series && c.series.length ? c.series : [c.uptime])
        .map((v) => `<i class="${v < 90 ? "e" : v < 99 ? "w" : ""}" title="${v.toFixed(1)}%"></i>`)
        .join("");
      return `<span class="n">${esc(c.name)}</span><span class="u">${c.uptime.toFixed(1)}%</span><div class="series">${series}</div>`;
    })
    .join("")}</div>`;
}

function limitsHtml(s) {
  const limits = s.limits || [];
  if (!limits.length) {
    return s.error ? "" : `<div class="card"><div class="empty-note">Нет данных о лимитах</div></div>`;
  }
  const warnAt = S.settings.warn_percent ?? 85;
  const top = bindingLimit(limits);
  const left = Math.max(0, 100 - top.used_percent);
  const lv = level(top.used_percent, warnAt);
  const R = 34, L = 2 * Math.PI * R;
  const color = { ok: "var(--ok)", warn: "var(--warn)", error: "var(--err)" }[lv];
  let h = `<div class="card"><div class="sub-hero">
    <div class="gauge" style="--gc:${color}"><svg viewBox="0 0 84 84"><circle class="bg" cx="42" cy="42" r="${R}"/>
      <circle class="fg" cx="42" cy="42" r="${R}" stroke-dasharray="${L.toFixed(2)}" data-gauge="${esc(s.id)}" data-len="${L.toFixed(2)}" data-to="${(left / 100).toFixed(4)}"/></svg>
      <div class="c"><div><b data-count="${left}" data-fmt="pct" data-key="${esc(s.id)}-g">${Math.round(left)}%</b><span>осталось</span></div></div></div>
    <div class="meta"><div class="n">${esc(top.name)}</div>
      ${top.resets_at ? `<div class="r">Сброс <b data-until="${top.resets_at}"></b></div><div class="r">${esc(clock(top.resets_at))}</div>` : `<div class="r">Время сброса неизвестно</div>`}
    </div></div></div>`;

  h += `<div class="card"><h4>Лимиты</h4>${limits
    .map((l) => {
      const rem = Math.max(0, 100 - l.used_percent);
      const lv = level(l.used_percent, warnAt);
      return `<div class="limit"><div class="top"><span class="n" title="${esc(l.name)}">${esc(l.name)}</span>
        <span class="p">${Math.round(rem)}% <span>осталось</span></span></div>
        <div class="bar"><i class="${lv}" data-bar="${esc(s.id + ":" + l.key)}" data-to="${l.used_percent.toFixed(2)}"></i></div>
        <div class="r"><span>Использовано ${Math.round(l.used_percent)}%</span>
        ${l.resets_at ? `<span>сброс <b data-until="${l.resets_at}"></b> · ${esc(clock(l.resets_at))}</span>` : ""}</div>
        ${l.detail ? `<div class="d">${esc(l.detail)}</div>` : ""}</div>`;
    })
    .join("")}</div>`;
  return h;
}

function addPageHtml() {
  const first = !S.snaps.length;
  return `${first ? `<div class="welcome">${pi("oneprovider").replace('class="pi', 'style="width:44px;height:44px;border-radius:13px;margin:0 auto" class="pi')}
      <h2>Добро пожаловать в OneMonitor</h2><p>Добавьте шлюз или войдите в аккаунты подписок — всё будет жить в трее.</p></div>`
      : ""}
    <div class="section-label" style="${first ? "" : "margin-top:6px"}">Шлюзы · API-ключ</div>
    <div class="grid">${KIND_ORDER.filter(isGateway).map(tile).join("")}</div>
    <div class="section-label">Подписки · вход через аккаунт</div>
    <div class="grid">${KIND_ORDER.filter((k) => !isGateway(k)).map(tile).join("")}</div>
    ${api.demo ? `<div class="hint" style="text-align:center;margin-top:14px">Демо-режим: открыт в браузере без Tauri</div>` : ""}`;
}

function tile(kind) {
  const k = KINDS[kind];
  return `<button class="tile" data-add="${kind}">${pi(kind)}<b>${esc(k.title)}</b><small>${esc(k.desc)}</small></button>`;
}

function bindPage(page, id) {
  if (id === "__add") {
    $$("[data-add]", page).forEach((b) => b.addEventListener("click", () => openAdd(b.dataset.add)));
    return;
  }
  $$("[data-act]", page).forEach((b) =>
    b.addEventListener("click", () => {
      const s = S.snaps.find((x) => x.id === id);
      if (!s) return;
      const act = b.dataset.act;
      if (act === "open") api.invoke("open_url", { url: b.dataset.url });
      if (act === "rename") renameDialog(s);
      if (act === "remove") removeDialog(s);
      if (act === "left" || act === "right") {
        api.invoke("move_account", { id, delta: act === "left" ? -1 : 1 }).catch(toast);
      }
    }),
  );
}

// ============================================================ carousel

function goTo(i, instant = false) {
  const ids = pageIds();
  i = Math.max(0, Math.min(ids.length - 1, i));
  S.idx = i;
  const track = $("#track");
  if (instant) track.style.transition = "none";
  track.style.transform = `translateX(${-i * 100}%)`;
  if (instant) {
    track.getBoundingClientRect();
    track.style.transition = "";
  }
  $$(".page", track).forEach((p, j) => p.classList.toggle("active", j === i));
  $$(".tab").forEach((t, j) => t.classList.toggle("active", j === i));
  $$("#pager i").forEach((d, j) => d.classList.toggle("on", j === i));
  $("#prev").disabled = i === 0;
  $("#next").disabled = i === ids.length - 1;
  moveInk(instant);
  if (ids[i] !== "__add") store.set("tab", ids[i]);
}

function moveInk(instant) {
  const tab = $$(".tab")[S.idx];
  const ink = $("#ink");
  if (!tab) return;
  if (instant) ink.style.transition = "none";
  ink.style.left = tab.offsetLeft + "px";
  ink.style.width = tab.offsetWidth + "px";
  if (instant) {
    ink.getBoundingClientRect();
    ink.style.transition = "";
  }
  const strip = $("#strip");
  const target = tab.offsetLeft - (strip.clientWidth - tab.offsetWidth) / 2;
  strip.scrollTo({ left: target, behavior: instant ? "auto" : "smooth" });
}

$("#prev").addEventListener("click", () => goTo(S.idx - 1));
$("#next").addEventListener("click", () => goTo(S.idx + 1));

document.addEventListener("keydown", (e) => {
  if (e.target.closest("input, textarea") || $("#sheet").classList.contains("open") || $("#dialog").classList.contains("open")) {
    if (e.key === "Escape") closeOverlays();
    return;
  }
  if (e.key === "ArrowLeft") goTo(S.idx - 1);
  else if (e.key === "ArrowRight") goTo(S.idx + 1);
  else if (e.key === "Tab" && e.ctrlKey) { e.preventDefault(); goTo(S.idx + (e.shiftKey ? -1 : 1)); }
  else if (e.key === "Escape") api.invoke("hide_main");
  else if (e.key === "F5" || (e.key === "r" && e.ctrlKey)) { e.preventDefault(); refresh(); }
});

// wheel: horizontal scroll (touchpads) or wheel over the tab strip
let wheelLock = 0;
function onWheel(e, vertical) {
  const d = vertical ? (Math.abs(e.deltaY) > Math.abs(e.deltaX) ? e.deltaY : e.deltaX) : e.deltaX;
  if (!vertical && Math.abs(e.deltaX) <= Math.abs(e.deltaY)) return;
  e.preventDefault();
  if (Math.abs(d) < 12 || Date.now() < wheelLock) return;
  wheelLock = Date.now() + 480;
  goTo(S.idx + (d > 0 ? 1 : -1));
}
$("#viewport").addEventListener("wheel", (e) => onWheel(e, false), { passive: false });
$("#strip").addEventListener("wheel", (e) => onWheel(e, true), { passive: false });

// drag / swipe
(() => {
  const vp = $("#viewport");
  const track = $("#track");
  let x0 = null, dx = 0, dragging = false, pid = null;
  vp.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || e.target.closest("button, input, a, select, .kv dd")) return;
    x0 = e.clientX; dx = 0; dragging = false; pid = e.pointerId;
  });
  vp.addEventListener("pointermove", (e) => {
    if (x0 == null || e.pointerId !== pid) return;
    dx = e.clientX - x0;
    if (!dragging && Math.abs(dx) > 8) {
      dragging = true;
      vp.setPointerCapture(pid);
      track.classList.add("dragging");
    }
    if (dragging) {
      const last = pageIds().length - 1;
      const edge = (S.idx === 0 && dx > 0) || (S.idx === last && dx < 0);
      const off = edge ? dx * 0.3 : dx;
      track.style.transform = `translateX(calc(${-S.idx * 100}% + ${off}px))`;
    }
  });
  const end = () => {
    if (x0 == null) return;
    track.classList.remove("dragging");
    if (dragging) {
      const th = vp.clientWidth * 0.18;
      goTo(dx < -th ? S.idx + 1 : dx > th ? S.idx - 1 : S.idx);
    }
    x0 = null; dragging = false;
  };
  vp.addEventListener("pointerup", end);
  vp.addEventListener("pointercancel", end);
})();

window.addEventListener("resize", () => moveInk(true));

// ============================================================ header actions

function setRefreshing(v) {
  S.refreshing = v;
  $("#btn-refresh").classList.toggle("spin", v);
  renderUpdated();
}

function refresh() {
  setRefreshing(true);
  api.invoke("refresh_now");
}

function setPinned(v, send = true) {
  S.pinned = v;
  $("#btn-pin").classList.toggle("on", v);
  $("#btn-pin").title = v ? "Открепить" : "Закрепить поверх окон";
  if (send) api.invoke("set_pinned", { pinned: v });
}

$("#btn-refresh").addEventListener("click", refresh);
$("#btn-pin").addEventListener("click", () => setPinned(!S.pinned));
$("#btn-close").addEventListener("click", () => api.invoke("hide_main"));
$("#btn-settings").addEventListener("click", openSettings);

// ============================================================ overlays

function openSheet(title, html, onBind) {
  $("#sheet-title").textContent = title;
  $("#sheet-body").innerHTML = html;
  onBind?.($("#sheet-body"));
  $("#scrim").classList.add("open");
  $("#sheet").classList.add("open");
}

function closeOverlays() {
  if (S.loginKind) api.invoke("cancel_login");
  $("#sheet").classList.remove("open");
  $("#dialog").classList.remove("open");
  $("#scrim").classList.remove("open");
}

$("#scrim").addEventListener("click", closeOverlays);
$("#sheet-close").addEventListener("click", closeOverlays);

let toastTimer;
function toast(msg) {
  const t = $("#toast");
  t.textContent = String(msg);
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("show"), 2600);
}

function dialog(html, bind) {
  const d = $("#dialog");
  d.innerHTML = html;
  bind(d);
  $("#scrim").classList.add("open");
  d.classList.add("open");
}

function renameDialog(s) {
  dialog(
    `<h3>Переименовать</h3><p>Название вкладки для ${esc(KINDS[s.kind]?.title || "")}</p>
     <input class="input" id="rn" value="${esc(s.label)}" maxlength="60" spellcheck="false" />
     <div class="acts"><button class="btn ghost" data-x>Отмена</button><button class="btn primary" data-ok>Сохранить</button></div>`,
    (d) => {
      const input = $("#rn", d);
      setTimeout(() => { input.focus(); input.select(); }, 60);
      const ok = () =>
        api.invoke("rename_account", { id: s.id, label: input.value })
          .then(closeOverlays).catch(toast);
      $("[data-ok]", d).addEventListener("click", ok);
      $("[data-x]", d).addEventListener("click", closeOverlays);
      input.addEventListener("keydown", (e) => e.key === "Enter" && ok());
    },
  );
}

function removeDialog(s) {
  dialog(
    `<h3>Удалить аккаунт?</h3><p>«${esc(s.label)}» исчезнет из OneMonitor, а его токены будут удалены.${s.source === "cli" ? " Файл сессии CLI не изменится." : ""}</p>
     <div class="acts"><button class="btn ghost" data-x>Отмена</button><button class="btn danger" data-ok>${ICONS.trash}Удалить</button></div>`,
    (d) => {
      $("[data-x]", d).addEventListener("click", closeOverlays);
      $("[data-ok]", d).addEventListener("click", () =>
        api.invoke("remove_account", { id: s.id })
          .then(() => { closeOverlays(); toast("Аккаунт удалён"); })
          .catch(toast),
      );
    },
  );
}

// ---------------------------------------------------------------- add account

function openAdd(kind) {
  const k = KINDS[kind];
  if (k.auth === "key") {
    openSheet(
      k.title,
      `<div class="field"><label>API-ключ</label>
        <div class="input-wrap"><input class="input mono" id="f-key" type="password" placeholder="sk-…" spellcheck="false" autocomplete="off" />
        <button class="icon-btn" id="f-eye" title="Показать">${ICONS.eye}</button></div></div>
       <div class="field"><label>Название вкладки <span class="muted">(необязательно)</span></label>
        <input class="input" id="f-label" placeholder="${esc(k.title)}" maxlength="60" /></div>
       <button class="btn primary block" id="f-add">Добавить</button>
       <div class="err-text" id="f-err"></div>
       <div class="hint">Без ключа будет отслеживаться только доступность API. Ключ хранится локально, зашифрован Windows DPAPI.
        <br/><a data-url="${esc(k.keyHelp.url)}">${esc(k.keyHelp.text)}</a></div>`,
      (b) => {
        const key = $("#f-key", b);
        setTimeout(() => key.focus(), 350);
        $("#f-eye", b).addEventListener("click", () => (key.type = key.type === "password" ? "text" : "password"));
        $("a[data-url]", b).addEventListener("click", (e) => api.invoke("open_url", { url: e.target.dataset.url }));
        const submit = () => {
          $("#f-add", b).disabled = true;
          api.invoke("add_key_account", { kind, key: key.value, label: $("#f-label", b).value || null })
            .then((id) => { closeOverlays(); toast("Добавлено"); selectLater(id); })
            .catch((e) => { $("#f-err", b).textContent = String(e); $("#f-add", b).disabled = false; });
        };
        $("#f-add", b).addEventListener("click", submit);
        b.addEventListener("keydown", (e) => e.key === "Enter" && submit());
      },
    );
    return;
  }

  openSheet(
    k.title,
    `<div id="login-idle">
       <p class="hint" style="margin:0 0 14px;font-size:12.5px;color:var(--text-2)">Откроется браузер для входа в аккаунт ${esc(k.title)}. После входа вкладка появится автоматически.</p>
       <button class="btn primary block" id="f-login">${ICONS.login}Войти через браузер</button>
       ${k.cli ? `<button class="btn block" id="f-cli" style="margin-top:8px">${ICONS.terminal}${esc(k.cli)}</button><div class="hint">${esc(k.cliHint)}</div>` : ""}
       <div class="err-text" id="f-err"></div>
       <div class="hint">Используется тот же OAuth-вход, что и в официальном клиенте. Токены хранятся только на этом компьютере, зашифрованы Windows DPAPI.</div>
     </div>
     <div id="login-wait" hidden>
       <div class="waiting"><div class="ring-spin"></div><div>Завершите вход в открывшемся браузере…</div>
       <button class="btn" id="f-cancel">Отмена</button></div>
     </div>`,
    (b) => {
      const idle = $("#login-idle", b), wait = $("#login-wait", b), err = $("#f-err", b);
      $("#f-login", b).addEventListener("click", () => {
        err.textContent = "";
        idle.hidden = true; wait.hidden = false;
        S.loginKind = kind;
        api.invoke("login", { kind })
          .then((id) => { S.loginKind = null; closeOverlays(); toast("Аккаунт подключён"); selectLater(id); })
          .catch((e) => {
            S.loginKind = null;
            idle.hidden = false; wait.hidden = true;
            if (!String(e).includes("отменён")) err.textContent = String(e);
          });
      });
      $("#f-cancel", b).addEventListener("click", () => api.invoke("cancel_login"));
      $("#f-cli", b)?.addEventListener("click", () => {
        err.textContent = "";
        api.invoke("import_cli", { kind })
          .then((id) => { closeOverlays(); toast("Сессия импортирована"); selectLater(id); })
          .catch((e) => (err.textContent = String(e)));
      });
    },
  );
}

function selectLater(id) {
  // the "snapshots" event re-renders the tabs; select the new one right after
  setTimeout(() => {
    const i = pageIds().indexOf(id);
    if (i >= 0) goTo(i);
  }, 60);
}

// ---------------------------------------------------------------- settings

function seg(id, options, value) {
  return `<div class="seg" id="${id}"><span class="seg-ink"></span>${options
    .map(([v, t]) => `<button data-v="${v}" class="${String(v) === String(value) ? "on" : ""}">${t}</button>`)
    .join("")}</div>`;
}

function placeSegInk(el) {
  const on = $("button.on", el);
  const ink = $(".seg-ink", el);
  if (!on) { ink.style.width = "0"; return; }
  ink.style.left = on.offsetLeft + "px";
  ink.style.width = on.offsetWidth + "px";
}

function sw(id, checked) {
  return `<label class="switch"><input type="checkbox" id="${id}" ${checked ? "checked" : ""}/><i></i></label>`;
}

function openSettings() {
  const s = S.settings;
  const w11 = S.info.win11;
  openSheet(
    "Настройки",
    `<div class="row"><div class="t"><b>Запуск вместе с Windows</b><span>Автозапуск в трей</span></div>${sw("set-autostart", S.info.autostart)}</div>
     <div class="row"><div class="t"><b>Сводка при наведении</b><span>Мини-окно над иконкой в трее</span></div>${sw("set-popup", s.popup_on_hover)}</div>
     <div class="row"><div class="t"><b>Скрывать при потере фокуса</b><span>Панель прячется, как системные меню</span></div>${sw("set-blur", s.hide_on_blur)}</div>
     <div class="section-label">Обновление</div>
     ${seg("set-interval", [[60, "1 мин"], [120, "2 мин"], [300, "5 мин"], [600, "10 мин"], [1800, "30 мин"]], s.refresh_secs)}
     <div class="section-label">Стекло ${w11 ? "" : '<span class="muted" style="text-transform:none;letter-spacing:0;font-weight:400">· эффекты — только Windows 11</span>'}</div>
     ${seg("set-effect", [["acrylic", "Acrylic"], ["blur", "Blur"], ["mica", "Mica"], ["none", "Нет"]], s.effect)}
     <div class="field" style="margin-top:12px"><label>Затемнение <span class="muted" id="tint-v">${Math.round(s.tint * 100)}%</span></label>
       <input type="range" id="set-tint" min="20" max="95" value="${Math.round(s.tint * 100)}" /></div>
     <div class="section-label">Пороги</div>
     <div class="row"><div class="t"><b>Низкий баланс</b><span>Подсветка шлюза, если меньше</span></div>
       <input class="input num" id="set-low" type="number" min="0" step="0.5" value="${s.low_balance}" style="width:90px;text-align:right" /></div>
     <div class="field" style="margin-top:10px"><label>Предупреждать, когда лимит израсходован на <span class="muted" id="warn-v">${Math.round(s.warn_percent)}%</span></label>
       <input type="range" id="set-warn" min="50" max="100" value="${Math.round(s.warn_percent)}" /></div>
     <div class="section-label">Данные</div>
     <div class="row"><div class="t" style="min-width:0"><b>Папка данных</b><span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(S.info.data_dir)}">${esc(S.info.data_dir)}</span></div>
       <button class="btn" id="set-folder">${ICONS.folder}Открыть</button></div>
     <div class="row"><div class="t"><b>OneMonitor ${esc(S.info.version)}</b><span>Портативная версия</span></div>
       <button class="btn danger" id="set-quit">${ICONS.power}Выход</button></div>`,
    (b) => {
      requestAnimationFrame(() => $$(".seg", b).forEach(placeSegInk));
      const save = (patch) => {
        S.settings = { ...S.settings, ...patch };
        applyLook(S.settings, S.info);
        api.invoke("save_settings", { settings: S.settings }).catch(toast);
      };
      $("#set-autostart", b).addEventListener("change", (e) =>
        api.invoke("set_autostart", { enabled: e.target.checked })
          .then((v) => { S.info.autostart = v; e.target.checked = v; })
          .catch((err) => { e.target.checked = !e.target.checked; toast(err); }),
      );
      $("#set-popup", b).addEventListener("change", (e) => save({ popup_on_hover: e.target.checked }));
      $("#set-blur", b).addEventListener("change", (e) => save({ hide_on_blur: e.target.checked }));
      $$(".seg", b).forEach((el) =>
        $$("button", el).forEach((btn) =>
          btn.addEventListener("click", () => {
            $$("button", el).forEach((x) => x.classList.toggle("on", x === btn));
            placeSegInk(el);
            if (el.id === "set-interval") save({ refresh_secs: +btn.dataset.v });
            if (el.id === "set-effect") save({ effect: btn.dataset.v });
          }),
        ),
      );
      const tint = $("#set-tint", b);
      tint.addEventListener("input", () => {
        $("#tint-v", b).textContent = tint.value + "%";
        S.settings.tint = tint.value / 100;
        applyLook(S.settings, S.info);
      });
      tint.addEventListener("change", () => save({ tint: tint.value / 100 }));
      const warn = $("#set-warn", b);
      warn.addEventListener("input", () => ($("#warn-v", b).textContent = warn.value + "%"));
      warn.addEventListener("change", () => save({ warn_percent: +warn.value }));
      $("#set-low", b).addEventListener("change", (e) => save({ low_balance: Math.max(0, +e.target.value || 0) }));
      $("#set-folder", b).addEventListener("click", () => api.invoke("open_data_dir").catch(toast));
      $("#set-quit", b).addEventListener("click", () => api.invoke("quit"));
    },
  );
}
