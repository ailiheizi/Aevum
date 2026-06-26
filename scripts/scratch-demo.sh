#!/usr/bin/env bash
# 里程碑8:全裸容器验证 —— Aevum 装的 hello 在 FROM scratch(无包管理器/无系统库)里真跑。
#
# 阶段A(有工具):aevum export-rootfs hello → 自包含 rootfs(hello+libc+loader 实体)。
# 阶段B(全裸):FROM scratch + COPY rootfs + CMD loader注入跑 hello。
#
# 对照:裸跑(无loader)必失败 vs Aevum闭包+loader 成功打印 Hello, world!
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export AEVUM_ROOT="${AEVUM_ROOT:-$REPO_ROOT/.aevum}"
ROOTFS="$AEVUM_ROOT/rootfs-hello"
TARGET="${CARGO_TARGET_DIR:-/tmp/aevum-target}"

echo "=== 前提检查 ==="
command -v docker >/dev/null || { echo "FATAL: 无 docker"; exit 1; }
ls "$AEVUM_ROOT/unpacked/hello/usr/bin/hello" >/dev/null 2>&1 \
  || { echo "FATAL: hello 未安装,先 aevum install hello --only hello"; exit 1; }

echo "=== 阶段A:导出自包含 rootfs ==="
. "$HOME/.cargo/env" 2>/dev/null || true
CARGO_TARGET_DIR="$TARGET" cargo run -q -p aevum-cli -- export-rootfs hello
echo "--- rootfs 内容(应只有 bin/ + lib/,无系统目录)---"
find "$ROOTFS" -type f | sed "s|$ROOTFS/||"

echo "=== 阶段B:FROM scratch 全裸容器 ==="
BUILD="$AEVUM_ROOT/scratch-build"
rm -rf "$BUILD"; mkdir -p "$BUILD"
cp -r "$ROOTFS" "$BUILD/rootfs"
# loader 名(lib/ 下的 ld-linux*)
LOADER=$(ls "$BUILD/rootfs/lib/" | grep "^ld-" | head -1)

cat > "$BUILD/Dockerfile" <<EOF
FROM scratch
COPY rootfs/ /
# 全裸:无 /bin /usr,无 shell,无包管理器。loader 注入跑 hello(PoC-2)。
ENTRYPOINT ["/lib/$LOADER", "--library-path", "/lib", "/bin/hello"]
EOF

echo "--- Dockerfile ---"; cat "$BUILD/Dockerfile"
echo "--- docker build (FROM scratch) ---"
docker build -q -t aevum-scratch-hello "$BUILD" >/dev/null && echo "build: ok"

echo "=== 对照1:全裸容器直接跑 hello(无 loader 注入)应失败 ==="
docker run --rm --entrypoint /bin/hello aevum-scratch-hello 2>&1 | head -2 || echo "(裸跑失败 = 预期:全裸环境无 interp/libc)"

echo "=== 对照2:Aevum 闭包 + loader 注入 应成功 ==="
OUT=$(docker run --rm aevum-scratch-hello 2>&1)
echo "容器输出: $OUT"
if echo "$OUT" | grep -q "Hello, world"; then
  echo "✅ 里程碑8 达成:hello 在 FROM scratch 全裸容器(无包管理器/无系统库)真跑通"
else
  echo "❌ 未输出 Hello, world(检查闭包/loader)"; exit 1
fi
