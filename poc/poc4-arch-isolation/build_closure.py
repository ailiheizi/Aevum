#!/usr/bin/env python3
"""
PoC-4 — Arch 包"补闭包 + 轻隔离"验证(WSL Debian 上跑)

回答两个 PoC 级未知:
  疑问2: 把非自包含的 Arch 包(rg,靠标准路径找库)补成闭包,成本多大?
  疑问1: 补完后用"轻隔离"(显式 loader + 私有 library-path)能否在
         "无标准 /lib"环境跑起来,且不依赖系统 /usr/lib?

实验:
  1. 取 Windows 侧已解出的 Arch rg 二进制(rg/usr/bin/rg)
  2. 解析它的 NEEDED → 从宿主 /lib 取齐依赖库,放进隔离 store(内容寻址)
     —— 这模拟 Aevum "把非自包含包补全成闭包"
  3. 在 mount-namespace 里遮蔽标准 /lib64 + /usr/lib(复现"无标准库"),
     用 store 内 loader + library-path 跑 rg
  4. 对比:裸跑(应失败) vs 轻隔离闭包跑(应成功)

诚实记录:补闭包时每个依赖是否在宿主找得到、找不到的缺口 = 真实成本信号。
"""
import os, struct, hashlib, shutil, subprocess, json, glob
from pathlib import Path

ARCH_RG = None
for c in ["/mnt/d/windows/code/project/Aevum/poc/poc4-arch-isolation/data/rg/usr/bin/rg"]:
    if os.path.exists(c): ARCH_RG = c
STORE = Path("/tmp/aevum-poc4/store")


def parse_needed(path):
    d = open(path, "rb").read()
    end = "<" if d[5] == 1 else ">"
    phoff = struct.unpack_from(end+"Q", d, 0x20)[0]; phes = struct.unpack_from(end+"H", d, 0x36)[0]; phn = struct.unpack_from(end+"H", d, 0x38)[0]
    shoff = struct.unpack_from(end+"Q", d, 0x28)[0]; shes = struct.unpack_from(end+"H", d, 0x3A)[0]; shn = struct.unpack_from(end+"H", d, 0x3C)[0]
    interp = None
    for i in range(phn):
        b = phoff+i*phes
        if struct.unpack_from(end+"I", d, b)[0] == 3:
            o = struct.unpack_from(end+"Q", d, b+8)[0]; s = struct.unpack_from(end+"Q", d, b+32)[0]; interp = d[o:o+s].rstrip(b"\x00").decode()
    def sec(i):
        b = shoff+i*shes
        return struct.unpack_from(end+"I", d, b+4)[0], struct.unpack_from(end+"Q", d, b+0x18)[0], struct.unpack_from(end+"Q", d, b+0x20)[0], struct.unpack_from(end+"I", d, b+0x28)[0], struct.unpack_from(end+"Q", d, b+0x38)[0]
    needed = []
    for i in range(shn):
        t, off, sz, link, ent = sec(i)
        if t != 6: continue
        _, so, ss, _, _ = sec(link); st = d[so:so+ss]; ent = ent or 16
        for o in range(off, off+sz, ent):
            tag, val = struct.unpack_from(end+"qQ", d, o)
            if tag == 0: break
            if tag == 1: needed.append(st[val:st.find(b"\x00", val)].decode())
    return interp, needed


def find_lib(soname):
    for dd in ["/lib/x86_64-linux-gnu", "/usr/lib/x86_64-linux-gnu", "/lib64", "/usr/lib"]:
        p = Path(dd)/soname
        if p.exists(): return str(p.resolve())
    return None


def put(realpath, name):
    blob = Path(realpath).read_bytes()
    h = hashlib.sha256(blob).hexdigest()[:8]
    d = STORE/f"{h}-{name}"; d.mkdir(parents=True, exist_ok=True)
    dst = d/name; dst.write_bytes(blob); dst.chmod(0o755)
    return d


def closure_walk(start_bin):
    """递归补闭包:从 rg 出发,解 NEEDED,取库,再解库的 NEEDED…直到闭合。"""
    interp, _ = parse_needed(start_bin)
    resolved = {}      # soname -> store dir
    missing = []
    queue = []
    _, top_needed = parse_needed(start_bin)
    queue += top_needed
    seen = set()
    while queue:
        so = queue.pop(0)
        if so in seen: continue
        seen.add(so)
        real = find_lib(so)
        if not real:
            missing.append(so); continue
        d = put(real, so)
        resolved[so] = str(d)
        # 递归解这个库自己的 NEEDED
        try:
            _, sub = parse_needed(real)
            for s in sub:
                if s not in seen: queue.append(s)
        except Exception:
            pass
    return interp, resolved, missing


def main():
    if STORE.exists(): shutil.rmtree(STORE.parent)
    STORE.mkdir(parents=True)
    report = {"target": ARCH_RG}
    if not ARCH_RG:
        report["error"] = "Arch rg 二进制未找到"; print(json.dumps(report)); return

    # 1. 把 rg 本体放进 store
    rg_dir = put(ARCH_RG, "rg")
    rg_store = str(rg_dir/"rg")

    # 2. 补闭包(递归)
    interp, resolved, missing = closure_walk(ARCH_RG)
    interp_real = os.path.realpath(interp) if interp and os.path.exists(interp) else find_lib(os.path.basename(interp or ""))
    ld_dir = put(interp_real, "ld-linux-x86-64.so.2") if interp_real else None

    report["closure"] = {
        "interpreter": interp,
        "libs_resolved": sorted(resolved.keys()),
        "libs_resolved_count": len(resolved),
        "libs_missing": missing,
        "closure_complete": len(missing) == 0,
    }
    report["cost_signal"] = {
        "total_libs_in_closure": len(resolved),
        "auto_resolved_from_host": len(resolved),
        "needed_manual": len(missing),
        "note": "missing 的库 = 宿主没有、需 Aevum 从上游另取 = 真实补闭包成本",
    }

    libpath = ":".join(resolved.values())
    report["store_layout_example"] = sorted([d.name for d in STORE.iterdir()])[:8]

    REP = Path("/tmp/aevum-poc4/report.json")
    REP.write_text(json.dumps(report, ensure_ascii=False, indent=2))
    # 把跑测试需要的路径也写出来,供外层 shell 在 namespace 里用
    Path("/tmp/aevum-poc4/paths.sh").write_text(
        f'RG="{rg_store}"\nLD="{ld_dir/"ld-linux-x86-64.so.2" if ld_dir else ""}"\nLIBS="{libpath}"\n'
    )
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
