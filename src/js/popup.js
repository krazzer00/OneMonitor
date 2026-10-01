import {
  api, esc, KINDS, isGateway, pi, LOGO, money, until, level, bindingLimit, STATE_TEXT, overall, tick, applyLook,
} from "./shared.js";

const $ = (s) => document.querySelector(s);
$("#logo").innerHTML = LOGO;

const st = await api.invoke("get_state");
let snaps = st.snapshots;
let settings = st.settings;
const info = st.info;
applyLook(settings, info);
render();

api.listen("snapshots", (s) => { snaps = s; render(); });
api.listen("settings", (s) => { settings = s; applyLook(settings, info); render(); });
api.listen("popup-shown", () => {
  const pop = $("#pop");
  pop.classList.remove("in");
  void pop.offsetWidth;
  pop.classList.add("in");
  tick();
});

document.addEventListener("mouseenter", () => api.invoke("popup_hover", { inside: true }));
document.addEventListener("mouseleave", () => api.invoke("popup_hover", { inside: false }));
setInterval(tick, 1000);

function row(s) {
  const k = KINDS[s.kind] || { title: s.kind };
  let value = "…", vcls = "ok", sub = "", bar = null;
  if (s.error) {
    value = "Ошибка"; vcls = "error";
    sub = `<span title="${esc(s.error)}">${esc(s.error.slice(0, 28))}</span>`;
  } else if (!s.updated_at) {
    value = "…"; sub = "<span>проверка</span>";
  } else if (isGateway(s.kind)) {
    const svc = s.service;
    if (s.balance) {
      value = (s.balance.stale ? "≈ " : "") + money(s.balance.amount);
      vcls = s.balance.stale || s.balance.amount < (settings.low_balance ?? 1) ? "warn" : "ok";
    } else if (svc) {
      value = svc.up ? "Доступен" : "Недоступен";
    }
    if (svc && !svc.up) vcls = "error";
    sub = svc ? `<span>${svc.up ? "API" : "API недоступен"}${svc.latency_ms != null ? " · " + svc.latency_ms + " ms" : ""}</span>` : "";
    if (s.spend && svc && svc.up) sub = `<span>сегодня ${esc(money(s.spend.today))} · ${svc.latency_ms ?? "—"} ms</span>`;
    if (svc && svc.up && svc.issues && svc.issues.length) {
      const names = svc.issues.map((x) => x.split(":")[0]).join(", ");
      sub = `<span class="iss" title="${esc(svc.issues.join(", "))}">перебои: ${esc(names)}</span>`;
      if (vcls === "ok") vcls = "warn";
    }
  } else {
    const top = bindingLimit(s.limits);
    if (top) {
      const left = Math.max(0, 100 - top.used_percent);
      value = Math.round(left) + "%";
      vcls = level(top.used_percent, settings.warn_percent ?? 85);
      sub = top.resets_at ? `<span data-until="${top.resets_at}">${esc(until(top.resets_at))}</span>` : "<span>осталось</span>";
      bar = { w: top.used_percent, cls: vcls };
    } else {
      value = "—";
    }
  }
  const who = s.email && s.email !== s.label
    ? s.email
    : s.label === k.title || s.label.startsWith(k.title + " ")
      ? (isGateway(s.kind) ? "Шлюз" : s.plan || k.title)
      : [k.title, s.plan].filter(Boolean).join(" · ");
  return `<div class="prow" data-id="${esc(s.id)}">${pi(s.kind)}
    <div class="who"><div class="l">${esc(s.label)}</div><div class="s">${esc(who)}</div></div>
    <div class="v"><b class="${vcls}">${esc(value)}</b>${sub}</div>
    ${bar ? `<div class="mini"><i class="${bar.cls}" style="width:${bar.w.toFixed(1)}%"></i></div>` : ""}</div>`;
}

function render() {
  const o = overall(snaps);
  $("#overall").innerHTML = `<i class="dot ${o === "none" ? "pending" : o}"></i><span>${STATE_TEXT[o]}</span>`;
  $("#rows").innerHTML = snaps.length
    ? snaps.map(row).join("")
    : `<div class="pop-empty">Аккаунты ещё не добавлены. Нажмите на иконку, чтобы открыть панель.</div>`;
  document.querySelectorAll(".prow").forEach((el, i) => {
    el.style.animationDelay = i * 30 + "ms";
    el.addEventListener("click", () => api.invoke("open_main", { tab: el.dataset.id }));
  });
  const ts = Math.max(0, ...snaps.map((s) => s.updated_at || 0));
  $("#foot").innerHTML = `<span>${ts ? `Обновлено <span data-ago="${ts}"></span>` : "Ожидание данных"}</span><span>клик — открыть</span>`;
  tick();
  requestAnimationFrame(fit);
}

function fit() {
  const h = $("#pop").getBoundingClientRect().height;
  api.invoke("popup_fit", { height: Math.ceil(h) });
}
