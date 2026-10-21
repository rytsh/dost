//! Command line: `dost notify "Build done"`, `cargo test; dost done`, and hooks for coding
//! agents (`dost hook claude`, `dost hook codex`). Talks to the running dog over MCP.

use std::io::Read;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const USAGE: &str = "\
Dost: the desktop dog. Without arguments it starts the dog.

Usage:
  dost notify <title> [message] [--sticky] [--url URL] [--copy TEXT] [--source NAME]
                      [--mood good|bad|neutral]
  dost ask <question> [--choices Yes,No] [--title T] [--timeout SECS]
                      Prints the answer; exit code 1 if nobody answered.
  dost done [title]   Bark with the last command's result: run it as `cmd; dost done`
                      (reads the exit code from $DOST_STATUS or the --status flag).
  dost run <command...>
                      Runs the command, then barks with how it went and how long it took.
  dost remind <minutes|HH:MM> <message>
  dost react <happy|sad|bark|fetch|sleep|wake|zoomies>
  dost status
  dost hook claude    Claude Code hook (reads the event on stdin); see `dost hook --help`.
  dost hook codex     Codex notify / hook (event as the last argument or on stdin).

Options:
  --url-mcp URL       Dost's MCP address (default $DOST_URL or http://127.0.0.1:7777/mcp)
  --source NAME       Shown above the title (default: Terminal)
";

const HOOK_USAGE: &str = r#"Hooks for coding agents. Dost must be running.

Claude Code (~/.claude/settings.json):
  {
    "hooks": {
      "PermissionRequest": [{ "hooks": [{ "type": "command", "command": "dost hook claude", "timeout": 600 }] }],
      "Notification":      [{ "matcher": "idle_prompt|elicitation_dialog", "hooks": [{ "type": "command", "command": "dost hook claude" }] }],
      "Stop":              [{ "hooks": [{ "type": "command", "command": "dost hook claude" }] }]
    }
  }
  PermissionRequest shows Allow / Always / Deny buttons and answers Claude with the click.

Codex (~/.codex/config.toml):
  notify = ["dost", "hook", "codex"]
  and, for permission buttons, in ~/.codex/hooks.json:
  { "hooks": { "PermissionRequest": [{ "hooks": [{ "type": "command", "command": "dost hook codex", "timeout": 600 }] }] } }
"#;

const DEFAULT_URL: &str = "http://127.0.0.1:7777/mcp";
/// A permission bubble waits this long; after that the agent asks in its own terminal.
const HOOK_ASK_SECS: u64 = 580;

/// Returns `None` when there is no command (start the dog), otherwise runs it.
pub fn run(args: &[String]) -> Option<ExitCode> {
    let first = args.first()?;
    // macOS passes -psn_... when an app is opened from Finder on old systems.
    if first.starts_with("-psn") {
        return None;
    }
    attach_console();
    // `dost run cargo build --release`: everything after `run` is the command.
    let mut o = if first == "run" {
        let (flags, cmd) = split_run(&args[1..]);
        let mut o = Opts::parse(&flags);
        o.pos = cmd;
        o
    } else {
        Opts::parse(&args[1..])
    };
    let code = match first.as_str() {
        "notify" => cmd_notify(&mut o),
        "ask" => cmd_ask(&mut o),
        "done" => cmd_done(&mut o),
        "run" => cmd_run(&mut o),
        "remind" => cmd_remind(&mut o),
        "react" => cmd_react(&mut o),
        "status" => cmd_status(&o),
        "hook" => cmd_hook(&mut o),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "version" | "-V" | "--version" => {
            println!("dost {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    Some(match code {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("dost: {e}");
            ExitCode::FAILURE
        }
    })
}

/// Positional arguments and `--flag value` / `--flag` options.
#[derive(Default, Debug)]
struct Opts {
    pos: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

/// Flags that take no value.
const SWITCHES: &[&str] = &["sticky", "help"];

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut o = Opts::default();
        let mut it = args.iter();
        let mut rest = false;
        while let Some(a) = it.next() {
            if rest {
                o.pos.push(a.clone());
                continue;
            }
            if a == "--" {
                rest = true;
            } else if let Some(name) = a.strip_prefix("--") {
                if let Some((k, v)) = name.split_once('=') {
                    o.flags.push((k.into(), Some(v.into())));
                } else if SWITCHES.contains(&name) {
                    o.flags.push((name.into(), None));
                } else {
                    o.flags.push((name.into(), it.next().cloned()));
                }
            } else {
                o.pos.push(a.clone());
            }
        }
        o
    }

    fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(k, _)| k == name)
    }

    fn url(&self) -> String {
        self.flag("url-mcp")
            .map(String::from)
            .or_else(|| std::env::var("DOST_URL").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| DEFAULT_URL.into())
    }

    fn source(&self, default: &str) -> String {
        self.flag("source").unwrap_or(default).to_string()
    }
}

