#!/usr/bin/env bash
# 关账联动模板 E2E：从内置模板 consolClose 实例化一个动作 → 一个动作**同时**起关账审批流（flowengine
# consol_close）+ 计算关账报表（cmx-report）。证明「流程/报表联动」已封装为可复用模板并端到端打通。
#
# 前置：onto-server :8097（ONTO_FLOW_API_KEY + ONTO_REPORT_API_KEY）+ flow :8091 + report :8092 在线。
set -uo pipefail
OB="http://127.0.0.1:8097/api/onto/v1"; FB="http://127.0.0.1:8091/api/flow/v1"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
OH=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
FH=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
DIR="$(cd "$(dirname "$0")" && pwd)"
RPT="STAT_01_D"; VER="V2"; ORG="CSCEC"; PER="2025"
PGX=(docker run --rm -e PGPASSWORD=9lWsRQY4i4VToPpkHcCRFzki postgres:latest psql -h 192.168.157.46 -p 5432 -U dbuser_dba -d cmx_fico -q -t -A -F'|')
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }
command -v jq >/dev/null 2>&1 || { echo "需要 jq"; exit 1; }
cleanup(){ "${PGX[@]}" -c "DELETE FROM cr_cell_element_map WHERE id=999000002; DELETE FROM cr_cell_data WHERE report_code='$RPT' AND org_code='$ORG' AND period_code='$PER';" >/dev/null 2>&1 || true; }
trap cleanup EXIT

# —— 模板注册表含 consolClose（起流程 + 生成报表两副作用）——
TPLS=$(curl "${OH[@]}" "$OB/action-templates")
TACT=$(echo "$TPLS" | jq -c '.data.templates[] | select(.key=="consolClose") | .action')
chk "模板注册表含 consolClose" "[ -n '$TACT' ] && [ '$TACT' != 'null' ]" "$(echo "$TPLS" | jq -c '.data.templates[].key' 2>/dev/null)"
chk "consolClose 含 startBusinessProcess(consol_close)" "echo '$TACT' | jq -e '.sideEffects[]|select(.kind==\"startBusinessProcess\" and .flowDefKey==\"consol_close\")' >/dev/null" "$TACT"
chk "consolClose 含 computeReport" "echo '$TACT' | jq -e '.sideEffects[]|select(.kind==\"computeReport\")' >/dev/null" "$TACT"

# —— 准备两端：发布 consol_close 流程 + 种可计算报表单元格 ——
BODY=$(jq -Rs '{name:"期末关账审批", bpmnXml:.}' "$DIR/_consol_close.bpmn")
DKEY=$(curl "${FH[@]}" -X POST "$FB/definitions/draft" -d "$BODY" | jq -r '.data.key // "consol_close"')
curl "${FH[@]}" -X POST "$FB/definitions/$DKEY/publish" -d '{"note":"tmpl-consol"}' >/dev/null 2>&1 || true
chk "consol_close 流程已发布" "[ '$DKEY' = 'consol_close' ]" "key=$DKEY"
"${PGX[@]}" -c "INSERT INTO cr_cell_element_map (id,code,name,report_code,version_code,sheet_code,region_code,row_id,col_id,cell_ref,value_type,calc_formula,is_editable,sort_no,status,create_time) VALUES (999000002,'ONTOTPL_C1','关账单元格','$RPT','$VER','S1','R1',1,1,'A1','number','1+1',0,1,1,now()) ON CONFLICT (id) DO UPDATE SET calc_formula=EXCLUDED.calc_formula;" >/dev/null 2>&1
"${PGX[@]}" -c "DELETE FROM cr_cell_data WHERE report_code='$RPT' AND org_code='$ORG' AND period_code='$PER';" >/dev/null 2>&1

# —— 从模板实例化动作（填 apiName）——
ACTION=$(echo "$TACT" | jq -c '. + {apiName:"tmplClose"}')
curl "${OH[@]}" -X POST "$OB/action-types" -d "$ACTION" >/dev/null
DEF=$(curl "${OH[@]}" "$OB/action-types/tmplClose")
chk "实例化动作 tmplClose 含两副作用" "echo '$DEF' | jq -e '(.data.sideEffects|map(.kind)) as \$k | (\$k|index(\"startBusinessProcess\")) and (\$k|index(\"computeReport\"))' >/dev/null" "$(echo "$DEF" | jq -c '.data.sideEffects|map(.kind)' 2>/dev/null)"

# —— 执行模板动作（org+period）→ 一次触发两联动 ——
EX=$(curl "${OH[@]}" -X POST "$OB/action-types/tmplClose/execute" -d "{\"params\":{\"orgCode\":\"$ORG\",\"periodCode\":\"$PER\"},\"subjects\":[\"role:admin\"]}")
chk "执行 tmplClose effects=2(流程+报表)" "echo '$EX' | grep -q '\"effects\":2'" "$EX"
DP=$(curl "${OH[@]}" -X POST "$OB/action-outbox/dispatch" -d '{}'); echo "dispatch: $DP"
chk "dispatch dispatched=2" "echo '$DP' | grep -qE '\"dispatched\":2'" "$DP"
chk "dispatch failed=0" "echo '$DP' | grep -qE '\"failed\":0'" "$DP"

# —— 验证联动一：flowengine 建了 consol_close 实例 ——
sleep 0.5; IID=""
for _ in 1 2 3 4 5 6; do
  IID=$(curl "${FH[@]}" "$FB/instances" | jq -r --arg bk "$PER" '.data.instances[]|select(.definitionKey=="consol_close" and .businessKey==$bk)|.id' | head -1)
  [ -n "$IID" ] && break; sleep 0.5
done
chk "flowengine 建了 consol_close 实例(businessKey=$PER)" "[ -n '$IID' ]" "iid=$IID"

# —— 验证联动二：cmx-report 真算落 cr_cell_data ——
VAL=$("${PGX[@]}" -c "SELECT num_value FROM cr_cell_data WHERE report_code='$RPT' AND org_code='$ORG' AND period_code='$PER' AND cell_ref='A1';" 2>/dev/null)
chk "cmx-report 真算写入 cr_cell_data A1=2" "echo \"$VAL\" | grep -q '^2'" "val=$VAL"

echo ""
echo "关账联动模板 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
