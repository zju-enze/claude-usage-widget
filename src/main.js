let invoke = null;
let getCurrentWindow = null;
try {
  invoke = window.__TAURI__.core.invoke;
  getCurrentWindow = window.__TAURI__.window.getCurrentWindow;
} catch (e) {
  // 拒绝 innerHTML：DOM XSS sink。改用 textContent 构造独立元素。
  showBootError(e && e.message ? e.message : "unknown");
}

function showBootError(message) {
  const pre = document.createElement("pre");
  pre.style.cssText = "color:red;padding:10px;font-size:11px;white-space:pre-wrap;word-break:break-all;";
  pre.textContent = "[BOOT_ERR] " + message;
  document.body.appendChild(pre);
}
const tlog = (level, msg) => {
  // 仅写入 WebView 控制台。生产构建绝不发送任意字符串到 Rust stderr。
  const fn = console[level === "info" ? "log" : level] || console.log;
  fn.call(console, "[FE]", msg);
};
tlog("info", "main.js loaded, window=" + (getCurrentWindow ? "ok" : "MISSING"));
tlog("info", "readyState=" + document.readyState + ", has-setup-overlay=" + !!document.getElementById("setup-overlay"));

const REFRESH_MS = 30000; // 30s 远程拉一次（API 限速）
const TZ = "Asia/Shanghai";

// ─── 工具 ──────────────────────────────────────────────
function escapeHtml(s) { return String(s ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c])); }

