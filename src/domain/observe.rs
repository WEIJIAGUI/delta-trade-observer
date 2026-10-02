//! 观察任务：循环检索各口径 → 截图 → 逐单元格 OCR 识别名称/价格 → 结构化持久化。
//!
//! 流程（与 `src/config.rs` 网格坐标契约一致）：
//! 1. 定位并聚焦游戏窗口；
//! 2. 每轮（cycle）遍历 `config.rs` 全部口径：模拟人类操作，点击检索栏 → 清空 →
//!    逐字符输入该口径名 → 回车，触发游戏列出该口径全部子弹；
//! 3. 截取「客户区（无标题栏）」整屏，对每枚子弹按标定矩形**逐单元格裁剪**：
//!    名称格走 OCR（`zh-CN` 引擎，先经 ocr 模块「近白二值化 + 4× 放大」预处理）；
//!    价格格走**逐位神经网络识别**（见 `digit` → `nn_classifier`），不再经过 OCR；
//! 4. 价格由 4 个数位（**千 / 百 / 十 / 个**）各自识别出的单字符拼接而成
//!    （逐位链路天然不会引入千分位分隔符，且规避整框 OCR 把彩色价格整片吞掉）；
//! 5. 结果写入 `save_path/records.jsonl`（口径、子弹名、识别名、价格、时间戳），
//!    供「价格可视化」Tab 读取展示；非法价格结构另行追加 `save_path/anomalies.log`；
//! 6. 每轮结束后等待 `interval_min` 分钟（控制面板可调，默认 30）再开始下一轮，
//!    F9 随时优雅中止。
//!
//! 坐标缩放：`config.rs` 常量量自标定视口（`CALIB_VIEWPORT_W`），本模块在运行时用
//! `config::Scale` 按实际客户区宽度映射，适配任意窗口分辨率。

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::System::SystemInformation::GetSystemTime;

