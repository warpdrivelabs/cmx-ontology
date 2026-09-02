# cmx-ontology 本体平台 · 全量测试报告

- **日期**：2026-08-31
- **被测服务**：`cmx-onto-server` @ `127.0.0.1:8097`（独立微服务壳，一芯多壳）
- **联调对端**：`cmx-flow-server` @ `127.0.0.1:8091` + `cmx-rpt-server` @ `127.0.0.1:8092`（均 jwt 模式 + 服务 API Key，DB 同 `cmx_fico`）；onto 以 `ONTO_FLOW_API_KEY` / `ONTO_REPORT_API_KEY` 注入服务身份调用
- **数据库**：`cmx_fico` @ `192.168.157.46:5432`（团队开发库，`om_*`/`oo_*`/`ol_*`/`oe_*` + flow `cmx_flow_*` 共库）
- **认证**：`mode=off`，`tenancy=single`，`X-API-Key → tenant=default`
- **前端**：Playwright 无头 Chromium（node v25.2.1，`NODE_PATH=/Users/nanomesh/node_modules`），门户 `@8080/portal`
- **工具链**：docker psql（DB 断言/清表）、tsc（OSDK 严格编译校验）
- **结论**：**全部通过 274/274，0 失败，0 前端控制台错误**

---

## 一、总览

| 层次 | 套件数 | 断言/用例数 | 结果 |
|---|---:|---:|:--:|
| 单元测试（Rust `cargo test`） | 3 crate | **54** | ✅ 54/54 |
| 后端功能测试（E2E, curl+docker psql） | 15 | **158** | ✅ 158/158 |
| 前端功能测试（CDP, Playwright） | 14 | **139** | ✅ 139/139 |
| **合计** | **32** | **351** | ✅ **351/351** |

---

## 二、单元测试（`cargo test --workspace --lib`）

| Crate | 测试数 | 覆盖 |
|---|---:|---|
| `cmx-onto-model`（内核，零 IO） | 47 | FEEL 引擎（tokenizer/Pratt/eval/~25 builtins）、动作 `resolve_edits`/`validate_params`/`run_validations`、副作用 `resolve_side_effects`（`$name` 替换/插值）、函数 `evaluate`、`authz` 残差集/脱敏、`funnel` 映射、`import` DOC/DCT 归一化、`osdk` TS 生成、对象集 `compile` |
| `cmx-onto-store-pg`（PG 存储） | 7 | `row_text`/DataValue 转换、Outbox 领取/终态、乐观锁合并等纯逻辑 |
| `cmx-onto-app`（应用层） | 0 | handler 全部经后端 E2E 覆盖（泛型 `<S>`，无独立单测） |
| **合计** | **54** | — |

> 结果：`cargo test: 54 passed (3 suites)`。

---

## 三、后端功能测试（`test/e2e/*.sh`）

