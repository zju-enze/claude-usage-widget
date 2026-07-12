let invoke = null;
let getCurrentWindow = null;
try {
  invoke = window.__TAURI__.core.invoke;
  getCurrentWindow = window.__TAURI__.window.getCurrentWindow;
} catch (e) {
  document.body.innerHTML += '<pre style="color:red;padding:8px;font-size:10px">[FE_TOP_ERR] ' + e.message + '</pre>';
}
const tlog = (level, msg) => {
  if (invoke) invoke("frontend_log", { level, msg }).catch(() => {});
  console[level === "info" ? "log" : level]("[FE]", msg);
};
tlog("info", "main.js loaded, window=" + (getCurrentWindow ? "ok" : "MISSING"));

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

// ─── 状态行 ──────────────────────────────────────────────
function setStatus(msg, isErr) {
  const el = document.getElementById("status");
  el.textContent = msg;
  el.classList.toggle("dim", !!isErr);
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
}

// ─── 模型行渲染 ──────────────────────────────────────────
function renderModels(list) {
  const root = document.getElementById("models-list");
  root.innerHTML = "";
  if (!Array.isArray(list) || list.length === 0) {
    root.innerHTML = '<div class="dim" style="padding:6px 0;">无数据</div>';
    return;
  }
  list.forEach((m) => {
    // current_weekly_remaining_percent 是剩余%，已用 = 100 - rem
    const rem = Number(m.current_weekly_remaining_percent);
    const used = isNaN(rem) ? 0 : Math.max(0, Math.min(100, 100 - rem));
    const cls = used >= 90 ? "bad" : used >= 70 ? "warn" : "";
    const row = document.createElement("div");
    row.className = "model-row";
    const status = m.current_interval_status === 1 ? "" :
                   m.current_interval_status === 3 ? " (赠送)" : " (?)";
    row.innerHTML = `
      <span class="mname">${escapeHtml(m.model_name || "?")}${status}</span>
      <span class="mbar-bg"><span class="mbar-fill ${cls}" style="width:${used}%"></span></span>
      <span class="mpct">${used.toFixed(0)}%</span>
    `;
    root.appendChild(row);
  });
}

// ─── 主刷新 ──────────────────────────────────────────────
async function refresh() {
  try {
    const snap = await invoke("fetch_minimax_usage");
    if (!snap.found) {
      setStatus(snap.error || "读取失败", true);
      return;
    }
    const arr = snap.raw?.model_remains;
    if (!Array.isArray(arr) || arr.length === 0) {
      setStatus("API 返回无 model_remains", true);
      return;
    }

    // 找出主模型：current_interval_status === 1 表示在用（=3 是赠送额度）
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

    document.getElementById("model-name").textContent = primary.model_name || "?";
    document.getElementById("model-block").style.display = "block";

    renderModels(arr);

    const ts = new Date(snap.fetched_at).toLocaleTimeString("zh-CN", {
      hour: "2-digit", minute: "2-digit", second: "2-digit", timeZone: TZ, hour12: false,
    });
    setStatus(`${snap.key_source === "missing" ? "no-key" : "env"} · ${ts}`, false);
  } catch (e) {
    setStatus(`读失败: ${e}`, true);
  }
}

function setupButtons() {
  const win = getCurrentWindow();
  document.getElementById("btn-refresh").addEventListener("click", (e) => { e.stopPropagation(); refresh(); });

  document.getElementById("btn-collapse").addEventListener("click", async (e) => {
    e.stopPropagation();
    const w = document.getElementById("widget");
    const collapsed = w.classList.toggle("collapsed");
    document.getElementById("btn-collapse").textContent = collapsed ? "+" : "−";
    try {
      await win.setSize(new (win.constructor || Object).Size(280, collapsed ? 36 : 220));
    } catch {}
  });

  document.getElementById("btn-close").addEventListener("click", (e) => { e.stopPropagation(); win.hide(); });
  document.querySelector("header").addEventListener("dblclick", (e) => {
    e.stopPropagation();
    if (win.isVisible()) win.hide(); else win.show();
  });
}

window.addEventListener("DOMContentLoaded", () => {
  setupButtons();
  refresh();
  setInterval(refresh, REFRESH_MS);
});
