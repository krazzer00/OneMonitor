mod model;
mod oauth;
mod providers;
mod store;
mod ui;
mod util;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, Rect, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::{oneshot, Notify};

use model::{Account, Health, Kind, Probe, Settings, Snapshot, Source};
use util::{now, Res};

const HISTORY_LEN: usize = 48;
const POPUP_WIDTH: f64 = 300.0;

#[derive(Default)]
struct UiState {
    tray_hover: bool,
    popup_hover: bool,
    tray_rect: Option<Rect>,
    pinned: bool,
    last_blur_hide: Option<Instant>,
    hover_gen: u64,
}

pub struct AppState {
    dir: PathBuf,
    settings: Mutex<Settings>,
    accounts: Mutex<Vec<Account>>,
    snapshots: Mutex<HashMap<String, Snapshot>>,
    history: Mutex<HashMap<String, VecDeque<Probe>>>,
    http: reqwest::Client,
    wake: Notify,
    refreshing: AtomicBool,
    login_cancel: Mutex<Option<oneshot::Sender<()>>>,
    ui: Mutex<UiState>,
}

impl AppState {
    fn ordered(&self) -> Vec<Snapshot> {
        let accounts = self.accounts.lock().unwrap();
        let snaps = self.snapshots.lock().unwrap();
        let hist = self.history.lock().unwrap();
        accounts
            .iter()
            .map(|a| {
                let mut s = snaps
                    .get(&a.id)
                    .cloned()
                    .unwrap_or_else(|| Snapshot::for_account(a));
                s.label = a.label.clone();
                s.email = a.email.clone();
                s.history = hist
                    .get(&a.id)
                    .map(|h| h.iter().copied().collect())
                    .unwrap_or_default();
                s
            })
            .collect()
    }

    fn save_accounts(&self) -> Res<()> {
        let accounts = self.accounts.lock().unwrap().clone();
        store::save_accounts(&self.dir, &accounts)
    }
}

fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

// ---- Refresh loop ---------------------------------------------------------------------

async fn refresh_all(app: &AppHandle) {
    let st = state(app);
    if st.refreshing.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = app.emit("refreshing", true);
    let accounts = st.accounts.lock().unwrap().clone();
    let settings = st.settings.lock().unwrap().clone();
    let http = st.http.clone();

    let outcomes = futures::future::join_all(
        accounts
            .into_iter()
            .map(|a| providers::fetch(&http, a, &settings)),
    )
    .await;

    let mut changed = false;
    {
        let mut accounts = st.accounts.lock().unwrap();
        let mut snaps = st.snapshots.lock().unwrap();
        let mut hist = st.history.lock().unwrap();
        for o in outcomes {
            let id = o.snap.id.clone();
            let Some(current) = accounts.iter_mut().find(|a| a.id == id) else {
                continue; // removed while refreshing
            };
            if let Some(svc) = &o.snap.service {
                let h = hist.entry(id.clone()).or_default();
                h.push_back(Probe {
                    t: o.snap.updated_at,
                    ms: svc.latency_ms,
                    ok: svc.up,
                });
                while h.len() > HISTORY_LEN {
                    h.pop_front();
                }
            }
            if let Some(updated) = o.account {
                // keep user edits (label) made while the request was in flight
                current.secret = updated.secret;
                current.meta = updated.meta;
                current.email = updated.email;
                changed = true;
            }
            snaps.insert(id, o.snap);
        }
    }
    if changed {
        if let Err(e) = st.save_accounts() {
            eprintln!("save accounts: {e}");
        }
    }
    st.refreshing.store(false, Ordering::SeqCst);
    let _ = app.emit("refreshing", false);
    publish(app);
}

fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            refresh_all(&app).await;
            let secs = state(&app).settings.lock().unwrap().refresh_secs.clamp(60, 3600);
            let st = state(&app);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(secs)) => {}
                _ = st.wake.notified() => {}
            }
        }
    });
}

/// Pushes fresh snapshots to both windows and updates the tray icon.
fn publish(app: &AppHandle) {
    let snaps = state(app).ordered();
    let _ = app.emit("snapshots", &snaps);
    update_tray(app, &snaps);
}

// ---- Tray -----------------------------------------------------------------------------

const TRAY_ID: &str = "onemonitor";

/// Tray menu checkbox, kept to sync it when autostart is toggled from the panel.
struct AutostartItem(Mutex<Option<CheckMenuItem<tauri::Wry>>>);

