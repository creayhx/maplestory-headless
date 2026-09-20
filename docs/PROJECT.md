# openstory-bot 项目现状

> 更新日期：2026-08-20 · 对应仓库 `D:\Github\maplestory-headless`
> 配套文档：`GUIDE.md`（功能攻略）、`ROADMAP.md`（需求/待办清单）

Rust 协议驱动的 MapleStory 079 自动化挂机机器人。无渲染、无 UI、无游戏资源，完全通过网络协议驱动角色，可在单进程内同时运行多个 bot。

---

## 1. 已实现能力

### 登录链路（完整跑通）
`连接 8484 握手 → LOGIN(0x01, acc+pass+6字节) → [新账号: CHOOSE_GENDER 0x04 → SET_GENDER] → SERVERLIST_REQUEST(0x02) → SERVERLIST → CHARLIST_REQUEST(0x09) → CHARLIST → CHAR_SELECT(0x0A, 仅 int cid) → SERVER_IP → 重连频道服 → PLAYER_LOGGEDIN(0x0B) → SET_FIELD(0x81) 进图`

- 加密：AES(预展开密钥) + Shanda + IV 更新 + 包头 XOR，版本 79（握手 `[2..3]` = 0x4F）
- 服务器重复 SERVERLIST/CHARLIST 多次，bot 用 phase 守卫去重

### 进图后
| 能力 | 包 | 说明 |
|---|---|---|
| 移动 | 0x24 | `33字节前缀 + movement片段 + byte X + ceil(X/2) + 恰8字节`；50ms tick 平滑行走 |
| 聊天 | 0x2D | `string + byte` |
| 怪物移动 | 0xF1 | 实时坐标更新，追怪不再追已移动位置 |
| 攻击广播 | 0xBC | 解析多目标伤害（伤害数字渲染暂停） |
| 跨图移动 | 0x21/0x61 | CHANGE_MAP/CHANGE_MAP_SPECIAL，targetid=-1 走 portal |
| 换线 | 0x22/0x13 | `channel <n>`（1 基）：0x22 byte(n-1) → 服务端存档+CharacterTransfer 缓存 → 回 0x13(ip+port) → 重连 → PLAYER_LOGGEDIN(cid) 恢复进图；重连复用所选频道、SET_FIELD 以服务端真实频道校准 |
| 角色属性 | 0x22 | HP/MP/EXP/LEVEL/AP 全解析 |
| 组队 | 0x3B/0x78/0x79 | 自动接受邀请、建队/邀请/离开 |
| 自定位 | 0xA2 | 从自己的 SPAWN_PLAYER 解析真实出生点 |
| 掉落/拾取 | 0x110/0x111/0xC6 | 跟踪掉落、走近拾取 |
| 背包/吃药/丢弃 | 0x20/0x45/0x44/0x5B | charinfo 全量背包 + 0x20 增量索引；自动喝 HP/MP 药；丢物品/金币 |
| 自主打怪 | 0x28/0xEF | 找最近怪→传送→攻击范围筛怪→按攻击方式发包；hunt attack=每怪一普攻包，hunt skill=技能群包/无技能群发(skill=0) |
| **技能施放** | **0x28/0x58/0x59** | **`skill cast <skillid> <oid>` 攻击技能；`buff <skillid>` 辅助技能（SPECIAL_MOVE，服务端校验等级，扣 MP 应用 buff + 0xC0 广播 + 0x23 GIVE_BUFF）；`buff cancel <skillid>` 取消（0x59）** |
| **键盘绑定** | **0x16F/0x83** | **keymap：90 键位解析（view keymap）+ 改键（0x83 零校验、持久化 DB），可绑任意技能/物品 id（配合满技实现跨职业技能）** |
| 平台狩猎 | — | 平台层级检测 + 梯形 x 范围 + 自下而上逐层清怪；当前层无怪时瞬移去最近已知怪（任意层，弥补 spawn 视野广播限制） |
| 瞬移模式 | 0x24 | `hunt.move_mode="teleport"`（config 驱动，命令已删），打怪/拾取/升层全部瞬间传送；`hunt.teleport_delay` 冷却门控（默认 150ms，不阻塞 tick）；落点直接用目标 y |
| 自动喝药 | 0x45/0x20 | HP<1/4、MP<1/2 自动使用药水，800ms 冷却 |
| 加点 | 0x54/0x5C | `ap <str|dex|int|luk|maxhp|maxmp> [n]` 立即加点（每点一个 DISTRIBUTE_AP 包） |
| 目标锁定 | — | 选定怪物持续追击直到死亡/跑远 |
| 发现机制 | 0x24 | 进图微移触发服务端推送同图玩家 |
| **自动卖装备** | 0x21/0x36/0x3A | **sellauto 规则（`rule open sellauto`，when `mapid==104040000 && task_active==0 && equips>=90`——mapid 写死 = 错位保护；08-07 实测移除过严的 `npc==1011100` 条件，该 npc 只在商店图 100000102，猎场恒为 false 导致规则永不触发）触发 `sell` 任务（`tasks` 节完整 9 步流程，NPC 步骤写死 npcid，可自定义；多地图自配规则+任务），锁栈模型：任务持锁期间 hunt 让路，完成/失败/stop 释放后 hunt 自然恢复；08-07 实测全闭环：打怪掉装 → equips 达标 → 规则自动触发 → 卖装回猎场 → hunt 恢复** |
| **NPC 对话** | **0x36/0x38/0x145** | **6种对话类型全覆盖：type=0~5，reply/next/prev/yes/no/num/text** |
| **奖励道具** | **0x70** | **REWARD_ITEM 打开拍卖快捷箱子等特殊道具** |
| **拍卖入口** | **0x8D** | **`auction` 指令发送 ENTER_MTS 空包（客户端"拍卖"按钮行为），打开拍卖；reward 被禁用时的替代路径** |
| **商店买卖** | **0x3A/0x146/0x147** | **买/卖/一键出售全部装备，tag检测普通 vs 可充能道具** |
| **怪控制权** | **0xF0 双义** | **aggro=0 时是 5 字节 stopControllingMonster，按 mobid=0 移除实体（消除 malformed 告警）** |
| **伤害确认** | **0xF8/0xFC** | **controller 收到 damage / mob HP bar 日志，支撑卡死检测** |
| **玩家交易** | **0x14F/0x77** | **接收+确认+放物+放钱全链路，规则驱动自动接受/确认（`rule open trade_accept`/`trade_confirm`），放物放钱由任务步骤编排；`trade invite <名字>` 主动发起** |
| **聊天系统** | **0x2D/0xA4/0x8A/0x8B/0x41/0x4E/0x75** | **发送 + 地图/组队/密语/系统消息接收解析，GBK 编解码往返无损；密语发送 0x75 mode 6** |
| **聊天控制** | **0x8B/0x75** | **ChatCommand：`--chat-admin` 白名单密语 `#指令` 控制 bot，逐行密语回执；查询（status/player/skills/inv <tab>/reactor/where）+ 操作（drop/dropmeso/pickup）+ `#rule`/`#task` 透传本地命令** |
| **拾取优先级** | **0xC6** | **kill 后 3s 窗口主动走向远掉落物；拒捡 2s 冷却重试、3 次放弃（唯一物品）；打怪效率不受影响** |
| **放置物挂机** | **0xC9/0x11E/0x11C/0x11F/0xC6** | **huntreactor：全图攻击所有 reactor（0xC9 无距离/冷却校验）+ 掉落立即无距离拾取（0xC6 上报物品坐标），自动收割重生循环** |

