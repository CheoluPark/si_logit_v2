; SI Logit NSIS installer hooks.
; Tauri inserts each defined macro into the corresponding generated install or
; uninstall section (run via !ifmacrodef so missing hooks are silently skipped).

; 安装写文件之前：把在跑的 hindsight.exe 杀掉。Hindsight 是托盘常驻应用，
; 覆盖安装（升级）时旧进程握着 hindsight.exe 的文件锁，NSIS 写入会报
; "Error opening file for writing"。Tauri 模板自带的"关闭运行中应用"检测
; 对托盘应用不可靠，这里强杀兜底。
!macro NSIS_HOOK_PREINSTALL
    DetailPrint "Stopping hindsight.exe before install..."
    nsExec::Exec '"taskkill" /F /IM hindsight.exe /T'
    Sleep 1000

    ; ── 迁移清理：历史版本 installMode 是 "both"，可能存在“所有用户”(per-machine,
    ; Program Files) 安装与本 per-user 安装并存。两份并存时更新器只更新其一、
    ; 快捷方式却可能指向另一份——用户“更新完重开还是旧版本”。这里发现 HKLM
    ; 残留就静默运行它的卸载器（需要一次 UAC 确认；拒绝则跳过并提示手动卸载）。
    ; 卸载器的“是否删除用户数据”弹窗带 /SD IDNO，静默模式自动选“否”；
    ; 数据库/截图不在安装目录，绝不受影响。
    ClearErrors
    ReadRegStr $0 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\hindsight" "UninstallString"
    IfErrors machine_copy_done
    StrCmp $0 "" machine_copy_done
    DetailPrint "Removing legacy all-users installation..."
    ; UninstallString 形如 "C:\Program Files\hindsight\uninstall.exe"（带引号），
    ; ShellExecute 的 file 参数不吃引号，剥掉首尾各一个字符
    StrCpy $1 $0 "" 1
    StrCpy $1 $1 -1
    ClearErrors
    ExecShellWait "runas" "$1" "/S"
    IfErrors 0 +2
    DetailPrint "Legacy copy NOT removed (elevation declined). Please uninstall the old 'hindsight' under Program Files manually."
    ; 卸载器会自我复制到临时目录后立即返回，留 2s 让它删完全机快捷方式，
    ; 再让本安装继续写当前用户快捷方式，避免先写后删的交错
    Sleep 2000
    machine_copy_done:
!macroend

; 卸载主流程开始前：把 hindsight.exe 杀掉，避免它握着安装目录里的可执行文件
; 或用户数据目录里的 SQLite/截图，导致后续 RMDir 删不干净。
; nsExec::Exec 是静默执行（无黑窗一闪），taskkill /F /T 强杀含子进程。
!macro NSIS_HOOK_PREUNINSTALL
    DetailPrint "Stopping hindsight.exe before uninstall..."
    nsExec::Exec '"taskkill" /F /IM hindsight.exe /T'
    Sleep 1000
    SetShellVarContext current
    Delete "$SMPROGRAMS\SI Logit\SI Logit.lnk"
    RMDir "$SMPROGRAMS\SI Logit"
!macroend

; 安装完成后把安装器旁的离线资源种入应用解析出的 data_root。
; 目标路径只由 bootstrap::data_root() 决定；NSIS 不推断用户目录。
!macro NSIS_HOOK_POSTINSTALL
    SetShellVarContext current
    CreateDirectory "$SMPROGRAMS\SI Logit"
    Delete "$SMPROGRAMS\SI Logit\SI Logit.lnk"
    CreateShortCut "$SMPROGRAMS\SI Logit\SI Logit.lnk" "$INSTDIR\hindsight.exe"
    DetailPrint "Created per-user Start Menu shortcut: SI Logit\SI Logit.lnk"

    IfFileExists "$EXEDIR\offline-assets\ai\*.*" postinstall_seed postinstall_no_assets
    postinstall_seed:
        DetailPrint "Seeding offline AI assets from $EXEDIR\offline-assets..."
        nsExec::ExecToLog '"$INSTDIR\hindsight.exe" --seed-resources "$EXEDIR\offline-assets"'
        Pop $0
        StrCmp $0 0 postinstall_seed_done
        DetailPrint "Offline asset seeding returned exit code $0; existing installation was left intact."
        Goto postinstall_seed_done
    postinstall_no_assets:
        DetailPrint "No adjacent offline-assets\ai folder found; installation will continue without offline AI assets."
    postinstall_seed_done:
!macroend