fn tray_image(h: Option<Health>) -> Image<'static> {
    let bytes: &'static [u8] = match h {
        Some(Health::Ok) => include_bytes!("../icons/tray-ok.png"),
        Some(Health::Warn) => include_bytes!("../icons/tray-warn.png"),
        Some(Health::Error) => include_bytes!("../icons/tray-error.png"),
        _ => include_bytes!("../icons/tray-idle.png"),
    };
    Image::from_bytes(bytes).expect("tray icon")
}

fn overall(snaps: &[Snapshot]) -> Option<Health> {
    snaps
        .iter()
        .map(|s| s.state)
        .filter(|h| *h != Health::Pending)
        .max()
}

fn tooltip(snaps: &[Snapshot]) -> String {
    let mut lines = vec!["OneMonitor".to_owned()];
    for s in snaps.iter().take(5) {
        let kind = s.kind.map(|k| k.title()).unwrap_or("");
        let value = if let Some(e) = &s.error {
            format!("ошибка: {}", e.chars().take(30).collect::<String>())
        } else if let Some(b) = &s.balance {
            util::fmt_usd(b.amount)
        } else if let Some(l) = s
            .limits
            .iter()
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
        {
            format!("осталось {:.0}%", 100.0 - l.used_percent)
        } else if let Some(svc) = &s.service {
            if svc.up { "доступен".into() } else { "недоступен".into() }
        } else {
            "…".into()
        };
        lines.push(format!("{kind}: {value}"));
    }
    let mut t = lines.join("\n");
    if t.chars().count() > 120 {
        t = t.chars().take(119).collect::<String>() + "…";
    }
    t
}

fn update_tray(app: &AppHandle, snaps: &[Snapshot]) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let _ = tray.set_icon(Some(tray_image(overall(snaps))));
    let popup = state(app).settings.lock().unwrap().popup_on_hover;
    // With the hover popup enabled the native tooltip would just overlap it.
    let _ = tray.set_tooltip(if popup { None } else { Some(tooltip(snaps)) });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let open = MenuItem::with_id(app, "open", "Открыть панель", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Обновить сейчас", true, None::<&str>)?;
    let auto = CheckMenuItem::with_id(app, "autostart", "Запускать вместе с Windows", true, autostart, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    *app.state::<AutostartItem>().0.lock().unwrap() = Some(auto.clone());
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &refresh,
            &PredefinedMenuItem::separator(app)?,
            &auto,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_image(None))
        .tooltip("OneMonitor")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_main(app, None),
            "refresh" => state(app).wake.notify_one(),
            "autostart" => {
                let enabled = app.autolaunch().is_enabled().unwrap_or(false);
                let _ = set_autostart_inner(app, !enabled);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    rect,
                    ..
                } => {
                    state(app).ui.lock().unwrap().tray_rect = Some(rect);
                    hide_popup(app);
                    toggle_main(app);
                }
                TrayIconEvent::Enter { rect, .. } => {
                    let st = state(app);
                    let gen = {
                        let mut ui = st.ui.lock().unwrap();
                        ui.tray_hover = true;
                        ui.tray_rect = Some(rect);
                        ui.hover_gen += 1;
                        ui.hover_gen
                    };
                    schedule_popup_show(app.clone(), gen);
                }
                TrayIconEvent::Move { rect, .. } => {
                    let st = state(app);
                    let mut ui = st.ui.lock().unwrap();
                    ui.tray_hover = true;
                    ui.tray_rect = Some(rect);
                }
                TrayIconEvent::Leave { .. } => {
                    state(app).ui.lock().unwrap().tray_hover = false;
                    schedule_popup_hide(app.clone());
                }
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}

fn set_autostart_inner(app: &AppHandle, enabled: bool) -> Res<bool> {
    let al = app.autolaunch();
    if enabled {
        al.enable().map_err(|e| e.to_string())?;
    } else {
        al.disable().map_err(|e| e.to_string())?;
    }
    let now_enabled = al.is_enabled().unwrap_or(enabled);
    if let Some(item) = app.state::<AutostartItem>().0.lock().unwrap().as_ref() {
        let _ = item.set_checked(now_enabled);
    }
    let _ = app.emit("autostart", now_enabled);
    Ok(now_enabled)
}

// ---- Windows --------------------------------------------------------------------------

