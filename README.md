<div align="center">

<h1><img src="art/px/sit/frames/frame-00.png" alt="" height="96" align="absmiddle"> Dost</h1>

**A pixel-art dog that lives on your taskbar and barks when your AI agents need you.**

Windows · macOS · single ~6 MB file · no account, no telemetry

</div>

*Dost* means "friend" in Turkish. It walks on your taskbar (or the Dock), and when something needs
your attention it barks and shows a speech bubble. Drag it around, click it to pet it, click the
bubble to dismiss it.

## Install

- **Windows**: download `dost.exe` from [Releases](https://github.com/rytsh/dost/releases) and run it.
- **macOS**: download `Dost.dmg` from [Releases](https://github.com/rytsh/dost/releases) and drag it
  into Applications. It is not notarized yet, so open it the first time with right click → *Open*.

## OpenCode

Bark when OpenCode asks a question, needs permission, finishes a task or fails:

```sh
opencode plugin add 'github:rytsh/dost::path:integrations/opencode'
```

Details in the [plugin's README](integrations/opencode/README.md).

## MCP

Dost serves MCP at `http://127.0.0.1:7777/mcp` (local only), so any AI assistant can use it:

```jsonc
// Claude Code, Cursor, ...
{ "mcpServers": { "dost": { "type": "http", "url": "http://127.0.0.1:7777/mcp" } } }

// OpenCode (opencode.json)
{ "mcp": { "dost": { "type": "remote", "url": "http://127.0.0.1:7777/mcp" } } }
```

Then ask things like *"tell me when localhost:3000 is up"* or *"bark when the build finishes"*.
Tools: `notify`, `add_watch`, `check_now`, `list_watches`, `remove_watch`, `status`.

> [!WARNING]
> Watches can run shell commands on your computer. Only connect clients you trust.

## Settings

Click the tray (menu bar) icon to change size, sound, walking, bubble duration, MCP and start at
login. Settings are saved to `%APPDATA%\dost\settings.json` (Windows) or
`~/.config/dost/settings.json` (macOS).

## Build

Requires [Rust](https://rustup.rs) 1.88+.

```sh
cargo build --release   # Windows or macOS
make app                # macOS app bundle
make build-windows      # Windows from WSL/Linux (needs cargo-xwin)
```

`make help` lists the rest. How the art was made: [art/README.md](art/README.md).

## License

[MIT](LICENSE)
