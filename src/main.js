const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;

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
  const totalSec = Math.floor(ms / 1000);
  const d = Math.floor(totalSec / 86400);
  const h = Math.floor((totalSec % 86400) / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  if (d > 0) return `${d} 天 ${h} 时`;
  if (h > 0) return `${h} 时 ${m} 分`;
  return `${m} 分`;
}

function fmtTimeUTC(iso) {
  if (!iso) return "—";
  try {
    return new Date(iso).toLocaleTimeString("zh-CN", {
      hour: "2-digit", minute: "2-digit", timeZone: TZ, hour12: false,
    });
  } catch { return "—"; }
}

// ─── 状态行 ──────────────────────────────────────────────
function setStatus(msg, isErr) {
  const el = document.getElementById("status");
  el.textContent = msg;
  el.classList.toggle("dim", !!isErr);
}

// ─── 单条进度条 ──────────────────────────────────────────
// MiniMax 字段语义：`percentage` 是"剩余 %"（剩余 ≥60% 绿，30–60 黄，<30 红）
function paintRemainingBar(fillId, pctId, restId, value, total, dropText) {
  const fillEl = document.getElementById(fillId);
  const textEl = document.getElementById(pctId);
  const restEl = document.getElementById(restId);

  if (value == null || total == null || total <= 0) {
    fillEl.style.width = "0%";
    fillEl.className = "bar-fill";
    textEl.textContent = "无数据";
    if (restEl) restEl.textContent = dropText || "—";
    return;
  }
  const rem = pct(value);
  fillEl.style.width = `${rem}%`;
  fillEl.className = "bar-fill " + (
    rem >= 60 ? "good" : rem >= 30 ? "warn" : "bad"
  );
  textEl.textContent = `剩 ${rem}%`;
  if (restEl) restEl.textContent = dropText || "—";
}

// ─── 模型行渲染 ──────────────────────────────────────────
function renderModels(list) {
  const root = document.getElementById("models-list");
  root.innerHTML = "";
  if (!Array.isArray(list) || list.length === 0) {
    root.innerHTML = '<div class="dim" style="padding:6px 0;">无数据</div>';
    return;
  }
  list
    .filter(m => num(m.weeklyTotal) > 0)
    .forEach((m) => {
      const rem = m.weeklyPercentage;
      const cls = rem >= 60 ? "" : rem >= 30 ? "warn" : "bad";
      const row = document.createElement("div");
      row.className = "model-row";
      row.innerHTML = `
        <span class="mname">${escapeHtml(m.name || "?")}</span>
        <span class="mbar-bg"><span class="mbar-fill ${cls}" style="width:${rem}%"></span></span>
        <span class="mpct">${rem}%</span>
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
    const m = arr[0];

    const five = {
      rem:   m.current_interval_remaining_percent,
      total: m.current_interval_total_count,
      reset: m.remains_time,
      start: m.start_time,
      end:   m.end_time,
    };
    const week = {
      rem:   m.current_weekly_remaining_percent,
      total: m.current_weekly_total_count,
      reset: m.weekly_remains_time,
      unlimited: m.current_weekly_total_count === 0 && m.current_weekly_remaining_percent == null,
    };

    paintRemainingBar(
      "fiveHour-fill", "fiveHour-pct", "fiveHour-resets",
      five.rem, five.total,
      `重置 ${fmtDuration(five.reset)} · ${fmtTimeUTC(five.start)}–${fmtTimeUTC(five.end)}`,
    );

    if (week.unlimited) {
      document.getElementById("sevenDay-fill").style.width = "100%";
      document.getElementById("sevenDay-fill").className = "bar-fill good";
      document.getElementById("sevenDay-pct").textContent = "无限";
      document.getElementById("sevenDay-resets").textContent = "本周不限量";
    } else {
      paintRemainingBar(
        "sevenDay-fill", "sevenDay-pct", "sevenDay-resets",
        week.rem, week.total,
        `重置 ${fmtDuration(week.reset)}`,
      );
    }

    document.getElementById("model-name").textContent = m.model_name || "MiniMax";
    document.getElementById("model-block").style.display = "block";

    renderModels(arr);

    const ts = new Date(snap.fetched_at).toLocaleTimeString("zh-CN", {
      hour: "2-digit", minute: "2-digit", second: "2-digit", timeZone: TZ, hour12: false,
    });
    setStatus(`key=${snap.key_source} · ${ts}`, false);
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
