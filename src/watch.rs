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
            check: n.check,
            negate: n.negate,
            interval_secs: n.interval_secs.max(MIN_INTERVAL),
            title: n.title,
            message: n.message,
            once: n.once,
            sticky: n.sticky,
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
            let body = if w.message.is_empty() {
                format!("{}: {detail}", w.name)
            } else {
                w.message.clone()
            };
            let mut n = Notification::new(w.title.clone(), body);
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

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
    }
}

fn check_http(
    url: &str,
    status: Option<u16>,
    contains: Option<&str>,
) -> Result<(bool, String), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
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
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut c = Command::new("cmd");
    c.arg("/C")
        .raw_arg(command)
        .creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(not(windows))]
fn shell(command: &str) -> Command {
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
        }
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
        let got = rx.try_recv().unwrap();
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