/// `dost run [--source X] [--] cmd args...`: Dost's own flags come first, then the command.
fn split_run(args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut i = 0;
    let mut flags = Vec::new();
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            i += 1;
            break;
        }
        let Some(name) = a.strip_prefix("--") else {
            break;
        };
        flags.push(a.clone());
        if !name.contains('=') && !SWITCHES.contains(&name) && i + 1 < args.len() {
            flags.push(args[i + 1].clone());
            i += 1;
        }
        i += 1;
    }
    (flags, args[i..].to_vec())
}

/// Calls an MCP tool on the running dog; returns its text, or an error.
fn call(url: &str, tool: &str, args: Value, wait: Duration) -> Result<String, String> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    });
    let agent = crate::watch::http_agent(wait);
    let mut resp = agent
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("cannot reach Dost at {url} (is it running?): {e}"))?;
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("bad reply from Dost: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad reply from Dost: {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(e["message"].as_str().unwrap_or("error").to_string());
    }
    let r = &v["result"];
    let text = r["content"][0]["text"].as_str().unwrap_or("").to_string();
    if r["isError"].as_bool().unwrap_or(false) {
        Err(text)
    } else {
        Ok(text)
    }
}

const SHORT: Duration = Duration::from_secs(5);

fn cmd_notify(o: &mut Opts) -> Result<(), String> {
    let title = o.pos.first().cloned().unwrap_or_default();
    let message = o.pos.get(1..).map(|m| m.join(" ")).unwrap_or_default();
    if title.is_empty() && message.is_empty() {
        return Err("usage: dost notify <title> [message]".into());
    }
    let mut a = json!({
        "title": title,
        "message": message,
        "source": o.source("Terminal"),
        "sticky": o.has("sticky"),
    });
    for k in ["url", "copy", "mood"] {
        if let Some(v) = o.flag(k) {
            a[k] = json!(v);
        }
    }
    call(&o.url(), "notify", a, SHORT).map(|_| ())
}

fn cmd_ask(o: &mut Opts) -> Result<(), String> {
    let question = o.pos.join(" ");
    if question.is_empty() {
        return Err("usage: dost ask <question> [--choices A,B]".into());
    }
    let timeout: u64 = o
        .flag("timeout")
        .and_then(|t| t.parse().ok())
        .unwrap_or(300);
    let choices: Vec<&str> = o
        .flag("choices")
        .unwrap_or("Yes,No")
        .split(',')
        .map(str::trim)
        .collect();
    let a = json!({
        "question": question,
        "title": o.flag("title").unwrap_or(""),
        "choices": choices,
        "timeout_secs": timeout,
        "source": o.source("Terminal"),
    });
    let text = call(&o.url(), "ask", a, Duration::from_secs(timeout + 10))?;
    match text.strip_prefix("The user answered: ") {
        Some(answer) => {
            println!("{answer}");
            Ok(())
        }
        None => Err(String::new()),
    }
}

/// `cmd; dost done`: the shell's `$?` is not visible to us, so it comes from `--status` or
/// `$DOST_STATUS` (`cmd; DOST_STATUS=$? dost done`). Without either it is just "done".
fn cmd_done(o: &mut Opts) -> Result<(), String> {
    let status = o
        .flag("status")
        .map(String::from)
        .or_else(|| std::env::var("DOST_STATUS").ok())
        .and_then(|s| s.trim().parse::<i32>().ok());
    let what = if o.pos.is_empty() {
        "Command".to_string()
    } else {
        o.pos.join(" ")
    };
    let (title, message, mood) = match status {
        Some(0) => (format!("{what} finished"), String::new(), "good"),
        None => (format!("{what} finished"), String::new(), "neutral"),
        Some(c) => (format!("{what} failed"), format!("Exit code {c}."), "bad"),
    };
    let a =
        json!({ "title": title, "message": message, "mood": mood, "source": o.source("Terminal") });
    call(&o.url(), "notify", a, SHORT)?;
    Ok(())
}

