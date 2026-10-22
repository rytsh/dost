<div align="center">

<h1><img src="art/px/sit/frames/frame-00.png" alt="" height="96" align="absmiddle"> Dost</h1>

**A pixel-art dog that lives on your taskbar and barks when your AI agents need you.**

Windows · macOS · Linux · single ~6 MB file · no account, no telemetry

</div>

*Dost* means "friend" in Turkish. It walks on your taskbar (or the Dock), and when something needs
your attention it barks and shows a speech bubble. Drag it around, click it to pet it, click the
bubble to dismiss it.

## Install

- **Windows**: download `dost.exe` from [Releases](https://github.com/rytsh/dost/releases) and run it.
- **macOS**: download `Dost.dmg` from [Releases](https://github.com/rytsh/dost/releases) and drag it
  into Applications. It is not notarized yet, so open it the first time with right click → *Open*.
- **Linux** (X11, or Wayland through XWayland): download `dost-linux-x86_64` from
  [Releases](https://github.com/rytsh/dost/releases), `chmod +x` it and run it. The tray icon needs a
  StatusNotifierItem host (KDE, most panels; on GNOME the AppIndicator extension), otherwise right-click
  the dog for settings. Sound plays through `paplay`, `pw-play` or `aplay`; copy and paste use `wl-copy`
  / `wl-paste` or `xclip` / `xsel`.

## OpenCode

Bark when OpenCode asks a question or needs permission (answer it from the bubble), and tell when a task finishes or fails; the **Agents** list shows what each session is doing:

```sh
opencode plugin add 'github:rytsh/dost#main::path:integrations/opencode'
```

Details in the [plugin's README](integrations/opencode/README.md).

## Claude Code and Codex

Permission requests show **Allow / Always / Deny** in the dog's bubble and the click answers the
agent; the dog also barks when it waits for you or finishes its turn. Add to
`~/.claude/settings.json`:

```json
{
  "hooks": {
    "PermissionRequest": [{ "hooks": [{ "type": "command", "command": "dost hook claude", "timeout": 600 }] }],
    "Notification": [{ "matcher": "idle_prompt|elicitation_dialog", "hooks": [{ "type": "command", "command": "dost hook claude" }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "dost hook claude" }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "dost hook claude" }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "dost hook claude" }] }]
  }
}
```

`UserPromptSubmit` and `SessionEnd` are optional: with them each Claude Code session shows in the
dog's **Agents** list as working, waiting for you, or gone.

Codex (`~/.codex/config.toml`): `notify = ["dost", "hook", "codex"]`, and for permission buttons
`~/.codex/hooks.json`:
`{ "hooks": { "PermissionRequest": [{ "hooks": [{ "type": "command", "command": "dost hook codex", "timeout": 600 }] }] } }`

`dost` must be on your `PATH` (on macOS: `ln -s /Applications/Dost.app/Contents/MacOS/dost /usr/local/bin/dost`).

## Command line

The same `dost` binary talks to the running dog:

```sh
dost run cargo build --release      # runs it, then barks: done or failed, and how long it took
make test; DOST_STATUS=$? dost done "Tests"
dost notify "Deploy finished" "api is live" --url https://example.com
dost ask "Deploy to prod?" --choices Yes,No   # prints the answer
dost remind 15:00 "Join the call"
```

`dost help` lists everything.

## MCP

Dost serves MCP at `http://127.0.0.1:7777/mcp` (local only), so any AI assistant can use it:

```jsonc
// Claude Code, Cursor, ...
{ "mcpServers": { "dost": { "type": "http", "url": "http://127.0.0.1:7777/mcp" } } }

// OpenCode (opencode.json)
{ "mcp": { "dost": { "type": "remote", "url": "http://127.0.0.1:7777/mcp" } } }
```

Then ask things like *"tell me when localhost:3000 is up"*, *"bark when the build finishes"*,
*"remind me at 15:00 to join the call"* or *"make a todo list for the release, and remind me about
the notes tomorrow at 10"*.

**From a browser**: web pages on `localhost` can always call it. For a browser-based MCP client on
another address, set settings → Connect → *Browsers* to **Any**, or list the pages in
`settings.json`: `"mcp_cors": { "allow_origins": ["https://app.example.com", "https://*.example.com"], "allow_private_network": true }`
(*allow_private_network* answers Chrome's preflight for a public page reaching this machine). Any page
that is allowed can make the dog bark, ask you questions and add watches, including `command`
watches that run shell commands, so only allow pages you trust.

| Tool | What the dog does |
| --- | --- |
| `notify` | Barks and shows a bubble. Optional `url` (opened on click), `copy` (put on the clipboard), `source` (who sent it), `mood` (`good`: the dog is happy, `bad`: it is sad and howls; guessed from the words when left out). |
| `ask` · `dismiss` | Shows a question with up to 4 answer buttons and waits for your click; the agent gets the answer. With `allow_text` the bubble also has *Reply…*, to answer in your own words. `dismiss` withdraws it. |
| `progress` | A progress bar for a long task, updated in place; the dog is happy when it finishes, barks if it fails. |
| `remind` | Barks later: after some minutes or at a local time. |
| `add_watch` | Checks something every few seconds and barks when it changes: a `url`, a `port`, a `file` (or text in it), a process (`pid`), a GitHub Actions run (`github`: `owner/repo` or a run URL; set `GITHUB_TOKEN` for private repos), or a shell `command`. |
| `react` | Happy, sad, bark, fetch (throws the bone), sleep, wake or zoomies. |
| `copy` | Puts text on the clipboard and says so. |
| `status` · `history` | What the dog is doing, how long you have been idle, and what you may have missed. |
| `day_add` · `day_list` · `day_remove` | The user's special days: birthdays, anniversaries (see Special days). |
| `cron_add` · `cron_list` · `cron_remove` · `cron_run` · `cron_enable` | Repeating jobs on a cron schedule: reminders, HTTP requests and commands (see Schedule). |
| `session` · `sessions` | An agent says what its session is doing (working, waiting, done, failed); you follow them all in **Agents**. `notify`, `ask` and `progress` take a `session` too. |
| `check_now` · `list_watches` · `remove_watch` | Manage watches and reminders. |
| `todo_lists` · `todo_create_list` · `todo_add` · `todo_update` · `todo_remove` | Todo lists the dog keeps for you: a list per piece of work, items to check off, and a reminder per item (the dog barks it unless it is done by then). |

**Todo** (settings → Todo): your lists, the same ones agents see. Click an item to check it off, ✕ to
remove it, type into *Add* for a new item or *New list* for a list. Saved to `todos.json` next to
`settings.json`.

**Quiet** (settings → Sound): *Quiet now* until you turn it off, or every day *From*–*To* (e.g.
22:00–08:00; the same time twice means no quiet hours). It is also quiet
while an app is full screen (a video, a presentation), unless you turn that off. While quiet the dog
does not bark and only shows questions, progress and sticky messages; the rest goes to *History*.

> [!WARNING]
> Watches can run shell commands on your computer. Only connect clients you trust.

## Schedule

Settings → **Schedule** runs jobs on a cron schedule, in your local time: a **reminder** (the dog
barks), an HTTP **request** (a health check, a webhook) or a **command** (a backup script). Pick a
quick schedule (hourly, daily at 9, weekdays, Mondays) or write a cron expression
(`*/15 9-17 * * 1-5`: every 15 minutes, 9 to 17, Monday to Friday; or `@daily`). Requests and
commands bark when they fail (or every time, if you like), and every run goes to History. Each job
can be paused, run now or removed. Agents add jobs with the MCP tools `cron_add`, `cron_list`,
`cron_remove`, `cron_run` and `cron_enable`. Runs missed while Dost was not running are not made
up.

## Workspace tools

Off by default. Settings → Connect → *Workspace tools* serves file and shell tools for agents at
`http://127.0.0.1:7777/mcp/local`, separately from the dog's own `/mcp`: `read`, `glob`, `grep`,
`edit`, `write`, `patch`, `shell` and `shell_job`, adapted from
[local-mcp](https://github.com/rakunlabs/local-mcp). They work in the chosen *Folder* (your home
folder when empty), the folders listed under it, and the system's temp folder; *Read only* leaves
only `read`, `glob` and `grep`. `glob` and `grep` use ripgrep when it is installed.

- **The dog asks first.** Before a command runs or a file changes, the dog barks with
  *Allow / Always / Deny* (on your phone too, while you are away). *Always* remembers the command
  (`cargo test*`) or the folder; the rules are listed in the settings, with ✕ to forget one.
  A command that chains others (`;`, `&&`, `|`) always asks. *Ask first* can be set to only
  commands, or never.
- **Time limit.** Commands stop after the chosen time (2 minutes by default, or none); an agent may
  ask for less, not more.
- **Live output.** Clients that accept an event stream (`Accept: text/event-stream`) get a
  command's output as `notifications/progress` while it runs. A command that takes longer than a
  few seconds also shows in the bubble as a progress bar with its latest line.
- **History.** Every command and change goes to History with who asked, how it ended and how long
  it took (also `history` over MCP).

```jsonc
// OpenCode (opencode.json)
{ "mcp": { "dost-local": { "type": "remote", "url": "http://127.0.0.1:7777/mcp/local" } } }
```

> [!WARNING]
> `shell` runs commands as you, and there is no token: while this is on, any program on this
> computer, and any web page the *Browsers* setting allows, can use it.

## Agents

Settings → **Agents** lists every agent session the dog knows about: what it works on, what it is
doing now, and how long for. Waiting ones come first (with a red count on the tab). The OpenCode
plugin and the Claude Code / Codex hooks report sessions by themselves; other agents use the MCP
`session` tool. Finished ones go after half an hour, or with *Clear done*.

## On your phone

Settings → **Phone** sends notifications to your phone while you are away (idle for 5 minutes or
the screen is locked), always, or never. Questions keep their buttons there, so you can allow a
command from the couch. Nothing listens on a port: Dost only makes outgoing HTTPS requests.

- **ntfy**: turn it on and a random topic is made. Subscribe to it in the
  [ntfy app](https://ntfy.sh) (*Copy link*). Works with ntfy.sh or your own server (with an access
  token if it is protected). Anyone who knows the topic can read and answer, so keep it secret.
- **Telegram**: make a bot with [@BotFather](https://t.me/BotFather), paste its token and send
  `/start` to the bot: the dog pairs with that chat. Answer with the buttons, or reply to a question
  in your own words when the agent allows it.

*Send a test* checks the setup.

## Special days

Settings → **Special days** (a page of its own in the settings window). They work the same in
every language; only the text follows it.

- **Built in**, each with its own switch:
  - **10 November**: the dog wears a black ribbon all day in memory of Atatürk. At **09:05** it
    stops, sits still and stays silent for a minute (no barking, notifications wait), then a quiet
    bubble remembers him. *Siren at 09:05* adds the siren (off by default).
  - **23 April, 19 May, 30 August, 29 October**: a little Turkish flag on the collar and a greeting.
  - **The dog's birthday** (party hat), **Halloween** (witch hat, pumpkin), **Christmas and New
    Year** (Santa hat).
- **Your days**: birthdays, anniversaries, holidays of your own. On the day the dog shows a bubble
  at 09:00 (*"🎂 Ayşe is 36 today!"* when you give the year), wears a party hat on birthdays, a
  heart on anniversaries or a star on other days, and can remind you days before and send it to
  your phone. Every year, or only once. Agents add them with `day_add` (`day_list`,
  `day_remove`): *"add Ayşe's birthday, 14 March 1990, remind me a day before"*.

## Make it yours

Settings → **Look**:

- **Name**: click it and type. The name shows in the panel and is told to AI assistants over MCP,
  so you can say *"tell Karabaş when the build is done"*. The dog's birthday is the day you first
  started it; on that day it wears a party hat.
- **Fur** and **collar** colours (or no collar), and what it **wears**: a beanie.
- **Holiday outfits**: a witch hat (and a pumpkin instead of the bone) 24–31 October, a Santa hat
  20 December – 1 January.
- **Bubble**: Classic, Dark, Sky or Mint. Each `source` gets a colour of its own; pick one with
  `"source_colors": { "ci": "#2f6fc4" }` in `settings.json`.

**Fetch**: a bone lies next to the dog. Drag it and fling it; the dog runs after it and brings it
back (just picking it up and letting go moves it without a game). Now and then the dog plays with
it by itself. In settings → **Play**, click the bone to throw it for you (or `dost react fetch`), or
drag it out of the panel to pick it up and throw it yourself. Right-click the bone to put it away;
*Out to play with* turns it on and off too.

**Moods**: good news (tests passed, done) makes the dog happy, failures make it sad. With *Happy,
sad and asking voices* on (settings → Sound) it also sounds different: a double yip, a little howl,
a short "wuf?" for questions.

**Your own bark**: list 16-bit PCM WAV files in `settings.json`; one is picked at random each time:
`"bark_files": ["~/sounds/woof1.wav", "~/sounds/woof2.wav"]`.

## Settings

Click the tray (menu bar) icon for the quick panel; its ⚙ (or *All settings…* in the right-click
menu) opens the **settings window**, with a sidebar of pages: General (language, which screen the
dog lives on), Dog, Look, Play, Special days, Sound, Phone, Agents, Todo, Schedule, History and
Connect.
**Language** is a drop-down in General: follow the system or choose English, Türkçe, Deutsch,
Français, Español, Italiano, Português (Brasil), Nederlands, Polski, Русский, Українська,
العربية, Bahasa Indonesia, 日本語, 한국어, 繁體中文 (Taiwan) or 简体中文. The list scrolls;
arrow keys and Enter select, Esc cancels. Long choices elsewhere also become drop-downs when
they cannot fit in one row.

Translations are built into the app. See [locales/README.md](locales/README.md) to review or
improve one. The new translations are AI-assisted and welcome native-speaker review. Arabic
text is shaped right to left, but the settings layout is not mirrored yet. CJK and Arabic need
appropriate system fonts; on Linux, install the matching Noto font packages if glyphs are missing.

Click the tray (menu bar) icon for settings. Everything in `settings.json` can be changed there:
size and text, the dog's name, birthday and look, source colours, the toy (**Play**), walking and
the offset (on **Dog**), sounds, quiet hours and your own bark files, the MCP address and which web pages may use
it, start at login and updates. Long tabs scroll; fields save on Enter or when you click elsewhere
(Esc cancels), and paste works in them. Settings are saved to `%APPDATA%\dost\settings.json`
(Windows) or `~/.config/dost/settings.json` (macOS, Linux); edits to the file show up in the panel too.

**Hide** sends the dog off the right edge of the screen. While it is away, questions, progress and
sticky messages still pop up in a bubble next to the tray icon; other notifications bark, go to
*History* and put a red mark on the icon. **Show** brings it back.

**Sleep** puts the dog to sleep: it stops barking and only questions, progress and sticky messages
are shown; the rest goes quietly to *History*. It works while hidden too. Click the dog or **Wake**
to wake it up.

Hidden and asleep are remembered: the dog starts the same way next time, and so is *History*.

**Keyboard**: while a bubble is up, <kbd>⌥⌘↩</kbd> (Windows, Linux: <kbd>Ctrl+Alt+Enter</kbd>) picks the
first answer or closes it, <kbd>⌥⌘1</kbd>–<kbd>4</kbd> pick an answer, <kbd>⌥⌘⌫</kbd> the last one
("no"), and <kbd>⌥⌘L</kbd> means *Later*: the bubble comes back in 10 minutes (also a link in the
bubble).

When you come back after 5 minutes away, a single bubble sums up what happened meanwhile. If the
screen was locked, important news also shows up as a system notification when you unlock.

Dost updates itself from GitHub Releases (settings → Connect → *Update automatically*); click the
version at the bottom of the settings to check now. See [packaging/README.md](packaging/README.md).

## Build

Requires [Rust](https://rustup.rs) 1.88+.

```sh
cargo build --release   # Windows, macOS or Linux
make app                # macOS app bundle
make build-windows      # Windows from WSL/Linux (needs cargo-xwin)
```

`make help` lists the rest. How the art was made: [art/README.md](art/README.md). The app icon (a
paw) is drawn by `examples/icon.rs`; `make icons` rebuilds `AppIcon.icns` and `dost.ico`, and
`build.rs` puts the icon into `dost.exe`.

## License

[MIT](LICENSE)