fn main_visible(app: &AppHandle) -> bool {
    ui::window(app, "main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

fn show_main(app: &AppHandle, tab: Option<String>) {
    let Some(win) = ui::window(app, "main") else { return };
    hide_popup(app);
    if !win.is_visible().unwrap_or(false) {
        let rect = state(app).ui.lock().unwrap().tray_rect;
        ui::place_near_tray(app, &win, rect);
    }
    let _ = win.show();
    let _ = win.unminimize();
    let _ = win.set_focus();
    let _ = app.emit_to("main", "panel-shown", tab);
}

fn toggle_main(app: &AppHandle) {
    if main_visible(app) {
        if let Some(w) = ui::window(app, "main") {
            let _ = w.hide();
        }
        return;
    }
    // The click that lands on the tray first blurs (and hides) the panel;
    // don't immediately re-open it.
    let recently = state(app)
        .ui
        .lock()
        .unwrap()
        .last_blur_hide
        .is_some_and(|t| t.elapsed() < Duration::from_millis(350));
    if !recently {
        show_main(app, None);
    }
}

fn show_popup(app: &AppHandle) {
    let Some(win) = ui::window(app, "popup") else { return };
    let rect = state(app).ui.lock().unwrap().tray_rect;
    ui::place_near_tray(app, &win, rect);
    let _ = win.show();
    let _ = app.emit_to("popup", "popup-shown", ());
}

fn hide_popup(app: &AppHandle) {
    if let Some(w) = ui::window(app, "popup") {
        let _ = w.hide();
    }
    state(app).ui.lock().unwrap().popup_hover = false;
}

fn schedule_popup_show(app: AppHandle, gen: u64) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(220)).await;
        let st = state(&app);
        let ok = {
            let ui = st.ui.lock().unwrap();
            ui.tray_hover && ui.hover_gen == gen
        };
        let enabled = st.settings.lock().unwrap().popup_on_hover;
        if ok && enabled && !main_visible(&app) {
            show_popup(&app);
        }
    });
}

fn schedule_popup_hide(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(320)).await;
        let st = state(&app);
        let hide = {
            let ui = st.ui.lock().unwrap();
            !ui.tray_hover && !ui.popup_hover
        };
        if hide {
            hide_popup(&app);
        }
    });
}

fn apply_styles(app: &AppHandle) {
    let effect = state(app).settings.lock().unwrap().effect.clone();
    for label in ["main", "popup"] {
        if let Some(w) = ui::window(app, label) {
            ui::apply_style(&w, &effect);
        }
    }
}

// ---- Commands -------------------------------------------------------------------------

#[derive(Serialize)]
struct AppInfo {
    version: String,
    data_dir: String,
    win11: bool,
    effect_active: bool,
    autostart: bool,
}

#[derive(Serialize)]
struct StateDto {
    snapshots: Vec<Snapshot>,
    settings: Settings,
    info: AppInfo,
    refreshing: bool,
    pinned: bool,
}

#[tauri::command]
fn get_state(app: AppHandle) -> StateDto {
    let st = state(&app);
    // Take every lock in its own statement: a guard created inside the struct
    // literal would live until the end of it, and re-locking the same mutex
    // there deadlocks the main thread.
    let settings = st.settings.lock().unwrap().clone();
    let pinned = st.ui.lock().unwrap().pinned;
    let effect_active = ui::effect_active(&settings.effect);
    StateDto {
        snapshots: st.ordered(),
        settings,
        info: AppInfo {
            version: app.package_info().version.to_string(),
            data_dir: st.dir.display().to_string(),
            win11: ui::is_win11(),
            effect_active,
            autostart: app.autolaunch().is_enabled().unwrap_or(false),
        },
        refreshing: st.refreshing.load(Ordering::SeqCst),
        pinned,
    }
}

#[tauri::command]
fn refresh_now(app: AppHandle) {
    state(&app).wake.notify_one();
}

