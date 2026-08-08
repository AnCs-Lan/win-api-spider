use serde::Deserialize;

/// data.csv 中的一行（目标清单）
#[derive(Debug, Clone, Deserialize)]
pub struct CsvApi {
    pub name: String,
    pub dll: String,
    pub signature: String,
    pub description: String,
}

/// 阶段2：索引到的 API（来自 Learn 模块页）
#[derive(Debug, Clone)]
pub struct IndexedApi {
    /// 函数名（URL 中提取，已转小写）
    pub name: String,
    /// 详情页完整 URL
    pub url: String,
    /// 模块页上的一句话简介
    pub summary: String,
}

/// 阶段3：详情页解析结果
#[derive(Debug, Clone, Default)]
pub struct ApiDetail {
    /// C++ 签名（语法区块）
    pub cpp_signature: Option<String>,
    /// 参数说明拼接文本
    pub params_text: Option<String>,
    /// 参数个数（用于启发式评分）
    pub param_count: usize,
    /// 返回值说明
    pub return_value: Option<String>,
    /// 备注
    pub remarks: Option<String>,
    /// 示例代码块
    pub examples: Vec<String>,
    /// See also 相关函数
    pub see_also: Vec<String>,
    /// 页面修改时间
    pub updated: Option<String>,
    /// 模块页简介（回填到 api 表 description）
    pub summary: String,
}

/// 启发式评分结果
#[derive(Debug, Clone, Copy)]
pub struct Scores {
    pub usage: f64,
    pub complexity: f64,
    pub risk: f64,
    pub total: f64,
}

/// 一条待写入 doc 表的条目
#[derive(Debug, Clone)]
pub struct DocEntry {
    pub title: String,
    pub content: String,
    pub tags: String,
}
