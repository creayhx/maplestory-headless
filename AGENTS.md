# AGENTS.md

给在本仓库工作的编码 agent 的速查。**详细设计在 `docs/`，这里只讲"是什么、在哪、别踩什么"。**

## 项目是什么

MapleStory CMS 079 的自动化挂机 bot —— 完全走网络协议驱动角色，**无渲染、无 UI、无游戏资源**。

一个进程内可同时挂 N 个账号（**无子进程、无 IPC**）：核心层完全不知道"多开"这回事，
多开由事件的路由与前端承担。

## 包结构

| 路径 | 包名 | 作用 |
|---|---|---|
| `src/` | `openstory-bot` | **核心**：协议/加密/状态机/指令引擎/配置。`lib.rs` + `main.rs`（headless 纯命令行） |
| `crates/console/` | `openstory-console` | **终端控制台**（主力前端）。实现全在 `src/lib.rs`；`src/main.rs` 与 `src/bin/tui.rs` 只是 `run()` 的转发，两个可执行文件行为必然一致 |
| `crates/console-lib/` | `openstory-console-lib` | **前端共享层**：文本/配色/中文渲染/日志缓冲/命令规格/补全/状态快照。注意 `theme.rs` 依赖 ratatui —— 未来的服务器版前端按本 crate `lib.rs` 的规划直接用「核心 + `emit` JSON + `Snapshot`」，**不整体复用**它 |
| `crates/webui/` | `openstory-webui` | **独立 Web 配置编辑器**（tiny_http + 单页 + `/api/config`）。与 bot 无进程内关联，直接读写配置档案 |
| `crates/resource/` | `openstory-resource` | WZ 数据导出，生成名字字典（物品/怪物/地图/传送点）。与 bot 运行无关 |

其它目录：`docs/` 设计文档 · `profiles/` 配置档案（仓库只带 2 份**测试夹具**档案）·
`tests/` 根 crate 集成测试 · `scripts/` + `test-support/` 驱动真实 TUI 的 PowerShell/C# 工具。

Cargo workspace：根包 + 上表 4 个成员（`resolver = "2"`，edition 2021）。

## 改代码前必须知道的六件事

1. **事件按会话路由**：`emit::with_session_emitter` 用 tokio task-local 把事件送进对应会话的队列。
   核心层不知道有几个 bot —— 新功能别在核心层引入"当前账号"这类全局概念。
2. **配置是分层的**：`RuntimeConfig` 支持 `include` 链（按顺序合并，同 `id` 的条目覆盖上一层）。
   档案通常是"只写差异"的薄文件，不是完整配置。
3. **每个会话的账号来自它自己的档案**：建模板一律走 `sessions::session_template(base, path)`。
   **不要**用 `Config::apply_login` —— 那是"只填空缺"语义，会把上一个号的账号带过来
   （曾经导致两个号登成同一个、服务器来回踢）。
4. **日志缓冲只有一份在渲染位置**：`App.log` 就是**选中会话**的活缓冲（切换时用 `LogBuf::swap` 换出去）。
   往某个会话的日志写要用 `SessionSet::note_at`，**不要**直接 `s.note(...)` —— 那会写进一份
   已经被换出去的旧缓冲，永远不上屏。
5. **命令表唯一来源**：`crates/console-lib/src/command_spec.rs`。补全、帮助、解析器一致性测试
   都从它推导 —— **新增/修改命令只改这一处**。
6. **中文对齐按显示列**：用 `text::pad_width` / `text::truncate_width`，
   **不要**用 `format!("{:<22}")`（按 `char` 个数补齐，一个汉字算 1 却占 2 列，整张表会逐列错开）。

## 开发规范

- **注释用中文，写"为什么"而不是"做了什么"。** 本仓库大量注释记录的是"踩过的坑 + 为什么不能那样写"，
  这是刻意的 —— 改代码时请沿用，不要删。
- `CmdSpec` 一律用 builder（`.arg(..).subs(..).dyn_args(..)`）。**不要**用结构体更新语法
  `CmdSpec { arg: X, ..cmd(..) }` —— 它会静默覆盖同名字段。
- 共享 `Mutex` 在可能 panic 的路径上取锁，用 `unwrap_or_else(|e| e.into_inner())`，
  **不要** `.unwrap()` / `.ok()`。毒化被当成"数据不存在"会让崩溃后的诊断信息全部丢失。
- 键位分发里，带 `Alt` 守卫的分支必须排在裸 `KeyCode::Up` / `Down` **之前**，否则永远不可达。
- 多开相关改动先问自己一句：**这个状态属于"某个会话"还是属于"App"？** 放错的典型症状是
  串台、切换后不刷新、退出条件永远不满足。
- 不要提交**自己的**账号数据与个人配置：`config.json`、`profiles/.manager.json`
  （后者是**明文密码**，已在 `.gitignore` 里）。`git status` 里这些文件的改动通常不是你的。
  `profiles/本地服_*.json` 是渲染/联调测试的夹具（**不含密码**），删了测试会挂。

## 测试

```bash
cargo test --workspace --no-fail-fast   # 必须加 --no-fail-fast, 否则第一个失败后面的套件根本不跑
cargo build --workspace --all-targets   # 期望: 零警告
```

- **联调测试需要本地服** `127.0.0.1:8484`（游戏服还有 `7575`）。账号凭据直接写在联调用例里，
  不要往文档、提交信息或新文件里抄。
  行为不一致：`tests/live_warp_test.rs` 连不上会**直接失败**；`crates/console/tests/` 下那两个
  会打印 `SKIP` 并返回。看到 `live_warp_test` 失败先确认服务器开了没。
- **渲染回归**在 `crates/console/tests/render_regression.rs`：驱动真实 `App` + `ui::render`，
  对真实帧缓冲做断言。**改 UI 一定要先看它**。断言左栏时用 `sidebar_of()`（只取 24 列），
  整屏匹配会串到右侧面板去。
- `cargo test` 跑成员 crate 时的 CWD 是 **crate 目录**；需要读仓库根文件的测试会自己 chdir。
- `session_layer_live.rs` 里有一把**进程内串行闸门**（`live_guard()`）：本地服扛不住并发登录，
  同一个二进制里的联调用例必须串起来跑。别把它们改成并行，也别靠"放宽超时"绕开 —— 那会把真实回归一起掩盖掉。
