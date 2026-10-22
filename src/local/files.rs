//! The tools that change files: `edit`, `write` and `patch`. Writes go through a temp file
//! so a failure never leaves a half-written file; line endings and a UTF-8 BOM are kept.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use super::patch::{self, Hunk, Kind};
use super::workspace::Workspace;

const BOM: &str = "\u{feff}";

/// Serializes changes to the same file across concurrent calls.
fn file_lock(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    locks.entry(path.to_path_buf()).or_default().clone()
}

fn hold(lock: &Mutex<()>) -> MutexGuard<'_, ()> {
    lock.lock().unwrap_or_else(|e| e.into_inner())
}

/// A text file's content without its BOM, and whether it had one.
struct TextFile {
    bom: bool,
    text: String,
}

impl TextFile {
    fn decode(data: Vec<u8>, path: &str) -> Result<TextFile, String> {
        let text =
            String::from_utf8(data).map_err(|_| format!("file is not valid UTF-8: {path}"))?;
        Ok(match text.strip_prefix(BOM) {
            Some(rest) => TextFile {
                bom: true,
                text: rest.to_string(),
            },
            None => TextFile { bom: false, text },
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.text.len() + 3);
        if self.bom {
            out.extend_from_slice(BOM.as_bytes());
        }
        out.extend_from_slice(self.text.as_bytes());
        out
    }
}

fn line_ending(s: &str) -> &'static str {
    if s.contains("\r\n") { "\r\n" } else { "\n" }
}

fn to_line_ending(s: &str, ending: &str) -> String {
    let s = s.replace("\r\n", "\n");
    if ending == "\r\n" {
        s.replace('\n', "\r\n")
    } else {
        s
    }
}

/// Writes through a temp file in the same folder, keeping an existing file's permissions and
/// creating missing parent folders.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("path has no parent folder")?;
    fs::create_dir_all(dir).map_err(|e| format!("create parent directories; {e}"))?;
    let perms = fs::metadata(path).ok().map(|m| m.permissions());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        fastrand::u64(..)
    ));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(data)?;
        if let Some(p) = &perms {
            f.set_permissions(p.clone())?;
        }
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}

/// A unified diff of two texts and its added and removed line counts.
struct DiffStat {
    patch: String,
    additions: usize,
    deletions: usize,
}

fn diff(name: &str, before: &str, after: &str) -> DiffStat {
    let before = before.replace("\r\n", "\n");
    let after = after.replace("\r\n", "\n");
    let patch = similar::TextDiff::from_lines(&before, &after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string();
    let mut st = DiffStat {
        patch,
        additions: 0,
        deletions: 0,
    };
    for line in st.patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            st.additions += 1;
        } else if line.starts_with('-') {
            st.deletions += 1;
        }
    }
    st
}

/// A diff for the agent, at most 60 lines.
fn preview_diff(patch: &str) -> String {
    const MAX: usize = 60;
    let mut lines: Vec<String> = patch
        .trim_end_matches('\n')
        .split('\n')
        .map(String::from)
        .collect();
    if lines.len() > MAX {
        let more = lines.len() - MAX;
        lines.truncate(MAX);
        lines.push(format!("... ({more} more diff lines)"));
    }
    format!("```diff\n{}\n```", lines.join("\n"))
}

pub fn edit(
    ws: &Workspace,
    path: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, String> {
    if old == new {
        return Err("no changes to apply: oldString and newString are identical".into());
    }
    if old.is_empty() {
        return Err("oldString must not be empty; use write to create or overwrite a file".into());
    }
    let abs = ws.resolve(path)?;
    let lock = file_lock(&abs);
    let _guard = hold(&lock);
    let data = match fs::read(&abs) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("file not found: {path}"));
        }
        Err(e) => return Err(format!("unable to edit {path}; {e}")),
    };
    let source = TextFile::decode(data, path)?;
    let ending = line_ending(&source.text);
    let old = to_line_ending(old, ending);
    let new = to_line_ending(new, ending);

    let count = source.text.matches(&old).count();
    if count == 0 {
        return Err("could not find oldString in the file; it must match exactly, including whitespace and indentation".into());
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "found {count} matches for oldString; provide more surrounding context to make it unique or set replaceAll to true"
        ));
    }
    let replaced = if replace_all {
        source.text.replace(&old, &new)
    } else {
        source.text.replacen(&old, &new, 1)
    };
    let next = TextFile {
        bom: source.bom,
        text: replaced,
    };
    write_atomic(&abs, &next.encode()).map_err(|e| format!("unable to edit {path}; {e}"))?;

    let name = ws.display(&abs);
    let st = diff(&name, &source.text, &next.text);
    let replacements = if replace_all { count } else { 1 };
    Ok(format!(
        "Edited {name} ({replacements} replacement(s), +{} -{})\n{}",
        st.additions,
        st.deletions,
        preview_diff(&st.patch)
    ))
}

