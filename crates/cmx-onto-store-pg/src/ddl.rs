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
        target_object_types JSONB      NOT NULL DEFAULT '[]',
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
    // —— 存档快照（不可变检查点；version 唯一；rev = 内容指纹去重锚；snapshot = 存档时全量清单+定义）——
    // 列名 archived_by/archived_at（方案 20260917 §6.5 命名清理：存档≠发布；旧库由下方 DO 块判存改名）。
    r#"CREATE TABLE IF NOT EXISTS om_version (
        version         INTEGER      PRIMARY KEY,
        rev             VARCHAR(32)  NOT NULL,
        summary         TEXT         NOT NULL DEFAULT '',
        snapshot        JSONB        NOT NULL,
        archived_by     VARCHAR(128),
        archived_at     TIMESTAMPTZ  NOT NULL,
        tag             VARCHAR(64),
        release_note    TEXT
    )"#,
    // —— 资源级修订历史（方案 20260917 §6.2；七类资源每次保存/流转/恢复同事务追加一条；
    //     deleted 墓碑：资源删除后历史保留可恢复；view 剥离 layout）——
    r#"CREATE TABLE IF NOT EXISTS om_revision (
        id            BIGSERIAL PRIMARY KEY,
        resource_kind VARCHAR(32)  NOT NULL,
        api_name      VARCHAR(128) NOT NULL,
        revision      INTEGER      NOT NULL,
        payload       JSONB        NOT NULL,
        change_note   TEXT,
        changed_by    VARCHAR(255) NOT NULL,
        changed_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
        deleted       BOOLEAN      NOT NULL DEFAULT FALSE,
        UNIQUE (resource_kind, api_name, revision)
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_om_revision_resource ON om_revision (resource_kind, api_name, revision DESC)",
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
    // —— P2-0 动作作用对象类型（保存期派生物化列，语义真源仍是 parameters/logic；幂等补列）——
    // boot 由 backfill_action_targets 回填存量；GIN 索引支撑「按对象类型查动作」的清单过滤。
    "ALTER TABLE om_action_type ADD COLUMN IF NOT EXISTS target_object_types JSONB NOT NULL DEFAULT '[]'",
    "CREATE INDEX IF NOT EXISTS idx_om_action_type_targets ON om_action_type USING GIN (target_object_types)",
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
    // 跨库漏斗：源数据源 db_id（可空=本体库 onto_pg）；读源在该库执行，写 oo_/隔离区仍走本体库
    "ALTER TABLE om_source_mapping ADD COLUMN IF NOT EXISTS source_db_id VARCHAR(64)",
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
    // —— 方案 20260917（P1 状态软治理 / P2 修订历史）：幂等补列（既有库自动迁移）——
    // 七类资源弃用元数据四列（仅 /lifecycle/transition 写入；save 剥离；离开 deprecated 即清空）。
    "ALTER TABLE om_object_type ADD COLUMN IF NOT EXISTS deprecation_reason TEXT",
    "ALTER TABLE om_object_type ADD COLUMN IF NOT EXISTS sunset_at DATE",
    "ALTER TABLE om_object_type ADD COLUMN IF NOT EXISTS replacement_api_name VARCHAR(128)",
    "ALTER TABLE om_object_type ADD COLUMN IF NOT EXISTS deprecated_at TIMESTAMPTZ",
    "ALTER TABLE om_link_type ADD COLUMN IF NOT EXISTS deprecation_reason TEXT",
    "ALTER TABLE om_link_type ADD COLUMN IF NOT EXISTS sunset_at DATE",
    "ALTER TABLE om_link_type ADD COLUMN IF NOT EXISTS replacement_api_name VARCHAR(128)",
    "ALTER TABLE om_link_type ADD COLUMN IF NOT EXISTS deprecated_at TIMESTAMPTZ",
    "ALTER TABLE om_interface ADD COLUMN IF NOT EXISTS deprecation_reason TEXT",
    "ALTER TABLE om_interface ADD COLUMN IF NOT EXISTS sunset_at DATE",
    "ALTER TABLE om_interface ADD COLUMN IF NOT EXISTS replacement_api_name VARCHAR(128)",
    "ALTER TABLE om_interface ADD COLUMN IF NOT EXISTS deprecated_at TIMESTAMPTZ",
    "ALTER TABLE om_shared_property ADD COLUMN IF NOT EXISTS deprecation_reason TEXT",
    "ALTER TABLE om_shared_property ADD COLUMN IF NOT EXISTS sunset_at DATE",
    "ALTER TABLE om_shared_property ADD COLUMN IF NOT EXISTS replacement_api_name VARCHAR(128)",
    "ALTER TABLE om_shared_property ADD COLUMN IF NOT EXISTS deprecated_at TIMESTAMPTZ",
    "ALTER TABLE om_action_type ADD COLUMN IF NOT EXISTS deprecation_reason TEXT",
    "ALTER TABLE om_action_type ADD COLUMN IF NOT EXISTS sunset_at DATE",
    "ALTER TABLE om_action_type ADD COLUMN IF NOT EXISTS replacement_api_name VARCHAR(128)",
    "ALTER TABLE om_action_type ADD COLUMN IF NOT EXISTS deprecated_at TIMESTAMPTZ",
    "ALTER TABLE om_function ADD COLUMN IF NOT EXISTS deprecation_reason TEXT",
    "ALTER TABLE om_function ADD COLUMN IF NOT EXISTS sunset_at DATE",
    "ALTER TABLE om_function ADD COLUMN IF NOT EXISTS replacement_api_name VARCHAR(128)",
    "ALTER TABLE om_function ADD COLUMN IF NOT EXISTS deprecated_at TIMESTAMPTZ",
    "ALTER TABLE om_view ADD COLUMN IF NOT EXISTS deprecation_reason TEXT",
    "ALTER TABLE om_view ADD COLUMN IF NOT EXISTS sunset_at DATE",
    "ALTER TABLE om_view ADD COLUMN IF NOT EXISTS replacement_api_name VARCHAR(128)",
    "ALTER TABLE om_view ADD COLUMN IF NOT EXISTS deprecated_at TIMESTAMPTZ",
    // 状态覆盖七类：shared_property / view 原本无 status 列（其余五类建表自带）。
    "ALTER TABLE om_shared_property ADD COLUMN IF NOT EXISTS status VARCHAR(32) NOT NULL DEFAULT 'experimental'",
    "ALTER TABLE om_view ADD COLUMN IF NOT EXISTS status VARCHAR(32) NOT NULL DEFAULT 'experimental'",
    // 乐观锁补齐（§6.2 顺手清债）：link / interface / shared_property / action / function 对齐 object/view。
    "ALTER TABLE om_link_type ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE om_interface ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE om_shared_property ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE om_action_type ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE om_function ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 0",
    // om_version 旧列名改存档语义（RENAME 不可幂等重放——DO 块判存；索引同步改名）。
    r#"DO $$ BEGIN
        IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'om_version' AND column_name = 'published_by')
           AND NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'om_version' AND column_name = 'archived_by') THEN
            ALTER TABLE om_version RENAME COLUMN published_by TO archived_by;
        END IF; END $$"#,
    r#"DO $$ BEGIN
        IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'om_version' AND column_name = 'published_at')
           AND NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'om_version' AND column_name = 'archived_at') THEN
            ALTER TABLE om_version RENAME COLUMN published_at TO archived_at;
        END IF; END $$"#,
    r#"DO $$ BEGIN
        IF EXISTS (SELECT 1 FROM pg_indexes WHERE indexname = 'idx_om_version_published')
           AND NOT EXISTS (SELECT 1 FROM pg_indexes WHERE indexname = 'idx_om_version_archived') THEN
            ALTER INDEX idx_om_version_published RENAME TO idx_om_version_archived;
        END IF; END $$"#,
    // 存档时间索引（改名后建——存量库 om_version 走 published_at→archived_at 改名，索引同步改名；
    // 新库建表即含 archived_at，此处幂等补建）。
    "CREATE INDEX IF NOT EXISTS idx_om_version_archived ON om_version (archived_at)",
    // 发布标记（方案 §6.4）：tag 唯一（PG 唯一约束允许多行 NULL——匿名检查点不占位）。
    "ALTER TABLE om_version ADD COLUMN IF NOT EXISTS tag VARCHAR(64)",
    "ALTER TABLE om_version ADD COLUMN IF NOT EXISTS release_note TEXT",
    r#"DO $$ BEGIN
        IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'uq_om_version_tag') THEN
            ALTER TABLE om_version ADD CONSTRAINT uq_om_version_tag UNIQUE (tag);
        END IF; END $$"#,
    // （om_draft 草稿工作区表已随直改 live 架构移除——存量库由下方 DDL_CLEANUPS 幂等清理）
    // —— 维护角色白名单（P2 写路径授权；方案 §七——不复用 om_policy（行级 PDP 语义错位））——
    // **空白名单 = 开放**（P1 全员维护等效语义；生产由 DBA 录入行即收敛为白名单模式）。
    r#"CREATE TABLE IF NOT EXISTS om_maintainer (
        subject         VARCHAR(128) PRIMARY KEY,
        subject_kind    VARCHAR(16)  NOT NULL DEFAULT 'user',
        created_at      TIMESTAMPTZ  NOT NULL DEFAULT now()
    )"#,
    // —— 方案 20260918（对象数据源统一抽象）：om_source_mapping 升格 = 绑定唯一权威行（E1）——
    // mode = 读路径来源（materialized=物化灌数 / virtual=虚拟直查下推）；漏斗建的映射恒 materialized，
    // virtual 只能经 POST /object-types/datasource/bind 建立。幂等补列 + 存量回填（DDL DEFAULT 不覆盖
    // 存量行，需显式 UPDATE，B-P2-3）。
    "ALTER TABLE om_source_mapping ADD COLUMN IF NOT EXISTS mode VARCHAR(16) NOT NULL DEFAULT 'materialized'",
    "ALTER TABLE om_source_mapping ADD COLUMN IF NOT EXISTS resource VARCHAR(256)",
    r#"UPDATE om_source_mapping SET mode = 'materialized' WHERE mode IS NULL"#,
    // M1b 数据源注册表（方案 §5.4）：kind ∈ pg|api|connector；config 按 kind 承载连接/端点配置
    // （凭证只存环境变量引用名，绝不落明文）；caps 声明能力矩阵（pg 源由实现内置，可空）。
    r#"CREATE TABLE IF NOT EXISTS om_data_source (
        id            VARCHAR(128) PRIMARY KEY,
        name          VARCHAR(200) NOT NULL,
        kind          VARCHAR(16)  NOT NULL,
        config        JSONB        NOT NULL,
        caps          JSONB,
        status        VARCHAR(16)  NOT NULL DEFAULT 'active',
        last_probe_at TIMESTAMPTZ,
        probe_report  JSONB,
        created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
        updated_at    TIMESTAMPTZ  NOT NULL DEFAULT now()
    )"#,
    // 映射行 → 注册表 id（M1b 起泛化寻址；读取优先 source_id、空则回退 source_db_id，Q6 双读）。
    "ALTER TABLE om_source_mapping ADD COLUMN IF NOT EXISTS source_id VARCHAR(128)",
];

