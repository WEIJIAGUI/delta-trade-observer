//! 购买阶段（领域层）：当某子弹扫描价低于其「最低接受价格」阈值时，进入购买阶段。
//!
//! 购买阶段分三步（与「查看阶段」`observe` 模块在代码层面物理分离）：
//! 1. **导航**：在 `buy_area` 内随机取点点击，打开购买界面（停留在该界面，不 ESC、不购买）；
//! 2. **就绪轮询**：每 150ms 重截一帧、按购买界面数位坐标逐位 NN 识别最新价格，
//!    连续两次读到一致值即视为界面已稳定、可信任（避免读到加载动画中的残影）；
//! 3. **决策**：稳定价 < 阈值 → `Proceed`（弹「开始购买」提示条 + 等待 + ESC）；
//!    否则（含超时未稳定）→ `Abort`（ESC 返回扫描）。
//!
//! 所有购买语义（导航触发、就绪判断、重验证、是否触发）集中在本模块，
//! 便于后续单独迭代购买逻辑而不影响扫描。界面关闭（ESC）由 `observe` 按决策统一处理。
//!
//! 购买阶段识别出的数字裁剪可按 `buy_digits_dir` 落盘（写入 `buy/` 子目录），
//! 与扫描阶段（`scan/`）物理分离，便于各自扩充训练数据。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::HWND;

use crate::config::{
    BuyDigitRects, BuyScene, BUY_DIGITS_3, BUY_DIGITS_4, QTY_SLIDER_REFS, Rect, SLIDER_ANCHOR_X,
    Scale,
};
use crate::buttons;
use crate::observe::ObserveMsg;
use crate::nn_classifier;
use crate::qty_classifier;
use crate::screenshot;
use crate::win;
use crate::Tour;

/// 进入购买界面后的就绪轮询参数。
/// 轮询间隔 150ms，最多 30 次（约 4.5s 上限），超时未稳定即保守中止，避免卡死扫描循环。
const BUY_POLL_INTERVAL_MS: u64 = 150;
const BUY_POLL_MAX: usize = 30;

/// 购买阶段数位裁剪文件名序号（进程内单调递增，跨多次购买事件保证不重名）。
static BUY_DIGIT_SEQ: AtomicU64 = AtomicU64::new(0);

/// 购买阶段数量调整：单次加/减点击后等待界面刷新的间隔（ms）。
const ADJUST_STEP_MS: u64 = 150;

/// 购买阶段每个数位的裁剪样本：`(RGBA 像素, 宽, 高, 识别标签)`。
/// 识别标签为神经网络判定字符（非数字类为 `None`，落盘时记为 `x`）。
type BuyDigitCrop = (Vec<u8>, i32, i32, Option<char>);

/// 可被中止信号中断的睡眠：把 `total_ms` 拆成每 `SLICE_MS` 一片轮询 `stop`，
/// 命中即提前返回 `true`（调用方应据以尽快退出购买阶段）；否则睡满返回 `false`。
/// 用于就绪轮询与数量调整的等待，使 F9 在长阻塞中也能在数十毫秒内生效。
const SLICE_MS: u64 = 20;
fn sleep_interruptible(total_ms: u64, stop: &AtomicBool) -> bool {
    let mut elapsed = 0u64;
    while elapsed < total_ms {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        let d = (total_ms - elapsed).min(SLICE_MS);
        thread::sleep(Duration::from_millis(d));
        elapsed += d;
    }
    false
}

/// 购买阶段决策结果。
pub enum BuyDecision {
    /// 不适用：未设阈值 / 扫描价非法 → 不进入购买阶段，继续扫描。
    Skip,
    /// 重验证后价格 ≥ 阈值（或界面超时未稳定、识别失败）→ 不购买，ESC 返回扫描。
    /// `scene` 为按钮检测器判定的当前界面场景（模型缺失时为 `None`）。
    Abort { scene: Option<BuyScene> },
    /// 重验证后价格 < 阈值 **且所选数量已确认对齐到目标值**：应触发购买。
    ///
    /// 仅在 [`adjust_buy_qty`] 成功把所选数量调到 `target_qty` 并复识别确认后才可能产出此变体；
    /// 数量未确认（识别失败 / 超上限未对齐 / 场景判定失败）一律返回
    /// [`Abort`]，绝不触发购买。按钮坐标取自场景标定模板（恒定可靠），故不再有「按钮缺失」这一失败分支。
    /// 因此 `Proceed` 本身就是「数量已确认」的强信号，是购买触发点的唯一闸门。
    /// `price` 为重验证阶段识别到的稳定最新价格；`scene` 为按钮检测器判定的当前界面场景；
    /// `qty` 为已确认对齐的购买数量；`buy` 为「购买」按钮客户区坐标，供 [`observe`] 作为唯一点击依据。
    Proceed {
        caliber: String,
        bullet: String,
        price: u32,
        scene: Option<BuyScene>,
        /// 已确认对齐的购买数量（`adjust_buy_qty` 复识别 == target_qty 后给出）。
        qty: u32,
        /// 「购买」按钮客户区坐标（来自场景标定模板，经 `Scale::rect` 映射）；仅在此变体出现，
        /// 是触发点击的唯一依据。改用模板而非检测器输出，杜绝「检测器把购买误判为加号」的误购。
        buy: Rect,
    },
}

