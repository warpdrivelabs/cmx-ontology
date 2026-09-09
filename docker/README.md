# cmx-ontology Docker 构建

蓝本：`backend/cmx-container/docker/`（V1 容器内编译模式；账密已参数化，Rust 版本对齐本仓 rust-toolchain.toml 的 1.97.1）。

## 文件

| 文件 | 用途 |
| --- | --- |
| `Dockerfile` | 多阶段构建：cargo-chef 依赖缓存 → 容器内编译 → bookworm-slim 运行时 |
| `backend/.dockerignore` | 构建上下文白名单（上下文=backend/ 目录，只送本仓 + cmx-container） |
| `config/docker.toml` | 容器内默认配置模板（数据库/Redis 为占位地址，生产挂载覆盖） |
| `entrypoint.sh` | root 修复挂载目录权限 → 降权 cmx 用户启动 |
| `scripts/build-docker.sh` | 构建脚本（可选推送 Harbor，凭据走环境变量或本地 `.env`） |
| `docker-compose.yml` | 本地运行编排（首次运行从镜像导出配置模板到 `runtime/`） |
| `.env.example` | 本地配置模板：复制为 `.env` 填写（不入库，可扩展任意配置键） |

## 构建

```bash
cd backend/cmx-ontology
./docker/scripts/build-docker.sh              # 时间戳版本
./docker/scripts/build-docker.sh 0.1.0        # 指定版本
```

## 运行

```bash
docker run -d --name cmx-onto -p 8097:8097 \
  --add-host=host.docker.internal:host-gateway \
  -v $(pwd)/config:/app/config \
  cmx-ontology:<版本>
```

配置优先级：挂载的 `/app/config/docker.toml` > 镜像内模板；单键可用环境变量覆盖
（段名+__+键名大写，如 `SERVER__PORT`）。探活为 HTTP 存活检查（curl 不带 -f，收到任意 HTTP 响应即健康；服务无 /api/health 端点）。

## docker compose 运行

```bash
cd backend/cmx-ontology/docker
mkdir -p runtime/config
docker compose run --rm cmx-onto cat /opt/cmx/config-template.toml > runtime/config/docker.toml
vi runtime/config/docker.toml                # 填真实数据库/Redis 地址
IMAGE_TAG=<版本> docker compose up -d        # 或 docker compose up -d --build 就地构建
docker compose logs -f
```

`docker/runtime/` 与 `docker/.env` 已在仓库根 `.gitignore` 排除，真实配置与凭据不会入库。

## 推送 Harbor

方式一（推荐）：填写本地凭据文件（不入库，已在仓库根 `.gitignore` 排除）：

```bash
cp docker/.env.example docker/.env   # 填写真实地址与账密
./docker/scripts/build-docker.sh 0.1.0 --push
```

方式二：直接用环境变量：

```bash
HARBOR_REGISTRY=harbor.example.com:8443 \
HARBOR_USER=admin HARBOR_PASSWORD=xxx \
  ./docker/scripts/build-docker.sh 0.1.0 --push
```

优先级：命令行 `--harbor` > 环境变量 > `docker/.env`；不设 `HARBOR_PASSWORD` 时要求已 `docker login`。
