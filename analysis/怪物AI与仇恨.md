# 怪物AI与仇恨系统

涵盖两个方向：
- **避免仇恨**：让怪物不攻击玩家
- **获取仇恨**：让自己始终成为怪物目标

---

## 1. 架构分析

CMS 79 中怪物行为是**服务器权威**的。服务器控制：
- 怪物何时攻击、攻击谁
- 造成多少伤害
- 仇恨值归属

客户端主要职责是渲染服务器下发的状态。**纯客户端修改无法阻止服务器决定攻击**，只能尝试绕过客户端上的伤害应用逻辑。

```
服务端控制仇恨 → 客户端无法强制获取仇恨
       ↓                    ↓
服务端决定谁打了怪物、打多少、仇恨值归属
客户端只能报告"我打了谁、打了多少"
```

---

## 2. 控制器机制

每个怪物由一名玩家客户端担任控制器。控制器负责：
- 发送怪物位置更新（opcode 186）
- 触发怪物AI决策（通过 opcode 186/187/189 包）
- 服务端**信任控制器**发送的包

**核心思路**：让客户端成为地图上所有怪物的控制器，然后利用 AI 更新循环自动发包。

### 关键函数

| 函数 | 地址 | 作用 |
|------|------|------|
| `CMob__OnMobAttacked` | `0x695DEE` | 怪物受击总入口(vtable[1])，周期性调用 |
| `CMob__IsController` | `0x691EAE` | 检查当前客户端是否为该怪物的控制器 |
| `CMob__RequestControl` | `0x691D78` | 向服务端请求怪物控制权 |
| `CMob::OnCtrlAck` | `0x69A85D` | 服务端确认控制权分配 |
| `CMob__ProcessAttackEffects` | `0x694BF6` | 攻击特效处理(目标检查入口) |
| `CMob__UpdateAIMovement` | `0x69D07B` | 更新AI移动路径 |
| `CMob__OnMobAttackMob` | `0x69F675` | 怪物攻击怪物(发 opcode 189) |
| `sub_66EDBD` | `0x66EDBD` | 怪物移动路径检测(186发送的条件) |

---

## 3. 数据流全景

### 攻击流程

```
攻击流程（客户端→服务端→所有客户端）：
 客户端(你)                         服务端                      其他客户端
    │                                 │                            │
    │  opcode 0x2C (攻击包)           │                            │
    │  ├─ timestamp(4B)               │                            │
    │  ├─ target_mob_id(4B)           │                            │
    │  ├─ damage(4B)                  │                            │
    │  ├─ 技能ID/位置等                │                            │
    │  └─ ...                         │                            │
    │ ──────────────────────────►     │                            │
    │                                 ├─ 验证攻击合法性            │
    │                                 ├─ 计算实际伤害              │
    │                                 ├─ 计算仇恨 (服务器端)       │
    │                                 ├─ 确定怪物下一个目标         │
    │                                 │                            │
    │                                 │  opcode 241 (怪物移动包)   │
    │                                 │  ├─ move_type(1B)          │
    │                                 │  │   └─ >>1 = 12-20 攻击   │
    │                                 │  ├─ 移动路径数据            │
    │                                 │  ├─ 攻击目标ID              │
    │                                 │  └─ ...                    │
    │                                 │ ──────────────────────► 所有客户端
    │                                 │                            │
    │                                 │  opcode 248 (伤害包)       │
    │                                 │  ├─ damage(4B)             │
    │                                 │  ├─ HP/MAX(各4B)           │
    │                                 │  └─ ...                    │
    │                                 │ ──────────────────────► 所有客户端
```

### 客户端接收处理流程

