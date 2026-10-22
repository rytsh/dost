//! MCP server (Streamable HTTP, JSON responses): AI assistants send notifications to the dog
//! and add status checks. Default address is `http://127.0.0.1:7777/mcp`.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Response, Server};

use crate::notify::{Mood, Notification, Notifier, Progress, ProgressState, Reaction};
use crate::todo::Todos;
use crate::watch::{Check, DEFAULT_INTERVAL, MIN_INTERVAL, NewWatch, Watches};

const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_BODY: u64 = 1 << 20;
/// How long `ask` waits for an answer by default, and at most (seconds).
const ASK_TIMEOUT: u64 = 300;
/// A workspace command that runs longer than this shows a progress bar in the bubble.
const LOCAL_BAR_AFTER: Duration = Duration::from_secs(5);

const ASK_TIMEOUT_MAX: u64 = 1800;

/// The dog's state, shared with the MCP server.
#[derive(Clone, Debug, Default)]
pub struct DogStatus {
    pub activity: String,
    /// How long it has been with the user ("2 years").
    pub age: String,
    pub visible: bool,
    pub showing: Option<String>,
    pub queued: usize,
    /// Seconds since the user last touched the mouse or keyboard (`None` if unknown).
    pub idle_secs: Option<u64>,
    pub quiet: bool,
    /// Recent notifications, newest last.
    pub history: VecDeque<HistoryEntry>,
}

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    /// Unix seconds when it arrived.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub title: String,
    pub message: String,
    /// "shown", "queued", "answered: <choice>", "expired", "skipped (quiet)", ...
    pub outcome: String,
}

/// How many notifications `history` keeps.
pub const HISTORY_LEN: usize = 50;

pub struct Mcp {
    notifier: Notifier,
    watches: Arc<Watches>,
    /// Todo lists; items with a reminder have a watch that barks it.
    todos: Arc<Todos>,
    status: Arc<Mutex<DogStatus>>,
    /// What agents are doing (`session`).
    sessions: Arc<crate::sessions::Sessions>,
    /// Scheduled jobs (`cron_*`).
    pub cron: Arc<crate::cron::Cron>,
    /// The user's special days (`day_*`).
    pub days: Arc<crate::days::Days>,
    /// The workspace tools at `/mcp/local`.
    pub local: crate::local::Local,
    /// The dog's name (settings), used in the tool descriptions.
    name: Mutex<String>,
    /// Which web pages may call the server from a browser (settings).
    cors: Mutex<Cors>,
}

/// CORS, as in local-mcp: lets browser-based MCP clients connect. Pages on localhost are
/// always allowed; others only when their origin is listed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Cors {
    /// Origins allowed besides localhost: `"*"` (every page), `"https://app.example.com"`,
    /// or with a wildcard, `"https://*.example.com"`.
    pub allow_origins: Vec<String>,
    /// Answers Chrome's Private Network Access preflight, which a page on a public address
    /// must pass to reach a server on this machine.
    pub allow_private_network: bool,
}

impl Cors {
    /// Every web page may connect.
    pub fn open() -> Self {
        Cors {
            allow_origins: vec!["*".into()],
            allow_private_network: true,
        }
    }

    pub fn allows(&self, origin: &str) -> bool {
        is_local_origin(origin)
            || self
                .allow_origins
                .iter()
                .any(|p| origin_matches(p.trim(), origin))
    }
}

/// The streamable HTTP transport posts requests, opens the event stream with GET and ends the
/// session with DELETE.
const CORS_METHODS: &str = "GET, POST, DELETE, OPTIONS";
const CORS_HEADERS: &str = "content-type, accept, authorization, cache-control, last-event-id, mcp-session-id, mcp-protocol-version";

/// A running server; stops and releases the port when dropped.
pub struct Running {
    server: Arc<Server>,
    pub addr: String,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

/// Starts the server on a separate thread. Returns an error if the port is taken.
pub fn serve(addr: &str, mcp: Arc<Mcp>) -> Result<Running, String> {
    let server = Arc::new(Server::http(addr).map_err(|e| format!("{addr}: {e}"))?);
    let srv = Arc::clone(&server);
    thread::spawn(move || {
        for req in srv.incoming_requests() {
            let mcp = Arc::clone(&mcp);
            thread::spawn(move || mcp.handle(req));
        }
    });
    Ok(Running {
        server,
        addr: addr.to_string(),
    })
}

impl Mcp {
    pub fn new(
        notifier: Notifier,
        watches: Arc<Watches>,
        todos: Arc<Todos>,
        status: Arc<Mutex<DogStatus>>,
        sessions: Arc<crate::sessions::Sessions>,
        cron: Arc<crate::cron::Cron>,
        days: Arc<crate::days::Days>,
    ) -> Self {
        Mcp {
            notifier,
            watches,
            todos,
            status,
            sessions,
            cron,
            days,
            local: crate::local::Local::new(Default::default()),
            name: Mutex::new(crate::settings::DEFAULT_NAME.into()),
            cors: Mutex::new(Cors::default()),
        }
    }

    /// The CORS settings changed.
    pub fn set_cors(&self, cors: &Cors) {
        if let Ok(mut c) = self.cors.lock() {
            *c = cors.clone();
        }
    }

    fn cors(&self) -> Cors {
        self.cors.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// The dog's name changed (settings).
    pub fn todo_lists(&self) -> Vec<crate::todo::List> {
        self.todos.lists()
    }

    /// A change from the settings panel. Errors (an empty text) are ignored.
    pub fn todo_edit(&self, edit: &crate::todo::Edit) {
        use crate::todo::Edit;
        let _ = match edit {
            Edit::NewList(name) => self.todos.create_list(name, &[]).map(|_| ()),
            Edit::Add { list, text } => self.todos.add(list, text, "", None).map(|_| ()),
            Edit::Toggle(id) => {
                let done = self
                    .todos
                    .lists()
                    .iter()
                    .flat_map(|l| &l.items)
                    .find(|i| i.id == *id)
                    .map(|i| !i.done);
                self.todos
                    .update(*id, done, None, None, None)
                    .map(|c| self.sync_reminder(&c, None))
            }
            Edit::Remove(id) => self.todos.remove_item(*id).map(|i| {
                if let Some(w) = i.watch {
                    self.watches.remove(w);
                }
            }),
            Edit::RemoveList(id) => self.todos.remove_list(&id.to_string()).map(|l| {
                for w in l.items.iter().filter_map(|i| i.watch) {
                    self.watches.remove(w);
                }
            }),
            Edit::ClearDone(id) => self.todos.clear_done(&id.to_string()).map(|_| ()),
        };
    }

    pub fn set_name(&self, name: &str) {
        if let Ok(mut n) = self.name.lock() {
            *n = name.to_string();
        }
    }

    fn name(&self) -> String {
        self.name
            .lock()
            .map(|n| n.clone())
            .unwrap_or_else(|_| crate::settings::DEFAULT_NAME.into())
    }

    fn handle(&self, mut req: tiny_http::Request) {
        let path = req.url().split('?').next().unwrap_or("").to_string();
        let path = path.trim_end_matches('/');
        // The workspace tools only exist while they are turned on.
        let local = path == "/mcp/local";
        if !(path == "/mcp" || path.is_empty() || local && self.local.settings().enabled) {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
            return;
        }
        // Web pages in a browser may only call when their origin is allowed (this also blocks
        // DNS rebinding); command-line clients send no Origin.
        let origin = header(&req, "Origin");
        let cors = self.cors();
        if let Some(o) = origin.as_deref()
            && !cors.allows(o)
        {
            let _ = req.respond(Response::from_string("forbidden origin").with_status_code(403));
            return;
        }
        let allow = |r: Response<std::io::Cursor<Vec<u8>>>| match origin.as_deref() {
            Some(o) => r
                .with_header(hdr("Access-Control-Allow-Origin", o))
                .with_header(hdr("Vary", "Origin"))
                .with_header(hdr("Access-Control-Expose-Headers", "Mcp-Session-Id")),
            None => r,
        };
        match req.method() {
            Method::Post => {}
            Method::Options => {
                // CORS preflight.
                let mut r = allow(Response::from_data(Vec::new()).with_status_code(204))
                    .with_header(hdr("Access-Control-Allow-Methods", CORS_METHODS))
                    .with_header(hdr("Access-Control-Allow-Headers", CORS_HEADERS))
                    .with_header(hdr("Access-Control-Max-Age", "600"));
                if cors.allow_private_network
                    && header(&req, "Access-Control-Request-Private-Network").as_deref()
                        == Some("true")
                {
                    r = r.with_header(hdr("Access-Control-Allow-Private-Network", "true"));
                }
                let _ = req.respond(r);
                return;
            }
            Method::Get | Method::Delete => {
                // No server-to-client streaming (SSE) and no sessions.
                let _ = req.respond(allow(
                    Response::from_data(Vec::new())
                        .with_status_code(405)
                        .with_header(hdr("Allow", "POST")),
                ));
                return;
            }
            _ => {
                let _ = req.respond(Response::empty(405));
                return;
            }
        }
        // Clients that take an event stream get a long command's output while it runs.
        let sse = header(&req, "Accept")
            .is_some_and(|a| a.to_ascii_lowercase().contains("text/event-stream"));
        let mut body = String::new();
        if req
            .as_reader()
            .take(MAX_BODY)
            .read_to_string(&mut body)
            .is_err()
        {
            let _ = req.respond(allow(json_response(
                400,
                &rpc_error(Value::Null, -32700, "parse error"),
            )));
            return;
        }
        if local
            && sse
            && let Ok(msg) = serde_json::from_str::<Value>(&body)
            && msg.get("method").and_then(Value::as_str) == Some("tools/call")
            && msg.get("id").is_some()
        {
            return self.stream_local(req, msg, origin.as_deref());
        }
        let reply = match serde_json::from_str::<Value>(&body) {
            Ok(Value::Array(batch)) => {
                let out: Vec<Value> = batch
                    .into_iter()
                    .filter_map(|m| self.route(local, m))
                    .collect();
                (!out.is_empty()).then_some(Value::Array(out))
            }
            Ok(msg) => self.route(local, msg),
            Err(_) => Some(rpc_error(Value::Null, -32700, "parse error")),
        };
        let _ = match reply {
            Some(v) => req.respond(allow(json_response(200, &v))),
            None => req.respond(allow(Response::from_data(Vec::new()).with_status_code(202))),
        };
    }

    /// A workspace tool call answered as an event stream: progress notifications with the
    /// command's output while it runs, then the result.
    fn stream_local(&self, req: tiny_http::Request, msg: Value, origin: Option<&str>) {
        use std::io::Write;
        let mut head = String::from(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n",
        );
        if let Some(o) = origin {
            head += &format!(
                "Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\nAccess-Control-Expose-Headers: Mcp-Session-Id\r\n"
            );
        }
        head += "\r\n";
        let mut w = req.into_writer();
        if w.write_all(head.as_bytes())
            .and_then(|_| w.flush())
            .is_err()
        {
            return;
        }
        let token = msg
            .pointer("/params/_meta/progressToken")
            .cloned()
            .unwrap_or_else(|| msg.get("id").cloned().unwrap_or(Value::Null));
        // Each event is one chunk; the empty chunk ends the response (the connection stays).
        let chunk = |w: &mut Box<dyn Write + Send>, v: &Value| {
            let ev = format!("event: message\ndata: {v}\n\n");
            let _ = write!(w, "{:x}\r\n{ev}\r\n", ev.len()).and_then(|_| w.flush());
        };
        let mut n = 0u64;
        let mut live = |text: String| {
            n += 1;
            let ev = json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": { "progressToken": token, "progress": n, "message": text },
            });
            chunk(&mut w, &ev);
        };
        let reply = self.dispatch_local_live(msg, Some(&mut live));
        if let Some(v) = reply {
            chunk(&mut w, &v);
        }
        let _ = w.write_all(b"0\r\n\r\n").and_then(|_| w.flush());
    }

