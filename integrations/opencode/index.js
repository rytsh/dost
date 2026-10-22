// opencode-dost: the Dost desktop dog barks when OpenCode needs you. OpenCode V2 plugin.
//
//  - A permission request shows Allow / Always / Reject buttons in the dog's bubble; the
//    click is sent back to OpenCode. Answering in the terminal withdraws the bubble.
//  - A question (form) barks and shows its text; answer it in OpenCode.
//  - Long tasks show a progress bar while they run; it turns green when done (the dog is
//    happy) and red on errors, with how long it took.
//
// Install:  opencode plugin add 'github:rytsh/dost::path:integrations/opencode'
// Options (opencode.json "plugins" entry, all optional):
//   url ("http://127.0.0.1:7777/mcp"), question (true), permissions (true), done (true),
//   error (true), progress (true), minWorkSeconds (10), lang ("en" | "tr")
// Debug:    start the OpenCode service with DOST_DEBUG=1; events go to <tmp>/dost-opencode.log

import { execFile } from "node:child_process"
import { appendFileSync, existsSync, readFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const DEFAULT_URL = "http://127.0.0.1:7777/mcp"
const WIN_CURL = "/mnt/c/Windows/System32/curl.exe"
const SOURCE = "OpenCode"
/// How long a permission bubble waits for a click (seconds); OpenCode keeps waiting after.
const ASK_SECONDS = 600

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

function rpcBody(name, args) {
  return JSON.stringify({
    jsonrpc: "2.0",
    id: Date.now(),
    method: "tools/call",
    params: { name, arguments: { source: SOURCE, ...args } },
  })
}

/// Sends a JSON-RPC body and returns the tool's text result (throws on transport errors).
async function sendHttp(url, body, timeoutMs) {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json, text/event-stream" },
    body,
    signal: AbortSignal.timeout(timeoutMs),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const json = await res.json().catch(() => ({}))
  const result = json?.result
  return { text: result?.content?.[0]?.text ?? "", isError: Boolean(result?.isError) }
}

// WSL in NAT mode cannot reach Windows' 127.0.0.1; Windows' own curl.exe can.
function sendViaWindowsCurl(url, body, timeoutMs) {
  return new Promise((resolve, reject) => {
    const secs = String(Math.ceil(timeoutMs / 1000))
    const child = execFile(
      WIN_CURL,
      ["-s", "-f", "-m", secs, "-H", "Content-Type: application/json", "--data-binary", "@-", url],
      { timeout: timeoutMs + 3000, windowsHide: true },
      (err, stdout) => {
        if (err) return reject(err)
        try {
          const result = JSON.parse(stdout)?.result
          resolve({ text: result?.content?.[0]?.text ?? "", isError: Boolean(result?.isError) })
        } catch {
          resolve({ text: "", isError: false })
        }
      },
    )
    child.stdin.end(body)
  })
}

function short(text, max = 140) {
  const t = String(text ?? "").replace(/\s+/g, " ").trim()
  return t.length > max ? `${t.slice(0, max - 1)}…` : t
}

/// "45 s", "3 min", "1 h 5 min".
function duration(ms) {
  const s = Math.round(ms / 1000)
  if (s < 60) return `${s} s`
  if (s < 3600) return `${Math.round(s / 60)} min`
  const h = Math.floor(s / 3600)
  const m = Math.round((s % 3600) / 60)
  return m ? `${h} h ${m} min` : `${h} h`
}

const TEXT = {
  en: {
    question: (s) => `Question: ${s}`,
    permission: (s) => `Permission: ${s}`,
    done: (s) => `Done: ${s}`,
    error: (s) => `Error: ${s}`,
    working: (s) => `Working: ${s}`,
    waiting: "Waiting for your answer in OpenCode.",
    wants: (what) => `Wants to ${what}`,
    yourTurn: (d) => `Finished in ${d}, your turn.`,
    failed: "The session stopped with an error.",
    step: (tool, n) => `${tool} · ${n} step${n === 1 ? "" : "s"}`,
    allow: "Allow",
    always: "Always",
    reject: "Reject",
  },
  tr: {
    question: (s) => `Soru: ${s}`,
    permission: (s) => `İzin: ${s}`,
    done: (s) => `Bitti: ${s}`,
    error: (s) => `Hata: ${s}`,
    working: (s) => `Çalışıyor: ${s}`,
    waiting: "OpenCode'da cevabını bekliyor.",
    wants: (what) => `İzin istiyor: ${what}`,
    yourTurn: (d) => `${d} içinde tamamlandı, sıra sende.`,
    failed: "Oturum hatayla durdu.",
    step: (tool, n) => `${tool} · ${n} adım`,
    allow: "İzin ver",
    always: "Hep izin ver",
    reject: "Reddet",
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
      permissions: opts.permissions !== false && opts.question !== false,
      done: opts.done !== false,
      error: opts.error !== false,
      progress: opts.progress !== false,
    }
    const t = TEXT[opts.lang] ?? TEXT[(process.env.LANG ?? "").slice(0, 2)] ?? TEXT.en
    const minWorkMs = (typeof opts.minWorkSeconds === "number" ? opts.minWorkSeconds : 10) * 1000
    const viaWindows = isWsl() && existsSync(WIN_CURL)

    let mode = "http"
    /// Calls a Dost tool; returns its result, or undefined if the dog is not reachable.
    async function call(name, args, timeoutMs = 3000) {
      if (DEBUG) log("call", mode, name, JSON.stringify(args))
      const body = rpcBody(name, args)
      if (mode === "http") {
        try {
          return await sendHttp(url, body, timeoutMs)
        } catch (err) {
          if (!viaWindows) return log(`dog not reachable (${url}): ${err.message}`)
          mode = "windows-curl"
        }
      }
      try {
        return await sendViaWindowsCurl(url, body, timeoutMs)
      } catch (err) {
        log(`dog not reachable (${url}, curl.exe): ${err.message}`)
      }
    }
    const notify = (title, message, extra = {}) => call("notify", { title, message, ...extra })

    // The session in the dog's Agents list (top-level sessions only).
    async function report(sessionID, state, detail) {
      if (await isChild(sessionID)) return
      const info = await sessionInfo(sessionID)
      const project = info?.location?.directory?.split(/[\\/]/).filter(Boolean).pop() ?? ""
      const args = { id: progressID(sessionID), title: short(info?.title ?? "", 60), project }
      if (state === "end") args.end = true
      else args.state = state
      if (detail) args.detail = short(detail)
      await call("session", args)
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

    // Running tasks: sessionID -> { started, steps, tool, shown }. The progress bar appears
    // once a task has run for minWorkSeconds; tool steps update it.
    const running = new Map()
    const progressID = (sessionID) => `opencode:${sessionID}`

    async function showProgress(sessionID) {
      const task = running.get(sessionID)
      if (!task) return
      task.shown = true
      await call("progress", {
        id: progressID(sessionID),
        title: t.working(await label(sessionID)),
        message: task.tool ? t.step(task.tool, task.steps) : "",
      })
    }

    // Permission bubbles waiting for a click: requestID -> sessionID.
    const pendingAsks = new Map()

    async function askPermission(data) {
      const what = [data.action, ...(data.resources ?? []).slice(0, 2)].filter(Boolean).join(" ")
      const choices = [t.allow, ...(data.save?.length ? [t.always] : []), t.reject]
      pendingAsks.set(data.id, data.sessionID)
      const res = await call(
        "ask",
        {
          key: data.id,
          session: progressID(data.sessionID),
          title: t.permission(await label(data.sessionID)),
          question: short(data.message || t.wants(what), 200),
          choices,
          timeout_secs: ASK_SECONDS,
        },
        (ASK_SECONDS + 10) * 1000,
      )
      const stillPending = pendingAsks.delete(data.id)
      const answer = res && !res.isError ? res.text.replace(/^The user answered: /, "") : undefined
      if (!answer || !stillPending) return
      const decision = answer === t.always ? "always" : answer === t.allow ? "once" : "reject"
      log("permission", data.id, decision)
      try {
        await ctx.permission.reply({ sessionID: data.sessionID, requestID: data.id, decision })
      } catch (err) {
        // Already answered in OpenCode; nothing to do.
        log(`permission reply ${data.id}: ${err?.message ?? err}`)
      }
    }

    async function handle(event) {
      const data = event.data ?? {}
      if (event.id && !firstTime(event.id)) return
      if (DEBUG && /^(form|permission|session\.execution)\./.test(event.type)) log("event", event.type)
      switch (event.type) {
        case "session.execution.started": {
          running.set(data.sessionID, { started: event.created ?? Date.now(), steps: 0, tool: "", shown: false })
          report(data.sessionID, "working").catch(() => {})
          if (!want.progress || (await isChild(data.sessionID))) return
          const sessionID = data.sessionID
          setTimeout(() => {
            const task = running.get(sessionID)
            if (task && !task.shown) showProgress(sessionID).catch(() => {})
          }, minWorkMs)
          return
        }
        case "session.renamed":
        case "session.metadata.updated":
          sessions.delete(data.sessionID)
          return
        case "session.deleted":
          report(data.sessionID, "end").catch(() => {})
          return
        case "form.created": {
          if (!want.question) return
          const form = data.form
          report(form.sessionID, "waiting", formSummary(form)).catch(() => {})
          await notify(t.question(await label(form.sessionID)), formSummary(form) || t.waiting, {
            sticky: true,
            session: progressID(form.sessionID),
          })
          return
        }
        case "permission.asked": {
          if (!want.permissions) return
          await askPermission(data)
          return
        }
        case "permission.replied": {
          // Answered in the terminal (or by us): take the bubble away.
          if (pendingAsks.delete(data.requestID)) await call("dismiss", { key: data.requestID })
          if (data.sessionID) report(data.sessionID, "working").catch(() => {})
          return
        }
        case "session.execution.succeeded": {
          const task = running.get(data.sessionID)
          running.delete(data.sessionID)
          const elapsed = (event.created ?? Date.now()) - (task?.started ?? Date.now())
          report(data.sessionID, "done", t.yourTurn(duration(elapsed))).catch(() => {})
          if (!want.done || (await isChild(data.sessionID))) return
          if (task && elapsed < minWorkMs) return
          const title = t.done(await label(data.sessionID))
          const message = t.yourTurn(duration(elapsed))
          if (task?.shown) {
            await call("progress", { id: progressID(data.sessionID), title, message, state: "done" })
          } else {
            await notify(title, message, { mood: "good" })
          }
          return
        }
        case "session.execution.failed": {
          const task = running.get(data.sessionID)
          running.delete(data.sessionID)
          report(data.sessionID, "failed", data.error?.message || t.failed).catch(() => {})
          if (!want.error || (await isChild(data.sessionID))) return
          const title = t.error(await label(data.sessionID))
          const message = short(data.error?.message || t.failed)
          if (task?.shown) {
            await call("progress", { id: progressID(data.sessionID), title, message, state: "failed" })
          } else {
            await notify(title, message, { mood: "bad" })
          }
          return
        }
        case "session.execution.interrupted": {
          const task = running.get(data.sessionID)
          running.delete(data.sessionID)
          report(data.sessionID, "idle").catch(() => {})
          if (task?.shown) {
            await call("progress", { id: progressID(data.sessionID), message: "", state: "failed" })
          }
          return
        }
      }
    }

    // Tool steps drive the progress bar's text.
    async function onTool(event) {
      const task = running.get(event.sessionID)
      if (!task) return
      task.steps += 1
      task.tool = event.tool ?? task.tool
      // Not every step: the list only needs to look alive.
      if (task.steps === 1 || task.steps % 5 === 0) {
        report(event.sessionID, "working", t.step(task.tool, task.steps)).catch(() => {})
      }
      if (task.shown && want.progress) await showProgress(event.sessionID)
    }

    log("loaded", url, viaWindows ? "(WSL: falls back to curl.exe)" : "")
    const registrations = []
    if (typeof ctx.tool?.hook === "function") {
      ctx.tool
        .hook("execute.before", (event) => {
          onTool(event).catch((err) => log(`tool hook: ${err?.stack ?? err}`))
        })
        .then((r) => registrations.push(r))
        .catch((err) => log(`tool hook unavailable: ${err?.message ?? err}`))
    }

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
    return () => {
      controller.abort()
      for (const r of registrations) r.dispose().catch(() => {})
    }
  },
}