```
CMobPool::OnPacket (opcode 241)
  → CMobPool::OnMobPacket
    → CMob::OnMove
      ├─ 解码 move_type, 提取移动/攻击标记
      ├─ mob+0xA8 = move_type & 1  (攻击标记位，NOT 目标ID)
      ├─ move_type>>1 分类:
      │   ├─ 12-20: 攻击移动 → CMob__ProcessAttackMove (路径/特效)
      │   ├─ 6-8:   特殊移动 → CMob__HandleAttackMove
      │   └─ 21-37: 特殊效果 → 查找目标实体ID + 音效播放
      └─ CMovePath::OnMovePacket
```

### AI 更新循环

```
CMob__OnMobAttacked (0x695DEE, 每帧/定时调用)
  ├─ CMob__UpdateTimers      — 更新定时器
  ├─ CMob__UpdateAIState     — AI状态机
  ├─ CMob__UpdateAIAction    — AI动作决策
  ├─ CMob__UpdateAIMovement  — AI移动更新
  ├─ CMob__ProcessAttackEffects (0x694BF6)
  │   ├─ 目标存在性检查 (mob+0x294, field+0x1A0, mob+0xA8)
  │   ├─ 攻击信息列表遍历
  │   │   ├─ 玩家碰撞检测 → CUserLocal::SetDamaged
  │   │   └─ 怪物碰撞检测 → CMob__OnMobAttackMob (发 opcode 189)
  │   └─ ...COM效果处理...
  ├─ CMob__ProcessBuffEffects
  ├─ CMob__CheckMobDead → CMob__ProcessAfterAttackAI
  └─ CMob__ShowHPIndicator / CMob__CreateHPIndicator
```

### 玩家攻击发包流程

```
CUserLocal::SetDamaged (0x96435E):
  玩家攻击怪物
    → CUserLocal__DoActiveSkill (技能激活调度)
    → 计算伤害、暴击、技能效果等
    → COutPacket (opcode 0x2C = 44)
      ├─ update_time(4B)
      ├─ 攻击类型分支:
      │   ├─ 无目标: encode 0xFE + 0x00 + mob_id + damage
      │   └─ 有目标: encode skill_type + target_mob_id + 位置 + 伤害 + ...
      └─ CClientSocket::SendPacket
```

### 控制器发包流程

```
CMob__OnMobAttacked 发包段落 (0x6964C0)
  ├─ opcode 186 (0xBA) → 怪物位置更新
  │   ├─ mob_id(4B)
  │   └─ 移动状态量(4B)
  │   条件: 时间间隔≥1000ms + 路径检测通过 + 移动计数>0
  │
  ├─ opcode 187 (0xBB) → 怪物碰撞通知
  │   ├─ 源怪物ID(4B)
  │   ├─ 玩家ID(4B)
  │   └─ 目标怪物ID(4B)
  │
  └─ opcode 189 (0xBD) → 怪物攻击报告
      ├─ source_mob_id(4B)
      ├─ player_id(4B)
      ├─ attack_index(1B)
      ├─ damage(4B)
      ├─ direction(1B)
      ├─ pos_x(2B)
      └─ pos_y(2B)
```

---

## 4. AI决策流程

### AI函数 sub_695DEE (0x695DEE) 关键控制流

```
sub_695DEE (ecx = CMob*):
  │
  ├── 0x695E03: 获取当前时间
  ├── 0x695E16: 遍历攻击信息模板 (mob+0xE4链表)
  │     └── CMobTemplate::GetAttackInfo
  │
  ├── 0x695E68: 处理伤害显示
  │
  ├── 0x695FEA: 处理COM/效果对象
  │
  ├── 0x6960E9: IsMobOurTeam 检查（友方→跳过寻敌）
  │
  ├── 0x696140: CRC移动状态检查
  │
  ├── 0x6961B4: AI暂停状态检查 (mob+0x408)
  │
  ├── 0x696247: sub_69F666 → 当前怪物活跃性检查
  │
  ├── 0x696256: 碰撞箱 + 时间阈值检查 (2000ms)
  │
  ├── 0x696317: sub_69F675 攻击执行
  │
  ├── 0x69638F: **技能动作选择 sub_69D1E5** ← 关键决策点
  │     ├─ 返回非0 → 执行技能 (sub_699C57)
  │     └─ 返回0 → 执行普通攻击 (sub_698BD6 type=3)
  │
  ├── 0x6964AD: 移动路径/捎带显示
  │
  ├── 0x696608: **目标选择与攻击** ← 关键决策点
  │     ├─ 读取 CMobPool+0x5C 目标
  │     ├─ sub_69F5CF 验证目标
  │     ├─ 碰撞箱重叠检测
  │     └─ 发送opcode 0xBB攻击包
  │
  └── 0x696C85: sub_694BF6 子AI处理
```

