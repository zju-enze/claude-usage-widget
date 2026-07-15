import test from "node:test";
import assert from "node:assert/strict";

import {
  formatDuration,
  isPlausibleProviderKey,
  normalizeProviderUsage,
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

test("MiniMax usage is normalized into the existing two metric slots", () => {
  const usage = normalizeProviderUsage("minimax", {
    model_remains: [{
      current_interval_status: 1,
      current_interval_remaining_percent: 64,
      current_weekly_remaining_percent: 28,
      remains_time: 90 * 60_000,
      weekly_remains_time: 27 * 60 * 60_000,
    }],
  });

  assert.equal(usage.metrics[0].label, "5 小时");
  assert.equal(usage.metrics[0].percent, 36);
  assert.match(usage.metrics[0].description, /1 时 30 分 后重置/);
  assert.equal(usage.metrics[1].percent, 72);
});

test("DeepSeek balance is normalized without inventing a spent percentage", () => {
  const usage = normalizeProviderUsage("deepseek", {
    is_available: true,
    balance_infos: [{
      currency: "CNY",
      total_balance: "110.00",
      granted_balance: "10.00",
      topped_up_balance: "100.00",
    }],
  });

  assert.equal(usage.metrics[0].label, "账户余额");
  assert.equal(usage.metrics[0].percent, null);
  assert.equal(usage.metrics[0].value, "¥110.00");
  assert.equal(usage.metrics[1].value, "可用");
  assert.equal(usage.metrics[1].tone, "good");
});

test("Zhipu quota limits map to token and MCP usage slots", () => {
  const usage = normalizeProviderUsage("zhipu", {
    data: {
      limits: [
        { type: "TOKENS_LIMIT", percentage: 42 },
        { type: "TIME_LIMIT", percentage: "17", currentValue: 3, usage: 20 },
      ],
    },
  });

  assert.equal(usage.metrics[0].label, "5 小时");
  assert.equal(usage.metrics[0].percent, 42);
  assert.equal(usage.metrics[1].label, "MCP 月额度");
  assert.equal(usage.metrics[1].percent, 17);
  assert.equal(usage.metrics[1].description, "3 / 20");
});

test("provider key validation follows each official key shape", () => {
  assert.equal(isPlausibleProviderKey("minimax", "sk-cp-12345678901234"), true);
  assert.equal(isPlausibleProviderKey("deepseek", "sk-12345678901234567"), true);
  assert.equal(isPlausibleProviderKey("zhipu", "1234567890.abcdefghijkl"), true);
  assert.equal(isPlausibleProviderKey("zhipu", "1234567890 abcdefghijkl"), false);
});
