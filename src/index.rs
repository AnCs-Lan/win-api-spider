use crate::types::IndexedApi;
use reqwest::Client;
use scraper::{Html, Selector};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::time::Duration;

const LANDING: &str = "https://learn.microsoft.com/en-us/windows/win32/api/";
const INDEX_CACHE: &str = "index_cache.tsv";

/// 内置 header 列表（核心 + 常用，配合 landing 页提取的入口）
const FALLBACK_HEADERS: &[&str] = &[
    "fileapi", "winbase", "winuser", "winreg", "winnt", "handleapi", "memoryapi",
    "processthreadsapi", "synchapi", "sysinfoapi", "errhandlingapi", "heapapi",
    "libloaderapi", "namedpipeapi", "processenv", "consoleapi", "debugapi",
    "ioapiset", "jobapi", "timezoneapi", "wow64apiset", "wingdi", "commctrl",
    "commdlg", "shellapi", "shlobj_core", "ole2", "oleauto", "combaseapi",
    "objbase", "objidl", "ocidl", "unknwn", "ws2tcpip", "winsock2", "winsock",
    "mswsock", "ws2def", "ws2spi", "iphlpapi", "netioapi", "ntsecapi",
    "securitybaseapi", "aclapi", "sddl", "wincon", "winnls", "winsvc",
    "windowsx", "winerror", "winver", "winspool", "wtsapi32", "winioctl",
    "wincrypt", "crypt32", "bcrypt", "ncrypt", "setupapi", "cfgmgr32",
    "shlwapi", "pathcch", "strsafe", "intsafe", "mmsystem", "winmm",
    "joystickapi", "uxtheme", "dwmapi", "gdiplus", "gdiplusheaders", "d2d1",
    "dwrite", "d3d11", "d3d12", "d3dcommon", "dxgi", "dcomptypes", "mfapi",
    "mfobjects", "mftransform", "audioclient", "mmdeviceapi", "endpointvolume",
    "imm", "immdev", "oleacc", "minwindef", "windef", "minwinbase", "basetsd",
    "guiddef", "ntdef", "ntstatus", "winternl", "threadpoollegacyapiset",
    "apiquery2", "consoleapi2", "fileapifromapp", "jobapi2", "processtopologyapi",
    "wow64apiset", "dhcpsapi", "ws2tcpip", "ws2def", "wincred", "ntquery",
    "winnls32", "winbase", "winuser", "winreg", "winsvc", "wtsapi32",
    "rpc", "rpcdce", "rpcnsi", "ntsecapi", "lsa", "secext", "security",
    "winldap", "winhttp", "wininet", "urlmon", "oleacc", "oaidl", "servprov",
    "mmreg", "mmsyscom", "dsound", "dmusici", "dmusicc", "winnls", "winnls32",
];

/// 索引缓存读写（TSV：name \t url \t summary）
pub fn write_index_cache(list: &[IndexedApi]) -> Result<(), Box<dyn std::error::Error>> {
    let mut content = String::new();
    for a in list {
        content.push_str(&format!("{}\t{}\t{}\n", a.name, a.url, a.summary));
    }
    fs::write(INDEX_CACHE, content)?;
    println!("索引缓存已写入 {}", INDEX_CACHE);
    Ok(())
}

pub fn read_index_cache() -> Result<Vec<IndexedApi>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(INDEX_CACHE)?;
    let mut list = Vec::new();
    for line in content.lines() {
        let mut parts = line.splitn(3, '\t');
        let name = parts.next().unwrap_or("").to_string();
        let url = parts.next().unwrap_or("").to_string();
        let summary = parts.next().unwrap_or("").to_string();
        if !name.is_empty() && !url.is_empty() {
            list.push(IndexedApi {
                name,
                url,
                summary,
            });
        }
    }
    Ok(list)
}

/// 带重试的 GET（网络错误/5xx 重试 3 次指数退避，4xx 立即失败）
pub async fn fetch_with_retry(
    client: &Client,
    url: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut delay = Duration::from_millis(300);
    let mut last_err: Option<Box<dyn std::error::Error>> = None;
    for attempt in 0..3 {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(resp.text().await?);
                }
                if status.is_client_error() {
                    // 4xx：不重试
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("HTTP {}", status),
                    )
                    .into());
                }
                last_err = Some(
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("HTTP {}", status),
                    )
                    .into(),
                );
            }
            Err(e) => last_err = Some(Box::new(e)),
        }
        if attempt < 2 {
            tokio::time::sleep(delay).await;
            delay *= 2;
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, "unknown fetch error").into()
    }))
}

fn clean_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 相对/绝对路径解析为完整 URL（处理 ../ 上跳），以 base_url 所在目录为基准
fn resolve_href(base_url: &str, href: &str) -> Option<String> {
    if href.starts_with("http") {
        return Some(href.to_string());
    }
    if href.starts_with('/') {
        return Some(format!("https://learn.microsoft.com{}", href));
    }
    if href.starts_with('#') || href.is_empty() {
        return None;
    }
    let mut base = base_url.trim_end_matches('/').to_string();
    let mut rest = href;
    while rest.starts_with("../") {
        if let Some(pos) = base.rfind('/') {
            base.truncate(pos);
        }
        rest = &rest[3..];
    }
    Some(format!("{}/{}", base, rest))
}

