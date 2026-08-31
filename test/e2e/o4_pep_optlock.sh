#!/usr/bin/env bash
# O4-写侧 E2E：PEP（策略 deny_actions 拒动作）+ 乐观锁（expectedUpdatedAt 冲突→conflict）。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 对象类型 + 对象 + 动作（改 status）
curl "${H[@]}" -X POST "$B/object-types" -d '{"apiName":"O4pOrd","displayName":"订单","primaryKey":"id","titleProperty":"id","status":"active","properties":[{"apiName":"id","baseType":"string"},{"apiName":"status","baseType":"string"},{"apiName":"note","baseType":"string"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O4pOrd" -d '{"properties":{"id":"P-1","status":"open","note":"n0"}}' >/dev/null
curl "${H[@]}" -X POST "$B/action-types" -d '{"apiName":"o4pClose","displayName":"关闭","status":"active","parameters":[{"name":"orderId","required":true}],"logic":[{"op":"modifyObject","objectType":"O4pOrd","pk":"$orderId","set":{"status":"closed"}}],"validations":[],"sideEffects":[]}' >/dev/null

# ── 写侧 PEP ──
# 策略：role:clerk 拒执行 o4pClose（deny_actions）
curl "${H[@]}" -X POST "$B/policies" -d '{"apiName":"clerkNoClose","displayName":"文员禁关单","objectType":"O4pOrd","subjectKind":"role","subject":"clerk","denyActions":["o4pClose"]}' >/dev/null

# clerk 执行 → 被拒
DENY=$(curl "${H[@]}" -X POST "$B/action-types/o4pClose/execute" -d '{"params":{"orderId":"P-1"},"subjects":["role:clerk"]}')
echo "clerk: $DENY"
chk "写侧 PEP：clerk 被策略拒" "echo '$DENY' | grep -qiE '被策略|PEP|拒绝'" "$DENY"
LD=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"O4pOrd"}}')
chk "被拒后未写回（status 仍 open）" "echo '$LD' | grep -q 'open' && ! echo '$LD' | grep -q 'closed'" "$LD"

# manager 执行 → 放行（无 deny 策略）
OK=$(curl "${H[@]}" -X POST "$B/action-types/o4pClose/execute" -d '{"params":{"orderId":"P-1"},"subjects":["role:manager"]}')
echo "manager: $OK"
chk "写侧 PEP：manager 放行 committed" "echo '$OK' | grep -q '\"status\":\"committed\"'" "$OK"
LD2=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"O4pOrd"}}')
chk "manager 写回 status=closed" "echo '$LD2' | grep -q 'closed'" "$LD2"

# ── 乐观锁 ──
# 读当前 updated_at（用 modify 空 set 探当前？改用直接读对象 → 需要 updatedAt）——先做一次 modify 取 updatedAt
M1=$(curl "${H[@]}" -X POST "$B/objects/O4pOrd/P-1/modify" -d '{"set":{"note":"n1"}}')
echo "m1: $M1"
UAT=$(echo "$M1" | grep -oE '"updatedAt":"[^"]+"' | head -1 | sed 's/.*"updatedAt":"//;s/"//')
chk "盲写 modify 返 ok + updatedAt" "echo '$M1' | grep -q '\"status\":\"ok\"' && [ -n '$UAT' ]" "$M1"

# 用正确 expectedUpdatedAt → 成功
M2=$(curl "${H[@]}" -X POST "$B/objects/O4pOrd/P-1/modify" -d "{\"set\":{\"note\":\"n2\"},\"expectedUpdatedAt\":\"$UAT\"}")
echo "m2: $M2"
chk "乐观锁：正确版本 → ok" "echo '$M2' | grep -q '\"status\":\"ok\"'" "$M2"
NEWUAT=$(echo "$M2" | grep -oE '"updatedAt":"[^"]+"' | head -1 | sed 's/.*"updatedAt":"//;s/"//')

# 用旧（过期）expectedUpdatedAt → 冲突
M3=$(curl "${H[@]}" -X POST "$B/objects/O4pOrd/P-1/modify" -d "{\"set\":{\"note\":\"n3\"},\"expectedUpdatedAt\":\"$UAT\"}")
echo "m3: $M3"
chk "乐观锁：过期版本 → conflict(code=0)" "echo '$M3' | grep -q '\"conflict\":true' && echo '$M3' | grep -q '\"code\":0'" "$M3"
chk "冲突回带当前值(note=n2 未被 n3 覆盖)" "echo '$M3' | grep -q 'n2' && ! echo '$M3' | grep -q 'n3'" "$M3"

# 不存在对象 → notFound
M4=$(curl "${H[@]}" -X POST "$B/objects/O4pOrd/GHOST/modify" -d '{"set":{"note":"x"}}')
chk "modify 不存在对象 → notFound" "echo '$M4' | grep -q '\"status\":\"notFound\"'" "$M4"

echo ""
echo "O4-写侧 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
