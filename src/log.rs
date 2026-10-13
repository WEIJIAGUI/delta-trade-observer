//! 轻量运行期日志：用于购买阶段三层闸门（L1 调用侧 / L2 按钮检测 / L3 数量识别）排查。
//!
//! 设计约束（用户要求）：
//! - 每次程序启动调用 [`init`] 以**截断模式**重建日志文件，清空上一轮的累积内容；
//! - 运行期只记录关键状态与结果（`write` 一行），避免冗余刷屏；
//! - 未调用 [`init`]（如单元测试）时为 no-op，不落盘、不影响测试。

use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// 日志句柄：初始化后持有文件，未初始化为 `None`（no-op）。
static HANDLE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

/// 启动时调用一次：在可执行文件同目录创建 `observe.log`（截断清空旧内容）。
pub fn init() {
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("observe.log")))
        .unwrap_or_else(|| std::path::PathBuf::from("observe.log"));
    // File::create 以截断模式打开：清空旧日志，避免跨班次无界累积。
    let f = std::fs::File::create(path).ok();
    let _ = HANDLE.get_or_init(|| Mutex::new(f));
}

/// 仅记录一行关键状态/结果。未初始化时静默 no-op。
pub fn write(msg: &str) {
    if let Some(g) = HANDLE.get() {
        if let Ok(mut g) = g.lock() {
            if let Some(f) = g.as_mut() {
                let _ = writeln!(f, "[{}] {}", stamp(), msg);
                let _ = f.flush();
            }
        }
    }
}

/// 粗略时间戳（UTC 当日时分秒），仅用于日志可读性，不引入额外 crate。
fn stamp() -> String {
    use std::time::SystemTime;
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() % 86400)
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
