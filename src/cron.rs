//! Scheduled jobs (cron): at times given by a cron expression the dog reminds the user,
//! sends an HTTP request or runs a command, and says how it went. Jobs are added from the
//! settings (Schedule) or over MCP (`cron_add`) and saved to `cron.json`.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

use crate::i18n::tf_in;
use crate::notify::{Mood, Notification, Notifier};

/// What a job does when it is due.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// A reminder: the dog barks and shows the text.
    Notify {
        title: String,
        #[serde(default)]
        message: String,
        #[serde(default)]
        sticky: bool,
    },
    /// An HTTP request. Succeeds on `expect_status` (any 2xx when unset) and, when given, a
    /// body that contains `contains`.
    Http {
        #[serde(default = "get")]
        method: String,
        url: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        headers: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contains: Option<String>,
    },
    /// A shell command (`sh -c`, `cmd /C` on Windows). Succeeds on exit code 0.
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
    },
}

fn get() -> String {
    "GET".into()
}

/// When a run of a request or command shows a bubble (reminders always do).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyWhen {
    Always,
    /// Only when it failed.
    #[default]
    Failure,
    Never,
}

/// How the last run went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Unix seconds.
    pub at: u64,
    pub ok: bool,
    /// "200 OK", "exit 0", the first line of the output or the error.
    pub detail: String,
    pub secs: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub name: String,
    /// A cron expression (`*/15 9-17 * * 1-5`) or a shortcut (`@daily`).
    pub schedule: String,
    #[serde(flatten)]
    pub action: Action,
    #[serde(default)]
    pub notify: NotifyWhen,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<Run>,
    #[serde(default)]
    pub runs: u32,
}

fn yes() -> bool {
    true
}

/// A request for a new job.
#[derive(Clone, Debug)]
pub struct NewJob {
    pub name: String,
    pub schedule: String,
    pub action: Action,
    pub notify: NotifyWhen,
    pub source: Option<String>,
}

/// A parsed cron expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    cron: croner::Cron,
    /// The five fields, with a shortcut (`@daily`) spelled out.
    expr: String,
}

const FIELDS: [(&str, u32, u32); 5] = [
    ("minute", 0, 59),
    ("hour", 0, 23),
    ("day of month", 1, 31),
    ("month", 1, 12),
    ("weekday", 0, 7),
];

