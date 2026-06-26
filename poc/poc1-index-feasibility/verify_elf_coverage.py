#!/usr/bin/env python3
"""
PoC-1 Step B — ELF 自动抽取 vs 人工声明 的真实覆盖率（抽样）

下载真实 .deb，解出每个 ELF 的 DT_NEEDED（它真正链接了哪些 .so），
把 NEEDED 的 soname 映射回"提供该 so 的包"，
和该包人工声明的 Depends 里的库依赖比对：
  - 自动抽取能命中多少人工声明的库依赖（召回）
  - 人工声明里有多少是 ELF 根本看不到的（语义依赖：运行时 dlopen、数据包、daemon）

不依赖系统 ar：用纯 Python 解析 .deb（ar 归档）。
不依赖 readelf：用纯 Python 解析 ELF 的 .dynamic / DT_NEEDED。
"""
import io
import re
import sys
import json
import gzip
import struct
import tarfile
import urllib.request
from pathlib import Path

DATA = Path(__file__).parent / "data"
DEB_DIR = DATA / "debs"
DEB_DIR.mkdir(parents=True, exist_ok=True)
MIRROR = "http://deb.debian.org/debian/"

# 抽样：覆盖不同类型——核心库、CLI 工具、带插件(dlopen)的、解释器
SAMPLE = [
    "coreutils", "grep", "ripgrep", "fd-find", "jq", "curl", "wget",
    "openssl", "zlib1g", "libpng16-16", "bash", "sqlite3", "imagemagick",
    "ffmpeg", "vim", "nginx-core", "git", "tar", "gzip", "zstd",
]


# ---------- 解析 Packages.gz：建 soname->包、包->Depends、包->Filename ----------
def load_index():
    pkgs = {}
    with gzip.open(DATA / "Packages.gz", "rt", encoding="utf-8", errors="replace") as f:
        cur = {}
        for line in f:
            if line.strip() == "":
                if "Package" in cur:
                    pkgs[cur["Package"]] = cur
                cur = {}
            elif line.startswith(" "):
                continue
            else:
                if ":" in line:
                    k, v = line.split(":", 1)
                    cur[k.strip()] = v.strip()
        if "Package" in cur:
            pkgs[cur["Package"]] = cur
    return pkgs


def lib_atoms(depends: str):
    """从 Depends 抽出库类依赖的包名（lib 开头、数字结尾约定）。"""
    out = []
    if not depends:
        return out
    LIB = re.compile(r"^lib[a-z0-9+._-]*[0-9]([._-].*)?$")
    for atom in depends.split(","):
        name = atom.split("(")[0].split("|")[0].strip()
        if LIB.match(name):
            out.append(name)
    return out


# ---------- 纯 Python 解 .deb (ar) ----------
def ar_members(blob: bytes):
    assert blob[:8] == b"!<arch>\n", "not an ar archive"
    off = 8
    members = {}
    while off + 60 <= len(blob):
        hdr = blob[off:off + 60]
        name = hdr[0:16].decode("ascii", "replace").strip()
        size = int(hdr[48:58].decode("ascii", "replace").strip() or "0")
        start = off + 60
        members[name.rstrip("/")] = blob[start:start + size]
        off = start + size + (size & 1)
    return members


# ---------- 纯 Python 解 ELF 的 DT_NEEDED ----------
def elf_needed(data: bytes):
    if data[:4] != b"\x7fELF":
        return None
    ei_class = data[4]; ei_data = data[5]
    is64 = ei_class == 2
    end = "<" if ei_data == 1 else ">"
    if is64:
        e_shoff = struct.unpack_from(end + "Q", data, 0x28)[0]
        e_shentsize = struct.unpack_from(end + "H", data, 0x3A)[0]
        e_shnum = struct.unpack_from(end + "H", data, 0x3C)[0]
        e_shstrndx = struct.unpack_from(end + "H", data, 0x3E)[0]
    else:
        e_shoff = struct.unpack_from(end + "I", data, 0x20)[0]
        e_shentsize = struct.unpack_from(end + "H", data, 0x2E)[0]
        e_shnum = struct.unpack_from(end + "H", data, 0x30)[0]
        e_shstrndx = struct.unpack_from(end + "H", data, 0x32)[0]
    if e_shoff == 0 or e_shnum == 0:
        return None

    def section(i):
        b = e_shoff + i * e_shentsize
        if is64:
            name = struct.unpack_from(end + "I", data, b)[0]
            typ = struct.unpack_from(end + "I", data, b + 4)[0]
            offset = struct.unpack_from(end + "Q", data, b + 0x18)[0]
            size = struct.unpack_from(end + "Q", data, b + 0x20)[0]
            link = struct.unpack_from(end + "I", data, b + 0x28)[0]
            entsize = struct.unpack_from(end + "Q", data, b + 0x38)[0]
        else:
            name = struct.unpack_from(end + "I", data, b)[0]
            typ = struct.unpack_from(end + "I", data, b + 4)[0]
            offset = struct.unpack_from(end + "I", data, b + 0x10)[0]
            size = struct.unpack_from(end + "I", data, b + 0x14)[0]
            link = struct.unpack_from(end + "I", data, b + 0x18)[0]
            entsize = struct.unpack_from(end + "I", data, b + 0x24)[0]
        return typ, offset, size, link, entsize

    SHT_DYNAMIC = 6
    needed = []
    for i in range(e_shnum):
        typ, offset, size, link, entsize = section(i)
        if typ != SHT_DYNAMIC:
            continue
        _, str_off, str_size, _, _ = section(link)
        strtab = data[str_off:str_off + str_size]
        DT_NEEDED = 1
        ent = entsize or (16 if is64 else 8)
        for o in range(offset, offset + size, ent):
            if is64:
                tag, val = struct.unpack_from(end + "qQ", data, o)
            else:
                tag, val = struct.unpack_from(end + "iI", data, o)
            if tag == 0:
                break
            if tag == DT_NEEDED:
                e = strtab.find(b"\x00", val)
                needed.append(strtab[val:e].decode("ascii", "replace"))
    return needed