---

## 5. 避免仇恨方案

### 方案一：攻击目标检查绕过（❌ 实测无效）

**原理：** 在 `CMob__ProcessAttackEffects` 中绕过目标检查链，让客户端认为怪物"无目标"。

```
检查链: mob+0x294 → field+0x1A0 → mob+0xA8
```

```asm
694e09:  cmp     [esi+294h], ebx     ; mob+0x294 = field data中的目标
694e0f:  jnz     short loc_694E1F    ; 有→跳
694e11:  mov     eax, [esi+178h]    ; field data指针
694e17:  cmp     [eax+1A0h], ebx    ; field+0x1A0 = 攻击标记
694e1d:  jnz     short loc_694E2B   ; [修改] → NOP
694e1f:  cmp     [esi+0A8h], ebx    ; mob+0xA8 = 攻击位(非目标ID)
694e25:  jnz     short loc_694E2B   ; [修改] → NOP
```

**修改：**
| 地址 | 原始字节 | 修改为 |
|------|---------|--------|
| `0x694E1D` | `75 0C` | `90 90` |
| `0x694E25` | `75 04` | `90 90` |

**结果：无效**。服务端独立计算怪物伤害，客户端的"无目标"判断不影响服务端下发的伤害包。

### 方案二：伤害条目清除（❌ 实测无效）

**原理：** 在 `sub_6931F2` 中清除攻击动作条目的目标ID。

```asm
69339a:  call    CMob__IsMobOurTeam
69339f:  test    eax, eax
6933a1:  jz      short loc_6933BA     ; [修改] → NOP
```

**结果：无效**。该代码处理的是怪物内部动作条目的COM同步，不影响服务端仇恨计算。

### 方案三：让怪物找不到攻击技能

**原理：** 让 `CMobTemplate::GetAttackInfo` 始终返回 NULL。

**函数位置：** `0x534B2D`

**修改点：** `0x534B4A` 处 `jge` → `jmp`（无条件跳转到 return 0）

```asm
534b44:  cmp     edi, [esi+224h]      ; 比较索引和技能总数
534b4a:  7D 46   jge     short loc_534B92  ; [修改] 7D→EB (jge→jmp)
                                           ; 现在无条件跳转到 return 0
```

| 地址 | 原始字节 | 原始指令 | 修改为 |
|------|---------|---------|--------|
| `0x534B4A` | `7D 46` | `jge short loc_534B92` | `EB 46` |

**关键发现：** 即使 `GetAttackInfo` 返回 NULL，怪物**仍然会发送攻击包**（opcode 189）到服务端。服务端独立处理伤害计算。

### 方案四：双管齐下（综合方案）

结合方案一和方案三：

| 地址 | 原始字节 | 修改为 | 效果 |
|------|---------|--------|------|
| `0x694E1D` | `75 0C` | `90 90` | 跳过 field_data 目标检查 |
| `0x694E25` | `75 04` | `90 90` | 跳过 target_id 目标检查 |
| `0x534B4A` | `7D 46` | `EB 46` | GetAttackInfo 永远返回 NULL |

```
真正的完整方案：
1. ✓ 绕过目标检查（方案一） → 阻止客户端攻击处理
2. ✓ 让技能检索失败（方案三） → 阻止客户端攻击参数获取
3. ❌ 阻止服务端伤害包  → 客户端无法实现，需改服务端
```

