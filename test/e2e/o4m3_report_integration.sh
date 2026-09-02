#!/usr/bin/env bash
# onto ↔ cmx-report 跨服务真联调 E2E：onto 动作 computeReport 副作用 → dispatcher 经 X-API-Key
# 调 report compute → cmx-report **真算落 cr_cell_data**。断言计算值确由 onto 触发写入。
#
# 前置：① onto-server :8097 以 ONTO_REPORT_API_KEY 启动（reportUrl 默认 :8092）；
#       ② report-server :8092 在线（CONFIG_FILE=report-server-local.toml，DB=192.168.157.46/cmx_fico）。
set -uo pipefail
OB="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
OH=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
RPT="STAT_01_D"; VER="V2"; ORG="CSCEC"; PER="2025"
PGX=(docker run --rm -e PGPASSWORD=9lWsRQY4i4VToPpkHcCRFzki postgres:latest psql -h 192.168.157.46 -p 5432 -U dbuser_dba -d cmx_fico -q -t -A -F'|')
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }
command -v jq >/dev/null 2>&1 || { echo "需要 jq"; exit 1; }
cleanup(){ "${PGX[@]}" -c "DELETE FROM cr_cell_element_map WHERE id=999000001; DELETE FROM cr_cell_data WHERE report_code='$RPT' AND org_code='$ORG' AND period_code='$PER';" >/dev/null 2>&1 || true; }
trap cleanup EXIT

# —— 前置断言（含读连通：onto 代理列 cmx-report 报表）——
CFG=$(curl "${OH[@]}" "$OB/action-outbox/config"); echo "onto config: $CFG"
chk "onto reportUrl=:8092" "echo '$CFG' | grep -q '\"reportUrl\":\"http://127.0.0.1:8092\"'" "$CFG"
chk "onto reportApiKeySet=true(带服务身份调 report)" "echo '$CFG' | grep -q '\"reportApiKeySet\":true'" "$CFG"
RC=$(curl "${OH[@]}" "$OB/report/definitions" | jq -r '.data.reports|length')
chk "onto 代理列报表 count>0(读连通)" "[ \"${RC:-0}\" -gt 0 ]" "count=$RC"

# —— 种可计算单元格（常量公式 1+1）到 STAT_01_D/V2；清旧计算数据 ——
"${PGX[@]}" -c "INSERT INTO cr_cell_element_map (id,code,name,report_code,version_code,sheet_code,region_code,row_id,col_id,cell_ref,value_type,calc_formula,is_editable,sort_no,status,create_time) VALUES (999000001,'ONTOINT_C1','联调单元格','$RPT','$VER','S1','R1',1,1,'A1','number','1+1',0,1,1,now()) ON CONFLICT (id) DO UPDATE SET calc_formula=EXCLUDED.calc_formula;" >/dev/null 2>&1
"${PGX[@]}" -c "DELETE FROM cr_cell_data WHERE report_code='$RPT' AND org_code='$ORG' AND period_code='$PER';" >/dev/null 2>&1
BEFORE=$("${PGX[@]}" -c "SELECT count(*) FROM cr_cell_data WHERE report_code='$RPT' AND org_code='$ORG' AND period_code='$PER';" 2>/dev/null)
chk "计算前无 cr_cell_data(已清)" "[ \"${BEFORE:-x}\" = \"0\" ]" "before=$BEFORE"

# —— onto 动作：computeReport 副作用（reportCode 固定，org/period 走参数映射）——
curl "${OH[@]}" -X POST "$OB/action-types" -d "{
  \"apiName\":\"rptGen\",\"displayName\":\"生成资产负债表\",\"status\":\"active\",
  \"parameters\":[{\"name\":\"org\",\"required\":true},{\"name\":\"period\",\"required\":true}],
  \"logic\":[],\"sideEffects\":[{\"kind\":\"computeReport\",\"reportCode\":\"$RPT\",\"version\":\"$VER\",\"orgCode\":\"\$org\",\"periodCode\":\"\$period\"}]
}" >/dev/null

EX=$(curl "${OH[@]}" -X POST "$OB/action-types/rptGen/execute" -d "{\"params\":{\"org\":\"$ORG\",\"period\":\"$PER\"},\"subjects\":[\"role:admin\"]}")
chk "onto 执行 rptGen effects=1" "echo '$EX' | grep -q '\"effects\":1'" "$EX"
DP=$(curl "${OH[@]}" -X POST "$OB/action-outbox/dispatch" -d '{}'); echo "dispatch: $DP"
chk "dispatch dispatched>=1(computeReport 真投递)" "echo '$DP' | grep -qE '\"dispatched\":[1-9]'" "$DP"
chk "dispatch failed=0" "echo '$DP' | grep -qE '\"failed\":0'" "$DP"
OBX=$(curl "${OH[@]}" "$OB/action-outbox?status=dispatched&limit=50")
chk "onto outbox computeReport→dispatched" "echo '$OBX' | grep -q computeReport" "$OBX"

# —— 验证：cr_cell_data 被 onto 触发的 compute 写入（A1=2）——
sleep 0.3
VAL=$("${PGX[@]}" -c "SELECT num_value FROM cr_cell_data WHERE report_code='$RPT' AND org_code='$ORG' AND period_code='$PER' AND cell_ref='A1';" 2>/dev/null)
chk "cr_cell_data A1 被 onto 触发写入(值=2)" "echo \"$VAL\" | grep -q '^2'" "val=$VAL"

echo ""
echo "onto↔report 跨服务联调 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
