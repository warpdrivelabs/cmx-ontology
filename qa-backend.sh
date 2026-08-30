#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════
# cmx-ontology O1 建模引擎 后端全面测试套件
# 直连 onto-server :8097。所有测试数据以 qa_ 前缀保留（不清理）。
# 输出：每断言 PASS/FAIL；结尾汇总。退出码 = 失败数。
#
# 用法：先起服务（./onto.sh 或 CONFIG_FILE=onto-server-dev.toml <bin>），再 ./qa-backend.sh
# ═══════════════════════════════════════════════════════════════════════════
B="${B:-http://127.0.0.1:8097/api/onto/v1}"
PASS=0; FAIL=0; declare -a FAILED_TESTS
J='-H Content-Type:application/json'

assert_eq() {
  if [ "$2" = "$3" ]; then PASS=$((PASS+1)); printf "  ✅ %s\n" "$1";
  else FAIL=$((FAIL+1)); FAILED_TESTS+=("$1 | 期望[$2] 实际[$3]"); printf "  ❌ %s\n     期望[%s] 实际[%s]\n" "$1" "$2" "$3"; fi
}
assert_contains() {
  if echo "$3" | grep -qF "$2"; then PASS=$((PASS+1)); printf "  ✅ %s\n" "$1";
  else FAIL=$((FAIL+1)); FAILED_TESTS+=("$1 | 应含[$2] 实际[$3]"); printf "  ❌ %s\n     应含[%s] 实际[%s]\n" "$1" "$2" "$3"; fi
}
# jget "<json>" "<expr>"：expr 以 [ 开头则自动补 d 前缀（d['data']...）；否则原样 eval（如 len(d['data'])）
jget() { EXPR="$2" python3 -c "
import sys,json,os
d=json.load(sys.stdin)
e=os.environ['EXPR']
print(eval(('d'+e) if e.startswith('[') else e))
" 2>/dev/null <<< "$1"; }

CURL="curl -s -m10 --no-keepalive"
post() { $CURL -XPOST "$B/$1" $J -d "$2"; }
get()  { $CURL "$B/$1"; }
del()  { $CURL -XDELETE "$B/$1"; }
code() { $CURL -o /dev/null -w "%{http_code}" -XPOST "$B/$1" $J -d "$2"; }
codeg(){ $CURL -o /dev/null -w "%{http_code}" "$B/$1"; }

echo "════════════════════════════════════════════════════════"
echo " cmx-ontology O1 建模引擎 后端全面测试 @ $B"
echo "════════════════════════════════════════════════════════"

# ═══════════════ 组 1：对象类型 CRUD + 校验 ═══════════════
echo; echo "【组1】对象类型 CRUD + 结构校验"
R=$(post "object-types" '{"apiName":"qa_Customer","displayName":"QA客户","primaryKey":"id","titleProperty":"name",
 "properties":[
   {"apiName":"id","baseType":"long","required":true},
   {"apiName":"name","baseType":"string","required":true,"isIndexed":true},
   {"apiName":"region","baseType":"string"},
   {"apiName":"createdAt","baseType":"timestamp"}],
 "status":"active"}')
assert_eq "存对象类型 qa_Customer code=0" "0" "$(jget "$R" "['code']")"
assert_eq "存对象类型 saved=true" "True" "$(jget "$R" "['data']['saved']")"

R=$(get "object-types/qa_Customer")
assert_eq "详情 apiName 正确" "qa_Customer" "$(jget "$R" "['data']['apiName']")"
assert_eq "详情 status=active" "active" "$(jget "$R" "['data']['status']")"
assert_eq "详情属性数=4" "4" "$(jget "$R" "len(d['data']['properties'])")"
assert_eq "详情 name 属性 isIndexed=true" "True" "$(jget "$R" "next(p['isIndexed'] for p in d['data']['properties'] if p['apiName']=='name')")"

R=$(get "object-types")
assert_contains "列表含 qa_Customer" "qa_Customer" "$R"

# 校验：主键不在属性中 → valid:false
R=$(post "object-types/validate" '{"apiName":"qa_Bad","primaryKey":"nope","properties":[{"apiName":"x","baseType":"string"}]}')
assert_eq "校验非法主键 valid=false" "False" "$(jget "$R" "['data']['valid']")"
assert_contains "校验错误提示含'主键'" "主键" "$R"

# 校验：apiName 非法（数字开头）→ 保存 400/业务错误
R=$(post "object-types" '{"apiName":"2bad","properties":[]}')
assert_contains "非法 apiName 保存被拒" "非法" "$R"

# 校验：属性重复
R=$(post "object-types/validate" '{"apiName":"qa_Dup","properties":[{"apiName":"id"},{"apiName":"id"}]}')
assert_eq "重复属性 valid=false" "False" "$(jget "$R" "['data']['valid']")"

# ═══════════════ 组 2：第二个对象类型（关系两端用）═══════════════
echo; echo "【组2】第二个对象类型 qa_Order"
R=$(post "object-types" '{"apiName":"qa_Order","displayName":"QA订单","primaryKey":"orderId","titleProperty":"orderNo",
 "properties":[{"apiName":"orderId","baseType":"long","required":true},{"apiName":"orderNo","baseType":"string"},
   {"apiName":"amount","baseType":"decimal","semanticType":"money"}]}')
assert_eq "存 qa_Order code=0" "0" "$(jget "$R" "['code']")"
assert_eq "默认 status=experimental" "experimental" "$(jget "$(get "object-types/qa_Order")" "['data']['status']")"

# ═══════════════ 组 3：关系类型 ═══════════════
echo; echo "【组3】关系类型 CRUD + 校验"
R=$(post "link-types" '{"apiName":"qa_customerPlacesOrder","displayName":"客户下单","objectTypeA":"qa_Customer",
 "objectTypeB":"qa_Order","cardinality":"oneToMany","roleA":"places","roleB":"placedBy"}')
assert_eq "存关系类型 code=0" "0" "$(jget "$R" "['code']")"

R=$(get "link-types/qa_customerPlacesOrder")
assert_eq "关系详情 A端=qa_Customer" "qa_Customer" "$(jget "$R" "['data']['objectTypeA']")"
assert_eq "关系详情 基数=oneToMany" "oneToMany" "$(jget "$R" "['data']['cardinality']")"
assert_eq "关系详情 roleA=places" "places" "$(jget "$R" "['data']['roleA']")"

# 校验：缺 B 端
R=$(post "link-types" '{"apiName":"qa_dangling","objectTypeA":"qa_Customer"}')
assert_contains "关系缺B端被拒" "两端" "$R"

# ═══════════════ 组 4：接口 / 共享属性 / 动作 / 函数 ═══════════════
echo; echo "【组4】接口/共享属性/动作/函数"
assert_eq "存接口 code=0" "0" "$(jget "$(post "interfaces" '{"apiName":"qa_Locatable","displayName":"可定位物","properties":["latitude","longitude"]}')" "['code']")"
assert_eq "存共享属性 code=0" "0" "$(jget "$(post "shared-properties" '{"apiName":"qa_currencyCode","displayName":"币种","baseType":"string","semanticType":"currency"}')" "['code']")"
assert_eq "存动作类型 code=0" "0" "$(jget "$(post "action-types" '{"apiName":"qa_reassignOrder","displayName":"改派订单","logic":[{"op":"modifyObject","target":"order"}],"sideEffects":[{"kind":"startBusinessProcess","flowDefKey":"order_reassign"}]}')" "['code']")"
assert_eq "存函数 code=0" "0" "$(jget "$(post "functions" '{"apiName":"qa_delayRisk","displayName":"延误风险","runtime":"feel","kind":"derivedProperty","body":"if amount > 1000 then 0.8 else 0.2"}')" "['code']")"

R=$(get "functions/qa_delayRisk")
assert_eq "函数详情 runtime=feel" "feel" "$(jget "$R" "['data']['runtime']")"
assert_eq "函数详情 kind=derivedProperty" "derivedProperty" "$(jget "$R" "['data']['kind']")"

R=$(get "action-types/qa_reassignOrder")
assert_contains "动作副作用含 startBusinessProcess" "startBusinessProcess" "$R"

# ═══════════════ 组 5：全量清单 ═══════════════
echo; echo "【组5】本体全量清单"
R=$(get "manifest")
assert_contains "清单含 qa_Customer" "qa_Customer" "$R"
assert_contains "清单含 qa_customerPlacesOrder" "qa_customerPlacesOrder" "$R"
assert_contains "清单含 qa_Locatable" "qa_Locatable" "$R"
assert_contains "清单含 qa_delayRisk" "qa_delayRisk" "$R"

# ═══════════════ 组 6：发布 / 版本快照 ═══════════════
echo; echo "【组6】发布 / 版本不可变快照"
R=$(post "publish" '{"summary":"qa 首个本体版本"}')
assert_eq "发布 code=0" "0" "$(jget "$R" "['code']")"
VER=$(jget "$R" "['data']['version']")
assert_contains "发布返回 rev（16 hex）" "rev" "$R"
[ -n "$VER" ] && echo "     → 发布版本 v$VER"

R=$(get "versions")
assert_contains "版本列表含 qa 摘要" "qa 首个本体版本" "$R"

R=$(get "versions/$VER")
assert_contains "版本快照含 objectTypes" "objectTypes" "$R"
assert_contains "版本快照含 qa_Customer 完整定义" "qa_Customer" "$R"
# 快照里对象类型带完整属性（非清单裁剪）
assert_eq "快照对象类型带完整属性" "True" "$(jget "$R" "any('properties' in o for o in d['data']['objectTypes'])")"

# 二次发布 → 版本递增
R=$(post "publish" '{"summary":"qa 第二版"}')
VER2=$(jget "$R" "['data']['version']")
assert_eq "二次发布版本递增" "$((VER+1))" "$VER2"

# ═══════════════ 组 7：stats + 删除 ═══════════════
echo; echo "【组7】统计 + 删除"
R=$(get "stats")
assert_eq "stats publishedVersion=$VER2" "$VER2" "$(jget "$R" "['data']['publishedVersion']")"
# 计数 ≥ 我们建的数量（库可能有其它遗留，用 >=）
assert_eq "stats objectTypes>=2" "True" "$(jget "$R" "d['data']['objectTypes']>=2")"

R=$(del "object-types/qa_Order")
assert_eq "删除 qa_Order deleted=true" "True" "$(jget "$R" "['data']['deleted']")"
assert_eq "删除后取详情 404" "404" "$(codeg "object-types/qa_Order")"

# 幂等删除（不存在 → deleted=false，非报错）
R=$(del "object-types/qa_Order")
assert_eq "幂等删除 deleted=false" "False" "$(jget "$R" "['data']['deleted']")"

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