/// 读取购买界面当前价格，并回传每个数位的裁剪样本。
///
/// 截一帧 → 按位数选购买界面数位坐标（≤3 位用 [`BUY_DIGITS_3`]，>3 位用 [`BUY_DIGITS_4`]，
/// 均经 `Scale::rect` 映射）→ 逐位 `nn_classifier::classify` 识别 → 拼接解析为 `u32`，
/// 同时把每个数位的裁剪图与识别标签一并回传（供购买阶段落盘扩充训练数据）。
///
/// 任一数位未识别、或拼接结果无法解析为数字时返回 `None`，调用方据此判定「界面未就绪 / 不可信」。
fn recognize_buy_price(
    hwnd: HWND,
    scale: &Scale,
    four: bool,
) -> Option<(u32, Vec<BuyDigitCrop>)> {
    let set: &BuyDigitRects = if four { &BUY_DIGITS_4 } else { &BUY_DIGITS_3 };
    let slots: &[Rect] = if four {
        &[set.thousands, set.hundreds, set.tens, set.ones]
    } else {
        &[set.hundreds, set.tens, set.ones]
    };

    let cap = screenshot::ClientCapturer::new(hwnd);
    let frame = cap.capture().ok()?;
    let mut chars: Vec<char> = Vec::new();
    let mut crops: Vec<BuyDigitCrop> = Vec::new();
    for &r in slots {
        let actual = scale.rect(r);
        // 与扫描阶段同款识别链路：NN 分类取识别字符（内含 90% 阈值与 x 非数字规则）。
        // 购买界面坐标未经内缩（与购买阶段原识别行为一致），保持识别口径统一。
        let label = nn_classifier::classify(&frame.rgba, frame.width, frame.height, actual)
            .and_then(|o| o.recognized);
        if let Some(c) = label {
            chars.push(c);
        }
        // 同一矩形裁剪出数位像素，与 classify 内部裁剪保持一致（供训练数据保存）。
        if let Some((b, w, h)) =
            screenshot::crop_region(&frame.rgba, frame.width, frame.height, actual)
        {
            crops.push((b, w, h, label));
        }
    }

    let s: String = chars.into_iter().collect();
    s.parse::<u32>().ok().map(|v| (v, crops))
}

/// 识别购买界面「所选子弹数量」：对**整块数量显示区域**做整体分类，直接得到整数值 1..200（不含 0）。
///
/// 与逐位识别（`recognize_buy_price`）不同——数量在画面中居中显示，不同位数对应不同坐标，
/// 逐位裁剪极易错位，故改用整块区域分类器 `qty_classifier::classify`（训练见
/// `nn-classifier/train_qty.py`）。区域取自场景对应的 `BuyScene::qty_region`
/// （标定视口坐标，经 `Scale::rect` 映射到实际客户区）。
///
/// 返回 `None` 表示模型未加载 / 裁剪无效 / 置信度不足（视为「界面未就绪 / 不可信」），
/// 调用方据此保守退出（见 `adjust_buy_qty`）。识别失败属正常降级路径，不会卡死或误购。
fn recognize_buy_qty(hwnd: HWND, scale: &Scale, scene: BuyScene) -> Option<u32> {
    let cap = screenshot::ClientCapturer::new(hwnd);
    let frame = cap.capture().ok()?;
    let region = scale.rect(scene.qty_region());
    // 整块区域一次性分类，输出即整数值；置信度不足时 classify 内部返回 None。
    qty_classifier::classify(&frame.rgba, frame.width, frame.height, region)
}

