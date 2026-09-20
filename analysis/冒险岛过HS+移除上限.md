# MapleStory 079 过 HackShield + 移除上限 — 最终分析归档

> **归档日期**: 2026-05-30
> **分析工具**: IDA Pro 9 (MCP Bridge) + Cheat Engine 11.4 (MCP Bridge)
> **目标二进制**: `MapleStory.exe` (CMS 079 脱壳版, ImageBase=0x00400000)
> **启动器**: `第三方启动器` (第三方编译, 自提取运行时)
> **启动器替代**: `mp-multi-client` (Rust, hs_bypass.rs)

---

## 一、执行流程概述

```
mp-multi-client (Rust 启动器)
  │
  ├── Step 1: CreateProcessW → MapleStory.exe (正常启动, 不挂起)
  │
  ├── Step 2: WriteProcessMemory × 14 处补丁 (进程创建后立即写入)
  │   ├── 5 处 HS 绕过
  │   ├── 1 处数据常量
  │   └── 8 处上限移除
  │
  ├── Step 3: WriteProcessMemory × 10 处分辨率补丁 (尺寸参数动态)
  │   ├── 2 处窗口创建参数 (push height/width)
  │   ├── 2 处窗口尺寸钳制 (mov eax, bounds)
  │   ├── 2 处 UI 组件尺寸 (push height/width)
  │   ├── 2 处 Render 尺寸 (mov edi/esi)
  │   └── 2 处窗口局部变量 (mov [ebp+disp32])
  │
  ├── Step 4: 后台轮询广告窗口 StartUpDlgClass
  │   ├── 检测到后: 再次写入 14 处补丁 (apply_hs_patches)
  │   └── 关闭广告窗口 → 进入登录页
  │
  └── Step 5: MapleStory.exe 正常运行 (HS 全关 + 上限移除 + 分辨率动态)
```

> **重要发现**: 测试证实过 HS 之后，整个 `HShield` 文件夹可删，游戏仍正常运行。
> HS 绕过 DLL (ehsvc.dll) 的部署是多余的——**仅靠内存补丁即可完整绕过 HS**。

---

## 二、HackShield 绕过：完整分析

### 2.1 HS 三层架构

IDA 调用链分析发现 HS 有三层结构：

```
sub_9FE52A (HS_MainEntry) ← HS 主控入口
  │
  ├── Step 1: sub_A0318C (HS_AllocContext)
  │   └── 设置 dword_BDD8A8 = 1 (全局 HS 就绪标志)
  │   └── **这是所有 HS 代码的"总开关"**
  │
  ├── Step 2: HS_Init_and_Load (0xA5A464)
  │   └── 加载 HShield\ehsvc.dll + SHA 校验
  │
  └── Step 3: sub_A5A671 (HS_LoadAndInitDll)
      └── LoadLibraryA("EHSvc.dll") + GetProcAddress
```

后续所有 HS 代码（`CClientSocket::SendPacket` 中的 HS 封包头部、
`sub_628259`/`sub_62849F` 的 HS 通知上报等）均通过 `if (dword_BDD8A8)` 守卫。

### 2.2 核心方案：HS_AllocContext RET 全局禁用

**原理**: `HS_AllocContext` (0xA0318C) 入口处写 `C3` (RET)，
函数直接返回，`dword_BDD8A8` 保持初始值 0。后续所有守卫代码全部跳过。

```
HS_AllocContext (0xA0318C):
  原始:  55 8B EC ...    push ebp; mov ebp, esp; ...  (16 字节)
  补丁:  C3              ret                           (1 字节)
```

**效果**: 一处补丁禁用全部 HS，**无需 ehsvc.dll 绕过文件**。
实测在打完内存补丁后，整个 `HShield` 文件夹删除后游戏仍正常运行。

### 2.3 确认有效的 HS 补丁清单（6处）

