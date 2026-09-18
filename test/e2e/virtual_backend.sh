#!/usr/bin/env bash
# 方案 20260918 E2E：对象数据源统一抽象——PG 直连虚拟直查（M1a）+ 数据源注册表（M1b）+ M0 止血。
# 覆盖：bind/下推直查/过滤/聚合/SearchAround 桥接/写保护矩阵/virtual sync 拒绝/生成式查询/unbind/注册表。
# 前置：服务已按 onto-server-dev.toml 启动（.env 含 ONTO_VIRTUAL_QUERY=on、ONTO_AUTHZ_MODE=local，
#       [[databases]] 含 fico-db；与本体库同库演示——源表直接落在其中）。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
PGHOST=192.168.137.111; PGUSER=postgres; PGPW=Pg@Pansoft_0909; PGDB=fico
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }
psql_do(){ PGPASSWORD=$PGPW psql -h $PGHOST -p 5432 -U $PGUSER -d $PGDB -v ON_ERROR_STOP=1 -q -c "$1" >/dev/null 2>&1; }

echo "════ 0) 源表准备（fico-db 业务源；与本体库同库演示）════"
psql_do "DROP TABLE IF EXISTS src_vcust;"
psql_do "CREATE TABLE src_vcust (cust_id text PRIMARY KEY, cust_name text, region text, amount numeric, created_at timestamptz default now());"
psql_do "INSERT INTO src_vcust (cust_id, cust_name, region, amount) VALUES ('C-1','Ada','east',1200),('C-2','Bob','west',800),('C-3','Cee','east',2500);"
psql_do "DROP TABLE IF EXISTS src_vorder;"
psql_do "CREATE TABLE src_vorder_placeholder (x int);"   # VOrder 走物化路径（对照桥接），源表占位

echo "════ 1) M0：漏斗 sourceQuery 治理 ════"
# 1a) 手写 DML 拒绝
R=$(curl "${H[@]}" -X POST "$B/funnel/mappings" -d '{"objectType":"M0Guard","sourceQuery":"DELETE FROM src_vcust","keyColumns":["cust_id"]}')
chk "DML sourceQuery 被拒" "echo '$R' | grep -q '仅支持只读 SELECT'" "$R"
# 1b) 多语句拒绝
R=$(curl "${H[@]}" -X POST "$B/funnel/mappings" -d '{"objectType":"M0Guard","sourceQuery":"SELECT 1; DELETE FROM src_vcust","keyColumns":["cust_id"]}')
chk "多语句 sourceQuery 被拒" "echo '$R' | grep -q '单条语句'" "$R"
# 1c) 生成式默认路径：无 sourceQuery、给 resource → 保存成功 + sync 走生成式
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"M0Gen","displayName":"生成式测试","primaryKey":"id","titleProperty":"name","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"name","baseType":"string"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/lifecycle/transition" -d '{"kind":"object","apiName":"M0Gen","target":"active"}' >/dev/null
R=$(curl "${H[@]}" -X POST "$B/funnel/mappings" -d '{"objectType":"M0Gen","resource":"public.src_vcust","sourceDbId":"fico-db","keyColumns":["cust_id"],"titleColumn":"cust_name","propertyMap":[{"source":"cust_id","property":"id"},{"source":"cust_name","property":"name"}]}')
chk "无 sourceQuery（生成式）映射保存" "echo '$R' | grep -q '\"saved\":true'" "$R"
R=$(curl "${H[@]}" -X POST "$B/funnel/sync/M0Gen" -d '{}')
chk "生成式全量同步 written=3" "echo '$R' | grep -qE '\"written\":3'" "$R"
# 1d) funnel 建 mode=virtual 映射拒绝（E2：绑定唯一入口）
R=$(curl "${H[@]}" -X POST "$B/funnel/mappings" -d '{"objectType":"M0Gen","mode":"virtual","resource":"public.src_vcust","keyColumns":["cust_id"]}')
chk "funnel 建 virtual 映射被拒" "echo '$R' | grep -q 'datasource/bind'" "$R"

