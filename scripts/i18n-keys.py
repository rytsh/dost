#!/usr/bin/env python3
"""Lists the app's own text (the keys of locales/<code>.json) and reports the ones a
translation file lacks.

The keys are the English texts the code looks up:
  - the first argument of t("…"), tf("…", …), tf_in(code, "…", …), translate(code, "…");
  - every text literal of src/panel.rs outside its tests: the panel draws its labels, notes,
    options and buttons through t() (section, note, toggle_row, text_row, choice_row,
    dropdown_row, stepper_row, swatch_row, button, and the tables they are given);
  - the tables and labels other files hand to the panel: settings::SPEEDS / TEXT_SIZES,
    look::Wear::ALL / Theme::ALL, local::Approve::ALL / TIME_LIMITS, sessions' State::label,
    the platforms' autostart_label, render's CLEAR_ALL / SNOOZE and local's approval titles.

Usage: scripts/i18n-keys.py [code]      (default: tr; "all" checks every file)
       scripts/i18n-keys.py --list      (prints every key)
Exits 1 when keys are missing.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"
LOCALES = ROOT / "locales"

STR = r'"((?:[^"\\\n]|\\.)*)"'

# Literals in the panel that are not text: placeholders shown as typed, technical values,
# symbols and the like. Brand names stay as keys (a language may write them its own way).
NOT_TEXT = re.compile(
    r"""^(
        |[^A-Za-z]*                      # no letters: symbols, numbers, emoji, times
        |n|m|at|age|res|left|name|date|month|years|px|macos   # placeholder names, cfg values
        |.*\{[^}]*\}.*                   # format strings (not tf keys; those are found above)
        |https?://.*|\./.*|~.*|dost-…    # addresses, paths, topics
        |[0-9*/ -]+[0-9* -]+             # cron expressions
        |GET|YYYY-MM-DD|DD\.MM.*|HH:MM
        |.*[⌥⌘].*|Ctrl\+Alt.*            # keyboard shortcuts (not translated)
        |123456:ABC…
        |answered|expired|finished|missed|opened\ link|skipped   # History outcomes, matched
    )$""",
    re.X,
)


def unescape(s: str) -> str:
    return json.loads(f'"{s}"')


def strip_comments(code: str) -> str:
    # Line comments only; keep "//" inside strings ("https://").
    out = []
    for line in code.splitlines():
        in_str = False
        i = 0
        while i < len(line):
            c = line[i]
            if c == "\\" and in_str:
                i += 2
                continue
            if c == '"':
                in_str = not in_str
            elif not in_str and line.startswith("//", i):
                line = line[:i]
                break
            i += 1
        out.append(line)
    return "\n".join(out)


def without_tests(code: str) -> str:
    return code.split("#[cfg(test)]\nmod tests")[0]


def lookups(code: str) -> set[str]:
    keys = set()
    pats = [
        r"(?<![\w.])t\(\s*" + STR,
        r"(?<![\w.])tf\(\s*" + STR,
        r"(?<![\w.])tf_in\(\s*[^,]+,\s*" + STR,
        r"(?<![\w.])translate\(\s*[^,]+,\s*" + STR,
        r"(?<![\w.])tr\(\s*" + STR,  # linux.rs: tr = |en| translate(lang, en)
    ]
    for p in pats:
        keys.update(unescape(m) for m in re.findall(p, code))
    return keys


def block(code: str, start: str) -> str:
    """From `start` to the end of its first bracketed part (or its `;` for a one-line const)."""
    i = code.find(start)
    if i < 0:
        return ""
    depth = 0
    for j in range(i, len(code)):
        c = code[j]
        if c == ";" and depth == 0:
            return code[i:j]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0 and c == "}":
                return code[i : j + 1]
    return code[i:]


def text_literals(code: str) -> set[str]:
    keys = set()
    for m in re.finditer(STR, code):
        s = unescape(m.group(1))
        before = code[: m.start()].rstrip()
        if before.endswith("format!("):
            continue
        if NOT_TEXT.match(s):
            continue
        keys.add(s)
    return keys


def extract() -> set[str]:
    keys = set()
    for f in sorted(SRC.rglob("*.rs")):
        if f.name == "i18n.rs":
            continue
        keys |= lookups(strip_comments(f.read_text()))
    panel = strip_comments(without_tests((SRC / "panel.rs").read_text()))
    keys |= text_literals(panel)
    tables = [
        ("settings.rs", "pub const SPEEDS"),
        ("settings.rs", "pub const TEXT_SIZES"),
        ("look.rs", "pub const ALL: [(Wear"),
        ("look.rs", "pub const ALL: [(Theme"),
        ("local/mod.rs", "pub const ALL: [(Approve"),
        ("local/mod.rs", "pub const TIME_LIMITS"),
        ("sessions.rs", "pub fn label(self)"),
        ("render.rs", "const CLEAR_ALL"),
        ("render.rs", "const SNOOZE"),
        ("mac.rs", "fn autostart_label"),
        ("win.rs", "fn autostart_label"),
        ("linux.rs", "fn autostart_label"),
    ]
    for file, start in tables:
        code = strip_comments((SRC / file).read_text())
        part = block(code, start)
        if not part:
            sys.exit(f"{file}: {start!r} not found; update scripts/i18n-keys.py")
        keys.update(unescape(m) for m in re.findall(STR, part))
    # Approval titles ("Run a command?") are looked up with t(&a.title) in mcp.rs.
    local = strip_comments(without_tests((SRC / "local" / "mod.rs").read_text()))
    keys.update(
        unescape(m) for m in re.findall(STR + r"\.into\(\)", local) if m.endswith("?")
    )
    return {k for k in keys if k}


def main() -> int:
    args = sys.argv[1:]
    keys = extract()
    if args == ["--list"]:
        for k in sorted(keys):
            print(k)
        return 0
    want = args[0] if args else "tr"
    files = sorted(LOCALES.glob("*.json")) if want == "all" else [LOCALES / f"{want}.json"]
    bad = 0
    for f in files:
        have = json.loads(f.read_text())
        if want == "all" and not have:
            print(f"{f.stem}: empty (not translated yet)")
            continue
        missing = sorted(k for k in keys if not have.get(k))
        unused = sorted(k for k in have if k not in keys)
        print(f"{f.stem}: {len(keys)} keys, {len(missing)} missing, {len(unused)} unused")
        for k in missing:
            print(f"  missing: {json.dumps(k, ensure_ascii=False)}")
        for k in unused:
            print(f"  unused:  {json.dumps(k, ensure_ascii=False)}")
        bad += len(missing)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
