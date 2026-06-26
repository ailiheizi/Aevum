#!/usr/bin/env python3
"""
PoC-3 — 零 LLM 确定性求解器

证伪目标：证明"模板 + override → 求解依赖闭包 → 产出 lock"这条 Aevum 核心路径
完全不需要 AI 也能跑通且可复现。这直接兑现 ADR-0003 边界1
（AI 不直接选 hash，确定性求解器算闭包）并回应评审 H5/H6
（AI 是可选增强而非必需门槛）。

输入：
  - 模板(template)：一组顶层包名意图 + 可选版本约束
  - override：覆盖某些包的版本约束 / 排除某些包
数据：真实 Debian stable Packages.gz（PoC-1 已下载）
输出：
  - lock：闭包内每个包的精确 name@version + 内容指纹 + closure_id

确定性保证（关键）：
  1. 候选版本选择规则固定：满足约束的版本中选「版本号最大」(确定的偏序)
  2. 闭包展开用确定性顺序（按包名排序的工作队列）
  3. alternatives (a|b) 选择规则固定：取第一个在索引中存在的
  4. closure_id = 对最终 (name,version,sha) 排序后取摘要
  → 同输入 + 同索引快照 必然同输出，无随机、无时钟、无 AI
"""
import gzip
import re
import sys
import json
import hashlib
from pathlib import Path
from functools import cmp_to_key

DATA = Path(__file__).parent.parent / "poc1-index-feasibility" / "data"
PACKAGES_GZ = DATA / "Packages.gz"
OUT = Path(__file__).parent / "out"
OUT.mkdir(exist_ok=True)


# ---------- Debian 版本比较(确定性偏序) ----------
def _split_ver(v):
    # 简化的 Debian 版本：[epoch:]upstream[-revision]
    epoch = "0"
    if ":" in v:
        epoch, v = v.split(":", 1)
    return epoch, v


def _cmp_segment(a, b):
    # 交替比较非数字段与数字段（Debian 算法的简化版，确定且够用）
    ia = ib = 0
    while ia < len(a) or ib < len(b):
        # 非数字部分
        na = ""
        while ia < len(a) and not a[ia].isdigit():
            na += a[ia]; ia += 1
        nb = ""
        while ib < len(b) and not b[ib].isdigit():
            nb += b[ib]; ib += 1
        if na != nb:
            # '~' 排在最前(预发布),其余按 ASCII
            return _cmp_nonnum(na, nb)
        # 数字部分
        da = ""
        while ia < len(a) and a[ia].isdigit():
            da += a[ia]; ia += 1
        db = ""
        while ib < len(b) and b[ib].isdigit():
            db += b[ib]; ib += 1
        va = int(da) if da else 0
        vb = int(db) if db else 0
        if va != vb:
            return -1 if va < vb else 1
    return 0


def _cmp_nonnum(a, b):
    # '~' < 空 < 字母 < 其他
    def key(s):
        out = []
        for c in s:
            if c == "~":
                out.append(-1)
            elif c.isalpha():
                out.append(ord(c))
            else:
                out.append(ord(c) + 256)
        return out
    ka, kb = key(a), key(b)
    return -1 if ka < kb else (1 if ka > kb else 0)


def deb_ver_cmp(v1, v2):
    e1, r1 = _split_ver(v1)
    e2, r2 = _split_ver(v2)
    if int(e1) != int(e2):
        return -1 if int(e1) < int(e2) else 1
    return _cmp_segment(r1, r2)


def ver_satisfies(ver, op, target):
    c = deb_ver_cmp(ver, target)
    return {
        ">=": c >= 0, "<=": c <= 0, "=": c == 0,
        ">>": c > 0, "<<": c < 0, ">": c > 0, "<": c < 0,
    }.get(op, True)