impl Schedule {
    /// Parses five fields (minute hour day-of-month month day-of-week; `*`, lists, ranges,
    /// steps, month and day names) or a shortcut (`@hourly`, `@daily`, `@weekly`,
    /// `@monthly`, `@yearly`).
    pub fn parse(expr: &str) -> Result<Schedule, String> {
        let expr = expr.trim();
        let expr = match expr.strip_prefix('@') {
            Some(name) => shortcut(name).ok_or_else(|| {
                format!(
                    "unknown shortcut @{name}; use @hourly, @daily, @weekly, @monthly or @yearly"
                )
            })?,
            None => expr,
        };
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "expected 5 fields: minute hour day month weekday (got {}), e.g. \"0 9 * * 1-5\"",
                fields.len()
            ));
        }
        for (field, (name, lo, hi)) in fields.iter().zip(FIELDS) {
            check_range(field, name, lo, hi)?;
        }
        let expr = fields.join(" ");
        let cron = croner::parser::CronParser::builder()
            .seconds(croner::parser::Seconds::Disallowed)
            .year(croner::parser::Year::Disallowed)
            .sloppy_ranges(true)
            .build()
            .parse(&expr)
            .map_err(|e| {
                let msg = e.to_string();
                let msg = ["Component error: ", "Invalid pattern: "]
                    .iter()
                    .fold(msg.as_str(), |m, p| m.strip_prefix(p).unwrap_or(m))
                    .trim_end_matches('.')
                    .to_string();
                format!("invalid schedule \"{expr}\": {msg}")
            })?;
        Ok(Schedule { cron, expr })
    }

    /// The first local time after `t` (whole minutes) that matches.
    pub fn next_after(&self, t: chrono::NaiveDateTime) -> Option<chrono::NaiveDateTime> {
        self.cron.find_next_occurrence(&t, false).ok()
    }

    /// The schedule in words ("every day at 09:00", "every 15 minutes, 9-17, Mon-Fri"), in the
    /// app's language.
    pub fn describe(&self) -> String {
        self.describe_in(crate::i18n::code())
    }

    /// [`Schedule::describe`] in the language with `code`.
    pub fn describe_in(&self, code: &str) -> String {
        if let Some(s) = self.describe_short(code) {
            return s;
        }
        let s = self.cron.describe();
        let s = s.trim_end_matches('.');
        if s.is_empty() {
            self.expr.clone()
        } else {
            s.to_string()
        }
    }

    /// The common shapes in a few words; `None` for the rest.
    fn describe_short(&self, code: &str) -> Option<String> {
        let f: Vec<&str> = self.expr.split_whitespace().collect();
        let [min, hour, dom, month, dow] = f[..] else {
            return None;
        };
        if month != "*" {
            return None;
        }
        // Turkish puts "at" and "on the" on the number as a suffix: 09:00'da, ayın 6'sında.
        let turkish = code == "tr";
        let num = |s: &str| s.parse::<u32>().ok();
        if hour == "*" && dom == "*" && dow == "*" {
            if min == "*" {
                return Some(tf_in(code, "every minute", &[]));
            }
            if let Some(n) = min.strip_prefix("*/").and_then(num) {
                return Some(tf_in(code, "every {n} minutes", &[("n", &n.to_string())]));
            }
            let m = num(min)?;
            let mut at = format!("{m:02}");
            if turkish {
                at = format!("{at}'{}", locative(m));
            }
            return Some(tf_in(code, "every hour at :{m}", &[("m", &at)]));
        }
        let (m, h) = (num(min)?, num(hour)?);
        let mut at = format!("{h:02}:{m:02}");
        if turkish {
            at = format!("{at}'{}", locative(if m == 0 { h } else { m }));
        }
        let at = [("at", at.as_str())];
        match (dom, dow) {
            ("*", "*") => Some(tf_in(code, "every day at {at}", &at)),
            ("*", d) if d == "1-5" || d.eq_ignore_ascii_case("MON-FRI") => {
                Some(tf_in(code, "weekdays at {at}", &at))
            }
            ("*", d) => Some(match weekday(d)? % 7 {
                0 => tf_in(code, "Sundays at {at}", &at),
                1 => tf_in(code, "Mondays at {at}", &at),
                2 => tf_in(code, "Tuesdays at {at}", &at),
                3 => tf_in(code, "Wednesdays at {at}", &at),
                4 => tf_in(code, "Thursdays at {at}", &at),
                5 => tf_in(code, "Fridays at {at}", &at),
                _ => tf_in(code, "Saturdays at {at}", &at),
            }),
            (d, "*") => {
                let d = num(d)?;
                let mut n = d.to_string();
                if turkish {
                    n = format!("{n}'{}", ordinal_locative(d));
                }
                Some(tf_in(
                    code,
                    "on day {n} of every month at {at}",
                    &[("n", n.as_str()), at[0]],
                ))
            }
            _ => None,
        }
    }
}

/// `@daily` and friends as five fields.
fn shortcut(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "hourly" => "0 * * * *",
        "daily" | "midnight" => "0 0 * * *",
        "weekly" => "0 0 * * 0",
        "monthly" => "0 0 1 * *",
        "yearly" | "annually" => "0 0 1 1 *",
        _ => return None,
    })
}

/// "minute 61 is out of range 0-59" for plain numbers in a field.
fn check_range(field: &str, name: &str, lo: u32, hi: u32) -> Result<(), String> {
    for item in field.split(',') {
        let range = item.split('/').next().unwrap_or("");
        for n in range.split('-').filter_map(|p| p.parse::<u32>().ok()) {
            if n < lo || n > hi {
                return Err(format!("{name} {n} is out of range {lo}-{hi}"));
            }
        }
    }
    Ok(())
}

/// 0-7 from "1" or "MON" (case-insensitive).
fn weekday(s: &str) -> Option<u32> {
    const NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
    s.parse().ok().filter(|n| *n <= 7).or_else(|| {
        NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(s))
            .map(|i| i as u32)
    })
}

/// The Turkish "at" suffix for a number read aloud: 9 -> "da", 30 -> "da", 5 -> "te".
fn locative(n: u32) -> &'static str {
    let ones = ["da", "de", "de", "te", "te", "te", "da", "de", "de", "da"];
    let tens = ["da", "da", "de", "da", "ta", "de"];
    match n % 10 {
        0 => tens[(n / 10 % 10).min(5) as usize],
        d => ones[d as usize],
    }
}

