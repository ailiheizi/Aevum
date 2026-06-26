#!/usr/bin/env bash
# 阶段1:组装最小可引导 initramfs + QEMU 引导。
#
# 证明:Aevum 世代/闭包能作为一个真实被内核引导的系统的内容。
# initramfs 里:
#   - busybox(Aevum 装的,静态)做 /init 与兜底 shell
#   - hello 闭包(Aevum 装的:hello + libc + loader)
#   - /init:挂 proc/sys → 打印标志 → loader 注入跑 Aevum 的 hello → 落 busybox shell
#
# 内核(vmlinuz)由 Aevum install linux-image 提供(吃狗粮)。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export AEVUM_ROOT="${AEVUM_ROOT:-$REPO_ROOT/.aevum}"
BUILD="$AEVUM_ROOT/boot-build"
IRD="$BUILD/initramfs"          # initramfs 根
BB="$AEVUM_ROOT/unpacked/busybox-static/usr/bin/busybox"
HELLO_ROOTFS="$AEVUM_ROOT/rootfs-hello"

echo "=== 前提 ==="
[ -x "$BB" ] || { echo "FATAL: 无 busybox(先 aevum install busybox-static)"; exit 1; }
[ -d "$HELLO_ROOTFS" ] || { echo "FATAL: 无 hello rootfs(先 aevum export-rootfs hello)"; exit 1; }

echo "=== 组装 initramfs 根 ==="
rm -rf "$IRD"; mkdir -p "$IRD"/{bin,lib,proc,sys,dev,aevum}
# busybox + 安装其 applet 软链(sh/mount/echo...)
cp "$BB" "$IRD/bin/busybox"
chmod 755 "$IRD/bin/busybox"
# Aevum 装的 hello 闭包整体放进 /aevum(证明系统内容来自 Aevum store)
cp -r "$HELLO_ROOTFS/." "$IRD/aevum/"

# /init:进程1。busybox 提供基础命令,Aevum 提供"装的软件"。
cat > "$IRD/init" <<'INIT'
#!/bin/busybox sh
/bin/busybox --install -s /bin    # 铺开 applet 软链(sh/mount/echo/cat...)
mount -t proc none /proc 2>/dev/null
mount -t sysfs none /sys 2>/dev/null
echo ""
echo "==================================================="
echo " Aevum bootable 阶段1: 内核已引导,init(进程1)接管"
echo "==================================================="
echo "[init] busybox(Aevum 装)做兜底 shell"
echo "[init] 运行 Aevum store 里的 hello(loader 注入,PoC-2):"
echo "---------------------------------------------------"
# 跑 Aevum 装的 hello:显式 loader + --library-path,内容全来自 Aevum 闭包
/aevum/lib/ld-linux-x86-64.so.2 --library-path /aevum/lib /aevum/bin/hello
echo "---------------------------------------------------"
echo "[init] ↑ 这行 Hello 来自 Aevum 世代的闭包,不是内核/initramfs 自带"
echo "[init] Aevum 世代成功作为系统内容被引导运行。阶段1 达成。"
echo ""
echo "[init] 落入 busybox shell(Ctrl-A X 退出 QEMU):"
exec /bin/busybox sh
INIT
chmod 755 "$IRD/init"

echo "=== 打包 initramfs(cpio.gz)==="
( cd "$IRD" && find . | cpio -o -H newc 2>/dev/null | gzip -9 ) > "$BUILD/initramfs.cpio.gz"
echo "OK: $BUILD/initramfs.cpio.gz ($(du -h "$BUILD/initramfs.cpio.gz" | cut -f1))"
echo "initramfs 内容:"
find "$IRD" -maxdepth 2 | sed "s|$IRD|  |" | head -20
