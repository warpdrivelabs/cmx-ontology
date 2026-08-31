#!/usr/bin/env bash
# O6 动态安全 E2E：同一对象集查询，按主体(subjects)适配策略 → 行级残差过滤 + 列级 marking 脱敏。
# 单租户模式下用请求体 subjects 声明主体（dev/off；jwt 模式以令牌为准）。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"; CT="Content-Type: application/json"
H=(-s -H "$CT" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 对象类型 O6Cust（region + ssn[marking=pii]）+ 种两行
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O6Cust","displayName":"客户","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"region","baseType":"string"},{"apiName":"ssn","baseType":"string","marking":"pii"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O6Cust" -d '{"properties":{"id":"C-1","region":"east","ssn":"111-11"}}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/O6Cust" -d '{"properties":{"id":"C-2","region":"west","ssn":"222-22"}}' >/dev/null

# 策略：role:east → 只看 region=east + 拒 pii 列
curl "${H[@]}" -X POST "$B/policies" -d '{
  "apiName":"eastOnly","displayName":"东区只读脱敏","objectType":"O6Cust","subjectKind":"role","subject":"east",
  "rowFilter":[{"kind":"eq","property":"region","value":"east"}],"denyMarkings":["pii"]}' >/dev/null

# 1) 主体 role:admin（无匹配策略）：全部 2 行、ssn 明文
D=$(curl "${H[@]}" -X POST "$B/secure/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"O6Cust"},"subjects":["role:admin"]}')
echo "admin: $D"
chk "admin 看到 C-1 与 C-2" "echo '$D' | grep -q 'C-1' && echo '$D' | grep -q 'C-2'" "$D"
chk "admin ssn 明文" "echo '$D' | grep -q '111-11'" "$D"
chk "admin 无 appliedPolicies" "echo '$D' | grep -qE '\"appliedPolicies\":\[\]'" "$D"

# 2) 主体 role:east（策略生效）：只 east 行(C-1) + ssn 脱敏
E=$(curl "${H[@]}" -X POST "$B/secure/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"O6Cust"},"subjects":["role:east","user:bob"]}')
echo "east: $E"
chk "east 行级残差：只见 C-1（无 C-2）" "echo '$E' | grep -q 'C-1' && ! echo '$E' | grep -q 'C-2'" "$E"
chk "east 列级脱敏：ssn=*** 无明文" "echo '$E' | grep -q '\\*\\*\\*' && ! echo '$E' | grep -q '111-11'" "$E"
chk "east appliedPolicies 含 eastOnly" "echo '$E' | grep -q 'eastOnly'" "$E"
chk "east subjects 回显 role:east" "echo '$E' | grep -q 'role:east'" "$E"

# 3) 主体 role:east 但换个对象集（filter west）：残差与用户 filter 复合 → 空（east∩west=∅）
X=$(curl "${H[@]}" -X POST "$B/secure/object-sets/load" -d '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"O6Cust"},"predicate":{"kind":"eq","property":"region","value":"west"}},"subjects":["role:east"]}')
chk "残差与用户过滤复合：east∩west=空" "echo '$X' | grep -qE '\"rows\":\[\]'" "$X"

# 4) 策略列表
L=$(curl "${H[@]}" "$B/policies")
chk "策略列表含 eastOnly" "echo '$L' | grep -q 'eastOnly'" "$L"

echo ""
echo "O6 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