/// The Turkish "on the Nth" suffix: 1 -> "inde", 2 -> "sinde", 30 -> "unda".
fn ordinal_locative(n: u32) -> &'static str {
    let ones = [
        "", "inde", "sinde", "ünde", "ünde", "inde", "sında", "sinde", "inde", "unda",
    ];
    let tens = ["", "unda", "sinde", "unda"];
    match n % 10 {
        0 => tens[(n / 10).min(3) as usize],
        d => ones[d as usize],
    }
}

const TICK: Duration = Duration::from_millis(500);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const BODY_LIMIT: u64 = 1 << 20;
#[cfg(not(test))]
const COMMAND_TIMEOUT: Duration = Duration::from_secs(600);
#[cfg(test)]
const COMMAND_TIMEOUT: Duration = Duration::from_secs(1);
const METHODS: [&str; 6] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    next_id: u64,
    jobs: Vec<Job>,
}

#[derive(Default)]
struct State {
    stored: Stored,
    running: HashSet<u64>,
    /// Unix seconds of each enabled job's next run (`None`: never); filled in lazily.
    due: HashMap<u64, Option<u64>>,
}

struct Inner {
    state: Mutex<State>,
    path: Option<PathBuf>,
    notifier: Notifier,
}

/// The jobs and the thread that runs them.
pub struct Cron {
    inner: Arc<Inner>,
}

impl Cron {
    /// Loads the jobs from `path` (when given); every change is written back.
    pub fn new(notifier: Notifier, path: Option<PathBuf>) -> Arc<Self> {
        let stored = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|d| serde_json::from_slice::<Stored>(&d).ok())
            .unwrap_or_default();
        Arc::new(Cron {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    stored,
                    ..State::default()
                }),
                path,
                notifier,
            }),
        })
    }

    /// Starts the thread that runs due jobs.
    pub fn start(self: &Arc<Self>) {
        let inner = Arc::clone(&self.inner);
        thread::spawn(move || {
            loop {
                for job in inner.take_due(unix_now()) {
                    inner.spawn_run(job);
                }
                thread::sleep(TICK);
            }
        });
    }

    /// Adds a job; errors on a bad schedule, URL or empty command.
    pub fn add(&self, n: NewJob) -> Result<Job, String> {
        let schedule = Schedule::parse(&n.schedule)?;
        validate(&n.action)?;
        let name = match n.name.trim() {
            "" => schedule.describe_in("en"),
            s => s.to_string(),
        };
        let mut st = self.inner.state.lock().unwrap();
        st.stored.next_id += 1;
        let job = Job {
            id: st.stored.next_id,
            name,
            schedule: n.schedule.trim().to_string(),
            action: n.action,
            notify: n.notify,
            enabled: true,
            source: n.source,
            last: None,
            runs: 0,
        };
        st.due
            .insert(job.id, next_due(&schedule, unix_now() as i64));
        st.stored.jobs.push(job.clone());
        self.inner.save(&st);
        Ok(job)
    }

    pub fn remove(&self, id: u64) -> bool {
        let mut st = self.inner.state.lock().unwrap();
        let before = st.stored.jobs.len();
        st.stored.jobs.retain(|j| j.id != id);
        st.due.remove(&id);
        let removed = st.stored.jobs.len() != before;
        if removed {
            self.inner.save(&st);
        }
        removed
    }

    pub fn set_enabled(&self, id: u64, on: bool) -> bool {
        let mut st = self.inner.state.lock().unwrap();
        let Some(job) = st.stored.jobs.iter_mut().find(|j| j.id == id) else {
            return false;
        };
        job.enabled = on;
        let due = on.then(|| job_due(job, unix_now()));
        match due {
            Some(d) => st.due.insert(id, d),
            None => st.due.remove(&id),
        };
        self.inner.save(&st);
        true
    }

    /// Runs a job now, in the background (not counted against its schedule).
    pub fn run_now(&self, id: u64) -> bool {
        let job = {
            let mut st = self.inner.state.lock().unwrap();
            if st.running.contains(&id) {
                return false;
            }
            let Some(job) = st.stored.jobs.iter().find(|j| j.id == id).cloned() else {
                return false;
            };
            st.running.insert(id);
            job
        };
        self.inner.spawn_run(job);
        true
    }

    pub fn list(&self) -> Vec<Job> {
        let mut jobs = self.inner.state.lock().unwrap().stored.jobs.clone();
        jobs.sort_by_key(|j| j.id);
        jobs
    }

    /// Unix seconds of the job's next run (`None` when disabled or never).
    pub fn next_run(&self, job: &Job) -> Option<u64> {
        if !job.enabled {
            return None;
        }
        let stored = self.inner.state.lock().unwrap().due.get(&job.id).copied();
        stored.unwrap_or_else(|| job_due(job, unix_now()))
    }
}

