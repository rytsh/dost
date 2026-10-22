//! Status watches: the dog checks a URL or command at intervals and, when the
//! condition becomes true (false -> true), shows a notification and barks.
//! Watches are added via MCP and saved to disk.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::notify::{Notification, Notifier};

pub const MIN_INTERVAL: u64 = 5;
pub const DEFAULT_INTERVAL: u64 = 30;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const BODY_LIMIT: u64 = 1 << 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    /// Condition: status code is `status` (or 2xx if unset) and the body contains `contains` (if given).
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contains: Option<String>,
    },
    /// Condition: exit code 0, or if `contains` is given, the output contains it.
    /// Runs with `cmd /C` on Windows and `sh -c` elsewhere.
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contains: Option<String>,
    },
    /// Condition: something accepts TCP connections on `host:port`.
    Port { host: String, port: u16 },
    /// Condition: the file exists and, if `contains` is given, its last 1 MB contains it.
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contains: Option<String>,
    },
    /// Condition: the process is running.
    Process { pid: u32 },
    /// Condition: the time has come (Unix seconds). Reminders.
    Time { at: u64 },
    /// Condition: a GitHub Actions run finished (any result). `run`: a run id; otherwise the
    /// newest run of `branch` (or of the repo) that started after the watch was added.
    Github {
        repo: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// Unix seconds; runs created before this are ignored (no `run` given).
        #[serde(default)]
        since: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Watch {
    pub id: u64,
    pub name: String,
    #[serde(flatten)]
    pub check: Check,
    /// Inverts the condition ("notify me when the site goes down").
    #[serde(default)]
    pub negate: bool,
    pub interval_secs: u64,
    pub title: String,
    #[serde(default)]
    pub message: String,
    /// Removed after it fires once.
    #[serde(default = "yes")]
    pub once: bool,
    #[serde(default)]
    pub sticky: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub state: WatchState,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WatchState {
    /// Result of the last check (with `negate` applied).
    pub matched: Option<bool>,
    pub detail: Option<String>,
    /// Unix seconds.
    pub checked_at: Option<u64>,
    pub fired: u32,
}

fn yes() -> bool {
    true
}

/// A request for a new watch (MCP `add_watch`).
#[derive(Clone, Debug)]
pub struct NewWatch {
    pub name: String,
    pub check: Check,
    pub negate: bool,
    pub interval_secs: u64,
    pub title: String,
    pub message: String,
    pub once: bool,
    pub sticky: bool,
    pub source: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    next_id: u64,
    watches: Vec<Watch>,
}

#[derive(Default)]
struct State {
    stored: Stored,
    running: HashSet<u64>,
    due: HashMap<u64, Instant>,
}

pub struct Watches {
    state: Mutex<State>,
    path: Option<PathBuf>,
    notifier: Notifier,
}

impl Watches {
    /// If `path` is given, watches are loaded from it and written to it on every change.
    pub fn new(notifier: Notifier, path: Option<PathBuf>) -> Arc<Self> {
        let stored = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|d| serde_json::from_slice::<Stored>(&d).ok())
            .unwrap_or_default();
        let w = Watches {
            state: Mutex::new(State {
                stored,
                ..State::default()
            }),
            path,
            notifier,
        };
        Arc::new(w)
    }

    /// Starts the thread that runs watches in the background.
    pub fn start(self: &Arc<Self>) {
        let me = Arc::clone(self);
        thread::spawn(move || {
            loop {
                for w in me.take_due() {
                    let me = Arc::clone(&me);
                    thread::spawn(move || {
                        let result = run_check(&w.check);
                        me.finish(w.id, result);
                    });
                }
                thread::sleep(Duration::from_millis(500));
            }
        });
    }

    pub fn add(&self, n: NewWatch) -> Watch {
        let mut st = self.state.lock().unwrap();
        st.stored.next_id += 1;
        let w = Watch {
            id: st.stored.next_id,
            name: n.name,
            interval_secs: match n.check {
                // Cheap and must fire on time.
                Check::Time { .. } => 1,
                _ => n.interval_secs.max(MIN_INTERVAL),
            },
            check: n.check,
            negate: n.negate,
            title: n.title,
            message: n.message,
            once: n.once,
            sticky: n.sticky,
            source: n.source,
            state: WatchState::default(),
        };
        st.stored.watches.push(w.clone());
        self.save(&st);
        w
    }

    pub fn remove(&self, id: u64) -> bool {
        let mut st = self.state.lock().unwrap();
        let before = st.stored.watches.len();
        st.stored.watches.retain(|w| w.id != id);
        st.due.remove(&id);
        let removed = st.stored.watches.len() != before;
        if removed {
            self.save(&st);
        }
        removed
    }

    pub fn list(&self) -> Vec<Watch> {
        self.state.lock().unwrap().stored.watches.clone()
    }

    fn take_due(&self) -> Vec<Watch> {
        let now = Instant::now();
        let mut st = self.state.lock().unwrap();
        let State {
            stored,
            running,
            due,
        } = &mut *st;
        let mut out = Vec::new();
        for w in &stored.watches {
            if running.contains(&w.id) || due.get(&w.id).is_some_and(|t| *t > now) {
                continue;
            }
            running.insert(w.id);
            due.insert(w.id, now + Duration::from_secs(w.interval_secs));
            out.push(w.clone());
        }
        out
    }

    fn finish(&self, id: u64, result: Result<(bool, String), String>) {
        let mut st = self.state.lock().unwrap();
        st.running.remove(&id);
        let Some(i) = st.stored.watches.iter().position(|w| w.id == id) else {
            return;
        };
        let w = &mut st.stored.watches[i];
        let (ok, detail) = match result {
            Ok(r) => r,
            Err(e) => (false, e),
        };
        let matched = ok != w.negate;
        let fire = matched && w.state.matched != Some(true);
        w.state.matched = Some(matched);
        w.state.detail = Some(detail.clone());
        w.state.checked_at = Some(unix_now());
        if fire {
            w.state.fired += 1;
            let mut n = match &w.check {
                Check::Github { repo, .. } => github_notification(w, repo, &detail),
                _ => {
                    let body = if w.message.is_empty() {
                        format!("{}: {detail}", w.name)
                    } else {
                        w.message.clone()
                    };
                    Notification::new(w.title.clone(), body)
                }
            }
            .source(w.source.clone());
            if w.sticky {
                n = n.sticky();
            }
            self.notifier.send(n);
            if w.once {
                st.stored.watches.remove(i);
            }
        }
        self.save(&st);
    }

    fn save(&self, st: &State) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(data) = serde_json::to_vec_pretty(&st.stored) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }
}

