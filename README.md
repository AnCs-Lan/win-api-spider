# win-api-spider

Windows API 数据获取 + 文档翻译项目：以 `data.csv`（17148 个 API 清单）为目标，爬取 Microsoft Learn 文档，
按 `win-api-search/schema.sql` 契约填充 `winapi.db`，供查询项目使用；并负责把文档英文翻译为中文（`doc.content_zh`）。

## 解耦关系

- 本项目的**唯一数据定义**是 `../win-api-search/schema.sql`（api / doc 表）
- `doc` 表含中文字段 `content_zh`（2026-08-09 全量翻译完成，覆盖率 100%），查询项目优先显示中文、空则回退英文
- 产出 `winapi.db` 后复制到查询项目目录即可查询：
  ```bash
  cp winapi.db ../win-api-search/winapi.db
  ```

## 数据流（三阶段）

1. **索引**：爬 `learn.microsoft.com/windows/win32/api/`（landing + 领域页 + 内置 header 列表）→ 收集 nf 详情页 URL 与一句话简介，结果缓存到 `index_cache.tsv`
2. **匹配**：读 `data.csv` 清单，按函数名匹配索引（索引约 1.8 万+，清单 17148）
3. **详情**：逐 API 爬详情页，解析签名 / 参数 / 返回值 / 备注 / 示例 / See also，写入 api + doc 表

## 使用（单次命令）

```bash
# 1. 构建索引缓存（约 4 分钟，仅首次需要；build_index 会自动写入）
cargo run -- --index-only

# 2. 试跑：解析 3 个 KERNEL32.dll 的 API，不写库，打印摘要
cargo run -- --dll KERNEL32.dll --limit 3 --dry-run

# 3. 正式写入（先小规模验证）
cargo run -- --dll KERNEL32.dll --limit 5 --fresh

# 4. 断点续爬（跳过已入库的，配合索引缓存加速）
cargo run -- --resume --skip-index
```

## 批量脚本 batch_crawl.sh（推荐全量使用）

按 dll 分批爬取全部 344 个 dll，自带**单 dll 成功检查**与**断点续传**：

```bash
./batch_crawl.sh               # 爬所有 dll（跳过已完成）
./batch_crawl.sh --dll NAME    # 只爬指定 dll（如 KERNEL32.dll）
./batch_crawl.sh --retry       # 只重跑上次失败的 dll
./batch_crawl.sh --check-only  # 只做成功检查，不爬取
./batch_crawl.sh --force       # 忽略已完成记录，全部重跑
```

- **成功检查**：每个 dll 爬完对比 `data.csv` 期望数 vs db 已入库数（`lower(module)` 匹配），100% 或 ≥80%（部分函数在 Learn 无页面）判定成功
- **断点续传**：`.crawl_done` 记录已完成 dll（重跑自动跳过），`.crawl_failed` 记录失败 dll（`--retry` 重跑）；spider 的 `--resume` 再兜一层跳过已入库 API
- **日志**：每个 dll 的输出存 `logs/<dll>.log`
- 脚本自动复用 `index_cache.tsv`（`--skip-index`），不会每个 dll 重跑索引

## 文档翻译（中文化）

翻译任务书见 `TASK_TRANSLATE.md`（含勘误记录）；翻译脚本 `translate.py`（Python 标准库，零依赖）已实现并全量跑完：

```bash
python3 translate.py --sample 20        # 小样本验证（随机 20 条）
python3 translate.py --modules "kernel32.dll,user32.dll"   # 指定模块
python3 translate.py                    # 全量（断点续翻：只处理 content_zh IS NULL）
```

- **范围**：只翻译 `函数说明` / `参数说明` / `返回值` / `备注`；`函数签名` 与 `示例*` 直接复制原文（代码/签名不翻译）
- **API**：DeepSeek `deepseek-v4-flash`，`thinking: {"type": "disabled"}` 关闭思考模式（注意：`none` 会 400，见任务书勘误）
- **容错**：并发 6-8、60s 超时重试 3 次、整批失败拆半递归降级、断点续翻兜底；`content_zh` 为空自动跳过
- **状态（2026-08-09）**：`doc.content_zh` 覆盖率 **100%**（43179/43179）；实测费用约 **¥12.9**（低于任务书预估 ¥17-30 约 46%）
- **交付**：`cp winapi.db ../win-api-search/winapi.db`（查询项目已适配 content_zh 优先显示并验证中文输出）

## CLI 参数

- `--dll <name>`：只处理指定 dll（大小写不敏感，如 `kernel32.dll`）
- `--limit <n>`：最多处理 n 个 API（0=不限）
- `--index-only`：只跑索引阶段（并写入 index_cache.tsv）
- `--skip-index`：跳过索引构建，从 index_cache.tsv 读取
- `--dry-run`：解析但不写库，打印摘要
- `--resume`：跳过已入库 API（断点续爬）
- `--fresh`：清空重建数据库
- `--csv <path>`：清单路径（默认 `data.csv`）

## 字段说明