# ---------- 索引加载 ----------
def load_index():
    """返回 name -> list[ {version, depends, filename, size} ]，以及 provides 映射。"""
    by_name = {}
    provides = {}  # 虚包 -> [真实包名]
    with gzip.open(PACKAGES_GZ, "rt", encoding="utf-8", errors="replace") as f:
        cur = {}
        def flush():
            if "Package" not in cur:
                return
            name = cur["Package"]
            rec = {
                "version": cur.get("Version", "0"),
                "depends": cur.get("Depends", ""),
                "predepends": cur.get("Pre-Depends", ""),
                "filename": cur.get("Filename", ""),
                "size": cur.get("Size", "0"),
                "sha256": cur.get("SHA256", ""),
            }
            by_name.setdefault(name, []).append(rec)
            for prov in cur.get("Provides", "").split(","):
                p = prov.split("(")[0].strip()
                if p:
                    provides.setdefault(p, []).append(name)
        for line in f:
            if line.strip() == "":
                flush(); cur = {}
            elif line.startswith(" "):
                continue
            elif ":" in line:
                k, v = line.split(":", 1)
                cur[k.strip()] = v.strip()
        flush()
    return by_name, provides


# ---------- 依赖原子解析 ----------
DEP_RE = re.compile(r"^([a-z0-9+.\-]+)(?:\s*\(\s*([<>=]+)\s*([^)]+)\))?")

def parse_atom(atom):
    """'libssl3 (>= 3.0)' -> (name, op, ver) ; 取 alternatives 第一个。"""
    first = atom.split("|")[0].strip()
    m = DEP_RE.match(first)
    if not m:
        return None
    return m.group(1), m.group(2), m.group(3)


def parse_alternatives(atom):
    """返回该原子的所有候选 (name,op,ver)，用于 a|b 确定性选择。"""
    out = []
    for alt in atom.split("|"):
        m = DEP_RE.match(alt.strip())
        if m:
            out.append((m.group(1), m.group(2), m.group(3)))
    return out


# ---------- 确定性版本选择 ----------
def pick_version(by_name, name, op, ver):
    """满足约束的版本中选最大者(确定性)。返回 rec 或 None。"""
    cands = by_name.get(name, [])
    ok = []
    for rec in cands:
        if op is None or ver_satisfies(rec["version"], op, ver):
            ok.append(rec)
    if not ok:
        return None
    ok.sort(key=cmp_to_key(lambda a, b: deb_ver_cmp(a["version"], b["version"])))
    return ok[-1]  # 最大版本


# ---------- 闭包求解 ----------
def resolve(by_name, provides, template, overrides):
    """
    template: list[ (name, op, ver) ]  顶层意图
    overrides: { name: (op, ver) | "exclude" }
    返回: closure dict, diagnostics
    """
    excluded = {n for n, v in overrides.items() if v == "exclude"}
    closure = {}        # name -> rec
    diag = {"unresolved": [], "excluded_hit": [], "virtual_resolved": [], "alt_chosen": []}

    # 确定性工作队列：按包名排序处理
    queue = sorted(template, key=lambda t: t[0])
    seen = set()

    while queue:
        queue.sort(key=lambda t: t[0])  # 每轮保持确定顺序
        name, op, ver = queue.pop(0)
        if name in excluded:
            diag["excluded_hit"].append(name)
            continue
        # 应用 override
        if name in overrides and overrides[name] != "exclude":
            op, ver = overrides[name]
        key = (name, op, ver)
        if key in seen:
            continue
        seen.add(key)

        rec = pick_version(by_name, name, op, ver)
        if rec is None:
            # 试虚包
            if name in provides:
                real = sorted(provides[name])[0]  # 确定性：取字典序第一
                diag["virtual_resolved"].append({"virtual": name, "chosen": real})
                rec = pick_version(by_name, real, None, None)
                name = real
            if rec is None:
                diag["unresolved"].append({"name": name, "op": op, "ver": ver})
                continue

        # 已在闭包且版本相同则跳过
        if name in closure and closure[name]["version"] == rec["version"]:
            continue
        closure[name] = rec

        # 展开依赖
        all_deps = (rec["depends"] + "," + rec["predepends"]).strip(",")
        for atom in all_deps.split(","):
            atom = atom.strip()
            if not atom:
                continue
            alts = parse_alternatives(atom)
            if not alts:
                continue
            chosen = None
            if len(alts) > 1:
                # 确定性 alternatives：选第一个在索引/虚包中存在的
                for cand in alts:
                    if cand[0] in by_name or cand[0] in provides:
                        chosen = cand
                        break
                if chosen:
                    diag["alt_chosen"].append({"atom": atom, "chosen": chosen[0]})
            else:
                chosen = alts[0]
            if chosen and chosen[0] not in excluded:
                queue.append(chosen)

    return closure, diag


