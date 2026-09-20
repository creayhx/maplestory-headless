# 冒险岛 人物属性 Hook 与加密机制

## 目录

1. [背景：BUFF 属性解码框架](#背景buff-属性解码框架)
2. [加密体系概述](#加密体系概述)
3. [查找流程](#查找流程)
4. [实战：魔防查找示例](#实战魔防查找示例)
5. [实战：物防详解](#实战物防详解)
6. [同族函数速查](#同族函数速查)
7. [BUFF 缓冲区偏移速查表](#buff-缓冲区偏移速查表)
8. [物防专用的 AA 脚本](#物防专用的-aa-脚本)

---

## 背景：BUFF 属性解码框架

所有 BUFF 属性在 `SecondaryStat::DecodeForLocal` (0x789E60) 中按顺序逐块处理。
每块代码的通用模板：

```
┌─ [可选] UINT128 标志检查 ──────────────────────┐
│  call UINT128::operator&                          │
│  call UINT128::operator_bool                      │
│  jz next_property                                 │
└──────────────────────────────────────────────────┘
        │
        ▼  ← 以下是当前属性的处理代码
call Decode2 / Decode4    ← 从包读取明文值
mov xx, xxx                ← 值处理(符号扩展/运算)
lea edx, [esi+OFFSET]     ← ★ 属性缓冲区
call ZtlSecureTear_long_  ← 加密
mov [esi+XX], eax          ← 存储校验和
```

函数基址 ESI = `0xBE4FA0`（BUFF 区域基址）。

---

## 加密体系概述

人物属性（包括 BUFF）均使用 `ZtlSecureTear` / `ZtlSecureFuse` 系列函数进行加密保护。

### 核心加/解密函数

#### ZtlSecureTear_long_ (0x416E36) — 加密

```
输入: ECX = 明文属性值, EDX = 输出缓冲区(8字节)
处理:
  1. key = CRand32::Random(g_rand_0)   ← 生成随机密钥
  2. buffer[0] = key                     ← 写入密钥
  3. encrypted = ROR4(plaintext XOR key, 5)  ← 加密
  4. buffer[1] = encrypted               ← 写入密文
  5. return encrypted + ROR4(key XOR 0xBAADF00D, 5)  ← 返回校验和
```

#### ZtlSecureFuse_long_ (0x416DE8) — 解密

```
输入: a1 = [key, encrypted] (8字节), a2 = 校验和
处理:
  1. plaintext = key XOR ROL5(encrypted)
  2. 验证: encrypted + ROR4(key XOR 0xBAADF00D, 5) == a2
  3. 不匹配 → 抛异常 ZException (游戏闪退)
  4. return plaintext
```

### 加密数据格式 (每 8 字节)

| 偏移 | 长度 | 说明 |
|------|------|------|
| +0x00 | dword | 随机密钥 key (CRand32::Random 生成) |
| +0x04 | dword | ROR4(plaintext XOR key, 5) — 加密后的值 |

每个属性占用 **3 组 × 8 字节 = 24 字节**，加密 3 个属性相关值（具体哪 3 个值因属性而异）。

---

## 查找流程

```
第1步: offset = 属性地址 - 0xBE4FA0
       ─────────────────────────────────
       计算出当前属性在 ESI 基址上的偏移

第2步: AOB 搜 lea edx, [esi+offset]
       ─────────────────────────────────
       offset ≤ 0x7F:  8D 56 XX
       offset > 0x7F:  8D 96 XX XX XX 00

       例(物防 offset=0x3C):
       搜 8D 56 3C → 找到 0x789E53 ✓

       ↓ 找到后你就知道了:
         - 属性在函数中的指令地址
         - 缓冲区 EDX 的值

第3步: 从 LEA 往上翻指令, 看 Decode 方式
       ─────────────────────────────────
       call Decode2 (0x4259A2) → 16位有符号
         ↓ 后面一定有 movsx ecx, ax (符号扩展)
       
       call Decode4 (0x4066FF) → 32位
         ↓ 后面一定有 mov ecx, eax (直接使用)

第4步: 确定 Hook 点
       ─────────────────────────────────
       Decode2: Hook 在 movsx ecx, ax
       Decode4: Hook 在 mov ecx, eax

       钩子时机: 值已从包读入 ECX,
                 但还没进入 ZtlSecureTear 加密之前

第5步: 记下原始字节, 写 AA 脚本
       ─────────────────────────────────
       读 Hook 地址往后 6 字节 (jmp 占 5 + nop 占 1)
       禁用时用这些字节恢复
```

---

## 实战：魔防查找示例

```
第1步: offset = 0xBE503C - 0xBE4FA0 = 0x9C

第2步: 搜 AOB → 8D 96 9C 00 00 00
       找到 → lea edx, [esi+9Ch] 指令地址

第3步: 往上翻:
       call Decode2 (0x4259A2)     ← 16位读取
       movsx ecx, ax               ← 符号扩展

第4步: Hook 点 = movsx ecx, ax 那条指令
       时机: 替换 ECX 后走加密, 校验和自动正确

第5步: 读原始字节 → 写 AA 脚本
```

---

## 实战：物防详解

### 基本信息

| 项目 | 内容 |
|------|------|
| 目标进程 | MapleStory.exe (32-bit) |
| 基址 | 0x400000 (无 ASLR) |
| 物防缓冲区 | `0x00BE4FDC` (24字节) |

### 物防写入流程

```
SecondaryStat::DecodeForLocal (0x789E60)
│
├─ Decode2 (0x4259A2)       ← 从包读取 2 字节(16位有符号)
│    │
│    └─ AX = 明文值 (如 +40、+9999)
│         │
│         ▼
├─ movsx ecx, ax            ← ECX = 明文值(符号扩展到32位)
│    │
│    ▼
├─ lea edx, [esi+3Ch]       ← EDX = 缓冲区 = 0xBE4FA0+0x3C = 0xBE4FDC
│    │
│    ▼
└─ call ZtlSecureTear_long_ ← 加密 ECX 写入 [0xBE4FDC]
     │
     └─ 校验和 → [esi+44h] = 0xBE4FE4
```

### 地址对应关系

| 基址 | ESI 偏移 | 说明 |
|------|---------|------|
| `[0x00BE4FDC]` = `ESI+0x3C` | +0x3C ~ +0x47 | 物防加密数据 (24字节) |
| `[0x00BE4FE4]` = `ESI+0x44` | +0x44 ~ +0x47 | 加密校验和 (第1组) |
| `[0x00BE4FE8]` = `ESI+0x48` | +0x48 ~ +0x4F | 下一属性加密数据 |
| `[0x00BE4FF0]` = `ESI+0x50` | +0x50 ~ +0x53 | 下一属性校验和 |

---

## 同族函数速查

| 地址 | 函数名 | 说明 |
|------|--------|------|
| 0x416E36 | `ZtlSecureTear_long_` | **加密** long 值 (8字节输出) |
| 0x416DE8 | `ZtlSecureFuse_long_` | **解密** long 值 |
| 0x6131B9 | `ZtlSecureTear_int_` | 加密 int 值 |
| 0x6131F4 | `ZtlSecureTear_double_` | 加密 double 值 |
| 0x4F7780 | `ZtlSecureTear_short_` | 加密 short 值 |
| 0xA705FB | `rol_custom__cdecl` | 自定义左旋转 ROL |
| 0xA700F4 | `__ROR4__` | 右旋转 4 字节 |

> 命名规律: **Tear** = 加密(撕碎保护), **Fuse** = 解密(恢复融合)

---

## BUFF 缓冲区偏移速查表

`SecondaryStat::DecodeForLocal` 中 ESI = `0xBE4FA0`（BUFF 区域基址）。
每个属性的偏移 = `属性地址 - 0xBE4FA0`。

| 地址 | ESI 偏移 | 属性 | Decode 方式 |
|------|---------|------|-------------|
| 0xBE4FAC | `ESI+0x0C` | 攻击力 | Decode4 (32bit) |
| **0xBE4FDC** | **`ESI+0x3C`** | **物防** | **Decode2 (16bit)** |
| 0xBE500C | `ESI+0x6C` | 魔法力 | Decode4 (32bit) |
| 0xBE503C | `ESI+0x9C` | 魔防 | Decode2 (16bit) |
| 0xBE506C | `ESI+0xCC` | 命中率 | — |
| 0xBE509C | `ESI+0xFC` | 闪避率 | — |
| 0xBE50FC | `ESI+0x15C` | 移动速度 | Decode2 (16bit) |
| 0xBE512C | `ESI+0x18C` | 跳跃力 | Decode2 (16bit) |
| 0xBE51A4 | `ESI+0x204` | 快速武器 | — |
| 0xBE5534 | `ESI+0x594` | 冒险岛勇士 | — |
| 0xBE5558 | `ESI+0x5B8` | 稳如泰山 | — |
| 0xBE557C | `ESI+0x5DC` | 火眼晶晶 | — |

> **Decode2** = 2 字节读取, `movsx ecx, ax` 符号扩展 (最大 32767)
>
> **Decode4** = 4 字节读取, `mov ecx, eax` 直接使用
>
> **—** = 待补充 (可按此流程自行定位)

---

## 物防专用的 AA 脚本

### 原理

在 `ZtlSecureTear_long_` **加密之前** 修改 ECX（明文值），
后续的加密和校验和计算都使用修改后的值，不会触发 CRC 检测。

### Hook 位置

```
0x789E50: movsx ecx, ax      ← 替换: 不再从包读取，直接用自定义值
0x789E53: lea edx, [esi+3Ch] ← 保留: 物防缓冲区指向
0x789E56: call ZtlSecureTear_long_ ← 保留: 走正常加密流程
```

### 操作步骤

1. 在 CE 表的 **BUFF** 分组下找到 `BUFF物防-修改值生成` 脚本
2. 编辑脚本，将 `mov ecx,#99999999` 中的数字改为目标值
3. **勾选启用** 脚本
4. 在游戏中释放一个 **物防 BUFF** 技能
5. 观察 `物防-生成的加密数据(24字节)` 条目，数据已更新
6. **选中 24 字节 → 右键 → 复制**
7. 粘贴到 CE 表 "物防" 条目的 "+9999" 子项中
8. **取消勾选禁用** 脚本

### 注意事项

1. **生成后立即禁用脚本**，否则后续物防 BUFF 都会被替换
2. 脚本已在 CE 表中验证字节完整：`0F BF C8 8D 56 3C` ↔ `E9 xx xx xx xx 90`
3. 加密校验和自动匹配，不会触发 CRC 检测
4. 生成的 24 字节数据可以永久保存、随时粘贴使用

---

## 一句话总结

**搜 AOB 定位 → 翻指令看 Decode 类型 → 在值进加密前 hook 替换 ECX → 24 字节带校验码自动生成**。