impl Inner {
    /// The enabled jobs that are due and not running; marks them running and schedules
    /// their next run from `now`, so a slot missed while asleep runs once.
    fn take_due(&self, now: u64) -> Vec<Job> {
        let mut st = self.state.lock().unwrap();
        let State {
            stored,
            running,
            due,
        } = &mut *st;
        let mut out = Vec::new();
        for job in stored.jobs.iter().filter(|j| j.enabled) {
            let next = *due.entry(job.id).or_insert_with(|| job_due(job, now));
            if running.contains(&job.id) || next.is_none_or(|t| t > now) {
                continue;
            }
            running.insert(job.id);
            due.insert(job.id, job_due(job, now));
            out.push(job.clone());
        }
        out
    }

    fn spawn_run(self: &Arc<Self>, job: Job) {
        let me = Arc::clone(self);
        thread::spawn(move || me.run(job));
    }

    fn run(&self, job: Job) {
        let at = unix_now();
        let start = Instant::now();
        let (ok, detail) = match &job.action {
            Action::Notify {
                title,
                message,
                sticky,
            } => {
                let mut n = Notification::new(title.clone(), message.clone()).source(Some(
                    job.source.clone().unwrap_or_else(|| "Schedule".into()),
                ));
                if *sticky {
                    n = n.sticky();
                }
                self.notifier.send(n);
                (true, "shown".to_string())
            }
            Action::Http { .. } => run_http(&job.action),
            Action::Command { command, workdir } => run_command(command, workdir.as_deref()),
        };
        let secs = start.elapsed().as_secs();
        if !matches!(job.action, Action::Notify { .. }) {
            self.report(&job, ok, &detail, secs);
        }
        self.finish(
            job.id,
            Run {
                at,
                ok,
                detail,
                secs,
            },
        );
    }

    /// The bubble (per the job's policy) for a request or command, or else a History entry.
    fn report(&self, job: &Job, ok: bool, detail: &str, secs: u64) {
        let source = job.source.clone().unwrap_or_else(|| "Schedule".into());
        let bubble = match job.notify {
            NotifyWhen::Always => true,
            NotifyWhen::Failure => !ok,
            NotifyWhen::Never => false,
        };
        if bubble {
            let mood = if ok { Mood::Good } else { Mood::Bad };
            let mut n = Notification::new(job.name.clone(), detail.to_string())
                .source(Some(source.clone()))
                .mood(Some(mood));
            if job.notify == NotifyWhen::Failure {
                n = n.sticky();
            }
            self.notifier.send(n);
            // The bubble is in History already.
            return;
        }
        let mut outcome = if ok { "done" } else { "failed" }.to_string();
        if secs >= 2 {
            outcome.push_str(&format!(" · {}", crate::watch::human_secs(secs)));
        }
        let source = match &job.source {
            Some(s) => format!("{s} · schedule"),
            None => source,
        };
        self.notifier.record(crate::mcp::HistoryEntry {
            id: 0,
            at: 0,
            source: Some(source),
            title: job.name.clone(),
            message: detail.to_string(),
            outcome,
        });
    }

    fn finish(&self, id: u64, run: Run) {
        {
            let mut st = self.state.lock().unwrap();
            st.running.remove(&id);
            let Some(job) = st.stored.jobs.iter_mut().find(|j| j.id == id) else {
                return;
            };
            job.last = Some(run);
            job.runs += 1;
            self.save(&st);
        }
        self.notifier.panel_changed();
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

fn validate(action: &Action) -> Result<(), String> {
    match action {
        Action::Notify { title, message, .. } => {
            if title.trim().is_empty() && message.trim().is_empty() {
                return Err("a reminder needs a title or a message".into());
            }
        }
        Action::Http { method, url, .. } => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!(
                    "the URL must start with http:// or https:// (got \"{url}\")"
                ));
            }
            if !METHODS.contains(&method.to_ascii_uppercase().as_str()) {
                return Err(format!(
                    "unknown HTTP method {method}; use one of {}",
                    METHODS.join(", ")
                ));
            }
        }
        Action::Command { command, .. } => {
            if command.trim().is_empty() {
                return Err("the command is empty".into());
            }
        }
    }
    Ok(())
}

