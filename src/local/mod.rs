//! Workspace tools for agents, at `/mcp/local` (off by default; settings → Connect): read,
//! glob, grep, edit, write, patch, shell and shell_job over the user's chosen folders. Adapted
//! from local-mcp (github.com/rakunlabs/local-mcp), which adapts OpenCode's built-in tools.
//!
//! Paths are confined to the root, the extra allowed folders and the system's temp folder;
//! `shell` runs with the user's full authority.

mod files;
mod output;
mod patch;
mod read;
mod search;
mod shell;
mod workspace;

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use workspace::Workspace;

/// The tools' names, in the order `tools/list` gives them.
pub const TOOLS: [&str; 8] = [
    "read",
    "glob",
    "grep",
    "edit",
    "write",
    "patch",
    "shell",
    "shell_job",
];
/// The tools that only read.
pub const READ_TOOLS: [&str; 3] = ["read", "glob", "grep"];

/// Which calls the dog asks the user about first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approve {
    /// Commands and every change to a file.
    #[default]
    Changes,
    /// Only commands (`shell`).
    Commands,
    Never,
}

impl Approve {
    pub const ALL: [(Approve, &str); 3] = [
        (Approve::Changes, "Changes"),
        (Approve::Commands, "Commands"),
        (Approve::Never, "Never"),
    ];
}

/// The choices of the commands' time limit (seconds; 0 is none).
pub const TIME_LIMITS: [(u64, &str); 5] = [
    (60, "1 min"),
    (120, "2 min"),
    (600, "10 min"),
    (1800, "30 min"),
    (0, "None"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSettings {
    /// Serve the tools at `/mcp/local`.
    pub enabled: bool,
    /// The workspace root; empty is the home folder.
    pub root: String,
    /// More folders the tools may read and write (the temp folder always may).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_dirs: Vec<String>,
    /// Only read, glob and grep.
    pub read_only: bool,
    /// Tools left out (names from [`TOOLS`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
    /// What the dog asks about before it happens.
    pub approve: Approve,
    /// Allowed for good ("Always"): `shell:<command prefix>*` or `files:<folder>`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub always_allow: Vec<String>,
    /// Commands stop after this long (seconds; 0 is no limit). It is also the default.
    pub time_limit_secs: u64,
}

impl Default for LocalSettings {
    fn default() -> Self {
        LocalSettings {
            enabled: false,
            root: String::new(),
            allowed_dirs: Vec::new(),
            read_only: false,
            disabled_tools: Vec::new(),
            approve: Approve::Changes,
            always_allow: Vec::new(),
            time_limit_secs: 120,
        }
    }
}

/// A call the user has to allow first.
#[derive(Clone, Debug, PartialEq)]
pub struct Approval {
    /// "Run a command?", "Change a file?"
    pub title: String,
    /// How many files change (0 for a command).
    pub files: usize,
    /// The command, or the files.
    pub detail: String,
    /// The rule "Always" adds (see [`LocalSettings::always_allow`]).
    pub rule: String,
}

impl Approval {
    /// The rule in words, for the settings list.
    pub fn describe(rule: &str) -> String {
        match rule.split_once(':') {
            Some(("shell", p)) => format!("$ {p}"),
            Some(("files", p)) => format!("{p}/…"),
            _ => rule.to_string(),
        }
    }
}

impl LocalSettings {
    /// The root folder in use: the setting with `~` expanded, or the home folder.
    pub fn root_dir(&self) -> PathBuf {
        let r = self.root.trim();
        if r.is_empty() {
            return home();
        }
        expand(r)
    }

    /// Whether tool `name` is offered.
    pub fn offers(&self, name: &str) -> bool {
        TOOLS.contains(&name)
            && !self.disabled_tools.iter().any(|d| d == name)
            && (!self.read_only || READ_TOOLS.contains(&name))
    }
}

/// `~` and `~/…` expanded to the home folder.
pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        return home();
    }
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        return home().join(rest);
    }
    PathBuf::from(p)
}

fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The tools, set up for one configuration. Cheap to share; background shell jobs live in it.
pub struct Local {
    settings: std::sync::Mutex<LocalSettings>,
    store: output::Store,
    jobs: shell::Jobs,
}

impl Local {
    pub fn new(s: LocalSettings) -> Self {
        Local {
            settings: std::sync::Mutex::new(s),
            store: output::Store::new(
                output::Store::default_dir(),
                output::MAX_LINES,
                output::MAX_BYTES,
            ),
            jobs: shell::Jobs::default(),
        }
    }

    /// The settings changed (root, allowed folders, which tools).
    pub fn configure(&self, s: &LocalSettings) {
        if let Ok(mut c) = self.settings.lock() {
            *c = s.clone();
        }
    }

    pub fn settings(&self) -> LocalSettings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// The MCP `tools/list` entries of the offered tools.
    pub fn tools(&self) -> Vec<Value> {
        let s = self.settings();
        TOOLS
            .iter()
            .filter(|n| offered(&s, n))
            .map(|n| definition(n, s.time_limit_secs))
            .collect()
    }

    /// Calls tool `name`: the MCP `content` array, or an error message for the agent.
    pub fn call(&self, name: &str, args: &Value) -> Result<Vec<Value>, String> {
        self.call_live(name, args, None)
    }

