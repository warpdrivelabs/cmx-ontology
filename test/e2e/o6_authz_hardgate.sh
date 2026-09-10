#!/usr/bin/env bash
# O6 读侧权限硬门 E2E（#3）：主链路 load/aggregate 挂 PEP。
# 覆盖：无策略放行（向后兼容）/ deny→403 / allow+row_filter 行残差 / deny_marking→列 Hide（移除列，非***）/
#       受控类型无授权→403（default-deny）/ 聚合走残差。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
CODE=(-s -o /dev/null -w "%{http_code}" -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 唯一化类型名 + 策略名：硬门让"受控类型"状态在库内累积，重跑须用全新未受控类型（隔离）。
SFX="$(date +%s)$$"
OT="O6Emp_$SFX"
LOAD="{\"objectSet\":{\"op\":\"base\",\"objectType\":\"$OT\"}"

# 对象类型（region + salary[marking=hr]）+ 种 3 行（east 2 / west 1）
curl "${H[@]}" -X POST "$B/object-types" -d "{
  \"apiName\":\"$OT\",\"displayName\":\"员工\",\"primaryKey\":\"id\",\"titleProperty\":\"id\",\"status\":\"active\",
  \"properties\":[{\"apiName\":\"id\",\"baseType\":\"string\"},{\"apiName\":\"region\",\"baseType\":\"string\"},{\"apiName\":\"salary\",\"baseType\":\"decimal\",\"marking\":\"hr\"}]}" >/dev/null
curl "${H[@]}" -X POST "$B/objects/$OT" -d '{"properties":{"id":"E-1","region":"east","salary":100}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/$OT" -d '{"properties":{"id":"E-2","region":"east","salary":200}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/$OT" -d '{"properties":{"id":"E-3","region":"west","salary":300}}' >/dev/null

# 1) 无策略 → 放行（向后兼容），能看到全部 3 行 + salary 明文
Z=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d "$LOAD}")
echo "无策略: $Z"
chk "无策略放行·见 E-1/E-2/E-3" "echo '$Z' | grep -q E-1 && echo '$Z' | grep -q E-3" "$Z"
chk "无策略·salary 明文可见" "echo '$Z' | grep -q '\"salary\":100'" "$Z"

# 2) deny 策略：拒绝 role:banned 读该类型 → 该主体 load 得 403
curl "${H[@]}" -X POST "$B/policies" -d "{
  \"apiName\":\"p_deny_banned_$SFX\",\"objectType\":\"$OT\",\"subjectKind\":\"role\",\"subject\":\"banned\",\"effect\":\"deny\",\"status\":\"active\"}" >/dev/null
D=$(curl "${CODE[@]}" -X POST "$B/object-sets/load" -d "$LOAD,\"subjects\":[\"role:banned\"]}")
echo "deny http: $D"
chk "deny 策略命中→403" "[ '$D' = '403' ]" "$D"

# 3) allow + row_filter：role:eastMgr 只能看 region=east → 只回 E-1/E-2，不含 E-3
curl "${H[@]}" -X POST "$B/policies" -d "{
  \"apiName\":\"p_east_only_$SFX\",\"objectType\":\"$OT\",\"subjectKind\":\"role\",\"subject\":\"eastMgr\",\"effect\":\"allow\",\"status\":\"active\",
  \"rowFilter\":[{\"kind\":\"eq\",\"property\":\"region\",\"value\":\"east\"}]}" >/dev/null
R=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d "$LOAD,\"subjects\":[\"role:eastMgr\"]}")
echo "row_filter: $R"
chk "allow+row_filter·见 E-1" "echo '$R' | grep -q E-1" "$R"
chk "allow+row_filter·不含 E-3(west)" "! echo '$R' | grep -q E-3" "$R"

# 4) allow + deny_marking：role:eastMgr 的 salary(marking=hr) 被 Hide（列移除，非 ***）
curl "${H[@]}" -X POST "$B/policies" -d "{
  \"apiName\":\"p_east_only_$SFX\",\"objectType\":\"$OT\",\"subjectKind\":\"role\",\"subject\":\"eastMgr\",\"effect\":\"allow\",\"status\":\"active\",
  \"rowFilter\":[{\"kind\":\"eq\",\"property\":\"region\",\"value\":\"east\"}],\"denyMarkings\":[\"hr\"]}" >/dev/null
M=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d "$LOAD,\"subjects\":[\"role:eastMgr\"]}")
echo "marking Hide: $M"
chk "deny_marking·salary 列被移除(Hide)" "! echo '$M' | grep -q salary" "$M"
chk "deny_marking·非 *** 软脱敏" "! echo '$M' | grep -q '\\*\\*\\*'" "$M"
chk "deny_marking·region 仍在" "echo '$M' | grep -q region" "$M"

# 5) 受控类型 default-deny：该类型已有针对性 allow 策略 → 无授权主体(role:nobody)读取 403
N=$(curl "${CODE[@]}" -X POST "$B/object-sets/load" -d "$LOAD,\"subjects\":[\"role:nobody\"]}")
echo "default-deny http: $N"
chk "受控类型·无 allow 命中→403" "[ '$N' = '403' ]" "$N"

# 6) 聚合走残差：eastMgr count 只数 east 两行 = 2
A=$(curl "${H[@]}" -X POST "$B/object-sets/aggregate" -d "{
  \"objectSet\":{\"op\":\"base\",\"objectType\":\"$OT\"},\"aggregation\":{\"kind\":\"count\"},\"subjects\":[\"role:eastMgr\"]}")
echo "agg: $A"
chk "聚合走行残差·count=2(仅east)" "echo '$A' | grep -qE '(\"count\":2|\"result\":2|:2[,}])'" "$A"

echo ""
echo "O6 读侧硬门 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
