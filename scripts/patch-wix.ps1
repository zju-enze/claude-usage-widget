# Tauri 2 msi 安装器默认会勾上"启动 widget"。我们不要这个行为。
# 在 build 后改 wix\x64\main.wxs，把 WIXUI_EXITDIALOGOPTIONALCHECKBOX 改为 "0"

$wxs = "F:\projects\claude-usage-widget\src-tauri\target\release\wix\x64\main.wxs"
if (-not (Test-Path $wxs)) { Write-Host "wxs not found, skip"; exit 0 }

$content = Get-Content $wxs -Raw
$pattern = 'WIXUI_EXITDIALOGOPTIONALCHECKBOX" Value="1"'
$replace = 'WIXUI_EXITDIALOGOPTIONALCHECKBOX" Value="0"'
if ($content -match [regex]::Escape($pattern)) {
    $content = $content -replace [regex]::Escape($pattern), $replace
    Set-Content -Path $wxs -Value $content -NoNewline
    Write-Host "✓ patched WIXUI_EXITDIALOGOPTIONALCHECKBOX to 0"
} else {
    Write-Host "(no patch needed, value already != 1)"
}
