//! The phone: important notifications go to the user's phone while they are away, through
//! ntfy (https://ntfy.sh or a self-hosted server) or a Telegram bot, and questions can be
//! answered from there. Nothing listens on a port: Dost publishes over HTTPS and reads the
//! answers back by subscribing (ntfy) or long-polling (Telegram).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::watch::http_agent;

/// When notifications go to the phone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhoneWhen {
    /// While the user is away (idle for 5 minutes) or the screen is locked.
    #[default]
    Away,
    Always,
    Never,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NtfySettings {
    pub enabled: bool,
    /// e.g. "https://ntfy.sh".
    pub server: String,
    /// Secret-ish topic name (anyone who knows it can read and answer).
    pub topic: String,
    /// Access token for protected servers (Bearer).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
}

impl Default for NtfySettings {
    fn default() -> Self {
        NtfySettings {
            enabled: false,
            server: "https://ntfy.sh".into(),
            topic: String::new(),
            token: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramSettings {
    pub enabled: bool,
    /// Bot token from @BotFather.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// The chat the bot talks to; set by sending /start to the bot.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub chat_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhoneSettings {
    pub when: PhoneWhen,
    pub ntfy: NtfySettings,
    pub telegram: TelegramSettings,
}

impl PhoneSettings {
    pub fn any_enabled(&self) -> bool {
        self.when != PhoneWhen::Never
            && ((self.ntfy.enabled && !self.ntfy.topic.trim().is_empty())
                || (self.telegram.enabled && !self.telegram.token.trim().is_empty()))
    }
}

/// A notification for the phone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteMessage {
    /// The notification's id in the app; answers refer to it.
    pub id: u64,
    pub source: Option<String>,
    pub title: String,
    pub body: String,
    /// Answer buttons (a question).
    pub choices: Vec<String>,
    /// A question that may be answered with typed text (Telegram: reply to the message).
    pub allow_text: bool,
    pub url: Option<String>,
    /// "good", "bad" or "neutral".
    pub mood: &'static str,
}

/// Something that came back from the phone.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteEvent {
    /// A question was answered: a choice's label, or typed text.
    Answer { id: u64, answer: String },
    /// The Telegram bot got /start from this chat (fills in `chat_id`).
    Paired { chat_id: String },
    /// A channel failed (shown once in the settings as the channel's status).
    Error {
        channel: &'static str,
        error: String,
    },
}

/// The phone channels. Cheap to call from the UI thread: network work runs on threads.
pub struct Remote {
    shared: Arc<Shared>,
    jobs: Sender<Job>,
}

impl Remote {
    pub fn new(events: Sender<RemoteEvent>) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            events,
        });
        let (jobs, rx) = mpsc::channel();
        let s = shared.clone();
        thread::spawn(move || worker(s, rx));
        Remote { shared, jobs }
    }

    /// (Re)starts the listeners for the new settings; stops the ones turned off.
    pub fn configure(&self, s: &PhoneSettings) {
        let mut inner = self.shared.lock();
        let old_ntfy = ntfy_key(&inner.settings);
        let old_tg = tg_key(&inner.settings);
        let old_chat = inner.settings.telegram.chat_id.trim().to_string();
        inner.settings = s.clone();
        let new_ntfy = ntfy_key(s);
        let new_tg = tg_key(s);
        if old_chat != s.telegram.chat_id.trim() {
            inner.tg_unpaired_told = false;
            inner.tg_open.clear();
        }
        if old_ntfy != new_ntfy {
            inner.ntfy_gen += 1;
            if old_ntfy.as_ref().map(|k| (&k.0, &k.1)) != new_ntfy.as_ref().map(|k| (&k.0, &k.1)) {
                inner.ntfy_open.clear();
            }
            if let Some((server, topic, token)) = new_ntfy {
                let (shared, jobs, generation) =
                    (self.shared.clone(), self.jobs.clone(), inner.ntfy_gen);
                thread::spawn(move || ntfy_listen(shared, jobs, generation, server, topic, token));
            }
        }
        if old_tg != new_tg {
            inner.tg_gen += 1;
            if let Some(token) = new_tg {
                let (shared, jobs, generation) =
                    (self.shared.clone(), self.jobs.clone(), inner.tg_gen);
                thread::spawn(move || tg_listen(shared, jobs, generation, token));
            }
        }
    }

    /// Sends a notification to every enabled channel.
    pub fn send(&self, m: RemoteMessage) {
        if !self.shared.lock().settings.any_enabled() {
            return;
        }
        let _ = self.jobs.send(Job::Send { m, test: false });
    }

    /// A question sent earlier was answered or dropped elsewhere: its buttons go away and it
    /// says how it ended ("answered: Allow", "expired").
    pub fn resolve(&self, id: u64, outcome: &str) {
        let _ = self.jobs.send(Job::Resolve {
            id,
            outcome: outcome.to_string(),
        });
    }

    /// Sends a test message to every enabled channel.
    pub fn test(&self) {
        let m = RemoteMessage {
            title: "Woof!".into(),
            body: "Dost can reach your phone.".into(),
            mood: "good",
            ..Default::default()
        };
        let _ = self.jobs.send(Job::Send { m, test: true });
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        let mut inner = self.shared.lock();
        inner.ntfy_gen += 1;
        inner.tg_gen += 1;
    }
}

