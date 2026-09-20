# openstory-bot 使用指南

> 更新日期：2026-08-11

## 1. 运行

```bash
cargo build --release
target\release\openstory-bot.exe --ip <ip> --port 8484 --account <acc> --password <pass> \
    [--gender 0] [--world 0] [--channel 0] [--char 0] [--cid <id>] \
    [--auto "cmd"] [--duration <secs>] [--show-packets] [--dump] [--damage <n>] \
    [--channel-ip <ip>] [--chat-admin <名字>] [--aes-key <hex16|hex512>] [--charlist-only]
```

> `--aes-key`：改密钥的服务端用——8 字节密钥（16 位 hex，如 `130A06B41B0F3352`，运行时自动展开）或 256 字节预展开表（512 位 hex）；从客户端提取 + 探针验证见 `examples/keyfind.rs` / `examples/probe.rs`。`--charlist-only`：登录到角色列表即停、不进图。

> **Web 配置编辑器（独立程序，可选）**：`cargo build --release -p openstory-webui` 后运行
> `target\release\openstory-web.exe [--port 8080] [--config config.json]`，浏览器内编辑
> config.json（按 bot 配置结构校验后原子写入；运行中的 bot 需 `reload` 或重启后生效）。

### 远端测试服务器

联调在远端环境验证登录/进图链路，登录后静默待命、不发任何动作。

**凭据不入库**：仓库里不放任何服务器地址与账号。请按 `profiles/<服>_<账号>.json` 自建档案
（`login` 节写 `ip`/`port`/`account`），或运行时用 `--ip` / `--account` / `--password` 显式传入。

**改密钥的服务器**：部分服的登录器会篡改客户端密钥，标准密钥登录会被拒。
这类服必须用 `--aes-key <hex16>` 注入自定义 key —— 16 位十六进制（8 字节密钥，
运行时展开成 256 字节轮密钥表），例如 `130A06B41B0F3352`（此处仅示意格式，
实际值请自行从客户端提取）；日常回归可以只跑到角色列表即停（`--charlist-only`）。

### 登录失败状态码

| reason | 含义 |
|--------|------|
| 4 | 密码错误 |
| 5 | 账号不存在 |
| 14 | 账号封禁 |

## 2. 命令手册

### 移动
| 命令 | 说明 |
|------|------|
| `move <x> [y]` | 走到 x（y 缺省用当前平台层） |
| `town <mapid> <portal>` | 通过传送门过图（如 `town 100000000 east00`） |
| `ports <portal>` | 直接进入传送门（CHANGE_MAP_SPECIAL） |
| `warp <portal>` | 当前地图按传送门名称传送（CHANGE_MAP，portal 查看 `view portals`） |
| `channel <n>` | 换线（1 基；0x22 → 服务端回 0x13 目标频道地址 → 重连 → PLAYER_LOGGEDIN 恢复） |
| `reenter` | 重新进入当前地图（商城往返：进商城→立即离开→CharacterTransfer 落回原图原频道，怪/NPC/reactor/掉落全量刷新；无登录通告） |

### 打怪
| 命令 | 说明 |
|------|------|
| `hunt on\|off` | 自主打怪开关（找最近怪 → 传送 → 攻击范围内筛怪 → 按攻击方式发包） |
| `hunt attack [n]` | 攻击方式=普攻：攻击范围内每只怪各发一个普攻包，最多 n 个（n 缺省 = `hunt.attack_max_targets`） |
| `hunt skill <id> [n] [range] [mp]` | 攻击方式=技能：id=0 即无技能群发（一个 skill=0 单包打多只，服务端按普攻逐目标处理、不读技能属性）；n=包内最大目标数、range=技能筛怪范围、mp=蓝耗（MP 不足不发包，不回退） |
| `hunt range <px>` | 攻击范围：传送后以人物为圆心筛选怪物的半径（默认 300px） |
| `hunt pickup on\|off` | 拾取开关（等价改 config `hunt.pickup_enabled`，**不打怪也生效**——独立拾取身边掉落） |
| `hunt pickup <px>` | 拾取半径：数值 px（0=全图拾取） |
| `hunt controller on\|off` | 只攻击有控制权的怪（0xF0 aggro=1）：默认 on（安全）；off = 攻击所有怪（多人同图分走控制权、或服务器不下发控制权时用） |
| `hunt stand on\|off` | 原地模式：不追怪，配合吸怪（`gather on`）原地清场 |
| `hunt once` | 手动单次攻击：找范围内怪发一轮攻击包（可用于规则 `while mobs>0 do hunt once`） |
| `skill cast <skillid> <oid>` | 对指定目标施放一次攻击技能（0x28 带技能，等级自动取） |
| `buff <skillid>` | 施放一次辅助/buff 技能（SPECIAL_MOVE 0x58，无目标） |
| `attack <oid>` | 对指定 oid 发一次普攻 |

> **配置独立、各自生效**：攻击方式（`hunt attack`/`hunt skill`）、攻击范围
> （`hunt range`）、拾取（`hunt pickup`）分开配置，最后 `hunt on` 开始——
> 每一步单独设置，执行时不再有任何隐式降级/匹配。

### 放置物（Reactor）
| 命令 | 说明 |
|------|------|
| `reactor on\|off` | 放置物挂机开关：全图攻击所有 reactor + 掉落立即拾取（`huntreactor on\|off` 同义，旧名保留） |
| `view reactor` | 列出当前地图 reactor（oid/rid/state/坐标） |
| `reactor hit all\|<oid>` | 手动全图攻击 / 指定 oid 攻击 |
| `reactor cooldown <ms>` | 放置物攻击轮次间隔（0 = 每 tick 全打一遍） |
| `pickup all` | 无距离拾取地图上所有掉落 |

### 键盘绑定（keymap）
| 命令 | 说明 |
|------|------|
| `view keymap` | 查看键盘绑定（90 键位，type：1=技能 2=物品 4=UI 5=表情 6=宏） |
| `keymap set <key> <type> <action>` | 改键（服务端零校验，可绑任意技能/物品 id，持久化到 DB） |

### 规则开关（rule）
| 命令 | 说明 |
|------|------|
| `rule open <id>` | 开启一条 config 规则（如 `rule open sellauto`，默认全关） |
| `rule close <id>` | 关闭一条规则 |
| `rule status [id]` | 列出规则（enabled/when/动作/冷却），可只查一个 |

