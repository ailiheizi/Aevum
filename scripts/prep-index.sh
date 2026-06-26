#!/usr/bin/env bash
# 里程碑5 准备:解压真实 Debian Packages 索引到 $AEVUM_ROOT/index/Packages。
#
# solver 只吃纯文本(不引 flate2),gzip 用系统 gunzip 解压。同 zstd 策略。
# 用法: bash scripts/prep-index.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$REPO_ROOT/poc/poc1-index-feasibility/data/Packages.gz"
AEVUM_ROOT="${AEVUM_ROOT:-$REPO_ROOT/.aevum}"
DEST="$AEVUM_ROOT/index/Packages"

if [ ! -f "$SRC" ]; then
  echo "FATAL: 找不到 Packages.gz: $SRC" >&2
  exit 1
fi
if ! command -v gunzip >/dev/null 2>&1; then
  echo "FATAL: 需要 gunzip(系统自带,通常在 gzip 包)" >&2
  exit 1
fi

mkdir -p "$(dirname "$DEST")"
if [ -f "$DEST" ]; then
  echo "已存在,跳过: $DEST"
else
  echo "解压 $SRC → $DEST"
  gunzip -c "$SRC" > "$DEST"
fi
n=$(grep -c "^Package: " "$DEST" 2>/dev/null || echo 0)
echo "OK: 索引就绪,约 $n 个包条目 → $DEST"