### 命令（stdin 或 `--auto`）
查看类统一 `view <对象>`：
`view` · `view player` · `view mobs` · `view players` · `view drops` · `view platforms` ·
`view inventory [tab]` · `view skills` · `view equips` · `view equip <itemid>` ·
`view party` · `view trade` · `view ap` · `view reactor` · `view keymap` · `view cashshop` ·
操作类：
`move <x> [y]` · `chat <text>` · `hunt on|off` · `hunt attack [n]` · `hunt skill <id> [n] [range] [mp]` ·
`hunt range <px>` · `hunt pickup on|off|<px>` ·
`huntreactor on|off` · `reactor hit all|<oid>` · `pickup all` ·
`attack <oid>` · `skill learn|cast|info <...>` · `buff <skillid>` · `buff cancel <skillid>` ·
`use <itemid>` · `reward <itemid>` · `drop <itemid> [qty]` · `dropmeso <amt>` ·
`ap <str|dex|int|luk|maxhp|maxmp> [n]` · `ap status` ·
`party create|invite <name>|leave` · `sleep <secs>` ·
`town <mapid> <portal>` · `ports <portal>` · `channel <n>` · `reenter` · `npc <oid|npcid>` · `reply <id>` · `next` · `prev` ·
`yes` · `no` · `num <n>` · `text "..."` · `cancel` · `buy <itemid> [qty]` · `sell <itemid> [qty]` ·
`sell type <equip|consume|etc>` ·
`sell type <equip|consume|etc>` · `shop leave` · `rule open|close|status [<id>]` · `group open|close|status [<id>]` ·
`cashshop [enter|buy <sn> [nx|points]|get [uniqueid]|out]`（`csbuy/csget/csout` 已并入）·
`task start <id>` · `task stop` · `task status` · `reload` ·
`keymap set <key> <type> <action>` ·
`trade accept|confirm|decline|quit|put <itemid> [qty]|meso <n>` · `help` · `quit`

### 运行
```bash
cargo build --release
target\release\openstory-bot.exe --ip <ip> --port 8484 --account <acc> --password <pass> \
    [--gender 0] [--world 0] [--channel 0] [--char 0] [--cid <id>] \
    [--auto "cmd"] [--duration <secs>] [--show-packets] [--dump] [--damage <n>] \
    [--channel-ip <ip>] [--chat-admin <名字>] [--aes-key <hex16|hex512>] [--charlist-only]
```

> `--aes-key`：改密钥的服务端用——8 字节密钥（16 位 hex，运行时 `expand_key` 展开）或 256 字节预展开表（512 位 hex）；`examples/keyfind.rs` 从客户端提取、`examples/probe.rs` 现场验证。`--charlist-only`：收到角色列表即停不进图（探测服用）。

### Web 配置编辑器（独立程序，`crates/webui`）
```bash
cargo build --release -p openstory-webui
target\release\openstory-web.exe [--port 8080] [--config config.json] [--dir <页面目录>]
```
浏览器内编辑 config.json（表单 + JSON 视图），保存时按 bot 的 `RuntimeConfig` 架构校验后原子写入；
与 bot 完全解耦（独立进程、无内嵌构建、无进程内关联），运行中的 bot 需 `reload` 或重启后生效。

---

## 2. 已破解协议格式（字节级）

### 传输层
- 握手 16 字节：`[2..3]` = 版本(79)，`[7..11]` / `[11..15]` = 收发 IV
- 包头：长度 XOR 79，LE

### 组队
- 服务端邀请 `0x3B`：`byte 4 + int partyid + string 队长名 + byte 0`
- 接受邀请 `0x79`：`byte 27 + int partyid`
- 建队 `0x78`：`byte 1`；邀请 `0x78`：`byte 4 + string 名字`；离开 `0x78`：`byte 2`
- 入队成功 `0x3B` op 15 / 静默更新 op 7：`int partyid + [op15: string] + 6×int id + 6×13字节名 + 6×int job + 6×int lvl + 6×int chan + int leader + 6×int mapid + 120字节门数据`

### 玩家交易（2026-08-05 实现并双向验证 ✅）
收发均走 `PLAYER_INTERACTION`：收 `0x14F`、发 `0x77`，首字节 action 子码。

**服务端 → 客户端（0x14F）**：
| action | 含义 | 格式 |
|---|---|---|
| 2 | 交易邀请 | `byte 2 + byte 3 + string 对方名 + int 0` |
| 4 | 对方进入 | `byte 4 + byte 1 + addCharLook + string 对方名` |
| 5 | 交易窗口 | `byte 5 + byte 3 + byte 2 + byte number + [number==1: byte 0+charLook+name 对方] + byte number + charLook+name 自己 + byte 255` |
| 10 | 结果/取消 | `byte 10 + byte 槽位 + byte message(0=取消/1=成功/2=完成/8=完成/9,10=失败)` |
| 14 | 对方放物品 | `byte 14 + byte number(0=自己1=对方) + addItemInfo`（复用 `parse_item_info_body`） |
| 15 | 对方放钱 | `byte 15 + byte number + int meso` |
| 16 | 对方锁定 | `byte 16` |

