# opencode-dost

An [OpenCode](https://opencode.ai) V2 plugin that makes the [Dost](../../README.md) desktop dog
bark when OpenCode needs you:

| When | What the dog does |
|---|---|
| A tool needs your permission | Barks and shows **Allow / Always / Reject** buttons; your click goes back to OpenCode. Answer in the terminal instead and the bubble goes away. |
| OpenCode asks you a question | Barks and shows the question until you click it; answer it in OpenCode. |
| A task is running | Nothing on screen; the dog's **Agents** list shows the session working, with the current tool and step count. |
| The task finishes (after 10 s or more) | A bubble says how long it took, and the dog is happy. |
| A session fails | A bubble shows the error and the dog barks. |

Bubbles are labelled **OPENCODE** and carry the session title and folder. Sub-agent sessions stay
quiet; only the session you are looking at reports back.

## Install

Dost must be running with its MCP server on (the default, `http://127.0.0.1:7777/mcp`).

```sh
opencode plugin add 'github:rytsh/dost#main::path:integrations/opencode'
```

Or copy [`index.js`](index.js) to `~/.config/opencode/plugins/dost.js`. OpenCode picks up the plugin
without a restart.

## Options

All optional, in `opencode.json`:

```jsonc
{
  "plugins": [
    {
      "package": "github:rytsh/dost#main::path:integrations/opencode",
      "options": {
        "url": "http://127.0.0.1:7777/mcp",
        "question": true,
        "permissions": true,
        "progress": false,
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
| `url` | `http://127.0.0.1:7777/mcp` | Dost's MCP address (Settings → Connect in the dog's menu) |
| `question` | `true` | Bark on questions |
| `permissions` | `true` | Permission requests with answer buttons (`false`: no bubble) |
| `progress` | `false` | Also show a progress bar in the bubble while a task runs |
| `done` | `true` | Tell when a task finishes |
| `error` | `true` | Tell when a session fails |
| `minWorkSeconds` | `10` | Tasks shorter than this get no "done" bubble (and no progress bar) |
| `lang` | from `$LANG`, else `en` | `en` or `tr` |

## OpenCode in WSL, Dost on Windows

WSL's default (NAT) network cannot reach Windows' `127.0.0.1`. The plugin detects WSL and falls back to
Windows' own `curl.exe`, so it works without changes. Setting `networkingMode=mirrored` in
`%USERPROFILE%\.wslconfig` makes the direct connection work too.

## Troubleshooting

Start the OpenCode service with `DOST_DEBUG=1`; the plugin then appends what it sees and sends to
`/tmp/dost-opencode.log` (or the system temp folder).
