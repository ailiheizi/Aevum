#!/usr/bin/env bash
# ADR-0006 阶段2(收口版):bootroot 完全来自 Aevum 世代,无任何旁路。
#
# 收口前:install 只装包文件,libc 要旁路 export-rootfs 补。
# 收口后:install 已把运行闭包(libc+loader)补进世代,故 bootroot 完全自包含:
#   只需 `aevum export-bootroot <gen>`(引擎从世代 store 对象产出全部内容),
#   脚本只补一个 busybox(做 init/shell,世代里 hello 是动态包不含 shell)+ 组 initramfs。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export AEVUM_ROOT="${AEVUM_ROOT:-$REPO_ROOT/.aevum}"
GEN="${1:-50}"
BUILD="$AEVUM_ROOT/boot2-build"
IRD="$BUILD/initramfs"
ROOT="$BUILD/bootroot"
BOOTROOT_SRC="$AEVUM_ROOT/bootroot-$GEN"
BB="$AEVUM_ROOT/unpacked/busybox-static/usr/bin/busybox"
TARGET="${CARGO_TARGET_DIR:-/tmp/aevum-target}"

run_aevum() { ( cd "$REPO_ROOT" && . "$HOME/.cargo/env" 2>/dev/null; CARGO_TARGET_DIR="$TARGET" cargo run -q -p aevum-cli -- "$@" ); }

echo "=== 1. 引擎产出 bootroot(全部内容来自世代 $GEN,含 libc+loader) ==="
run_aevum export-bootroot "$GEN" >/dev/null
[ -f "$BOOTROOT_SRC/usr/lib/libc.so.6" ] || { echo "FATAL: 世代未自带 libc(install 补闭包未生效?)"; exit 1; }

echo "=== 2. 组可作根的 bootroot(引擎产物 + busybox做init) ==="
rm -rf "$ROOT"; mkdir -p "$ROOT"/{bin,lib,lib64,sbin,proc,sys,dev,tmp}
# 世代内容(引擎产):usr/bin/* → /bin、usr/lib/* → /lib、loader软链
[ -d "$BOOTROOT_SRC/usr/bin" ] && cp -a "$BOOTROOT_SRC/usr/bin/." "$ROOT/bin/"
[ -d "$BOOTROOT_SRC/usr/lib" ] && cp -a "$BOOTROOT_SRC/usr/lib/." "$ROOT/lib/"
ln -sf "/lib/ld-linux-x86-64.so.2" "$ROOT/lib64/ld-linux-x86-64.so.2"
cp "$BOOTROOT_SRC/AEVUM_GENERATION_ROOT" "$ROOT/AEVUM_GENERATION_ROOT"
# busybox 做 init/shell(世代里 hello 是应用,不含 shell;busybox 是平台兜底工具)
cp "$BB" "$ROOT/bin/busybox"; chmod 755 "$ROOT/bin/busybox"
chmod -R 755 "$ROOT/bin" "$ROOT/lib" 2>/dev/null || true

cat > "$ROOT/sbin/init" <<'GINIT'
#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc none /proc 2>/dev/null; mount -t sysfs none /sys 2>/dev/null
echo ""; echo "==================================================="
echo " Aevum 阶段2(收口): 世代完全自包含,bootroot 无旁路"
echo "==================================================="
echo "[gen-init] 根标志: $(cat /AEVUM_GENERATION_ROOT | head -1)"
echo "[gen-init] / 布局(全来自世代): $(ls / | tr '\n' ' ')"
echo "[gen-init] /lib 内容(世代自带的 libc+loader): $(ls /lib | tr '\n' ' ')"
echo "[gen-init] hello 用【世代自带】libc 跑(非旁路补):"
echo "---------------------------------------------------"
/bin/hello
echo "---------------------------------------------------"
echo "[gen-init] 世代自包含 = 系统根。install→世代→引导 全链由引擎驱动。"
echo "[gen-init] 落入 shell:"; exec /bin/busybox sh
GINIT
chmod 755 "$ROOT/sbin/init"

echo "=== 3. 组 switch_root initramfs ==="
rm -rf "$IRD"; mkdir -p "$IRD"/{bin,proc,sys,dev,mnt}
cp "$BB" "$IRD/bin/busybox"; chmod 755 "$IRD/bin/busybox"
cp -a "$ROOT" "$IRD/mnt/gen"
cat > "$IRD/init" <<'INIT'
#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc none /proc 2>/dev/null; mount -t sysfs none /sys 2>/dev/null
echo "[initramfs] switch_root 到 Aevum 世代根 ..."
mkdir -p /newroot; mount -t tmpfs none /newroot
cp -a /mnt/gen/. /newroot/
exec switch_root /newroot /sbin/init
echo "[initramfs] FATAL"; exec /bin/busybox sh
INIT
chmod 755 "$IRD/init"
( cd "$IRD" && find . | cpio -o -H newc 2>/dev/null | gzip -9 ) > "$BUILD/initramfs.cpio.gz"
echo "OK: $BUILD/initramfs.cpio.gz ($(du -h "$BUILD/initramfs.cpio.gz" | cut -f1))"
echo "bootroot 来源: aevum export-bootroot $GEN(全部内容,含 libc+loader,无旁路)"