def fetch(url):
    req = urllib.request.Request(url, headers={"User-Agent": "aevum-poc/1"})
    with urllib.request.urlopen(req, timeout=90) as r:
        return r.read()


def main():
    idx = load_index()
    # soname -> 提供它的包：扫 Provides 不够，用 Packages 里的 Filename 列表无法直接拿 so。
    # 近似：用包名匹配 soname（libssl3 包提供 libssl.so.3）——足够本 PoC 比对召回。
    results = []
    for pkgname in SAMPLE:
        meta = idx.get(pkgname)
        if not meta or "Filename" not in meta:
            results.append({"package": pkgname, "status": "not_found"})
            continue
        try:
            blob = fetch(MIRROR + meta["Filename"])
            mem = ar_members(blob)
            data_key = next((k for k in mem if k.startswith("data.tar")), None)
            raw = mem[data_key]
            if data_key.endswith(".zst"):
                import subprocess
                raw = subprocess.run(["zstd", "-d", "-c"], input=raw,
                                     capture_output=True).stdout
            elif data_key.endswith(".xz"):
                import lzma; raw = lzma.decompress(raw)
            elif data_key.endswith(".gz"):
                raw = gzip.decompress(raw)
            tf = tarfile.open(fileobj=io.BytesIO(raw))
            all_needed = set()
            elf_count = 0
            for m in tf.getmembers():
                if not m.isfile():
                    continue
                f = tf.extractfile(m)
                head = f.read()
                nd = elf_needed(head)
                if nd is not None:
                    elf_count += 1
                    all_needed.update(nd)
            declared_libs = set(lib_atoms(meta.get("Depends", "")))
            # 把 NEEDED 的 soname (libssl.so.3) 粗映射到包名风格 (libssl3) 做交集估计
            def soname_to_pkgish(so):
                base = re.sub(r"\.so.*$", "", so)
                ver = re.findall(r"\.so\.(\d+)", so)
                return (base + (ver[0] if ver else "")).lower()
            needed_pkgish = {soname_to_pkgish(s) for s in all_needed}
            declared_norm = {d.lower() for d in declared_libs}
            hit = needed_pkgish & declared_norm
            results.append({
                "package": pkgname,
                "elf_files": elf_count,
                "elf_needed_sonames": sorted(all_needed),
                "declared_lib_deps": sorted(declared_libs),
                "declared_total_deps": len([a for a in meta.get("Depends", "").split(",") if a.strip()]),
                "declared_lib_count": len(declared_libs),
                "declared_nonlib_count": len([a for a in meta.get("Depends", "").split(",") if a.strip()]) - len(declared_libs),
                "approx_auto_hits": sorted(hit),
            })
            print(f"  ok {pkgname}: {elf_count} ELF, {len(all_needed)} sonames, "
                  f"{len(declared_libs)} declared-lib / {results[-1]['declared_nonlib_count']} declared-nonlib")
        except Exception as e:
            results.append({"package": pkgname, "status": f"error: {e}"})
            print(f"  ERR {pkgname}: {e}", file=sys.stderr)

    # 汇总：人工声明依赖里，lib vs nonlib 的总比例（nonlib = ELF 永远看不到的语义依赖）
    ok = [r for r in results if "declared_total_deps" in r]
    tot_deps = sum(r["declared_total_deps"] for r in ok)
    tot_lib = sum(r["declared_lib_count"] for r in ok)
    tot_nonlib = sum(r["declared_nonlib_count"] for r in ok)
    summary = {
        "sampled_ok": len(ok),
        "total_declared_deps": tot_deps,
        "declared_lib_deps": tot_lib,
        "declared_nonlib_deps": tot_nonlib,
        "nonlib_pct_invisible_to_elf": round(100 * tot_nonlib / tot_deps, 1) if tot_deps else 0,
    }
    out = {"summary": summary, "packages": results}
    (DATA / "step_b_result.json").write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")
    print("\n=== Step B summary ===")
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