**客户端 → 服务端（0x77）**：
| action | 含义 | 格式 |
|---|---|---|
| 0 | 开始交易 | `byte 0 + byte 3` |
| 2 | 邀请 | `byte 2 + int 对方cid` |
| 3 | 拒绝 | `byte 3` |
| 4 | 接受进入 | `byte 4` |
| 6 | 交易聊天 | `byte 6 + string` |
| 10 | 退出/取消 | `byte 10` |
| 14 | 放物品 | `byte 14 + byte 栏位(1=EQUIP..) + short 背包槽 + short qty + byte 交易槽(0xFF=服务端自动分配)` |
| 15 | 放钱 | `byte 15 + int meso` |
| 16 | 确认锁定 | `byte 16` |

**状态机**：邀请方 `CREATE(0+3)` → `INVITE(2+cid)`；被邀方收到 2 后发 `VISIT(4)` 接受 → 双方收 TRADE_START(5)；放物/放钱（14/15，服务端双向广播）；`CONFIRM(16)` 锁定（双方都锁才交换，meso 收税 `getTaxAmount`，完成后 TRADE_MESSAGE(10,8)）；`EXIT(10)` 取消。交易最多 9 格物品，同账号双角色交易会封号。

**命令**：`trade accept|confirm|decline|quit|put <itemid> [qty]|meso <n>`（状态看 `view trade`）。
- `trade put`/`trade meso` 须交易窗口已开；放物/放钱是流程行为，用任务步骤编排（如 `wait: "trade_active==1"` 后依次 put/meso/confirm，见 GUIDE 3.9 示例）；旧"窗口未开自动入队"机制已删（08-07）
- 自动接受/确认走规则：`rule open trade_accept`（自动接受邀请）、`rule open trade_confirm`（对方锁定后自动确认）
- `trade invite <名字>`（2026-08-06 新增）：**主动发起交易**——`0x77 0+3`（startTrade）→ `0x77 2+cid`（inviteTrade，须同地图、按 entities 玩家名查 cid）
- 物品自动入库：交易完成服务端发 0x20（ADD）自动进背包（`view inventory`），金币走 0x22
- 双向交易：收取（物品+钱）与给出（物品+钱）均支持

### 装备系统（2026-08-05 实现 ✅）
**装备属性块（`addEquipStats`，位于 addItemInfo 的装备分支）**：
```
byte 升级次数 + byte 等级 + 12×short(str dex int luk hp mp watk matk wdef mdef acc avoid)
+ short hands + short speed + short jump + string owner + short flag
+ byte incSkill + byte 需求等级 + int exp% + int vicious
[+ 8 唯一ID（无 unique 时）] + long getTime(-2) + int -1
```

**charinfo 装备栏编码（`addInventoryInfo`）**：装备栏分两块写出，位置字节
`pos<0 → -pos; write(pos>100 ? pos-100 : pos)`：
- 块1 普通位：-1~-99 → 字节 1~99
- 块2 现金位：-100~-999 → 字节 100（-100）、1~99（-101~-199）、100~899（-200~-999）
- 解析还原：块1 `slot=-p`；块2 `p<100 → -(100+p)`，`p==100 → -100`，`p>100 → -p`
- 之后才是 EQUIP 背包块（正槽位 1~96），字节流布局一致

**换装 `0x44`（ITEM_MOVE，客户端→服务端）**：`int tick + byte 栏位(1=EQUIP) + short src + short dst + short qty`
- `src>0, dst<0` → 穿装（dst=身体位负数，如 -1 帽 / -11 武器）
- `src<0, dst>0` → 脱装（dst=背包空槽）
- `dst=0` → 丢弃（已有 drop 命令）；服务端 `MapleInventoryManipulator.equip/unequip` 按符号分发

**0x20 MODIFY_INVENTORY_ITEM**：`byte updateTick + byte count + [byte mode + byte invType + short pos + ...]`，
换装回包为 MOVE(2) 模式（pos=旧位 → short 新位），装备位为负值时包尾附 1 字节 addMovement（bot 忽略）。
bot 按 `pos<0` 区分 equipped/inventory 双表同步。

**身体位映射**（itemid 前缀）：100帽(-1) 101脸(-2) 102眼(-3) 103耳(-4) 104上装(-5) 105全身(-5)
106下装(-6) 107手套(-7) 108鞋(-8) 109披风(-9) 110盾(-10) 111戒指(-12..-15) 112项链(-16)
113徽章(-17) 114勋章(-21) 115腰带(-19) 116口袋(-20) 119坐骑(-18) 130~149武器(-11)。

**命令**：`view equips`（穿着列表+属性）、`view equip <itemid>`（背包装备属性，背包优先/穿着兜底）、
`equip <itemid>`（穿，目标位占用则拒绝）、`unequip <itemid>`（脱，自动找空槽）。
支持：穿帽、脱帽（0x20 同步）、占位拒绝、属性解析。

### 技能系统（2026-08-06 实现 ✅）
**charinfo 技能块（`addSkillInfo`，位于 addInventoryInfo 之后）**：
```
short count + [int skillid + int level + (四转: int masterlevel)] × count
```
- 四转判定：skillid 末两位 12/22/32/42/52/62/72/82/92（`isFourthJobSkill`）
- charinfo 顺序：`... addInventoryInfo(5 个 tab) → addSkillInfo → addCoolDownInfo → addQuestInfo → addRingInfo → addRocksInfo → addMonsterBookInfo → QuestInfoPacket → short 0×3`

**SP 字段**：`addCharStats` 的 `short sp`；0x22 UPDATE_STATS `AVAILABLESP`(131072) 增量同步。

