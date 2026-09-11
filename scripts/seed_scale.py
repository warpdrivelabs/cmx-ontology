#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
本体工作室 P1 规模造数脚本（方案 §八 P1 交付项：1000 类型造数脚本·临时版）。

覆盖三类验收场景：
  1. DAM 分布   —— 8 个域 × 应用 × 模块三级分布（auto 域默认视图播种出现）；
  2. 域消失     —— 一个"易逝域"内类型可整体清空（auto 虚拟条目随之自然消失）；
  3. 未分组     —— 一批 dam.domain 为空的类型（不播种 auto 视图，仅目录可见）。

附带造关系（跨域/域内，喂角标 +N 与域盒聚合边）、接口/共享属性/动作/函数薄清单。
幂等：默认先清理上次造数（cmx_origin.source='seed' 或 apiName 前缀匹配）再重灌。

用法（工作区任意位置）：
  python3 backend/cmx-ontology/scripts/seed_scale.py                # 读 onto-server-dev.toml 的 db_url
  python3 backend/cmx-ontology/scripts/seed_scale.py --url postgres://...
  python3 backend/cmx-ontology/scripts/seed_scale.py --clean        # 仅清理
  依赖 psql 命令行（subprocess 调用，与 sync_menu_db.py 同模式）。
"""
import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

WS = Path(__file__).resolve().parents[3]  # 工作区根
ONTO_TOML = WS / "backend/cmx-ontology/onto-server-dev.toml"
PREFIX = "Sd"  # 造数 apiName 前缀（Sd0001…），幂等清理锚点
DOMAINS = ["采购域", "销售域", "库存域", "生产域", "财务域", "人力域", "项目域", "易逝域"]
EPHEMERAL = "易逝域"  # 域消失演示域
UNGROUPED_N = 40
TOTAL = 1000


def resolve_db_url(cli_url: str | None) -> str:
    if cli_url:
        return cli_url
    env = os.environ.get("CMX_ONTO_DB_URL")
    if env:
        return env
    txt = ONTO_TOML.read_text(encoding="utf-8")
    m = re.search(r'db_url\s*=\s*"([^"]+)"', txt)
    if not m:
        sys.exit(f"✗ 未能从 {ONTO_TOML} 解析 db_url，请用 --url 或 CMX_ONTO_DB_URL 指定")
    return m.group(1)


def psql(url: str, sql: str, quiet: bool = False) -> str:
    env = dict(os.environ)
    m = re.match(r"postgres(?:ql)?://([^:]+):([^@]+)@", url)
    if m:
        env["PGPASSWORD"] = m.group(2)
    r = subprocess.run(
        ["psql", url, "-v", "ON_ERROR_STOP=1", "-X", "-q", "-A", "-t", "-c", sql],
        env=env, capture_output=True, text=True,
    )
    if r.returncode != 0:
        sys.exit(f"✗ psql 失败:\n{r.stderr}")
    if not quiet and r.stdout.strip():
        print(r.stdout.strip())
    return r.stdout.strip()


def clean(url: str) -> None:
    print("== 清理上次造数 ==")
    has_view = psql(url, "SELECT to_regclass('om_view') IS NOT NULL", quiet=True).strip()
    stmts = [
        f"DELETE FROM om_link_type WHERE api_name LIKE 'sd_%'",
        f"DELETE FROM om_object_type WHERE api_name LIKE '{PREFIX}%'",
        f"DELETE FROM om_interface WHERE api_name LIKE 'sd_if_%'",
        f"DELETE FROM om_shared_property WHERE api_name LIKE 'sd_sp_%'",
        f"DELETE FROM om_action_type WHERE api_name LIKE 'sd_act_%'",
        f"DELETE FROM om_function WHERE api_name LIKE 'sd_fn_%'",
    ]
    if has_view == "t":
        stmts.append("DELETE FROM om_view WHERE api_name LIKE 'scene_seed_%'")
    for stmt in stmts:
        psql(url, stmt, quiet=True)
    print("✓ 已清理（对象/关系/接口/共享属性/动作/函数/种子场景视图）")


def build_rows() -> tuple[list[dict], list[dict]]:
    objs: list[dict] = []
    per = TOTAL // len(DOMAINS)  # 每域基础配额
    idx = 0
    for di, dom in enumerate(DOMAINS):
        n = per + (TOTAL % len(DOMAINS) if di == 0 else 0)
        n = max(n - 6, 8) if dom == EPHEMERAL else n  # 易逝域小一点
        apps = ["应用A", "应用B", "应用C"]
        for i in range(n):
            idx += 1
            api = f"{PREFIX}{idx:04d}"
            app = apps[i % len(apps)]
            mod = f"模块{i % 4 + 1}"
            pk = "id"
            title = "name"
            props = [
                {"apiName": "id", "displayName": "ID", "baseType": "long", "required": True},
                {"apiName": "name", "displayName": "名称", "baseType": "string"},
                {"apiName": "amount", "displayName": "金额", "baseType": "decimal", "semanticType": "金额"},
                {"apiName": "createdAt", "displayName": "创建时间", "baseType": "timestamp"},
            ]
            objs.append({
                "api_name": api, "display_name": f"{dom}对象{i:03d}", "domain": dom,
                "application": app, "module": mod, "props": json.dumps(props, ensure_ascii=False),
                "pk": pk, "title": title, "status": "active" if i % 3 else "experimental",
            })
    # 未分组（dam 域空）
    for i in range(UNGROUPED_N):
        idx += 1
        objs.append({
            "api_name": f"{PREFIX}U{i:04d}", "display_name": f"未分组对象{i:03d}",
            "domain": "", "application": "", "module": "",
            "props": json.dumps([
                {"apiName": "id", "displayName": "ID", "baseType": "long", "required": True},
                {"apiName": "name", "displayName": "名称", "baseType": "string"},
            ], ensure_ascii=False), "pk": "id", "title": "name", "status": "experimental",
        })
    # 关系：域内链 + 跨域链（喂角标 +N）
    links: list[dict] = []
    named = [o for o in objs if o["domain"] and o["domain"] != EPHEMERAL]
    for i in range(len(named) - 1):
        a, b = named[i], named[i + 1]
        cross = a["domain"] != b["domain"]
        if not cross and i % 3:  # 域内边稀疏一点
            continue
        if i % 4 == 3:
            continue
        links.append({
            "api": f"sd_rel_{i:05d}", "display": f"关联{i:05d}",
            "a": a["api_name"], "b": b["api_name"],
            "card": "oneToMany" if i % 2 else "manyToMany",
        })
    return objs, links


def seed(url: str) -> None:
    clean(url)
    print("== 生成造数行 ==")
    objs, links = build_rows()
    print(f"对象 {len(objs)}（含未分组 {UNGROUPED_N}），关系 {len(links)}")

    now = "now()"
    # 接口/共享属性/动作/函数 薄清单（各 30/20/25/25）
    ifaces = [f"sd_if_{i:03d}" for i in range(30)]
    sps = [f"sd_sp_{i:03d}" for i in range(20)]
    acts = [f"sd_act_{i:03d}" for i in range(25)]
    fns = [f"sd_fn_{i:03d}" for i in range(25)]
    vals = ",".join(
        f"('{n}','种子接口{n}','[]','[]','active',{now},{now})" for n in ifaces
    )
    psql(url, f"INSERT INTO om_interface (api_name,display_name,properties,extends,status,created_at,updated_at) VALUES {vals} ON CONFLICT (api_name) DO NOTHING", quiet=True)
    vals = ",".join(f"('{n}','种子共享属性{n}','string',NULL,'',{now},{now})" for n in sps)
    psql(url, f"INSERT INTO om_shared_property (api_name,display_name,base_type,semantic_type,description,created_at,updated_at) VALUES {vals} ON CONFLICT (api_name) DO NOTHING", quiet=True)
    vals = ",".join(f"('{n}','种子动作{n}','[]','[]','[]','[]',NULL,'active',{now},{now})" for n in acts)
    psql(url, f"INSERT INTO om_action_type (api_name,display_name,parameters,logic,validations,side_effects,function_backing,status,created_at,updated_at) VALUES {vals} ON CONFLICT (api_name) DO NOTHING", quiet=True)
    vals = ",".join(f"('{n}','种子函数{n}','feel','query','[]','{{}}','','',{now},{now})".replace("'','',","'','',") for n in fns)
    psql(url, "INSERT INTO om_function (api_name,display_name,runtime,kind,inputs,output,body,description,status,created_at,updated_at) SELECT n,n,'feel','query','[]'::jsonb,'{}'::jsonb,'','','active',now(),now() FROM (VALUES " + ",".join(f"('{n}')" for n in fns) + ") AS t(n) ON CONFLICT (api_name) DO NOTHING", quiet=True)

    # 对象分批 INSERT（每批 100）
    print("== 写入对象 ==")
    batch = 100
    for i in range(0, len(objs), batch):
        chunk = objs[i:i + batch]
        values = []
        for o in chunk:
            dom = o["domain"]
            dam_json = json.dumps({"domain": dom, "application": o["application"], "module": o["module"]}, ensure_ascii=False)
            values.append(
                f"('{o['api_name']}','{o['display_name']}','','','',"
                f"'{o['pk']}','{o['title']}','{o['status']}',"
                f"'{o['props'].replace(chr(39), chr(39) * 2)}','[]','{dam_json.replace(chr(39), chr(39) * 2)}','{{}}',"
                f"NULL,NULL,1,{now},{now})"
            )
        psql(url, f"INSERT INTO om_object_type (api_name,display_name,description,icon,color,primary_key,title_property,status,properties,implements,dam,doc_type,datasource,cmx_origin,version,created_at,updated_at) VALUES {','.join(values)} ON CONFLICT (api_name) DO NOTHING", quiet=True)
        done = min(i + batch, len(objs))
        print(f"  对象 {done}/{len(objs)}")

    # 关系分批
    print("== 写入关系 ==")
    for i in range(0, len(links), 200):
        chunk = links[i:i + 200]
        values = []
        for l in chunk:
            values.append(
                f"('{l['api']}','{l['display']}','{l['card']}','{l['a']}','{l['b']}','relates','relatedBy','{{}}','active',{now},{now})"
            )
        psql(url, f"INSERT INTO om_link_type (api_name,display_name,cardinality,object_type_a,object_type_b,role_a,role_b,backing,status,created_at,updated_at) VALUES {','.join(values)} ON CONFLICT (api_name) DO NOTHING", quiet=True)
        print(f"  关系 {min(i + 200, len(links))}/{len(links)}")

    # 一个手动种子场景（场景管理演示）
    psql(url, "INSERT INTO om_view (api_name,display_name,description,dam,members,source,layout,version,created_at,updated_at) "
              "SELECT 'scene_seed_procure','种子场景·采购','造数附带的手动场景（采购域快照成员）','{\"domain\":\"采购域\"}'::jsonb,"
              "jsonb_build_object('objects',(SELECT COALESCE(jsonb_agg(api_name),'[]'::jsonb) FROM om_object_type WHERE dam->>'domain'='采购域' AND api_name LIKE 'Sd%'),'interfaces','[]'::jsonb),"
              "'manual','{}',1,now(),now() ON CONFLICT (api_name) DO NOTHING", quiet=True)

    n_obj = psql(url, f"SELECT COUNT(*) FROM om_object_type WHERE api_name LIKE '{PREFIX}%'", quiet=True)
    n_link = psql(url, "SELECT COUNT(*) FROM om_link_type WHERE api_name LIKE 'sd_rel_%'", quiet=True)
    print(f"✅ 造数完成：对象 {n_obj}，关系 {n_link}；易逝域「{EPHEMERAL}」可 DELETE 其对象后观察 auto 视图自然消失")


def main() -> None:
    ap = argparse.ArgumentParser(description="本体工作室 P1 规模造数")
    ap.add_argument("--url", default="", help="PG 连接串（缺省读 onto-server-dev.toml）")
    ap.add_argument("--clean", action="store_true", help="仅清理")
    args = ap.parse_args()
    url = resolve_db_url(args.url)
    print(f"目标库：{re.sub(r'://([^:]+):[^@]+@', r'://\\1:***@', url)}")
    if args.clean:
        clean(url)
    else:
        seed(url)


if __name__ == "__main__":
    main()
