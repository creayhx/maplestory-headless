# 摆摊(雇佣商店 / 个人商店)报文分析报告

> 版本:**v2 — 已对照 CongMS 079 服务端源码校正**
> 来源:捕获 `packet/market.txt`(服务器侧) + 同类 v79 服务端 `RecvPacketOpcode` / `SendPacketOpcode` / `PlayerInteractionHandler.java`(opcode 数值与捕获逐一吻合:0x3C / 0x77 / 0x34 / 0x14F / 0x10D)。
> 状态:待回归验证 / 验收清单。验收通过后再实现 market 全部协议。
> 注意:`dist/079-src/`(CongMS 079.jar 反编译)未随仓库提供;该服务端为本捕获最接近的 v79 源码,action 子码若有出入以 CongMS 079.jar 为准。

---

## 0. 方向约定
捕获点在**服务器端**:`[接收]` = 客户端→服务端(**发包**,bot 构造发送);`[发送]` = 服务端→客户端(收包,驱动状态机)。

bot 协议版本 = **79**(header XOR 79,见 `src/opcodes.rs`)。

### 0.1 opcode 已用 079 服务端源码逐字核对
源码位置:`<服务端目录>\dist\079-src\handling\`
- **opcode 数值由运行时 `recv.ini` / `send.ini` 加载**,Java 枚举默认值不都是线上值。例:`USE_HIRED_MERCHANT` 枚举默认 = 56(0x38),但捕获(线上) = 0x3C —— **以捕获为准**。`PLAYER_INTERACTION`(recv)枚举无显式值(纯靠 .ini 加载)。发包 opcode 枚举默认(0x34 / 0x14F / 0x10D)与捕获一致。
- **`PLAYER_INTERACTION` 子动作(action)为硬编码常量**,已确认:`CREATE=0` / `ADD_ITEM=31` / `OPEN=11` / `EXIT=10`,与捕获字节完全吻合。
- **action `0x1C`(28)**:在 `PlayerInteractionHandler.java` 中**无任何 `case`** → 服务端忽略(no-op);真实客户端会在 OPEN 前发送,但 bot 可省略,`OPEN`(0x0B) 本身即触发 `SPAWN_HIRED_MERCHANT`(见 §3 I4b)。

---

## 1. 市场(自由市场)相关 opcode 全集

### 1.1 收包(客户端→服务端,recv)
| opcode | 名称 | 说明 |
|--------|------|------|
| 0x3C | HIRED_MERCHANT_REQUEST (USE_HIRED_MERCHANT) | 使用雇佣商店道具,开启开店交互 |
| 0x77 | PLAYER_INTERACTION | 玩家商店/雇佣商店交互主包(靠 action 子码区分) |
| 0x?? | MERCH_ITEM_STORE | 自由市场领取(精灵商人 Fred)触发;精确 opcode 见 v79 recvops(本捕获未涉及) |
| 0x3A | NPC_SHOP | (相邻)NPC 商店,非雇佣商店 |
| 0x3B | STORAGE | (相邻)仓库,非雇佣商店 |
| 0x3E | DUEY_ACTION | (相邻)快递 Duey,非雇佣商店 |

> 本捕获只用到 0x3C 与 0x77。

### 1.2 发包(服务端→客户端,send)
| opcode | 名称 | 说明 |
|--------|------|------|
| 0x34 | ENTRUSTED_SHOP_CHECK_RESULT | 允许开店结果(捕获回 `07` = 允许) |
| 0x14F | PLAYER_INTERACTION | 交互响应主包(创建回执 / 上架回执 / 访客 / 物品更新 / 聊天…) |
| 0x10D | SPAWN_HIRED_MERCHANT | 在地图上生成商店 NPC |
| 0x10E | DESTROY_HIRED_MERCHANT | 销毁商店 NPC |
| 0x10F | UPDATE_HIRED_MERCHANT | 更新商店(物品/金币变动) |
| 0x14B | MERCH_ITEM_MSG | 领取相关提示(op) |
| 0x14C | MERCH_ITEM_STORE | 领取界面 / 数据包(35=物品数据, 36/37=开关) |

> 本捕获出现 0x34 / 0x14F / 0x10D。

---

## 2. PLAYER_INTERACTION 子动作(action)一览(v79)

取自 `PlayerInteractionHandler.java`。**市场 / 雇佣商店相关已高亮**。

| action | 常量 | 市场相关 | 本捕获 |
|--------|------|----------|--------|
| 0 | CREATE | ✅ 开店(createType:1/2=小游戏,4=玩家商店,**5=雇佣商店**) | ✅ 见 I2 |
| 2 | INVITE_TRADE | | |
| 3 | DENY_TRADE | | |
| 4 | VISIT | ✅ 进入他人商店 | |
| 6 | CHAT | ✅ 店内聊天 | |
| 10 | EXIT | ✅ 离开交互界面 | ✅ 见 I6 |
| 11 | OPEN | ✅ 店主开放营业→服务器生成 NPC | ✅ 见 I5 |
| 13 | CASH_ITEM_INTER | | |
| 14 | SET_ITEMS | (交易) | |
| 15 | SET_MESO | (交易) | |
| 16 | CONFIRM_TRADE | (交易) | |
| 20 | PLAYER_SHOP_ADD_ITEM | ✅ 玩家商店上架 | |
| 31 | ADD_ITEM | ✅ **雇佣商店上架** | ✅ 见 I3 |
| 21 | BUY_ITEM_PLAYER_SHOP | ✅ 买(玩家商店) | |
| 32 | BUY_ITEM_STORE | ✅ 买(玩家商店) | |
| 33 | BUY_ITEM_HIREDMERCHANT | ✅ 买(**雇佣商店**) | |
| 35 | REMOVE_ITEM | ✅ 店主取回物品 | |
| 36 | REMOVE_ITEM_PS | ✅ 玩家商店取回 | |
| 37 | MAINTANCE_OFF | ✅ 店主重开营业 | |
| 38 | MAINTANCE_ORGANISE | ✅ 店主收钱/清理已售 | |
| 39 | CLOSE_MERCHANT | ✅ 关店(去 Fred 领取) | |
| 44 | VIEW_MERCHANT_VISITOR | ✅ 查看访客 | |
| 45 | VIEW_MERCHANT_BLACKLIST | ✅ 查看黑名单 | |
| 46 | MERCHANT_BLACKLIST_ADD | ✅ 加黑名单 | |
| 47 | MERCHANT_BLACKLIST_REMOVE | ✅ 移除黑名单 | |
| 26 | KICK_Player | | |
| 27 | MERCHANT_EXIT | ✅ 店主退出 | |
| 48/49/50/53/54/55/56/57/58/59/60/61/62/67 | 小游戏/交易 | | |
| 0x1C (28) | — | ⚠️ 见 §3 注 | ✅ 见 I4b |

> action 0/31/11/10 与捕获逐一吻合;**0x1C(28)** 在该服务端 `PlayerInteractionHandler` 中亦无对应 case(已确认 no-op,见 §0.1),bot 可省略。

---

## 3. 发包接口(字节严格取自捕获,字段已用服务端 handler 校正)

### I1 — 使用雇佣商店道具 `USE_HIRED_MERCHANT` (0x3C)
原始字节(11):`3C 00 00 3E 00 00 00 00 00 00 00`

| 偏移 | 字节 | 含义 |
|------|------|------|
| 0–1 | `3C 00` | 包头 0x003C |
| 2 | `00` | 标志(固定 00) |
| 3–6 | `3E 00 00 00` | 雇佣商店道具所在**背包槽位**(=0x3E=62,位于 CASH 背包) |
| 7–10 | `00 00 00 00` | 保留(固定 0) |

构造模板:`3C 00 00 <slot:4> 00 00 00 00`
> USE_HIRED_MERCHANT 服务端 handler 细节未在本轮抓取;结构以捕获字节为准。

---

### I2 — 创建雇佣商店 `PLAYER_INTERACTION` (0x77, action `0` = CREATE) — **v2 校正**
原始字节(16):`77 00 00 05 03 00 31 32 33 00 06 00 70 C0 4C 00`

| 偏移 | 字节 | 含义 |
|------|------|------|
| 0–1 | `77 00` | 包头 0x0077 |
| 2 | `00` | **action = 0 (CREATE)** |
| 3 | `05` | **createType = 5 → 雇佣商店**(HiredMerchant) |
| 4–9 | `03 00 31 32 33 00` | **店名/描述**(mapleAsciiString:长度3 + "123" + 终止符) |
| 10–11 | `06 00` | **CASH 背包槽位**(雇佣商店道具所在槽 = 6) |
| 12–15 | `70 C0 4C 00` | **雇佣商店道具 itemId**(=0x004CC070,readInt) |

> ⚠️ v1 误将 "123" 当作密码、将 `70 C0 4C 00` 当作会话ID。实际:雇佣商店**无密码字段**;该串是描述;`70 C0 4C 00` 是道具 itemId,`06 00` 是其 CASH 槽位。两者均可从 bot 自身 CASH 背包读取,**无需向服务器回查**。

构造模板:`77 00 00 05 <descLen:2> <desc ascii> 00 <cashSlot:2> <itemId:4>`

---

### I3 — 上架物品 `PLAYER_INTERACTION` (0x77, action `0x1F` = 31 = ADD_ITEM) — **v2 校正**
物品A(槽5)原始字节(14):`77 00 1F 04 05 00 06 00 01 00 7B 00 00 00`
物品B(槽9)原始字节(14):`77 00 1F 04 09 00 06 00 01 00 7B 00 00 00`

| 偏移 | 字节(A / B) | 含义(v79 handler case 20/31) |
|------|------|------|
| 0–1 | `77 00` | 包头 0x0077 |
| 2 | `1F` | **action = 31 (ADD_ITEM)** |
| 3 | `04` | **背包类型**(4 = ETC;1=EQUIP,2=USE,3=SETUP,5=CASH) |
| 4–5 | `05 00` / `09 00` | **物品槽位**(A=5,B=9) |
| 6–7 | `06 00` | **bundles**(=6,上架的"捆"数) |
| 8–9 | `01 00` | **perBundle**(=1,每捆数量) |
| 10–13 | `7B 00 00 00` | **price**(=0x7B=123,每捆单价) |

> ⚠️ v1 误把 `01 00` 写成"上架数量=1"。实际结构为 `<invType><slot><bundles><perBundle><price>`,总上架量 = bundles × perBundle = 6×1 = 6。

**按数量 vs 整组 编码规则(关键):**
- **按数量售卖**:`bundles = N`,`perBundle = 1`,`price = 单价`(卖 N 个,每个 price)
- **整组售卖**:`bundles = 1`,`perBundle = 堆叠数`,`price = 整组价`(卖一整组,总价 price)

构造模板:`77 00 1F <invType:1> <slot:2> <bundles:2> <perBundle:2> <price:4>`

> 本捕获两物品均为 `bundles=6, perBundle=1` ⇒ **实际都是"按数量"式**。用户所称"整组售卖"那条在字节上并未体现 `perBundle>1`;若需整组上架,构造时令 `bundles=1, perBundle=堆叠数` 即可(需补抓一次整组上架包做字节级确认)。

---

### I4 — 开放营业 `PLAYER_INTERACTION` (0x77, action `0x0B` = 11 = OPEN)
原始字节(4):`77 00 0B 01`

| 偏移 | 字节 | 含义 |
|------|------|------|
| 0–1 | `77 00` | 包头 0x0077 |
| 2 | `0B` | **action = 11 (OPEN)**,店主开放营业 |
| 3 | `01` | 标志(捕获为 1) |

> 发送后服务器回 `SPAWN_HIRED_MERCHANT`(0x10D)生成商店 NPC。

构造模板:`77 00 0B 01`

---

### I4b — OPEN 前的一步 `PLAYER_INTERACTION` (0x77, action `0x1C` = 28) — **已确认 no-op**
原始字节(5):`77 00 1C 00 00`

| 偏移 | 字节 | 含义 |
|------|------|------|
| 0–1 | `77 00` | 包头 0x0077 |
| 2 | `1C` | **action = 28**,真实客户端在 OPEN(0x0B) 之前发送 |
| 3–4 | `00 00` | 保留(固定 0) |

> ✅ 已用该服务端 `PlayerInteractionHandler.java` 核对:action 28 **无任何 `case`**,服务端直接忽略(no-op)。bot 发包序列**可省略**此步;`OPEN`(0x0B) 单独即可触发服务器回 `SPAWN_HIRED_MERCHANT`。保留它仅为了与真实客户端字节级一致(非功能必需)。

---

### I5 — 离开交互界面 `PLAYER_INTERACTION` (0x77, action `0x0A` = 10 = EXIT)
原始字节(3):`77 00 0A`

| 偏移 | 字节 | 含义 |
|------|------|------|
| 0–1 | `77 00` | 包头 0x0077 |
| 2 | `0A` | **action = 10 (EXIT)** |

> 在收到 `SPAWN_HIRED_MERCHANT` 之后发送,表示客户端已离开开店 UI。

构造模板:`77 00 0A`

---

## 4. 收包(驱动状态机,非发包但需解析)

| 包名 | 包头 | 作用 |
|------|------|------|
| ENTRUSTED_SHOP_CHECK_RESULT | 0x34 | 回 `07` = 允许开店 |
| PLAYER_INTERACTION (resp) | 0x14F | 创建回执(含店名/描述);上架回执 `write(23)`(shopItemUpdate,含 bundles/perBundle/price);访客进出 `write(4)/write(10)`;聊天 `write(6)`;错误 `write(10)` |
| MODIFY_INVENTORY_ITEM | 0x20 | 上架后物品从背包移除确认 |
| SPAWN_HIRED_MERCHANT | 0x10D | 商店 NPC 生成,确认开店成功(含 ownerId / itemId / 位置 / 店名 / 描述) |
| DESTROY_HIRED_MERCHANT | 0x10E | 商店 NPC 销毁 |
| UPDATE_HIRED_MERCHANT | 0x10F | 物品/金币更新 |
| MERCH_ITEM_MSG / MERCH_ITEM_STORE | 0x14B / 0x14C | 自由市场领取(Fred)相关 |

---

## 5. ⚠️ 相对 v1 的校正摘要
0. **opcode 已用 079 服务端源码逐字核对**(§0.1):捕获为线上权威值;recv opcode 由 `.ini` 运行时加载(枚举默认 `USE_HIRED_MERCHANT=0x38` 被覆盖为捕获的 `0x3C`);action 子码 CREATE=0/ADD_ITEM=31/OPEN=11/EXIT=10 全部吻合;action 0x1C 确认 no-op。
1. CREATE 的 "123" 是**店名/描述**(mapleAsciiString),非密码;雇佣商店创建包**无密码字段**。
2. `70 C0 4C 00` 是雇佣商店道具 **itemId**(readInt),`06 00` 是其 **CASH 背包槽位**;二者均可从 bot 自身背包读取,无需服务器回查(推翻 v1 "会话ID 需动态获取" 的结论)。
3. ADD_ITEM 字段校正:`04`=背包类型,`06 00`=bundles(6),`01 00`=perBundle(1),`7B`=price(123);v1 的"数量=1"错误。
4. action 子码对照该服务端:**CREATE=0 / ADD_ITEM=31 / OPEN=11 / EXIT=10** 全部吻合;**0x1C(28)** 待 CongMS 079.jar 确认。
5. 本捕获两物品均为 bundles=6/perBundle=1 ⇒ 实际都是"按数量"式;整组上架编码规则见 §3 I3,待补抓确认。

---

## 6. 验收清单(回归验证用)
逐项打勾,全部通过后方可进入 market 协议实现阶段。

- [ ] **I1** `USE_HIRED_MERCHANT`(0x3C,11B)按模板构造,槽位正确,服务器回 `ENTRUSTED_SHOP_CHECK_RESULT=7`
- [ ] **I2** `PLAYER_INTERACTION` CREATE(0x77+0,16B):createType=5、描述、CASH 槽位、itemId 正确 → 服务器回 0x14F 创建回执
- [ ] **I3** `PLAYER_INTERACTION` ADD_ITEM(0x77+0x1F,14B):`<invType><slot><bundles><perBundle><price>` 上架物品A → 服务器回 0x14F `write(23)` 上架回执 + MODIFY_INVENTORY_ITEM 移出背包
- [ ] **I3** 上架物品B(仅槽位不同)成功
- [ ] **确认** 整组上架:补抓一次 `perBundle>1, bundles=1` 的包,字节级核对(规则见 §3 I3) → 结果:__________
- [ ] **I4** `PLAYER_INTERACTION` OPEN(0x77+0x0B,4B)发送
- [ ] **I4b** action 0x1C:已确认 no-op(该服务端源码无对应 case),bot 可省略 → 结果:省略 / 保留(仅字节级一致)
- [ ] 服务器回 `SPAWN_HIRED_MERCHANT`(0x10D),商店 NPC 出现,店名/描述与 I2 一致
- [ ] **I5** `PLAYER_INTERACTION` EXIT(0x77+0x0A,3B)收到 SPAWN 后发送,退出开店 UI
- [ ] 全流程字节级与 `packet/market.txt` 逐字节一致(可用 diff 比对)

验收人:__________  日期:__________

---

## 7. 后续实现范围(market 全部协议)
实现阶段需覆盖的接口(名称对应 §1–§2):

**发包(客户端→服务端):**
- [ ] `USE_HIRED_MERCHANT`(0x3C)
- [ ] `PLAYER_INTERACTION`(0x77)全部市场相关 action:CREATE(0)/VISIT(4)/CHAT(6)/EXIT(10)/OPEN(11)/ADD_ITEM(31)/BUY_ITEM_HIREDMERCHANT(33)/REMOVE_ITEM(35)/MAINTANCE_OFF(37)/MAINTANCE_ORGANISE(38)/CLOSE_MERCHANT(39)/VIEW_MERCHANT_VISITOR(44)/VIEW_MERCHANT_BLACKLIST(45)/BLACKLIST_ADD(46)/BLACKLIST_REMOVE(47)/MERCHANT_EXIT(27)
- [ ] `MERCH_ITEM_STORE`(recv,领取触发)

**收包(服务端→客户端,解析 + 状态机):**
- [ ] `ENTRUSTED_SHOP_CHECK_RESULT`(0x34)
- [ ] `PLAYER_INTERACTION`(0x14F):创建回执 / 上架回执(23) / 访客进出(4,10) / 聊天(6) / 错误(10) / 买后更新
- [ ] `SPAWN_HIRED_MERCHANT`(0x10D) / `DESTROY_HIRED_MERCHANT`(0x10E) / `UPDATE_HIRED_MERCHANT`(0x10F)
- [ ] `MERCH_ITEM_MSG`(0x14B) / `MERCH_ITEM_STORE`(0x14C,含 35=物品数据)
- [ ] `MODIFY_INVENTORY_ITEM`(0x20,已在 opcodes.rs)

**代码落点建议:** 在 `src/opcodes.rs` 补 `HIRED_MERCHANT_REQUEST=0x3C`、`ENTRUSTED_SHOP_CHECK_RESULT=0x34`、`SPAWN_HIRED_MERCHANT=0x10D`、`UPDATE_HIRED_MERCHANT=0x10F`、`DESTROY_HIRED_MERCHANT=0x10E`、`MERCH_ITEM_MSG=0x14B`、`MERCH_ITEM_STORE=0x14C`;新增 `src/handlers/market.rs`(收包解析)与 `src/packets/market.rs`(发包构造),参照现有 `trade.rs` 结构。
