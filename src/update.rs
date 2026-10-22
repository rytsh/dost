//! Updates from GitHub Releases: checks for a newer version and, if allowed, downloads it,
//! verifies its SHA-256 and swaps it in. Runs on a background thread; the app shows the result.
//!
//! Release assets (made by CI): `dost-windows.exe`, `Dost-macos.zip` (Dost.app),
//! `dost-linux-x86_64`, each with a `.sha256` file next to it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/rytsh/dost/releases/latest";
const TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(windows)]
const ASSET: &str = "dost-windows.exe";
#[cfg(target_os = "macos")]
const ASSET: &str = "Dost-macos.zip";
#[cfg(target_os = "linux")]
const ASSET: &str = "dost-linux-x86_64";
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
const ASSET: &str = "";

/// What a check found.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    UpToDate,
    /// A newer version; `ready`: installed, takes effect on restart.
    Newer {
        version: String,
        url: String,
        ready: bool,
    },
    Failed(String),
}

/// Checks in the background. `install`: also download and swap in the new version.
pub fn check(install: bool) -> Receiver<Found> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run(install).unwrap_or_else(Found::Failed));
    });
    rx
}

fn run(install: bool) -> Result<Found, String> {
    let agent = crate::watch::http_agent(TIMEOUT);
    // DOST_UPDATE_URL: another releases API (a mirror, or a local one for testing).
    let latest = std::env::var("DOST_UPDATE_URL").unwrap_or_else(|_| LATEST.into());
    let mut resp = agent
        .get(&latest)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "dost")
        .call()
        .map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("GitHub HTTP {}", resp.status().as_u16()));
    }
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or("");
    let page = v["html_url"].as_str().unwrap_or("").to_string();
    if !newer(tag, env!("CARGO_PKG_VERSION")) {
        return Ok(Found::UpToDate);
    }
    let version = tag.trim_start_matches('v').to_string();
    let asset = |name: &str| {
        v["assets"].as_array().and_then(|a| {
            a.iter()
                .find(|x| x["name"].as_str() == Some(name))
                .and_then(|x| x["browser_download_url"].as_str())
                .map(String::from)
        })
    };
    let ready = install
        && match (asset(ASSET), asset(&format!("{ASSET}.sha256"))) {
            (Some(bin), Some(sum)) => {
                let data = download(&bin)?;
                let want = String::from_utf8_lossy(&download(&sum)?)
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
                let got = hex(&Sha256::digest(&data));
                if want != got {
                    return Err("the download's checksum does not match".into());
                }
                install_update(&data)?;
                true
            }
            _ => false,
        };
    Ok(Found::Newer {
        version,
        url: page,
        ready,
    })
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let agent = crate::watch::http_agent(DOWNLOAD_TIMEOUT);
    let mut resp = agent
        .get(url)
        .header("User-Agent", "dost")
        .call()
        .map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("download HTTP {}", resp.status().as_u16()));
    }
    resp.body_mut()
        .with_config()
        .limit(200 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// "v0.2.0" > "0.1.9"? Compares dotted numbers; pre-release suffixes count as older.
pub fn newer(tag: &str, current: &str) -> bool {
    let parse = |s: &str| -> Option<(Vec<u64>, bool)> {
        let s = s.trim().trim_start_matches('v');
        let (nums, pre) = match s.split_once('-') {
            Some((n, _)) => (n, true),
            None => (s, false),
        };
        let v: Option<Vec<u64>> = nums.split('.').map(|p| p.parse().ok()).collect();
        v.map(|v| (v, pre))
    };
    let (Some((a, a_pre)), Some((b, b_pre))) = (parse(tag), parse(current)) else {
        return false;
    };
    let n = a.len().max(b.len());
    let pad = |v: &[u64]| {
        (0..n)
            .map(|i| v.get(i).copied().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    match pad(&a).cmp(&pad(&b)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // Same numbers: a release beats a pre-release.
        std::cmp::Ordering::Equal => b_pre && !a_pre,
    }
}

/// Windows: the running exe can be renamed but not overwritten, so it moves aside.
#[cfg(windows)]
fn install_update(data: &[u8]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = exe.with_extension("new.exe");
    let old = exe.with_extension("old.exe");
    std::fs::write(&new, data).map_err(|e| format!("cannot write the update: {e}"))?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("cannot replace dost.exe: {e}"))?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("cannot replace dost.exe: {e}"));
    }
    Ok(())
}

