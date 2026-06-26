#!/usr/bin/env bash
# 里程碑1 准备步骤:把真实 Arch rg 包解到 $AEVUM_ROOT/unpacked/rg。
#
# closure-builder 只吃已解压目录(不引入 Rust zstd/tar 依赖,见计划"硬约束")。
# 解包用系统 zstd + tar;缺 zstd 则以 root 免密 apt 装(WSL Debian)。
#
# 用法: bash scripts/prep-rg.sh
# 环境: AEVUM_ROOT(默认 ./.aevum)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG="$REPO_ROOT/poc/poc4-arch-isolation/data/rg.pkg.tar.zst"
AEVUM_ROOT="${AEVUM_ROOT:-$REPO_ROOT/.aevum}"
DEST="$AEVUM_ROOT/unpacked/rg"

if [ ! -f "$PKG" ]; then
  echo "FATAL: 找不到 rg 包: $PKG" >&2
  exit 1
fi

# 确保 zstd 可用(WSL Debian 精简,默认没装)。
if ! command -v zstd >/dev/null 2>&1; then
  echo "zstd 未安装,尝试以 root 安装..." >&2
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    sudo apt-get update -qq && sudo apt-get install -y zstd
  else
    # WSL 无密码 root 入口
    apt-get update -qq && apt-get install -y zstd 2>/dev/null \
      || { echo "FATAL: 无法安装 zstd(需 root)。请手动: apt-get install -y zstd" >&2; exit 1; }
  fi
fi

echo "解包 $PKG → $DEST"
rm -rf "$DEST"
mkdir -p "$DEST"
# Arch 包 = zstd 压缩的 tar。
zstd -dc "$PKG" | tar -x -C "$DEST"

RG="$DEST/usr/bin/rg"
if [ ! -f "$RG" ]; then
  echo "FATAL: 解包后未找到 $RG" >&2
  exit 1
fi
echo "OK: rg 解包就绪 → $RG"
ls -la "$RG"
