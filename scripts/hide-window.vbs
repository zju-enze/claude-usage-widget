' 隐藏 bat 窗口运行指定脚本
CreateObject("Wscript.Shell").Run """" & WScript.Arguments(0) & """", 0, False