function fmtDuration(ms) {
  if (ms == null || ms <= 0) return "—";
  // API 给的就是毫秒
  const totalSec = Math.floor(ms / 1000);
  const d = Math.floor(totalSec / 86400);
  const h = Math.floor((totalSec % 86400) / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  if (d > 0) return `${d} 天 ${h} 时`;
  if (h > 0) return `${h} 时 ${m} 分`;
  return `${m} 分`;
}

function fmtTimeLocal(ms) {
  if (!ms) return "—";
  const v = Number(ms);
  if (isNaN(v) || v <= 0) return "—";
  // API 返回毫秒级 Unix 时间戳
  return new Date(v).toLocaleTimeString("zh-CN", {
    hour: "2-digit", minute: "2-digit", timeZone: TZ, hour12: false,
  });
}

// ─── 状态行（保留内部 lastUpdated 状态；不再写入可见底部） ─────
let _lastUpdatedAt = null;          // Date | null —— 内部状态
let _refreshInFlight = false;       // 防止叠加请求
let _refreshFailed = false;         // 最近一次是否失败

function setConnectionState(state) {
  // state: "idle" | "loading" | "ok" | "warn" | "error"
  const dot = document.getElementById("status-dot");
  if (!dot) return;
  dot.classList.remove("state-idle", "state-loading", "state-ok", "state-warn", "state-error");
  dot.classList.add("state-" + state);
  // 同步刷新按钮 tooltip 与 aria-label
  updateRefreshTooltip();
}

function updateRefreshTooltip() {
  const btn = document.getElementById("btn-refresh");
  if (!btn) return;
  let tip = "刷新用量";
  const lines = [];
  if (_refreshInFlight) {
    lines.push("正在刷新…");
  } else if (_refreshFailed) {
    lines.push("刷新失败 · 点击重试");
  } else if (_lastUpdatedAt) {
    const ts = _lastUpdatedAt.toLocaleTimeString("zh-CN", {
      hour: "2-digit", minute: "2-digit", second: "2-digit", timeZone: TZ, hour12: false,
    });
    lines.push(`上次更新：${ts}`);
  } else {
    lines.push("尚未刷新");
  }
  tip += "\n" + lines[0];
  btn.setAttribute("title", tip);
  btn.setAttribute("aria-label", tip.replace("\n", " · "));
}

// 上次更新独立 meta-row 文本生成。
// 同一天：`HH:mm:ss`；跨天：`昨天 HH:mm:ss` 或 `MM-DD HH:mm:ss`（更早）。
function formatLastUpdated(date) {
  if (!date) return "--";
  const now = new Date();
  const sameDay = date.toDateString() === now.toDateString();
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  const isYesterday = date.toDateString() === yesterday.toDateString();
  const hh = date.toLocaleTimeString("zh-CN", {
    hour: "2-digit", minute: "2-digit", second: "2-digit", timeZone: TZ, hour12: false,
  });
  if (sameDay) return hh;
  if (isYesterday) return `昨天 ${hh}`;
  const md = String(date.getMonth() + 1).padStart(2, "0") + "-" + String(date.getDate()).padStart(2, "0");
  return `${md} ${hh}`;
}

function updateLastUpdatedRow() {
  const el = document.getElementById("last-updated");
  if (!el) return;
  el.textContent = formatLastUpdated(_lastUpdatedAt);
}

// ─── 单条进度条 ──────────────────────────────────────────
// 颜色：已用 ≤70% 绿、70–90% 黄、>90% 红
function paintBar(fillId, pctId, subId, usedPct, resetMs, subText) {
  const fillEl = document.getElementById(fillId);
  const textEl = document.getElementById(pctId);
  const subEl = document.getElementById(subId);

  if (usedPct == null || isNaN(usedPct)) {
    fillEl.style.width = "0%";
    fillEl.className = "bar-fill";
    textEl.textContent = "无数据";
    if (subEl) subEl.textContent = subText || "—";
    return;
  }
  const v = Math.max(0, Math.min(100, Number(usedPct)));
  const prev = fillEl.style.width;
  fillEl.style.width = `${v}%`;
  fillEl.className = "bar-fill " + (
    v >= 90 ? "bad" : v >= 70 ? "warn" : "good"
  );
  textEl.textContent = `已用 ${v.toFixed(0)}%`;
  if (subEl) subEl.textContent = subText || "—";
  // 副文本里加 reset 倒计时
  if (subEl && resetMs != null) {
    const r = fmtDuration(resetMs);
    if (r !== "—") subEl.textContent = `${subText || ""} · 重置 ${r}`.replace(/^\s·\s/, "");
  }
  // 仅在宽度变化时触发一次性光泽滑过
  if (prev && prev !== `${v}%`) {
    fillEl.classList.remove("is-shine");
    void fillEl.offsetWidth;
    fillEl.classList.add("is-shine");
  }
}

// ─── 套餐名称已不在 UI 展示 ─────────────────────────────
//
// 为什么完全没有套餐字段：
//   API `/v1/api/openplatform/coding_plan/remains` 不返回套餐名称。
//   公开的 minimaxi.com 开发者文档也未列出专门的套餐信息端点。
//   在没有权威数据源时，"硬编码 + 显示" 就是编造数据，违反本项目"真实数据驱动"原则。
//
// 当前 UI 中已无套餐行。如果未来 minimaxi 提供返回套餐名的端点，
// 再启用 read_plan_metadata + 前端套餐行。

// ─── 当前模型读取 ───────────────────────────────────────
// 严禁从 Token Plan API 的 model_name 字段推断当前模型。
// 真实来源：环境变量 / Claude Code 配置文件（~/.claude/settings.json）。
// 读取失败：隐藏整行而不是编造。
async function loadActiveModel() {
  const row = document.getElementById("model-row");
  const pill = document.getElementById("model-name");
  try {
    const info = await invoke("read_active_model");
    if (info && info.model && info.model.trim()) {
      const m = info.model.trim();
      pill.textContent = m;
      pill.title = m;
      pill.classList.remove("is-empty");
      row.style.display = "";
    } else {
      pill.textContent = "未检测到";
      pill.title = "无法可靠读取 Claude Code 当前模型";
      pill.classList.add("is-empty");
      row.style.display = "";
    }
  } catch (e) {
    pill.textContent = "未检测到";
    pill.title = "无法可靠读取 Claude Code 当前模型";
    pill.classList.add("is-empty");
    row.style.display = "";
  }
}

// ─── 主刷新 ──────────────────────────────────────────────
//
// 数据来源与处理边界：
//   - 5h / 7d 用量：来自后端 UsageViewModel.five_hour / .weekly
//     （后端已校验字段范围、丢弃无效值，不再直接暴露 API 原始结构）
//   - 当前模型：来自 read_active_model（环境变量 / Claude Code 配置）
//   - 套餐：来自 read_plan_metadata（API 不返回套餐名，本组件不展示）
//   - 各模型剩余 %：API 字段存在但被后端丢弃，前端永不接收
async function refresh() {
  if (_refreshInFlight) return;
  _refreshInFlight = true;
  setConnectionState("loading");
  updateRefreshTooltip();
  try {
    const vm = await invoke("fetch_minimax_usage");
    if (!vm.found || vm.state !== "ok") {
      _refreshFailed = true;
      setConnectionState("error");
      return;
    }

    // 后端已校验 0..=100 范围 + NaN/Infinity 过滤
    paintBar(
      "fiveHour-fill", "fiveHour-pct", "fiveHour-resets",
      vm.five_hour_used_percent,
      vm.five_hour_reset_after_ms,
      `${fmtTimeLocal(vm.five_hour_start_at_ms)}–${fmtTimeLocal(vm.five_hour_end_at_ms)}`,
    );
    paintBar(
      "sevenDay-fill", "sevenDay-pct", "sevenDay-resets",
      vm.weekly_used_percent,
      vm.weekly_reset_after_ms,
      "本周",
    );

    _lastUpdatedAt = vm.fetched_at ? new Date(vm.fetched_at) : new Date();
    _refreshFailed = false;
    setConnectionState("ok");
  } catch (e) {
    _refreshFailed = true;
    setConnectionState("error");
  } finally {
    // 必须在 _refreshInFlight=false 之后再 update tooltip，否则 tooltip 会
    // 卡在"正在刷新…"（因为 updateRefreshTooltip 根据 _refreshInFlight 推断状态）
    _refreshInFlight = false;
    updateRefreshTooltip();
    updateLastUpdatedRow();
  }
}

function setupButtons() {
  const win = getCurrentWindow();
  const refreshBtn = document.getElementById("btn-refresh");
  refreshBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (_refreshInFlight) return;
    refresh();
  });

  document.getElementById("btn-collapse").addEventListener("click", async (e) => {
    e.stopPropagation();
    const w = document.getElementById("widget");
    const collapsed = w.classList.toggle("collapsed");
    const icon = document.querySelector("#btn-collapse svg");
    if (icon) {
      icon.style.transform = collapsed ? "rotate(45deg)" : "rotate(0deg)";
    }
    try {
      await win.setSize(new (win.constructor || Object).Size(360, collapsed ? 44 : 198));
    } catch {}
  });

  document.getElementById("btn-close").addEventListener("click", (e) => { e.stopPropagation(); win.hide(); });
  document.querySelector("header").addEventListener("dblclick", (e) => {
    e.stopPropagation();
    if (win.isVisible()) win.hide(); else win.show();
  });
  // 初始 tooltip
  updateRefreshTooltip();
}

