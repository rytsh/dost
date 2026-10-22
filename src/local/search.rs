//! Finds files by glob and lines by regular expression: with ripgrep when it is installed,
//! else with a built-in engine that walks the tree the same way (honouring `.gitignore`,
//! `.ignore`, `.rgignore` and `.git/info/exclude`, with `--glob` override semantics).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use ignore::overrides::{Override, OverrideBuilder};
use serde_json::Value;

/// How long one search may run.
pub const TIMEOUT: Duration = Duration::from_secs(30);

pub struct GlobInput<'a> {
    pub dir: &'a Path,
    pub pattern: &'a str,
    pub hidden: bool,
    /// The search stops after `limit + 1` results so the caller can tell it was cut.
    pub limit: usize,
}

pub struct GrepInput<'a> {
    /// The folder searched, or the parent of `file`.
    pub dir: &'a Path,
    /// When set, only this file (relative to `dir`) is searched.
    pub file: &'a str,
    pub pattern: &'a str,
    pub literal: bool,
    pub case_sensitive: bool,
    pub include: &'a str,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// Relative to the searched folder, with `/`.
    pub path: String,
    pub line: u64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum Engine {
    Native,
    Ripgrep(PathBuf),
}

impl Engine {
    /// Ripgrep when it can be found, else the built-in engine.
    pub fn auto() -> Engine {
        static RG: OnceLock<Option<PathBuf>> = OnceLock::new();
        match RG.get_or_init(find_rg) {
            Some(p) => Engine::Ripgrep(p.clone()),
            None => Engine::Native,
        }
    }

    pub fn glob(&self, input: &GlobInput) -> Result<Vec<String>, String> {
        match self {
            Engine::Native => native_glob(input),
            Engine::Ripgrep(rg) => rg_glob(rg, input),
        }
    }

    pub fn grep(&self, input: &GrepInput) -> Result<Vec<Match>, String> {
        match self {
            Engine::Native => native_grep(input),
            Engine::Ripgrep(rg) => rg_grep(rg, input),
        }
    }
}

