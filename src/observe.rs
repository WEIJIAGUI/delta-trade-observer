//! 观察任务：循环检索各口径 → 截图 → 逐单元格 OCR 识别名称/价格 → 结构化持久化。
//!
//! 流程（与 `src/config.rs` 网格坐标契约一致）：
//! 1. 定位并聚焦游戏窗口；
//! 2. 每轮（cycle）遍历 `config.rs` 全部口径：模拟人类操作，点击检索栏 → 清空 →
//!    逐字符输入该口径名 → 回车，触发游戏列出该口径全部子弹；
//! 3. 截取「客户区（无标题栏）」整屏，对每枚子弹按标定矩形**逐单元格裁剪**：
//!    名称格用 `zh-CN` 引擎、价格格用 `en-US` 引擎分别识别
//!    （先经 ocr 模块「近白二值化 + 4× 放大」预处理，针对价格准确率难点）；
//! 4. 价格经 `extract_price` 抽取纯数字——实测截图中 `1.920` 的 `.` 是千分位
//!    分隔符，一律剔除；
//! 5. 结果写入 `save_path/prices/records.jsonl`（口径、子弹名、识别名、价格、时间戳），
//!    并落盘调试裁剪图 / 二值化图 / OCR 原文，便于坐标校准与识别调参；
//! 6. 每轮结束后等待 `interval_min` 分钟（控制面板可调，默认 30）再开始下一轮，
//!    F9 随时优雅中止。
//!
//! 坐标缩放：`config.rs` 常量按 1080px 宽客户区标定（`CALIB_REF_W`），本模块在
//! 运行时按实际客户区宽度等比缩放（`scale_rect`），适配任意窗口分辨率。

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::System::SystemInformation::GetSystemTime;

use crate::config::{CALIB_REF_W, Config, GAME_KEYWORD, SEARCH_BAR};
use crate::digit;
use crate::ocr;
use crate::win;

/// 观察任务回传给主线程的消息。
pub enum ObserveMsg {
    /// 进度：第几轮、当前口径名、口径序号（0-based）、口径总数、已识别子弹数。
    Progress {
        cycle: usize,
        caliber: String,
        idx: usize,
        total: usize,
        done_bullets: usize,
    },
    /// 轮间等待：本轮已完成，`remain_sec` 秒后开始下一轮（每秒回传一次）。
    Waiting { cycle: usize, remain_sec: u64 },
    /// 完成：是否成功（至少记录到一条）、瞬时提示、毛玻璃通知文字。
    Done {
        ok: bool,
        flash: String,
        toast: String,
    },
}