/// 数量调整结果：是否已将「所选子弹数量」对齐并确认到目标值。
///
/// 该状态是「能否触发购买」的唯一闸门——仅 [`Confirmed`] 允许后续点击购买按钮；
/// 其余情况（识别失败 / 减加或购买按钮缺失 / 超上限仍未对齐）一律 [`Unconfirmed`]，
/// 调用方据此降级为 `Abort`（ESC 返回，不购买）。
pub enum QtyAdjustResult {
    /// 已确认：复识别到当前数量 == 目标数量，可安全触发购买。
    Confirmed(u32),
    /// 未确认：识别失败或调整未达目标，禁止触发购买。
    Unconfirmed,
}

/// 购买阶段「数量调整」：识别当前所选子弹数量，用「减 / 加」按钮点击把数量调到 `target_qty`。
///
/// **按钮坐标取自场景标定模板**（`scene.layout()`，经 `Scale::rect` 映射），**不依赖 NN 检测器对
/// `plus`/`buy` 的输出**——检测器曾把「购买」误判为「加号」（实机已观察到点到购买按钮），故此处
/// 与数据集采集工具 `qty_tool` 一致改用标定模板，从根本上杜绝「点加号却点到购买」的误购风险（缺陷 D1）。
/// 模板坐标与数量无关、恒定无抖动，也规避了「检测器只跑一次、坐标冻结复用」的陈旧错位（缺陷 D2）。
///
/// **不再设总体次数上限**：循环持续至数量对齐到 `target_qty`（返回 [`Confirmed`]）或单步重试耗尽
/// （返回 [`Unconfirmed`]）。进入循环前先做**快速调整（滑块跳转）**：从 [`QTY_SLIDER_REFS`] 已知参考点中
/// 选取离 `target_qty` 最近者，点击数量滑块跳到该参考数量（仅当跳转后离目标更近才执行，避免越跳越远），
/// 以大幅减少后续加减次数；跳转后由下方加减循环精确微调兜底。每步（一次加/减点击）逻辑：
/// 1. **调整前**：用 CNN（`recognize_buy_qty`）识别并记录当前数量 `before`；
/// 2. 点击「加」（`before < target`）或「减」（`before > target`）按钮；
/// 3. **调整后**：再次用 CNN 识别 `after`，与 `before` 对比：
///    - `after != before` → 本次调整生效，回到步骤 1 继续后续调整；
///    - `after == before` → 本次调整未生效，自动重试（最多 [`MAX_RETRIES`]=3 次）；
/// 4. 单步重试次数耗尽仍 `after == before` → 落点失效 / 点击未生效，返回 [`Unconfirmed`]，由
///    `observe` 统一 ESC（不购买）。
/// 任一识别失败（`recognize_buy_qty` 返回 `None`）即保守停止调整、返回 [`Unconfirmed`]（不卡死、不误购）。
/// 点击坐标转换失败（无法派发）同样立即返回 [`Unconfirmed`]，不做无意义重试。
/// 购买界面「数量滑块」参考点选取：从 [`QTY_SLIDER_REFS`] 中挑出「数量值」离 `target` 最近的已知点。
///
/// 该表后续追加更多参考点后，数量调整会自动就近选用，无需改动调用逻辑（见 [`adjust_buy_qty`]）。
fn nearest_ref_point(target: u32) -> Option<(u32, i32)> {
    let mut best: Option<(u32, i32)> = None;
    let mut best_dist: i64 = i64::MAX;
    for &(q, dx) in QTY_SLIDER_REFS {
        let d = (q as i64 - target as i64).abs();
        if d < best_dist {
            best_dist = d;
            best = Some((q, dx));
        }
    }
    best
}

/// 计算数量滑块上「某参考点」的点击矩形（实际客户区坐标）。
///
/// 滑块横向铺设于「减 / 加」按钮之间，y 取按钮所在行中心（二按钮 y 相同，取其行中心），
/// x = [`SLIDER_ANCHOR_X`] + 参考点横向偏移 `dx`，经 [`Scale::rect`] 映射到实际客户区。
/// 点击框取 10×10 小块，命中滑块轨道即可。
fn slider_click_rect(scene: BuyScene, scale: &Scale, dx: i32) -> Rect {
    let layout = scene.layout();
    let btn_y = layout.minus[1];
    let btn_h = layout.minus[3];
    let cy = btn_y + btn_h / 2;
    let x = SLIDER_ANCHOR_X + dx;
    scale.rect([x, cy - 5, 10, 10])
}