    fn route(&self, local: bool, msg: Value) -> Option<Value> {
        if local {
            self.dispatch_local(msg)
        } else {
            self.dispatch(msg)
        }
    }

    /// A JSON-RPC message for `/mcp/local` (the workspace tools).
    pub fn dispatch_local(&self, msg: Value) -> Option<Value> {
        self.dispatch_local_live(msg, None)
    }

    fn dispatch_local_live(
        &self,
        msg: Value,
        live: Option<&mut dyn FnMut(String)>,
    ) -> Option<Value> {
        let id = msg.get("id").cloned()?;
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == asked)
                    .unwrap_or(&PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "dost-local", "title": "Dost workspace tools", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": self.local.instructions(),
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.local.tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Ok(match self.call_local(name, &args, live) {
                    Ok(content) => json!({ "content": content, "isError": false }),
                    Err(e) => {
                        json!({ "content": [{ "type": "text", "text": e }], "isError": true })
                    }
                })
            }
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, m)) => rpc_error(id, code, &m),
        })
    }

    /// Runs a workspace tool: asks the user first when the settings say so, and keeps what
    /// changed things in History.
    fn call_local(
        &self,
        name: &str,
        args: &Value,
        live: Option<&mut dyn FnMut(String)>,
    ) -> Result<Vec<Value>, String> {
        let source = str_arg(args, "source").unwrap_or_else(|| "Agent".into());
        let approval = self.local.approval(name, args);
        if let Some(a) = &approval {
            let tr = crate::i18n::t;
            let choices = vec![
                tr("Allow").to_string(),
                tr("Always").to_string(),
                tr("Deny").to_string(),
            ];
            let (n, rx) = Notification::new(tr(&a.title), a.detail.clone())
                .source(Some(format!("{source} · {}", tr("Workspace"))))
                .ask(choices.clone(), Duration::from_secs(ASK_TIMEOUT));
            if !self.notifier.send(n) {
                return Err("the dog is not running".into());
            }
            let answer = rx.recv_timeout(Duration::from_secs(ASK_TIMEOUT + 2)).ok();
            let pick = answer
                .as_deref()
                .and_then(|x| choices.iter().position(|c| c == x));
            match pick {
                Some(0) => {}
                Some(1) => {
                    self.local.allow_always(&a.rule);
                    self.notifier.allow_always(a.rule.clone());
                }
                Some(_) => {
                    self.record_local(name, &source, a, "denied");
                    return Err(
                        "The user denied this. Do not retry it; ask them what to do instead."
                            .into(),
                    );
                }
                None => {
                    self.record_local(name, &source, a, "expired (no answer)");
                    return Err(format!(
                        "The user did not answer within {ASK_TIMEOUT} s; nothing was done."
                    ));
                }
            }
        }
        let started = std::time::Instant::now();
        // A command that runs for a while shows in the dog's bubble as a progress bar with its
        // latest output line (and goes on to the client's live stream, if any).
        let bar_id = format!("local-{}", fastrand::u64(..));
        let command = str_arg(args, "command").unwrap_or_default();
        let mut shown = false;
        let mut outer = live;
        let mut tee = |text: String| {
            if started.elapsed() >= LOCAL_BAR_AFTER {
                let last = text.lines().rev().find(|l| !l.trim().is_empty());
                if let Some(line) = last.or(if shown { None } else { Some("") }) {
                    shown = true;
                    self.local_bar(&bar_id, &source, &command, line, ProgressState::Running);
                }
            }
            if !text.is_empty()
                && let Some(f) = outer.as_deref_mut()
            {
                f(text);
            }
        };
        let out = self.local.call_live(name, args, Some(&mut tee));
        if shown {
            let state = if out.is_ok() {
                ProgressState::Done
            } else {
                ProgressState::Failed
            };
            let took = crate::watch::human_secs(started.elapsed().as_secs());
            self.local_bar(&bar_id, &source, &command, &took, state);
        }
        if matches!(name, "shell" | "edit" | "write" | "patch") {
            let what = approval.unwrap_or_else(|| self.local.describe_call(name, args));
            let secs = started.elapsed().as_secs();
            let took = if secs >= 2 {
                format!(" · {}", crate::watch::human_secs(secs))
            } else {
                String::new()
            };
            let outcome = match &out {
                Ok(_) => format!("done{took}"),
                Err(e) if e.contains("Exit code:") => {
                    let code = e
                        .split("Exit code:")
                        .nth(1)
                        .and_then(|c| c.split_whitespace().next())
                        .unwrap_or("?");
                    format!("failed (exit {code}){took}")
                }
                Err(_) => format!("failed{took}"),
            };
            self.record_local(name, &source, &what, &outcome);
        }
        out
    }

    fn local_bar(&self, id: &str, source: &str, command: &str, line: &str, state: ProgressState) {
        let title = format!("$ {}", command.lines().next().unwrap_or_default());
        let mut n = Notification::new(
            title.chars().take(80).collect::<String>(),
            line.chars().take(160).collect::<String>(),
        )
        .source(Some(format!("{source} · shell")))
        .sticky();
        n.mood = match state {
            ProgressState::Running => Mood::Neutral,
            ProgressState::Done => Mood::Good,
            ProgressState::Failed => Mood::Bad,
        };
        n.progress = Some(Progress {
            id: id.to_string(),
            value: None,
            state,
        });
        self.notifier.send(n);
    }

    fn record_local(&self, tool: &str, source: &str, a: &crate::local::Approval, outcome: &str) {
        let title = match tool {
            "shell" => format!("$ {}", a.detail.lines().next().unwrap_or_default()),
            _ => a.detail.lines().next().unwrap_or_default().to_string(),
        };
        self.notifier.record(HistoryEntry {
            id: 0,
            at: 0,
            source: Some(format!("{source} · {tool}")),
            title: title.chars().take(160).collect(),
            message: a.detail.chars().take(500).collect(),
            outcome: outcome.into(),
        });
    }

    /// Handles a single JSON-RPC message. Notifications (without an id) get no reply.
    pub fn dispatch(&self, msg: Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let id = id?;
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools(&self.name()) })),
            "tools/call" => self.call_tool(&params),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, m)) => rpc_error(id, code, &m),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("");
        let version = PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == asked)
            .unwrap_or(&PROTOCOL_VERSIONS[0]);
        let name = self.name();
        let dog = dog_label(&name);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "dost", "title": format!("{dog} (Dost desktop dog)"), "version": env!("CARGO_PKG_VERSION") },
            "instructions": format!("{dog}: a dog on the user's screen. Use `notify` to tell the user something, \
                `ask` to ask a question with answer buttons and wait for the click, and `progress` \
                to show a long task's progress bar. Use `add_watch` / `remind` to get the user \
                notified later (a site comes up, a port opens, a file appears, a process ends, at a time). \
                Always notify when something needs the user, even if they seem active: they often \
                listen for the bark and look later. Pass `source` with your name so the user can \
                tell agents apart. When the user talks about {name} (\"tell {name}\", \"ask {name}\", \
                \"{name}, remind me...\"), they mean these tools."),
        })
    }

    fn call_tool(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let out = match name {
            "notify" => self.tool_notify(&args),
            "ask" => self.tool_ask(&args),
            "dismiss" => self.tool_dismiss(&args),
            "progress" => self.tool_progress(&args),
            "react" => self.tool_react(&args),
            "copy" => self.tool_copy(&args),
            "remind" => self.tool_remind(&args),
            "add_watch" => self.tool_add_watch(&args),
            "list_watches" => Ok(self.tool_list_watches()),
            "remove_watch" => self.tool_remove_watch(&args),
            "check_now" => self.tool_check_now(&args),
            "cron_add" => self.tool_cron_add(&args),
            "cron_list" => Ok(self.tool_cron_list()),
            "cron_remove" => self.tool_cron_id(&args, "remove"),
            "cron_run" => self.tool_cron_id(&args, "run"),
            "cron_enable" => self.tool_cron_id(&args, "enable"),
            "day_add" => self.tool_day_add(&args),
            "day_list" => Ok(self.tool_day_list()),
            "day_remove" => self.tool_day_remove(&args),
            "status" => Ok(self.tool_status()),
            "session" => self.tool_session(&args),
            "sessions" => Ok(self.tool_sessions()),
            "history" => Ok(self.tool_history(&args)),
            "todo_lists" => Ok(self.tool_todo_lists(&args)),
            "todo_create_list" => self.tool_todo_create_list(&args),
            "todo_add" => self.tool_todo_add(&args),
            "todo_update" => self.tool_todo_update(&args),
            "todo_remove" => self.tool_todo_remove(&args),
            _ => return Err((-32602, format!("unknown tool: {name}"))),
        };
        Ok(match out {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
            Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
        })
    }

    fn tool_notify(&self, a: &Value) -> Result<String, String> {
        let title = str_arg(a, "title").unwrap_or_default();
        let message = str_arg(a, "message").unwrap_or_default();
        if title.is_empty() && message.is_empty() {
            return Err("`title` or `message` is required".into());
        }
        let mut n = Notification::new(title, message)
            .source(str_arg(a, "source"))
            .mood(parse_mood(a)?);
        if a.get("sticky").and_then(Value::as_bool).unwrap_or(false) {
            n = n.sticky();
        } else if let Some(s) = a.get("timeout_secs").and_then(Value::as_u64) {
            n = n.timeout(Duration::from_secs(s.clamp(2, 3600)));
        }
        if let Some(url) = str_arg(a, "url") {
            if !is_web_url(&url) {
                return Err("`url` must start with http:// or https://".into());
            }
            n.url = Some(url);
            // Leave time to click it.
            if n.timeout == crate::notify::Timeout::Default {
                n = n.timeout(Duration::from_secs(30));
            }
        }
        n.copy = str_arg(a, "copy");
        n.session = str_arg(a, "session");
        let copied = n.copy.is_some();
        let important = n.is_important();
        if !self.notifier.send(n) {
            return Err("the dog is not running".into());
        }
        let quiet = self.status.lock().map(|s| s.quiet).unwrap_or(false);
        Ok(match (quiet, important, copied) {
            (true, false, _) => {
                "It is quiet now (the dog is asleep, Quiet is on, or the user is in a full-screen app): this notification is skipped (it stays in history). Use sticky for something that must reach the user.".into()
            }
            (true, true, _) => {
                "It is quiet now (asleep, Quiet, or full screen): shown silently, without a bark.".into()
            }
            (false, _, true) => {
                "Notification sent; the text will be on the clipboard when the bubble appears.".into()
            }
            (false, _, false) => "Notification sent; the dog is barking.".into(),
        })
    }

    fn tool_ask(&self, a: &Value) -> Result<String, String> {
        let question = str_arg(a, "question").ok_or("`question` is required")?;
        let mut choices: Vec<String> = a
            .get("choices")
            .and_then(Value::as_array)
            .map(|c| {
                c.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.chars().take(24).collect())
                    .collect()
            })
            .unwrap_or_default();
        choices.dedup();
        if choices.is_empty() {
            choices = vec!["Yes".into(), "No".into()];
        }
        if choices.len() > 4 {
            return Err("at most 4 `choices`".into());
        }
        let wait = a
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(ASK_TIMEOUT)
            .clamp(10, ASK_TIMEOUT_MAX);
        let (mut n, rx) = Notification::new(str_arg(a, "title").unwrap_or_default(), question)
            .source(str_arg(a, "source"))
            .ask(choices, Duration::from_secs(wait));
        n.key = str_arg(a, "key");
        n.session = str_arg(a, "session");
        n.allow_text = a
            .get("allow_text")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !self.notifier.send(n) {
            return Err("the dog is not running".into());
        }
        // The app drops the question (and the sender) when it expires or is dismissed.
        match rx.recv_timeout(Duration::from_secs(wait + 2)) {
            Ok(answer) => Ok(format!("The user answered: {answer}")),
            Err(_) => Err(format!(
                "No answer within {wait} s (or the question was withdrawn); the user may be away. Ask again later or carry on with a safe default."
            )),
        }
    }

    fn tool_dismiss(&self, a: &Value) -> Result<String, String> {
        let key = str_arg(a, "key").ok_or("`key` is required")?;
        if !self.notifier.dismiss(key.clone()) {
            return Err("the dog is not running".into());
        }
        Ok(format!("Anything with key `{key}` is withdrawn."))
    }

    fn tool_progress(&self, a: &Value) -> Result<String, String> {
        let id = str_arg(a, "id").ok_or("`id` is required")?;
        let state = match str_arg(a, "state").as_deref() {
            None | Some("running") => ProgressState::Running,
            Some("done") => ProgressState::Done,
            Some("failed") => ProgressState::Failed,
            Some(s) => return Err(format!("unknown state `{s}` (running, done, failed)")),
        };
        let value = match (
            a.get("value").and_then(Value::as_f64),
            a.get("total").and_then(Value::as_f64),
        ) {
            (Some(v), Some(t)) if t > 0.0 => Some((v / t) as f32),
            (Some(v), None) => Some(if v > 1.0 { v as f32 / 100.0 } else { v as f32 }),
            _ => None,
        }
        .map(|v| v.clamp(0.0, 1.0));
        let title = str_arg(a, "title").unwrap_or_else(|| id.clone());
        let mut message = str_arg(a, "message").unwrap_or_default();
        if message.is_empty()
            && let (Some(v), Some(t)) = (
                a.get("value").and_then(Value::as_u64),
                a.get("total").and_then(Value::as_u64),
            )
        {
            message = format!("{v} / {t}");
        }
        let mut n = Notification::new(title, message)
            .source(str_arg(a, "source"))
            .sticky();
        n.mood = match state {
            ProgressState::Running => Mood::Neutral,
            ProgressState::Done => Mood::Good,
            ProgressState::Failed => Mood::Bad,
        };
        n.progress = Some(Progress {
            id: id.clone(),
            value,
            state,
        });
        n.session = str_arg(a, "session");
        if !self.notifier.send(n) {
            return Err("the dog is not running".into());
        }
        Ok(match state {
            ProgressState::Running => format!(
                "Progress `{id}` shown. Call again with the same id to update it, and with state done or failed at the end."
            ),
            _ => format!("Progress `{id}` finished."),
        })
    }

    fn tool_react(&self, a: &Value) -> Result<String, String> {
        let name = str_arg(a, "reaction").ok_or("`reaction` is required")?;
        let r = Reaction::parse(&name).ok_or_else(|| {
            format!(
                "unknown reaction `{name}`; use one of {}",
                Reaction::NAMES.join(", ")
            )
        })?;
        if !self.notifier.react(r) {
            return Err("the dog is not running".into());
        }
        Ok(format!("{} reacts: {name}.", self.name()))
    }

    fn tool_copy(&self, a: &Value) -> Result<String, String> {
        let text = a
            .get("text")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("`text` is required")?;
        let preview: String = text.chars().take(80).collect();
        let more = if text.chars().count() > 80 { "…" } else { "" };
        let mut n = Notification::new(
            str_arg(a, "title").unwrap_or_else(|| "Copied to clipboard".into()),
            str_arg(a, "message").unwrap_or_else(|| format!("{preview}{more}")),
        )
        .source(str_arg(a, "source"));
        n.copy = Some(text.to_string());
        if !self.notifier.send(n) {
            return Err("the dog is not running".into());
        }
        Ok("The text goes on the clipboard when the dog shows the bubble.".into())
    }

    fn tool_remind(&self, a: &Value) -> Result<String, String> {
        let message = str_arg(a, "message").ok_or("`message` is required")?;
        let now = crate::watch::now_unix();
        let at = match (
            a.get("after_minutes").and_then(Value::as_f64),
            str_arg(a, "at"),
        ) {
            (Some(m), None) if m > 0.0 && m <= 7.0 * 24.0 * 60.0 => now + (m * 60.0).round() as u64,
            (None, Some(t)) => crate::watch::next_local_time(&t)
                .ok_or("`at` must be a local time like \"15:30\"")?,
            (Some(_), None) => return Err("`after_minutes` must be between 0 and 10080".into()),
            _ => return Err("exactly one of `after_minutes` or `at` is required".into()),
        };
        let title = str_arg(a, "title").unwrap_or_else(|| "Reminder".into());
        let w = self.watches.add(NewWatch {
            name: format!("Reminder: {message}"),
            check: Check::Time { at },
            negate: false,
            interval_secs: 1,
            title,
            message,
            once: true,
            sticky: a.get("sticky").and_then(Value::as_bool).unwrap_or(true),
            source: str_arg(a, "source"),
        });
        Ok(format!(
            "Reminder #{} set; the dog will bark in {}. Remove it with remove_watch.",
            w.id,
            crate::watch::human_secs(at.saturating_sub(now))
        ))
    }

    fn tool_add_watch(&self, a: &Value) -> Result<String, String> {
        let check = parse_check(a)?;
        let name = str_arg(a, "name").unwrap_or_else(|| check_label(&check));
        let n = NewWatch {
            title: str_arg(a, "title").unwrap_or_else(|| name.clone()),
            message: str_arg(a, "message").unwrap_or_default(),
            name,
            check,
            negate: a.get("negate").and_then(Value::as_bool).unwrap_or(false),
            interval_secs: a
                .get("interval_secs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_INTERVAL),
            once: a.get("once").and_then(Value::as_bool).unwrap_or(true),
            sticky: a.get("sticky").and_then(Value::as_bool).unwrap_or(false),
            source: str_arg(a, "source"),
        };
        let w = self.watches.add(n);
        Ok(format!(
            "Watch #{} added: \"{}\", every {} s. {}",
            w.id,
            w.name,
            w.interval_secs,
            if w.once {
                "It will bark once when the condition is met, then be removed."
            } else {
                "It will bark every time the condition is met."
            }
        ))
    }

    fn tool_cron_add(&self, a: &Value) -> Result<String, String> {
        use crate::cron::{Action as Job, NewJob, NotifyWhen};
        let schedule = str_arg(a, "schedule").ok_or("`schedule` is required")?;
        let kind = str_arg(a, "kind").unwrap_or_else(|| "notify".into());
        let action = match kind.as_str() {
            "notify" => Job::Notify {
                title: str_arg(a, "title").unwrap_or_else(|| "Reminder".into()),
                message: str_arg(a, "message").unwrap_or_default(),
                sticky: a.get("sticky").and_then(Value::as_bool).unwrap_or(false),
            },
            "http" => Job::Http {
                method: str_arg(a, "method")
                    .unwrap_or_else(|| "GET".into())
                    .to_ascii_uppercase(),
                url: str_arg(a, "url").ok_or("`url` is required for http")?,
                headers: a
                    .get("headers")
                    .and_then(Value::as_object)
                    .map(|h| {
                        h.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default(),
                body: str_arg(a, "body"),
                expect_status: a
                    .get("expect_status")
                    .and_then(Value::as_u64)
                    .map(|s| s as u16),
                contains: str_arg(a, "contains"),
            },
            "command" => Job::Command {
                command: str_arg(a, "command").ok_or("`command` is required for command")?,
                workdir: str_arg(a, "workdir"),
            },
            k => return Err(format!("unknown kind `{k}`; use notify, http or command")),
        };
        let notify = match str_arg(a, "notify").as_deref() {
            None | Some("failure") => NotifyWhen::Failure,
            Some("always") => NotifyWhen::Always,
            Some("never") => NotifyWhen::Never,
            Some(n) => {
                return Err(format!(
                    "unknown notify `{n}`; use always, failure or never"
                ));
            }
        };
        let job = self.cron.add(NewJob {
            name: str_arg(a, "name").unwrap_or_default(),
            schedule,
            action,
            notify,
            source: str_arg(a, "source"),
        })?;
        self.notifier.panel_changed();
        let next = self
            .cron
            .next_run(&job)
            .map(|t| {
                format!(
                    " Next run in {}.",
                    crate::watch::human_secs(t.saturating_sub(crate::watch::now_unix()))
                )
            })
            .unwrap_or_default();
        Ok(format!(
            "Job #{} \"{}\" scheduled ({}).{next} The user sees it in Schedule; cron_remove deletes it.",
            job.id, job.name, job.schedule
        ))
    }

    fn tool_day_add(&self, a: &Value) -> Result<String, String> {
        use crate::days::{Kind, Look, NewDay};
        let date = str_arg(a, "date").ok_or("`date` is required (MM-DD or YYYY-MM-DD)")?;
        let parts: Vec<&str> = date.trim().split('-').collect();
        let num = |s: &str| s.parse::<i64>().ok();
        let (year, month, day) = match parts[..] {
            [m, d] => (None, num(m), num(d)),
            [y, m, d] => (num(y), num(m), num(d)),
            _ => (None, None, None),
        };
        let (Some(month), Some(day)) = (month, day) else {
            return Err(format!("`date` {date:?} must be MM-DD or YYYY-MM-DD"));
        };
        let kind = match str_arg(a, "kind").as_deref() {
            None | Some("birthday") => Kind::Birthday,
            Some("anniversary") => Kind::Anniversary,
            Some("holiday") => Kind::Holiday,
            Some("other") => Kind::Other,
            Some(k) => return Err(format!("unknown kind `{k}`")),
        };
        let look = match str_arg(a, "look").as_deref() {
            None | Some("auto") => Look::Auto,
            Some("none") => Look::None,
            Some("party_hat") => Look::PartyHat,
            Some("heart") => Look::Heart,
            Some("star") => Look::Star,
            Some("ribbon") => Look::Ribbon,
            Some("flag") => Look::Flag,
            Some(l) => return Err(format!("unknown look `{l}`")),
        };
        let d = self.days.add(NewDay {
            name: str_arg(a, "name").unwrap_or_default(),
            kind,
            month: month as u32,
            day: day as u32,
            year: year.map(|y| y as i32),
            once: a.get("once").and_then(Value::as_bool).unwrap_or(false),
            message: str_arg(a, "message").unwrap_or_default(),
            at: str_arg(a, "at").unwrap_or_default(),
            remind_days_before: a
                .get("remind_days_before")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            look,
            phone: a.get("phone").and_then(Value::as_bool).unwrap_or(false),
            source: str_arg(a, "source"),
        })?;
        self.notifier.days_changed();
        let next = self
            .days
            .next_date(&d, crate::look::today())
            .map(|n| format!(" Next: {n}."))
            .unwrap_or_default();
        Ok(format!(
            "Special day #{} \"{}\" added ({:02}-{:02}{}), the dog says so at {}.{next}",
            d.id,
            d.name,
            d.month,
            d.day,
            if d.once { ", once" } else { ", every year" },
            d.at
        ))
    }

    fn tool_day_list(&self) -> String {
        let days = self.days.list();
        if days.is_empty() {
            return "No special days of the user's own.".into();
        }
        let today = crate::look::today();
        let list: Vec<Value> = days
            .iter()
            .map(|d| {
                let mut v = serde_json::to_value(d).unwrap_or_default();
                if let Some(n) = self.days.next_date(d, today) {
                    v["next"] = json!(n.to_string());
                }
                v
            })
            .collect();
        serde_json::to_string_pretty(&list).unwrap_or_default()
    }

    fn tool_day_remove(&self, a: &Value) -> Result<String, String> {
        let id = a
            .get("id")
            .and_then(Value::as_u64)
            .ok_or("`id` is required")?;
        if !self.days.remove(id) {
            return Err(format!("no special day #{id}"));
        }
        self.notifier.days_changed();
        Ok(format!("Special day #{id} removed."))
    }

    fn tool_cron_list(&self) -> String {
        let jobs = self.cron.list();
        if jobs.is_empty() {
            return "No scheduled jobs.".into();
        }
        let list: Vec<Value> = jobs
            .iter()
            .map(|j| {
                let mut v = serde_json::to_value(j).unwrap_or_default();
                if let Some(t) = self.cron.next_run(j) {
                    v["next_run"] = json!(t);
                }
                v
            })
            .collect();
        serde_json::to_string_pretty(&list).unwrap_or_default()
    }

    fn tool_cron_id(&self, a: &Value, what: &str) -> Result<String, String> {
        let id = a
            .get("id")
            .and_then(Value::as_u64)
            .ok_or("`id` is required")?;
        let ok = match what {
            "remove" => self.cron.remove(id),
            "run" => self.cron.run_now(id),
            _ => self.cron.set_enabled(
                id,
                a.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            ),
        };
        if !ok {
            return Err(format!("no job #{id}"));
        }
        self.notifier.panel_changed();
        Ok(match what {
            "remove" => format!("Job #{id} removed."),
            "run" => format!("Job #{id} is running now; cron_list shows how it went."),
            _ => format!("Job #{id} updated."),
        })
    }

    fn tool_list_watches(&self) -> String {
        let list = self.watches.list();
        if list.is_empty() {
            return "No watches.".into();
        }
        serde_json::to_string_pretty(&list).unwrap_or_default()
    }

    fn tool_remove_watch(&self, a: &Value) -> Result<String, String> {
        let id = a
            .get("id")
            .and_then(Value::as_u64)
            .ok_or("`id` is required")?;
        if self.watches.remove(id) {
            Ok(format!("Watch #{id} removed."))
        } else {
            Err(format!("no watch #{id}"))
        }
    }

    /// Parses `remind_after_minutes` / `remind_at` of a todo tool: `Some(None)` clears it
    /// (`remind_at: ""`), `None` leaves it.
    fn todo_reminder(a: &Value) -> Result<Option<Option<u64>>, String> {
        let now = crate::watch::now_unix();
        match (
            a.get("remind_after_minutes").and_then(Value::as_f64),
            str_arg(a, "remind_at"),
        ) {
            (None, None) if a.get("remind_at").is_some_and(|v| v == "") => Ok(Some(None)),
            (None, None) => Ok(None),
            (Some(m), None) if m > 0.0 && m <= 30.0 * 24.0 * 60.0 => {
                Ok(Some(Some(now + (m * 60.0).round() as u64)))
            }
            (Some(_), None) => Err("`remind_after_minutes` must be between 0 and 43200".into()),
            (None, Some(t)) => crate::watch::next_local_time(&t)
                .or_else(|| parse_local_datetime(&t))
                .map(|at| Some(Some(at)))
                .ok_or_else(|| "`remind_at` must be \"HH:MM\" or \"YYYY-MM-DD HH:MM\"".into()),
            _ => Err("give `remind_after_minutes` or `remind_at`, not both".into()),
        }
    }

    /// Sets (or removes) the watch that barks an item's reminder, to match the item.
    fn sync_reminder(&self, c: &crate::todo::Changed, source: Option<String>) {
        if let Some(old) = c.item.watch {
            self.watches.remove(old);
        }
        let watch = match c.item.remind_at {
            Some(at) if !c.item.done => {
                let w = self.watches.add(NewWatch {
                    name: format!("Todo: {}", c.item.text),
                    check: Check::Time { at },
                    negate: false,
                    interval_secs: 1,
                    title: c.list.clone(),
                    message: c.item.text.clone(),
                    once: true,
                    sticky: true,
                    source,
                });
                Some(w.id)
            }
            _ => None,
        };
        self.todos.set_watch(c.item.id, watch);
    }

    fn tool_todo_lists(&self, a: &Value) -> String {
        if let Some(key) = str_arg(a, "list") {
            return match self.todos.list(&key) {
                Some(l) => crate::todo::render(&l),
                None => format!("No list \"{key}\"."),
            };
        }
        let lists = self.todos.lists();
        if lists.is_empty() {
            return "No todo lists yet. Make one with todo_create_list or todo_add.".into();
        }
        lists
            .iter()
            .map(crate::todo::render)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn tool_todo_create_list(&self, a: &Value) -> Result<String, String> {
        let name = str_arg(a, "name").ok_or("`name` is required")?;
        let items: Vec<String> = a
            .get("items")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .filter_map(|i| i.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let l = self.todos.create_list(&name, &items)?;
        self.notifier.panel_changed();
        Ok(crate::todo::render(&l))
    }

    fn tool_todo_add(&self, a: &Value) -> Result<String, String> {
        let text = str_arg(a, "text").ok_or("`text` is required")?;
        let remind = Self::todo_reminder(a)?.flatten();
        let c = self.todos.add(
            &str_arg(a, "list").unwrap_or_default(),
            &text,
            &str_arg(a, "note").unwrap_or_default(),
            remind,
        )?;
        if remind.is_some() {
            self.sync_reminder(&c, str_arg(a, "source"));
        }
        self.notifier.panel_changed();
        let when = remind.map_or(String::new(), |at| {
            format!("; the dog will bark it {}", crate::todo::local_time(at))
        });
        Ok(format!("Added #{} to \"{}\"{when}.", c.item.id, c.list))
    }

    fn tool_todo_update(&self, a: &Value) -> Result<String, String> {
        let id = a
            .get("id")
            .and_then(Value::as_u64)
            .ok_or("`id` is required")?;
        let reminder = Self::todo_reminder(a)?;
        let c = self.todos.update(
            id,
            a.get("done").and_then(Value::as_bool),
            str_arg(a, "text").as_deref(),
            str_arg(a, "note").as_deref(),
            reminder,
        )?;
        // Done items do not bark; undone ones with a reminder in the future do again.
        if reminder.is_some() || a.get("done").is_some() {
            self.sync_reminder(&c, str_arg(a, "source"));
        }
        self.notifier.panel_changed();
        Ok(format!(
            "#{} in \"{}\": [{}] {}",
            c.item.id,
            c.list,
            if c.item.done { "x" } else { " " },
            c.item.text
        ))
    }

    fn tool_todo_remove(&self, a: &Value) -> Result<String, String> {
        let out = match (a.get("id").and_then(Value::as_u64), str_arg(a, "list")) {
            (Some(id), None) => {
                let item = self.todos.remove_item(id)?;
                if let Some(w) = item.watch {
                    self.watches.remove(w);
                }
                format!("Removed #{id}.")
            }
            (None, Some(list)) if a.get("done_only").and_then(Value::as_bool).unwrap_or(false) => {
                format!("Removed {} done item(s).", self.todos.clear_done(&list)?)
            }
            (None, Some(list)) => {
                let l = self.todos.remove_list(&list)?;
                for w in l.items.iter().filter_map(|i| i.watch) {
                    self.watches.remove(w);
                }
                format!("Removed the list \"{}\" ({} items).", l.name, l.items.len())
            }
            _ => return Err("give `id` (an item) or `list` (a whole list)".into()),
        };
        self.notifier.panel_changed();
        Ok(out)
    }

    fn tool_check_now(&self, a: &Value) -> Result<String, String> {
        let check = parse_check(a)?;
        let negate = a.get("negate").and_then(Value::as_bool).unwrap_or(false);
        let (ok, detail) = match crate::watch::run_check(&check) {
            Ok(r) => r,
            Err(e) => (false, e),
        };
        Ok(format!(
            "condition {}: {detail}",
            if ok != negate { "MET" } else { "not met" }
        ))
    }

    fn tool_status(&self) -> String {
        let s = self.status.lock().unwrap().clone();
        let v = json!({
            "name": self.name(),
            "age": s.age,
            "activity": s.activity,
            "visible": s.visible,
            "showing": s.showing,
            "queued": s.queued,
            "watches": self.watches.list().len(),
            "open_todos": self.todos.lists().iter().map(|l| l.open_count()).sum::<usize>(),
            "quiet": s.quiet,
            "user_idle_secs": s.idle_secs,
            "note": "user_idle_secs is how long since the user touched the mouse or keyboard. \
                Do not skip notifications because of it: the user may be listening for the bark. \
                When idle for long, prefer sticky so the message waits for them.",
        });
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }

    fn tool_session(&self, a: &Value) -> Result<String, String> {
        let id = str_arg(a, "id").ok_or("`id` is required")?;
        let state = match str_arg(a, "state") {
            Some(s) => Some(crate::sessions::State::parse(&s).ok_or_else(|| {
                format!(
                    "unknown state `{s}`; use one of {}",
                    crate::sessions::State::NAMES.join(", ")
                )
            })?),
            None => None,
        };
        let end = a.get("end").and_then(Value::as_bool).unwrap_or(false);
        let open = str_arg(a, "open");
        if let Some(o) = &open
            && !is_web_url(o)
        {
            return Err("`open` must be an http(s) link".into());
        }
        let changed = self.sessions.report(
            crate::sessions::Report {
                id: id.clone(),
                source: str_arg(a, "source"),
                title: str_arg(a, "title"),
                project: str_arg(a, "project"),
                state,
                detail: str_arg(a, "detail").map(|d| d.chars().take(160).collect()),
                open,
                end,
            },
            crate::watch::now_unix(),
        );
        if changed {
            self.notifier.panel_changed();
        }
        Ok(if end {
            format!("Session `{id}` ended.")
        } else {
            format!("Session `{id}` updated; the user sees it in the dog's Agents list.")
        })
    }

    fn tool_sessions(&self) -> String {
        let list = self.sessions.list();
        if list.is_empty() {
            return "No agent sessions.".into();
        }
        serde_json::to_string_pretty(&list).unwrap_or_default()
    }

    fn tool_history(&self, a: &Value) -> String {
        let limit = a
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, HISTORY_LEN as u64) as usize;
        let s = self.status.lock().unwrap();
        if s.history.is_empty() {
            return "No notifications yet.".into();
        }
        let recent: Vec<&HistoryEntry> = s.history.iter().rev().take(limit).collect();
        serde_json::to_string_pretty(&recent).unwrap_or_default()
    }
}

/// The scheduler's tools (`cron_*`).
fn cron_tools(name: &str) -> Value {
    let dog = dog_label(name);
    let source = json!({ "type": "string", "description": "Who is sending this (your name, e.g. \"OpenCode\", \"CI\"); shown above the title." });
    json!([
        {
            "name": "cron_add",
            "title": "Schedule a job",
            "description": format!("Schedules a repeating job with a cron expression, in the user's local time: a reminder that {dog} barks (kind notify), an HTTP request (kind http), or a shell command (kind command). Requests and commands tell the user when they fail (notify: failure, the default), every time (always) or never; each run goes to History. Use remind for a one-off reminder instead."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "schedule": { "type": "string", "description": "5 fields: minute hour day-of-month month day-of-week (e.g. \"0 9 * * 1-5\" weekdays at 09:00, \"*/15 * * * *\" every 15 minutes), or @hourly, @daily, @weekly, @monthly, @yearly." },
                    "kind": { "type": "string", "enum": ["notify", "http", "command"], "description": "Default notify." },
                    "name": { "type": "string", "description": "Shown in the user's Schedule list; default describes the schedule." },
                    "title": { "type": "string", "description": "notify: the bubble's title (default \"Reminder\")." },
                    "message": { "type": "string", "description": "notify: the text." },
                    "sticky": { "type": "boolean", "description": "notify: stays until clicked." },
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"] },
                    "url": { "type": "string", "description": "http: an http(s) URL." },
                    "headers": { "type": "object", "additionalProperties": { "type": "string" } },
                    "body": { "type": "string" },
                    "expect_status": { "type": "integer", "description": "http: success status (default any 2xx)." },
                    "contains": { "type": "string", "description": "http: the body must contain this." },
                    "command": { "type": "string", "description": "command: run with the user's shell; succeeds on exit 0." },
                    "workdir": { "type": "string" },
                    "notify": { "type": "string", "enum": ["failure", "always", "never"] },
                    "source": source
                },
                "required": ["schedule"]
            }
        },
        {
            "name": "day_add",
            "title": "Add a special day",
            "description": format!("Adds a day {dog} remembers every year (or once): a birthday, an anniversary, a holiday. On the day {dog} shows a bubble at `at` (default 09:00), with how many years when the year is known, and can wear a party hat (birthdays) or a pin on its collar. It can remind days before, and go to the phone. The user sees them in Special days."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Whose day or what it is (\"Ayşe\", \"Our wedding\")." },
                    "date": { "type": "string", "description": "MM-DD, or YYYY-MM-DD when the year is known (the bubble then says how many years)." },
                    "kind": { "type": "string", "enum": ["birthday", "anniversary", "holiday", "other"], "description": "Default birthday." },
                    "once": { "type": "boolean", "description": "Only once (needs the year), not every year." },
                    "message": { "type": "string", "description": "The bubble's text; default depends on the kind." },
                    "at": { "type": "string", "description": "Local time of the bubble, HH:MM (default 09:00)." },
                    "remind_days_before": { "type": "integer", "description": "Also remind this many days before (0-30)." },
                    "look": { "type": "string", "enum": ["auto", "none", "party_hat", "heart", "star", "ribbon", "flag"], "description": "What the dog wears on the day; auto: party hat for birthdays, a heart for anniversaries, a star for holidays." },
                    "phone": { "type": "boolean", "description": "Also send it to the user's phone (if set up)." },
                    "source": source
                },
                "required": ["name", "date"]
            }
        },
        {
            "name": "day_list",
            "title": "Special days",
            "description": "Lists the user's own special days with the next date of each.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "day_remove",
            "title": "Remove a special day",
            "description": "Removes one of the user's special days by id.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] },
            "annotations": { "destructiveHint": true }
        },
        {
            "name": "cron_list",
            "title": "Scheduled jobs",
            "description": "Lists the scheduled jobs with their next run and how the last run went.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "cron_remove",
            "title": "Remove a scheduled job",
            "description": "Deletes a scheduled job by id.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] },
            "annotations": { "destructiveHint": true }
        },
        {
            "name": "cron_run",
            "title": "Run a scheduled job now",
            "description": "Runs a scheduled job once right away, without changing its schedule.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }
        },
        {
            "name": "cron_enable",
            "title": "Pause or resume a scheduled job",
            "description": "Turns a scheduled job off (enabled: false) or on again.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" }, "enabled": { "type": "boolean" } }, "required": ["id", "enabled"] }
        }
    ])
}

