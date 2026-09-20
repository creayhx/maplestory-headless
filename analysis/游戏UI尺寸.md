# 游戏 UI 分辨率相关地址大全

> 目标: MapleStory CMS 079 (脱壳版, ImageBase=0x00400000)
> 来源: 079分辨率地址.txt (第三方开源工程) + IDA 验证
> 状态: 所有 txt 中的 CMS 079 地址已全部分类纳入 ✅
> 最后更新: 2026-05-30 (IDA修正: 0x8D4C75为高度非Y位, 新增QuickSlot按钮24地址, StatusBar Y=22)

---

## 一、已补丁地址（代码已实现 ~95 处分辨率补丁）

集中在 `hs_bypass.rs::apply_resolution_patches` 中。以下仅列部分关键地址，详见代码。

### 1.1 窗口创建参数

| 序号 | 地址 | 汇编(原始) | 原始值 | 补丁方式 |
|------|------|-----------|--------|---------|
| 1 | `0x00A00FA0` | `push 258h` | 600 | `0x68 + height` |
| 2 | `0x00A00FA5` | `push 320h` | 800 | `0x68 + width` |

### 1.2 窗口尺寸边界钳制

| 序号 | 地址 | 汇编(原始) | 原始值 | 补丁方式 |
|------|------|-----------|--------|---------|
| 3 | `0x005CB0F1` | `mov eax, 320h` | 800 | `0xB8 + width` |
| 4 | `0x005CB10A` | `mov eax, 258h` | 600 | `0xB8 + height` |

### 1.3 主窗口尺寸

| 序号 | 地址 | 汇编(原始) | 原始值 | 补丁方式 |
|------|------|-----------|--------|---------|
| 5 | `0x009FFF01` | `push 258h` | 600 | `0x68 + height` |
| 6 | `0x009FFF06` | `push 320h` | 800 | `0x68 + width` |

### 1.4 窗口表面（含 StatusBar 背景）

| 序号 | 地址 | 汇编(原始) | 原始值 | 补丁方式 |
|------|------|-----------|--------|---------|
| 7 | `0x008D6FE8` | `push 320h` | 800 | `0x68 + width` |
| 8 | `0x008D6FE3` | `push 242h` | 578 | `0x68 + height-22` |

> 注：0x8D6FE3 实际在 `CUIStatusBar::OnDraw` 内，是 **StatusBar 背景 Y**，不是纯窗口表面。补丁值用 height-22。

### 1.5 游戏渲染表面（客户区高=高-22）

| 序号 | 地址 | 汇编(原始) | 原始值 | 补丁方式 |
|------|------|-----------|--------|---------|
| 9 | `0x008D6D40` | `push 320h` | 800 | `0x68 + width` |
| 10 | `0x008D6D3B` | `push 242h` | 578 | `0x68 + (height-22)` |

### 1.6 UI 参考半宽/半高（数据区, ÷2）

| 序号 | 地址 | 数据类型 | 原始值 | 补丁方式 |
|------|------|---------|--------|---------|
| 11 | `0x00BD35E0` | `dd 190h` | 400 (=800/2) | 写 width/2 |
| 12 | `0x00BD35E4` | `dd 12Ch` | 300 (=600/2) | 写 height/2 |
| 13 | `0x00BD1788` | `dd 190h` | 400 (=800/2) | 写 width/2 |
| 14 | `0x00BD178C` | `dd 12Ch` | 300 (=600/2) | 写 height/2 |

### 1.7 UI 钳位边界

| 序号 | 地址 | 汇编(原始) | 原始值 | 补丁方式 |
|------|------|-----------|--------|---------|
| 15 | `0x009AFFC0` | `mov edx, 320h` | 800 | `0xBA + width` |
| 16 | `0x009AFFCD` | `mov edx, 258h` | 600 | `0xBA + height` |

---

## 二、鼠标限制范围（★ 核心，新增）

从 `079分辨率地址.txt` 提取的完整鼠标系统。MapleStory 的鼠标系统在 800x600 下使用**向量偏移 + 边界钳制**实现居中限制。

### 2.1 鼠标系统架构

```
800x600 下的计算流程:
  center = (width/2, height/2) = (400, 300)
  vector = (-width/2, -height/2) = (-400, -300)   ← 从中心偏移到左上角
  clamp: 鼠标位置限制在 (0,0) ~ (800,600)
  update_limit: 鼠标更新范围上限 (800, 600)
```

需要改成 1366x768:
```
  center = (683, 384)
  vector = (-683, -384)
  clamp: (0,0) ~ (1366, 768)
  update_limit: (1366, 768)
```

### 2.2 鼠标向量（Cursor Vector）

鼠标从窗口**左上角(0,0)偏移到中心**所用的向量值。

| 地址 | 原始值(800x600) | 目标值(1366x768) | 说明 |
|------|----------------|-----------------|------|
| `0x005CA9AC` | -300 (0xFFFFFED4) | -384 (0xFFFFFE80) | 鼠标向量 Y = `-height/2` |
| `0x005CA9B8` | -400 (0xFFFFFE70) | -683 (0xFFFFFD55) | 鼠标向量 X = `-width/2` |

第三方命名: `dwCursorVectorVPos / dwCursorVectorHPos`
IDA: 在 `CInputSystem::LoadCursorState` 附近的立即数

**重要性: ★★★★★** — 鼠标向量决定了点击坐标的起始偏移。不改的话鼠标点击会偏移 (400-683, 300-384) 像素。

### 2.3 鼠标中心点计算

| 地址 | 原始值(800x600) | 目标值(1366x768) | 说明 |
|------|----------------|-----------------|------|
| `0x005CA787` | 400 (0x190) | 683 (0x2AB) | 鼠标中心 X = `floor(width/2)` |
| `0x005CA78D` | 300 (0x12C) | 384 (0x180) | 鼠标中心 Y = `floor(height/2)` |

第三方命名: `0x005CEE54+2 / 0x005CEE5A+6` → 079 `0x005CA787 / 0x005CA78D`
函数: `CInputSystem::LoadCursorState`

**重要性: ★★★★★** — 鼠标中心不对，游戏内的鼠标坐标计算全部偏移。

### 2.4 鼠标位置限制（Cursor Pos Limit，已补丁）

| 地址 | 原始值(800x600) | 目标值(1366x768) | 说明 |
|------|----------------|-----------------|------|
| `0x005CB0F1` | 800 (0x320) | 1366 (0x556) | 鼠标 X 位置上限 |
| `0x005CB10A` | 600 (0x258) | 768 (0x300) | 鼠标 Y 位置上限 |

### 2.5 鼠标更新范围限制（Update Mouse Limit）

| 地址 | 原始值(800x600) | 目标值(1366x768) | 说明 |
|------|----------------|-----------------|------|
| `0x005CB67E` | 800 (0x320) | 1366 (0x556) | 鼠标更新 X 上限 |
| `0x005CB697` | 600 (0x258) | 768 (0x300) | 鼠标更新 Y 上限 |

