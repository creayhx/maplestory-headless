# 配置指南 (config.json)

所有可配置行为集中于此。复制 `config.default.json` 为 `config.json` 使用。
运行时命令(如 `hunt range 300`)只改内存值,不写回文件;`reload` 重载文件。

## 配置档案 (profiles/): 多服多账号

一份配置 = 一个 服×账号 的完整独立文件,放在 `profiles/<服>_<账号>.json`,
互不影响。**密码永不入库**(向导提交后只回写 账号/ip/port/aes_key),每次启动手输。

- **TUI**: 启动即弹出档案选择屏(`profiles/*.json` + 根 config.json 兜底项),
  ↑↓ 选择 Enter 确认;N 新建(自模板)、C 复制、D 删除(二次确认)。选中后
  向导预填该档案的账号/IP/AES,之后 `hunt save`、向导保存等一切写盘都
  只落这份档案。`_` 开头的文件是库文件,不在列表显示。
- **headless CLI**: `--config <path>` 指定档案,可重复传并按序合并
  (后者覆盖前者,写盘目标 = 最后一个)。凭据仍必须显式传:
  `openstory-bot --config profiles\本地服_100000003.json --account <acc> --password <pass> ...`
- **webUI**: `openstory-web --config profiles\xxx.json` 编辑指定档案。
- 不带 `--config` 且不选档案时 = 传统行为(读工作目录 config.json)。
- 注意: 两个进程同时写同一档案会互相覆盖(webUI+TUI 同开一个档案时留意)。

### data_dir（独立数据目录）

每个档案可绑定独立数据目录，用于不同服务器的客户端数据（物品/怪物/NPC/地图/传送门/技能）：

```jsonc
{
  "data_dir": "data/xiaoserverA",   // 相对或绝对路径
  ...
}
```

- 不设置或为空 = 使用默认 `data/` 目录
- `data_dir` 下缺失的文件自动回退到默认 `data/` 目录
- 切换档案时自动检测 `data_dir` 变化并重载数据词典

### 规则/任务库与 include 引用

通用脚本(来人下线/自动接交易等)写进库文件, 档案里**显式声明**要引用哪些:

```jsonc
// profiles/my_profile.json
{
  "include": ["autosell_rules.json", "trade_rules.json"],   // 可列多个, 按顺序合并
  ...
}
```

- 不写 `include` 就只加载档案本身——绝不隐式加载任何文件, 避免意外合并;
  库文件可以按范围建: `autosell_rules.json`(自动出售)、`trade_rules.json`(交易)各取所需。
- include 路径相对声明它的文件; 支持库文件再引用库文件(嵌套); 同一文件
  一次启动至多加载一次(重复引用/循环引用安全终止, 入口档案始终最后合并)。
- 合并语义(多层叠加通用):
  - 顶层标量与普通数组(potion 等): 后层整体覆盖前层;
  - `rules` / `tasks` / `groups` 数组**按 id 合并**: 档案内同 id 只覆盖
    写出的字段(其余继承库层), 新 id 追加, 同 id `"enabled": false` 即在该
    档案禁用继承项。组内嵌套的 rules/tasks 同样按 id 合并。
- 库文件永不作为写盘目标(`hunt save` 等始终落档案本身)。
- 引用的文件缺失或损坏 = 整份配置回落默认值(启动时)/保持现状(reload 时),
  绝不带病合并。

## 顶层字段

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `tick_ms` | u64 | 50 | 主循环 tick 周期(毫秒)。默认 50(20 tick/s); 压测/高频场景可调小(如 10) |
| `potion_cooldown` | u64 | 800 | 喝药冷却(毫秒): 两次喝药的最小间隔 |
| `reconnect` | bool | true | 断线/连接失败自动重连。重连保留配置类内存态(规则/组开关/hunt 含 damage) |
| `reconnect_delay` | u64 | 5 | 重连间隔(秒) |
| `reconnect_max` | u64 | 0 | 最大重连次数(0 = 无限) |
| `chat_admins` | string[] | [] | 游戏内密语控制白名单(`#` 指令者); CLI `--chat-admin` 启动时并入 |
| `login` | object | - | 登录信息: `account` / `password` / `ip` / `port` / `aes_key`（**不含 channel**） |
| `channel` | - | - | 频道为运行时选择：首次登录 TUI 手动选（或 `--channel <n>` 1 基），自动重连**复用本次所选频道**，不写入 config.json |
| `potion` | PotionRule[] | [] | 喝药规则(见下) |
| `rules` | Rule[] | [] | 通用规则: 谓词触发执行动作(见下) |
| `tasks` | TaskDef[] | [] | 顶层任务(见下) |
| `groups` | GroupDef[] | [] | 功能分组: 打包规则+任务, 手动开关(见下) |
| `hunt` | object | - | 打怪行为参数(见下) |

