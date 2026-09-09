# =============================================================================
# cmx-ontology 服务镜像 —— 多阶段构建（cargo-chef 依赖缓存 → 容器内编译 → slim 运行时）
# =============================================================================
# 蓝本：backend/cmx-container/docker/Dockerfile（V1 容器内原生编译模式；该目录为旧
#       web-server 时代产物，本文件按其骨架适配现行服务仓，Rust 版本对齐各仓
#       rust-toolchain.toml 的 1.97.1）。
#
# ★ 构建上下文 = backend/ 目录
#   本仓经跨 workspace path 引用 cmx-container，相关仓必须随上下文同送；
#   上下文白名单在上下文根的 .dockerignore（backend/.dockerignore，各引擎构建共用）。
# 构建：cd backend/cmx-ontology && ./docker/scripts/build-docker.sh [版本号]
# 运行：docker run -d --name cmx-onto -p 8097:8097 \
#         --add-host=host.docker.internal:host-gateway \
#         -v /宿主配置目录:/app/config \
#         cmx-ontology:<版本>
#   镜像内 /app/config/docker.toml 仅为模板（数据库/Redis 是占位地址），
#   生产环境用 -v 把真实配置目录挂到 /app/config 覆盖；
#   单键亦可用环境变量覆盖（段名+__+键名大写，如 SERVER__PORT / ASSETS__ROOT）。
# =============================================================================

# ---------- 阶段1：依赖编译基座（cargo-chef 依赖层缓存） ----------
FROM rust:1.97-slim-bookworm AS chef
WORKDIR /build
# 国内源：rustup 工具链走 rsproxy；cargo 镜像源 + nora registry 定义随本仓 .cargo 挂到 CARGO_HOME
#（planner 的 cargo metadata 解析 cmx-container 根 manifest 的 [patch.nora] 也需要该定义）
ENV RUSTUP_DIST_SERVER="https://rsproxy.cn"
ENV RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
COPY cmx-ontology/.cargo /root/.cargo
# 各仓 rust-toolchain.toml 钉 1.97.1：按 pin 精确预装（planner/builder 继承，免重复下载）
COPY cmx-ontology/rust-toolchain.toml ./rust-toolchain.toml
RUN cargo --version && cargo install cargo-chef --locked

# ---------- 阶段2：分析依赖生成 recipe.json ----------
FROM chef AS planner
COPY cmx-ontology/ /build/cmx-ontology/
COPY cmx-container/ /build/cmx-container/
WORKDIR /build/cmx-ontology
RUN cargo chef prepare --recipe-path recipe.json

# ---------- 阶段3：编译（依赖层可缓存，源码层最后） ----------
FROM chef AS builder
# .cargo 镜像源配置已由 chef 阶段挂载（继承）；path 依赖仓保持与仓内 ../ 引用同构的 sibling 布局
COPY cmx-container/ /build/cmx-container/
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# rustfmt：build.rs 代码生成（rpc-gen 等）格式化需要
RUN rustup component add rustfmt
COPY cmx-ontology/ /build/cmx-ontology/
COPY --from=planner /build/cmx-ontology/recipe.json /build/cmx-ontology/recipe.json
WORKDIR /build/cmx-ontology
# 依赖层：只依赖各仓 Cargo.toml/Cargo.lock，改源码不失效
RUN cargo chef cook --release --recipe-path recipe.json
# 源码层：重拷本仓源码后编译目标 bin
COPY cmx-ontology/ /build/cmx-ontology/
RUN cargo build --release -p cmx-onto-server

# ---------- 阶段4：运行时 ----------
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd -r cmx && useradd -r -g cmx -m -d /app cmx
WORKDIR /app
COPY --from=builder --chown=cmx:cmx /build/cmx-ontology/target/release/cmx-onto-server /app/cmx-onto-server
# 资产随包（cmx-container 资产真源）
COPY --chown=cmx:cmx cmx-container/assets/onto/ /app/assets/onto/
COPY --chown=cmx:cmx cmx-ontology/docker/config/docker.toml /app/config/docker.toml
# 模板副本放挂载点之外：compose 挂载 runtime/config 后仍可从镜像导出模板
COPY --chown=cmx:cmx cmx-ontology/docker/config/docker.toml /opt/cmx/config-template.toml
COPY --chown=cmx:cmx cmx-ontology/docker/entrypoint.sh /app/entrypoint.sh
RUN mkdir -p /app/config /app/logs \
    && chown cmx:cmx /app/config /app/logs
ENV CONFIG_FILE=/app/config/docker.toml \
    RUST_LOG=info
VOLUME ["/app/config", "/app/logs"]
EXPOSE 8097
# 服务无 /api/health 端点：能返回任意 HTTP 响应即视为存活（curl 不带 -f，不校验状态码；深度健康检查用编排层探针）
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -s --max-time 3 -o /dev/null http://127.0.0.1:8097/
ENTRYPOINT ["/app/entrypoint.sh"]
CMD ["/app/cmx-onto-server"]

ARG BUILD_DATE
ARG VERSION
ARG VCS_URL
ARG VCS_REF
LABEL org.opencontainers.image.title="cmx-ontology" \
      org.opencontainers.image.description="CMX 本体平台服务" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.source="${VCS_URL}" \
      org.opencontainers.image.revision="${VCS_REF}"
