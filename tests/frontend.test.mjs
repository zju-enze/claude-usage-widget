// Frontend unit tests (Node --test runner)
// These tests verify pure formatters used by main.js. We re-implement
// the same logic here (small, stable) rather than dynamically importing
// main.js — main.js's top-level executes Tauri/DOM-touching code that
// requires the WebView runtime and would crash Node.

import { test } from "node:test";
import assert from "node:assert/strict";

// Mirror of main.js::escapeHtml
function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
}

// Mirror of main.js::fmtDuration
function fmtDuration(ms) {
  if (ms == null || ms <= 0) return "—";
  const totalSec = Math.floor(ms / 1000);
  const d = Math.floor(totalSec / 86400);
  const h = Math.floor((totalSec % 86400) / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  if (d > 0) return `${d} 天 ${h} 时`;
  if (h > 0) return `${h} 时 ${m} 分`;
  return `${m} 分`;
}

// Mirror of main.js::formatLastUpdated
function formatLastUpdated(date, tz = "Asia/Shanghai") {
  if (!date) return "--";
  const now = new Date();
  const sameDay = date.toDateString() === now.toDateString();
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  const isYesterday = date.toDateString() === yesterday.toDateString();
  const hh = date.toLocaleTimeString("zh-CN", {
    hour: "2-digit", minute: "2-digit", second: "2-digit", timeZone: tz, hour12: false,
  });
  if (sameDay) return hh;
  if (isYesterday) return `昨天 ${hh}`;
  const md = String(date.getMonth() + 1).padStart(2, "0") + "-" + String(date.getDate()).padStart(2, "0");
  return `${md} ${hh}`;
}

test("escapeHtml neutralizes HTML metacharacters", () => {
  assert.equal(
    escapeHtml(`<script>alert("x")</script>' & "`),
    "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;&#39; &amp; &quot;"
  );
});

test("escapeHtml handles null and undefined", () => {
  assert.equal(escapeHtml(null), "");
  assert.equal(escapeHtml(undefined), "");
});

test("escapeHtml passes plain strings through", () => {
  assert.equal(escapeHtml("hello world"), "hello world");
});

test("fmtDuration null/negative returns em dash", () => {
  assert.equal(fmtDuration(null), "—");
  assert.equal(fmtDuration(undefined), "—");
  assert.equal(fmtDuration(0), "—");
  assert.equal(fmtDuration(-100), "—");
});

test("fmtDuration minutes only", () => {
  assert.equal(fmtDuration(45 * 60 * 1000), "45 分");
});

test("fmtDuration hours and minutes", () => {
  assert.equal(fmtDuration((2 * 3600 + 17 * 60) * 1000), "2 时 17 分");
});

test("fmtDuration days and hours", () => {
  assert.equal(fmtDuration((3 * 86400 + 5 * 3600) * 1000), "3 天 5 时");
});

test("formatLastUpdated null returns placeholder", () => {
  assert.equal(formatLastUpdated(null), "--");
  assert.equal(formatLastUpdated(undefined), "--");
});

test("formatLastUpdated today returns HH:mm:ss", () => {
  const now = new Date();
  const text = formatLastUpdated(now, "UTC");
  assert.match(text, /^\d{2}:\d{2}:\d{2}$/);
});

test("formatLastUpdated yesterday returns 昨天 HH:mm:ss", () => {
  const y = new Date();
  y.setDate(y.getDate() - 1);
  const text = formatLastUpdated(y, "UTC");
  assert.match(text, /^昨天 \d{2}:\d{2}:\d{2}$/);
});

test("formatLastUpdated older returns MM-DD HH:mm:ss", () => {
  const old = new Date("2024-01-15T10:30:00Z");
  const text = formatLastUpdated(old, "UTC");
  assert.match(text, /^01-15 \d{2}:\d{2}:\d{2}$/);
});

// ─── UI contract checks ──────────────────────────────
// Spec demands: no "各模型剩余", no "env", no general/video in any
// rendered string. main.js does not render these strings, but we
// regression-guard by inspecting the source for forbidden tokens.

test("main.js source does not contain forbidden UI tokens", async () => {
  const fs = await import("node:fs/promises");
  const path = await import("node:path");
  const main = await fs.readFile(path.join(process.cwd(), "src", "main.js"), "utf8");
  // Strip // line comments and /* block comments */ before scanning
  const stripped = main
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
  // These tokens must NEVER appear as code/strings in main.js.
  // (They are allowed in explanatory comments — checked separately.)
  for (const bad of ["各模型剩余", "env ·", "general 剩余", "video 剩余"]) {
    assert.ok(
      !stripped.includes(bad),
      `forbidden UI token "${bad}" found in main.js code`,
    );
  }
});

test("index.html source does not contain forbidden UI tokens", async () => {
  const fs = await import("node:fs/promises");
  const path = await import("node:path");
  const html = await fs.readFile(path.join(process.cwd(), "src", "index.html"), "utf8");
  for (const bad of ["各模型剩余", "general", "video"]) {
    assert.ok(
      !html.includes(bad),
      `forbidden UI token "${bad}" found in index.html`,
    );
  }
});

test("provider preference is constrained to the public provider whitelist", async () => {
  const fs = await import("node:fs/promises");
  const path = await import("node:path");
  const main = await fs.readFile(path.join(process.cwd(), "src", "main.js"), "utf8");

  assert.match(
    main,
    /PROVIDER_IDS\s*=\s*Object\.freeze\(\["minimax",\s*"deepseek",\s*"zhipu"\]\)/,
  );
  assert.match(main, /if \(!isKnownProvider\(provider\)\) return false;/);
  assert.match(main, /localStorage\.setItem\(PROVIDER_STORAGE_KEY, provider\)/);
  assert.equal(
    [...main.matchAll(/localStorage\.setItem\(/g)].length,
    1,
    "only the non-sensitive provider preference may be stored in localStorage",
  );
});

test("frontend consumes normalized metrics and never vendor raw JSON", async () => {
  const fs = await import("node:fs/promises");
  const path = await import("node:path");
  const main = await fs.readFile(path.join(process.cwd(), "src", "main.js"), "utf8");

  assert.match(main, /Array\.isArray\(snapshot\.metrics\)/);
  assert.match(main, /paintMetric\(0, snapshot\.metrics\[0\]\)/);
  assert.match(main, /paintMetric\(1, snapshot\.metrics\[1\]\)/);
  assert.ok(!main.includes("snapshot.raw"));
  assert.ok(!main.includes("normalizeProviderUsage"));
});

test("provider-scoped commands always receive the selected provider", async () => {
  const fs = await import("node:fs/promises");
  const path = await import("node:path");
  const main = await fs.readFile(path.join(process.cwd(), "src", "main.js"), "utf8");

  for (const command of ["fetch_usage", "save_key_and_test", "clear_key", "open_help_page", "probe_state"]) {
    const calls = [...main.matchAll(new RegExp(`invoke\\("${command}"\\s*,\\s*\\{([^}]*)\\}`, "g"))];
    assert.ok(calls.length > 0, `${command} must be invoked with arguments`);
    for (const call of calls) {
      assert.match(call[1], /\bprovider\b/, `${command} call must include provider`);
    }
  }
});

test("provider picker is keyboard-oriented and DOM writes stay text-only", async () => {
  const fs = await import("node:fs/promises");
  const path = await import("node:path");
  const [main, html] = await Promise.all([
    fs.readFile(path.join(process.cwd(), "src", "main.js"), "utf8"),
    fs.readFile(path.join(process.cwd(), "src", "index.html"), "utf8"),
  ]);

  assert.match(html, /role="radiogroup"/);
  assert.equal([...html.matchAll(/role="radio"/g)].length, 3);
  assert.match(main, /"ArrowLeft", "ArrowRight", "Home", "End"/);
  assert.ok(!main.includes("innerHTML"));
  assert.ok(!html.includes("<script>") && !html.includes("javascript:"));
});