// ─── 启动探测 ──────────────────────────────────────────
async function probe() {
  try {
    const state = await invoke("probe_state");
    return state;
  } catch (e) {
    return { has_key: false, source: "error", error: String(e) };
  }
}

function showSetup(show) {
  tlog("info", "showSetup(" + show + ")");
  const overlay = document.getElementById("setup-overlay");
  tlog("info", "overlay el: " + !!overlay);
  if (!overlay) return;
  overlay.classList.toggle("hidden", !show);
  const content = document.getElementById("content");
  if (content) content.style.opacity = show ? "0.25" : "1";
  const hdr = document.querySelector("header");
  if (hdr) hdr.style.opacity = show ? "0.25" : "1";
  if (show) {
    setTimeout(() => {
      const i = document.getElementById("setup-key-input");
      if (i) i.focus();
    }, 100);
  }
}

async function submitSetup() {
  const input = document.getElementById("setup-key-input");
  const errEl = document.getElementById("setup-error");
  const btn = document.getElementById("setup-submit");
  const key = input.value.trim();
  errEl.textContent = "";
  if (!key || !key.startsWith("sk-cp-") || key.length < 20) {
    errEl.textContent = "sk-cp key 应该以 'sk-cp-' 开头";
    return;
  }
  btn.disabled = true;
  btn.textContent = "连接中…";
  try {
    const result = await invoke("save_key_and_test", { key });
    if (result.ok) {
      // 保存成功 → 隐藏 setup、启动轮询
      input.value = "";
      showSetup(false);
      if (window.__widget && window.__widget.startPolling) {
        window.__widget.startPolling();
      } else {
        refresh();
      }
    } else {
      errEl.textContent = `保存/测试失败：${result.error || "未知错误"}`;
      btn.disabled = false;
      btn.textContent = "连接并保存";
    }
  } catch (e) {
    errEl.textContent = `出错了：${e}`;
    btn.disabled = false;
    btn.textContent = "连接并保存";
  }
}
// ─── 启动逻辑 ──────────────────────────────────────────
// 1. 默认隐藏（Rust 端 setup 阶段已经 hide）
// 2. 后台线程每 5s 查 claude.exe：
//    - 启动 → show
//    - 关闭 → hide
// 所以 widget 永远不"独立"显示 —— 只能跟随 Claude Code 出现。