| # | 地址 | 大小 | 补丁 | 作用 | 验证 |
|---|------|------|------|------|------|
| 1 | `0x00A0318C` | 1 | `C3` | **HS_AllocContext RET — 全局开关** | ✅ CE 运行时确认 |
| 2 | `0x00A5A464` | 1 | `C3` | HS_Init_and_Load RET (冗余, 防御性) | ✅ IDA 确认 |
| 3 | `0x00A5A949` | 1 | `C3` | 附加 HS 函数 RET | ✅ 实测稳定 |
| 4 | `0x00D02B4B` | **5** | `68 6C 97 52 07` | HS 动态段调度: push 0x0752976C | ⚠ 必须是 5 字节 (`push imm32`), 4 字节会截断 |
| 5 | `0x00958D81` | 2 | `90 90` | HS 消息/聊天抑制 NOP | ✅ IDA 确认 |
| 6 | `0x00B064B8` | 8 | `00 00 00 40 DD 4A DF 41` | 数据常量 double: ~2.0e9 | ✅ CE 运行时确认 |

### 2.4 已废弃的补丁

| 地址 | 原因 |
|------|------|
| `0x009FE755` | `74 14` (JZ) → `75 14` (JNZ) **导致 HS 启动错误！** 因为条件翻转反而激活了 HS 代码路径。**不可使用**。 |

---

## 三、上限移除：验证过程

### 3.1 第一阶段：IDA 静态分析失败

IDA 分析得到 3 处代码地址和统一 cap 值 `0x3B6B2800` (997M)：

| IDA 地址 | 归属函数 | IDA 推定原始值 | 推定补丁值 |
|----------|---------|---------------|-----------|
| 0x00786A90 | sub_7869BB | 1999 | `B8 00 28 6B 3B` (997M) |
| 0x0078884E | sub_78790C | 999 | `B8 00 28 6B 3B` (997M) |
| 0x0078881C | sub_78790C | 999 | `B8 00 28 6B 3B` (997M) |

**部署后结果**: 运行时崩溃 ❌

### 3.2 第二阶段：地址偏移 1 字节（崩溃根因）

IDA 精确反汇编发现 `mov reg, imm32` 指令起始地址比预期早 1 字节：

```asm
; IDA 实际反汇编:
0x786A8F: B8 CF 07 00 00    mov eax, 1999    ; ← 指令从 0x786A8F 开始
0x786A94: 3B D8             cmp ebx, eax
0x786A96: 7D 02             jge +2
0x786A98: 8B C3             mov eax, ebx
0x786A9A: 5F                pop edi

; 但之前补丁写在了 0x786A90:
0x786A90: B8 00 28 6B 3B    ; CPU 解码为 mov eax, 0x6B2800B8 ← 完全错位！
```

| 错误地址 | **正确地址** | 偏差 |
|----------|-------------|------|
| 0x00786A90 | **0x00786A8F** | +1 |
| 0x0078884E | **0x0078884D** | +1 |
| 0x0078881C | **0x0078881B** | +1 |

### 3.3 第三阶段：CE 运行时验证 cap 值

连接 CE 到**工具版本客户端**（已正确打补丁的参考实现），
读取各地址的实际运行时值，发现 cap 值并非统一的 997M：

| 正确地址 | 指令 | 实际运行时值 | 十进制 |
|----------|------|-------------|-------|
| `0x00786A8F` | `B8 B8 00 28 6B` | `mov eax, 0x6B2800B8` | ~1,797,201,112 |
| `0x0078884D` | `B9 B8 00 28 6B` | `mov ecx, 0x6B2800B8` | ~1,797,201,112 |
| `0x0078881B` | `B9 B8 00 28 6B` | `mov ecx, 0x6B2800B8` | ~1,797,201,112 |
| `0x0078876B` | `BF FF E0 F5 05` | `mov edi, 0x05F5E0FF` | 99,999,999 |
| `0x007869A9` | `B8 FF E0 F5 00` | `mov eax, 0x00F5E0FF` | 16,113,919 |
| `0x007868CF` | `B8 FF E0 F5 1C` | `mov eax, 0x1CF5E0FF` | 485,875,967 |
| `0x00796BF2` | `BE FF FF FF 00` | `mov esi, 0x00FFFFFF` | 16,777,215 |
| `0x008C8BAE` | `B8 FF E0 F5 64` | `mov eax, 0x64F5E0FF` | 1,697,448,703 |