> 规则默认 `enabled: false`（config.json 里 `"enabled": true` 可持久开启）；
> `reload` 后回到 config.json 文件值（运行时开关不持久化）。

**防人规则示例**（config.json `rules` 节，示例配置已内置 `evade` 并默认开启）：
```json
{"id": "evade", "enabled": true, "when": "players>=1", "then": ["quit"], "cooldown": 1000}
```
> 谓词 `players` = 同图玩家数（**排除自己与队友**）；有人进图即触发 `quit` 断开。
> 规则动作支持 `quit`（结束会话），需要时用 `rule close evade` 关闭。

### 功能分组（group）
> 把「规则 + 任务」打包成功能组（如"自动出售"、"自动买药"），组是**总开关**：
> 组开启 → 组内规则参与规则引擎、组内任务可 `task start`；组关闭 → 全部不生效。
> 组内规则的 `enabled` 字段被忽略（组开关统管）。组是组织 + 隔离手段，
> 解决多个功能堆在 `rules`/`tasks` 里难维护、以及多个功能抢同一资源（如商店窗口）的冲突。

| 命令 | 说明 |
|------|------|
| `group open <id>` | 开启功能组（组内规则/任务即刻生效） |
| `group close <id>` | 关闭功能组 |
| `group status [id]` | 列出分组（on/off、exclusive、组内规则/任务清单），可只查一个 |

`exclusive: true` 的组：`group open` 时会**自动关闭其他已打开的互斥组**——两个都要用商店
窗口的功能（如自动出售 vs 自动买药）标成互斥，同一时刻只会有一个在跑，不抢窗口。

config.json 示例（`groups` 节，数组整段覆盖）：
```json
{
  "groups": [
    {
      "id": "autosell",
      "enabled": false,
      "exclusive": true,
      "rules": [
        {"id": "sell_loop", "when": "shop_open==1", "then": ["sell 4000000 9999", "buy 4000000 9999"], "cooldown": 1000}
      ],
      "tasks": [
        {"id": "open_shop", "priority": 10, "timeout": 30000, "vars": {"box": "2022552", "s1": "5", "s2": "1"}, "steps": [
          {"cmd": "reward {box}", "timeout": 10000},
          {"cmd": "reply {s1}", "wait": "dialog==1"},
          {"cmd": "reply {s2}", "wait": "dialog==1"},
          {"cmd": "sleep 0", "wait": "shop_open==1"}
        ]}
      ]
    },
    {
      "id": "autopotion",
      "enabled": false,
      "exclusive": true,
      "rules": [
        {"id": "potion_loop", "when": "shop_open==1 && mp_pct<50", "then": ["buy 2000002 100", "use 2000002"], "cooldown": 2000}
      ],
      "tasks": [
        {"id": "open_potion_shop", "priority": 10, "timeout": 30000, "vars": {"npcid": "1011100", "s1": "2"}, "steps": [
          {"cmd": "npc {npcid}"},
          {"cmd": "reply {s1}", "wait": "dialog==1"},
          {"cmd": "sleep 0", "wait": "shop_open==1"}
        ]}
      ]
    }
  ]
}
```

> 用法：`group open autosell` 开"自动出售"，`group open autopotion` 会自动关掉
> autosell（互斥），切换到"自动买药"；`group status` 查看当前开着的功能。
> 规则/任务 id 全局唯一（顶层与组内不重名），`task start <id>` / `#task` 用法不变。
> **注意区分**：对组内规则执行 `rule open/close` 会**直接报错**（提示所属组与正确用法，
> 密语回执同样可见）——组内规则只由组开关统管，`rule open` 只会作用于顶层规则，
> 防止"以为开了但组没开 → 不生效"的困惑。
> **状态输出分离**：`rule status` / `task status` **只列顶层**规则/任务（带序号、多行展示
> when/then/步骤明细）；组内全部明细看 `group status`（每组分段：序号 + 组开关/互斥 +
> 组内规则 when/then/冷却 + 组内任务逐步骤与 wait 条件）——一眼看出有哪些、条件是什么。

#### 复用：任务级 `vars` 占位符
> 各 group 的"打开商店"流程相似但参数不同（不同箱子/NPC/选项 id）。把差异写进任务级
> `vars` 表，步骤 `cmd`/`wait` 里用 `{key}` 引用——启动时替换（vars 先于 state 占位符
> `{hunt_mapid}`/`{mapid}`，未命中的键原样保留给 state 层）。复制任务模板只改 vars 即可，
> 差异一目了然：上面的 `open_shop`（箱子 `reward {box}` → `reply {s1}` → `reply {s2}`）
> 与 `open_potion_shop`（`npc {npcid}` → `reply {s1}`）是同一套"开商店"模式的两个实例。

### 自动卖装备
| 命令 | 说明 |
|------|------|
| `rule open sellauto` | 开启自动卖装（equips≥90 且无任务时自动触发卖装任务） |
| `task start sell` | 立即触发一次卖装流程（等同聊天 `#task start sell`，锁栈模型） |

### 任务（config.json 线性任务）
| 命令 | 说明 |
|------|------|
| `task start <id>` | 压栈启动任务（只执行栈顶，快照当前地图为 hunt_mapid） |
| `task stop` | 清空整栈（释放锁+重置游标，任务视为从未执行） |
| `task status` | 查看任务栈（顶带 `->`）+ 已定义任务 |
| `reload` / `cfg` | 热重载 config.json（tasks/rules/potion 生效） |

### 流程变量
| 命令 | 说明 |
|------|------|
| `setvar <name> <value>` | 设置流程变量（规则谓词 `var==name:value` 匹配） |
| `clearvar <name>` | 删除流程变量（手动生命周期管理） |

### 玩家交易
| 命令 | 说明 |
|------|------|
| `trade invite <名字>` | 主动发起交易（须同地图，自动开窗+邀请） |
| `trade accept` | 接受待处理邀请；窗口已开则确认 |
| `trade confirm` | 锁定当前交易 |
| `trade decline` | 拒绝邀请 |
| `trade quit` | 退出交易 |
| `trade put <itemid> [qty]` | 放物品（须窗口已开，流程化请用任务步骤编排） |
| `trade meso <n>` | 放金币（须窗口已开，流程化请用任务步骤编排） |
| `view trade` | 查看交易状态 |