use crate::config::{CALIB_VIEWPORT_W, Config, GAME_KEYWORD, Scale, SEARCH_BAR};
use crate::digit::PriceRec;
use crate::recognition::BulletRecognition;
use crate::win;
use crate::screenshot;

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
    /// 完成：是否成功（至少记录到一条）、毛玻璃通知文字。
    Done {
        ok: bool,
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

/// 识别记录文件路径：观察端写入、价格可视化端读取的唯一数据契约（save_path 根目录）。
pub fn records_file(save_path: &str) -> String {
    format!("{save_path}/records.jsonl")
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

/// 输入口径名后等待游戏刷新列表的时间。
///
/// 检索是异步的：回车后列表需要一两帧才重排，立即截图会拍到上一口径的残留。
/// 取 700ms 是实测下来的稳定值（再短偶发拍到旧列表）。
const POST_SEARCH_SETTLE_MS: u64 = 700;

/// 允许的连续截图失败次数，超过则判定窗口失效并终止观察。
///
/// 单次失败可能是偶发（窗口正在重绘），连续失败则说明窗口已关闭 / 最小化，
/// 继续循环只会刷屏报错，故到此为止。
const MAX_CAPTURE_FAIL_STREAK: usize = 3;

/// 当前 UTC 时间戳（ISO 8601，秒级，形如 `2024-09-27T08:42:10Z`）。
fn iso_now() -> String {
    let st = unsafe { GetSystemTime() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

/// 构造一条 JSONL 记录行（口径 / 子弹名 / 识别名 / 价格 / 时间戳）。
fn build_record_line(cal: &str, bullet: &str, name: &str, price: &str, ts_ms: u64) -> String {
    format!(
        "{{\"caliber\":\"{}\",\"bullet\":\"{}\",\"ocr_name\":\"{}\",\"price\":\"{}\",\"ts_ms\":{},\"timestamp\":\"{}\"}}\n",
        jstr(cal),
        jstr(bullet),
        jstr(name),
        jstr(price),
        ts_ms,
        iso_now(),
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
    // 是否把每次识别出的数字裁剪写入 `nn-classifier/dataset/<label>/` 用于神经网络训练。
    // 默认由 UI 复选框控制（默认关闭），仅勾选时才落盘，避免无谓 IO。
    // `digits_dir` 为用户在 UI 指定的输出目录（开启保留功能时输入），不做分类处理。
    save_digits: bool,
    digits_dir: String,
) {
    // 1) 定位游戏窗口
    let hwnd = match win::find_window_by_keyword(GAME_KEYWORD) {
        Some(h) => h,
        None => {
            let _ = tx.send(ObserveMsg::Done {
            ok: false,
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
            toast: "✗ 口径配置为空".into(),
        });
        return;
    }

    // 持久化：识别记录直接写入 save_path 根目录（不再使用 prices/ 子目录）。
    let records_path = records_file(&save_path);
    if std::fs::create_dir_all(&save_path).is_err() {
        let _ = tx.send(ObserveMsg::Done {
            ok: false,
            toast: "✗ 无法创建保存目录".into(),
        });
        return;
    }

    // 数字裁剪输出目录：仅在勾选「保存数字」且路径非空时创建，统一落盘到用户指定目录（不做分类）。
    let digits_dir: Option<String> = if save_digits && !digits_dir.trim().is_empty() {
        let _ = std::fs::create_dir_all(&digits_dir);
        Some(digits_dir.clone())
    } else {
        None
    };
    // 数字裁剪文件序号（保证文件名唯一、不互相覆盖）。
    let mut digit_seq: u64 = 0;

    // —— 即开即用组件：观察开始时构造一次，整个观察周期复用 ——
    // 截图器（客户区原点，无黑边）与共享子弹识别器（名称 OCR + 价格逐位神经网络识别，
    // 见 `recognition` 模块；「OCR 测试」Tab 复用同一逻辑）。
    let capturer = screenshot::ClientCapturer::new(hwnd);
    let mut bullet_rec = BulletRecognition::new();

    // 标定坐标 → 实际客户区的映射（每轮刷新，适配窗口尺寸变化）。
    let mut scale = Scale::for_client_width(
        {
            let cw = capturer.client_width();
            if cw > 0 { cw } else { CALIB_VIEWPORT_W }
        },
    );

    let mut total_done = 0usize;
    let mut any_ok = false;
    let mut cycle = 1usize;

    'cycles: loop {
        {
            let cw = capturer.client_width();
            if cw > 0 {
                scale = Scale::for_client_width(cw);
            }
        }
        let mut fail_streak = 0usize;

        for (ci, cal) in calibers.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                break 'cycles;
            }
            // 2) 模拟人类操作：点击检索栏 → 清空 → 输入口径名 → 回车
            win::focus_window(hwnd);
            win::type_into_search(hwnd, cal, scale.rect(SEARCH_BAR));
            // 等待游戏刷新列表
            std::thread::sleep(Duration::from_millis(POST_SEARCH_SETTLE_MS));

            // 3) 截取客户区整屏（原点 = 客户区左上角，无黑边）
            let frame = match capturer.capture() {
                Ok(f) => f,
                Err(_) => {
                    // 连续失败保护：窗口失效/最小化等情况避免无限报错循环。
                    fail_streak += 1;
                    if fail_streak >= MAX_CAPTURE_FAIL_STREAK {
                        let _ = tx.send(ObserveMsg::Done {
                            ok: any_ok,
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

            // 4) 逐单元格：名称 + 价格统一走共享识别器（OCR 测试 Tab 复用同一逻辑，见 `recognition` 模块）
            for (bullet, rects) in bullets.iter() {
                if stop.load(Ordering::Relaxed) {
                    break 'cycles;
                }
                // 用 *_detail 变体：识别结果与 OCR 测试 Tab 完全一致，同时返回每位数位的
                // 裁剪图与匹配详情，供「保存数字用于训练」落盘（见下文保存块）。
                let (rec, details) = bullet_rec.recognize_bullet_detail(&frame, rects);
                let name_raw = rec.name;
                // 价格走逐位数字链路（千/百/十/个 4 框分别识别一位数字后拼接），
                // 规避通用 / 整框 OCR 把彩色价格文字整片抹成背景或粘连错识。
                // 返回 `PriceRec`：合法给价格字符串，全空给 Empty，断层（如百位缺失）给 Invalid。
                let price_rec = rec.price;
                let price: String;
                let mut anomaly_reason: Option<&'static str> = None;
                let mut anomaly_partial: String = String::new();
                match price_rec {
                    PriceRec::Price(s) => {
                        price = s;
                    }
                    PriceRec::Empty => {
                        price = String::new();
                    }
                    PriceRec::Invalid { reason, partial } => {
                        // 非法结构：价格置空，并写入 anomaly 日志供后续排查/重标定。
                        price = String::new();
                        anomaly_reason = Some(reason);
                        anomaly_partial = partial;
                    }
                }

                // （可选）保留识别出的数字裁剪：勾选「保存数字」时，把每个数位裁剪
                // 统一写入用户指定的单一目录（不做分类），文件名含序号与识别标签
                // （label 为神经网络识别结果 0-9，未识别 <90% 则为 x，仅供人工校正参考）。
                if let Some(ddir) = &digits_dir {
                    for d in &details {
                        if let Some(dt) = d {
                            let label = dt
                                .recognized
                                .filter(|c| c.is_ascii_digit())
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "x".to_string());
                            let _ = screenshot::save_rgba_png(
                                ddir,
                                &format!("d{digit_seq:08}_{label}.png"),
                                &dt.crop,
                                dt.crop_w as u32,
                                dt.crop_h as u32,
                            );
                            digit_seq += 1;
                        }
                    }
                }

                // 非法价格结构（如百位缺失的断层）：价格已置空，此处追加到 anomalies.log，
                // 便于事后区分「坐标错位」还是「识别漏位」，指导重标定或调阈值。
                if let Some(reason) = anomaly_reason {
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(format!("{save_path}/anomalies.log"))
                        .and_then(|mut f| {
                            f.write_all(
                                format!(
                                    "{} {} {}: {} [{}]\n",
                                    iso_now(),
                                    sanitize(cal),
                                    sanitize(bullet),
                                    reason,
                                    anomaly_partial,
                                )
                                .as_bytes(),
                            )
                        });
                }

                // 5) 结构化写入 jsonl：口径 / 子弹名 / 识别名 / 价格 / 时间戳
                let ts_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let line = build_record_line(cal, bullet, &name_raw, &price, ts_ms);
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
        toast: format!("✓ 已记录 {total_done} 条价格"),
    });
}
