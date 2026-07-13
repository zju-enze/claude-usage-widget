# Claude Usage Widget

A small floating widget that shows your **MiniMax Coding Plan** usage
in two windows: a **5-hour** rolling quota and a **7-day** weekly quota.

Built with **Tauri 2** (Rust) + native HTML/CSS/JS, packaged as a
single-file transparent window that sits at the top-right of your
primary monitor.

> **Platform:** Windows 10 / 11 (WebView2). Other platforms are not
> built or tested.

---

## What it shows

```
┌────────────────────────────────────┐
│  ○  Claude Usage          ↻  −  × │
│                                    │
│  5 小时                    已用 56% │
│  ████████████░░░░░░░░░░            │
│  10:00–15:00 · 重置 2 时 6 分      │
│                                    │
│  7 天                       已用 4% │
│  ██░░░░░░░░░░░░░░░░░░░░            │
│  本周 · 重置 6 天 11 时             │
│                                    │
│  配置模型   MiniMax-M2.7-highspeed  │
│  上次更新   13:42:16                │
└────────────────────────────────────┘
```

The 5-hour bar is your rolling session quota; the 7-day bar is your
weekly plan quota. Both are **used percent** (not remaining).

---

## Data source

The widget calls the official MiniMax Coding Plan API directly:

```
GET https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains
Authorization: Bearer <sk-cp-...>
```

with a 30-second polling interval and exponential backoff on transient
errors. On a successful response, the **5-hour** bar reads
`100 - current_interval_remaining_percent`, and the **7-day** bar reads
`100 - current_weekly_remaining_percent`. The reset countdown is
`remains_time` / `weekly_remains_time` (milliseconds).

`general` and `video` (which appear in the server's internal
`model_remains[].model_name` field) are **resource categories**, not
user-visible models. They are dropped server-side; the WebView never
sees them. Plan-name metadata is **not** shown — the public API does
not return a plan-name field, and we deliberately do not hardcode a
display value.

---

## Security

- **API Key** is stored using **Windows DPAPI** (per-user encrypted
  blob in `%APPDATA%\.claude-usage-widget\key.bin`). Plain-text
  storage on non-Windows is refused.
- The DPAPI blob buffer is wrapped in RAII; `LocalFree` is called on
  every code path (success or failure).
- Production builds log **only** HTTP status + duration + response
  length, never the response body, never the Authorization header,
  never any environment variable values.
- Strict CSP in `tauri.conf.json`:
  `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`.
- The single outbound URL (the MiniMax key-management page) is
  hardcoded in Rust; the frontend cannot pass any URL argument.
- All HTML output uses `textContent`; there are no `innerHTML` sinks.

---

## Install

### Windows (recommended)