第三方命名: `dwUpdateMouseLimitHPos / dwUpdateMouseLimitVPos`
函数: `sub_5CB573`（滚动条/鼠标位置更新函数）

**重要性: ★★★★** — 限制鼠标更新的最大范围。不修改的话鼠标可能在某些区域无法移动过去。

### 2.6 鼠标系统内存示意图

```
005CA787: [width/2]    ← 鼠标中心 X  (数据段)
005CA78D: [height/2]   ← 鼠标中心 Y  (数据段)
005CA9AC: -height/2    ← 鼠标向量 Y  (代码段, 指令立即数)
005CA9B8: -width/2     ← 鼠标向量 X  (代码段, 指令立即数)
005CB0F1: width        ← 鼠标 X 上限 (代码段, mov eax, imm32)
005CB10A: height       ← 鼠标 Y 上限 (代码段, mov eax, imm32)
005CB67E: width        ← 更新 X 上限 (代码段, 立即数)
005CB697: height       ← 更新 Y 上限 (代码段, 立即数)
```

---

## 三、裁剪边界（17处，高优先级）

这些地址将坐标限制在 800x600 范围内，导致 UI 被居中裁剪。

| 序号 | 地址 | 函数 | 指令 | 含义 | 优先级 |
|------|------|------|------|------|--------|
| 1 | `0x0081C804` | sub_81C7E4 (clip函数) | `mov edx, 320h` | X 裁剪边界 | ★★★ |
| 2 | `0x0081C817` | sub_81C7E4 (clip函数) | `mov edi, 258h` | Y 裁剪边界 | ★★★ |
| 3 | `0x004D88AF` | sub_4D85C6 | `mov eax, 320h; cmp edx, eax; jle` | X 裁剪 | ★★★ |
| 4 | `0x004D88C4` | sub_4D85C6 | `cmp ecx, 258h; jle` | Y 裁剪（配对） | ★★★ |
| 5 | `0x004E33A0` | sub_4E30B7 | `mov eax, 320h; cmp edx, eax; jle` | X 裁剪 | ★★★ |
| 6 | `0x004E33B5` | sub_4E30B7 | `cmp ecx, 258h; jle` | Y 裁剪（配对） | ★★★ |
| 7 | `0x005CB688` | sub_5CB573（滚动条位置） | `jl (if >=800)` | 滚动条 X 裁剪 | ★★★ |
| 8 | `0x005CB6A1` | sub_5CB573（滚动条位置） | `jl (if >=600)` | 滚动条 Y 裁剪 | ★★★ |
| 9 | `0x008F224E` | sub_8F2071 | `mov eax, 320h; cmp edx, eax; jle` | X 裁剪 | ★★★ |
| 10 | `0x008F226A` | sub_8F2071 | `mov eax, 258h; cmp ecx, eax; jle` | Y 裁剪（配对） | ★★★ |
| 11 | `0x006931E7` | sub_6931D1 | `cmp esi, 320h; jl` | 裁剪 | ★★★ |
| 12 | `0x0086D3BC` | sub_86D353 | `cmp edx, 320h; jle` | 裁剪 | ★★★ |
| 13 | `0x007E4073` | sub_7E4007 | `cmp edx, 320h; jle` | 裁剪 | ★★★ |
| 14 | `0x009A36A6` | sub_9A354F | `cmp edi, 320h; jle` | 裁剪 | ★★★ |
| 15 | `0x009A4B0D` | (无函数名) | `cmp edi, 320h; jle` | 裁剪 | ★★★ |
| 16 | `0x009A4FF0` | sub_9A4E29 | `cmp edi, 320h; jle` | 裁剪 | ★★★ |
| 17 | `0x0093BEA8` | CUser__CreateSkillEffect | `push 320h; push 12Ch; call Render_SetResolution` | 硬编码渲染分辨率 800x300 | ★★★ |

---

## 四、居中计算（~28处）

这些地址做 `(800 - width)/2` 或 `(600 - height)/2`，将 UI 元素居中。补丁后 UI 在新的分辨率下居中显示。

### 4.1 X 轴居中（mov eax, 320h; sub eax, xxx; shr 1）

| 序号 | 地址 | 函数 | 指令模式 |
|------|------|------|---------|
| 1 | `0x00589CF5` | sub_589855 | `mov ecx, 320h; sub ecx, eax; shr 1` |
| 2 | `0x005B7123` | sub_5B6DB8 | `mov eax, 320h; sub eax, ebx; cdq` |
| 3 | `0x005B7872` | sub_5B752E | `mov eax, 320h; sub eax, edi; cdq` |
| 4 | `0x005DDA90` | sub_5DD7CA | `mov eax, 320h; sub eax, [ebx+1Ch]` |
| 5 | `0x005DEC30` | sub_5DE94B | `mov eax, 320h; sub eax, [ebx+1Ch]` |
| 6 | `0x005DFE78` | sub_5DFB8A | `mov eax, 320h; sub eax, [edi+1Ch]` |
| 7 | `0x005E0E42` | sub_5E0B5D | `mov eax, 320h; sub eax, [ebx+1Ch]` |
| 8 | `0x005E1C90` | sub_5E19A2 | `mov eax, 320h; sub eax, [ebx+1Ch]` |
| 9 | `0x005F52A0` | (无函数) | `mov eax, 320h` |
| 10 | `0x005F677B` | sub_5F648D | `mov eax, 320h; sub eax, [edi+1Ch]` |
| 11 | `0x007D69ED` | sub_7D66EB | `mov eax, 320h; sub eax, [edi+1Ch]` |
| 12 | `0x007EC157` | sub_7EC073 | `mov ecx, 320h; sub ecx, eax; shr 1` |
| 13 | `0x007EC6ED` | sub_7EC5A3 | `mov ecx, 320h; sub ecx, eax; shr 1` |
| 14 | `0x00803569` | sub_80345B | `mov ecx, 320h; sub ecx, eax; shr 1` |
| 15 | `0x00804E8F` | sub_804D7B | `mov ecx, 320h; sub ecx, eax; shr 1` |
| 16 | `0x00806551` | sub_80641F | `mov ecx, 320h; sub ecx, eax; shr 1` |
| 17 | `0x008070A1` | sub_806B95 | `mov ecx, 320h; sub ecx, eax; shr 1` |
| 18 | `0x0080AB51` | sub_80AA5E | `mov ecx, 320h; sub ecx, eax; shr 1` |
| 19 | `0x008B23BA` | sub_8B20B8 | `mov eax, 320h; sub eax, [edi+1Ch]; sar 1` |
| 20 | `0x009E9D8C` | sub_9E8B87 | `mov ecx, 320h; sub ecx, eax` |
| 21 | `0x004FD3E3` | sub_4FD3C2 | `mov eax, 320h` |
| 22 | `0x004FD494` | sub_4FD3FA | `mov ecx, 320h` |
| 23 | `0x005434BA` | CField::ShowMobHPTag | `mov eax, 320h; sub eax, [ebp-44h]` |
| 24 | `0x009C4BF8` | - | `add eax, 320h; div ecx`（含 800 偏移的除法）|