- `api`：name（清单）/ module（清单 dll）/ category（按 dll 映射）/ signature（清单签名）/ related（See also）/ 四维评分
- `doc`：函数说明（模块页简介）/ 函数签名 / 参数说明 / 返回值 / 备注 / 示例，source = Microsoft Learn；`content_zh` 为中文译文（空则查询项目回退英文）
- 评分：usage 按模块热度、complexity 按签名参数数、risk 按句柄/指针特征，total 加权（启发式，可人工修正）

## 工程机制

- 限速 150-300ms/请求，网络错误/5xx 重试 3 次指数退避，4xx 立即跳过
- reqwest 客户端 20s 超时
- 索引约 184 个模块页（约 4 分钟）；全量 17148 个 API 预计数小时，推荐 `./batch_crawl.sh` 分批

## 目录结构

```
├── Cargo.toml / src/         # 爬虫（Rust，reqwest + sqlx）
├── batch_crawl.sh            # 批量爬取脚本（断点续爬）
├── translate.py              # 文档翻译脚本（Python 标准库）
├── TASK_TRANSLATE.md         # 翻译任务书（含勘误）
├── data.csv                  # API 清单（17148 个）
├── index_cache.tsv           # 索引缓存（约 1.8 万 nf）
├── logs/                     # 每 dll 爬取日志
├── translate_*.log           # 翻译进度/费用日志
├── winapi.db                 # 产出数据库（含 content_zh）
├── .crawl_done / .crawl_failed  # 断点续爬标记
└── .env                      # DATABASE_URL（gitignore 忽略）
```

## 开发经验总结

### 1. 索引策略：BFS 递归 → 模块页直爬

**原始设计**：从 landing 页 BFS 递归发现所有 header 模块页，再逐页提取函数链接。

**遇到的问题**：
- landing 页是**技术领域目录**（DirectML / GDI / Media Foundation 等分组），并不直接列出 header 模块页；其链接是相对路径（`_directml/`），且 Akamai 缓存版本差异导致同一次运行提取数在 39 与 0 之间跳变（绝对/相对路径混杂）
- 领域页（`_xxx/`）大多没有函数表格，BFS 漫游大量无效页面；链接膨胀快（两个页面就能发现 700+ 新链接），但有效索引停滞在 551 个 nf
- 请求失控：单页无超时 + 递归膨胀，500 秒只爬完 100 页，进程被 timeout 杀掉

**方向转换**：放弃领域页递归，改为**内置 ~140 个核心 header 列表直爬**（URL 模式固定 `api/{header}/`），landing 提取的入口作为补充。

**效果对比**：

| 指标 | BFS 递归 | 模块页直爬 |
|---|---|---|
| 模块页请求 | 膨胀至 600+ 上限 | 184 个（可控） |
| nf 详情页覆盖 | 551 | 18640（约 34 倍） |
| 单次耗时 | 500s 未完成 | 约 4 分钟 |

**优劣总结**：
- BFS 理论上能发现全部 header（含冷门），但领域页收益低、请求量不可控、行为依赖页面缓存变量，适合小规模/结构未知的站点
- 模块页直爬覆盖核心 header 且请求可预期，代价是冷门 header 依赖内置列表，靠 landing 提取入口缓解——对"核心 API 查询"场景是正确取舍
- 核心教训：**先验证目标页面的真实结构，再决定遍历策略**；页面结构假设（landing 直接列 header）与实际（领域目录）不符是 BFS 低效的根源

### 2. URL 解析与函数名匹配的修正过程

三个连续的 bug，都源于 Learn URL 的结构特征，按"dry-run 小样本验证"逐个暴露并修正：

**a. landing 相对路径解析**
landing 的链接是 `data-linktype="relative-path"` 的相对路径（`_directml/`），最初只用 `strip_prefix("/en-us/windows/win32/api/")` 匹配绝对路径——缓存版本返回相对路径时提取数直接归零（39 → 0 跳变）。修正为 `resolve_api_url`：以当前页面 URL 为基准，统一拼接相对 / 绝对路径。

**b. 模块页 `../` 上跳链接**
模块页内函数链接形如 `../joystickapi/nf-joystickapi-joyconfigchanged`，直接拼接域名前缀会得到损坏 URL（`https://learn.microsoft.com../joystickapi/...`）。修正为 `resolve_href`：先按 `../` 逐级上跳目录，再拼接完整 URL。

**c. nf URL 的函数名提取（最隐蔽）**
详情页 URL 是 `nf-{header}-{name}` 结构（如 `nf-fileapi-createfilew`），最初用 `strip_prefix("nf-")` 提取函数名，得到的是 `fileapi-createfilew`——带着 header 前缀。与 data.csv 的函数名（`createfilew`）匹配时**全部失败**：CloseHandle、GetLastError、CreateFileW 等核心 API 全部报"索引未找到"。修正为用模块页 `<a>` 链接文本（即纯函数名）作为索引 key，匹配立即恢复（ClearCommBreak 等 3 个样本全部命中）。

**过程价值**：这条经验是"先小规模验证再全量"的直接收益——dry-run 只爬 3 个 API 就暴露了匹配 bug，避免了全量 17148 个 API 爬取后的整体返工。每个 bug 都有对应的失败证据（提取数 39→0、`learn.microsoft.com../` 损坏 URL、1343 个 KERNEL32 API 全跳过），修正后都有对照验证。
