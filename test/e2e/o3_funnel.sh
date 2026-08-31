#!/usr/bin/env bash
# O3 数据集成 E2E：既有源表 → 映射 → 全量同步物化为对象；违规行入隔离区；管道状态。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
PG="postgres://dbuser_dba:9lWsRQY4i4VToPpkHcCRFzki@192.168.157.46:5432/cmx_fico"
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }
psql_do(){ docker run --rm -e PGPASSWORD=9lWsRQY4i4VToPpkHcCRFzki postgres:latest psql -h 192.168.157.46 -p 5432 -U dbuser_dba -d cmx_fico -v ON_ERROR_STOP=1 -q -c "$1" >/dev/null 2>&1; }

# 0) 建源表 src_o3cust（3 好行 + 1 坏行：cust_name 为空 → 违反 required=name）
psql_do "DROP TABLE IF EXISTS src_o3cust;"
psql_do "CREATE TABLE src_o3cust (cust_id text, cust_name text, region_code text);"
psql_do "INSERT INTO src_o3cust VALUES ('C-1','Ada','east'),('C-2','Bob','west'),('C-3','Cee','east'),('C-4',NULL,'north');"

# 1) 建对象类型 O3Cust
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"O3Cust","displayName":"客户","primaryKey":"id","titleProperty":"name","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"name","baseType":"string"},{"apiName":"region","baseType":"string"}]}' >/dev/null

# 2) 建源映射
MAP='{"objectType":"O3Cust","sourceQuery":"SELECT cust_id, cust_name, region_code FROM src_o3cust",
  "keyColumns":["cust_id"],"titleColumn":"cust_name",
  "propertyMap":[{"source":"cust_id","property":"id"},{"source":"cust_name","property":"name"},{"source":"region_code","property":"region"}],
  "required":["name"]}'
MS=$(curl "${H[@]}" -X POST "$B/funnel/mappings" -d "$MAP")
chk "源映射保存" "echo '$MS' | grep -q '\"saved\":true'" "$MS"
ML=$(curl "${H[@]}" "$B/funnel/mappings")
chk "映射列表含 O3Cust" "echo '$ML' | grep -q 'O3Cust' && echo '$ML' | grep -q 'src_o3cust'" "$ML"

# 3) 全量同步 → read 4 / written 3 / quarantined 1
SY=$(curl "${H[@]}" -X POST "$B/funnel/sync/O3Cust" -d '{}')
echo "sync: $SY"
chk "同步 read=4" "echo '$SY' | grep -qE '\"read\":4'" "$SY"
chk "同步 written=3" "echo '$SY' | grep -qE '\"written\":3'" "$SY"
chk "同步 quarantined=1" "echo '$SY' | grep -qE '\"quarantined\":1'" "$SY"

# 4) 对象已物化（对象集加载见 C-1/C-2/C-3，无 C-4）
LD=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"O3Cust"}}')
echo "loaded: $LD"
chk "物化对象含 C-1/C-2/C-3" "echo '$LD' | grep -q 'C-1' && echo '$LD' | grep -q 'C-2' && echo '$LD' | grep -q 'C-3'" "$LD"
chk "违规行 C-4 未进主库" "! echo '$LD' | grep -q 'C-4'" "$LD"
chk "属性映射正确（Ada/east）" "echo '$LD' | grep -q 'Ada' && echo '$LD' | grep -q 'east'" "$LD"

# 5) 隔离区含 C-4 + violations
QO=$(curl "${H[@]}" "$B/funnel/quarantine?objectType=O3Cust")
echo "quarantine: $QO"
chk "隔离区含 C-4" "echo '$QO' | grep -q 'C-4'" "$QO"
chk "隔离区记 violations(name 缺失)" "echo '$QO' | grep -qiE 'name|必填'" "$QO"

# 6) 管道状态
PS=$(curl "${H[@]}" "$B/funnel/pipeline-status/O3Cust")
echo "pipeline: $PS"
chk "管道 extract/map ready" "echo '$PS' | grep -q 'extract' && echo '$PS' | grep -q 'ready'" "$PS"
chk "管道 index objects=3" "echo '$PS' | grep -qE '\"objects\":3'" "$PS"
chk "管道 quarantined=1" "echo '$PS' | grep -qE '\"quarantined\":1'" "$PS"

# 7) 增量幂等：改一行源数据 + 再同步 → upsert 覆盖（C-1 region 变 south）
psql_do "UPDATE src_o3cust SET region_code='south' WHERE cust_id='C-1';"
curl "${H[@]}" -X POST "$B/funnel/sync/O3Cust" -d '{}' >/dev/null
LD2=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"O3Cust"},"predicate":{"kind":"eq","property":"id","value":"C-1"}}}')
chk "再同步 upsert 覆盖（C-1 region=south）" "echo '$LD2' | grep -q 'south'" "$LD2"

# 清理源表
psql_do "DROP TABLE IF EXISTS src_o3cust;"
echo ""
echo "O3 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