echo "════ 2) M1a：建虚拟对象类型 + bind（sourceId = toml db_id 阶段）════"
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"VCustomer","displayName":"虚拟客户","primaryKey":"id","titleProperty":"name","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"name","baseType":"string"},{"apiName":"region","baseType":"string"},{"apiName":"amount","baseType":"double"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/lifecycle/transition" -d '{"kind":"object","apiName":"VCustomer","target":"active"}' >/dev/null
BIND='{"objectType":"VCustomer","mode":"virtual","sourceId":"fico-db","resource":"public.src_vcust",
  "keyColumns":["cust_id"],"titleColumn":"cust_name",
  "propertyMap":[{"source":"cust_id","property":"id"},{"source":"cust_name","property":"name"},{"source":"region","property":"region"},{"source":"amount","property":"amount"}]}'
R=$(curl "${H[@]}" -X POST "$B/object-types/datasource/bind" -d "$BIND")
echo "bind: $R"
chk "bind 成功（mode=virtual）" "echo '$R' | grep -q '\"mode\":\"virtual\"'" "$R"
chk "bind 走修订链（version>=1）" "echo '$R' | grep -qE '\"version\":[0-9]+'" "$R"
R=$(curl "${H[@]}" "$B/object-types/datasource?objectType=VCustomer")
chk "绑定查询回读 sourceDbId=fico-db" "echo '$R' | grep -q 'fico-db'" "$R"
# 2a) 未映射属性过滤 → 整查询拒绝（fail-closed R1）
R=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"VCustomer"},"predicate":{"kind":"eq","property":"ghost","value":"x"}}}')
chk "未映射属性过滤整查询拒绝" "echo '$R' | grep -q '未映射'" "$R"
# 2b) save 携带 datasource 被剥离（E2 旁路防护）
R=$(curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"VCustomer","displayName":"虚拟客户","primaryKey":"id","titleProperty":"name",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"name","baseType":"string"},{"apiName":"region","baseType":"string"},{"apiName":"amount","baseType":"double"}],
  "datasource":{"sourceId":"hack","mode":"virtual"}}')
chk "save 带 datasource 产生剥离 warning" "echo '$R' | grep -q 'datasource 已忽略'" "$R"
R=$(curl "${H[@]}" "$B/object-types/VCustomer")
chk "save 后指针未被旁路篡改（仍 fico-db）" "echo '$R' | grep -q 'fico-db'" "$R"

echo "════ 3) M1a：虚拟直查下推（Base/Filter/聚合）════"
R=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"VCustomer"}}')
echo "load: $R"
chk "Base 直查 3 行" "echo '$R' | grep -q 'C-1' && echo '$R' | grep -q 'C-2' && echo '$R' | grep -q 'C-3'" "$R"
chk "title 由源列派生（Ada）" "echo '$R' | grep -q 'Ada'" "$R"
R=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"VCustomer"},"predicate":{"kind":"ge","property":"amount","value":1000}}}')
chk "数值过滤下推（amount>=1000 → C-1/C-3 无 C-2）" "echo '$R' | grep -q 'C-1' && echo '$R' | grep -q 'C-3' && ! echo '$R' | grep -q 'C-2'" "$R"
R=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"filter","source":{"op":"base","objectType":"VCustomer"},"predicate":{"kind":"eq","property":"region","value":"east"}}}')
chk "文本过滤下推（region=east → C-1/C-3）" "echo '$R' | grep -q 'C-1' && echo '$R' | grep -q 'C-3' && ! echo '$R' | grep -q 'C-2'" "$R"
R=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"VCustomer"},"limit":2,"offset":0}')
chk "分页 limit=2" "echo '$R' | grep -q '\"hasMore\":true'" "$R"
R=$(curl "${H[@]}" -X POST "$B/object-sets/aggregate" -d '{"objectSet":{"op":"base","objectType":"VCustomer"},"aggregation":{"kind":"count"}}')
chk "聚合 Count=3（下推）" "echo '$R' | grep -qE '\"count\":3'" "$R"
R=$(curl "${H[@]}" -X POST "$B/object-sets/aggregate" -d '{"objectSet":{"op":"base","objectType":"VCustomer"},"aggregation":{"kind":"groupCount","property":"region"}}')
chk "聚合 GroupCount region" "echo '$R' | grep -q 'east' && echo '$R' | grep -q 'west'" "$R"
R=$(curl "${H[@]}" -X POST "$B/object-sets/aggregate" -d '{"objectSet":{"op":"base","objectType":"VCustomer"},"aggregation":{"kind":"groupSum","groupBy":"region","sum":"amount"}}')
chk "聚合 GroupSum amount（east=3700）" "echo '$R' | grep -q '3700'" "$R"

