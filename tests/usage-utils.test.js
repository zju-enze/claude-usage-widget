import test from "node:test";
import assert from "node:assert/strict";

import {
  formatDuration,
  remainingToUsedPercent,
  selectPrimaryUsage,
} from "../src/usage-utils.js";

test("remaining percentage is converted to used percentage and clamped", () => {
  assert.equal(remainingToUsedPercent(64), 36);
  assert.equal(remainingToUsedPercent("28"), 72);
  assert.equal(remainingToUsedPercent(130), 0);
  assert.equal(remainingToUsedPercent(-5), 100);
  assert.equal(remainingToUsedPercent(undefined), null);
  assert.equal(remainingToUsedPercent("not-a-number"), null);
});

test("primary usage prefers the active interval and handles empty input", () => {
  const fallback = { current_interval_status: 0, id: "fallback" };
  const active = { current_interval_status: 1, id: "active" };
  assert.equal(selectPrimaryUsage([fallback, active]), active);
  assert.equal(selectPrimaryUsage([fallback]), fallback);
  assert.equal(selectPrimaryUsage([]), null);
  assert.equal(selectPrimaryUsage(null), null);
});

test("duration formatting keeps compact widget-friendly units", () => {
  assert.equal(formatDuration(30_000), "1 分");
  assert.equal(formatDuration(90 * 60_000), "1 时 30 分");
  assert.equal(formatDuration(27 * 60 * 60_000), "1 天 3 时");
  assert.equal(formatDuration(0), "—");
});
