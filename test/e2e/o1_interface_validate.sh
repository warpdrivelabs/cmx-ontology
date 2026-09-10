#!/usr/bin/env bash
# O1 接口强校验 E2E（#4）：对象类型 implements 接口时，须具备接口要求的共享属性且 baseType 匹配。
# 覆盖：缺属性被拒 / 类型不符被拒 / 补齐后通过 / 未定义接口被拒。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 前置：共享属性 geoCode(geohash) + 接口 Locatable 要求 [geoCode]
curl "${H[@]}" -X POST "$B/shared-properties" -d '{
  "apiName":"geoCode","displayName":"地理编码","baseType":"geohash"}' >/dev/null
curl "${H[@]}" -X POST "$B/interfaces" -d '{
  "apiName":"Locatable","displayName":"可定位","properties":["geoCode"],"status":"active"}' >/dev/null

# 1) 缺共享属性 → 被拒（implements Locatable 但没有引用 geoCode 的属性）
R1=$(curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O1Store","displayName":"门店","primaryKey":"id","status":"active",
  "implements":["Locatable"],
  "properties":[{"apiName":"id","baseType":"string"}]}')
echo "缺属性: $R1"
chk "缺共享属性的对象类型被拒" "echo '$R1' | grep -qE '未满足|接口契约'" "$R1"

# 2) 类型不符 → 被拒（有引用 geoCode 的属性但 baseType=string≠geohash）
R2=$(curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O1Store","displayName":"门店","primaryKey":"id","status":"active",
  "implements":["Locatable"],
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"loc","baseType":"string","sharedProperty":"geoCode"}]}')
echo "类型不符: $R2"
chk "共享属性类型不符被拒" "echo '$R2' | grep -qE '未满足|接口契约'" "$R2"

# 3) 补齐（loc 引用 geoCode 且 baseType=geohash）→ 通过
R3=$(curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O1Store","displayName":"门店","primaryKey":"id","status":"active",
  "implements":["Locatable"],
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"loc","baseType":"geohash","sharedProperty":"geoCode"}]}')
echo "补齐: $R3"
chk "补齐共享属性后通过" "echo '$R3' | grep -qE '\"saved\":true'" "$R3"

# 4) 未定义接口 → 被拒
R4=$(curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O1Ghost","displayName":"幽灵","primaryKey":"id","status":"active",
  "implements":["NoSuchIface"],
  "properties":[{"apiName":"id","baseType":"string"}]}')
echo "未定义接口: $R4"
chk "声明未定义接口被拒" "echo '$R4' | grep -qE '未定义|接口契约'" "$R4"

# 5) 无 implements 声明 → 不受影响（向后兼容）
R5=$(curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O1Plain","displayName":"普通","primaryKey":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"}]}')
echo "无接口: $R5"
chk "无 implements 的对象类型正常保存" "echo '$R5' | grep -qE '\"saved\":true'" "$R5"

echo ""
echo "O1 接口强校验 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