---

## 6. 获取仇恨方案

### 核心矛盾

```
服务端控制仇恨 → 客户端无法强制获取仇恨
       ↓                    ↓
服务端决定谁打了怪物、打多少、仇恨值归属
客户端只能报告"我打了谁、打了多少"
```

**结论**：纯客户端无法强制服务端把仇恨给你。但可以通过以下方式**影响**或**伪装**。

### 方案A：控制器劫持法（最有望实现）

**原理：** 成为怪物控制器后，利用控制器身份发送虚假攻击包。

`CMob::OnCtrlAck` 逻辑：
```c
if ( !CMob__IsController(this) )
    CMob__RequestControl(1);  // 请求控制权
*(this+98) = decoded_short;
*(this+81) = ctrl_type;
*(this+82) = ctrl_subtype;
```

**关键修改点：**

| 地址 | 原始 | 修改为 | 说明 |
|------|------|--------|------|
| `0x69A863` | `call CMob__IsController` | `90 90 90 90 90` | 绕过控制权检查，强制认为自己有控制权 |

**实现思路：**
1. 始终请求控制权：在 `CMob::OnCtrlAck` 中，无论是否已有控制器，都强制请求
2. 利用控制器身份发送虚假攻击包：通过 `CMob__SendMobAttackPacket` 发送 `opcode 189`

**可行性评估：**
| 方面 | 评价 |
|------|------|
| 难度 | ⭐⭐⭐ 中等 |
| 效果 | ⭐⭐⭐ 可能影响服务端，但不确定 |
| 风险 | ⭐⭐ 发包可能被服务端校验 |

### 方案B：攻击包伤害放大法

**原理：** 修改发送给服务端的攻击包中的伤害值，可能间接影响仇恨计算。

攻击包（opcode 0x2C）在 `CUserLocal::SetDamaged` 中构建，位于 `0x965591`：

```asm
96558c: push 2Ch              ; opcode 0x2C
965591: call COutPacket__COutPacket_2
```

**可行性评估：**
| 方面 | 评价 |
|------|------|
| 难度 | ⭐⭐ 较容易 |
| 效果 | ⭐ 服务端可能校验伤害，上限受技能限制 |
| 风险 | ⭐⭐ 发包有频率限制 |

### 方案C：MoveType 目标覆盖法

**原理：** 强制 `mob+0xA8` 永远为 1（始终有攻击标记），影响客户端本地攻击显示。

| 地址 | 原始 | 修改为 | 说明 |
|------|------|--------|------|
| `0x69A7C5` | `mov [esi+0A8h], eax` | `mov byte ptr [esi+0A8h], 1` | 强制攻击标记=1 |

同时绕过目标检查（反向——**强制有目标**）：

| 地址 | 原始 | 修改为 | 说明 |
|------|------|--------|------|
| `0x694E09` | `cmp [esi+294h], ebx` | `mov [esi+294h], 1` | 强制有field目标 |
| `0x694E1D` | `75 0C` | `90 90` | 跳过 field 标记检查 |
| `0x694E25` | `75 04` | `90 90` | 跳过 mob 标记检查 |

**可行性评估：**
| 方面 | 评价 |
|------|------|
| 难度 | ⭐ 最简单 |
| 效果 | ⭐⭐ 客户端上怪物会显示攻击状态 |
| 风险 | ⭐ 纯本地修改，无封号风险 |

### 方案D：组合方案（推荐）

结合方案A（控制器）和方案B（伤害放大）：

```
1. 劫持怪物控制器 (修改 CMob::OnCtrlAck / CMob__IsController)
2. 作为控制器，发送 opcode 189 报告对怪物造成巨额伤害
3. 修改 CUserLocal::SetDamaged 放大攻击包伤害
4. 本地强制 mob+0xA8 = 1 保证客户端显示正确
```