const NTFY: &str = "ntfy";
const TELEGRAM: &str = "telegram";
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
/// ntfy sends a keepalive every 45 s; the stream is reopened after this anyway.
const STREAM_TIMEOUT: Duration = Duration::from_secs(600);
const TG_POLL: u64 = 50;
const TG_TIMEOUT: Duration = Duration::from_secs(70);
const ERROR_EVERY: Duration = Duration::from_secs(60);
const PAIRED: &str = "Woof! Dost will send notifications here.";
const REPLY_HINT: &str = "Reply to this message to answer in your own words.";
/// ntfy shows at most 3 actions.
const NTFY_ACTIONS: usize = 3;
const TG_BODY_CHARS: usize = 3000;

struct Shared {
    inner: Mutex<Inner>,
    events: Sender<RemoteEvent>,
}

#[derive(Default)]
struct Inner {
    settings: PhoneSettings,
    ntfy_gen: u64,
    tg_gen: u64,
    /// Questions published to ntfy: id -> title.
    ntfy_open: HashMap<u64, String>,
    /// Questions sent to Telegram that still have their buttons.
    tg_open: HashMap<u64, TgQuestion>,
    tg_unpaired_told: bool,
    last_error: HashMap<&'static str, Instant>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, e: RemoteEvent) {
        let _ = self.events.send(e);
    }

    /// Reports a channel's failure, at most once a minute.
    fn report(&self, channel: &'static str, error: String) {
        let mut inner = self.lock();
        if inner
            .last_error
            .get(channel)
            .is_none_or(|t| t.elapsed() >= ERROR_EVERY)
        {
            inner.last_error.insert(channel, Instant::now());
            drop(inner);
            self.emit(RemoteEvent::Error { channel, error });
        }
    }

    fn alive(&self, channel: &'static str, generation: u64) -> bool {
        let inner = self.lock();
        if channel == NTFY {
            inner.ntfy_gen == generation
        } else {
            inner.tg_gen == generation
        }
    }

    /// Sleeps `secs` seconds; `false` when the listener should stop.
    fn nap(&self, channel: &'static str, generation: u64, secs: u64) -> bool {
        for _ in 0..secs {
            thread::sleep(Duration::from_secs(1));
            if !self.alive(channel, generation) {
                return false;
            }
        }
        self.alive(channel, generation)
    }
}