### 背包
| 命令 | 说明 |
|------|------|
| `view inventory` | 列出全部背包物品（含 EQUIP/USE/ETC） |
| `view inventory <tab>` | 只看指定栏位：`equip` / `use` / `setup` / `etc` / `cash` |
| `use <itemid>` | 使用物品（USE_ITEM 0x45） |
| `scroll <卷轴id> <装备id>` | 对装备使用升级卷轴（USE_UPGRADE_SCROLL 0x53；目标装备优先身上穿戴，其次背包；别名 `upgrade`） |
| `reward <itemid>` | 打开奖励道具（REWARD_ITEM 0x70，如拍卖快捷箱子；被禁时用 `auction` 替代） |
| `auction` | 打开拍卖（ENTER_MTS 0x8D 空包，客户端"拍卖"按钮行为） |
| `drop <itemid> [qty]` | 丢弃物品 |
| `dropmeso <amount>` | 丢弃金币 |

### 装备
| 命令 | 说明 |
|------|------|
| `view equips` | 查看当前穿着装备（部位 + itemid + 属性：力/攻击/防御/升级次数等） |
| `view equip <itemid>` | 查看指定装备的实际属性（背包优先，穿着兜底） |
| `equip <itemid>` | 从背包穿上装备（自动映射身体部位；该部位已占用会提示） |
| `unequip <itemid>` | 脱下装备回背包（自动找空槽） |

> 穿着装备来自 charinfo 的 EQUIPPED 块（普通位 -1~-99 + 现金位 -100~-999），
> 换装走 ITEM_MOVE 0x44（`int tick + byte 1 + short src + short dst + short qty`，
> dst<0 穿 / src<0 脱），服务端 0x20 回包同步装备栏。支持部位：帽(-1) 脸(-2)
> 眼(-3) 耳(-4) 上装(-5) 下装(-6) 手套(-7) 鞋(-8) 披风(-9) 盾(-10) 武器(-11)
> 戒指(-12) 项链(-16) 徽章(-17) 坐骑(-18) 腰带(-19) 口袋(-20) 勋章(-21)。

### 技能
| 命令 | 说明 |
|------|------|
| `view skills` | 查看剩余技能点 + 已学技能列表（含四转 max 等级） |
| `skill learn <skillid>` | 给指定技能加 1 点（DISTRIBUTE_SP 0x57） |
| `skill info <skillid>` | 查看单个技能等级 |
| `skill cast <skillid> <oid>` | 对目标施放一次攻击技能 |
| `buff <skillid>` | 施放一次辅助技能（SPECIAL_MOVE 0x58，无目标，服务端校验等级一致） |
| `buff cancel <skillid>` | 取消指定 buff（CANCEL_BUFF 0x59） |

> 技能列表来自 charinfo 的 addSkillInfo 块；加点后服务端回 0x22（SP 扣减）+ 0x27
> （技能升级）自动同步。加满/前置不满足会被服务端静默拒绝。buff 施放链路：
> 0x58 → 服务端扣 MP + 应用 buff + 广播 0xC0 动画 + 回 0x23 GIVE_BUFF（冷却 >0 回 0xEC）。

### 聊天
| 命令 | 说明 |
|------|------|
| `chat <text>` / `say` / `c` | 地图频道发言（GENERAL_CHAT 0x2D，GBK 编码） |
| `--chat-admin <名字>` | 启动参数：允许该角色通过密语控制 bot（可重复） |

> 自动接收：地图聊天 `[chat] 名字: 内容`、组队/好友/公会/联盟 `[party]` 等、
> 密语 `[whisper]`、GM 黄字与系统横幅 `[notice]`。文本按服务端 GBK 编码解码，
> 中文收发无损。服务端 `@`/`!` 前缀指令为服务端私有化实现，不集成。

### 聊天控制（ChatCommand）
白名单角色（`--chat-admin`）密语 bot（角色名）发送 `#指令`，bot **逐行密语回执**（聊天栏 shell 式报告）：

| 命令 | 功能 |
|------|------|
| `#help` | 支持的命令列表 |
| `#status` | 地图/位置/挂机状态/血量 |
| `#player` | 等级/HP/MP/EXP/AP/SP/金币 |
| `#where` | 地图 + 坐标 |
| `#skills` | 技能列表（逐行） |
| `#reactor` | 地图 reactor 列表 |
| `#inv [equip\|use\|setup\|etc\|cash]` | 背包全部 / 指定栏位 |
| `#drop <itemid> [qty]` | 丢物品 |
| `#dropmeso <amount>` | 丢钱（10..50000） |
| `#pickup` | 捡取地图全部掉落（物品+钱） |
| `#rule <open\|close\|status> [id]` | 规则开关/查看（透传本地 rule 命令） |
| `#task <start\|stop\|status> [id]` | 任务控制/查看（透传本地 task 命令，如 `#task start sell`） |
| `#hunt <on\|off\|attack\|skill\|range\|status>` | 手动控制打怪：开关/攻击方式/攻击范围/查看状态。技能 id 由 config 写死，`#hunt skill <id>` 暂不支持（以后开放） |
| `#say <文本>` | bot 在地图频道公屏说这句话（远程喊话，同图玩家可见） |

> 非白名单角色的 `#` 消息直接忽略。操作类复用现有指令链路（白名单内执行）。

### NPC 对话
| 命令 | 说明 |
|------|------|
| `npc <oid\|npcid>` | 发起对话（先按 oid 匹配，未命中按 npcid 匹配——配置任务里可直接写固定 npcid 如 `npc 1011100`） |
| `next` / `ok` | 翻页(type=0) / 确认(type=1) |
| `prev` / `back` | 上一页(type=0) / 返回(type=1) |
| `reply <id>` / `select <id>` | 选择选项(type=4) |
| `yes` / `y` | 是(type=5) |
| `no` / `n` | 否(type=5) / 返回(type=1) |
| `num <n>` / `number <n>` | 数字输入(type=3) |
| `text <...>` / `input <...>` | 文字输入(type=2) |
| `cancel` / `bye` | 关闭对话 |

