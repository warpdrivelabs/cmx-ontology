#!/usr/bin/env bash
#
# 启动本体平台微服务（ONTOLOGY · :8097）。
#
# 统一启动契约（门户/流程/报表/主数据/规则/本体各服务同一套）：
#   1) cd 到本 workspace 根（.env / *-server.toml 的相对路径基准）
#   2) cargo run 对应 bin（bin 自动读 .env → 配置生效，无需手动 source）
#
# 用法：
#   ./onto.sh                 # 开发模式（debug，读 onto-server-dev.toml 连开发库）
#   ./onto.sh --release       # 发布模式（透传给 cargo run）
#   CONFIG_FILE=onto-server.toml ./onto.sh   # 指定配置
#
# 依赖：PostgreSQL（含 om_* 本体定义/发布表，首启自动建）。
# 起后访问：
#   http://127.0.0.1:8097/                          本体建模控制台
#   http://127.0.0.1:8097/api/onto/v1/stats         各类型计数
#   http://127.0.0.1:8097/api/onto/v1/manifest      本体全量清单
#   http://127.0.0.1:8097/_mon                       技术监控
set -euo pipefail
cd "$(dirname "$0")"
export CONFIG_FILE="${CONFIG_FILE:-onto-server-dev.toml}"
exec cargo run -p cmx-onto-server "$@"
