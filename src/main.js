import {
  TIME_ZONE,
  formatDuration,
  formatLocalTime,
  remainingToUsedPercent,
  selectPrimaryUsage,
} from "./usage-utils.js";

const nativeInvoke = globalThis.__TAURI__?.core?.invoke;
const hasNativeBridge = typeof nativeInvoke === "function";
const REFRESH_MS = 30_000;

const ERROR_MESSAGES = {
  invalid_key_format: "Key 格式不正确，请检查后重试",
  missing_key: "需要连接 MiniMax",
  unauthorized: "Key 已失效，请更新连接",
  rate_limited: "请求过于频繁，请稍后重试",
  service_unavailable: "MiniMax 服务暂时不可用",
  request_failed: "MiniMax 拒绝了本次请求",
  response_too_large: "服务响应异常，请稍后重试",
  invalid_response: "收到无法识别的用量数据",
  network: "网络连接失败，点击刷新重试",
  no_usage: "当前账户暂无可用用量数据",
  storage_error: "本机加密保存失败",
  storage_unsupported: "此平台不支持安全保存，请改用环境变量",
  remove_failed: "移除本机 Key 失败",
  open_failed: "无法打开帮助页面",
  native_unavailable: "请在桌面应用中运行",
};

const $ = (id) => document.getElementById(id);

let lastUpdatedAt = null;
let refreshInFlight = false;
let refreshFailed = false;
let lastErrorCode = null;
let pollingTimer = null;
let keySource = "missing";
let hasSavedKey = false;
let setupMode = "initial";
let collapsed = false;
let clearConfirmTimer = null;
let setupReturnFocus = null;

function invoke(command, args = {}) {
  if (!hasNativeBridge) return Promise.reject(new Error("native_unavailable"));
  return nativeInvoke(command, args);
}

function friendlyError(code) {
  return ERROR_MESSAGES[code] ?? "操作未完成，请重试";
}

function errorCode(error) {
  if (typeof error?.code === "string") return error.code;
  if (typeof error?.message === "string" && ERROR_MESSAGES[error.message]) {
    return error.message;
  }
  if (typeof error === "string" && ERROR_MESSAGES[error]) return error;
  return "network";
}

function usageError(code) {
  const error = new Error(code);
  error.code = code;
  return error;
}

function formatClock(date) {
  return date.toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
    timeZone: TIME_ZONE,
  });
}

function updateLastUpdated() {
  const element = $("last-updated");
  if (!element) return;

  if (!hasNativeBridge) {
    element.textContent = "仅桌面应用可刷新";
  } else if (refreshInFlight) {
    element.textContent = "正在安全同步…";
  } else if (refreshFailed && lastUpdatedAt) {
    element.textContent = `数据截至 ${formatClock(lastUpdatedAt)}`;
  } else if (refreshFailed && lastErrorCode) {
    element.textContent = friendlyError(lastErrorCode);
  } else if (lastUpdatedAt) {
    element.textContent = `更新于 ${formatClock(lastUpdatedAt)}`;
  } else {
    element.textContent = "尚未同步";
  }
}

function updateRefreshTooltip() {
  const button = $("btn-refresh");
  if (!button) return;

  let detail = "尚未刷新";
  if (!hasNativeBridge) detail = "仅桌面应用可刷新";
  else if (refreshInFlight) detail = "正在刷新…";
  else if (refreshFailed && lastErrorCode) detail = friendlyError(lastErrorCode);
  else if (lastUpdatedAt) detail = `上次更新：${formatClock(lastUpdatedAt)}`;

  const label = `刷新用量 · ${detail}`;
  button.title = label;
  button.setAttribute("aria-label", label);
}

function setConnectionState(state, message) {
  const dot = $("status-dot");
  const status = $("connection-status");
  const refreshButton = $("btn-refresh");
  const content = $("content");

  const defaultMessages = {
    idle: "待同步",
    loading: "同步中",
    ok: "已同步",
    warning: "需要连接",
    error: "连接异常",
    offline: "桌面预览",
  };

  dot.className = `dot state-${state}`;
  status.textContent = message || defaultMessages[state] || defaultMessages.idle;
  status.title = status.textContent;
  refreshButton.classList.toggle("is-spinning", state === "loading");
  refreshButton.disabled = state === "loading" || !hasNativeBridge;
  content.setAttribute("aria-busy", String(state === "loading"));
  content.classList.toggle("is-stale", state === "error" && Boolean(lastUpdatedAt));
  $("widget").dataset.connection = state;

  updateLastUpdated();
  updateRefreshTooltip();
}

