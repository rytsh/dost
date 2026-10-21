//! MCP server (Streamable HTTP, JSON responses): AI assistants send notifications to the dog
//! and add status checks. Default address is `http://127.0.0.1:7777/mcp`.

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};
use tiny_http::{Header, Method, Response, Server};

use crate::notify::{Notification, Notifier};
use crate::watch::{Check, DEFAULT_INTERVAL, MIN_INTERVAL, NewWatch, Watches};

const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_BODY: u64 = 1 << 20;

/// The dog's current state, for the `status` tool's reply.
#[derive(Clone, Debug, Default)]
pub struct DogStatus {
    pub activity: String,
    pub visible: bool,
    pub showing: Option<String>,
    pub queued: usize,
}

pub struct Mcp {
    notifier: Notifier,
    watches: Arc<Watches>,
    status: Arc<Mutex<DogStatus>>,
}

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
    pub fn new(notifier: Notifier, watches: Arc<Watches>, status: Arc<Mutex<DogStatus>>) -> Self {
        Mcp {
            notifier,
            watches,
            status,
        }
    }

    fn handle(&self, mut req: tiny_http::Request) {
        let path = req.url().split('?').next().unwrap_or("").to_string();
        if path != "/mcp" && path != "/" {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
            return;
        }
        // Block web pages in a browser from sending requests to localhost (DNS rebinding).
        let origin = header(&req, "Origin");
        if origin.as_deref().is_some_and(|o| !is_local_origin(o)) {
            let _ = req.respond(Response::from_string("forbidden origin").with_status_code(403));
            return;
        }
        match req.method() {
            Method::Post => {}
            Method::Get | Method::Delete => {
                // No server-to-client streaming (SSE) and no sessions.
                let _ = req.respond(Response::empty(405).with_header(hdr("Allow", "POST")));
                return;
            }
            _ => {
                let _ = req.respond(Response::empty(405));
                return;
            }
        }
        let mut body = String::new();
        if req
            .as_reader()
            .take(MAX_BODY)
            .read_to_string(&mut body)
            .is_err()
        {
            let _ = req.respond(json_response(
                400,
                &rpc_error(Value::Null, -32700, "parse error"),
            ));
            return;
        }
        let reply = match serde_json::from_str::<Value>(&body) {
            Ok(Value::Array(batch)) => {
                let out: Vec<Value> = batch.into_iter().filter_map(|m| self.dispatch(m)).collect();
                (!out.is_empty()).then_some(Value::Array(out))
            }
            Ok(msg) => self.dispatch(msg),
            Err(_) => Some(rpc_error(Value::Null, -32700, "parse error")),
        };
        let _ = match reply {
            Some(v) => req.respond(json_response(200, &v)),
            None => req.respond(Response::empty(202)),
        };
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
            "tools/list" => Ok(json!({ "tools": tools() })),
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
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "dost", "title": "Dost (desktop dog)", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "A dog on the user's screen. Use `notify` to tell the user something; \
                use `add_watch` to get notified when something happens (a site comes up, a build finishes, \
                a file appears...). On a notification the dog barks and shows the message in a speech bubble.",
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
            "add_watch" => self.tool_add_watch(&args),
            "list_watches" => Ok(self.tool_list_watches()),
            "remove_watch" => self.tool_remove_watch(&args),
            "check_now" => self.tool_check_now(&args),
            "status" => Ok(self.tool_status()),
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
        let mut n = Notification::new(title, message);
        if a.get("sticky").and_then(Value::as_bool).unwrap_or(false) {
            n = n.sticky();
        } else if let Some(s) = a.get("timeout_secs").and_then(Value::as_u64) {
            n = n.timeout(std::time::Duration::from_secs(s.clamp(2, 3600)));
        }
        if !self.notifier.send(n) {
            return Err("the dog is not running".into());
        }
        Ok("Notification sent; the dog is barking.".into())
    }

    fn tool_add_watch(&self, a: &Value) -> Result<String, String> {
        let check = parse_check(a)?;
        let name = str_arg(a, "name").unwrap_or_else(|| match &check {
            Check::Http { url, .. } => url.clone(),
            Check::Command { command, .. } => command.clone(),
        });
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
            "activity": s.activity,
            "visible": s.visible,
            "showing": s.showing,
            "queued": s.queued,
            "watches": self.watches.list().len(),
        });
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }
}

