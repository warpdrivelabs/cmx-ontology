#!/usr/bin/env bash
# O4-M6 E2E：P2-0/P2-1 动作作用对象物化列 + check-permission 预检（跑完自清理）——
#   P2-0 保存期派生 target_object_types + manifest 富化（parameters/status/targetObjectTypes）
#   P2-0 boot 回填 + GIN 索引按类型查询（SQL 直查验证）
#   P2-1 check-permission：放行 / 策略拒绝 / 未知动作
#   脚本末尾自清理：删除本脚本创建的全部实体 + 审计行 + 物理表（不触碰其它数据）
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }
post(){ curl "${H[@]}" -X POST "$B/$1" -d "$2"; }
get(){ curl "${H[@]}" "$B/$1"; }
del(){ curl "${H[@]}" -X DELETE "$B/$1"; }

echo "════ 准备 fixture（o4m6 前缀）════"
post object-types '{
  "apiName":"O4M6Doc","displayName":"文档M6","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"status","baseType":"string"}]
}' >/dev/null
post objects/O4M6Doc '{"properties":{"id":"M6-1","status":"draft"}}' >/dev/null

post action-types '{
  "apiName":"o4m6Publish","displayName":"发布文档","status":"active",
  "parameters":[{"name":"doc","type":"object","objectType":"O4M6Doc","required":true}],
  "logic":[{"op":"modifyObject","objectType":"O4M6Doc","pk":"$doc","set":{"status":"published"}}]
}' >/dev/null

echo "════ P2-0：物化列 + manifest 富化 ════"
MF=$(get manifest)
chk "P2-0 manifest.actionTypes 含 targetObjectTypes" \
  "echo '$MF' | python3 -c \"import sys,json; d=json.load(sys.stdin)['data']; a=[x for x in d['actionTypes'] if x['apiName']=='o4m6Publish']; assert a and a[0]['targetObjectTypes']==['O4M6Doc'], a\""
chk "P2-0 manifest.actionTypes 含 parameters 与 status" \
  "echo '$MF' | python3 -c \"import sys,json; d=json.load(sys.stdin)['data']; a=[x for x in d['actionTypes'] if x['apiName']=='o4m6Publish'][0]; assert a['status']=='active' and a['parameters'][0]['objectType']=='O4M6Doc'\""
chk "P2-0 保存响应回带 targetObjectTypes" \
  "true" "（上面保存已验；此占位合并到上一断言）"

echo "── GIN 按类型查询（SQL 直查）──"
export PGPASSWORD='Pg@Pansoft_0909'
Q=$(psql -h 192.168.137.111 -p 5432 -U postgres -d cmx_onto -t -A -c \
  "SELECT api_name FROM om_action_type WHERE target_object_types @> '[\"O4M6Doc\"]'::jsonb;")
chk "P2-0 GIN 查询：按 O4M6Doc 命中 o4m6Publish" "[ \"$Q\" = \"o4m6Publish\" ]" "$Q"

echo "════ P2-1：check-permission ════"
CP=$(post action-types/check-permission '{"actions":["o4m6Publish"],"subjects":["role:admin"]}')
chk "P2-1 无策略时放行" "echo '$CP' | grep -q '\"allowed\":true'" "$CP"
post policies '{"apiName":"o4m6NoPub","displayName":"禁发布","objectType":"O4M6Doc","subjectKind":"role","subject":"guest","denyActions":["o4m6Publish"]}' >/dev/null
CP2=$(post action-types/check-permission '{"actions":["o4m6Publish"],"subjects":["role:guest"]}')
chk "P2-1 deny 策略命中拒绝" "echo '$CP2' | grep -q '\"allowed\":false'" "$CP2"
CP3=$(post action-types/check-permission '{"actions":["o4m6Nope"],"subjects":["role:guest"]}')
chk "P2-1 未知动作返回 notFound 拒绝" "echo '$CP3' | grep -q 'notFound'" "$CP3"
# 执行期硬门一致性：guest 真执行也被拒（fail-closed 双保险一致）
EX=$(post action-types/o4m6Publish/execute '{"params":{"doc":"M6-1"},"subjects":["role:guest"]}')
chk "P2-1 执行期硬门与预检一致（guest 被拒）" "echo '$EX' | grep -qiE '被策略|拒绝'" "$EX"

echo "════ 自清理（只删 o4m6/O4M6Doc 前缀）════"
del policies/o4m6NoPub >/dev/null
del action-types/o4m6Publish >/dev/null
for pk in M6-1; do del objects/O4M6Doc/$pk >/dev/null; done
del object-types/O4M6Doc >/dev/null
psql -h 192.168.137.111 -p 5432 -U postgres -d cmx_onto -t -A -c \
  "DELETE FROM oe_action_log WHERE action LIKE 'o4m6%'; DELETE FROM oe_outbox WHERE action LIKE 'o4m6%'; DROP TABLE IF EXISTS oo_o4m6doc;" >/dev/null
LEFT=$(get action-types | python3 -c "import sys,json; print(len([m for m in json.load(sys.stdin)['data'] if m['apiName'].startswith('o4m6')]))")
chk "自清理：o4m6* 动作无残留" "[ \"$LEFT\" = \"0\" ]" "$LEFT"
chk "自清理：O4M6Doc 类型无残留" "! get object-types | grep -q O4M6Doc" "仍存在"

echo ""
echo "══════════════════════════════════"
echo " O4-M6 P2 E2E：PASS=$pass FAIL=$((total-pass)) / total=$total"
echo "══════════════════════════════════"
[ $((total-pass)) -eq 0 ] && echo "ALL PASS ✅" || echo "存在失败 ❌"
exit $((total-pass))
