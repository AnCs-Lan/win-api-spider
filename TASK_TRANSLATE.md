# 翻译任务书（TASK_TRANSLATE.md）

> 本任务书自包含，供新会话独立执行。执行前请先通读本文件与相关项目 README。
> **勘误记录（2026-08-09 全量翻译实测后更新）**：4.1 思考参数、4.2 JSON 格式、4.4 容错、4.5 成本均已按实测修正，新会话直接按正文执行。

## 一、背景与现状（已就绪，勿重复爬取）

- **数据**：`~/Documents/MyDocuments/win-api-spider/winapi.db`
  - `api` 表：9020 行（函数名/模块/签名/评分）
  - `doc` 表：43179 行（函数说明/函数签名/参数说明/返回值/备注/示例，全英文，来源 Microsoft Learn）
- **查询项目**：`~/Documents/MyDocuments/win-api-search`（TUI + CLI，读同一 schema）
- **数据契约**：`win-api-search/schema.sql`（api/doc 表）
- 目标：把 `doc.content` 英文翻译为中文存入新列 `doc.content_zh`，查询项目优先显示中文、空则回退英文
- **翻译状态（2026-08-09 已全量完成）**：`doc.content_zh` 覆盖率 100%（43179/43179），实际总费用约 ¥12.9（实测），`winapi.db` 已交付查询项目

## 二、范围

- **翻译**：说明性文本——`title IN ('函数说明','参数说明','返回值','备注')`
- **不翻译**（`content_zh` 直接复制原文）：`title` 为 `函数签名`、`示例`、`示例N` 的条目（代码/签名）
- **优先级**：核心模块先行验证，再全量
  - 核心：kernel32(1056) / user32(736) / advapi32(575) / gdi32(417) / oleaut32(394) / shlwapi(360) / setupapi(329) / ole32(285)，共约 4000+ API

## 三、Schema 迁移

```sql
-- 检查列是否存在（SQLite 无 IF NOT EXISTS for column）
SELECT COUNT(*) FROM pragma_table_info('doc') WHERE name='content_zh';
-- 不存在则执行：
ALTER TABLE doc ADD COLUMN content_zh TEXT;
-- 签名/示例类直接复制原文（避免翻译代码）：
UPDATE doc SET content_zh = content
WHERE title = '函数签名' OR title LIKE '示例%';
```

## 四、翻译程序（新会话实现）

建议独立 Python 脚本（`translate.py`，标准库 `sqlite3` + `urllib`，放 win-api-spider 目录；已存在，可直接复用）；
也可在 win-api-spider 内加 Rust bin（`src/bin/translate.rs`，复用 reqwest/sqlx）。任选，Python 更轻。

### 4.1 DeepSeek API 调用（关键：关闭思考模式）

- `POST https://api.deepseek.com/chat/completions`
- Header：`Authorization: Bearer $DEEPSEEK_API_KEY`（**从环境变量读取，不硬编码；未配置先询问用户**）
- Body：
  ```json
  {
    "model": "deepseek-v4-flash",
    "messages": [{"role":"system","content":"..."},{"role":"user","content":"..."}],
    "response_format": {"type": "json_object"},
    "thinking": {"type": "disabled"}
  }
  ```
- **`"thinking": {"type": "disabled"}` 即关闭思考模式**（**勘误**：原任务书写 `none`，实测 API 返回 400 `unknown variant 'none'`，可选值为 `adaptive` / `enabled` / `disabled`；OpenAI SDK 走 `extra_body`，原生 requests/urllib 放 body 顶层）
- 验证是否生效：返回 message 中**不应出现 `reasoning_content` 字段**；实测全量翻译期间 0 次出现，token 用量与关闭前一致
- 价格：输入 ¥1/M（未命中缓存）、输出 ¥2/M；关闭思考可省 30-50% token

### 4.2 批处理与 JSON 输出