fn parse_check(a: &Value) -> Result<Check, String> {
    let contains = str_arg(a, "contains");
    match (str_arg(a, "url"), str_arg(a, "command")) {
        (Some(url), None) => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("`url` must start with http:// or https://".into());
            }
            let status = a.get("status").and_then(Value::as_u64).map(|s| s as u16);
            Ok(Check::Http {
                url,
                status,
                contains,
            })
        }
        (None, Some(command)) => Ok(Check::Command { command, contains }),
        _ => Err("exactly one of `url` or `command` is required".into()),
    }
}

fn tools() -> Value {
    let check_props = json!({
        "url": { "type": "string", "description": "HTTP(S) check: sends a GET request to this URL." },
        "status": { "type": "integer", "description": "Expected status code for url (default: any 2xx)." },
        "command": { "type": "string", "description": "Command check: runs in a shell on the user's computer (cmd /C on Windows). The condition is met if the exit code is 0." },
        "contains": { "type": "string", "description": "If given, the condition also requires the response body / command output to contain this text." },
        "negate": { "type": "boolean", "description": "Invert the condition (e.g. notify when a site goes DOWN)." },
    });
    let mut watch_props = check_props.clone();
    for (k, v) in json!({
        "name": { "type": "string", "description": "Short name shown in the list." },
        "title": { "type": "string", "description": "Title shown in the bubble when triggered." },
        "message": { "type": "string", "description": "Message shown in the bubble when triggered." },
        "interval_secs": { "type": "integer", "minimum": MIN_INTERVAL, "description": format!("Check interval in seconds (default {DEFAULT_INTERVAL}).") },
        "once": { "type": "boolean", "description": "true (default): bark on the first trigger and remove the watch. false: bark every time the condition goes from false to true." },
        "sticky": { "type": "boolean", "description": "Keep the bubble until the user clicks it." },
    })
    .as_object()
    .unwrap()
    {
        watch_props[k] = v.clone();
    }
    json!([
        {
            "name": "notify",
            "title": "Notify via the dog",
            "description": "The dog on the user's screen barks and shows the message in a speech bubble. Use it when a long task finishes or when you need the user's attention.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short title." },
                    "message": { "type": "string", "description": "Message (a few sentences)." },
                    "sticky": { "type": "boolean", "description": "Keep it until the user clicks it." },
                    "timeout_secs": { "type": "integer", "description": "How long the bubble stays, in seconds (default 10)." }
                }
            }
        },
        {
            "name": "add_watch",
            "title": "Add a status watch",
            "description": "The dog checks a condition at regular intervals; when it is met, the dog barks and notifies the user. Examples: 'when the site is up' (url), 'when the build finishes' (command + contains), 'when the site goes down' (url + negate). One of `url` or `command` is required. Watches persist across dog restarts.",
            "inputSchema": { "type": "object", "properties": watch_props }
        },
        {
            "name": "list_watches",
            "title": "List watches",
            "description": "Lists saved status watches with their latest results and ids.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "remove_watch",
            "title": "Remove a watch",
            "description": "Removes a status watch by id.",
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
            "title": "Dog status",
            "description": "Returns what the dog is doing, the notification on screen, and the number of queued notifications and watches.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }
    ])
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify;

    fn mcp() -> (Mcp, std::sync::mpsc::Receiver<Notification>) {
        let (tx, rx) = notify::channel();
        let watches = Watches::new(tx.clone(), None);
        (Mcp::new(tx, watches, Arc::default()), rx)
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
                "add_watch",
                "list_watches",
                "remove_watch",
                "check_now",
                "status"
            ]
        );
        let r = m
            .dispatch(json!({ "jsonrpc": "2.0", "id": 3, "method": "nope" }))
            .unwrap();
        assert_eq!(r["error"]["code"], -32601);
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
        let n = rx.try_recv().unwrap();
        assert_eq!(
            (n.title.as_str(), n.body.as_str(), n.timeout),
            ("Build", "done", crate::notify::Timeout::Sticky)
        );
        assert_eq!(call(&m, "notify", json!({}))["isError"], true);
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
    fn origins() {
        assert!(is_local_origin("http://localhost:5173"));
        assert!(is_local_origin("http://127.0.0.1"));
        assert!(!is_local_origin("https://evil.example"));
        assert!(!is_local_origin("http://localhost.evil.example"));
    }
}
