# 冒险岛 BUFF 属性加密机制

## 基本信息

| 项目 | 内容 |
|------|------|
| 目标进程 | MapleStory.exe (32-bit) |
| 基址 | 0x400000 (无 ASLR) |
| BUFF 缓冲区 | `0x00BE4FAC` ~ `0x00BE557C` |

---

## CInPacket 函数说明

### CInPacket::DecodeBuffer (0x431E77)

通用包解码函数，从接收到的包中解码一段原始字节数据。
使用 `memcpy` 拷贝数据，并推进包的读取位置。
**非 BUFF 专用函数**，是所有包解码的基础设施。

### CInPacket::AppendBuffer (0x6F5896)

通用包编码函数，将数据编码到发送包中。
处理 XOR 加密 (`Decode2`)，管理包长度和状态。
**非 BUFF 专用函数**，是所有包编码的基础设施。

---

## BUFF 属性加密体系

### SecondaryStat::DecodeForLocal (0x789E60)

从服务器包中解码本地玩家的 BUFF（辅助状态/Secondary Stat）数据。
该函数遍历所有 BUFF 属性，调用 `ZtlSecureTear_long_` 加密后写入 `0xBE4FDC` 等缓冲区。

---

## 核心加/解密函数

### ZtlSecureTear_long_ (0x416E36) — 加密

```
输入: ECX = 明文属性值, EDX = 输出缓冲区(8字节)
处理:
  1. key = CRand32::Random(g_rand_0)   ← 生成随机密钥
  2. buffer[0] = key                     ← 写入密钥
  3. encrypted = ROR4(plaintext XOR key, 5)  ← 加密
  4. buffer[1] = encrypted               ← 写入密文
  5. return encrypted + ROR4(key XOR 0xBAADF00D, 5)  ← 校验和
```

### ZtlSecureFuse_long_ (0x416DE8) — 解密

```
输入: a1 = [key, encrypted] (8字节), a2 = 校验和
处理:
  1. plaintext = key XOR ROL5(encrypted)
  2. 验证: encrypted + ROR4(key XOR 0xBAADF00D, 5) == a2
  3. 不匹配 → 抛异常 (ZException)
  4. return plaintext
```

### 加密数据格式 (每 8 字节)

```
偏移  长度  说明
+0x00  dword  随机密钥 key (CRand32::Random 生成)
+0x04  dword  ROR4(plaintext XOR key, 5)  加密后的值
```

### 同族函数

| 地址 | 函数名 | 说明 |
|------|--------|------|
| 0x416E36 | `ZtlSecureTear_long_` | 加密 long 值 (8字节输出) |
| 0x416DE8 | `ZtlSecureFuse_long_` | 解密 long 值 |
| 0x6131B9 | `ZtlSecureTear_int_` | 加密 int 值 |
| 0x6131F4 | `ZtlSecureTear_double_` | 加密 double 值 |
| 0x4F7780 | `ZtlSecureTear_short_` | 加密 short 值 |

> 命名规律: **Tear** = 加密(撕碎保护), **Fuse** = 解密(恢复融合)

---

## 24 字节 BUFF 数据结构

每个 BUFF 属性占用 **24 字节** = 3 × `ZtlSecureTear_long_` 输出：

```
[0xBE4FDC] 物防:  XX XX XX XX  YY YY YY YY  ZZ ZZ ZZ ZZ  ... ← 24字节
                  [key1]       [enc1]       [key2]       [enc2]       [key3]       [enc3]
                  密钥1        密文1        密钥2        密文2        密钥3        密文3
```

每个 8 字节块加密一个属性相关值（基础值、增量值、标志等）。

### BUFF 缓冲区地址表

| 地址 | CE 表名 | 最大修改值 |
|------|---------|-----------|
| 0xBE4FAC | 攻击力 | 32767 |
| 0xBE4FDC | 物防 | +9999 |
| 0xBE500C | 魔法力 | 32767 |
| 0xBE503C | 魔防 | +9999 |
| 0xBE506C | 命中率 | +32767 |
| 0xBE509C | 闪避率 | +32767 |
| 0xBE50FC | 移动速度 | 40 |
| 0xBE512C | 跳跃力 | 20 |
| 0xBE51A4 | 快速武器 | -10 |
| 0xBE5534 | 冒险岛勇士 | 32767% |
| 0xBE5558 | 稳如泰山 | 100 |
| 0xBE557C | 火眼晶晶 | 100% |

---

## "随机值可复用" 原理

每次使用 BUFF，加密函数都会生成**随机密钥 key**。因为密钥被保存在 8 字节数据中，所以：

```
BUFF 应用: 明文(+9999) → 随机key(=0xA1B2C3D4) → 8字节: [A1B2C3D4, ROR4(9999 XOR A1B2C3D4, 5)]
BUFF 取消: 8字节被清除/重置
BUFF 粘贴: 写入保存的8字节 → 解密得 +9999 → 属性恢复
```

**每次生成的 24 字节数据看起来不同**，因为：
1. `CRand32::Random` 每次产生不同的随机 key
2. 不同 key → 不同加密结果

**但任何一组都可以直接粘贴回去使用**，因为：
1. 密钥和数据一起保存，是**自包含**的
2. 只要数据没被篡改（校验和正确），解密后一定得到原始属性值
3. 改动任何字节都会导致校验和不匹配 → 游戏抛异常

这就是"看起来随机、但都能用"的原因。
