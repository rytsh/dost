//! Resolves tool paths and keeps them inside the folders the tools may touch.

use std::io;
use std::path::{Component, Path, PathBuf};

/// The root and the other folders paths may resolve into, all canonical.
#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
    allowed: Vec<PathBuf>,
}

impl Workspace {
    /// Canonicalizes `root` (which must be a folder) and the extra allowed folders. Relative
    /// allowed folders resolve from the root; missing ones are kept so they work once created.
    pub fn new(root: &Path, allowed: &[PathBuf]) -> Result<Self, String> {
        let canonical_root = canonical(&absolute(root))
            .map_err(|e| format!("resolve workspace root {}; {e}", root.display()))?;
        if !canonical_root.is_dir() {
            return Err(format!(
                "workspace root {} is not a directory",
                root.display()
            ));
        }
        let mut w = Workspace {
            root: canonical_root,
            allowed: Vec::new(),
        };
        for dir in allowed {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let dir = clean(&w.root.join(dir));
            let c = canonical(&dir)
                .map_err(|e| format!("resolve allowed folder {}; {e}", dir.display()))?;
            if !w.allowed.contains(&c) {
                w.allowed.push(c);
            }
        }
        Ok(w)
    }

    /// This workspace with one more allowed folder.
    pub fn with(&self, dir: &Path) -> Workspace {
        let mut w = self.clone();
        if let Ok(c) = canonical(&absolute(dir))
            && !w.allowed.contains(&c)
        {
            w.allowed.push(c);
        }
        w
    }

    /// A tool path as a canonical absolute path. Relative paths resolve from the root and `~`
    /// is the home folder. Symlinks are followed, so one pointing outside the allowed folders
    /// is rejected. The target itself need not exist.
    pub fn resolve(&self, p: &str) -> Result<PathBuf, String> {
        let p = if p.is_empty() { "." } else { p };
        let joined = clean(&self.root.join(super::expand(p)));
        let c = canonical(&joined).map_err(|e| format!("unable to resolve {p}; {e}"))?;
        if !self.contains(&c) {
            return Err(format!(
                "path is outside the allowed directories: {}",
                joined.display()
            ));
        }
        Ok(c)
    }

    /// Whether canonical path `p` is inside the root or an allowed folder.
    pub fn contains(&self, p: &Path) -> bool {
        p.starts_with(&self.root) || self.allowed.iter().any(|d| p.starts_with(d))
    }

    /// `p` relative to the root (with `/`) when inside it, else absolute.
    pub fn display(&self, p: &Path) -> String {
        match p.strip_prefix(&self.root) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
            Ok(rel) => slash(rel),
            Err(_) => p.display().to_string(),
        }
    }
}

/// The system's temp folders, canonical: the one the OS reports and, on Unix, `/tmp`.
pub fn temp_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![std::env::temp_dir()];
    if cfg!(unix) {
        dirs.push(PathBuf::from("/tmp"));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if let Ok(c) = canonical(&absolute(&d))
            && !out.contains(&c)
        {
            out.push(c);
        }
    }
    out
}

/// A relative path with `/` separators.
pub fn slash(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        return clean(p);
    }
    clean(&std::env::current_dir().unwrap_or_default().join(p))
}

/// `p` with `.` dropped and `..` applied, without touching the filesystem.
pub fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Resolves the symlinks of the longest existing prefix of absolute, clean `p` and joins the
/// missing rest onto it. A dangling symlink is followed to where its target would be.
pub fn canonical(p: &Path) -> io::Result<PathBuf> {
    canonical_depth(p, 0)
}

fn canonical_depth(p: &Path, depth: usize) -> io::Result<PathBuf> {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut current = p.to_path_buf();
    loop {
        match std::fs::canonicalize(&current) {
            Ok(resolved) => {
                let mut out = simplify(resolved);
                out.extend(rest.iter().rev());
                return Ok(out);
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            Err(e) => {
                if let Ok(target) = std::fs::read_link(&current) {
                    if depth > 40 {
                        return Err(e);
                    }
                    let parent = current.parent().unwrap_or(Path::new("/"));
                    let mut next = clean(&parent.join(target));
                    next.extend(rest.iter().rev());
                    return canonical_depth(&clean(&next), depth + 1);
                }
                let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else {
                    return Ok(p.to_path_buf());
                };
                rest.push(name.to_os_string());
                current = parent.to_path_buf();
            }
        }
    }
}

/// Drops Windows' `\\?\` prefix from canonical paths, so they compare with plain ones.
fn simplify(p: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return p;
    }
    let s = p.to_string_lossy();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}
