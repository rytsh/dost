//! The `read` tool: numbered, paged text; images and PDFs as MCP content; folder listings.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde_json::{Value, json};

use super::workspace::Workspace;

pub const MAX_LINES: usize = 2000;
pub const MAX_BYTES: usize = 50 * 1024;
const MAX_LINE_CHARS: usize = 2000;
const MAX_MEDIA_BYTES: u64 = 20 * 1024 * 1024;

const BINARY_EXTENSIONS: [&str; 30] = [
    "zip", "tar", "gz", "exe", "dll", "so", "class", "jar", "war", "7z", "doc", "docx", "xls",
    "xlsx", "ppt", "pptx", "odt", "ods", "odp", "bin", "dat", "obj", "o", "a", "lib", "wasm",
    "pyc", "pyo", "dylib", "rlib",
];

pub fn read(ws: &Workspace, path: &str, offset: i64, limit: i64) -> Result<Vec<Value>, String> {
    if offset < 0 || limit < 0 {
        return Err("offset and limit must be positive".into());
    }
    let offset = (offset.max(1)) as usize;
    let limit = match limit as usize {
        0 => MAX_LINES,
        l => l.min(MAX_LINES),
    };
    let abs = ws.resolve(path)?;
    let meta = match fs::metadata(&abs) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("file not found: {path}{}", suggest(ws, &abs)));
        }
        Err(e) => return Err(format!("unable to read {path}; {e}")),
    };
    if meta.is_dir() {
        return read_dir(ws, &abs, path, offset, limit).map(|t| vec![text(t)]);
    }
    if !meta.is_file() {
        return Err(format!("path is not a file or directory: {path}"));
    }
    read_file(&abs, path, offset, limit, meta.len())
}

pub fn text(s: impl Into<String>) -> Value {
    json!({ "type": "text", "text": s.into() })
}

fn read_dir(
    ws: &Workspace,
    abs: &Path,
    path: &str,
    offset: usize,
    limit: usize,
) -> Result<String, String> {
    let entries = fs::read_dir(abs).map_err(|e| format!("unable to list {path}; {e}"))?;
    let mut items: Vec<(bool, String)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(target) = fs::canonicalize(entry.path()) else {
            continue;
        };
        let target = super::workspace::canonical(&target).unwrap_or(target);
        if !ws.contains(&target) {
            continue;
        }
        let Ok(st) = fs::metadata(&target) else {
            continue;
        };
        if st.is_dir() {
            items.push((true, name + "/"));
        } else if st.is_file() {
            items.push((false, name));
        }
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let start = (offset - 1).min(items.len());
    let end = (start + limit).min(items.len());
    let mut b = String::new();
    for (_, name) in &items[start..end] {
        b.push_str(name);
        b.push('\n');
    }
    if items.is_empty() {
        b.push_str("(empty directory)\n");
    }
    if end < items.len() {
        b += &format!(
            "\n(Showing entries {}-{end} of {}. Use offset={} to continue.)",
            start + 1,
            items.len(),
            end + 1
        );
    } else if start > 0 || end > 0 {
        b += &format!("\n({} entries)", items.len());
    }
    Ok(b.trim_end_matches('\n').to_string())
}

fn media_type(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if head.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF8") {
        Some("image/gif")
    } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else if head.starts_with(b"%PDF") {
        Some("application/pdf")
    } else {
        None
    }
}

fn looks_binary(b: &[u8]) -> bool {
    if b.is_empty() {
        return false;
    }
    if b.contains(&0) {
        return true;
    }
    let odd = b.iter().filter(|&&c| c < 9 || (c > 13 && c < 32)).count();
    odd as f64 / b.len() as f64 > 0.3
}

fn read_file(
    abs: &Path,
    path: &str,
    offset: usize,
    limit: usize,
    size: u64,
) -> Result<Vec<Value>, String> {
    let err = |e: std::io::Error| format!("unable to read {path}; {e}");
    let f = fs::File::open(abs).map_err(err)?;
    let mut reader = BufReader::with_capacity(64 * 1024, f);
    let head = reader.fill_buf().map_err(err)?;
    let head = head[..head.len().min(8 * 1024)].to_vec();

    if let Some(mt) = media_type(&head) {
        if size > MAX_MEDIA_BYTES {
            return Err(format!(
                "media exceeds {MAX_MEDIA_BYTES} byte limit: {path}"
            ));
        }
        let mut data = Vec::new();
        reader.read_to_end(&mut data).map_err(err)?;
        let data = base64(&data);
        if mt.starts_with("image/") {
            return Ok(vec![
                text(format!("Image read successfully: {path}")),
                json!({ "type": "image", "data": data, "mimeType": mt }),
            ]);
        }
        return Ok(vec![
            text(format!("PDF read successfully: {path}")),
            json!({ "type": "resource", "resource": {
                "uri": format!("file://{}", abs.to_string_lossy().replace('\\', "/")),
                "mimeType": mt,
                "blob": data,
            }}),
        ]);
    }

    let ext = abs
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if BINARY_EXTENSIONS.contains(&ext.as_str()) || looks_binary(&head) {
        return Err(format!("cannot read binary file: {path}"));
    }

    let mut out = String::new();
    let (mut bytes, mut line, mut kept, mut next) = (0, 0, 0, 0);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if reader.read_until(b'\n', &mut raw).map_err(err)? == 0 {
            break;
        }
        line += 1;
        if line < offset {
            continue;
        }
        if kept >= limit {
            next = line;
            break;
        }
        let mut content: &[u8] = raw.strip_suffix(b"\n").unwrap_or(&raw);
        content = content.strip_suffix(b"\r").unwrap_or(content);
        if content.contains(&0) {
            return Err(format!("cannot read binary file: {path}"));
        }
        let Ok(content) = std::str::from_utf8(content) else {
            return Err(format!("file is not valid UTF-8: {path}"));
        };
        let entry = match content.char_indices().nth(MAX_LINE_CHARS) {
            Some((cut, _)) => format!(
                "{line}: {}... (line truncated to {MAX_LINE_CHARS} chars)\n",
                &content[..cut]
            ),
            None => format!("{line}: {content}\n"),
        };
        if bytes + entry.len() > MAX_BYTES {
            next = line;
            break;
        }
        out.push_str(&entry);
        bytes += entry.len();
        kept += 1;
    }

    if kept == 0 && offset > 1 {
        return Err(format!(
            "offset {offset} is out of range for this file ({line} lines)"
        ));
    }
    let mut out = out.trim_end_matches('\n').to_string();
    if next > 0 {
        out += &format!(
            "\n\n(Showing lines {offset}-{}. File has more lines; use offset={next} to continue.)",
            next - 1
        );
    } else if kept == 0 {
        out = "(empty file)".into();
    } else {
        out += &format!("\n\n(End of file - total {line} lines)");
    }
    Ok(vec![text(out)])
}

/// Up to three files in the parent folder with a similar name, so a typo is easy to fix.
fn suggest(ws: &Workspace, abs: &Path) -> String {
    let (Some(dir), Some(base)) = (abs.parent(), abs.file_name()) else {
        return String::new();
    };
    let base = base.to_string_lossy().to_lowercase();
    let Ok(entries) = fs::read_dir(dir) else {
        return String::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let hits: Vec<String> = names
        .into_iter()
        .filter(|n| {
            let n = n.to_lowercase();
            n.contains(&base) || base.contains(&n)
        })
        .take(3)
        .map(|n| ws.display(&dir.join(n)))
        .collect();
    if hits.is_empty() {
        return String::new();
    }
    format!("\n\nDid you mean one of these?\n{}", hits.join("\n"))
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
