//! Shared OAuth helpers: PKCE and a tiny loopback HTTP server that catches the
//! browser redirect with the authorization code.

use std::collections::HashMap;
use std::time::Duration;

use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::util::{b64url, Res};

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Pkce {
    let mut bytes = [0u8; 48];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = b64url(&bytes);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

pub fn random_state() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    b64url(&bytes)
}

pub fn build_url(base: &str, params: &[(&str, &str)]) -> String {
    let mut url = url::Url::parse(base).expect("valid oauth base url");
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
    }
    url.to_string()
}

pub struct Loopback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    pub port: u16,
}

impl Loopback {
    /// Binds 127.0.0.1 (and ::1 when possible) on `port` (0 = random).
    pub async fn bind(port: u16) -> Res<Self> {
        let v4 = TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| {
            if port != 0 {
                format!(
                    "Порт {port} занят (возможно, запущен вход в Codex CLI). Закройте его и повторите. ({e})"
                )
            } else {
                e.to_string()
            }
        })?;
        let port = v4.local_addr().map_err(|e| e.to_string())?.port();
        let v6 = TcpListener::bind(("::1", port)).await.ok();
        Ok(Self { v4, v6, port })
    }

    /// Waits for `GET <path>?code=...&state=...`, validates the state and returns the code.
    pub async fn wait_code(
        self,
        path: &str,
        state: &str,
        mut cancel: oneshot::Receiver<()>,
    ) -> Res<String> {
        let deadline = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(deadline);
        loop {
            let stream = tokio::select! {
                r = self.v4.accept() => r.map(|(s, _)| s).map_err(|e| e.to_string())?,
                r = accept_opt(&self.v6) => r.map_err(|e| e.to_string())?,
                _ = &mut deadline => return Err("Время ожидания входа истекло".into()),
                _ = &mut cancel => return Err("Вход отменён".into()),
            };
            match handle(stream, path, state).await {
                Some(result) => return result,
                None => continue, // favicon or unrelated request
            }
        }
    }
}

async fn accept_opt(l: &Option<TcpListener>) -> std::io::Result<TcpStream> {
    match l {
        Some(l) => l.accept().await.map(|(s, _)| s),
        None => std::future::pending().await,
    }
}

async fn handle(mut stream: TcpStream, path: &str, state: &str) -> Option<Res<String>> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let target = head.lines().next()?.split_whitespace().nth(1)?.to_owned();
    let url = url::Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != path {
        let _ = respond(&mut stream, 404, "Not found").await;
        return None;
    }
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();

    let result = if let Some(err) = q.get("error") {
        let desc = q.get("error_description").cloned().unwrap_or_default();
        Err(format!("Авторизация отклонена: {err} {desc}").trim().to_owned())
    } else {
        match q.get("code") {
            None => Err("В ответе нет кода авторизации".into()),
            Some(code) => {
                // Anthropic may append "#state" to the code.
                let (code, st) = match code.split_once('#') {
                    Some((c, s)) => (c.to_owned(), Some(s.to_owned())),
                    None => (code.clone(), q.get("state").cloned()),
                };
                if st.as_deref() != Some(state) {
                    Err("Неверный параметр state — повторите вход".into())
                } else {
                    Ok(code)
                }
            }
        }
    };
    let page = match &result {
        Ok(_) => page("Вход выполнен", "Можно закрыть эту вкладку и вернуться в OneMonitor.", true),
        Err(e) => page("Не удалось войти", e, false),
    };
    let _ = respond(&mut stream, 200, &page).await;
    Some(result)
}

async fn respond(stream: &mut TcpStream, code: u16, body: &str) -> std::io::Result<()> {
    let reason = if code == 200 { "OK" } else { "Not Found" };
    let resp = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

fn page(title: &str, text: &str, ok: bool) -> String {
    let color = if ok { "#34d399" } else { "#f87171" };
    let icon = if ok { "✓" } else { "!" };
    let text = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><html lang="ru"><head><meta charset="utf-8"><title>OneMonitor</title>
<style>
html,body{{height:100%;margin:0}}
body{{display:flex;align-items:center;justify-content:center;background:radial-gradient(circle at 30% 20%,#2a2c35,#101115 70%);
font-family:"Segoe UI Variable Text","Segoe UI",system-ui,sans-serif;color:#e8e9ee}}
.card{{padding:36px 44px;border-radius:20px;background:rgba(255,255,255,.06);border:1px solid rgba(255,255,255,.1);
backdrop-filter:blur(20px);box-shadow:0 20px 60px rgba(0,0,0,.45);text-align:center;max-width:420px}}
.i{{width:56px;height:56px;border-radius:50%;margin:0 auto 18px;display:flex;align-items:center;justify-content:center;
font-size:28px;color:{color};background:{color}22;border:1px solid {color}55}}
h1{{font-size:20px;font-weight:600;margin:0 0 8px}}p{{margin:0;color:#a3a6b3;font-size:14px;line-height:1.5}}
</style></head><body><div class="card"><div class="i">{icon}</div><h1>{title}</h1><p>{text}</p></div></body></html>"#
    )
}