function setupSetupHandlers() {
  document.getElementById("setup-submit").addEventListener("click", (e) => { e.stopPropagation(); submitSetup(); });
  document.getElementById("setup-key-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.stopPropagation(); submitSetup(); }
    e.stopPropagation(); // 防止被 Tauri drag region 拦截
  });
  document.getElementById("setup-help").addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    invoke("open_minimax_key_page").catch(() => {});
  });
}

// ─── 幂等轮询控制器 ──────────────────────────────────
// 多重 init / setup 成功 / 多次 setupOverlay 关闭 都可能触发 startPolling，
// 必须保证只创建一个 interval。
let _pollTimer = null;
let _visible = typeof document !== "undefined" ? !document.hidden : true;

function startPolling() {
  if (_pollTimer !== null) return;
  // 启动立即拉一次，再周期性拉
  refresh();
  _pollTimer = setInterval(refresh, REFRESH_MS);
}

function stopPolling() {
  if (_pollTimer === null) return;
  clearInterval(_pollTimer);
  _pollTimer = null;
}

// 窗口隐藏时暂停高频请求（CPU / 流量）。显示时立即恢复并补一次。
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    const wasVisible = _visible;
    _visible = !document.hidden;
    if (!wasVisible && _visible) {
      // hidden → visible：恢复轮询 + 立刻拉一次
      startPolling();
    } else if (wasVisible && !_visible) {
      // visible → hidden：暂停轮询
      stopPolling();
    }
  });
  // 页面卸载时清理 timer
  window.addEventListener("pagehide", () => stopPolling());
  window.addEventListener("beforeunload", () => stopPolling());
}

// 暴露给 setup 成功时调用
window.__widget = window.__widget || {};
window.__widget.startPolling = startPolling;
window.__widget.stopPolling = stopPolling;

async function init() {
  try {
    tlog("info", "init: start");
    setupButtons();
    tlog("info", "init: setupButtons done");
    setupSetupHandlers();
    tlog("info", "init: setupSetupHandlers done");

    // 当前模型：与用量解耦，独立加载
    await loadActiveModel();

    setConnectionState("idle");
    const probeResult = await probe();
    tlog("info", "init: probe_state returned " + JSON.stringify(probeResult));
    if (!probeResult.has_key) {
      showSetup(true);
    } else if (_visible) {
      startPolling();
    }
  } catch (e) {
    tlog("error", "init failed: " + e.message);
    showBootError(e && e.message ? e.message : String(e));
  }
}

if (document.readyState === "loading") {
  window.addEventListener("DOMContentLoaded", () => init());
} else {
  init();
}