**关键发现**: `0x00796BF2` 原始错误补丁为 `BE FF FF FF FF` (`esi = -1`)，
配合后续指令 `jl` (有符号小于跳转) 会导致所有**正数值都触发上限**，
返回 `0xFFFFFFFF`。这是之前所有上限补丁一起开时崩溃的根本原因之一。

### 3.4 确认有效的上限移除补丁（8处）

所有补丁均为 5 字节 `mov reg, imm32`，只替换 cap 立即数，保留原比较逻辑不变：

| # | 地址 | 补丁 | 含义 |
|---|------|------|------|
| 1 | `0x00786A8F` | `B8 B8 00 28 6B` | mov eax, 1.79B — StatCalc |
| 2 | `0x0078884D` | `B9 B8 00 28 6B` | mov ecx, 1.79B — CoreStatCalc #1 |
| 3 | `0x0078881B` | `B9 B8 00 28 6B` | mov ecx, 1.79B — CoreStatCalc #2 |
| 4 | `0x0078876B` | `BF FF E0 F5 05` | mov edi, 99,999,999 |
| 5 | `0x007869A9` | `B8 FF E0 F5 00` | mov eax, 16,113,919 |
| 6 | `0x007868CF` | `B8 FF E0 F5 1C` | mov eax, 485,875,967 |
| 7 | `0x00796BF2` | `BE FF FF FF 00` | mov esi, 16,777,215 |
| 8 | `0x008C8BAE` | `B8 FF E0 F5 64` | mov eax, 1,697,448,703 |

### 3.5 其他候选地址（已确认无关，保留备忘）

以下地址来自 第三方启动器 数据区字符串，经 CE 确认与分辨率/上限**无关**：

```
0x009EC61A, 0x005CA9AC, 0x005CA9B8, 0x009FFF01, 0x009FFF06
0x0057609F, 0x005760A5, 0x00BD178C, 0x00BD1788, 0x00BD35E4
0x00BD35E0, 0x009AFFC0, 0x009AFFCD, 0x008D6FE8, 0x008D6FE3
0x008D8D74, 0x008D6D3B, 0x00841181, 0x00841186, 0x008DD6F6
```

---

## 四、最终补丁清单（Rust 实现: hs_bypass.rs）

### 4.1 静态补丁: `get_patches()` — 14 处

```rust
// ── HS 绕过 (5处) ──
HsPatch { address: 0x00A0318C, data: vec![0xC3] },
HsPatch { address: 0x00A5A464, data: vec![0xC3] },
HsPatch { address: 0x00A5A949, data: vec![0xC3] },
HsPatch { address: 0x00D02B4B, data: vec![0x68, 0x6C, 0x97, 0x52, 0x07] },
HsPatch { address: 0x00958D81, data: vec![0x90, 0x90] },

// ── 数据常量 ──
HsPatch { address: 0x00B064B8, data: vec![0x00, 0x00, 0x00, 0x40, 0xDD, 0x4A, 0xDF, 0x41] },

// ── 上限移除 (8处, 5字节 mov reg, imm32) ──
HsPatch { address: 0x00786A8F, data: vec![0xB8, 0xB8, 0x00, 0x28, 0x6B] },
HsPatch { address: 0x0078884D, data: vec![0xB9, 0xB8, 0x00, 0x28, 0x6B] },
HsPatch { address: 0x0078881B, data: vec![0xB9, 0xB8, 0x00, 0x28, 0x6B] },
HsPatch { address: 0x0078876B, data: vec![0xBF, 0xFF, 0xE0, 0xF5, 0x05] },
HsPatch { address: 0x007869A9, data: vec![0xB8, 0xFF, 0xE0, 0xF5, 0x00] },
HsPatch { address: 0x007868CF, data: vec![0xB8, 0xFF, 0xE0, 0xF5, 0x1C] },
HsPatch { address: 0x00796BF2, data: vec![0xBE, 0xFF, 0xFF, 0xFF, 0x00] },
HsPatch { address: 0x008C8BAE, data: vec![0xB8, 0xFF, 0xE0, 0xF5, 0x64] },
```