### 商店
| 命令 | 说明 |
|------|------|
| `npc <oid>` | 先对话打开商店 |
| `buy <itemid> [qty]` | 购买道具 |
| `sell <itemid> [qty]` | 出售道具（qty<=0 = 卖光该物品所有槽位） |
| `sell <itemid> <qty> slot <n>` | 只卖指定槽位的那一份 |
| `sell tab <equip\|consume\|etc>` | 卖出整个栏位（默认 equip） |
| `sellammo` | 一键出售全部弹药（USE 栏：箭矢 206xxxx / 飞镖 207xxxx / 子弹 233xxxx，`ammo` 别名） |
| `shop leave` | 关闭商店 |

### 队伍
| 命令 | 说明 |
|------|------|
| `party create\|invite <名>\|leave` | 队伍操作（状态看 `view party`） |

### 查看（view）
> 所有只读查询统一 `view <对象>`，纯查看不发送任何包。

| 命令 | 说明 |
|------|------|
| `view` | 完整状态（位置/血量/组队/挂机） |
| `view player` | 战斗数值（HP/MP/EXP/等级/AP/SP） |
| `view mobs` | 地图怪物列表 |
| `view players` | 同图玩家列表 |
| `view drops` | 地图掉落列表 |
| `view platforms` | 平台层级 |
| `view inventory [tab]` | 背包（全部或 equip/use/setup/etc/cash 单栏） |
| `view skills` | 技能列表 + 剩余 SP |
| `view equips` | 身上穿着装备 |
| `view equip <itemid>` | 指定装备属性（背包优先，穿着兜底） |
| `view party` | 队伍状态 |
| `view trade` | 交易状态 |
| `view ap` | 剩余 AP |
| `view reactor` | 地图放置物列表 |
| `view keymap` | 键盘绑定 |
| `view cashshop` | 商城余额/点数/商城物品（`view cs` 同义） |

### 能力点（AP）
| 命令 | 说明 |
|------|------|
| `ap <str\|dex\|int\|luk\|maxhp\|maxmp> [n]` | 立即加点（n 缺省 1；每点一个 DISTRIBUTE_AP 包） |
| `ap status` | 查看剩余 AP |

> 查看类指令统一 `view <对象>`；`view equip <itemid>` 优先查背包，背包无则查穿着。

### 其他
| 命令 | 说明 |
|------|------|
| `sleep <秒>` | 非阻塞等待 |
| `help` | 帮助 |
| `quit` | 退出 |

### 商城（cashshop）
> 同类操作统一收编为 `cashshop` 二级命令（顶层 `csbuy`/`csget`/`csout` 已移除）。

| 命令 | 说明 |
|------|------|
| `cashshop` / `cashshop enter` | 进商城 |
| `cashshop buy <sn> [nx\|points]` | 买商城物品（默认 nx 币） |
| `cashshop get [uniqueid\|last]` | 取出商城物品（缺省 = 最近一个） |
| `cashshop out` | 离开商城 |

## 3. 功能逻辑

### 3.1 自主打怪（hunt）
- 每 tick（config `tick_ms` 默认 50ms，最小可压到 1ms）执行打怪循环
- 优先拾取 150px 内掉落（独立拾取：**不打怪也生效**），更远的掉落随移动目标靠近（walk 250 / 传送 800px）
- 找全图最近怪物直接传送过去（无距离限制），按攻击距离筛怪发包
- 怪物死后自动选下一个目标
- **攻击方式**：`hunt attack` 普攻（攻击范围内每怪一个普攻包）；`hunt skill` 技能（id=0 即无技能群发，>0 真技能群攻）
- 范围外怪不筛不追，等刷近

### 3.1.1 进图"移动唤醒"机制（discovery）
- 客户端进图后，服务器通常通过 SPAWN_PLAYER 下发自己的位置；拿到位置后 bot 发一次绝对移动包（`discovery move`），让其他玩家看到角色出现
- **部分该服进图后不发自己的 SPAWN_PLAYER**，本地位置一直是 (0,0)——此时服务器同样在**等待客户端第一个移动包**才推送地图数据（怪物/NPC/上线通告全不发，只有全局公告正常）
- 机制理解：客户端加载地图资源时角色"卡在天上"，玩家一动说明资源加载完成——服务器以第一个移动包作为"进图完成"信号，之后才开始下发地图内容，客户端此时接收也能正确显示
- bot 适配：进图 5 秒后位置仍未知也强制发一次移动包"唤醒"服务器（tick.rs discovery 兜底），否则该服挂机永远看不到怪

### 3.2 移动方式
- 移动方式固定 **teleport**（walk 模式已移除）：打怪接近、捡掉落全部用绝对传送代替走路
- 传送受 `hunt.teleport_delay` 冷却门控，冷却期间正常执行攻击/拾取，不阻塞主循环
- 落点直接用目标坐标 y，可跨平台层攻击；**瞬移是绝对移动，不做平台 x 钳制**
- 攻击前受 `hunt.tp_hold` 门控：瞬移后固定等待，确保攻击时服务器已确认新位置

### 3.3 攻击方式（hunt attack / hunt skill）
- `hunt attack [n]`：普攻——攻击范围内每只怪各发一个普攻包，最多 n 个（`hunt.attack_max_targets`）
- `hunt skill <id> [n] [range] [mp]`：技能——一个包打多个目标；**id=0 即无技能群发**（skill 字段 0，服务器按普攻逐目标处理；不读技能属性——不扣 MP、无冷却，段数/伤害可任意指定，等于绕过技能校验的裸普攻群发）
- `hunt hits <n>`：技能每目标命中段数（1..15）——如群攻技能是 2 段，则每目标发 2 个伤害值，**每段独立在伤害范围内取值**（如 1200 上限 = 每段 1080~1200，不拆分、不超单段校验）；tbyte 低 4 位 = 段数，服务端逐段结算。无技能群发模式下同样生效（= 任意段数普攻）
- 技能参数由命令设置（缺省用 config）：`attack_skill`（技能 id）、`attack_skill_max_targets`（包内最大目标数）、`attack_skill_range`（技能筛怪范围，0=用攻击范围）、`attack_skill_mp_cost`（蓝耗，MP 不足不发包）、`attack_skill_hits`（每目标段数）
- **筛怪半径规则**：`attack_mode=skill`（含 skill=0 无技能群发）且 `attack_skill_range > 0` 时用 `attack_skill_range`；否则一律用 `attack_range`（`hunt range` 设置，默认 300）。普攻模式永远用 `attack_range`
- **无任何隐式降级**：指定技能就一直发技能包（MP 不够就停手），指定普攻就一直发普攻包，不会自动回退
- **攻击方式**：config `hunt.attack_mode`（`attack`/`skill`），**启动时套用一次**；之后手动 `hunt attack|skill` 的选择永久保持，过图/任务往返/reenter 都不会重置

