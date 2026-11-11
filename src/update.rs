//! Self-update from GitHub Releases.
//!
//! `check` asks the GitHub API for the latest release. `install` downloads its `veyra.exe`,
//! checks it against the SHA-256 GitHub publishes for the asset, and swaps it in next to the
//! running exe: a running exe cannot be overwritten on Windows, but it can be renamed, so the
//! current one becomes `veyra.old.exe` (deleted by the next start) and the new one takes its
//! name. The caller then starts the new exe with `--after-update` and quits.

use std::path::{Path, PathBuf};

use windows::core::*;
use windows::Foundation::Uri;
use windows::Security::Cryptography::Core::{HashAlgorithmNames, HashAlgorithmProvider};
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Storage::Streams::{DataReader, IBuffer};
use windows::Web::Http::{HttpClient, HttpStatusCode};
use windows::Win32::System::WinRT::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::events::{post, Event};
use crate::util::wide;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "AhmadUlyani/Veyra";
pub const RELEASES_PAGE: &str = "https://github.com/AhmadUlyani/Veyra/releases/latest";

#[derive(Clone, Debug)]
pub struct Release {
    /// "1.5.0" (without the leading v)
    pub version: String,
    pub url: String,
    /// lowercase hex, when GitHub reports it
    pub sha256: Option<String>,
    pub size: u64,
}

/// "v1.4.2" / "1.4.2" -> (1, 4, 2); missing parts count as 0.
fn parse_ver(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let s = s.split(['-', '+']).next()?;
    let mut it = s.split('.').map(|p| p.parse::<u32>());
    let a = it.next()?.ok()?;
    let b = it.next().unwrap_or(Ok(0)).ok()?;
    let c = it.next().unwrap_or(Ok(0)).ok()?;
    Some((a, b, c))
}

/// The running version; testing aid: VEYRA_UPDATE_FROM=1.0.0 pretends to be an older one.
pub fn current() -> String {
    std::env::var("VEYRA_UPDATE_FROM").unwrap_or_else(|_| CURRENT.to_string())
}

/// Whether `tag` is a newer version than `cur`.
pub fn newer(tag: &str, cur: &str) -> bool {
    match (parse_ver(tag), parse_ver(cur)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn client() -> Option<HttpClient> {
    let c = HttpClient::new().ok()?;
    // the GitHub API refuses requests without a User-Agent
    let _ = c.DefaultRequestHeaders().ok()?.UserAgent().ok()?.TryParseAdd(&HSTRING::from(format!("Veyra-Updater/{CURRENT}")));
    Some(c)
}

fn parse_release(json: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let tag = v["tag_name"].as_str()?;
    parse_ver(tag)?;
    let assets = v["assets"].as_array()?;
    // the plain veyra.exe, else any .exe with veyra in its name
    let asset = assets
        .iter()
        .find(|a| a["name"].as_str().map(|n| n.eq_ignore_ascii_case("veyra.exe")).unwrap_or(false))
        .or_else(|| {
            assets.iter().find(|a| {
                let n = a["name"].as_str().unwrap_or("").to_ascii_lowercase();
                n.ends_with(".exe") && n.contains("veyra")
            })
        })?;
    let url = asset["browser_download_url"].as_str()?.to_string();
    // only from this repository's own releases
    if !url.starts_with(&format!("https://github.com/{REPO}/releases/download/")) {
        return None;
    }
    let sha256 = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|h| h.to_ascii_lowercase());
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url,
        sha256,
        size: asset["size"].as_u64().unwrap_or(0),
    })
}

fn fetch_latest() -> std::result::Result<Release, ()> {
    let c = client().ok_or(())?;
    // a query string that changes every time keeps the Windows HTTP cache from answering
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let uri = Uri::CreateUri(&HSTRING::from(format!("https://api.github.com/repos/{REPO}/releases/latest?t={t}"))).map_err(|_| ())?;
    let resp = c.GetAsync(&uri).and_then(|op| op.get()).map_err(|_| ())?;
    if resp.StatusCode().map_err(|_| ())? != HttpStatusCode::Ok {
        return Err(());
    }
    let body = resp.Content().and_then(|b| b.ReadAsStringAsync()).and_then(|op| op.get()).map_err(|_| ())?;
    parse_release(&body.to_string()).ok_or(())
}

/// Ask GitHub for the latest release in the background. Answers with `Event::UpdateInfo`:
/// `Ok(Some(r))` newer release, `Ok(None)` up to date, `Err(())` could not check.
pub fn check(manual: bool) {
    std::thread::spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let r = fetch_latest().map(|r| newer(&r.version, &current()).then_some(r));
        post(Event::UpdateInfo(r, manual));
    });
}

fn sha256_hex(buf: &IBuffer) -> Option<String> {
    let alg = HashAlgorithmProvider::OpenAlgorithm(&HashAlgorithmNames::Sha256().ok()?).ok()?;
    let h = alg.HashData(buf).ok()?;
    Some(CryptographicBuffer::EncodeToHexString(&h).ok()?.to_string().to_ascii_lowercase())
}

