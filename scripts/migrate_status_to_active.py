#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
存量状态迁移：experimental 全量转 active（方案 20260917 附录 B-9，用户裁决）。

背景：现状默认 experimental + 漏传静默降级导致库内大量 experimental；
读取侧分层过滤（D9）上线后默认口径仅返回 active，若不迁移默认视图将几乎全空。
全量转 active 与现状「全部可见」语义等价无缝。

行为：
  - 七类资源表 status='experimental' 的存量行直写 UPDATE 转 active
    （跳过 /lifecycle/transition 校验器，避免矩阵校验的对象/关系转换顺序依赖）；
  - 迁移完成后跑矩阵合规自检（全 active 后 (active,active) 三态皆可，存量关系任何
    状态均合法，自检必过，异常行输出报告）；
  - 输出迁移清单留档（kind / apiName / 时间），支持 --dry-run；幂等（重跑以清单为准
    不重复处理——已迁移行不再命中）。

用法（工作区任意位置）：
  python3 backend/cmx-ontology/scripts/migrate_status_to_active.py            # 读 onto-server-dev.toml
  python3 backend/cmx-ontology/scripts/migrate_status_to_active.py --url postgres://...
  python3 backend/cmx-ontology/scripts/migrate_status_to_active.py --dry-run
  依赖 psql 命令行（subprocess 调用，与 seed_scale.py 同模式）。
"""
import argparse
import datetime
import os
import json
import re
import subprocess
import sys
from pathlib import Path

WS = Path(__file__).resolve().parents[3]
ONTO_TOML = WS / "backend/cmx-ontology/onto-server-dev.toml"
ARCHIVE_DIR = WS / "documents" / "plans"

# 七类资源（kind → 表名；kind 与 /lifecycle/transition、om_revision.resource_kind 同值域）。
TABLES = {
    "object": "om_object_type",
    "link": "om_link_type",
    "interface": "om_interface",
    "shared_property": "om_shared_property",
    "action": "om_action_type",
    "function": "om_function",
    "view": "om_view",
}


def load_db_url() -> str:
    url = os.environ.get("ONTO_DB_URL", "").strip()
    if url:
        return url
    text = ONTO_TOML.read_text(encoding="utf-8")
    m = re.search(r'db_url\s*=\s*"([^"]+)"', text)
    if not m:
        sys.exit(f"未能在 {ONTO_TOML} 解析 db_url（或用 --url / ONTO_DB_URL 指定）")
    return m.group(1)


def psql(url: str, sql: str, tuples_only: bool = True) -> str:
    args = ["psql", url, "--no-psqlrc", "-v", "ON_ERROR_STOP=1"]
    if tuples_only:
        args.append("-tAc")
    else:
        args.extend(["-c"])
    args.append(sql)
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"psql 失败: {r.stderr.strip()}")
    return r.stdout.strip()


def main() -> None:
    ap = argparse.ArgumentParser(description="存量 experimental → active 迁移（方案附录 B-9）")
    ap.add_argument("--url", default="", help="postgres 连接串（缺省读 onto-server-dev.toml）")
    ap.add_argument("--dry-run", action="store_true", help="仅输出迁移清单，不落库")
    args = ap.parse_args()
    url = args.url or load_db_url()

    # 1. 迁移前清单（幂等锚点：仅 experimental 行）。
    rows = []
    for kind, table in TABLES.items():
        out = psql(
            url,
            f"SELECT api_name FROM {table} WHERE status = 'experimental' ORDER BY api_name",
        )
        names = [ln for ln in out.splitlines() if ln.strip()]
        for name in names:
            rows.append({"kind": kind, "apiName": name})
    print(f"待迁移（experimental → active）：{len(rows)} 项")
    for r in rows:
        print(f"  {r['kind']:16s} {r['apiName']}")

    if args.dry_run:
        print("（dry-run：未落库）")
        return
    if not rows:
        print("无需迁移。")
        return

    # 2. 直写 UPDATE（逐表一条；跳过 transition 校验器，避免矩阵校验顺序依赖）。
    total = 0
    for kind, table in TABLES.items():
        before = psql(url, f"SELECT count(*) FROM {table} WHERE status = 'experimental'")
        psql(
            url,
            f"UPDATE {table} SET status = 'active', updated_at = now() WHERE status = 'experimental'",
            tuples_only=False,
        )
        after = psql(url, f"SELECT count(*) FROM {table} WHERE status = 'experimental'")
        moved = int(before) - int(after)
        total += moved
        print(f"  {kind:16s} 迁移 {moved} 行（残留 experimental {after}）")

    # 3. 矩阵合规自检（附录 B-9）：全 active 后 (active,active) 三态皆可 → 任何关系状态合法；
    #    若仍有对象非 active（并发新资源），校验关系状态是否落在矩阵允许集内，异常行报告。
    bad = psql(
        url,
        """
        SELECT l.api_name, l.status, a.status, b.status
        FROM om_link_type l
        LEFT JOIN om_object_type a ON a.api_name = l.object_type_a
        LEFT JOIN om_object_type b ON b.api_name = l.object_type_b
        WHERE NOT (
          (a.status = 'deprecated' OR b.status = 'deprecated' AND l.status = 'deprecated')
          OR (a.status IS NOT DISTINCT FROM 'deprecated' AND l.status = 'deprecated')
          OR (COALESCE(a.status,'active') <> 'deprecated' AND COALESCE(b.status,'active') <> 'deprecated'
              AND (l.status = ANY(ARRAY['active','experimental','deprecated'])
                   OR (COALESCE(a.status,'active') = 'experimental' OR COALESCE(b.status,'active') = 'experimental')
                      AND l.status = 'experimental'))
        )
        """,
    )
    if bad:
        print("⚠️ 矩阵自检发现异常行：")
        for ln in bad.splitlines():
            print("  ", ln)
    else:
        print("✅ 矩阵合规自检通过（存量关系状态均落在矩阵允许集内）")

    # 4. 清单留档。
    stamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
    archive = ARCHIVE_DIR / f"migrate_status_to_active_{stamp}.json"
    archive.write_text(
        json.dumps({"migratedAt": stamp, "count": len(rows), "items": rows}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    print(f"✅ 迁移完成：{total} 行；清单留档 {archive}")


if __name__ == "__main__":
    main()