## hunt(打怪)

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `damage` | i32 | 0 | 攻击包上报伤害(`hunt damage <n>`): 0 = 按等级公式估算 (lvl²/2); >0 = 固定值。服务器会校验上报伤害合理性(对比玩家面板实际伤害), 估算值偏高会触发"检测到异常行为"掉线; 固定为角色实际伤害区间(如 400)稳定。**随 hunt save 一起写回** |
| `attack_mode` | string | "attack" | 攻击方式: `attack`=普攻(每怪各发一个普攻包); `skill`=技能(`attack_skill`=0 即无技能群发: 单包任意目标数(含单体), 段数始终生效, 不混发普攻包, 不读技能属性) |
| `attack_skill` | i32 | 0 | 攻击技能 id: 0 = 普攻; >0 = 技能 id |
| `attack_skill_range` | i32 | 0 | 技能筛怪范围 px(技能模式下优先于攻击范围); 0 = 用攻击范围 |
| `attack_skill_max_targets` | usize | 0 | 技能包最大目标数; 0 = 用 1 兜底(单目标) |
| `attack_skill_mp_cost` | i16 | 0 | 技能消耗 MP: 施放前检查, 不足不发包; 0 = 不检查 |
| `attack_range` | i32 | 70 | 攻击距离 px: 怪距人物 ≤ 此值才发攻击包, 更远的先传送过去再打。实测 300 稳定(400 触发检测) |
| `attack_cooldown` | u64 | 700 | 攻击冷却(毫秒): 两次攻击包最小间隔。实测 600-700 稳定 |
| `attack_max_targets` | usize | 6 | 普攻模式攻击范围内最大发包数量(每怪一包); 15 是协议上限 |
| `until` | string\|null | null | 谓词: 满足时自动停止打怪(如 `equips>=24`); null = 不自动停 |
| `teleport_delay` | u64 | 150 | 移动后到攻击的最小间隔(毫秒): 紧跟移动攻击是脚本特征, 客户端是停下再打 |
| `pickup_enabled` | bool | true | 是否拾取掉落; false = 不捡(纯打怪测速/省包) |
| `pickup_range` | i32 | 400 | 拾取半径 px(以人物为圆心); 0 = 全图拾取 |
| `pickup_filter_mode` | string | "off" | 拾取过滤模式: `off`=不过滤 / `allow`=只捡 `pickup_allow` 清单 / `deny`=除了 `pickup_deny` 清单都捡。金币(meso)恒捡不受影响。运行期用 `hunt filter` 切换, 随 hunt save 持久化 |
| `pickup_allow` | i32[] | [] | **仅拾取清单**: `mode=allow` 时只捡这些 itemid |
| `pickup_deny` | i32[] | [] | **仅过滤清单**: `mode=deny` 时不捡这些 itemid |
| | | | 被过滤的掉落根本不进状态——四个拾取路径(每tick半径捡/远距传送捡/放置物全图捡/pickup all)一并豁免, 每 tick 循环开销反而更小。改配置后 `reload` 生效(已在地上但被新规则拒绝的掉落立即清出状态) |

`hunt filter` 运行期指令(改完立即生效并剪枝, 无需 reload):