| # | 套件 | 断言 | 覆盖能力 |
|---:|---|---:|---|
| 1 | `o3_funnel` | 14 | **O3 数据集成 Funnel**：源→字段映射→全量同步（Full=替换）+ 校验失败入隔离区 `oo_quarantine` |
| 2 | `o4_action_engine` | 12 | **O4 动作执行**：`logic`→`ObjectEdit` 单事务写回 + dry-run + `oe_action_log` 审计 |
| 3 | `o4m2_validations` | 6 | **O4-M2 提交校验**：FEEL 校验表达式，负例（`取款额须为正`）拦截 |
| 4 | `o4_pep_optlock` | 9 | **O4 写侧安全**：PEP 权限门（`check_action_permission`）+ 乐观锁（`expectedUpdatedAt` 冲突检测） |
| 5 | `o5_functions` | 6 | **O5 函数计算**：FEEL 求值 FunctionDef（Query/派生/objectSet/Aggregation） |
| 6 | `o6_dynamic_security` | 9 | **O6 动态安全**：残差 Filter 合并 + marking 字段脱敏（`om_policy` 自建） |
| 7 | `o7_headless` | 11 | **O7 headless**：v1 契约 + OpenAPI/Swagger + SSE 事件流 + API Key |
| 8 | `p1_doc_dct_import` | 11 | **P1 反向导入**：DOC/DCT 归一化 JSON → 对象类型 + 组合/参照关系 + 种子项（解耦，不依赖 cmx-model） |
| 9 | `p2_osdk` | 11 | **P2 OSDK 代码生成**：读本体 → 强类型 TS 客户端；**生成的 SDK 过 `tsc --strict` 编译** |
| 10 | `o4m3_outbox` | 11 | **O4-M3 副作用**：解析入 Outbox（`oe_outbox`）+ 事务性（随动作一起提交） |
| 11 | `o4m3_dispatcher` | 9 | **O4-M3 dispatcher**：`emitEvent`→SSE、`callFunction`→O5、`webhook` 外部 host 被 SSRF 白名单拦→`failed` |
| 12 | `o4m3_dispatch_delivery` | 18 | **dispatcher 真投递**：`webhook` 真发 HTTP + `startBusinessProcess` 调 flow v1 `/api/flow/v1/instances`（本地 sink 捕获真出站，断言 `definitionKey` 插值/`X-Tenant`/payload/终态/幂等） |
| 13 | `o4m3_flow_integration` | 12 | **onto↔flowengine 跨服务真联调**：onto 动作 `startBusinessProcess` → dispatcher 经 `X-API-Key` 调**真实 flow-server**（jwt 模式）→ flowengine **真建流程实例**；按唯一 `businessKey` 定位实例，断言 `definitionKey`/`variables.orderId`（onto 透传）/停在 `approve` 节点 |
| 14 | `o4m3_report_integration` | 9 | **onto↔cmx-report 跨服务真联调**：onto 动作 `computeReport` → dispatcher 经 `X-API-Key` 调**真实 rpt-server** compute → cmx-report **真算落 `cr_cell_data`**；seed 常量公式单元格（`1+1`），断言 onto 触发的计算写入 `A1=2`（含读连通：`/report/definitions` 代理列 213 张报表） |
| 15 | `o4m3_template_consol_close` | 10 | **关账联动模板（flow+report 双联动）**：从内置模板 `consolClose` 实例化一个动作 → **一个动作同时**起关账审批流（flowengine `consol_close`）+ 算关账报表（cmx-report）；断言实例化含两副作用、执行 `effects=2`、flowengine 建 `consol_close` 实例 + `cr_cell_data A1=2` |
| — | **合计** | **158** | — |

---

## 四、前端功能测试（`test/fe/*.cjs`，Playwright CDP）

