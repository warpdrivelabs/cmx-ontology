# cmx-ontology · Palantir 式企业本体平台 · 完整实现方案（图文并茂）

> **文档定位**：本文是 `cmx-ontology`（Rust 企业本体平台）**已落地实现**的完整技术方案，图文并茂、随代码校准。
> 与两份设计蓝本互补：[`20260828_…Rust完整建设方案.md`](./20260828_cmx-ontology_Palantir式企业本体平台_Rust完整建设方案.md)（平台设计）、
> [`20260829_…前端本体定义…设计方案.md`](./20260829_cmx-ontology_前端本体定义_专业简洁可视化_设计方案.md)（前端 UX 设计）。本文聚焦「**做成了什么、怎么做的**」。
>
> **一句话**：以 Palantir Foundry Ontology 为蓝本，用 Rust「一芯多壳」建的独立本体微服务（`:8097`）——把企业数据抽象为**对象 / 关系 / 接口 / 动作 / 函数**的语义层，可视化建模、Per-Type 物化存储、对象集代数一条 SQL 查询。
>
> **交付状态（2026-08-30）**：后端 **O0 / O1 / O2** ✅ · 前端 **UI0–UI5** ✅ · 门户菜单集成 ✅ · 20 Rust 单测 + 66 E2E + 11 组件 vitest + 6 CDP 套件全绿。O3–O8 见路线图。

---

## 目录