/// The job's next run after `now` (unix seconds), or `None` for a bad schedule or none.
fn job_due(job: &Job, now: u64) -> Option<u64> {
    next_due(&Schedule::parse(&job.schedule).ok()?, now as i64)
}

/// The next matching local time after `now`, as unix seconds. Times that don't exist (the
/// DST gap) are skipped, and a time that repeats (DST end) runs only the first time.
fn next_due(schedule: &Schedule, now: i64) -> Option<u64> {
    let mut t = Local.timestamp_opt(now, 0).single()?.naive_local();
    for _ in 0..8 {
        let n = schedule.next_after(t)?;
        if let Some(at) = Local.from_local_datetime(&n).earliest()
            && at.timestamp() > now
        {
            return u64::try_from(at.timestamp()).ok();
        }
        t = n;
    }
    None
}

fn run_http(action: &Action) -> (bool, String) {
    let Action::Http {
        method,
        url,
        headers,
        body,
        expect_status,
        contains,
    } = action
    else {
        return (false, "not a request".into());
    };
    let start = Instant::now();
    let mut req = ureq::http::Request::builder()
        .method(method.to_ascii_uppercase().as_str())
        .uri(url.as_str());
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let agent = crate::watch::http_agent(HTTP_TIMEOUT);
    let resp = match body {
        Some(b) => req.body(b.clone()).map(|r| agent.run(r)),
        None => req.body(()).map(|r| agent.run(r)),
    };
    let mut resp = match resp {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return (false, format!("unreachable: {e}")),
        Err(e) => return (false, format!("bad request: {e}")),
    };
    let status = resp.status();
    let code = status.as_u16();
    let mut ok = match expect_status {
        Some(s) => code == *s,
        None => status.is_success(),
    };
    let mut missing = None;
    if let Some(needle) = contains {
        let mut buf = Vec::new();
        let _ = resp
            .body_mut()
            .as_reader()
            .take(BODY_LIMIT)
            .read_to_end(&mut buf);
        if !String::from_utf8_lossy(&buf).contains(needle.as_str()) {
            ok = false;
            missing = Some(needle);
        }
    }
    let mut detail = format!(
        "{code} {} · {} ms",
        status.canonical_reason().unwrap_or(""),
        start.elapsed().as_millis()
    )
    .replace("  ·", " ·");
    if let Some(needle) = missing {
        detail.push_str(&format!(" · body has no \"{needle}\""));
    }
    (ok, detail)
}

fn run_command(command: &str, workdir: Option<&str>) -> (bool, String) {
    let mut cmd = crate::watch::shell(command);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = workdir.filter(|d| !d.trim().is_empty()) {
        cmd.current_dir(crate::local::expand(dir));
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("failed to run: {e}")),
    };
    let readers = [
        child.stdout.take().map(|o| read_all(Box::new(o))),
        child.stderr.take().map(|e| read_all(Box::new(e))),
    ];
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return (false, e.to_string()),
        }
        if start.elapsed() > COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return (
                false,
                format!(
                    "timed out after {}",
                    crate::watch::human_secs(COMMAND_TIMEOUT.as_secs())
                ),
            );
        }
        thread::sleep(Duration::from_millis(50));
    };
    let line = readers
        .into_iter()
        .flatten()
        .map(|r| r.join().unwrap_or_default())
        .find_map(|out| first_line(&out));
    let mut detail = match status.code() {
        Some(c) => format!("exit {c}"),
        None => "killed".to_string(),
    };
    if let Some(line) = line {
        detail.push_str(" · ");
        detail.push_str(&line);
    }
    (status.success(), detail)
}