### 4.2 Y 轴居中（mov eax, 258h; ...）

| 序号 | 地址 | 函数 | 指令模式 |
|------|------|------|---------|
| 1 | `0x008B23AC` | sub_8B20B8 | `mov eax, 258h; sub eax, [edi+20h]; sar 1` |
| 2 | `0x0099EBA9` | sub_99EB5B | `mov eax, 258h; sub eax, ecx; cdq` |
| 3 | `0x009A1BFC` | sub_9A1B3E | `mov eax, 258h; sub eax, ecx; cdq` |
| 4 | `0x009E9F02` | sub_9E8B87 | `mov ecx, 258h; sub ecx, eax` |

---

## 五、分辨率检查分支（8处）

这些地址检查参数是否为 600/800，决定走哪条路径。

| 序号 | 地址 | 函数 | 指令 |
|------|------|------|------|
| 1 | `0x004BB185` | sub_4BB179 | `cmp [ebp+arg_4], 258h; jz` |
| 2 | `0x004BB18E` | sub_4BB179 | `cmp [ebp+arg_4], 320h; jnz` |
| 3 | `0x004BC061` | sub_4BC055 | `cmp [ebp+arg_4], 320h; jnz` |
| 4 | `0x005C59C2` | sub_5C59B3 | `cmp [ebp+arg_4], 258h; jnz` |
| 5 | `0x0062D349` | sub_62D33A | `cmp [ebp+arg_4], 258h; jnz` |
| 6 | `0x0063FC23` | sub_63FC14 | `cmp [ebp+arg_4], 258h; jnz` |
| 7 | `0x007DFB88` | sub_7DFB7C | `cmp [ebp+arg_4], 320h; jnz` |
| 8 | `0x005F0385` | sub_5F0372 | `cmp edi, 320h; jnz` |

---

## 六、UI 组件坐标地址（新增）

从 `079分辨率地址.txt` 提取的 UI 组件坐标地址。这些控制特定 UI 元素在屏幕上的位置和限制。

### 6.1 状态栏（StatusBar）

状态栏在游戏窗口底部，显示经验值、HP/MP 等。

| 序号 | 地址 | 函数 | 原始值(800x600) | 说明 | 补丁状态 |
|------|------|------|----------------|------|---------|
| 1 | `0x008D4C75` | `CUIStatusBar::CUIStatusBar` | 578 (=600-22) | ★ **高度** (a5), 不是Y位置! sub_9E8502 参数顺序 (X,Y,W,H) | ✅ rh |
| 2 | `0x008D4C7A` | `CUIStatusBar::CUIStatusBar` | 800 | ★ **宽度** (a4) | ✅ w |
| 3 | `0x008D4C7F` | `CUIStatusBar::CUIStatusBar` | 22 (0x16) | ★ **Y 坐标** (a3), 标题栏高度, 背景位置核心 | ❌ |
| 4 | `0x008D4C81` | `CUIStatusBar::CUIStatusBar` | 0 (ebx) | X 坐标 (a2) | N/A |
| 5 | `0x008210CE` | `CUIStatusBarSub::CUIStatusBarSub` | 578 | 子组件 rect Y (push imm32) | ✅ rh |
| 6 | `0x008D6FE3` | `CUIStatusBar::OnDraw` | 578 (=600-22) | 背景渲染目标 #2 Y (this+0xC74) | ✅ rh |
| 7 | `0x008D6FE8` | `CUIStatusBar::OnDraw` | 800 | 背景渲染目标 #2 X | ✅ w |
| 8 | `0x008D6D3B` | `CUIStatusBar::OnDraw` | 578 | 背景渲染目标 #1 Y (this+0xC70) 原标"渲染表面" | ✅ rh |
| 9 | `0x008D6D40` | `CUIStatusBar::OnDraw` | 800 | 背景渲染目标 #1 X | ✅ w |
| 10 | `0x008D6F54` | `CUIStatusBar::OnDraw` | `push 22` (高度常量) | ★ CE验证: 非Y位置，是StatusBar高度22 | N/A |
| 11 | `0x008D6F59` | `CUIStatusBar::OnDraw` | `push eax` (动态值) | ★ CE验证: 非硬编码，虚表调用参数 | N/A |
| 12 | `0x008D7160` | (待查) | 578 | StatusBar 输入框 Y | ❌ |
| 13 | `0x008D716B` | (待查) | 800 | StatusBar 输入框右边界 X | ❌ |

函数重命名:
- `sub_8D49F0` → `CUIStatusBar::CUIStatusBar`（构造函数，调用 `sub_9E8502` 创建主状态栏）
- `sub_8D50C8` → `CUIStatusBar::OnDraw`（14KB 大函数，绘制背景 + 各元素）
- `sub_820FF6` → `CUIStatusBarSub::CUIStatusBarSub`（子组件，含 `sub_932702(29,4,558,10,...)` 和 `sub_A70B1E` 创建11个子窗口）

第三方命名: `dwStatusBarVPos / dwStatusBarPosRetn / dwStatusBarBackgroundVPos / dwStatusBarBackgroundPosRetn / dwStatusBarInputVPos / dwStatusBarInputPosRetn`

**重要性: ★★★** — 不改的话状态栏背景位置偏移，底部出现空白。

### 6.2 提示框限制（ToolTip Limit）

| 地址 | 原始值(800x600) | 目标值(1366x768) | 说明 |
|------|----------------|-----------------|------|
| `0x008F93CC` | 599 (0x257) | 767 (0x2FF) | ToolTip Y 边界限制 |
| `0x008F93B9` | 799 (0x31F) | 1365 (0x555) | ToolTip X 边界限制 |

第三方命名: `dwToolTipLimitVPos / dwToolTipLimitHPos`
原始值比分辨率小1 (=800-1, 600-1)，是 `<` 比较的条件

**重要性: ★★★** — 提示框可能被限制在原 800x600 区域。

### 6.3 Buff 图标 / 临时状态（TempStat）

| 序号 | 地址 | 函数 | 说明 |
|------|------|------|------|
| 1 | `0x007B9AC9` | - | TempStat Icon VPos |
| 2 | `0x007B9AE7` | - | TempStat Icon HPos |
| 3 | `0x007B9BD2` | - | TempStat CoolTime VPos |
| 4 | `0x007B9BF0` | - | TempStat CoolTime HPos |
| 5 | `0x007B9CD2` | - | TempStat ToolTip Draw |
| 6 | `0x007B9EB8` | - | TempStat ToolTip Find (右键) |

第三方命名: `dwTempStatIconVPos / dwTempStatIconHpos / dwTempStatCoolTimeVPos / dwTempStatCoolTimeHPos / dwTempStatToolTipDraw / dwTempStatToolTipFind`

