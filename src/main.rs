mod db;
mod detail;
mod index;
mod types;

use clap::Parser;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use types::{CsvApi, Scores};

#[derive(Parser, Debug)]
struct Args {
    /// 只处理指定 dll（大小写不敏感，如 kernel32.dll）
    #[arg(long)]
    dll: Option<String>,
    /// 最多处理 n 个 API（0=不限）
    #[arg(long, default_value_t = 0)]
    limit: usize,
    /// 只跑索引阶段，输出统计与样例，不写库
    #[arg(long)]
    index_only: bool,
    /// 跳过索引构建，从 index_cache.tsv 读取（需先跑过一次索引）
    #[arg(long)]
    skip_index: bool,
    /// 解析详情但不写库，打印摘要
    #[arg(long)]
    dry_run: bool,
    /// 跳过已入库的 API（断点续爬）
    #[arg(long)]
    resume: bool,
    /// 清空重建数据库
    #[arg(long)]
    fresh: bool,
    /// data.csv 路径
    #[arg(long, default_value = "data.csv")]
    csv: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL").expect("未找到DATABASE_URL，请检查 .env");
    let connect_option = sqlx::sqlite::SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(connect_option).await?;

    if args.fresh {
        db::reset(&pool).await?;
        println!("数据库已重建");
    } else {
        db::ensure_tables(&pool).await?;
    }

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 win-api-spider/0.1")
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

    // ---- 阶段1+2：索引（可缓存复用） ----
    let index = if args.skip_index {
        println!("从索引缓存读取（index_cache.tsv）");
        index::read_index_cache()?
    } else {
        let idx = index::build_index(&client, args.index_only).await?;
        println!("索引完成：共 {} 个 API 页面", idx.len());
        if args.index_only {
            return Ok(());
        }
        idx
    };
    let index_map: HashMap<String, &types::IndexedApi> =
        index.iter().map(|a| (a.name.clone(), a)).collect();

    // ---- 读取目标清单 ----
    let mut rdr = csv::Reader::from_path(&args.csv)?;
    let mut csv_apis: Vec<CsvApi> = Vec::new();
    for rec in rdr.deserialize() {
        csv_apis.push(rec?);
    }
    println!("清单：共 {} 个 API", csv_apis.len());

    // ---- dll 过滤 ----
    let filtered: Vec<&CsvApi> = if let Some(dll) = &args.dll {
        let d = dll.to_ascii_lowercase();
        csv_apis
            .iter()
            .filter(|a| a.dll.to_ascii_lowercase() == d)
            .collect()
    } else {
        csv_apis.iter().collect()
    };
    println!("筛选后待处理：{} 个", filtered.len());

    // ---- 阶段3：详情爬取 ----
    let mut processed = 0usize;
    let mut skipped = 0usize;
    let mut missing = 0usize;
    let mut failed = 0usize;

    for api in filtered {
        if args.limit > 0 && processed >= args.limit {
            break;
        }
        if args.resume && db::api_exists(&pool, &api.name).await? {
            skipped += 1;
            continue;
        }
        let Some(entry) = index_map.get(&api.name.to_ascii_lowercase()) else {
            missing += 1;
            println!("[跳过] {}：Learn 索引中未找到", api.name);
            continue;
        };

        let detail = match detail::fetch_and_parse(&client, &entry.url, &entry.summary).await {
            Ok(d) => d,
            Err(e) => {
                failed += 1;
                println!("[失败] {}: {}", api.name, e);
                continue;
            }
        };

        let scores = heuristic_scores(api, &detail);

        if args.dry_run {
            println!("\n[dry-run] {}  <-  {}", api.name, entry.url);
            println!("  简介: {}", &detail.summary[..detail.summary.len().min(80)]);
            println!(
                "  签名: {:?}",
                detail
                    .cpp_signature
                    .as_deref()
                    .map(|s| &s[..s.len().min(80)])
            );
            println!("  参数: {} 个", detail.param_count);
            println!(
                "  参数文本: {:?}",
                detail
                    .params_text
                    .as_deref()
                    .map(|s| &s[..s.len().min(80)])
            );
            println!(
                "  返回值: {:?}",
                detail
                    .return_value
                    .as_deref()
                    .map(|s| &s[..s.len().min(60)])
            );
            println!(
                "  备注: {:?}",
                detail.remarks.as_deref().map(|s| &s[..s.len().min(60)])
            );
            println!("  示例: {} 个", detail.examples.len());
            println!("  see-also: {:?}", detail.see_also);
            println!(
                "  评分: usage={:.1} complexity={:.1} risk={:.1} total={:.1}",
                scores.usage, scores.complexity, scores.risk, scores.total
            );
            processed += 1;
            continue;
        }

        let entries = db::build_doc_entries(api, &detail);
        db::insert_api_with_docs(&pool, api, &detail, &scores, &entries).await?;
        println!("[写入] {}（{} 条文档条目）", api.name, entries.len());
        processed += 1;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    println!(
        "\n完成：处理 {}，跳过(已存在) {}，索引缺失 {}，失败 {}",
        processed, skipped, missing, failed
    );
    Ok(())
}

/// 启发式评分（0-5 分）
fn heuristic_scores(api: &CsvApi, detail: &types::ApiDetail) -> Scores {
    // usage：按模块热度
    let d = api.dll.to_ascii_lowercase();
    let usage = if d.contains("kernel32")
        || d.contains("user32")
        || d.contains("gdi32")
        || d.contains("advapi32")
        || d.contains("shell32")
    {
        5.0
    } else if d.contains("ole") || d.contains("ws2") || d.contains("combase") {
        4.0
    } else {
        3.0
    };

    // complexity：按签名中的 [in]/[out] 参数标记计数（比数段落更准）
    let n = detail
        .cpp_signature
        .as_deref()
        .map(|s| s.matches("[in").count() + s.matches("[out").count())
        .unwrap_or(detail.param_count);
    let complexity = if n >= 10 {
        5.0
    } else if n >= 7 {
        4.0
    } else if n >= 4 {
        3.0
    } else if n >= 1 {
        2.0
    } else {
        1.0
    };

    // risk：句柄/指针返回值、OUT 参数特征
    let sig = format!(
        "{} {}",
        api.signature,
        detail.cpp_signature.as_deref().unwrap_or("")
    );
    let risk = if sig.contains("HANDLE") || sig.contains('*') {
        4.0
    } else if sig.contains("BOOL") {
        2.0
    } else {
        3.0
    };

    let total = (usage * 0.4 + complexity * 0.3 + risk * 0.3) * 10.0;
    Scores {
        usage,
        complexity,
        risk,
        total: total.round() / 10.0,
    }
}
