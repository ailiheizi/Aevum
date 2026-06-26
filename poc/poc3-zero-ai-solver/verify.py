#!/usr/bin/env python3
"""PoC-3 验证：可复现性、override 生效、多模板。结果写 JSON 避免控制台编码问题。"""
import json
import io
from pathlib import Path
import solver as S

OUT = Path(__file__).parent / "out"
OUT.mkdir(exist_ok=True)

by_name, provides = S.load_index()
report = {"checks": [], "templates": {}}


def solve(tname, overrides=None):
    overrides = overrides or {}
    closure, diag = S.resolve(by_name, provides, S.TEMPLATES[tname], overrides)
    return S.build_lock(closure, diag, S.TEMPLATES[tname], overrides)


# 1. 所有模板求解
for t in S.TEMPLATES:
    lock = solve(t)
    report["templates"][t] = {
        "closure_id": lock["closure_id"],
        "package_count": lock["package_count"],
        "unresolved": len(lock["diagnostics"]["unresolved"]),
        "virtual_resolved": len(lock["diagnostics"]["virtual_resolved"]),
        "alt_chosen": len(lock["diagnostics"]["alt_chosen"]),
    }

# 2. 可复现性：同输入连解 3 次，closure_id 必须全等
ids = [solve("cli-tools")["closure_id"] for _ in range(3)]
report["checks"].append({
    "name": "determinism_same_input_3x",
    "ids": ids,
    "pass": len(set(ids)) == 1,
})

# 3. 跨模板隔离：不同模板 closure_id 必须不同
all_ids = {t: report["templates"][t]["closure_id"] for t in S.TEMPLATES}
report["checks"].append({
    "name": "distinct_templates_distinct_ids",
    "pass": len(set(all_ids.values())) == len(all_ids),
})

# 4. override(exclude)生效：排除 curl 后，web-server 闭包应变化且 closure_id 不同
base = solve("web-server")
excl = solve("web-server", {"curl": "exclude"})
report["checks"].append({
    "name": "override_exclude_changes_closure",
    "base_id": base["closure_id"],
    "excluded_id": excl["closure_id"],
    "base_count": base["package_count"],
    "excluded_count": excl["package_count"],
    "pass": base["closure_id"] != excl["closure_id"],
})

# 5. lock 可重放：从 lock 的 locked 列表能否原样还原 closure_id（纯数据，无需再求解、无需 AI）
def replay(lock):
    blob = "\n".join(f"{x['name']}@{x['version']}#{x['fingerprint']}" for x in lock["locked"])
    import hashlib
    return "clo-" + hashlib.sha256(blob.encode()).hexdigest()[:16]
cli = solve("cli-tools")
report["checks"].append({
    "name": "lock_replay_without_solving",
    "original": cli["closure_id"],
    "replayed": replay(cli),
    "pass": cli["closure_id"] == replay(cli),
})

# 6. 内容寻址：闭包内包是否带真实 sha256 指纹(来自 Debian 索引)
sha_count = sum(1 for x in cli["locked"] if x["fingerprint"].startswith("sha256:"))
report["checks"].append({
    "name": "real_content_addressed_fingerprints",
    "with_sha256": sha_count,
    "total": cli["package_count"],
    "pass": sha_count == cli["package_count"],
})

report["all_pass"] = all(c["pass"] for c in report["checks"])
(OUT / "verify_result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
print(json.dumps(report, ensure_ascii=False, indent=2))
