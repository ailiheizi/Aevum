#!/usr/bin/env python3
"""
PoC-6 — 三个架构盲区压测(WSL Debian)

A. setuid 包:内容寻址 store 复制 + 隔离视图下,setuid 权限位还成不成立?
   架构风险:内容寻址按"内容"算 hash,setuid 是 inode 元数据(权限位),
            复制进 store 时极易丢失;且 user-namespace 内 setuid 行为受限。
B. 多包通信:两个各自隔离闭包的程序,能否互相调用(进程级通信)?
   架构风险:隔离做过头会挡住正常的 exec/pipe。
C. 多版本磁盘代价:N 个包各带同源 glibc 闭包,内容寻址去重能省多少?
   架构问题:同源闭包要求每包带自己的 glibc,会不会磁盘爆炸?

只用宿主现成文件,不下新包。输出 JSON。
"""
import os, struct, hashlib, shutil, subprocess, json, stat
from pathlib import Path

ROOT = Path("/tmp/aevum-poc6")
STORE = ROOT / "store"


def sh(cmd):
    try:
        r = subprocess.run(cmd, capture_output=True, timeout=20)
        return r.returncode, r.stdout.decode(errors="replace")[:200], r.stderr.decode(errors="replace")[:200]
    except Exception as e:
        return -1, "", f"exc:{e}"


# ---------- A. setuid 权限位 ----------
def test_setuid():
    res = {"name": "setuid_through_content_store"}
    # 找一个 setuid 二进制
    cands = ["/usr/bin/sudo", "/usr/bin/passwd", "/bin/su", "/usr/bin/ping", "/usr/bin/chsh", "/usr/bin/newgrp"]
    src = None
    for c in cands:
        if os.path.exists(c):
            m = os.stat(c).st_mode
            if m & stat.S_ISUID:
                src = c; break
    if not src:
        res["finding"] = "宿主无 setuid 样本(WSL 常裁剪),改用理论+权限位往返验证"
        # 退而求其次:自造一个文件,设 setuid,走"内容寻址复制"看位是否保留
        src = str(ROOT / "fake")
        ROOT.mkdir(parents=True, exist_ok=True)
        Path(src).write_bytes(b"#!/bin/true\n"); os.chmod(src, 0o4755)
    orig_mode = oct(os.stat(src).st_mode & 0o7777)
    res["source"] = src
    res["source_mode"] = orig_mode
    res["source_has_setuid"] = bool(os.stat(src).st_mode & stat.S_ISUID)

    # 模拟内容寻址复制:read bytes → 写新文件(常见实现会丢权限位!)
    blob = Path(src).read_bytes()
    h = hashlib.sha256(blob).hexdigest()[:8]
    d = STORE / f"{h}-setuidtest"; d.mkdir(parents=True, exist_ok=True)
    dst = d / "bin"
    dst.write_bytes(blob)  # 注意:只写内容,没 copy mode —— 这正是天真实现的 bug
    naive_mode = oct(os.stat(dst).st_mode & 0o7777)
    res["naive_copy_mode"] = naive_mode
    res["naive_lost_setuid"] = not bool(os.stat(dst).st_mode & stat.S_ISUID)

    # 正确做法:显式恢复权限位
    os.chmod(dst, int(orig_mode, 8))
    fixed_mode = oct(os.stat(dst).st_mode & 0o7777)
    res["explicit_chmod_mode"] = fixed_mode
    res["explicit_restored"] = bool(os.stat(dst).st_mode & stat.S_ISUID)

    res["arch_conclusion"] = (
        "内容寻址只哈希内容,setuid/权限位是带外元数据。天真的 read→write 复制丢 setuid("
        + str(res["naive_lost_setuid"]) + ")。store 必须显式记录并恢复权限位(含 setuid),"
        "且权限位应纳入内容寻址的规范化输入,否则 sudo/ping 这类包入 store 后提权失效。"
    )
    return res


# ---------- B. 多包通信 ----------
def test_ipc():
    res = {"name": "isolated_packages_ipc"}
    # 用宿主的 sh 和 echo 模拟两个"隔离包",让一个调另一个
    # 隔离 = 各自 library-path,但 exec/pipe 是进程级,不受 library-path 隔离影响
    sh_bin = shutil.which("sh") or "/bin/sh"
    # 程序A(sh)调用程序B(echo)并取其输出 —— 跨"包"调用
    rc, out, err = sh([sh_bin, "-c", "echo from_pkgA $(echo from_pkgB)"])
    res["cross_pkg_exec_rc"] = rc
    res["cross_pkg_exec_out"] = out.strip()
    res["cross_pkg_exec_works"] = rc == 0 and "from_pkgB" in out
    # 管道通信
    rc2, out2, _ = sh([sh_bin, "-c", "echo hello | cat"])
    res["pipe_rc"] = rc2
    res["pipe_works"] = rc2 == 0 and "hello" in out2
    res["arch_conclusion"] = (
        "轻隔离只隔离【库搜索路径】,不隔离进程/exec/pipe/socket。"
        "所以两个轻隔离包能正常互相调用与通信(cross-exec=" + str(res["cross_pkg_exec_works"])
        + ", pipe=" + str(res["pipe_works"]) + ")。"
        "只有【强隔离 namespace】才会切断这些,那时需显式打洞(挂载/socket 共享)。"
        "→ 默认轻隔离不破坏包间协作,符合预期。"
    )
    return res


# ---------- C. 多版本磁盘代价 ----------
def test_disk():
    res = {"name": "multiversion_disk_dedup"}
    libc = None
    for c in ["/lib/x86_64-linux-gnu/libc.so.6", "/usr/lib/x86_64-linux-gnu/libc.so.6"]:
        if os.path.exists(c): libc = c; break
    if not libc:
        res["finding"] = "无 libc 样本"; return res
    libc_size = os.path.getsize(libc)
    res["libc_size_bytes"] = libc_size

    # 场景:10 个程序,每个带"同源闭包"(都含同一个 glibc + 各自一个小私有库)
    N = 10
    # 不去重:每个程序独立存全套(glibc 复制 N 份)
    no_dedup = N * libc_size + N * 50000  # 各自私有库估 50KB
    # 内容寻址去重:同一个 glibc hash 只存 1 份,私有库各存
    blob = Path(libc).read_bytes()
    glibc_hash = hashlib.sha256(blob).hexdigest()[:8]
    dedup = 1 * libc_size + N * 50000  # glibc 共享 1 份
    res["scenario"] = f"{N} 个程序各带同源闭包(共享同版本 glibc + 各自私有库)"
    res["no_dedup_bytes"] = no_dedup
    res["content_addressed_dedup_bytes"] = dedup
    res["saved_bytes"] = no_dedup - dedup
    res["saved_pct"] = round(100 * (no_dedup - dedup) / no_dedup, 1)
    res["arch_conclusion"] = (
        f"同源闭包要求每包带自己的 glibc,但若多包用【同一版本】glibc,内容寻址按 hash 去重 → "
        f"glibc 只存 1 份而非 {N} 份,省 {res['saved_pct']}%。"
        f"真实风险只在【多个不同版本】glibc 并存时(保留两份场景),那才真占空间——但那是有意的多版本代价,GC 在引用归零后回收。"
    )
    return res


def main():
    if ROOT.exists(): shutil.rmtree(ROOT)
    STORE.mkdir(parents=True)
    report = {"tests": [test_setuid(), test_ipc(), test_disk()]}
    (ROOT / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
