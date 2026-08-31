#!/usr/bin/env bash
# O4-M3 dispatcher **真投递** E2E：webhook 真发 HTTP + startBusinessProcess 调 flow v1，
# 用本地 sink(:8770) 捕获真出站请求并断言 payload/插值/租户头/终态。
#
# 前置：onto-server 须以 ONTO_FLOW_URL=http://127.0.0.1:8770 ONTO_WEBHOOK_ALLOW=127.0.0.1,localhost
# 启动（restart 时注入；本测先查 /action-outbox/config 确认，不满足直接 FAIL 提示）。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
CAP="/tmp/onto-sink-captured.jsonl"; SINK_PORT=8770
DIR="$(cd "$(dirname "$0")" && pwd)"
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

command -v node >/dev/null 2>&1 || { echo "需要 node（本地 sink）"; exit 1; }

# 前置：确认 onto 出站已指向 sink（restart 注入 env）——否则本测无意义
CFG=$(curl "${H[@]}" "$B/action-outbox/config"); echo "config: $CFG"
chk "onto flowUrl 指向 sink :$SINK_PORT" "echo '$CFG' | grep -q '\"flowUrl\":\"http://127.0.0.1:$SINK_PORT\"'" "$CFG"
chk "webhook 白名单含 127.0.0.1" "echo '$CFG' | grep -q '127.0.0.1'" "$CFG"

# 起本地 sink
CAP=$CAP SINK_PORT=$SINK_PORT node "$DIR/_dispatch_sink.cjs" > /tmp/onto-sink.log 2>&1 &
SINK_PID=$!
trap 'kill $SINK_PID 2>/dev/null || true' EXIT
for i in $(seq 1 30); do curl -s -m1 "http://127.0.0.1:$SINK_PORT/__ping" >/dev/null 2>&1 && break; sleep 0.3; done
chk "sink 就绪" "curl -s -m2 http://127.0.0.1:$SINK_PORT/__ping | grep -q ok" "$(cat /tmp/onto-sink.log)"

# 清 oe_outbox 求确定性（dev 库；outbox 为瞬态）
docker run --rm -e PGPASSWORD=9lWsRQY4i4VToPpkHcCRFzki postgres:latest psql -h 192.168.157.46 -p 5432 -U dbuser_dba -d cmx_fico -q -c "DELETE FROM oe_outbox;" >/dev/null 2>&1 || true

# 种类型 + 对象 + 动作（webhook + startBusinessProcess 两副作用，target/payload 均含 $orderId 插值）
curl "${H[@]}" -X POST "$B/object-types" -d '{"apiName":"DsOrd","displayName":"订单","primaryKey":"id","titleProperty":"id","status":"active","properties":[{"apiName":"id","baseType":"string"},{"apiName":"status","baseType":"string"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/DsOrd" -d '{"properties":{"id":"DS-1","status":"open"}}' >/dev/null
curl "${H[@]}" -X POST "$B/action-types" -d '{
  "apiName":"dsFire","displayName":"触发外部","status":"active",
  "parameters":[{"name":"orderId","required":true}],
  "logic":[{"op":"modifyObject","objectType":"DsOrd","pk":"$orderId","set":{"status":"closed"}}],
  "sideEffects":[
    {"kind":"webhook","url":"http://127.0.0.1:8770/hook/onto","event":"order.fired","orderId":"$orderId"},
    {"kind":"startBusinessProcess","flowDefKey":"approve_$orderId","orderId":"$orderId"}
  ]}' >/dev/null

EX=$(curl "${H[@]}" -X POST "$B/action-types/dsFire/execute" -d '{"params":{"orderId":"DS-1"},"subjects":["role:admin"]}')
chk "执行 effects=2" "echo '$EX' | grep -q '\"effects\":2'" "$EX"

DP=$(curl "${H[@]}" -X POST "$B/action-outbox/dispatch" -d '{}'); echo "dispatch: $DP"
chk "派发 dispatched=2(webhook+startBusinessProcess 真投递)" "echo '$DP' | grep -qE '\"dispatched\":2'" "$DP"
chk "派发 failed=0" "echo '$DP' | grep -qE '\"failed\":0'" "$DP"
chk "派发 deferred=0" "echo '$DP' | grep -qE '\"deferred\":0'" "$DP"

sleep 0.5
CAPD=$(cat "$CAP" 2>/dev/null); echo "--- captured ---"; echo "$CAPD"; echo "----------------"
# startBusinessProcess 真到达 sink（flow v1 起实例契约）
chk "sink 收到 flow 起实例 POST /api/flow/v1/instances" "grep -q '/api/flow/v1/instances' '$CAP'" "$CAPD"
chk "flow definitionKey=approve_DS-1(target 插值)" "grep '/api/flow/v1/instances' '$CAP' | grep -q 'approve_DS-1'" "$CAPD"
chk "flow variables 含 orderId=DS-1" "grep '/api/flow/v1/instances' '$CAP' | grep -q '\"orderId\":\"DS-1\"'" "$CAPD"
chk "flow 请求带 X-Tenant:default(租户隔离)" "grep '/api/flow/v1/instances' '$CAP' | grep -qi '\"x-tenant\":\"default\"'" "$CAPD"
# webhook 真到达 sink
chk "sink 收到 webhook POST /hook/onto" "grep -q '/hook/onto' '$CAP'" "$CAPD"
chk "webhook payload orderId=DS-1(插值)" "grep '/hook/onto' '$CAP' | grep -q '\"orderId\":\"DS-1\"'" "$CAPD"
chk "webhook payload event=order.fired" "grep '/hook/onto' '$CAP' | grep -q '\"event\":\"order.fired\"'" "$CAPD"

# Outbox 终态 dispatched（两条）
OBd=$(curl "${H[@]}" "$B/action-outbox?status=dispatched&limit=50")
chk "Outbox webhook→dispatched" "echo '$OBd' | grep -q 'webhook'" "$OBd"
chk "Outbox startBusinessProcess→dispatched" "echo '$OBd' | grep -q 'startBusinessProcess'" "$OBd"

# 对象编辑随动作事务已提交
OBJ=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"DsOrd"}}')
chk "对象 DS-1 status=closed(动作事务提交)" "echo '$OBJ' | grep -q 'closed'" "$OBJ"

# 再派幂等
DP2=$(curl "${H[@]}" -X POST "$B/action-outbox/dispatch" -d '{}')
chk "无 pending 再派 total=0(幂等)" "echo '$DP2' | grep -qE '\"total\":0'" "$DP2"

echo ""
echo "O4-M3 真投递 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
