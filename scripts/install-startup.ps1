# 安装 / 卸载开机自启
# 用法: powershell -ExecutionPolicy Bypass -File .\install-startup.ps1 -Install | -Uninstall
param([switch]$Install, [switch]$Uninstall)

$startup = [Environment]::GetFolderPath('Startup')
$linkName = "ClaudeUsageWidget.vbs"
$linkPath = Join-Path $startup $linkName
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

# 选择要启动的 exe：release 优先，回退到 dev 模式 bat
$releaseExe = Join-Path $scriptDir "..\src-tauri\target\release\claude-usage-widget.exe"
$devBat     = Join-Path $scriptDir "start-widget.bat"

if (Test-Path $releaseExe) {
    # 直接跑 release exe（最干净，没有黑窗口）
    $target = $releaseExe
} else {
    Write-Warning "未找到 release 版 exe，回退到 dev 模式（首次 cargo build 慢）"
    $target = $devBat
}

if ($Install) {
    # 用 WScript.Shell 启动 exe（隐藏窗口）
    $vb = Join-Path $scriptDir "hide-window-exe.vbs"

    # 生成启动脚本（vbs 调用 exe，不阻塞）
    @"
CreateObject("Wscript.Shell").Run """" & WScript.Arguments(0) & """", 0, False
"@ | Out-File -Encoding ASCII $vb

    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($linkPath)
    $sc.TargetPath = "wscript.exe"
    $sc.Arguments  = "`"$vb`" `"$target`""
    $sc.WorkingDirectory = Split-Path -Parent $releaseExe
    $sc.Save()

    Write-Host "✓ 已创建启动项：" -ForegroundColor Green
    Write-Host "  $linkPath" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "目标: $target" -ForegroundColor DarkCyan
    Write-Host ""
    Write-Host "提示：登录后悬浮窗会自动后台运行。" -ForegroundColor Yellow
    Write-Host "  注意：电脑登录后第一次启动前请先运行" -ForegroundColor Yellow
    Write-Host "         setx MINIMAX_API_KEY 'eyJhbGciOi...'" -ForegroundColor Yellow
    Write-Host "         否则悬浮窗会显示 'Missing API key'" -ForegroundColor Yellow
}
elseif ($Uninstall) {
    if (Test-Path $linkPath) {
        Remove-Item $linkPath -Force
        Write-Host "✓ 删除 $linkPath" -ForegroundColor Green
    }
    $vb = Join-Path $scriptDir "hide-window-exe.vbs"
    if (Test-Path $vb) { Remove-Item $vb -Force }
}
else {
    Write-Host "用法: powershell -ExecutionPolicy Bypass -File .\install-startup.ps1 -Install | -Uninstall"
}
