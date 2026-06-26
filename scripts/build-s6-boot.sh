#!/usr/bin/env bash
# ADR-0006 阶段4a:s6 接管 PID1(最小验证)。
#
# 目标(设计见 docs/architecture/bootable/04-init-services-config.md §6):
#   把 busybox-as-PID1 换成 s6-svscan 监督树,QEMU 验证:
#     1. s6-svscan 作为世代 init(switch_root 后的 PID1)起来;
#     2. 它从 scandir 拉起一个 demo longrun 服务(每秒打印);
#     3. 监督树活着(s6-svscan reap 子进程、demo 服务被 s6-supervise 监督)。
#
# 路线甲(已与用户确认):s6 来自 Debian 包(gen-60 已 install s6+execline 闭包),
#   动态链接、用世代自带 libc+loader 跑。静态化(musl)留作 4a 后待办。
#
# 复用阶段3 的 FAT+syslinux 引导骨架,但 init 逻辑换成 s6。普通用户跑(要 cargo)。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export AEVUM_ROOT="${AEVUM_ROOT:-$REPO_ROOT/.aevum}"
BUILD="$AEVUM_ROOT/boot4a-build"
IMG="$BUILD/aevum-s6.img"
VMLINUZ="$AEVUM_ROOT/boot-build/vmlinuz"
BB="$AEVUM_ROOT/unpacked/busybox-static/usr/bin/busybox"
GEN="${1:-60}"                                  # s6 世代(默认 gen-60)
TARGET="${CARGO_TARGET_DIR:-/tmp/aevum-target}"

run_aevum() { ( cd "$REPO_ROOT" && . "$HOME/.cargo/env" 2>/dev/null; CARGO_TARGET_DIR="$TARGET" cargo run -q -p aevum-cli -- "$@" ); }

echo "=== 前提 ==="
[ -f "$VMLINUZ" ] || { echo "FATAL: 无内核 $VMLINUZ"; exit 1; }
[ -f "$BB" ] || { echo "FATAL: 无 busybox $BB"; exit 1; }
command -v mcopy >/dev/null && command -v syslinux >/dev/null || { echo "FATAL: 缺 mtools/syslinux"; exit 1; }

rm -rf "$BUILD"; mkdir -p "$BUILD/stage"

# ---------- 1. 引擎产 s6 世代的 bootroot ----------
echo "=== 1. export-bootroot gen-$GEN(引擎产 s6 世代根)==="
run_aevum export-bootroot "$GEN" >/dev/null
SRC="$AEVUM_ROOT/bootroot-$GEN"
[ -d "$SRC" ] || { echo "FATAL: 无 bootroot-$GEN"; exit 1; }

# ---------- 2. 组世代根:s6 二进制 + libc/loader + scandir + demo 服务 ----------
echo "=== 2. 组 s6 世代根 + scandir + demo 服务 ==="
ROOT="$BUILD/root"
rm -rf "$ROOT"; mkdir -p "$ROOT"/{bin,sbin,usr/bin,usr/lib,lib,lib64,proc,sys,dev,tmp,run,etc/s6/scandir}
# 引擎产物(s6 二进制在 usr/bin,库在 usr/lib/...):整体搬进世代根,保留布局。
cp -a "$SRC/usr/." "$ROOT/usr/" 2>/dev/null || true
# loader 软链:世代自带(export-bootroot 已放 usr/lib64/ld-linux)。补 /lib64 常规位置。
[ -e "$ROOT/usr/lib64/ld-linux-x86-64.so.2" ] && cp -a "$SRC/usr/lib64/." "$ROOT/lib64/" 2>/dev/null || true
# busybox 作平台兜底工具(min-toolset 占位:mount/echo/sleep 等),装进 /bin。
cp "$BB" "$ROOT/bin/busybox"; chmod 755 "$ROOT/bin/busybox"
cp "$SRC/AEVUM_GENERATION_ROOT" "$ROOT/AEVUM_GENERATION_ROOT"

# s6 二进制目录加进 PATH 用的符号:把 usr/bin 的 s6-* 也软链到 /bin 方便 init 调。
# (s6 工具间靠 PATH 找彼此,svscan 会 exec s6-supervise。)
ln -sf /usr/bin/s6-svscan   "$ROOT/bin/s6-svscan"
ln -sf /usr/bin/s6-supervise "$ROOT/bin/s6-supervise"
ln -sf /usr/bin/s6-svscanctl "$ROOT/bin/s6-svscanctl"
ln -sf /usr/bin/execlineb    "$ROOT/bin/execlineb"