**重要性: ★★** — Buff 图标的位置。不改的话在高分辨率下可能有偏移。

### 6.4 快捷键栏（QuickSlot）

| 序号 | 地址 | 函数 | 说明 | 补丁状态 |
|------|------|------|------|---------|
| 1 | `0x008D677E` | sub_8D4B70 | add eax, 533(0x215) → VPos Init (add eax, imm32, 复杂) | ❌ 回滚 |
| 2 | `0x008D6785` | sub_8D4B70 | push 572(0x23C) → HPos Init | ✅ CE: 976 |
| 3 | `0x008E411C` | sub_8E3E64 | add esi, 533(0x215) → VPos (add esi, imm32, 复杂) | ❌ 回滚 |
| 4 | `0x008E4170` | sub_8E3E64 | push 572(0x23C) → HPos | ✅ CE: 976 |
| 5 | `0x008E33C9` | sub_8E31CA | lea ebx, [eax-427] → CWnd VPos (lea 复杂) | ❌ 回滚 |
| 6 | `0x008E33C0` | sub_8E31CA | lea edi, [eax-572] → CWnd HPos (lea 复杂) | ❌ 回滚 |
| 7 | `0x008E4448` | sub_8E4353 | push 580(0x244) → 4功能按钮0 X | ✅ push w*580/800 |
| 8 | `0x008E4443` | sub_8E4353 | push 440(0x1B8) → 4功能按钮0 Y | ✅ push h*440/600 |
| 9 | `0x008E44C5` | sub_8E4353 | push 614(0x266) → 4功能按钮1 X | ✅ push w*614/800 |
| 10 | `0x008E44C0` | sub_8E4353 | push 440(0x1B8) → 4功能按钮1 Y | ✅ push h*440/600 |
| 11 | `0x008E4542` | sub_8E4353 | push 580(0x244) → 4功能按钮2 X | ✅ push w*580/800 |
| 12 | `0x008E453D` | sub_8E4353 | push 473(0x1D9) → 4功能按钮2 Y | ✅ push h*473/600 |
| 13 | `0x008E45BF` | sub_8E4353 | push 614(0x266) → 4功能按钮3 X | ✅ push w*614/800 |
| 14 | `0x008E45BA` | sub_8E4353 | push 473(0x1D9) → 4功能按钮3 Y | ✅ push h*473/600 |

第三方命名: `dwQuickSlotInitVPos / dwQuickSlotInitHPos / dwQuickSlotVPos / dwQuickSlotHPos / dwQuickSlotCWndVPos / dwQuickSlotCWndHPos`

IDA 新增: sub_8E4353 内4个功能按钮(2×2网格, ID 2000-2003) 的8个 push imm32

**重要性: ★★** — 快捷键栏的位置调整。

### 6.5 头像/米加（AvatarMega）

| 地址 | 原始值(800x600) | 目标值(1366x768) | 说明 |
|------|----------------|-----------------|------|
| `0x0045B1A6` | 900 (0x384) | ~1533 | Avatar Mega HPos（角色选择界面） |
| `0x00459F1B` | 800 (0x320) | 1366 | Avatar Mega Width |

**重要性: ★★** — 角色选择界面大头的显示尺寸。

### 6.6 视口（ViewPort）

| 地址 | 原始值(800x600) | 说明 |
|------|----------------|------|
| `0x009E9D20` | 宽(lea -800立即数) | ViewPort Width ← 原标Height，CE验证 -800 为宽度计算 |
| `0x009E9E98` | 高(lea -600立即数) | ViewPort Height ← 原标Width，CE验证 -600 为高度计算 |

**重要性: ★★★** — 视口尺寸决定了游戏世界的可见范围。

---

## 七、特殊地址

### 7.1 截图尺寸

| 地址 | 函数 | 指令 | 说明 |
|------|------|------|------|
| `0x007518B2` | ijlWrite | `mov edx, 320h; mov ecx, 258h` | JPEG 截图宽高硬编码 |

### 7.2 图像步长/跨度计算

| 地址 | 函数 | 指令 | 说明 |
|------|------|------|------|
| `0x00629B49` | - | `imul eax, 258h` | 步长 = x * 600 |
| `0x00629B8D` | - | `imul eax, 258h` | 步长 |
| `0x00629BC4` | - | `imul eax, 258h` | 步长 |
| `0x00647BE8` | - | `imul eax, 258h` | 步长 |

### 7.3 图块/列索引计算

| 地址 | 函数 | 指令 | 说明 |
|------|------|------|------|
| `0x009D046F` | - | `mov ecx, 320h; div ecx` | 除以 800（图块索引） |

### 7.4 动画/矩形计算区（CAnimationDisplayer + CRectangle）

0x0066xxxx 段是 CAnimationDisplayer 和内部 CRectangle 计算区的大量成对地址：

| 地址 | 原始值(800x600) | 说明 |
|------|----------------|------|
| `0x0066E60E` | 400 (=800/2) | VRTop X 中心 (floor(width/2)) |
| `0x0066E675` | 300 (=600/2) | VRRight Y 中心 (floor(height/2)) |
| `0x0066E73C` | 300 (=600/2) | Y 中心计算 |
| `0x0066E1DB` | 800 | SetCenterOrigin Width |
| `0x0066E1E5` | 600 | SetCenterOrigin Height |
| `0x0066E0C6` | 600 | CAnimationDisplayer height |
| `0x0066E0BF` | 800 | CAnimationDisplayer width |
| `0x0066CB0A` | 600 | CRectangle height |
| `0x0066CAF8` | 800 | CRectangle width |
| `0x0066CC90` | 600 | CRectangle height (多处) |
| `0x0066CC7E` | 800 | CRectangle width (多处) |
| `0x0066CC18` | 600 | CRectangle height |
| `0x0066CC1D` | 600 | CRectangle height |
| `0x0066CC4B` | 600 | CRectangle height |
| `0x0066CC06` | 800 | CRectangle width |
| `0x0066CC39` | 800 | CRectangle width |
| `0x0066CC26` | 400 | floor(width/2) modulus |
| `0x0066CA3E` | 400 | floor(width/2) |
| `0x0066CA43` | 300 | floor(height/2) |
| `0x0066CC56` | -400 | -width/2 |
| `0x0066D6AD` | -300 | -height/2 |
| `0x0066D6B5` | -400 | -width/2 |

### 7.5 IWzGr2DLayer::Getcanvas 尺寸

| 地址 | 原始值(800x600) | 说明 |
|------|----------------|------|
| `0x0065ACF4` | 600 | IWzGr2DLayer::Getcanvas height |
| `0x0065B4CB` | 800 | IWzGr2DLayer::Getcanvas width |
| `0x0065B4C6` | 600 | IWzGr2DLayer::Getcanvas height |
| `0x0065ACF9` | 800 | IWzGr2DLayer::Getcanvas width |
| `0x0065A64D` / `0x0065A8E3` / `0x0065B030` | 600 | UI/Logo/Wizet height (模糊匹配) |
| `0x0065A652` / `0x0065B035` / `0x0065A8E8` | 800 | UI/Logo/Wizet width (模糊匹配) |