| 指令 | 作用 |
|---|---|
| `hunt filter` | 查看当前模式 + 两清单 |
| `hunt filter off` | 暂停过滤(清单保留, 方便下次切换) |
| `hunt filter allow` | **只切到仅拾取模式**(保留现有清单) |
| `hunt filter allow <id...>` | 切到仅拾取模式并**替换**清单(例: `hunt filter allow 2000000 2000001`) |
| `hunt filter deny` | **只切到仅过滤模式**(保留现有清单) |
| `hunt filter deny <id...>` | 切到仅过滤模式并替换清单 |
| `hunt filter add <id...>` | 追加到**当前模式**的清单(先 allow/deny 才有模式) |
| `hunt filter del <id...>` | 从当前模式清单删除 |
| `gather_step` | i32 | 150 | 吸怪每步距离 px: 0 = 直接拉到位; 单步超过 150 会被记吸怪违规(MOB_VAC: reduce_x>200 \|\| reduce_y>150) |
| `gather_interval` | u64 | 0 | 吸怪轮次间隔(毫秒); 0 = 每 tick 拉一轮。服务器对移动包频率敏感时调大(实测 1500ms 在 70+ 怪地图稳定, 700-1000ms 触发检测) |
| `gather_max` | usize | 0 | 每轮最多拉怪数; 0 = 不限。服务器对批量移动敏感时建议 3~10(实测 10 稳定) |
| `strange_report` | bool | true | 客户端周期性上报包(0x15 STRANGE_DATA, 官方约 3~4 次/秒), 服务器靠它确认客户端活跃 |

> **稳定性前提**：只要角色能获取到怪的控制权（`gather controller on` 只拉 0xF0 aggro=1 的怪；攻击侧 `hunt.attack_controller_only=true` 默认只打有控制权的怪），上表这套参数（攻击冷却 600-700ms、吸怪 1500ms×10 只、攻击范围 300px）实测即可稳定挂机。多人同图分走控制权、或服务器不下发控制权时，把 `attack_controller_only` 设为 `false` 即可攻击所有怪。

## potion(喝药规则)

```json
{ "stat": "hp", "threshold_pct": 25, "itemids": [2000002, 2000001] }
```

| 字段 | 说明 |
|---|---|
| `stat` | "hp" 或 "mp" |
| `threshold_pct` | 低于该百分比时喝药(如 25 = 血 <25% 喝) |
| `itemids` | 按优先级排列的药水 itemid(第一个有库存的会被使用); 模板可省略(空), 使用时填清单 |

## rule(通用规则)

```json
{ "id": "bf_autosell", "enabled": false, "when": "task_active==0 && equips>=40", "then": ["task start bf_sell"], "cooldown": 60000 }
```

| 字段 | 说明 |
|---|---|
| `id` | 规则名(`rule open/close <id>` 切换) |
| `enabled` | 是否参与规则引擎(config 可写); 运行时 `rule open/close` 切换, `reload` 回文件值 |
| `when` | 触发谓词(见下方谓词清单), 命中且冷却已过 → 执行 |
| `then` | 动作命令串数组(复用命令管线, 依次执行) |
| `cooldown` | 谓词命中后距下次触发的最小间隔(毫秒) |

## task(任务: 线性指令序列)

```json
{
  "id": "bf_sell",
  "priority": 10,
  "steps": [
    { "cmd": "auction", "timeout": 5000 },
    { "cmd": "npc reply 2", "wait": "dialog==1", "timeout": 5000 },
    { "cmd": "sell type equip" },
    { "cmd": "shop leave" }
  ],
  "timeout": 10000,
  "vars": { "{box}": "5" },
  "loop": false
}
```

| 字段 | 说明 |
|---|---|
| `id` | 任务名(`task start <id>` 启动) |
| `priority` | 优先级(大者先?) |
| `steps` | 步骤数组: 每步等待 `wait` 谓词满足后执行 `cmd`, 顺序推进 |
| `timeout` | 任务级默认步骤超时(毫秒): 步骤未单独配 timeout 时套用; 超时跳过该步骤 |
| `vars` | 任务级变量表: 把步骤 cmd/wait 里的 `{key}` 替换为值(复用开店模板, 差异参数化) |
| `loop` | true = 完成后从头重跑(游标复位, 不释放锁); false(默认) = 执行一次即结束 |

### 步骤字段

| 字段 | 说明 |
|---|---|
| `cmd` | 命令串(与命令行相同, 如 `auction`/`npc reply 5`/`sell type equip`/`shop leave`) |
| `wait` | 等待谓词(缺省 = 立即执行) |
| `timeout` | 等待超时(毫秒): wait 未满足且超时 → 带 cmd 的步骤**跳过**本步骤; 纯等待步骤(`cmd` 为空, 规则 `wait <谓词>` 内联产物)超时 → **放弃整个任务**(规则 cd 后重试) |

