# Claude Usage Widget

一个面向 Windows 的轻量桌面悬浮组件，用来查看 MiniMax、DeepSeek 或智谱在 Claude Code 中的当前用量与模型。

应用根据 Claude Code 配置自动识别当前提供商，并直接读取对应官方 API，不依赖 `claude-monitor`、state 文件或其他常驻中间层。

## 主要功能

- 类 Apple 液态玻璃风格：透明、无边框、置顶，并适配深浅色环境；Windows 开启“透明效果”时使用原生背景模糊与 DPI 感知圆角裁切，关闭时自动保留轻量半透明回退。
- MiniMax 显示 5 小时和 7 天窗口的已用比例与重置倒计时。
- DeepSeek 显示 API 账户余额、赠送/充值余额明细与当前可用状态。
- 智谱显示 GLM Coding Plan 的 5 小时 Token 用量和月度 MCP 用量。
- 显示从 Claude Code 环境变量或配置文件中检测到的当前模型；无法可靠检测时明确显示“未检测到”。
- 连接成功且窗口可见时，每 30 秒自动刷新；也可通过标题栏刷新按钮立即同步。
- 支持展开、折叠为 52px 高的紧凑栏，以及双击标题区域切换折叠状态。
- 通过“连接设置”更新或移除本机保存的 Key；环境变量提供的 Key 只提示来源，不会在界面中读取或显示。
- 后台每 5 秒检测 `claude.exe`：Claude Code 启动时显示小组件，退出时自动隐藏。

## 安装

从 [GitHub Releases](https://github.com/zju-enze/claude-usage-widget/releases) 下载最新的 `.msi` 或 NSIS `.exe` 安装包并运行。

首次连接时，应用会根据 `ANTHROPIC_BASE_URL`、模型和专用 Key 环境变量识别提供商；未识别到其他提供商时保持原有 MiniMax 默认行为。也可以在安全连接界面输入当前提供商的 API Key，Key 验证通过后才会保存。

标题栏关闭按钮只隐藏小组件，不退出后台进程；当 Claude Code 退出并再次启动时，小组件会重新显示。

## API Key 与安全

应用先按以下顺序识别当前提供商：

1. `ANTHROPIC_BASE_URL` 的官方域名：`api.deepseek.com`、`open.bigmodel.cn` / `api.z.ai` 或 MiniMax 域名。
2. 当前模型名：`deepseek-*`、`glm-*` 或 `minimax-*`。
3. 提供商专用 Key 环境变量；均未识别时默认 MiniMax，以兼容现有安装。

随后依次从进程环境变量、Windows 当前用户环境变量、用户目录下的 `.claude/settings.local.json` / `.claude/settings.json` 和本机加密文件查找 Key。支持的字段如下：

| 提供商 | 专用字段 | Claude Code 字段 |
|---|---|---|
| MiniMax | `MINIMAX_API_KEY`、`MINIMAX_CP_TOKEN` | Base URL 指向 MiniMax 时支持 `ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_API_KEY` |
| DeepSeek | `DEEPSEEK_API_KEY` | Base URL 指向 DeepSeek 时支持 `ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_API_KEY` |
| 智谱 | `ZAI_API_KEY`、`ZHIPUAI_API_KEY`、`BIGMODEL_API_KEY` | Base URL 指向智谱时支持 `ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_API_KEY` |

外部配置优先于本机保存值。使用环境变量或 Claude Code 配置时，无需在界面中再次保存 Key；修改配置后请重新启动应用。

在 Windows 上，界面保存的 Key 使用 Windows DPAPI 加密，密文位于：

```text
%APPDATA%\claude-usage-widget\key.bin           # MiniMax，兼容原路径
%APPDATA%\claude-usage-widget\key-deepseek.bin  # DeepSeek
%APPDATA%\claude-usage-widget\key-zhipu.bin     # 智谱
```

DPAPI 密文绑定当前 Windows 用户，并非 AES-GCM 文件。应用不会把 Key 明文写入磁盘。

Key 的网络用途只有验证和查询当前提供商用量。后端使用仅 HTTPS、禁止重定向的 `reqwest` 客户端，并固定请求官方端点：

| 提供商 | 官方端点 | 读取内容 |
|---|---|---|
| MiniMax | `GET https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains` | Coding Plan 5 小时 / 7 天用量 |
| DeepSeek | `GET https://api.deepseek.com/user/balance` | API 余额与可用状态 |
| 智谱 | `GET https://open.bigmodel.cn/api/monitor/usage/quota/limit` | GLM Coding Plan Token / MCP 配额 |

智谱国际站配置 `https://api.z.ai/api/anthropic` 时，仅切换到同域名的官方 monitor 端点。MiniMax 与 DeepSeek 使用 Bearer 认证；智谱 monitor 请求与官方 `glm-plan-usage` 插件保持一致，直接在 `Authorization` 头中发送 Token。

Key 不会发送到其他网络地址，也不会写入应用日志或回传到用量界面。

## 界面操作

| 操作 | 方式 |
|---|---|
| 拖动 | 按住标题区域拖动 |
| 刷新 | 点击标题栏刷新按钮；连接后默认每 30 秒自动刷新 |
| 折叠 / 展开 | 点击“收起 / 展开”，或双击标题区域 |
| 连接设置 | 更新或移除本机 DPAPI 密钥，查看环境变量来源提示 |
| 隐藏 | 点击关闭按钮；不会退出后台进程 |

有百分比的指标按已用量显示：低于 70% 为绿色，70%–89% 为黄色，90% 及以上为红色。DeepSeek 官方余额接口不提供总预算，因此余额以金额显示，不虚构已用百分比。

## 本地开发

要求：

- Windows 10/11 与 WebView2
- Node.js 18+
- Rust stable 与 Windows MSVC 构建工具链
- 至少一个可用的 MiniMax、DeepSeek 或智谱 API Key

```powershell
git clone https://github.com/zju-enze/claude-usage-widget.git
cd claude-usage-widget
npm install
npm run tauri dev
```

## 验证与构建

```powershell
npm test
npm run check
npm run check:rust
npm run tauri build
```

`npm run check` 检查前端 JavaScript 语法，`npm test` 运行前端数据处理测试，`npm run check:rust` 检查全部 Rust targets。发布构建产物位于 `src-tauri\target\release\bundle\`。

## 项目结构

```text
claude-usage-widget/
├── src/                    # HTML、液态玻璃样式和前端交互
├── src-tauri/              # Tauri/Rust 后端、DPAPI 与窗口联动
├── tests/                  # 前端数据处理测试
├── scripts/                # Windows 辅助脚本
├── package.json
└── README.md
```

## License

MIT
