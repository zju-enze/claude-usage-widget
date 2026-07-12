const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;

const REFRESH_MS = 5000;

function pct(n) {
  if (n == null || isNaN(n)) return 0;
  return Math.max(0, Math.min(100, Number(n)));
}

function formatNum(n) {
  if (n == null) return "—";
  return Number(n).toLocaleString("en-US");
}

function timeUntil(iso) {
  if (!iso) return "—";
  const target = new Date(iso).getTime();
  if (isNaN(target)) return "—";
  const diff = target - Date.now();
  if (diff <= 0) return "now";
  const h = Math.floor(diff / 3.6e6);
  const m = Math.floor((diff % 3.6e6) / 6e4);
  if (h >= 24) return `${Math.floor(h / 24)}d ${h % 24}h`;
  return `${h}h ${m}m`;
}

function classifyFill(el, usedPct) {
  el.classList.remove("warn", "bad");
  if (usedPct >= 90) el.classList.add("bad");
  else if (usedPct >= 70) el.classList.add("warn");
}

function setBar(fillEl, textEl, used, limit, resetsAt, hideText) {
  const usePct = pct(used);
  const capPct = limit > 0 ? (used / limit) * 100 : 0;
  const display = Math.min(100, capPct);
  fillEl.style.width = `${display.toFixed(1)}%`;
  classifyFill(fillEl, capPct);
  if (textEl) {
    if (hideText) {
      textEl.textContent = limit > 0
        ? `${usePct.toFixed(0)}%`
        : "—";
    } else {
      textEl.textContent = limit > 0
        ? `${formatNum(used)} / ${formatNum(limit)} · 重置 ${timeUntil(resetsAt)}`
        : `${formatNum(used)} / ?`;
    }
  }
}

function renderModels(models) {
  const list = document.getElementById("models-list");
  list.innerHTML = "";
  if (!models || !Array.isArray(models) || models.length === 0) {
    list.innerHTML = '<div style="color:var(--fg-dim);padding:4px 0;">无模型分布数据</div>';
    return;
  }
  models
    .filter((m) => Number(m.percentage) > 0)
    .sort((a, b) => b.percentage - a.percentage)
    .slice(0, 5)
    .forEach((m) => {
      const row = document.createElement("div");
      row.className = "model-row";
      row.innerHTML = `
        <span class="name">${escapeHtml(m.family || "?")}</span>
        <span class="mbar-bg"><span class="mbar-fill" style="width:${Math.min(100, m.percentage).toFixed(1)}%"></span></span>
        <span class="pct">${m.percentage.toFixed(0)}%</span>
      `;
      list.appendChild(row);
    });
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

let lastError = null;

async function refresh() {
  const statusEl = document.getElementById("status");
  try {
    const state = await invoke("read_monitor_state");
    if (!state.found) {
      statusEl.textContent = "未找到 state 文件 — 请先运行 claude-monitor --write-state";
      document.getElementById("status").classList.add("dim");
      return;
    }
    const raw = state.raw || {};
    const five = raw.limits?.five_hour || {};
    const seven = raw.limits?.seven_day || {};
    const local = raw.local || {};
    const models = local.model_distribution || [];

    setBar(
      document.getElementById("fiveHour-fill"),
      document.getElementById("fiveHour-text"),
      five.tokens_used,
      five.token_limit,
      five.resets_at,
      false
    );

    setBar(
      document.getElementById("sevenDay-fill"),
      document.getElementById("sevenDay-text"),
      seven.tokens_used,
      seven.token_limit,
      seven.resets_at,
      false
    );

    document.getElementById("messages").textContent = local.sent_messages != null
      ? `${formatNum(local.sent_messages)} 条`
      : "—";
    document.getElementById("resets-in").textContent = timeUntil(five.resets_at);

    renderModels(models);

    const conf = raw.confidence || "unknown";
    const planName = raw.plan || "?";
    statusEl.textContent = `${planName} · ${conf} · 更新于 ${new Date(state.fetched_at).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit" })}`;
    statusEl.classList.remove("dim");
    lastError = null;
  } catch (e) {
    statusEl.textContent = `读取失败: ${e}`;
    statusEl.classList.add("dim");
    lastError = e;
  }
}

function setupButtons() {
  const win = getCurrentWindow();
  document.getElementById("btn-refresh").addEventListener("click", (e) => {
    e.stopPropagation();
    refresh();
  });

  document.getElementById("btn-collapse").addEventListener("click", (e) => {
    e.stopPropagation();
    const w = document.getElementById("widget");
    w.classList.toggle("collapsed");
    const c = w.classList.contains("collapsed");
    document.getElementById("btn-collapse").textContent = c ? "+" : "−";
    // 折叠后高度变化，让 Tauri 调整窗口高度
    if (c) {
      win.setSize(new (win.constructor || Object).Size(280, 36)).catch(() => {});
    } else {
      win.setSize(new (win.constructor || Object).Size(280, 200)).catch(() => {});
    }
  });

  document.getElementById("btn-close").addEventListener("click", (e) => {
    e.stopPropagation();
    win.hide();
  });

  // 双击标题栏隐藏/显示
  document.querySelector("header").addEventListener("dblclick", (e) => {
    e.stopPropagation();
    if (win.isVisible()) win.hide();
    else win.show();
  });
}

window.addEventListener("DOMContentLoaded", () => {
  setupButtons();
  refresh();
  setInterval(refresh, REFRESH_MS);
});
