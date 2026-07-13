let invoke = null;
let getCurrentWindow = null;
try {
  invoke = window.__TAURI__.core.invoke;
  getCurrentWindow = window.__TAURI__.window.getCurrentWindow;
} catch (e) {
  document.body.innerHTML += '<pre style="color:red;padding:8px;font-size:10px">[FE_TOP_ERR] ' + e.message + '</pre>';
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
function pct(n) { if (n == null) return null; return Math.max(0, Math.min(100, Number(n))); }
function num(n) { if (n == null) return 0; return Number(n); }
function fmt(n) {
  if (n == null || isNaN(n)) return "—";
  const v = Number(n);
  if (v >= 1e8) return (v / 1e8).toFixed(1).replace(/\.0$/, "") + " 亿";
  if (v >= 1e4) return (v / 1e4).toFixed(1).replace(/\.0$/, "") + " 万";
  return v.toLocaleString("zh-CN");
}
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

function setStatus(_msg, _isErr) {
  // 旧版底部 env · time 状态行已删除。这里保留为空实现，
  // 保证历史调用点不会崩。lastUpdated 状态用 _lastUpdatedAt 单独维护。
}

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
// 实现 Rust 端 read_plan_metadata + 前端套餐行。

async function loadPlanMetadata() {
  // 后端始终返回 None —— 调用保留以便将来扩展，但当前不做任何 UI 更新。
  try { await invoke("read_plan_metadata"); } catch (e) { /* 静默 */ }
}

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
//   - 5h / 7d 用量：来自 coding_plan/remains 的 model_remains[0]
//     （primary = current_interval_status === 1，否则取第一条）
//   - 当前模型：来自 read_active_model（环境变量 / Claude Code 配置），
//     不再从 model_remains[].model_name 推断
//   - 套餐：来自 read_plan_metadata（API 不返回套餐名，硬编码显示）
//   - 各模型剩余 %：API 字段存在但本组件不再进入 ViewModel
async function refresh() {
  if (_refreshInFlight) return;
  _refreshInFlight = true;
  setConnectionState("loading");
  try {
    const snap = await invoke("fetch_minimax_usage");
    if (!snap.found) {
      _refreshFailed = true;
      setConnectionState("error");
      updateRefreshTooltip();
      return;
    }
    const arr = snap.raw?.model_remains;
    if (!Array.isArray(arr) || arr.length === 0) {
      _refreshFailed = true;
      setConnectionState("error");
      updateRefreshTooltip();
      return;
    }

    // primary 仅用于 5h / 7d 字段解析（接口里 status===1 表示主档）
    const primary = arr.find(x => x.current_interval_status === 1) || arr[0];

    // `start_time`/`end_time` 是毫秒级 Unix 时间戳；`remains_time` 是毫秒数。
    function used(remaining) {
      if (remaining == null) return null;
      const v = Number(remaining);
      // 直接用：API 返回 0–100 整数（已用% = 100 - 剩余%）
      return Math.max(0, Math.min(100, 100 - v));
    }

    const fiveRem = primary.current_interval_remaining_percent;
    const weekRem = primary.current_weekly_remaining_percent;
    const fiveResetsMs = Number(primary.remains_time) || 0;
    const weekResetsMs = Number(primary.weekly_remains_time) || 0;
    const startMs = Number(primary.start_time);
    const endMs   = Number(primary.end_time);

    paintBar(
      "fiveHour-fill", "fiveHour-pct", "fiveHour-resets",
      used(fiveRem),
      fiveResetsMs,
      `${fmtTimeLocal(startMs)}–${fmtTimeLocal(endMs)}`,
    );
    paintBar(
      "sevenDay-fill", "sevenDay-pct", "sevenDay-resets",
      used(weekRem),
      weekResetsMs,
      "本周",
    );

    // 注意：不再写 #model-name（由 loadActiveModel 维护）

    _lastUpdatedAt = new Date(snap.fetched_at);
    _refreshFailed = false;
    setConnectionState("ok");
    updateRefreshTooltip();
  } catch (e) {
    _refreshFailed = true;
    setConnectionState("error");
    updateRefreshTooltip();
  } finally {
    _refreshInFlight = false;
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
      // 保存成功 → 隐藏 setup、开始刷新
      input.value = "";
      showSetup(false);
      refresh();
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
    invoke("open_url", { url: "https://platform.minimaxi.com/user-center/basic-information/interface-key" }).catch(() => {});
  });
}

async function init() {
  try {
    tlog("info", "init: start");
    setupButtons();
    tlog("info", "init: setupButtons done");
    setupSetupHandlers();
    tlog("info", "init: setupSetupHandlers done");

    // 套餐 + 当前模型：与用量解耦，独立加载
    await Promise.all([loadPlanMetadata(), loadActiveModel()]);

    setConnectionState("idle");
    const probeResult = await probe();
    tlog("info", "init: probe_state returned " + JSON.stringify(probeResult));
    if (!probeResult.has_key) {
      showSetup(true);
    } else {
      refresh();
      setInterval(refresh, REFRESH_MS);
    }
  } catch (e) {
    tlog("error", "init failed: " + e.message);
    document.body.innerHTML += '<pre style="color:red;padding:10px;font-size:11px;">[BOOT_ERR] ' + e.message + '</pre>';
  }
}

if (document.readyState === "loading") {
  window.addEventListener("DOMContentLoaded", () => init());
} else {
  init();
}
