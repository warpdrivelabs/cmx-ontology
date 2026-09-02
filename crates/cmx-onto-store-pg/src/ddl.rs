//! 本体平台 PG 表结构 DDL（幂等）。
//!
//! 硬约束（对齐 flow/rules store-pg）：`om_` 前缀；禁外键，用索引替代；DDL 幂等（IF NOT EXISTS）。
//! 七表——六类元模型元素各一张定义表 + 一张发布快照表。多租户：per-tenant DB 隔离，故表内不带
//! tenant 列（库即租户边界）。所有定义体以 jsonb 承载（O1 建模态；O2 对象存储另建 oo_*/ol_* 物化表）。

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
        row_filter      JSONB        NOT NULL DEFAULT '[]',
        deny_markings   JSONB        NOT NULL DEFAULT '[]',
        deny_actions    JSONB        NOT NULL DEFAULT '[]',
        status          VARCHAR(16)  NOT NULL DEFAULT 'active',
        created_at      TIMESTAMPTZ  NOT NULL DEFAULT now()
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_om_policy_match ON om_policy (object_type, subject_kind, subject)",
    "ALTER TABLE om_policy ADD COLUMN IF NOT EXISTS deny_actions JSONB NOT NULL DEFAULT '[]'",
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
];
