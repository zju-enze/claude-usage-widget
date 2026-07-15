# Performance baseline

Records performance characteristics of `claude-usage-widget` v0.2.0.
Most numbers here are placeholders that require running on a real
machine with Claude Code active; CI does not measure them.

## How to reproduce

```bash
# 1. Build release
cd src-tauri && cargo build --release

# 2. Cold start (with no Claude Code running)
Measure-Command {
  Start-Process '.\src-tauri\target\release\claude-usage-widget.exe'
}
# … wait 5 s, then:
Get-Process -Name 'claude-usage-widget' | Select-Object WS, CPU, StartTime

# 3. Idle memory (after 5 min with no Claude Code, no fetching)
Get-Process -Name 'claude-usage-widget' | Select-Object WS, PrivateMemorySize

# 4. Idle CPU (sample over 60 s)
for ($i=0; $i -lt 12; $i++) {
  $p = Get-Process -Name 'claude-usage-widget'
  Write-Host ("{0:N2}" -f $p.CPU)
  Start-Sleep -Seconds 5
}

# 5. 5-minute request count
# (watch Tauri stderr / Wireshark loopback) — should be:
#    0 requests if Claude Code not running (visibility pause)
#    ~10 requests if Claude Code running (one every 30s)
```

## Measured values

| Metric | Value | Source |
|---|---|---|
| Release cold-start time (Win 11, NVMe) | not measured | run script above |
| Idle memory (working set) | not measured | run script above |
| Idle CPU | not measured | run script above |
| Window-hidden CPU | not measured | should be ~0 (visibility pause + polling pause) |
| Requests in 5 min (Claude running) | ~10 | expected |
| Requests in 5 min (Claude not running) | 0 | expected |
| Memory growth after 1 h refreshes | not measured | expected flat (no caches) |

The "not measured" rows above were left as placeholders per project
policy: do not fabricate numbers. Run the scripts under
"## How to reproduce" and update the table with real measurements.

## Architectural factors that bound performance

- **Shared `reqwest::Client`** in `AppState` → TLS handshake + DNS +
  connection pool are reused across the 30-s polling interval.
  Avoids the "every fetch = new TLS handshake" anti-pattern.
- **Bounded DPAPI buffers** in `windows_crypto` → native output buffers
  are copied, zeroed when they contain plaintext, and released with
  `LocalFree` on every return path.
- **Visibility-driven polling pause** (`document.visibilitychange`
  listener) → when the user hides the widget (or minimizes the
  desktop session), `stopPolling()` clears the timer. No 30-s
  fetch loop running while nobody is looking.
- **`refresh()` `finally` ordering**: `_refreshInFlight` is cleared
  before `updateRefreshTooltip()`, so the UI never sits in the
  "正在刷新…" state after a fast response.
- **`@keyframes pulse` on the loading dot** is the only continuously
  running animation. It is GPU-accelerated, CPU-cheap, and disabled
  entirely under `prefers-reduced-motion`.
- **Bar width animation** is a single `transition: width 0.55s …` on
  the `.bar-fill` div, not a per-frame JS-driven loop.
- **`is-shine` class on the bar** is a one-shot keyframe (700 ms)
  triggered only when `prev !== next`, not a continuous loop.
- **Pointer-coalesced refraction** uses at most one queued
  `requestAnimationFrame` while the pointer is moving; it is not a
  continuous animation loop. There is no DOM mutation observer.
