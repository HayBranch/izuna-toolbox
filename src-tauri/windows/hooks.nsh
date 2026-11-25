; 泉奈工具箱 · NSIS 安装器钩子
;
; 为什么需要这个钩子：
; 应用内更新用 `/UPDATE /P` 启动安装器——被动模式只显示进度条页并自动关闭，
; 会跳过带「运行程序」勾选框的完成页，所以安装器不会自己拉起新版程序。
; v0.1.2 及以前靠应用退出前挂的 PowerShell 看门进程补这一步，但看门进程只能
; 靠「旧进程退出 + exe 能打开 + 写入时间稳定」猜安装是否结束：安装器还没开始
; 写文件时 exe 就已经满足条件，抢先启动的旧实例会被安装器的 RestartManager
; 强制关掉，结果装完什么都不剩（用户看到的就是「更新后不会自动启动」）。
;
; 这里把「启动新版程序」放进安装器自己的 POSTINSTALL 阶段：此时 exe、资源、
; 注册表、快捷方式都已写完，是整条链路里最确定的时机，不依赖任何外部轮询。
;
; 只在 `/UPDATE`（更新模式，即应用内更新）下自动重启；首次安装仍交给安装向导
; 完成页的「运行」勾选框，避免和它重复启动。
!macro NSIS_HOOK_POSTINSTALL
  ${If} $UpdateMode = 1
    DetailPrint "更新完成，正在重新启动 ${PRODUCTNAME}"
    nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
  ${EndIf}
!macroend
