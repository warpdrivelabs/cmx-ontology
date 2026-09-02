#!/usr/bin/env bash
# cmx-flow / cmx-report 连通性全面测试：配置、可达、鉴权姿态、读代理、错误传播（双服务）。
# 深度触发（真建实例 / 真算落库 / 模板双联动）见 o4m3_flow_integration、o4m3_report_integration、
# o4m3_template_consol_close。前置：onto:8097(双 key) + flow:8091 + report:8092 在线。
set -uo pipefail
OB="http://127.0.0.1:8097/api/onto/v1"; FB="http://127.0.0.1:8091/api/flow/v1"; RB="http://127.0.0.1:8092/api/report-design"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
OH=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }
command -v jq >/dev/null 2>&1 || { echo "需要 jq"; exit 1; }
docker run --rm -e PGPASSWORD=9lWsRQY4i4VToPpkHcCRFzki postgres:latest psql -h 192.168.157.46 -p 5432 -U dbuser_dba -d cmx_fico -q -c "DELETE FROM oe_outbox;" >/dev/null 2>&1 || true

CFG=$(curl "${OH[@]}" "$OB/action-outbox/config")
F1OK=$(echo "$CFG" | jq -r 'if .data.flowUrl=="http://127.0.0.1:8091" and .data.flowApiKeySet==true then 1 else 0 end')
R1OK=$(echo "$CFG" | jq -r 'if .data.reportUrl=="http://127.0.0.1:8092" and .data.reportApiKeySet==true then 1 else 0 end')

echo "──── cmx-flow 连通性 ────"
chk "F1 onto 配置 flowUrl=:8091 + flowApiKeySet=true" "[ \"$F1OK\" = 1 ]" "$CFG"
F2=$(curl -s -m5 "$FB/definitions" -H "X-API-Key: $K" -o /dev/null -w "%{http_code}")
chk "F2 flow 直连可达（带 key→200）" "[ \"$F2\" = 200 ]" "http=$F2"
F3=$(curl -s -m5 "$FB/definitions" -o /dev/null -w "%{http_code}")
chk "F3 flow 鉴权强制（no-key→401）" "[ \"$F3\" = 401 ]" "http=$F3"
FN=$(curl "${OH[@]}" "$OB/flow/definitions" | jq -r '.data.definitions|length')
chk "F4 onto→flow 读代理（定义数>0）" "[ \"${FN:-0}\" -gt 0 ]" "n=$FN"
curl "${OH[@]}" -X POST "$OB/action-types" -d '{"apiName":"connFlowBad","displayName":"坏流程键","status":"active","parameters":[],"logic":[],"sideEffects":[{"kind":"startBusinessProcess","flowDefKey":"__no_such_flow__"}]}' >/dev/null
curl "${OH[@]}" -X POST "$OB/action-types/connFlowBad/execute" -d '{"params":{},"subjects":["role:admin"]}' >/dev/null
FFAIL=$(curl "${OH[@]}" -X POST "$OB/action-outbox/dispatch" -d '{}' | jq -r '.data.failed')
chk "F5 onto→flow 错误传播（坏键→failed>=1）" "[ \"${FFAIL:-0}\" -ge 1 ]" "failed=$FFAIL"
FERR=$(curl "${OH[@]}" "$OB/action-outbox?status=failed&limit=10" | jq -r '.data[]|select(.kind=="startBusinessProcess")|.lastError' | head -1)
chk "F5b flow 业务错误消息回传 outbox（未部署）" "echo \"$FERR\" | grep -q '未部署'" "err=$FERR"

echo "──── cmx-report 连通性 ────"
chk "R1 onto 配置 reportUrl=:8092 + reportApiKeySet=true" "[ \"$R1OK\" = 1 ]" "$CFG"
R2=$(curl -s -m5 "$RB/reports" -H "X-API-Key: $K" -o /dev/null -w "%{http_code}")
chk "R2 report 直连可达（带 key→200）" "[ \"$R2\" = 200 ]" "http=$R2"
R3=$(curl -s -m5 "$RB/reports" -o /dev/null -w "%{http_code}")
chk "R3 report 鉴权强制（no-key→401）" "[ \"$R3\" = 401 ]" "http=$R3"
RN=$(curl "${OH[@]}" "$OB/report/definitions" | jq -r '.data.reports|length')
chk "R4 onto→report 读代理（报表数>0）" "[ \"${RN:-0}\" -gt 0 ]" "n=$RN"
curl "${OH[@]}" -X POST "$OB/action-types" -d '{"apiName":"connRptBad","displayName":"坏报表码","status":"active","parameters":[],"logic":[],"sideEffects":[{"kind":"computeReport","reportCode":"__NO_RPT__","orgCode":"CSCEC","periodCode":"2025"}]}' >/dev/null
curl "${OH[@]}" -X POST "$OB/action-types/connRptBad/execute" -d '{"params":{},"subjects":["role:admin"]}' >/dev/null
RFAIL=$(curl "${OH[@]}" -X POST "$OB/action-outbox/dispatch" -d '{}' | jq -r '.data.failed')
chk "R5 onto→report 错误传播（坏码→failed>=1）" "[ \"${RFAIL:-0}\" -ge 1 ]" "failed=$RFAIL"
RERR=$(curl "${OH[@]}" "$OB/action-outbox?status=failed&limit=10" | jq -r '.data[]|select(.kind=="computeReport")|.lastError' | head -1)
chk "R5b report 业务错误消息回传 outbox（无版本/无法计算）" "echo \"$RERR\" | grep -qE '无版本|无法计算'" "err=$RERR"

echo ""
echo "cmx-flow/cmx-report 连通性 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