enum Job {
    Send { m: RemoteMessage, test: bool },
    Resolve { id: u64, outcome: String },
    TgCall { method: &'static str, body: Value },
}

#[derive(Clone, Debug, PartialEq)]
struct TgQuestion {
    message_id: i64,
    chat_id: String,
    /// The message's HTML text, to append the outcome to.
    text: String,
    choices: Vec<String>,
    allow_text: bool,
    url: Option<String>,
}

/// What a listener does after a Telegram update.
#[derive(Debug, PartialEq)]
enum TgAction {
    Event(RemoteEvent),
    Call(&'static str, Value),
    Forget(u64),
}

fn ntfy_server(s: &NtfySettings) -> String {
    let server = s.server.trim().trim_end_matches('/');
    if server.is_empty() {
        "https://ntfy.sh".into()
    } else {
        server.into()
    }
}

/// What the ntfy listener depends on: (server, topic, token), `None` when it is off.
fn ntfy_key(s: &PhoneSettings) -> Option<(String, String, String)> {
    let n = &s.ntfy;
    let topic = n.topic.trim();
    (s.when != PhoneWhen::Never && n.enabled && !topic.is_empty()).then(|| {
        (
            ntfy_server(n),
            topic.to_string(),
            n.token.trim().to_string(),
        )
    })
}

/// What the Telegram listener depends on: the token, `None` when it is off.
fn tg_key(s: &PhoneSettings) -> Option<String> {
    let t = &s.telegram;
    let token = t.token.trim();
    (s.when != PhoneWhen::Never && t.enabled && !token.is_empty()).then(|| token.to_string())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_question(m: &RemoteMessage) -> bool {
    !m.choices.is_empty() || m.allow_text
}

fn full_title(m: &RemoteMessage) -> String {
    match m.source.as_deref().map(str::trim) {
        Some(src) if !src.is_empty() => format!("{src} · {}", m.title),
        _ => m.title.clone(),
    }
}

// ---- ntfy ----

fn ntfy_publish_body(server: &str, topic: &str, token: &str, m: &RemoteMessage) -> Value {
    let question = is_question(m);
    let mut tags = vec!["dog"];
    match m.mood {
        "good" => tags.push("white_check_mark"),
        "bad" => tags.push("x"),
        _ => {}
    }
    if question {
        tags.push("question");
    }
    let message = if m.body.trim().is_empty() {
        m.title.clone()
    } else {
        m.body.clone()
    };
    let mut body = json!({
        "topic": topic,
        "title": full_title(m),
        "message": message,
        "tags": tags,
        "priority": if question { 4 } else { 3 },
    });
    if let Some(url) = m.url.as_deref().filter(|u| !u.trim().is_empty()) {
        body["click"] = json!(url.trim());
    }
    if !m.choices.is_empty() {
        let actions: Vec<Value> = m
            .choices
            .iter()
            .take(NTFY_ACTIONS)
            .map(|c| {
                let mut a = json!({
                    "action": "http",
                    "label": c,
                    "url": format!("{server}/{topic}-answers"),
                    "method": "POST",
                    "body": format!("{}\n{c}", m.id),
                    "clear": true,
                });
                if !token.is_empty() {
                    a["headers"] = json!({ "Authorization": format!("Bearer {token}") });
                }
                a
            })
            .collect();
        body["actions"] = json!(actions);
    }
    body
}

/// "<id>\n<answer>" -> (id, answer).
fn parse_answer(s: &str) -> Option<(u64, String)> {
    let (id, answer) = s.split_once('\n')?;
    let id = id.trim().parse().ok()?;
    let answer = answer.trim();
    (!answer.is_empty()).then(|| (id, answer.to_string()))
}

/// A line of ntfy's JSON stream: for a message, its id and the answer it carries (if any).
fn ntfy_line(line: &str) -> Option<(String, Option<(u64, String)>)> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v["event"] != "message" {
        return None;
    }
    let id = v["id"].as_str()?.to_string();
    Some((id, v["message"].as_str().and_then(parse_answer)))
}

fn ntfy_post(agent: &ureq::Agent, server: &str, token: &str, body: &Value) -> Result<(), String> {
    let mut req = agent
        .post(server)
        .header("Content-Type", "application/json")
        .header("User-Agent", "dost");
    if !token.is_empty() {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let mut resp = req.send(body.to_string()).map_err(|e| e.to_string())?;
    let code = resp.status().as_u16();
    if code == 200 {
        return Ok(());
    }
    let text = resp
        .body_mut()
        .with_config()
        .limit(64 << 10)
        .read_to_string()
        .unwrap_or_default();
    let error = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string));
    Err(match error {
        Some(e) => format!("HTTP {code}: {e}"),
        None => format!("HTTP {code}"),
    })
}

fn ntfy_listen(
    shared: Arc<Shared>,
    jobs: Sender<Job>,
    generation: u64,
    server: String,
    topic: String,
    token: String,
) {
    let agent = http_agent(STREAM_TIMEOUT);
    let mut since = unix_now().to_string();
    let mut backoff = 2;
    while shared.alive(NTFY, generation) {
        let url = format!("{server}/{topic}-answers/json?since={since}");
        let mut req = agent.get(&url).header("User-Agent", "dost");
        if !token.is_empty() {
            req = req.header("Authorization", &format!("Bearer {token}"));
        }
        let resp = req
            .call()
            .map_err(|e| e.to_string())
            .and_then(|r| match r.status().as_u16() {
                200 => Ok(r),
                code => Err(format!("HTTP {code}")),
            });
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                if !shared.alive(NTFY, generation) {
                    return;
                }
                shared.report(NTFY, e);
                if !shared.nap(NTFY, generation, backoff) {
                    return;
                }
                backoff = (backoff * 2).min(60);
                continue;
            }
        };
        backoff = 2;
        let reader = BufReader::new(resp.into_body().into_reader());
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if !shared.alive(NTFY, generation) {
                return;
            }
            let Some((msg_id, answer)) = ntfy_line(&line) else {
                continue;
            };
            since = msg_id;
            if let Some((id, answer)) = answer {
                shared.lock().ntfy_open.remove(&id);
                let _ = jobs.send(Job::Resolve {
                    id,
                    outcome: format!("answered: {answer}"),
                });
                shared.emit(RemoteEvent::Answer { id, answer });
            }
        }
        if !shared.nap(NTFY, generation, 1) {
            return;
        }
    }
}

