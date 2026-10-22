//! The `shell` and `shell_job` tools: commands run with the user's authority in their own
//! process group, output streamed to a file in the output store, and background jobs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::output::Store;

/// How long a stopped command gets to exit before it is killed outright.
const GRACE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(20);

/// Windows: start console programs without a window (Dost is a GUI app).
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The shell commands run in: `$SHELL` (else `/bin/sh`), or `%COMSPEC%` (else `cmd.exe`).
pub fn shell_path() -> String {
    let var = if cfg!(windows) { "COMSPEC" } else { "SHELL" };
    match std::env::var(var) {
        Ok(s) if !s.trim().is_empty() => s,
        _ if cfg!(windows) => "cmd.exe".into(),
        _ => "/bin/sh".into(),
    }
}

/// `command` run by the shell in `dir`, in its own process group, with stdin closed and
/// stdout and stderr both going to `out`.
fn spawn(command: &str, dir: &Path, out: &std::fs::File) -> std::io::Result<Child> {
    let mut cmd = Command::new(shell_path());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg("-c").arg(command).process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.arg("/C")
            .raw_arg(command)
            .creation_flags(CREATE_NO_WINDOW);
    }
    cmd.current_dir(dir)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out.try_clone()?)
        .spawn()
}

/// Asks the command and everything it started to stop (SIGTERM to the group on Unix; on
/// Windows the tree is killed at once).
fn stop_tree(child: &mut Child) {
    #[cfg(unix)]
    signal_group(child.id(), "-TERM");
    #[cfg(windows)]
    kill_tree(child);
}

