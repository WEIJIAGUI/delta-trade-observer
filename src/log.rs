//! 统一运行期日志：输出到项目专属 `logs/` 文件夹，格式为紧凑单行 JSON（AI 易解析、低 token 消耗）。
//!
//! 设计要点（用户要求）：
//! - 统一写入 `<项目根>/logs/run.log`（项目根 = 向上查找 `Cargo.toml` 得到；可用环境变量覆盖）；
//! - **开发模式**（`debug` 构建，或环境变量 `DELTA_OBSERVER_DEV=1/true/yes/on`）启动时**先清空**再写入；
//!   **生产模式**（`release`）以追加方式写入，保留历史；
//! - 格式为单行 JSON：`{"t":<unix_ms>,"l":"INFO|WARN","m":"<模块>","e":"<事件>",<结构化字段>}`
//!   —— 无缩进、无换行、字段紧凑，便于 AI 直接 `split('\n')` 后逐条 `json.loads`，降低积分消耗；
//! - 既有行式 `write` 继续可用（自动包成 JSON `note` 事件），新增 `event` 支持结构化字段；
//! - 未调用 `init`（如单元测试）时为 no-op，不落盘、不影响测试。

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 日志句柄：初始化后持有文件，未初始化为 `None`（no-op）。
static HANDLE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

/// 是否处于开发模式：`debug` 构建，或显式设置环境变量 `DELTA_OBSERVER_DEV=1/true/yes/on`。
///
/// Rust 本身不区分「开发/生产」运行语义，仅在**编译期**通过 `cfg(debug_assertions)`
/// 区分 debug / release 构建（见 `main.rs` 顶部说明与下方推荐实现）。此处用构建标志 +
/// 环境变量双重判定，兼顾「构建即决定」与「运行时可覆盖」。
pub fn dev_mode() -> bool {
    if let Ok(v) = std::env::var("DELTA_OBSERVER_DEBUG_DEV_OVERRIDE") {
        let v = v.trim().to_ascii_lowercase();
        if matches!(v.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    if let Ok(v) = std::env::var("DELTA_OBSERVER_DEV") {
        let v = v.trim().to_ascii_lowercase();
        if matches!(v.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    cfg!(debug_assertions)
}

/// 解析日志目录：优先级 `DELTA_OBSERVER_LOG_DIR` > 向上查找 `Cargo.toml` 的项目根 `/logs` > exe 父目录 `/logs`。
fn log_dir() -> PathBuf {
    if let Ok(d) = std::env::var("DELTA_OBSERVER_LOG_DIR") {
        let p = PathBuf::from(d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    if let Some(exe) = std::env::current_exe().ok() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            if d.join("Cargo.toml").is_file() {
                return d.join("logs");
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
        if let Some(parent) = exe.parent() {
            return parent.join("logs");
        }
    }
    PathBuf::from("logs")
}

/// 启动时调用一次：在日志目录创建 `run.log`；开发模式截断清空，生产模式追加。
///
/// 必须在 `main()` 最开头调用（先于任何业务日志），开发模式下它即完成「清空再写入」的第一件事。
pub fn init() {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("run.log");
    let f = if dev_mode() {
        // 开发模式：File::create 以截断模式打开 —— 清空上一轮累积内容，避免跨班次无界增长。
        std::fs::File::create(&path).ok()
    } else {
        // 生产模式：创建（若不存在）+ 追加，保留历史日志。
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
    };
    let _ = HANDLE.get_or_init(|| Mutex::new(f));
    event(
        "CORE",
        "init",
        &format!(
            "\"mode\":\"{}\",\"path\":\"{}\"",
            if dev_mode() { "dev" } else { "prod" },
            path.display()
        ),
    );
}

/// 当前 Unix 毫秒时间戳（用于日志 `t` 字段，单调、易排序）。
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 最小化 JSON 字符串转义（仅处理 `"` `\` 与 `\n \r \t`，其余原样，UTF-8 中文保留）。
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            _ => o.push(c),
        }
    }
    o
}

/// 行式日志（兼容既有调用）：自动包成 JSON `note` 事件。
pub fn write(msg: &str) {
    emit("INFO", "", "note", &format!("\"msg\":\"{}\"", esc(msg)));
}

/// 结构化事件日志：`fields` 为**不含外层花括号**的 JSON 对象体（调用方负责内嵌字段合法），
/// 例如 `event("BUY","decide","\"scene\":\"Exchangeable\",\"qty\":31")`。
pub fn event(module: &str, evt: &str, fields: &str) {
    emit("INFO", module, evt, fields);
}

/// 告警级事件（同 `event`，但级别为 `WARN`）。预留公共 API，暂未在各模块广泛使用。
#[allow(dead_code)]
pub fn warn(module: &str, evt: &str, fields: &str) {
    emit("WARN", module, evt, fields);
}

/// 内部：拼装并写入单行 JSON 日志。
fn emit(level: &str, module: &str, evt: &str, fields: &str) {
    if let Some(g) = HANDLE.get() {
        if let Ok(mut g) = g.lock() {
            if let Some(f) = g.as_mut() {
                let line = if fields.is_empty() {
                    format!(
                        "{{\"t\":{},\"l\":\"{}\",\"m\":\"{}\",\"e\":\"{}\"}}",
                        now_ms(),
                        level,
                        esc(module),
                        esc(evt)
                    )
                } else {
                    format!(
                        "{{\"t\":{},\"l\":\"{}\",\"m\":\"{}\",\"e\":\"{}\",{}}}",
                        now_ms(),
                        level,
                        esc(module),
                        esc(evt),
                        fields
                    )
                };
                let _ = writeln!(f, "{}", line);
                let _ = f.flush();
            }
        }
    }
}

/// 将 `[x,y,w,h]` 格式化为 JSON 数组串（用于结构化字段，如 `\"buy\":[1482,829,297,61]`）。
pub fn fmt_rect(r: [i32; 4]) -> String {
    format!(
        "[{},{},{},{}]",
        r[0], r[1], r[2], r[3]
    )
}

/// 将 `Option<[x,y,w,h]>` 格式化为 JSON（`[x,y,w,h]` 或 `null`），便于结构化字段直接内嵌。
pub fn fmt_opt_rect(o: Option<[i32; 4]>) -> String {
    match o {
        Some(r) => fmt_rect(r),
        None => "null".to_string(),
    }
}