---

## 7. 控制器劫持 + 强制发包方案（推荐）

### 原理

`CMob__OnMobAttacked` 是每个怪物周期性调用的 AI 更新函数。其中有一段逻辑会发送 **opcode 186**（怪物位置更新包）到服务端。

```asm
; CMob__OnMobAttacked 发包段落 (0x6964C0)
6964c9:  cmp     eax, 3E8h        ; 距上次发送 >= 1000ms?
6964d4:  jl      loc_696608       ; [修改] → NOP × 6

6964f3:  call    sub_4376F2       ; 当前时间
6964fa:  call    sub_66EDBD       ; 路径检测
696501:  jz      loc_696608       ; [修改] → NOP × 2

696507:  cmp     [ebp-4Ch], edi   ; 移动计数 > 0?
69650a:  jle     loc_696608       ; [修改] → NOP × 2

; --- 通过所有检查后发送 opcode 186 ---
696510:  push    0BAh             ; opcode 186
696515:  lea     ecx, [ebp-44h]
696518:  call    COutPacket__COutPacket_2
```

### 条件检查详解

| 条件 | 地址 | 原始逻辑 | 修改方式 |
|------|------|---------|---------|
| 时间间隔 | `0x6964C9-0x6964D4` | 距上次发送需 >= 1000ms | NOP 掉 jl 指令 |
| 路径检测 | `0x696501` | `sub_66EDBD` 需返回 true | NOP 掉 jz 指令 |
| 移动计数 | `0x69650A` | `[ebp-4Ch]` > 0 | NOP 掉 jle 指令 |

### 控制器劫持

```asm
; CMob__IsController (0x691EAE)
; 原始: 各种复杂检查后返回真假
; 修改后:
691eae:  b0 01          mov     al, 1
691eb0:  c3             retn
```

### 所有子方案对比

| 方案 | 修改点 | 难度 | 效果预期 | 风险 |
|------|--------|------|---------|------|
| **方案一** 控制器+强制发包 | `CMob__IsController` + 3处NOP | ⭐ 低 | ⭐⭐⭐⭐ 怪物主动持续发位置包 | ⭐ 可能触发检测 |
| **方案二** 仅控制器 | `CMob__IsController` 仅1处 | ⭐ 极低 | ⭐⭐ 依赖怪物自有AI | 极低 |
| **方案三** 碰撞广播 | 触发 opcode 187 条件 | ⭐⭐ 中 | ⭐⭐⭐ 让服务端处理碰触 | ⭐ 中等 |
| **方案四** 攻击报告 | 触发 opcode 189 | ⭐⭐⭐ 高 | ⭐⭐⭐⭐ 最接近真实攻击 | ⭐⭐ 发包多 |

**建议测试顺序**：方案二 → 方案一 → 方案三

---

## 8. AI行为修改：保持仇恨与禁用魔法技能

### 1. 让怪物一直有玩家的仇恨

#### 原理

怪物仇恨（aggro）通过**控制权机制**管理：

```
服务端 → OnMobChangeController(packet)
  ├─ v3 = Decode1(): 0=释放控制, 1=获得控制
  ├─ v4 = Decode4(): 怪物ID
  ├─ v3==0 → sub_6A5C82 (释放控制: 清除目标、重置状态)
  └─ v3!=0 → sub_6A5B97 (获得控制: 创建/初始化怪物、设置CRC)
```

#### Hook方案

##### 方案A：阻止失去控制权（推荐）

```
地址: 0x6A6F4C (CMobPool::OnMobChangeController 内)
指令: call sub_6A5C82     ; 释放怪物控制权

修改前: E8 31 ED FF FF
修改后: 90 90 90 90 90     (NOP)
```

效果：玩家一旦获得怪物的控制权就永远不会失去，所有被控制的怪物都会持续以玩家为目标。

##### 方案B：强制通过目标验证

