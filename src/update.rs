//! Updates from GitHub Releases: checks for a newer version and, if allowed, downloads it,
//! verifies its SHA-256 and swaps it in. Runs on a background thread; the app shows the result.
//!
//! Release archives use `dost_<OS>_<arch>` names and one versioned checksum manifest.
//! Each platform selects the running binary's architecture; macOS archives contain Dost.app.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/rytsh/dost/releases/latest";
const TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
#[cfg(any(windows, target_os = "linux"))]
const MAX_EXECUTABLE: u64 = 200 << 20;

fn archive_name(os: &str, arch: &str) -> Option<String> {
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => return None,
    };
    match os {
        "windows" => Some(format!("dost_Windows_{arch}.zip")),
        "linux" => Some(format!("dost_Linux_{arch}.tar.gz")),
        "macos" => Some(format!("dost_Darwin_{arch}.tar.gz")),
        _ => None,
    }
}

/// Match the exact filename, not the first hash (which may belong to another platform).
fn checksum(manifest: &str, name: &str) -> Result<String, String> {
    let mut found = None;
    for line in manifest.lines() {
        let mut fields = line.split_whitespace();
        let (Some(hash), Some(file)) = (fields.next(), fields.next()) else {
            continue;
        };
        if file.trim_start_matches('*') != name {
            continue;
        }
        if fields.next().is_some()
            || hash.len() != 64
            || !hash.bytes().all(|c| c.is_ascii_hexdigit())
            || found.is_some()
        {
            return Err("invalid or duplicate update checksum".into());
        }
        found = Some(hash.to_ascii_lowercase());
    }
    found.ok_or_else(|| "the update archive is missing from the checksum manifest".into())
}

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
    let archive = archive_name(std::env::consts::OS, std::env::consts::ARCH);
    let ready = install
        && match (
            archive.as_deref().and_then(asset),
            asset(&format!("dost-{version}-checksums.txt")),
        ) {
            (Some(bin), Some(sum)) => {
                let data = download(&bin)?;
                let manifest = download(&sum)?;
                let want = checksum(
                    &String::from_utf8_lossy(&manifest),
                    archive.as_deref().unwrap(),
                )?;
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

/// Limit decompressed size as well as the download; archives never get a general unpack().
#[cfg(any(windows, target_os = "linux"))]
fn read_executable(reader: impl std::io::Read) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut data = Vec::new();
    reader
        .take(MAX_EXECUTABLE + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("cannot read the update executable: {e}"))?;
    if data.is_empty() || data.len() as u64 > MAX_EXECUTABLE {
        return Err("the update executable is empty or too large".into());
    }
    Ok(data)
}

#[cfg(windows)]
fn unpack_executable(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))
        .map_err(|e| format!("invalid update ZIP: {e}"))?;
    let file = archive
        .by_name("dost.exe")
        .map_err(|_| "the update ZIP does not contain dost.exe".to_string())?;
    if !file.is_file() || file.is_symlink() {
        return Err("the update executable is not a regular file".into());
    }
    read_executable(file)
}