fn read_all(mut r: Box<dyn Read + Send>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.by_ref().take(BODY_LIMIT).read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// The first non-empty line, cut to about 120 characters.
fn first_line(out: &str) -> Option<String> {
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(if line.chars().count() > 120 {
        format!("{}…", line.chars().take(119).collect::<String>())
    } else {
        line.to_string()
    })
}

fn unix_now() -> u64 {
    crate::watch::now_unix()
}

/// Where the jobs are kept: `cron.json` next to the settings.
pub fn default_path() -> Option<PathBuf> {
    crate::watch::default_path().and_then(|p| Some(p.parent()?.join("cron.json")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{self, Event};
    use chrono::{NaiveDate, NaiveDateTime};
    use std::io::Write;
    use std::sync::mpsc::Receiver;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    fn next(expr: &str, t: NaiveDateTime) -> Option<NaiveDateTime> {
        Schedule::parse(expr).unwrap().next_after(t)
    }

    fn new_job(schedule: &str, action: Action, notify: NotifyWhen) -> NewJob {
        NewJob {
            name: "job".into(),
            schedule: schedule.into(),
            action,
            notify,
            source: None,
        }
    }

    fn command(c: &str) -> Action {
        Action::Command {
            command: c.into(),
            workdir: None,
        }
    }

    fn http(url: String, contains: Option<&str>) -> Action {
        Action::Http {
            method: "GET".into(),
            url,
            headers: vec![("X-Test".into(), "1".into())],
            body: None,
            expect_status: None,
            contains: contains.map(String::from),
        }
    }

    /// Runs the job and waits for its `PanelChanged`; returns the events before it.
    fn run_and_wait(cron: &Cron, id: u64, rx: &Receiver<Event>) -> Vec<Event> {
        assert!(cron.run_now(id));
        let mut events = Vec::new();
        loop {
            match rx
                .recv_timeout(Duration::from_secs(10))
                .expect("no PanelChanged")
            {
                Event::PanelChanged => return events,
                e => events.push(e),
            }
        }
    }

    fn last(cron: &Cron, id: u64) -> Run {
        cron.list()
            .into_iter()
            .find(|j| j.id == id)
            .unwrap()
            .last
            .unwrap()
    }

    /// Answers `count` requests with `status` and `body`.
    fn serve(status: &'static str, body: &'static str, count: usize) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        thread::spawn(move || {
            for stream in listener.incoming().take(count) {
                let mut s = stream.unwrap();
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn parse_errors_are_clear() {
        let err = |e: &str| Schedule::parse(e).unwrap_err();
        assert_eq!(err("61 * * * *"), "minute 61 is out of range 0-59");
        assert_eq!(err("* 9-24 * * *"), "hour 24 is out of range 0-23");
        assert_eq!(err("* * 0 * *"), "day of month 0 is out of range 1-31");
        assert_eq!(err("* * * 13 *"), "month 13 is out of range 1-12");
        assert_eq!(err("* * * * 8"), "weekday 8 is out of range 0-7");
        assert!(err("* * * *").starts_with("expected 5 fields: minute hour day month weekday"));
        assert!(err("0 * * * * *").starts_with("expected 5 fields"));
        assert!(err("@sometimes").contains("unknown shortcut"));
        assert!(err("x * * * *").contains("illegal character"));
        assert!(err("*/0 * * * *").contains("Step cannot be zero"));
        for ok in [
            "*/15 9-17 * * 1-5",
            "0 0 1,15 jan-Mar sun",
            "5/10 * * * 7",
            "@hourly",
            "@Daily",
            "@midnight",
            "@weekly",
            "@monthly",
            "@yearly",
            "@annually",
        ] {
            assert!(Schedule::parse(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn next_times() {
        let t = at(2026, 1, 31, 23, 59);
        assert_eq!(next("0 0 1 * *", t), Some(at(2026, 2, 1, 0, 0)));
        assert_eq!(next("@daily", t), Some(at(2026, 2, 1, 0, 0)));
        assert_eq!(
            next("*/15 * * * *", at(2026, 1, 1, 10, 0)),
            Some(at(2026, 1, 1, 10, 15))
        );
        assert_eq!(
            next("*/15 * * * *", at(2026, 1, 1, 10, 59)),
            Some(at(2026, 1, 1, 11, 0))
        );
        // Strictly after, on a whole minute.
        let mid = at(2026, 1, 1, 10, 0) + chrono::TimeDelta::seconds(30);
        assert_eq!(next("* * * * *", mid), Some(at(2026, 1, 1, 10, 1)));
        // Leap day: next is 2028.
        assert_eq!(
            next("0 0 29 2 *", at(2026, 3, 1, 0, 0)),
            Some(at(2028, 2, 29, 0, 0))
        );
        assert_eq!(next("0 0 30 2 *", at(2026, 1, 1, 0, 0)), None);
        // 2026-10-23 is a Friday: weekdays skip to Monday.
        assert_eq!(
            next("0 9 * * 1-5", at(2026, 10, 23, 9, 0)),
            Some(at(2026, 10, 26, 9, 0))
        );
        assert_eq!(
            next("0 9 * * MON-FRI", at(2026, 10, 24, 0, 0)),
            Some(at(2026, 10, 26, 9, 0))
        );
        assert_eq!(
            next("0 0 * * 7", at(2026, 10, 22, 0, 0)),
            Some(at(2026, 10, 25, 0, 0))
        );
        // Day of month or day of week: the 13th, or any Friday.
        assert_eq!(
            next("0 0 13 * 5", at(2026, 10, 22, 0, 0)),
            Some(at(2026, 10, 23, 0, 0))
        );
        assert_eq!(
            next("0 0 13 * 5", at(2026, 11, 7, 0, 0)),
            Some(at(2026, 11, 13, 0, 0))
        );
    }

    #[test]
    fn descriptions() {
        let d = |e: &str, tr: bool| {
            Schedule::parse(e)
                .unwrap()
                .describe_in(if tr { "tr" } else { "en" })
        };
        assert_eq!(d("* * * * *", false), "every minute");
        assert_eq!(d("0 9 * * *", false), "every day at 09:00");
        assert_eq!(d("0 9 * * 1-5", false), "weekdays at 09:00");
        assert_eq!(d("30 8 * * mon", false), "Mondays at 08:30");
        assert_eq!(d("* * * * *", true), "her dakika");
        assert_eq!(d("*/15 * * * *", true), "her 15 dakikada");
        assert_eq!(d("30 * * * *", true), "her saat :30'da");
        assert_eq!(d("0 9 * * *", true), "her gün 09:00'da");
        assert_eq!(d("@daily", true), "her gün 00:00'da");
        assert_eq!(d("0 9 * * 1-5", true), "hafta içi 09:00'da");
        assert_eq!(d("30 8 * * mon", true), "pazartesi 08:30'da");
        assert_eq!(d("0 0 1 * *", true), "her ayın 1'inde 00:00'da");
        assert_eq!(d("15 14 6 * *", true), "her ayın 6'sında 14:15'te");
        // Other shapes fall back to croner's English.
        assert_eq!(d("*/15 9-17 * * 1-5", true), d("*/15 9-17 * * 1-5", false));
    }

    #[test]
    fn add_list_remove_persist() {
        let dir = std::env::temp_dir().join(format!("dost-cron-{}", std::process::id()));
        let path = dir.join("cron.json");
        let (tx, _rx) = notify::channel();
        let cron = Cron::new(tx.clone(), Some(path.clone()));
        let bad = |s: &str, a: Action| cron.add(new_job(s, a, NotifyWhen::Failure)).unwrap_err();
        assert!(bad("61 * * * *", command("true")).contains("minute 61"));
        assert!(bad("@daily", command(" ")).contains("empty"));
        assert!(bad("@daily", http("ftp://x".into(), None)).contains("http://"));
        let empty = Action::Notify {
            title: String::new(),
            message: String::new(),
            sticky: false,
        };
        assert!(bad("@daily", empty).contains("title or a message"));

        let mut n = new_job("0 9 * * *", command("true"), NotifyWhen::Never);
        n.name = String::new();
        let a = cron.add(n).unwrap();
        assert_eq!(a.name, "every day at 09:00");
        let b = cron
            .add(new_job("@hourly", command("true"), NotifyWhen::Always))
            .unwrap();
        assert_eq!(b.id, a.id + 1);
        let now = unix_now();
        let due = cron.next_run(&a).unwrap();
        assert!(due > now && due <= now + 25 * 3600);
        assert!(cron.set_enabled(a.id, false));
        assert_eq!(cron.next_run(&cron.list()[0]), None);

        let again = Cron::new(tx.clone(), Some(path.clone()));
        let jobs = again.list();
        assert_eq!(jobs.len(), 2);
        assert!(!jobs[0].enabled);
        assert_eq!(jobs[1].schedule, "@hourly");
        assert!(again.remove(a.id));
        assert!(!again.remove(a.id));
        let c = again
            .add(new_job("@daily", command("true"), NotifyWhen::Never))
            .unwrap();
        assert_eq!(c.id, b.id + 1);
        assert_eq!(Cron::new(tx, Some(path)).list().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn due_jobs_run_once_and_reschedule() {
        let (tx, _rx) = notify::channel();
        let cron = Cron::new(tx, None);
        let job = cron
            .add(new_job("* * * * *", command("true"), NotifyWhen::Never))
            .unwrap();
        let now = unix_now();
        assert!(cron.inner.take_due(now).is_empty());
        // Hours later (woken from sleep): one run, then the next minute.
        let later = now + 5 * 3600;
        assert_eq!(cron.inner.take_due(later).len(), 1);
        assert!(cron.inner.take_due(later + 120).is_empty(), "still running");
        cron.inner.state.lock().unwrap().running.clear();
        let due = cron.next_run(&job).unwrap();
        assert!(due > later && due <= later + 60);
    }

    #[test]
    fn notify_job() {
        let (tx, rx) = notify::channel();
        let cron = Cron::new(tx, None);
        let action = Action::Notify {
            title: "Stand up".into(),
            message: "stretch".into(),
            sticky: true,
        };
        let job = cron
            .add(new_job("@hourly", action, NotifyWhen::Never))
            .unwrap();
        let events = run_and_wait(&cron, job.id, &rx);
        assert_eq!(events.len(), 1);
        let Event::Notify(n) = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(n.title, "Stand up");
        assert_eq!(n.source.as_deref(), Some("Schedule"));
        assert_eq!(n.timeout, notify::Timeout::Sticky);
        let run = last(&cron, job.id);
        assert!(run.ok);
        assert_eq!(run.detail, "shown");
        assert_eq!(cron.list()[0].runs, 1);
    }

    #[test]
    fn http_jobs() {
        let (tx, rx) = notify::channel();
        let cron = Cron::new(tx, None);

        let url = serve("200 OK", "all good", 2);
        let ok = cron
            .add(new_job(
                "@hourly",
                http(url.clone(), Some("good")),
                NotifyWhen::Always,
            ))
            .unwrap();
        let events = run_and_wait(&cron, ok.id, &rx);
        let run = last(&cron, ok.id);
        assert!(run.ok, "{}", run.detail);
        assert!(run.detail.starts_with("200 OK · "), "{}", run.detail);
        let Event::Notify(n) = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(n.mood, Mood::Good);
        assert_eq!(n.title, "job");
        assert_eq!(n.source.as_deref(), Some("Schedule"));
        assert!(
            !events.iter().any(|e| matches!(e, Event::Record(_))),
            "a bubble is in History already"
        );

        let missing = cron
            .add(new_job(
                "@hourly",
                http(url, Some("bad")),
                NotifyWhen::Never,
            ))
            .unwrap();
        let events = run_and_wait(&cron, missing.id, &rx);
        assert!(!last(&cron, missing.id).ok);
        assert_eq!(events.len(), 1, "Never: only the History record");
        assert!(matches!(&events[0], Event::Record(r) if r.outcome == "failed"));

        let url = serve("500 Internal Server Error", "oops", 1);
        let mut n = new_job("@hourly", http(url, None), NotifyWhen::Failure);
        n.source = Some("OpenCode".into());
        let err = cron.add(n).unwrap();
        let events = run_and_wait(&cron, err.id, &rx);
        let run = last(&cron, err.id);
        assert!(!run.ok);
        assert!(
            run.detail.starts_with("500 Internal Server Error"),
            "{}",
            run.detail
        );
        let Event::Notify(n) = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(n.mood, Mood::Bad);
        assert_eq!(n.timeout, notify::Timeout::Sticky);
        assert_eq!(n.source.as_deref(), Some("OpenCode"));
    }

    #[cfg(unix)]
    #[test]
    fn command_jobs() {
        let (tx, rx) = notify::channel();
        let cron = Cron::new(tx, None);
        let ok = cron
            .add(new_job(
                "@hourly",
                command("echo; echo hello"),
                NotifyWhen::Failure,
            ))
            .unwrap();
        let events = run_and_wait(&cron, ok.id, &rx);
        let run = last(&cron, ok.id);
        assert!(run.ok);
        assert_eq!(run.detail, "exit 0 · hello");
        assert_eq!(events.len(), 1, "Failure: no bubble when it worked");
        assert!(!cron.run_now(999));

        let dir = std::env::temp_dir();
        let pwd = cron
            .add(new_job(
                "@hourly",
                Action::Command {
                    command: "pwd; exit 3".into(),
                    workdir: Some(dir.to_string_lossy().into()),
                },
                NotifyWhen::Failure,
            ))
            .unwrap();
        let events = run_and_wait(&cron, pwd.id, &rx);
        let run = last(&cron, pwd.id);
        assert!(!run.ok);
        assert!(run.detail.starts_with("exit 3 · /"), "{}", run.detail);
        assert!(matches!(&events[0], Event::Notify(n) if n.mood == Mood::Bad));

        let slow = cron
            .add(new_job("@hourly", command("sleep 5"), NotifyWhen::Never))
            .unwrap();
        run_and_wait(&cron, slow.id, &rx);
        assert!(last(&cron, slow.id).detail.starts_with("timed out"));
    }
}