/// 一次性清理（幂等）：已废弃表随启动重放删除（直改 live 架构移除草稿工作区；
/// 稳定后可整段移除）。
pub const DDL_CLEANUPS: &[&str] = &["DROP TABLE IF EXISTS om_draft"];

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
    "COMMENT ON COLUMN om_action_type.target_object_types IS '作用对象类型（P2-0 保存期从 parameters+logic 派生的物化列；语义真源仍是 parameters，GIN 索引支撑按类型查动作）'",
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
    "COMMENT ON TABLE om_version IS '本体存档快照（不可变检查点；每次存档/回滚留痕一行，承载存档时全量清单+定义，可整体回滚）；tag 非空即命名发布标记'",
    "COMMENT ON COLUMN om_version.version IS '存档版本号（递增主键，唯一）'",
    "COMMENT ON COLUMN om_version.rev IS '内容指纹（xxh64 快照指纹；与最新版相同去重不插行）'",
    "COMMENT ON COLUMN om_version.summary IS '存档说明'",
    "COMMENT ON COLUMN om_version.snapshot IS '存档时全量清单+定义 jsonb（views 段剥离 layout）'",
    "COMMENT ON COLUMN om_version.archived_by IS '存档人（可空；原名 published_by，20260917 命名清理：存档≠发布）'",
    "COMMENT ON COLUMN om_version.archived_at IS '存档时间（原名 published_at，同上）'",
    "COMMENT ON COLUMN om_version.tag IS '命名发布标记（如 v1.2；NULL = 匿名检查点；唯一约束允许多行 NULL）'",
    "COMMENT ON COLUMN om_version.release_note IS '发布说明（随 tag 写入；可空）'",
    // —— 资源级修订历史 ——
    "COMMENT ON TABLE om_revision IS '资源级修订历史（七类资源每次保存/流转/恢复同事务追加一条；git revert 式回滚的数据基础；修订从 20260917 上线时刻积累）'",
    "COMMENT ON COLUMN om_revision.id IS '自增主键'",
    "COMMENT ON COLUMN om_revision.resource_kind IS '资源类别：object/link/interface/shared_property/action/function/view'",
    "COMMENT ON COLUMN om_revision.api_name IS '资源 apiName（删除后重建同名的修订号接续墓碑前最大值）'",
    "COMMENT ON COLUMN om_revision.revision IS 'per-resource 递增修订号（事务内 max+1，UNIQUE 兜底并发重试）'",
    "COMMENT ON COLUMN om_revision.payload IS '单资源完整定义 jsonb（view 剥离 layout）'",
    "COMMENT ON COLUMN om_revision.change_note IS '变更说明（保存为空；revert/恢复/流转带语义说明；可空）'",
    "COMMENT ON COLUMN om_revision.changed_by IS '变更人'",
    "COMMENT ON COLUMN om_revision.changed_at IS '变更时间'",
    "COMMENT ON COLUMN om_revision.deleted IS '墓碑：true = 本次修订后资源被删除（历史保留，可 revert 恢复）'",
    // —— 弃用元数据四列（七类同构，注释挂对象类型处详述，余表简注）——
    "COMMENT ON COLUMN om_object_type.deprecation_reason IS '弃用原因（仅 /lifecycle/transition 写入；离开 deprecated 即清空）'",
    "COMMENT ON COLUMN om_object_type.sunset_at IS '预期下线期限（transition 必填项之一）'",
    "COMMENT ON COLUMN om_object_type.replacement_api_name IS '替代资源 apiName（可选，展示用引用不建 FK）'",
    "COMMENT ON COLUMN om_object_type.deprecated_at IS '废弃动作时间（transition 落库时刻）'",
    "COMMENT ON COLUMN om_link_type.deprecation_reason IS '弃用原因（同 om_object_type；级联降级由服务端自动填充）'",
    "COMMENT ON COLUMN om_link_type.sunset_at IS '预期下线期限（级联降级继承触发对象的 sunset_at）'",
    "COMMENT ON COLUMN om_link_type.replacement_api_name IS '替代资源 apiName（可空）'",
    "COMMENT ON COLUMN om_link_type.deprecated_at IS '废弃动作时间'",
    "COMMENT ON COLUMN om_interface.deprecation_reason IS '弃用原因（同 om_object_type）'",
    "COMMENT ON COLUMN om_interface.sunset_at IS '预期下线期限'",
    "COMMENT ON COLUMN om_interface.replacement_api_name IS '替代资源 apiName（可空）'",
    "COMMENT ON COLUMN om_interface.deprecated_at IS '废弃动作时间'",
    "COMMENT ON COLUMN om_shared_property.deprecation_reason IS '弃用原因（同 om_object_type）'",
    "COMMENT ON COLUMN om_shared_property.sunset_at IS '预期下线期限'",
    "COMMENT ON COLUMN om_shared_property.replacement_api_name IS '替代资源 apiName（可空）'",
    "COMMENT ON COLUMN om_shared_property.deprecated_at IS '废弃动作时间'",
    "COMMENT ON COLUMN om_action_type.deprecation_reason IS '弃用原因（同 om_object_type）'",
    "COMMENT ON COLUMN om_action_type.sunset_at IS '预期下线期限'",
    "COMMENT ON COLUMN om_action_type.replacement_api_name IS '替代资源 apiName（可空）'",
    "COMMENT ON COLUMN om_action_type.deprecated_at IS '废弃动作时间'",
    "COMMENT ON COLUMN om_function.deprecation_reason IS '弃用原因（同 om_object_type）'",
    "COMMENT ON COLUMN om_function.sunset_at IS '预期下线期限'",
    "COMMENT ON COLUMN om_function.replacement_api_name IS '替代资源 apiName（可空）'",
    "COMMENT ON COLUMN om_function.deprecated_at IS '废弃动作时间'",
    "COMMENT ON COLUMN om_view.deprecation_reason IS '弃用原因（场景纳入 lifecycle，同 om_object_type）'",
    "COMMENT ON COLUMN om_view.sunset_at IS '预期下线期限'",
    "COMMENT ON COLUMN om_view.replacement_api_name IS '替代场景 apiName（可空）'",
    "COMMENT ON COLUMN om_view.deprecated_at IS '废弃动作时间'",
    "COMMENT ON COLUMN om_shared_property.status IS '生命周期：experimental（默认）/ active / deprecated（20260917 起覆盖七类资源）'",
    "COMMENT ON COLUMN om_view.status IS '生命周期：experimental（默认）/ active / deprecated（场景纳入 lifecycle）'",
    "COMMENT ON COLUMN om_link_type.version IS '乐观锁版本号（每次保存 +1；20260917 补齐五类表）'",
    "COMMENT ON COLUMN om_interface.version IS '乐观锁版本号（每次保存 +1）'",
    "COMMENT ON COLUMN om_shared_property.version IS '乐观锁版本号（每次保存 +1）'",
    "COMMENT ON COLUMN om_action_type.version IS '乐观锁版本号（每次保存 +1）'",
    "COMMENT ON COLUMN om_function.version IS '乐观锁版本号（每次保存 +1）'",
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
    "COMMENT ON TABLE om_source_mapping IS '源→对象映射（方案 20260918 升格 = 绑定唯一权威行：mode=materialized 漏斗灌数 / mode=virtual 虚拟直查下推）'",
    "COMMENT ON COLUMN om_source_mapping.object_type IS '目标对象类型 apiName（主键，一对象一映射）'",
    "COMMENT ON COLUMN om_source_mapping.source_query IS '源查询 SQL（物化全量同步读源用；可空 = 由 resource+映射生成参数化 SELECT；虚拟映射恒空）'",
    "COMMENT ON COLUMN om_source_mapping.key_columns IS '业务键列名 jsonb 数组（源行 upsert 对象的键）'",
    "COMMENT ON COLUMN om_source_mapping.title_column IS '标题列名（映射对象 titleProperty；可空）'",
    "COMMENT ON COLUMN om_source_mapping.property_map IS '源列→属性映射 jsonb 数组'",
    "COMMENT ON COLUMN om_source_mapping.required IS '必填项 jsonb 数组（缺失行进隔离区）'",
    "COMMENT ON COLUMN om_source_mapping.last_sync_at IS '最近全量同步时间（可空 = 未同步过）'",
    "COMMENT ON COLUMN om_source_mapping.last_report IS '最近一次同步报告 jsonb（合格/隔离/失败计数等）'",
    "COMMENT ON COLUMN om_source_mapping.created_at IS '创建时间'",
    "COMMENT ON COLUMN om_source_mapping.mode IS '绑定模式 = 读路径来源（E1 权威值）：materialized 物化 / virtual 虚拟直查'",
    "COMMENT ON COLUMN om_source_mapping.resource IS '源资源名（PG 表名 schema.table 或 API 资源名；生成式查询与下推的取数对象）'",
    "COMMENT ON COLUMN om_source_mapping.source_id IS 'om_data_source.id（M1b 注册表寻址；读取优先于 source_db_id）'",
    "COMMENT ON TABLE om_data_source IS '数据源注册表（M1b）：pg|api|connector；凭证只存环境变量引用名不落明文'",
    "COMMENT ON COLUMN om_data_source.kind IS '数据源类型：pg / api / connector（预留）'",
    "COMMENT ON COLUMN om_data_source.config IS 'kind 相关配置 jsonb（pg: ref 池名或独立连接；api: baseUrl/auth/分页协议；凭证仅环境变量名）'",
    "COMMENT ON COLUMN om_data_source.caps IS '声明的能力矩阵 jsonb（api 源必填；pg 源实现内置可空）'",
    "COMMENT ON COLUMN om_data_source.probe_report IS '最近探测报告 jsonb（连通/版本/列基线——schema 漂移比对依据）'",
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
    // —— 维护角色白名单 ——
    "COMMENT ON TABLE om_maintainer IS '本体维护角色白名单（P2 写路径授权）：空表 = 开放（P1 全员维护等效）；有行 = 仅命中者可写（评审门不可被直连 API 绕过）'",
    "COMMENT ON COLUMN om_maintainer.subject IS '主体标识：用户 id / 用户名（subject_kind=user）或角色名（subject_kind=role）'",
    "COMMENT ON COLUMN om_maintainer.subject_kind IS '主体类型：user 用户（默认）/ role 角色（JWT roles claim 命中即放行）'",
    "COMMENT ON COLUMN om_maintainer.created_at IS '创建时间'",
];