### 4.2 动态分辨率补丁: `apply_resolution_patches()` — 10 处

根据传入的 width/height 动态生成机器码写入：

| # | VA 地址 | 指令 (原始) | 原始默认值 | 作用 | 补丁格式 |
|---|---------|------------|-----------|------|---------|
| 1 | `0x00A00FA0` | `68 58 02 00 00` (push 600) | 600 | 窗口高度参数 | `68 HH HH HH HH` |
| 2 | `0x00A00FA5` | `68 20 03 00 00` (push 800) | 800 | 窗口宽度参数 | `68 WW WW WW WW` |
| 3 | `0x005CB0F1` | `B8 20 03 00 00` (mov eax, 800) | 800 | 宽度边界钳制 | `B8 WW WW WW WW` |
| 4 | `0x005CB10A` | `B8 58 02 00 00` (mov eax, 600) | 600 | 高度边界钳制 | `B8 HH HH HH HH` |
| 5 | `0x005BBAE3` | `68 58 02 00 00` (push 600) | 600 | UI 组件高度 | `68 HH HH HH HH` |
| 6 | `0x005BBAE8` | `68 20 03 00 00` (push 800) | 800 | UI 组件宽度 | `68 WW WW WW WW` |
| 7 | `0x00436E61` | `BF 58 02 00 00` (mov edi, 600) | 600 | Render 高度 | `BF HH HH HH HH` |
| 8 | `0x00436E67` | `BE 20 03 00 00` (mov esi, 800) | 800 | Render 宽度 | `BE WW WW WW WW` |
| 9 | `0x009FC107` | `C7 85 04FEFFFF 20030000` (mov [ebp-0x1FC], 800) | 800 | 主窗口宽局部变量 | `C7 85 04 FE FF FF WW WW WW WW` |
| 10 | `0x009FC111` | `C7 85 08FEFFFF 29020000` (mov [ebp-0x1F8], 553) | 553 | 主窗口高局部变量 | `C7 85 08 FE FF FF HH HH HH HH` |

> 注: 这些地址原为硬编码的 800×600 常量，现在通过 WPM 动态替换为用户输入的宽高值。
> #10 原始值为 553（= 600 - 47，疑似扣除了窗口标题栏后的 client 区高度）。

### 4.3 写入时序

```
进程创建 (CreateProcessW)
  │
  ├─ 立即写入: write_all_patches()  → 14 处静态补丁
  ├─ 立即写入: apply_resolution_patches() → 10 处分辨率补丁
  │
  └─ 后台检测到广告窗口 (StartUpDlgClass)
       └─ 再次写入: apply_hs_patches() → 14 处静态补丁 (二次保障)
```

### 4.4 分类统计

| 类别 | 数量 | 说明 |
|------|------|------|
| HS 绕过 | 5 | 含 HS_AllocContext 全局 RET |
| 数据常量 | 1 | double ~2.0e9 |
| 上限移除 | 8 | 5 字节 mov reg,imm32 |
| 分辨率窗口 | 10 | 动态生成，按用户输入宽高写入 |
| **总计** | **24** | (14 静态 + 10 动态) |

### 4.5 写入方式

- 进程创建后**立即**调用 `WriteProcessMemory`（无需 `CREATE_SUSPENDED`）
- 分辨率补丁需在窗口创建前写入才有效（进程创建后立即写）
- 写入顺序不重要（地址不重叠）

---

## 五、排错历程总结