function paintBar(fillId, valueId, trackId, descriptionId, usedPercent, resetMs, prefix) {
  const fill = $(fillId);
  const value = $(valueId);
  const track = $(trackId);
  const description = $(descriptionId);

  if (!Number.isFinite(usedPercent)) {
    fill.style.width = "0%";
    fill.className = "bar-fill is-empty";
    fill.dataset.value = "";
    value.textContent = "暂无数据";
    description.textContent = prefix || "等待首次同步";
    track.removeAttribute("aria-valuenow");
    track.setAttribute("aria-valuetext", "暂无数据");
    return;
  }

  const normalized = Math.max(0, Math.min(100, Number(usedPercent)));
  const rounded = Math.round(normalized);
  const previous = fill.dataset.value;
  fill.style.width = `${normalized}%`;
  fill.dataset.value = String(normalized);
  fill.className = `bar-fill ${normalized >= 90 ? "is-bad" : normalized >= 70 ? "is-warning" : "is-good"}`;
  value.textContent = `已用 ${rounded}%`;
  track.setAttribute("aria-valuenow", String(rounded));
  track.setAttribute("aria-valuetext", `已使用 ${rounded}%`);

  const reset = formatDuration(resetMs);
  description.textContent = reset === "—" ? prefix : `${prefix} · ${reset} 后重置`;

  if (previous && previous !== String(normalized)) {
    fill.classList.remove("is-shine");
    requestAnimationFrame(() => fill.classList.add("is-shine"));
  }
}

function clearUsage(message = "连接后显示") {
  paintBar("fiveHour-fill", "fiveHour-pct", "fiveHour-track", "fiveHour-resets", null, 0, message);
  paintBar("sevenDay-fill", "sevenDay-pct", "sevenDay-track", "sevenDay-resets", null, 0, message);
}

async function loadActiveModel() {
  const model = $("model-name");
  if (!hasNativeBridge) {
    model.textContent = "待桌面检测";
    model.title = "模型仅能在桌面应用中检测";
    model.classList.add("is-empty");
    return;
  }

  try {
    const info = await invoke("read_active_model");
    const name = info?.model?.trim();
    if (name) {
      model.textContent = name;
      model.title = name;
      model.classList.remove("is-empty");
    } else {
      model.textContent = "未检测到";
      model.title = "未在 Claude Code 配置中检测到模型";
      model.classList.add("is-empty");
    }
  } catch {
    model.textContent = "未检测到";
    model.title = "读取 Claude Code 模型失败";
    model.classList.add("is-empty");
  }
}

async function refresh() {
  if (!hasNativeBridge) {
    setConnectionState("offline");
    return;
  }
  if (refreshInFlight) return;

  refreshInFlight = true;
  setConnectionState("loading");
  let finalState = "error";
  let finalMessage = "连接异常";
  let shouldOpenSetup = false;

  try {
    const snapshot = await invoke("fetch_minimax_usage");
    if (!snapshot?.found) throw usageError(snapshot?.error || "invalid_response");

    keySource = snapshot.key_source || keySource;
    const primary = selectPrimaryUsage(snapshot.raw?.model_remains);
    if (!primary) throw usageError("no_usage");

    const intervalStart = formatLocalTime(primary.start_time);
    const intervalEnd = formatLocalTime(primary.end_time);
    const intervalLabel = intervalStart === "—" || intervalEnd === "—"
      ? "本周期"
      : `${intervalStart}–${intervalEnd}`;

    paintBar(
      "fiveHour-fill",
      "fiveHour-pct",
      "fiveHour-track",
      "fiveHour-resets",
      remainingToUsedPercent(primary.current_interval_remaining_percent),
      Number(primary.remains_time),
      intervalLabel,
    );
    paintBar(
      "sevenDay-fill",
      "sevenDay-pct",
      "sevenDay-track",
      "sevenDay-resets",
      remainingToUsedPercent(primary.current_weekly_remaining_percent),
      Number(primary.weekly_remains_time),
      "本周",
    );

    const fetchedAt = new Date(snapshot.fetched_at);
    lastUpdatedAt = Number.isNaN(fetchedAt.getTime()) ? new Date() : fetchedAt;
    refreshFailed = false;
    lastErrorCode = null;
    finalState = "ok";
    finalMessage = "已同步";
  } catch (error) {
    lastErrorCode = errorCode(error);
    refreshFailed = true;
    finalMessage = friendlyError(lastErrorCode);
    shouldOpenSetup = lastErrorCode === "missing_key";
  } finally {
    refreshInFlight = false;
    setConnectionState(finalState, finalMessage);
  }

  if (shouldOpenSetup) {
    stopPolling();
    await showSetup(true, "initial", "missing");
  }
}