### 3.4 自动喝药
- 由 `config.json` 的 `potion` 规则驱动：`{stat, threshold_pct, itemids}`，stat（hp/mp）低于阈值百分比时按 `itemids` 优先级使用第一个有库存的药水
- 默认规则行为：hp<25% 喝 HP 药、mp<50% 喝 MP 药、hp<50% 兜底
- 800ms 冷却（一次 tick 最多喝一瓶）

### 3.5 跨图移动
- 使用 `town <mapid> <portal>` 通过传送门过图
- `mapid` 参数被忽略，实际发 `targetid = -1`
- 服务端根据 portal 名称在当前地图查找传送门，进入后自动传送到目标地图
- 过图后自动发送发现移动（MOVE_PLAYER）广播位置
- 10s 超时自动恢复，防止卡死

### 3.5.1 重新进入当前地图（reenter，2026-08-11 新增）
- `reenter` = 商城强制往返：发 `ENTER_CASH_SHOP 0x23` → 服务端 CharacterTransfer 交接到商城服（回 0x13 地址）→ 重连 + PLAYER_LOGGEDIN → SET_CASH_SHOP 进商城 → bot **自动**发 `CHANGE_MAP 0x21` 空包离开（商城服侧 = `LeaveCashShop`）→ 回 0x13（原频道）→ 重连 → SET_FIELD **落回原图原频道**，地图对象（怪/NPC/reactor/掉落）全量重新生成
- 与换线同一套 CharacterTransfer 机制 = 同频道"轻量重登"，**无登录通告**（`LoggedIn` 走 pendingCharacter 恢复分支，不触发非法登录检测）
- 服务端禁入商城的地图仅 980000xxx 道场系列 + 180000001，其余任意地图可用（事件实例/防作弊中/商城关闭时进入被拒）
- 进入被拒（10s 无商城回包）自动放弃并恢复 hunt 快照；往返全程 hunt 让路，SET_FIELD 回原图后按快照恢复
- 进图时清掉旧图残留（怪/玩家实体、平台层、地面 y、锁定目标），重入后不会追幽灵怪

### 3.6 NPC 对话
- 6 种对话类型全部支持：
  - type=0 sendSay：纯文本，`next` 翻页 / `prev` 返回
  - type=1 sendNextPrev：确认/翻页，`next` 确认 / `prev` 返回
  - type=2 sendGetText：文字输入，`text "..."` 回复
  - type=3 sendGetNumber：数字输入，`num <n>` 回复
  - type=4 sendSimple：选项列表，`reply <id>` 选择
  - type=5 sendYesNo：是/否，`yes` / `no` 回复
- 奖励道具（如拍卖快捷箱子 2022552）通过 `reward <itemid>` 打开；**reward 被服务端禁用时改用 `auction`**（发送 ENTER_MTS 0x8D 空包，即客户端"拍卖"按钮行为，打开拍卖；实测测试服可用）
- 完整链路示例：`reward 2022552 → reply 5 → reply 1 → 打开杂货小铺`（或 `auction → reply …` 同构）

### 3.7 商店买卖
- `npc <oid>` 打开商店 → `buy`/`sell` 买卖
- `sell type equip` 出售全部装备（服务端自动过滤不可交易道具；原 `sellall` 已并入此命令）
- 商店格式：`int npcId + short count + 每项[int itemId + int price + short tag + short qty]`
- TCP 保证顺序，不需要等 OPEN_NPC_SHOP 响应即可发送卖包

### 3.8 自动卖装备（sellauto 规则）
- 触发：示例 config.json 自带 `sellauto` 规则（`mapid==104040000 && task_active==0 && equips>=90` 时启动 `sell` 任务）；规则默认 `enabled: false`，用 `rule open sellauto` 开启（或 config.json 写 `"enabled": true` 持久开启）；**与 hunt 解耦**，人物先到位再开
- **mapid 写死 = 错位保护**：规则里写死挂机地图，人物不在该地图就不触发，防止路线写死的地图外执行失败。**不要在 when 里加 `npc==<商店npc>` 条件**——商店 NPC 只在商店图在场，触发时人物在猎场恒为 false，规则永不触发（08-07 实测修复）；NPC 是否在场应放到任务步骤的 `wait` 里
- **多地图挂机**：每张挂机图配一条规则 + 一个任务——复制 `sellauto` 规则改 `id`/`mapid`（如 `sellauto_ellinia` + `mapid==100000000`），复制 `sell` 任务改 `id`（如 `sell_ellinia`）和步骤路线，规则 `then` 指到对应任务
- `sell` 任务就是 config.json `tasks` 里的普通任务：默认 9 步（回城→杂货店→`sell type equip` 卖装→返回打猎地图），**NPC 步骤直接写固定 npcid**（`npc 1011100`，oid 由 npc 命令动态解析；步骤带 `wait: "npc==1011100"` 等 NPC spawn 到位，找不到时步骤报错任务干净中止，不会无商店硬卖），可自由增删步骤、加 `wait` 谓词与 `timeout`（如想卖特定物品在步骤里加 `sell <itemid> [qty]`，配合热重载）
- `sell type equip` 只卖 EQUIP 栏
- 任务走**锁栈模型**：持锁期间 hunt 让路，任务完成/失败/`task stop` 释放锁后 hunt 自然恢复，无 `resume` 链
- sellauto 规则带 60s 冷却：任务失败放弃后不会立即重试（防失败风暴）
- 推荐启动参数：`--auto "rule open sellauto" --auto "hunt on"`

