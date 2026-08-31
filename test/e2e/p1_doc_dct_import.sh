#!/usr/bin/env bash
# DOC/DCT 反向导入 E2E：DOC→对象类型+组合关系；DCT→参照类型+种子项；导入后可查询/Search-Around。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# ── DOC 导入：销售订单主从（头/行）+ 组合关系 ──
DOC=$(curl "${H[@]}" -X POST "$B/import/doc" -d '{
  "apiName":"ImpSO","displayName":"销售订单",
  "entities":[
    {"apiName":"ImpSoHead","displayName":"订单头","primaryKey":"id","titleProperty":"orderNo",
     "properties":[{"apiName":"id","baseType":"string"},{"apiName":"orderNo","baseType":"string"},{"apiName":"amount","baseType":"decimal"}]},
    {"apiName":"ImpSoLine","displayName":"订单行","primaryKey":"lineId",
     "properties":[{"apiName":"lineId","baseType":"string"},{"apiName":"product","baseType":"string"},{"apiName":"qty","baseType":"long"}]}
  ],
  "relations":[{"from":"ImpSoHead","to":"ImpSoLine","cardinality":"oneToMany","role":"lines","displayName":"订单行"}]
}')
echo "doc: $DOC"
chk "DOC 建 2 对象类型" "echo '$DOC' | grep -qE '\"createdTypes\":2'" "$DOC"
chk "DOC 建 1 组合关系" "echo '$DOC' | grep -qE '\"createdLinks\":1'" "$DOC"
chk "DOC 关系名 ImpSoHead_lines_ImpSoLine" "echo '$DOC' | grep -q 'ImpSoHead_lines_ImpSoLine'" "$DOC"

# 验证对象类型详情（属性/主键）已建
OT=$(curl "${H[@]}" "$B/object-types/ImpSoHead")
chk "对象类型 ImpSoHead 主键=id" "echo '$OT' | grep -q '\"primaryKey\":\"id\"'" "$OT"
chk "对象类型 ImpSoHead 属性齐(orderNo/amount)" "echo '$OT' | grep -q 'orderNo' && echo '$OT' | grep -q 'amount'" "$OT"
LT=$(curl "${H[@]}" "$B/link-types/ImpSoHead_lines_ImpSoLine")
chk "关系两端 ImpSoHead↔ImpSoLine" "echo '$LT' | grep -q 'ImpSoHead' && echo '$LT' | grep -q 'ImpSoLine'" "$LT"

# 导入后即可写对象 + 建边（真机验证类型可用）
curl "${H[@]}" -X POST "$B/objects/ImpSoHead" -d '{"properties":{"id":"SO-1","orderNo":"NO-1","amount":999}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/ImpSoLine" -d '{"properties":{"lineId":"L-1","product":"P","qty":3}}' >/dev/null
curl "${H[@]}" -X POST "$B/links" -d '{"link":"ImpSoHead_lines_ImpSoLine","aPk":"SO-1","bPk":"L-1"}' >/dev/null
SA=$(curl "${H[@]}" "$B/objects/ImpSoHead/SO-1/links/ImpSoHead_lines_ImpSoLine")
chk "导入类型可 Search-Around(头→行 L-1)" "echo '$SA' | grep -q 'L-1'" "$SA"

# ── DCT 导入：币种字典 → 参照类型 + 种子项 ──
DCT=$(curl "${H[@]}" -X POST "$B/import/dct" -d '{
  "apiName":"ImpCurrency","displayName":"币种",
  "items":[{"code":"USD","name":"美元"},{"code":"CNY","name":"人民币"},{"code":"EUR","name":"欧元"}]
}')
echo "dct: $DCT"
chk "DCT 种 3 字典项" "echo '$DCT' | grep -qE '\"seededItems\":3'" "$DCT"
DOT=$(curl "${H[@]}" "$B/object-types/ImpCurrency")
chk "参照类型主键=code 标题=name" "echo '$DOT' | grep -q '\"primaryKey\":\"code\"' && echo '$DOT' | grep -q '\"titleProperty\":\"name\"'" "$DOT"
# 字典项作为对象可查
LD=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"ImpCurrency"}}')
chk "字典项已物化为对象(USD/CNY/EUR)" "echo '$LD' | grep -q 'USD' && echo '$LD' | grep -q '人民币' && echo '$LD' | grep -q 'EUR'" "$LD"
# 过滤字典（对象集代数用在字典上）
FD=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"ImpCurrency"},"predicate":{"kind":"eq","property":"code","value":"USD"}}}')
chk "字典可对象集过滤(code=USD→美元)" "echo '$FD' | grep -q '美元' && ! echo '$FD' | grep -q '欧元'" "$FD"

echo ""
echo "DOC/DCT 导入 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
