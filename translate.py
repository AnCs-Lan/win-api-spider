#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""translate.py — Windows API 文档中文化（doc.content -> doc.content_zh）

用法:
  python3 translate.py --sample 20                 # 小样本验证（随机 20 条）
  python3 translate.py --modules kernel32.dll --limit 500
  python3 translate.py --modules "kernel32.dll,user32.dll,gdi32.dll,advapi32.dll,oleaut32.dll,shlwapi.dll,setupapi.dll,ole32.dll"   # 核心模块
  python3 translate.py                             # 全量（断点续翻，可中断重跑）

特性:
  - 断点续翻: 只取 content_zh IS NULL 的行
  - 并发 8, 失败重试 3 次指数退避
  - 关闭思考模式: thinking={"type":"none"}
  - 费用统计: 输入 ¥1/M token, 输出 ¥2/M token
"""
import argparse
import json
import os
import random
import sqlite3
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from urllib import request, error

DB_PATH = os.environ.get(
    "WINAPI_DB",
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "winapi.db"),
)
API_URL = "https://api.deepseek.com/chat/completions"
MODEL = "deepseek-v4-flash"
TRANSLATE_TITLES = ("函数说明", "参数说明", "返回值", "备注")
MAX_SINGLE = 4000          # content 超过此长度单独请求
PRICE_IN = 1.0             # 元 / M token（输入）
PRICE_OUT = 2.0            # 元 / M token（输出）

SYSTEM_PROMPT = """你是 Windows API 文档翻译专家，负责把英文 Microsoft Learn 文档翻译成简体中文。

