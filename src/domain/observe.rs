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
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::System::SystemInformation::GetSystemTime;

use crate::config::{CALIB_VIEWPORT_W, Config, GAME_KEYWORD, Rect, Scale, SEARCH_BAR};
use crate::digit::PriceRec;
use crate::recognition::BulletRecognition;
use crate::buy;
use crate::win;
use crate::screenshot;
use crate::Tour;

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
    /// 购买阶段重验证识别到的价格：主线程直接渲染到毛玻璃面板（仅显示数字）。
    BuyNotice { price: u32 },
    /// 购买界面场景判定结果（按钮检测器据「购买」按钮位置判定）：主线程展示于毛玻璃面板。
    BuySceneNotice { label: String },
    /// 购买阶段实时识别到的「所选子弹数量」（减/加调整过程中的当前值）：毛玻璃面板实时展示。
    DetectedQty { qty: Option<u32> },
    /// 购买结果 toast 归类（点击购买后监听游戏顶部弹窗）：
    /// 「正常购买成功 / 低价购买成功 / 未捕获到成功提示」，毛玻璃面板展示，并按场景记入核查表。
    BuyResultNotice {
        label: String,
        /// 本次购买所属界面场景（来自 `Proceed` 决策）；`None` 表示场景未判定（实际不会触发购买）。
        scene: Option<crate::config::BuyScene>,
    },
    /// 进入新一颗子弹的购买判定前清空毛玻璃上的购买相关展示（价格/场景/数量），避免残留到扫描阶段。
    BuyCleared,
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

