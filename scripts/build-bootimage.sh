#!/usr/bin/env bash
# ADR-0006 阶段3:bootloader 多世代菜单(开机选世代 + 回滚)。
#
# 造一个 FAT 可引导磁盘镜像,装 syslinux,extlinux.conf 列出多个可引导世代:
#   每个世代一个菜单项 → 各自的 initramfs(switch_root 到该世代根)→ 共享内核。
#   DEFAULT = active 世代。选不同项 = 启不同世代 = 回滚机制。
#
# 用 mtools(免 loop 挂载)往 FAT 塞文件;syslinux 装引导扇区。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export AEVUM_ROOT="${AEVUM_ROOT:-$REPO_ROOT/.aevum}"
BUILD="$AEVUM_ROOT/boot3-build"
IMG="$BUILD/aevum-disk.img"
VMLINUZ="$AEVUM_ROOT/boot-build/vmlinuz"      # 共享内核(Aevum install 的)
BB="$AEVUM_ROOT/unpacked/busybox-static/usr/bin/busybox"
TARGET="${CARGO_TARGET_DIR:-/tmp/aevum-target}"
GENS=("$@"); [ ${#GENS[@]} -eq 0 ] && GENS=(50 51)   # 默认 gen-50,51
ACTIVE="${GENS[0]}"                                    # 第一个作 DEFAULT(active)

run_aevum() { ( cd "$REPO_ROOT" && . "$HOME/.cargo/env" 2>/dev/null; CARGO_TARGET_DIR="$TARGET" cargo run -q -p aevum-cli -- "$@" ); }

echo "=== 前提 ==="
[ -f "$VMLINUZ" ] || { echo "FATAL: 无内核 $VMLINUZ"; exit 1; }
command -v mcopy >/dev/null && command -v syslinux >/dev/null || { echo "FATAL: 缺 mtools/syslinux"; exit 1; }

rm -rf "$BUILD"; mkdir -p "$BUILD/stage"

# ---------- 1. 为每个世代造 initramfs(引擎产 bootroot + switch_root)----------
build_gen_initramfs() {
  local gen="$1"
  local ird="$BUILD/ird-$gen" root="$BUILD/root-$gen" src
  echo "  [gen-$gen] export-bootroot(引擎产) ..."
  run_aevum export-bootroot "$gen" >/dev/null
  src="$AEVUM_ROOT/bootroot-$gen"
  [ -d "$src" ] || { echo "FATAL: 无 bootroot-$gen"; exit 1; }
  # 组世代根:引擎产物 + busybox(init)
  rm -rf "$root"; mkdir -p "$root"/{bin,lib,lib64,sbin,proc,sys,dev,tmp}
  [ -d "$src/usr/bin" ] && cp -a "$src/usr/bin/." "$root/bin/"
  [ -d "$src/usr/lib" ] && cp -a "$src/usr/lib/." "$root/lib/"
  ln -sf /lib/ld-linux-x86-64.so.2 "$root/lib64/ld-linux-x86-64.so.2" 2>/dev/null || true
  cp "$src/AEVUM_GENERATION_ROOT" "$root/AEVUM_GENERATION_ROOT"
  cp "$BB" "$root/bin/busybox"; chmod 755 "$root/bin/busybox"
  cat > "$root/sbin/init" <<GINIT
#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc none /proc 2>/dev/null; mount -t sysfs none /sys 2>/dev/null
echo ""; echo "=== Aevum 已引导进 gen-$gen ==="
cat /AEVUM_GENERATION_ROOT | head -1
echo "/bin: \$(ls /bin | tr '\n' ' ')"
[ -x /bin/hello ] && { echo "跑 hello:"; /bin/hello; }
echo "[gen-$gen] 这是从 bootloader 菜单选中的世代。"
exec /bin/busybox sh
GINIT
  chmod 755 "$root/sbin/init"
  # switch_root initramfs
  rm -rf "$ird"; mkdir -p "$ird"/{bin,proc,sys,dev,mnt}
  cp "$BB" "$ird/bin/busybox"; chmod 755 "$ird/bin/busybox"
  cp -a "$root" "$ird/mnt/gen"
  cat > "$ird/init" <<'INIT'
#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc none /proc 2>/dev/null
mkdir -p /newroot; mount -t tmpfs none /newroot; cp -a /mnt/gen/. /newroot/
exec switch_root /newroot /sbin/init
INIT
  chmod 755 "$ird/init"
  ( cd "$ird" && find . | cpio -o -H newc 2>/dev/null | gzip -9 ) > "$BUILD/stage/initrd-$gen.gz"
  echo "  [gen-$gen] initramfs: $(du -h "$BUILD/stage/initrd-$gen.gz" | cut -f1)"
}

echo "=== 2. 为各世代造 initramfs ==="
for g in "${GENS[@]}"; do build_gen_initramfs "$g"; done

# ---------- 3. 渲染 syslinux.cfg 多世代菜单(引擎驱动)----------
echo "=== 3. 渲染多世代引导菜单(引擎 aevum boot-menu,DEFAULT=gen-$ACTIVE)==="
cp "$VMLINUZ" "$BUILD/stage/vmlinuz"
cp /usr/lib/syslinux/modules/bios/{menu.c32,libutil.c32} "$BUILD/stage/" 2>/dev/null || true   # ldlinux.c32 由 syslinux --install 自己装(只读),不在此 cp 以免 mcopy 撞名
# 菜单不再用脚本 here-doc 手拼,改由引擎渲染(BootMenu::render):GENS 第一个作 DEFAULT。
gens_csv="$(IFS=,; echo "${GENS[*]}")"
run_aevum boot-menu --gens "$gens_csv" --out "$BUILD/stage/syslinux.cfg"
echo "--- syslinux.cfg(引擎产)---"; cat "$BUILD/stage/syslinux.cfg"

# ---------- 4. 造 FAT 镜像 + 装 syslinux + mcopy 文件 ----------
echo "=== 4. 造 FAT 可引导磁盘镜像 ==="
SIZE_MB=64
dd if=/dev/zero of="$IMG" bs=1M count=$SIZE_MB status=none
mkfs.fat -F 32 -n AEVUMBOOT "$IMG" >/dev/null
syslinux --install "$IMG"
# mcopy 所有 stage 文件进 FAT 根(免 loop)
for f in "$BUILD"/stage/*; do mcopy -D o -i "$IMG" "$f" ::/; done   # -D o: 非交互覆盖(syslinux 已装 ldlinux.c32,避免撞名卡交互提示)
echo "OK: $IMG ($SIZE_MB MB)"
echo "镜像内文件:"; mdir -i "$IMG" :: | grep -E "vmlinuz|initrd|syslinux|c32" || true
echo ""
echo "QEMU 引导: qemu-system-x86_64 -drive file=$IMG,format=raw -nographic -m 512"
