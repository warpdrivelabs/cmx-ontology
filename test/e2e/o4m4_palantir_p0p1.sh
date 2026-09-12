#!/usr/bin/env bash
# O4-M4 E2E：动作引擎 Palantir 对标 P0+P1（方案 v2.1 验收）——
#   P0-1 函数背书（Run function rule 最小闭环 + validations 保留 + 互斥保存）
#   P0-2 提交校验引用对象状态（objects.<param>.<property>）
#   P0-3 值映射五来源（param/static/currentUser/currentTime/paramProperty）+ logic 拼接内插
#   P0-4 组合序列校验（四规则，含 upsert 矩阵）+ 保存期拦截
#   P0-5 createOrModifyObject（upsert：新 pk 建 / 旧 pk 合并不抹属性）
#   P1-1 dry-run 响应升级（proposedChanges diff / executionLog / 双通道错误）
#   P1-2 参数体系（multipleChoice / defaultValue / hidden / 保存期派生 derivedParams）
#   P1-3 execute-batch（同事务全回滚 / 上限 / 失败批审计）
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }
post(){ curl "${H[@]}" -X POST "$B/$1" -d "$2"; }
get(){ curl "${H[@]}" "$B/$1"; }
# 对象读取（GET /objects/{type}/{pk} 无路由）：object-sets/load static 查询
objget(){ post object-sets/load "{\"objectSet\":{\"op\":\"static\",\"objectType\":\"$1\",\"primaryKeys\":[\"$2\"]},\"limit\":10}"; }

echo "════ P0 准备：对象类型 + FEEL 函数 ════"
post object-types '{
  "apiName":"O4M4Order","displayName":"订单M4","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"owner","baseType":"string"},
    {"apiName":"status","baseType":"string"},{"apiName":"amount","baseType":"double"},
    {"apiName":"memo","baseType":"string"},{"apiName":"closedBy","baseType":"string"},
    {"apiName":"closedAt","baseType":"string"},{"apiName":"customerId","baseType":"string"},
    {"apiName":"tier","baseType":"string"}]
}' >/dev/null

# 函数 closeByQty：入参 order(object，壳层装载) → 返回编辑 JSON（Run function rule 最小闭环）。
# 注意：FEEL 表达式不支持 { 对象字面量，返回编辑 JSON 的函数用 Rhai `#{...}` map（serde 桥 → JSON）。
post functions '{
  "apiName":"o4m4CloseFn","displayName":"关单函数","runtime":"rhai","kind":"actionLogic",
  "inputs":[{"name":"order","type":"object"}],
  "body":"#{ \"edits\": [ #{ \"op\": \"modifyObject\", \"objectType\": \"O4M4Order\", \"pk\": order.pk, \"set\": #{ \"status\": \"closed\", \"tier\": order.tier } } ] }"
}' >/dev/null

echo "════ P0-2/P0-3：提交校验引用对象状态 + 五来源值映射 ════"
post action-types '{
  "apiName":"o4m5Close","displayName":"关单五来源","status":"active",
  "parameters":[{"name":"orderId","type":"object","objectType":"O4M4Order","required":true},
                {"name":"newOwner","type":"string","required":true}],
  "validations":[{"expression":"objects.orderId.status == '\''open'\''","message":"仅 open 状态可关单"}],
  "logic":[{"op":"modifyObject","objectType":"O4M4Order","pk":"$orderId","set":{
      "owner":{"src":"param","name":"newOwner"},
      "memo":{"src":"static","value":"年度结转"},
      "closedBy":{"src":"currentUser"},
      "closedAt":{"src":"currentTime"},
      "customerId":{"src":"paramProperty","param":"orderId","property":"customerId"}}}]
}' >/dev/null

# 种对象（upsert，幂等重跑）：O-1(open, C-7, tier=T1) 与 O-2(closed)
post objects/O4M4Order '{"properties":{"id":"O-1","owner":"U-1","status":"open","amount":100,"customerId":"C-7","tier":"T1"}}' >/dev/null
post objects/O4M4Order '{"properties":{"id":"O-2","owner":"U-1","status":"closed","amount":50,"customerId":"C-9","tier":"T2"}}' >/dev/null

DR=$(post action-types/o4m5Close/dry-run '{"params":{"orderId":"O-1","newOwner":"U-9"},"actor":"e2e-admin"}')
chk "P0-2 校验通过（对象状态 open）" "echo '$DR' | grep -q '\"status\":\"dryRun\"'" "$DR"
chk "P1-1 dry-run 返回 proposedChanges" "echo '$DR' | grep -q 'proposedChanges'" "$DR"
chk "P1-1 diff from→to（owner U-1→U-9）" "echo '$DR' | grep -q '\"from\":\"U-1\"' && echo '$DR' | grep -q '\"to\":\"U-9\"'" "$DR"
chk "P1-1 executionLog 含 submissionCriteria 与 pepCheck" "echo '$DR' | grep -q 'submissionCriteria' && echo '$DR' | grep -q 'pepCheck'" "$DR"
chk "P0-3 currentUser/currentTime/paramProperty 在 diff 生效" "echo '$DR' | grep -q 'e2e-admin' && echo '$DR' | grep -q 'C-7' && echo '$DR' | grep -q 'closedAt'" "$DR"

