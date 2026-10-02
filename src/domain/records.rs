//! 识别记录（records.jsonl）的行解析与加载：被「价格可视化」读取展示，
//! 由 `observe` 模块写入（唯一数据契约见 [`crate::observe::records_file`]）。
//!
//! 字段结构固定，手工提取即可，无需 serde 依赖。

use std::time::SystemTime;

use crate::observe::records_file;

/// 一条识别记录（records.jsonl 的行解析结果）。
#[derive(Clone, Debug)]
pub struct PriceRecord {
    pub caliber: String,
    pub bullet: String,
    /// 识别出的价格（非法 / 空为 `None`，绘图时跳过）。
    pub price: Option<u32>,
    pub ts_ms: u64,
}

/// 提取 JSON 行中字符串字段的值（形如 `"key":"value"`）。
fn json_str_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let i = line.find(&pat)? + pat.len();
    let rest = line[i..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// 提取 JSON 行中无符号数字字段的值（形如 `"key":123`）。
fn json_u64_field(line: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let i = line.find(&pat)? + pat.len();
    let rest = line[i..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// 解析一条 JSONL 记录行；字段缺失 / 格式非法返回 `None`（跳过该行）。
pub fn parse_record(line: &str) -> Option<PriceRecord> {
    Some(PriceRecord {
        caliber: json_str_field(line, "caliber")?.to_string(),
        bullet: json_str_field(line, "bullet")?.to_string(),
        price: json_str_field(line, "price").and_then(|s| s.parse::<u32>().ok()),
        ts_ms: json_u64_field(line, "ts_ms")?,
    })
}

/// 读取识别记录：新路径（save_path 根目录）优先；旧版 `prices/` 子目录仅作只读兼容，
/// 便于沿用既有历史数据（本模块从不写入 prices/）。
pub fn load_records(save_path: &str) -> Vec<PriceRecord> {
    for p in [
        records_file(save_path),
        format!("{save_path}/prices/records.jsonl"),
    ] {
        if let Ok(text) = std::fs::read_to_string(&p) {
            return text.lines().filter_map(parse_record).collect();
        }
    }
    Vec::new()
}

/// 记录文件 mtime（新路径优先），用于缓存失效判断。
pub fn records_mtime(save_path: &str) -> Option<SystemTime> {
    for p in [
        records_file(save_path),
        format!("{save_path}/prices/records.jsonl"),
    ] {
        if let Ok(m) = std::fs::metadata(&p) {
            if let Ok(t) = m.modified() {
                return Some(t);
            }
        }
    }
    None
}