### 3.9 玩家交易
- 自动接受/确认走规则：`rule open trade_accept`（自动接受邀请）、`rule open trade_confirm`（对方锁定后自动确认）
- 放物/放钱是**流程行为，用任务编排**（不再入队）：自定义任务等窗口打开后依次 `trade put`/`trade meso`/`trade confirm`，可给 put 步骤加 `wait: "trade_active==1"` 确保窗口已开；示例任务：
  ```json
  {"id": "trade_flow", "priority": 10, "steps": [
    {"cmd": "trade put 4000000 5", "wait": "trade_active==1", "timeout": 30000},
    {"cmd": "trade meso 100000"},
    {"cmd": "trade confirm"}
  ]}
  ```
- 完整自动化推荐：`--auto "rule open trade_accept" --auto "rule open trade_confirm" --auto "task start trade_flow"`，bot 自动接受交易→放物放钱→锁定
- 交易完成：物品自动进背包（0x20），金币自动更新（0x22），无需手动刷新
- 限制：交易最多 9 格物品；**同账号双角色交易会被服务端封号**
- 支持双向交易：收取物品/金币、给出物品/金币

### 3.10 装备系统
- `view equips` 查看穿着装备：部位 + itemid + 实际属性（str/dex/watk/wdef/升级次数/需求等级等），属性直接解析自服务端 `addEquipStats` 块
- `equip <itemid>` 穿装：自动按 itemid 类别映射身体位（ITEM_MOVE 0x44，dst<0）；目标位已占用会拒绝并提示，不会盲目替换
- `unequip <itemid>` 脱装：src<0 发回背包空槽
- 换装后 0x20（MOVE 模式）自动同步装备栏与背包，无需手动刷新

### 3.11 技能系统
- `view skills` 查看剩余技能点 + 全部已学技能（含四转 `Lv/满级`），数据来自 charinfo addSkillInfo 块
- `skill learn <skillid>` 加 1 点技能（DISTRIBUTE_SP 0x57）；成功后服务端回 0x22（SP 扣减）+ 0x27（技能升级）自动同步本地
- 加满/前置不满足/职业不符会被服务端静默拒绝（无惩罚），本地 sp 与技能等级实时跟踪
- `skill cast <skillid> <oid>` 施放攻击技能（CLOSE_RANGE_ATTACK 0x28 带技能 id，等级取自本地技能表）
- `buff <skillid>` 施放辅助技能（SPECIAL_MOVE 0x58 = `skip(4)+int skillid+byte level+pos+byte facing`；服务端校验等级必须与本地一致，扣 MP 后应用 buff，广播 0xC0 动画 + 回 0x23 GIVE_BUFF，冷却 >0 回 0xEC；解除走 CANCEL_BUFF 0x59）
- `buff cancel <skillid>` 取消 buff（0x59 = `int sourceid`，服务端 `cancelEffect` 按技能取消；蓄力技能清 keyDown + 广播取消）
- **buff 覆盖**：服务端 `registerEffect` 的 effects map 按 MapleBuffStat 为键 put 覆盖——同类型 buff（共享 buffstat，如 WDEF 系）后放的顶掉先放的，客户端只渲染不决策

### 3.12 键盘绑定（keymap）
- `view keymap` 查看 90 个键位绑定（登录时服务端下发 0x16F：`byte 0 + 90 × (byte type + int action)`，键码隐含 0-89）
- `keymap set <key> <type> <action>` 改键（0x83 = `int tick + int count + [int key + byte type + int action] × count`）——**服务端零校验**，任意键绑任意技能/物品 id 都接受并持久化 DB（重登回传）
- type：1=技能 2=物品 4=UI 5=表情 6=宏；常用键码：Shift=42、PageDown=81
- **跨职业技能绑定**：满技（技能进角色表）后，把技能树外的技能绑到键盘 → 客户端可见 → 施放链路（0x58）无职业校验 → 全 buff 可达（如战士绑法师终极无限 2121004）

### 3.13 定点瞬击（hunt spot）【已移除】
- spot 定点瞬击与 walk 模式一同移除：打怪统一为"找最近怪 → 传送 → 攻击范围筛怪 → 发包"

### 3.14 换线（channel）
- `channel <n>`（1 基）发 CHANGE_CHANNEL 0x22：`byte channel = n-1`（wire 0 基，服务端 +1）
- 服务端：前置检查（禁换线地图 FieldLimit/事件图）→ 存档 → buff/冷却/异常状态存 PlayerBuffStorage → 角色打包 CharacterTransfer 存 World → 回 **0x13**：`byte 1 + IP(4) + short port + byte 0`（无 cid）
- bot：断开 TCP → 重连目标频道 → 发 PLAYER_LOGGEDIN 0x0B（int cid）→ 服务端按 cid 取 CharacterTransfer 恢复 → SET_FIELD 进图
- 防作弊：login key 三重校验（非法换线断连+GM 广播）+ 同账号多开检测；换线不等同完整重登（不重走账号/选区/选人）

### 3.15 队伍
- 自动接受组队邀请
- 支持创建/邀请/离队/查看状态（`view party`）

### 3.16 放置物（Reactor）挂机
- `reactor on` 全图攻击所有 reactor：攻击无距离/冷却校验（DAMAGE_REACTOR 0xC9 = oid + charPos + stance，服务端只查 oid 存活），每 tick（50ms）打一遍全部 reactor；`reactor cooldown <ms>` 可加轮次间隔（0 = 每 tick）
- 掉落生成在 reactor 位置、owner=触发者本人（`reactor/1102000.js` 等脚本 `rm.dropItems()` + DB reactordrops 表概率），有掉落立即无距离拾取
- 拾取上报物品自身坐标（规避 50px 的 ITEMVAC_CLIENT 检查）；服务端真实距离 >800px 仅记 ITEMVAC_SERVER（每 5 次一条 GM 广播提示，**无封号、不拦截**）
- 打几次到终结态后销毁（REACTOR_DESTROY 0x11F），delay 后自动重生（reactorTime 秒×1000，state 归 0 重发 0x11E），循环往复
- 推荐启动参数：`--auto "reactor on"`