# 校验拦截：closed 对象提交 → 双通道错误（userMessage + adminDetail + executionLog）
ER=$(post action-types/o4m5Close/execute '{"params":{"orderId":"O-2","newOwner":"U-9"},"actor":"e2e"}')
chk "P0-2 校验拦截 closed 对象（userMessage）" "echo '$ER' | grep -q '仅 open 状态可关单'" "$ER"
chk "P1-1 adminDetail 含失败表达式" "echo '$ER' | grep -q 'adminDetail'" "$ER"
chk "P1-1 错误携带 executionLog" "echo '$ER' | grep -q 'executionLog'" "$ER"
EX=$(post action-types/o4m5Close/execute '{"params":{"orderId":"O-1","newOwner":"U-9"},"actor":"e2e-admin"}')
chk "P0-3 五来源执行写回（closedBy/customerId/memo）" \
  "echo '$EX' | grep -q 'e2e-admin' && echo '$EX' | grep -q 'C-7' && echo '$EX' | grep -q '年度结转'" "$EX"
OBJ=$(objget O4M4Order O-1)
chk "P0-3 写回落库验证（closedBy=C-7=memo）" "echo '$OBJ' | grep -q '年度结转' && echo '$OBJ' | grep -q 'C-7'" "$OBJ"

echo "════ P0-1：函数背书（Run function rule）════"
MUT=$(post action-types '{"apiName":"o4m5BadFn","displayName":"非法组合","status":"active","functionBacking":"o4m4CloseFn","logic":[{"op":"deleteObject","objectType":"O4M4Order","pk":"X"}],"parameters":[]}')
chk "P0-1 function_backing+logic 保存被拒" "echo '$MUT' | grep -q '不能同时配置'" "$MUT"
# 合法函数动作：object 型入参显式声明（含 objectType 供壳层装载）；validations 保留
post action-types '{
  "apiName":"o4m5FnClose","displayName":"函数关单","status":"active",
  "functionBacking":"o4m4CloseFn",
  "validations":[{"expression":"objects.order.status == '\''open'\''","message":"仅 open 可函数关单"}],
  "parameters":[{"name":"order","type":"object","objectType":"O4M4Order","required":true}]
}' >/dev/null
FR=$(post action-types/o4m5FnClose/execute '{"params":{"order":"O-1"},"actor":"fnexec"}')
chk "P0-1 函数动作真实写回（committed）" "echo '$FR' | grep -q '\"status\":\"committed\"'" "$FR"
FO=$(objget O4M4Order O-1)
chk "P0-1 函数编辑落库（status=closed，tier 透传 T1）" "echo '$FO' | grep -q '\"tier\":\"T1\"' && echo '$FO' | grep -q '\"status\":\"closed\"'" "$FO"
# 函数动作的 validations 仍生效（O-2 已 closed）
FR2=$(post action-types/o4m5FnClose/execute '{"params":{"order":"O-2"},"actor":"fnexec"}')
chk "P0-1 函数动作保留提交校验" "echo '$FR2' | grep -q '仅 open 可函数关单'" "$FR2"

echo "════ P0-4/P0-5：组合校验 + upsert ════"
BAD1=$(post action-types '{"apiName":"o4m5Seq1","displayName":"坏序列1","status":"active","parameters":[],
  "logic":[{"op":"modifyObject","objectType":"O4M4Order","pk":"K1","set":{"memo":"a"}},{"op":"createObject","objectType":"O4M4Order","pk":"K1","properties":{"id":"K1"}}]}')
chk "P0-4 modify→create 保存被拒" "echo '$BAD1' | grep -q 'modify 之后'" "$BAD1"
BAD2=$(post action-types '{"apiName":"o4m5Seq2","displayName":"坏序列2","status":"active","parameters":[],
  "logic":[{"op":"createOrModifyObject","objectType":"O4M4Order","pk":"K2","set":{"memo":"a"}},{"op":"deleteObject","objectType":"O4M4Order","pk":"K2"}]}')
chk "P0-4 upsert→delete 保存被拒" "echo '$BAD2' | grep -q 'upsert 后不得 delete'" "$BAD2"
BAD3=$(post action-types '{"apiName":"o4m5Seq3","displayName":"坏序列3","status":"active","parameters":[],
  "logic":[{"op":"createObject","objectType":"O4M4Order","pk":"K3","properties":{"id":"K3"}},{"op":"createObject","objectType":"O4M4Order","pk":"K3","properties":{"id":"K3"}}]}')
chk "P0-4 create 两次保存被拒" "echo '$BAD3' | grep -q '创建两次'" "$BAD3"