## group(功能分组)

```json
{ "id": "bf_money", "enabled": false, "exclusive": true, "rules": [...], "tasks": [...] }
```

| 字段 | 说明 |
|---|---|
| `id` | 组名(`group open/close <id>` 切换) |
| `enabled` | 组总开关(config 可写); 运行时 `group open/close` 切换 |
| `exclusive` | 互斥组: `group open` 时自动关闭其他已开的互斥组(抢同一资源如商店窗口时保证同时只有一个在跑) |
| `rules` | 组内规则: 仅组开启时**有资格**参与规则引擎, 且各自 `enabled` 独立生效(缺省 true)——组开+自身关 = 跳过, 方便把临时动作在正式跑时关掉 |
| `tasks` | 组内任务(纯方法, 无 enabled): 仅组开启时可被 `task run` 调用, 调用才执行 |

### enabled 与 reload 语义

| 层级 | reload 之后 |
|---|---|
| 顶层 rule / task / group 的 `enabled` | **同步为文件值**(文件开着就保持开), 定义取最新 |
| 组内 rule 的 `enabled` | 同步文件值; 生效 = 组开 && 自身开 |
| 组内 task | 无开关; 跟随定义更新 |

> 运行时切换(`rule open/close`、`group run/stop`)是内存态, reload 后一律回到文件值。任务本身永远只被调用(rule 的 `task run` 或手动)才执行。

---

## 谓词清单

谓词用于 `rule.when` / `task step.wait` / `hunt.until`。支持 `&&`(且)与 `||`(或)组合, 如 `hp_pct<30 && mp_pct>10`。

### 数值型(配合 `< <= > >= ==`, 与数字比较)

| 谓词 | 含义 | 示例 |
|---|---|---|
| `hp` | 当前血量 | `hp<25` |
| `hp_pct` | 血量百分比(0-100) | `hp_pct<30` |
| `mp` | 当前蓝量 | `mp<50` |
| `mp_pct` | 蓝量百分比 | `mp_pct<50` |
| `ap` | 剩余属性点 | `ap>=5` |
| `sp` | 剩余技能点 | `sp>0` |
| `meso` | 金币数 | `meso>1000000` |
| `level` | 角色等级 | `level>=40` |
| `equips` | 背包装备件数(未穿) | `equips>=40` |
| `equips_worn` | 已穿装备件数 | `equips_worn>=10` |
| `players` | 同图玩家数(不含自己与队友) | `players>=1` |
| `mapid` | 当前地图 id | `mapid==104040000` |
| `mobs` | 当前地图存活怪物数 | `mobs>0` / 打完判断 `mobs==0` |
| `drops` | 当前地图待拾取掉落数(已按拾取过滤) | `drops>0` / 捡完判断 `drops==0` |
| `npcs` | 当前地图 NPC 总数 | `npcs>0` |
| `reactors` | 当前地图放置物数 | `reactors>0` / `reactors==0` |

### 布尔型(配合 `==1` / `==0`)

| 谓词 | 含义 | 示例 |
|---|---|---|
| `hunt` | 打怪开关 | `hunt==1` |
| `hunt_reactor` | 放置物打怪开关 | `hunt_reactor==1` |
| `shop_open` | 商店窗口是否打开 | `shop_open==1` |
| `dialog` | 是否在 NPC 对话框 | `dialog==1` |
| `trade_invite` | 有交易邀请 | `trade_invite==1` |
| `trade_active` | 交易进行中 | `trade_active==1` |
| `trade_partner_locked` | 对方已锁定交易 | `trade_partner_locked==1` |
| `trade_locked` | 己方已锁定交易 | `trade_locked==0` |
| `task_active` | 有任务在运行(任务锁被持有) | `task_active==0` |

### 特殊