```
地址: 0x696636 (sub_695DEE AI主函数内)
指令: jz loc_69681A        ; 目标验证失败则跳过攻击

修改前: 0F 84 DE 01 00 00
 修改后: 90 90 90 90 90 90  (NOP)
```

效果：无论目标验证是否通过，都继续执行攻击逻辑。

##### 方案C：组合方案（最强效果）

两个hook同时使用，既防止失去控制权，又强制通过目标验证。

---

### 2. 不让怪物使用魔法技能

#### WZ模板层的 "onlyNormalAttack" 标志

```
sub_6AA857 (CMobTemplate::GetMobTemplate 的WZ读取函数):

0x6AC26C: push "onlyNormalAttack"
0x6AC2B3: call ZtlSecureTear_int_
0x6AC2B8: mov [edi+2A0h], eax    ; 模板+0x2A0 = onlyNormalAttack (CRC保护)
```

#### Hook方案

##### 方案A：NOP跳转指令（推荐）

```
地址: 0x696396 (sub_695DEE AI主函数内)
指令: jnz loc_6963BC        ; sub_69D1E5返回非0 → 执行技能

修改前: 0F 85 22 00 00 00
修改后: 90 90 90 90 90 90  (NOP)
```

修改后流程：
```
sub_69D1E5 → 返回1(有技能) → jnz被NOP → 继续执行到0x696398
  → 检查普通攻击能力 [eax+240h]
  → 有普攻 → sub_698BD6(type=3) 执行普通攻击
  → 无普攻 → jz 0x6963D9 跳过（不发技能）
```

##### 方案B：强制sub_69D1E5返回0

```
地址: 0x69D1E5 函数开头
修改前: 原始函数代码
修改后: 33 C0    (xor eax, eax)
         C3      (ret)
```

效果：sub_69D1E5始终返回0，AI永远不会执行技能。

---

### 关键数据结构总览

| 地址/偏移 | 类型 | 含义 |
|-----------|------|------|
| `dword_BDD49C` | CMobPool* | 怪物池全局指针 |
| `[CMobPool*+0x5C]` | CMob* | 当前交互目标/玩家控制的实体 |
| `[CMobPool*+0x60]` | void* | 相关对象引用 |
| `[CMobPool*+0x68]` | CMob* | 当前活跃处理中的怪物 |
| `[CMobPool*+0x6C]` | DWORD | 上次攻击检测时间戳 |
| `mob+0x294` | int | 技能禁用标志（非0=不执行技能） |
| `mob+0x24C` | int | 状态禁用标志 |
| `mob+0x224` | int | 状态禁用标志 |
| `mob+0x230` | int | 状态禁用标志 |
| `mob+0x2A0` | int | 状态禁用标志 |
| `mob+0x144` | int | 当前待执行技能模板ID |
| `mob+0x36C` | int | 技能锁（非0禁止使用技能） |
| `mob+0xA4` | DWORD | 技能冷却到期时间 |
| `mob+0xA8` | byte | 攻击标记（move_type & 1） |
| `mob+0x324` | byte | 动作标志1 |
| `mob+0x328` | byte | 动作标志2 |
| `mob+0x16C` | int | AI状态标志 |
| `CMobTemplate+0x2A0` | int | "onlyNormalAttack" WZ属性（CRC保护） |
| `CMobTemplate+0x240` | int | 普通攻击能力标志 |

### Hook地址速查表（AI/仇恨）