fn cmd_run(o: &mut Opts) -> Result<(), String> {
    let Some((prog, args)) = o.pos.split_first() else {
        return Err("usage: dost run <command...>".into());
    };
    let start = Instant::now();
    let status = std::process::Command::new(prog)
        .args(args)
        .status()
        .map_err(|e| format!("{prog}: {e}"))?;
    let took = crate::watch::human_secs(start.elapsed().as_secs());
    let name = std::path::Path::new(prog)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| prog.clone());
    let line: String = std::iter::once(name.as_str())
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    let line: String = line.chars().take(120).collect();
    let (title, msg) = if status.success() {
        (format!("Done: {name}"), format!("{line}\nTook {took}."))
    } else {
        let code = status
            .code()
            .map_or_else(|| "a signal".to_string(), |c| format!("exit code {c}"));
        (
            format!("Failed: {name}"),
            format!("{line}\n{code} after {took}."),
        )
    };
    let mood = if status.success() { "good" } else { "bad" };
    let a = json!({ "title": title, "message": msg, "mood": mood, "source": o.source("Terminal") });
    if let Err(e) = call(&o.url(), "notify", a, SHORT) {
        eprintln!("dost: {e}");
    }
    // Pass the command's exit code on, so `dost run make && deploy` still works.
    if status.success() {
        Ok(())
    } else {
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn cmd_remind(o: &mut Opts) -> Result<(), String> {
    let (Some(when), Some(_)) = (o.pos.first(), o.pos.get(1)) else {
        return Err("usage: dost remind <minutes|HH:MM> <message>".into());
    };
    let message = o.pos[1..].join(" ");
    let mut a = json!({ "message": message, "source": o.source("Terminal") });
    if when.contains(':') {
        a["at"] = json!(when);
    } else {
        let m: f64 = when
            .parse()
            .map_err(|_| format!("`{when}` is not minutes or HH:MM"))?;
        a["after_minutes"] = json!(m);
    }
    let text = call(&o.url(), "remind", a, SHORT)?;
    println!("{text}");
    Ok(())
}

fn cmd_react(o: &mut Opts) -> Result<(), String> {
    let r = o.pos.first().ok_or("usage: dost react <reaction>")?;
    call(&o.url(), "react", json!({ "reaction": r }), SHORT).map(|_| ())
}

fn cmd_status(o: &Opts) -> Result<(), String> {
    let text = call(&o.url(), "status", json!({}), SHORT)?;
    println!("{text}");
    Ok(())
}

fn cmd_hook(o: &mut Opts) -> Result<(), String> {
    let agent = o.pos.first().cloned().unwrap_or_default();
    if o.has("help") || agent.is_empty() {
        print!("{HOOK_USAGE}");
        return Ok(());
    }
    let source = match agent.as_str() {
        "claude" => "Claude Code",
        "codex" => "Codex",
        other => return Err(format!("unknown agent `{other}` (claude, codex)")),
    };
    // The event: Codex `notify` passes it as the last argument, hooks send it on stdin.
    let event: Result<Value, String> = match o.pos.get(1) {
        Some(arg) => serde_json::from_str(arg).map_err(|e| format!("bad event JSON: {e}")),
        None => {
            let mut s = String::new();
            let _ = std::io::stdin().read_to_string(&mut s);
            serde_json::from_str(&s).map_err(|e| format!("bad event JSON on stdin: {e}"))
        }
    };
    let out = event.and_then(|ev| hook(&o.url(), source, &ev));
    match out {
        Ok(Some(reply)) => {
            println!("{reply}");
            Ok(())
        }
        Ok(None) => Ok(()),
        // A hook must never break the agent: report, but exit 0 with no decision.
        Err(e) => {
            eprintln!("dost: {e}");
            Ok(())
        }
    }
}

/// Handles one agent event; returns what to print on stdout (a hook decision), if anything.
fn hook(url: &str, source: &str, ev: &Value) -> Result<Option<String>, String> {
    let s = |k: &str| ev.get(k).and_then(Value::as_str).unwrap_or("");
    let project = std::path::Path::new(s("cwd"))
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let in_project = |t: &str| {
        if project.is_empty() {
            t.to_string()
        } else {
            format!("{t} · {project}")
        }
    };
    let event = if ev.get("type").is_some() {
        s("type").to_string()
    } else {
        s("hook_event_name").to_string()
    };
    match event.as_str() {
        "PermissionRequest" => {
            let tool = s("tool_name");
            let input = &ev["tool_input"];
            let what = input
                .get("description")
                .or_else(|| input.get("command"))
                .or_else(|| input.get("file_path"))
                .and_then(Value::as_str)
                .map(short)
                .unwrap_or_else(|| short(&input.to_string()));
            let key = format!("{source}:{}:{tool}:{what}", s("session_id"));
            let choices: &[&str] = if source == "Codex" {
                &["Allow", "Deny"]
            } else {
                &["Allow", "Always", "Deny"]
            };
            let a = json!({
                "title": in_project(&format!("Permission: {tool}")),
                "question": what,
                "choices": choices,
                "timeout_secs": HOOK_ASK_SECS,
                "source": source,
                "key": key,
            });
            let answer = match call(url, "ask", a, Duration::from_secs(HOOK_ASK_SECS + 10)) {
                Ok(t) => t.strip_prefix("The user answered: ").map(String::from),
                // No answer: leave it to the agent's own prompt.
                Err(_) => None,
            };
            Ok(answer.map(|a| permission_reply(source, &a, ev)))
        }
        "Notification" => {
            let kind = s("notification_type");
            // The permission bubble is already up (PermissionRequest).
            if kind == "permission_prompt" {
                return Ok(None);
            }
            let title = match kind {
                "idle_prompt" => "Waiting for you".to_string(),
                _ if !s("title").is_empty() => s("title").to_string(),
                _ => "Needs your attention".to_string(),
            };
            let a = json!({
                "title": in_project(&title),
                "message": short(s("message")),
                "mood": "neutral",
                "source": source,
            });
            call(url, "notify", a, SHORT).map(|_| None)
        }
        "Stop" | "agent-turn-complete" => {
            if ev
                .get("stop_hook_active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(None);
            }
            let last = ev
                .get("last_assistant_message")
                .or_else(|| ev.get("last-assistant-message"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let a = json!({
                "title": in_project("Your turn"),
                "message": short(last),
                "mood": "neutral",
                "source": source,
            });
            call(url, "notify", a, SHORT)?;
            // A Codex Stop hook must print JSON.
            Ok((source == "Codex" && event == "Stop").then(|| "{}".to_string()))
        }
        "SubagentStop" => Ok(None),
        "SessionEnd" | "SessionStart" => Ok(None),
        _ => Ok(None),
    }
}

/// The hook output that answers a permission request.
fn permission_reply(source: &str, answer: &str, ev: &Value) -> String {
    let decision = match answer {
        "Allow" => json!({ "behavior": "allow" }),
        "Always" if source != "Codex" => {
            let mut d = json!({ "behavior": "allow" });
            // Claude's own suggestion for "don't ask again", when it gave one.
            if let Some(s) = ev
                .get("permission_suggestions")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty())
            {
                d["updatedPermissions"] = Value::Array(s.clone());
            }
            d
        }
        _ => json!({ "behavior": "deny", "message": "Denied from Dost." }),
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": decision,
        }
    })
    .to_string()
}