/// Default name for a watch.
fn check_label(c: &Check) -> String {
    match c {
        Check::Http { url, .. } => url.clone(),
        Check::Command { command, .. } => command.clone(),
        Check::Port { host, port } => format!("{host}:{port}"),
        Check::File { path, .. } => path.clone(),
        Check::Process { pid } => format!("process {pid}"),
        Check::Time { .. } => "reminder".into(),
        Check::Github { repo, branch, .. } => match branch {
            Some(b) => format!("CI {repo}@{b}"),
            None => format!("CI {repo}"),
        },
    }
}

fn is_web_url(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://"))
        && !url.chars().any(char::is_whitespace)
}

fn parse_check(a: &Value) -> Result<Check, String> {
    let contains = str_arg(a, "contains");
    let url = str_arg(a, "url");
    let command = str_arg(a, "command");
    let port = a.get("port").and_then(Value::as_u64);
    let file = str_arg(a, "file");
    let pid = a.get("pid").and_then(Value::as_u64);
    let github = str_arg(a, "github");
    let given = [
        url.is_some(),
        command.is_some(),
        port.is_some(),
        file.is_some(),
        pid.is_some(),
        github.is_some(),
    ]
    .iter()
    .filter(|g| **g)
    .count();
    if given != 1 {
        return Err(
            "exactly one of `url`, `command`, `port`, `file`, `pid` or `github` is required".into(),
        );
    }
    if let Some(repo) = github {
        return parse_github(&repo, a);
    }
    if let Some(url) = url {
        if !is_web_url(&url) {
            return Err("`url` must start with http:// or https://".into());
        }
        let status = a.get("status").and_then(Value::as_u64).map(|s| s as u16);
        return Ok(Check::Http {
            url,
            status,
            contains,
        });
    }
    if let Some(command) = command {
        return Ok(Check::Command { command, contains });
    }
    if let Some(port) = port {
        let port = u16::try_from(port)
            .ok()
            .filter(|p| *p > 0)
            .ok_or("`port` must be 1-65535")?;
        let host = str_arg(a, "host").unwrap_or_else(|| "127.0.0.1".into());
        return Ok(Check::Port { host, port });
    }
    if let Some(path) = file {
        return Ok(Check::File { path, contains });
    }
    let pid = u32::try_from(pid.unwrap_or(0))
        .ok()
        .filter(|p| *p > 0)
        .ok_or("`pid` must be a positive process id")?;
    Ok(Check::Process { pid })
}