### 7.6 IWzVector2D::RelMove / CreateWnd 成对地址

以下地址成对出现，用 `m_nGameWidth` / `m_nGameHeight` 做窗口创建参数或相对移动：

| 高地址 | 宽地址 | 函数标注 | 说明 |
|--------|--------|---------|------|
| `0x00575D78` (600) | `0x00575D7D` (800) | RelMove? | 窗口尺寸写入对 |
| `0x00575DF4` (600) | - | RelMove? | 仅高度 (疑似误标为m_nGameWidth) |
| `0x005E0E32` (600) | `0x005E0E42` (800) | RelMove? | UI 偏移对 |
| `0x005E1C80` (600) | `0x005E1C90` (800) | RelMove? | UI 偏移对 |
| `0x005DFE68` (600) | `0x005DFE78` (800) | RelMove? | UI 偏移对 |
| `0x005F5290` (600) | `0x005F52A0` (800) | RelMove? | UI 偏移对 |
| `0x005F676B` (600) | `0x005F677B` (800) | RelMove? | UI 偏移对 |
| `0x007D69DF` (600) | `0x007D69ED` (800) | IWzVector2D::RelMove | UI 偏移对 |
| `0x0080AB3D` (600) | `0x0080AB51` (800) | CWnd::CreateWnd | 子窗口创建尺寸 |
| `0x004D88C4` (600) | `0x004D88AF` (800) | CreateWnd | ✅ 已在三.3 |
| `0x004E33B5` (600) | `0x004E33A0` (800) | CreateWnd | ✅ 已在三.5 |
| `0x008B23AC` (600) | `0x008B23BA` (800) | IWzVector2D::RelMove | ✅ 已在四 |
| `0x009E9F02` (600) | `0x009E9D8C` (800) | IWzVector2D::RelMove | ✅ 已在四 |
| `0x004E3250`/`0x004D875F` (800) | - | CreateWnd | 仅宽 (模糊匹配) |
| `0x004E7AF7` (800) | - | - | 仅宽 |
| `0x007F7C09` (600) | `0x007F7C0E` (800) | CWnd::GetCanvas | Canvas 尺寸 |
| `0x007F8207` (800) | - | - | 仅宽 |
| `0x008BB421` (600) | `0x008BB426` (800) | CreateWnd | 窗口创建 |
| `0x00436E61` (600) | `0x00436E67` (800) | CreateWnd | ✅ 已在 7.5 |
| `0x0046B007` (600) | `0x0046B019` (800) | IWzVector2D::RelMove | UI 偏移对 |
| `0x009A3691` (600) | `0x009A36A6` (800) | - | 尺寸对 |
| `0x009A4FDB` (600) | `0x009A4FF0` (800) | - | ✅ 宽已在三.16 |
| `0x008DCDF2` (800) | `0x008DCDED` (578) | - | ✅ 高已在 7.7 (Height-22) |
| `0x0065ACF4` (600) | `0x0065ACF9` (800) | IWzGr2DLayer | ✅ 已在上节 |
| `0x006BE6EA` | 400 | floor(width/2) | 中心偏移 |
| `0x006BF23E` (800) | `0x006BF359` (-400) | - | width / -width/2 对 |
| `0x009FF84F` | 300 | MapleStoryClass | floor(height/2) |

### 7.7 RelMove 负偏移（-height/2 或 -width/2）

| 地址 | 原始值(800x600) | 说明 |
|------|----------------|------|
| `0x0057609F` | -300 (= -600/2) | RelMove -height/2 |
| `0x005760A5` | -400 (= -800/2) | RelMove -width/2 |
| `0x00436F9C` | -300/-400 | RelMove -height/2 or -width/2 |
| `0x00436F97` | -300 | RelMove -height/2 |
| `0x009EC61A` | -300 (= -600/2) | COM 偏移 -height/2 |
| `0x009EC620` | -400 (= -800/2) | COM 偏移 -width/2 |
| `0x0062A7DE` | -300 | -height/2 |
| `0x0062A922` | -300 | -height/2 |
| `0x00669275` | -400 | -width/2 | ← CE验证失败: 此处为 je+6，非立即数，地址存疑 |
| `0x006BF359` | -400 | -width/2 |
| `0x0093B675` | -300 | -height/2 |
| `0x0095DBA3` | -300 | -height/2 |
| `0x0098EE27` | -300 | -height/2 |
| `0x0098F84C` | -300 | -height/2 |
| `0x00A4EAC9` / `0x00A55A79` | -300 | -height/2, CWvsPhysicalSpace2D::Load |
| `0x009CFD87` | -300 | -height/2 |
| `0x009CFD95` | 300 | height/2 |

### 7.8 UI 高度偏移（Height-N 类地址）

许多 UI 组件使用 `Height - N` 的偏移来定位：

| 地址 | 原始值(800x600) | 公式 | 说明 |
|------|----------------|------|------|
| `0x008D6D3B` | 578 | Height - 22 | 游戏渲染表面 |
| `0x008D6FE3` | 578 | Height - 22 | D3D 窗口表面 |
| `0x008D4C75` | 578 | Height - 22 | StatusBar VPos |
| `0x008DCDED` | 578 | Height - 22 | 底部 Y 位置 |
| `0x008D7458` | 567 | Height - 33 | 渲染 Y 位置 |
| `0x008E36A8` | 580 | Height - 20 | UI 内部 Y |
| `0x008E3944` | 580 | Height - 20 | UI 内部 Y |
| `0x008D7739` | 581 | Height - 19 | 渲染 Y |
| `0x008D797F` | 581 | Height - 19 | 渲染 Y |
| `0x008DD6F6` | 581 | Height - 19 | 渲染 Y |
| `0x008DDE76` | 581 | Height - 19 | 渲染 Y |
| `0x008DE5C1` | 581 | Height - 19 | 渲染 Y |
| `0x0045B0C0` | 575 | Height - 25 | 角色选择 Y | ✅ rh25 |
| `0x008D4C7F` | 22 (0x16) | 标题栏高度 | ★ StatusBar Y 位置核心 (a3) | ❌ |
| `0x0086D3C4` | 720 | Width - 80 | 对话框宽度 |
| `0x0086D3C4` | 720 | Width - 80 | 对话框宽度 |
| `0x009A36AE` | 700 | Width - 100 | 对话框宽度 |
| `0x009A4FF8` | 700 | Width - 100 | 对话框宽度 |

这些地址在补丁时直接用 `Height - 22` 之类的公式重新计算即可。

---

## 八、079分辨率地址.txt 完整地址清单

> 以下为 `079分辨率地址.txt` 中的所有地址，按类型分组。已包含至前文各章节的在此列出补丁状态。

### 8.1 命名常量（dw前缀）

