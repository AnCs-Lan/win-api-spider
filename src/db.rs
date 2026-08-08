use crate::types::{ApiDetail, CsvApi, DocEntry, Scores};
use sqlx::SqlitePool;

/// 与 win-api-search/schema.sql 契约一致的建表语句
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS api (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL UNIQUE,
    module     TEXT    NOT NULL,
    category   TEXT,
    signature  TEXT,
    usage      REAL    DEFAULT 0,
    complexity REAL    DEFAULT 0,
    risk       REAL    DEFAULT 0,
    total      REAL    DEFAULT 0,
    related    TEXT
);

CREATE TABLE IF NOT EXISTS doc (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    api_id    INTEGER NOT NULL REFERENCES api(id),
    title     TEXT,
    content   TEXT    NOT NULL,
    tags      TEXT,
    source    TEXT,
    time      TEXT,
    up_vote   INTEGER DEFAULT 1,
    down_vote INTEGER DEFAULT 1,
    pro       REAL    DEFAULT 0,
    exp       REAL    DEFAULT 0,
    frd       REAL    DEFAULT 0,
    total     REAL    DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_doc_api    ON doc(api_id);
CREATE INDEX IF NOT EXISTS idx_api_module ON api(module);
CREATE INDEX IF NOT EXISTS idx_api_name   ON api(name);
"#;

/// 建表（幂等）
pub async fn ensure_tables(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// 清空重建
pub async fn reset(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql("DROP TABLE IF EXISTS doc; DROP TABLE IF EXISTS api;")
        .execute(pool)
        .await?;
    ensure_tables(pool).await
}

/// 断点判断：该 API 是否已入库
pub async fn api_exists(pool: &SqlitePool, name: &str) -> Result<bool, sqlx::Error> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await?;
    Ok(row.0 > 0)
}

/// 事务写入一个 API 及其文档条目
pub async fn insert_api_with_docs(
    pool: &SqlitePool,
    api: &CsvApi,
    detail: &ApiDetail,
    scores: &Scores,
    doc_entries: &[DocEntry],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let related = if detail.see_also.is_empty() {
        None
    } else {
        Some(detail.see_also.join(","))
    };

    let (usage, complexity, risk, total) =
        (scores.usage, scores.complexity, scores.risk, scores.total);

    let api_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO api (name, module, category, signature, usage, complexity, risk, total, related)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(name) DO UPDATE SET
            module=excluded.module,
            signature=excluded.signature,
            usage=excluded.usage,
            complexity=excluded.complexity,
            risk=excluded.risk,
            total=excluded.total,
            related=excluded.related
        RETURNING id
        "#,
    )
    .bind(&api.name)
    .bind(&api.dll)
    .bind(category_of(&api.dll))
    .bind(&api.signature)
    .bind(usage)
    .bind(complexity)
    .bind(risk)
    .bind(total)
    .bind(&related)
    .fetch_one(&mut *tx)
    .await?;

    for entry in doc_entries {
        sqlx::query(
            r#"
            INSERT INTO doc (api_id, title, content, tags, source, time, pro, exp, frd, total)
            VALUES (?, ?, ?, ?, ?, ?, 4.0, 4.0, 4.0, 4.0)
            "#,
        )
        .bind(api_id)
        .bind(&entry.title)
        .bind(&entry.content)
        .bind(&entry.tags)
        .bind("Microsoft Learn")
        .bind(detail.updated.as_deref().unwrap_or(""))
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await
}

/// 按 dll 映射分类（大小写不敏感）
fn category_of(dll: &str) -> &'static str {
    let d = dll.to_ascii_lowercase();
    if d.contains("user32") || d.contains("gdi32") {
        "窗口/图形"
    } else if d.contains("advapi32") || d.contains("wincrypt") {
        "注册表/安全"
    } else if d.contains("ws2") || d.contains("winsock") || d.contains("mswsock") {
        "网络"
    } else if d.contains("kernel32") || d.contains("kernelbase") {
        "内核/IO"
    } else if d.contains("shell32") || d.contains("shlwapi") {
        "Shell"
    } else if d.contains("ole") || d.contains("combase") {
        "COM"
    } else if d.contains("d3d") || d.contains("d2d") || d.contains("dwrite") {
        "图形/媒体"
    } else {
        "其他"
    }
}

/// 由 CsvApi 生成 doc 条目（说明来自清单描述或模块页简介）
pub fn build_doc_entries(api: &CsvApi, detail: &ApiDetail) -> Vec<DocEntry> {
    let mut entries = Vec::new();

    let summary_text = if !api.description.is_empty() && api.description != "待补充" {
        api.description.clone()
    } else if !detail.summary.is_empty() {
        detail.summary.clone()
    } else {
        String::new()
    };
    if !summary_text.is_empty() {
        entries.push(DocEntry {
            title: "函数说明".into(),
            content: summary_text,
            tags: "说明".into(),
        });
    }

    if let Some(sig) = &detail.cpp_signature {
        entries.push(DocEntry {
            title: "函数签名".into(),
            content: sig.clone(),
            tags: "签名".into(),
        });
    }

    if let Some(params) = &detail.params_text {
        entries.push(DocEntry {
            title: "参数说明".into(),
            content: params.clone(),
            tags: "参数".into(),
        });
    }

    if let Some(rv) = &detail.return_value {
        entries.push(DocEntry {
            title: "返回值".into(),
            content: rv.clone(),
            tags: "返回值".into(),
        });
    }

    if let Some(rm) = &detail.remarks {
        entries.push(DocEntry {
            title: "备注".into(),
            content: rm.clone(),
            tags: "备注".into(),
        });
    }

    for (i, ex) in detail.examples.iter().enumerate() {
        entries.push(DocEntry {
            title: if detail.examples.len() == 1 {
                "示例".into()
            } else {
                format!("示例{}", i + 1)
            },
            content: ex.clone(),
            tags: "示例".into(),
        });
    }

    entries
}