/// 把字符串清洗为合法文件名片段：替换 Windows 非法字符为 `_`。
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// JSON 字符串转义（仅转义必要的控制字符与引号/反斜杠，其余原样）。
fn jstr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        let u = c as u32;
        match u {
            0x22 => out.push_str("\\\""),
            0x5C => out.push_str("\\\\"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// 把标定参考分辨率（宽 `CALIB_REF_W`）下的矩形等比缩放到实际客户区宽度。
pub fn scale_rect(r: [i32; 4], client_w: i32) -> [i32; 4] {
    let s = client_w as f32 / CALIB_REF_W as f32;
    [
        (r[0] as f32 * s).round() as i32,
        (r[1] as f32 * s).round() as i32,
        ((r[2] as f32 * s).round() as i32).max(1),
        ((r[3] as f32 * s).round() as i32).max(1),
    ]
}

/// 裁剪矩形区域并 OCR，返回按阅读顺序（同行按 x、行按 y）以空格拼接的识别文本。
pub fn ocr_crop(rgba: &[u8], w: i32, h: i32, rect: [i32; 4], tag: &str) -> String {
    let Some((img, iw, ih)) = win::crop_region(rgba, w, h, rect) else {
        return String::new();
    };
    let mut words = ocr::recognize_words(&img, iw, ih, tag);
    words.sort_by(|a, b| (a.y / 8).cmp(&(b.y / 8)).then(a.x.cmp(&b.x)));
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 从价格区域 OCR 文本中抽取价格数字。
///
/// 难点与对策（基于 2026-09-27 实测截图）：
/// - 价格形如 `440` / `1.920`：`.` 是**千分位分隔符**而非小数点，须整体剔除；
/// - OCR 可能混入单位/图标噪声字符：取最长 `[0-9., ]` 连续段后仅保留数字。
/// 从价格区域 OCR 文本中抽取价格数字（对外复用接口）。
pub fn extract_price(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut best_start = 0usize;
    let mut best_len = 0usize;
    let mut cur_start: Option<usize> = None;

    for (i, &b) in bytes.iter().enumerate() {
        let ok = (b >= b'0' && b <= b'9') || b == b',' || b == b'.' || b == b' ';
        if ok {
            if cur_start.is_none() {
                cur_start = Some(i);
            }
        } else if let Some(st) = cur_start {
            let len = i - st;
            if len > best_len {
                best_len = len;
                best_start = st;
            }
            cur_start = None;
        }
    }
    if let Some(st) = cur_start {
        let len = bytes.len() - st;
        if len > best_len {
            best_len = len;
            best_start = st;
        }
    }
    if best_len == 0 {
        return String::new();
    }
    // 千分位分隔符（',' '.' ' '）一律剔除，仅保留数字。
    s[best_start..best_start + best_len]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect()
}

/// 当前 UTC 时间戳（ISO 8601，秒级，形如 `2024-09-27T08:42:10Z`）。
fn iso_now() -> String {
    let st = unsafe { GetSystemTime() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

/// 观察主流程（后台线程执行，通过 `tx` 回传进度/完成，通过 `stop` 支持优雅中止）。
///
/// `interval_min`：轮间等待分钟数（每轮遍历全部口径结束后等待该时长再开始下一轮）。
pub fn run_observation(
    save_path: String,
    info: Arc<Config>,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<ObserveMsg>,
    interval_min: f64,
) {
    // 1) 定位游戏窗口
    let hwnd = match win::find_window_by_keyword(GAME_KEYWORD) {
        Some(h) => h,
        None => {
            let _ = tx.send(ObserveMsg::Done {
                ok: false,
                flash: format!("未检测到「{GAME_KEYWORD}」窗口，无法开始观察"),
                toast: format!("✗ 未检测到「{GAME_KEYWORD}」窗口"),
            });
            return;
        }
    };
    win::focus_window(hwnd);

    let calibers: Vec<String> = info.keys().cloned().collect();
    let total_cal = calibers.len();
    if total_cal == 0 {
        let _ = tx.send(ObserveMsg::Done {
            ok: false,
            flash: "口径配置为空，无法观察".into(),
            toast: "✗ 口径配置为空".into(),
        });
        return;
    }

    // 持久化目录：save_path/prices
    let prices_dir = format!("{save_path}/prices");
    let records_path = format!("{prices_dir}/records.jsonl");
    if std::fs::create_dir_all(&prices_dir).is_err() {
        let _ = tx.send(ObserveMsg::Done {
            ok: false,
            flash: format!("无法创建目录：{prices_dir}"),
            toast: "✗ 无法创建保存目录".into(),
        });
        return;
    }

    // 标定坐标的缩放基准：实际客户区宽度（每轮刷新，适配窗口尺寸变化）。
    let mut cw = win::client_crop_rect(hwnd)
        .map(|r| r[2])
        .filter(|v| *v > 0)
        .unwrap_or(CALIB_REF_W);

    let mut total_done = 0usize;
    let mut any_ok = false;
    let mut cycle = 1usize;

    'cycles: loop {
        if let Some(r) = win::client_crop_rect(hwnd) {
            if r[2] > 0 {
                cw = r[2];
            }
        }
        let mut fail_streak = 0usize;

        for (ci, cal) in calibers.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                break 'cycles;
            }
            // 2) 模拟人类操作：点击检索栏 → 清空 → 输入口径名 → 回车
            win::focus_window(hwnd);
            win::type_into_search(hwnd, cal, scale_rect(SEARCH_BAR, cw));
            // 等待游戏刷新列表
            std::thread::sleep(Duration::from_millis(700));

            // 3) 截取客户区整屏
            let (rgba, w, h) = match win::capture_client_rgba(hwnd) {
                Ok(v) => v,
                Err(e) => {
                    // 连续失败保护：窗口失效/最小化等情况避免无限报错循环。
                    fail_streak += 1;
                    if fail_streak >= 3 {
                        let _ = tx.send(ObserveMsg::Done {
                            ok: any_ok,
                            flash: format!("连续 3 次截图失败（{e}），观察终止"),
                            toast: "✗ 截图连续失败，观察终止".into(),
                        });
                        return;
                    }
                    continue;
                }
            };
            fail_streak = 0;

            let bullets = match info.get(cal) {
                Some(b) => b,
                None => continue,
            };
            let cal_dir = format!("{prices_dir}/{}", sanitize(cal));

            // 4) 逐单元格裁剪 + 双引擎 OCR（名称 zh-CN / 价格 en-US）
            for (bullet, rects) in bullets.iter() {
                if stop.load(Ordering::Relaxed) {
                    break 'cycles;
                }
                let nrect = scale_rect(rects.name, cw);
                let prect = scale_rect(rects.price, cw);
                let pdigits = rects.price_digits.map(|r| scale_rect(r, cw));
                let name_raw = ocr_crop(&rgba, w, h, nrect, "zh-CN");
                // 价格走逐位数字链路（digit：万/千/百/十/个 5 框分别识别一位数字后拼接），
                // 避免通用 / 整框 OCR 把彩色价格文字整片抹成背景或粘连错识。
                let price = digit::recognize_price_digits(&rgba, w, h, pdigits);

                // 调试：原始裁剪图 + 价格二值化放大图 + OCR 原文
                let sb = sanitize(bullet);
                if let Some((img, iw, ih)) = win::crop_region(&rgba, w, h, nrect) {
                    let _ = win::save_rgba_png(
                        &cal_dir,
                        &format!("{sb}.name.png"),
                        &img,
                        iw as u32,
                        ih as u32,
                    );
                }
                if let Some((img, iw, ih)) = win::crop_region(&rgba, w, h, prect) {
                    let _ = win::save_rgba_png(
                        &cal_dir,
                        &format!("{sb}.price.png"),
                        &img,
                        iw as u32,
                        ih as u32,
                    );
                    let (pimg, pw, ph) = ocr::preprocessed(&img, iw, ih);
                    let _ = win::save_rgba_png(
                        &cal_dir,
                        &format!("{sb}.price.bin.png"),
                        &pimg,
                        pw as u32,
                        ph as u32,
                    );
                }
                let _ = std::fs::write(
                    format!("{cal_dir}/{sb}.ocr.txt"),
                    format!("name: {name_raw}\nprice: {price}\n"),
                );

                // 5) 结构化写入 jsonl：口径 / 子弹名 / 识别名 / 价格 / 时间戳
                let ts_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let line = format!(
                    "{{\"caliber\":\"{}\",\"bullet\":\"{}\",\"ocr_name\":\"{}\",\"price\":\"{}\",\"ts_ms\":{},\"timestamp\":\"{}\"}}\n",
                    jstr(cal),
                    jstr(bullet),
                    jstr(&name_raw),
                    jstr(&price),
                    ts_ms,
                    iso_now(),
                );
                if std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&records_path)
                    .and_then(|mut f| f.write_all(line.as_bytes()))
                    .is_ok()
                {
                    total_done += 1;
                    any_ok = true;
                }

                let _ = tx.send(ObserveMsg::Progress {
                    cycle,
                    caliber: cal.clone(),
                    idx: ci,
                    total: total_cal,
                    done_bullets: total_done,
                });
            }
        }

        if stop.load(Ordering::Relaxed) {
            break 'cycles;
        }

        // 6) 轮间等待：每秒检查一次中止信号，并回传剩余秒数供悬浮面板显示。
        let wait_secs = (interval_min.max(0.0) * 60.0).round() as u64;
        for remain in (1..=wait_secs).rev() {
            if stop.load(Ordering::Relaxed) {
                break 'cycles;
            }
            let _ = tx.send(ObserveMsg::Waiting { cycle, remain_sec: remain });
            std::thread::sleep(Duration::from_secs(1));
        }
        cycle += 1;
    }

    let _ = tx.send(ObserveMsg::Done {
        ok: any_ok,
        flash: format!("观察结束（第 {cycle} 轮中止）：共记录 {total_done} 条价格"),
        toast: format!("✓ 已记录 {total_done} 条价格"),
    });
}