**加点 `0x57`（DISTRIBUTE_SP，客户端→服务端）**：`int tick + int skillid`（每次 1 点）。
服务端 `StatsHandling.DistributeSP` 校验：remainingSp、前置技能、`curLevel+1 <= maxlevel`、
`canBeLearnedBy(job)`、blockedSkills —— 失败静默忽略，无惩罚。

**加点回包 `0x27`（UPDATE_SKILLS，服务端→客户端）**：
`byte 1 + short 1 + int skillid + int level + int masterlevel + long expiration + byte 4`。
（`MaplePacketCreator.updateSkill`；多技能更新时 `byte 1; short count` 后重复条目）

**CASH 宠物项**（addItemInfo 宠物分支）：type 字节写 3，body 为 `addPetItemInfo`
（`long expiration + 13 字节名 + byte 等级 + short 亲密度 + byte 饱食 + long expiration + short 0 + short flags + short 0 + 4 字节`），
无 qty/owner/flag。宠物 itemid 前缀 500xxxx。修复前 CASH 块在此错位，导致 addSkillInfo 读取失败。

**命令**：`view skills`（SP + 全部技能等级）、`skill learn <skillid>`（加 1 点）、
`skill info <skillid>`（单技能等级）、`skill cast <skillid> <oid>`（攻击技能）、`buff <skillid>`（辅助技能）。
加点链路：0x22 扣 SP + 0x27 技能升级，重登后服务端回传一致。

**施放攻击技能 `0x28`**（CLOSE_RANGE_ATTACK 带 skillid）：布局同攻击（`DamageParse.parseDmgM`），
服务端校验技能等级与 MP/冷却后结算伤害，等级不需要客户端上报（取服务端记录）。

**施放辅助技能 `0x58`（SPECIAL_MOVE，客户端→服务端，`PlayerHandler.SpecialMove`）**：
```
skip(4) + int skillid + byte skillLevel + [short x + short y + byte faceLeft](可选尾)
```
- 服务端校验：`chr.getSkillLevel(skill) > 0` 且 **== 上报等级**（12101000 例外）；存活+地图存在
- 冷却：`effect.getCooldown() > 0 && !isGM` → skillisCooling 冷却中拒绝；否则回 `0xEC COOLDOWN` + 注册冷却
- 应用：`effect.applyTo(chr, pos)` → 扣 MP（0x22 回包）→ `overTime` 技能 `registerEffect`：
  广播 `0xC0 SKILL_EFFECT`（其他玩家动画）+ 给自己 `0x23 GIVE_BUFF`（图标+数值+duration）；
  特殊技能走专用包（影分身/灵魂箭矢/连击 giveForeignBuff 变体、海盗 givePirate）
- 到期/解除：`0x24 CANCEL_BUFF`（自己）+ `0xC1 CANCEL_SKILL_EFFECT`（其他玩家）；客户端取消发 `0x59 CANCEL_BUFF`
- 攻击技能与 buff 的本质区别：攻击走 0x28/0x29/0x2A（DamageParse 距离/目标校验）；buff 走 0x58（无目标无距离判定）
- 充能蓄力技能（暴风箭雨类）走 `0x5A SKILL_EFFECT`（`PlayerHandler.SkillEffect`，仅广播动画不应用 buff）

**取消 buff `0x59`（CANCEL_BUFF，客户端→服务端，`PlayerHandler.CancelBuffHandler`）**：
`int sourceid`；普通 buff → `cancelEffect(skill.getEffect(1))`；蓄力技能 → 清 keyDown + 广播 `skillCancel`。

**buff 覆盖规则（服务端）**：`registerEffect` 的 `effects: Map<MapleBuffStat, Holder>` 按 buffstat 键 `put` 覆盖——同类型 buff 只存在一个，后放的顶掉先放的（0x23 重新下发替换图标），客户端只渲染不决策。

### 键盘绑定 keymap（2026-08-06 实现 ✅）
**下发 `0x16F KEYMAP`**（登录时，`MaplePacketCreator.getKeymap` / `MapleKeyLayout.writeData`）：
```
byte 0 + 固定 90 键位(0-89) × [byte type + int action]   （空绑定 = (0,0)，键码隐含）
```
**改键 `0x83 CHANGE_KEYMAP`**（`PlayerHandler.ChangeKeymap`）：
```
int tick + int count + [int key + byte type + int action] × count
```
- **服务端零校验**：任意键绑任意 action 都接受（`changeKeybinding`），登出 `saveKeys` 存 DB keymap 表（characterid/keye/type/action），重登回传
- type：0=空 1=技能 2=物品 3=? 4=UI 5=表情 6=宏；键码为客户端 1-89（Shift=42、PageDown=81 等）
- 应用：配合一键满技（技能进角色表），把技能树外的技能绑到键盘，施放链路无职业校验 → 跨职业技能全 buff

### 聊天系统（2026-08-06 实现 ✅）
**opcode 权威来源**：服务端实际 opcode 表在 `079.jar` 内 `properties/recv.ini` 与
`properties/send.ini`（反编译源码里的枚举是过时的！）。已核对：
- 客户端→服务端：GENERAL_CHAT 0x2D、PARTYCHAT 0x74、WHISPER 0x75、MESSENGER 0x76、
  PLAYER_INTERACTION 0x77、PARTY_OPERATION 0x78、PET_CHAT 0xA6
- 服务端→客户端：SERVERMESSAGE 0x41、SET_WEEK_EVENT_MESSAGE 0x4E、MULTICHAT 0x8A、
  WHISPER 0x8B、CHATTEXT 0xA4、PET_CHAT 0xB0
（bot 原 opcode 全部正确，无需修正）

**编码**：服务端 `writeMapleAsciiString` 用 `ServerConstants.MAPLE_TYPE.getANSI()`，
`MAPLE_TYPE = MapleType.中国` → **GBK**。bot 解码：先试 UTF-8 严格解码（ASCII 直通），
失败回退 GBK；发送聊天 `write_string_gb`（非 ASCII 才做 GBK 编码）。
中文发送/接收往返无损。

