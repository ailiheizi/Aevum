# Aevum VM 启动脚本(PowerShell)
# 在 Windows PowerShell 或 Terminal 里跑本脚本

$QEMU = "$env:USERPROFILE\scoop\apps\qemu\current\qemu-system-x86_64.exe"
$VM_DIR = "D:\windows\code\project\Aevum\vm"

# 先杀残留 QEMU
Get-Process -Name "qemu-system-x86_64" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2

# 启动 VM:WHPX 加速 + 2GB RAM + 串口终端 + SSH 转发
# -nographic = 串口输出到当前终端(你能看到 Linux 启动日志和登录提示)
# root 密码: aevum (cloud-init 设的)
& $QEMU `
  -accel whpx `
  -m 2048 -smp 2 `
  -drive "file=$VM_DIR\debian-13.qcow2,format=qcow2" `
  -drive "file=$VM_DIR\seed.img,format=raw,if=virtio" `
  -netdev "user,id=net0,hostfwd=tcp::2223-:22" `
  -device "virtio-net-pci,netdev=net0" `
  -nographic

# 启动后:
#   - 等 1-2 分钟看到 login: 提示
#   - 用户: root  密码: aevum
#   - 或另开终端: ssh -p 2223 root@localhost (密码 aevum)
#
# 进去后装 Aevum:
#   curl -L <aevum二进制URL> -o /usr/local/bin/aevum && chmod +x /usr/local/bin/aevum
#   (或从 host 共享:QEMU -virtfs 或 scp)
#
# 退出 VM: 输入 poweroff 或按 Ctrl+A 然后 X