### 3.17 聊天控制（ChatCommand）
- 启动参数 `--chat-admin <名字>`（可重复）白名单角色，密语 bot 角色发 `#指令`
- 解析链路：0x8B 密语接收 → 白名单校验 → `#` 前缀 → 执行 → **逐行 0x75 密语回执**（聊天栏 shell 式报告）
- 查询类（纯状态）：`#status` `#player` `#where` `#skills` `#reactor` `#inv [tab]`
- 操作类（复用本地指令链路）：`#drop <itemid> [qty]` `#dropmeso <amount>` `#pickup`；`#rule`/`#task` 透传本地 rule/task 命令（如 `#rule open sellauto`、`#task start sell`、`#task status`）
- 安全：非白名单直接忽略；操作类无二次确认（仅白名单可触发）
- 示例：`#inv equip` → 回执装备栏列表；`#drop 4000000 5` → 丢 5 个叶子；`#task start sell` → 立即跑卖装任务
- 喊话：`#say <文本>` 远程公屏喊话（定时喊话已下线，周期/白名单重设计规划中）

### 3.18 播报（报告通道）
- 机制：`--chat-admin` 白名单角色会收到 bot 主动推送的密语，格式 `[类型] 内容`，单行一条
- 类型：`[INFO]` 一般信息 · `[OK]` 操作成功 · `[WARN]` 可恢复警告 · `[ERR]` 错误/失败
- 已接入事件：**hunt 卡死警告**（挂机中 30s 无伤害 → `[WARN] hunt stuck`，每分钟最多一条，防刷屏）
- 无 `--chat-admin` 时播报落到本地 stdout（`[report]` 前缀），不影响功能
- 各功能（买装/交易等）的结果播报内容在实现对应指令时接入

### 3.19 配置系统（config.json）
- 启动时自动加载工作目录 `config.json`：**不存在则生成默认文件**；存在则与内置默认值**深度合并**（文件可只写想覆盖的字段，数组整体替换）。**exe 内置默认 = 最小内核（不含任何 rule/task/group）**；根目录随附的 `config.json` 是示例源，自带示例规则/任务/分组，可直接修改使用
- 四个配置节：

| 节 | 内容 |
|---|---|
| `potion` | 喝药规则：`{stat: hp\|mp, threshold_pct, itemids 按优先级}` |
| `rules` | 通用规则：`{id, enabled, when 谓词, then 动作列表, cooldown}`（`cooldown` = 冷却毫秒）；规则默认关闭，`enabled: true` 或运行时 `rule open <id>` 开启；谓词命中且冷却已过 → 依次执行动作 |
| `tasks` | 线性任务：`{id, priority, timeout, steps: [{cmd, wait 谓词, timeout}]}`，每 tick 推进一步；**任务级 `timeout`** = 步骤未单独配超时时的默认值（如 `120000` = 每步最多等 2 分钟防卡死），步骤级可覆盖 |
| `groups` | 功能分组：`{id, enabled, exclusive, rules, tasks}`——把规则+任务打包成功能组（见"功能分组（group）"） |

- **谓词 DSL**（`when`/`wait` 用）：`键 操作符 值`，支持 `< <= > >= ==` 与 `&&`（且）/ `||`（或）链；RHS 支持变量键（如 `equips>=90` 直接写数字）；未知键/格式错误安全返回 false

| 键 | 含义 |
|---|---|
| `hp_pct` `mp_pct` `hp` `mp` | 血量/蓝量（百分比或绝对值） |
| `ap` `sp` `meso` `level` | 属性点/技能点/金币/等级 |
| `equips` | 背包装备（EQUIP 栏）物品数 |
| `equips_worn` | 身上穿着的装备数（08-08 新增，与 `equips` 区分） |
| `players` | 同图玩家数（**排除自己与队友**，08-15 新增——"有人来就下线"规则用） |
| `mapid` | 当前地图 id |
| `npc==<npcid>` | 当前地图上是否存在该 npcid 的 NPC（进图 SPAWN_NPC 填充，如 `npc==1011100`） |
| `hunt` `hunt_reactor` `teleport` `shop_open` | 状态开关（`teleport` = hunt.move_mode 为 teleport） |
| `dialog` | NPC 对话框是否处于打开状态（服务端来新对话页 = 1；发送 reply/next/选择等 = 0，见 3.21） |
| `trade_invite` `trade_active` `trade_partner_locked` `trade_locked` | 交易状态 |
| `task_active` | 是否有任务活动（锁栈非空） |

- **动作**：命令串（复用命令管线，与任务步骤同一入口，如 `use 2000000`、`trade accept`、`task start sell`）
- **任务示例**（config.json 的 `tasks` 节）：每步 `cmd` 执行一条命令，可带 `wait` 谓词等待条件（如 `mapid==104040000`）与 `timeout`（等待超时跳过）
- 任务控制命令：`task start <id>` / `task stop` / `task status`（stdin、`--auto`、聊天均可用）；任务走**锁栈模型**：`task start` 压栈，只执行栈顶任务，嵌套任务按后进先出（LIFO）完成；`task stop` 清空整栈（释放锁+重置游标，任务视为从未执行）；栈空时 hunt 等无锁行为自动恢复
- **热重载**：`reload`（或 `cfg`）命令重新加载 config.json（tasks/rules/potion 立即生效）；进行中任务的步骤在启动时已展开冻结，不受 reload 影响
- **任务占位符**（步骤 `cmd` 中展开）：任务级 `vars` 表（`{key}` → 值，启动时替换，见"功能分组"的复用说明）、`{hunt_mapid}` = 任务启动时快照的当前地图（任何任务 `task start` 时记下，用于卖完回挂机点，如 `town {hunt_mapid} west00`）、`{mapid}` = 当前地图；**NPC 对话直接在步骤里写固定 npcid**（`npc 1011100`，npc 命令先按 oid 后按 npcid 解析，无需占位符）
- **任务失败**：步骤命令解析失败或动作执行出错 → 打印原因并放弃该任务（弹栈释放锁），不重试；被打断（如用户 `task stop`）属正常现象，不做补偿
- 示例 config.json 自带规则全部 `enabled: false`（零副作用）：`trade_accept`（自动接受邀请）、`trade_confirm`（对方锁定后自动确认）、`sellauto`（`mapid==104040000` 且 equips≥90 启动卖装任务）；开启方式统一 `rule open <id>` 或 config.json 写 `"enabled": true`
- 内置行为边界：引擎仅内置**战斗**（hunt）与**喝药**（potion 规则）；卖装/买药/交易/副本等业务行为均为配置任务，无隐式行为——配置数据只有被任务步骤显式引用才生效；**exe 内置默认不含规则/任务/分组**，随附 config.json 提供示例

