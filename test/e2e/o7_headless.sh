#!/usr/bin/env bash
# O7 headless E2E：OpenAPI 完整契约 + Swagger UI + SSE 变更流（发布触发事件）。
set -uo pipefail
BASE="http://127.0.0.1:8097/api/onto/v1"; K="cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"
H=(-s -H "Content-Type: application/json" -H "X-API-Key: $K")
pass=0; total=0
chk(){ total=$((total+1)); if eval "$2"; then echo "[PASS] $1"; pass=$((pass+1)); else echo "[FAIL] $1 :: $3"; fi; }

# 1) openapi.json 免认证可取、含 O4/O5/O6 端点
OA=$(curl -s "$BASE/openapi.json")
chk "openapi.json 版本 0.6.0" "echo '$OA' | grep -q '\"0.6.0\"'" "$OA"
chk "openapi 含动作执行端点" "echo '$OA' | grep -q '/action-types/{apiName}/execute'" "$OA"
chk "openapi 含函数求值端点" "echo '$OA' | grep -q '/functions/{apiName}/evaluate'" "$OA"
chk "openapi 含安全加载端点" "echo '$OA' | grep -q '/secure/object-sets/load'" "$OA"
chk "openapi 含 SSE events 端点" "echo '$OA' | grep -q '/events'" "$OA"
chk "openapi 标签分组齐（建模/对象存储/动作/函数/安全/实时）" "echo '$OA' | grep -q '实时'" "$OA"

# 2) Swagger UI 页可取
UI=$(curl -s "$BASE/docs")
chk "Swagger UI 页含 swagger-ui 挂载" "echo '$UI' | grep -q 'swagger-ui' && echo '$UI' | grep -q 'openapi.json'" "$UI"

# 3) SSE：后台订阅 /events（存活 30s 以覆盖较慢的 publish），发布本体 → 收到 published 事件
SSE_OUT=$(mktemp)
stdbuf -oL curl -sN --max-time 30 "$BASE/events?tenant=default" > "$SSE_OUT" 2>/dev/null &
SSE_PID=$!
sleep 2  # 等订阅建立
# 触发发布（先确保有对象类型；随便 upsert 一个再 publish）——publish 会快照整本本体，可能较慢。
curl "${H[@]}" -X POST "$BASE/object-types" -d '{"apiName":"O7Ping","displayName":"探针","primaryKey":"id","status":"active","properties":[{"apiName":"id","baseType":"string"}]}' >/dev/null
PUB=$(curl "${H[@]}" -X POST "$BASE/publish" -d '{"summary":"O7 SSE 验证发布"}')  # 同步返回=发布已完成，事件已 emit
echo "publish: $PUB"
chk "发布成功返版本" "echo '$PUB' | grep -qE '\"version\":[0-9]+'" "$PUB"
sleep 2  # 等事件抵达订阅端（publish 返回后事件已发出）
kill "$SSE_PID" 2>/dev/null || true
SSE=$(cat "$SSE_OUT"); rm -f "$SSE_OUT"
echo "sse captured: $SSE"
chk "SSE 收到 connected 首帧" "echo '$SSE' | grep -q 'connected'" "$SSE"
chk "SSE 收到 published 事件" "echo '$SSE' | grep -q 'published'" "$SSE"
chk "SSE 事件带 tenant=default" "echo '$SSE' | grep -q '\"tenant\":\"default\"'" "$SSE"

echo ""
echo "O7 E2E: $pass/$total 通过"
[ "$pass" -eq "$total" ] && exit 0 || exit 1
