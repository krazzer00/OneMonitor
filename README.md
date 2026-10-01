# OneMonitor

Портативное приложение для Windows, которое живёт в трее и следит за:

- **OneProvider** — доступность шлюза (`api.oneprovider.dev`), наблюдения по семействам моделей со [страницы статуса](https://oneprovider.dev/status) и баланс ключа;
- **OpenRouter** — доступность API, задержка, кредиты и лимит ключа;
- **ChatGPT** — лимиты подписки (5-часовое окно и неделя, как в Codex `/status`) и время их сброса;
- **Claude** — лимиты Pro / Max (5 часов, неделя, Opus / Sonnet) и время сброса;
- **Antigravity** — квоты моделей (Claude, Gemini Pro / Flash и т. д.) и время сброса.

Интерфейс в тёмной полупрозрачной теме Grey Glass: окно без рамки со скруглёнными углами и эффектом Acrylic / Blur / Mica в Windows 11. Каждый аккаунт — отдельная вкладка, их можно листать влево и вправо: стрелками, колесом мыши, свайпом или клавишами ←/→.

## Возможности

| | |
|---|---|
| **Трей** | Цвет иконки показывает общее состояние: зелёный — всё в порядке, жёлтый — нужно внимание (мало денег или лимит почти израсходован), красный — сбой или ошибка, серый — нет данных. |
| **Сводка при наведении** | Если навести курсор на иконку, появится мини-окно со всеми аккаунтами: баланс, сколько осталось лимита и когда сброс. Клик по строке открывает вкладку этого аккаунта. |
| **Панель** | Открывается кликом по иконке рядом с треем. Её можно перетащить за заголовок или закрепить поверх других окон (📌). |
| **Вход через аккаунт** | ChatGPT, Claude и Antigravity подключаются через OAuth в браузере. Аккаунтов одного сервиса может быть несколько. |
| **Импорт из CLI** | Можно взять уже готовую сессию Codex CLI (`~/.codex/auth.json`) или Claude Code (`~/.claude/.credentials.json`). Обновлённые токены записываются обратно в тот же файл, так что CLI продолжит работать. |
| **Автозапуск** | Включается в настройках или через меню трея (ключ `HKCU\…\Run`). При автозапуске приложение стартует свёрнутым в трей. |
| **Портативность** | Один `.exe`. Настройки и аккаунты лежат в папке `data` рядом с ним. Если туда нельзя писать, используется `%APPDATA%\OneMonitor`. |
| **Безопасность** | Ключи и токены шифруются Windows DPAPI и привязаны к текущему пользователю Windows. Никуда, кроме API самих сервисов, они не отправляются. |

## Установка

1. Скачайте `OneMonitor.exe`: из артефакта сборки в [Actions](../../actions) или из релиза (`OneMonitor-portable-win-x64.zip`).
2. Положите его в любую папку, например `C:\Tools\OneMonitor\`, и запустите.
3. Нажмите **+** и добавьте ключи или аккаунты. В ⚙ включите «Запуск вместе с Windows».

Нужна Windows 10 или 11 с WebView2 Runtime. В Windows 11 и в современных Windows 10 он уже установлен; если нет, его можно скачать [у Microsoft](https://developer.microsoft.com/microsoft-edge/webview2/).

## Откуда берутся данные

| Сервис | Доступность | Баланс или лимиты |
|---|---|---|
| OneProvider | `GET /v1/models` (latency, код ответа) и `GET /_model_status.json` | `GET /v1/dashboard/balance` ([документация](https://oneprovider.dev/docs/llms.txt), §10.1). Эндпоинт у OneProvider переезжал и пропадал, поэтому приложение пробует несколько адресов (`api.`, `dashboard.`, `/v1/usage`), а при сбое показывает последний известный баланс с пометкой «устарел». Расход по дням, топ моделей и прогноз — из `GET /v1/usage` |
| OpenRouter | `GET /api/v1/key` (latency, код ответа) | `GET /api/v1/credits`, если недоступно — лимит ключа |
| ChatGPT | — | `chatgpt.com/backend-api/wham/usage` (тот же эндпоинт, что у `/status` в Codex CLI) |
| Claude | — | `api.anthropic.com/api/oauth/usage` (данные `/usage` из Claude Code) |
| Antigravity | — | `cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels` |

Без ключа OneProvider и OpenRouter отслеживают только доступность API.

> Эндпоинты лимитов ChatGPT, Claude и Antigravity внутренние и официально не документированы. Для входа используются те же OAuth-клиенты, что и в официальных CLI. Если сервис изменит API, соответствующая вкладка покажет ошибку, остальные продолжат работать.

## Сборка из исходников

```powershell
# Windows: Rust (stable) + MSVC build tools
cd src-tauri
cargo build --release
# результат: src-tauri\target\release\onemonitor.exe
```

Кросс-сборка из Linux:

```bash
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin      # нужны clang, lld, llvm-rc
cd src-tauri
cargo xwin build --release --target x86_64-pc-windows-msvc
```

GitHub Actions (`.github/workflows/build.yml`) собирает portable-версию на каждый push, а на теги `v*` публикует релиз с zip-архивом.

Интерфейс можно посмотреть и без Tauri: откройте `src/` любым статическим сервером (например, `python -m http.server`). В браузере приложение запустится в демо-режиме с тестовыми данными.

### Структура

```
src/                  интерфейс (HTML/CSS/JS без сборщика)
  index.html          панель со вкладками-аккаунтами
  popup.html          сводка при наведении на трей
  js/shared.js        мост к Tauri, иконки, форматирование
  js/mock.js          демо-данные для просмотра в браузере
src-tauri/            Rust (Tauri 2)
  src/lib.rs          трей, окна, фоновый опрос, команды
  src/ui.rs           стекло, скругления, позиционирование у трея
  src/store.rs        портативное хранилище + DPAPI
  src/oauth.rs        PKCE + локальный callback-сервер
  src/providers/      OneProvider, OpenRouter, ChatGPT, Claude, Antigravity
scripts/gen_icons.py  генерация иконок (Pillow)
```

## Лицензия

MIT