### 问题 1：HS 启动错误
- **症状**: 提示"HS 初始化错误"
- **原因**: `0x9FE755` 处 `74 14` (JZ) 被改为 `75 14` (JNZ)，条件翻转后激活了 HS
- **解决**: 彻底删除该补丁

### 问题 2：运行时崩溃
- **症状**: 进游戏后立即崩溃
- **原因**: 3 处 cap 补丁**地址偏移 1 字节**，写入在指令中间
- **解决**: 修正为正确地址（0x786A90→0x786A8F 等）

### 问题 3：运行时崩溃（所有 cap 全开时）
- **症状**: "挂了，上限里面有问题"
- **原因**: `0x796BF2` 处 `BE FF FF FF FF` 设 `esi = -1`，`jl` 导致所有正数触发上限
- **解决**: 改为 `BE FF FF FF 00` (`esi = 0x00FFFFFF`)，CE 实测值修复

### 问题 4：cap 值不统一
- **症状**: 用统一值 997M 写入，但各地址实际要求不同
- **原因**: IDA 静态分析无法知道工具版本的自定义 cap 值
- **解决**: CE 连接运行中游戏，直接读取各地址实际值

### 问题 5：push imm32 缺字节
- **症状**: 0xD02B4B 写入 4 字节后运行异常
- **原因**: `push imm32` 编码为 5 字节 (`68 xx xx xx xx`)，只写 4 字节留下垃圾字节
- **解决**: 写入 5 字节

### 问题 6：分辨率地址错误（RVA vs VA）
- **症状**: 输入 1366×768 实际为 800×600
- **原因**: 分辨率补丁地址用了 RVA（`0x00600FA0`）而非 VA（`0x00A00FA0`），遗漏了 ImageBase（`0x400000`）
- **解决**: 修正为完整 VA

### 问题 7：窗口大盒套小盒
- **症状**: 1280×720 窗口中有一个 800×600 的内嵌子框
- **原因**: 此前仅补丁了 2 处窗口创建参数（`0x00A00FA0/A5`），另有 8 处分辨率硬编码（bound clamp、UI组件、Render尺寸、窗口局部变量）未补丁
- **解决**: 增补至 10 处分辨率补丁

---

## 六、关键文件结构

```
mp-multi-client/
  └── src-tauri/client/src/
      └── hs_bypass.rs           ← Rust 补丁实现 (14 处静态 + 10 处动态分辨率)
  └── src-tauri/src/cmd/
      └── tool_cmd.rs             ← Tauri 命令入口 (start_game)


mcp/
  └── maplestory/
      └── 冒险岛过HS+移除上限.md  ← 本文档 (分析归档)
```

---

## 七、参考

| 项目 | 值 |
|------|-----|
| dword_BDD8A8 (HS 全局标志) | `0x00BDD8A8` |
| MapleStory.exe ImageBase | `0x00400000` |
| HS 主控入口 | `sub_9FE52A` |
| HS_AllocContext | `sub_A0318C` @ `0x00A0318C` |
| HS_Init_and_Load | `sub_A5A464` @ `0x00A5A464` |
| 动态段 (________) | 运行时生成, 非 PE 区段 |
| 分辨率窗口参数 | `0x00A00FA0` (H) / `0x00A00FA5` (W) |
| 分辨率边界钳制 | `0x005CB0F1` (W) / `0x005CB10A` (H) |
| 分辨率 UI 组件 | `0x005BBAE3` (H) / `0x005BBAE8` (W) |
| 分辨率 Render | `0x00436E61` (H) / `0x00436E67` (W) |
| 分辨率窗口局部变量 | `0x009FC107` (W) / `0x009FC111` (H) |

---

> **分析结论**: HS 绕过 5 处 + 上限移除 8 处 + 分辨率补丁 10 处，共计 **24 处补丁**。
> HS 绕过仅靠内存补丁即可，**无需 DLL 部署**，整个 `HShield` 文件夹可删除。
> **最终验证**: CE 运行时读取确认 × 游戏内实测稳定