1. [定位与总体架构](#一定位与总体架构)
2. [本体元模型（平台的心脏）](#二本体元模型平台的心脏)
3. [五大核心组件](#三五大核心组件)
4. [数据持久化 · 双层模型](#四数据持久化--双层模型)
5. [对象集代数与查询编译](#五对象集代数与查询编译)
6. [前端 · 一台三面板双向同步](#六前端--一台三面板双向同步)
7. [Rust 工作区与 crate 结构](#七rust-工作区与-crate-结构)
8. [API 契约（v1）](#八api-契约v1)
9. [多租户 · 安全 · 集成地图](#九多租户--安全--集成地图)
10. [落地路线图 O0–O8](#十落地路线图-o0o8)
11. [测试与质量](#十一测试与质量)
12. [附录：关键实现要点与坑](#十二附录关键实现要点与坑)

---

## 一、定位与总体架构

Palantir Foundry 的核心不是数据库，而是**本体（Ontology）**——一层把杂乱物理数据翻译成业务语义（客户、订单、供应商……名词；下单、改签……动词）的语义模型。`cmx-ontology` 把这套范式用 Rust 重铸为**独立可部署的微服务**，与 `cmx-flowengine`（流程）、`cmx-rulesengine`（规则）同构，是 cmx 元模型家族（metaKind）的**第六元**：`DCT / DOC / FLX / RPT / RULE / ONTOLOGY`。

**「一芯多壳」+ 依赖反转**是骨架的灵魂：把「框架无关的本体语义内核」与「可执行/接入形态的薄壳」彻底分离，外层依赖内层，内层零 IO、可纯单测。

<p align="center"><img alt="cmx-ontology 总体架构：一芯多壳 + 生态复用 + 双层持久化" src="{{IMG:01_architecture}}" width="100%"/></p>

- **芯 Kernel · `cmx-onto-model`**：元模型类型、`OntologyStore` 契约、对象集代数、错误。**零 IO、框架无关**，是平台的心脏，可离线单测。
- **实现 Store · `cmx-onto-store-pg`**：用 `tokio-postgres` 落地 `OntologyStore`——`om_*` 元数据表、`oo_*`/`ol_edge` 对象表、对象集代数编译器。
- **中立核 App · `cmx-onto-app`**：框架无关 handler（泛型 `<S: OntologyStore>`）——元模型 CRUD、对象引擎、多租户、OpenAPI、建模台。
- **壳 Shell · `cmx-onto-server`**：独立 bin，`:8097` boot（本方案主壳）；另有门户内嵌/反代壳 `cmx-onto-api`（`OntoProxyModule`）。

**生态复用（依赖反转注入）**：本体层只做「本体语义」编排，不重复造轮子——校验接 `cmx-rulesengine`、动作副作用接 `cmx-flowengine`、行列/ReBAC 权限接 `cmx-data-auth`、聚合接 `cmx-agg`、图编辑范式借鉴 `@cmx/decision-graph`、启动/多租户/监控/认证走 `cmx-web-chassis`。前端经门户 **F3 反代**到本体壳的 `/api/native-pages` 与 `/api/onto/*`，内嵌或独立部署由 `urls.onto` 决定、前端字节一致。

---

## 二、本体元模型（平台的心脏）

元模型是整个平台的「共同语言」——所有组件、存储、前端、API 都围绕它。共**六类元素 + 版本**，同库同契约（`OntologyStore`），结构化校验在芯层。

<p align="center"><img alt="本体元模型：对象/属性/关系/接口/共享属性/动作/函数 + 版本" src="{{IMG:02_metamodel}}" width="100%"/></p>

| 元素 | 角色 | 关键点 |
| --- | --- | --- |
| **对象类型 ObjectType** | 名词（业务实体） | `apiName` + `displayName` + 属性集 + `status`（experimental/active/deprecated）+ `color`；发布时物化为 `oo_<type>` |
| **属性类型 PropertyType** | 字段（内嵌于对象类型） | `baseType` · `required` · `isPrimaryKey ◇` · `isTitle ⌾` · `isIndexed ⚡` · `semanticType`（语义类型徽标） |
| **关系类型 LinkType** | 连线 | 两端 `object_type_a ↔ object_type_b` + 基数（one/oneToMany/manyToMany）+ 角色（A→B / B→A）；物化为统一边表 `ol_edge` |
| **接口 Interface** | 多态 | `«interface»`；对象类型可 `implements` 多个接口，实现跨类型多态查询 |
| **共享属性 SharedProperty** | 标准化 | 属性可「引用」共享属性，统一语义与类型（如全局 `currencyCode`） |
| **动作类型 ActionType** | 动词（写） | 编辑管线：参数 → 校验 → 事务落库 → Outbox 副作用 → 审计；`dry-run` 试算（O4） |
| **函数类型 FunctionType** | 计算（读） | 派生属性 / 查询函数 / 聚合；FEEL / Rhai 载体，吃对象集出标量或对象集（O5） |
| **版本 Version** | 发布快照 | `POST /publish` 把六类元素**全量冻结**进不可变版本（`om_version.snapshot jsonb`），供回溯/对比 |

芯层提供**结构化校验**（`POST …/validate` 仅校验、不落库）：apiName 合法性、引用完整性（LinkType 两端类型存在、属性引用的共享属性存在）、主键/标题唯一等。`GET /manifest` 出全量清单，供前端一次装载。

---

## 三、五大核心组件

元模型是共同语言，五大组件围绕它分工。绿=已交付（真机 + 单测全绿），虚线=路线图既定。

<p align="center"><img alt="五大核心组件：建模/存储/动作/计算/集成" src="{{IMG:03_five_components}}" width="100%"/></p>

1. **本体建模引擎（语义核心）· ✅ O1**：元模型 CRUD + 结构化校验 + 发布/版本快照 + 建模台。`om_*` 持久化，`GET /manifest` / `POST /publish`。
2. **对象存储与索引引擎 · ✅ O2**：Per-Type 物化表 `oo_<type>` + 统一边表 `ol_edge` + **对象集代数编译器**（Base/Filter/SearchAround 双向/∪∩−/Static）+ 聚合（Count/GroupCount/GroupSum）。
3. **动作引擎（Action Engine）· ○ O4**：动作类型 → 校验（接 rules）→ 事务写回 → Outbox 副作用（接 flow）→ `oe_*` 审计；`dry-run` 试算。
4. **函数与计算引擎 · ○ O5**：派生属性 · 查询函数 · 聚合（FEEL/Rhai；聚合接 `cmx-agg`，如按区域 GMV）。
5. **数据集成与管道（Object Data Funnel）· ○ O3**：SourceConnector（PG 表/数据集）+ Mapping + 全量/增量灌入 + 管道图 + 接异步任务中心，把既有 fico 表物化进 `oo_*`/`ol_edge`。

---

## 四、数据持久化 · 双层模型

持久化分两层，每租户一库（`db-per-tenant`）。**元数据层**是本体定义的单一真相，**对象数据层**由元模型驱动物化。

<p align="center"><img alt="双层持久化：om_ 元数据 7 表 + oo_/ol_edge 对象层" src="{{IMG:04_persistence}}" width="100%"/></p>

**元数据层 · `om_*` · 7 表**（`cmx-onto-store-pg/ddl.rs`，全 `CREATE TABLE IF NOT EXISTS`）：`om_object_type` · `om_link_type` · `om_interface` · `om_shared_property` · `om_action_type` · `om_function` · `om_version`。发布时把六类元素快照进 `om_version.snapshot`。

**对象数据层 · O2 物化**（`object_store.rs`）：

- **`oo_<type>`**：每个对象类型一张物化表（发布时建、幂等补列）。列：`pk`（主键）· `title`（标题，建 `idx_<t>_title` 索引）· `props jsonb`（全部属性，半结构随元模型演进免频繁 DDL）· `created_at`/`updated_at`。事务 upsert（对象 + 其边一并写）。
- **`ol_edge`**：**统一关系边表**，承载所有 LinkType 的实例。列：`link`（关系 apiName）· `a_pk` · `b_pk` · `props jsonb` · `created_at`。双向索引 `idx_ol_edge_fwd (link, a_pk)` / `idx_ol_edge_rev (link, b_pk)` —— Search-Around 双向遍历即**单表 JOIN**，无需 N 张连接表。

> **设计取舍**：`props JSONB` 让本体演进免频繁 DDL；只有标题/索引这类高频过滤列走真列。统一边表让「关系遍历」从「按关系建表」退化为「单表按 `link` 过滤 + 双向索引 JOIN」，是三跳一条 SQL 的物理基础。

---

## 五、对象集代数与查询编译

Palantir 的 Object-Set 是本体查询的精华：把「集合运算 + 关系遍历」抽象成可组合的代数，再编译成一条 SQL。`cmx-onto-model/objectset.rs` 定义代数，`cmx-onto-store-pg/compile.rs` 编译。

<p align="center"><img alt="对象集代数编译为一条参数化 SQL：三跳 Search-Around" src="{{IMG:05_objectset_sql}}" width="100%"/></p>

**代数算子**（每个都递归产出「产出 `pk` 的子查询」）：

- `Base { object_type }` → `SELECT pk FROM oo_<type>`
- `Static { object_type, primary_keys }` → `SELECT pk FROM (VALUES (\$1),(\$2)…) AS v(pk)`（空集特判）
- `Filter { source, predicate }` → `SELECT o.pk FROM oo_<type> o WHERE o.pk IN (<inner>) AND (<pred>)`；谓词 `Eq/Ne/Gt/Ge/Lt/Le/In/Contains/IsNull/And/Or/Not`，属性经 `props ->> 'name'` 抽文本比较
- `SearchAround { source, link, direction }` → 经 `ol_edge` 遍历：**Forward** `SELECT DISTINCT e.b_pk AS pk FROM ol_edge e WHERE e.link=\$n AND e.a_pk IN (<inner>)`（终端=B 端类型）；**Reverse** 对称（`b_pk`⇄`a_pk`，终端=A 端）
- `Union ∪ / Intersect ∩ / Subtract −` → `(<l>) UNION|INTERSECT|EXCEPT (<r>)`（两侧对象类型须一致）

**编译结果**：整棵代数树自内向外**嵌套**成一条参数化 SQL，外层再从终端 `oo_<terminal>` 取 `pk, title, props`。三跳 Search-Around = 三层 `ol_edge` 嵌套 JOIN，**一次往返一条 SQL**。全程 `$n` 参数化绑定杜绝注入；数值比较 `::text::numeric` 兜底。

**聚合**（Count / GroupCount / GroupSum）与代数正交：在 `pk` 集合外再包一层聚合 SQL 即可。**动态安全**（O6）将把 `cmx-data-auth` 的行/列残差约束合并进 `Filter` 谓词——同一查询、换租户/角色返回不同行/脱敏列。

---

## 六、前端 · 一台三面板双向同步

前端遵循「图为先 + 强类型 Inspector」：一台工作台三面板，以单一内存真相 `OntologySpec` 双向同步，宿主是 native 四区工作台 `portal.onto.designer`。

<p align="center"><img alt="前端一台三面板：图 ⇄ 规格 ⇄ 后端 双向同步" src="{{IMG:06_frontend_sync}}" width="100%"/></p>

- **explorer · 类型浏览器**：六类元素分组树 + 查找 + 计数徽标；点选同步高亮画布/属性。
- **content · 本体图 `<cmx-ontology-graph>`**：独立零框架 Web Component（clean-room，借鉴不改造 `@cmx/decision-graph`）。富卡片（顶部色条 + displayName + apiName + 属性行 + 状态描边）、拖拽布局、右缘小圆拉线建关系、自关联。
- **property · 强类型 Inspector**：对象/关系/动作/函数 Tab；属性表格支持拖排、语义类型、引用共享属性、主键/标题/必填/索引开关。

**组件契约（薄壳/命令式）**：数据入 `setSpec(def)` / 属性 `data-spec`；逃生舱 `getSpec()` / `getModel()`；命令 `addObjectType` / `addInterface` / `addLink` / `delNode` / `delLink` / `autoLayout` / `selectNode` / `refresh` / `validate`；事件出（bubbles+composed）`spec-change` / `type-select` / `edge-select` / `link-add` / `link-added` / `node-add` / `node-del` / `connect-rejected`。领域强类型编辑不在组件内，交宿主 Inspector。

**双向同步**：`图 ⇄ OntologySpec ⇄ 后端`。任一面板编辑写回 Spec → 画布即时重画；`OntologySpec` 与后端 `/api/onto/v1` 粒度化保存（object-types / link-types / …）与 `POST /publish` / `GET /manifest`。

**落地里程碑 UI0–UI5（CDP 全绿）**：UI2 属性编辑深化（拖排/语义/引用共享属性）· UI3 画布直接操作（拉线速建气泡/内联速建/自关联）· UI4 演进安全（破坏性变更专业对话框 + 影响面）· UI5 动能层（动作/函数编辑器，FEEL 内联/四段）。

**构建/同步纪律**（`@cmx/ontology-graph`，离线工具链）：改组件源 `src/{element,render,layout,model,interaction}` 后须 `./build.sh`（tsc 类型检查 + esbuild 打单文件 ESM）→ `./sync-component.sh`（拷进本体壳 `vendor/cmx-ontology-graph.js`），否则本体平台用旧组件。`designer.js` 从磁盘实时投递。

> **本次交付的两处画布修复（2026-08-30）**：① 画布上下左右 **100% 拉伸**（去掉 `height:560px` 固定高，host `:host{width:100%;height:100%}` + 元素 `height:100%`，配合 content 区 `flex:1` 容器链）；② 卡片**左上/右上角闭合**——顶部色条 `rx=3` 方角溢出圆角卡片 `rx=10`，改为 `<clipPath rx=10>` 裁剪色条至卡片圆角，观感专业。

---

## 七、Rust 工作区与 crate 结构

独立 workspace（`cmx-ontology/`），四 crate 一芯多壳，依赖反转（外壳 → 中立核 → 实现 → 芯）。约 4000 行 Rust。

| crate | 角色 | 关键模块 |
| --- | --- | --- |
| `cmx-onto-model` | **芯·内核**（零 IO、框架无关） | `def`（元模型类型）· `store`（`OntologyStore` 契约）· `objectset`（对象集代数）· `object_store` · `error` |
| `cmx-onto-store-pg` | **实现**（tokio-postgres 落地契约） | `ddl`（om_ 建表）· `store`（元模型 CRUD）· `object_store`（oo_/ol_edge）· `compile`（代数→SQL）· `link_resolver` |
| `cmx-onto-app` | **中立核**（框架无关 handler，泛型 `<S>`） | `handlers` · `object_handlers` · `engine` · `object_engine` · `tenancy`/`tenant`（多租户）· `openapi` · `dashboard` · `stats` · `auth` · `resp` |
| `cmx-onto-server` | **壳**（独立 bin `:8097`） | `main`（chassis 装配 + `flow_routes` 式路由挂载 + native 页投递）|

前端组件独立包 `@cmx/ontology-graph`（`cmx-ontology-graph/`，TS + tsc + esbuild），产物同步进 `cmx-container/assets/onto/web/ui-native/vendor/`。

---

## 八、API 契约（v1）

REST v1，前缀 `/api/onto/v1`（旧前缀 `/api/onto` 内嵌壳兼容）。信封 `ApiResp`，门户 `/portal` 应用自动拆封。

```
GET/POST     /object-types             列表 / upsert（结构校验）
POST         /object-types/validate    仅校验（不落库）
GET/DELETE   /object-types/{apiName}    详情 / 删除
GET/POST     /link-types               GET/DELETE /link-types/{apiName}
GET/POST     /interfaces               GET/DELETE /interfaces/{apiName}
GET/POST     /shared-properties        DELETE     /shared-properties/{apiName}
GET/POST     /action-types             GET/DELETE /action-types/{apiName}
GET/POST     /functions                GET/DELETE /functions/{apiName}
GET          /manifest                 本体全量清单（前端一次装载）
POST         /publish                  发布快照（body: {summary}）
GET          /versions                 版本列表
GET          /versions/{version}       某版本快照
GET          /stats                    建模台数据源（各类型计数）
```

对象存储与对象集查询端点（O2）在 `object_handlers` 提供 upsert / 按对象集查询 / 聚合；OpenAPI/Swagger 由 `openapi` 模块暴露。

---

## 九、多租户 · 安全 · 集成地图

- **多租户**：`db-per-tenant`，每租户库各自持有 `om_*` + `oo_*` + `ol_edge`；租户上下文经 `task_local` 传递，首次访问懒建表（对齐 flow/rule 的 `<tenant>` 库范式）。
- **认证**：走 `cmx-web-chassis` / engine-kit；即便 `off` 模式，门户转发的 `X-API-Key` 也须在本体壳 `[auth].api_keys` 白名单内，否则被拒 401。
- **动态安全（O6，规划）**：接 `cmx-data-auth`——对象/行/列 + ReBAC + Marking；对象集编译时把残差约束合并进 `Filter`。
- **门户集成**：反代壳 `cmx-onto-api`（`OntoProxyModule`）转发 `/api/onto/*` + `with_onto_page_proxy` 反代 `portal.onto.*` native 页；`[center_client.services].onto = { url = "http://127.0.0.1:8097" }`。菜单落 DAM `cmx_menu`（模块 `basic/dataplatform/onto` + 菜单 `onto-designer` 四区工作区节点），真机 `/portal` 端到端 7/7 绿。

---

## 十、落地路线图 O0–O8

每个里程碑遵循既有纪律：真机 boot + curl E2E + Rust 单测 + CDP 前端 + 零回归门。

<p align="center"><img alt="落地路线图 O0-O8 交付状态" src="{{IMG:07_roadmap}}" width="100%"/></p>

**已交付**：O0 骨架 · O1 建模引擎 · O2 对象存储（后端）+ UI0–UI5（前端）+ 门户菜单集成。
**下一步**：优先 **O3 数据集成**（让本体「有数据」）或 **O4 动作引擎**（让本体「能改数据」）；随后 O5 函数计算、O6 动态安全、O7 API/SDK（OSDK 代码生成）、O8 前端完善 + 端到端案例（客户 360 / 供应链）。

---

## 十一、测试与质量

| 层 | 覆盖 | 规模 |
| --- | --- | --- |
| 芯/实现 Rust 单测 | 元模型校验、对象集编译、DDL | **20** |
| 后端 E2E（curl） | O1 建模 41 + O2 对象存储/查询 25 | **66** |
| 组件 vitest | `@cmx/ontology-graph` layout/model | **11** |
| 前端 CDP（playwright） | designer · portal_menu · UI2/UI3/UI4/UI5 | **6 套件** |

零回归门：对齐 flow 159/159、rules 223 断言范式——每里程碑真机全绿方视为交付。

---

## 十二、附录：关键实现要点与坑

**后端**：

- **共享 target-dir**：本 ws 复用共享 `target`；`.cargo/config.toml` 须定义 `[patch]`，外部 crate 走 aliyun 镜像、版本与 `cmx-container` 根对齐。
- **可空 jsonb 必须 `NullTyped`**：可空 `jsonb` 列写 `None` 时不能裸 `Null` 序列化（500），一律 `NullTyped`（同 flow 空 `dimensions` 教训）。
- **serde `tag` 不 cascade 字段**：枚举内部 tag 反序列化不自动透传字段，须显式。
- **文本参数比 numeric**：`props ->> 'x'` 出文本，与数值比较须 `::text::numeric` 兜底。
- **`cmx-onto-model` 双址 package collision**：多处引用须同源，否则类型不通。

**前端**：

- **vendor 双产物纪律**：改 `@cmx/ontology-graph` 源必须 `build.sh` + `sync-component.sh`，否则平台用旧组件；`designer.js` 实时读盘。
- **native 页在 `/portal` 非根 `/`**：组件门户（DAM/native-pages）在 `:8080/portal`，根 `/` 是另一 Vue 壳；CDP 测试须打 `/portal`。`/portal` 应用改写 `fetch` 自动拆信封（断言写 `Array.isArray(r)?r:r.data`）。
- **主题令牌**：`--og-*` 锚 `--sap*`（UI5 在 `:root` 重定义、穿透 shadow），裸 hex 仅兜底、不随主题翻转。
- **画布 100% 拉伸 + 卡片圆角**（本次）：host 须 `width/height:100%` 且容器链 `flex:1` 提供确定高度；卡片顶部色条须 `clipPath` 裁到卡片圆角矩形，避免方角溢出。

---

<sub>本文档由实现代码校准生成；图为内嵌 base64 SVG（`<img>` 标签），源与装配脚本见 `docs/assets/`（`svg/*.svg` + `assemble.cjs`）。生成于 2026-08-30。</sub>