**接收格式**：
| opcode | 来源 | 格式 |
|---|---|---|
| 0xA4 CHATTEXT | 地图广播 | `int cidfrom + byte whiteBG + string text + byte show` |
| 0x8A MULTICHAT | 跨图分组 | `byte mode + string name + string text`（0=好友 1=组队 2=公会 3=联盟） |
| 0x8B WHISPER | 密语 | 收 `byte 18 + string sender + short channel-1 + string text`；回执 `byte 10 + string + byte reply`（忽略） |
| 0x4E SET_WEEK_EVENT_MESSAGE | GM 黄字 | `byte -1 + string` |
| 0x41 SERVERMESSAGE | 系统 | `byte type + [type==4: byte bool] + string text + 尾部`（type 3/8/11/12: channel+bool…、10: lines…、13: item，尾部全 skip；type 4 && bool==0 无文本） |

**发送格式**：`chat <text>` → GENERAL_CHAT 0x2D `string + byte 0`（非 GM 文本 ≥80 字符被拒；
CanTalk 失败回 serverNotice(6)）。服务端 `@`/`!` 前缀命令（CommandProcessor 私有化）不集成。

**命令**：`chat <text>` / `say` / `c`。
接收覆盖：地图聊天、密语、GM 黄字、系统消息（type 0/1/4/6 滚动横幅等）。

**密语发送 `0x75`（WHISPER，客户端→服务端，`ChatHandler.WhisperFind` mode 6）**：
`byte 6 + string recipient + string text`（GBK）。服务端跨频道投递（`World.Find.findChannel`），回 `getWhisperReply`。

**聊天控制 ChatCommand（2026-08-06 实现 ✅）**：
- 入口：`0x8B` 密语接收（`parse_whisper`）→ 白名单（`--chat-admin`，config.chat_admins）→ 文本 `#` 前缀 → `chat_command`
- 回执：结果逐行 `0x75` 密语发送（聊天栏 shell 式报告）
- 查询类（纯状态）：status/player/where/skills/reactor/inv[<tab>]；操作类（复用 `command::run`）：drop/dropmeso/pickup
- 安全：白名单外直接忽略；操作类无二次确认（仅限白名单）

### 攻击
- 客户端→服务端 `0x28`（近战，`DamageParse.parseDmgM`）：`skip(1)+skip(8)+byte tbyte(目标<<4|次数)+skip(8)+int skill+skip(12)+byte unk+byte display(朝向)+byte animation(0)+skip(1)+byte speed+int 时间戳+[每目标: int oid+skip(14)+次数×int 伤害+skip(4)]+short x+short y`
- 服务端→客户端广播 `0xBC`（`closeRangeAttack`）：`int cid+byte tbyte+byte lvl+[byte level+int skill | byte 0]+byte unk+byte display+byte animation+byte speed+byte mastery+int 0+[每目标: int oid+byte 7+次数×int 伤害]`

### 换线（2026-08-06 实现 ✅，08-20 实测确认）
- 客户端→服务端 `0x22 CHANGE_CHANNEL`：`byte channel`（wire 0 基，命令层 1 基发 n-1；服务端 `readByte()+1`；`InterServerHandler.ChangeChannel`）
- 前置检查：blockedInventory / 事件图 / `FieldLimitType.ChannelSwitch`（禁换线地图）/ 反外挂 → 拒绝回 enableActions
- 服务端 `MapleCharacter.changeChannel`：`saveToDB` → dispelBuff → buff/冷却/异常状态存 `PlayerBuffStorage` → `World.channelChangeData(CharacterTransfer)` 缓存 → 移除当前频道 → 回 **`0x13`**（`getChannelChange`）：`byte 1 + byte ip×4 + short port + byte 0`（**无 cid**）；目标频道 == 当前频道或频道不存在/shutdown → 回 `serverBlocked`（0x85）不发 0x13
- 服务端→客户端 `0x13` 解析：`skip(1)+ip(4)+short port`（`parse_channel_change`）
- 客户端：断开 TCP → 重连目标频道 → `PLAYER_LOGGEDIN 0x0B`（`int cid`）→ `InterServerHandler.LoggedIn` 按 cid 取 `getPendingCharacter` 恢复 → SET_FIELD 进图
- 防作弊：login key 三重校验（非法换线断连 + GM 广播）+ 同账号多开检测（同账号角色在线 → 踢 + GM 广播）
- bot 复用登录的 `SERVER_IP → reconnect → PLAYER_LOGGEDIN` 链路（`reconnect_to_channel`），cid 取 `my_cid`
- **08-20 本地服实测（`packet/channel.txt` 官方客户端 + 服务端 `Log.txt` 双向核对）**：bot 的 0x22 请求
  `22 00 <channel> <tick 4B>` 与官方客户端**逐字节一致**（tick 为单调毫秒，服务端不校验）；0x13 应答
  `13 00 01 <ip×4> <port×2> 00` 解析正确；双向换线（0↔1）4/4 成功，全程无守卫拒绝。tick 语义：官方客户端
  用开机起毫秒（~10⁸ 量级），bot 用进程起毫秒——服务端只读首字节频道号，无影响。
- **08-20 修复**：换线重连后 `[timeout] map change timed out` 误触发——`reconnect_to_channel` 只设
  `phase=EnteringMap` 未重置 `map_enter_time`（reenter 路径有重置），陈旧计时器在重连后立刻超时。
  已在 `src/handlers/login.rs` reconnect_to_channel 重置 `map_enter_time`，实测不再误报。

### 重新进入当前地图 `reenter`（2026-08-11 实现 ✅）
- **机制**：商城强制往返，与换线同一套 CharacterTransfer 交接，落回**原图原频道**：
  `ENTER_CASH_SHOP 0x23`（空包）→ 服务端 `InterServerHandler.EnterCashShop`：saveToDB → dispelBuff → buff/冷却存 `PlayerBuffStorage` → `World.channelChangeData(CharacterTransfer, -10)` → 移除当前频道 → 回 **0x13**（商城服 ip+port）→ bot 重连 + PLAYER_LOGGEDIN → 商城服 `SET_CASH_SHOP 0x83` 进商城（`state.phase = CashShop`）
  → bot **自动**发 `CHANGE_MAP 0x21` 空包（商城服侧 `CashShopOperation.LeaveCashShop` 不读包体）：saveToDB → `World.channelChangeData(CharacterTransfer, 频道)` → 回 **0x13**（原频道 ip+port）→ bot 重连 + PLAYER_LOGGEDIN → `LoggedIn` 按 pendingCharacter 恢复 → SET_FIELD **原图** → 地图对象全量重新生成