/// "CI passed" / "CI failed" with a link to the run.
fn github_notification(w: &Watch, repo: &str, detail: &str) -> Notification {
    let mut parts = detail.splitn(3, " · ");
    let conclusion = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("run");
    let link = parts.next().unwrap_or("");
    let ok = conclusion == "success";
    let title = if w.title.is_empty() || w.title == w.name {
        format!("{name} {}", if ok { "passed" } else { "failed" })
    } else {
        w.title.clone()
    };
    let body = if w.message.is_empty() {
        format!("{repo}: {conclusion}")
    } else {
        w.message.clone()
    };
    let mut n = Notification::new(title, body);
    if link.starts_with("https://") {
        n.url = Some(link.to_string());
        n = n.timeout(Duration::from_secs(30));
    }
    n
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Unix seconds for the next local "HH:MM" (today, or tomorrow if it has passed).
pub fn next_local_time(hhmm: &str) -> Option<u64> {
    use chrono::{Local, NaiveTime, TimeZone};
    let mins = crate::settings::parse_hhmm(hhmm)?;
    let t = NaiveTime::from_hms_opt(mins / 60, mins % 60, 0)?;
    let now = Local::now();
    let mut day = now.date_naive();
    if now.time() >= t {
        day = day.succ_opt()?;
    }
    let at = Local.from_local_datetime(&day.and_time(t)).earliest()?;
    u64::try_from(at.timestamp()).ok()
}

pub fn now_unix() -> u64 {
    unix_now()
}

/// Runs a check: `Ok((condition, detail))`, or `Err(detail)` if unreachable.
pub fn run_check(check: &Check) -> Result<(bool, String), String> {
    match check {
        Check::Http {
            url,
            status,
            contains,
        } => check_http(url, *status, contains.as_deref()),
        Check::Command { command, contains } => check_command(command, contains.as_deref()),
        Check::Port { host, port } => Ok(check_port(host, *port)),
        Check::File { path, contains } => Ok(check_file(path, contains.as_deref())),
        Check::Process { pid } => Ok(check_process(*pid)),
        Check::Github {
            repo,
            run,
            branch,
            since,
        } => check_github(repo, *run, branch.as_deref(), *since),
        Check::Time { at } => {
            let now = unix_now();
            Ok(if now >= *at {
                (true, "time is up".into())
            } else {
                (false, format!("in {}", human_secs(at - now)))
            })
        }
    }
}

/// "1 h 5 min", "3 min", "40 s".
pub fn human_secs(s: u64) -> String {
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s.div_ceil(60)),
        _ => {
            let (h, m) = (s / 3600, (s % 3600) / 60);
            if m == 0 {
                format!("{h} h")
            } else {
                format!("{h} h {m} min")
            }
        }
    }
}

