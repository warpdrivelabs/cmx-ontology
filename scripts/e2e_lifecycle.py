#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
e2e API 测试：状态生命周期 / 修订历史 / 发布标记 / 场景 links（方案 20260917 P1-P3）。

直连本体服务（:8097），X-API-Key 开发免登录；测试数据 E2E 前缀隔离，结束清理。
用法：python3 backend/cmx-ontology/scripts/e2e_lifecycle.py [--base http://127.0.0.1:8097]
"""
import argparse
import datetime
import json
import sys
import urllib.error
import urllib.request

API_KEY = "cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
PFX = "E2Elc"  # 测试数据前缀（隔离 + 清理锚点）

PASS, FAIL = [], []


def req(base, method, path, body=None, expect=200):
    """请求 + 期望码校验；返回 (data, status)。非 JSON 错误体保底解析。"""
    url = base + path
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(url, data=data, method=method)
    r.add_header("Content-Type", "application/json")
    r.add_header("Accept", "application/json")
    r.add_header("X-API-Key", API_KEY)
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            status = resp.status
            raw = resp.read().decode()
    except urllib.error.HTTPError as e:
        status = e.code
        raw = e.read().decode()
    try:
        j = json.loads(raw)
    except Exception:
        j = {"raw": raw}
    payload = j.get("data", j) if isinstance(j, dict) else j
    if expect is not None and status != expect:
        raise AssertionError(f"{method} {path} → {status}（期望 {expect}）：{str(j)[:220]}")
    return payload, status


def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(f"{name}{(' — ' + detail) if (detail and not cond) else ''}")
    print(("  ✅ " if cond else "  ❌ ") + name + (f"  [{detail}]" if detail and not cond else ""))


def cleanup(base):
    """清理上次运行残留（E2E 前缀）。active 资源先降级（active 保护不豁免清理）。"""
    m, _ = req(base, "GET", f"/api/onto/v1/manifest?include=all", None)
    for t in m.get("objectTypes", []):
        if t["apiName"].startswith(PFX):
            if t.get("status") != "experimental":
                req(base, "POST", "/api/onto/v1/lifecycle/transition",
                    {"kind": "object", "apiName": t["apiName"], "target": "experimental"}, expect=None)
            req(base, "DELETE", f"/api/onto/v1/object-types/{t['apiName']}", None, expect=None)
    for l in m.get("linkTypes", []):
        if l["apiName"].startswith(PFX):
            if l.get("status") != "experimental":
                req(base, "POST", "/api/onto/v1/lifecycle/transition",
                    {"kind": "link", "apiName": l["apiName"], "target": "experimental"}, expect=None)
            req(base, "DELETE", f"/api/onto/v1/link-types/{l['apiName']}", None, expect=None)
    views, _ = req(base, "GET", "/api/onto/v1/views", None)
    for v in views if isinstance(views, list) else []:
        if v.get("apiName", "").startswith(PFX):
            req(base, "POST", "/api/onto/v1/views/remove", {"apiName": v["apiName"]}, expect=None)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8097")
    args = ap.parse_args()
    base = args.base.rstrip("/")

    print("── 清理残留 ──")
    cleanup(base)

    print("── P1-1 save 剥离 status：新建默认 experimental，status 字段被忽略并返回 warning ──")
    req(base, "POST", "/api/onto/v1/object-types", {
        "apiName": f"{Pfx if False else PFX}ObjA", "displayName": "e2e对象A", "primaryKey": "code",
        "titleProperty": "code", "status": "active",
        "properties": [{"apiName": "code", "baseType": "string", "required": True}],
    })
    m, _ = req(base, "GET", "/api/onto/v1/manifest", None)
    in_default = any(t["apiName"] == f"{PFX}ObjA" for t in m["objectTypes"])
    check("新建（请求带 status=active）落库为 experimental → 默认 manifest 不含", not in_default)
    m_all, _ = req(base, "GET", "/api/onto/v1/manifest?include=all", None)
    row = next((t for t in m_all["objectTypes"] if t["apiName"] == f"{PFX}ObjA"), None)
    check("include=all 可见且 status=experimental", row and row["status"] == "experimental", str(row and row["status"]))

    print("── P1-2 默认仅 active / include 逐级打开 ──")
    m_exp, _ = req(base, "GET", "/api/onto/v1/manifest?include=experimental", None)
    check("include=experimental 后可见", any(t["apiName"] == f"{PFX}ObjA" for t in m_exp["objectTypes"]))

    print("── P1-3 transition：experimental → active（先 dryRun 预览）──")
    pre, _ = req(base, "POST", "/api/onto/v1/lifecycle/transition", {
        "kind": "object", "apiName": f"{PFX}ObjA", "target": "active", "dryRun": True})
    check("dryRun 返回且无级联", pre.get("dryRun") is True and pre.get("cascade") == [])
    out, _ = req(base, "POST", "/api/onto/v1/lifecycle/transition", {
        "kind": "object", "apiName": f"{PFX}ObjA", "target": "active", "changeNote": "e2e 激活"})
    check("流转成功 experimental → active", out.get("from") == "experimental" and out.get("to") == "active")
    m, _ = req(base, "GET", "/api/onto/v1/manifest", None)
    check("激活后默认 manifest 可见", any(t["apiName"] == f"{PFX}ObjA" for t in m["objectTypes"]))

    print("── P1-4 active 保护：删除 409 / 改主键 409 ──")
    _, st = req(base, "DELETE", f"/api/onto/v1/object-types/{PFX}ObjA", None, expect=None)
    check("删除 active 对象 → 409", st == 409, f"got {st}")
    g, _ = req(base, "GET", f"/api/onto/v1/object-types/{PFX}ObjA", None)
    g["primaryKey"] = "code2"
    g["properties"].append({"apiName": "code2", "baseType": "string"})
    _, st = req(base, "POST", "/api/onto/v1/object-types", g, expect=None)
    check("active 改主键 → 409", st == 409, f"got {st}")

    print("── P1-5 废弃必填元数据 + 读取 404 语义 ──")
    _, st = req(base, "POST", "/api/onto/v1/lifecycle/transition", {
        "kind": "object", "apiName": f"{PFX}ObjA", "target": "deprecated"}, expect=None)
    check("缺弃用元数据 → 400", st == 400, f"got {st}")
    out, _ = req(base, "POST", "/api/onto/v1/lifecycle/transition", {
        "kind": "object", "apiName": f"{PFX}ObjA", "target": "deprecated",
        "deprecation": {"reason": "e2e 废弃", "sunsetAt": "2027-12-31", "replacementApiName": f"{PFX}ObjB"},
        "changeNote": "e2e 废弃"})
    check("带元数据废弃成功", out.get("to") == "deprecated")
    g, _ = req(base, "GET", f"/api/onto/v1/object-types/{PFX}ObjA", None)
    d = g.get("deprecation") or {}
    check("GET 详情回读弃用元数据四列", d.get("reason") == "e2e 废弃" and d.get("sunsetAt") == "2027-12-31" and bool(d.get("deprecatedAt")))
    # 数据链路 404：写一条数据 → 默认 load 404；include=deprecated 200
    req(base, "POST", f"/api/onto/v1/objects/{PFX}ObjA", {"properties": {"code": "K1", "code2": "K2"}}, expect=None)
    _, st = req(base, "POST", "/api/onto/v1/object-sets/load",
                {"objectSet": {"op": "base", "objectType": f"{PFX}ObjA"}}, expect=None)
    check("deprecated 未 include：load → 404", st == 404, f"got {st}")
    _, st = req(base, "POST", "/api/onto/v1/object-sets/load",
                {"objectSet": {"op": "base", "objectType": f"{PFX}ObjA"}, "include": "deprecated"}, expect=None)
    check("include=deprecated：load → 200", st == 200, f"got {st}")
    # 写入豁免（D2）：deprecated 类型照常可写
    _, st = req(base, "POST", f"/api/onto/v1/objects/{PFX}ObjA", {"properties": {"code": "K3", "code2": "K3"}}, expect=None)
    check("写入不拦截（软治理 D2）→ 200", st == 200, f"got {st}")

    print("── P1-6 兼容矩阵与机械级联 ──")
    # 对象 B（active）+ 关系 A(deprecated)-B
    req(base, "POST", "/api/onto/v1/object-types", {
        "apiName": f"{PFX}ObjB", "displayName": "e2e对象B", "primaryKey": "code", "titleProperty": "code",
        "properties": [{"apiName": "code", "baseType": "string", "required": True}]})
    req(base, "POST", "/api/onto/v1/lifecycle/transition", {"kind": "object", "apiName": f"{PFX}ObjB", "target": "active"})
    _, st = req(base, "POST", "/api/onto/v1/link-types", {
        "apiName": f"{PFX}L1", "displayName": "e2e关系", "cardinality": "oneToMany",
        "objectTypeA": f"{PFX}ObjA", "objectTypeB": f"{PFX}ObjB"}, expect=None)
    check("新建关系含 deprecated 端 → 409（N14）", st == 409, f"got {st}")
    # 对象 A 复活 → experimental，再建关系：默认 experimental 恰好合法
    req(base, "POST", "/api/onto/v1/lifecycle/transition", {"kind": "object", "apiName": f"{PFX}ObjA", "target": "experimental"})
    g, _ = req(base, "GET", f"/api/onto/v1/object-types/{PFX}ObjA", None)
    check("复活后弃用元数据已清空", not g.get("deprecation"))
    req(base, "POST", "/api/onto/v1/link-types", {
        "apiName": f"{PFX}L1", "displayName": "e2e关系", "cardinality": "oneToMany",
        "objectTypeA": f"{PFX}ObjA", "objectTypeB": f"{PFX}ObjB"})
    # 矩阵：A(experimental)-B(active) → 关系仅允许 experimental；转 active 被拒
    _, st = req(base, "POST", "/api/onto/v1/lifecycle/transition", {
        "kind": "link", "apiName": f"{PFX}L1", "target": "active"}, expect=None)
    check("矩阵拒绝：试验端关系转 active → 409", st == 409, f"got {st}")
    # 机械级联：A → deprecated ⇒ 关系自动降 deprecated（级联 1 条）
    out, _ = req(base, "POST", "/api/onto/v1/lifecycle/transition", {
        "kind": "object", "apiName": f"{PFX}ObjA", "target": "deprecated",
        "deprecation": {"reason": "级联源", "sunsetAt": "2027-06-30"}})
    cas = out.get("cascade") or []
    check(f"对象废弃级联关系自动降级（{len(cas)} 条）", len(cas) == 1 and cas[0]["apiName"] == f"{PFX}L1" and cas[0]["to"] == "deprecated", str(cas))
    m_all, _ = req(base, "GET", "/api/onto/v1/manifest?include=all", None)
    lrow = next((l for l in m_all["linkTypes"] if l["apiName"] == f"{PFX}L1"), {})
    check("级联降级后关系 status=deprecated 且自动填充弃用元数据",
          lrow.get("status") == "deprecated" and (lrow.get("deprecation") or {}).get("reason", "").startswith("级联降级"))
    # 机械级联：A(deprecated) → experimental ⇒ 强制 experimental（离开 deprecated 清空元数据）
    out, _ = req(base, "POST", "/api/onto/v1/lifecycle/transition", {"kind": "object", "apiName": f"{PFX}ObjA", "target": "experimental"})
    cas = out.get("cascade") or []
    check("对象转试验：deprecated 对象端关系强制 experimental", len(cas) == 1 and cas[0]["to"] == "experimental", str(cas))
    m_all, _ = req(base, "GET", "/api/onto/v1/manifest?include=all", None)
    lrow = next((l for l in m_all["linkTypes"] if l["apiName"] == f"{PFX}L1"), {})
    check("强制对齐后关系元数据已清空", lrow.get("status") == "experimental" and not lrow.get("deprecation"))
    # save 剥离校验：带 status 保存 → warning
    g, _ = req(base, "GET", f"/api/onto/v1/link-types/{PFX}L1", None)
    g["displayName"] = "e2e关系v2"
    r, _ = req(base, "POST", "/api/onto/v1/link-types", g)
    check("save 带 status 返回结构化 warning", isinstance(r.get("warnings"), list) and any("status" in w for w in r["warnings"]), str(r.get("warnings")))

    print("── P2-1 修订历史与 revert ──")
    revs, _ = req(base, "GET", f"/api/onto/v1/revisions?kind=object&apiName={PFX}ObjA", None)
    check(f"对象修订历史 {len(revs)} 条（保存/流转各留痕）", len(revs) >= 3, str(len(revs)))
    # 再改一次描述 → 新修订；然后 revert 到第一版
    g, _ = req(base, "GET", f"/api/onto/v1/object-types/{PFX}ObjA", None)
    g["description"] = "v2描述"
    req(base, "POST", "/api/onto/v1/object-types", g)
    revs, _ = req(base, "GET", f"/api/onto/v1/revisions?kind=object&apiName={PFX}ObjA", None)
    top = max(revs, key=lambda x: x["revision"])
    first = min(revs, key=lambda x: x["revision"])
    det, _ = req(base, "GET", f"/api/onto/v1/revisions/detail?id={first['id']}", None)
    out, _ = req(base, "POST", "/api/onto/v1/revisions/revert", {
        "kind": "object", "apiName": f"{PFX}ObjA", "revision": first["revision"], "changeNote": "e2e revert"})
    g, _ = req(base, "GET", f"/api/onto/v1/object-types/{PFX}ObjA", None)
    check("revert 到首修订：description 回到旧值", g.get("description", "") == (det.get("payload") or {}).get("description", ""), g.get("description", ""))
    revs2, _ = req(base, "GET", f"/api/onto/v1/revisions?kind=object&apiName={PFX}ObjA", None)
    check("revert 产生新修订（历史只追加）", len(revs2) == len(revs) + 1, f"{len(revs)}→{len(revs2)}")

    print("── P2-2 删除墓碑 + revert 恢复 ──")
    req(base, "POST", "/api/onto/v1/object-types", {
        "apiName": f"{PFX}ObjT", "displayName": "e2e墓碑对象", "primaryKey": "code", "titleProperty": "code",
        "properties": [{"apiName": "code", "baseType": "string", "required": True}]})
    req(base, "DELETE", f"/api/onto/v1/object-types/{PFX}ObjT", None)
    deleted, _ = req(base, "GET", "/api/onto/v1/revisions?deleted=true&kind=object", None)
    hit = next((d for d in deleted if d.get("apiName") == f"{PFX}ObjT"), None)
    check("删除写墓碑（deleted 清单可见）", hit is not None)
    out, _ = req(base, "POST", "/api/onto/v1/revisions/revert", {
        "kind": "object", "apiName": f"{PFX}ObjT", "revision": hit["revision"]})  # 三元组契约
    check("墓碑 revert 自动升级创建语义（restored=true）", out.get("restored") is True, str(out))
    g, _ = req(base, "GET", f"/api/onto/v1/object-types/{PFX}ObjT", None)
    check("恢复后定义完整", g.get("apiName") == f"{PFX}ObjT")

    print("── P2-3 发布门禁与 tag ──")
    req(base, "POST", "/api/onto/v1/snapshots", {"summary": "e2e 存档基线"})
    _, st = req(base, "POST", "/api/onto/v1/releases", {"tag": "e2e-v1"}, expect=None)
    gate = st == 409
    check("发布门禁：live 含非 active 资源 → 409（需 acknowledgeWarnings）", gate, f"got {st}")
    rel, st = req(base, "POST", "/api/onto/v1/releases", {"tag": "e2e-v1", "note": "e2e 发布", "acknowledgeWarnings": True}, expect=None)
    if st is None or st != 200:
        # 库内可能已存在同名 tag（重跑）——先解除再打
        req(base, "POST", "/api/onto/v1/releases/remove", {"tag": "e2e-v1"}, expect=None)
        rel, _ = req(base, "POST", "/api/onto/v1/releases", {"tag": "e2e-v1", "note": "e2e 发布", "acknowledgeWarnings": True})
    else:
        check("acknowledgeWarnings 放行发布成功", True)
    check("发布返回 tag 与 version", bool(rel.get("tag")) and bool(rel.get("version")), str(rel))
    vs, _ = req(base, "GET", "/api/onto/v1/versions", None)
    vrow = next((v for v in vs if v.get("tag") == "e2e-v1"), None)
    check("版本列表带 archivedBy/tag/releaseNote 字段", vrow is not None and "archivedAt" in vrow, str(bool(vrow)))
    # tag 复用规则：live 无变化再打新 tag → 新行同 rev
    rel2, _ = req(base, "POST", "/api/onto/v1/releases", {"tag": "e2e-v2", "note": "同内容新 tag", "acknowledgeWarnings": True})
    check("同 rev 换 tag → 复用行（tag NULL 才复用；此处 v1 行已带 tag → 新行）",
          rel2.get("reused") is False and rel2.get("version") != rel.get("version"), str(rel2))
    req(base, "POST", "/api/onto/v1/releases/remove", {"tag": "e2e-v2"})
    req(base, "POST", "/api/onto/v1/releases/remove", {"tag": "e2e-v1"})

    print("── P3-1 场景 links 白名单与 availableLinks ──")
    req(base, "POST", "/api/onto/v1/views", {
        "apiName": f"{PFX}scene", "displayName": "e2e场景", "source": "manual",
        "members": {"objects": [f"{PFX}ObjA", f"{PFX}ObjB"], "interfaces": [], "links": []}, "version": 0})
    g, _ = req(base, "GET", f"/api/onto/v1/graph?view={PFX}scene", None)
    check("未加 links：两端在场无边", g["spec"]["edges"] == [], str(g["spec"]["edges"]))
    avail = g.get("availableLinks") or []
    check("availableLinks 实时给出可加入关系", any(a["apiName"] == f"{PFX}L1" for a in avail), str([a.get('apiName') for a in avail]))
    # 加入白名单 → 边出现
    v0 = next(v for v in (req(base, "GET", "/api/onto/v1/views", None)[0]) if v["apiName"] == f"{PFX}scene")
    req(base, "POST", "/api/onto/v1/views", {
        "apiName": f"{PFX}scene", "displayName": "e2e场景", "source": "manual",
        "members": {"objects": [f"{PFX}ObjA", f"{PFX}ObjB"], "interfaces": [], "links": [f"{PFX}L1"]},
        "version": v0["version"]})
    g, _ = req(base, "GET", f"/api/onto/v1/graph?view={PFX}scene", None)
    check("加入白名单后边出现", any(e["apiName"] == f"{PFX}L1" for e in g["spec"]["edges"]))
    check("加入后 availableLinks 清空", (g.get("availableLinks") or []) == [])

    print("── P3-2 数据链路场景过滤 ──")
    # 状态与场景过滤取交集（方案 §7.3）：先把成员与关系转 active，排除状态口径干扰。
    req(base, "POST", "/api/onto/v1/lifecycle/transition", {"kind": "object", "apiName": f"{PFX}ObjA", "target": "active"})
    req(base, "POST", "/api/onto/v1/lifecycle/transition", {"kind": "object", "apiName": f"{PFX}ObjB", "target": "active"})
    req(base, "POST", "/api/onto/v1/lifecycle/transition", {"kind": "link", "apiName": f"{PFX}L1", "target": "active"})
    _, st = req(base, "POST", "/api/onto/v1/object-sets/load",
                {"objectSet": {"op": "base", "objectType": f"{PFX}ObjT"}, "view": f"{PFX}scene"}, expect=None)
    check("场景模式查非成员类型 → 409", st == 409, f"got {st}")
    _, st = req(base, "POST", "/api/onto/v1/object-sets/load",
                {"objectSet": {"op": "base", "objectType": f"{PFX}ObjA"}, "view": f"{PFX}scene"}, expect=None)
    check("场景成员类型 → 200", st == 200, f"got {st}")
    mv, _ = req(base, "GET", f"/api/onto/v1/manifest?view={PFX}scene", None)
    names = [t["apiName"] for t in mv.get("objectTypes", [])]
    check("manifest?view= 六段口径（仅场景成员）", set(names) == {f"{PFX}ObjA", f"{PFX}ObjB"}, str(names))
    check("场景内关系随 links 白名单", [l["apiName"] for l in mv.get("linkTypes", [])] == [f"{PFX}L1"])
    # Search-Around：先把对象 A 写数据 + 建边
    req(base, "POST", f"/api/onto/v1/objects/{PFX}ObjA", {"properties": {"code": "A1", "code2": "A1"}})
    req(base, "POST", f"/api/onto/v1/objects/{PFX}ObjB", {"properties": {"code": "B1"}})
    req(base, "POST", "/api/onto/v1/links", {"link": f"{PFX}L1", "aPk": "A1", "bPk": "B1"})
    _, st = req(base, "GET", f"/api/onto/v1/objects/{PFX}ObjA/A1/links/{PFX}L1?view={PFX}scene", None, expect=None)
    check("Search-Around 场景内关系 → 200", st == 200, f"got {st}")
    # 从场景移除该关系 → 钻取 409
    v0 = next(v for v in (req(base, "GET", "/api/onto/v1/views", None)[0]) if v["apiName"] == f"{PFX}scene")
    req(base, "POST", "/api/onto/v1/views", {
        "apiName": f"{PFX}scene", "displayName": "e2e场景", "source": "manual",
        "members": {"objects": [f"{PFX}ObjA", f"{PFX}ObjB"], "interfaces": [], "links": []},
        "version": v0["version"]})
    _, st = req(base, "GET", f"/api/onto/v1/objects/{PFX}ObjA/A1/links/{PFX}L1?view={PFX}scene", None, expect=None)
    check("Search-Around 白名单外关系 → 409", st == 409, f"got {st}")

    print("── P3-3 删除关系 → 场景引用级联清理 ──")
    v0 = next(v for v in (req(base, "GET", "/api/onto/v1/views", None)[0]) if v["apiName"] == f"{PFX}scene")
    req(base, "POST", "/api/onto/v1/views", {
        "apiName": f"{PFX}scene", "displayName": "e2e场景", "source": "manual",
        "members": {"objects": [f"{PFX}ObjA", f"{PFX}ObjB"], "interfaces": [], "links": [f"{PFX}L1"]},
        "version": v0["version"]})
    # active 保护生效中：直接删 → 409；先降级 experimental 再删
    _, st = req(base, "DELETE", f"/api/onto/v1/link-types/{PFX}L1", None, expect=None)
    check("active 关系删除被拒 → 409", st == 409, f"got {st}")
    req(base, "POST", "/api/onto/v1/lifecycle/transition", {"kind": "link", "apiName": f"{PFX}L1", "target": "experimental"})
    out, st = req(base, "DELETE", f"/api/onto/v1/link-types/{PFX}L1", None, expect=None)
    check("删除关系成功且返回 sceneRefs", st == 200 and out.get("deleted") is True, f"{st} {str(out)[:120]}")
    check("响应含受影响场景明细", any(s.get("apiName") == f"{PFX}scene" for s in (out.get("sceneRefs") or [])), str(out.get("sceneRefs")))
    v0 = next(v for v in (req(base, "GET", "/api/onto/v1/views", None)[0]) if v["apiName"] == f"{PFX}scene")
    check("场景 links 已级联移除该关系", f"{PFX}L1" not in (v0["members"].get("links") or []))
    vrevs, _ = req(base, "GET", f"/api/onto/v1/revisions?kind=view&apiName={PFX}scene", None)
    check("受影响场景各留一条修订", any("级联清理" in (r.get("changeNote") or "") for r in vrevs), str(len(vrevs)))

    print("── 清理测试数据 ──")
    cleanup(base)
    for vn in [f"{PFX}scene"]:
        req(base, "POST", "/api/onto/v1/views/remove", {"apiName": vn}, expect=None)

    print(f"\n══ e2e 结果：通过 {len(PASS)} / 失败 {len(FAIL)} ══")
    if FAIL:
        for f in FAIL:
            print("  ❌", f)
        sys.exit(1)


if __name__ == "__main__":
    main()
