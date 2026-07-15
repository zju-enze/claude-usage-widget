import {
  TIME_ZONE,
  isPlausibleProviderKey,
} from "./usage-utils.js";

const nativeInvoke = globalThis.__TAURI__?.core?.invoke;
const hasNativeBridge = typeof nativeInvoke === "function";
const REFRESH_MS = 30_000;
const PROVIDER_STORAGE_KEY = "claude-usage-widget.selected-provider";
const PROVIDER_IDS = Object.freeze(["minimax", "deepseek", "zhipu"]);

const PROVIDERS = {
  minimax: {
    name: "MiniMax",
    labels: ["5 小时", "7 天"],
    description: "输入你的 MiniMax API Key，以读取 Coding Plan 用量。",
    placeholder: "sk-cp-...",
    security: "验证和查询时只发送至 MiniMax 官方 HTTPS 接口；Windows 使用系统凭据加密后保存在本机。",
    environment: "支持 MINIMAX_API_KEY 或 MINIMAX_CP_TOKEN；应用不会在界面中读取或显示其内容。",
  },
  deepseek: {
    name: "DeepSeek",
    labels: ["账户余额", "API 状态"],
    description: "输入你的 DeepSeek API Key，以读取 API 账户余额与可用状态。",
    placeholder: "sk-...",
    security: "验证和查询时只发送至 DeepSeek 官方 HTTPS 接口；Windows 使用系统凭据加密后保存在本机。",
    environment: "支持 DEEPSEEK_API_KEY；使用 DeepSeek Base URL 时也支持 ANTHROPIC_AUTH_TOKEN 或 ANTHROPIC_API_KEY。",
  },
  zhipu: {
    name: "智谱",
    labels: ["5 小时", "MCP 月额度"],
    description: "输入你的智谱 API Key，以读取 GLM Coding Plan 配额。",
    placeholder: "请输入 API Key",
    security: "验证和查询时只发送至智谱官方 HTTPS 接口；Windows 使用系统凭据加密后保存在本机。",
    environment: "支持 ZAI_API_KEY、ZHIPUAI_API_KEY 或 BIGMODEL_API_KEY；使用智谱 Base URL 时也支持 ANTHROPIC_AUTH_TOKEN。",
  },
};