| 第三方命名 | 079 地址 | 原始值 | 补丁状态 | 说明 |
|---------|---------|--------|---------|------|
| `dwApplicationHeight` | `0x00A00FA0` | 600 | ✅ 已补丁 | 窗口创建高度 |
| `dwApplicationWidth` | `0x00A00FA5` | 800 | ✅ 已补丁 | 窗口创建宽度 |
| `dwCursorVectorVPos` | `0x005CA9AC` | -300 | ✅ 已补丁 | 鼠标向量 Y |
| `dwCursorVectorHPos` | `0x005CA9B8` | -400 | ✅ 已补丁 | 鼠标向量 X |
| `dwCursorPosLimitVPos` | `0x005CB10A` | 600 | ✅ 已补丁 | 鼠标 Y 上限 |
| `dwCursorPosLimitHPos` | `0x005CB0F1` | 800 | ✅ 已补丁 | 鼠标 X 上限 |
| `dwUpdateMouseLimitVPos` | `0x005CB697` | 600 | ✅ 已补丁 | 鼠标更新 Y 上限 |
| `dwUpdateMouseLimitHPos` | `0x005CB67E` | 800 | ✅ 已补丁 | 鼠标更新 X 上限 |
| `dwToolTipLimitVPos` | `0x008F93CC` | 599(=600-1) | ✅ 已补丁 | ToolTip Y 边界 |
| `dwToolTipLimitHPos` | `0x008F93B9` | 799(=800-1) | ✅ 已补丁 | ToolTip X 边界 |
| `dwStatusBarVPos` | `0x008D4C75` | 578 | ✅ 已补丁 | ★ 实为**高度**(a5), sub_9E8502参数顺序(X,Y,W,H) |
| `dwStatusBarPosRetn` | `0x008D4C84` | (返回地址) | N/A | 状态栏返回点 |
| `dwStatusBarBackgroundVPos` | `0x008D6F54` | `push 22` | N/A | ★ CE验证: 高度常量，非Y |
| `dwStatusBarBackgroundPosRetn` | `0x008D6F59` | `push eax` | N/A | ★ CE验证: 动态值 |
| `dwStatusBarInputVPos` | `0x008D7160` | 578 | ❌ 未补丁 | 输入框 Y (CE搜索0x8D7000段无push 578, 地址存疑) |
| `dwStatusBarInputPosRetn` | `0x008D716B` | 800 | ❌ 未补丁 | 输入框右边界 X |
| `dwTempStatIconVPos` | `0x007B9AC9` | 297(0x129) | ❌ sub ebx(复杂) | Buff 图标 Y, 已回滚 |
| `dwTempStatIconHpos` | `0x007B9AE7` | 397(0x18D) | ❌ lea(复杂) | Buff 图标 X |
| `dwTempStatCoolTimeVPos` | `0x007B9BD2` | 297(0x129) | ❌ sub ebx(复杂) | Buff 冷却 Y |
| `dwTempStatCoolTimeHPos` | `0x007B9BF0` | 397(0x18D) | ❌ lea(复杂) | Buff 冷却 X |
| `dwTempStatToolTipDraw` | `0x007B9CD2` | -797(0x31D) | ❌ lea(复杂) | Buff 提示绘制 |
| `dwTempStatToolTipFind` | `0x007B9EB8` | -797(0x31D) | ❌ lea(复杂) | Buff 提示(右键) |
| `dwQuickSlotInitVPos` | `0x008D677E` | 533(0x215) | ❌ add eax(复杂) | 快捷键栏初始化 Y, 已回滚 |
| `dwQuickSlotInitHPos` | `0x008D6785` | 572(0x23C) | ✅ push w*572/800 | 快捷键栏初始化 X (CE: 976) |
| `dwQuickSlotVPos` | `0x008E411C` | 533(0x215) | ❌ add esi(复杂) | 快捷键栏 Y, 已回滚 |
| `dwQuickSlotHPos` | `0x008E4170` | 572(0x23C) | ✅ push w*572/800 | 快捷键栏 X (CE: 976) |
| `dwQuickSlotCWndVPos` | `0x008E33C9` | 427(0x1AB) | ❌ lea(复杂) | 快捷键栏 CWnd Y, 已回滚 |
| `dwQuickSlotCWndHPos` | `0x008E33C0` | 572(0x23C) | ❌ lea(复杂) | 快捷键栏 CWnd X, 已回滚 |
| `dwByteAvatarMegaHPos` | `0x0045B1A6` | 900 | ✅ 已补丁 | Avatar 大图 HPos |
| `dwAvatarMegaWidth` | `0x00459F1B` | 800 | ✅ 已补丁 | Avatar 大图宽 |
| `dwViewPortHeight` | `0x009E9D20` | (-800 lea) | ❌ 未补丁 | ViewPort 宽度(CE验证) |
| `dwViewPortWidth` | `0x009E9E98` | (-600 lea) | ❌ 未补丁 | ViewPort 高度(CE验证) |

### 8.2 CreateWnd / RelMove 成对地址

这些来自 txt 的 `*(unsigned long*)(addr + offset) = m_nGameHeight/Width;` 写入模式。