/// `github`: "owner/repo" or a run URL (https://github.com/o/r/actions/runs/123).
fn parse_github(s: &str, a: &Value) -> Result<Check, String> {
    let s = s.trim().trim_end_matches('/');
    let path = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("github.com/"))
        .unwrap_or(s);
    let parts: Vec<&str> = path.split('/').collect();
    let ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    if parts.len() < 2 || !ok(parts[0]) || !ok(parts[1]) {
        return Err("`github` must be \"owner/repo\" or a GitHub Actions run URL".into());
    }
    let repo = format!("{}/{}", parts[0], parts[1]);
    let run = match parts.get(2..4) {
        Some(["actions", "runs"]) => Some(
            parts
                .get(4)
                .and_then(|r| r.parse::<u64>().ok())
                .ok_or("bad run id in the URL")?,
        ),
        _ => a.get("run_id").and_then(Value::as_u64),
    };
    let branch = str_arg(a, "branch");
    if let Some(b) = &branch
        && !b
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
    {
        return Err("bad `branch`".into());
    }
    Ok(Check::Github {
        repo,
        run,
        branch,
        // A little slack: the push that started the run may be a few seconds older.
        since: crate::watch::now_unix().saturating_sub(120),
    })
}

fn tools(name: &str) -> Value {
    let mut all = base_tools(name);
    if let (Some(list), Value::Array(more)) = (all.as_array_mut(), cron_tools(name)) {
        // The scheduler's tools go right after the watches'.
        let at = list
            .iter()
            .position(|t| t["name"] == "check_now")
            .unwrap_or(list.len());
        list.splice(at..at, more);
    }
    all
}

