# openstory-bot 功能清单

> 更新日期：2026-09-06 · ✅ = 已实现，⏳ = 进行中，⬜ = 待做

## 核心功能

### 自主打怪
- `hunt on/off` 自动寻怪、瞬移追怪、攻击、拾取
- `hunt attack [n]` 普攻（攻击范围内每怪一个普攻包，最多 n 个）
- `hunt skill <id> [n] [range] [mp]` 技能（id=0 即无技能群发，一个包打 n 只）
- `hunt hits <n>` 技能每目标命中段数（1..15，每段独立取值）
- `hunt range <px>` 攻击范围（传送后筛怪半径，默认 300px）
- `hunt pickup on/off` 拾取开关（不打怪也生效）
- `hunt pickup <px>` 拾取半径（0=全图）
- `hunt controller on/off` 只攻击有控制权的怪（默认 on）
- `hunt stand on/off` 原地模式（不追怪，配合吸怪原地清场）
- `hunt once` 手动单次攻击（找范围内怪发一轮包，可用于规则 `while mobs>0 do hunt once`）
- `hunt damage <n>` 设置攻击上报伤害（0=公式 lvl²/2，>0=固定值）
- `hunt save` 持久化当前 hunt 配置到文件
- `hunt filter [off/allow/deny/add/del]` 拾取过滤（运行期即时生效）
- `skill cast <skillid> <oid>` 对目标施放攻击技能
- `buff <skillid>` 施放辅助技能
- `buff cancel <skillid>` 取消 buff

### 吸怪
- `gather on/off` 吸怪开关（把圈外怪拉向人物，独立于 hunt）
- `gather step <px>` 每步距离（0=直接拉到位）
- `gather interval <ms>` 轮次间隔（实测 1500ms 稳定）
- `gather max <n>` 每轮最多拉怪数
- `gather controller on/off` 只拉有控制权的怪

### 移动
- `move <x> [y]` 绝对坐标移动
- `town <mapid> <portal>` 通过传送门过图
- `ports <portal>` 直接进入传送门（CHANGE_MAP_SPECIAL）
- `warp <portal>` 当前地图传送门传送
- `channel <n>` 换线（1 基）
- `reenter` 商城往返刷新全图对象

### 背包
- `view inventory [tab]` 查看背包
- `use <itemid>` 使用物品
- `drop <itemid> [qty]` 丢弃物品
- `dropmeso <amount>` 丢弃金币
- `bag` 打开背包界面（TUI 本地页面）

### 装备
- `view equips` 查看穿着装备
- `view equip <itemid>` 查看装备属性
- `equip <itemid>` 穿装备
- `unequip <itemid>` 脱装备

### 技能
- `view skills` 查看技能列表
- `skill learn <skillid>` 加点
- `skill info <skillid>` 查看技能等级

### 键盘绑定
- `view keymap` 查看键盘绑定
- `keymap set <key> <type> <action>` 改键

### NPC 交互
- `npc <oid|npcid>` 发起对话
- `next/prev/reply <id>/yes/no/num <n>/text <...>/cancel` 对话回复

### 商店
- `buy <itemid> [qty]` 购买
- `sell <itemid> [qty] [slot <n>]` 出售（qty<=0 = 卖光所有槽位）
- `sell tab <equip|consume|etc>` 卖出整个栏位
- `sellammo` 一键出售弹药（箭/飞镖/子弹）
- `shop leave` 关闭商店

### 商城
- `cashshop enter/buy <sn>/get/out` 商城操作

### 玩家交易
- `trade invite <名字>` 发起交易
- `trade accept/confirm/decline/quit` 交易操作
- `trade put <itemid> [qty]` 放物品
- `trade meso <n>` 放金币
- `view trade` 查看交易状态

### 队伍
- `party create/invite <名>/leave` 队伍操作

### 规则 / 任务 / 功能组
- `rule open/close <id>` 规则开关
- `rule status [id]` 查看规则
- `group open/close <id>` 功能组开关
- `group status [id]` 查看功能组
- `task start <id>/stop/status` 任务控制
- `reload / cfg` 热重载配置

### 查看体系
- `view` 完整状态
- `view player/mobs/players/drops/platforms/inventory/skills/equips/party/trade/ap/reactor/keymap/cashshop` 各类只读查询

### 其他
- `ap <str|dex|int|luk|maxhp|maxmp> [n]` 加点
- `chat <text>` 地图发言
- `sleep <秒>` 非阻塞等待
- `quit` 退出
- `clear / cls` 清空日志
- `help` 帮助

---

## 配置系统

### 档案系统（profiles/）
- 多服多账号：`profiles/<服>_<账号>.json`，每份独立配置
- `--config <path>` 指定档案（headless CLI），可重复传按序合并
- `include: ["库文件.json"]` 引用通用规则库，按 id 合并
- `data_dir` 每个档案可绑定独立数据目录（`data/*.json`）
- TUI 启动时弹出档案选择屏（N 新建 / C 复制 / D 删除）

### 规则引擎
- 谓词触发（`when`）+ 动作执行（`then`），支持 `&&`/`||` 组合
- 谓词：hp/mp/level/meso/equips/players/mapid/mobs/drops/npcs/shop_open/dialog/task_active 等
- `while <pred> do <cmd>` 循环步骤（规则内联为 loop step）
- `setvar/clearvar` 流程变量（`var==name:value` 谓词匹配）
- `quit` 动作可直接下线（防人规则）
- 冷却防刷屏

### 任务引擎
- 线性指令序列 + `wait` 谓词 + `timeout` + `vars` 占位符
- 锁栈模型（任务持锁期间 hunt 让路）
- `loop: true` 循环执行
- `task run <id>` 内联展开（规则动作中）

### 功能分组（group）
- `exclusive: true` 互斥组（自动关闭其他已开互斥组）
- 组内规则/任务由组开关统管
- 组内规则 `enabled` 独立生效（组开+自身关 = 跳过）

---

## 性能

- CPU 占用 ≈ 0%（空闲时），内存 ≈ 0.9MB
- `tick_ms` 可压到 1ms（极限节奏）
- names.rs 零拷贝查询（RwLock + 就地查 HashMap）
- logbuf 前缀和 + 二分查找（O(log n) 窗口滚动）

---

## 待实现

### P2 — 信息完整度
- 伤害数字确认（客户端 0xBC 渲染）
- 技能属性数据（从 Skill.img 解析技能范围/目标数/消耗）

### P3 — 平台/移动打磨
- 平台间跳跃移动（跳跃片段代替传送）
- 移动平滑（tick 间隔/步长调优）

### P4 — 远期
- 技能自动加点（按职业白名单自动分配）
- 组队跟随（跨图跟随、平台 y 判定）

## 已完成（原 P4 远期项）

- ✅ **多开**（`openstory-console --profiles a.json b.json …`）—— 采用**进程内多会话**
  而不是 IPC 方案：一个进程内跑 N 个 `runtime::run` 任务，不需要序列化、不需要
  socket 生命周期管理。子进程 + IPC 的那套实现（`crates/ipc` / `crates/manager`）
  已删除。含左栏账号列表、`F2` 看板、`@` 目标批量下发、掉线自动拉起、密码双模式。
  设计与进度见 [MANAGER_PLAN.md](MANAGER_PLAN.md)
