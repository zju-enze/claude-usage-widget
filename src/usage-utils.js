export const TIME_ZONE = "Asia/Shanghai";

export function isPlausibleProviderKey(provider, value) {
  const candidate = String(value ?? "").trim();
  if (candidate.length < 20 || candidate.length > 512 || /\s/.test(candidate)) return false;
  if (provider === "minimax") return candidate.startsWith("sk-cp-");
  if (provider === "deepseek") return candidate.startsWith("sk-");
  return provider === "zhipu";
}