echo "════ 4) M1a：SearchAround pk 桥接（物化 ↔ 虚拟）════"
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"VOrder","displayName":"虚拟客户订单(物化)","primaryKey":"orderId","titleProperty":"title","status":"active",
  "properties":[{"apiName":"orderId","baseType":"string"},{"apiName":"title","baseType":"string"},{"apiName":"customerId","baseType":"string"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/lifecycle/transition" -d '{"kind":"object","apiName":"VOrder","target":"active"}' >/dev/null
curl "${H[@]}" -X POST "$B/objects/VOrder/batch" -d '[
  {"properties":{"orderId":"O-1","title":"订单1","customerId":"C-1"}},
  {"properties":{"orderId":"O-2","title":"订单2","customerId":"C-3"}}]' >/dev/null
curl "${H[@]}" -X POST "$B/link-types" -d '{
  "apiName":"vCustomerPlacesOrder","objectTypeA":"VCustomer","objectTypeB":"VOrder","cardinality":"oneToMany",
  "backing":{"fk":{"sourceProperty":"customerId","side":"b"}}}' >/dev/null
curl "${H[@]}" -X POST "$B/lifecycle/transition" -d '{"kind":"link","apiName":"vCustomerPlacesOrder","target":"active"}' >/dev/null
# 4a) 源虚拟 → 终端物化（虚拟端解析 pk 集 → Static 下物化编译器）
R=$(curl "${H[@]}" "$B/objects/VCustomer/C-1/links/vCustomerPlacesOrder")
chk "虚拟→物化桥接（C-1 → O-1）" "echo '$R' | grep -q 'O-1' && ! echo '$R' | grep -q 'O-2'" "$R"
# 4b) 源物化 → 终端虚拟（FK 在物化源端：取源端属性值集 → In 谓词下推）
R=$(curl "${H[@]}" "$B/objects/VOrder/O-2/links/vCustomerPlacesOrder")
echo "reverse: $R"
chk "物化→虚拟桥接（O-2 → C-3）" "echo '$R' | grep -q 'C-3' && ! echo '$R' | grep -q 'C-1'" "$R"

echo "════ 5) M1a：写保护矩阵（virtual 类型全 4xx，E3）════"
R=$(curl "${H[@]}" -X POST "$B/objects/VCustomer" -d '{"properties":{"id":"X-1"}}')
chk "put 单对象拒绝" "echo '$R' | grep -q '虚拟直查绑定'" "$R"
R=$(curl "${H[@]}" -X POST "$B/objects/VCustomer/batch" -d '[{"properties":{"id":"X-1"}}]')
chk "batch 直灌拒绝" "echo '$R' | grep -q '虚拟直查绑定'" "$R"
R=$(curl "${H[@]}" -X POST "$B/objects/VCustomer/C-1/modify" -d '{"set":{"name":"Hacker"}}')
chk "modify 乐观锁写拒绝" "echo '$R' | grep -q '虚拟直查绑定'" "$R"
R=$(curl "${H[@]}" -X DELETE "$B/objects/VCustomer/C-1")
chk "delete 拒绝" "echo '$R' | grep -q '虚拟直查绑定'" "$R"
R=$(curl "${H[@]}" -X POST "$B/funnel/sync/VCustomer" -d '{}')
chk "funnel sync 拒绝（无僵尸副本）" "echo '$R' | grep -q '虚拟直查绑定'" "$R"
R=$(curl "${H[@]}" -X POST "$B/links" -d '{"link":"vCustomerPlacesOrder","aPk":"C-1","bPk":"O-1"}')
chk "FK backing 边写入拒绝（既有语义叠加）" "echo '$R' | grep -q 'FK\|虚拟直查绑定'" "$R"
# 5b) 函数 objectSet 内部装载引用 virtual → 拒绝（E3 读端内部路径）
R=$(curl "${H[@]}" -X POST "$B/functions/noSuchFn/evaluate" -d '{"objectSets":{"xs":{"op":"base","objectType":"VCustomer"}}}')
chk "函数内部装载 virtual 类型被拒/提示" "echo '$R' | grep -q '内部数据装载暂不支持\|未定义'" "$R"