| 079 地址 | 值 | 标签（来自 txt） | 补丁状态 |
|---------|-----|-----------------|---------|
| `0x00436E61` | 600 | CreateWnd | ❌ |
| `0x00436E67` | 800 | CreateWnd | ❌ |
| `0x004D88AF` | 800 | CreateWnd | ✅ 已在三.裁剪 |
| `0x004D88C4` | 600 | CreateWnd | ✅ 已在三.裁剪 |
| `0x004E33A0` | 800 | CreateWnd | ✅ 已在三.裁剪 |
| `0x004E33B5` | 600 | CreateWnd | ✅ 已在三.裁剪 |
| `0x004E3250` / `0x004D875F` | 800 | CreateWnd(仅宽) | ❌ 待确认 |
| `0x004E7AF7` | 800 | (仅宽) | ❌ 待确认 |
| `0x004FD3E3` | 800 | CreateWnd | ❌ |
| `0x004FD494` | 800 | CreateWnd | ❌ |
| `0x00575D78` | 600 | RelMove? | ❌ |
| `0x00575D7D` | 800 | RelMove? | ❌ |
| `0x00575DF4` | 600 | RelMove?(仅高) | ❌ 待确认 |
| `0x005DFE68` | 600 | RelMove? | ❌ |
| `0x005DFE78` | 800 | RelMove? | ❌ |
| `0x005E0E32` | 600 | RelMove? | ❌ |
| `0x005E0E42` | 800 | RelMove? | ❌ |
| `0x005E1C80` | 600 | RelMove? | ❌ |
| `0x005E1C90` | 800 | RelMove? | ❌ |
| `0x005F5290` | 600 | RelMove? | ❌ |
| `0x005F52A0` | 800 | RelMove? | ✅ 已在四.置中 |
| `0x005F676B` | 600 | RelMove? | ❌ |
| `0x005F677B` | 800 | RelMove? | ❌ |
| `0x007D69DF` | 600 | IWzVector2D::RelMove | ❌ |
| `0x007D69ED` | 800 | IWzVector2D::RelMove | ❌ |
| `0x007F7C09` | 600 | CWnd::GetCanvas | ❌ |
| `0x007F7C0E` | 800 | CWnd::GetCanvas | ❌ |
| `0x007F8207` | 800 | (仅宽) | ❌ |
| `0x0080AB3D` | 600 | CWnd::CreateWnd | ❌ |
| `0x0080AB51` | 800 | CWnd::CreateWnd | ❌ |
| `0x0086D3BC` | 800 | (裁剪) | ✅ 已在三.裁剪 |
| `0x0086D3C4` | 720(=800-80) | CreateDlg 对话框宽 | ❌ |
| `0x008B23AC` | 600 | IWzVector2D::RelMove | ✅ 已在四.置中 |
| `0x008B23BA` | 800 | IWzVector2D::RelMove | ❌ |
| `0x008BB421` | 600 | CreateWnd | ❌ |
| `0x008BB426` | 800 | CreateWnd | ❌ |
| `0x008F224E` | 800 | (裁剪) | ✅ 已在三.裁剪 |
| `0x008F226A` | 600 | (裁剪) | ✅ 已在三.裁剪 |
| `0x009A3691` | 600 | (成对) | ❌ |
| `0x009A36A6` | 800 | (裁剪) | ✅ 已在三.裁剪 |
| `0x009A36AE` | 700(=800-100) | CreateDlg 对话框宽 | ❌ |
| `0x009A4FDB` | 600 | (成对) | ❌ |
| `0x009A4FF0` | 800 | (裁剪) | ✅ 已在三.裁剪 |
| `0x009A4FF8` | 700(=800-100) | CreateDlg 对话框宽 | ❌ |
| `0x009AFFC0` | 800 | CreateDlg | ✅ 已在六.UI钳位 |
| `0x009AFFCD` | 600 | CreateDlg | ✅ 已在六.UI钳位 |
| `0x009E9D8C` | 800 | IWzVector2D::RelMove | ✅ 已在四.置中 |
| `0x009E9F02` | 600 | IWzVector2D::RelMove | ✅ 已在四.置中 |
| `0x009FFF01` | 600 | (窗口) | ✅ 已在窗口尺寸 |
| `0x009FFF06` | 800 | (窗口) | ✅ 已在窗口尺寸 |
| `0x00BD1788` | 400(=800/2) | floor(w/2) 数据区 | ✅ 已在半宽/高 |
| `0x00BD35E0` | 400(=800/2) | floor(w/2) 数据区 | ✅ 已在半宽/高 |
| `0x00BD35E4` | 300(=600/2) | floor(h/2) 数据区 | ✅ 已在半宽/高 |

### 8.3 IWzGr2DLayer::Getcanvas / Rectangle 成对

| 079 地址 | 值 | 函数标注 | 补丁状态 |
|---------|-----|---------|---------|
| `0x0065ACF4` | 600 | IWzGr2DLayer::Getcanvas | ❌ |
| `0x0065ACF9` | 800 | IWzGr2DLayer::Getcanvas | ❌ |
| `0x0065B4C6` | 600 | IWzGr2DLayer::Getcanvas | ❌ |
| `0x0065B4CB` | 800 | IWzGr2DLayer::Getcanvas | ❌ |
| `0x0065A64D` / `0x0065A8E3` / `0x0065B030` | 600 | UI/Logo/Wizet height | ❌ |
| `0x0065A652` / `0x0065B035` / `0x0065A8E8` | 800 | UI/Logo/Wizet width | ❌ |
| `0x0066CAF8` | 800 | CRectangle width | ❌ |
| `0x0066CB0A` | 600 | CRectangle height | ❌ |
| `0x0066CC06` | 800 | CRectangle width | ❌ |
| `0x0066CC18` | 600 | CRectangle height | ❌ |
| `0x0066CC1D` | 600 | CRectangle height | ❌ |
| `0x0066CC39` | 800 | CRectangle width | ❌ |
| `0x0066CC4B` | 600 | CRectangle height | ❌ |
| `0x0066CC7E` | 800 | CRectangle width | ❌ |
| `0x0066CC90` | 600 | CRectangle height | ❌ |
| `0x0066CA3E` | floor(w/2) | 动画中心 X | ❌ |
| `0x0066CA43` | floor(h/2) | 动画中心 Y | ❌ |
| `0x0066CC26` | floor(w/2) | 模运算 | ❌ |

### 8.4 CAnimationDisplayer 尺寸

| 079 地址 | 值 | 说明 | 补丁状态 |
|---------|-----|------|---------|
| `0x0066E0BF` | 800 | CAnimationDisplayer width | ❌ |
| `0x0066E0C6` | 600 | CAnimationDisplayer height | ❌ |
| `0x0066E1DB` | 800 | SetCenterOrigin Width | ❌ |
| `0x0066E1E5` | 600 | SetCenterOrigin Height | ❌ |
| `0x0066E60E` | floor(w/2) | VRTop X | ❌ |
| `0x0066E675` | floor(h/2) | VRRight Y | ❌ |
| `0x0066E73C` | floor(h/2) | 中心 Y | ❌ |
| `0x006BE6EA` | floor(w/2) | RelMove? | ❌ |

### 8.5 对话框宽度
| 079 地址 | 值 | 说明 | 补丁状态 |
|---------|-----|------|---------|
| `0x0086D3C4` | 720(=800-80) | 对话框宽 | ❌ |
| `0x009A36AE` | 700(=800-100) | 对话框宽 | ❌ |
| `0x009A4FF8` | 700(=800-100) | 对话框宽 | ❌ |

### 8.6 屏幕中心 / 其他计算

| 079 地址 | 值 | 说明 | 补丁状态 |
|---------|-----|------|---------|
| `0x005CA787` | floor(w/2) | 鼠标中心 X | ✅ 已在鼠标 |
| `0x005CA78D` | floor(h/2) | 鼠标中心 Y | ✅ 已在鼠标 |
| `0x0045B0C0` | 575(=600-25) | AvatarMega Y | ❌ |
| `0x0046B007` | 600 | IWzVector2D::RelMove | ❌ |
| `0x0046B019` | 800 | IWzVector2D::RelMove | ❌ |
| `0x0081C804` | 800 | 裁剪边界 X | ✅ 已在三 |
| `0x0081C817` | 600 | 裁剪边界 Y | ✅ 已在三 |
| `0x008D4C75` | 578(=600-22) | CUIStatusBar | ✅ |
| `0x008D4C7A` | 800 | CUIStatusBar | ✅ |
| `0x008D6D3B` | 578(=600-22) | 渲染表面高 | ✅ 已在窗口 |
| `0x008D6D40` | 800 | 渲染表面宽 | ✅ 已在窗口 |
| `0x008D6FE3` | 578(=600-22) | CUIStatusBar::OnDraw | ✅ |
| `0x008D6FE8` | 800 | CUIStatusBar::OnDraw | ✅ |
| `0x008D7458` | 567(=600-33) | 渲染 Y 偏移 | ✅ |
| `0x008D7739` | 581(=600-19) | 渲染子元素 Y | ✅ |
| `0x008D797F` | 581(=600-19) | 渲染子元素 Y | ✅ |
| `0x008DD6F6` | 581(=600-19) | 渲染子元素 Y | ✅ |
| `0x008DDE76` | 581(=600-19) | 渲染子元素 Y | ✅ |
| `0x008DE5C1` | 581(=600-19) | 渲染子元素 Y | ✅ |
| `0x008DCDED` | 578(=600-22) | 底部栏 Y | ✅ |
| `0x008DCDF2` | 800 | 底部栏 X | ✅ |
| `0x008E36A8` | 580(=600-20) | UI 内部 Y | ❌ |
| `0x008E3944` | 580(=600-20) | UI 内部 Y | ❌ |
| `0x0093BEA8` | 800 | push 800+300 硬编码技能渲染 | ❌ |
| `0x009FF84F` | floor(h/2) | MapleStoryClass | ❌ |