| 功能 | 地址 | 原始指令 | 修改为 | 长度 |
|------|------|---------|--------|------|
| 防止失去控制权 | 0x6A6F4C | `E8 31 ED FF FF` | `90 90 90 90 90` | 5字节 |
| 强制通过目标验证 | 0x696636 | `0F 84 DE 01 00 00` | `90 90 90 90 90 90` | 6字节 |
| 禁用魔法技能 | 0x696396 | `0F 85 22 00 00 00` | `90 90 90 90 90 90` | 6字节 |
| 强制有攻击标记 | 0x69A7C5 | `mov [esi+0A8h], eax` | `mov byte ptr [esi+0A8h], 1` | - |
| 控制器劫持 | 0x691EAE | 函数入口 | `mov al,1; ret` | 2字节 |
| 绕过目标检查1 | 0x694E1D | `75 0C` | `90 90` | 2字节 |
| 绕过目标检查2 | 0x694E25 | `75 04` | `90 90` | 2字节 |
| 禁用GetAttackInfo | 0x534B4A | `7D 46` | `EB 46` | 2字节 |

### 崩溃风险评估

| 风险场景 | 概率 | 后果 | 说明 |
|---------|------|------|------|
| NOP控制权释放导致怪物重复 | 低 | 无严重问题 | 怪物可能重复收到控制权包 |
| 强制目标验证通过 | 低 | 攻击空气 | 无有效目标时可能会攻击空气位置 |
| 禁用技能后怪物无普攻 | 低 | 怪物发呆 | 只使用技能的怪物会停止攻击 |
| 模板层冲突 | 极低 | 无影响 | AI层hook不影响模板加载 |

---

## 9. 关键发现总结

### mob+0xA8 实际含义（重要修正）

| 条目 | 原理解 | 实际分析结果 |
|------|--------|-------------|
| mob+0xA8 | 目标玩家ID | **move_type & 1 攻击标记位**（0或1） |
| 设置位置 | 服务端直接下发 | `CMob::OnMove` 0x69A7C5 |
| 检查位置 | 目标ID比对 | `CMob__ProcessAttackEffects` 0x694E1F-0x694E25 |
| 实际作用 | 标识怪物是否有目标 | 标识怪物是否在攻击状态 |

### 怪物攻击检测链

```
[服务端] 计算仇恨 → 发送 opcode 241 (move_type + 攻击标记)
  → [客户端] CMob::OnMove
    → mob+0xA8 = move_type & 1 (攻击标记)
    → CMob__ProcessAttackEffects 检查:
        mob+0x294 (field目标)  OR  field+0x1A0 (攻击标记)  OR  mob+0xA8 (本地标记)
        → 任一非0 → "有目标"
```

### 已知怪物对象字段（仇恨相关）

| 偏移 | 名称 | 说明 |
|------|------|------|
| mob+0xA4 | attack_end_time | 攻击结束时间 |
| mob+0xA8 | attacking_flag | 攻击标记 (move_type & 1) |
| mob+0xD0 | attack_list | 攻击信息列表 |
| mob+0x178 | pFieldData | 场数据指针 |
| mob+0x294 | field_target | 场数据中的目标 |
| mob+0x376 | pTemplate | CMobTemplate 指针 |
| mob+0x458 | effect_list_head | 特效链表头 |

### 所有方案对比

| 需求 | 可行性 | 实现方式 | 备注 |
|------|--------|---------|------|
| 怪物不攻击你 | ❌ 客户端无法实现 | 需改服务端 mob AI | 服务端独立计算 |
| 始终获取仇恨 | ❌ 客户端无法强制 | 需改服务端仇恨计算 | 可尝试影响 |
| 劫持控制器 | ✅ 客户端可修改 | 改 `CMob__IsController` | 影响怪物AI决策 |
| 本地攻击标记欺骗 | ✅ 客户端可修改 | 改 `mob+0xA8` 设置 | 仅影响客户端显示 |
| 放大攻击包伤害 | ⚠️ 可能有校验 | 改 `CUserLocal::SetDamaged` | 服务端可能验证 |
| 发送虚假怪物攻击 | ⚠️ 需控制器身份 | 改 `CMob__OnMobAttackMob` | 发包 opcode 189 |