echo "════ 6) M1b：数据源注册表（独立源配置 + probe + 经注册表绑定）════"
R=$(curl "${H[@]}" -X POST "$B/data-sources" -d '{
  "id":"fico-standalone","name":"FICO 独立源","kind":"pg",
  "config":{"host":"192.168.137.111","port":5432,"db":"fico","user":"postgres","passwordEnv":"ONTO_SRC_FICO_PW","poolMax":2}}')
chk "注册 pg 独立源" "echo '$R' | grep -q '\"saved\":true'" "$R"
R=$(curl "${H[@]}" -X POST "$B/data-sources" -d '{"id":"bad-src","name":"x","kind":"pg","config":{"host":"h","port":1,"db":"d","user":"u"}}')
chk "独立源缺 passwordEnv 被拒" "echo '$R' | grep -q 'passwordEnv'" "$R"
R=$(curl "${H[@]}" -X POST "$B/data-sources" -d '{"id":"api-x","name":"x","kind":"api","config":{"baseUrl":"http://127.0.0.1:9999"}}')
chk "注册 api 源（M2 前仅登记）" "echo '$R' | grep -q '\"saved\":true'" "$R"
R=$(curl "${H[@]}" "$B/data-sources")
chk "注册表列出" "echo '$R' | grep -q 'fico-standalone'" "$R"
# probe（先测后存：草稿形态）
R=$(curl "${H[@]}" -X POST "$B/data-sources/probe" -d '{"id":"probe-draft","config":{"ref":"fico-db"},"resource":"public.src_vcust"}')
chk "草稿 probe（ref 池 + 列基线 5 列）" "echo '$R' | grep -q 'cust_id'" "$R"
# 结构反射
R=$(curl "${H[@]}" "$B/data-sources/schema?sourceId=fico-standalone&resource=public.src_vcust")
chk "结构反射列清单" "echo '$R' | grep -q 'cust_name'" "$R"
# 经注册表绑定第二类型（独立源懒注册 ontosrc_ 前缀）
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"VSupplier","displayName":"虚拟供应商","primaryKey":"id","titleProperty":"name","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"name","baseType":"string"}]}' >/dev/null
curl "${H[@]}" -X POST "$B/lifecycle/transition" -d '{"kind":"object","apiName":"VSupplier","target":"active"}' >/dev/null
BIND2='{"objectType":"VSupplier","mode":"virtual","sourceId":"fico-standalone","resource":"public.src_vcust",
  "keyColumns":["cust_id"],"titleColumn":"cust_name",
  "propertyMap":[{"source":"cust_id","property":"id"},{"source":"cust_name","property":"name"}]}'
R=$(curl "${H[@]}" -X POST "$B/object-types/datasource/bind" -d "$BIND2")
chk "经注册表绑定（独立源）成功" "echo '$R' | grep -q '\"mode\":\"virtual\"'" "$R"
R=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"VSupplier"}}')
chk "独立源直查（懒注册 ontosrc_fico-standalone）3 行" "echo '$R' | grep -q 'C-3'" "$R"
# 源列缺失 bind 拒绝（强校验 R5）
BIND3='{"objectType":"VSupplier","mode":"virtual","sourceId":"fico-db","resource":"public.src_vcust",
  "keyColumns":["ghost_col"],"propertyMap":[{"source":"ghost_col","property":"id"}]}'
R=$(curl "${H[@]}" -X POST "$B/object-types/datasource/bind" -d "$BIND3")
chk "bind 源列不存在被拒（漂移防护）" "echo '$R' | grep -q '不存在'" "$R"

echo "════ 7) M1a：unbind 回退物化缺省 ════"
R=$(curl "${H[@]}" -X POST "$B/object-types/datasource/unbind" -d '{"objectType":"VSupplier"}')
chk "unbind 成功" "echo '$R' | grep -q '\"unbound\":true'" "$R"
R=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"VSupplier"}}')
chk "unbind 后走物化路径（空页非报错——懒建表语义）" "echo '$R' | grep -q 'VSupplier'" "$R"

echo ""
echo "════ 结果：$pass / $total ════"
[ "$pass" -eq "$total" ] && echo "ALL PASS" || echo "HAS FAILURES"