Download the latest `Claude Usage Widget_0.1.x_x64_en-US.msi` (or
`..._x64-setup.exe` for the lighter NSIS variant) from
[Releases](https://github.com/zju-enze/claude-usage-widget/releases).

The MSI installs to `%ProgramFiles%\Claude Usage Widget\` and writes
the WebView2 bootstrap path. The NSIS installer is portable — pick
whichever you prefer.

### Build from source

Requires Rust stable (1.78+), Node 18+, and WebView2 (Windows).

```bash
git clone https://github.com/zju-enze/claude-usage-widget.git
cd claude-usage-widget
npm ci
npm run tauri build     # produces msi + nsis installers
```

The freshly built single-file executable lives at
`src-tauri\target\release\claude-usage-widget.exe`.

---

## First run

On first launch (no key yet), the window appears immediately with a
**setup overlay** asking for your MiniMax `sk-cp-...` key. Once saved
the key is encrypted with DPAPI and used for every subsequent
request. The widget also accepts the key from `MINIMAX_API_KEY` or
`MINIMAX_CP_TOKEN` environment variables, which take precedence.

### Auto-launch with Claude Code

The widget polls for `claude.exe` every 10 seconds:

- Claude Code running → widget window shows
- Claude Code stopped → widget window hides (no orphan floating card)
- Manually hidden → not auto-shown until Claude toggles state

It does **not** steal the keyboard focus from other apps when it
appears.

---

## Close, reopen, kill

| Action | Effect |
|---|---|
| Click `×` | Hide the window (process keeps running) |
| Double-click the title bar | Toggle hide / show |
| Click `−` | Collapse to a 44 px header (a single tap of the `+` expands back) |
| Right-click tray | (planned) Quit / Show / Refresh / Auto-start |

To fully quit, end the `claude-usage-widget.exe` process via Task
Manager. (A real tray icon is planned.)

---

## Auto-start on login (optional)

Run the included PowerShell script once:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install-startup.ps1 -Install
```

This writes `ClaudeUsageWidget` to `HKCU\...\Run\`, pointing at the
installed exe. Re-run with `-Uninstall` to remove.

---

## How the "current model" is read

The widget reads **the model Claude Code is configured to use**, not
the model implied by the API. Priority:

1. `~/.claude/settings.json` → `model` field (or `model.id`)
2. `~/.claude/settings.json` → `env.ANTHROPIC_MODEL` /
   `env.MINIMAX_MODEL` / `env.CLAUDE_MODEL`
3. The same keys under `~/.claude/settings.local.json`
4. Process environment variables of the same names

If nothing is found the row reads **"未检测到"** instead of fabricating
a model name.

> **This is the model Claude Code was *launched* with.** It is not
> guaranteed to equal the model routing any particular request took
> (e.g. a fallback to a different model inside Claude Code is not
> observable to this widget).

---

## Privacy

- The widget only contacts `https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains`.
- No telemetry, no analytics, no third-party scripts, no auto-update.
- The HTTP client is built with `https_only(true)`; cleartext HTTP is
  refused.
- Request timeout: 10 s connect + total, with at most 3 attempts
  (250 ms / 750 ms backoff + jitter, honours `Retry-After`).
- Response body hard-capped at 256 KiB before deserialization.
- The widget never writes outside its own `%APPDATA%` directory.

---

## Known limitations

- Only **Windows** is built. Linux / macOS would need a different
  key-storage story (Secret Service / Keychain).
- Plan name is not shown — the API does not expose one. We do not
  hardcode a value because that would be fabricating data.
- The 30-second polling is fixed. There is no manual "force refresh
  every N seconds" knob.
- No multi-language UI. Strings are simplified-Chinese.
- No auto-update mechanism. Bump the version in `tauri.conf.json` and
  re-install to upgrade.
- A real system tray is not yet implemented; closing the window hides
  it but the process keeps polling in the background.

---

## How to clear / replace your API key

The widget reads key resolution in this order on every refresh:

```
process env MINIMAX_API_KEY / MINIMAX_CP_TOKEN
  → user-level env (HKCU\Environment)
  → DPAPI-encrypted blob at %APPDATA%\.claude-usage-widget\key.bin
```

To replace a stored key: run the setup flow again (re-launching the
app with the stored key deleted from disk will surface the setup
overlay). To wipe everything:

```powershell
Remove-Item "$env:APPDATA\claude-usage-widget\key.bin" -ErrorAction SilentlyContinue
```

---

## Development

```bash
npm ci                       # install Node deps
npm test                     # Node-side unit tests
cd src-tauri && cargo test  # Rust-side unit tests
cd src-tauri && cargo fmt --check
cd src-tauri && cargo clippy --locked --all-targets -- -D warnings
npm run tauri dev           # dev mode (hot-reload of src/)
npm run tauri build         # release installers
```

Test totals (as of the most recent commit on master):
`cargo test --lib` → 28 pass, `npm test` → 13 pass.

---

## Project layout

```
.
├── docs/
│   └── AUDIT_BASELINE.md
├── src/
│   ├── index.html
│   ├── main.js
│   └── styles.css
├── src-tauri/
│   ├── Cargo.toml / Cargo.lock
│   ├── capabilities/default.json
│   ├── tauri.conf.json
│   └── src/
│       ├── lib.rs              # commands, window lifecycle, keystore
│       └── api.rs              # typed DTO + view-model + retry
├── tests/
│   └── frontend.test.mjs       # node --test
├── .github/
│   ├── workflows/ci.yml
│   └── dependabot.yml
└── scripts/
    └── install-startup.ps1
```

---

## License

MIT.