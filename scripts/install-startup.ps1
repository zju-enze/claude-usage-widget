# 安装 / 卸载开机自启
param([switch]$Install, [switch]$Uninstall)

$startup = [Environment]::GetFolderPath('Startup')
$linkMonitor = Join-Path $startup "ClaudeUsageMonitor.bat"
$linkWidget  = Join-Path $startup "ClaudeUsageWidget.bat"
$scriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Definition
$batMonitor  = Join-Path $scriptDir "start-monitor.bat"
$batWidget   = Join-Path $scriptDir "start-widget.bat"

if ($Install) {
    # 用 VBScript 的最小化窗口方式（避免黑窗口一直闪）
    $ws = New-Object -ComObject WScript.Shell
    $sc1 = $ws.CreateShortcut($linkMonitor)
    $sc1.TargetPath = "wscript.exe"
    $sc1.Arguments  = "`"$(Join-Path $PSScriptRoot "hide-window.vbs")`" `"$batMonitor`""
    $sc1.WorkingDirectory = $scriptDir
    $sc1.Save()

    $sc2 = $ws.CreateShortcut($linkWidget)
    $sc2.TargetPath = "wscript.exe"
    $sc2.Arguments  = "`"$(Join-Path $PSScriptRoot "hide-window.vbs")`" `"$batWidget`""
    $sc2.WorkingDirectory = $scriptDir
    $sc2.Save()

    Write-Host "✓ 已创建启动项：" -ForegroundColor Green
    Write-Host "  $linkMonitor" -ForegroundColor Cyan
    Write-Host "  $linkWidget"  -ForegroundColor Cyan
    Write-Host ""
    Write-Host "提示：登录后两个窗口会后台启动。如不想立刻试，可手动双击运行。" -ForegroundColor Yellow
}
elseif ($Uninstall) {
    foreach ($f in @($linkMonitor, $linkWidget)) {
        if (Test-Path $f) {
            Remove-Item $f -Force
            Write-Host "✓ 删除 $f" -ForegroundColor Green
        }
    }
}
else {
    Write-Host "用法: powershell -ExecutionPolicy Bypass -File .\install-startup.ps1 -Install | -Uninstall"
}