### 8.7 RelMove 负偏移（-half）

| 079 地址 | 值 | 说明 | 补丁状态 |
|---------|-----|------|---------|
| `0x00436F97` | -300(= -600/2) | -height/2 | ✅ |
| `0x00436F9C` | -300或-400 | -height/2 或 -width/2 | ✅ |
| `0x0057609F` | -300(= -600/2) | -height/2 | ✅ |
| `0x005760A5` | -400(= -800/2) | -width/2 | ✅ |
| `0x0062A7DE` | -300(= -600/2) | -height/2 | ✅ |
| `0x0062A922` | -300(= -600/2) | -height/2 | ✅ |
| `0x0066CC56` | -400(= -800/2) | -width/2 | ✅ |
| `0x0066D6AD` | -300(= -600/2) | -height/2 | ✅ |
| `0x0066D6B5` | -400(= -800/2) | -width/2 | ✅ |
| `0x006BF23E` | 800 | width (配对) | ✅ |
| `0x006BF359` | -400(= -800/2) | -width/2 | ✅ |
| `0x00669275` | -400(= -800/2) | ★ CE验证: je+6非立即数，地址存疑 | ❌ |
| `0x0093B675` | -300(= -600/2) | -height/2 | ✅ |
| `0x0095DBA3` | -300(= -600/2) | -height/2 | ❌ |
| `0x0098EE27` | -300(= -600/2) | -height/2 | ❌ |
| `0x0098F84C` | -300(= -600/2) | -height/2 | ❌ |
| `0x009CFD87` | -300(= -600/2) | -height/2 (mov [ebp+disp]) | ✅ |
| `0x009CFD95` | 300(= 600/2) | +height/2 (mov [ebp+disp]) | ✅ |
| `0x009EC61A` | -300(= -600/2) | COM -height/2 | ✅ |
| `0x009EC620` | -400(= -800/2) | COM -width/2 | ✅ |
| `0x00A55A79` | -300(= -600/2) | CWvsPhysicalSpace2D::Load | ✅ |

### 8.8 排除地址（以下为非分辨率相关的 800/600 值）

| 地址 | 值/指令 | 原因 |
|------|---------|------|
| StringPool (~50处) | `push 320h` | 800 是字符串资源 ID |
| `0x00741E32` | `add eax, 320h` | 800ms 超时值 |
| `0x00741F46` | `add eax, 320h` | 800ms 超时值 |
| `0x00742858` | `add eax, 320h` | 800ms 超时值 |
| `0x00A66198` | `sub esp, 320h` | 800 字节栈空间分配 |
| `0x005D4261` | `push 258h` | BGM 音轨 ID 600 |
| `0x0066E860` | CSoundMan::PlayBGM | BGM 音轨 ID |
| `0x00634BB1/0x00634BBB` | CSoundMan::PlayBGM | 音轨参数 |
| `0x0075930E` | `*(value+1)=m_nGameHeight` | 疑似音效，周围BGM相关 |
| `0x007591B6` | `*(value+1)=m_nGameHeight` | 疑似音效 |
| `0x00758C19` | `*(value+1)=m_nGameHeight` | 疑似音效 |
| `0x00758D04` | `*(value+1)=m_nGameHeight` | 疑似音效 |
| `0x006E7F58/0x006E72BD/0x006E8849` | `*(value+1)=m_nGameHeight` | 疑似音效(多版本) |
| `0x0061ACCE` | `*(value+1)=m_nGameHeight` | 单地址无配对，疑似音效 |

---

## 九、补丁策略备忘

### 优先级排序

1. **鼠标限制（二）** — 鼠标向量 + 鼠标中心 + 更新限制，解决点击偏移
2. **裁剪边界（三）** — 解决"UI 被居中裁剪"问题
3. **视口尺寸（6.6）** — 游戏世界渲染范围
4. **UI 组件坐标（六）** — 状态栏 / 提示框 / Buff 图标 等
5. **特殊关键点 0x93BEA8** — 硬编码渲染分辨率
6. **居中计算（四）** — 精细 UI 定位
7. **分辨率检查分支（五）** — 极端情况

### 指令模式补丁方案

| 指令模式 | 补丁方式 | 示例 |
|---------|---------|------|
| `mov eax/edx/edi, IMM32` (5字节) | `0xB8/0xBA/0xBF + new_value` | 0x81C804, 0x81C817 |
| `cmp reg, IMM32; jle` (6+字节) | 改写 IMM32 部分 | 0x4D88AF, 0x4E33A0 |
| `push IMM32` (5字节) | `0x68 + new_value` | 0x93BEA8 |
| 数据区 `dd` (4字节) | 直接写新值 | 0xBD1788, 0xBD35E0 |
| `jl short` (2字节) | `EB` (jmp) 替代 `7C` (jl) | 0x5CB688 |

### 鼠标系统补丁公式汇总

```
width  = 1366
height = 768

0x005CA9AC: 写 -height/2 = -384  (有符号 dword: 0xFFFFFE80)
0x005CA9B8: 写 -width/2  = -683  (有符号 dword: 0xFFFFFD55)

0x005CA787: 写 floor(width/2)  = 683 (0x2AB)
0x005CA78D: 写 floor(height/2) = 384 (0x180)

0x005CB0F1: 写 width  = 1366 (0x556)
0x005CB10A: 写 height = 768  (0x300)

0x005CB67E: 写 width  = 1366 (0x556)
0x005CB697: 写 height = 768  (0x300)
```

### 注意

1. 鼠标向量是**有符号立即数**，注意处理负数的字节编码
2. 鼠标中心点 `0x005CA787` / `0x005CA78D` 是在 `CInputSystem::LoadCursorState` 内的代码地址（指令立即数），不是数据段
3. BD1788/BD178C/BD35E0/BD35E4 是数据段地址（直接写 dword），不是代码段