function stopPolling() {
  if (pollingTimer !== null) {
    clearInterval(pollingTimer);
    pollingTimer = null;
  }
}

function startPolling() {
  stopPolling();
  if (!hasNativeBridge || keySource === "missing" || document.hidden) return;
  pollingTimer = window.setInterval(refresh, REFRESH_MS);
}

async function setWindowMode(mode) {
  if (!hasNativeBridge) return;
  try {
    await invoke("set_window_mode", { mode });
  } catch {
    // The CSS state remains usable even when the native resize is unavailable.
  }
}

function resetClearConfirmation() {
  if (clearConfirmTimer !== null) {
    clearTimeout(clearConfirmTimer);
    clearConfirmTimer = null;
  }
  const button = $("setup-clear");
  button.dataset.armed = "false";
  button.textContent = button.dataset.defaultLabel || "移除本机密钥";
  button.classList.remove("is-danger");
}

async function showSetup(show, mode = "manage", source = keySource) {
  const overlay = $("setup-overlay");
  const header = $("widget-header");
  const content = $("content");
  const input = $("setup-key-input");
  const toggle = $("setup-toggle-key");
  const submit = $("setup-submit");
  const clear = $("setup-clear");
  const cancel = $("setup-cancel");

  if (!show) {
    overlay.classList.add("hidden");
    overlay.setAttribute("aria-hidden", "true");
    header.inert = false;
    content.inert = false;
    input.value = "";
    input.type = "password";
    await setWindowMode(collapsed ? "collapsed" : "expanded");
    startPolling();
    if (setupReturnFocus?.isConnected) setupReturnFocus.focus();
    setupReturnFocus = null;
    return;
  }

  stopPolling();
  const opening = overlay.classList.contains("hidden");
  if (opening) {
    setupReturnFocus = mode === "manage" && document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
  }
  setupMode = mode;
  keySource = source;
  $("setup-error").textContent = "";
  input.setAttribute("aria-invalid", "false");
  input.value = "";
  input.type = "password";
  toggle.textContent = "显示";
  toggle.setAttribute("aria-label", "显示 API Key");

  const usesEnvironment = source === "env";
  $("setup-title").textContent = usesEnvironment
    ? "环境变量已连接"
    : mode === "initial" ? "连接 MiniMax" : "更新连接";
  $("setup-description").textContent = usesEnvironment
    ? "当前 Key 由系统环境变量提供。为避免来源冲突，请在环境变量中修改或移除。"
    : "输入你的 MiniMax API Key，以读取 Coding Plan 用量。";
  $("setup-security").textContent = usesEnvironment
    ? "支持 MINIMAX_API_KEY 或 MINIMAX_CP_TOKEN；应用不会在界面中读取或显示其内容。"
    : "验证和查询时只发送至 MiniMax 官方 HTTPS 接口；Windows 使用系统凭据加密后保存在本机。";

  input.disabled = usesEnvironment;
  toggle.disabled = usesEnvironment;
  submit.classList.toggle("hidden", usesEnvironment);
  clear.dataset.defaultLabel = usesEnvironment
    ? "移除本机备用密钥"
    : "移除本机密钥";
  resetClearConfirmation();
  clear.classList.toggle("hidden", !hasSavedKey);
  cancel.textContent = mode === "initial" ? "隐藏小组件" : "返回";

  overlay.classList.remove("hidden");
  overlay.setAttribute("aria-hidden", "false");
  header.inert = true;
  content.inert = true;
  await setWindowMode("setup");

  window.setTimeout(() => {
    (usesEnvironment ? cancel : input).focus();
  }, 80);
}

async function submitSetup(event) {
  event.preventDefault();
  const input = $("setup-key-input");
  const errorElement = $("setup-error");
  const submit = $("setup-submit");
  const candidate = input.value.trim();

  errorElement.textContent = "";
  input.setAttribute("aria-invalid", "false");
  if (!candidate.startsWith("sk-cp-") || candidate.length < 20 || candidate.length > 512) {
    errorElement.textContent = friendlyError("invalid_key_format");
    input.setAttribute("aria-invalid", "true");
    input.focus();
    return;
  }

  submit.disabled = true;
  submit.textContent = "正在验证…";
  try {
    const result = await invoke("save_key_and_test", { key: candidate });
    if (!result?.ok) throw usageError(result?.error || "storage_error");

    keySource = "saved";
    hasSavedKey = true;
    await showSetup(false);
    await refresh();
    startPolling();
  } catch (error) {
    const code = errorCode(error);
    errorElement.textContent = friendlyError(code);
    input.setAttribute("aria-invalid", "true");
  } finally {
    submit.disabled = false;
    submit.textContent = "验证并保存";
  }
}