/// [`human_secs`] in the app's language (for the panel; agents get English).
pub fn human_secs_t(s: u64) -> String {
    use crate::i18n::tf;
    let n = |v: u64| v.to_string();
    match s {
        0..60 => tf("{n} s", &[("n", &n(s))]),
        60..3600 => tf("{n} min", &[("n", &n(s.div_ceil(60)))]),
        _ => {
            let (h, m) = (s / 3600, (s % 3600) / 60);
            if m == 0 {
                tf("{n} h", &[("n", &n(h))])
            } else {
                tf("{n} h {m} min", &[("n", &n(h)), ("m", &n(m))])
            }
        }
    }
}

/// The run's state from the GitHub API. Detail: "<conclusion> · <name> · <url>" when done.
/// Uses $GITHUB_TOKEN / $GH_TOKEN when set (needed for private repos, raises rate limits).
fn check_github(
    repo: &str,
    run: Option<u64>,
    branch: Option<&str>,
    since: u64,
) -> Result<(bool, String), String> {
    let url = match run {
        Some(id) => format!("https://api.github.com/repos/{repo}/actions/runs/{id}"),
        None => {
            let mut u = format!("https://api.github.com/repos/{repo}/actions/runs?per_page=5");
            if let Some(b) = branch {
                u.push_str(&format!("&branch={b}"));
            }
            u
        }
    };
    let agent = http_agent(HTTP_TIMEOUT);
    let mut req = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "dost");
    if let Some(t) = ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|t| !t.is_empty()))
    {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut resp = req.call().map_err(|e| format!("unreachable: {e}"))?;
    let code = resp.status().as_u16();
    let body = resp
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()
        .unwrap_or_default();
    if code != 200 {
        return Err(format!("GitHub HTTP {code}"));
    }
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let r = match run {
        Some(_) => Some(&v),
        None => v["workflow_runs"].as_array().and_then(|runs| {
            runs.iter().find(|r| {
                r["created_at"]
                    .as_str()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .is_some_and(|t| t.timestamp() >= since as i64)
            })
        }),
    };
    let Some(r) = r else {
        return Ok((false, "no new run yet".into()));
    };
    let name = r["name"].as_str().unwrap_or("run");
    let status = r["status"].as_str().unwrap_or("");
    if status != "completed" {
        return Ok((false, format!("{name}: {status}")));
    }
    let conclusion = r["conclusion"].as_str().unwrap_or("done");
    let link = r["html_url"].as_str().unwrap_or("");
    Ok((true, format!("{conclusion} · {name} · {link}")))
}

/// An HTTP client with the platform's TLS: rustls on Windows, the system's on macOS (the
/// provider must be chosen; the default is rustls, which is not built there).
pub fn http_agent(timeout: Duration) -> ureq::Agent {
    let mut cfg = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false);
    if cfg!(target_os = "macos") {
        cfg = cfg.tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        );
    }
    cfg.build().into()
}

fn check_port(host: &str, port: u16) -> (bool, String) {
    use std::net::{TcpStream, ToSocketAddrs};
    let addrs = match (host, port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(e) => return (false, format!("{host}: {e}")),
    };
    for a in &addrs {
        if TcpStream::connect_timeout(a, Duration::from_secs(3)).is_ok() {
            return (true, format!("{host}:{port} is open"));
        }
    }
    (false, format!("{host}:{port} is closed"))
}

