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

function fmtDuration(value) {
  if (value == null || value <= 0) return "—";
  // 自动检测单位：如果值看起来像秒（< 1 千万），按秒；否则按毫秒
  const looksLikeSeconds = value < 1e7;
  const totalSec = looksLikeSeconds ? Math.floor(value) : Math.floor(value / 1000);
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
  list
    .filter(m => num(m.weeklyTotal) > 0)
    .forEach((m) => {
      // `weeklyPercentage` 是"剩余 %"。已用 = 100 - rem
      let rem = Number(m.weeklyPercentage);
      if (!isNaN(rem) && rem <= 1) rem = rem * 100;
      const used = isNaN(rem) ? 0 : Math.max(0, Math.min(100, 100 - rem));
      const cls = used >= 90 ? "bad" : used >= 70 ? "warn" : "";
      const row = document.createElement("div");
      row.className = "model-row";
      row.innerHTML = `
        <span class="mname">${escapeHtml(m.name || "?")}</span>
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

    function usedFromRemaining(rem) {
      if (rem == null) return null;
      // API 在不同账户/计划下可能返回 0–100（剩余）或者 0–1（小数）。
      // 安全处理两种情况：<1 视为小数，要 × 100
      let v = Number(rem);
      if (v <= 1) v = v * 100;
      // 业务侧语义是"已用"，所以已用 = 100 - 剩余
      return Math.max(0, Math.min(100, 100 - v));
    }

    paintBar(
      "fiveHour-fill", "fiveHour-pct", "fiveHour-resets",
      usedFromRemaining(five.rem),
      five.reset,
      `${fmtTimeUTC(five.start)}–${fmtTimeUTC(five.end)}`,
    );

    if (week.unlimited) {
      document.getElementById("sevenDay-fill").style.width = "0%";
      document.getElementById("sevenDay-fill").className = "bar-fill good";
      document.getElementById("sevenDay-pct").textContent = "无限制";
      document.getElementById("sevenDay-resets").textContent = "本周不限量";
    } else {
      paintBar(
        "sevenDay-fill", "sevenDay-pct", "sevenDay-resets",
        usedFromRemaining(week.rem),
        week.reset,
        "本周",
      );
    }
    }

    document.getElementById("model-name").textContent = m.model_name || "MiniMax";
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
