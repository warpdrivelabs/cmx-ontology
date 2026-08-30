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
];