/// The tools, without the scheduler's (one `json!` can only hold so many).
fn base_tools(name: &str) -> Value {
    let dog = dog_label(name);
    let source = json!({ "type": "string", "description": "Who is sending this (your name, e.g. \"OpenCode\", \"CI\"); shown above the title." });
    let session = json!({ "type": "string", "description": "Your session id (see `session`): the user's Agents list shows this session waiting, done or failed." });
    let check_props = json!({
        "url": { "type": "string", "description": "HTTP(S) check: sends a GET request to this URL. Met on 2xx (or `status`)." },
        "status": { "type": "integer", "description": "Expected status code for url (default: any 2xx)." },
        "port": { "type": "integer", "description": "Port check: met when something accepts TCP connections on host:port (e.g. a dev server or database came up)." },
        "host": { "type": "string", "description": "Host for `port` (default 127.0.0.1)." },
        "file": { "type": "string", "description": "File check: met when the file exists, or with `contains`, when its end contains the text (e.g. a log line). ~ is the home folder." },
        "pid": { "type": "integer", "description": "Process check: met while the process runs. Use with negate: true to bark when it ends." },
        "github": { "type": "string", "description": "GitHub Actions check: \"owner/repo\" (the next run, optionally of `branch`) or a run URL. Met when the run finishes; the bubble says passed/failed and links to it. Set GITHUB_TOKEN for private repos. Use a 30-60 s interval." },
        "branch": { "type": "string", "description": "With github: only runs of this branch." },
        "run_id": { "type": "integer", "description": "With github: a specific run id." },
        "command": { "type": "string", "description": "Command check: runs in a shell on the user's computer (cmd /C on Windows). Met if the exit code is 0. Prefer url/port/file/pid when they fit." },
        "contains": { "type": "string", "description": "With url/command/file: the condition also requires the response body / output / file to contain this text." },
        "negate": { "type": "boolean", "description": "Invert the condition (e.g. notify when a site goes DOWN or a process ENDS)." },
    });
    let mut watch_props = check_props.clone();
    for (k, v) in json!({
        "name": { "type": "string", "description": "Short name shown in the list." },
        "title": { "type": "string", "description": "Title shown in the bubble when triggered." },
        "message": { "type": "string", "description": "Message shown in the bubble when triggered." },
        "interval_secs": { "type": "integer", "minimum": MIN_INTERVAL, "description": format!("Check interval in seconds (default {DEFAULT_INTERVAL}).") },
        "once": { "type": "boolean", "description": "true (default): bark on the first trigger and remove the watch. false: bark every time the condition goes from false to true." },
        "sticky": { "type": "boolean", "description": "Keep the bubble until the user clicks it." },
        "source": source,
    })
    .as_object()
    .unwrap()
    {
        watch_props[k] = v.clone();
    }
    json!([
        {
            "name": "notify",
            "title": format!("Notify via {name}"),
            "description": format!("{dog}, the dog on the user's screen, barks and shows the message in a speech bubble. Use it when a long task finishes or when you need the user's attention. Always send it when something needs the user, even if they seem active: they listen for the bark. The dog looks happy for good news and sad for failures (see `mood`)."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short title." },
                    "message": { "type": "string", "description": "Message (a few sentences)." },
                    "source": source,
                    "mood": { "type": "string", "enum": Mood::NAMES, "description": "good (it worked: the dog is happy), bad (it failed: the dog is sad and howls) or neutral. Default: guessed from the words (failed, error, passed, done...)." },
                    "url": { "type": "string", "description": "http(s) link opened when the user clicks the bubble (a PR, a failed job, a preview)." },
                    "copy": { "type": "string", "description": "Text put on the user's clipboard when the bubble appears (a command to run, a token, a path)." },
                    "session": session,
                    "sticky": { "type": "boolean", "description": "Keep it until the user clicks it." },
                    "timeout_secs": { "type": "integer", "description": "How long the bubble stays, in seconds (default 10, 30 with a url)." }
                }
            }
        },
        {
            "name": "ask",
            "title": "Ask the user",
            "description": format!("Shows a question in {dog}'s bubble with answer buttons, barks, and WAITS until the user clicks one; returns the chosen answer. Use it for quick decisions (yes/no, pick one of a few) so the user does not have to switch to the terminal. Returns an error if nobody answers in time."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The question (a sentence or two)." },
                    "choices": { "type": "array", "items": { "type": "string" }, "maxItems": 4, "description": "Answer buttons, 1-4 short labels (default [\"Yes\", \"No\"]). The first is highlighted as the suggested answer." },
                    "title": { "type": "string", "description": "Short title above the question." },
                    "source": source,
                    "key": { "type": "string", "description": "Your id for this question, to withdraw it with `dismiss` (e.g. when it was answered somewhere else)." },
                    "allow_text": { "type": "boolean", "description": "The user may also type an answer in their own words (\"Reply…\"); the result is then that text instead of a choice." },
                    "session": session,
                    "timeout_secs": { "type": "integer", "minimum": 10, "maximum": ASK_TIMEOUT_MAX, "description": format!("How long to wait for an answer (default {ASK_TIMEOUT}).") }
                },
                "required": ["question"]
            }
        },
        {
            "name": "dismiss",
            "title": "Withdraw a question",
            "description": "Removes a pending `ask` (shown or queued) by its `key`, e.g. because the user answered it in the terminal. The waiting `ask` call returns with no answer.",
            "inputSchema": {
                "type": "object",
                "properties": { "key": { "type": "string" } },
                "required": ["key"]
            }
        },
        {
            "name": "progress",
            "title": "Show progress",
            "description": format!("Shows or updates a progress bar in {dog}'s bubble for a long task (build, tests, migration). Call with the same `id` to update; finish with state done (the dog is happy) or failed (the dog is sad). Only the first call barks."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Identifies the task; reuse it for updates." },
                    "title": { "type": "string", "description": "Task name (default: the id)." },
                    "message": { "type": "string", "description": "Current step (default: \"value / total\")." },
                    "value": { "type": "number", "description": "Progress: with `total`, steps done; alone, a fraction 0-1 or a percent 0-100. Omit for an indeterminate bar." },
                    "total": { "type": "number", "description": "Total steps." },
                    "state": { "type": "string", "enum": ["running", "done", "failed"], "description": "Default running." },
                    "source": source,
                    "session": session
                },
                "required": ["id"]
            }
        },
        {
            "name": "react",
            "title": format!("Make {name} react"),
            "description": format!("{dog} does something without a bubble: happy (celebrate a success), sad (something went wrong), bark, zoomies (runs around, for big wins), fetch (the user throws the bone and the dog brings it back: a little break), sleep or wake. Only put the dog to sleep if the user asks: while asleep it does not bark and only questions, progress and sticky messages are shown."),
            "inputSchema": {
                "type": "object",
                "properties": { "reaction": { "type": "string", "enum": Reaction::NAMES } },
                "required": ["reaction"]
            }
        },
        {
            "name": "copy",
            "title": "Copy to clipboard",
            "description": "Puts text on the user's clipboard and tells them in a bubble. Use it for a command they should run, a URL, a snippet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to copy." },
                    "title": { "type": "string", "description": "Bubble title (default \"Copied to clipboard\")." },
                    "message": { "type": "string", "description": "Bubble message (default: the start of the text)." },
                    "source": source
                },
                "required": ["text"]
            }
        },
        {
            "name": "remind",
            "title": "Set a reminder",
            "description": format!("{dog} barks with a message later: after some minutes, or at a local time (today, or tomorrow if it has passed). Reminders survive restarts; list and remove them with list_watches / remove_watch."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "What to remind about." },
                    "after_minutes": { "type": "number", "description": "Minutes from now (up to a week)." },
                    "at": { "type": "string", "description": "Local time \"HH:MM\" (24 h)." },
                    "title": { "type": "string", "description": "Bubble title (default \"Reminder\")." },
                    "sticky": { "type": "boolean", "description": "Keep the bubble until clicked (default true)." },
                    "source": source
                },
                "required": ["message"]
            }
        },
        {
            "name": "add_watch",
            "title": "Add a status watch",
            "description": format!("{dog} checks a condition at regular intervals; when it is met, the dog barks and notifies the user. Exactly one of url, port, file, pid or command. Examples: 'when the site is up' (url), 'when the dev server listens' (port), 'when the build output appears' (file), 'when an ERROR shows up in the log' (file + contains), 'when this process finishes' (pid + negate), 'when the site goes down' (url + negate). Watches persist across dog restarts."),
            "inputSchema": { "type": "object", "properties": watch_props }
        },
        {
            "name": "list_watches",
            "title": "List watches",
            "description": "Lists saved status watches and reminders with their latest results and ids.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "remove_watch",
            "title": "Remove a watch",
            "description": "Removes a status watch or reminder by id.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] },
            "annotations": { "destructiveHint": true }
        },
        {
            "name": "check_now",
            "title": "Test a condition now",
            "description": "Runs a check once right away without saving it and returns the result; use it to verify a condition before add_watch.",
            "inputSchema": { "type": "object", "properties": check_props }
        },
        {
            "name": "status",
            "title": format!("{name}'s status"),
            "description": format!("Returns the dog's name ({name}) and age, what it is doing, the notification on screen, queued notifications, watches, whether it is quiet (Quiet on, quiet hours, a full-screen app, or the dog put to sleep: no barking, only questions/progress/sticky shown), and how long the user has been idle (no mouse/keyboard)."),
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "session",
            "title": "Report a session",
            "description": format!("Tells {dog} what your session is doing, so the user can follow several agents at once in the Agents list (settings): working (with the current step in `detail`), waiting (for the user), done, failed or idle. Call it when you start working, when you wait for the user and when you finish; `end` removes it. Questions and progress with the same `session` update it too."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "A stable id for this session (e.g. your session id)." },
                    "state": { "type": "string", "enum": crate::sessions::State::NAMES },
                    "title": { "type": "string", "description": "What the session works on." },
                    "project": { "type": "string", "description": "The project folder's name." },
                    "detail": { "type": "string", "description": "The current step, or the last message." },
                    "open": { "type": "string", "description": "An http(s) link that shows the session (opened when the user clicks it)." },
                    "end": { "type": "boolean", "description": "The session is over: remove it." },
                    "source": source
                },
                "required": ["id"]
            }
        },
        {
            "name": "sessions",
            "title": "Agent sessions",
            "description": "Lists the agent sessions the user follows and what each is doing.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "todo_lists",
            "title": "Todo lists",
            "description": format!("The user's todo lists kept with {dog}: every list with its items (id, done or not, reminder), or one list. Read them before adding, to avoid duplicates."),
            "inputSchema": { "type": "object", "properties": { "list": { "type": "string", "description": "A list name or id (default: all lists)." } } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "todo_create_list",
            "title": "New todo list",
            "description": "Creates a named todo list for a piece of work, with its steps (\"Release 0.3\": tag, build, write notes). If a list with that name exists, the items are added to it. The user sees the lists in the dog's settings (Todo tab).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The list's name." },
                    "items": { "type": "array", "items": { "type": "string" }, "description": "Items to add, in order." }
                },
                "required": ["name"]
            }
        },
        {
            "name": "todo_add",
            "title": "Add a todo",
            "description": format!("Adds an item to a todo list (created if needed; default list \"{}\"). With a reminder, {dog} barks with it at that time unless it is done by then.", crate::todo::DEFAULT_LIST),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "What to do." },
                    "list": { "type": "string", "description": "List name or id." },
                    "note": { "type": "string", "description": "Details." },
                    "remind_after_minutes": { "type": "number", "description": "Bark about it after this many minutes (up to 30 days)." },
                    "remind_at": { "type": "string", "description": "Bark about it at a local time: \"HH:MM\" (next one) or \"YYYY-MM-DD HH:MM\"." },
                    "source": source
                },
                "required": ["text"]
            }
        },
        {
            "name": "todo_update",
            "title": "Update a todo",
            "description": "Checks off an item (done: true) or changes its text, note or reminder (remind_at: \"\" removes it). Done items do not bark.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Item id (from todo_lists)." },
                    "done": { "type": "boolean" },
                    "text": { "type": "string" },
                    "note": { "type": "string" },
                    "remind_after_minutes": { "type": "number" },
                    "remind_at": { "type": "string", "description": "\"HH:MM\", \"YYYY-MM-DD HH:MM\", or \"\" to remove the reminder." },
                    "source": source
                },
                "required": ["id"]
            }
        },
        {
            "name": "todo_remove",
            "title": "Remove todos",
            "description": "Removes an item (id), a whole list (list), or only the done items of a list (list + done_only).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "An item id." },
                    "list": { "type": "string", "description": "A list name or id." },
                    "done_only": { "type": "boolean", "description": "With list: remove only its done items." }
                }
            },
            "annotations": { "destructiveHint": true }
        },
        {
            "name": "history",
            "title": "Notification history",
            "description": "Recent notifications, newest first, with what happened to each (shown, answered, dismissed, expired, skipped while quiet...). Use it to see what the user may have missed.",
            "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": HISTORY_LEN, "description": "How many (default 10)." } } },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