| # | 套件 | 用例 | 覆盖能力 |
|---:|---|---:|---|
| 1 | `onto_designer` | 10 | **设计工作台四区**：造对象/关系 → 发布 → `<cmx-ontology-graph>` 画布渲染富卡片 + 关系边 |
| 2 | `onto_ui2_props` | 11 | 属性 Inspector / 对象类型概览 / 属性卡内表格 |
| 3 | `onto_ui3_links` | 11 | 关系连线 · 基数 · **属性到属性**锚点连接（FK 属性右 → PK 属性左） |
| 4 | `onto_ui4_safety` | 15 | 画布安全交互：锚点常显 + 拖拽悬停高亮 + **非锚点释放不弹创建关系泡** + 边路由避让 |
| 5 | `onto_ui5_kinetic` | 11 | 动效编辑：拖拽节点、**中段线段拖拽**（水平段上下/垂直段左右）、**非破坏重排** |
| 6 | `onto_explorer` | 7 | **对象浏览器**：对象集构造器（过滤谓词）+ Search-Around 图谱钻取 |
| 7 | `onto_workshop` | 8 | **Workshop 客户360**：选对象 → 属性卡 + 关系区 → 动作执行闭环 → 写回刷新 |
| 8 | `onto_o8_run` | 9 | **O8 运行台**：eval-function / dry-run-action / exec-action + 结果面板 |
| 9 | `onto_explorer_portal` | 4 | **门户集成**：登录 `/portal` → `openNode(onto-explorer)` → 四区渲染（经 OntoProxy 反代） |
| 10 | `onto_portal_menu` | 7 | **门户菜单**：onto 三工作台（designer/explorer/workshop）入口真机可达 |
| 11 | `onto_flow_sideeffect` | 13 | **动作「起流程」副作用可视化配置**：设计台动作 Inspector 富配置块（flowDefKey 选择器 + businessKey + 参数→变量映射）；选择器由 onto `/flow/definitions` 代理填充（12 个已发布流程）；编辑保存 → API 校验落库 `sideEffects` 正确（无 `_vars` 泄漏）；kind 下拉切换出富块 |
| 12 | `onto_sideeffect_webhook_notif` | 13 | **动作「Webhook / 通知」副作用可视化配置**：富块——Webhook（URL + 请求体字段映射）、通知（模板 + 通知数据字段）渲染/回填；编辑加字段 → 保存 → API 校验落库 `sideEffects`（内联键正确、无 `_vars` 泄漏）；新增副作用即富块、kind 切换 |
| 13 | `onto_sideeffect_report` | 12 | **动作「生成报表」副作用可视化配置**：富块——reportCode 选择器（onto `/report/definitions` 代理填充 213 张报表）+ 报表参数映射（orgCode/periodCode/version）渲染/回填；编辑加参数 → 保存 → API 校验落库；kind 切换出富块 |
| 14 | `onto_action_template` | 8 | **从模板新建动作**：设计台「+ 从模板」→ 选内置 `consolClose`（关账联动）+ 填 apiName → 建动作 → 编辑器自动呈现两条副作用富块（`consol_close` + `STAT_01_D`）；API 校验含 startBusinessProcess + computeReport |
| — | **合计** | **139** | — |

> 全部 `rc=0`，无 `FAIL`，无浏览器 `console.error`。

---

## 五、能力覆盖矩阵（O0–O8 + P0–P2 + dispatcher）

| 里程碑 | 能力 | 覆盖测试 |
|---|---|---|
| O0 | 元模型 `om_*` / 物化 `oo_*` / 统一边 `ol_edge` | 单测 + 全量 E2E 间接 |
| O1 | 对象集代数（Base/Filter/SearchAround/Union…） | `o5_functions`(objectSet) · `onto_explorer` |
| O2 | 对象浏览 / Search-Around | `onto_explorer` · `onto_workshop` |
| O3 | 数据集成 Funnel + 隔离区 | `o3_funnel` |
| O4 | 动作引擎（执行/校验/PEP/乐观锁/Outbox/dispatcher） | `o4_action_engine` `o4m2_validations` `o4_pep_optlock` `o4m3_outbox` `o4m3_dispatcher` `o4m3_dispatch_delivery` · `onto_o8_run` `onto_workshop` |
| O5 | 函数计算引擎 | `o5_functions` · `onto_o8_run` |
| O6 | 动态安全（残差 Filter + 脱敏） | `o6_dynamic_security` |
| O7 | headless（v1/OpenAPI/SSE/API Key） | `o7_headless` |
| O8 | 运行台 / 对象浏览器 / Workshop | `onto_o8_run` `onto_explorer` `onto_workshop` |
| P0 | O3 数据集成 + O4 写侧 | `o3_funnel` `o4_pep_optlock` |
| P1 | 对象浏览器 + dispatcher + DOC/DCT 导入 | `onto_explorer(_portal)` `o4m3_*` `p1_doc_dct_import` |
| P2 | OSDK + Workshop 客户360 | `p2_osdk` `onto_workshop` |
| — | dispatcher 真投递（webhook/flow v1） | `o4m3_dispatch_delivery` |
| — | onto↔flowengine 跨服务真联调（真建实例） | `o4m3_flow_integration` |
| — | onto↔cmx-report 跨服务真联调（真算落库） | `o4m3_report_integration` |
| — | 关账联动模板（flow+report 双联动 + 从模板新建） | `o4m3_template_consol_close` · `onto_action_template` |
| 前端图组件 | 锚点/边路由/线段编辑/非破坏重排/门户接入 | `onto_ui2/ui3/ui4/ui5` `onto_designer` `onto_portal_menu` |

---