- **无登录通告**：CharacterTransfer 正常交接走 `getPendingCharacter` 恢复分支，不触发非法登录检测/GM 广播（完全重登才走 `loadCharacterNamesByCharId` 重复检测）
- **地图限制**（服务端 `MapleServerHandler.ENTER_CASH_SHOP`）：禁入图仅 980000xxx 道场系列 + 180000001；额外条件：非 shutdown / `CS_ENABLE` / 非事件实例 / 无防作弊进行中
- **被拒兜底**：进入被拒（CS_ENABLE=false/禁入图/事件图，只回 enableActions 不重连，phase 停在 InGame）→ main 循环 10s watchdog 放弃往返并按快照恢复 hunt
- **实现**：`state.reenter: Option<ReenterCtx{restore_hunt, mapid, leave_sent, started}>`；`Command::Reenter` 快照 hunt→发 enter→重置 `map_enter_time`（复用 10s 窗口覆盖两次 0x13）；main 循环 tick 分支：`CashShop && !leave_sent` → 自动发一次 leave（防 50ms 刷包），`InGame && !leave_sent && 超10s` → 放弃；SET_FIELD(mode2==3) 落回 `mapid==ctx.mapid` → 恢复 hunt 快照
- **进图清场**（reenter/换线/town 同受益）：SET_FIELD mode2==3 现清 `entities`（retain 仅自己 cid 玩家项）、`platforms`、`ground_y`、`hunt_target`（原只清 npcs/reactors/drops，残留会导致 hunt 追旧图幽灵怪）

### 掉落 `0x110` / 拾取 `0xC6`
- `0x110 dropItemFromMapObject`：`byte mod+int oid+byte isMeso+int itemId+int owner+byte dropType+short x+short y+[int]+[short fx;fy;0]+[long exp]+short playerDrop`
- `0xC6 ITEM_PICKUP`：`int tick+byte 0+short x+short y+int oid`（上报物品自身坐标可规避 50px 的 ITEMVAC_CLIENT；服务端真实距离 >800px 仅记 ITEMVAC_SERVER，每 5 次 GM 广播提示，无自动封号、不拦截拾取）
- `0x111 removeItemFromMap`：`byte animation+int oid+[int cid]+[byte slot]`

### 放置物 Reactor（2026-08-06 破解 ✅）
- 客户端→服务端 `0xC9 DAMAGE_REACTOR`：`int oid+int charPos+short stance`；服务端（`PlayersHandler.HitReactor`→`MapleReactor.hitReactor`）**只查 oid 存活，无距离/冷却/技能校验、无防作弊**
- 状态机：hit 推进一态（type==2 机关类要求 charPos∈{0,2}），终结态 → 销毁 + 执行 `反应堆/<rid>.js`（`rm.dropItems()` 按 DB reactordrops 表 chance 抽，掉在 reactor 位置，owner=触发者）；`delay=reactorTime×1000ms` 后重生（state 归 0）
- 服务端→客户端：`0x11E REACTOR_SPAWN` = `int oid+int rid+byte state+pos+byte facing+string name`；`0x11C REACTOR_HIT` = `int oid+byte state+pos+short stance+byte 0+byte 4`；`0x11F REACTOR_DESTROY` = `int oid+byte state+pos`
- 与打怪的本质区别：怪攻击有 `DamageParse` 范围/技能判定 + 封号，reactor 攻击零校验；拾取距离也只警告不拦截

### 移动广播 `0xBB` 与移动片段
`int cid + int 0 + byte 数量 + 移动片段`（`serializeMovementList`）。
片段（`StaticLifeMovement.serialize`）：
- command 0/5/17：`short x+short y+short vx+short vy+short unk+byte newstate+short duration`（绝对）
- command 1/2/6/12/13/16/18/19/22：`short vx+short vy+byte newstate+short duration`（相对速度）
- command 3/4/7/8/9/11：`short x+short y+short unk+byte newstate+short duration`
- command 10：`byte wui`；command 14：`short vx+short vy+short fh+byte newstate+short duration`
- command 15：绝对 + `short fh`

### UPDATE_STATS `0x22`
`byte itemReaction + int mask + 值...`，值顺序按 `MapleStat` 枚举声明序，类型：SKIN/LEVEL=byte，
JOB/STR/DEX/INT/LUK/HP/MAXHP/MP/MAXMP/AP/FAME=short，EXP/FACE/HAIR/MESO=int，PET=3×long。
已解析：HP/MAXHP/MP/MAXMP/EXP/LEVEL/AP（AVAILABLEAP）。加点 `DISTRIBUTE_AP 0x54`：
`int tick + int 属性值(STR=256/DEX=512/INT=1024/LUK=2048/MAXHP=8192/MAXMP=32768)`，一次 +1。

### 角色姿态（newstate，+1 = 面向左）
- WALK=2、STAND=4、FALL=6、ALERT=8、PRONE=10、SWIM=12、LADDER=14、ROPE=16、DIED=18、SIT=20
- 移动片段 newstate 低位（bit0）= 朝向：0=右、1=左

### SPAWN_PLAYER 自定位
从自己的 `0xA2` 解析真实出生点（268 字节包）：head(`int cid+byte lvl+string name`) +
guild(`string+6字节`) + 16字节 buff 掩码 + 127字节固定块 + `addCharLook` + 20字节固定 + `short x + short y + byte stance`。

---

## 3. 架构