    /// [`Local::call`], with a command's output sent to `live` while it runs.
    pub fn call_live(
        &self,
        name: &str,
        args: &Value,
        live: Option<&mut dyn FnMut(String)>,
    ) -> Result<Vec<Value>, String> {
        let s = self.settings();
        if !offered(&s, name) {
            return Err(format!("unknown tool: {name}"));
        }
        let ws = self.workspace(&s)?;
        let text = |r: Result<String, String>| r.map(|t| vec![read::text(t)]);
        match name {
            "read" => read::read(
                &ws,
                &path_arg(args),
                int_arg(args, "offset"),
                int_arg(args, "limit"),
            ),
            "glob" => text(self.glob(&ws, args)),
            "grep" => text(self.grep(&ws, args)),
            "edit" => text(files::edit(
                &ws,
                &path_arg(args),
                &str_arg(args, "oldString"),
                &str_arg(args, "newString"),
                bool_arg(args, "replaceAll").unwrap_or(false),
            )),
            "write" => text(files::write(
                &ws,
                &path_arg(args),
                &str_arg(args, "content"),
            )),
            "patch" => text(files::apply_patch(&ws, &str_arg(args, "patchText"))),
            "shell" => text(self.shell(&ws, args, live)),
            "shell_job" => text(self.shell_job(args)),
            _ => Err(format!("unknown tool: {name}")),
        }
    }

