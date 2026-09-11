//! 本体平台 PG 表结构 DDL（幂等）。
//!
//! 硬约束（对齐 flow/rules store-pg）：`om_` 前缀；禁外键，用索引替代；DDL 幂等（IF NOT EXISTS）。
//! 七表——六类元模型元素各一张定义表 + 一张发布快照表。多租户：per-tenant DB 隔离，故表内不带
//! tenant 列（库即租户边界）。所有定义体以 jsonb 承载（O1 建模态；O2 对象存储另建 oo_*/ol_* 物化表）。
//!
//! `DDL_STATEMENTS` = 结构（建表/索引/补列）；`DDL_COMMENTS` = 表/列注释（COMMENT ON 幂等覆盖，
//! 每次启动随结构一起重放，改文案不碰结构）。字段语义与 `cmx-onto-model` 的 def.rs 对齐。

/// 建表 DDL（幂等）。按顺序执行。
pub const DDL_STATEMENTS: &[&str] = &[
    // —— 对象类型（properties/implements/datasource/cmxOrigin 均 jsonb 承载）——
    r#"CREATE TABLE IF NOT EXISTS om_object_type (
        api_name        VARCHAR(128) PRIMARY KEY,
        display_name    VARCHAR(256) NOT NULL DEFAULT '',
        description     TEXT         NOT NULL DEFAULT '',
        icon            VARCHAR(128) NOT NULL DEFAULT '',
        color           VARCHAR(32)  NOT NULL DEFAULT '',
        primary_key     VARCHAR(128) NOT NULL DEFAULT '',
        title_property  VARCHAR(128) NOT NULL DEFAULT '',
        status          VARCHAR(32)  NOT NULL DEFAULT 'experimental',
        properties      JSONB        NOT NULL DEFAULT '[]',
        implements      JSONB        NOT NULL DEFAULT '[]',
        datasource      JSONB,
        cmx_origin      JSONB,
        version         INTEGER      NOT NULL DEFAULT 0,
        created_at      TIMESTAMPTZ  NOT NULL,
        updated_at      TIMESTAMPTZ  NOT NULL
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_om_object_type_status ON om_object_type (status)",
    // —— 关系类型 ——
    r#"CREATE TABLE IF NOT EXISTS om_link_type (
        api_name        VARCHAR(128) PRIMARY KEY,
        display_name    VARCHAR(256) NOT NULL DEFAULT '',
        cardinality     VARCHAR(32)  NOT NULL DEFAULT 'oneToMany',
        object_type_a   VARCHAR(128) NOT NULL DEFAULT '',
        object_type_b   VARCHAR(128) NOT NULL DEFAULT '',
        role_a          VARCHAR(128) NOT NULL DEFAULT '',
        role_b          VARCHAR(128) NOT NULL DEFAULT '',
        backing         JSONB        NOT NULL DEFAULT '{}',
        status          VARCHAR(32)  NOT NULL DEFAULT 'experimental',
        created_at      TIMESTAMPTZ  NOT NULL,
        updated_at      TIMESTAMPTZ  NOT NULL
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_om_link_type_a ON om_link_type (object_type_a)",
    "CREATE INDEX IF NOT EXISTS idx_om_link_type_b ON om_link_type (object_type_b)",
    // —— 接口 ——
    r#"CREATE TABLE IF NOT EXISTS om_interface (
        api_name        VARCHAR(128) PRIMARY KEY,
        display_name    VARCHAR(256) NOT NULL DEFAULT '',
        properties      JSONB        NOT NULL DEFAULT '[]',
        extends         JSONB        NOT NULL DEFAULT '[]',
        status          VARCHAR(32)  NOT NULL DEFAULT 'experimental',
        created_at      TIMESTAMPTZ  NOT NULL,
        updated_at      TIMESTAMPTZ  NOT NULL
    )"#,
    // —— 共享属性类型 ——
    r#"CREATE TABLE IF NOT EXISTS om_shared_property (
        api_name        VARCHAR(128) PRIMARY KEY,
        display_name    VARCHAR(256) NOT NULL DEFAULT '',
        base_type       VARCHAR(32)  NOT NULL DEFAULT 'string',
        semantic_type   VARCHAR(64),
        description     TEXT         NOT NULL DEFAULT '',
        created_at      TIMESTAMPTZ  NOT NULL,
        updated_at      TIMESTAMPTZ  NOT NULL
    )"#,
    // —— 动作类型（parameters/logic/validations/sideEffects 均 jsonb）——
    r#"CREATE TABLE IF NOT EXISTS om_action_type (
        api_name          VARCHAR(128) PRIMARY KEY,
        display_name      VARCHAR(256) NOT NULL DEFAULT '',
        description       TEXT         NOT NULL DEFAULT '',
        parameters        JSONB        NOT NULL DEFAULT '[]',
        logic             JSONB        NOT NULL DEFAULT '[]',
        validations       JSONB        NOT NULL DEFAULT '[]',
        side_effects      JSONB        NOT NULL DEFAULT '[]',
        function_backing  VARCHAR(128),
        status            VARCHAR(32)  NOT NULL DEFAULT 'experimental',
        created_at        TIMESTAMPTZ  NOT NULL,
        updated_at        TIMESTAMPTZ  NOT NULL
    )"#,
    // —— 函数 ——
    r#"CREATE TABLE IF NOT EXISTS om_function (
        api_name        VARCHAR(128) PRIMARY KEY,
        display_name    VARCHAR(256) NOT NULL DEFAULT '',
        runtime         VARCHAR(32)  NOT NULL DEFAULT 'feel',
        kind            VARCHAR(32)  NOT NULL DEFAULT 'query',
        inputs          JSONB        NOT NULL DEFAULT '[]',
        output          JSONB        NOT NULL DEFAULT '{}',
        body            TEXT         NOT NULL DEFAULT '',
        description     TEXT         NOT NULL DEFAULT '',
        status          VARCHAR(32)  NOT NULL DEFAULT 'experimental',
        created_at      TIMESTAMPTZ  NOT NULL,
        updated_at      TIMESTAMPTZ  NOT NULL
    )"#,
    // —— 发布快照（不可变；version 唯一；rev = 内容哈希；snapshot = 发布时全量清单+定义）——
    r#"CREATE TABLE IF NOT EXISTS om_version (
        version         INTEGER      PRIMARY KEY,
        rev             VARCHAR(32)  NOT NULL,
        summary         TEXT         NOT NULL DEFAULT '',
        snapshot        JSONB        NOT NULL,
        published_by    VARCHAR(128),
        published_at    TIMESTAMPTZ  NOT NULL
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_om_version_published ON om_version (published_at)",
    // —— O4 动作执行审计（每次动作执行落一行；含参数/编辑/结果/dry-run）——
    r#"CREATE TABLE IF NOT EXISTS oe_action_log (
        id              BIGSERIAL    PRIMARY KEY,
        action          VARCHAR(128) NOT NULL,
        params          JSONB        NOT NULL DEFAULT '{}',
        edits           JSONB        NOT NULL DEFAULT '[]',
        edit_count      INTEGER      NOT NULL DEFAULT 0,
        dry_run         BOOLEAN      NOT NULL DEFAULT FALSE,
        status          VARCHAR(16)  NOT NULL,
        error           TEXT,
        actor           VARCHAR(128),
        executed_at     TIMESTAMPTZ  NOT NULL
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_oe_action_log_action ON oe_action_log (action, executed_at)",
    // —— O4-M3 副作用事务性 Outbox（与编辑同事务写入；下游 dispatcher 抽取投递）——
    r#"CREATE TABLE IF NOT EXISTS oe_outbox (
        id              BIGSERIAL    PRIMARY KEY,
        action          VARCHAR(128) NOT NULL,
        log_id          BIGINT,
        kind            VARCHAR(32)  NOT NULL,
        target          VARCHAR(512) NOT NULL,
        payload         JSONB        NOT NULL DEFAULT '{}',
        status          VARCHAR(16)  NOT NULL DEFAULT 'pending',
        attempts        INTEGER      NOT NULL DEFAULT 0,
        last_error      TEXT,
        created_at      TIMESTAMPTZ  NOT NULL,
        dispatched_at   TIMESTAMPTZ
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_oe_outbox_pending ON oe_outbox (status, id)",
    // —— O6 动态安全策略（行级残差 + 列级 marking 授予；按 subject 匹配）——
    r#"CREATE TABLE IF NOT EXISTS om_policy (
        api_name        VARCHAR(128) PRIMARY KEY,
        display_name    VARCHAR(200) NOT NULL DEFAULT '',
        object_type     VARCHAR(128),
        subject_kind    VARCHAR(16)  NOT NULL DEFAULT 'role',
        subject         VARCHAR(128) NOT NULL,
        effect          VARCHAR(16)  NOT NULL DEFAULT 'allow',
        row_filter      JSONB        NOT NULL DEFAULT '[]',
        deny_markings   JSONB        NOT NULL DEFAULT '[]',
        deny_actions    JSONB        NOT NULL DEFAULT '[]',
        status          VARCHAR(16)  NOT NULL DEFAULT 'active',
        created_at      TIMESTAMPTZ  NOT NULL DEFAULT now()
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_om_policy_match ON om_policy (object_type, subject_kind, subject)",
    "ALTER TABLE om_policy ADD COLUMN IF NOT EXISTS deny_actions JSONB NOT NULL DEFAULT '[]'",
    // 读侧硬门（#3）：effect=deny 语义（deny_read → 403）；幂等补列，既有库自动迁移。
    "ALTER TABLE om_policy ADD COLUMN IF NOT EXISTS effect VARCHAR(16) NOT NULL DEFAULT 'allow'",
    // 对象类型 DAM 三级分类（域/应用/模块）——本体图分域折叠；幂等补列。
    "ALTER TABLE om_object_type ADD COLUMN IF NOT EXISTS dam JSONB NOT NULL DEFAULT '{}'",
    // 对象类型 业务单据类型（对象浏览器在模块下再分一层）；幂等补列。
    "ALTER TABLE om_object_type ADD COLUMN IF NOT EXISTS doc_type JSONB NOT NULL DEFAULT '{}'",
    // —— O3 数据集成：源→对象映射（持久化，可复跑同步）——
    r#"CREATE TABLE IF NOT EXISTS om_source_mapping (
        object_type     VARCHAR(128) PRIMARY KEY,
        source_query    TEXT         NOT NULL,
        key_columns     JSONB        NOT NULL DEFAULT '[]',
        title_column    VARCHAR(128),
        property_map    JSONB        NOT NULL DEFAULT '[]',
        required        JSONB        NOT NULL DEFAULT '[]',
        last_sync_at    TIMESTAMPTZ,
        last_report     JSONB,
        created_at      TIMESTAMPTZ  NOT NULL DEFAULT now()
    )"#,
    // —— O3 隔离区：Funnel 校验不通过的源行（不污染主对象库）——
    r#"CREATE TABLE IF NOT EXISTS oo_quarantine (
        id              BIGSERIAL    PRIMARY KEY,
        object_type     VARCHAR(128) NOT NULL,
        raw             JSONB        NOT NULL,
        violations      JSONB        NOT NULL,
        source          VARCHAR(64)  NOT NULL DEFAULT 'funnel',
        created_at      TIMESTAMPTZ  NOT NULL DEFAULT now()
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_oo_quarantine_type ON oo_quarantine (object_type, id)",
    // —— 场景视图（本体工作室 P1；方案 §2.1/§七）——
    // auto 视图成员读时按 DAM 现算（members 恒空数组，仅元数据+布局可落行）；manual 物化成员。
    r#"CREATE TABLE IF NOT EXISTS om_view (
        api_name        VARCHAR(128) PRIMARY KEY,
        display_name    VARCHAR(256) NOT NULL DEFAULT '',
        description     TEXT         NOT NULL DEFAULT '',
        dam             JSONB        NOT NULL DEFAULT '{}',
        members         JSONB        NOT NULL DEFAULT '{"objects":[],"interfaces":[]}',
        source          VARCHAR(16)  NOT NULL DEFAULT 'manual',
        layout          JSONB        NOT NULL DEFAULT '{}',
        version         INTEGER      NOT NULL DEFAULT 0,
        created_at      TIMESTAMPTZ  NOT NULL,
        updated_at      TIMESTAMPTZ  NOT NULL
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_om_view_source ON om_view (source)",
    // —— 草稿工作区（本体工作室 P2，方案 §2.4/§七）——
    // 单工作区恒一行（id=1）；content = 七类元素 + deletions + views 段；base_rev = fork/最近
    // 同步时的 live 快照指纹（发布比对当前 live 指纹，防覆盖；不一致 409 → rebase）。
    r#"CREATE TABLE IF NOT EXISTS om_draft (
        id              INTEGER      PRIMARY KEY,
        content         JSONB        NOT NULL,
        base_rev        VARCHAR(64)  NOT NULL DEFAULT '',
        version         INTEGER      NOT NULL DEFAULT 0,
        updated_by      VARCHAR(128),
        updated_at      TIMESTAMPTZ  NOT NULL
    )"#,
    // —— 维护角色白名单（P2 写路径授权；方案 §七——不复用 om_policy（行级 PDP 语义错位））——
    // **空白名单 = 开放**（P1 全员维护等效语义；生产由 DBA 录入行即收敛为白名单模式）。
    r#"CREATE TABLE IF NOT EXISTS om_maintainer (
        subject         VARCHAR(128) PRIMARY KEY,
        subject_kind    VARCHAR(16)  NOT NULL DEFAULT 'user',
        created_at      TIMESTAMPTZ  NOT NULL DEFAULT now()
    )"#,
];