fn adjust_buy_qty(
    hwnd: HWND,
    scale: &Scale,
    scene: BuyScene,
    target_qty: u32,
    tx: &mpsc::Sender<ObserveMsg>,
    stop: &AtomicBool,
) -> QtyAdjustResult {
    // 按钮坐标取标定模板（与 qty_tool 同源）：恒定、与数量无关、零检测误差，永不依赖检测器对 buy/plus 的误判。
    let layout = scene.layout();
    let minus = scale.rect(layout.minus);
    let plus = scale.rect(layout.plus);
    let buy = scale.rect(layout.buy);

    // —— 快速调整（滑块跳转）：先点滑块跳到离目标最近的已知参考点，减少后续加减次数 ——
    // 仅当跳转后离目标更近才执行（避免越跳越远）；跳转后由下方加减循环兜底精确微调。
    // 滑块点击失败（坐标转换失败）或跳转后未生效时，下方循环会从当前数量照常加减微调，无副作用。
    if let Some(cur0) = recognize_buy_qty(hwnd, scale, scene) {
        let _ = tx.send(ObserveMsg::DetectedQty { qty: Some(cur0) });
        if cur0 != target_qty {
            if let Some((ref_qty, ref_dx)) = nearest_ref_point(target_qty) {
                let cur_dist = (cur0 as i64 - target_qty as i64).abs();
                let ref_dist = (ref_qty as i64 - target_qty as i64).abs();
                if ref_dist < cur_dist {
                    let pos = slider_click_rect(scene, scale, ref_dx);
                    if win::click_client_rect(hwnd, pos) {
                        crate::log::write(&format!(
                            "[L3] 快速调整：点数量滑块跳至参考点 {ref_qty} 发（目标 {target_qty} 发）"
                        ));
                        if sleep_interruptible(ADJUST_STEP_MS, stop) {
                            // 等待期间收到中止信号：提前结束，未确认 → 不购买。
                            crate::log::write("[L3] 快速调整等待期间收到停止信号，结束调整（未确认，不购买）");
                            let _ = tx.send(ObserveMsg::DetectedQty { qty: None });
                            win::move_mouse_to_client(hwnd, buy);
                            return QtyAdjustResult::Unconfirmed;
                        }
                        if let Some(jumped) = recognize_buy_qty(hwnd, scale, scene) {
                            crate::log::write(&format!(
                                "[L3] 滑块跳转后识别数量={jumped}（参考点 {ref_qty}，目标 {target_qty}）"
                            ));
                            let _ = tx.send(ObserveMsg::DetectedQty { qty: Some(jumped) });
                        }
                    } else {
                        crate::log::write("[L3] 快速调整：点滑块失败（坐标转换失败，改用加减按钮微调）");
                    }
                }
            }
        }
    }

    // 单步调整（一次加/减点击）的最大重试次数：点击后若数量未变化，说明本次调整未生效，自动重试，最多 3 次。
    const MAX_RETRIES: usize = 3;

    // 不再设总体次数上限：循环持续至数量对齐到目标值（Confirmed）或单步重试耗尽（Unconfirmed）。
    loop {
        if stop.load(Ordering::Relaxed) {
            // 收到中止信号：立即结束数量调整，未确认 → 不购买，由 observe 统一 ESC。
            crate::log::write("[L3] 收到停止信号，提前结束数量调整（未确认，不购买）");
            let _ = tx.send(ObserveMsg::DetectedQty { qty: None });
            win::move_mouse_to_client(hwnd, buy);
            return QtyAdjustResult::Unconfirmed;
        }
        // 调整前：用 CNN 识别并记录当前数量（作为本次调整的「前」基准，供点击后对比）。
        let before = match recognize_buy_qty(hwnd, scale, scene) {
            Some(c) => {
                // 实时上报当前识别到的数量，毛玻璃面板持续刷新（供实机核对减/加落点）。
                let _ = tx.send(ObserveMsg::DetectedQty { qty: Some(c) });
                crate::log::write(&format!("[L3] 识别数量={c}，目标={target_qty}"));
                c
            }
            None => {
                // 识别失败：无法判定基准与后续对比，保守停止调整（不购买）。
                crate::log::write("[L3] 数量识别失败(qty=None)，保守停止调整（未确认）");
                let _ = tx.send(ObserveMsg::DetectedQty { qty: None });
                win::move_mouse_to_client(hwnd, buy);
                return QtyAdjustResult::Unconfirmed;
            }
        };
        if before == target_qty {
            // 数量已对齐并确认：允许触发购买（后续 Proceed 唯一产出条件）。
            win::move_mouse_to_client(hwnd, buy);
            crate::log::write("[OK] 数量已对齐并确认，允许触发购买");
            return QtyAdjustResult::Confirmed(before);
        }
        // 选择按钮：偏少点加、偏多点减。
        let (button, dir) = if before < target_qty {
            (plus, "加")
        } else {
            (minus, "减")
        };
        // 单步调整 + 重试：点击后再次识别，与「调整前」对比；变化=生效继续，未变=重试（最多 MAX_RETRIES 次）。
        let mut step_ok = false;
        for attempt in 0..=MAX_RETRIES {
            if !win::click_client_rect(hwnd, button) {
                // 坐标转换失败，无法派发点击：无法调整，直接中止（重试无意义）。
                crate::log::write(&format!(
                    "[L3] 点击{dir}按钮失败（坐标转换失败，未派发），中止调整（不购买）"
                ));
                let _ = tx.send(ObserveMsg::DetectedQty { qty: None });
                win::move_mouse_to_client(hwnd, buy);
                return QtyAdjustResult::Unconfirmed;
            }
            crate::log::write(&format!(
                "[L3] 数量{}（目标{target_qty}）→ 点击{dir}按钮（已派发）",
                before
            ));
            if sleep_interruptible(ADJUST_STEP_MS, stop) {
                // 等待期间收到中止信号：提前结束，未确认 → 不购买。
                crate::log::write("[L3] 等待期间收到停止信号，结束数量调整（未确认，不购买）");
                let _ = tx.send(ObserveMsg::DetectedQty { qty: None });
                win::move_mouse_to_client(hwnd, buy);
                return QtyAdjustResult::Unconfirmed;
            }
            // 调整后：再次用 CNN 识别，与「调整前」对比。
            let after = match recognize_buy_qty(hwnd, scale, scene) {
                Some(a) => a,
                None => {
                    // 调整后识别失败：无法确认是否生效，保守停止（不购买）。
                    crate::log::write("[L3] 点击后数量识别失败(qty=None)，中止调整（不购买）");
                    let _ = tx.send(ObserveMsg::DetectedQty { qty: None });
                    win::move_mouse_to_client(hwnd, buy);
                    return QtyAdjustResult::Unconfirmed;
                }
            };
            if after != before {
                // 数量发生变化 → 本次调整生效，继续后续调整。
                step_ok = true;
                break;
            }
            // 数量未变化 → 本次调整未生效，自动重试（最多 MAX_RETRIES 次）。
            crate::log::write(&format!(
                "[L3] 第{}次点击{dir}后数量未变({before})，自动重试（已重试{}/{}次）",
                attempt + 1,
                attempt + 1,
                MAX_RETRIES
            ));
        }
        if !step_ok {
            // 单步重试次数耗尽仍无效：落点失效 / 点击未生效，保守停止（不购买）。
            crate::log::write(&format!(
                "[WARN] {dir}按钮点击 {} 次均无效（数量未变化={before}），调整失败，停止",
                MAX_RETRIES + 1
            ));
            let _ = tx.send(ObserveMsg::DetectedQty { qty: None });
            win::move_mouse_to_client(hwnd, buy);
            return QtyAdjustResult::Unconfirmed;
        }
        // 单步生效，回到循环顶部重新识别并判断是否需要继续调整。
    }
}