- 每请求打包 8-12 条：user 消息内容为 `[{"id": 1, "title": "...", "content": "..."}, ...]` 的 JSON 数组（附 title 帮助模型理解语境）
- system 提示要点：你是 Windows API 文档翻译专家；保留代码、常量、标识符、`GENERIC_READ` 类名原样；术语用中文社区常用译名；输出严格 JSON
- **输出格式（实测勘误）**：`response_format: json_object` 要求顶层是 JSON 对象，数组不保证被接受。**用对象包装输出**：`{"results": [{"id":1,"content_zh":"..."}]}`，解析后取 `results`。实测偶发情况：模型把译文放进 `content` 字段而非 `content_zh`（脚本做兼容容错：若 `content` 含中文字符则接受）；提示中明确"只输出 content_zh 字段"可显著降低概率
- `response_format: json_object` 保证可解析；解析失败整批重试

### 4.3 断点续翻

- 只取 `content_zh IS NULL` 的行；每批 `UPDATE doc SET content_zh=? WHERE id=?`（注意 executemany 参数顺序是 `(content_zh, id)`）
- 中断后重跑自动从断点继续（天然断点）

### 4.4 并发与容错（实测经验）

- 并发 6-8 个请求（实测 8 并发稳定；40 并发也未被硬限流但意义不大）；失败重试 3 次指数退避（1s/2s/4s/8s）
- **超时建议 60s（实测勘误）**：API 偶发**静默挂起**（不返回 429，请求挂着 30-40s 无响应），180s 超时会拖慢整体；60s 超时 + 重试可自愈
- **整批重试失败后拆半递归降级**（10 条 → 5 条 → 单条），单条仍失败则跳过记录，由断点续翻兜底；实测全量仅 kernel32 8 条首次遗漏，补跑即完成
- 每批 UPDATE 后立即 commit；打印进度（已翻译 X / 待翻译 Y / 累计费用）

### 4.5 成本预估（实测核对）

- 预估：全量 43179 条 ≈ ¥17-30（中值 ¥24）；核心模块 ≈ ¥5 以内
- **实测（2026-08-09，deepseek-v4-flash + thinking disabled）**：全量 33597 条待翻译实际约 **¥12.9**（其中 ¥10.97 为脚本日志实测、kernel32 主体约 ¥1.9 为按模块均价估算）；核心 8 模块（15661 条）约 **¥5.7**
- 低于预估约 46%；后续增量翻译参考单价约 ¥0.0004/条

## 五、查询项目适配（win-api-search）

1. `src/data_query.rs` `search_doc`：SELECT 增加 `d.content_zh AS content_zh`
2. `src/types.rs` `Doc`：增加 `pub content_zh: Option<String>`
3. `src/frontend.rs` `draw_single_doc`：正文显示 `content_zh`（空则回退 `content`）
4. `src/cli.rs`：同样优先 `content_zh`
5. 改完 `cargo build` + CLI 验证中文显示
- **注意**：改完必须重新编译（`cargo build`），旧二进制不读新列；已适配并验证（2026-08-09）

## 六、验证计划

1. **小样本（20 条）**：翻译后人工抽查——术语准确、代码/常量原样、无思考内容混入、JSON 解析正常
2. **核心模块**：批量翻译 → `cd win-api-search && cargo run -- a CreateFileW` 看中文输出
3. **全量**：统计 `content_zh` 非空率（应 ≈100%），抽查长文档
4. **费用核对**：实际消耗 vs 预估

## 七、执行步骤

1. 检查 `DEEPSEEK_API_KEY`；未配置先问用户
2. Schema 迁移（加列 + 签名/示例复制）
3. 实现翻译程序 → 20 条小样本验证（思考关闭生效 + 质量）
4. 核心模块批量翻译（约 4000+ API / 对应 doc）
5. 适配 win-api-search 查询 → 验证中文显示
6. 全量翻译（可分批、可中断续跑，建议后台 nohup）
7. 交付：`cp winapi.db ../win-api-search/winapi.db` + 总结（翻译行数、费用、匹配率）

## 八、风险与注意

- **涨价在即**：翻译尽快执行（本次已完成全量，后续增量无忧）
- **思考模式参数**：`disabled` 而非 `none`（已勘误，见 4.1）
- **质量**：术语不统一可接受（后续可人工修正）；代码/标识符必须原样保留
- **Key 安全**：仅环境变量读取，日志不打印 key
- **后台进程**：nohup 启动的翻译进程跨 code_execution 调用可能因 API 抖动静默挂起，需 60s 超时自愈；重跑前先 `pkill -f translate.py` 防并发重复（模块过滤不同则互不冲突）