/// 解析口径处理顺序（纯函数，便于单测锁定不变量）。
///
/// 规则：以 `preferred`（UI 拖拽调整的口径顺序）为基准，剔除校准配置 `info` 已不存在的口径，
/// 再把 `info` 中存在但未被列入的口径按名称排序补到末尾。空 `preferred` 时返回「口径名排序」全量，
/// 等价于旧版默认顺序，向后兼容。
fn resolve_caliber_order(preferred: &[String], info: &Config) -> Vec<String> {
    let mut out: Vec<String> = preferred
        .iter()
        .filter(|c| info.contains_key(*c))
        .cloned()
        .collect();
    let mut missing: Vec<String> = info
        .keys()
        .filter(|c| !preferred.contains(c))
        .cloned()
        .collect();
    missing.sort();
    out.extend(missing);
    out
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
    // 是否把扫描阶段识别出的数字裁剪写入 `{digits_dir}/scan/` 用于神经网络训练（默认关闭）。
    save_scan_digits: bool,
    // 是否把购买阶段识别出的数字裁剪写入 `{digits_dir}/buy/` 用于神经网络训练（默认关闭）。
    save_buy_digits: bool,
    // 数字裁剪输出根目录：扫描阶段 -> scan/ 子目录、购买阶段 -> buy/ 子目录，各自独立受对应开关控制。
    digits_dir: String,
    // 是否购买：勾选后，价格低于对应子弹「最低接受价格」时自动执行点击+ESC 恢复。
    buy_enabled: bool,
    // 各 (口径, 子弹) 的最低接受价格（u32，哈夫币）。仅含非空且可解析者。
    buy_thresholds: HashMap<(String, String), u32>,
    // 是否保存购买界面的完整截图（PNG）。默认关闭，与数字裁剪保存（scan/buy）完全独立。
    save_buy_ui: bool,
    // 购买界面截图输出目录（独立设置，不共享 digits_dir）。
    buy_ui_save_path: String,
    // 面板设定的「目标购买数量」：重验证价格仍低于阈值时，购买阶段据当前识别数点击减/加调到该值。
    buy_qty: u32,
    // 购买口径处理顺序（口径级）：UI 拖拽调整后的口径顺序；运行期与校准配置求交集并补尾，
    // 保证「配置新增的口径不会漏处理、已移除的口径不会误检索」。空列表回退到口径名排序。
    buy_caliber_order: Vec<String>,
    // 导游模式协调器：每口径检索前展示计划并等待回车确认；关闭态（`Tour::disabled`）下为无操作。
    tour: Tour,
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

    // 口径处理顺序：优先采用 UI 拖拽调整的 `buy_caliber_order`（与校准配置求交集，剔除已移除口径），
    // 剩余未列入的口径（如校准配置新增）按名称排序补到末尾，保证「新增不漏、移除不误」。
    // 空顺序列表时 `ordered` 为空、全部走补尾分支，等价于旧版「口径名排序」，向后兼容。
    let calibers = resolve_caliber_order(&buy_caliber_order, &info);
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

    // 数字裁剪输出根目录：扫描阶段 -> {根}/scan/，购买阶段 -> {根}/buy/，各自独立受对应开关控制。
    let digits_root = digits_dir.trim().to_string();
    let scan_dir: Option<String> = if save_scan_digits && !digits_root.is_empty() {
        let d = format!("{digits_root}/scan");
        let _ = std::fs::create_dir_all(&d);
        Some(d)
    } else {
        None
    };
    let buy_dir: Option<String> = if save_buy_digits && !digits_root.is_empty() {
        let d = format!("{digits_root}/buy");
        let _ = std::fs::create_dir_all(&d);
        Some(d)
    } else {
        None
    };
    // 购买界面截图输出目录：独立于数字裁剪根目录（digits_dir），由自身开关与目录控制。
    let buy_ui_dir: Option<String> = if save_buy_ui && !buy_ui_save_path.trim().is_empty() {
        let d = buy_ui_save_path.trim().to_string();
        let _ = std::fs::create_dir_all(&d);
        Some(d)
    } else {
        None
    };
    // 数字裁剪文件序号（保证文件名唯一、不互相覆盖）。
    let mut digit_seq: u64 = 0;

    // 购买目标数量钳制到分类器可识别区间 1..=200（NUM_CLASSES），避免 UI 越界值
    // （理论上已被 UI 限制）导致数量永远无法对齐、盲目重试。钳制在此一处完成，下游无需再判。
    let buy_qty = buy_qty.clamp(1, crate::qty_classifier::NUM_CLASSES as u32);

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
            // 导游模式：进入该口径检索前先展示计划并等待回车确认（关闭态为无操作）。
            tour.ask(&format!("【计划】开始检索「{cal}」口径子弹"));
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
            // 固定子弹扫描顺序：按名称条坐标 (y 行 → x 列) 行优先排序，
            // 即「从上到下、同行从左到右」逐格识别，对应交易行 3×3 网格的阅读顺序。
            let mut bullet_order: Vec<_> = bullets.iter().collect();
            bullet_order.sort_by(|a, b| {
                let ra = &a.1.name;
                let rb = &b.1.name;
                ra[1].cmp(&rb[1]).then(ra[0].cmp(&rb[0]))
            });

            // 4) 逐单元格：名称 + 价格统一走共享识别器（OCR 测试 Tab 复用同一逻辑，见 `recognition` 模块）
            for (bullet, rects) in bullet_order {
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
                if let Some(ddir) = &scan_dir {
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

                // 「是否购买」阶段（与查看物理分离，见 `buy` 模块）：扫描价低于阈值时进入购买阶段，
                // 先导航打开购买界面（偏移点击价格），就绪轮询等待界面稳定后重验证当前价格，再决定。
                // 决策与收尾（弹提示条 / ESC）全部下沉到 `buy` 模块与 `win` 平台层。
                if buy_enabled {
                    // 进入本轮子弹的购买判定前，清空上一颗子弹残留的购买展示（价格/场景/数量）。
                    let _ = tx.send(ObserveMsg::BuyCleared);
                    let th = buy_thresholds.get(&(cal.clone(), bullet.clone())).copied();
                    match                         buy::verify_then_decide(
                            hwnd,
                            &scale,
                            rects.buy_area,
                            &price,
                            th,
                            cal,
                            bullet,
                            buy_dir.clone(),
                            buy_ui_dir.clone(),
                            buy_qty,
                            stop.clone(),
                            &tx,
                            tour.clone(),
                        ) {
                        buy::BuyDecision::Proceed { price, scene, qty, buy, .. } => {
                            // 购买界面场景判定：渲染到毛玻璃面板（仅展示，不弹提示框）。
                            if let Some(s) = scene {
                                let _ = tx.send(ObserveMsg::BuySceneNotice {
                                    label: s.label().to_string(),
                                });
                            }
                            // 购买阶段重验证识别到的价格，直接渲染到毛玻璃面板（仅显示数字，不弹提示框）。
                            let _ = tx.send(ObserveMsg::BuyNotice { price });
                            // 此时数量已经 `buy.rs` 复识别确认 == 目标值（Proceed 的唯一产出条件），
                            // 此处才触发「购买」按钮点击——全流程中唯一允许购买触发的位置。
                            crate::log::write(&format!(
                                "[BUY] 数量已确认={qty}，触发购买按钮点击（price={price}）"
                            ));
                            win::click_client_rect(hwnd, buy);
                            // 点击后监听游戏顶部弹出的结果 toast（~3s），归类两种购买成功
                            // （正常 / 低价成交）。失败类 toast 按需求不处理；未捕获到成功
                            // toast（可能点击静默失败）仅记日志，不影响主流程，随后照常 ESC 返回扫描。
                            match crate::toast::monitor_after_buy(hwnd, &stop) {
                                Some(kind) => {
                                    crate::log::write(&format!("[BUY] 购买结果：{}", kind.label()));
                                    let _ = tx.send(ObserveMsg::BuyResultNotice {
                                        label: kind.label().to_string(),
                                        scene,
                                    });
                                    // 导游：据判定结果提醒玩家本次是否为低价购买（关闭态为无操作）。
                                    let tip = match kind {
                                        crate::toast::BuyToastKind::LowerPriceSuccess =>
                                            "【购买结果】本次为「低价购买成功」（已用更低价格成交），按 Enter 继续",
                                        crate::toast::BuyToastKind::NormalSuccess =>
                                            "【购买结果】本次为「正常购买成功」，按 Enter 继续",
                                    };
                                    tour.ask(tip);
                                }
                                None => {
                                    crate::log::write(
                                        "[BUY][WARN] 未捕获到购买成功 toast（点击可能未生效），本次购买结果未知",
                                    );
                                    let _ = tx.send(ObserveMsg::BuyResultNotice {
                                        label: "未捕获到成功提示".into(),
                                        scene,
                                    });
                                    tour.ask("【购买结果】未捕获到成功提示（点击可能未生效），按 Enter 继续");
                                }
                            }
                            win::send_esc();
                            win::focus_window(hwnd);
                        }
                        buy::BuyDecision::Abort { scene } => {
                            // 购买界面场景判定：渲染到毛玻璃面板（仅展示，不弹提示框）。
                            if let Some(s) = scene {
                                let _ = tx.send(ObserveMsg::BuySceneNotice {
                                    label: s.label().to_string(),
                                });
                            }
                            // 重验证后价格未低于阈值（或已回升）→ ESC 返回扫描，不购买。
                            win::send_esc();
                            win::focus_window(hwnd);
                        }
                        buy::BuyDecision::Skip => {}
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BulletRects, Config};

    fn rect() -> Rect {
        [0, 0, 10, 10]
    }

    /// 构造仅含占位坐标的测试配置（坐标无关，仅用于口径/子弹枚举）。
    fn make_config(calibers: &[&str]) -> Config {
        let mut c = Config::new();
        for cal in calibers {
            let mut inner = std::collections::HashMap::new();
            inner.insert(
                "b1".to_string(),
                BulletRects {
                    name: rect(),
                    ones: rect(),
                    tens: rect(),
                    hundreds: rect(),
                    thousands: rect(),
                    buy_area: rect(),
                },
            );
            c.insert(cal.to_string(), inner);
        }
        c
    }

    #[test]
    fn resolve_order_keeps_preferred_and_appends_missing_sorted() {
        let info = make_config(&["A", "B", "C", "D"]);
        // 用户顺序仅列 B、A（且 B 先于 A）；未列的 C、D 应补到末尾并按名排序。
        let order = vec!["B".to_string(), "A".to_string()];
        let got = resolve_caliber_order(&order, &info);
        assert_eq!(got, vec!["B", "A", "C", "D"]);
    }

    #[test]
    fn resolve_order_drops_stale_and_empty_means_sorted_all() {
        let info = make_config(&["A", "B"]);
        // preferred 含已移除口径 X（应剔除），且只列了 B；剩余 A 补尾并按名排序 -> [B, A]。
        let order = vec!["X".to_string(), "B".to_string()];
        let got = resolve_caliber_order(&order, &info);
        assert_eq!(got, vec!["B", "A"]);
        // 空 preferred：等价于口径名排序全量（向后兼容旧版默认顺序）。
        let got2 = resolve_caliber_order(&[], &info);
        assert_eq!(got2, vec!["A", "B"]);
    }
}

