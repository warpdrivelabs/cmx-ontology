# cmx-flow / cmx-report 连通性测试报告

- **日期**：2026-09-01
- **被测连接**：`cmx-ontology`（onto）动作引擎 dispatcher → `cmx-flowengine`（flow）+ `cmx-report`（report）两大微服务
- **测试范围**：服务可达 · onto 配置装配 · 鉴权姿态 · 读代理连通 · 触发（写）连通 · 错误传播 · 出站熄火 · SSRF 护栏 · 前端选择器连通
- **结论**：**连通性全部通过（52 断言 + 熄火场景，0 失败）**；一处安全**发现项**：cmx-report 当前未接鉴权中间件（详见 §6）

---

## 一、环境与拓扑

| 服务 | 地址 | 配置 | 鉴权 | DB |
|---|---|---|---|---|
| cmx-onto-server | `127.0.0.1:8097` | `onto-server-dev.toml` + `ONTO_FLOW_API_KEY`/`ONTO_REPORT_API_KEY` | jwt+APIKey | `cmx_fico`@192.168.157.46 |
| cmx-flow-server | `127.0.0.1:8091` | `flow-server-local.toml` | **jwt + 服务 APIKey** | `cmx_fico`@192.168.157.46 |
| cmx-rpt-server | `127.0.0.1:8092` | `report-server-local.toml` | **jwt + 服务 APIKey（本次补齐鉴权门）** | `cmx_fico`@192.168.157.46 |

连接方式：onto dispatcher 经 `reqwest`（rustls）以服务 `X-API-Key`（portal 同款服务身份）+ `X-Tenant` 调用两服务的 v1 契约。**跨微服务只经 HTTP**，onto 不 path-dep flow/report。

```
                    ┌──── startBusinessProcess ──→ POST /api/flow/v1/instances ──→ cmx-flow  :8091
onto 动作 dispatcher ┤──── computeReport ─────────→ POST /api/report-design/reports/{code}/compute → cmx-report :8092
                    └──── (webhook / notification / callFunction / emitEvent 略)
读代理：onto GET /flow/definitions  →  flow GET /definitions
        onto GET /report/definitions → report GET /report-design/reports
```

---

## 二、连通性矩阵

| 维度 | cmx-flow（:8091） | cmx-report（:8092） | 用例 |
|---|:--:|:--:|---|
| 服务可达（带 key→200） | ✅ | ✅ | conn F2 / R2 |
| onto 配置装配（URL + apiKeySet） | ✅ | ✅ | conn F1 / R1 |
| 鉴权强制（no-key） | ✅ **401** | ✅ **401（本次补齐鉴权门）** | conn F3 / R3 |
| 读代理连通（onto 代理列举） | ✅ 定义列表 | ✅ **213 张报表** | conn F4 / R4 |
| 触发（写）连通 | ✅ **真建流程实例** | ✅ **真算落 `cr_cell_data`** | o4m3_flow / o4m3_report |
| 错误传播（坏目标→failed + 服务消息） | ✅ | ✅ | conn F5/F5b · R5/R5b |
| 出站熄火（`ONTO_OUTBOUND=off`→deferred） | ✅ | ✅ | kill-switch 场景 |
| SSRF 护栏（webhook 外部 host→拦） | ✅（旁证） | — | o4m3_dispatcher |
| 模板双联动（一动作串两服务） | ✅ | ✅ | o4m3_template_consol_close |
| 前端选择器连通（设计台拉服务数据） | ✅ 12 定义 | ✅ 213 报表 | onto_flow_sideeffect / onto_sideeffect_report（CDP） |

---

## 三、测试套件结果

| 套件 | 断言 | 结果 | 覆盖 |
|---|---:|:--:|---|
| `conn_flow_report.sh` | 12 | ✅ 12/12 | 配置·可达·鉴权·读代理·错误传播（双服务） |
| `o4m3_flow_integration.sh` | 12 | ✅ 12/12 | onto→flow 真建流程实例（businessKey 定位 + 变量溯源 + 停 approve） |
| `o4m3_report_integration.sh` | 9 | ✅ 9/9 | onto→report 真算落 `cr_cell_data`（A1=2） |
| `o4m3_template_consol_close.sh` | 10 | ✅ 10/10 | 关账联动模板：一动作**同时**建流程实例 + 真算报表 |
| `o4m3_dispatcher.sh` | 9 | ✅ 9/9 | 进程内投递（emitEvent/callFunction）+ webhook SSRF 拦 failed |
| 熄火场景（手动） | 1 | ✅ | `ONTO_OUTBOUND=off` → 两副作用 `deferred:2` |
| **合计** | **53** | ✅ **53/53** | — |

---

## 四、关键证据

