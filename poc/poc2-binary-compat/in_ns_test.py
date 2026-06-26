#!/usr/bin/env python3
"""
PoC-2b — 真正复现 NixOS 困境后再验证 Aevum 方案

PoC-2 的弱点:裸跑成功是因为宿主有标准 /lib64。本脚本用 mount namespace
把标准库路径"遮蔽"掉,真实复现 NixOS"无标准 /lib"的环境,再对比:
  A. 裸跑二进制         → 预期失败(找不到 interpreter / 库)= NixOS 痛点
  B. Aevum 显式 loader  → 预期成功(loader 和库都在 store 里,不依赖标准路径)

实现:在 `unshare -rm`(user+mount namespace,免 root)内,
用 tmpfs 覆盖 /lib64 与 /usr/lib/x86_64-linux-gnu，制造"标准路径空了"的世界。
store 在 /tmp/aevum-poc2 下，不受覆盖影响。

本脚本由外层 driver 在 namespace 内调用；它假定 store 已由 experiment.py 建好。
"""
import os
import sys
import json
import subprocess
from pathlib import Path

STORE = Path("/tmp/aevum-poc2/store")


def find_in_store(suffix):
    for d in STORE.iterdir():
        if d.name.endswith(suffix):
            f = d / suffix
            if f.exists():
                return f
    return None


def run(cmd):
    try:
        r = subprocess.run(cmd, capture_output=True, timeout=20)
        return r.returncode, r.stdout.decode(errors="replace")[:120], r.stderr.decode(errors="replace")[:300]
    except Exception as e:
        return -1, "", f"exc: {e}"


def main():
    bin_path = find_in_store("curl")
    ld_path = find_in_store("ld-linux-x86-64.so.2")
    libs = [str(d) for d in STORE.iterdir() if d.name.endswith((".so.4", ".so.1", ".so.6"))]

    result = {"env": "mount-ns with /lib64 + /usr/lib/x86_64-linux-gnu shadowed by empty tmpfs"}

    # 确认标准 loader 路径在本 namespace 内已不可见
    std_loader = Path("/lib64/ld-linux-x86-64.so.2")
    result["std_loader_visible"] = std_loader.exists()

    # A. 裸跑（依赖标准 PT_INTERP /lib64/...）—— 复现 NixOS 困境
    a_rc, a_out, a_err = run([str(bin_path), "--version"])
    result["A_naive_run"] = {
        "rc": a_rc,
        "stdout_head": a_out.split(chr(10))[0] if a_out else "",
        "stderr": a_err,
        "expected": "fail (no standard /lib64 loader) = NixOS pain",
        "failed_as_expected": a_rc != 0,
    }

    # B. Aevum 显式 loader（loader+库都在 store，不碰标准路径）
    b_cmd = [str(ld_path), "--library-path", ":".join(libs), str(bin_path), "--version"]
    b_rc, b_out, b_err = run(b_cmd)
    result["B_aevum_loader"] = {
        "rc": b_rc,
        "stdout_head": b_out.split(chr(10))[0] if b_out else "",
        "stderr": b_err if b_rc != 0 else "",
        "expected": "success (store self-contained)",
        "succeeded": b_rc == 0,
    }

    # 真正的证明：困境下 A 失败、B 成功
    result["proves_aevum_value"] = (a_rc != 0) and (b_rc == 0)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
