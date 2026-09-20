# 驱动真实 TUI 走一遍"选择屏 → 管理器 → F4 → 输密码 → 连上"。
#
# 判定用的是 **TCP 连接数** (Get-NetTCPConnection 看该进程的 Established),
# 不是读像素 —— 那是最能说明"到底连上了没有"的信号。
#
# 前提: 本地游戏服在 127.0.0.1:8484 上跑着。
# 用法: pwsh -File scripts/drive-tui.ps1 [-KeepOpen]
param([switch]$KeepOpen)

$ErrorActionPreference = 'Continue'
Write-Host '[trace] compiling WinDrive'
Add-Type -TypeDefinition (Get-Content (Join-Path $PSScriptRoot '..\test-support\WinDrive.cs') -Raw)
Write-Host '[trace] WinDrive ok'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$exe = Join-Path $root 'target\debug\openstory-console.exe'
if (-not (Test-Path $exe)) { throw '先构建: cargo build -p openstory-console' }

$fail = 0
function Check([string]$name, [bool]$ok) {
  Write-Output ("{0} {1}" -f $(if ($ok) { '[ok]' } else { '[!!]' }), $name)
  if (-not $ok) { $script:fail++ }
  return $ok
}
function Conns($procId) {
  return @(Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue |
    Where-Object { $_.OwningProcess -eq $procId })
}

# 直接启动, **不重定向 stdout** —— crossterm 需要真控制台
$p = Start-Process -FilePath $exe -ArgumentList '--no-remember-password' `
  -WorkingDirectory $root -PassThru
Start-Sleep -Seconds 4
$p.Refresh()
$h = $p.MainWindowHandle
if ($h -eq 0) { throw '拿不到窗口句柄' }

[void][WinDrive]::Focus($h)
Start-Sleep -Milliseconds 1000
if (-not (Check '窗口已在前台 (按键才会发对地方)' ([WinDrive]::IsFg($h)))) {
  Write-Output '前台抢不到 —— 按键会发到别的窗口, 结果不可信, 中止'
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  # 不 `exit`: 那会连已经写出的检查点一起丢掉 (实测: 只剩最后一行)。
  # 让脚本自然结束。
  return
}
Check '窗口可见且没最小化' ([WinDrive]::Ready($h)) | Out-Null
Check '启动时没有任何连接 (手动模式)' ((Conns $p.Id).Count -eq 0) | Out-Null

# 1. 选择屏: 走到第 7 项 (本地服_100000001) 并勾选
[WinDrive]::Key(0x28, 6)      # Down × 6
[WinDrive]::Key(0x20, 1)      # Space 勾选
Start-Sleep -Milliseconds 400

# 2. Enter 进入控制台
[WinDrive]::Key(0x0D, 1)
Start-Sleep -Seconds 3
Check '进入后仍然没有连接 (还没按 F4)' ((Conns $p.Id).Count -eq 0) | Out-Null

# 3. F4 → 缺密码 → 弹登录向导 → 输密码 + Enter → 应连上
[WinDrive]::Key(0x73, 1)      # F4
Start-Sleep -Seconds 2
[WinDrive]::Text('test_password')
Start-Sleep -Milliseconds 800
[WinDrive]::Key(0x0D, 1)      # Enter

$connected = $false
for ($i = 0; $i -lt 20; $i++) {
  Start-Sleep -Seconds 1
  if ((Conns $p.Id).Count -ge 1) { $connected = $true; break }
}
Check "F4 → 输密码 → Enter 之后建立了连接 (等了 $($i+1)s, 共 $((Conns $p.Id).Count) 条)" $connected | Out-Null

if ($KeepOpen) { Write-Output '保留运行中' }
else { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
Write-Output "失败项: $fail"