**触发连通（写路径）**
```
# flow：onto 动作 startBusinessProcess → 真建实例
onto 日志:   startBusinessProcess 已投递 target=onto_int_approve instance=c5a81213-…
flow /instances: {id:"c5a81213-…", businessKey:"onto-int-…", definitionKey:"onto_int_approve", state:"ACTIVE"}

# report：onto 动作 computeReport → 真算落库
onto 日志:   computeReport 已投递 report=STAT_01_D
cr_cell_data: STAT_01_D | CSCEC | 2025 | A1 | 2.000000

# 模板双联动：一个 tmplClose 动作
dispatch: {"dispatched":2,"deferred":0,"failed":0}
→ flowengine 建 consol_close 实例(businessKey=2025) + cr_cell_data A1=2
```

**鉴权姿态**
```
flow  no-key → HTTP 401 ；bad-key → HTTP 401     （jwt 强制，服务 APIKey 命中即 200 tenant=default）
report no-key → HTTP 401 ；bad-key → HTTP 401     （本次补齐鉴权门；svc-key→200；开放路由 / 与 /api/rpt/stats 仍免认证）
```

**错误传播（连接的健壮性——服务业务错误原样回传 onto outbox）**
```
startBusinessProcess [__no_such_flow__]: 起流程返回 code=1 流程定义未部署: __no_such_flow__
computeReport        [__NO_RPT__]      : 生成报表返回 code=1 报表无版本，无法计算
→ 二者 dispatch 均 failed=1（非网络/鉴权错，是服务侧业务错 → 证明 HTTP+鉴权链路已通、错误如实穿透）
```

**出站熄火**
```
ONTO_OUTBOUND=off → config.outboundEnabled=false
dispatch(startBusinessProcess + computeReport) → {"dispatched":0,"deferred":2,"failed":0}
恢复后 outboundEnabled=true
```

---

## 五、连通性判定说明

- **读通**：onto 的 `/flow/definitions`、`/report/definitions` 代理**真实**穿透到对端并归一化返回（flow 定义列表、report 213 张）——证明「onto→服务」GET 链路 + 服务鉴权（flow）/直通（report）均通。
- **写通**：onto 动作副作用经 dispatcher **真实**触达对端并产生副作用（flow 建实例、report 落 `cr_cell_data`）——证明「onto→服务」POST 链路 + 参数透传（org/period/businessKey/变量插值）均正确。
- **错通**：坏目标使对端返回业务错误，onto 如实落 outbox `failed` 并回传对端消息——证明链路端到端贯通且错误可观测（非静默吞没）。
- **控通**：`ONTO_OUTBOUND=off` 熄火使两类外部投递退化为 `deferred`——出站有全局闸。

---

## 六、发现项与修复

> **【发现项 · 中 → 已修复】cmx-report 未接入鉴权中间件。**
> 初测发现 `cmx-rpt-server/src/main.rs` 路由仅挂 `cmx_web_monitor::observe`，**未挂 auth 中间件**（flow / onto 均挂 `auth_middleware`），故 `report-server-*.toml` 的 `[auth]` 段实际不生效，report 对**无 key 请求也返回 200**。
>
> **本次已修复（对齐 flow / onto 同款鉴权门）：**
> 1. 新增 `cmx-rpt-app::auth`（收编唯一真源 `cmx-engine-kit::auth::jwt`，`JwtSpec::new("report", &[], None)`——report 无 SSE 票据）+ 导出 `auth_middleware`；`cmx-rpt-app` 增 `cmx-engine-kit` 依赖。
> 2. `cmx-rpt-server` 路由分组：**业务 API**（`report_routes` + `consol_routes`）挂 `auth_middleware`（内层 observe 采身份）；**免认证**保留根大盘 `/`、`/api/rpt/stats`（大盘轮询）、前端页只读投递（门户 F3 反代 / 独立自投递）。
> 3. `report-server-dev.toml` 补 `[auth]`（jwt + 服务 APIKey）并修「`default = true` 被注释半行」的启动缺陷。
>
> **复测**：report no-key / bad-key → **401**，服务 key → **200**；`/` 与 `/api/rpt/stats` 仍 **200**；onto→report 连通**不受影响**（`o4m3_report_integration` 9/9、`o4m3_template_consol_close` 10/10、`conn_flow_report` 12/12 复测通过）。

其余维度（flow 鉴权、错误传播、熄火、SSRF、前端选择器）均**符合预期**，无异常。

---

## 七、结论

onto ↔ cmx-flow、onto ↔ cmx-report 两条跨服务连接**全面连通、健壮可控**：读通 / 写通 / 错通 / 控通四态齐备，**53/53 断言通过**。关账联动模板证明双服务可由**单一业务动作**同时驱动。初测发现的 cmx-report 鉴权中间件缺失已**当场补齐**（§6，报表 API 现 jwt + 服务 APIKey 强制，`no-key→401`），复测双服务连通与业务功能均不受影响。

> 复现：`bash test/e2e/conn_flow_report.sh`；`bash test/e2e/o4m3_{flow,report}_integration.sh`；`bash test/e2e/o4m3_template_consol_close.sh`；`bash test/e2e/o4m3_dispatcher.sh`。前置：onto:8097（`ONTO_FLOW_API_KEY`+`ONTO_REPORT_API_KEY`）+ flow:8091（`flow-server-local.toml`）+ report:8092（`report-server-local.toml`）在线。
