#!/usr/bin/env bash
# O4-M2 提交校验 E2E：动作带 FEEL validations → 违规参数被拒（不落库），合规参数通过并写回。
set -uo pipefail
B="http://127.0.0.1:8097/api/onto/v1"
K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 对象类型 M2Acct（余额 balance）
curl "${H[@]}" -X POST "$B/object-types" -d '{
  "apiName":"M2Acct","displayName":"账户","primaryKey":"id","titleProperty":"id","status":"active",
  "properties":[{"apiName":"id","baseType":"string"},{"apiName":"balance","baseType":"decimal"}]
}' >/dev/null
# 种 A-1 余额 100
curl "${H[@]}" -X POST "$B/objects/M2Acct" -d '{"properties":{"id":"A-1","balance":100}}' >/dev/null

# 动作 withdraw：参数 acct/amount；validations= amount>0 且 amount<=1000；logic= 置 balance=$newBal（调用方先算）
curl "${H[@]}" -X POST "$B/action-types" -d '{
  "apiName":"m2Withdraw","displayName":"取款","status":"active",
  "parameters":[{"name":"acct","required":true},{"name":"amount","required":true},{"name":"newBal","required":true}],
  "validations":[
    {"expression":"amount > 0","message":"取款额须为正"},
    {"expression":"amount <= 1000","message":"单次取款不得超过 1000"}
  ],
  "logic":[{"op":"modifyObject","objectType":"M2Acct","pk":"$acct","set":{"balance":"$newBal"}}]
}' >/dev/null

# 1) 违规：amount=-5 → 被拒（含 message），不落库
BAD=$(curl "${H[@]}" -X POST "$B/action-types/m2Withdraw/execute" -d '{"params":{"acct":"A-1","amount":-5,"newBal":105}}')
echo "neg resp: $BAD"
chk "负数取款被校验拒" "echo '$BAD' | grep -q '取款额须为正'" "$BAD"
LD1=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"M2Acct"}}')
chk "校验失败未落库（balance 仍 100）" "echo '$LD1' | grep -qE '\"balance\":100([,}]|\.0)'" "$LD1"

# 2) 违规：amount=5000 → 超限被拒
OVER=$(curl "${H[@]}" -X POST "$B/action-types/m2Withdraw/execute" -d '{"params":{"acct":"A-1","amount":5000,"newBal":-4900}}')
chk "超限取款被校验拒" "echo '$OVER' | grep -q '不得超过 1000'" "$OVER"

# 3) 合规：amount=30 → 通过并写回 balance=70
OK=$(curl "${H[@]}" -X POST "$B/action-types/m2Withdraw/execute" -d '{"params":{"acct":"A-1","amount":30,"newBal":70},"actor":"teller"}')
echo "ok resp: $OK"
chk "合规取款 committed" "echo '$OK' | grep -q '\"status\":\"committed\"'" "$OK"
LD2=$(curl "${H[@]}" -X POST "$B/object-sets/load" -d '{"objectSet":{"op":"base","objectType":"M2Acct"}}')
chk "合规写回 balance=70" "echo '$LD2' | grep -qE '\"balance\":70'" "$LD2"

# 4) dry-run 也走校验：违规 dry-run 应被拒
DRB=$(curl "${H[@]}" -X POST "$B/action-types/m2Withdraw/dry-run" -d '{"params":{"acct":"A-1","amount":0,"newBal":70}}')
chk "dry-run 也执行校验（amount=0 拒）" "echo '$DRB' | grep -q '取款额须为正'" "$DRB"

echo ""
echo "O4-M2 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