async function manageKey() {
  let source = keySource;
  try {
    const state = await invoke("probe_state");
    source = state?.source || source;
    hasSavedKey = Boolean(state?.has_saved_key);
  } catch {
    source = keySource;
  }
  await showSetup(true, "manage", source);
}

async function clearSavedKey() {
  const button = $("setup-clear");
  if (button.dataset.armed !== "true") {
    button.dataset.armed = "true";
    button.textContent = "再次点击确认移除";
    button.classList.add("is-danger");
    clearConfirmTimer = window.setTimeout(resetClearConfirmation, 5_000);
    return;
  }

  button.disabled = true;
  try {
    await invoke("clear_key");
    hasSavedKey = false;
    if (keySource === "env") {
      await showSetup(true, "manage", "env");
      return;
    }
    keySource = "missing";
    stopPolling();
    lastUpdatedAt = null;
    refreshFailed = false;
    lastErrorCode = null;
    clearUsage();
    setConnectionState("warning", "等待连接");
    await showSetup(true, "initial", "missing");
  } catch (error) {
    $("setup-error").textContent = friendlyError(errorCode(error));
  } finally {
    button.disabled = false;
  }
}

async function hideWidget() {
  stopPolling();
  const input = $("setup-key-input");
  input.value = "";
  input.type = "password";
  $("setup-toggle-key").textContent = "显示";
  $("setup-toggle-key").setAttribute("aria-label", "显示 API Key");
  try {
    await invoke("hide_main_window");
  } catch {
    // Native hiding is unavailable in a normal browser preview.
  }
}

async function toggleCollapsed() {
  if (!$("setup-overlay").classList.contains("hidden")) return;
  collapsed = !collapsed;
  $("widget").classList.toggle("is-collapsed", collapsed);
  const button = $("btn-collapse");
  button.textContent = collapsed ? "展开" : "收起";
  button.title = collapsed ? "展开小组件" : "收起小组件";
  button.setAttribute("aria-label", button.title);
  button.setAttribute("aria-expanded", String(!collapsed));
  await setWindowMode(collapsed ? "collapsed" : "expanded");
}

function setupInteractions() {
  $("btn-refresh").addEventListener("click", refresh);
  $("btn-collapse").addEventListener("click", toggleCollapsed);
  $("btn-close").addEventListener("click", hideWidget);
  $("btn-manage-key").addEventListener("click", manageKey);
  $("setup-form").addEventListener("submit", submitSetup);
  $("setup-clear").addEventListener("click", clearSavedKey);

  $("setup-toggle-key").addEventListener("click", () => {
    const input = $("setup-key-input");
    const reveal = input.type === "password";
    input.type = reveal ? "text" : "password";
    $("setup-toggle-key").textContent = reveal ? "隐藏" : "显示";
    $("setup-toggle-key").setAttribute("aria-label", `${reveal ? "隐藏" : "显示"} API Key`);
    input.focus();
  });

  $("setup-help").addEventListener("click", async () => {
    try {
      await invoke("open_help_page");
    } catch {
      $("setup-error").textContent = friendlyError("open_failed");
    }
  });

  $("setup-cancel").addEventListener("click", async () => {
    if (setupMode === "initial") await hideWidget();
    else await showSetup(false);
  });

  $("widget-header").addEventListener("dblclick", (event) => {
    if (!event.target.closest("button")) toggleCollapsed();
  });

  document.addEventListener("keydown", async (event) => {
    if (event.key !== "Escape" || $("setup-overlay").classList.contains("hidden")) return;
    event.preventDefault();
    if (setupMode === "initial") await hideWidget();
    else await showSetup(false);
  });

  document.addEventListener("visibilitychange", () => {
    if (document.hidden) stopPolling();
    else if (keySource !== "missing") {
      refresh();
      startPolling();
    }
  });
}

async function init() {
  setupInteractions();
  clearUsage("等待首次同步");
  setConnectionState("idle");
  await loadActiveModel();

  if (!hasNativeBridge) {
    $("btn-manage-key").disabled = true;
    setConnectionState("offline", "桌面预览");
    return;
  }

  try {
    const state = await invoke("probe_state");
    keySource = state?.source || "missing";
    hasSavedKey = Boolean(state?.has_saved_key);
    if (!state?.has_key) {
      setConnectionState("warning", "等待连接");
      await showSetup(true, "initial", "missing");
      return;
    }

    await showSetup(false);
    await refresh();
    startPolling();
  } catch {
    setConnectionState("error", "初始化失败");
    await showSetup(true, "initial", "missing");
  }
}

init();