/// 表 / 列注释（COMMENT ON 幂等覆盖）。随 `DDL_STATEMENTS` 一起在启动钩子重放。
pub const DDL_COMMENTS: &[&str] = &[
    // —— 对象类型 ——
    "COMMENT ON TABLE om_object_type IS '对象类型定义（本体的「名词」）：真实世界实体的 schema，属性/实现/数据源均以 jsonb 承载'",
    "COMMENT ON COLUMN om_object_type.api_name IS '稳定 API 名（跨版本不变的唯一锚，如 Customer）'",
    "COMMENT ON COLUMN om_object_type.display_name IS '显示名'",
    "COMMENT ON COLUMN om_object_type.description IS '描述'",
    "COMMENT ON COLUMN om_object_type.icon IS '图标名（图谱/卡片渲染用）'",
    "COMMENT ON COLUMN om_object_type.color IS '图谱着色'",
    "COMMENT ON COLUMN om_object_type.primary_key IS '主键属性 apiName（须在 properties 中）'",
    "COMMENT ON COLUMN om_object_type.title_property IS '展示标题属性 apiName（对象卡片用它当「名字」）'",
    "COMMENT ON COLUMN om_object_type.status IS '生命周期：experimental 试验（默认）/ active 激活 / deprecated 废弃'",
    "COMMENT ON COLUMN om_object_type.properties IS '属性定义数组 jsonb（apiName/displayName/baseType/required/isIndexed/semanticType/sharedProperty/marking/constraints/description）'",
    "COMMENT ON COLUMN om_object_type.implements IS '实现的接口 apiName 数组（多态）'",
    "COMMENT ON COLUMN om_object_type.datasource IS '背书数据源原始 jsonb（O3 Funnel 从哪里灌数；可空）'",
    "COMMENT ON COLUMN om_object_type.cmx_origin IS '由 cmx-model DOC/DCT 生成时的来源回指 jsonb（可空）'",
    "COMMENT ON COLUMN om_object_type.version IS '乐观锁版本号（每次保存 +1，发布比对用）'",
    "COMMENT ON COLUMN om_object_type.created_at IS '创建时间'",
    "COMMENT ON COLUMN om_object_type.updated_at IS '最近更新时间'",
    "COMMENT ON COLUMN om_object_type.dam IS 'DAM 三级分类 jsonb（domain/application/module；本体图分域折叠；幂等补列）'",
    "COMMENT ON COLUMN om_object_type.doc_type IS '业务单据类型 jsonb（code/name；DOC 导入回填，对象浏览器在模块下再分一层；幂等补列）'",
    // —— 关系类型 ——
    "COMMENT ON TABLE om_link_type IS '关系类型定义（对象类型间的关系；Search-Around 的路径）'",
    "COMMENT ON COLUMN om_link_type.api_name IS '稳定 API 名（唯一锚）'",
    "COMMENT ON COLUMN om_link_type.display_name IS '显示名'",
    "COMMENT ON COLUMN om_link_type.cardinality IS '关系基数：oneToOne / oneToMany（默认）/ manyToOne / manyToMany'",
    "COMMENT ON COLUMN om_link_type.object_type_a IS 'A 端对象类型 apiName'",
    "COMMENT ON COLUMN om_link_type.object_type_b IS 'B 端对象类型 apiName'",
    "COMMENT ON COLUMN om_link_type.role_a IS 'A→B 方向角色名（如 places）'",
    "COMMENT ON COLUMN om_link_type.role_b IS 'B→A 方向角色名（如 placedBy）'",
    "COMMENT ON COLUMN om_link_type.backing IS '关系落存储方式 jsonb（ForeignKey/JoinTable/Intermediary；O2 对象存储消费）'",
    "COMMENT ON COLUMN om_link_type.status IS '生命周期：experimental / active / deprecated'",
    "COMMENT ON COLUMN om_link_type.created_at IS '创建时间'",
    "COMMENT ON COLUMN om_link_type.updated_at IS '最近更新时间'",
    // —— 接口 ——
    "COMMENT ON TABLE om_interface IS '接口定义（对象类型的形状契约，提供多态）'",
    "COMMENT ON COLUMN om_interface.api_name IS '稳定 API 名（唯一锚）'",
    "COMMENT ON COLUMN om_interface.display_name IS '显示名'",
    "COMMENT ON COLUMN om_interface.properties IS '要求实现者具备的属性定义数组 jsonb'",
    "COMMENT ON COLUMN om_interface.extends IS '继承的父接口 apiName 数组'",
    "COMMENT ON COLUMN om_interface.status IS '生命周期：experimental / active / deprecated'",
    "COMMENT ON COLUMN om_interface.created_at IS '创建时间'",
    "COMMENT ON COLUMN om_interface.updated_at IS '最近更新时间'",
    // —— 共享属性类型 ——
    "COMMENT ON TABLE om_shared_property IS '共享属性类型（全局标准属性，一处定义处处引用）'",
    "COMMENT ON COLUMN om_shared_property.api_name IS '稳定 API 名（唯一锚）'",
    "COMMENT ON COLUMN om_shared_property.display_name IS '显示名'",
    "COMMENT ON COLUMN om_shared_property.base_type IS '基础类型：string（默认）/integer/long/double/decimal/boolean/date/timestamp/array/struct/attachment/mediaReference/marking/geohash/geoShape/vector'",
    "COMMENT ON COLUMN om_shared_property.semantic_type IS '语义类型（复用 cmx-meta-data semanticType：金额/百分比/邮箱…；可空）'",
    "COMMENT ON COLUMN om_shared_property.description IS '描述'",
    "COMMENT ON COLUMN om_shared_property.created_at IS '创建时间'",
    "COMMENT ON COLUMN om_shared_property.updated_at IS '最近更新时间'",
    // —— 动作类型 ——
    "COMMENT ON TABLE om_action_type IS '动作类型定义（一组受治理的编辑+校验+副作用；本体的「动词」；O1 建模，执行引擎见 O4）'",
    "COMMENT ON COLUMN om_action_type.api_name IS '稳定 API 名（唯一锚）'",
    "COMMENT ON COLUMN om_action_type.display_name IS '显示名'",
    "COMMENT ON COLUMN om_action_type.description IS '描述'",
    "COMMENT ON COLUMN om_action_type.parameters IS '表单参数 jsonb 数组（可绑对象/对象集/标量）'",
    "COMMENT ON COLUMN om_action_type.logic IS '编辑规则 jsonb 数组（Create/Modify/Delete Object、Add/Remove Link）'",
    "COMMENT ON COLUMN om_action_type.validations IS '提交校验 jsonb 数组（O4 落规则引擎 FEEL）'",
    "COMMENT ON COLUMN om_action_type.side_effects IS '副作用 jsonb 数组（通知/webhook/函数/流程/事件）'",
    "COMMENT ON COLUMN om_action_type.function_backing IS '函数背书：复杂逻辑走函数（om_function.api_name；可空）'",
    "COMMENT ON COLUMN om_action_type.status IS '生命周期：experimental / active / deprecated'",
    "COMMENT ON COLUMN om_action_type.created_at IS '创建时间'",
    "COMMENT ON COLUMN om_action_type.updated_at IS '最近更新时间'",
    // —— 函数 ——
    "COMMENT ON TABLE om_function IS '函数定义（原生吃对象/对象集的计算逻辑；O1 建模，执行引擎见 O5）'",
    "COMMENT ON COLUMN om_function.api_name IS '稳定 API 名（唯一锚）'",
    "COMMENT ON COLUMN om_function.display_name IS '显示名'",
    "COMMENT ON COLUMN om_function.runtime IS '运行时：feel（默认）/ rhai / wasm / nativeRust'",
    "COMMENT ON COLUMN om_function.kind IS '用途：query / derivedProperty / validation / actionLogic / aggregation'",
    "COMMENT ON COLUMN om_function.inputs IS '输入参数 jsonb（可吃对象/对象集/标量）'",
    "COMMENT ON COLUMN om_function.output IS '返回类型 jsonb'",
    "COMMENT ON COLUMN om_function.body IS '函数体（源码或引用）'",
    "COMMENT ON COLUMN om_function.description IS '描述'",
    "COMMENT ON COLUMN om_function.status IS '生命周期：experimental / active / deprecated'",
    "COMMENT ON COLUMN om_function.created_at IS '创建时间'",
    "COMMENT ON COLUMN om_function.updated_at IS '最近更新时间'",
    // —— 发布快照 ——
    "COMMENT ON TABLE om_version IS '发布快照（不可变；每次发布一行，承载发布时全量清单+定义）'",
    "COMMENT ON COLUMN om_version.version IS '版本号（递增主键，唯一）'",
    "COMMENT ON COLUMN om_version.rev IS '内容哈希（快照指纹，比对两版间是否真有变更）'",
    "COMMENT ON COLUMN om_version.summary IS '发布说明'",
    "COMMENT ON COLUMN om_version.snapshot IS '发布时全量清单+定义 jsonb'",
    "COMMENT ON COLUMN om_version.published_by IS '发布人（可空）'",
    "COMMENT ON COLUMN om_version.published_at IS '发布时间'",
    // —— O4 动作执行审计 ——
    "COMMENT ON TABLE oe_action_log IS 'O4 动作执行审计（每次动作执行落一行，含 dry-run；任一步失败即回滚并落 failed）'",
    "COMMENT ON COLUMN oe_action_log.id IS '自增主键（Outbox 经 log_id 回指）'",
    "COMMENT ON COLUMN oe_action_log.action IS '动作类型 apiName'",
    "COMMENT ON COLUMN oe_action_log.params IS '入参 jsonb'",
    "COMMENT ON COLUMN oe_action_log.edits IS '编辑明细 jsonb 数组（Create/Modify/Delete Object、Add/Remove Link）'",
    "COMMENT ON COLUMN oe_action_log.edit_count IS '编辑条数'",
    "COMMENT ON COLUMN oe_action_log.dry_run IS '是否试跑（true = 校验+预演但不落库）'",
    "COMMENT ON COLUMN oe_action_log.status IS '执行状态：committed 已提交 / failed 失败回滚 / dryRun 试跑'",
    "COMMENT ON COLUMN oe_action_log.error IS '失败原因（成功为空）'",
    "COMMENT ON COLUMN oe_action_log.actor IS '执行人（可空）'",
    "COMMENT ON COLUMN oe_action_log.executed_at IS '执行时间'",
    // —— O4-M3 副作用 Outbox ——
    "COMMENT ON TABLE oe_outbox IS 'O4-M3 副作用事务性 Outbox（与编辑同事务写入，提交后由 dispatcher 抽取投递）'",
    "COMMENT ON COLUMN oe_outbox.id IS '自增主键（领取按 id ASC + FOR UPDATE SKIP LOCKED）'",
    "COMMENT ON COLUMN oe_outbox.action IS '来源动作类型 apiName'",
    "COMMENT ON COLUMN oe_outbox.log_id IS '关联审计行 oe_action_log.id（可空）'",
    "COMMENT ON COLUMN oe_outbox.kind IS '副作用类型（通知/webhook/函数/流程/事件）'",
    "COMMENT ON COLUMN oe_outbox.target IS '投递目标（地址/端点标识）'",
    "COMMENT ON COLUMN oe_outbox.payload IS '投递负载 jsonb'",
    "COMMENT ON COLUMN oe_outbox.status IS '投递状态：pending 待投递 / processing 已领取 / dispatched 已投递 / failed 失败'",
    "COMMENT ON COLUMN oe_outbox.attempts IS '投递尝试次数'",
    "COMMENT ON COLUMN oe_outbox.last_error IS '最近一次失败原因'",
    "COMMENT ON COLUMN oe_outbox.created_at IS '入箱时间'",
    "COMMENT ON COLUMN oe_outbox.dispatched_at IS '投递终态回标时间（可空）'",
    // —— O6 动态安全策略 ——
    "COMMENT ON TABLE om_policy IS 'O6 动态安全策略（行级残差约束 + 列级 marking 脱敏 + 动作拒绝；按主体匹配，决策/执行解耦的执行侧）'",
    "COMMENT ON COLUMN om_policy.api_name IS '稳定 API 名（唯一锚）'",
    "COMMENT ON COLUMN om_policy.display_name IS '显示名'",
    "COMMENT ON COLUMN om_policy.object_type IS '适用对象类型 apiName；空 = 全局策略（通配全部对象类型）'",
    "COMMENT ON COLUMN om_policy.subject_kind IS '主体类型：role 角色（默认）/ user 用户'",
    "COMMENT ON COLUMN om_policy.subject IS '主体标识（角色名或用户名）'",
    "COMMENT ON COLUMN om_policy.row_filter IS '行级残差谓词 jsonb 数组（合并进对象集查询 Filter）'",
    "COMMENT ON COLUMN om_policy.deny_markings IS '拒绝的列级 marking 列表 jsonb（命中列值脱敏为 ***）'",
    "COMMENT ON COLUMN om_policy.deny_actions IS '拒绝执行的动作 apiName 列表 jsonb（写侧 PEP）'",
    "COMMENT ON COLUMN om_policy.status IS '策略状态：active 生效（默认）'",
    "COMMENT ON COLUMN om_policy.created_at IS '创建时间'",
    // —— O3 数据集成：源→对象映射 ——
    "COMMENT ON TABLE om_source_mapping IS 'O3 数据集成：源→对象映射（持久化映射定义，支持复跑全量同步）'",
    "COMMENT ON COLUMN om_source_mapping.object_type IS '目标对象类型 apiName（主键，一对象一映射）'",
    "COMMENT ON COLUMN om_source_mapping.source_query IS '源查询 SQL（全量同步时执行读源行）'",
    "COMMENT ON COLUMN om_source_mapping.key_columns IS '业务键列名 jsonb 数组（源行 upsert 对象的键）'",
    "COMMENT ON COLUMN om_source_mapping.title_column IS '标题列名（映射对象 titleProperty；可空）'",
    "COMMENT ON COLUMN om_source_mapping.property_map IS '源列→属性映射 jsonb 数组'",
    "COMMENT ON COLUMN om_source_mapping.required IS '必填项 jsonb 数组（缺失行进隔离区）'",
    "COMMENT ON COLUMN om_source_mapping.last_sync_at IS '最近全量同步时间（可空 = 未同步过）'",
    "COMMENT ON COLUMN om_source_mapping.last_report IS '最近一次同步报告 jsonb（合格/隔离/失败计数等）'",
    "COMMENT ON COLUMN om_source_mapping.created_at IS '创建时间'",
    // —— O3 隔离区 ——
    "COMMENT ON TABLE oo_quarantine IS 'O3 隔离区：Funnel 校验不通过的源行（不污染主对象库，供修复后重放）'",
    "COMMENT ON COLUMN oo_quarantine.id IS '自增主键'",
    "COMMENT ON COLUMN oo_quarantine.object_type IS '归属对象类型 apiName'",
    "COMMENT ON COLUMN oo_quarantine.raw IS '原始源行 jsonb'",
    "COMMENT ON COLUMN oo_quarantine.violations IS '校验违规明细 jsonb（缺失必填/类型不符等）'",
    "COMMENT ON COLUMN oo_quarantine.source IS '来源标识（默认 funnel）'",
    "COMMENT ON COLUMN oo_quarantine.created_at IS '入区时间'",
    // —— 场景视图 ——
    "COMMENT ON TABLE om_view IS '场景视图（本体工作室）：场景是过滤器/透镜不是容器——只存成员引用与布局，元素定义仍在 om_* 全局唯一'",
    "COMMENT ON COLUMN om_view.api_name IS '视图稳定名（manual 由维护者命名；auto 域默认视图固定 auto:<domain> 前缀）'",
    "COMMENT ON COLUMN om_view.display_name IS '显示名'",
    "COMMENT ON COLUMN om_view.description IS '描述'",
    "COMMENT ON COLUMN om_view.dam IS '视图自身 DAM 归属 jsonb（展示分组用，可空）'",
    "COMMENT ON COLUMN om_view.members IS '成员引用 jsonb（{objects:[],interfaces:[]}；仅 manual 物化，auto 恒空——成员读时按 DAM 现算）'",
    "COMMENT ON COLUMN om_view.source IS '视图来源：auto（域默认视图，读时派生）/ manual（手动场景，快照语义）'",
    "COMMENT ON COLUMN om_view.layout IS '画布布局 jsonb（组件 _layout 形状；LWW 直写不占乐观锁，发布/保存互不覆盖）'",
    "COMMENT ON COLUMN om_view.version IS '乐观锁版本号（每次保存 +1；layout 单列更新不递增）'",
    "COMMENT ON COLUMN om_view.created_at IS '创建时间'",
    "COMMENT ON COLUMN om_view.updated_at IS '最近更新时间'",
    // —— 草稿工作区 ——
    "COMMENT ON TABLE om_draft IS '草稿工作区（本体工作室 P2 双轨）：单工作区一行，编辑写草稿、发布原子应用 om_* 并打版本快照'",
    "COMMENT ON COLUMN om_draft.id IS '恒 1（单工作区；按域多草稿列二期）'",
    "COMMENT ON COLUMN om_draft.content IS '草稿内容 jsonb（六类元素全量定义 + deletions 显式删除清单 + views 场景视图段）'",
    "COMMENT ON COLUMN om_draft.base_rev IS '基线 live 快照指纹（fork 时 xxh64；发布比对当前 live 指纹，不一致 409 → rebase）'",
    "COMMENT ON COLUMN om_draft.version IS '行级乐观锁（他人保存过 → 409；与 base_rev 409 是两个错误源，客户端提示可区分）'",
    "COMMENT ON COLUMN om_draft.updated_by IS '最近编辑人'",
    "COMMENT ON COLUMN om_draft.updated_at IS '最近编辑时间（发布对话框绑发标注用）'",
    // —— 维护角色白名单 ——
    "COMMENT ON TABLE om_maintainer IS '本体维护角色白名单（P2 写路径授权）：空表 = 开放（P1 全员维护等效）；有行 = 仅命中者可写（评审门不可被直连 API 绕过）'",
    "COMMENT ON COLUMN om_maintainer.subject IS '主体标识：用户 id / 用户名（subject_kind=user）或角色名（subject_kind=role）'",
    "COMMENT ON COLUMN om_maintainer.subject_kind IS '主体类型：user 用户（默认）/ role 角色（JWT roles claim 命中即放行）'",
    "COMMENT ON COLUMN om_maintainer.created_at IS '创建时间'",
];
