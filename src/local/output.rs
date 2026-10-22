//! Bounds tool output: text over the limit is cut to a preview and the full text is saved to
//! a file under the temp folder that `read` and `grep` can open.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The most lines and bytes a tool returns before cutting.
pub const MAX_LINES: usize = 2000;
pub const MAX_BYTES: usize = 50 * 1024;

/// Saved outputs older than this are removed when the store opens.
const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Copy, PartialEq)]
pub enum Direction {
    Head,
    Tail,
}

pub struct Store {
    dir: PathBuf,
    max_lines: usize,
    max_bytes: usize,
    seq: AtomicU64,
}

impl Store {
    pub fn new(dir: PathBuf, max_lines: usize, max_bytes: usize) -> Self {
        let _ = fs::create_dir_all(&dir);
        let dir = super::workspace::canonical(&dir).unwrap_or(dir);
        let s = Store {
            dir,
            max_lines,
            max_bytes,
            seq: AtomicU64::new(0),
        };
        s.cleanup();
        s
    }

    /// The default store: `<temp>/dost/tool-output`.
    pub fn default_dir() -> PathBuf {
        std::env::temp_dir().join("dost").join("tool-output")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn new_path(&self) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let n = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        self.dir.join(format!("tool_{nanos}_{n}"))
    }

    /// A new file in the store for streaming output into.
    pub fn create(&self) -> Result<(File, PathBuf), String> {
        let _ = fs::create_dir_all(&self.dir);
        let path = self.new_path();
        let mut o = OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        o.open(&path)
            .map(|f| (f, path))
            .map_err(|e| format!("create output file; {e}"))
    }

    /// The file's content when it fits the limits (removing the file unless `keep`); otherwise
    /// its last lines that fit, keeping the file so the full output stays available.
    pub fn tail_file(&self, path: &Path, keep: bool) -> io::Result<String> {
        let mut f = File::open(path)?;
        let size = f.metadata()?.len();
        let start = size.saturating_sub(self.max_bytes as u64);
        f.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        drop(f);
        let content = String::from_utf8_lossy(&buf).into_owned();
        let mut lines: Vec<&str> = content.split('\n').collect();
        if start == 0 && lines.len() <= self.max_lines {
            if !keep {
                let _ = fs::remove_file(path);
            }
            return Ok(content);
        }
        if start > 0 {
            // The first line is probably cut in the middle.
            lines.remove(0);
        }
        if lines.len() > self.max_lines {
            lines.drain(..lines.len() - self.max_lines);
        }
        let preview = lines.join("\n");
        let removed = size.saturating_sub(preview.len() as u64);
        Ok(format!(
            "...{removed} bytes truncated...\n\nThe output was truncated. Full output saved to: {}\n\
             Use grep to search the full content or read with offset/limit to view specific sections.\n\n{preview}",
            path.display()
        ))
    }

    /// Saves text to a new file in the store.
    pub fn write(&self, text: &str) -> Result<PathBuf, String> {
        let (mut f, path) = self.create()?;
        io::Write::write_all(&mut f, text.as_bytes()).map_err(|e| format!("save output; {e}"))?;
        Ok(path)
    }

    /// `text` unchanged when it fits; otherwise a preview from the head or tail with a hint
    /// pointing at the saved full text.
    pub fn truncate(&self, text: &str, direction: Direction) -> String {
        let lines: Vec<&str> = text.split('\n').collect();
        if lines.len() <= self.max_lines && text.len() <= self.max_bytes {
            return text.to_string();
        }
        let mut kept: Vec<&str> = Vec::new();
        let mut bytes = 0;
        let mut hit_byte = false;
        let order: Box<dyn Iterator<Item = &&str>> = match direction {
            Direction::Head => Box::new(lines.iter()),
            Direction::Tail => Box::new(lines.iter().rev()),
        };
        for line in order {
            if kept.len() >= self.max_lines {
                break;
            }
            let size = line.len() + usize::from(!kept.is_empty());
            if bytes + size > self.max_bytes {
                hit_byte = true;
                break;
            }
            kept.push(line);
            bytes += size;
        }
        if direction == Direction::Tail {
            kept.reverse();
        }
        let (removed, unit) = if hit_byte {
            (text.len() - bytes, "bytes")
        } else {
            (lines.len() - kept.len(), "lines")
        };
        let preview = kept.join("\n");
        let path = match self.write(text) {
            Ok(p) => p,
            Err(e) => {
                return format!(
                    "{preview}\n\n...{removed} {unit} truncated (full output could not be saved: {e})..."
                );
            }
        };
        let hint = format!(
            "The tool call succeeded but the output was truncated. Full output saved to: {}\n\
             Use grep to search the full content or read with offset/limit to view specific sections.",
            path.display()
        );
        match direction {
            Direction::Head => format!("{preview}\n\n...{removed} {unit} truncated...\n\n{hint}"),
            Direction::Tail => format!("...{removed} {unit} truncated...\n\n{hint}\n\n{preview}"),
        }
    }

    fn cleanup(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let cutoff = SystemTime::now() - RETENTION;
        for e in entries.flatten() {
            if !e.file_name().to_string_lossy().starts_with("tool_") {
                continue;
            }
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t < cutoff);
            if old {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}
