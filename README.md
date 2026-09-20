# maplestory-headless

MapleStory CMS 079（冒险岛 079）的**无头自动化引擎**。

角色完全由**网络协议**驱动：自己实现加密握手、登录链路、地图与场景解析、移动与战斗包，
**不含渲染器、不含游戏画面、不读取客户端 WZ 资源**。因此它可以跑在任何地方 ——
服务器、容器、CI、一台只开终端的机器。

一个进程内可同时驱动 **N 个账号**（无子进程、无 IPC）；自带**全中文终端控制台**，
也可纯命令行运行 —— *同一内核，三种驾驶舱*。

---

## 它是什么 / 不是什么

| | |
|---|---|
| ✅ **是** | 协议层的完整实现：AES/Shanda 加密、登录→选服→选角→进频道→进图全链路、场景对象追踪、移动与战斗包构造 |
| ✅ **是** | 一个可编排的自动化引擎：规则引擎 + 任务引擎 + 功能分组，用声明式配置描述"什么条件下做什么" |
| ✅ **是** | 三种前端共用的一颗内核：终端控制台 / 纯命令行 / Web 配置编辑器 |
| ❌ **不是** | 游戏画面渲染器 —— 没有贴图、没有地图绘制、没有角色动画 |
| ❌ **不是** | 客户端外挂或内存修改器 —— 不注入进程、不读写游戏内存、不修改客户端文件 |

---

## 核心能力

### 🎯 自主挂机

| 能力 | 说明 |
|---|---|
| 自动打怪 | `hunt on` 自动寻怪 → 瞬移追怪 → 攻击 → 拾取，全流程闭环 |
| 追怪瞬移 | 接近、拾取、攻击全部走**绝对坐标瞬移**，不受平台边界钳制 |
| 吸怪 | `gather on` 把圈外怪物拉向角色（独立于打怪，可配合原地清场） |
| 技能攻击 | `hunt skill <id>` 真技能群攻；`hunt skill 0 <n>` 无技能群发（一个包打 n 只） |
| 攻击调参 | 攻击范围、冷却、目标上限、每目标命中段数、上报伤害全部可调 |
| 拾取过滤 | `hunt filter allow/deny` 按物品 id 白/黑名单，运行期即时生效 |
| 自动喝药 | HP/MP 低于阈值自动喝药（配置 `potion` 规则驱动） |
| 放置物挂机 | `reactor on` 全图打 Reactor + 掉落即拾 |
| 极限节奏 | `tick_ms` 可压到 **1ms** |

### 🧭 移动与地图

`move`（绝对坐标）、`town`（跨图传送门）、`warp`、`ports`、`channel`（换线）、
`reenter`（商城往返刷新全图对象）。传送导致的重新进图由内核自动接管。

### 🏪 交互全覆盖

| 系统 | 能力 |
|---|---|
| NPC 对话 | 6 种对话类型全覆盖：`next`/`prev`/`reply`/`yes`/`no`/`num`/`text`/`cancel` |
| 商店 | `buy` / `sell` / `sell tab`（卖整类）/ `sellammo`（一键卖弹药）/ `shop leave` |
| 商城 | `cashshop enter/buy/get/out` |
| 背包与装备 | `view inventory` / `use` / `drop` / `dropmeso` / `equip` / `unequip` |
| 技能 | `skill learn`（加点）/ `cast` / `info` / `buff` / `buff cancel` |
| 键盘绑定 | `keymap set` 改键 —— 配合满技能实现跨职业技能全 buff |
| 队伍 | `party create/invite/leave`，自动接受邀请 |
| 交易 | `trade invite/accept/confirm/put/meso`，可规则触发自动接受+确认 |
| 加点 | `ap <str\|dex\|int\|luk\|maxhp\|maxmp>` |

### 🎛 可编排性（规则 / 任务 / 功能组）

这是本项目区别于"一段脚本"的地方 —— 自动化逻辑是**声明式配置**，不是硬编码：

- **规则引擎**：`when`（谓词）+ `then`（动作），支持 `&&` / `||` 组合与冷却防刷屏。
  谓词覆盖 `hp`/`mp`/`level`/`meso`/`equips`/`players`/`mapid`/`mobs`/`drops`/`npcs`/
  `shop_open`/`dialog`/`task_active` 等；动作可以是任意指令，含 `quit`（直接下线）。
