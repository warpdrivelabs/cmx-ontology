#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════
# cmx-ontology O2 对象存储与索引 后端测试套件
# 直连 onto-server :8097。测试数据以 qo_ 前缀（对象类型）/ 数值 pk 保留（不清理）。
# 前置：O1 已可用（本套件自建所需对象/关系类型）。退出码 = 失败数。
#
# 覆盖：对象写入(单/批) · 关系边 · 对象集代数(Base/Filter/SearchAround双向/集合运算/Static) ·
#       三跳单SQL(Search-Around 不 N+1) · 聚合(Count/GroupCount/GroupSum)。
# ═══════════════════════════════════════════════════════════════════════════
B="${B:-http://127.0.0.1:8097/api/onto/v1}"
PASS=0; FAIL=0; declare -a FAILED_TESTS
J='-H Content-Type:application/json'

assert_eq() {
  if [ "$2" = "$3" ]; then PASS=$((PASS+1)); printf "  ✅ %s\n" "$1";
  else FAIL=$((FAIL+1)); FAILED_TESTS+=("$1 | 期望[$2] 实际[$3]"); printf "  ❌ %s\n     期望[%s] 实际[%s]\n" "$1" "$2" "$3"; fi
}
jget() { EXPR="$2" python3 -c "
import sys,json,os
d=json.load(sys.stdin)
e=os.environ['EXPR']
print(eval(('d'+e) if e.startswith('[') else e))
" 2>/dev/null <<< "$1"; }
CURL="curl -s -m10 --no-keepalive"
post() { $CURL -XPOST "$B/$1" $J -d "$2"; }
get()  { $CURL "$B/$1"; }
del()  { $CURL -XDELETE "$B/$1" $J -d "${2:-}"; }

echo "════════════════════════════════════════════════════════"
echo " cmx-ontology O2 对象存储与索引 测试 @ $B"
echo "════════════════════════════════════════════════════════"

# ═══════════════ 组 0：建类型（幂等）═══════════════
echo; echo "【组0】建对象/关系类型"
post "object-types" '{"apiName":"qo_Cust","displayName":"O2客户","primaryKey":"id","titleProperty":"name","properties":[{"apiName":"id","baseType":"long"},{"apiName":"name","baseType":"string"},{"apiName":"region","baseType":"string"}]}' >/dev/null
post "object-types" '{"apiName":"qo_Ord","displayName":"O2订单","primaryKey":"oid","titleProperty":"no","properties":[{"apiName":"oid","baseType":"long"},{"apiName":"no","baseType":"string"},{"apiName":"amount","baseType":"decimal"}]}' >/dev/null
R=$(post "link-types" '{"apiName":"qo_places","objectTypeA":"qo_Cust","objectTypeB":"qo_Ord","cardinality":"oneToMany","roleA":"places","roleB":"placedBy"}')
assert_eq "建关系类型 code=0" "0" "$(jget "$R" "['code']")"

# ═══════════════ 组 1：对象写入 ═══════════════
echo; echo "【组1】对象写入（单条 pk 自动抽取 + 批量事务）"
R=$(post "objects/qo_Cust" '{"properties":{"id":1,"name":"北方贸易","region":"north"}}')
assert_eq "写对象 pk 自动=1" "1" "$(jget "$R" "['data']['pk']")"
post "objects/qo_Cust" '{"properties":{"id":2,"name":"南方物流","region":"south"}}' >/dev/null
post "objects/qo_Cust" '{"properties":{"id":3,"name":"北国重工","region":"north"}}' >/dev/null
R=$(post "objects/qo_Ord/batch" '[
 {"properties":{"oid":101,"no":"SO-101","amount":1500}},
 {"properties":{"oid":102,"no":"SO-102","amount":800}},
 {"properties":{"oid":103,"no":"SO-103","amount":3200}},
 {"properties":{"oid":104,"no":"SO-104","amount":500}}]')
assert_eq "批量写 4 对象 written=4" "4" "$(jget "$R" "['data']['written']")"
# 写未定义类型 → 业务错误
R=$(post "objects/qo_Ghost" '{"properties":{"x":1}}')
assert_eq "写未定义类型被拒 code=1" "1" "$(jget "$R" "['code']")"

# ═══════════════ 组 2：关系边 ═══════════════
echo; echo "【组2】关系边"
post "links" '{"link":"qo_places","aPk":"1","bPk":"101"}' >/dev/null
post "links" '{"link":"qo_places","aPk":"1","bPk":"102"}' >/dev/null
post "links" '{"link":"qo_places","aPk":"3","bPk":"103"}' >/dev/null
R=$(post "links" '{"link":"qo_places","aPk":"3","bPk":"104"}')
assert_eq "建关系边 saved=true" "True" "$(jget "$R" "['data']['saved']")"
# 未定义关系类型 → 拒
R=$(post "links" '{"link":"qo_nolink","aPk":"1","bPk":"101"}')
assert_eq "未定义关系边被拒 code=1" "1" "$(jget "$R" "['code']")"

# ═══════════════ 组 3：Base / Filter ═══════════════
echo; echo "【组3】Base / Filter 对象集"
R=$(post "object-sets/load" '{"objectSet":{"op":"base","objectType":"qo_Cust"}}')
assert_eq "Base qo_Cust 命中 3" "3" "$(jget "$R" "len(d['data']['rows'])")"
assert_eq "Base 终端类型 qo_Cust" "qo_Cust" "$(jget "$R" "['data']['objectType']")"
R=$(post "object-sets/load" '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"qo_Cust"},"predicate":{"kind":"eq","property":"region","value":"north"}}}')
assert_eq "Filter region=north 命中 2" "2" "$(jget "$R" "len(d['data']['rows'])")"
# 数值过滤
R=$(post "object-sets/load" '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"qo_Ord"},"predicate":{"kind":"ge","property":"amount","value":1000}}}')
assert_eq "Filter amount>=1000 命中 2 (101,103)" "2" "$(jget "$R" "len(d['data']['rows'])")"

# ═══════════════ 组 4：Search-Around（本体灵魂）═══════════════
echo; echo "【组4】Search-Around 关系遍历"
R=$(get "objects/qo_Cust/1/links/qo_places")
assert_eq "客户1 --places--> 命中 2 (101,102)" "2" "$(jget "$R" "len(d['data']['rows'])")"
assert_eq "Search-Around 终端类型 qo_Ord" "qo_Ord" "$(jget "$R" "['data']['objectType']")"
# Reverse
R=$(post "object-sets/load" '{"objectSet":{"op":"searchAround","source":{"op":"static","objectType":"qo_Ord","primaryKeys":["103"]},"link":"qo_places","direction":"reverse"}}')
assert_eq "订单103 --reverse--> 客户3" "3" "$(jget "$R" "d['data']['rows'][0]['pk']")"
assert_eq "Reverse 终端类型 qo_Cust" "qo_Cust" "$(jget "$R" "['data']['objectType']")"

# ═══════════════ 组 5：★三跳单 SQL（不 N+1）═══════════════
echo; echo "【组5】★三跳: (Cust region=north) --places--> Ord where amount>=1000"
R=$(post "object-sets/load" '{"objectSet":{"op":"filter","source":{"op":"searchAround","source":{"op":"filter","source":{"op":"base","objectType":"qo_Cust"},"predicate":{"kind":"eq","property":"region","value":"north"}},"link":"qo_places","direction":"forward"},"predicate":{"kind":"ge","property":"amount","value":1000}}}')
assert_eq "三跳命中 2 (101,103)" "2" "$(jget "$R" "len(d['data']['rows'])")"
assert_eq "三跳终端类型 qo_Ord" "qo_Ord" "$(jget "$R" "['data']['objectType']")"
# 命中集恰为 {101,103}
assert_eq "三跳结果集=[101,103]" "['101', '103']" "$(jget "$R" "sorted(r['pk'] for r in d['data']['rows'])")"

# ═══════════════ 组 6：集合运算 ═══════════════
echo; echo "【组6】Union / Intersect / Subtract"
R=$(post "object-sets/aggregate" '{"objectSet":{"op":"union","left":{"op":"filter","source":{"op":"base","objectType":"qo_Cust"},"predicate":{"kind":"eq","property":"region","value":"north"}},"right":{"op":"filter","source":{"op":"base","objectType":"qo_Cust"},"predicate":{"kind":"eq","property":"region","value":"south"}}},"aggregation":{"kind":"count"}}')
assert_eq "north ∪ south = 3" "3" "$(jget "$R" "['data']['count']")"
R=$(post "object-sets/aggregate" '{"objectSet":{"op":"subtract","left":{"op":"base","objectType":"qo_Cust"},"right":{"op":"filter","source":{"op":"base","objectType":"qo_Cust"},"predicate":{"kind":"eq","property":"region","value":"north"}}},"aggregation":{"kind":"count"}}')
assert_eq "全部 - north = 1 (south)" "1" "$(jget "$R" "['data']['count']")"
# 异类型集合运算 → 报错
R=$(post "object-sets/aggregate" '{"objectSet":{"op":"union","left":{"op":"base","objectType":"qo_Cust"},"right":{"op":"base","objectType":"qo_Ord"}},"aggregation":{"kind":"count"}}')
assert_eq "异类型 union 报错 code!=0" "True" "$(jget "$R" "d['code']!=0")"

# ═══════════════ 组 7：聚合 ═══════════════
echo; echo "【组7】聚合 Count / GroupCount / GroupSum"
R=$(post "object-sets/aggregate" '{"objectSet":{"op":"base","objectType":"qo_Ord"},"aggregation":{"kind":"count"}}')
assert_eq "订单 Count=4" "4" "$(jget "$R" "['data']['count']")"
R=$(post "object-sets/aggregate" '{"objectSet":{"op":"base","objectType":"qo_Cust"},"aggregation":{"kind":"groupCount","property":"region"}}')
assert_eq "GroupCount north=2" "2" "$(jget "$R" "next(g['count'] for g in d['data']['groups'] if g['group']=='north')")"
assert_eq "GroupCount south=1" "1" "$(jget "$R" "next(g['count'] for g in d['data']['groups'] if g['group']=='south')")"
# GroupSum：按 no 分组 sum amount（每组1条）。sum 回文本 numeric（可能带小数），用 float 比较。
R=$(post "object-sets/aggregate" '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"qo_Ord"},"predicate":{"kind":"ge","property":"amount","value":1000}},"aggregation":{"kind":"groupSum","groupBy":"no","sum":"amount"}}')
GS=$(jget "$R" "int(float(next(g['sum'] for g in d['data']['groups'] if g['group']=='SO-103')))")
assert_eq "GroupSum SO-103=3200" "3200" "$GS"

# ═══════════════ 组 8：删除对象连带清边 ═══════════════
echo; echo "【组8】删对象连带清关系边"
del "objects/qo_Cust/1" >/dev/null
R=$(get "objects/qo_Cust/1/links/qo_places")
assert_eq "删客户1后其边已清 命中 0" "0" "$(jget "$R" "len(d['data']['rows'])")"

# ═══════════════ 汇总 ═══════════════
echo
echo "════════════════════════════════════════════════════════"
echo " 结果：PASS=$PASS  FAIL=$FAIL"
if [ "$FAIL" -gt 0 ]; then
  echo " 失败明细："
  for t in "${FAILED_TESTS[@]}"; do echo "   - $t"; done
fi
echo "════════════════════════════════════════════════════════"
exit "$FAIL"
