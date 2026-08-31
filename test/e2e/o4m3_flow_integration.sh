#!/usr/bin/env bash
# onto ↔ flowengine 跨服务真联调 E2E：onto 动作 startBusinessProcess 副作用 → dispatcher 经 X-API-Key
# 调 flow v1 /instances → flowengine **真建流程实例**。断言实例存在 + businessKey/变量确来自 onto。
#
# 前置：① onto-server :8097 以 ONTO_FLOW_API_KEY=<flow服务key> 启动（flowUrl 默认 :8091）；
#       ② flow-server :8091 在线（CONFIG_FILE=flow-server-local.toml，jwt+api_keys，DB=192.168.157.46/cmx_fico）。
set -uo pipefail
OB="http://127.0.0.1:8097/api/onto/v1"; FB="http://127.0.0.1:8091/api/flow/v1"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
OH=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
FH=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
DIR="$(cd "$(dirname "$0")" && pwd)"
UNIQ=$(date +%s); BIZ="onto-int-$UNIQ"; ORD="FI-$UNIQ"
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }
command -v jq >/dev/null 2>&1 || { echo "需要 jq"; exit 1; }

# —— 前置断言 ——
CFG=$(curl "${OH[@]}" "$OB/action-outbox/config"); echo "onto config: $CFG"
chk "onto flowUrl=:8091" "echo '$CFG' | grep -q '\"flowUrl\":\"http://127.0.0.1:8091\"'" "$CFG"
chk "onto flowApiKeySet=true(带服务身份调 flow)" "echo '$CFG' | grep -q '\"flowApiKeySet\":true'" "$CFG"
DEFPROBE=$(curl "${FH[@]}" -o /dev/null -w "%{http_code}" "$FB/definitions")
chk "flowengine :8091 在线" "[ '$DEFPROBE' = '200' ]" "http=$DEFPROBE"

# —— flow：部署 + 发布 BPMN（key 由 process id 决定；幂等，已存在忽略）——
BODY=$(jq -Rs '{name:"本体联调审批", bpmnXml:.}' "$DIR/_onto_int_flow.bpmn")
DKEY=$(curl "${FH[@]}" -X POST "$FB/definitions/draft" -d "$BODY" | jq -r '.data.key // "onto_int_approve"')
curl "${FH[@]}" -X POST "$FB/definitions/$DKEY/publish" -d '{"note":"onto-int"}' >/dev/null 2>&1 || true
chk "流程定义已发布 key=$DKEY" "[ -n '$DKEY' ]" ""

# —— onto：种对象 + 动作（modifyObject 事务 + startBusinessProcess 副作用；businessKey/orderId 唯一）——
curl "${OH[@]}" -X POST "$OB/object-types" -d '{"apiName":"FiOrd","displayName":"联调订单","primaryKey":"id","titleProperty":"id","status":"active","properties":[{"apiName":"id","baseType":"string"},{"apiName":"status","baseType":"string"}]}' >/dev/null
curl "${OH[@]}" -X POST "$OB/objects/FiOrd" -d '{"properties":{"id":"FI-1","status":"open"}}' >/dev/null
curl "${OH[@]}" -X POST "$OB/action-types" -d "{
  \"apiName\":\"fiStart\",\"displayName\":\"发起审批流\",\"status\":\"active\",
  \"parameters\":[{\"name\":\"orderId\",\"required\":true},{\"name\":\"bizKey\",\"required\":true}],
  \"logic\":[{\"op\":\"modifyObject\",\"objectType\":\"FiOrd\",\"pk\":\"FI-1\",\"set\":{\"status\":\"processing\"}}],
  \"sideEffects\":[{\"kind\":\"startBusinessProcess\",\"flowDefKey\":\"$DKEY\",\"businessKey\":\"\$bizKey\",\"orderId\":\"\$orderId\"}]
}" >/dev/null

EX=$(curl "${OH[@]}" -X POST "$OB/action-types/fiStart/execute" -d "{\"params\":{\"orderId\":\"$ORD\",\"bizKey\":\"$BIZ\"},\"subjects\":[\"role:admin\"]}")
chk "onto 执行 fiStart effects=1" "echo '$EX' | grep -q '\"effects\":1'" "$EX"

# —— dispatch → onto 经 X-API-Key 调 flow 真起实例 ——
DP=$(curl "${OH[@]}" -X POST "$OB/action-outbox/dispatch" -d '{}'); echo "dispatch: $DP"
chk "dispatch dispatched>=1" "echo '$DP' | grep -qE '\"dispatched\":[1-9]'" "$DP"
chk "dispatch failed=0" "echo '$DP' | grep -qE '\"failed\":0'" "$DP"
OBX=$(curl "${OH[@]}" "$OB/action-outbox?status=dispatched&limit=50")
chk "onto outbox startBusinessProcess→dispatched" "echo '$OBX' | grep -q startBusinessProcess" "$OBX"

sleep 0.5
# —— flow 侧验证：按唯一 businessKey 定位 onto 真起的实例（轮询抗列表延迟/最新窗口）——
IID=""; LIST=""
for _ in 1 2 3 4 5 6; do
  LIST=$(curl "${FH[@]}" "$FB/instances")
  IID=$(echo "$LIST" | jq -r --arg bk "$BIZ" '.data.instances[]|select(.businessKey==$bk)|.id' | head -1)
  [ -n "$IID" ] && break
  sleep 0.5
done
chk "flow 实例列表含 businessKey=$BIZ(onto 真起)" "[ -n '$IID' ]" "$(echo "$LIST" | jq -c '.data.instances[0]//empty' 2>/dev/null)"
DET=$(curl "${FH[@]}" "$FB/instances/${IID:-none}")
DEFOK=$(echo "$DET" | jq -r --arg k "$DKEY" 'if .data.definitionKey==$k then 1 else 0 end' 2>/dev/null)
ORDOK=$(echo "$DET" | jq -r --arg o "$ORD" 'if .data.variables.orderId==$o then 1 else 0 end' 2>/dev/null)
NODEOK=$(echo "$DET" | jq -r 'if (.data.activeNodes|index("approve")) then 1 else 0 end' 2>/dev/null)
chk "flow 实例 definitionKey=$DKEY" "[ \"$DEFOK\" = 1 ]" "$DET"
chk "flow 实例变量 orderId=$ORD(onto 透传)" "[ \"$ORDOK\" = 1 ]" "$(echo "$DET" | jq -c '.data.variables' 2>/dev/null)"
chk "flow 实例停在 approve(BPMN 已执行)" "[ \"$NODEOK\" = 1 ]" "$(echo "$DET" | jq -c '.data.activeNodes' 2>/dev/null)"

echo ""
echo "onto↔flow 跨服务联调 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
