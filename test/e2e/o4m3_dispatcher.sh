#!/usr/bin/env bash
# O4-M3 dispatcher E2E：动作副作用入 Outbox → 真投递（emitEvent→SSE / callFunction→O5 / webhook→deferred）。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 清空 oe_outbox 求确定性（dev 库；outbox 为瞬态）——含前次残留 processing/pending
docker run --rm -e PGPASSWORD=9lWsRQY4i4VToPpkHcCRFzki postgres:latest psql -h 192.168.157.46 -p 5432 -U dbuser_dba -d cmx_fico -q -c "DELETE FROM oe_outbox;" >/dev/null 2>&1 || true

# 对象类型 + 对象 + 函数（callFunction 目标）+ 动作（3 副作用）
curl "${H[@]}" -X POST "$B/object-types" -d '{"apiName":"DOrd","displayName":"订单","primaryKey":"id","titleProperty":"id","status":"active","properties":[{"apiName":"id","baseType":"string"},{"apiName":"status","baseType":"string"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/DOrd" -d '{"properties":{"id":"D-1","status":"open"}}' >/dev/null
curl "${H[@]}" -X POST "$B/functions" -d '{"apiName":"dPing","displayName":"探针","runtime":"feel","kind":"query","inputs":[],"output":{"type":"double"},"body":"1 + 1","status":"active"}' >/dev/null
curl "${H[@]}" -X POST "$B/action-types" -d '{
  "apiName":"dClose","displayName":"关闭+副作用","status":"active",
  "parameters":[{"name":"orderId","required":true}],
  "logic":[{"op":"modifyObject","objectType":"DOrd","pk":"$orderId","set":{"status":"closed"}}],
  "sideEffects":[
    {"kind":"emitEvent","topic":"orderClosed_$orderId"},
    {"kind":"callFunction","function":"dPing"},
    {"kind":"webhook","url":"https://hook.example/x"}
  ]}' >/dev/null

# 执行 → 3 副作用入 Outbox pending
EX=$(curl "${H[@]}" -X POST "$B/action-types/dClose/execute" -d '{"params":{"orderId":"D-1"},"subjects":["role:admin"]}')
chk "执行 effects=3" "echo '$EX' | grep -q '\"effects\":3'" "$EX"

# 订阅 SSE（长窗口）→ dispatch → emitEvent 应到达 SSE
SSE_OUT=$(mktemp)
stdbuf -oL curl -sN --max-time 20 "$B/events?tenant=default" > "$SSE_OUT" 2>/dev/null &
SSE_PID=$!
sleep 2
DP=$(curl "${H[@]}" -X POST "$B/action-outbox/dispatch" -d '{}')
echo "dispatch: $DP"
chk "派发 dispatched=2(emitEvent+callFunction)" "echo '$DP' | grep -qE '\"dispatched\":2'" "$DP"
chk "派发 deferred=1(webhook外部未接)" "echo '$DP' | grep -qE '\"deferred\":1'" "$DP"
chk "派发 failed=0" "echo '$DP' | grep -qE '\"failed\":0'" "$DP"
sleep 2
kill "$SSE_PID" 2>/dev/null || true
SSE=$(cat "$SSE_OUT"); rm -f "$SSE_OUT"
echo "sse: $SSE"
chk "SSE 收到 emitEvent(orderClosed_D-1)" "echo '$SSE' | grep -q 'orderClosed_D-1'" "$SSE"

# Outbox 状态：emitEvent/callFunction=dispatched，webhook=deferred，无 pending
OBd=$(curl "${H[@]}" "$B/action-outbox?status=dispatched&limit=50")
chk "Outbox emitEvent 转 dispatched" "echo '$OBd' | grep -q 'emitEvent'" "$OBd"
chk "Outbox callFunction 转 dispatched" "echo '$OBd' | grep -q 'callFunction'" "$OBd"
OBf=$(curl "${H[@]}" "$B/action-outbox?status=deferred&limit=50")
chk "Outbox webhook 转 deferred" "echo '$OBf' | grep -q 'webhook'" "$OBf"
# 再次 dispatch → 无 pending 可派（全 0）
DP2=$(curl "${H[@]}" -X POST "$B/action-outbox/dispatch" -d '{}')
chk "无 pending 再派 total=0(幂等)" "echo '$DP2' | grep -qE '\"total\":0'" "$DP2"

echo ""
echo "O4-M3 dispatcher E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