### 3.20 hunt 配置与卖装任务（P1）
- `hunt` 节（`config.json`）控制打怪参数：

| 字段 | 取值 | 默认 | 含义 |
|---|---|---|---|
| `pickup_range` | 数值 px | `400` | 拾取半径（以人物为圆心的圆形范围）：数值如 `400`、`500`；`0` = 全图拾取 |
| `pickup_enabled` | `true` \| `false` | `true` | 拾取总开关（**不打怪也生效**：独立拾取范围内掉落） |
| `attack_mode` | `attack` \| `skill` | `attack` | 攻击方式：`attack`=普攻（每怪一包）；`skill`=技能（id=0 即无技能群发）。`hunt attack`/`hunt skill` 设置 |
| `attack_max_targets` | 1..=15 | `6` | 普攻模式攻击范围内最大发包数量（每怪一包；15 为协议上限，写太大可能被检测） |
| `attack_skill` | 技能 id，`0` = 无技能群发 | `0` | 技能 id（`hunt skill <id>` 设置）；0=单包多目标、不读技能属性（不扣 MP/无冷却） |
| `attack_skill_max_targets` | 1..=15 | `0` | 技能包最大目标数（0 = 未配置用 1 兜底） |
| `attack_skill_range` | px | `0` | 技能筛怪范围（0 = 用攻击范围兜底） |
| `attack_skill_mp_cost` | 数字 | `0` | 技能蓝耗（MP 不足不发包，不回退；0 = 不检查） |
| `attack_skill_hits` | 1..=15 | `1` | 技能每目标命中段数（`hunt hits` 设置）：如群攻 2 段 = 每目标发 2 个伤害值，每段独立在伤害范围内取值（不拆分、不超单段上限） |
| `tp_hold` | 毫秒 | `100` | 瞬移后到可攻击的静默期（确保攻击时位置已确认） |
| `until` | 谓词或 `null` | `null` | 谓词满足时自动停猎（如 `"equips>=24"`、`"level>=60"`） |
| `teleport_delay` | 毫秒 | `0` | 两次瞬移的最小间隔 |
| `attack_cooldown` | 毫秒 | `700` | 两次攻击包的最小间隔（实测 700 稳定） |
| `attack_controller_only` | `true` \| `false` | `true` | 只攻击有控制权的怪（0xF0 aggro=1）；多人同图分走控制权、或服务器不下发控制权打不了时设 `false` 攻击所有怪 |
| `tick_ms` | 毫秒 | `50` | 主循环 tick 周期（可压到 1 = 极限节奏，`tick_ms` 在 config 顶层） |

- 示例 `sell` 任务 = config.json `tasks` 里的完整 9 步售卖流程（按 104040000 附近配：east00 进射手村、in00/in01 到杂货店、`npc 1011100` 开商店、`sell type equip` 卖装、west00 回训练场），末步 `town 104040000 west00` 回挂机点（**104040000 写死**：无路由表，路线固定写死；`{hunt_mapid}` 占位符机制保留，换挂机点可改回 `{hunt_mapid}` 或自配任务）；**自己改任务步骤即可自定义售卖细节**（不同打猎地图可配不同任务，如 `sell_henesys`/`sell_ellinia`）；超时配置在任务级 `timeout`（默认 120s/步）
- 触发条件在 `sellauto` 规则里（`mapid==104040000 && task_active==0 && equips>=90`，60s 冷却——mapid 写死 = 这套配置只对该地图有效，人物错位不触发；**不要加 `npc==<商店npc>` 条件**，商店 NPC 不在猎场，加了永不触发）；完成后释放锁，hunt 自然恢复（锁模型，无 resume 链）
- 聊天 `#task start sell` 等同本地 `task start sell`

### 3.21 NPC 对话 / 商店的时序与状态谓词（写自动买卖前必读）

**机制：不是完整状态机，是"服务端回包驱动"的布尔字段**——服务端回什么包，handler 就置什么位，谓词直接读这些字段：

| 状态 | 字段 | 置 1（开） | 置 0（关） |
|------|------|-----------|-----------|
| 对话框 | `dialog_open` | 服务端来 `NPC_TALK` 回包（每页都会来一次） | 本地发出 reply/next/yes/no/num/text/prev/cancel 时**乐观置 0**（不等回包，服务端下一页到了自然变回 1） |
| 商店窗口 | `shop_open` | 服务端来 `OPEN_NPC_SHOP`（对话走到商店那一项） | 本地 `shop leave`；**换图（SET_FIELD）自动置 0** |

- 对话是**逐页往返**：`reply`（或 next 等）→ 服务端回下一页 `NPC_TALK` → `dialog==1` → 再 reply → … 直到最后一项打开商店（`shop_open==1`、`dialog==0`）或取消。
- 谓词 `dialog==1` 就是为任务 `wait` 准备的：**触发步骤（`reward`/`npc`）本身不带 `wait`**（wait 先于步骤 cmd 检查，触发步骤带 wait 会把自己堵死），`wait: "dialog==1"` 只加在**后续回复步骤**上；最后用 `{"cmd": "sleep 0", "wait": "shop_open==1"}` 确认商店真的开了再结束任务（`sleep 0` 无副作用，仅作确认占位）。
- 商店买卖（`buy`/`sell`）与对话无关，只要 `shop_open==1` 就能发；`buy` 后服务端回包（0x20 库存 / 0x22 金币）自动同步本地，`sell` 按 itemid 查本地库存槽位——所以**卖要放在 buy 之后至少一个冷却周期**（等库存回包），规则里写 `["sell x 9999", "buy x 9999"]` 先卖后买、首轮卖失败无害，是最稳的写法。
- 服务端回包 → handler 置位 → 谓词读到 → 规则/任务 `wait` 放行：写配置时只要记住"**等什么状态就看对应谓词**"即可。

## 4. 性能
- CPU 占用 ≈ 0%（空闲时），内存 ≈ 0.9MB
- 无头客户端，无渲染、无 UI、无 NX 资源

> 中文 TUI 控制台（openstory-console）的界面布局、快捷键、中文渲染体系详见 [docs/TUI.md](TUI.md)。





