// opencode-dost: barks the Dost desktop dog when OpenCode asks you something,
// needs a permission, finishes a task or fails. OpenCode V2 plugin.
//
// Install:  opencode plugin add 'github:rytsh/dost::path:integrations/opencode'
// Options (opencode.json "plugins" entry, all optional):
//   url ("http://127.0.0.1:7777/mcp"), question (true), done (true), error (true),
//   minWorkSeconds (10), lang ("en" | "tr")
// Debug:    start the OpenCode service with DOST_DEBUG=1; events go to <tmp>/dost-opencode.log

import { execFile } from "node:child_process"
import { appendFileSync, existsSync, readFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const DEFAULT_URL = "http://127.0.0.1:7777/mcp"
const WIN_CURL = "/mnt/c/Windows/System32/curl.exe"

function isWsl() {
  if (process.platform !== "linux") return false
  if (process.env.WSL_DISTRO_NAME || process.env.WSL_INTEROP) return true
  if (existsSync("/proc/sys/fs/binfmt_misc/WSLInterop")) return true
  try {
    return /microsoft|wsl/i.test(readFileSync("/proc/version", "utf8"))
  } catch {
    return false
  }
}

// OpenCode loads one plugin instance per location and every instance receives the same
// events. A process-wide record makes sure each event is handled once.
const shared = (globalThis.__dostNotify ??= { seen: new Map() })
function firstTime(key) {
  const now = Date.now()
  if (shared.seen.size > 500) {
    for (const [k, t] of shared.seen) if (now - t > 60_000) shared.seen.delete(k)
  }
  if (shared.seen.has(key)) return false
  shared.seen.set(key, now)
  return true
}

// With DOST_DEBUG=1, events and errors are appended to <tmp>/dost-opencode.log.
const DEBUG = Boolean(process.env.DOST_DEBUG)
function log(...parts) {
  const line = `${new Date().toISOString()} ${parts.join(" ")}`
  if (DEBUG) {
    try {
      appendFileSync(join(tmpdir(), "dost-opencode.log"), line + "\n")
    } catch {}
  }
  console.error(`[dost] ${parts.join(" ")}`)
}

function rpcBody(title, message) {
  return JSON.stringify({
    jsonrpc: "2.0",
    id: Date.now(),
    method: "tools/call",
    params: { name: "notify", arguments: { title, message } },
  })
}

async function sendHttp(url, body) {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json, text/event-stream" },
    body,
    signal: AbortSignal.timeout(3000),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
}

// WSL in NAT mode cannot reach Windows' 127.0.0.1; Windows' own curl.exe can.
function sendViaWindowsCurl(url, body) {
  return new Promise((resolve, reject) => {
    const child = execFile(
      WIN_CURL,
      ["-s", "-f", "-m", "5", "-H", "Content-Type: application/json", "--data-binary", "@-", url],
      { timeout: 8000, windowsHide: true },
      (err) => (err ? reject(err) : resolve()),
    )
    child.stdin.end(body)
  })
}

function short(text, max = 140) {
  const t = String(text ?? "").replace(/\s+/g, " ").trim()
  return t.length > max ? `${t.slice(0, max - 1)}…` : t
}

const TEXT = {
  en: {
    question: (s) => `Question: ${s}`,
    permission: (s) => `Permission: ${s}`,
    done: (s) => `Done: ${s}`,
    error: (s) => `Error: ${s}`,
    waiting: "Waiting for your answer.",
    wants: (what) => `Wants to ${what}`,
    yourTurn: "Finished, your turn.",
    failed: "The session stopped with an error.",
  },
  tr: {
    question: (s) => `Soru: ${s}`,
    permission: (s) => `İzin: ${s}`,
    done: (s) => `Bitti: ${s}`,
    error: (s) => `Hata: ${s}`,
    waiting: "Senden bir cevap bekliyor.",
    wants: (what) => `İzin istiyor: ${what}`,
    yourTurn: "İş tamamlandı, sıra sende.",
    failed: "Oturum hatayla durdu.",
  },
}

function formSummary(form) {
  const field = form?.fields?.[0]
  return short(field?.description || field?.title || form?.title || "")
}

export default {
  id: "dost.notify",
  setup(ctx) {
    const opts = ctx.options ?? {}
    const url = typeof opts.url === "string" && opts.url ? opts.url : DEFAULT_URL
    const want = {
      question: opts.question !== false,
      done: opts.done !== false,
      error: opts.error !== false,
    }
    const t = TEXT[opts.lang] ?? TEXT[(process.env.LANG ?? "").slice(0, 2)] ?? TEXT.en
    const minWorkMs = (typeof opts.minWorkSeconds === "number" ? opts.minWorkSeconds : 10) * 1000
    const viaWindows = isWsl() && existsSync(WIN_CURL)

    let mode = "http"
    async function notify(title, message) {
      log("notify", mode, title, "|", message)
      const body = rpcBody(title, message)
      if (mode === "http") {
        try {
          return await sendHttp(url, body)
        } catch (err) {
          if (!viaWindows) return log(`dog not reachable (${url}): ${err.message}`)
          mode = "windows-curl"
        }
      }
      try {
        await sendViaWindowsCurl(url, body)
      } catch (err) {
        log(`dog not reachable (${url}, curl.exe): ${err.message}`)
      }
    }

    // Cache session titles and whether a session is a sub-agent.
    const sessions = new Map()
    async function sessionInfo(sessionID) {
      if (!sessions.has(sessionID)) {
        sessions.set(
          sessionID,
          ctx.session.get({ sessionID }).catch(() => undefined),
        )
      }
      return sessions.get(sessionID)
    }
    async function label(sessionID) {
      const info = await sessionInfo(sessionID)
      const where = info?.location?.directory?.split(/[\\/]/).filter(Boolean).pop()
      return short([info?.title, where].filter(Boolean).join(" · ") || "OpenCode", 60)
    }
    async function isChild(sessionID) {
      return Boolean((await sessionInfo(sessionID))?.parentID)
    }

    const startedAt = new Map()

    async function handle(event) {
      const data = event.data ?? {}
      if (event.id && !firstTime(event.id)) return
      if (DEBUG && /^(form|permission|session\.execution)\./.test(event.type)) log("event", event.type)
      switch (event.type) {
        case "session.execution.started":
          startedAt.set(data.sessionID, event.created ?? Date.now())
          return
        case "session.renamed":
        case "session.metadata.updated":
          sessions.delete(data.sessionID)
          return
        case "form.created": {
          if (!want.question) return
          const form = data.form
          await notify(t.question(await label(form.sessionID)), formSummary(form) || t.waiting)
          return
        }
        case "permission.asked": {
          if (!want.question) return
          const what = [data.action, ...(data.resources ?? []).slice(0, 2)].filter(Boolean).join(" ")
          await notify(t.permission(await label(data.sessionID)), short(data.message || t.wants(what)))
          return
        }
        case "session.execution.succeeded": {
          const started = startedAt.get(data.sessionID)
          startedAt.delete(data.sessionID)
          if (!want.done || (await isChild(data.sessionID))) return
          if (started && (event.created ?? Date.now()) - started < minWorkMs) return
          await notify(t.done(await label(data.sessionID)), t.yourTurn)
          return
        }
        case "session.execution.failed": {
          startedAt.delete(data.sessionID)
          if (!want.error || (await isChild(data.sessionID))) return
          await notify(t.error(await label(data.sessionID)), short(data.error?.message || t.failed))
          return
        }
        case "session.execution.interrupted":
          startedAt.delete(data.sessionID)
          return
      }
    }

    log("loaded", url, viaWindows ? "(WSL: falls back to curl.exe)" : "")
    const controller = new AbortController()
    ;(async () => {
      while (!controller.signal.aborted) {
        try {
          for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
            handle(event).catch((err) => log(`${event.type}: ${err?.stack ?? err}`))
          }
        } catch (err) {
          if (controller.signal.aborted) return
          log(`event stream ended: ${err?.stack ?? err}`)
        }
        // The stream is live-only and does not reconnect by itself.
        await new Promise((r) => setTimeout(r, 2000))
      }
    })()
    return () => controller.abort()
  },
}
