# Claude Usage Widget

一个常驻桌面右上角的浮动卡片，显示 **MiniMax Token Plan** 的实时用量（5 小时窗口 + 7 天周配额 + 各模型剩余 % + 重置倒计时）。

后端数据来自 MiniMax Coding Plan 官方 API：

```
GET https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains
Authorization: Bearer <sk-cp-...>
```

直接调用、返回 JSON、解析后渲染。不依赖 claude-monitor 之类的中间层。

## 字段预览

```
┌─ Claude Usage ───── ✕ ┐
│ 5 小时                  │
│ ▓▓▓░░░░░░░ 剩 60%        │
│ 重置 2h 14m · 13:42–18:42│
│ 7 天                    │
│ ▓▓░░░░░░░░ 剩 12%        │
│ 重置 4d 03h             │
│ 模型 MiniMax-Text-01    │
│ ▸ 各模型剩余 %           │
└────────────────────────┘
```

进度条是"剩余 %"：绿色表示剩得多（≥60%）、黄色注意（30–60%）、红色要省着用（<30%）。

## 环境要求

- **Rust** stable（1.78+）
- **Node.js** 18+
- **WebView2**（Win11 / 较新的 Win10 预装）
- **MiniMax Coding Plan**（Plus/Pro/Max 等 Token Plan 订阅），sk-cp key 一个

## 安装

```bash
# 1. 克隆本仓库
git clone https://github.com/zju-enze/claude-usage-widget.git
cd claude-usage-widget

# 2. 安装 Node 依赖
npm install

# 3. 设置 sk-cp key（两种方式任选）
#    方式 a：在当前 shell 设环境变量（仅本会话）
$env:MINIMAX_API_KEY = "eyJhbGciOi..."
#    方式 b：写入 .env.local（自动加载，不入 git）
"eyJhbGciOi..." | Out-File -Encoding ascii .env.local
"MINIMAX_API_KEY" | Out-File -Encoding ascii -Append .env.local

# 4. 启动（首次 cargo build 较慢，需要 5–10 分钟）
npm run tauri dev
```

启动后悬浮窗自动出现在主屏右上角。**首次自动拉一次 API**，状态行会显示 `key=env · 18:42:01`。

> 找不到 key 的话，进 https://platform.minimaxi.com/user-center/basic-information/interface-key 复制 sk-cp 开头的那个。

## 数据怎么来的

悬浮窗**每 30 秒**直接调用 MiniMax 官方 API：

```
GET https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains
Authorization: Bearer <MINIMAX_API_KEY>
referer: https://platform.minimaxi.com/
```

返回 JSON 长这样（节选）：

```json
{
  "model_remains": [
    {
      "model_name": "MiniMax-Text-01",
      "current_interval_total_count": 5000,
      "current_interval_remaining_percent": 87,
      "remains_time": 1234567,
      "current_weekly_total_count": 50000,
      "current_weekly_remaining_percent": 100,
      "weekly_remains_time": 6789012,
      "start_time": "2026-07-12T13:00:00+08:00",
      "end_time":   "2026-07-12T18:00:00+08:00"
    }
  ]
}
```

字段含义：

| 字段 | 含义 |
|---|---|
| `current_interval_*` | 当前 5 小时窗口（默认）|
| `current_weekly_*` | 当前 7 天窗口 |
| `*_remaining_percent` | **剩余 %**（不是已用 %）|
| `remains_time` / `weekly_remains_time` | 距下次重置的毫秒数 |

进度条颜色：

- 剩 ≥60% → 绿色（healthy）
- 剩 30–60% → 黄色（warning）
- 剩 <30% → 红色（urgent）

## 开机自启（可选）

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install-startup.ps1 -Install
```

安装两个启动项：

- `ClaudeUsageMonitor.bat` — 后台跑 monitor 守护
- `ClaudeUsageWidget.bat` — 启动悬浮窗（调用 `npm run tauri dev`）

> ⚠️ `npm run tauri dev` 启动慢，每次重启都要等一会儿。建议先打包 release 版本再绑自启（见下节）。

## 打包 release（推荐用于自启场景）

```bash
cd "F:/projects/claude-usage-widget"
npm run tauri build
```

产物：

- `src-tauri/target/release/claude-usage-widget.exe`（直接双击运行）
- `src-tauri/target/release/bundle/{msi,nsis}/`（安装包）

自启脚本里把 `npm run tauri dev` 换成 `start-exe.bat`，指向 release 下的 `.exe`：

```bat
@echo off
start "" "F:\projects\claude-usage-widget\src-tauri\target\release\claude-usage-widget.exe"
```

## 字段说明

| 字段 | 来源 | 说明 |
|---|---|---|
| **5h** | `limits.five_hour` | 当前 5 小时会话窗口 |
| **7d** | `limits.seven_day` | 当前 7 天周配额（Pro/Max5/Max20 才有；custom 可能为 null） |
| **消息** | `local.sent_messages` | 当前会话已发出消息数 |
| **重置** | `limits.five_hour.resets_at` | 距下次重置倒计时 |
| **模型分布** | `local.model_distribution` | 按 `family` 聚合的 token 占比 |

颜色规则：

- `>90%` 红色
- `>70%` 黄色
- 其它 绿色

## 常用操作

| 操作 | 方式 |
|---|---|
| **拖动** | 鼠标按住顶部标题栏 |
| **刷新** | 标题栏 `⟳` 按钮 / 自动每 5 秒 |
| **折叠** | `−` 按钮 — 收缩成 36px 小条 |
| **关闭** | `×` 按钮 — 隐藏窗口（不退出进程） |
| **再次显示** | 双击标题栏唤出，或重新执行启动脚本 |

## 常见问题

**Q: 标题栏显示 `未找到 state 文件`？**
先单次跑一次 `claude-monitor --once --write-state` 生成文件，再启动悬浮窗。

**Q: 数据一直不刷新？**
monitor 没在持续运行。`scripts/start-monitor.bat` 或自己用 `claude-monitor --write-state --refresh-rate 5`。

**Q: `custom` 计划下 5h 显示不正常（百分比爆炸）？**
`custom` 计划是基于你历史 P90 自动估的限额，第一次启动没有历史时会偏小。跑几天让 monitor 累积历史后会准。

**Q: 窗口被任务栏遮住 / 想后台？**
Tauri 2 + Win11 上 `alwaysOnTop: true` + `skipTaskbar: true` 已配置。标题栏拖到任意位置，下方空白处双击会隐藏/显示。

## 目录结构

```
claude-usage-widget/
├── src/                       # 前端 (HTML/CSS/JS)
│   ├── index.html
│   ├── main.js
│   └── styles.css
├── src-tauri/                 # Rust 后端
│   ├── src/
│   │   ├── main.rs
│   │   └── lib.rs              # read_monitor_state 命令
│   ├── tauri.conf.json         # 浮动窗配置（无边框、置顶）
│   └── Cargo.toml
├── scripts/
│   ├── start-monitor.bat       # monitor 守护脚本
│   ├── start-widget.bat        # 启动悬浮窗
│   ├── hide-window.vbs         # 后台启动 vbs 包装
│   └── install-startup.ps1     # 开机自启安装 / 卸载
└── README.md
```

## License

MIT
