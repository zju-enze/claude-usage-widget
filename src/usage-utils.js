export const TIME_ZONE = "Asia/Shanghai";

export function remainingToUsedPercent(value) {
  if (value == null || value === "") return null;
  const remaining = Number(value);
  if (!Number.isFinite(remaining)) return null;
  return 100 - Math.max(0, Math.min(100, remaining));
}

export function selectPrimaryUsage(items) {
  if (!Array.isArray(items) || items.length === 0) return null;
  return items.find((item) => item?.current_interval_status === 1) ?? items[0] ?? null;
}

function usedPercent(value) {
  if (value == null || value === "") return null;
  const percent = Number(value);
  if (!Number.isFinite(percent)) return null;
  return Math.max(0, Math.min(100, percent));
}

function resetDescription(prefix, milliseconds) {
  const reset = formatDuration(milliseconds);
  return reset === "—" ? prefix : `${prefix} · ${reset} 后重置`;
}

function normalizeMinimaxUsage(raw) {
  const primary = selectPrimaryUsage(raw?.model_remains);
  if (!primary) return null;

  const intervalStart = formatLocalTime(primary.start_time);
  const intervalEnd = formatLocalTime(primary.end_time);
  const intervalLabel = intervalStart === "—" || intervalEnd === "—"
    ? "本周期"
    : `${intervalStart}–${intervalEnd}`;

  return {
    metrics: [
      {
        label: "5 小时",
        percent: remainingToUsedPercent(primary.current_interval_remaining_percent),
        description: resetDescription(intervalLabel, Number(primary.remains_time)),
      },
      {
        label: "7 天",
        percent: remainingToUsedPercent(primary.current_weekly_remaining_percent),
        description: resetDescription("本周", Number(primary.weekly_remains_time)),
      },
    ],
  };
}

function currencyAmount(currency, value) {
  const amount = String(value ?? "—");
  if (currency === "CNY") return `¥${amount}`;
  if (currency === "USD") return `$${amount}`;
  return `${currency || "余额"} ${amount}`;
}

function normalizeDeepseekUsage(raw) {
  if (typeof raw?.is_available !== "boolean" || !Array.isArray(raw?.balance_infos)) return null;
  const balance = raw.balance_infos.find((item) => item?.currency === "CNY")
    ?? raw.balance_infos[0];
  if (!balance) return null;

  const currency = balance.currency;
  const available = raw.is_available;
  return {
    metrics: [
      {
        label: "账户余额",
        percent: null,
        value: currencyAmount(currency, balance.total_balance),
        description: `赠送 ${currencyAmount(currency, balance.granted_balance)} · 充值 ${currencyAmount(currency, balance.topped_up_balance)}`,
        ariaText: `账户余额 ${currencyAmount(currency, balance.total_balance)}`,
      },
      {
        label: "API 状态",
        percent: 100,
        value: available ? "可用" : "不可用",
        description: available ? "余额足以继续调用" : "余额不足，请充值后重试",
        ariaText: available ? "API 可用" : "API 不可用",
        tone: available ? "good" : "bad",
      },
    ],
  };
}

function quotaDescription(limit, fallback) {
  if (!limit) return fallback;
  const current = limit.currentValue ?? limit.currentUsage;
  const total = limit.usage ?? limit.total ?? limit.totol;
  if (current != null && total != null) return `${current} / ${total}`;
  return fallback;
}

function normalizeZhipuUsage(raw) {
  const limits = raw?.data?.limits ?? raw?.limits;
  if (!Array.isArray(limits)) return null;
  const tokens = limits.find((item) => item?.type === "TOKENS_LIMIT");
  const tools = limits.find((item) => item?.type === "TIME_LIMIT");
  if (!tokens && !tools) return null;

  return {
    metrics: [
      {
        label: "5 小时",
        percent: usedPercent(tokens?.percentage),
        description: quotaDescription(tokens, "GLM Coding Plan Token 配额"),
      },
      {
        label: "MCP 月额度",
        percent: usedPercent(tools?.percentage),
        description: quotaDescription(tools, "GLM Coding Plan MCP 配额"),
      },
    ],
  };
}

export function normalizeProviderUsage(provider, raw) {
  if (provider === "deepseek") return normalizeDeepseekUsage(raw);
  if (provider === "zhipu") return normalizeZhipuUsage(raw);
  if (provider === "minimax") return normalizeMinimaxUsage(raw);
  return null;
}

export function isPlausibleProviderKey(provider, value) {
  const candidate = String(value ?? "").trim();
  if (candidate.length < 20 || candidate.length > 512 || /\s/.test(candidate)) return false;
  if (provider === "minimax") return candidate.startsWith("sk-cp-");
  if (provider === "deepseek") return candidate.startsWith("sk-");
  return provider === "zhipu";
}

export function formatDuration(milliseconds) {
  const value = Number(milliseconds);
  if (!Number.isFinite(value) || value <= 0) return "—";

  const totalMinutes = Math.max(1, Math.floor(value / 60_000));
  const days = Math.floor(totalMinutes / (24 * 60));
  const hours = Math.floor((totalMinutes % (24 * 60)) / 60);
  const minutes = totalMinutes % 60;

  if (days > 0) return `${days} 天 ${hours} 时`;
  if (hours > 0) return `${hours} 时 ${minutes} 分`;
  return `${minutes} 分`;
}

export function formatLocalTime(milliseconds, timeZone = TIME_ZONE) {
  const value = Number(milliseconds);
  if (!Number.isFinite(value) || value <= 0) return "—";
  return new Date(value).toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    timeZone,
  });
}
