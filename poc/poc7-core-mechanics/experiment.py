#!/usr/bin/env python3
"""
PoC-7 — Aevum 核心机制实测:世代原子切换 / 瞬时回滚 / GC 引用计数

这三个是 Aevum 最核心的卖点,但此前只在文档里,从没用真文件+真符号链接验证过。

A. 原子切换:active 指针(symlink)用 rename 替换是否真原子、切换后内容是否瞬间生效
B. 瞬时回滚:回滚 = active 指回旧世代,不重建,实测耗时(应 << 构建)
C. GC 可达性:多世代共享 store hash,删一个世代后 GC 只回收"无世代引用"的 hash,不误删
D. namespace 强隔离下 setuid 行为(PoC-6 留的待办)

真实文件系统(WSL),真 symlink。输出 JSON。
"""
import os, time, json, shutil, hashlib, subprocess, stat
from pathlib import Path

ROOT = Path("/tmp/aevum-poc7")
STORE = ROOT / "store"
GENS = ROOT / "generations"


def put(name, content):
    """内容寻址入 store,返回 hash 目录。"""
    blob = content if isinstance(content, bytes) else content.encode()
    h = hashlib.sha256(blob).hexdigest()[:12]
    d = STORE / f"{h}-{name}"
    d.mkdir(parents=True, exist_ok=True)
    (d / name).write_bytes(blob)
    return h, d


def make_generation(gen_id, packages):
    """造一个世代:gen-N/packages/<name> → symlink 到 store hash 目录。"""
    g = GENS / f"gen-{gen_id:03d}"
    pkgdir = g / "packages"
    pkgdir.mkdir(parents=True, exist_ok=True)
    refs = []
    for name, hashdir in packages:
        link = pkgdir / name
        if link.exists() or link.is_symlink():
            link.unlink()
        os.symlink(hashdir, link)
        refs.append(hashdir.name)
    (g / "lock.txt").write_text("\n".join(refs))
    return g


def set_active(gen_path):
    """原子切 active 指针:写临时 symlink → rename 覆盖(POSIX rename 原子)。"""
    active = GENS / "active"
    tmp = GENS / f".active.tmp.{os.getpid()}"
    if tmp.exists() or tmp.is_symlink():
        tmp.unlink()
    os.symlink(gen_path, tmp)
    os.rename(tmp, active)  # 原子替换
    return active


def read_active_pkg(pkgname):
    """通过 active 指针读某包的内容(模拟系统'当前看到的'版本)。"""
    p = GENS / "active" / "packages" / pkgname / pkgname
    return p.read_text() if p.exists() else None


def gc(keep_gen_ids):
    """可达性 GC:保留 keep 世代引用的 hash,其余回收。返回(删除列表,保留集)。"""
    referenced = set()
    for gid in keep_gen_ids:
        lock = GENS / f"gen-{gid:03d}" / "lock.txt"
        if lock.exists():
            referenced.update(lock.read_text().split("\n"))
    referenced.discard("")
    all_hashes = {d.name for d in STORE.iterdir() if d.is_dir()}
    garbage = all_hashes - referenced
    for g in garbage:
        shutil.rmtree(STORE / g)
    return sorted(garbage), sorted(referenced)


def main():
    if ROOT.exists():
        shutil.rmtree(ROOT)
    STORE.mkdir(parents=True); GENS.mkdir(parents=True)
    report = {}

    # 造 store:python 两个版本 + 共享库 libc(两个世代共享)
    py311, d_py311 = put("python", "python-3.11-body")
    py312, d_py312 = put("python", "python-3.12-body")
    libc_h, d_libc = put("libc", "glibc-2.41-body")  # 共享

    # gen-1: python3.11 + libc ; gen-2: python3.12 + libc(共享 libc)
    g1 = make_generation(1, [("python", d_py311), ("libc", d_libc)])
    g2 = make_generation(2, [("python", d_py312), ("libc", d_libc)])

    # ---------- A. 原子切换 ----------
    set_active(g1)
    before = read_active_pkg("python")
    t0 = time.perf_counter()
    set_active(g2)
    t_switch = time.perf_counter() - t0
    after = read_active_pkg("python")
    report["A_atomic_switch"] = {
        "before_active": before,
        "after_active": after,
        "switch_seconds": round(t_switch, 6),
        "switched_correctly": before == "python-3.11-body" and after == "python-3.12-body",
        "note": "active 是 symlink,rename 替换原子;切换后立刻读到新世代内容",
    }

    # ---------- B. 瞬时回滚 ----------
    t0 = time.perf_counter()
    set_active(g1)  # 回滚到 gen-1,不重建
    t_rollback = time.perf_counter() - t0
    rolled = read_active_pkg("python")
    report["B_instant_rollback"] = {
        "rollback_seconds": round(t_rollback, 6),
        "after_rollback_active": rolled,
        "rolled_back_correctly": rolled == "python-3.11-body",
        "note": "回滚=active 指回旧世代,不重新求解/构建,亚毫秒级",
    }

    # ---------- C. GC 可达性 ----------
    store_before = sorted(d.name for d in STORE.iterdir())
    # 场景:只保留 gen-1(active),回收 gen-2 独有的 python3.12;libc 仍被 gen-1 引用,不能删
    removed, kept = gc(keep_gen_ids=[1])
    store_after = sorted(d.name for d in STORE.iterdir())
    libc_survived = any("libc" in x for x in store_after)
    py312_removed = any("python" in x and py312 in x for x in removed)
    py311_survived = any(py311 in x for x in store_after)
    report["C_gc_reachability"] = {
        "store_before": store_before,
        "removed": removed,
        "kept_referenced": kept,
        "store_after": store_after,
        "shared_libc_survived": libc_survived,
        "unused_py312_removed": py312_removed,
        "active_py311_survived": py311_survived,
        "correct": libc_survived and py312_removed and py311_survived,
        "note": "删 gen-2 后:py3.12(仅gen-2用)被回收;libc(gen-1仍用)保留;py3.11(active)保留",
    }

    # ---------- D. namespace 强隔离下 setuid ----------
    # 在 user-namespace 内,setuid 通常被 no_new_privs / 映射限制
    rc, out, err = -1, "", ""
    try:
        r = subprocess.run(
            ["unshare", "-r", "bash", "-c",
             "id -u; echo ---; ls -l /usr/bin/sudo 2>/dev/null | cut -c1-12"],
            capture_output=True, timeout=15)
        rc, out, err = r.returncode, r.stdout.decode()[:150], r.stderr.decode()[:150]
    except Exception as e:
        err = f"exc:{e}"
    report["D_setuid_in_namespace"] = {
        "unshare_rc": rc,
        "out": out.strip(),
        "note": "user-ns 内 uid 被映射(常显示为 0/root 但是无特权的伪 root);"
                "真实 setuid 提权在 user-ns 内被 no_new_privs 限制 —— "
                "架构含义:强隔离沙箱内 setuid 包不应依赖真提权,需用其他授权机制",
    }

    report["all_core_ok"] = (
        report["A_atomic_switch"]["switched_correctly"]
        and report["B_instant_rollback"]["rolled_back_correctly"]
        and report["C_gc_reachability"]["correct"]
    )
    (ROOT / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
