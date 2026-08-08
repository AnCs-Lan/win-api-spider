use crate::index::fetch_with_retry;
use crate::types::ApiDetail;
use reqwest::Client;
use scraper::{ElementRef, Html, Selector};

/// 区块停止标记：遇到这些 id 的标题元素即停止收集
const STOPS: &[&str] = &[
    "syntax", "parameters", "return-value", "remarks", "examples", "requirements", "see-also",
];

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 判断兄弟元素是否为区块停止点（下一个区块标题）
fn is_stop(sib: &ElementRef) -> bool {
    sib.value()
        .id()
        .map(|s| STOPS.contains(&s))
        .unwrap_or(false)
}

/// 取 id 元素的后续兄弟文本，遇到下一个区块标题即停（Learn 页面区块为兄弟节点）
fn section_text(doc: &Html, id: &str) -> Option<String> {
    let sel = Selector::parse(&format!("#{}", id)).ok()?;
    let el = doc.select(&sel).next()?;
    let mut out = String::new();
    let mut cur = el.next_sibling();
    while let Some(node) = cur {
        let Some(sib) = ElementRef::wrap(node) else {
            cur = node.next_sibling();
            continue;
        };
        if is_stop(&sib) {
            break;
        }
        out.push_str(&sib.text().collect::<String>());
        cur = sib.next_sibling();
    }
    let out = clean(&out);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 收集 #examples 区块后的所有 pre 代码块（每个 pre 单独一条）
fn section_pres(doc: &Html, id: &str) -> Vec<String> {
    let sel = match Selector::parse(&format!("#{}", id)).ok() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let Some(el) = doc.select(&sel).next() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cur = el.next_sibling();
    while let Some(node) = cur {
        let Some(sib) = ElementRef::wrap(node) else {
            cur = node.next_sibling();
            continue;
        };
        if is_stop(&sib) {
            break;
        }
        if sib.value().name() == "pre" {
            let code = clean(&sib.text().collect::<String>());
            if !code.is_empty() {
                out.push(code);
            }
        }
        cur = sib.next_sibling();
    }
    out
}

/// 收集 #see-also 区块后的链接文本
fn section_links(doc: &Html, id: &str) -> Vec<String> {
    let sel = match Selector::parse(&format!("#{}", id)).ok() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let Some(el) = doc.select(&sel).next() else {
        return Vec::new();
    };
    let a_sel = Selector::parse("a").unwrap();
    let mut out = Vec::new();
    let mut cur = el.next_sibling();
    while let Some(node) = cur {
        let Some(sib) = ElementRef::wrap(node) else {
            cur = node.next_sibling();
            continue;
        };
        if is_stop(&sib) {
            break;
        }
        for a in sib.select(&a_sel) {
            let t = clean(&a.text().collect::<String>());
            if !t.is_empty() {
                out.push(t);
            }
        }
        cur = sib.next_sibling();
    }
    out
}

/// 数 #parameters 区块中的参数段落/表格行数（启发式评分用）
fn count_params(doc: &Html, id: &str) -> usize {
    let sel = match Selector::parse(&format!("#{}", id)).ok() {
        Some(s) => s,
        None => return 0,
    };
    let Some(el) = doc.select(&sel).next() else {
        return 0;
    };
    let mut count = 0usize;
    let mut cur = el.next_sibling();
    while let Some(node) = cur {
        let Some(sib) = ElementRef::wrap(node) else {
            cur = node.next_sibling();
            continue;
        };
        if is_stop(&sib) {
            break;
        }
        if sib.value().name() == "p" {
            count += 1;
        } else if sib.value().name() == "table" {
            let tr_sel =
                Selector::parse("tbody tr").unwrap_or_else(|_| Selector::parse("tr").unwrap());
            count += sib.select(&tr_sel).count();
        }
        cur = sib.next_sibling();
    }
    count
}

/// 抓取并解析 API 详情页
pub async fn fetch_and_parse(
    client: &Client,
    url: &str,
    summary: &str,
) -> Result<ApiDetail, Box<dyn std::error::Error>> {
    let body = fetch_with_retry(client, url).await?;
    let doc = Html::parse_document(&body);

    let mut detail = ApiDetail {
        summary: summary.to_string(),
        ..Default::default()
    };

    // 签名：Syntax 区块（h2 的兄弟 pre）
    detail.cpp_signature = section_text(&doc, "syntax");
    // 参数
    detail.params_text = section_text(&doc, "parameters");
    detail.param_count = count_params(&doc, "parameters");
    // 返回值 / 备注
    detail.return_value = section_text(&doc, "return-value");
    detail.remarks = section_text(&doc, "remarks");
    // 示例
    detail.examples = section_pres(&doc, "examples");
    // See also
    detail.see_also = section_links(&doc, "see-also");
    // 更新时间：页脚 local-time
    if let Some(lt) = doc
        .select(&Selector::parse("local-time[datetime]").unwrap())
        .next()
    {
        if let Some(dt) = lt.value().attr("datetime") {
            detail.updated = Some(dt.to_string());
        }
    }

    Ok(detail)
}
