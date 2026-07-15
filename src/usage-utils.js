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
