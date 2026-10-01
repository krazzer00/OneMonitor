//! Self-update from GitHub Releases for the portable exe.
//!
//! A running exe cannot be overwritten on Windows, but it can be renamed:
//! the current file becomes `<name>.old`, the new one takes its place and the
//! app restarts. The `.old` file is removed on the next start.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::util::Res;

const LATEST_URL: &str = "https://api.github.com/repos/krazzer00/OneMonitor/releases/latest";
const ASSET: &str = "OneMonitor-portable-win-x64.zip";

#[derive(Serialize, Clone, Debug)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// API URL of the asset (served with `Accept: application/octet-stream`).
    #[serde(skip)]
    pub asset_url: String,
    /// Public download link, used if the API download fails.
    #[serde(skip)]
    pub download_url: String,
    #[serde(skip)]
    pub sha256: Option<String>,
}

pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

pub async fn latest(http: &Client) -> Res<Release> {
    let resp = crate::providers::send(
        http.get(LATEST_URL)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28"),
    )
    .await?;
    let v = resp.json()?;
    let tag = v
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or("В ответе GitHub нет tag_name")?;
    let asset = v
        .get("assets")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|x| x.get("name").and_then(Value::as_str) == Some(ASSET))
        })
        .ok_or("В релизе нет архива для Windows")?;
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_owned(),
        url: v
            .get("html_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        asset_url: asset
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        download_url: asset
            .get("browser_download_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        sha256: asset
            .get("digest")
            .and_then(Value::as_str)
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_lowercase),
    })
}

/// True when running from a cargo `target` dir: never self-replace there.
pub fn is_dev_build() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|d| d.components().any(|c| c.as_os_str() == "target"))
        .unwrap_or(true)
}

fn old_path(exe: &Path) -> PathBuf {
    exe.with_extension("old")
}

/// Removes the previous exe left over by an update (best effort).
pub fn cleanup_old() {
    if let Ok(exe) = std::env::current_exe() {
        let old = old_path(&exe);
        if old.exists() {
            for _ in 0..10 {
                if std::fs::remove_file(&old).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

fn extract_exe(zip_bytes: &[u8]) -> Res<Vec<u8>> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| e.to_string())?;
        if f.name().to_lowercase().ends_with(".exe") {
            let mut out = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut out).map_err(|e| e.to_string())?;
            return Ok(out);
        }
    }
    Err("В архиве нет exe".into())
}

/// Downloads the release archive, checks its SHA-256 against the digest GitHub
/// publishes for the asset and returns the exe inside it.
pub async fn download_verified(http: &Client, rel: &Release) -> Res<Vec<u8>> {
    let mut last = "нет ссылки на архив".to_owned();
    let mut bytes = None;
    for (url, accept) in [
        (&rel.asset_url, "application/octet-stream"),
        (&rel.download_url, "*/*"),
    ] {
        if url.is_empty() {
            continue;
        }
        let resp = http
            .get(url)
            .header("Accept", accept)
            .timeout(Duration::from_secs(180))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(b) => {
                    bytes = Some(b);
                    break;
                }
                Err(e) => last = crate::util::net_error(e),
            },
            Ok(r) => last = format!("HTTP {}", r.status()),
            Err(e) => last = crate::util::net_error(e),
        }
    }
    let bytes = bytes.ok_or_else(|| format!("Не удалось скачать обновление: {last}"))?;
    if let Some(expected) = &rel.sha256 {
        let actual = hex(&Sha256::digest(&bytes));
        if &actual != expected {
            return Err("Контрольная сумма обновления не совпала — установка отменена".into());
        }
    }
    let exe_bytes = extract_exe(&bytes)?;
    if exe_bytes.len() < 1024 * 1024 || !exe_bytes.starts_with(b"MZ") {
        return Err("Архив обновления повреждён".into());
    }
    Ok(exe_bytes)
}

/// Downloads the release, verifies it and swaps the exe on disk.
/// The caller restarts the app afterwards.
pub async fn install(http: &Client, rel: &Release) -> Res<()> {
    if is_dev_build() {
        return Err("Обновление недоступно для сборки из исходников".into());
    }
    let exe_bytes = download_verified(http, rel).await?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    swap_exe(&exe, &exe_bytes)
}

/// Puts `bytes` in place of `exe`, keeping the previous file as `<exe>.old`.
fn swap_exe(exe: &Path, bytes: &[u8]) -> Res<()> {
    let new = exe.with_extension("new");
    let old = old_path(exe);
    std::fs::write(&new, bytes).map_err(|e| format!("{}: {e}", new.display()))?;
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("Не удалось заменить exe: {e}"));
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        // roll back so the app keeps working
        let _ = std::fs::rename(&old, exe);
        return Err(format!("Не удалось заменить exe: {e}"));
    }
    Ok(())
}

/// Starts the (new) exe, which waits for this process to exit first.
pub fn spawn_restarted(version: &str) -> Res<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .arg(format!("--wait-pid={}", std::process::id()))
        .arg(format!("--updated={version}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Called first thing on start: waits until the previous instance (the one
/// that installed the update) has exited, so single-instance does not bounce us.
pub fn wait_for_previous_instance() {
    let Some(pid) = std::env::args()
        .find_map(|a| a.strip_prefix("--wait-pid=").and_then(|p| p.parse::<u32>().ok()))
    else {
        return;
    };
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, 15_000);
            CloseHandle(h);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        std::thread::sleep(Duration::from_millis(800));
    }
}

/// `--updated=<version>` passed by the previous instance after an update.
pub fn updated_to() -> Option<String> {
    std::env::args().find_map(|a| a.strip_prefix("--updated=").map(str::to_owned))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.1.5", "0.1.4"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.4", "0.1.4"));
        assert!(!is_newer("0.1.3", "0.1.4"));
        assert!(!is_newer("garbage", "0.1.4"));
        assert_eq!(parse_version("v1.2.3-beta.1"), Some((1, 2, 3)));
    }

    #[test]
    fn swaps_exe_and_keeps_the_old_one() {
        let dir = std::env::temp_dir().join(format!("onemonitor-upd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("OneMonitor.exe");
        std::fs::write(&exe, b"old").unwrap();
        swap_exe(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"old");
        assert!(!exe.with_extension("new").exists());
        // a second update replaces the stale .old
        swap_exe(&exe, b"newer").unwrap();
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_exe_from_zip() {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file::<_, ()>("OneMonitor.exe", Default::default()).unwrap();
            z.write_all(b"MZpayload").unwrap();
            z.finish().unwrap();
        }
        assert_eq!(extract_exe(buf.get_ref()).unwrap(), b"MZpayload");
    }
}