    /// What the user has to allow before `name` runs with `args`, if anything.
    pub fn approval(&self, name: &str, args: &Value) -> Option<Approval> {
        let s = self.settings();
        let ask = match s.approve {
            Approve::Never => false,
            Approve::Commands => name == "shell",
            Approve::Changes => matches!(name, "shell" | "edit" | "write" | "patch"),
        };
        if !ask || !offered(&s, name) {
            return None;
        }
        if name == "shell" {
            let command = str_arg(args, "command");
            let command = command.trim();
            if s.always_allow.iter().any(|r| shell_rule_allows(r, command)) {
                return None;
            }
            return Some(Approval {
                title: "Run a command?".into(),
                files: 0,
                detail: command.to_string(),
                rule: format!("shell:{}*", command_prefix(command)),
            });
        }
        let ws = self.workspace(&s).ok()?;
        let files: Vec<PathBuf> = match name {
            "patch" => patch_paths(&str_arg(args, "patchText"))
                .iter()
                .filter_map(|p| ws.resolve(p).ok())
                .collect(),
            _ => ws.resolve(&path_arg(args)).ok().into_iter().collect(),
        };
        // Bad paths fail in the tool itself, without asking.
        if files.is_empty() {
            return None;
        }
        let allowed = |f: &PathBuf| {
            s.always_allow.iter().any(|r| {
                r.strip_prefix("files:")
                    .is_some_and(|d| f.starts_with(std::path::Path::new(d)))
            })
        };
        if files.iter().all(allowed) {
            return None;
        }
        let dir = files[0]
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let detail = files
            .iter()
            .map(|f| f.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        Some(Approval {
            title: if files.len() == 1 {
                "Change a file?".into()
            } else {
                format!("Change {} files?", files.len())
            },
            files: files.len(),
            detail,
            rule: format!("files:{dir}"),
        })
    }

    /// What a call does, in the same words as an approval (for History).
    pub fn describe_call(&self, name: &str, args: &Value) -> Approval {
        let s = LocalSettings {
            approve: Approve::Changes,
            always_allow: Vec::new(),
            ..self.settings()
        };
        let probe = Local {
            settings: std::sync::Mutex::new(s),
            store: output::Store::new(self.store.dir().to_path_buf(), 1, 1),
            jobs: shell::Jobs::default(),
        };
        probe.approval(name, args).unwrap_or(Approval {
            title: name.into(),
            files: 0,
            detail: name.into(),
            rule: String::new(),
        })
    }

    /// Adds an "Always" rule (until the settings are saved over it).
    pub fn allow_always(&self, rule: &str) {
        if let Ok(mut s) = self.settings.lock()
            && !s.always_allow.iter().any(|r| r == rule)
        {
            s.always_allow.push(rule.to_string());
        }
    }

    /// Server instructions for `initialize`.
    pub fn instructions(&self) -> String {
        let s = self.settings();
        let mut dirs: Vec<String> = s
            .allowed_dirs
            .iter()
            .filter(|d| !d.trim().is_empty())
            .map(|d| expand(d.trim()).display().to_string())
            .collect();
        dirs.extend(
            workspace::temp_dirs()
                .iter()
                .map(|d| d.display().to_string()),
        );
        format!(
            "Local workspace tools. Workspace root: {}. Relative paths resolve from it. \
             Paths may also be in: {}.",
            s.root_dir().display(),
            dirs.join(", ")
        )
    }

    /// Where paths may go: the root, the allowed folders, the temp folders and the output store.
    fn workspace(&self, s: &LocalSettings) -> Result<Workspace, String> {
        let mut allowed: Vec<PathBuf> = s
            .allowed_dirs
            .iter()
            .map(|d| d.trim())
            .filter(|d| !d.is_empty())
            .map(expand)
            .collect();
        allowed.extend(workspace::temp_dirs());
        Ok(Workspace::new(&s.root_dir(), &allowed)?.with(self.store.dir()))
    }

    fn glob(&self, ws: &Workspace, args: &Value) -> Result<String, String> {
        let pattern = str_arg(args, "pattern");
        if pattern.trim().is_empty() {
            return Err("pattern is required".into());
        }
        let limit = limit_arg(args);
        let path = str_arg(args, "path");
        let dir = ws.resolve(&path)?;
        if !dir.is_dir() {
            return Err(format!("not a directory: {path}"));
        }
        let mut found = search::Engine::auto()
            .glob(&search::GlobInput {
                dir: &dir,
                pattern: &pattern,
                hidden: bool_arg(args, "hidden").unwrap_or(false),
                limit,
            })
            .map_err(|e| format!("unable to find files matching {pattern}; {e}"))?;
        if found.is_empty() {
            return Ok("No files found".into());
        }
        let truncated = found.len() > limit;
        found.truncate(limit);
        let mut out = found
            .iter()
            .map(|f| ws.display(&dir.join(f)))
            .collect::<Vec<_>>()
            .join("\n");
        if truncated {
            out += &format!(
                "\n\n(Results are truncated at {limit} files. Use a more specific path or pattern, or raise limit.)"
            );
        }
        Ok(out)
    }

    fn grep(&self, ws: &Workspace, args: &Value) -> Result<String, String> {
        let pattern = str_arg(args, "pattern");
        if pattern.is_empty() {
            return Err("pattern is required".into());
        }
        let limit = limit_arg(args);
        let path = str_arg(args, "path");
        let target = ws.resolve(&path)?;
        let meta = std::fs::metadata(&target).map_err(|_| format!("path not found: {path}"))?;
        let (dir, file) = if meta.is_dir() {
            (target.clone(), String::new())
        } else {
            let parent = target.parent().map(PathBuf::from).unwrap_or_default();
            let name = target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (parent, name)
        };
        let mut matches = search::Engine::auto()
            .grep(&search::GrepInput {
                dir: &dir,
                file: &file,
                pattern: &pattern,
                literal: bool_arg(args, "literal").unwrap_or(false),
                case_sensitive: bool_arg(args, "caseSensitive").unwrap_or(true),
                include: &str_arg(args, "include"),
                limit,
            })
            .map_err(|e| format!("unable to grep for {pattern}; {e}"))?;
        if matches.is_empty() {
            return Ok("No matches found".into());
        }
        let truncated = matches.len() > limit;
        matches.truncate(limit);
        let mut b = format!("Found {} matches", matches.len());
        if truncated {
            b += &format!(" (truncated at limit {limit})");
        }
        let mut current = "";
        for m in &matches {
            if m.path != current {
                current = &m.path;
                b += &format!("\n\n{}:", ws.display(&dir.join(&m.path)));
            }
            let line = match m.text.char_indices().nth(GREP_MAX_LINE) {
                Some((cut, _)) => format!("{}...", &m.text[..cut]),
                None => m.text.clone(),
            };
            b += &format!("\n  Line {}: {line}", m.line);
        }
        Ok(self.store.truncate(&b, output::Direction::Head))
    }

    fn shell(
        &self,
        ws: &Workspace,
        args: &Value,
        live: Option<&mut dyn FnMut(String)>,
    ) -> Result<String, String> {
        let command = str_arg(args, "command");
        if command.trim().is_empty() {
            return Err("command is required".into());
        }
        let timeout = int_arg(args, "timeout");
        if timeout < 0 {
            return Err(format!(
                "invalid timeout {timeout}; must be positive milliseconds"
            ));
        }
        let workdir = str_arg(args, "workdir");
        let dir = ws.resolve(&workdir)?;
        if !dir.is_dir() {
            return Err(format!("working directory is not a directory: {workdir}"));
        }
        // The setting is the default and the most a command may ask for (0: no limit).
        let limit_ms = self.settings().time_limit_secs * 1000;
        let capped = |t: u64| match (t, limit_ms) {
            (0, l) => l,
            (t, 0) => t,
            (t, l) => t.min(l),
        };
        if bool_arg(args, "background").unwrap_or(false) {
            let job = self.jobs.start(
                &self.store,
                &command,
                &dir,
                Duration::from_millis(capped(timeout as u64)),
            )?;
            return Ok(format!(
                "Started background job {} (pid {}).\nOutput file: {}\n\
                 Use shell_job with action status, wait or kill and id {:?}.",
                job.id,
                job.pid,
                job.output.display(),
                job.id
            ));
        }
        let timeout = match capped(timeout as u64) {
            0 => u64::MAX / 4,
            t => t,
        };
        let outcome = shell::run_foreground(
            &self.store,
            &command,
            &dir,
            Duration::from_millis(timeout),
            live,
        )?;
        let (body, meta, ok) = match outcome {
            shell::Outcome::Exited(code, body) => (body, format!("Exit code: {code}"), code == 0),
            shell::Outcome::TimedOut(body) => (
                body,
                format!(
                    "Command was terminated after exceeding the timeout of {timeout} ms. \
                     If it is expected to take longer and is not waiting for input, retry with \
                     a larger timeout or background: true."
                ),
                false,
            ),
        };
        let mut body = body.trim_end_matches('\n').to_string();
        if body.trim().is_empty() {
            body = "(no output)".into();
        }
        let out = format!("{body}\n\n<shell_metadata>\n{meta}\n</shell_metadata>");
        if ok { Ok(out) } else { Err(out) }
    }

    fn shell_job(&self, args: &Value) -> Result<String, String> {
        let action = str_arg(args, "action");
        if action == "list" {
            let jobs = self.jobs.list();
            if jobs.is_empty() {
                return Ok("No background jobs".into());
            }
            return Ok(jobs
                .iter()
                .map(|j| j.describe())
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let id = str_arg(args, "id");
        if id.is_empty() {
            return Err("id is required".into());
        }
        let job = self.jobs.get(&id)?;
        match action.as_str() {
            "status" => {}
            "wait" => {
                let ms = match int_arg(args, "timeout") {
                    t if t <= 0 => 30_000,
                    t => (t as u64).min(600_000),
                };
                self.jobs.wait(&job, Duration::from_millis(ms));
            }
            "kill" => self.jobs.kill(&job),
            _ => {
                return Err(format!(
                    "unknown action {action:?}; use list, status, wait or kill"
                ));
            }
        }
        let body = self
            .store
            .tail_file(&job.output, true)
            .map_err(|e| format!("read job output; {e}"))?;
        let body = if body.trim().is_empty() {
            "(no output yet)".to_string()
        } else {
            body
        };
        Ok(format!("{}\n\n{body}", job.describe()))
    }
}

const SEARCH_DEFAULT_LIMIT: usize = 100;
const GREP_MAX_LINE: usize = 2000;

/// Programs whose first argument is a subcommand: an "Always" for `cargo test` must not also
/// allow `cargo install`.
const WITH_SUBCOMMANDS: [&str; 16] = [
    "cargo", "npm", "pnpm", "yarn", "bun", "npx", "git", "go", "docker", "kubectl", "make", "just",
    "uv", "pip", "brew", "gh",
];

/// The start of a command an "Always" rule keeps: the program, plus its subcommand for the
/// programs that have them.
fn command_prefix(command: &str) -> String {
    let mut words = command.split_whitespace();
    let first = words.next().unwrap_or_default();
    match words.next() {
        Some(w) if WITH_SUBCOMMANDS.contains(&first) && !w.starts_with('-') => {
            format!("{first} {w}")
        }
        _ => first.to_string(),
    }
}

/// Whether rule `shell:<prefix>*` allows `command`. A command that chains others (`;`, `&&`,
/// `|`, `` ` ``, `$(`) is never allowed by a rule: the prefix says nothing about the rest.
fn shell_rule_allows(rule: &str, command: &str) -> bool {
    let Some(prefix) = rule
        .strip_prefix("shell:")
        .and_then(|r| r.strip_suffix('*'))
    else {
        return false;
    };
    let chained = ["&&", "||", ";", "|", "`", "$(", "\n", ">", "<", "&"]
        .iter()
        .any(|c| command.contains(c));
    !chained
        && command.starts_with(prefix)
        && command[prefix.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
}

/// The files a patch touches (added, updated, deleted, and where they move to).
fn patch_paths(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            [
                "*** Add File:",
                "*** Update File:",
                "*** Delete File:",
                "*** Move to:",
            ]
            .iter()
            .find_map(|p| l.strip_prefix(p))
            .map(|p| p.trim().to_string())
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// Whether tool `name` is listed: as [`LocalSettings::offers`], and `shell_job` only with
/// `shell` (a job manager is useless without the tool that starts jobs).
fn offered(s: &LocalSettings, name: &str) -> bool {
    s.offers(name) && (name != "shell_job" || s.offers("shell"))
}

fn str_arg(a: &Value, key: &str) -> String {
    a.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `path` argument (`filePath` is accepted too, as OpenCode's tools name it).
fn path_arg(a: &Value) -> String {
    match a.get("path").and_then(Value::as_str) {
        Some(p) => p.to_string(),
        None => str_arg(a, "filePath"),
    }
}

fn int_arg(a: &Value, key: &str) -> i64 {
    a.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(0)
}

fn bool_arg(a: &Value, key: &str) -> Option<bool> {
    a.get(key).and_then(Value::as_bool)
}

fn limit_arg(a: &Value) -> usize {
    match int_arg(a, "limit") {
        l if l <= 0 => SEARCH_DEFAULT_LIMIT,
        l => l as usize,
    }
}

/// The MCP tool definition of `name`.
fn definition(name: &str, limit_secs: u64) -> Value {
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    let edits = json!({ "destructiveHint": true, "openWorldHint": false });
    let (description, properties, required, annotations) = match name {
        "read" => (
            READ_DESCRIPTION.to_string(),
            json!({
                "path": string("File or directory to read. Relative paths resolve from the workspace root."),
                "offset": integer("The 1-based line or directory entry to start reading from"),
                "limit": integer("The maximum number of lines or directory entries to read (defaults to and capped at 2000)"),
            }),
            vec!["path"],
            read_only,
        ),
        "glob" => (
            GLOB_DESCRIPTION.to_string(),
            json!({
                "pattern": string("Glob pattern to match files against, e.g. **/*.go or src/**/*.ts"),
                "path": string("Directory to search. Defaults to the workspace root."),
                "hidden": boolean("Include hidden files and directories (default false)"),
                "limit": integer("Maximum number of matching files to return (default 100)"),
            }),
            vec!["pattern"],
            read_only,
        ),
        "grep" => (
            GREP_DESCRIPTION.to_string(),
            json!({
                "pattern": string("Regular expression (RE2 / ripgrep syntax) or literal text to match in file contents"),
                "path": string("File or directory to search. Defaults to the workspace root."),
                "include": string("Glob pattern to filter files, e.g. *.go or *.{ts,tsx}"),
                "literal": boolean("Treat pattern as exact text instead of a regular expression (default false)"),
                "caseSensitive": boolean("Use case-sensitive matching (default true)"),
                "limit": integer("Maximum number of matching lines to return (default 100)"),
            }),
            vec!["pattern"],
            read_only,
        ),
        "edit" => (
            EDIT_DESCRIPTION.to_string(),
            json!({
                "path": string("File to edit. Relative paths resolve from the workspace root."),
                "oldString": string("Exact text to find and replace"),
                "newString": string("Text to replace oldString with (must differ from oldString)"),
                "replaceAll": boolean("Replace every occurrence of oldString (default false)"),
            }),
            vec!["path", "oldString", "newString"],
            edits,
        ),
        "write" => (
            WRITE_DESCRIPTION.to_string(),
            json!({
                "path": string("File to write. Relative paths resolve from the workspace root."),
                "content": string("Content to write to the file"),
            }),
            vec!["path", "content"],
            edits,
        ),
        "patch" => (
            PATCH_DESCRIPTION.to_string(),
            json!({
                "patchText": string("The full patch text describing add, update, move and delete operations"),
            }),
            vec!["patchText"],
            edits,
        ),
        "shell" => (
            shell_description(limit_secs),
            json!({
                "command": string("Shell command string to execute"),
                "workdir": string("Working directory. Defaults to the workspace root; relative paths resolve from it. Use this instead of cd in the command."),
                "timeout": integer("Timeout in milliseconds. Foreground commands default to 120000. Background commands have no timeout unless one is given."),
                "background": boolean("Run in the background and return a job id immediately. Use for dev servers and long-running builds; inspect it with shell_job."),
            }),
            vec!["command"],
            json!({ "destructiveHint": true, "openWorldHint": true }),
        ),
        _ => (
            SHELL_JOB_DESCRIPTION.to_string(),
            json!({
                "action": { "type": "string", "enum": ["list", "status", "wait", "kill"], "description": "One of: list, status, wait, kill" },
                "id": string("Job id returned by shell with background: true. Required except for list."),
                "timeout": integer("For wait: how long to wait in milliseconds before returning (default 30000, at most 600000)."),
            }),
            vec!["action"],
            edits,
        ),
    };
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
        "annotations": annotations,
    })
}

fn string(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn integer(description: &str) -> Value {
    json!({ "type": "integer", "description": description })
}

fn boolean(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

const READ_DESCRIPTION: &str = "Read the contents of a file or directory. Supports text files, images, and PDFs.
Each text line is prefixed by its 1-based line number as <line>: <content>. The prefix is for reference and is not part of the file content.
Directory entries are returned one per line, sorted with directories first and marked with a trailing /.
Use offset and limit to read large files or directories in sections. Text pages hold at most 2000 lines and 50 KiB; lines longer than 2000 characters are shortened.
Prefer one larger read over many small slices, and use grep to find specific content in large files.";

const GLOB_DESCRIPTION: &str = "Search file paths using a glob pattern (examples: \"**/*.go\", \"src/**/*.tsx\").
A pattern without a slash matches the file name at any depth. Files ignored by .gitignore are skipped.
Returns matching file paths, one per line.";

const GREP_DESCRIPTION: &str = "Search file contents using regular expressions or literal text.
Narrow the search with path and include. Files ignored by .gitignore are skipped; hidden files are searched.
Returns matching file paths, line numbers, and line previews, grouped by file.";

const EDIT_DESCRIPTION: &str = "Edit the contents of a file by finding and replacing exact text.
When editing text from read output, preserve the exact indentation (tabs or spaces) and omit the line-number prefix, such as \"1: \". Never include the prefix in oldString or newString.
The edit fails if oldString is not found. By default oldString must identify a UNIQUE location; multiple matches fail unless replaceAll is true. Add more surrounding context to disambiguate, or set replaceAll to true to replace every occurrence (e.g. renaming a variable).
Line endings and a UTF-8 BOM in the file are preserved. Use write to create a file.";

const WRITE_DESCRIPTION: &str = "Create a text file or completely replace its content. Missing parent directories are created.
An existing UTF-8 BOM is preserved. Use edit instead when only part of an existing file should change.";

const PATCH_DESCRIPTION: &str = "Apply one patch that can add, update, move, or delete several files.

*** Begin Patch
*** Add File: path/new.txt
+first line of the new file
*** Update File: path/app.py
*** Move to: path/main.py
@@ def greet():
-print(\"Hi\")
+print(\"Hello, world!\")
*** Delete File: path/obsolete.txt
*** End Patch

- Add File: every following line is a + line (the initial contents).
- Update File: one or more @@ chunks. Text after @@ is an optional context line that is located first. Lines start with \" \" (context), \"-\" (remove) or \"+\" (add). Include a few context lines around each change. \"*** End of File\" anchors a chunk at the end of the file.
- Move to: optional, directly after Update File, renames the file.
- Delete File: nothing follows.
Paths are relative to the workspace root. Every operation is validated before any file is changed.";

const SHELL_JOB_DESCRIPTION: &str = "Manage background jobs started by shell with background: true.
- list: every job with its state.
- status: state plus the latest output of one job.
- wait: block until the job finishes or the timeout passes, then report status and output.
- kill: stop the job and its whole process group.
Full output stays in the job's output file, which read and grep can open.";

fn shell_description(limit_secs: u64) -> String {
    let limit = match limit_secs {
        0 => "Foreground commands have no time limit unless timeout is set.".to_string(),
        s => format!(
            "Commands stop after {} ms; timeout may be shorter, not longer.",
            s * 1000
        ),
    };
    format!(
        "Execute a shell command and return its combined stdout and stderr.
Runs with the host user's filesystem, process and network authority. OS: {}, Shell: {}.
The workspace root is the default working directory; set workdir instead of using cd. Stdin is closed, so interactive commands fail.
{limit} When the timeout passes, the whole process group is stopped.
Set background to true for dev servers and other long-running processes; it returns a job id, and shell_job lists, inspects, waits for and stops jobs.
Output over {} lines or {} bytes is cut to its last part, and the full output is saved to a file you can grep or read.
Prefer the dedicated read, glob, grep, edit, write and patch tools over shell for file operations.
Do not chain shell commands with separators like echo \"====\"; it only adds noise.",
        std::env::consts::OS,
        shell::shell_path(),
        output::MAX_LINES,
        output::MAX_BYTES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A folder removed on drop. By default under `target/`, outside the temp folder the
    /// tools may always use, so confinement can be tested.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(name: &str) -> TempDir {
            let base = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("local-tests");
            TempDir::under(&base, name)
        }

        pub fn in_temp(name: &str) -> TempDir {
            TempDir::under(&std::env::temp_dir(), name)
        }

        fn under(base: &Path, name: &str) -> TempDir {
            let dir = base.join(format!(
                "{name}-{}-{}",
                std::process::id(),
                fastrand::u64(..)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(workspace::canonical(&dir).unwrap())
        }

        pub fn path(&self) -> &Path {
            &self.0
        }

        pub fn file(&self, name: &str, content: &[u8]) -> PathBuf {
            let p = self.0.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
            p
        }

        pub fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.0.join(name)).unwrap()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Harness {
        local: Local,
        root: TempDir,
        outside: TempDir,
    }

    fn harness() -> Harness {
        let root = TempDir::new("root");
        let local = Local::new(LocalSettings {
            enabled: true,
            root: root.path().display().to_string(),
            ..LocalSettings::default()
        });
        Harness {
            local,
            root,
            outside: TempDir::new("outside"),
        }
    }

    impl Harness {
        fn call(&self, name: &str, args: Value) -> Result<String, String> {
            self.local.call(name, &args).map(|content| {
                content
                    .iter()
                    .filter_map(|c| c["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }

        fn ok(&self, name: &str, args: Value) -> String {
            self.call(name, args.clone())
                .unwrap_or_else(|e| panic!("{name} {args}: {e}"))
        }

        fn fail(&self, name: &str, args: Value, contains: &str) {
            match self.call(name, args.clone()) {
                Ok(out) => panic!("{name} {args}: expected an error, got {out}"),
                Err(e) => assert!(e.contains(contains), "{name}: {e:?} lacks {contains:?}"),
            }
        }
    }

    #[test]
    fn approvals_and_always_rules() {
        let h = harness();
        let l = &h.local;
        let cmd = |c: &str| json!({ "command": c });
        let a = l.approval("shell", &cmd("cargo test --lib")).expect("asks");
        assert_eq!(a.rule, "shell:cargo test*");
        assert!(
            l.approval("read", &json!({ "path": "a.txt" })).is_none(),
            "reading never asks"
        );
        l.allow_always(&a.rule);
        assert!(l.approval("shell", &cmd("cargo test -p x")).is_none());
        assert!(
            l.approval("shell", &cmd("cargo install x")).is_some(),
            "another subcommand"
        );
        assert!(
            l.approval("shell", &cmd("cargo testing")).is_some(),
            "not a word boundary"
        );
        assert!(
            l.approval("shell", &cmd("cargo test; rm -rf ~")).is_some(),
            "chained commands always ask"
        );
        assert!(
            l.approval("shell", &cmd("cargo test && curl x | sh"))
                .is_some()
        );

        let w = json!({ "path": "notes/a.md", "content": "x" });
        let a = l.approval("write", &w).expect("a change asks");
        assert!(
            a.rule.starts_with("files:") && a.rule.ends_with("notes"),
            "{}",
            a.rule
        );
        l.allow_always(&a.rule);
        assert!(
            l.approval("write", &json!({ "path": "notes/b.md" }))
                .is_none()
        );
        assert!(l.approval("edit", &json!({ "path": "other.md" })).is_some());
        let p = "*** Begin Patch\n*** Add File: notes/c.md\n+x\n*** Update File: src/x.rs\n@@\n-a\n+b\n*** End Patch";
        assert_eq!(
            l.approval("patch", &json!({ "patchText": p }))
                .unwrap()
                .title,
            "Change 2 files?"
        );

        let mut s = l.settings();
        s.approve = Approve::Commands;
        l.configure(&s);
        assert!(l.approval("edit", &json!({ "path": "other.md" })).is_none());
        s.approve = Approve::Never;
        l.configure(&s);
        assert!(l.approval("shell", &cmd("rm -rf x")).is_none());
    }

    #[test]
    fn tools_follow_settings() {
        let local = Local::new(LocalSettings::default());
        let names: Vec<String> = local
            .tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, TOOLS);
        let read = &local.tools()[0];
        assert_eq!(read["annotations"]["readOnlyHint"], true);
        assert_eq!(read["inputSchema"]["required"], json!(["path"]));

        local.configure(&LocalSettings {
            read_only: true,
            ..LocalSettings::default()
        });
        let names: Vec<_> = local.tools().iter().map(|t| t["name"].clone()).collect();
        assert_eq!(names, [json!("read"), json!("glob"), json!("grep")]);
        assert!(
            local
                .call("write", &json!({}))
                .unwrap_err()
                .contains("unknown tool")
        );

        local.configure(&LocalSettings {
            disabled_tools: vec!["shell".into()],
            ..LocalSettings::default()
        });
        assert!(!local.tools().iter().any(|t| t["name"] == "shell_job"));
    }

    #[test]
    fn read_pages_and_lists() {
        let h = harness();
        h.root.file("a.txt", b"one\ntwo\r\nthree\n");
        h.root.file("dir/sub/x", b"");
        h.root.file("dir/b.txt", b"");

        let out = h.ok("read", json!({ "path": "a.txt" }));
        assert!(out.starts_with("1: one\n2: two\n3: three\n"), "{out}");
        assert!(out.contains("total 3 lines"), "{out}");

        let out = h.ok(
            "read",
            json!({ "filePath": "a.txt", "offset": 2, "limit": 1 }),
        );
        assert!(
            out.starts_with("2: two\n") && out.contains("offset=3"),
            "{out}"
        );

        h.fail(
            "read",
            json!({ "path": "a.txt", "offset": 10 }),
            "out of range",
        );
        let out = h.ok("read", json!({ "path": "dir" }));
        assert!(out.starts_with("sub/\nb.txt"), "{out}");

        h.fail("read", json!({ "path": "missing.txt" }), "file not found");
        h.fail("read", json!({ "path": "a.tx" }), "a.txt");
        let outside = h.outside.path().join("x").display().to_string();
        h.fail("read", json!({ "path": outside }), "outside");
        h.fail("read", json!({ "path": "../escape" }), "outside");

        h.root.file("bin.o", b"abc");
        h.fail("read", json!({ "path": "bin.o" }), "binary");

        h.root.file("long.txt", "x".repeat(2500).as_bytes());
        let out = h.ok("read", json!({ "path": "long.txt" }));
        assert!(out.contains("line truncated to 2000"), "{out}");

        let many: String = (1..=3000).map(|i| format!("{i}\n")).collect();
        h.root.file("many.txt", many.as_bytes());
        let out = h.ok("read", json!({ "path": "many.txt" }));
        assert!(out.contains("Showing lines 1-2000") && out.contains("offset=2001"));
        let wide = format!("{}\n", "y".repeat(100)).repeat(1000);
        h.root.file("wide.txt", wide.as_bytes());
        let out = h.ok("read", json!({ "path": "wide.txt" }));
        assert!(out.len() <= read::MAX_BYTES + 200, "{}", out.len());
        assert!(out.contains("File has more lines"), "{out}");
    }

    #[test]
    fn read_media() {
        let h = harness();
        h.root.file("p.png", b"\x89PNG\r\n\x1a\nrest");
        h.root.file("d.pdf", b"%PDF-1.4");
        let png = h.local.call("read", &json!({ "path": "p.png" })).unwrap();
        assert_eq!(png[1]["type"], "image");
        assert_eq!(png[1]["mimeType"], "image/png");
        assert_eq!(png[1]["data"], read::base64(b"\x89PNG\r\n\x1a\nrest"));
        let pdf = h.local.call("read", &json!({ "path": "d.pdf" })).unwrap();
        assert_eq!(pdf[1]["type"], "resource");
        assert_eq!(pdf[1]["resource"]["mimeType"], "application/pdf");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let h = harness();
        let secret = h.outside.file("secret", b"x");
        std::os::unix::fs::symlink(&secret, h.root.path().join("link")).unwrap();
        std::os::unix::fs::symlink(h.outside.path(), h.root.path().join("dirlink")).unwrap();
        h.fail("read", json!({ "path": "link" }), "outside");
        h.fail(
            "write",
            json!({ "path": "link", "content": "y" }),
            "outside",
        );
        h.fail(
            "write",
            json!({ "path": "dirlink/new/f", "content": "y" }),
            "outside",
        );
        std::os::unix::fs::symlink(
            h.outside.path().join("nope"),
            h.root.path().join("dangling"),
        )
        .unwrap();
        h.fail(
            "write",
            json!({ "path": "dangling", "content": "y" }),
            "outside",
        );
        let out = h.ok("read", json!({ "path": "." }));
        assert!(!out.contains("link"), "{out}");
    }

    #[test]
    fn temp_and_allowed_dirs() {
        let h = harness();
        let tmp = TempDir::in_temp("tmp");
        let f = tmp.path().join("t.txt").display().to_string();
        h.ok("write", json!({ "path": f, "content": "hi\n" }));
        assert!(h.ok("read", json!({ "path": f })).starts_with("1: hi"));
        if cfg!(unix) {
            let name = format!("dost-local-{}-{}", std::process::id(), fastrand::u64(..));
            let p = format!("/tmp/{name}");
            h.ok("write", json!({ "path": p, "content": "x" }));
            let _ = std::fs::remove_file(&p);
        }

        let outside = h.outside.path().join("o.txt").display().to_string();
        h.fail(
            "write",
            json!({ "path": outside, "content": "x" }),
            "outside",
        );
        h.local.configure(&LocalSettings {
            root: h.root.path().display().to_string(),
            allowed_dirs: vec![h.outside.path().display().to_string()],
            ..LocalSettings::default()
        });
        h.ok("write", json!({ "path": outside, "content": "x" }));
        assert!(
            h.local
                .instructions()
                .contains(&h.outside.path().display().to_string())
        );
    }

    #[test]
    fn glob_and_grep() {
        let h = harness();
        h.root.file("a.go", b"package a\nfunc Hello() {}\n");
        h.root.file("sub/b.go", b"package sub\n// hello\n");
        h.root.file("c.md", b"Hello\n");

        assert_eq!(
            h.ok("glob", json!({ "pattern": "**/*.go" })),
            "a.go\nsub/b.go"
        );
        assert_eq!(h.ok("glob", json!({ "pattern": "*.rs" })), "No files found");
        let out = h.ok("glob", json!({ "pattern": "*", "limit": 1 }));
        assert!(out.contains("truncated at 1"), "{out}");

        let out = h.ok("grep", json!({ "pattern": "hello", "include": "*.go" }));
        assert!(out.contains("Found 1 matches"), "{out}");
        assert!(out.contains("sub/b.go:\n  Line 2: // hello"), "{out}");
        let out = h.ok(
            "grep",
            json!({ "pattern": "hello", "caseSensitive": false }),
        );
        assert!(out.contains("Found 3 matches"), "{out}");
        let out = h.ok(
            "grep",
            json!({ "pattern": "Hello()", "literal": true, "path": "a.go" }),
        );
        assert!(out.contains("a.go:\n  Line 2: func Hello() {}"), "{out}");
        h.fail("grep", json!({ "pattern": "(" }), "invalid pattern");
    }

    #[test]
    fn edit_keeps_bom_and_line_endings() {
        let h = harness();
        h.root
            .file("a.txt", "\u{feff}alpha\r\nbeta\r\nbeta\r\n".as_bytes());
        let edit =
            |old: &str, new: &str| json!({ "path": "a.txt", "oldString": old, "newString": new });
        h.fail("edit", edit("beta", "x"), "found 2 matches");
        h.fail("edit", edit("gamma", "x"), "could not find");
        h.fail("edit", edit("a", "a"), "identical");
        h.fail("edit", edit("", "a"), "must not be empty");

        let out = h.ok("edit", edit("alpha\nbeta", "ALPHA\nBETA"));
        assert!(
            out.contains("Edited a.txt") && out.contains("+ALPHA"),
            "{out}"
        );
        assert_eq!(h.root.read("a.txt"), "\u{feff}ALPHA\r\nBETA\r\nbeta\r\n");

        h.ok(
            "edit",
            json!({ "path": "a.txt", "oldString": "A", "newString": "a", "replaceAll": true }),
        );
        assert_eq!(h.root.read("a.txt"), "\u{feff}aLPHa\r\nBETa\r\nbeta\r\n");
    }

    #[test]
    fn write_creates_and_overwrites() {
        let h = harness();
        let out = h.ok("write", json!({ "path": "new/dir/f.txt", "content": "hi" }));
        assert!(out.contains("Created"), "{out}");
        let out = h.ok(
            "write",
            json!({ "path": "new/dir/f.txt", "content": "bye" }),
        );
        assert!(out.contains("Wrote"), "{out}");
        assert_eq!(h.root.read("new/dir/f.txt"), "bye");
        h.fail(
            "write",
            json!({ "path": "new", "content": "" }),
            "directory",
        );

        h.root.file("bom.txt", "\u{feff}x".as_bytes());
        h.ok("write", json!({ "path": "bom.txt", "content": "y" }));
        assert_eq!(h.root.read("bom.txt"), "\u{feff}y");
    }

    #[test]
    fn patch_applies_or_changes_nothing() {
        let h = harness();
        h.root
            .file("app.py", b"import os\n\ndef greet():\n    print(\"Hi\")\n");
        h.root.file("old.txt", b"bye\n");
        let out = h.ok(
            "patch",
            json!({ "patchText": "*** Begin Patch
*** Add File: docs/new.md
+# Title
*** Update File: app.py
*** Move to: src/main.py
@@ def greet():
-    print(\"Hi\")
+    print(\"Hello\")
*** Delete File: old.txt
*** End Patch" }),
        );
        for want in ["A docs/new.md", "R app.py -> src/main.py", "D old.txt"] {
            assert!(out.contains(want), "{want}: {out}");
        }
        assert_eq!(h.root.read("docs/new.md"), "# Title\n");
        assert_eq!(
            h.root.read("src/main.py"),
            "import os\n\ndef greet():\n    print(\"Hello\")\n"
        );
        assert!(!h.root.path().join("app.py").exists());
        assert!(!h.root.path().join("old.txt").exists());

        h.fail(
            "patch",
            json!({ "patchText": "*** Begin Patch
*** Add File: z.txt
+z
*** Update File: src/main.py
@@
-not there
+x
*** End Patch" }),
            "failed to find",
        );
        assert!(!h.root.path().join("z.txt").exists());
        h.fail(
            "patch",
            json!({ "patchText": "*** Begin Patch\n*** Add File: ../x\n+x\n*** End Patch" }),
            "outside",
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_runs_and_times_out() {
        let h = harness();
        h.root.file("sub/f", b"");
        let out = h.ok(
            "shell",
            json!({ "command": "pwd; echo err >&2", "workdir": "sub" }),
        );
        assert!(out.contains("/sub\n") && out.contains("err"), "{out}");
        assert!(out.contains("Exit code: 0"), "{out}");

        h.fail("shell", json!({ "command": "exit 3" }), "Exit code: 3");

        let start = std::time::Instant::now();
        h.fail(
            "shell",
            json!({ "command": "sleep 30 & sleep 30", "timeout": 300 }),
            "exceeding the timeout",
        );
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "{:?}",
            start.elapsed()
        );

        let out = h.ok("shell", json!({ "command": "seq 1 3000" }));
        assert!(
            out.contains("Full output saved to:") && out.contains("\n3000\n"),
            "{out}"
        );
        let saved = out
            .split("Full output saved to: ")
            .nth(1)
            .and_then(|s| s.lines().next())
            .unwrap()
            .trim();
        let got = h.ok("read", json!({ "path": saved, "limit": 1 }));
        assert!(got.starts_with("1: 1"), "{got}");
        let got = h.ok("grep", json!({ "pattern": "^2999$", "path": saved }));
        assert!(got.contains("Line 2999: 2999"), "{got}");
        let _ = std::fs::remove_file(saved);

        let outside = h.outside.path().display().to_string();
        h.fail(
            "shell",
            json!({ "command": "true", "workdir": outside }),
            "outside",
        );

        // The setting caps what a command may ask for.
        let mut s = h.local.settings();
        s.time_limit_secs = 1;
        h.local.configure(&s);
        let start = std::time::Instant::now();
        h.fail(
            "shell",
            json!({ "command": "sleep 20", "timeout": 600_000 }),
            "exceeding the timeout",
        );
        assert!(start.elapsed() < Duration::from_secs(6));
    }

    #[cfg(unix)]
    #[test]
    fn shell_output_comes_live() {
        let h = harness();
        let mut got: Vec<(std::time::Instant, String)> = Vec::new();
        let start = std::time::Instant::now();
        let mut live = |t: String| {
            if !t.is_empty() {
                got.push((std::time::Instant::now(), t));
            }
        };
        h.local
            .call_live(
                "shell",
                &json!({ "command": "echo one; sleep 1.2; echo two" }),
                Some(&mut live),
            )
            .unwrap();
        let text: String = got.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(text, "one\ntwo\n");
        assert!(
            got[0].0 - start < Duration::from_millis(900),
            "the first line came before the command ended"
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_background_jobs() {
        let h = harness();
        let out = h.ok(
            "shell",
            json!({ "command": "echo started; sleep 0.2; echo done", "background": true }),
        );
        assert!(out.contains("job_1"), "{out}");
        let out = h.ok(
            "shell_job",
            json!({ "action": "wait", "id": "job_1", "timeout": 5000 }),
        );
        assert!(out.contains("[exited]") && out.contains("exit=0"), "{out}");
        assert!(out.contains("done"), "{out}");

        h.ok(
            "shell",
            json!({ "command": "sleep 60", "background": true }),
        );
        let out = h.ok("shell_job", json!({ "action": "kill", "id": "job_2" }));
        assert!(out.contains("[killed]"), "{out}");

        let out = h.ok("shell_job", json!({ "action": "list" }));
        assert_eq!(out.matches("job_").count(), 2, "{out}");
        h.fail(
            "shell_job",
            json!({ "action": "status", "id": "job_9" }),
            "unknown job",
        );
        h.fail(
            "shell_job",
            json!({ "action": "nope", "id": "job_1" }),
            "unknown action",
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_runs_on_windows() {
        let h = harness();
        let out = h.ok("shell", json!({ "command": "echo hello" }));
        assert!(
            out.contains("hello") && out.contains("Exit code: 0"),
            "{out}"
        );
        h.fail("shell", json!({ "command": "exit 3" }), "Exit code: 3");
    }
}