/// `rg` on `PATH`, or where package managers put it (apps started from the desktop often get
/// a short `PATH`).
fn find_rg() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "rg.exe" } else { "rg" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if cfg!(unix) {
        dirs.extend(
            ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
                .iter()
                .map(PathBuf::from),
        );
    }
    dirs.push(super::expand("~/.cargo/bin"));
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// `p` relative and with `/`, without a leading `./`.
fn clean_rel(p: &str) -> String {
    let mut p = p.replace('\\', "/");
    while let Some(rest) = p.strip_prefix("./") {
        p = rest.to_string();
    }
    p.trim_start_matches('/').to_string()
}

fn overrides(dir: &Path, glob: &str, what: &str) -> Result<Override, String> {
    let mut b = OverrideBuilder::new(dir);
    if !glob.is_empty() {
        b.add(glob)
            .map_err(|e| format!("invalid {what} {glob:?}: {e}"))?;
    }
    b.build()
        .map_err(|e| format!("invalid {what} {glob:?}: {e}"))
}

/// Visits every regular file below `dir` in path order until `visit` returns false. Like
/// ripgrep's `--glob`, a file matching `glob` is kept even when ignore files or the hidden
/// rule would skip it; ignored folders are still not entered.
fn walk(
    dir: &Path,
    hidden: bool,
    glob: &str,
    what: &str,
    mut visit: impl FnMut(&Path, String) -> bool,
) -> Result<(), String> {
    let over = overrides(dir, glob, what)?;
    let deadline = Instant::now() + TIMEOUT;
    let walker = WalkBuilder::new(dir)
        .hidden(!hidden)
        .git_global(false)
        .require_git(false)
        .add_custom_ignore_filename(".rgignore")
        .overrides(over)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for entry in walker {
        if Instant::now() > deadline {
            return Err("search timed out".into());
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(dir) else {
            continue;
        };
        if !visit(entry.path(), super::workspace::slash(rel)) {
            break;
        }
    }
    Ok(())
}

fn native_glob(input: &GlobInput) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    walk(
        input.dir,
        input.hidden,
        input.pattern,
        "glob pattern",
        |_, rel| {
            out.push(rel);
            out.len() <= input.limit
        },
    )?;
    Ok(out)
}

fn native_grep(input: &GrepInput) -> Result<Vec<Match>, String> {
    let mut expr = if input.literal {
        regex::escape(input.pattern)
    } else {
        input.pattern.to_string()
    };
    if !input.case_sensitive {
        expr = format!("(?i){expr}");
    }
    let re = regex::bytes::Regex::new(&expr).map_err(|e| format!("invalid pattern: {e}"))?;
    let mut out = Vec::new();
    if !input.file.is_empty() {
        let rel = clean_rel(input.file);
        let over = overrides(input.dir, input.include, "include pattern")?;
        if input.include.is_empty() || !over.matched(&rel, false).is_ignore() {
            grep_file(
                &input.dir.join(input.file),
                &rel,
                &re,
                input.limit + 1,
                &mut out,
            );
        }
        return Ok(out);
    }
    walk(
        input.dir,
        true,
        input.include,
        "include pattern",
        |abs, rel| {
            grep_file(abs, &rel, &re, input.limit + 1, &mut out);
            out.len() <= input.limit
        },
    )?;
    Ok(out)
}

/// Adds the matching lines of a text file to `out`, up to `limit` in all.
fn grep_file(abs: &Path, rel: &str, re: &regex::bytes::Regex, limit: usize, out: &mut Vec<Match>) {
    let Ok(f) = std::fs::File::open(abs) else {
        return;
    };
    let mut reader = BufReader::with_capacity(64 * 1024, f);
    let Ok(head) = reader.fill_buf() else { return };
    if head[..head.len().min(8 * 1024)].contains(&0) {
        return;
    }
    let mut line = Vec::new();
    let mut n = 0;
    while out.len() < limit {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        n += 1;
        while line.last().is_some_and(|&b| b == b'\n' || b == b'\r') {
            line.pop();
        }
        if re.is_match(&line) {
            out.push(Match {
                path: rel.to_string(),
                line: n,
                text: String::from_utf8_lossy(&line).into_owned(),
            });
        }
    }
}

fn rg_glob(rg: &Path, input: &GlobInput) -> Result<Vec<String>, String> {
    let mut args = vec!["--no-config", "--files", "--sort=path"];
    if input.hidden {
        args.push("--hidden");
    }
    let glob = format!("--glob={}", input.pattern);
    args.extend([glob.as_str(), "--glob=!**/.git/**", "."]);
    let mut out = Vec::new();
    rg_run(rg, input.dir, &args, |line| {
        out.push(clean_rel(line));
        out.len() <= input.limit
    })?;
    Ok(out)
}

fn rg_grep(rg: &Path, input: &GrepInput) -> Result<Vec<Match>, String> {
    let mut args = vec![
        "--no-config",
        "--json",
        "--hidden",
        "--no-messages",
        "--sort=path",
    ];
    if input.literal {
        args.push("--fixed-strings");
    }
    args.push(if input.case_sensitive {
        "--case-sensitive"
    } else {
        "--ignore-case"
    });
    let include = format!("--glob={}", input.include);
    if !input.include.is_empty() {
        args.push(&include);
    }
    let target = if input.file.is_empty() {
        "."
    } else {
        input.file
    };
    args.extend(["--glob=!**/.git/**", "--", input.pattern, target]);
    let mut out = Vec::new();
    rg_run(rg, input.dir, &args, |line| {
        if !line.starts_with(r#"{"type":"match""#) {
            return true;
        }
        let Ok(rec) = serde_json::from_str::<Value>(line) else {
            return true;
        };
        let data = &rec["data"];
        out.push(Match {
            path: clean_rel(data["path"]["text"].as_str().unwrap_or("")),
            line: data["line_number"].as_u64().unwrap_or(0),
            text: data["lines"]["text"]
                .as_str()
                .unwrap_or("")
                .trim_end_matches(['\r', '\n'])
                .to_string(),
        });
        out.len() <= input.limit
    })?;
    Ok(out)
}

/// Streams rg's output lines to `each` until it returns false, then stops rg.
fn rg_run(
    rg: &Path,
    dir: &Path,
    args: &[&str],
    mut each: impl FnMut(&str) -> bool,
) -> Result<(), String> {
    let mut cmd = Command::new(rg);
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, super::shell::CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| format!("start ripgrep; {e}"))?;
    let stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));

    let (done, finished) = mpsc::channel::<()>();
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = {
        let child = child.clone();
        let timed_out = timed_out.clone();
        std::thread::spawn(move || {
            if finished.recv_timeout(TIMEOUT).is_err() {
                timed_out.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Ok(mut c) = child.lock() {
                    let _ = c.kill();
                }
            }
        })
    };
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.take(8 * 1024).read_to_string(&mut s);
        }
        s
    });

    let mut stopped = false;
    if let Some(stdout) = stdout {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                continue;
            }
            if !each(line) {
                stopped = true;
                if let Ok(mut c) = child.lock() {
                    let _ = c.kill();
                }
                break;
            }
        }
    }
    let status = child.lock().map_err(|e| e.to_string())?.wait();
    let _ = done.send(());
    let _ = watchdog.join();
    let msg = err_reader.join().unwrap_or_default();
    let msg = msg.trim();
    if stopped {
        return Ok(());
    }
    if timed_out.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("search timed out".into());
    }
    let status = status.map_err(|e| format!("ripgrep failed; {e}"))?;
    match status.code() {
        Some(0) | Some(1) => Ok(()),
        Some(2) if msg.contains("regex parse error") || msg.contains("error parsing") => {
            Err(format!("invalid pattern: {msg}"))
        }
        // Some files could not be read; keep the partial result.
        Some(2) => Ok(()),
        _ if msg.is_empty() => Err(format!("ripgrep failed: {status}")),
        _ => Err(format!("ripgrep failed: {msg}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> super::super::tests::TempDir {
        let dir = super::super::tests::TempDir::new("search");
        let files: &[(&str, &[u8])] = &[
            (".gitignore", b"ignored/\n*.log\n"),
            (
                "main.go",
                b"package main\n\nfunc main() {\n\tprintln(\"Hello\")\n}\n",
            ),
            ("pkg/a/a.go", b"package a\n// TODO: hello\n"),
            ("pkg/a/a_test.go", b"package a\n"),
            ("web/app.ts", b"const hello = 1\n"),
            ("web/app.tsx", b"export {}\n"),
            ("ignored/x.go", b"package hello\n"),
            ("debug.log", b"hello\n"),
            (".hidden/secret.go", b"package hello\n"),
            ("bin.dat", b"hel\x00lo"),
            ("pkg/a/.ignore", b"a_test.go\n"),
            ("pkg/b/nested/b.go", b"package b // hello\n"),
            (".git/config", b"hello\n"),
        ];
        for (name, content) in files {
            dir.file(name, content);
        }
        dir
    }

    fn engines() -> Vec<Engine> {
        let mut out = vec![Engine::Native];
        if let Some(rg) = find_rg() {
            out.push(Engine::Ripgrep(rg));
        }
        out
    }

    #[test]
    fn glob_engines() {
        let dir = fixture();
        let all = [
            "main.go",
            "pkg/a/a.go",
            "pkg/a/a_test.go",
            "pkg/b/nested/b.go",
        ];
        let cases: Vec<(&str, bool, Vec<&str>)> = vec![
            ("*.go", false, all.to_vec()),
            ("**/*.go", false, all.to_vec()),
            (
                "pkg/**/*.go",
                false,
                vec!["pkg/a/a.go", "pkg/a/a_test.go", "pkg/b/nested/b.go"],
            ),
            ("*.{ts,tsx}", false, vec!["web/app.ts", "web/app.tsx"]),
            ("*.log", false, vec!["debug.log"]),
            ("!*.go", false, vec!["bin.dat", "web/app.ts", "web/app.tsx"]),
            (
                "*.go",
                true,
                [".hidden/secret.go"]
                    .iter()
                    .chain(all.iter())
                    .copied()
                    .collect(),
            ),
        ];
        for e in engines() {
            for (pattern, hidden, want) in &cases {
                let mut got = e
                    .glob(&GlobInput {
                        dir: dir.path(),
                        pattern,
                        hidden: *hidden,
                        limit: 100,
                    })
                    .unwrap();
                got.sort();
                assert_eq!(got, *want, "{e:?} glob {pattern:?} hidden={hidden}");
            }
        }
    }

    #[test]
    fn grep_engines() {
        let dir = fixture();
        let grep =
            |e: &Engine, pattern: &str, file: &str, literal, case_sensitive, include, limit| {
                e.grep(&GrepInput {
                    dir: dir.path(),
                    file,
                    pattern,
                    literal,
                    case_sensitive,
                    include,
                    limit,
                })
            };
        for e in engines() {
            let got = grep(&e, "hello", "", false, true, "", 100).unwrap();
            let mut paths: Vec<_> = got.iter().map(|m| m.path.as_str()).collect();
            paths.sort();
            assert_eq!(
                paths,
                [
                    ".hidden/secret.go",
                    "pkg/a/a.go",
                    "pkg/b/nested/b.go",
                    "web/app.ts"
                ],
                "{e:?}"
            );

            let got = grep(&e, "hello", "", false, false, "*.go", 100).unwrap();
            assert_eq!(got.len(), 4, "{e:?} insensitive include: {got:?}");

            let got = grep(&e, "println(\"", "main.go", true, true, "", 100).unwrap();
            assert_eq!(
                got,
                [Match {
                    path: "main.go".into(),
                    line: 4,
                    text: "\tprintln(\"Hello\")".into()
                }],
                "{e:?}"
            );

            let err = grep(&e, "(", "", false, true, "", 10).unwrap_err();
            assert!(err.contains("invalid pattern"), "{e:?}: {err}");

            let got = grep(&e, "e", "", false, true, "", 2).unwrap();
            assert_eq!(got.len(), 3, "{e:?} limit+1");
        }
    }
}
