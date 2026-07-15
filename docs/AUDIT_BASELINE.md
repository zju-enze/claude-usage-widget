# Audit Baseline — Claude Usage Widget

记录于 v0.2 重构开始前，作为所有后续变更的对照基准。

## 1. 分支与提交

| 项 | 值 |
|---|---|
| Branch | `master` |
| HEAD commit | `b60141a chore: bump version to 0.1.2` |
| 工作树状态 | clean（无未提交改动）|
| Cargo.lock | 存在但**未 git 跟踪**（属于"幽灵文件"）|

最近 3 个提交：

```
b60141a chore: bump version to 0.1.2
71c8ae5 fix(ui): remove fabricated plan + drop model-remaining region
676cae5 feat(ui): Liquid Glass refresh
```

## 2. 技术栈

| 层 | 技术 |
|---|---|
| Desktop runtime | Tauri 2 |
| Rust edition | 2021 |
| HTTP client | reqwest 0.12 + rustls |
| Crypto（Windows）| Windows DPAPI（`windows` crate 0.58）|
| 前端 | 原生 HTML / CSS / JavaScript（无构建步骤）|
| WebView | Windows WebView2 |
| 时间 | chrono 0.4 |
| 注册表 | winreg 0.52 |

## 3. 文件结构（重构前）

```
.
├── README.md
├── package.json
├── package-lock.json
├── src/
│   ├── assets/
│   ├── index.html
│   ├── main.js
│   └── styles.css
├── src-tauri/
│   ├── Cargo.toml
│   ├── Cargo.lock           ← 未 git 跟踪
│   ├── build.rs
│   ├── capabilities/
│   │   └── default.json
│   ├── gen/
│   ├── icons/
│   ├── src/
│   │   ├── lib.rs            ← 600+ 行，包含 API、DPAPI、配置、窗口生命周期
│   │   └── main.rs
│   └── tauri.conf.json
├── scripts/
└── node_modules/
```

无 `.github/`（无 CI / Dependabot）。
无 `docs/`（本目录在本次重构中创建）。

## 4. 构建基线

| 命令 | 结果 |
|---|---|
| `cargo check` | ✅ 通过 5.22s |
| `cargo test --no-run` | ✅ 通过 31.32s（编译通过，**无任何单元测试**）|
| `cargo fmt --check` | ❌ 预期失败：当前 Rust 代码风格不符合 rustfmt 默认（`{` 不换行）|
| `cargo clippy --all-targets -- -D warnings` | ❌ 预期失败：当前代码 `eprintln!` / `unwrap_or_default()` / `let _ = ...` 等多处触发 lint |
| `node --check src/main.js` | ✅ 通过 |
| `npm test` | ❌ 无脚本（package.json 无 `test` 字段）|
| `npm run tauri build` | ✅ 上一次成功 55.10s |

## 5. 已知安全 / 可靠性问题（清单）

| 严重度 | 问题 |
|---|---|
| 🔴 P0 | `fetch_minimax_usage` 在 `eprintln!` 中打印完整 API 响应体（潜在 Key / 用户数据泄漏到 stderr）|
| 🔴 P0 | `frontend_log` 接收前端任意字符串并写入 stderr（生产可被注入）|
| 🔴 P0 | DPAPI 缓冲区（`CryptProtectData` / `CryptUnprotectData`）**未调用 `LocalFree`** —— 进程退出前一直泄漏 |
| 🔴 P0 | 非 Windows 平台把 Key 写入明文 `key.txt` |
| 🔴 P0 | `save_key_and_test` 先存盘再测，失败删除 —— 网络错误会误删旧 Key |
| 🔴 P0 | `probe_state` 与 `get_api_key` 解析规则不一致 |
| 🟠 P1 | `tauri.conf.json` 的 `csp: null`（完全无 CSP）|
| 🟠 P1 | `opener:default` 允许任意 URL |
| 🟠 P1 | `document.body.innerHTML += error` 仍存在于 init 错误路径（虽然参数受控）|
| 🟠 P1 | 前端 `setInterval(refresh, REFRESH_MS)` 多次触发会创建多个 timer |
| 🟠 P1 | API 返回 `serde_json::Value` 直接暴露给前端（`raw` 字段）|
| 🟠 P1 | `set_autohide` 是空实现，前端也未调用 |
| 🟡 P2 | HTTP client 每次 `fetch_minimax_usage` 重新创建 |
| 🟡 P2 | 解密后的 Key 每次请求都重新解密 |
| 🟡 P2 | 折叠尺寸 `setSize(new (win.constructor || Object).Size(...))` 用错 API |
| 🟡 P2 | `isVisible()` 未 await |
| 🟡 P2 | `read_plan_metadata` 返回 None + PlanMetadata 死结构 |
| 🟡 P2 | `setStatus` / `loadPlanMetadata` 前端空实现 |
| 🟡 P2 | README 描述的是旧 claude-monitor 项目（不是当前项目）|

## 6. 修改前 UI 行为

- 5 小时窗口：已用百分比 + 时间范围 + 重置倒计时（数据逻辑保持，**禁止修改**）
- 7 天窗口：同上
- 当前模型：从 `~/.claude/settings.json` 的 `env.ANTHROPIC_MODEL` 读取
- 套餐行：**已删除**（API 不返回，硬编码就是编造数据）
- 底部独立状态行 "env · 时间"：**已删除**
- "各模型剩余 %" 区域：**已删除**
- 窗口尺寸 360 × 198 px
- 折叠高度 44 px
- 自动 30s 定时刷新

## 7. 修改前性能（未测量）

本文档记录于重构开始前，**未实际测量**。重构完成后将在 `docs/PERFORMANCE.md` 中补充真实测量值。

## 8. 修改前测试覆盖

- Rust 单元测试：**0 个**
- 前端测试：**0 个**
- 集成测试：**0 个**

后续每个阶段会逐步补充。