# upsert：新 pk 建 + 旧 pk 合并不抹属性
post action-types '{
  "apiName":"o4m5Upsert","displayName":"upsert订单","status":"active",
  "parameters":[{"name":"id","type":"string","required":true},{"name":"status","type":"string"}],
  "logic":[{"op":"createOrModifyObject","objectType":"O4M4Order","pk":"$id","set":{"status":"$status","memo":"upsert-touch"}}]
}' >/dev/null
UP1=$(post action-types/o4m5Upsert/execute '{"params":{"id":"O-NEW","status":"new"},"actor":"e2e"}')
chk "P0-5 upsert 新 pk 创建" "echo '$UP1' | grep -q '\"status\":\"committed\"'" "$UP1"
UP2=$(post action-types/o4m5Upsert/execute '{"params":{"id":"O-1","status":"reopened"},"actor":"e2e"}')
O1=$(objget O4M4Order O-1)
chk "P0-5 upsert 旧 pk 合并不抹属性（owner/customerId 保留）" "echo '$O1' | grep -q 'C-7' && echo '$O1' | grep -q 'reopened'" "$O1"

echo "════ P1-2：参数体系（multipleChoice/defaultValue/hidden/派生）════"
post action-types '{
  "apiName":"o4m5Prio","displayName":"设优先级","status":"active",
  "parameters":[{"name":"orderId","type":"object","objectType":"O4M4Order","required":true},
                {"name":"priority","type":"string","defaultValue":"P2","constraints":{"kind":"multipleChoice","options":["P0","P1","P2"]}},
                {"name":"ticket","type":"string","hidden":true,"required":false}],
  "logic":[{"op":"modifyObject","objectType":"O4M4Order","pk":"$orderId","set":{"memo":"prio-$priority-$ticketRef"}}]
}' >/dev/null
# 保存期自动派生：$ticketRef 补入参数（重取定义验证）
D2=$(get action-types/o4m5Prio)
chk "P1-2 保存期自动派生 \$ticketRef 参数" "echo '$D2' | grep -q 'ticketRef'" "$D2"
# 默认值补齐 + 拼接内插：只传 orderId → priority 默认 P2 生效
P1=$(post action-types/o4m5Prio/execute '{"params":{"orderId":"O-1"},"actor":"e2e"}')
chk "P1-2 defaultValue 补齐 + 内插（prio-P2-）" "echo '$P1' | grep -q 'prio-P2-'" "$P1"
# multipleChoice 拦截
P2=$(post action-types/o4m5Prio/execute '{"params":{"orderId":"O-1","priority":"P9"},"actor":"e2e"}')
chk "P1-2 multipleChoice 拦截 P9" "echo '$P2' | grep -q '不在可选范围'" "$P2"

echo "════ P1-3：execute-batch ════"
BT=$(post action-types/execute-batch '{"apiName":"o4m5Upsert","items":[{"params":{"id":"B-1","status":"b1"}},{"params":{"id":"B-2","status":"b2"}}],"actor":"e2e"}')
chk "P1-3 批量执行成功（committed，2 items）" "echo '$BT' | grep -q 'committed' && echo '$BT' | grep -q '\"items\":2'" "$BT"
BF=$(post action-types/execute-batch '{"apiName":"o4m5Seq0","items":[{"params":{}}],"actor":"e2e"}')
chk "P1-3 未知动作整批拒绝" "echo '$BF' | grep -q '未定义'" "$BF"
# 失败回滚：第 2 项 modify 不存在对象 → 整体回滚，B-1 的 touch 不落库
post action-types '{
  "apiName":"o4m5Touch","displayName":"touch状态","status":"active",
  "parameters":[{"name":"id","type":"object","objectType":"O4M4Order","required":true}],
  "logic":[{"op":"modifyObject","objectType":"O4M4Order","pk":"$id","set":{"memo":"touched"}}]
}' >/dev/null
BR=$(post action-types/execute-batch '{"apiName":"o4m5Touch","items":[{"params":{"id":"O-NEW"}},{"params":{"id":"O-NOPE"}}],"actor":"e2e"}')
chk "P1-3 第 2 项失败整批拒绝（已整体回滚）" "echo '$BR' | grep -q '已整体回滚'" "$BR"
B1CHK=$(objget O4M4Order O-NEW)
chk "P1-3 失败批次无部分写入（O-NEW memo 非 touched）" "echo '$B1CHK' | grep -qv 'touched'" "$B1CHK"
BL=$(get "action-logs?limit=5")
chk "P1-3 失败批审计落 failed" "echo '$BL' | grep -q '批量执行失败'" "$BL"

echo ""
echo "══════════════════════════════════"
echo " O4-M4 P0+P1 E2E：PASS=$pass FAIL=$((total-pass)) / total=$total"
echo "══════════════════════════════════"
[ $((total-pass)) -eq 0 ] && echo "ALL PASS ✅" || echo "存在失败 ❌"
exit $((total-pass))