| 谓词 | 含义 |
|---|---|
| `always` | 恒真(或空串) |
| `npc==<npcid>` | 地图上存在指定 id 的 NPC(如 `npc==1011100`) |
| `var==<名>:<值>` | 流程变量精确匹配(`setvar boss_summoned 1` 后 `var==boss_summoned:1` 为真; 仅 `clearvar` 手动清除, 无隐式重置) |
| `{hunt_mapid}` / `{mapid}` | 任务步骤 cmd/wait 里的占位符, 执行前替换为当前挂机图/地图 id |

> 未知键/格式错误安全返回 false。谓词与 `until` 共用同一求值器。

### 规则门控流程示例(打 boss)

把每个阶段抽成规则, 用 `mobs`/`drops`/`var` 做节点判断, 失败后过冷却自动重试:

```jsonc
// 各阶段用规则拆分, 失败过冷却自动重试。所有"等待"用规则内嵌 wait 跨 tick 完成:
// 进门: 清标记→进图→等真正进入 boss 房(mapid 同步):
{ "id": "boss_enter", "when": "mapid==220080000 && dialog==0 && task_active==0",
  "then": ["setvar boss_summoned 0", "ports in00", "wait mapid==220080001 5000"], "cooldown": 1500 }
// 召唤: 规则内嵌 wait → 整个序列变任务步骤。等放置物数据到位(reactors>0) → 击打 →
// 等 boss 真出现(mobs>0) → 才标记。任一步超时=放弃任务, cd 后整体重试(不会没召唤就跳步):
{ "id": "boss_summon", "when": "mapid==220080001 && mobs==0 && var==boss_summoned:0 && dialog==0 && task_active==0",
  "then": ["wait reactors>0 5000", "reactor hit all", "wait mobs>0 5000", "setvar boss_summoned 1"], "cooldown": 2000 }
// 打怪: 不受任务锁影响, 每 tick 发一次 hunt once(实际节奏由 attack_cooldown 把关):
{ "id": "boss_fight", "when": "mapid==220080001 && mobs>0 && dialog==0", "then": ["hunt once"], "cooldown": 1 }
// 拾取: boss 死后掉落落地即扫(捡不起的靠全图自动放弃兜底, 不卡锁):
{ "id": "boss_pickup", "when": "mapid==220080001 && mobs==0 && drops>0 && dialog==0 && task_active==0",
  "then": ["pickup all"], "cooldown": 200 }
// 退出: 已召唤才退(防误触)。对话→等弹窗→确认→等真正出图:
{ "id": "boss_exit", "when": "mapid==220080001 && mobs==0 && var==boss_summoned:1 && npcs>0 && dialog==0 && task_active==0",
  "then": ["npc 2041025", "wait dialog==1 5000", "npc yes", "wait mapid==220080000 5000"], "cooldown": 1500 }
```

> **规则动作内联转换**（实现细节）
> - `wait <谓词> [超时ms]` 只在规则动作里用：触发瞬间把"当前 wait 动作 + 剩余动作"整体转成任务步骤推入执行栈（第 0 步为纯等待门 `cmd=""`+`wait`+`timeout`，缺省 8000ms；其后为普通步）。谓词满足→推进；纯等待门超时→**放弃整个任务**释放锁，规则冷却后整体重来。带**同 id 重入防护**：若栈中已有同规则 id 的运行实例则跳过本次内联（与 `start_task` 同 id no-op 对齐），叠加 `when task_active==0` 闸门，持锁期间不会重复堆积。
> - `task run <id>` 内联：把任务步骤（变量展开、缺省超时继承）拼规则剩余动作推栈。**按顺序推入执行栈、不做同 id 去重**——同一规则若再次触发（且 `when` 未用 `task_active==0` 拦住），会把同一任务再压一份到栈顶，按 LIFO 串行跑完（每个实例 `step_idx` 独立、推进互不串台，无崩溃/无副作用）。要防重复执行，请在 `when` 里加 `task_active==0`。
> - 两者都只把动作转成现有 `wait`+`timeout` 步骤，**非阻塞、无新并发语义**（主循环 `tokio::select!` 三臂争锁，无 sleep/线程阻塞）。
> - 注：等待超时计时从步骤开始算起；地图切换期（`phase != InGame`）任务挂起但计时仍走，慢 warp 可能在 5s 内提前 abandon —— 入图类 `wait` 建议给较宽超时。

> 手动也可直接输入 `hunt once` 精准单次攻击。