- **任务引擎**：线性指令序列 + `wait` 谓词 + `timeout` + `vars` 占位符；`loop: true` 循环执行。
  采用**锁栈模型** —— 任务持锁期间打怪让路，保证长流程不被抢占。
- **功能分组**：把规则+任务打包成互斥功能组（自动出售、自动买药…），`group open/close` 一键切换。
- **流程变量**：`setvar` / `clearvar` + `var==name:value` 谓词。

### 🧩 多开

一个进程内跑 N 个账号 —— **没有子进程，没有 IPC**。
核心层完全不知道"多开"这回事，多开由事件路由与前端承担：

- 左栏账号列表，每个账号**独立的日志缓冲 / 状态 / 滚动位置 / 过滤页签**
- `Alt+↑↓` 切号、`F4` 启停、`F3` 手动重连、`F2` 看板（全部账号一行一个）
- `@all hunt on` / `@<档案前缀> hunt on` 批量下发（前缀无命中会报错，不会误发给当前账号）
- **掉线自动拉起**：`5s → 15s → 45s` 退避重连，最多 3 次后标记「已放弃—需人工」；
  **登录失败绝不重试**（避免把账号刷成风控）
- 运行中可 `profiles add` / `profiles remove` 增删账号，不必重启

### 🕹 无人值守

- **远程控制**：`--chat-admin <游戏角色名>` —— 游戏内密语 `#指令` 控制 bot，逐行密语回执
- **分级播报**：`[INFO]/[OK]/[WARN]/[ERR]` 密语推送，挂机卡死自动告警
- **防人下线**：示例规则 `evade` —— 同图出现陌生人（排除队友）自动断开重连

### 👀 查看体系

`view player / mobs / players / drops / platforms / inventory / skills / equips /
party / trade / ap / reactor / keymap / cashshop` —— 全部只读信息，诊断用。

---

## 快速开始

```bash
# 构建
cargo build --release                              # headless CLI
cargo build --release -p openstory-console         # 中文控制台（单开/多开同一个程序）
cargo build --release -p openstory-webui           # Web 配置编辑器（可选）

# 测试（628 项）
cargo test --workspace --no-fail-fast
```

### 方式一：中文控制台（推荐）

```bash
# 单开
target\release\openstory-console.exe --ip <IP> --port 8484 --account <账号> --password <密码>

# 多开：一个进程内跑 N 个账号
target\release\openstory-console.exe --profiles profiles\服A_账号1.json profiles\服A_账号2.json --password <共用密码>
```

不带参数启动会先弹**配置档案选择屏**（扫描 `profiles/`），再进登录向导，
世界/频道/角色全程点选。进图后右侧面板实时显示角色/场景/挂机状态，日志全中文。

**管理器模式**（挂上账号，谁连由你决定）—— 推荐入口是选择屏勾选：

```bash
target\release\openstory-console.exe                        # 选择屏 → Space 勾选多个 → Enter
target\release\openstory-console.exe --profiles a.json b.json --no-autostart
```

| 操作 | 键 |
|---|---|
| 切换账号 | `Alt+↑↓` |
| 启动 / 停止选中账号 | `F4`（也可输入 `start` / `stop`） |
| 看板 | `F2` |
| 手动重连 | `F3` |
| 批量下发 | `@all hunt on` / `@<档案前缀> hunt on` |
| 日志过滤页签 | `Tab` / `Shift+Tab`（13 个页签，每账号各自一份） |

### 方式二：纯命令行（headless）

```bash
target\release\openstory-bot.exe --config profiles\<服>_<账号>.json ^
    --account <账号> --password <密码> --auto "gather on" --auto "hunt on"
```

`--config` 指定规则/任务/分组所在档案，可重复传按序合并；凭据仍必须显式传。
`--duration <秒>` 可无 stdin 自动退出（适合 CI / 计划任务）。
`--charlist-only` 只跑到角色列表（探针用）。

**推荐挂机参数**：吸怪拉近 + 无技能群发清场（配置默认即此组合）。

### 方式三：Web 配置编辑器（独立程序，可选）

```bash
target\release\openstory-webui.exe     # http://127.0.0.1:8080，编辑当前目录 config.json
```

浏览器内以表单 + JSON 双视图编辑配置，保存时按 bot 的配置结构校验并原子写入。
与 bot **完全解耦**（独立进程），运行中的 bot 通过 `reload` 指令或重启生效。

---

## 指令速查

