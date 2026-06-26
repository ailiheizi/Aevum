#!/usr/bin/env bash
# 里程碑4:装 imagemagick 的核心 delegate 库(转 PNG/JPEG 必需)。
#
# im 的 libMagickCore 直接 NEEDED liblcms2 等,+ coders dlopen 各格式库。
# 装到宿主标准路径后,HostLibResolver 自然解出 → 补进闭包。
# 冷门格式(heif/jxl/exr/raw)delegate 不装,对应 coder 仍 missing(不影响 PNG)。
#
# root 免密;装不上的包容错跳过(只要核心 PNG/JPEG 链装上)。
set -uo pipefail

# 核心 delegate(按 trixie 包名;t64 是 64 位时间过渡命名)。
PKGS=(
  liblcms2-2          # 色彩管理(libMagickCore 直接 NEEDED)
  libjpeg62-turbo     # JPEG
  libpng16-16t64      # PNG
  libfreetype6        # 字体(注释/标签)
  libraqm0            # 复杂文本排版(libMagickCore 直接 NEEDED)
  libfontconfig1      # 字体配置(启动必需)
  libglib2.0-0t64     # glib(启动必需)
  libx11-6            # X11(启动必需)
  libxext6            # X11 扩展
  libtiff6            # TIFF
  libwebp7            # WebP
  libxml2             # 配置/SVG(注:Arch 链接 libxml2.so.16,Debian 仅 .so.2 — 跨源 ABI 差异,见下)
  libltdl7            # 模块加载
  libgomp1            # OpenMP
  libfftw3-double3    # FFT(部分滤镜)
  libbz2-1.0          # bzip2
  liblzma5            # xz
)
# 注:libxml2.so.16(Arch soname)在 Debian 不存在(只有 .so.2)——这是真实跨发行版 ABI
# 差异,印证 PoC-4"同源补闭包"铁律(坑在 ABI 不在路径)。PNG 转图不依赖 libxml2,不影响。

echo "更新 apt 索引..."
apt-get update -qq 2>&1 | tail -1 || true

ok=0; skip=0
for p in "${PKGS[@]}"; do
  if apt-get install -y "$p" >/dev/null 2>&1; then
    echo "  装上: $p"; ok=$((ok+1))
  else
    echo "  跳过(无候选/失败): $p"; skip=$((skip+1))
  fi
done

echo "delegate 安装完成: $ok 装上, $skip 跳过"

# —— 跨源 ABI 桥接(诚实标注)——
# Arch 的 magick 链接 libxml2.so.16,Debian 只有 libxml2.so.2(同库不同 soname 版本号)。
# 这是 PoC-4"坑在 ABI 不在路径"的真实案例。不污染宿主 /usr/lib,在项目桥接目录
# 建 libxml2.so.16 → 宿主 .so.2 的软链,经 --library-path 提供给 magick。
# 注:这是版本号桥接(两者均 libxml2,主版本 ABI 通常兼容);真实多源场景应走同源库。
BRIDGE="${AEVUM_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.aevum}/abi-bridge"
mkdir -p "$BRIDGE"
HOST_XML2=$(find /usr/lib /lib -name "libxml2.so.2" 2>/dev/null | head -1)
if [ -n "$HOST_XML2" ] && [ ! -e "$BRIDGE/libxml2.so.16" ]; then
  ln -sf "$HOST_XML2" "$BRIDGE/libxml2.so.16"
  echo "ABI 桥接: libxml2.so.16 -> $HOST_XML2 (跨源版本号桥接,见注释)"
fi

echo "=== 核心库落位检查 ==="
for so in liblcms2.so.2 libjpeg.so.62 libpng16.so.16 libfreetype.so.6 liblqr-1.so.0; do
  f=$(find /usr/lib /lib -name "$so" 2>/dev/null | head -1)
  echo "  $so -> ${f:-MISSING}"
done