# library-path:s6 动态链接 libskarnet(在 multiarch 目录)+ libc。
# 用 /etc/ld-aevum.path 记录,init 里 export LD_LIBRARY_PATH。
S6_LIBS="/usr/lib/x86_64-linux-gnu:/usr/lib"

# 注:soname 软链(libX.so.A → libX.so.A.B.C)由引擎 export-bootroot 保留(CHANGELOG 第28轮修复
# PoC-5 铁律违反:此前软链被跳过/解引用)。脚本不再需要 workaround 补链。

# overlayfs 内核模块(4c):该内核 overlay 是模块非内建,需 insmod 才能挂 /etc overlay。
# 从 Aevum 装的 linux-image 包取 overlay.ko(.xz 解压),放进世代根 /lib/modules。
echo "  [4c] 准备 overlayfs 内核模块 ..."
KMOD_SRC=$(find "$AEVUM_ROOT/unpacked" -path "*kernel/fs/overlayfs/overlay.ko*" 2>/dev/null | head -1)
if [ -n "$KMOD_SRC" ]; then
  mkdir -p "$ROOT/lib/modules"
  case "$KMOD_SRC" in
    *.xz) xz -dc "$KMOD_SRC" > "$ROOT/lib/modules/overlay.ko" ;;
    *)    cp "$KMOD_SRC" "$ROOT/lib/modules/overlay.ko" ;;
  esac
  echo "    overlay.ko → /lib/modules/overlay.ko ($(du -h "$ROOT/lib/modules/overlay.ko" | cut -f1))"
else
  echo "    ⚠ 未找到 overlay.ko(overlay 将降级)"
fi

# demo 服务:由引擎 service-compiler 从 TOML 声明编译(阶段4b)。
# 不再脚本手写 run here-doc —— 走 examples/services/demo.toml → aevum service compile。
# 注:/etc 走 overlay(阶段4c),基底放 /etc-lower(只读 lower),含 s6 scandir + 声明生成的配置。
ETC_LOWER="$ROOT/etc-lower"
mkdir -p "$ETC_LOWER/s6/scandir"
echo "  [4b] 从 TOML 声明编译 demo 服务(引擎 aevum service compile)..."
run_aevum service compile "$REPO_ROOT/examples/services/demo.toml" \
  --scandir "$ETC_LOWER/s6/scandir" --lib-path "$S6_LIBS"

# /etc 系统配置基底:引擎 etc-builder 从 TOML 声明生成(阶段4c)。
echo "  [4c] 从 TOML 声明生成 /etc 基底(引擎 aevum etc build)..."
run_aevum etc build "$REPO_ROOT/examples/etc/system.toml" --out "$ETC_LOWER"

# 世代 init:挂伪文件系统 → 挂 /etc overlay → 打印标志 → exec s6-svscan 监督 scandir。
cat > "$ROOT/sbin/init" <<GINIT
#!/bin/busybox sh
/bin/busybox --install -s /bin
export LD_LIBRARY_PATH=$S6_LIBS
mount -t proc none /proc 2>/dev/null
mount -t sysfs none /sys 2>/dev/null
mount -t tmpfs none /run 2>/dev/null
# 阶段4c:/etc = overlayfs(lower=世代生成的只读基底 /etc-lower,upper=可变持久层)。
# 基底随世代回退;本地写入落 upper(此处 upper 用 tmpfs 演示;真部署用持久卷,按 runtime/04 处理)。
mkdir -p /etc /run/etc-rw/upper /run/etc-rw/work
# 加载 overlayfs 模块(该内核 overlay 是模块,需 insmod)。
[ -f /lib/modules/overlay.ko ] && /bin/busybox insmod /lib/modules/overlay.ko 2>/dev/null
echo "[init] overlay 支持自检: \$(grep -c overlay /proc/filesystems) (来自 /proc/filesystems)"
ovl_err=\$(mount -t overlay overlay -o lowerdir=/etc-lower,upperdir=/run/etc-rw/upper,workdir=/run/etc-rw/work /etc 2>&1)
if [ -z "\$ovl_err" ]; then
  echo "[init] /etc = overlay(lower=/etc-lower 只读基底 + upper 可变层)"
else
  echo "[init] ⚠ overlay 挂载失败: \$ovl_err"
  echo "[init]   降级:直接铺基底(本地修改不隔离)"
  cp -a /etc-lower/. /etc/ 2>/dev/null