/// One line, at most 140 characters.
fn short(t: &str) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 140 {
        format!("{}…", t.chars().take(139).collect::<String>())
    } else {
        t
    }
}

/// The Windows release build has no console of its own: use the one it was started from.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(windows))]
fn attach_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(a: &[&str]) -> Opts {
        Opts::parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_flags_and_words() {
        let o = opts(&[
            "Build",
            "done",
            "--sticky",
            "--url",
            "https://x",
            "--source=CI",
        ]);
        assert_eq!(o.pos, ["Build", "done"]);
        assert!(o.has("sticky"));
        assert_eq!(o.flag("url"), Some("https://x"));
        assert_eq!(o.source("Terminal"), "CI");
    }

    #[test]
    fn claude_permission_answers() {
        let ev = json!({
            "hook_event_name": "PermissionRequest",
            "permission_suggestions": [{ "type": "addRules", "behavior": "allow" }],
        });
        let allow: Value =
            serde_json::from_str(&permission_reply("Claude Code", "Allow", &ev)).unwrap();
        assert_eq!(allow["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert_eq!(
            allow["hookSpecificOutput"]["hookEventName"],
            "PermissionRequest"
        );
        let always: Value =
            serde_json::from_str(&permission_reply("Claude Code", "Always", &ev)).unwrap();
        assert_eq!(
            always["hookSpecificOutput"]["decision"]["updatedPermissions"][0]["type"],
            "addRules"
        );
        let deny: Value = serde_json::from_str(&permission_reply("Codex", "Deny", &ev)).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(
            deny["hookSpecificOutput"]["decision"]
                .get("updatedPermissions")
                .is_none()
        );
    }

    #[test]
    fn run_keeps_the_commands_flags() {
        let a: Vec<String> = ["--source", "CI", "cargo", "build", "--release"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (flags, cmd) = split_run(&a);
        assert_eq!(flags, ["--source", "CI"]);
        assert_eq!(cmd, ["cargo", "build", "--release"]);
    }

    #[test]
    fn short_text() {
        assert_eq!(short("  a\n b  "), "a b");
        assert_eq!(short(&"x".repeat(200)).chars().count(), 140);
    }
}
