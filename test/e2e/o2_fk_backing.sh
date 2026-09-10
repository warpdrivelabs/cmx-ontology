#!/usr/bin/env bash
# O2 ForeignKey backing 编译分派 E2E（#5）：关系 backing=foreignKey 时，SearchAround 走对象表 FK 列 JOIN
# （绝不建 ol_edge），凭 Order.props.custId 指回 Customer.pk 完成前向/反向遍历。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 对象类型：客户 + 订单（订单含 custId 外键列）
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O5FkCust","displayName":"FK客户","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"name","baseType":"string"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O5FkOrd","displayName":"FK订单","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"custId","baseType":"string"},{"apiName":"amount","baseType":"decimal"}]}' >/dev/null

# 关系：A=客户 B=订单，backing=foreignKey（外键列 custId 落在 B 端订单表，指回客户 pk）
SAVE=$(curl "${H[@]}" -X POST "$B/link-types" -d '{
  "apiName":"o5FkPlaces","displayName":"下单(FK)","cardinality":"oneToMany",
  "objectTypeA":"O5FkCust","objectTypeB":"O5FkOrd","roleA":"orders","roleB":"placedBy",
  "backing":{"kind":"foreignKey","property":"custId","side":"b"},"status":"active"}')
echo "建关系: $SAVE"
chk "FK backing 关系类型保存成功" "echo '$SAVE' | grep -qE '\"saved\":true'" "$SAVE"

# 种客户 2 个
curl "${H[@]}" -X POST "$B/objects/O5FkCust" -d '{"properties":{"id":"FC-1","name":"甲"}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O5FkCust" -d '{"properties":{"id":"FC-2","name":"乙"}}' >/dev/null
# 种订单 3 个：FO-1/FO-2 属 FC-1，FO-3 属 FC-2。仅靠 custId 外键列，绝不建 ol_edge。
curl "${H[@]}" -X POST "$B/objects/O5FkOrd" -d '{"properties":{"id":"FO-1","custId":"FC-1","amount":100}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O5FkOrd" -d '{"properties":{"id":"FO-2","custId":"FC-1","amount":200}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O5FkOrd" -d '{"properties":{"id":"FO-3","custId":"FC-2","amount":300}}' >/dev/null

# 1) Forward：FC-1 → 订单（经 FK 列 custId=FC-1 拿到 FO-1/FO-2，不含 FO-3）
F1=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{
  "objectSet":{"op":"searchAround","direction":"forward","link":"o5FkPlaces",
    "source":{"op":"static","objectType":"O5FkCust","primaryKeys":["FC-1"]}}}')
echo "forward FC-1: $F1"
chk "FK Forward: FC-1 拿到 FO-1" "echo '$F1' | grep -q 'FO-1'" "$F1"
chk "FK Forward: FC-1 拿到 FO-2" "echo '$F1' | grep -q 'FO-2'" "$F1"
chk "FK Forward: FC-1 不含 FO-3" "! echo '$F1' | grep -q 'FO-3'" "$F1"

# 2) Reverse：FO-3 → 客户（经 FK 列 custId=FC-2 拿回 FC-2）
R1=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{
  "objectSet":{"op":"searchAround","direction":"reverse","link":"o5FkPlaces",
    "source":{"op":"static","objectType":"O5FkOrd","primaryKeys":["FO-3"]}}}')
echo "reverse FO-3: $R1"
chk "FK Reverse: FO-3 拿回 FC-2" "echo '$R1' | grep -q 'FC-2'" "$R1"
chk "FK Reverse: FO-3 不含 FC-1" "! echo '$R1' | grep -q 'FC-1'" "$R1"

# 3) Forward 再套 filter：FC-1 的订单里 amount>=200（应只剩 FO-2）
F2=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{
  "objectSet":{"op":"filter",
    "source":{"op":"searchAround","direction":"forward","link":"o5FkPlaces",
      "source":{"op":"static","objectType":"O5FkCust","primaryKeys":["FC-1"]}},
    "predicate":{"kind":"ge","property":"amount","value":200}}}')
echo "forward+filter: $F2"
chk "FK Forward+filter: 只剩 FO-2" "echo '$F2' | grep -q 'FO-2' && ! echo '$F2' | grep -q 'FO-1'" "$F2"

# 4) FK property 为空 → 关系保存被拒（validate 拦截）
BAD=$(curl "${H[@]}" -X POST "$B/link-types" -d '{
  "apiName":"o5FkBad","objectTypeA":"O5FkCust","objectTypeB":"O5FkOrd",
  "backing":{"kind":"foreignKey","property":""},"status":"active"}')
echo "空FK: $BAD"
chk "FK property 为空被拒" "echo '$BAD' | grep -qE 'property|非法|不能为空'" "$BAD"

echo ""
echo "O2 FK backing E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