def content_fingerprint(rec):
    """内容指纹：优先用索引提供的 SHA256(真实内容寻址);否则用 name@ver 占位。"""
    if rec.get("sha256"):
        return "sha256:" + rec["sha256"]
    h = hashlib.sha256(f"{rec['version']}".encode()).hexdigest()
    return "placeholder:" + h


def build_lock(closure, diag, template, overrides):
    locked = []
    for name in sorted(closure):
        rec = closure[name]
        locked.append({
            "name": name,
            "version": rec["version"],
            "fingerprint": content_fingerprint(rec),
            "filename": rec["filename"],
        })
    # closure_id：对排序后的 (name,version,fingerprint) 取摘要
    blob = "\n".join(f"{x['name']}@{x['version']}#{x['fingerprint']}" for x in locked)
    closure_id = "clo-" + hashlib.sha256(blob.encode()).hexdigest()[:16]
    return {
        "closure_id": closure_id,
        "input": {
            "template": [list(t) for t in template],
            "overrides": {k: (list(v) if isinstance(v, tuple) else v) for k, v in overrides.items()},
        },
        "package_count": len(locked),
        "locked": locked,
        "diagnostics": diag,
    }


# ---------- 内置模板 ----------
TEMPLATES = {
    "dev-python": [("python3", None, None), ("python3-pip", None, None)],
    "web-server": [("nginx-core", None, None), ("curl", None, None)],
    "cli-tools": [("ripgrep", None, None), ("jq", None, None), ("git", None, None)],
    "media": [("ffmpeg", None, None), ("imagemagick", None, None)],
}


def main():
    if len(sys.argv) < 2:
        print("用法: python solver.py <template_name> [--exclude pkg] [--pin pkg=op:ver]")
        print("可用模板:", ", ".join(TEMPLATES))
        sys.exit(1)
    tname = sys.argv[1]
    if tname not in TEMPLATES:
        print(f"未知模板 {tname}", file=sys.stderr); sys.exit(1)

    overrides = {}
    i = 2
    while i < len(sys.argv):
        if sys.argv[i] == "--exclude":
            overrides[sys.argv[i + 1]] = "exclude"; i += 2
        elif sys.argv[i] == "--pin":
            pkg, spec = sys.argv[i + 1].split("=")
            op, ver = spec.split(":")
            overrides[pkg] = (op, ver); i += 2
        else:
            i += 1

    by_name, provides = load_index()
    template = TEMPLATES[tname]
    closure, diag = resolve(by_name, provides, template, overrides)
    lock = build_lock(closure, diag, template, overrides)

    out_file = OUT / f"lock-{tname}.json"
    out_file.write_text(json.dumps(lock, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"模板: {tname}  |  override: {overrides or '无'}")
    print(f"closure_id: {lock['closure_id']}")
    print(f"闭包包数: {lock['package_count']}")
    print(f"未解析: {len(diag['unresolved'])}  虚包解析: {len(diag['virtual_resolved'])}  "
          f"alternatives 选择: {len(diag['alt_chosen'])}")
    print(f"→ {out_file}")


if __name__ == "__main__":
    main()