| 命令 | 作用 |
|---|---|
| `hunt on\|off` / `hunt attack [n]` / `hunt skill <id> [n]` | 打怪开关 / 普攻 / 技能 |
| `hunt range <px>` / `hunt pickup [on\|off\|<px>]` / `hunt stand on\|off` | 攻击范围 / 拾取 / 原地模式 |
| `hunt once` / `hunt controller on\|off` / `hunt damage <n>` | 单次攻击 / 只打有控制权的怪 / 上报伤害 |
| `gather on\|off` / `gather step\|interval\|max\|controller` | 吸怪开关与调参 |
| `rule open\|close\|status <id>` | 顶层规则开关 |
| `group open\|close\|status <id>` | 功能组总开关 |
| `task start <id>\|stop\|status` / `reload` | 任务控制 / 热重载配置 |
| `view <对象>` | 查看体系（全部只读） |
| `channel <n>` / `town <mapid> <portal>` / `warp <portal>` / `reenter` | 换线 / 过图 / 传送 / 重进当前图 |
| `npc <oid>` / `buy` / `sell [tab <类>]` / `sellammo` | NPC 商店与卖物品 |
| `cashshop [enter\|buy <sn>\|get\|out]` | 商城 |
| `skill learn\|cast\|info <id>` / `buff <id>` / `keymap set` | 技能 / 辅助 / 改键 |
| `party create\|invite\|leave` / `trade invite\|accept\|put` | 队伍 / 交易 |
| `chat <text>` / `use` / `drop` / `dropmeso` / `ap <stat>` | 聊天与道具操作 |
| `profiles [list\|add <路径>\|remove <档案名>]` | 多开账号管理 |
| `help` / `clear` / `sleep <秒>` / `quit` | 辅助 |

完整手册见 [docs/GUIDE.md](docs/GUIDE.md)。

---

## 配置体系

配置是**分层**的：`RuntimeConfig` 支持 `include` 链（按顺序合并，同 `id` 的条目覆盖上一层），
所以一份档案通常只写"差异"，不是完整配置。

```
config.json              全局默认（可被档案覆盖）
config.default.json      出厂默认（首次运行时复制成 config.json）
profiles/
  <服>_<账号>.json        一份档案 = 一个独立账号
  .manager.json          多开凭据存储（明文，仅在 --remember-password 时生成，勿提交）
data/
  <服>/                  物品/地图/怪物/NPC/传送点/技能 中文名词典（由 crates/resource 导出）
```

档案可通过 `"data_dir": "data/server_a"` 绑定独立的词典目录，从而同时挂不同服务器的号。

> ⚠️ **账号来自档案，不来自全局配置。** 多开时每个会话走
> `sessions::session_template(base, path)` 构造自己的配置；**不要**用 `apply_login`
> —— 那是"只填空缺"语义，会让多个号继承同一个账号（服务器会来回踢）。

---

## 工程结构

```
src/                      核心库 openstory-bot —— 零 UI 依赖
├── crypto.rs             加密：AES + Shanda + IV 更新 + 包头 XOR
├── packet.rs / opcodes.rs 包编解码与操作码
├── handlers/             协议分发：login / field / party / trade / cashshop
├── parsers/              包解析：login / field / inventory / chat / movement / skill
├── packets/              包构造：login / combat / item / map / npc / movement ...
├── command/              指令引擎 + 挂机主循环（parse / run / tick / rules / tasks / view）
├── state.rs              角色状态机（非阻塞，长流程每 tick 只走一步）
├── runtime_config.rs     配置 / 规则 / 任务 / 分组
├── emit.rs               结构化事件（Field 语义键 + emit_f! 宏）
└── names.rs              中文名词典（零拷贝查询）

crates/console/           终端控制台 openstory-console（ratatui）
├── app.rs                应用状态与事件循环
├── sessions.rs / session.rs / watcher.rs   会话层与掉线拉起
├── ui.rs / profile.rs / wizard.rs / credentials.rs
└── tests/render_regression.rs   渲染回归（驱动真实 App + ui::render）

crates/console-lib/       前端共享层（与前端形态无关的展示/交互逻辑）
                          command_spec / completion / zh / logbuf / snapshot / text

crates/webui/             独立 Web 配置编辑器（tiny_http + 单页）
crates/resource/          WZ 数据导出：生成 data/ 下的中文名词典
```

**分层原则**：核心层不知道有几个 bot —— 事件用 tokio task-local 按会话路由
（`emit::with_session_emitter`）。新增功能不要在核心层引入"当前账号"这类全局概念。