## 六、已知缺口 / 未覆盖（非本次回归失败，属未实现或范围外）

1. **Rhai / Wasm 函数运行时**：`function.rs` 目前仅 FEEL；Rhai/Wasm 返回 `UnsupportedRuntime`，无对应测试。
2. **派生属性物化**：O5 可求派生值，但缺「落库 + 触发重算」通道，未测。
3. **ReBAC / Marking 强制门**：O6 已做残差 Filter + 字段脱敏，尚未做「硬拒绝」访问门，未测。
4. ~~`startBusinessProcess` 对真实 flowengine 的跨服务端到端~~ —— **已完成**（后端 #13 `o4m3_flow_integration` 12/12）：启动真实 `cmx-flow-server`（jwt 模式 + 服务 API Key，DB 同 `cmx_fico`），dispatcher 新增 `ONTO_FLOW_API_KEY`（portal 同款服务身份）+ `businessKey` 透传，onto 真起流程实例，实例 `c5a81213-fb83-4cc7-9998-682618a99757` 在 flowengine 侧确认为 `ACTIVE`、`businessKey`/`variables.orderId` 均来自 onto。
5. **性能 / 并发 / 压测**：本轮为功能与契约测试，未做吞吐/并发/大数据量压测。
6. **`cmx-onto-app` 独立 Rust 单测**：handler 经泛型 `<S>` 设计，行为由 12 套后端 E2E 覆盖，无内联单测。

---

## 七、结论

本体平台 **O0–O8 主线 + P0/P1/P2 + dispatcher 真投递 + onto↔flowengine/cmx-report 跨服务联调 + 动作副作用可视化配置 + 关账联动模板** 全部通过真机验证：

- **单元 54/54 · 后端 158/158 · 前端 139/139 = 351/351，零失败。**
- **跨服务真联调打通（双服务）**：onto 动作 `startBusinessProcess` → 真实 flowengine 建流程实例（`c5a81213-…` `ACTIVE`）；onto 动作 `computeReport` → 真实 cmx-report `compute` 真算落 `cr_cell_data`（`A1=2`）。均经服务 `X-API-Key`（portal 同款）+ jwt 模式。
- **动作副作用可视化配置**：设计台富配置块统一支持 **起流程**（flowDefKey 选已发布流程 + businessKey + 参数→流程变量）、**生成报表**（reportCode 选报表 + 报表参数 orgCode/periodCode/version）、**Webhook**（URL + 请求体字段映射）、**通知**（模板 + 通知数据字段）；四者共用「参数→载荷」键值映射，存前折成内联键、`_vars` 不外泄。选择器分别由 onto `GET /flow/definitions`、`GET /report/definitions` 代理填充（容错降级）。
- **关账联动模板**：内置动作模板注册表（`GET /action-templates`）+ 设计台「从模板新建动作」；旗舰模板 `consolClose` 一个动作串起 **起关账流（flowengine `consol_close`）+ 算关账报表（cmx-report）**——真机验证 UI 从模板建的动作执行后**同时**建流程实例 + 真算落库。附带放宽执行门：仅副作用（无 edit）的动作亦可执行。
- 测试用临时配置（dispatcher sink `:8770`）已回滚；当前留存 onto(:8097, 已配 `ONTO_FLOW_API_KEY`) + flow(:8091) 双服务在线，供继续联调。
- 全部代码/前端**未提交**（按约定）。

> 复现：单测 `cargo test --workspace --lib`；后端 `for t in test/e2e/o*.sh; do bash $t; done`；前端 `NODE_PATH=/Users/nanomesh/node_modules node test/fe/*.cjs`（门户用例需 `:8080` 门户在线）。
> 其中 `o4m3_dispatch_delivery` 需以 `ONTO_FLOW_URL=http://127.0.0.1:8770 ONTO_WEBHOOK_ALLOW=127.0.0.1,localhost` 重启 onto；`o4m3_flow_integration` 需 flow-server 在线（`CONFIG_FILE=flow-server-local.toml`）+ onto 以 `ONTO_FLOW_API_KEY=<flow服务key>` 启动。
