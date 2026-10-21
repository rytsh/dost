# opencode-dost

An [OpenCode](https://opencode.ai) V2 plugin that makes the [Dost](../../README.md) desktop dog
bark when OpenCode needs you:

| When | Bubble |
|---|---|
| OpenCode asks you a question | **Question:** *session title · folder* |
| A tool needs your permission | **Permission:** *session title · folder* |
| A task finishes (after at least 10 s of work) | **Done:** *session title · folder* |
| A session fails | **Error:** *session title · folder* |

Sub-agent sessions stay quiet; only the session you are looking at reports back.

## Install

Dost must be running with its MCP server on (the default, `http://127.0.0.1:7777/mcp`).

```sh
opencode plugin add 'github:rytsh/dost::path:integrations/opencode'
```

Or copy [`index.js`](index.js) to `~/.config/opencode/plugins/dost.js`. OpenCode picks up the plugin
without a restart.

## Options

All optional, in `opencode.json`:

```jsonc
{
  "plugins": [
    {
      "package": "github:rytsh/dost::path:integrations/opencode",
      "options": {
        "url": "http://127.0.0.1:7777/mcp",
        "question": true,
        "done": true,
        "error": true,
        "minWorkSeconds": 10,
        "lang": "en"
      }
    }
  ]
}
```

| Option | Default | Meaning |
|---|---|---|
| `url` | `http://127.0.0.1:7777/mcp` | Dost's MCP address (Settings → MCP in the dog's tray menu) |
| `question` | `true` | Bark on questions and permission requests |
| `done` | `true` | Bark when a task finishes |
| `error` | `true` | Bark when a session fails |
| `minWorkSeconds` | `10` | Ignore tasks shorter than this |
| `lang` | from `$LANG`, else `en` | `en` or `tr` |

## OpenCode in WSL, Dost on Windows

WSL's default (NAT) network cannot reach Windows' `127.0.0.1`. The plugin detects WSL and falls back to
Windows' own `curl.exe`, so it works without changes. Setting `networkingMode=mirrored` in
`%USERPROFILE%\.wslconfig` makes the direct connection work too.

## Troubleshooting

Start the OpenCode service with `DOST_DEBUG=1`; the plugin then appends what it sees and sends to
`/tmp/dost-opencode.log` (or the system temp folder).