/// 将购买阶段数位裁剪落盘到 `dir`（文件名含序号与识别标签，与扫描阶段命名规则一致）。
/// `dir` 由调用方保证已创建（见 `observe::run_observation` 的 `buy_dir`）。
fn save_buy_digits(dir: &str, crops: &[BuyDigitCrop]) {
    for (b, w, h, label) in crops {
        let lab = match label {
            Some(c) if c.is_ascii_digit() => c.to_string(),
            _ => "x".to_string(),
        };
        let seq = BUY_DIGIT_SEQ.fetch_add(1, Ordering::Relaxed);
        let _ = screenshot::save_rgba_png(dir, &format!("d{seq:08}_{lab}.png"), b, *w as u32, *h as u32);
    }
}

/// 购买阶段：扫描价低于阈值后进入，先导航打开购买界面、轮询等待其稳定，
/// 再重验证当前价格是否仍低于阈值，决定 `Proceed` / `Abort` / `Skip`。
///
/// `buy_area` 为该子弹的「可点击购买区域」（标定视口坐标，由调用方取自 `BulletRects::buy_area`），
/// 导航时在其中随机取点点击以打开购买界面（见 `win::open_buy_interface`）。
/// `buy_digits_dir` 为购买阶段数字裁剪的输出目录（已带 `buy/` 后缀）；为 `Some` 时，
/// 本次购买阶段识别到的稳定数位裁剪会落盘，便于扩充训练数据（与扫描阶段 `scan/` 分离）。
/// `buy_ui_dir` 为购买界面完整截图的输出目录（独立设置，不与数字裁剪共享）；为 `Some` 时，
/// 本次购买阶段界面**稳定后**会把整幅客户区截图存为 PNG，便于复核购买界面布局与标定校准。
/// `target_qty` 为面板设定的「目标购买数量」：当重验证价格仍低于阈值判定 `Proceed` 时，
/// 进入购买阶段数量调整（识别当前数量 → 用减/加按钮调到该值 → 鼠标移到「购买」按钮待命）。
///
/// 注意：本函数会**实际触发一次打开购买界面的点击**；若判定为 `Abort`/`Skip`，
/// 界面关闭（ESC）由 `observe` 在拿到决策后统一执行。
pub fn verify_then_decide(
    hwnd: HWND,
    scale: &Scale,
    buy_area: Rect,
    scan_price: &str,
    threshold: Option<u32>,
    caliber: &str,
    bullet: &str,
    buy_digits_dir: Option<String>,
    buy_ui_dir: Option<String>,
    target_qty: u32,
    stop: Arc<AtomicBool>,
    tx: &mpsc::Sender<ObserveMsg>,
    // 导游模式协调器：进入购买流程的各关键步骤前展示计划并等待回车确认；关闭态为无操作。
    tour: Tour,
) -> BuyDecision {
    let Some(th) = threshold else {
        // 导游：未设阈值 → 跳过，仍向使用者说明（覆盖「不购买」场景）。
        tour.ask(&format!("【跳过】「{caliber}」{bullet} 未设置最低价，跳过购买判定"));
        if stop.load(Ordering::Relaxed) {
            return BuyDecision::Abort { scene: None };
        }
        return BuyDecision::Skip;
    };

    // 扫描阶段价格位数决定读取几个数位（购买界面坐标与扫描界面不同，按位数选集合）。
    let digits = scan_price.chars().filter(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        // 导游：扫描价格无效（无有效数字）→ 跳过，向使用者说明。
        tour.ask(&format!(
            "【跳过】「{caliber}」{bullet} 扫描价格无效（{scan_price}），无法判定，跳过"
        ));
        if stop.load(Ordering::Relaxed) {
            return BuyDecision::Abort { scene: None };
        }
        return BuyDecision::Skip;
    }
    let four = digits > 3;

    // 导游：扫描价低于阈值时才提示「是否进入购买流程」；确认后才真正打开购买界面（否则直接进后续重验证）。
    if let Ok(sp) = scan_price.parse::<u32>() {
        if sp < th {
            tour.ask(&format!(
                "【购买意向】「{bullet}」扫描价 {sp}，设置最低价 {th}，低于阈值。是否进入购买流程？"
            ));
            if stop.load(Ordering::Relaxed) {
                return BuyDecision::Abort { scene: None };
            }
        }
    }
    // 1) 导航：在 buy_area 中央 50×50 区域内随机取点点击以打开购买界面（仅点击，不 ESC，停留等待重验证）。
    // 传入未缩放的 buy_area 与 scale，由 win::open_buy_interface 内部锁定中央子区域，杜绝越界点击。
    win::open_buy_interface(hwnd, buy_area, scale);

    // 2) 就绪轮询：每 150ms 读一次价格，连续两次一致即认为购买界面已稳定、可信任。
    //    连续一致可过滤加载动画中的瞬时残影（首读可能是旧列表价或未渲染完的值）。
    let mut prev: Option<u32> = None;
    let mut stable: Option<u32> = None;
    let mut stable_crops: Option<Vec<BuyDigitCrop>> = None;
    for _ in 0..BUY_POLL_MAX {
        if stop.load(Ordering::Relaxed) {
            // 中止信号：保守退出，由 observe 统一 ESC；外层循环随后据 stop 退出。
            crate::log::write("[L1] 收到停止信号，中止购买阶段就绪轮询");
            return BuyDecision::Abort { scene: None };
        }
        if sleep_interruptible(BUY_POLL_INTERVAL_MS, &stop) {
            crate::log::write("[L1] 就绪轮询等待期间收到停止信号，中止购买阶段");
            return BuyDecision::Abort { scene: None };
        }
        match recognize_buy_price(hwnd, scale, four) {
            Some((v, crops)) => {
                if prev == Some(v) {
                    stable = Some(v);
                    stable_crops = Some(crops);
                    break;
                }
                prev = Some(v);
            }
            None => prev = None,
        }
    }

    // 3) 决策：稳定价 < 阈值 → Proceed；否则（含超时未稳定 / 识别失败）→ Abort。
    //    界面关闭（ESC）由调用方按决策统一处理。
    //    只要界面已稳定（连续两次读数一致），且对应开关开启，即落盘：
    //      - 购买界面完整截图（buy_ui_dir，便于复核布局 / 标定校准）；
    //      - 数位裁剪（buy_digits_dir，扩充训练数据）。
    //    二者互不干扰、各自独立受开关控制；Proceed 与 Abort 都视为有效的购买阶段结果。
    let decision = match stable {
        Some(mut v) => {
            // 位数决策（读取 3 位 / 4 位槽位）以**扫描价位数**为权威来源：扫描价与购买价同属一颗子弹、
            // 数值应一致，故位数必然相同；且扫描界面数位坐标已充分标定、识别可靠。
            // 另：购买界面 3 位布局的「百位」与 4 位布局的「千位」落在同一像素区域（[298,174]），
            // 单凭图像无法区分 3/4 位，因此扫描价位数是最可靠的判据。
            // 此处仅做一层一致性守卫：若稳定价位数与 `four` 明显矛盾（如按 4 位读却得到 <1000 的值，
            // 或按 3 位读却 ≥1000），用翻转后的位数重识别一次作为兜底（罕见路径，正常不影响）。
            let value_digits = v.to_string().chars().filter(|c| c.is_ascii_digit()).count();
            let inconsistent = (four && value_digits < 4) || (!four && value_digits >= 4);
            if inconsistent {
                let alt_four = !four;
                if let Some((v2, crops2)) = recognize_buy_price(hwnd, scale, alt_four) {
                    let v2_digits = v2.to_string().chars().filter(|c| c.is_ascii_digit()).count();
                    if (alt_four && v2_digits >= 4) || (!alt_four && v2_digits < 4) {
                        crate::log::write(&format!(
                            "[L1] 位数决策一致性守卫触发：原 four={four} 读得 {v}（{value_digits}位），改用 {alt_four} 重识别得 {v2}（{v2_digits}位）"
                        ));
                        v = v2;
                        stable_crops = Some(crops2);
                    }
                }
            }
            // 界面稳定：保存完整购买界面截图（若开启）。
            if let Some(dir) = &buy_ui_dir {
                save_buy_ui(dir, hwnd, caliber, bullet, v);
            }
            // 同时落盘数位裁剪（若开启购买阶段数字保存）。
            if let Some(dir) = &buy_digits_dir {
                if let Some(c) = &stable_crops {
                    save_buy_digits(dir, c);
                }
            }
            // 按钮布局检测：当前购买界面打开，一次性定位三按钮并据「购买」bbox 判定场景。
            // 同时给出三按钮 bbox，供下方「数量调整」复用，避免重复推理。
            // 模型缺失 / 推理失败时 det 为 None，scene 为 None，照常继续，不影响既有扫描与判定。
            let det = buttons::detect_buttons(hwnd, scale);
            let scene = det.as_ref().and_then(|d| d.scene);
            if v < th {
                // 购买阶段数量调整 + 闸门：仅当「场景已判定」且「数量已确认对齐到目标值」时，才产出 Proceed
                // （触发购买）。按钮坐标统一取自场景标定模板（见 adjust_buy_qty），不再依赖检测器对
                // plus/buy 的输出，杜绝「点加号却点到购买」的误购。任一条件不满足一律 Abort（ESC 返回，不购买）。
                // —— 这是「数量未确认则禁止触发购买」的唯一闸门，杜绝「跳过调整直接买」。
                let mut confirmed_qty: Option<u32> = None;
                let mut buy_rect: Option<Rect> = None;
                // 导游：再次确认价格低于阈值，是否开始调整购买数量（确认后才进入减/加点击）。
                tour.ask(&format!(
                    "【数量调整】再次确认「{bullet}」当前价 {v} 低于最低价 {th}，是否开始调整购买数量？"
                ));
                if stop.load(Ordering::Relaxed) {
                    return BuyDecision::Abort { scene };
                }
                if let Some(s) = scene {
                    // 按钮坐标统一取场景标定模板，与 qty_tool 同源；det 仅用于判定场景，不再提供点击坐标。
                    crate::log::write(&format!(
                        "[L1] 进入数量调整：场景={}(按钮坐标取标定模板)，目标数量={target_qty}",
                        s.label()
                    ));
                    match adjust_buy_qty(hwnd, scale, s, target_qty, tx, stop.as_ref()) {
                        QtyAdjustResult::Confirmed(q) => {
                            // 购买条件判断：当前数量 q 已 == 目标 target_qty（adjust_buy_qty 复识别确认），
                            // 即「数量满足购买条件」。导游：明确告知使用者该判定结果，确认后才产出 Proceed。
                            tour.ask(&format!(
                                "【确认购买】「{bullet}」当前数量 {q} 发，已与设置({target_qty}发)一致，满足购买条件，是否点击购买？"
                            ));
                            if stop.load(Ordering::Relaxed) {
                                return BuyDecision::Abort { scene };
                            }
                            confirmed_qty = Some(q);
                            buy_rect = Some(scale.rect(s.layout().buy));
                            crate::log::write(&format!("[L1] 数量已确认={q}，满足购买条件，允许触发购买"));
                        }
                        QtyAdjustResult::Unconfirmed => {
                            // 数量未满足购买条件（识别失败 / 超上限未对齐 / 落点失效）→ 向使用者说明，不购买。
                            tour.ask(&format!(
                                "【未满足购买条件】「{bullet}」数量未能调整到设置值 {target_qty} 发（识别失败 / 超上限 / 落点失效），不满足购买条件，不购买，返回扫描"
                            ));
                            if stop.load(Ordering::Relaxed) {
                                return BuyDecision::Abort { scene };
                            }
                            crate::log::write("[L1] 数量未确认，禁止触发购买，转 Abort 安全退出");
                        }
                    }
                } else {
                    // 购买界面场景未判定 → 无法定位按钮、数量未确认 → 不满足购买条件，向使用者说明，不购买。
                    tour.ask(&format!(
                        "【未满足购买条件】「{bullet}」购买界面场景未判定，无法定位按钮、数量未确认，不满足购买条件，不购买，返回扫描"
                    ));
                    if stop.load(Ordering::Relaxed) {
                        return BuyDecision::Abort { scene };
                    }
                    crate::log::write("[L1] 场景未判定(scene=None)，数量未确认，禁止触发购买");
                }
                if let (Some(qty), Some(buy)) = (confirmed_qty, buy_rect) {
                    BuyDecision::Proceed {
                        caliber: caliber.to_string(),
                        bullet: bullet.to_string(),
                        price: v,
                        scene,
                        qty,
                        buy,
                    }
                } else {
                    // 数量未确认 → 不购买，ESC 返回扫描（不触发购买按钮点击）。
                    BuyDecision::Abort { scene }
                }
            } else {
                // 导游：价格回升 / 不低于阈值 → 不购买，向使用者说明。
                tour.ask(&format!(
                    "【放弃购买】「{caliber}」{bullet} 当前价 {v} 不低于最低价 {th}，不购买，返回扫描"
                ));
                if stop.load(Ordering::Relaxed) {
                    return BuyDecision::Abort { scene };
                }
                BuyDecision::Abort { scene }
            }
        }
        _ => {
            // 超时未稳定：仍尝试判定一次场景（界面可能只是未读到数字），失败则 None。
            let scene = buttons::detect_scene(hwnd, scale);
            // 导游：购买界面超时未稳定 → 不购买，向使用者说明。
            tour.ask(&format!(
                "【放弃购买】「{caliber}」{bullet} 购买界面超时未稳定，不购买，返回扫描"
            ));
            if stop.load(Ordering::Relaxed) {
                return BuyDecision::Abort { scene };
            }
            BuyDecision::Abort { scene }
        }
    };
    decision
}

/// 保存购买界面的完整客户区截图（PNG）到 `dir`。
///
/// 仅在购买界面已稳定（连续两次读数一致）时调用，确保存到的是有效界面而非加载残影。
/// 文件名含口径、子弹、识别价与毫秒时间戳，避免覆盖：`buyui_{口径}_{子弹}_{价}_{ts}.png`。
/// `dir` 由调用方保证已创建（见 `observe::run_observation` 的 `buy_ui_dir`）。
fn save_buy_ui(dir: &str, hwnd: HWND, caliber: &str, bullet: &str, price: u32) {
    let cap = match screenshot::ClientCapturer::new(hwnd).capture() {
        Ok(c) => c,
        Err(_) => return,
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let name = format!(
        "buyui_{}_{}_{}_{}.png",
        sanitize(caliber),
        sanitize(bullet),
        price,
        ts,
    );
    let _ = screenshot::save_rgba_png(dir, &name, &cap.rgba, cap.width as u32, cap.height as u32);
}

/// 文件名安全化：替换 Windows 非法字符（含空格）为 `_`，供购买界面截图命名。
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => out.push('_'),
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}