fi
echo ""
echo "=== Aevum 阶段4a/4b/4c:s6 接管 PID1 + 声明式服务 + /etc 基底 (gen-$GEN) ==="
cat /AEVUM_GENERATION_ROOT | head -1
echo "[init] /etc 基底来自声明(hostname=\$(cat /etc/hostname 2>/dev/null)):"
echo "  /etc/hostname = \$(cat /etc/hostname 2>/dev/null)"
echo "  /etc/motd     = \$(cat /etc/motd 2>/dev/null | head -1)"
echo "[init] 启动 s6-svscan 监督树,scandir=/etc/s6/scandir"
# 后台验证:5 秒后看服务状态 + /etc overlay 可写性。
{
  /bin/busybox sleep 6
  echo ""
  echo "[verify] s6-svstat demo 服务状态:"
  /usr/bin/s6-svstat /etc/s6/scandir/demo 2>&1 || echo "  (s6-svstat 失败)"
  echo "[verify] /etc overlay 可写性测试(写 /etc/local-test 应落 upper,不污染只读基底):"
  echo "本地运行时写入" > /etc/local-test 2>&1 && echo "  写 /etc/local-test OK: \$(cat /etc/local-test)" || echo "  写失败"
  echo "  基底 /etc-lower/local-test 应不存在: \$(ls /etc-lower/local-test 2>&1)"
  echo "[verify] 阶段4a/b/c 验证点:s6=PID1、声明服务 up、/etc 基底来自声明且可叠加本地写入。"
} &
# s6-svscan 成为 PID1 的前台监督进程(exec 替换 init)。
exec /usr/bin/s6-svscan /etc/s6/scandir
GINIT
chmod 755 "$ROOT/sbin/init"

# ---------- 3. switch_root initramfs ----------
echo "=== 3. 造 switch_root initramfs ==="
IRD="$BUILD/ird"
rm -rf "$IRD"; mkdir -p "$IRD"/{bin,proc,sys,dev,mnt}
cp "$BB" "$IRD/bin/busybox"; chmod 755 "$IRD/bin/busybox"
cp -a "$ROOT" "$IRD/mnt/gen"
cat > "$IRD/init" <<'INIT'
#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t proc none /proc 2>/dev/null
mkdir -p /newroot; mount -t tmpfs none /newroot; cp -a /mnt/gen/. /newroot/
exec switch_root /newroot /sbin/init
INIT
chmod 755 "$IRD/init"
( cd "$IRD" && find . | cpio -o -H newc 2>/dev/null | gzip -9 ) > "$BUILD/stage/initrd-s6.gz"
echo "  initramfs: $(du -h "$BUILD/stage/initrd-s6.gz" | cut -f1)"

# ---------- 4. syslinux.cfg(单项,直接引导 s6 世代)----------
echo "=== 4. 渲染引导菜单(引擎 boot-menu,单世代)==="
cp "$VMLINUZ" "$BUILD/stage/vmlinuz"
cp /usr/lib/syslinux/modules/bios/{menu.c32,libutil.c32} "$BUILD/stage/" 2>/dev/null || true
# 直接写一个单项 cfg(initrd 名是 initrd-s6.gz,非 boot-menu 默认的 initrd-<gen>.gz)。
cat > "$BUILD/stage/syslinux.cfg" <<CFG
DEFAULT s6
PROMPT 0
TIMEOUT 10
UI menu.c32
MENU TITLE Aevum 阶段4a - s6 接管 PID1

LABEL s6
  MENU LABEL Aevum gen-$GEN (s6-svscan as PID1)
  KERNEL /vmlinuz
  INITRD /initrd-s6.gz
  APPEND console=ttyS0 rdinit=/init panic=1
CFG
echo "--- syslinux.cfg ---"; cat "$BUILD/stage/syslinux.cfg"

# ---------- 5. 造 FAT 镜像 ----------
echo "=== 5. 造 FAT 可引导镜像 ==="
SIZE_MB=96
dd if=/dev/zero of="$IMG" bs=1M count=$SIZE_MB status=none
mkfs.fat -F 32 -n AEVUMS6 "$IMG" >/dev/null
syslinux --install "$IMG"
for f in "$BUILD"/stage/*; do mcopy -D o -i "$IMG" "$f" ::/; done
echo "OK: $IMG ($SIZE_MB MB)"
echo "QEMU: qemu-system-x86_64 -drive file=$IMG,format=raw -nographic -m 512"