fn insert_account(app: &AppHandle, acc: Account) -> Res<String> {
    let st = state(app);
    let id = {
        let mut accounts = st.accounts.lock().unwrap();
        let same = accounts.iter_mut().find(|a| {
            a.kind == acc.kind
                && a.source == acc.source
                && match (&a.email, &acc.email) {
                    (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
                    _ => acc.source == Source::Cli,
                }
        });
        match same {
            Some(existing) => {
                existing.secret = acc.secret;
                existing.meta.extend(acc.meta);
                existing.email = acc.email.or(existing.email.take());
                existing.id.clone()
            }
            None => {
                let id = acc.id.clone();
                accounts.push(acc);
                id
            }
        }
    };
    st.save_accounts()?;
    st.snapshots.lock().unwrap().remove(&id);
    publish(app);
    st.wake.notify_one();
    Ok(id)
}

#[tauri::command]
fn add_key_account(app: AppHandle, kind: Kind, key: String, label: Option<String>) -> Res<String> {
    if !kind.is_gateway() {
        return Err("Для этого сервиса используйте вход через аккаунт".into());
    }
    let key = key
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '“' || c == '”' || c == '«' || c == '»')
        .trim()
        .to_owned();
    let label = label
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| {
            if key.len() > 8 {
                format!("{} ···{}", kind.title(), &key[key.len() - 4..])
            } else {
                kind.title().to_owned()
            }
        });
    let acc = Account {
        id: uuid::Uuid::new_v4().to_string(),
        kind,
        label,
        email: None,
        source: Source::ApiKey,
        meta: Default::default(),
        created_at: now(),
        secret: model::Secret {
            api_key: Some(key).filter(|k| !k.is_empty()),
            tokens: None,
        },
    };
    // API-key accounts are never merged: two keys = two tabs.
    let st = state(&app);
    let id = acc.id.clone();
    st.accounts.lock().unwrap().push(acc);
    st.save_accounts()?;
    publish(&app);
    st.wake.notify_one();
    Ok(id)
}

#[tauri::command]
async fn login(app: AppHandle, kind: Kind) -> Res<String> {
    let (tx, rx) = oneshot::channel();
    {
        let st = state(&app);
        let mut slot = st.login_cancel.lock().unwrap();
        if let Some(prev) = slot.take() {
            let _ = prev.send(());
        }
        *slot = Some(tx);
    }
    let http = state(&app).http.clone();
    let opener = app.clone();
    let open = move |url: &str| {
        let _ = opener.opener().open_url(url, None::<&str>);
    };
    let result = match kind {
        Kind::ChatGpt => providers::chatgpt::login(&http, open, rx).await,
        Kind::Claude => providers::claude::login(&http, open, rx).await,
        Kind::Antigravity => providers::antigravity::login(&http, open, rx).await,
        _ => Err("Для этого сервиса нужен API-ключ".into()),
    };
    state(&app).login_cancel.lock().unwrap().take();
    if result.is_ok() {
        show_main(&app, None);
    }
    insert_account(&app, result?)
}

#[tauri::command]
fn cancel_login(app: AppHandle) {
    if let Some(tx) = state(&app).login_cancel.lock().unwrap().take() {
        let _ = tx.send(());
    }
}

#[tauri::command]
async fn import_cli(app: AppHandle, kind: Kind) -> Res<String> {
    let http = state(&app).http.clone();
    let acc = match kind {
        Kind::ChatGpt => providers::chatgpt::import_cli()?,
        Kind::Claude => providers::claude::import_cli(&http).await?,
        _ => return Err("Импорт доступен только для Codex CLI и Claude Code".into()),
    };
    insert_account(&app, acc)
}

#[tauri::command]
fn remove_account(app: AppHandle, id: String) -> Res<()> {
    let st = state(&app);
    st.accounts.lock().unwrap().retain(|a| a.id != id);
    st.snapshots.lock().unwrap().remove(&id);
    st.history.lock().unwrap().remove(&id);
    st.save_accounts()?;
    publish(&app);
    Ok(())
}

#[tauri::command]
fn rename_account(app: AppHandle, id: String, label: String) -> Res<()> {
    let label = label.trim().to_owned();
    if label.is_empty() {
        return Err("Название не может быть пустым".into());
    }
    let st = state(&app);
    if let Some(a) = st.accounts.lock().unwrap().iter_mut().find(|a| a.id == id) {
        a.label = label;
    }
    st.save_accounts()?;
    publish(&app);
    Ok(())
}

