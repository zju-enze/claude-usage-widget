# Claude Usage Widget

一个常驻桌面右上角的浮动卡片，实时显示 Claude Code 用量（5h 会话 + 7d 周配额 + 消息数 + 模型分布）。

后端数据来自 [Claude-Code-Usage-Monitor](https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor) 的 `--write-state` 输出（写到 `~/.claude-monitor/state/latest.json`）。
本项目只做悬浮显示层，不重复实现用量计算 / 订阅识别。

## 字段预览

```
┌─ Claude Usage ───── ✕ ┐
│ 5 小时                  │
│ ▓▓▓▓▓▓░░░░ 60% · 2h 14m │
│ 7 天                    │
│ ▓▓░░░░░░░░ 12% · 4d 03h │
│ ─────────────           │
│ 消息     54 条          │
│ 重置     2h 14m         │
│ ▸ 模型分布              │
└────────────────────────┘
```

## 环境要求

- **Rust** stable（1.78+）
- **Node.js** 18+
- **WebView2**（Win11 / 较新的 Win10 预装）

## 安装

```powershell
# 1. 装 Claude-Code-Usage-Monitor（数据源）
uv tool install claude-monitor

# 2. 克隆本仓库
git clone <repo-url>
cd claude-usage-widget

# 3. 安装依赖
npm install

# 4. 启动（首次 cargo build 较慢，需要 5–10 分钟）
npm run tauri dev
```

启动后悬浮窗自动出现在主屏右上角。

> 想立即看到数据的话，先单次跑一次：
> ```bash
> claude-monitor --once --write-state
> ```
> 这会立刻生成 `~/.claude-monitor/state/latest.json`，悬浮窗第一次刷新就能看到内容。

## 后台常驻 monitor

悬浮窗只**读取** state.json，不会启动 monitor。要让数据持续更新，每 5 秒跑一次 monitor：

```bash
# 手动跑一次（前台，ctrl+c 退出）
claude-monitor --write-state --refresh-rate 5 --no-clear

# 或者用项目自带的脚本（出错自动重启）
scripts/start-monitor.bat
```

`start-monitor.bat` 把日志写到 `%USERPROFILE%\.claude-monitor\monitor.log`。

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