```
src/
  main.rs       入口 + select! 主循环 + stdin
  lib.rs        模块声明
  config.rs     CLI 配置
  session.rs    tokio TCP + 握手/加密/分包/收发
  packet.rs     读写游标（越界置 failed 标志，不 panic）
  opcodes.rs    CongMS 收发 opcode 表
  crypto.rs     AES+Shanda+IV+包头
  state.rs      角色状态/实体/掉落/平台/stats/背包/队伍
  emit.rs       结构化事件通道：Category 分类 + Field 语义键 + emit_f! 宏
  names.rs      data/*.json 中文名称词典（地图/NPC/怪物/物品/技能）
  chat_report.rs 播报通道（[INFO]/[OK]/[WARN]/[ERR] 密语推送 admin）

  command/      命令系统
    mod.rs      枚举定义 + 共享辅助函数
    parse.rs    命令解析（stdin / --auto）
    run.rs      命令执行（操作类）
    view.rs     查看体系（view <对象> 全部只读查询）
    tick.rs     周期逻辑：打怪/拾取/攻击方式(普攻/技能/范围/定点瞬击)/喝药/卖装/reactor 挂机/卡死播报
    rules.rs    规则引擎（谓词求值 + 动作执行，支持 quit 动作下线）
    tasks.rs    任务引擎（锁栈模型 + 组门控）
    tick.rs     周期逻辑（见上）

  handlers/     协议处理
    mod.rs      主分发（opcode → handler）
    login.rs    登录/选角/进图/换线重连
    field.rs    场景/实体/移动/攻击/掉落/升级/聊天控制/keymap/reactor
    party.rs    队伍操作

  parsers/      协议解析
    mod.rs      模块声明 + 重导出
    login.rs    登录状态/世界列表/角色列表/服务器地址/换线地址
    field.rs    怪物/玩家/攻击广播/掉落/血蓝经验/角色信息
    movement.rs 移动片段解析
    inventory.rs 背包物品解析
    skill.rs    技能块解析
    chat.rs     聊天解析

  packets/      协议构造
    login.rs    登录/选角/心跳/换线
    movement.rs 移动包构造
    combat.rs   攻击/buff/取消buff/reactor/keymap 包构造
    item.rs     拾取/使用/丢弃
    party.rs    队伍包
    stats.rs    加点包
    chat.rs     聊天/密语包
    map.rs      跨图移动（CHANGE_MAP / CHANGE_MAP_SPECIAL）
    npc.rs      NPC对话/商店（NPC_TALK / NPC_TALK_MORE / NPC_SHOP）

crates/console-lib/ 共享前端层（控制台与将来的 Web 前端共用，不含 ratatui 依赖）
  text.rs       字符/显示宽度、插入删除、折行、截断（中文算 2 列）
  theme.rs      冒险岛配色
  zh.rs         中文渲染层（tag 词典/字段渲染/整句覆盖/隐藏规则）
  enrich.rs     id → 中文名富化（mobid=/npcid=/itemid=）
  logbuf.rs     1000 行环形日志 + 折行缓存 + O(1) 交换
  eventq.rs     事件队列（每会话一份；毒化安全）
  snapshot.rs   状态快照（try_lock 一次取全）
  command_spec.rs 命令表唯一数据源（ArgSpec / subs / dyn_args / sample）
  completion.rs 补全五级决策（表驱动，与解析器一致性由测试守住）

crates/console/    中文控制台（openstory-console，ratatui + crossterm）—— 单开/多开/管理器同一个程序
  main.rs       入口：选择屏多选 / 向导 / 多开启动 / bot 线程 + 渲染线程
  app.rs        App 状态机：按键/补全/历史/滚动/事件轮询/拉起调度/启停与增删账号
  ui.rs         界面绘制（状态栏/日志/右侧面板/左栏账号列表/F2 看板/弹窗）
  session.rs    单个 bot 会话（状态/队列/日志/指令通道/拉起状态机/手动启停状态）
  sessions.rs   会话集合（选中 / `@` 目标解析 / 批量下发 / 启停 / 增删账号 / 退出条件）
  watcher.rs    掉线自动拉起的纯状态机（退避 / 放弃 / 手动停 / 进程级句柄）
  credentials.rs 密码双模式存储（profiles/.manager.json，默认不落盘）
  profile.rs    档案选择屏（多选 = 管理器入口）+ 缺密码询问
  wizard.rs     登录向导

crates/webui/  独立 Web 配置编辑器（openstory-webui，与 bot 无进程内关联）
  lib.rs       tiny_http 服务器：页面 + /api/config 端点（回调注入）
  main.rs      openstory-webui 入口：CLI 参数 + config.json 读写校验（原子写入）
  web/index.html 浏览器配置页（登录/猎杀/药水/规则/任务/通用表单 + JSON 视图）

examples/       工具
  convert_skills.rs  Skill.img.json → data/skill.json 归一化转换
  keyfind.rs / probe.rs / login_test.rs / decrypt_real_pkt.rs

tests/login_flow.rs  假服务器端到端测试
```

### 事件通道与中文渲染
- bot 核心所有输出走 `src/emit.rs`：`Event{ts, level, category, text, fields}`，
  `Category` 决定日志过滤页签（13 类：聊天/公告/NPC/打怪/命令/查看/规则/**组**/任务/封包/系统/错误），
  `Field` 语义键（约 45 项）承载结构化事实
- CLI 未注册 Emitter 时直接打印英文原文（逐字节稳定）；TUI 注册队列 Emitter 后
  按 `zh_hidden → zh_body → render_fields → 英文回退` 顺序渲染中文
- 详情见 [docs/TUI.md](docs/TUI.md) 的「中文渲染体系」

### 数据流
收包：`session.recv_packet → handlers::handle → parsers::* → state 更新`
发包：`command/tick|chat_report → packets/* 构造 → session.send_packet`

### 主循环调度
`select!` 收包 + stdin + duration 到期 + 周期唤醒；tick 用墙钟门控（周期 =
config `tick_ms`，默认 50ms、可压到 1ms）在 select 外执行，避免 select 重建
定时器导致节流失控。

---

## 4. 已知问题与限制

1. ~~**玩家发现（已修复）**~~：CongMS 的 `MapleMap.addPlayer` 给新玩家发送的入图快照不含
   PLAYER；新玩家需移动一次触发 `map.movePlayer` 动态可见性。→ 已实现进图自动微移。