fn download(r: &Release) -> std::result::Result<Vec<u8>, String> {
    let c = client().ok_or("No connection")?;
    let uri = Uri::CreateUri(&HSTRING::from(r.url.as_str())).map_err(|_| "Bad download link")?;
    let resp = c.GetAsync(&uri).and_then(|op| op.get()).map_err(|_| "Download failed")?;
    if resp.StatusCode().map_err(|_| "Download failed")? != HttpStatusCode::Ok {
        return Err("Download failed".into());
    }
    let buf = resp.Content().and_then(|b| b.ReadAsBufferAsync()).and_then(|op| op.get()).map_err(|_| "Download failed")?;
    if let Some(want) = &r.sha256 {
        if sha256_hex(&buf).as_deref() != Some(want.as_str()) {
            return Err("The download is damaged".into());
        }
    }
    let n = buf.Length().map_err(|_| "Download failed")? as usize;
    let mut v = vec![0u8; n];
    let rd = DataReader::FromBuffer(&buf).map_err(|_| "Download failed")?;
    rd.ReadBytes(&mut v).map_err(|_| "Download failed")?;
    // a Windows program, of a sensible size
    if n < 200_000 || n > 50_000_000 || !v.starts_with(b"MZ") || (r.size > 0 && n as u64 != r.size) {
        return Err("The download is damaged".into());
    }
    Ok(v)
}

fn old_path(exe: &Path) -> PathBuf {
    exe.with_file_name("veyra.old.exe")
}

/// Put `bytes` in place of the running exe (see the module docs).
fn swap_in(exe: &Path, bytes: &[u8]) -> std::result::Result<(), String> {
    let new = exe.with_file_name("veyra.new.exe");
    let old = old_path(exe);
    std::fs::write(&new, bytes).map_err(|_| "Can't write to Veyra's folder")?;
    let _ = std::fs::remove_file(&old);
    if std::fs::rename(exe, &old).is_err() {
        let _ = std::fs::remove_file(&new);
        return Err("Can't replace veyra.exe".into());
    }
    if std::fs::rename(&new, exe).is_err() {
        // put the old one back
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err("Can't replace veyra.exe".into());
    }
    Ok(())
}

/// Download and swap in `r` in the background. Answers with `Event::UpdateDone`.
pub fn install(r: Release) {
    std::thread::spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let res = (|| {
            let exe = std::env::current_exe().map_err(|_| "Can't find veyra.exe".to_string())?;
            let bytes = download(&r)?;
            swap_in(&exe, &bytes)
        })();
        post(Event::UpdateDone(res));
    });
}

/// Start the freshly installed exe; it waits for this copy to exit.
pub fn relaunch() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    // current_exe may still name the renamed file on some systems: use the original name
    let exe = if exe.file_name().map(|n| n.eq_ignore_ascii_case("veyra.old.exe")).unwrap_or(false) {
        exe.with_file_name("veyra.exe")
    } else {
        exe
    };
    // testing aids must not carry over (the new copy would update itself again)
    std::process::Command::new(exe)
        .arg("--after-update")
        .env_remove("VEYRA_DEMO")
        .env_remove("VEYRA_UPDATE_FROM")
        .env_remove("VEYRA_DUMP")
        .spawn()
        .is_ok()
}

/// Remove what an earlier update left behind (the old exe may still be closing).
pub fn cleanup() {
    let Ok(exe) = std::env::current_exe() else { return };
    let old = old_path(&exe);
    let new = exe.with_file_name("veyra.new.exe");
    if !old.exists() && !new.exists() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..20 {
            let _ = std::fs::remove_file(&new);
            if std::fs::remove_file(&old).is_ok() || !old.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

pub fn open_releases_page() {
    let url = wide(RELEASES_PAGE);
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(url.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("v1.5.0", "1.4.2"));
        assert!(newer("1.4.10", "1.4.2"));
        assert!(newer("v2", "1.9.9"));
        assert!(!newer("v1.4.2", "1.4.2"));
        assert!(!newer("v1.4.1", "1.4.2"));
        assert!(!newer("nightly", "1.4.2"));
        assert!(!newer("v1.5.0-beta", "1.5.0"));
    }

    #[test]
    fn release_json() {
        let j = r#"{"tag_name":"v1.5.0","draft":false,"prerelease":false,"assets":[
            {"name":"notes.txt","browser_download_url":"https://github.com/AhmadUlyani/Veyra/releases/download/v1.5.0/notes.txt"},
            {"name":"veyra.exe","size":1172480,"digest":"sha256:ABCD","browser_download_url":"https://github.com/AhmadUlyani/Veyra/releases/download/v1.5.0/veyra.exe"}]}"#;
        let r = parse_release(j).unwrap();
        assert_eq!(r.version, "1.5.0");
        assert!(r.url.ends_with("/veyra.exe"));
        assert_eq!(r.sha256.as_deref(), Some("abcd"));
        assert_eq!(r.size, 1172480);
        // a download from anywhere else is refused
        let bad = j.replace("https://github.com/AhmadUlyani/Veyra/", "https://example.com/");
        assert!(parse_release(&bad).is_none());
        // no exe at all
        assert!(parse_release(r#"{"tag_name":"v1.5.0","assets":[]}"#).is_none());
    }
}
