#!/bin/sh
# =============================================================================
# CMX 服务容器启动脚本：root 修复挂载目录权限 → 降权 cmx 用户启动
# =============================================================================
set -e

if [ "$(id -u)" = "0" ]; then
    echo "[entrypoint] 修复挂载目录权限..."
    for d in /app/config /app/logs /app/plugins /app/storage /app/uploads; do
        [ -d "$d" ] && chown -R cmx:cmx "$d"
    done
    echo "[entrypoint] 切换 cmx 用户启动..."
    exec su -s /bin/sh -c "exec \"\$0\" \"\$\"" cmx -- "$@"
fi

exec "$@"