fn check_file(path: &str, contains: Option<&str>) -> (bool, String) {
    use std::io::{Seek, SeekFrom};
    let path = expand_home(path);
    let Ok(mut f) = std::fs::File::open(&path) else {
        return (false, "file does not exist".into());
    };
    let Some(needle) = contains else {
        return (true, "file exists".into());
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(BODY_LIMIT)));
    let mut buf = Vec::new();
    let _ = f.take(BODY_LIMIT).read_to_end(&mut buf);
    if String::from_utf8_lossy(&buf).contains(needle) {
        (true, format!("file contains \"{needle}\""))
    } else {
        (false, format!("file does not contain \"{needle}\" yet"))
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/").zip(std::env::var_os("HOME")) {
        Some((rest, home)) => PathBuf::from(home).join(rest),
        None => PathBuf::from(path),
    }
}

#[cfg(unix)]
fn check_process(pid: u32) -> (bool, String) {
    // `kill -0` checks existence without sending a signal.
    let alive = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if alive {
        (true, format!("process {pid} is running"))
    } else {
        (false, format!("process {pid} is not running"))
    }
}

#[cfg(windows)]
fn check_process(pid: u32) -> (bool, String) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let alive =
        out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")));
    if alive {
        (true, format!("process {pid} is running"))
    } else {
        (false, format!("process {pid} is not running"))
    }
}

fn check_http(
    url: &str,
    status: Option<u16>,
    contains: Option<&str>,
) -> Result<(bool, String), String> {
    let agent = http_agent(HTTP_TIMEOUT);
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("unreachable: {e}"))?;
    let code = resp.status().as_u16();
    let status_ok = match status {
        Some(s) => code == s,
        None => (200..300).contains(&code),
    };
    let mut ok = status_ok;
    if let Some(needle) = contains {
        let body = resp
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_string()
            .unwrap_or_default();
        ok &= body.contains(needle);
    }
    Ok((ok, format!("HTTP {code}")))
}

fn check_command(command: &str, contains: Option<&str>) -> Result<(bool, String), String> {
    let mut cmd = shell(command);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("failed to run: {e}"))?;
    let mut stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.by_ref().take(BODY_LIMIT).read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if start.elapsed() > COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("timed out".into());
        }
        thread::sleep(Duration::from_millis(100));
    };
    let out = reader.join().unwrap_or_default();
    let code = status.code().unwrap_or(-1);
    let ok = match contains {
        Some(needle) => out.contains(needle),
        None => status.success(),
    };
    Ok((ok, format!("exit code {code}")))
}

#[cfg(windows)]
pub(crate) fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut c = Command::new("cmd");
    c.arg("/C")
        .raw_arg(command)
        .creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(not(windows))]
pub(crate) fn shell(command: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(command);
    c
}