#[cfg(target_os = "linux")]
fn unpack_executable(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    let entries = archive
        .entries()
        .map_err(|e| format!("invalid update archive: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("invalid update archive entry: {e}"))?;
        let path = entry.path().map_err(|e| e.to_string())?;
        if path.as_ref() == Path::new("dost") {
            if !entry.header().entry_type().is_file() {
                return Err("the update executable is not a regular file".into());
            }
            return read_executable(entry);
        }
    }
    Err("the update archive does not contain dost".into())
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
    let data = unpack_executable(data)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = exe.with_extension("new.exe");
    let old = exe.with_extension("old.exe");
    std::fs::write(&new, &data).map_err(|e| format!("cannot write the update: {e}"))?;
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
    let archive = tmp.join("Dost.tar.gz");
    std::fs::write(&archive, data).map_err(|e| e.to_string())?;
    // macOS bsdtar restores the bundle's permissions and Apple metadata, including its ticket.
    let ok = std::process::Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&tmp)
        .status()
        .is_ok_and(|s| s.success());
    let fresh = tmp.join("Dost.app");
    if !ok || !fresh.join("Contents/MacOS/dost").exists() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("the update could not be unpacked".into());
    }
    let valid = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(&fresh)
        .status()
        .is_ok_and(|s| s.success());
    if !valid {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("the update app's signature is invalid".into());
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
    let data = unpack_executable(data)?;
    let exe = linux_exe().ok_or("cannot find the dost executable")?;
    let new = exe.with_extension("new");
    std::fs::write(&new, &data).map_err(|e| format!("cannot write the update: {e}"))?;
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
    fn release_archive_names() {
        assert_eq!(
            archive_name("windows", "x86_64").as_deref(),
            Some("dost_Windows_x86_64.zip")
        );
        assert_eq!(
            archive_name("windows", "aarch64").as_deref(),
            Some("dost_Windows_arm64.zip")
        );
        assert_eq!(
            archive_name("linux", "aarch64").as_deref(),
            Some("dost_Linux_arm64.tar.gz")
        );
        assert_eq!(
            archive_name("linux", "x86_64").as_deref(),
            Some("dost_Linux_x86_64.tar.gz")
        );
        assert_eq!(
            archive_name("macos", "aarch64").as_deref(),
            Some("dost_Darwin_arm64.tar.gz")
        );
        assert_eq!(
            archive_name("macos", "x86_64").as_deref(),
            Some("dost_Darwin_x86_64.tar.gz")
        );
        assert!(archive_name("linux", "arm").is_none());
        assert!(archive_name("unknown", "x86_64").is_none());
    }

    #[test]
    fn manifest_matches_exact_filename() {
        let hash = hex(&Sha256::digest(b"archive"));
        let manifest = format!(
            "{}  dost.dmg\r\n{}  dost_Linux_arm64.tar.gz\r\n",
            "0".repeat(64),
            hash.to_uppercase()
        );
        assert_eq!(
            checksum(&manifest, "dost_Linux_arm64.tar.gz"),
            Ok(hash.clone())
        );
        assert_eq!(
            checksum(&format!("{hash} *file.zip\n"), "file.zip"),
            Ok(hash)
        );
        assert!(checksum(&manifest, "dost_Linux_x86_64.tar.gz").is_err());
        assert!(checksum(&manifest, "arm64.tar.gz").is_err());
    }

    #[test]
    fn reject_invalid_or_ambiguous_checksums() {
        let hash = "a".repeat(64);
        for manifest in [
            "bad  file.zip".to_string(),
            format!("{}  file.zip", "g".repeat(64)),
            format!("{hash}  file.zip extra"),
            format!("{hash}  file.zip\n{hash}  file.zip"),
            String::new(),
        ] {
            assert!(checksum(&manifest, "file.zip").is_err());
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn reject_empty_executable() {
        assert!(read_executable(std::io::empty()).is_err());
        assert_eq!(read_executable(&b"executable"[..]).unwrap(), b"executable");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unpack_only_linux_executable() {
        fn archive(name: &str, kind: tar::EntryType, contents: &[u8]) -> Vec<u8> {
            let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            let mut tar = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(contents.len() as u64);
            header.set_entry_type(kind);
            if kind.is_symlink() {
                header.set_link_name("/tmp/not-an-executable").unwrap();
            }
            header.set_cksum();
            tar.append_data(&mut header, name, contents).unwrap();
            tar.into_inner().unwrap().finish().unwrap()
        }
        let data = archive("dost", tar::EntryType::Regular, b"ELF executable");
        assert_eq!(unpack_executable(&data).unwrap(), b"ELF executable");
        assert!(unpack_executable(&archive("other/dost", tar::EntryType::Regular, b"x")).is_err());
        assert!(unpack_executable(&archive("dost", tar::EntryType::Symlink, b"")).is_err());
        assert!(unpack_executable(&archive("dost", tar::EntryType::Regular, b"")).is_err());
        assert!(unpack_executable(b"not an archive").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn unpack_only_windows_executable() {
        use std::io::Write;
        fn archive(name: &str, contents: &[u8]) -> Vec<u8> {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents).unwrap();
            zip.finish().unwrap().into_inner()
        }
        assert_eq!(
            unpack_executable(&archive("dost.exe", b"MZ executable")).unwrap(),
            b"MZ executable"
        );
        assert!(unpack_executable(&archive("other/dost.exe", b"x")).is_err());
        assert!(unpack_executable(&archive("dost.exe", b"")).is_err());
        assert!(unpack_executable(b"not an archive").is_err());
    }

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
