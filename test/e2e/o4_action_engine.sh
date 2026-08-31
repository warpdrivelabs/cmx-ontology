#!/usr/bin/env bash
# O4 动作引擎 E2E：建对象类型 Order + 动作 reassignOrder → 种一个对象 → dry-run → 执行 → 校验写回 + 审计。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 1) 建对象类型 Order（主键 id，属性 id/owner/status）
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O4Order","displayName":"订单","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"owner","baseType":"string"},{"apiName":"status","baseType":"string"}]
}' >/dev/null

# 2) 建动作 reassignOrder：参数 orderId/newOwner（必填）；logic = 改 owner + 置 status=reassigned
curl "${H[@]}" -X POST "$B/action-types" -d '{
  "apiName":"o4Reassign","displayName":"改派订单","status":"active",
  "parameters":[{"name":"orderId","type":"scalar","required":true},{"name":"newOwner","type":"scalar","required":true}],
  "logic":[{"op":"modifyObject","objectType":"O4Order","pk":"$orderId","set":{"owner":"$newOwner","status":"reassigned"}}]
}' >/dev/null

# 3) 种一个对象 O-1（owner=U-1, status=open）
curl "${H[@]}" -X POST "$B/objects/O4Order" -d '{"properties":{"id":"O-1","owner":"U-1","status":"open"}}' >/dev/null

# 4) dry-run：应返回 dryRun + edits，但不改库
DR=$(curl "${H[@]}" -X POST "$B/action-types/o4Reassign/dry-run" -d '{"params":{"orderId":"O-1","newOwner":"U-9"}}')
echo "dry-run resp: $DR"
chk "dry-run 标 dryRun=true" "echo '$DR' | grep -q '\"dryRun\":true'" "$DR"
chk "dry-run 出 1 条编辑" "echo '$DR' | grep -q '\"applied\":1'" "$DR"
# dry-run 后对象 owner 仍为 U-1
OW0=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"O4Order"}}')
chk "dry-run 未改库（owner 仍 U-1）" "echo '$OW0' | grep -q 'U-1' && ! echo '$OW0' | grep -q 'U-9'" "$OW0"

# 5) 缺必填参数 → 应报错
MISS=$(curl "${H[@]}" -X POST "$B/action-types/o4Reassign/execute" -d '{"params":{"orderId":"O-1"}}')
chk "缺必填参数被拒" "echo '$MISS' | grep -qiE 'newOwner|必填|required|error|false'" "$MISS"

# 6) 真正执行 → 写回
EX=$(curl "${H[@]}" -X POST "$B/action-types/o4Reassign/execute" -d '{"params":{"orderId":"O-1","newOwner":"U-9"},"actor":"tester"}')
echo "execute resp: $EX"
chk "执行 committed" "echo '$EX' | grep -q '\"status\":\"committed\"'" "$EX"
chk "执行返 logId" "echo '$EX' | grep -qE '\"logId\":[0-9]+'" "$EX"

# 7) 校验写回：owner=U-9, status=reassigned
LD=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"O4Order"}}')
echo "loaded: $LD"
chk "写回 owner=U-9" "echo '$LD' | grep -q 'U-9'" "$LD"
chk "写回 status=reassigned" "echo '$LD' | grep -q 'reassigned'" "$LD"

# 8) 审计：至少 2 行（dryRun + committed），含 action=o4Reassign
LOG=$(curl "${H[@]}" "$B/action-logs?action=o4Reassign&limit=10")
echo "logs: $LOG"
chk "审计含 committed" "echo '$LOG' | grep -q 'committed'" "$LOG"
chk "审计含 dryRun" "echo '$LOG' | grep -q 'dryRun'" "$LOG"
chk "审计记 actor=tester" "echo '$LOG' | grep -q 'tester'" "$LOG"

# 9) modify 不存在对象 → 失败 + 回滚 + failed 审计
BAD=$(curl "${H[@]}" -X POST "$B/action-types/o4Reassign/execute" -d '{"params":{"orderId":"NOPE","newOwner":"U-9"}}')
chk "modify 不存在对象报错" "echo '$BAD' | grep -qiE '不存在|error|false'" "$BAD"

echo ""
echo "O4 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
