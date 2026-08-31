#!/usr/bin/env bash
# OSDK 代码生成 E2E：生成 TypeScript SDK → 校验接口/客户端 shape → tsc 编译证明有效 TS。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "X-API-Key: $K")
TSC="../cmx-ontology-graph/.tsc-tool/node_modules/typescript/bin/tsc"
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 种一个已知类型（确保 SDK 含它）
curl -s -H "Content-Type: application/json" -H "X-API-Key: $K" -X POST "$B/object-types" -d '{
  "apiName":"SdkCust","displayName":"客户","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string","required":true},{"apiName":"amount","baseType":"decimal"},{"apiName":"active","baseType":"boolean"}]}' >/dev/null

# 生成 SDK
SDK=$(curl "${H[@]}" "$B/osdk/typescript")
echo "$SDK" > /tmp/ontology-sdk.ts
LINES=$(wc -l < /tmp/ontology-sdk.ts)
echo "SDK lines: $LINES"
chk "SDK 含 SdkCust 强类型接口" "grep -q 'export interface SdkCust extends OntObject' /tmp/ontology-sdk.ts" "$(head -c 100 /tmp/ontology-sdk.ts)"
chk "SdkCust.id 必填(无?)" "grep -qE '  id: string;' /tmp/ontology-sdk.ts" ""
chk "SdkCust.amount 可选 number" "grep -qE '  amount\?: number;' /tmp/ontology-sdk.ts" ""
chk "SdkCust.active 可选 boolean" "grep -qE '  active\?: boolean;' /tmp/ontology-sdk.ts" ""
chk "含 OntologyClient 客户端类" "grep -q 'export class OntologyClient' /tmp/ontology-sdk.ts" ""
chk "含 objects.SdkCust.all/filter" "grep -qE 'SdkCust: \{' /tmp/ontology-sdk.ts && grep -q 'all: (limit' /tmp/ontology-sdk.ts" ""
chk "含 actions 调用器" "grep -qE 'actions = \{' /tmp/ontology-sdk.ts" ""
chk "含 functions 调用器" "grep -qE 'functions = \{' /tmp/ontology-sdk.ts" ""
chk "含 searchAround" "grep -q 'searchAround<T' /tmp/ontology-sdk.ts" ""
chk "含 ObjectSet 代数类型" "grep -q 'export type ObjectSet' /tmp/ontology-sdk.ts" ""

# tsc 编译证明有效 TypeScript（strict + noEmit）
TSCOUT=$(node "$TSC" --strict --noEmit --skipLibCheck --lib ES2022,DOM /tmp/ontology-sdk.ts 2>&1)
TSCRC=$?
echo "tsc rc=$TSCRC ${TSCOUT:0:200}"
chk "生成的 SDK 通过 tsc 严格编译" "[ $TSCRC -eq 0 ]" "$TSCOUT"

echo ""
echo "OSDK E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
