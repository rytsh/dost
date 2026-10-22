# Translating Dost

Dost is a pixel-art desktop dog that lives on the taskbar and barks when AI coding agents (OpenCode,
Claude Code, Codex) need the user. The text in `locales/<code>.json` maps the app's English text to
a translation. `locales/tr.json` (Turkish) is complete and is a good second reference.

## Rules

- Keep every placeholder exactly: `{n}`, `{name}`, `{age}`, `{at}`, `{m}`, `{left}`, `{res}`,
  `{years}`, `{date}`, `{month}`. Move them where the grammar needs them.
- Keep emoji, symbols and punctuation that carry meaning: `…`, `·`, `↗`, `✓`, `✗`, `🎂`, `💞`, `⭐`,
  `$`, `:`. Keep `MCP`, `ntfy`, `Telegram`, `OpenCode`, `Claude Code`, `Codex`, `cron`, `URL`,
  `WAV`, `@BotFather`, `/start`, `localhost`, `Dost`, `Atatürk`.
- The tone is friendly, short and plain, like a macOS or Windows settings window. Use the informal
  "you" where a language has one and apps commonly use it (de: du, fr: vous is also fine but be
  consistent, es: tú, it: tu, pt-BR: você, nl: je, pl: ty, ru/uk: ты is too casual for UI — use the
  neutral impersonal UI style, ja/ko/zh: neutral UI style).
- Labels are short: settings rows sit next to a switch or a choice; tab names must fit ~10 characters
  if possible ("Dog", "Look", "Play", "Sound", "Agents", "Todo", "History").
- Times: `{at}` is "09:00"; durations like "{n} min", "{n} h", "{n} s".
- Things the dog does: it barks (havlar), sits, sleeps, is petted, fetches its bone. "Bubble" is
  the dog's speech bubble.
- Days: "10 November" is the anniversary of Atatürk's death (remembrance, a minute of silence at
  09:05), "National holidays" are Türkiye's (23 April, 19 May, 30 August, 29 October). Keep the
  Turkish holiday names translated naturally (e.g. "Republic Day").
- Every key must have a translation; never leave a value empty or English unless it is a proper
  name or the English word is what that language's UIs use (e.g. "Token", "Server" in some).
- Output: valid JSON, keys sorted, 2-space indent, UTF-8 (not \u escapes), newline at the end.

Check with `python3 scripts/i18n-keys.py <code>` (missing keys) and `cargo test i18n`
(placeholders).

## Contributing

The new translations are AI-assisted; native-speaker corrections are welcome. Edit only the
values in your language's JSON file, keeping the English keys and placeholders unchanged.
`zh-TW` uses Traditional Chinese and Taiwan vocabulary; `zh-CN` uses Simplified Chinese and
mainland vocabulary. They are separate translations, not character conversions.

For new UI text, use `i18n::t("English text")` or `i18n::tf("{n} items", &[("n", value)])`.
Add the same key to every locale, then run:

```sh
python3 scripts/i18n-keys.py all
cargo test i18n
cargo run --example preview target/preview.png
```

English is the source text in code, so there is no `en.json`. Missing translations fall back to
English at runtime; CI rejects missing or empty entries. The translation layer currently has one
form per key, not a general plural-rules engine; use count-neutral phrasing or abbreviations when
needed. Do not translate text supplied by agents or users (names, commands, notification bodies).

Arabic glyphs are shaped right to left, but the settings layout remains left to right. System
fonts provide the glyphs; Linux users may need Noto CJK/Arabic packages. This is not a complete
RTL or locale-specific date/time implementation.
