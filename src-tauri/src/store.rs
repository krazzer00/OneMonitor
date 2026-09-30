//! Portable persistence: settings and accounts live in a `data` folder next to the
//! executable (falls back to %APPDATA%\OneMonitor when that folder is not writable).
//! Secrets are encrypted with Windows DPAPI (bound to the current Windows user).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::{Account, Kind, Secret, Settings, Source};
use crate::util::{b64, unb64, Res};

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("data")))
    {
        if is_writable(&dir) {
            return dir;
        }
    }
    let dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("OneMonitor");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn is_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let ok = fs::write(&probe, b"ok").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// Writes a file atomically (write to temp + rename).
pub fn write_atomic(path: &Path, data: &[u8]) -> Res<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load_settings(dir: &Path) -> Settings {
    fs::read(dir.join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_settings(dir: &Path, s: &Settings) -> Res<()> {
    let data = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    write_atomic(&dir.join("settings.json"), &data)
}

#[derive(Serialize, Deserialize)]
struct StoredAccount {
    id: String,
    kind: Kind,
    label: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    source: Source,
    #[serde(default)]
    meta: Map<String, Value>,
    #[serde(default)]
    created_at: i64,
    /// "dpapi:<base64>" or "plain:<base64>"
    #[serde(default)]
    secret: String,
}

#[derive(Serialize, Deserialize, Default)]
struct AccountsFile {
    version: u32,
    accounts: Vec<StoredAccount>,
}

pub fn load_accounts(dir: &Path) -> Vec<Account> {
    let Ok(bytes) = fs::read(dir.join("accounts.json")) else {
        return vec![];
    };
    let file: AccountsFile = serde_json::from_slice(&bytes).unwrap_or_default();
    file.accounts
        .into_iter()
        .map(|a| Account {
            secret: decode_secret(&a.secret).unwrap_or_default(),
            id: a.id,
            kind: a.kind,
            label: a.label,
            email: a.email,
            source: a.source,
            meta: a.meta,
            created_at: a.created_at,
        })
        .collect()
}

pub fn save_accounts(dir: &Path, accounts: &[Account]) -> Res<()> {
    let file = AccountsFile {
        version: 1,
        accounts: accounts
            .iter()
            .map(|a| {
                Ok(StoredAccount {
                    id: a.id.clone(),
                    kind: a.kind,
                    label: a.label.clone(),
                    email: a.email.clone(),
                    source: a.source,
                    meta: a.meta.clone(),
                    created_at: a.created_at,
                    secret: encode_secret(&a.secret)?,
                })
            })
            .collect::<Res<Vec<_>>>()?,
    };
    let data = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    write_atomic(&dir.join("accounts.json"), &data)
}

fn encode_secret(s: &Secret) -> Res<String> {
    let json = serde_json::to_vec(s).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        Ok(format!("dpapi:{}", b64(&dpapi::protect(&json)?)))
    }
    #[cfg(not(windows))]
    {
        Ok(format!("plain:{}", b64(&json)))
    }
}

fn decode_secret(s: &str) -> Res<Secret> {
    let bytes = if let Some(rest) = s.strip_prefix("dpapi:") {
        #[cfg(windows)]
        {
            dpapi::unprotect(&unb64(rest)?)?
        }
        #[cfg(not(windows))]
        {
            let _ = rest;
            return Err("DPAPI is only available on Windows".into());
        }
    } else if let Some(rest) = s.strip_prefix("plain:") {
        unb64(rest)?
    } else {
        return Ok(Secret::default());
    };
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

#[cfg(windows)]
mod dpapi {
    use crate::util::Res;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    const ENTROPY: &[u8] = b"OneMonitor/secrets/v1";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        v
    }

    pub fn protect(data: &[u8]) -> Res<Vec<u8>> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        unsafe {
            if CryptProtectData(
                &input,
                null(),
                &entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            ) == 0
            {
                return Err("CryptProtectData failed".into());
            }
            Ok(take(out))
        }
    }

    pub fn unprotect(data: &[u8]) -> Res<Vec<u8>> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        unsafe {
            if CryptUnprotectData(
                &input,
                null_mut(),
                &entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            ) == 0
            {
                return Err("CryptUnprotectData failed (данные зашифрованы другим пользователем Windows)".into());
            }
            Ok(take(out))
        }
    }
}
