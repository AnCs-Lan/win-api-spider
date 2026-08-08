#!/bin/bash
# ============================================================
# win-api-spider 按 dll 分批全量爬取脚本
# ------------------------------------------------------------
# 功能：
#   1. 从 data.csv 提取所有唯一 dll，逐个调用 spider 爬取
#   2. 单 dll 检查报告：csv 期望数 vs db 已入库数（匹配率，信息性）
#   3. 断点续传：已完成 dll 记录在 .crawl_done，失败记录在 .crawl_failed
#
# 成败判据：spider 退出码（rc=0 即成功）。
# "Learn 索引缺失"是预期情况（部分 API 在 Learn 无页面），不算失败；
# 匹配率仅作报告展示，不参与成败判断。
#
# 用法：
#   ./batch_crawl.sh               # 爬所有 dll（跳过已完成）
#   ./batch_crawl.sh --dll NAME    # 只爬指定 dll（如 KERNEL32.dll）
#   ./batch_crawl.sh --retry       # 只重跑上次失败的 dll
#   ./batch_crawl.sh --check-only  # 只做匹配率检查报告，不爬取
#   ./batch_crawl.sh --force       # 忽略已完成记录，全部重跑
#
# 依赖：cargo / sqlite3
# ============================================================

set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

CSV="data.csv"
DB="winapi.db"
DONE_FILE=".crawl_done"
FAIL_FILE=".crawl_failed"
LOG_DIR="logs"
mkdir -p "$LOG_DIR"

# ---- 参数解析 ----
MODE="all"        # all | retry
FORCE=0
CHECK_ONLY=0
TARGET_DLL=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dll)       TARGET_DLL="$2"; shift 2 ;;
        --force)     FORCE=1; shift ;;
        --check-only) CHECK_ONLY=1; shift ;;
        --retry)     MODE="retry"; shift ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

[ -f "$CSV" ] || { echo "错误：缺少 $CSV"; exit 1; }
command -v sqlite3 >/dev/null || { echo "错误：需要 sqlite3"; exit 1; }
command -v cargo >/dev/null || { echo "错误：需要 cargo"; exit 1; }

# ---- 工具函数 ----

# 提取 data.csv 中的唯一 dll 列表（第 2 列）
get_dlls() {
    tail -n +2 "$CSV" | cut -d',' -f2 | tr -d '"' | sort -u
}

is_done()   { [ "$FORCE" -eq 1 ] && return 1; grep -qxF "$1" "$DONE_FILE" 2>/dev/null; }
is_failed() { grep -qxF "$1" "$FAIL_FILE" 2>/dev/null; }

# 单个 dll 匹配率报告（信息性，不参与成败）
check_dll() {
    local dll="$1"
    local expected actual
    expected=$(awk -F',' -v d="$dll" 'tolower($2)==tolower(d) {c++} END {print c+0}' "$CSV")
    if [ -f "$DB" ]; then
        actual=$(sqlite3 "$DB" "SELECT COUNT(*) FROM api WHERE lower(module)=lower('${dll//\'/\'\'}');" 2>/dev/null || echo 0)
    else
        actual=0
    fi
    if [ "$expected" -gt 0 ]; then
        local pct=$((actual * 100 / expected))
        echo "  [检查] $dll: 期望 $expected，已入库 $actual（匹配率 $pct%）"
    else
        echo "  [检查] $dll: csv 中无此 dll"
    fi
}

# 爬取单个 dll：成败以 spider 退出码为准
crawl_dll() {
    local dll="$1"
    local log_name="$LOG_DIR/$(echo "$dll" | tr '/.' '__').log"
    echo "===== [$dll] 开始爬取（日志: $log_name） ====="
    cargo run --quiet -- --dll "$dll" --resume --skip-index 2>&1 | tee "$log_name"
    local rc=${PIPESTATUS[0]}
    check_dll "$dll"
    if [ "$rc" -eq 0 ]; then
        grep -qxF "$dll" "$DONE_FILE" 2>/dev/null || echo "$dll" >> "$DONE_FILE"
        sed -i "/^${dll}$/d" "$FAIL_FILE" 2>/dev/null || true
        echo "===== [$dll] ✅ 成功 ====="
        return 0
    else
        grep -qxF "$dll" "$FAIL_FILE" 2>/dev/null || echo "$dll" >> "$FAIL_FILE"
        echo "===== [$dll] ❌ 失败（rc=$rc，已记录，可 --retry 重跑） ====="
        return 1
    fi
}

# ---- 主流程 ----

# 确保索引缓存存在（首次自动构建，约 4 分钟）
if [ ! -f "index_cache.tsv" ]; then
    echo "未发现 index_cache.tsv，先构建索引缓存..."
    cargo run --quiet -- --index-only || { echo "索引构建失败"; exit 1; }
fi

echo "编译检查..."
cargo build --quiet || { echo "编译失败"; exit 1; }

dlls=$(get_dlls)
if [ -n "$TARGET_DLL" ]; then
    dlls="$TARGET_DLL"
fi

echo "待处理 dll 列表（$(echo "$dlls" | wc -l | tr -d ' ') 个）"
count=0
for dll in $dlls; do
    if [ "$MODE" = "retry" ]; then
        is_failed "$dll" || { echo "[跳过] $dll 上次未失败"; continue; }
    fi
    if [ "$CHECK_ONLY" -eq 1 ]; then
        check_dll "$dll"
        continue
    fi
    if is_done "$dll"; then
        echo "[跳过] $dll 已完成"
        continue
    fi
    count=$((count + 1))
    crawl_dll "$dll" || true   # 单个 dll 失败不中断整个批次
done

echo ""
echo "========== 批次结束 =========="
echo "本次实际处理: $count 个 dll"
done_count=$([ -f "$DONE_FILE" ] && wc -l < "$DONE_FILE" || echo 0)
fail_count=$([ -f "$FAIL_FILE" ] && wc -l < "$FAIL_FILE" || echo 0)
echo "已完成: $done_count 个 / 失败: $fail_count 个"
if [ "$count" -eq 0 ]; then
    echo "全部完成，无待处理项。"
fi