/// "Karabaş (the user's dog)" or "Dost" when it has no name of its own.
fn dog_label(name: &str) -> String {
    if name == crate::settings::DEFAULT_NAME {
        name.to_string()
    } else {
        format!("{name} (the user's dog)")
    }
}

/// `mood`: `None` when not given (guessed from the text).
fn parse_mood(a: &Value) -> Result<Option<Mood>, String> {
    match str_arg(a, "mood") {
        None => Ok(None),
        Some(m) => Mood::parse(&m)
            .map(Some)
            .ok_or_else(|| format!("unknown mood `{m}`; use one of {}", Mood::NAMES.join(", "))),
    }
}

/// Unix seconds for a local "YYYY-MM-DD HH:MM" in the future.
fn parse_local_datetime(s: &str) -> Option<u64> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let t = NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M").ok()?;
    let at = Local.from_local_datetime(&t).earliest()?.timestamp();
    (at > Local::now().timestamp()).then_some(at as u64)
}

fn str_arg(a: &Value, key: &str) -> Option<String> {
    a.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn hdr(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap()
}

fn header(req: &tiny_http::Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn json_response(code: u16, v: &Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(serde_json::to_vec(v).unwrap_or_default())
        .with_status_code(code)
        .with_header(hdr("Content-Type", "application/json"))
}

fn is_local_origin(origin: &str) -> bool {
    let host = origin.split("://").nth(1).unwrap_or(origin);
    let host = host.split('/').next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, p)| {
        if p.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// `pattern` from the allowed origins: `*`, an exact origin, or one `*` as a wildcard
/// (`https://*.example.com`).
fn origin_matches(pattern: &str, origin: &str) -> bool {
    let (pattern, origin) = (pattern.trim_end_matches('/'), origin.trim_end_matches('/'));
    match pattern.split_once('*') {
        _ if pattern == "*" => true,
        None => pattern.eq_ignore_ascii_case(origin),
        Some((pre, post)) => {
            let o = origin.to_ascii_lowercase();
            o.len() > pre.len() + post.len()
                && o.starts_with(&pre.to_ascii_lowercase())
                && o.ends_with(&post.to_ascii_lowercase())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify;

    fn mcp() -> (Mcp, std::sync::mpsc::Receiver<notify::Event>) {
        let (tx, rx) = notify::channel();
        let watches = Watches::new(tx.clone(), None);
        let todos = Arc::new(Todos::new(None));
        (
            Mcp::new(
                tx.clone(),
                watches,
                todos,
                Arc::default(),
                Arc::default(),
                crate::cron::Cron::new(tx, None),
                Arc::new(crate::days::Days::new(None)),
            ),
            rx,
        )
    }

    #[test]
    fn github_targets() {
        let a = json!({});
        let run = parse_github("https://github.com/rytsh/dost/actions/runs/123", &a).unwrap();
        assert!(
            matches!(run, Check::Github { ref repo, run: Some(123), .. } if repo == "rytsh/dost")
        );
        let b = json!({ "branch": "main" });
        let next = parse_github("rytsh/dost", &b).unwrap();
        assert!(
            matches!(next, Check::Github { run: None, branch: Some(ref br), .. } if br == "main")
        );
        assert!(parse_github("nope", &a).is_err());
        assert!(parse_github("a/b c", &a).is_err());
        assert!(parse_github("a/b", &json!({ "branch": "x&y" })).is_err());
    }

    fn note(rx: &std::sync::mpsc::Receiver<notify::Event>) -> Notification {
        rx.try_recv().unwrap().into_notification().unwrap()
    }

    fn call(m: &Mcp, name: &str, args: Value) -> Value {
        let r = m.dispatch(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } }));
        r.unwrap()["result"].clone()
    }

    #[test]
    fn initialize_and_list() {
        let (m, _) = mcp();
        let r = m.dispatch(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert!(
            m.dispatch(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .is_none()
        );
        let r = m
            .dispatch(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .unwrap();
        let names: Vec<_> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "notify",
                "ask",
                "dismiss",
                "progress",
                "react",
                "copy",
                "remind",
                "add_watch",
                "list_watches",
                "remove_watch",
                "cron_add",
                "day_add",
                "day_list",
                "day_remove",
                "cron_list",
                "cron_remove",
                "cron_run",
                "cron_enable",
                "check_now",
                "status",
                "session",
                "sessions",
                "todo_lists",
                "todo_create_list",
                "todo_add",
                "todo_update",
                "todo_remove",
                "history"
            ]
        );
        let r = m
            .dispatch(json!({ "jsonrpc": "2.0", "id": 3, "method": "nope" }))
            .unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn todo_tools() {
        let (m, _) = mcp();
        let text = |v: Value| v["content"][0]["text"].as_str().unwrap().to_string();
        assert!(text(call(&m, "todo_lists", json!({}))).starts_with("No todo lists"));
        let r = call(
            &m,
            "todo_create_list",
            json!({ "name": "Release 0.3", "items": ["tag", "build"] }),
        );
        assert!(text(r).contains("2 open"));
        // An item with a reminder: a watch barks it, until it is done.
        let r = call(
            &m,
            "todo_add",
            json!({ "list": "release 0.3", "text": "notes", "remind_after_minutes": 30 }),
        );
        assert!(text(r.clone()).contains("bark"), "{r}");
        assert_eq!(m.watches.list().len(), 1);
        let id = m.todos.list("Release 0.3").unwrap().items[2].id;
        call(&m, "todo_update", json!({ "id": id, "done": true }));
        assert!(m.watches.list().is_empty(), "done: no bark");
        call(&m, "todo_update", json!({ "id": id, "done": false }));
        assert_eq!(m.watches.list().len(), 1, "undone: it barks again");
        call(&m, "todo_update", json!({ "id": id, "remind_at": "" }));
        assert!(m.watches.list().is_empty());
        assert!(
            call(&m, "todo_add", json!({ "text": "x", "remind_at": "soon" }))["isError"] == true
        );
        // Default list, then removing.
        call(
            &m,
            "todo_add",
            json!({ "text": "buy milk", "remind_at": "2099-01-02 09:00" }),
        );
        assert!(text(call(&m, "todo_lists", json!({ "list": "Todo" }))).contains("buy milk"));
        assert_eq!(m.watches.list().len(), 1);
        call(&m, "todo_remove", json!({ "list": "Todo" }));
        assert!(
            m.watches.list().is_empty(),
            "the list's reminders go with it"
        );
        let r = call(
            &m,
            "todo_remove",
            json!({ "list": "Release 0.3", "done_only": true }),
        );
        assert!(text(r).contains("0 done"));
        assert_eq!(call(&m, "todo_remove", json!({}))["isError"], true);
    }

    #[test]
    fn notify_tool_sends() {
        let (m, rx) = mcp();
        let r = call(
            &m,
            "notify",
            json!({ "title": "Build", "message": "done", "sticky": true }),
        );
        assert_eq!(r["isError"], false);
        let n = note(&rx);
        assert_eq!(
            (n.title.as_str(), n.body.as_str(), n.timeout),
            ("Build", "done", crate::notify::Timeout::Sticky)
        );
        assert_eq!(call(&m, "notify", json!({}))["isError"], true);

        let r = call(
            &m,
            "notify",
            json!({ "title": "PR", "url": "https://example.com/pr/1", "copy": "git pull", "source": "CI" }),
        );
        assert_eq!(r["isError"], false);
        let n = note(&rx);
        assert_eq!(n.url.as_deref(), Some("https://example.com/pr/1"));
        assert_eq!(n.copy.as_deref(), Some("git pull"));
        assert_eq!(n.source.as_deref(), Some("CI"));
        assert_eq!(
            call(
                &m,
                "notify",
                json!({ "title": "x", "url": "file:///etc/passwd" })
            )["isError"],
            true
        );
    }

    #[test]
    fn local_calls_ask_the_dog_and_go_to_history() {
        let (m, rx) = mcp();
        let m = Arc::new(m);
        let dir = std::env::temp_dir().join(format!("dost-mcp-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        m.local.configure(&crate::local::LocalSettings {
            enabled: true,
            root: dir.display().to_string(),
            ..Default::default()
        });
        let run = |cmd: &str| {
            let m = Arc::clone(&m);
            let msg = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "shell", "arguments": { "command": cmd, "source": "Test" } } });
            thread::spawn(move || m.dispatch_local(msg).unwrap()["result"].clone())
        };
        // The next event that is a question; History entries and rules are collected.
        let next_question = || loop {
            match rx
                .recv_timeout(Duration::from_secs(5))
                .expect("nothing arrived")
            {
                notify::Event::Notify(n) => return *n,
                _ => continue,
            }
        };

        let t = run("echo hi");
        let q = next_question();
        // The labels follow the language (another test may switch it): pick by position.
        assert_eq!(q.body, "echo hi");
        assert_eq!(q.choices.len(), 3);
        q.reply
            .as_ref()
            .unwrap()
            .send(q.choices[2].clone())
            .unwrap();
        let r = t.join().unwrap();
        assert_eq!(r["isError"], true);

        let t = run("echo hi");
        {
            let q = next_question();
            q.reply
                .as_ref()
                .unwrap()
                .send(q.choices[1].clone())
                .unwrap();
        }
        let r = t.join().unwrap();
        assert_eq!(r["isError"], false, "{r}");
        // Now it runs without asking, and History has it.
        let r = run("echo again").join().unwrap();
        assert!(r["content"][0]["text"].as_str().unwrap().contains("again"));
        let mut events = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_millis(300)) {
            events.push(e);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, notify::Event::AllowAlways(r) if r == "shell:echo*"))
        );
        let outcomes: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                notify::Event::Record(h) => Some(format!("{} {}", h.title, h.outcome)),
                _ => None,
            })
            .collect();
        assert!(
            outcomes.iter().any(|o| o == "$ echo again done"),
            "{outcomes:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ask_waits_for_the_answer() {
        let (m, rx) = mcp();
        let m = Arc::new(m);
        let asker = {
            let m = Arc::clone(&m);
            thread::spawn(move || {
                call(
                    &m,
                    "ask",
                    json!({ "question": "Update tests?", "choices": ["Yes", "Later"] }),
                )
            })
        };
        let n = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("no question arrived")
            .into_notification()
            .unwrap();
        assert_eq!(n.choices, ["Yes", "Later"]);
        n.reply.as_ref().unwrap().send("Later".into()).unwrap();
        let r = asker.join().unwrap();
        assert_eq!(r["isError"], false);
        assert!(r["content"][0]["text"].as_str().unwrap().contains("Later"));

        // An expired question drops its sender: the call returns an error, not a hang.
        let asker = {
            let m = Arc::clone(&m);
            thread::spawn(move || call(&m, "ask", json!({ "question": "Still there?" })))
        };
        let n = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .into_notification()
            .unwrap();
        assert_eq!(n.choices, ["Yes", "No"]);
        drop(n);
        assert_eq!(asker.join().unwrap()["isError"], true);

        assert_eq!(
            call(
                &m,
                "ask",
                json!({ "question": "?", "choices": ["a", "b", "c", "d", "e"] })
            )["isError"],
            true
        );
    }

    #[test]
    fn progress_react_copy_remind() {
        let (m, rx) = mcp();
        let r = call(
            &m,
            "progress",
            json!({ "id": "build", "value": 3, "total": 10 }),
        );
        assert_eq!(r["isError"], false);
        let n = note(&rx);
        let p = n.progress.unwrap();
        assert_eq!((p.id.as_str(), p.state), ("build", ProgressState::Running));
        assert!((p.value.unwrap() - 0.3).abs() < 1e-6);
        assert_eq!(n.body, "3 / 10");
        call(&m, "progress", json!({ "id": "build", "state": "done" }));
        assert_eq!(note(&rx).progress.unwrap().state, ProgressState::Done);
        assert_eq!(call(&m, "progress", json!({}))["isError"], true);
        assert_eq!(
            call(&m, "progress", json!({ "id": "x", "state": "weird" }))["isError"],
            true
        );

        assert_eq!(
            call(&m, "react", json!({ "reaction": "zoomies" }))["isError"],
            false
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            notify::Event::React(Reaction::Zoomies)
        ));
        assert_eq!(
            call(&m, "react", json!({ "reaction": "fly" }))["isError"],
            true
        );

        call(&m, "copy", json!({ "text": "npm run dev" }));
        assert_eq!(note(&rx).copy.as_deref(), Some("npm run dev"));

        let r = call(
            &m,
            "remind",
            json!({ "message": "Stretch", "after_minutes": 25 }),
        );
        assert_eq!(r["isError"], false, "{r}");
        let w = &m.watches.list()[0];
        assert!(matches!(w.check, Check::Time { .. }));
        assert!(w.sticky);
        assert_eq!(
            call(&m, "remind", json!({ "message": "x", "at": "25:99" }))["isError"],
            true
        );
        assert_eq!(
            call(&m, "remind", json!({ "message": "x" }))["isError"],
            true
        );
    }

    #[test]
    fn new_watch_kinds() {
        let (m, _) = mcp();
        for (args, label) in [
            (json!({ "port": 5432 }), "127.0.0.1:5432"),
            (
                json!({ "file": "~/build.log", "contains": "ERROR" }),
                "~/build.log",
            ),
            (json!({ "pid": 4242, "negate": true }), "process 4242"),
        ] {
            let r = call(&m, "add_watch", args);
            assert_eq!(r["isError"], false, "{r}");
            assert_eq!(m.watches.list().last().unwrap().name, label);
        }
        assert_eq!(
            call(&m, "add_watch", json!({ "port": 99999 }))["isError"],
            true
        );
        assert_eq!(
            call(&m, "add_watch", json!({ "port": 80, "pid": 1 }))["isError"],
            true
        );
    }

    #[test]
    fn status_and_history() {
        let (m, _) = mcp();
        {
            let mut s = m.status.lock().unwrap();
            s.idle_secs = Some(42);
            s.history.push_back(HistoryEntry {
                id: 1,
                at: 0,
                source: None,
                title: "Old".into(),
                message: String::new(),
                outcome: "shown".into(),
            });
            s.history.push_back(HistoryEntry {
                id: 2,
                at: 0,
                source: Some("CI".into()),
                title: "New".into(),
                message: String::new(),
                outcome: "answered: Yes".into(),
            });
        }
        let r = call(&m, "status", json!({}));
        let v: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["user_idle_secs"], 42);
        let r = call(&m, "history", json!({ "limit": 1 }));
        let v: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["title"], "New");
        assert_eq!(v[0]["source"], "CI");
    }

    #[test]
    fn watch_tools() {
        let (m, _) = mcp();
        assert_eq!(
            call(&m, "add_watch", json!({ "name": "x" }))["isError"],
            true
        );
        assert_eq!(
            call(&m, "add_watch", json!({ "url": "ftp://x" }))["isError"],
            true
        );
        let r = call(
            &m,
            "add_watch",
            json!({ "url": "http://localhost:3000", "interval_secs": 1 }),
        );
        assert_eq!(r["isError"], false, "{r}");
        let w = &m.watches.list()[0];
        assert_eq!(w.interval_secs, MIN_INTERVAL);
        assert_eq!(
            call(&m, "remove_watch", json!({ "id": w.id }))["isError"],
            false
        );
        assert!(m.watches.list().is_empty());
    }

    #[test]
    fn name_mood_and_new_reactions() {
        let (m, rx) = mcp();
        let list = |m: &Mcp| {
            m.dispatch(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
                .unwrap()["result"]["tools"]
                .to_string()
        };
        assert!(!list(&m).contains("the user's dog"), "no name yet");
        m.set_name("Karabaş");
        let tools = list(&m);
        assert!(tools.contains("Notify via Karabaş"), "{tools}");
        assert!(tools.contains("Karabaş (the user's dog)"));
        let r = call(&m, "status", json!({}));
        let v: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["name"], "Karabaş");

        call(&m, "notify", json!({ "title": "Deploy", "mood": "bad" }));
        assert_eq!(note(&rx).mood, Mood::Bad);
        call(&m, "notify", json!({ "title": "Tests passed" }));
        assert_eq!(note(&rx).mood, Mood::Good, "guessed");
        assert_eq!(
            call(&m, "notify", json!({ "title": "x", "mood": "meh" }))["isError"],
            true
        );
        call(&m, "progress", json!({ "id": "b", "state": "failed" }));
        assert_eq!(note(&rx).mood, Mood::Bad);
        for (name, r) in [("sad", Reaction::Sad), ("fetch", Reaction::Fetch)] {
            let out = call(&m, "react", json!({ "reaction": name }));
            assert!(
                out["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .starts_with("Karabaş")
            );
            assert!(matches!(rx.try_recv().unwrap(), notify::Event::React(x) if x == r));
        }
    }

    #[test]
    fn origins() {
        assert!(is_local_origin("http://localhost:5173"));
        assert!(is_local_origin("http://127.0.0.1"));
        assert!(!is_local_origin("https://evil.example"));
        assert!(!is_local_origin("http://localhost.evil.example"));
        let local_only = Cors::default();
        assert!(local_only.allows("http://localhost:3000"));
        assert!(!local_only.allows("https://app.example.com"));
        assert!(Cors::open().allows("https://anything.test"));
        let some = Cors {
            allow_origins: vec!["https://*.example.com".into(), "https://chat.test/".into()],
            ..Cors::default()
        };
        assert!(some.allows("https://app.example.com"));
        assert!(some.allows("https://chat.test"));
        assert!(!some.allows("https://example.com"));
        assert!(!some.allows("https://evil.test"));
    }

    #[test]
    fn local_tools_only_while_turned_on() {
        let (m, _rx) = mcp();
        let m = Arc::new(m);
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let (srv, mm) = (Arc::clone(&server), Arc::clone(&m));
        thread::spawn(move || {
            for req in srv.incoming_requests() {
                mm.handle(req);
            }
        });
        let post = |path: &str, body: &str| {
            use std::io::Write;
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        assert!(
            post("/mcp/local", list).starts_with("HTTP/1.1 404"),
            "off by default"
        );
        m.local.configure(&crate::local::LocalSettings {
            enabled: true,
            read_only: true,
            ..Default::default()
        });
        let out = post("/mcp/local", list);
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        assert!(
            out.contains("\"grep\"") && !out.contains("\"shell\""),
            "{out}"
        );
        // The dog's own tools stay on /mcp.
        assert!(post("/mcp", list).contains("\"notify\""));
        assert!(!post("/mcp/local", list).contains("\"notify\""));
    }

    #[test]
    fn cors_over_http() {
        use std::io::Write;
        let (m, _rx) = mcp();
        let m = Arc::new(m);
        m.set_cors(&Cors {
            allow_origins: vec!["https://*.example.com".into()],
            allow_private_network: true,
        });
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let (srv, mm) = (Arc::clone(&server), Arc::clone(&m));
        thread::spawn(move || {
            for req in srv.incoming_requests() {
                mm.handle(req);
            }
        });
        let send = |method: &str, origin: &str, extra: &str, body: &str| {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "{method} /mcp HTTP/1.1\r\nHost: x\r\nOrigin: {origin}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out.to_ascii_lowercase()
        };
        let pre = send(
            "OPTIONS",
            "https://app.example.com",
            "Access-Control-Request-Method: POST\r\nAccess-Control-Request-Private-Network: true\r\n",
            "",
        );
        assert!(pre.starts_with("http/1.1 204"), "{pre}");
        assert!(pre.contains("access-control-allow-origin: https://app.example.com"));
        assert!(pre.contains("access-control-allow-private-network: true"));
        assert!(pre.contains("mcp-session-id"));
        let ok = send(
            "POST",
            "https://app.example.com",
            "",
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        );
        assert!(ok.starts_with("http/1.1 200"), "{ok}");
        assert!(ok.contains("access-control-allow-origin: https://app.example.com"));
        let bad = send("POST", "https://evil.test", "", "{}");
        assert!(bad.starts_with("http/1.1 403"), "{bad}");
        server.unblock();
    }
}