**最终结论**：CMS 79 的怪物仇恨完全由服务端计算，纯客户端修改**无法强制**获取仇恨。最实用的客户端方案是**控制器劫持 + 伤害放大**的组合，通过影响服务端的决策来间接争取仇恨。但这无法保证100%获取仇恨——当有其他玩家同时攻击时，服务端仍可能选择其他目标。

---

## 10. IDA 函数重命名对照表

| 原始名称 | 新名称 | 地址 | 说明 |
|---------|--------|------|------|
| `sub_695DEE` | `CMob__OnMobAttacked` | `0x695DEE` | 怪物受击处理总入口(vtable[0]) |
| `sub_694BF6` | `CMob__ProcessAttackEffects` | `0x694BF6` | 攻击特效综合处理 + 目标检查 |
| `sub_69C432` | `CMob__ProcessAttackMove` | `0x69C432` | 攻击路径/特效处理 |
| `sub_69F675` | `CMob__OnMobAttackMob` | `0x69F675` | 怪物攻击怪物(opcode 189) |
| `sub_770668` | `MobPool__FindEntityByID` | `0x770668` | 按ID查找实体(玩家/怪物) |
| `sub_6931F2` | `CMob__ClearAttackActionEntries` | `0x6931F2` | 清除攻击动作条目 |
| `sub_691EAE` | `CMob__IsController` | `0x691EAE` | 检查是否控制此怪物 |
| `sub_691D78` | `CMob__RequestControl` | `0x691D78` | 请求怪物控制权 |
| `sub_69F666` | `CMob__CanProcessAttack` | `0x69F666` | 检查能否处理攻击 |
| `sub_69F5CF` | `CMob__HasTargetInRange` | `0x69F5CF` | 检查目标是否在范围内 |
| `sub_69F44E` | `CMob__UpdateAttackState` | `0x69F44E` | 更新攻击状态 |
| `sub_699ACC` | `CMob__SendMobAttackPacket` | `0x699ACC` | 发送怪物攻击包 |
| `sub_69716A` | `CMob__GetAttackState` | `0x69716A` | 获取怪物攻击状态 |
| `sub_69E42C` | `CMob__DecodeEnterFieldData` | `0x69E42C` | 解码入场数据 |
| `sub_69CBB8` | `CMob__UpdateTimers` | `0x69CBB8` | 更新计时器 |
| `sub_69CD10` | `CMob__UpdateAIState` | `0x69CD10` | 更新AI状态 |
| `sub_69CD72` | `CMob__UpdateAIAction` | `0x69CD72` | 更新AI动作 |
| `sub_69D07B` | `CMob__UpdateAIMovement` | `0x69D07B` | 更新AI移动 |
| `sub_69D1E5` | `CMob__CheckAttackCondition` | `0x69D1E5` | 检查攻击条件 |
| `sub_69D41E` | `CMob__ProcessAIAttackSelect` | `0x69D41E` | AI选择攻击类型 |
| `sub_698BD6` | `CMob__CheckMobAttackRange` | `0x698BD6` | 检查怪物攻击范围 |
| `sub_692E65` | `CMob__ApplyDamageToMob` | `0x692E65` | 应用伤害到怪物 |
| `sub_693683` | `CMob__HandleAttackMove` | `0x693683` | 处理攻击移动类型 |
| `sub_695DAF` | `CMob__IsMobDead` | `0x695DAF` | 检查怪物是否死亡 |
| `sub_69F1F0` | `CMob__ProcessAfterAttackAI` | `0x69F1F0` | 攻击后AI处理 |
| `sub_69F5A5` | `CMob__CheckMobDead` | `0x69F5A5` | 检查怪物死亡状态 |
| `sub_69E6AE` | `CMob__ProcessBuffEffects` | `0x69E6AE` | 处理Buff效果 |
| `sub_69E11F` | `CMob__ProcessMobSkillEffect` | `0x69E11F` | 处理怪物技能效果 |
| `sub_694AB2` | `CMob__ProcessDamagedHitEffect` | `0x694AB2` | 处理受击特效 |