2. **平台间移动是"传送"式**：瞬移模式（tp）下直接绝对移动，步行模式升层也是直接跳（非跳跃动画），视觉突兀。
3. **伤害/攻击动画**：客户端不显示 bot 的伤害数字，疑似客户端渲染限制（结论待验证）——
   服务端链路已确认无问题：`maxViewRangeSq()=Integer.MAX_VALUE`（GameConstants.java:178）→ 攻击广播
   0xBC 全图广播无距离裁剪（MapleMap.java:3205），伤害值为服务端权威重算（Modify_AttackCrit）。
   客户端侧渲染被视野门控：攻击者与受击怪都要落在玩家屏幕内，才会播放动画并浮出伤害数字。
    推测"看不到"场景：① bot 瞬移打怪/定点瞬击谎报位置 (30000,y) → 攻击者在玩家屏幕外，原版客户端
    攻击动画不渲染 → 数字不浮出；② 秒杀时 0xEF 怪死亡移除与 0xBC 几乎同时到，客户端先删怪 →
    数字来不及显示；③ 怪不在玩家视野内。贴身打怪 +
    玩家同屏 + 怪未被秒杀时可见，符合"有时看不到"的观察。待客户端抓包验证 0xBC 必达。
4. **spawn 视野限制**：怪 spawn 只广播给 `maxViewRangeSq` 内玩家，瞬移拉远后可能看不到新刷的怪（已有"去最近已知怪"兜底）。
5. **hunt 卡死告警的信号为 EXP 增长**（08-08 两次修正）：旧实现只认 0xF8（怪受伤
   广播）——该包只发给怪的控制者，别人抢怪/控怪时 bot 收不到，导致明明在持续打怪
   也误报 `[WARN] hunt stuck`。现已改为只看 0x22 EXP 增长（击杀入账必然收到，
   组队分摊也收；0xF8 仅保留 [dmg] 日志，不再作检测信号），按 1 条/分钟节流。
   仅在真无进展时告警，不干预。

6. **攻击方式配置化**（08-16）：`hunt attack [n]`（普攻每怪一包，n=attack_max_targets）、
   `hunt skill <id> [n] [range] [mp]`（技能群包/无技能群发，id=0）、`hunt range <px>`
   （攻击范围）。游戏内密语 **#hunt** 支持 `on|off|attack|skill|range|status`，
   `#hunt skill <id>` 带 id 暂不支持——等技能属性数据齐了再开放游戏内输技能 id
   （本地 stdin `hunt skill <id>` 仍保留调试用）。
7. **任务结束 hunt 恢复**（08-08 修复）：手动 `#task start sell` 卖完回来不再傻站——
   任务启动时快照 hunt 开关（`TaskRuntime.hunt_before`），锁栈全空时按最早任务快照
   恢复，hunt 在任务前开着的就继续打怪。
8. **装备谓词语义明确化**（08-08）：谓词 `equips` = **背包装备（EQUIP 栏）件数**
   （使用率高，默认保留），新增 `equips_worn` = **身上穿着装备数**
   （`state.equipped.len()`）。命名与指令视角对齐：`view equips` 看穿着、
   `view inventory [equip]` 看背包装备，`equip <itemid>` 穿装——`equips`（穿着
   查询）与 `equip`（穿装动作）指令路径不冲突，tab 名保持 `equip|use|setup|etc|cash`。
9. **攻击广播的 display/animation 字节语义修正**（08-08）：0x28 攻击包的
   `display`/`animation` 两字节经服务端原样转进 0xBC 广播，客户端解析为
   `toleft`（朝向）与 `stance`（攻击姿态选择器）。旧实现把朝向塞进 animation
   bit7（0x80），且 display 恒 0——客户端取到无效姿态（有效值 1~36，
   0/≥37 视为无）时**攻击动画不播放**、特效镜像方向恒朝右，
   客户端看不到 bot 的动作与技能特效。已修正：`display←facing(0/1)`、
   `animation←23 (swingO1 单手剑挥砍)`，客户端正常触发
   挥砍动画 + afterimage 刀光 + 技能 hit 特效（特效数据在客户端本地 wz，
   由 0xBC 的 skill+level 触发播放，服务端只做权威转发）。
   注意：客户端渲染仍有可见性门控——攻击者需在玩家屏幕内（`chars.get_char`）、
   被攻击怪需在视野内（`mobs.contains`）才有动画/特效/伤害数字（见问题 3）。

---

## 5. 服务端源码参考（读取指引）

服务器运行 jar 的反编译源码：`<服务端目录>\dist\079-src\`
（与 `079.jar` 一致，可直接当权威实现查）。

**opcode 权威来源**：`079.jar` 内 `properties/recv.ini`（客户端→服务端）、
`properties/send.ini`（服务端→客户端）——反编译源码 `MaplePacketCreator`/handler 里的
枚举值是**过时的**，以 ini 为准（如 CHANGE_CHANNEL：recv.ini=0x22 客户端发 / send.ini=0x13 服务端发）。

**服务端抓包日志**：`<服务端目录>\logs\数据包接收\Log.txt`
（GBK 编码，PowerShell 读取：`[System.Text.Encoding]::GetEncoding(936)`）。
视角以服务端为准：`[接收]`=客户端→服务端，`[發送]`=服务端→客户端。
bot 侧对照抓包：`packet/channel.txt`（官方客户端换线）、`--show-packets --dump` 原始十六进制。

**关键类/位置**（换线链路为主线）：
| 关注点 | 位置 |
|---|---|
| 换线守卫 + 分发 | `handling/channel/handler/InterServerHandler.java`（ChangeChannel 约 L442） |
| changeChannel / serverBlocked | `client/MapleCharacter.java`（changeChannel） |
| 0x13 getChannelChange / enableActions / updatePlayerStats | `tools/MaplePacketCreator.java`（enableActions ≈ L126，空 UPDATE_STATS `22 00 01 00 00 00 00 00 00` 9 字元） |
| opcode 表（recv/send.ini） | `079.jar` 内 `properties/` |
| 中文字符串编码（GBK） | `ServerConstants.MAPLE_TYPE = MapleType.中国` → `writeMapleAsciiString` |