/// macOS: only when running from an app bundle; the new Dost.app replaces it.
#[cfg(target_os = "macos")]
fn install_update(data: &[u8]) -> Result<(), String> {
    let app = app_bundle().ok_or("not running from Dost.app (built from source?)")?;
    let parent = app.parent().ok_or("no folder")?;
    let tmp = parent.join(".dost-update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("cannot write next to Dost.app: {e}"))?;
    let zip = tmp.join("Dost.zip");
    std::fs::write(&zip, data).map_err(|e| e.to_string())?;
    // ditto keeps the bundle's signature and attributes intact.
    let ok = std::process::Command::new("ditto")
        .args(["-x", "-k"])
        .arg(&zip)
        .arg(&tmp)
        .status()
        .is_ok_and(|s| s.success());
    let fresh = tmp.join("Dost.app");
    if !ok || !fresh.join("Contents/MacOS/dost").exists() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("the update could not be unpacked".into());
    }
    let old = tmp.join("Dost-old.app");
    std::fs::rename(&app, &old).map_err(|e| format!("cannot replace Dost.app: {e}"))?;
    if let Err(e) = std::fs::rename(&fresh, &app) {
        let _ = std::fs::rename(&old, &app);
        return Err(format!("cannot replace Dost.app: {e}"));
    }
    // Downloaded by us (not a browser), so no quarantine; clear it anyway.
    let _ = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&app)
        .status();
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// Linux: the new binary is written next to the running one and renamed over it (the running
/// process keeps the old file open).
#[cfg(target_os = "linux")]
fn install_update(data: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let exe = linux_exe().ok_or("cannot find the dost executable")?;
    let new = exe.with_extension("new");
    std::fs::write(&new, data).map_err(|e| format!("cannot write the update: {e}"))?;
    let done = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
        .and_then(|()| std::fs::rename(&new, &exe));
    if let Err(e) = done {
        let _ = std::fs::remove_file(&new);
        return Err(format!("cannot replace {}: {e}", exe.display()));
    }
    Ok(())
}

/// The running executable's path; after it was replaced, Linux reports it as "(deleted)".
#[cfg(target_os = "linux")]
fn linux_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let s = exe.to_string_lossy();
    Some(match s.strip_suffix(" (deleted)") {
        Some(p) => p.into(),
        None => exe,
    })
}

/// Linux: the binary and its folder can be written (not a package manager's /usr/bin).
#[cfg(target_os = "linux")]
fn linux_writable() -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn access(path: *const std::ffi::c_char, mode: i32) -> i32;
    }
    const W_OK: i32 = 2;
    let ok = |p: &Path| {
        CString::new(p.as_os_str().as_bytes())
            .is_ok_and(|c| unsafe { access(c.as_ptr(), W_OK) } == 0)
    };
    linux_exe().is_some_and(|exe| ok(&exe) && exe.parent().is_some_and(ok))
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn install_update(_: &[u8]) -> Result<(), String> {
    Err("updates are for Windows, macOS and Linux".into())
}

/// The `.app` folder this binary runs from, if any.
pub fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension().is_some_and(|e| e == "app") && exe.parent()?.ends_with("Contents/MacOS"))
        .then(|| app.to_path_buf())
}

/// Whether this copy can update itself (a release build in its usual place).
pub fn can_install() -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    if cfg!(target_os = "macos") {
        return app_bundle().is_some();
    }
    #[cfg(target_os = "linux")]
    return linux_writable();
    #[cfg(not(target_os = "linux"))]
    cfg!(windows)
}

/// Starts the (new) app again and returns; the caller quits.
pub fn restart() {
    #[cfg(target_os = "macos")]
    if let Some(app) = app_bundle() {
        // -n: a new instance even though this one is still running.
        let _ = std::process::Command::new("open")
            .arg("-n")
            .arg(&app)
            .spawn();
        return;
    }
    #[cfg(target_os = "linux")]
    let exe = linux_exe();
    #[cfg(not(target_os = "linux"))]
    let exe = std::env::current_exe().ok();
    if let Some(exe) = exe {
        let _ = std::process::Command::new(exe).spawn();
    }
}

/// Removes what an update left behind (Windows: the old exe once it is no longer running).
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let old: &Path = &exe.with_extension("old.exe");
        if cfg!(windows) && old.exists() {
            let _ = std::fs::remove_file(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("v1.0", "0.9.9"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("v0.1.0", "0.2.0"));
        assert!(newer("v0.2.0", "0.2.0-beta"));
        assert!(!newer("v0.2.0-beta", "0.2.0"));
        assert!(!newer("latest", "0.1.0"));
    }

    #[test]
    fn checksum_hex() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
