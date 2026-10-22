//! The `*** Begin Patch` file-oriented patch format of OpenCode's patch tool: parsing, and
//! applying update chunks to a file's text.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Add,
    Delete,
    Update,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub kind: Kind,
    pub path: String,
    pub move_path: String,
    pub contents: String,
    pub chunks: Vec<Chunk>,
}

#[derive(Clone, Debug, Default)]
pub struct Chunk {
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub change_context: String,
    pub end_of_file: bool,
}

/// The patch inside a `<<EOF … EOF` heredoc (optionally after `cat`), or `s` as is.
fn strip_heredoc(s: &str) -> &str {
    let body = s.strip_prefix("cat").map(str::trim_start).unwrap_or(s);
    let Some(rest) = body.strip_prefix("<<") else {
        return s;
    };
    let rest = rest.trim_start_matches(['\'', '"']);
    let Some((first, inner)) = rest.split_once('\n') else {
        return s;
    };
    let tag = first.trim().trim_end_matches(['\'', '"']);
    if tag.is_empty() || !tag.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return s;
    }
    match inner.rsplit_once('\n') {
        Some((patch, last)) if last.trim() == tag => patch,
        _ => s,
    }
}

pub fn parse(text: &str) -> Result<Vec<Hunk>, String> {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = strip_heredoc(text.trim()).split('\n').collect();
    let begin = lines.iter().position(|l| l.trim() == "*** Begin Patch");
    let end = lines.iter().rposition(|l| l.trim() == "*** End Patch");
    let (Some(begin), Some(end)) = (begin, end) else {
        return Err("invalid patch format: missing *** Begin Patch / *** End Patch markers".into());
    };
    if begin >= end {
        return Err("invalid patch format: missing *** Begin Patch / *** End Patch markers".into());
    }

    let mut hunks = Vec::new();
    let mut i = begin + 1;
    while i < end {
        let line = lines[i];
        if let Some(path) = line.strip_prefix("*** Add File:") {
            let path = path.trim();
            if path.is_empty() {
                return Err("invalid add file path".into());
            }
            let mut content = Vec::new();
            i += 1;
            while i < end && !lines[i].starts_with("***") {
                let Some(l) = lines[i].strip_prefix('+') else {
                    return Err(format!(
                        "invalid add file line (must start with +): {}",
                        lines[i]
                    ));
                };
                content.push(l);
                i += 1;
            }
            hunks.push(Hunk {
                kind: Kind::Add,
                path: path.into(),
                move_path: String::new(),
                contents: content.join("\n"),
                chunks: Vec::new(),
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File:") {
            let path = path.trim();
            if path.is_empty() {
                return Err("invalid delete file path".into());
            }
            hunks.push(Hunk {
                kind: Kind::Delete,
                path: path.into(),
                move_path: String::new(),
                contents: String::new(),
                chunks: Vec::new(),
            });
            i += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File:") {
            let path = path.trim();
            if path.is_empty() {
                return Err("invalid update file path".into());
            }
            let mut move_path = String::new();
            i += 1;
            if i < end
                && let Some(m) = lines[i].strip_prefix("*** Move to:")
            {
                move_path = m.trim().into();
                if move_path.is_empty() {
                    return Err("invalid move file path".into());
                }
                i += 1;
            }
            let (chunks, next) = parse_update(&lines, i, end)?;
            if chunks.is_empty() && move_path.is_empty() {
                return Err(format!(
                    "invalid update hunk for {path}: expected at least one @@ chunk"
                ));
            }
            hunks.push(Hunk {
                kind: Kind::Update,
                path: path.into(),
                move_path,
                contents: String::new(),
                chunks,
            });
            i = next;
        } else if line.trim().is_empty() {
            i += 1;
        } else {
            return Err(format!("invalid patch line: {line}"));
        }
    }
    Ok(hunks)
}

fn parse_update(lines: &[&str], mut i: usize, end: usize) -> Result<(Vec<Chunk>, usize), String> {
    let mut chunks = Vec::new();
    while i < end && !lines[i].starts_with("***") {
        let Some(context) = lines[i].strip_prefix("@@") else {
            return Err(format!(
                "invalid update file line (expected @@): {}",
                lines[i]
            ));
        };
        let mut c = Chunk {
            change_context: context.trim().into(),
            ..Chunk::default()
        };
        i += 1;
        while i < end && !lines[i].starts_with("@@") {
            let line = lines[i];
            if line == "*** End of File" {
                c.end_of_file = true;
                i += 1;
                break;
            }
            if line.starts_with("***") {
                break;
            }
            if let Some(l) = line.strip_prefix(' ') {
                c.old_lines.push(l.into());
                c.new_lines.push(l.into());
            } else if let Some(l) = line.strip_prefix('-') {
                c.old_lines.push(l.into());
            } else if let Some(l) = line.strip_prefix('+') {
                c.new_lines.push(l.into());
            } else if line.is_empty() {
                // A blank context line whose leading space was stripped.
                c.old_lines.push(String::new());
                c.new_lines.push(String::new());
            } else {
                return Err(format!("invalid update chunk line: {line}"));
            }
            i += 1;
        }
        chunks.push(c);
    }
    Ok((chunks, i))
}

/// Applies update chunks to the original text (without BOM). The result ends with a newline
/// and keeps the original line endings.
pub fn derive(path: &str, chunks: &[Chunk], original: &str) -> Result<String, String> {
    let ending = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let normalized = original.replace("\r\n", "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(String::from).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    // (start, lines removed, lines inserted), in file order.
    let mut reps: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut idx = 0;
    for c in chunks {
        if !c.change_context.is_empty() {
            let Some(at) = seek(&lines, std::slice::from_ref(&c.change_context), idx, false) else {
                return Err(format!(
                    "failed to find context {:?} in {path}",
                    c.change_context
                ));
            };
            idx = at + 1;
        }
        if c.old_lines.is_empty() {
            reps.push((lines.len(), 0, c.new_lines.clone()));
            continue;
        }
        let mut old = &c.old_lines[..];
        let mut new = &c.new_lines[..];
        let mut found = seek(&lines, old, idx, c.end_of_file);
        if found.is_none() && old.last().is_some_and(String::is_empty) {
            old = &old[..old.len() - 1];
            if new.last().is_some_and(String::is_empty) {
                new = &new[..new.len() - 1];
            }
            found = seek(&lines, old, idx, c.end_of_file);
        }
        let Some(found) = found else {
            return Err(format!(
                "failed to find expected lines in {path}:\n{}",
                c.old_lines.join("\n")
            ));
        };
        reps.push((found, old.len(), new.to_vec()));
        idx = found + old.len();
    }
    for (start, remove, insert) in reps.into_iter().rev() {
        lines.splice(start..start + remove, insert);
    }
    Ok(lines.join(ending) + ending)
}

fn seek(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() || pattern.len() > lines.len() {
        return None;
    }
    let cmps: [fn(&str, &str) -> bool; 4] = [
        |a, b| a == b,
        |a, b| a.trim_end_matches([' ', '\t']) == b.trim_end_matches([' ', '\t']),
        |a, b| a.trim() == b.trim(),
        |a, b| normalize(a.trim()) == normalize(b.trim()),
    ];
    let last = lines.len() - pattern.len();
    let at = |off: usize, cmp: fn(&str, &str) -> bool| {
        pattern
            .iter()
            .enumerate()
            .all(|(i, p)| cmp(&lines[off + i], p))
    };
    for cmp in cmps {
        if eof && last >= start && at(last, cmp) {
            return Some(last);
        }
        if let Some(off) = (start..=last).find(|&off| at(off, cmp)) {
            return Some(off);
        }
    }
    None
}

/// Typographic quotes, dashes, ellipses and non-breaking spaces as plain ASCII.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => out.push('\''),
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => out.push('"'),
            '\u{2010}'..='\u{2015}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            '\u{a0}' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_derive() {
        let text = "*** Begin Patch
*** Add File: new.txt
+hello
+world
*** Update File: app.py
*** Move to: main.py
@@ def greet():
-    print(\"Hi\")
+    print(\"Hello\")
*** Delete File: old.txt
*** End Patch";
        let hunks = parse(text).unwrap();
        assert_eq!(hunks.len(), 3);
        assert_eq!(hunks[0].kind, Kind::Add);
        assert_eq!(hunks[0].contents, "hello\nworld");
        assert_eq!(hunks[1].kind, Kind::Update);
        assert_eq!(hunks[1].move_path, "main.py");
        assert_eq!(hunks[1].chunks[0].change_context, "def greet():");
        assert_eq!(hunks[2].kind, Kind::Delete);
        assert_eq!(hunks[2].path, "old.txt");

        let got = derive(
            "app.py",
            &hunks[1].chunks,
            "x = 1\ndef greet():\n    print(\"Hi\")\n",
        )
        .unwrap();
        assert_eq!(got, "x = 1\ndef greet():\n    print(\"Hello\")\n");
    }

    #[test]
    fn derive_keeps_crlf_and_fuzzy_matches() {
        let chunks = [Chunk {
            old_lines: vec!["a = “x”".into(), "b".into()],
            new_lines: vec!["a = 1".into(), "b".into()],
            ..Chunk::default()
        }];
        let got = derive("f", &chunks, "a = \"x\"  \r\nb\r\n").unwrap();
        assert_eq!(got, "a = 1\r\nb\r\n");
    }

    #[test]
    fn derive_end_of_file_and_missing() {
        let chunks = [Chunk {
            old_lines: vec!["x".into()],
            new_lines: vec!["y".into()],
            end_of_file: true,
            ..Chunk::default()
        }];
        assert_eq!(derive("f", &chunks, "x\na\nx\n").unwrap(), "x\na\ny\n");

        let missing = [Chunk {
            old_lines: vec!["nope".into()],
            new_lines: vec!["x".into()],
            ..Chunk::default()
        }];
        let err = derive("f", &missing, "a\n").unwrap_err();
        assert!(err.contains("failed to find"), "{err}");
    }

    #[test]
    fn parse_rejects() {
        for text in [
            "no markers",
            "*** Begin Patch\n*** Add File: a\nmissing plus\n*** End Patch",
            "*** Begin Patch\n*** Update File: a\n*** End Patch",
            "*** Begin Patch\ngarbage\n*** End Patch",
        ] {
            assert!(parse(text).is_err(), "expected an error for {text:?}");
        }
    }

    #[test]
    fn heredoc() {
        let hunks =
            parse("cat <<'EOF'\n*** Begin Patch\n*** Delete File: a\n*** End Patch\nEOF").unwrap();
        assert_eq!(hunks.len(), 1);
        let hunks =
            parse("<<EOF\n*** Begin Patch\n*** Delete File: a\n*** End Patch\nEOF").unwrap();
        assert_eq!(hunks.len(), 1);
    }
}