规则：
1. 代码、常量、标识符、函数名、类型名、宏（如 GENERIC_READ、CreateFileW、HANDLE、LPCWSTR）必须原样保留，绝不翻译。
2. 术语用中文社区常用译名：handle=句柄、thread=线程、process=进程、buffer=缓冲区、return value=返回值、device=设备、window=窗口、message=消息。
3. 翻译准确通顺、信息完整；超长文本可适当精简但不得遗漏关键信息与数字。
4. 严格只输出一个 JSON 对象，格式：{"results": [{"id": <输入中的id>, "content_zh": "中文翻译"}]}。
   只输出 content_zh 字段，不要输出 title、content 等其他字段。
   示例：{"results": [{"id": 42, "content_zh": "检索当前本地日期和时间。"}]}"""

KEY = os.environ.get("DEEPSEEK_API_KEY", "")

def log(msg):
    print(msg, flush=True)

def call_api(batch, strict=False):
    """发送一批, 返回 (parsed_results, usage, has_thinking, raw)。失败抛异常。"""
    user_content = json.dumps(batch, ensure_ascii=False)
    if strict:
        user_content += "\n\n请严格按格式输出：{\"results\": [{\"id\": <原id>, \"content_zh\": \"中文翻译\"}]}。只输出 content_zh 字段。"
    payload = {
        "model": MODEL,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_content},
        ],
        "response_format": {"type": "json_object"},
        "thinking": {"type": "disabled"},
        "temperature": 0.3,
        "max_tokens": 8192,
    }
    req = request.Request(
        API_URL,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Authorization": "Bearer " + KEY,
        },
        method="POST",
    )
    with request.urlopen(req, timeout=60) as resp:
        body = json.loads(resp.read().decode("utf-8"))
    msg = body["choices"][0]["message"]
    content = msg.get("content", "")
    usage = body.get("usage", {})
    has_thinking = "reasoning_content" in msg or "thinking" in msg
    parsed = json.loads(content)
    if isinstance(parsed, dict) and "results" in parsed:
        return parsed["results"], usage, has_thinking, content
    if isinstance(parsed, list):
        return parsed, usage, has_thinking, content
    raise ValueError("无法识别的 JSON 结构: " + content[:200])

def _merge_usage(usages):
    u = {"prompt_tokens": 0, "completion_tokens": 0}
    for x in usages:
        u["prompt_tokens"] += int(x.get("prompt_tokens", 0) or 0)
        u["completion_tokens"] += int(x.get("completion_tokens", 0) or 0)
    return u

def translate_batch(rows, retries=3, depth=0):
    """rows: [(id, title, content), ...]; 返回 ([(content_zh, id), ...], usage, has_thinking)
    失败重试 retries 次；仍失败则拆半递归（降级到单条）；单条仍失败返回 ([], usage, False)。"""
    batch = [
        {"id": r[0], "title": r[1], "content": r[2][:MAX_SINGLE] if len(r[2]) > MAX_SINGLE else r[2]}
        for r in rows
    ]
    last_err = None
    usages = []
    has_thinking = False
    for attempt in range(retries + 1):
        try:
            results, usage, ht, raw = call_api(batch, strict=attempt > 0)
            usages.append(usage)
            has_thinking = has_thinking or ht
            by_id = {}
            for r in results:
                if isinstance(r, dict) and "id" in r:
                    zh = r.get("content_zh")
                    if zh is None and isinstance(r.get("content"), str) and any("\u4e00" <= c <= "\u9fff" for c in r["content"][:200]):
                        zh = r["content"]  # 兼容: 模型把译文放进了 content 字段
                    if zh is not None:
                        try:
                            by_id[int(r["id"])] = zh
                        except (ValueError, TypeError):
                            pass
            if len(by_id) != len(batch):
                raise ValueError(f"返回条数 {len(by_id)} != 请求 {len(batch)} | 原始: {raw[:160]!r}")
            out = [(by_id[i], i) for i, _, _ in rows if i in by_id]
            if len(out) != len(rows):
                raise ValueError(f"id 映射缺失: {len(out)}/{len(rows)}")
            return out, _merge_usage(usages), has_thinking
        except Exception as e:
            last_err = e
            wait = 2 ** attempt
            log(f"  批次失败(第{attempt+1}次, 深度{depth}): {str(e)[:140]}; {wait}s 后重试")
            time.sleep(wait)
    # 降级: 拆半递归
    if len(rows) > 1 and depth < 5:
        mid = len(rows) // 2
        l = translate_batch(rows[:mid], retries=1, depth=depth + 1)
        r = translate_batch(rows[mid:], retries=1, depth=depth + 1)
        return l[0] + r[0], _merge_usage([l[1], r[1]]), l[2] or r[2]
    log(f"  !! 丢弃 {len(rows)} 条(最终失败): {str(last_err)[:120]}")
    return [], _merge_usage(usages), has_thinking

def main():
    ap = argparse.ArgumentParser(description="Windows API 文档翻译")
    ap.add_argument("--sample", type=int, default=0, help="随机取 N 条待翻译样本")
    ap.add_argument("--limit", type=int, default=0, help="最多处理 N 条 (0=不限)")
    ap.add_argument("--modules", type=str, default="", help="逗号分隔模块白名单，如 kernel32.dll,user32.dll")
    ap.add_argument("--batch", type=int, default=10, help="每批条数 (8-12)")
    ap.add_argument("--concurrency", type=int, default=6, help="并发请求数")
    ap.add_argument("--dry-run", action="store_true", help="只统计不调用 API")
    args = ap.parse_args()

    if not KEY:
        sys.exit("错误: 未设置 DEEPSEEK_API_KEY 环境变量")

    db = sqlite3.connect(DB_PATH)
    cur = db.cursor()
    where = f"content_zh IS NULL AND title IN ({','.join('?' * len(TRANSLATE_TITLES))})"
    params = list(TRANSLATE_TITLES)
    if args.modules:
        mods = [m.strip().lower() for m in args.modules.split(",") if m.strip()]
        where += f" AND lower((SELECT module FROM api WHERE id = doc.api_id)) IN ({','.join('?' * len(mods))})"
        params += mods

    total = int(cur.execute(f"SELECT COUNT(*) FROM doc WHERE {where}", params).fetchone()[0])
    log(f"待翻译: {total} 条 (样本={args.sample}, 限制={args.limit})")

    if args.sample > 0:
        rows = cur.execute(
            f"SELECT id, title, content FROM doc WHERE {where} ORDER BY RANDOM() LIMIT ?",
            params + [args.sample],
        ).fetchall()
    else:
        rows = cur.execute(f"SELECT id, title, content FROM doc WHERE {where}", params).fetchall()
        if args.limit > 0:
            rows = rows[: args.limit]

    if args.dry_run:
        log("dry-run: 未调用 API")
        return

    done = 0
    tok_in = tok_out = 0
    thinking_seen = 0
    t0 = time.time()
    batches = [rows[i : i + args.batch] for i in range(0, len(rows), args.batch)]
    log(f"共 {len(batches)} 批, 并发 {args.concurrency}")

    with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        futures = {pool.submit(translate_batch, b): b for b in batches}
        for fut in as_completed(futures):
            b = futures[fut]
            try:
                results, usage, has_thinking = fut.result()
            except Exception as e:
                log(f"!! 批次异常({len(b)}条): {e}")
                continue
            cur.executemany("UPDATE doc SET content_zh = ? WHERE id = ?", results)
            db.commit()
            done += len(results)
            tok_in += int(usage.get("prompt_tokens", 0) or 0)
            tok_out += int(usage.get("completion_tokens", 0) or 0)
            if has_thinking:
                thinking_seen += 1
            cost = tok_in / 1e6 * PRICE_IN + tok_out / 1e6 * PRICE_OUT
            el = time.time() - t0
            log(f"进度 {done}/{len(rows)} | 输入 {tok_in} tok | 输出 {tok_out} tok | 费用 ¥{cost:.4f} | {el:.0f}s")

    db.close()
    cost = tok_in / 1e6 * PRICE_IN + tok_out / 1e6 * PRICE_OUT
    log("=" * 60)
    log(f"完成: {done} 条")
    log(f"思考模式出现次数: {thinking_seen} (应为 0)")
    log(f"输入 token: {tok_in} | 输出 token: {tok_out}")
    log(f"费用估算: ¥{cost:.4f}")

if __name__ == "__main__":
    main()