pub fn write(ws: &Workspace, path: &str, content: &str) -> Result<String, String> {
    let abs = ws.resolve(path)?;
    let lock = file_lock(&abs);
    let _guard = hold(&lock);
    let mut next = match content.strip_prefix(BOM) {
        Some(rest) => TextFile {
            bom: true,
            text: rest.into(),
        },
        None => TextFile {
            bom: false,
            text: content.into(),
        },
    };
    let existed = match fs::metadata(&abs) {
        Ok(m) if m.is_dir() => return Err(format!("path is a directory: {path}")),
        Ok(_) => {
            if fs::read(&abs).is_ok_and(|old| old.starts_with(BOM.as_bytes())) {
                next.bom = true;
            }
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(format!("unable to write {path}; {e}")),
    };
    write_atomic(&abs, &next.encode()).map_err(|e| format!("unable to write {path}; {e}"))?;
    let verb = if existed { "Wrote" } else { "Created" };
    Ok(format!("{verb} file successfully: {}", ws.display(&abs)))
}

/// One patch operation, checked and with its new content worked out, before anything is
/// written.
struct Prepared {
    hunk: Hunk,
    abs: PathBuf,
    move_abs: Option<PathBuf>,
    bom: bool,
    before: String,
    after: String,
}

pub fn apply_patch(ws: &Workspace, patch_text: &str) -> Result<String, String> {
    if patch_text.trim().is_empty() {
        return Err("patchText is required".into());
    }
    let hunks = patch::parse(patch_text).map_err(|e| format!("patch verification failed: {e}"))?;
    if hunks.is_empty() {
        return Err("patch rejected: empty patch".into());
    }
    let mut changes = Vec::with_capacity(hunks.len());
    let mut seen = HashSet::new();
    for h in hunks {
        let path = h.path.clone();
        let c = prepare(ws, h).map_err(|e| format!("unable to apply patch at {path}: {e}"))?;
        for p in std::iter::once(&c.abs).chain(c.move_abs.as_ref()) {
            if !seen.insert(p.clone()) {
                return Err(format!("patch touches {} more than once", ws.display(p)));
            }
        }
        changes.push(c);
    }

    let mut lock_paths: Vec<&PathBuf> = changes.iter().map(|c| &c.abs).collect();
    lock_paths.sort();
    let locks: Vec<_> = lock_paths.into_iter().map(|p| file_lock(p)).collect();
    let _guards: Vec<_> = locks.iter().map(|l| hold(l)).collect();

    let (mut applied, mut diffs, mut adds, mut dels) = (Vec::new(), Vec::new(), 0, 0);
    for c in &changes {
        if let Err(e) = apply_change(c) {
            if applied.is_empty() {
                return Err(format!("unable to apply patch at {}: {e}", c.hunk.path));
            }
            return Err(format!(
                "patch partially applied before failing at {}: {e}\nApplied:\n{}",
                c.hunk.path,
                applied.join("\n")
            ));
        }
        let name = ws.display(&c.abs);
        applied.push(match (&c.hunk.kind, &c.move_abs) {
            (Kind::Add, _) => format!("A {name}"),
            (Kind::Delete, _) => format!("D {name}"),
            (_, Some(to)) => format!("R {name} -> {}", ws.display(to)),
            _ => format!("M {name}"),
        });
        let st = diff(&name, &c.before, &c.after);
        adds += st.additions;
        dels += st.deletions;
        if !st.patch.is_empty() {
            diffs.push(st.patch.trim_end_matches('\n').to_string());
        }
    }
    let mut out = format!("Applied patch (+{adds} -{dels}):\n{}", applied.join("\n"));
    if !diffs.is_empty() {
        out += &format!("\n{}", preview_diff(&diffs.join("\n")));
    }
    Ok(out)
}

fn prepare(ws: &Workspace, hunk: Hunk) -> Result<Prepared, String> {
    let abs = ws.resolve(&hunk.path)?;
    let mut c = Prepared {
        hunk,
        abs,
        move_abs: None,
        bom: false,
        before: String::new(),
        after: String::new(),
    };
    if c.hunk.kind == Kind::Add {
        if fs::symlink_metadata(&c.abs).is_ok() {
            return Err("file already exists; use Update File to change it".into());
        }
        c.after = c.hunk.contents.clone();
        if !c.after.is_empty() && !c.after.ends_with('\n') {
            c.after.push('\n');
        }
        return Ok(c);
    }
    let meta = fs::metadata(&c.abs).map_err(|_| "file not found")?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let data = fs::read(&c.abs).map_err(|e| e.to_string())?;
    let source = TextFile::decode(data, &c.hunk.path)?;
    c.bom = source.bom;
    c.before = source.text;
    if c.hunk.kind == Kind::Delete {
        return Ok(c);
    }
    c.after = if c.hunk.chunks.is_empty() {
        c.before.clone()
    } else {
        patch::derive(&c.hunk.path, &c.hunk.chunks, &c.before)?
    };
    if !c.hunk.move_path.is_empty() {
        let to = ws.resolve(&c.hunk.move_path)?;
        if to != c.abs {
            if fs::symlink_metadata(&to).is_ok() {
                return Err(format!("move target {} already exists", c.hunk.move_path));
            }
            c.move_abs = Some(to);
        }
    }
    Ok(c)
}

fn apply_change(c: &Prepared) -> Result<(), String> {
    match c.hunk.kind {
        Kind::Add => return write_atomic(&c.abs, c.after.as_bytes()),
        Kind::Delete => return fs::remove_file(&c.abs).map_err(|e| e.to_string()),
        Kind::Update => {}
    }
    let data = TextFile {
        bom: c.bom,
        text: c.after.clone(),
    }
    .encode();
    let Some(to) = &c.move_abs else {
        return write_atomic(&c.abs, &data);
    };
    write_atomic(to, &data)?;
    if let Ok(m) = fs::metadata(&c.abs) {
        let _ = fs::set_permissions(to, m.permissions());
    }
    fs::remove_file(&c.abs).map_err(|e| e.to_string())
}