---

## 技术要点

- **加密传输**：AES + Shanda + IV 更新 + 包头 XOR，版本 79
- **完整登录链路**：握手 → LOGIN → 选服 → 选角 → 进频道 → 进图；换线/商城往返自动接管
- **结构化事件**：核心输出 `Field` 语义键而非拼好的字符串 —— 所以 CLI 英文输出逐字节不变，
  而前端能按字段渲染成中文。新功能自带中文，未收录消息回退英文，**永不断链**
- **非阻塞状态机**：长流程每 tick 只执行一步，网络收发不受影响
- **性能**：空闲时 CPU ≈ 0%、内存 ≈ 0.9MB；词典零拷贝查询（RwLock + 就地 HashMap）；
  日志窗口滚动用前缀和 + 二分查找

---

## 测试

```bash
cargo test --workspace --no-fail-fast     # 必须加 --no-fail-fast，否则第一个失败后面根本不跑
cargo build --workspace --all-targets     # 期望：零警告
```

当前基线 **628 项**（627 通过 / 1 项需本地服）：

| 套件 | 数量 | 说明 |
|---|---|---|
| `openstory-bot` lib | 264 | 协议 / 加密 / 配置 / 指令解析 / 状态机 |
| `openstory-console` lib | 207 | 会话层 / 拉起 / 档案 / UI 逻辑 |
| `openstory-console-lib` lib | 80 | 文本 / 中文渲染 / 日志缓冲 / 命令规格 |
| `render_regression` | 53 | **驱动真实 `App` + `ui::render` 对帧缓冲断言** —— 改 UI 必看 |
| `spec_matches_parser` | 11 | 命令表与解析器一致性（唯一来源是 `command_spec.rs`） |
| `openstory-webui` | 4 + 2 | 配置校验与 HTTP 接口 |
| `session_layer_live` | 3 | 会话层联调 |
| `login_flow` | 2 | 登录链路 |
| `multi_session_isolation` | 1 | **一个进程内两个会话互不串台** |
| `live_warp_test` | 1 | ⚠️ 端到端联调，**需要本地服 `127.0.0.1:8484`**，连不上直接失败 |

> 需要本地服的用例连不上时会打印 `SKIP` 并返回（`live_warp_test` 例外，它会直接失败）。
> `session_layer_live` 内置**进程内串行闸门** —— 本地服扛不住并发登录，
> 别改成并行，也别靠放宽超时绕开，那会把真实回归一起掩盖掉。

---

## 文档

| 文档 | 内容 |
|---|---|
| [docs/GUIDE.md](docs/GUIDE.md) | 使用指南：完整命令手册、功能逻辑 |
| [docs/TUI.md](docs/TUI.md) | 中文控制台详解：布局 / 快捷键 / 中文渲染体系 |
| [docs/MANAGER_PLAN.md](docs/MANAGER_PLAN.md) | 多开控制台：需求、架构决策、实施阶段 |
| [docs/PROJECT.md](docs/PROJECT.md) | 项目现状：已实现能力、字节级协议格式、架构 |
| [docs/CONFIG.md](docs/CONFIG.md) | 配置参考 |
| [docs/ROADMAP.md](docs/ROADMAP.md) | 功能清单与路线图 |
| [AGENTS.md](AGENTS.md) | 给编码 agent 的速查（改代码前必读） |

---

## 说明

本项目源自一套自用的冒险岛 079 无头自动化实现，功能与测试基线保持一致。
个人账号档案与个人配置**未包含**在仓库中 —— 首次运行会从 `config.default.json`
生成默认配置，请自行通过控制台的选择屏创建档案。

仓库内所有服务器地址、账号、密码均为占位符；`profiles/` 下两份示例档案指向
`127.0.0.1`，仅供测试夹具使用。

## 协议来源

封包格式与加密方案来自两条线索，都是**独立实现**，未移植任何第三方客户端的代码：

- **实测封包截取** —— 对目标服务器收发流量的逐字节比对，是最终权威
- **079 服务端源码** —— 用于交叉核对包结构与操作码语义

加密算法（AES / Shanda / IV 更新 / 包头 XOR）本身是公开规范，实现按实测行为校准。

## 许可证

[MIT](LICENSE) —— 允许任意使用、修改、分发与再许可，仅需保留版权与许可声明。

仅供学习协议实现与私有服务器自动化使用；请遵守你所在服务器的服务条款。