/// 把 href 解析为 api/ 前缀的模块页完整 URL（支持相对与绝对路径），非模块页返回 None
fn resolve_api_url(base_url: &str, href: &str) -> Option<String> {
    let full = if href.starts_with("http") {
        href.to_string()
    } else if href.starts_with('/') {
        format!("https://learn.microsoft.com{}", href)
    } else if href.starts_with('#') || href.is_empty() {
        return None;
    } else {
        format!("{}{}", base_url, href)
    };
    let rest = full.strip_prefix("https://learn.microsoft.com/en-us/windows/win32/api/")?;
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest.starts_with("nf-") {
        return None;
    }
    Some(format!(
        "https://learn.microsoft.com/en-us/windows/win32/api/{}/",
        rest
    ))
}

/// 阶段1+2：从 landing 页出发，收集所有模块/领域页并提取 nf 详情链接
pub async fn build_index(
    client: &Client,
    index_only: bool,
) -> Result<Vec<IndexedApi>, Box<dyn std::error::Error>> {
    // ---- 阶段1+2 合并：从 landing 出发 BFS，每页一次 GET 同时提取 nf 链接 ----
    let mut module_urls: BTreeSet<String> = BTreeSet::new();
    let a_sel = Selector::parse("a[href]").unwrap();
    let tr_sel = Selector::parse("tr").unwrap();
    let nf_a_sel = Selector::parse("a[href*='/nf-']").unwrap();
    let landing_base = "https://learn.microsoft.com/en-us/windows/win32/api/";
    let mut index: HashMap<String, IndexedApi> = HashMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::from([LANDING.to_string()]);

    // 初始队列：landing 页提取的模块/领域页 + 内置 header 兜底
    let landing = fetch_with_retry(client, LANDING).await?;
    let mut queue: Vec<String> = Vec::new();
    {
        let doc = Html::parse_document(&landing);
        for el in doc.select(&a_sel) {
            if let Some(href) = el.value().attr("href") {
                if let Some(u) = resolve_api_url(landing_base, href) {
                    module_urls.insert(u.clone());
                    queue.push(u);
                }
            }
        }
    }
    for h in FALLBACK_HEADERS {
        let u = format!("https://learn.microsoft.com/en-us/windows/win32/api/{}/", h);
        module_urls.insert(u.clone());
        queue.push(u);
    }
    println!(
        "[阶段1] 初始队列 {} 个模块/领域页（landing {} + 兜底 {}），爬取中",
        queue.len(),
        module_urls.len() - FALLBACK_HEADERS.len(),
        FALLBACK_HEADERS.len()
    );

    // 顺序爬取初始集合（landing 入口 + 内置 header），提取 nf 链接
    let mut module_list: Vec<String> = queue.drain(..).collect();
    module_list.sort();
    module_list.dedup();
    let mut processed_pages = 0usize;
    for url in module_list {
        if !seen.insert(url.clone()) {
            continue;
        }
        let body = match fetch_with_retry(client, &url).await {
            Ok(b) => b,
            Err(_) => {
                processed_pages += 1;
                continue;
            }
        };
        processed_pages += 1;
        let doc = Html::parse_document(&body);

        // 提取 nf 详情链接（阶段2）
        for tr in doc.select(&tr_sel) {
            let Some(a) = tr.select(&nf_a_sel).next() else { continue };
            let Some(href) = a.value().attr("href") else { continue };
            let Some(full_url) = resolve_href(&url, href) else {
                continue;
            };
            // 函数名取链接文本（URL 是 nf-{header}-{name}，不能直接剥前缀）
            let display = clean_text(&a.text().collect::<String>());
            let name = display.to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            let full = tr.text().collect::<String>();
            let summary = clean_text(&full.replacen(&display, "", 1));
            index.entry(name.clone()).or_insert(IndexedApi {
                name,
                url: full_url,
                summary,
            });
        }
        if processed_pages % 50 == 0 {
            println!(
                "  [索引] 已爬 {} 个模块页，索引 {} 个 nf",
                processed_pages,
                index.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    println!(
        "[阶段1+2] 共爬 {} 个模块页，索引 {} 个 nf 详情页",
        processed_pages,
        index.len()
    );

    let mut list: Vec<IndexedApi> = index.into_values().collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));

    // 写索引缓存（供 --skip-index 复用）
    write_index_cache(&list)?;

    if index_only {
        println!("\n===== 索引统计 =====");
        println!("模块/领域页: {} 个", module_urls.len());
        println!("nf 详情页: {} 个", list.len());
        println!("\n----- 样例（前 10 条）-----");
        for a in list.iter().take(10) {
            println!("{}  ->  {}", a.name, a.url);
            println!("   简介: {}", &a.summary[..a.summary.len().min(80)]);
        }
    }

    Ok(list)
}
