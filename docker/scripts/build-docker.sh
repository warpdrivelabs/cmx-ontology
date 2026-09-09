#!/usr/bin/env bash
# =============================================================================
# cmx-ontology Docker 镜像构建脚本
# =============================================================================
# 蓝本：cmx-container/docker/scripts/build-docker.sh（账密参数化，不写死凭据）。
#
# 用法:
#   ./build-docker.sh [VERSION] [OPTIONS]
#
#   VERSION    镜像版本号；缺省自动生成时间戳（yyyyMMddHHmmss）
#   --push     构建后推送 Harbor
#   --clean    推送前删除本地同名远端 tag
#   --harbor URL   Harbor 地址（与环境变量 HARBOR_REGISTRY 等效，参数优先）
#   -h         帮助
#
# 环境变量（推送时使用；推荐放 docker/.env，该文件不入库）:
#   HARBOR_REGISTRY   Harbor 地址（如 harbor.example.com:8443）
#   HARBOR_USER       用户名
#   HARBOR_PASSWORD   密码（设置则 --password-stdin 登录，不落盘）
#   优先级：命令行 --harbor > 环境变量 > docker/.env
#
# 示例:
#   ./build-docker.sh                        # 本地构建，时间戳版本
#   ./build-docker.sh 0.1.0                  # 指定版本
#   HARBOR_REGISTRY=harbor.example.com:8443 \
#     HARBOR_USER=admin HARBOR_PASSWORD=xxx ./build-docker.sh 0.1.0 --push
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOCKER_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(dirname "$DOCKER_DIR")"
CONTEXT="$(cd "$REPO_ROOT/.." && pwd)"          # backend/ 目录（本仓 + path 引用仓同送上下文）
DOCKERFILE="$DOCKER_DIR/Dockerfile"

# ---- 本地配置文件（docker/.env 不入库，见同目录 .gitignore）----
# 文件格式：KEY=VALUE（# 开头为注释）；通用键值装载，任何变量均可放此处；
# 环境变量已设置的键不覆盖（环境变量优先）
ENV_FILE="$DOCKER_DIR/.env"
if [ -f "$ENV_FILE" ]; then
    echo "---- 读取本地配置: docker/.env ----"
    while IFS= read -r _line || [ -n "${_line:-}" ]; do
        _line="${_line%$'\r'}"
        case "$_line" in ''|\#*) continue ;; esac
        _key="${_line%%=*}"; _val="${_line#*=}"
        _key="$(printf '%s' "$_key" | tr -d '[:space:]')"
        _val="${_val%\"}"; _val="${_val#\"}"; _val="${_val%\'}"; _val="${_val#\'}"
        if [ -z "${_key:-}" ]; then continue; fi
        [ -z "${!_key+x}" ] && export "${_key}=${_val}"
    done < "$ENV_FILE"
fi

# ---- 参数解析 ----
if [ -z "${1:-}" ] || [[ "${1:-}" == --* ]]; then
    VERSION=$(date +%Y%m%d%H%M%S)
else
    VERSION="$1"; shift
fi
PUSH=false; CLEAN=false
while [ $# -gt 0 ]; do
    case "$1" in
        --push)  PUSH=true;  shift ;;
        --clean) CLEAN=true; shift ;;
        --harbor) HARBOR_REGISTRY="$2"; shift 2 ;;
        -h|--help) grep '^#' "$0" | sed 's/^# \?//'; exit 0 ;;
        *) echo "未知参数: $1（-h 看用法）" >&2; exit 2 ;;
    esac
done

HARBOR_REGISTRY="${HARBOR_REGISTRY:-}"
IMAGE_NAME="cmx-ontology"
REMOTE_IMAGE="${HARBOR_REGISTRY:+$HARBOR_REGISTRY/}cmx/${IMAGE_NAME}:${VERSION}"

# ---- 上下文前置检查 ----
[ -f "$CONTEXT/cmx-container/Cargo.toml" ] || {
    echo "错误: 上下文 $CONTEXT 下找不到 cmx-container（跨仓 path 依赖）" >&2
    exit 1
}

# ---- 构建（BuildKit：启用 Dockerfile.dockerignore 白名单） ----
export DOCKER_BUILDKIT=1
BUILD_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
VCS_REF=$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)

echo "==== 构建 ${IMAGE_NAME}:${VERSION} ===="
echo "  上下文:   $CONTEXT"
echo "  仓 commit: $VCS_REF"
docker build \
    --build-arg VERSION="${VERSION}" \
    --build-arg BUILD_DATE="${BUILD_DATE}" \
    --build-arg VCS_REF="${VCS_REF}" \
    -t "${IMAGE_NAME}:${VERSION}" \
    -f "$DOCKERFILE" "$CONTEXT"

# ---- 推送 ----
if [ "$PUSH" = true ]; then
    [ -n "${HARBOR_REGISTRY:-}" ] || {
        echo "错误: --push 需要环境变量 HARBOR_REGISTRY（可选 HARBOR_USER/HARBOR_PASSWORD）" >&2
        exit 1
    }
    if [ -n "${HARBOR_PASSWORD:-}" ]; then
        echo "---- docker login $HARBOR_REGISTRY（--password-stdin）----"
        printf '%s' "$HARBOR_PASSWORD" | docker login "$HARBOR_REGISTRY" -u "${HARBOR_USER:-}" --password-stdin
    fi
    if [ "$CLEAN" = true ]; then
        docker rmi "$REMOTE_IMAGE" 2>/dev/null || true
    fi
    echo "---- 推送 $REMOTE_IMAGE ----"
    docker tag "${IMAGE_NAME}:${VERSION}" "$REMOTE_IMAGE"
    docker push "$REMOTE_IMAGE"
    echo "已推送: $REMOTE_IMAGE"
fi

echo "==== 完成，本地镜像: ===="
docker images "${IMAGE_NAME}"