/// The file watches are saved to.
pub fn default_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("watches.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify;

    fn new_watch(check: Check) -> NewWatch {
        NewWatch {
            name: "test".into(),
            check,
            negate: false,
            interval_secs: 30,
            title: "T".into(),
            message: String::new(),
            once: true,
            sticky: false,
            source: None,
        }
    }

    #[test]
    fn github_bubble_links_to_the_run() {
        let w = Watch {
            id: 1,
            name: "CI a/b".into(),
            check: Check::Github {
                repo: "a/b".into(),
                run: None,
                branch: None,
                since: 0,
            },
            negate: false,
            interval_secs: 30,
            title: "CI a/b".into(),
            message: String::new(),
            once: true,
            sticky: false,
            source: None,
            state: WatchState::default(),
        };
        let n = github_notification(
            &w,
            "a/b",
            "failure · Tests · https://github.com/a/b/actions/runs/9",
        );
        assert_eq!(n.title, "Tests failed");
        assert_eq!(n.body, "a/b: failure");
        assert_eq!(
            n.url.as_deref(),
            Some("https://github.com/a/b/actions/runs/9")
        );
        let n = github_notification(
            &w,
            "a/b",
            "success · CI · https://github.com/a/b/actions/runs/9",
        );
        assert_eq!(n.title, "CI passed");
    }

    #[test]
    fn new_checks() {
        let dir = std::env::temp_dir().join(format!("dost-file-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("log.txt");
        let file = |contains: Option<&str>| Check::File {
            path: f.to_string_lossy().into(),
            contains: contains.map(String::from),
        };
        assert!(!run_check(&file(None)).unwrap().0);
        std::fs::write(&f, "starting\nbuild ok\n").unwrap();
        assert!(run_check(&file(None)).unwrap().0);
        assert!(run_check(&file(Some("build ok"))).unwrap().0);
        assert!(!run_check(&file(Some("ERROR"))).unwrap().0);
        let _ = std::fs::remove_dir_all(dir);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let open = Check::Port {
            host: "127.0.0.1".into(),
            port,
        };
        assert!(run_check(&open).unwrap().0);
        drop(listener);
        // The port may take a moment to close.
        let closed = (0..50).any(|_| {
            let open_now = run_check(&open).unwrap().0;
            if open_now {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            !open_now
        });
        assert!(closed);

        assert!(
            run_check(&Check::Process {
                pid: std::process::id()
            })
            .unwrap()
            .0
        );
        assert!(!run_check(&Check::Process { pid: 4_000_000 }).unwrap().0);

        assert!(run_check(&Check::Time { at: 1 }).unwrap().0);
        assert!(
            !run_check(&Check::Time {
                at: unix_now() + 120
            })
            .unwrap()
            .0
        );
    }

    #[test]
    fn local_times_and_durations() {
        let at = next_local_time("00:00").unwrap();
        let now = unix_now();
        assert!(at > now && at <= now + 24 * 3600 + 3600);
        assert_eq!(next_local_time("24:00"), None);
        assert_eq!(human_secs(40), "40 s");
        assert_eq!(human_secs(150), "3 min");
        assert_eq!(human_secs(3900), "1 h 5 min");
        assert_eq!(human_secs(7200), "2 h");
    }

    #[test]
    fn fires_on_rising_edge_only() {
        let (tx, rx) = notify::channel();
        let ws = Watches::new(tx, None);
        let mut n = new_watch(Check::Command {
            command: "true".into(),
            contains: None,
        });
        n.once = false;
        let w = ws.add(n);
        ws.finish(w.id, Ok((true, "a".into())));
        ws.finish(w.id, Ok((true, "b".into())));
        assert_eq!(rx.try_iter().count(), 1);
        ws.finish(w.id, Ok((false, "c".into())));
        ws.finish(w.id, Ok((true, "d".into())));
        assert_eq!(rx.try_iter().count(), 1);
        assert_eq!(ws.list()[0].state.fired, 2);
    }

    #[test]
    fn once_removes_and_negate_inverts() {
        let (tx, rx) = notify::channel();
        let ws = Watches::new(tx, None);
        let mut n = new_watch(Check::Http {
            url: "http://127.0.0.1:9".into(),
            status: None,
            contains: None,
        });
        n.negate = true;
        let w = ws.add(n);
        ws.finish(w.id, Err("unreachable".into()));
        let got = rx.try_recv().unwrap().into_notification().unwrap();
        assert!(got.body.contains("unreachable"));
        assert!(ws.list().is_empty());
    }

    #[test]
    fn persists_to_disk() {
        let dir = std::env::temp_dir().join(format!("dost-test-{}", std::process::id()));
        let path = dir.join("watches.json");
        let (tx, _rx) = notify::channel();
        let id = Watches::new(tx.clone(), Some(path.clone()))
            .add(new_watch(Check::Command {
                command: "true".into(),
                contains: Some("x".into()),
            }))
            .id;
        let again = Watches::new(tx, Some(path));
        assert_eq!(again.list()[0].id, id);
        assert_eq!(
            again
                .add(new_watch(Check::Command {
                    command: "true".into(),
                    contains: None
                }))
                .id,
            id + 1
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn command_check() {
        assert!(
            run_check(&Check::Command {
                command: "exit 0".into(),
                contains: None
            })
            .unwrap()
            .0
        );
        assert!(
            !run_check(&Check::Command {
                command: "exit 3".into(),
                contains: None
            })
            .unwrap()
            .0
        );
        let c = Check::Command {
            command: "echo build ok".into(),
            contains: Some("ok".into()),
        };
        assert!(run_check(&c).unwrap().0);
    }
}