#[tauri::command]
fn move_account(app: AppHandle, id: String, delta: i32) -> Res<()> {
    let st = state(&app);
    {
        let mut accounts = st.accounts.lock().unwrap();
        let Some(i) = accounts.iter().position(|a| a.id == id) else {
            return Ok(());
        };
        let j = (i as i32 + delta).clamp(0, accounts.len() as i32 - 1) as usize;
        let acc = accounts.remove(i);
        accounts.insert(j, acc);
    }
    st.save_accounts()?;
    publish(&app);
    Ok(())
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Res<()> {
    let st = state(&app);
    let mut s = settings;
    s.refresh_secs = s.refresh_secs.clamp(60, 3600);
    s.tint = s.tint.clamp(0.2, 0.95);
    let (effect_changed, interval_changed) = {
        let mut cur = st.settings.lock().unwrap();
        let r = (cur.effect != s.effect, cur.refresh_secs != s.refresh_secs);
        *cur = s.clone();
        r
    };
    store::save_settings(&st.dir, &s)?;
    if effect_changed {
        apply_styles(&app);
    }
    // Re-evaluate thresholds on cached data right away.
    {
        let mut snaps = st.snapshots.lock().unwrap();
        for snap in snaps.values_mut() {
            if snap.updated_at > 0 {
                providers::evaluate(snap, &s);
            }
        }
    }
    let _ = app.emit("settings", &s);
    publish(&app);
    if interval_changed {
        st.wake.notify_one();
    }
    Ok(())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Res<bool> {
    set_autostart_inner(&app, enabled)
}

#[tauri::command]
fn hide_main(app: AppHandle) {
    if let Some(w) = ui::window(&app, "main") {
        let _ = w.hide();
    }
}

#[tauri::command]
fn set_pinned(app: AppHandle, pinned: bool) {
    state(&app).ui.lock().unwrap().pinned = pinned;
    if let Some(w) = ui::window(&app, "main") {
        let _ = w.set_always_on_top(pinned);
    }
}

#[tauri::command]
fn open_main(app: AppHandle, tab: Option<String>) {
    show_main(&app, tab);
}

#[tauri::command]
fn popup_hover(app: AppHandle, inside: bool) {
    state(&app).ui.lock().unwrap().popup_hover = inside;
    if !inside {
        schedule_popup_hide(app);
    }
}

#[tauri::command]
fn popup_fit(app: AppHandle, height: f64) {
    let Some(win) = ui::window(&app, "popup") else { return };
    let h = height.clamp(80.0, 640.0).ceil();
    let _ = win.set_size(LogicalSize::new(POPUP_WIDTH, h));
    if win.is_visible().unwrap_or(false) {
        let rect = state(&app).ui.lock().unwrap().tray_rect;
        ui::place_near_tray(&app, &win, rect);
    }
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Res<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Разрешены только http(s)-ссылки".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_data_dir(app: AppHandle) -> Res<()> {
    let dir = state(&app).dir.display().to_string();
    app.opener()
        .open_path(dir, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

// ---- Entry point ----------------------------------------------------------------------

/// Release builds have no console and abort on panic, so record panics to a file.
fn install_crash_log(dir: &std::path::Path) {
    let path = dir.join("crash.log");
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(
                f,
                "[{}] OneMonitor {} panic: {info}\n{}",
                chrono::Utc::now().to_rfc3339(),
                env!("CARGO_PKG_VERSION"),
                std::backtrace::Backtrace::force_capture()
            );
        }
    }));
}

pub fn run() {
    let dir = store::data_dir();
    install_crash_log(&dir);
    let settings = store::load_settings(&dir);
    let accounts = store::load_accounts(&dir);
    let started_by_autostart = std::env::args().any(|a| a == "--autostart");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app, None);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_opener::init())
        .manage(AutostartItem(Mutex::new(None)))
        .manage(AppState {
            dir,
            settings: Mutex::new(settings),
            accounts: Mutex::new(accounts),
            snapshots: Mutex::new(HashMap::new()),
            history: Mutex::new(HashMap::new()),
            http: providers::client(),
            wake: Notify::new(),
            refreshing: AtomicBool::new(false),
            login_cancel: Mutex::new(None),
            ui: Mutex::new(UiState::default()),
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            refresh_now,
            add_key_account,
            login,
            cancel_login,
            import_cli,
            remove_account,
            rename_account,
            move_account,
            save_settings,
            set_autostart,
            hide_main,
            set_pinned,
            open_main,
            popup_hover,
            popup_fit,
            open_url,
            open_data_dir,
            quit,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            apply_styles(&handle);
            build_tray(&handle)?;
            spawn_scheduler(handle.clone());

            let no_accounts = state(&handle).accounts.lock().unwrap().is_empty();
            if !started_by_autostart || no_accounts {
                show_main(&handle, None);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            let app = window.app_handle();
            match (window.label(), event) {
                ("main", WindowEvent::CloseRequested { api, .. }) => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                ("main", WindowEvent::Focused(false)) => {
                    let st = state(app);
                    let hide = st.settings.lock().unwrap().hide_on_blur
                        && !st.ui.lock().unwrap().pinned
                        && st.login_cancel.lock().unwrap().is_none();
                    if hide {
                        st.ui.lock().unwrap().last_blur_hide = Some(Instant::now());
                        let _ = window.hide();
                    }
                }
                ("popup", WindowEvent::CloseRequested { api, .. }) => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building OneMonitor")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                // Closing windows must not quit the tray app; only explicit exit does.
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
