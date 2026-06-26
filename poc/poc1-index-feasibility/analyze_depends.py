#!/usr/bin/env python3
"""
PoC-1 Step A — 依赖元数据可机器生成性分析（静态）

回应评审 C1/H1 的核心问题：
  "Linux 包的依赖元数据，有多大比例能从二进制(ELF)自动生成，
   多少必须人工标注（语义级/版本约束/虚包/alternatives）？"

方法：解析 Debian Packages 索引（人工维护的 Depends 字段），
对每一条依赖关系分类：
  - lib_soname    : 形如 libfoo1, libssl3 —— 纯共享库依赖，理论上 ELF NEEDED 可自动生成
  - versioned     : 带版本约束 (>=, <<, =) —— 机器能抓约束但选哪个版本是语义决策
  - virtual_or_alt: 虚包 / alternatives (| ) —— 纯人工语义，ELF 无从得知
  - non_lib       : 非库的程序/数据依赖 (e.g. 依赖某 daemon、某配置包) —— 人工语义

输出比例，给 C1 一个真实数字。
"""
import gzip
import re
import sys
import json
from pathlib import Path
from collections import Counter

DATA = Path(__file__).parent / "data"
PACKAGES_GZ = DATA / "Packages.gz"

# 形如 libssl3, libc6, libstdc++6, libpython3.11 —— 共享库包命名约定
LIB_NAME_RE = re.compile(r"^lib[a-z0-9].*[0-9]$|^lib.*\.so.*$|^lib[a-z0-9+._-]+[0-9]$")
# 更宽松：以 lib 开头且名字以数字或 soversion 结尾，是 Debian 库包的强约定
LIB_SONAME_RE = re.compile(r"^lib[a-z0-9+._-]*[0-9]([._-].*)?$")


def classify_dep(atom: str):
    """对单个依赖原子分类。atom 形如 'libssl3 (>= 3.0.0)' 或 'foo | bar'。"""
    atom = atom.strip()
    if not atom:
        return None
    has_version = "(" in atom
    name = atom.split("(")[0].split("|")[0].strip()
    is_libish = bool(LIB_SONAME_RE.match(name))
    return name, is_libish, has_version


def main():
    if not PACKAGES_GZ.exists():
        print(f"ERROR: {PACKAGES_GZ} 不存在，请先运行下载步骤", file=sys.stderr)
        sys.exit(1)

    pkg_count = 0
    dep_total = 0
    cat = Counter()
    # 包级别：该包的全部 Depends 是否「100% 由 lib soname 构成」（即理论上可全自动）
    pkg_fully_auto = 0
    pkg_with_deps = 0
    pkg_has_alt = 0
    pkg_has_virtual_guess = 0

    cur_pkg = None
    cur_depends = None

    def flush():
        nonlocal pkg_fully_auto, pkg_with_deps, pkg_has_alt
        if cur_pkg is None:
            return
        if not cur_depends:
            return
        nonlocal dep_total
        atoms = [a for a in cur_depends.split(",")]
        pkg_with_deps_local = False
        all_lib = True
        any_dep = False
        has_alt = False
        for atom in atoms:
            res = classify_dep(atom)
            if res is None:
                continue
            name, is_libish, has_version = res
            any_dep = True
            dep_total += 1
            if "|" in atom:
                has_alt = True
                cat["virtual_or_alt"] += 1
                all_lib = False
            elif is_libish and has_version:
                cat["lib_versioned"] += 1
                # lib 名可自动，版本约束需语义判定 —— 半自动
            elif is_libish and not has_version:
                cat["lib_soname"] += 1
            elif (not is_libish) and has_version:
                cat["versioned_nonlib"] += 1
                all_lib = False
            else:
                cat["non_lib"] += 1
                all_lib = False
        if any_dep:
            pkg_with_deps += 1
            if has_alt:
                pkg_has_alt += 1
            if all_lib:
                pkg_fully_auto += 1

    with gzip.open(PACKAGES_GZ, "rt", encoding="utf-8", errors="replace") as f:
        for line in f:
            if line.startswith("Package:"):
                flush()
                cur_pkg = line.split(":", 1)[1].strip()
                cur_depends = None
                pkg_count += 1
            elif line.startswith("Depends:"):
                cur_depends = line.split(":", 1)[1].strip()
            elif line.startswith(("Pre-Depends:",)) and cur_depends is not None:
                cur_depends += ", " + line.split(":", 1)[1].strip()
        flush()

    # lib_soname = 纯自动可生成；lib_versioned = 名字自动+版本半人工
    auto = cat["lib_soname"]
    semi = cat["lib_versioned"]
    manual = cat["virtual_or_alt"] + cat["versioned_nonlib"] + cat["non_lib"]

    result = {
        "total_packages_in_index": pkg_count,
        "packages_with_depends": pkg_with_deps,
        "total_dependency_atoms": dep_total,
        "by_category": dict(cat),
        "rollup": {
            "auto_lib_soname": auto,
            "semi_lib_versioned": semi,
            "manual_semantic": manual,
        },
        "pct": {
            "auto_pct": round(100 * auto / dep_total, 1) if dep_total else 0,
            "semi_pct": round(100 * semi / dep_total, 1) if dep_total else 0,
            "manual_pct": round(100 * manual / dep_total, 1) if dep_total else 0,
        },
        "package_level": {
            "packages_with_deps": pkg_with_deps,
            "fully_auto_packages": pkg_fully_auto,
            "fully_auto_pct": round(100 * pkg_fully_auto / pkg_with_deps, 1) if pkg_with_deps else 0,
            "packages_using_alternatives": pkg_has_alt,
            "alt_pct": round(100 * pkg_has_alt / pkg_with_deps, 1) if pkg_with_deps else 0,
        },
    }
    out = DATA / "step_a_result.json"
    out.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
