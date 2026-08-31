#!/usr/bin/env bash
# O5 函数计算 E2E：Query(标量) / DerivedProperty(读对象) / objectSet(FEEL sum) / Aggregation(存储层).
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 对象类型 O5Ord（region/amount）+ 种 3 个对象
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O5Ord","displayName":"订单","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"region","baseType":"string"},{"apiName":"amount","baseType":"decimal"},{"apiName":"qty","baseType":"long"},{"apiName":"price","baseType":"decimal"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O5Ord" -d '{"properties":{"id":"O-1","region":"east","amount":1500,"qty":3,"price":10}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O5Ord" -d '{"properties":{"id":"O-2","region":"east","amount":800,"qty":2,"price":20}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O5Ord" -d '{"properties":{"id":"O-3","region":"west","amount":300,"qty":1,"price":5}}' >/dev/null

# 1) Query 函数：折扣率 = if amount>1000 then 0.8 else 0.2（标量输入）
curl "${H[@]}" -X POST "$B/functions" -d '{
  "apiName":"o5Discount","displayName":"折扣率","runtime":"feel","kind":"query",
  "inputs":[{"name":"amount","type":"double"}],"output":{"type":"double"},
  "body":"if amount > 1000 then 0.8 else 0.2","status":"active"}' >/dev/null
Q1=$(curl "${H[@]}" -X POST "$B/functions/o5Discount/evaluate" -d '{"args":{"amount":1500}}')
echo "query: $Q1"
chk "Query 折扣率(1500→0.8)" "echo '$Q1' | grep -qE '\"result\":0.8'" "$Q1"
Q2=$(curl "${H[@]}" -X POST "$B/functions/o5Discount/evaluate" -d '{"args":{"amount":500}}')
chk "Query 折扣率(500→0.2)" "echo '$Q2' | grep -qE '\"result\":0.2'" "$Q2"

# 2) DerivedProperty：小计 = order.qty * order.price（object 输入，从库读 O-1）
curl "${H[@]}" -X POST "$B/functions" -d '{
  "apiName":"o5Subtotal","displayName":"小计","runtime":"feel","kind":"derivedProperty",
  "inputs":[{"name":"order","type":"object"}],"output":{"type":"double"},
  "body":"order.qty * order.price","status":"active"}' >/dev/null
D1=$(curl "${H[@]}" -X POST "$B/functions/o5Subtotal/evaluate" -d '{"objects":{"order":{"objectType":"O5Ord","pk":"O-1"}}}')
echo "derived: $D1"
chk "DerivedProperty 小计(O-1: 3*10=30)" "echo '$D1' | grep -qE '\"result\":30'" "$D1"

# 3) objectSet 输入 + FEEL：east 区订单金额之和（把行注入，FEEL 取 amount 再 sum via for/sum）
curl "${H[@]}" -X POST "$B/functions" -d '{
  "apiName":"o5EastSum","displayName":"东区总额","runtime":"feel","kind":"query",
  "inputs":[{"name":"orders","type":"objectSet"}],"output":{"type":"double"},
  "body":"sum(for o in orders return o.amount)","status":"active"}' >/dev/null
S1=$(curl "${H[@]}" -X POST "$B/functions/o5EastSum/evaluate" -d '{
  "objectSets":{"orders":{"op":"filter","source":{"op":"base","objectType":"O5Ord"},"predicate":{"kind":"eq","property":"region","value":"east"}}}}')
echo "objectset: $S1"
chk "objectSet FEEL 东区总额(1500+800=2300)" "echo '$S1' | grep -qE '\"result\":2300'" "$S1"

# 4) Aggregation 用途：走存储层 aggregate（Count 全体订单=3）
curl "${H[@]}" -X POST "$B/functions" -d '{
  "apiName":"o5Count","displayName":"订单数","runtime":"feel","kind":"aggregation",
  "inputs":[],"output":{"type":"long"},"body":"","status":"active"}' >/dev/null
A1=$(curl "${H[@]}" -X POST "$B/functions/o5Count/evaluate" -d '{
  "objectSet":{"op":"base","objectType":"O5Ord"},"aggregation":{"kind":"count"}}')
echo "aggregation: $A1"
chk "Aggregation 订单数=3" "echo '$A1' | grep -qE '(\"result\":3|\"count\":3|:3[,}])'" "$A1"

# 5) 缺输入 → 报错
MISS=$(curl "${H[@]}" -X POST "$B/functions/o5Discount/evaluate" -d '{"args":{}}')
chk "缺输入被拒" "echo '$MISS' | grep -qiE '缺输入|missing|amount|error|\"code\":1'" "$MISS"

echo ""
echo "O5 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