const ERROR_MESSAGES = {
  invalid_key_format: "Key 格式不正确，请检查后重试",
  missing_key: "需要连接",
  unauthorized: "Key 已失效，请更新连接",
  rate_limited: "请求过于频繁，请稍后重试",
  service_unavailable: "服务暂时不可用",
  request_failed: "服务拒绝了本次请求",
  response_too_large: "服务响应异常，请稍后重试",
  invalid_response: "收到无法识别的用量数据",
  network: "网络连接失败，点击刷新重试",
  timeout: "请求超时，点击刷新重试",
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
let currentProvider = "minimax";
let providerRevision = 0;
let providerSwitchInFlight = false;
let setupSubmitInFlight = false;
let collapsed = false;
let clearConfirmTimer = null;
let setupReturnFocus = null;
let liquidFrame = null;
let liquidPoint = null;

function invoke(command, args = {}) {
  if (!hasNativeBridge) return Promise.reject(new Error("native_unavailable"));
  return nativeInvoke(command, args);
}

function friendlyError(code) {
  if (code === "missing_key") return `需要连接 ${PROVIDERS[currentProvider].name}`;
  return ERROR_MESSAGES[code] ?? "操作未完成，请重试";
}

function isKnownProvider(provider) {
  return typeof provider === "string" && PROVIDER_IDS.includes(provider);
}

function readStoredProvider() {
  try {
    const provider = localStorage.getItem(PROVIDER_STORAGE_KEY);
    if (provider == null) return null;
    if (isKnownProvider(provider)) return provider;
    localStorage.removeItem(PROVIDER_STORAGE_KEY);
  } catch {
    // Storage can be unavailable in private or hardened WebView contexts.
  }
  return null;
}

function persistProvider(provider) {
  if (!isKnownProvider(provider)) return false;
  try {
    localStorage.setItem(PROVIDER_STORAGE_KEY, provider);
    return true;
  } catch {
    return false;
  }
}

function providerButtons() {
  return Array.from(document.querySelectorAll(".provider-option[data-provider]"));
}

function renderProviderPicker({ focus = false } = {}) {
  for (const button of providerButtons()) {
    const selected = button.dataset.provider === currentProvider;
    button.classList.toggle("is-selected", selected);
    button.setAttribute("aria-checked", String(selected));
    button.tabIndex = selected ? 0 : -1;
    if (selected && focus) button.focus();
  }
}

function setupSourceSummary() {
  const name = PROVIDERS[currentProvider].name;
  if (providerSwitchInFlight) return `正在检测 ${name} 的连接状态…`;
  if (keySource === "env") {
    return hasSavedKey ? "环境变量已连接 · 本机另有备用 Key" : "环境变量已连接";
  }
  if (keySource === "saved") return "本机加密 Key · 已保存并连接";
  if (hasSavedKey) return "本机已有 Key · 当前需要更新";
  return "尚未连接";
}

function syncSetupControlState() {
  const picker = $("provider-picker");
  if (!picker) return;
  const busy = providerSwitchInFlight || setupSubmitInFlight;
  picker.setAttribute("aria-busy", String(providerSwitchInFlight));
  for (const button of providerButtons()) button.disabled = busy;

  const usesEnvironment = keySource === "env";
  $("setup-key-input").disabled = usesEnvironment || busy;
  $("setup-toggle-key").disabled = usesEnvironment || busy;
  $("setup-submit").disabled = busy;
  $("setup-provider-state").textContent = setupSourceSummary();
}

function applyProvider(provider, { persist = false } = {}) {
  if (!isKnownProvider(provider)) return false;
  const changed = provider !== currentProvider;
  currentProvider = provider;
  if (persist) persistProvider(provider);
  if (changed) {
    providerRevision += 1;
    if ($("five-hour-label")) clearUsage("等待首次同步");
  }
  renderProviderPicker();
  return true;
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

function paintMetric(slot, metric = {}) {
  const ids = slot === 0
    ? {
        label: "five-hour-label",
        fill: "fiveHour-fill",
        value: "fiveHour-pct",
        track: "fiveHour-track",
        description: "fiveHour-resets",
      }
    : {
        label: "seven-day-label",
        fill: "sevenDay-fill",
        value: "sevenDay-pct",
        track: "sevenDay-track",
        description: "sevenDay-resets",
      };
  const label = $(ids.label);
  const fill = $(ids.fill);
  const value = $(ids.value);
  const track = $(ids.track);
  const description = $(ids.description);
  const usedPercent = Number(metric.percent);

  label.textContent = metric.label || PROVIDERS[currentProvider].labels[slot];
  if (metric.percent == null || !Number.isFinite(usedPercent)) {
    fill.style.width = "0%";
    fill.className = "bar-fill is-empty";
    fill.dataset.value = "";
    value.textContent = metric.value || "暂无数据";
    description.textContent = metric.description || "等待首次同步";
    track.removeAttribute("aria-valuenow");
    track.setAttribute("aria-valuetext", metric.ariaText || value.textContent);
    return;
  }

  const normalized = Math.max(0, Math.min(100, Number(usedPercent)));
  const rounded = Math.round(normalized);
  const previous = fill.dataset.value;
  fill.style.width = `${normalized}%`;
  fill.dataset.value = String(normalized);
  const tone = metric.tone || (normalized >= 90 ? "bad" : normalized >= 70 ? "warning" : "good");
  fill.className = `bar-fill is-${tone}`;
  value.textContent = metric.value || `已用 ${rounded}%`;
  track.setAttribute("aria-valuenow", String(rounded));
  track.setAttribute("aria-valuetext", metric.ariaText || `已使用 ${rounded}%`);
  description.textContent = metric.description || "已同步";

  if (previous && previous !== String(normalized)) {
    fill.classList.remove("is-shine");
    requestAnimationFrame(() => fill.classList.add("is-shine"));
  }
}

function clearUsage(message = "连接后显示") {
  const labels = PROVIDERS[currentProvider].labels;
  paintMetric(0, { label: labels[0], description: message });
  paintMetric(1, { label: labels[1], description: message });
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

async function loadWindowEffectMode() {
  let mode = "preview";
  if (hasNativeBridge) {
    try {
      const nativeMode = await invoke("window_effect_mode");
      if (nativeMode === "blur" || nativeMode === "transparent") {
        mode = nativeMode;
      }
    } catch {
      mode = "transparent";
    }
  }
  document.documentElement.dataset.windowEffect = mode;
}

function resetLiquidGlass() {
  liquidPoint = null;
  if (liquidFrame !== null) {
    cancelAnimationFrame(liquidFrame);
    liquidFrame = null;
  }
  const widget = $("widget");
  widget.style.setProperty("--pointer-x", "50%");
  widget.style.setProperty("--pointer-y", "18%");
  widget.style.setProperty("--refract-x", "0px");
  widget.style.setProperty("--refract-y", "-1px");
  widget.style.setProperty("--liquid-strength", "0.62");
}

function renderLiquidGlass() {
  liquidFrame = null;
  const widget = $("widget");
  if (!liquidPoint || document.hidden || widget.classList.contains("is-setup-open")) return;

  const rect = widget.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return;
  const x = Math.min(1, Math.max(0, (liquidPoint.x - rect.left) / rect.width));
  const y = Math.min(1, Math.max(0, (liquidPoint.y - rect.top) / rect.height));
  const motionScale = collapsed ? 0.5 : 1;
  const refractX = (x - 0.5) * 6 * motionScale;
  const refractY = (y - 0.5) * 4 * motionScale;

  widget.style.setProperty("--pointer-x", `${(x * 100).toFixed(2)}%`);
  widget.style.setProperty("--pointer-y", `${(y * 100).toFixed(2)}%`);
  widget.style.setProperty("--refract-x", `${refractX.toFixed(2)}px`);
  widget.style.setProperty("--refract-y", `${refractY.toFixed(2)}px`);
  widget.style.setProperty("--liquid-strength", collapsed ? "0.72" : "0.92");
}

function setupLiquidGlass() {
  const canTrackPointer = window.matchMedia("(hover: hover) and (pointer: fine)").matches;
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const highContrast = window.matchMedia("(prefers-contrast: more)").matches;
  if (!canTrackPointer || reduceMotion || highContrast) return;

  const widget = $("widget");
  widget.addEventListener("pointermove", (event) => {
    if (widget.classList.contains("is-setup-open")) return;
    liquidPoint = { x: event.clientX, y: event.clientY };
    if (liquidFrame === null) liquidFrame = requestAnimationFrame(renderLiquidGlass);
  }, { passive: true });
  widget.addEventListener("pointerleave", resetLiquidGlass, { passive: true });
}

async function refresh() {
  if (!hasNativeBridge) {
    setConnectionState("offline");
    return;
  }
  if (refreshInFlight) return;

  refreshInFlight = true;
  const requestedProvider = currentProvider;
  const refreshRevision = providerRevision;
  setConnectionState("loading");
  let finalState = "error";
  let finalMessage = "连接异常";
  let shouldOpenSetup = false;

  try {
    const snapshot = await invoke("fetch_usage", { provider: requestedProvider });
    if (refreshRevision !== providerRevision) return;
    if (!snapshot || snapshot.provider !== requestedProvider) {
      throw usageError("invalid_response");
    }
    if (!snapshot?.found) throw usageError(snapshot?.error || "invalid_response");

    keySource = snapshot.key_source === "env" || snapshot.key_source === "saved"
      ? snapshot.key_source
      : keySource;
    if (!Array.isArray(snapshot.metrics) || snapshot.metrics.length < 2) {
      throw usageError("no_usage");
    }
    paintMetric(0, snapshot.metrics[0]);
    paintMetric(1, snapshot.metrics[1]);

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
    if (refreshRevision === providerRevision) setConnectionState(finalState, finalMessage);
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

function renderSetupContent(source = keySource, { resetInput = true } = {}) {
  keySource = source === "env" || source === "saved" ? source : "missing";
  const profile = PROVIDERS[currentProvider];
  const input = $("setup-key-input");
  const toggle = $("setup-toggle-key");
  const submit = $("setup-submit");
  const clear = $("setup-clear");
  const cancel = $("setup-cancel");

  $("setup-error").textContent = "";
  input.setAttribute("aria-invalid", "false");
  if (resetInput) {
    input.value = "";
    input.type = "password";
  }
  input.placeholder = profile.placeholder;
  $("setup-key-label").textContent = `${profile.name} API Key`;
  toggle.textContent = input.type === "password" ? "显示" : "隐藏";
  toggle.setAttribute("aria-label", `${toggle.textContent} API Key`);

  const usesEnvironment = keySource === "env";
  $("setup-title").textContent = usesEnvironment
    ? `${profile.name} 环境变量已连接`
    : setupMode === "initial" ? `连接 ${profile.name}` : `更新 ${profile.name} 连接`;
  $("setup-description").textContent = usesEnvironment
    ? "当前 Key 由系统环境变量提供。为避免来源冲突，请在环境变量中修改或移除。"
    : profile.description;
  $("setup-security").textContent = usesEnvironment
    ? profile.environment
    : profile.security;

  submit.classList.toggle("hidden", usesEnvironment);
  clear.dataset.defaultLabel = usesEnvironment
    ? "移除本机备用密钥"
    : "移除本机密钥";
  resetClearConfirmation();
  clear.classList.toggle("hidden", !hasSavedKey);
  cancel.textContent = setupMode === "initial" ? "隐藏小组件" : "返回";

  renderProviderPicker();
  syncSetupControlState();
}

async function selectProvider(provider, { focus = true } = {}) {
  if (!isKnownProvider(provider) || providerSwitchInFlight || setupSubmitInFlight) return;
  if (provider === currentProvider) {
    renderProviderPicker({ focus });
    return;
  }

  stopPolling();
  providerSwitchInFlight = true;
  applyProvider(provider, { persist: true });
  keySource = "missing";
  hasSavedKey = false;
  lastUpdatedAt = null;
  refreshFailed = false;
  lastErrorCode = null;
  clearUsage("正在检测连接");
  setConnectionState("loading", `检测 ${PROVIDERS[provider].name}`);
  renderSetupContent("missing");
  let switchError = null;

  try {
    if (!hasNativeBridge) throw usageError("native_unavailable");
    const state = await invoke("probe_state", { provider });
    if (!state || state.provider !== provider || !isKnownProvider(state.provider)) {
      throw usageError("invalid_response");
    }

    keySource = state.source === "env" || state.source === "saved"
      ? state.source
      : "missing";
    hasSavedKey = Boolean(state.has_saved_key);
    renderSetupContent(keySource);
    if (state.has_key) {
      setConnectionState("idle", `${PROVIDERS[provider].name} 已连接`);
    } else {
      setConnectionState("warning", `连接 ${PROVIDERS[provider].name}`);
    }
  } catch (error) {
    keySource = "missing";
    hasSavedKey = false;
    switchError = friendlyError(errorCode(error));
    setConnectionState("error", "连接检测失败");
  } finally {
    providerSwitchInFlight = false;
    renderSetupContent(keySource, { resetInput: false });
    if (switchError) $("setup-error").textContent = switchError;
    renderProviderPicker({ focus });
  }
}

async function showSetup(show, mode = "manage", source = keySource) {
  const overlay = $("setup-overlay");
  const header = $("widget-header");
  const content = $("content");
  const input = $("setup-key-input");
  const cancel = $("setup-cancel");
  $("widget").classList.toggle("is-setup-open", show);
  if (show) resetLiquidGlass();

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
  renderSetupContent(source);

  overlay.classList.remove("hidden");
  overlay.setAttribute("aria-hidden", "false");
  header.inert = true;
  content.inert = true;
  await setWindowMode("setup");

  window.setTimeout(() => {
    (keySource === "env" ? cancel : input).focus();
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
  if (!isPlausibleProviderKey(currentProvider, candidate)) {
    errorElement.textContent = friendlyError("invalid_key_format");
    input.setAttribute("aria-invalid", "true");
    input.focus();
    return;
  }

  const provider = currentProvider;
  setupSubmitInFlight = true;
  syncSetupControlState();
  submit.textContent = "正在验证…";
  try {
    const result = await invoke("save_key_and_test", { provider, key: candidate });
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
    setupSubmitInFlight = false;
    syncSetupControlState();
    submit.textContent = "验证并保存";
  }
}

async function manageKey() {
  let source = keySource;
  try {
    const state = await invoke("probe_state", { provider: currentProvider });
    if (!state || state.provider !== currentProvider) throw usageError("invalid_response");
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
    await invoke("clear_key", { provider: currentProvider });
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
  resetLiquidGlass();
  await setWindowMode(collapsed ? "collapsed" : "expanded");
}

function setupInteractions() {
  setupLiquidGlass();
  $("btn-refresh").addEventListener("click", refresh);
  $("btn-collapse").addEventListener("click", toggleCollapsed);
  $("btn-close").addEventListener("click", hideWidget);
  $("btn-manage-key").addEventListener("click", manageKey);
  $("setup-form").addEventListener("submit", submitSetup);
  $("setup-clear").addEventListener("click", clearSavedKey);

  $("provider-picker").addEventListener("click", (event) => {
    const button = event.target instanceof Element
      ? event.target.closest(".provider-option[data-provider]")
      : null;
    if (!(button instanceof HTMLButtonElement)) return;
    void selectProvider(button.dataset.provider, { focus: true });
  });

  $("provider-picker").addEventListener("keydown", (event) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    const buttons = providerButtons();
    if (buttons.length === 0) return;
    event.preventDefault();

    const activeIndex = Math.max(0, buttons.indexOf(document.activeElement));
    let nextIndex = activeIndex;
    if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = buttons.length - 1;
    else if (event.key === "ArrowLeft") nextIndex = (activeIndex - 1 + buttons.length) % buttons.length;
    else nextIndex = (activeIndex + 1) % buttons.length;

    void selectProvider(buttons[nextIndex].dataset.provider, { focus: true });
  });

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
      await invoke("open_help_page", { provider: currentProvider });
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
    if (document.hidden) {
      resetLiquidGlass();
      stopPolling();
    }
    else if (keySource !== "missing") {
      refresh();
      startPolling();
    }
  });
}

async function init() {
  setupInteractions();
  const storedProvider = readStoredProvider();
  if (storedProvider) applyProvider(storedProvider);
  else renderProviderPicker();
  await loadWindowEffectMode();
  clearUsage("等待首次同步");
  setConnectionState("idle");
  await loadActiveModel();

  if (!hasNativeBridge) {
    $("btn-manage-key").disabled = true;
    setConnectionState("offline", "桌面预览");
    return;
  }

  try {
    const state = await invoke("probe_state", { provider: storedProvider });
    if (!state || !applyProvider(state.provider, { persist: true })) {
      throw usageError("invalid_response");
    }
    keySource = state.source === "env" || state.source === "saved"
      ? state.source
      : "missing";
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