// ---- Telegram ----

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// "a:<id>:<index>" -> (id, index).
fn parse_callback(data: &str) -> Option<(u64, usize)> {
    let rest = data.strip_prefix("a:")?;
    let (id, i) = rest.split_once(':')?;
    Some((id.parse().ok()?, i.parse().ok()?))
}

fn tg_url(url: Option<&str>) -> Option<&str> {
    url.map(str::trim)
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
}

fn tg_text(m: &RemoteMessage) -> String {
    let mut text = format!("<b>{}</b>", html_escape(&full_title(m)));
    let body = m.body.trim();
    if !body.is_empty() {
        let body: String = body.chars().take(TG_BODY_CHARS).collect();
        text.push('\n');
        text.push_str(&html_escape(&body));
    }
    if m.allow_text {
        text.push_str(&format!("\n\n<i>{REPLY_HINT}</i>"));
    }
    text
}

fn url_button(url: &str) -> Value {
    json!({ "text": "Open ↗", "url": url })
}

/// The sendMessage request for a notification.
fn tg_send_body(m: &RemoteMessage, chat_id: &str, with_url: bool) -> Value {
    let mut body = json!({
        "chat_id": chat_id,
        "text": tg_text(m),
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    let mut rows = Vec::new();
    if !m.choices.is_empty() {
        let row: Vec<Value> = m
            .choices
            .iter()
            .enumerate()
            .map(|(i, c)| json!({ "text": c, "callback_data": format!("a:{}:{i}", m.id) }))
            .collect();
        rows.push(Value::Array(row));
    }
    if let Some(url) = tg_url(m.url.as_deref()).filter(|_| with_url) {
        rows.push(json!([url_button(url)]));
    }
    if !rows.is_empty() {
        body["reply_markup"] = json!({ "inline_keyboard": rows });
    }
    body
}

/// The editMessageText request that closes a question: `suffix` (HTML) is appended and
/// the answer buttons go away (the link stays).
fn tg_edit_body(q: &TgQuestion, suffix: &str) -> Value {
    let mut body = json!({
        "chat_id": q.chat_id,
        "message_id": q.message_id,
        "text": format!("{}\n\n{suffix}", q.text),
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    if let Some(url) = tg_url(q.url.as_deref()) {
        body["reply_markup"] = json!({ "inline_keyboard": [[url_button(url)]] });
    }
    body
}

fn id_string(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

fn tg_answered(id: u64, q: &TgQuestion, answer: &str) -> Vec<TgAction> {
    vec![
        TgAction::Event(RemoteEvent::Answer {
            id,
            answer: answer.to_string(),
        }),
        TgAction::Forget(id),
        TgAction::Call(
            "editMessageText",
            tg_edit_body(q, &format!("✓ {}", html_escape(answer))),
        ),
    ]
}

/// What to do about one update from getUpdates. `chat_id` is the paired chat (may be empty).
fn tg_update(u: &Value, chat_id: &str, open: &HashMap<u64, TgQuestion>) -> Vec<TgAction> {
    let chat_id = chat_id.trim();
    let mut out = Vec::new();
    if let Some(msg) = u.get("message") {
        let chat = &msg["chat"];
        let from = id_string(&chat["id"]);
        let text = msg["text"].as_str().unwrap_or("").trim();
        let command = text.split_whitespace().next().unwrap_or("");
        if command == "/start" || command.starts_with("/start@") {
            if chat["type"] == "private"
                && !from.is_empty()
                && (chat_id.is_empty() || chat_id == from)
            {
                out.push(TgAction::Event(RemoteEvent::Paired {
                    chat_id: from.clone(),
                }));
                out.push(TgAction::Call(
                    "sendMessage",
                    json!({ "chat_id": from, "text": PAIRED }),
                ));
            }
            return out;
        }
        if chat_id.is_empty() || from != chat_id || text.is_empty() || text.starts_with('/') {
            return out;
        }
        let target = match msg["reply_to_message"]["message_id"].as_i64() {
            Some(mid) => open
                .iter()
                .find(|(_, q)| q.message_id == mid && q.allow_text),
            None => {
                let mut it = open.iter().filter(|(_, q)| q.allow_text);
                match (it.next(), it.next()) {
                    (Some(q), None) => Some(q),
                    _ => None,
                }
            }
        };
        if let Some((&id, q)) = target {
            out.extend(tg_answered(id, q, text));
        }
    } else if let Some(cb) = u.get("callback_query") {
        let from = id_string(&cb["message"]["chat"]["id"]);
        if chat_id.is_empty() || from != chat_id {
            return out;
        }
        let cb_id = cb["id"].as_str().unwrap_or("");
        let picked = cb["data"]
            .as_str()
            .and_then(parse_callback)
            .and_then(|(id, i)| {
                let q = open.get(&id)?;
                Some((id, q, q.choices.get(i)?.clone()))
            });
        match picked {
            Some((id, q, answer)) => {
                out.push(TgAction::Call(
                    "answerCallbackQuery",
                    json!({ "callback_query_id": cb_id }),
                ));
                out.extend(tg_answered(id, q, &answer));
            }
            None => out.push(TgAction::Call(
                "answerCallbackQuery",
                json!({ "callback_query_id": cb_id, "text": "This question is closed." }),
            )),
        }
    }
    out
}

/// Calls a Bot API method; returns its `result`.
fn tg_call(agent: &ureq::Agent, token: &str, method: &str, body: &Value) -> Result<Value, String> {
    let url = format!("https://api.telegram.org/bot{token}/{method}");
    let mut resp = agent
        .post(&url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "dost")
        .send(body.to_string())
        .map_err(|e| e.to_string().replace(token, "…"))?;
    let code = resp.status().as_u16();
    let text = resp
        .body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|_| format!("HTTP {code}"))?;
    if v["ok"].as_bool() == Some(true) {
        Ok(v["result"].clone())
    } else {
        Err(v["description"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {code}")))
    }
}

fn tg_listen(shared: Arc<Shared>, jobs: Sender<Job>, generation: u64, token: String) {
    let agent = http_agent(TG_TIMEOUT);
    let mut offset: Option<i64> = None;
    let mut backoff = 2;
    while shared.alive(TELEGRAM, generation) {
        let mut body = json!({
            "timeout": TG_POLL,
            "allowed_updates": ["message", "callback_query"],
        });
        if let Some(o) = offset {
            body["offset"] = json!(o);
        }
        let updates = match tg_call(&agent, &token, "getUpdates", &body) {
            Ok(v) => v,
            Err(e) => {
                if !shared.alive(TELEGRAM, generation) {
                    return;
                }
                shared.report(TELEGRAM, e);
                if !shared.nap(TELEGRAM, generation, backoff) {
                    return;
                }
                backoff = (backoff * 2).min(60);
                continue;
            }
        };
        backoff = 2;
        for u in updates.as_array().into_iter().flatten() {
            if let Some(uid) = u["update_id"].as_i64() {
                offset = Some(offset.map_or(uid + 1, |o| o.max(uid + 1)));
            }
            let mut inner = shared.lock();
            if inner.tg_gen != generation {
                return;
            }
            let actions = tg_update(u, &inner.settings.telegram.chat_id, &inner.tg_open);
            for a in &actions {
                if let TgAction::Forget(id) = a {
                    inner.tg_open.remove(id);
                }
            }
            drop(inner);
            for a in actions {
                match a {
                    TgAction::Event(e) => {
                        if let RemoteEvent::Answer { id, answer } = &e {
                            let _ = jobs.send(Job::Resolve {
                                id: *id,
                                outcome: format!("answered: {answer}"),
                            });
                        }
                        shared.emit(e);
                    }
                    TgAction::Call(method, body) => {
                        let _ = jobs.send(Job::TgCall { method, body });
                    }
                    TgAction::Forget(_) => {}
                }
            }
        }
    }
}

// ---- the worker: everything outgoing, in order ----

fn worker(shared: Arc<Shared>, rx: Receiver<Job>) {
    let agent = http_agent(SEND_TIMEOUT);
    for job in rx {
        let settings = shared.lock().settings.clone();
        match job {
            Job::Send { m, test } => {
                if !test && !settings.any_enabled() {
                    continue;
                }
                let on = test || settings.when != PhoneWhen::Never;
                let n = &settings.ntfy;
                if on && n.enabled && !n.topic.trim().is_empty() {
                    ntfy_send(&shared, &agent, n, &m);
                }
                let t = &settings.telegram;
                if on && t.enabled && !t.token.trim().is_empty() {
                    tg_send(&shared, &agent, t, &m);
                }
            }
            Job::Resolve { id, outcome } => {
                let (ntfy_title, tg_q) = {
                    let mut inner = shared.lock();
                    (inner.ntfy_open.remove(&id), inner.tg_open.remove(&id))
                };
                let n = &settings.ntfy;
                if let Some(title) = ntfy_title.filter(|_| n.enabled && !n.topic.trim().is_empty())
                {
                    let mark = if outcome.starts_with("answered") {
                        "✓"
                    } else {
                        "—"
                    };
                    let body = json!({
                        "topic": n.topic.trim(),
                        "title": title,
                        "message": format!("{mark} {outcome}"),
                        "tags": ["dog"],
                        "priority": 2,
                    });
                    if let Err(e) = ntfy_post(&agent, &ntfy_server(n), n.token.trim(), &body) {
                        shared.report(NTFY, e);
                    }
                }
                let token = settings.telegram.token.trim();
                if let Some(q) = tg_q.filter(|_| !token.is_empty()) {
                    let body = tg_edit_body(&q, &format!("— {}", html_escape(&outcome)));
                    if let Err(e) = tg_call(&agent, token, "editMessageText", &body) {
                        shared.report(TELEGRAM, e);
                    }
                }
            }
            Job::TgCall { method, body } => {
                let token = settings.telegram.token.trim();
                if token.is_empty() {
                    continue;
                }
                if let Err(e) = tg_call(&agent, token, method, &body) {
                    shared.report(TELEGRAM, e);
                }
            }
        }
    }
}

fn ntfy_send(shared: &Shared, agent: &ureq::Agent, n: &NtfySettings, m: &RemoteMessage) {
    let server = ntfy_server(n);
    let (topic, token) = (n.topic.trim(), n.token.trim());
    let body = ntfy_publish_body(&server, topic, token, m);
    match ntfy_post(agent, &server, token, &body) {
        Ok(()) => {
            if is_question(m) {
                shared.lock().ntfy_open.insert(m.id, full_title(m));
            }
        }
        Err(e) => shared.report(NTFY, e),
    }
}

fn tg_send(shared: &Shared, agent: &ureq::Agent, t: &TelegramSettings, m: &RemoteMessage) {
    let chat_id = t.chat_id.trim();
    if chat_id.is_empty() {
        let mut inner = shared.lock();
        if !inner.tg_unpaired_told {
            inner.tg_unpaired_told = true;
            drop(inner);
            shared.emit(RemoteEvent::Error {
                channel: TELEGRAM,
                error: "send /start to the bot first".into(),
            });
        }
        return;
    }
    let token = t.token.trim();
    let mut with_url = tg_url(m.url.as_deref()).is_some();
    let mut result = tg_call(
        agent,
        token,
        "sendMessage",
        &tg_send_body(m, chat_id, with_url),
    );
    // Telegram refuses some links (localhost, odd schemes): send without the button then.
    if with_url && result.as_ref().is_err_and(|e| e.contains("BUTTON_URL")) {
        with_url = false;
        result = tg_call(
            agent,
            token,
            "sendMessage",
            &tg_send_body(m, chat_id, false),
        );
    }
    match result {
        Ok(v) => {
            if let (true, Some(message_id)) = (is_question(m), v["message_id"].as_i64()) {
                shared.lock().tg_open.insert(
                    m.id,
                    TgQuestion {
                        message_id,
                        chat_id: chat_id.to_string(),
                        text: tg_text(m),
                        choices: m.choices.clone(),
                        allow_text: m.allow_text,
                        url: m.url.clone().filter(|_| with_url),
                    },
                );
            }
        }
        Err(e) => shared.report(TELEGRAM, e),
    }
}

/// A random topic name for ntfy ("dost-" and 16 letters and digits).
pub fn random_topic() -> String {
    let mut rng = fastrand::Rng::new();
    let s: String = (0..16)
        .map(|_| {
            let c = rng.u8(0..36);
            if c < 10 {
                (b'0' + c) as char
            } else {
                (b'a' + c - 10) as char
            }
        })
        .collect();
    format!("dost-{s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question() -> RemoteMessage {
        RemoteMessage {
            id: 7,
            source: Some("Claude".into()),
            title: "Run tests?".into(),
            body: "a <b> & c".into(),
            choices: vec!["Yes".into(), "No".into(), "Later".into(), "Never".into()],
            allow_text: true,
            url: Some("https://example.com/x".into()),
            mood: "neutral",
        }
    }

    fn open_question(allow_text: bool) -> TgQuestion {
        TgQuestion {
            message_id: 100,
            chat_id: "42".into(),
            text: "<b>Q</b>".into(),
            choices: vec!["Yes".into(), "No".into()],
            allow_text,
            url: None,
        }
    }

    #[test]
    fn ntfy_body_for_a_question() {
        let b = ntfy_publish_body("https://ntfy.sh", "dost-abc", "tk", &question());
        assert_eq!(b["topic"], "dost-abc");
        assert_eq!(b["title"], "Claude · Run tests?");
        assert_eq!(b["message"], "a <b> & c");
        assert_eq!(b["tags"], json!(["dog", "question"]));
        assert_eq!(b["priority"], 4);
        assert_eq!(b["click"], "https://example.com/x");
        let actions = b["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 3);
        assert_eq!(
            actions[1],
            json!({
                "action": "http",
                "label": "No",
                "url": "https://ntfy.sh/dost-abc-answers",
                "method": "POST",
                "body": "7\nNo",
                "clear": true,
                "headers": { "Authorization": "Bearer tk" },
            })
        );
    }

    #[test]
    fn ntfy_body_for_a_notification() {
        let m = RemoteMessage {
            title: "Done".into(),
            body: "Build passed".into(),
            mood: "good",
            ..Default::default()
        };
        let b = ntfy_publish_body("https://ntfy.sh", "t", "", &m);
        assert_eq!(b["title"], "Done");
        assert_eq!(b["tags"], json!(["dog", "white_check_mark"]));
        assert_eq!(b["priority"], 3);
        assert!(b.get("actions").is_none());
        assert!(b.get("click").is_none());
        let bad = RemoteMessage { mood: "bad", ..m };
        assert_eq!(
            ntfy_publish_body("s", "t", "", &bad)["tags"],
            json!(["dog", "x"])
        );
    }

    #[test]
    fn ntfy_settings_are_trimmed() {
        let mut s = PhoneSettings::default();
        s.ntfy.enabled = true;
        s.ntfy.server = " https://ntfy.example/ ".into();
        s.ntfy.topic = " t ".into();
        assert_eq!(
            ntfy_key(&s),
            Some(("https://ntfy.example".into(), "t".into(), String::new()))
        );
        s.when = PhoneWhen::Never;
        assert_eq!(ntfy_key(&s), None);
    }

    #[test]
    fn answers_parse() {
        assert_eq!(parse_answer("7\nYes"), Some((7, "Yes".into())));
        assert_eq!(
            parse_answer("12\nmy own\nwords "),
            Some((12, "my own\nwords".into()))
        );
        assert_eq!(parse_answer("x\nYes"), None);
        assert_eq!(parse_answer("7\n "), None);
        assert_eq!(parse_answer("7"), None);
        let line =
            r#"{"id":"abc","time":1,"event":"message","topic":"t-answers","message":"7\nNo"}"#;
        assert_eq!(
            ntfy_line(line),
            Some(("abc".into(), Some((7, "No".into()))))
        );
        assert_eq!(ntfy_line(r#"{"id":"k","event":"keepalive"}"#), None);
        assert_eq!(
            ntfy_line(r#"{"id":"m","event":"message","message":"hi"}"#),
            Some(("m".into(), None))
        );
        assert_eq!(ntfy_line("garbage"), None);
    }

    #[test]
    fn callback_data_parses() {
        assert_eq!(parse_callback("a:7:1"), Some((7, 1)));
        assert_eq!(parse_callback("a:7"), None);
        assert_eq!(parse_callback("b:7:1"), None);
        assert_eq!(parse_callback("a:x:1"), None);
    }

    #[test]
    fn html_is_escaped() {
        assert_eq!(html_escape("a<b>&c\"'"), "a&lt;b&gt;&amp;c\"'");
    }

    #[test]
    fn telegram_message_body() {
        let b = tg_send_body(&question(), "42", true);
        assert_eq!(b["chat_id"], "42");
        assert_eq!(b["parse_mode"], "HTML");
        assert_eq!(
            b["text"],
            format!("<b>Claude · Run tests?</b>\na &lt;b&gt; &amp; c\n\n<i>{REPLY_HINT}</i>")
        );
        let rows = b["reply_markup"]["inline_keyboard"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].as_array().unwrap().len(), 4);
        assert_eq!(
            rows[0][3],
            json!({ "text": "Never", "callback_data": "a:7:3" })
        );
        assert_eq!(rows[1][0]["url"], "https://example.com/x");
        let plain = RemoteMessage {
            title: "Hi".into(),
            ..Default::default()
        };
        let b = tg_send_body(&plain, "42", true);
        assert_eq!(b["text"], "<b>Hi</b>");
        assert!(b.get("reply_markup").is_none());
    }

    #[test]
    fn start_pairs() {
        let u = json!({"update_id": 1, "message": {"message_id": 5, "text": "/start",
            "chat": {"id": 42, "type": "private"}}});
        let a = tg_update(&u, "", &HashMap::new());
        assert_eq!(
            a[0],
            TgAction::Event(RemoteEvent::Paired {
                chat_id: "42".into()
            })
        );
        assert_eq!(
            a[1],
            TgAction::Call("sendMessage", json!({"chat_id": "42", "text": PAIRED}))
        );
        assert_eq!(tg_update(&u, "42", &HashMap::new()).len(), 2);
        assert!(tg_update(&u, "43", &HashMap::new()).is_empty());
        let group = json!({"update_id": 1, "message": {"text": "/start",
            "chat": {"id": -5, "type": "group"}}});
        assert!(tg_update(&group, "", &HashMap::new()).is_empty());
    }

    #[test]
    fn button_answers() {
        let open = HashMap::from([(7, open_question(false))]);
        let u = json!({"update_id": 2, "callback_query": {"id": "cb1", "data": "a:7:1",
            "message": {"message_id": 100, "chat": {"id": 42}}}});
        let a = tg_update(&u, "42", &open);
        assert_eq!(
            a,
            vec![
                TgAction::Call("answerCallbackQuery", json!({"callback_query_id": "cb1"})),
                TgAction::Event(RemoteEvent::Answer {
                    id: 7,
                    answer: "No".into()
                }),
                TgAction::Forget(7),
                TgAction::Call(
                    "editMessageText",
                    json!({"chat_id": "42", "message_id": 100, "text": "<b>Q</b>\n\n✓ No",
                        "parse_mode": "HTML", "disable_web_page_preview": true})
                ),
            ]
        );
        // Other chats are ignored; closed questions only get the callback answered.
        assert!(tg_update(&u, "43", &open).is_empty());
        let a = tg_update(&u, "42", &HashMap::new());
        assert_eq!(a.len(), 1);
        assert!(matches!(&a[0], TgAction::Call("answerCallbackQuery", _)));
    }

    #[test]
    fn text_answers() {
        let reply = json!({"update_id": 3, "message": {"text": "do it later",
            "chat": {"id": 42, "type": "private"}, "reply_to_message": {"message_id": 100}}});
        let plain = json!({"update_id": 4, "message": {"text": "ok then",
            "chat": {"id": 42, "type": "private"}}});
        let open = HashMap::from([(7, open_question(true))]);
        let a = tg_update(&reply, "42", &open);
        assert_eq!(
            a[0],
            TgAction::Event(RemoteEvent::Answer {
                id: 7,
                answer: "do it later".into()
            })
        );
        let a = tg_update(&plain, "42", &open);
        assert_eq!(
            a[0],
            TgAction::Event(RemoteEvent::Answer {
                id: 7,
                answer: "ok then".into()
            })
        );
        assert!(tg_update(&plain, "", &open).is_empty());
        assert!(tg_update(&plain, "43", &open).is_empty());
        // Buttons only: typed text is not an answer.
        let closed = HashMap::from([(7, open_question(false))]);
        assert!(tg_update(&reply, "42", &closed).is_empty());
        // Two open questions: a plain message is ambiguous, a reply is not.
        let mut other = open_question(true);
        other.message_id = 101;
        let two = HashMap::from([(7, open_question(true)), (8, other)]);
        assert!(tg_update(&plain, "42", &two).is_empty());
        assert_eq!(tg_update(&reply, "42", &two).len(), 3);
    }

    #[test]
    fn closing_keeps_the_link() {
        let mut q = open_question(false);
        q.url = Some("https://example.com".into());
        let b = tg_edit_body(&q, "— expired");
        assert_eq!(b["text"], "<b>Q</b>\n\n— expired");
        assert_eq!(
            b["reply_markup"]["inline_keyboard"][0][0]["url"],
            "https://example.com"
        );
    }
}