/// Kills the command and everything it started.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    signal_group(child.id(), "-KILL");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([signal, "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Waits for `child` until `deadline` or until `stop` is set. `None` when it is still running.
fn wait_until(
    child: &mut Child,
    deadline: Option<Instant>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Option<ExitStatus>> {
    loop {
        if let Some(st) = child.try_wait()? {
            return Ok(Some(st));
        }
        if deadline.is_some_and(|d| Instant::now() >= d)
            || stop.is_some_and(|s| s.load(Ordering::Relaxed))
        {
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

/// Stops the command's process group, then kills it if it has not exited after a grace period.
fn terminate(child: &mut Child) -> Option<ExitStatus> {
    stop_tree(child);
    if let Ok(Some(st)) = wait_until(child, Some(Instant::now() + GRACE), None) {
        // Children that ignored SIGTERM must not outlive the shell.
        #[cfg(unix)]
        signal_group(child.id(), "-KILL");
        return Some(st);
    }
    kill_tree(child);
    child.wait().ok()
}

fn exit_code(st: &ExitStatus) -> i32 {
    st.code().unwrap_or(-1)
}

pub enum Outcome {
    Exited(i32, String),
    TimedOut(String),
}

/// How often new output goes to the live stream.
const STREAM_EVERY: Duration = Duration::from_millis(500);
/// At most this much output per live update (the full text is in the output file).
const STREAM_MAX: usize = 16 * 1024;

/// Runs `command` in the foreground: its output (the tail when it is long) and how it ended.
/// `live` is called every half second with the new output (empty when there is none).
pub fn run_foreground(
    store: &Store,
    command: &str,
    dir: &Path,
    timeout: Duration,
    mut live: Option<&mut dyn FnMut(String)>,
) -> Result<Outcome, String> {
    let (file, path) = store.create()?;
    let mut child = match spawn(command, dir, &file) {
        Ok(c) => c,
        Err(e) => {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(format!("unable to execute command; {e}"));
        }
    };
    drop(file);
    let deadline = Instant::now().checked_add(timeout);
    let mut sent = 0u64;
    let status = loop {
        let step = Instant::now() + STREAM_EVERY;
        let until = deadline.map_or(step, |d| d.min(step));
        let st = wait_until(&mut child, Some(until), None)
            .map_err(|e| format!("unable to execute command; {e}"))?;
        if let Some(f) = live.as_deref_mut() {
            stream_new(&path, &mut sent, f);
        }
        if st.is_some() || deadline.is_some_and(|d| Instant::now() >= d) {
            break st;
        }
    };
    let timed_out = status.is_none();
    let status = status.or_else(|| terminate(&mut child));
    let body = store
        .tail_file(&path, false)
        .map_err(|e| format!("read command output; {e}"))?;
    Ok(match (timed_out, status) {
        (false, Some(st)) => Outcome::Exited(exit_code(&st), body),
        _ => Outcome::TimedOut(body),
    })
}

/// Sends what was written to `path` after byte `sent`.
fn stream_new(path: &Path, sent: &mut u64, f: &mut dyn FnMut(String)) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    if file.seek(SeekFrom::Start(*sent)).is_err() {
        return;
    }
    let mut buf = Vec::new();
    let Ok(n) = file.take(STREAM_MAX as u64).read_to_end(&mut buf) else {
        return;
    };
    // Whole lines only, unless one line is longer than the whole update.
    let cut = match buf.iter().rposition(|b| *b == b'\n') {
        Some(i) => i + 1,
        None if n >= STREAM_MAX => n,
        None => 0,
    };
    *sent += cut as u64;
    f(String::from_utf8_lossy(&buf[..cut]).into_owned());
}

/// Background shell commands. They live as long as [`super::Local`] and are stopped with it.
pub struct Jobs {
    seq: AtomicU64,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

pub struct Job {
    pub id: String,
    pub command: String,
    pub pid: u32,
    pub output: PathBuf,
    started: Instant,
    kill: AtomicBool,
    state: Mutex<JobState>,
    done: Condvar,
}

struct JobState {
    /// running, killing, exited, killed, timed out or `failed: …`.
    state: String,
    exit_code: i32,
    finished: Option<Instant>,
}

impl Job {
    fn finished(&self) -> bool {
        self.state
            .lock()
            .map(|s| s.finished.is_some())
            .unwrap_or(true)
    }

    /// Blocks until the job ends or `timeout` passes.
    fn wait(&self, timeout: Duration) {
        let Ok(guard) = self.state.lock() else {
            return;
        };
        let _ = self
            .done
            .wait_timeout_while(guard, timeout, |s| s.finished.is_none());
    }

    /// One line of state plus the command and the output file.
    pub fn describe(&self) -> String {
        let (state, code, finished) = match self.state.lock() {
            Ok(s) => (s.state.clone(), s.exit_code, s.finished),
            Err(_) => ("unknown".into(), -1, None),
        };
        let elapsed = finished.unwrap_or_else(Instant::now) - self.started;
        let mut s = format!(
            "{} [{state}] pid={} elapsed={}",
            self.id,
            self.pid,
            duration(elapsed)
        );
        if state == "exited" {
            s += &format!(" exit={code}");
        }
        format!(
            "{s}\n  command: {}\n  output: {}",
            self.command,
            self.output.display()
        )
    }
}

impl Default for Jobs {
    fn default() -> Self {
        Jobs {
            seq: AtomicU64::new(0),
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

impl Jobs {
    /// Starts `command` in the background; `timeout` zero means none.
    pub fn start(
        &self,
        store: &Store,
        command: &str,
        dir: &Path,
        timeout: Duration,
    ) -> Result<Arc<Job>, String> {
        let (file, path) = store.create()?;
        let mut child = match spawn(command, dir, &file) {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(format!("unable to start command; {e}"));
            }
        };
        drop(file);
        let job = Arc::new(Job {
            id: format!("job_{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1),
            command: command.into(),
            pid: child.id(),
            output: path,
            started: Instant::now(),
            kill: AtomicBool::new(false),
            state: Mutex::new(JobState {
                state: "running".into(),
                exit_code: -1,
                finished: None,
            }),
            done: Condvar::new(),
        });
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.insert(job.id.clone(), job.clone());
        }
        let deadline = (!timeout.is_zero()).then(|| Instant::now() + timeout);
        let j = job.clone();
        thread::spawn(move || {
            let result = wait_until(&mut child, deadline, Some(&j.kill));
            let (state, code) = match result {
                Ok(Some(st)) => ("exited".to_string(), exit_code(&st)),
                Ok(None) => {
                    terminate(&mut child);
                    let state = if j.kill.load(Ordering::Relaxed) {
                        "killed"
                    } else {
                        "timed out"
                    };
                    (state.to_string(), -1)
                }
                Err(e) => {
                    kill_tree(&mut child);
                    let _ = child.wait();
                    (format!("failed: {e}"), -1)
                }
            };
            if let Ok(mut s) = j.state.lock() {
                s.state = state;
                s.exit_code = code;
                s.finished = Some(Instant::now());
            }
            j.done.notify_all();
        });
        Ok(job)
    }

    pub fn get(&self, id: &str) -> Result<Arc<Job>, String> {
        self.jobs
            .lock()
            .ok()
            .and_then(|j| j.get(id).cloned())
            .ok_or_else(|| format!("unknown job {id:?}"))
    }

    /// Every job, oldest first.
    pub fn list(&self) -> Vec<Arc<Job>> {
        let mut jobs: Vec<_> = self
            .jobs
            .lock()
            .map(|j| j.values().cloned().collect())
            .unwrap_or_default();
        jobs.sort_by_key(|j| j.started);
        jobs
    }

    /// Stops the job and its process group, waiting a little for it to end.
    pub fn kill(&self, job: &Job) {
        if let Ok(mut s) = job.state.lock()
            && s.state == "running"
        {
            s.state = "killing".into();
        }
        job.kill.store(true, Ordering::Relaxed);
        job.wait(Duration::from_secs(5));
    }

    pub fn wait(&self, job: &Job, timeout: Duration) {
        job.wait(timeout);
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        let jobs = self.list();
        for job in &jobs {
            job.kill.store(true, Ordering::Relaxed);
        }
        for job in &jobs {
            if !job.finished() {
                job.wait(Duration::from_secs(5));
            }
        }
    }
}

/// A duration the way Go prints one rounded to milliseconds: `250ms`, `1.5s`, `2m3.25s`.
fn duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let (m, rest) = (ms / 60_000, ms % 60_000);
    let secs = format!("{}.{:03}", rest / 1000, rest % 1000);
    let secs = secs.trim_end_matches('0').trim_end_matches('.');
    if m == 0 {
        format!("{secs}s")
    } else {
        format!("{m}m{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration(Duration::from_millis(250)), "250ms");
        assert_eq!(duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(duration(Duration::from_millis(2000)), "2s");
        assert_eq!(duration(Duration::from_millis(123_250)), "2m3.25s");
    }
}
