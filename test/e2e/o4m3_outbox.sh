#!/usr/bin/env bash
# O4-M3 副作用 Outbox E2E：动作带 side_effects → 提交后入 oe_outbox（同事务）；dry-run/失败不入；dispatcher 回标。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 对象类型 M3Ord + 种 O-1
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"M3Ord","displayName":"订单","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"owner","baseType":"string"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/M3Ord" -d '{"properties":{"id":"O-1","owner":"U-1"}}' >/dev/null

# 动作 m3Reassign：改 owner + 两个副作用（触发流程 approve_$orderId / webhook）
curl "${H[@]}" -X POST "$B/action-types" -d '{
  "apiName":"m3Reassign","displayName":"改派+触发","status":"active",
  "parameters":[{"name":"orderId","required":true},{"name":"newOwner","required":true}],
  "logic":[{"op":"modifyObject","objectType":"M3Ord","pk":"$orderId","set":{"owner":"$newOwner"}}],
  "sideEffects":[
    {"kind":"startBusinessProcess","flowDefKey":"approve_$orderId"},
    {"kind":"webhook","url":"https://hook.example/$newOwner"}
  ]}' >/dev/null

# 1) dry-run：不入 Outbox
# 先清掉此前跑残留的 approve_O-1 pending（本测试幂等：只关心「本轮 dry-run 不新增」）。
PRE=$(curl "${H[@]}" "$B/action-outbox?status=pending&limit=100")
for pid in $(echo "$PRE" | grep -oE '"id":[0-9]+,"action":"m3Reassign"' | grep -oE '[0-9]+'); do
  curl "${H[@]}" -X POST "$B/action-outbox/$pid/dispatched" -d '{"ok":true}' >/dev/null
done
DR=$(curl "${H[@]}" -X POST "$B/action-types/m3Reassign/dry-run" -d '{"params":{"orderId":"O-1","newOwner":"U-9"}}')
chk "dry-run effects=0" "echo '$DR' | grep -q '\"effects\":0'" "$DR"
OB0=$(curl "${H[@]}" "$B/action-outbox?status=pending&limit=50")
chk "dry-run 后无 O-1 相关 pending outbox" "! echo '$OB0' | grep -q 'approve_O-1'" "$OB0"

# 2) 真正执行：写回 + 2 条副作用入 Outbox
EX=$(curl "${H[@]}" -X POST "$B/action-types/m3Reassign/execute" -d '{"params":{"orderId":"O-1","newOwner":"U-9"},"actor":"t"}')
echo "execute: $EX"
chk "执行 effects=2" "echo '$EX' | grep -q '\"effects\":2'" "$EX"
chk "执行 committed" "echo '$EX' | grep -q '\"status\":\"committed\"'" "$EX"
LD=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"M3Ord"}}')
chk "编辑写回 owner=U-9" "echo '$LD' | grep -q 'U-9'" "$LD"

# 3) Outbox 有两条 pending，含插值后的 target + kind
OB=$(curl "${H[@]}" "$B/action-outbox?status=pending&limit=50")
echo "outbox: $OB"
chk "Outbox 含 startBusinessProcess approve_O-1" "echo '$OB' | grep -q 'approve_O-1'" "$OB"
chk "Outbox 含 webhook 插值 URL" "echo '$OB' | grep -q 'hook.example/U-9'" "$OB"
chk "Outbox 关联 logId" "echo '$OB' | grep -qE '\"logId\":[0-9]+'" "$OB"

# 4) dispatcher 回标一条为 dispatched
OID=$(echo "$OB" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
MK=$(curl "${H[@]}" -X POST "$B/action-outbox/$OID/dispatched" -d '{"ok":true}')
chk "标记投递成功" "echo '$MK' | grep -q '\"updated\":true'" "$MK"
OBD=$(curl "${H[@]}" "$B/action-outbox?status=dispatched&limit=50")
chk "该条转 dispatched" "echo '$OBD' | grep -q \"\\\"id\\\":$OID\"" "$OBD"

# 5) 失败编辑 → Outbox 也回滚（不残留 pending）
BADO=$(curl "${H[@]}" "$B/action-outbox?status=pending&limit=50" | grep -oE 'NOPEHOOK' | wc -l | tr -d ' ')
curl "${H[@]}" -X POST "$B/action-types" -d '{
  "apiName":"m3Bad","displayName":"必失败","status":"active",
  "parameters":[{"name":"orderId","required":true}],
  "logic":[{"op":"modifyObject","objectType":"M3Ord","pk":"$orderId","set":{"owner":"X"}}],
  "sideEffects":[{"kind":"webhook","url":"https://NOPEHOOK/$orderId"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/action-types/m3Bad/execute" -d '{"params":{"orderId":"GHOST"}}' >/dev/null
OBB=$(curl "${H[@]}" "$B/action-outbox?limit=100")
chk "失败动作的 webhook 未入 Outbox（随事务回滚）" "! echo '$OBB' | grep -q 'NOPEHOOK'" "$OBB"

echo ""
echo "O4-M3 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
