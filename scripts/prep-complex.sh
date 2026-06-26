#!/usr/bin/env bash
# 里程碑2 准备:解 python / imagemagick 复杂包到 $AEVUM_ROOT/unpacked/。
#
# 沿用 prep-rg.sh 的策略:系统 zstd + tar,不引 Rust 解包依赖。
# 用法: bash scripts/prep-complex.sh [py|im|all]   (默认 all)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="$REPO_ROOT/poc/poc5-complex-pkg/data"
AEVUM_ROOT="${AEVUM_ROOT:-$REPO_ROOT/.aevum}"
WHICH="${1:-all}"

if ! command -v zstd >/dev/null 2>&1; then
  echo "zstd 未安装,尝试以 root 安装..." >&2
  apt-get update -qq && apt-get install -y zstd 2>/dev/null \
    || { echo "FATAL: 无法安装 zstd(需 root)" >&2; exit 1; }
fi

unpack() {
  local pkg="$1" name="$2" probe="$3"
  if [ ! -f "$pkg" ]; then
    echo "SKIP $name: 找不到 $pkg" >&2
    return 0
  fi
  local dest="$AEVUM_ROOT/unpacked/$name"
  echo "解包 $name → $dest"
  rm -rf "$dest"; mkdir -p "$dest"
  zstd -dc "$pkg" | tar -x -C "$dest"
  if [ ! -e "$dest/$probe" ]; then
    echo "FATAL: $name 解包后未找到 $probe" >&2
    exit 1
  fi
  echo "OK: $name → $dest/$probe"
}

case "$WHICH" in
  py|all)  unpack "$DATA/py.pkg.tar.zst" python "usr/bin/python3" ;;
esac
case "$WHICH" in
  im|all)  unpack "$DATA/im.pkg.tar.zst" imagemagick "usr/bin/magick" ;;
esac
echo "DONE"
