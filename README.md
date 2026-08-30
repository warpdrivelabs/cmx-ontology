# cmx-ontology · Palantir 式企业本体平台（Rust）

以 Palantir Foundry Ontology 为蓝本、用 Rust 构建的企业本体平台。独立微服务（`:8097`），
「一芯多壳」骨架，与 `cmx-flowengine` / `cmx-rulesengine` 同构。

> 完整方案：`../docs/20260828_cmx-ontology_Palantir式企业本体平台_Rust完整建设方案.md`

## 进度

- **O0 骨架** ✅ 独立 workspace + 一芯多壳 + chassis 装配 + `:8097` boot + 多租户接缝。
- **O1 建模引擎** ✅ 本体元模型（对象/属性/关系/接口/共享属性/动作/函数类型）+ CRUD +
  `om_*` 持久化 + 发布/版本快照 + 建模台雏形（自包含 HTML 控制台，根路径 `/`）。
- O2+ 对象存储与索引 / 动作引擎 / 函数计算 / 数据集成 / 安全 / SDK / 前端联邦 —— 见方案路线图。

## crate（一芯多壳）

| crate | 角色 |
| --- | --- |
| `cmx-onto-model` | 【芯·内核】元模型类型 + `OntologyStore` 契约 + 错误（框架无关，零 IO） |
| `cmx-onto-store-pg` | `OntologyStore` 的 tokio-postgres 实现（`om_*` 表 + 发布/版本） |
| `cmx-onto-app` | 【芯·应用】handlers + `onto_routes::<S>()` + 租户/认证/响应横切 + 建模台 |
| `cmx-onto-server` | 【壳·独立】独立 bin `:8097`（chassis `run()`） |

平台内嵌壳 `cmx-onto-api`（留 `cmx-container`，调 `onto_routes::<CmxAppState>()`）待 O8。

## 运行

```bash
# 真机开发库（192.168.157.46:5432/cmx_fico）
CONFIG_FILE=onto-server-dev.toml cargo run -p cmx-onto-server
# 浏览器打开 http://127.0.0.1:8097/ 进入本体建模控制台
```

## API（v1 契约，前缀 `/api/onto/v1`；旧前缀 `/api/onto` 内嵌壳兼容）

```
GET/POST     /object-types            列表 / upsert（结构校验）
POST         /object-types/validate   仅校验
GET/DELETE   /object-types/{apiName}  详情 / 删除
GET/POST     /link-types              GET/DELETE /link-types/{apiName}
GET/POST     /interfaces             GET/DELETE /interfaces/{apiName}
GET/POST     /shared-properties      DELETE     /shared-properties/{apiName}
GET/POST     /action-types           GET/DELETE /action-types/{apiName}
GET/POST     /functions              GET/DELETE /functions/{apiName}
GET          /manifest                本体全量清单
POST         /publish                 发布快照（body: {summary}）
GET          /versions                版本列表
GET          /versions/{version}      某版本快照
GET          /stats                   建模台数据源（各类型计数）
```
