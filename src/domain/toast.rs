//! 购买结果 toast 识别（领域层）。
//!
//! 点击「购买」后，游戏在窗口顶部居中弹出成功 toast：
//! 实测（1080p 录屏逐帧标定）出现延迟 ≈0.3s、持续 ≈1.1s，两类成功文案为：
//! - 正常购买成功：`9x19mm RIP x 31购买成功，已存入仓库`
//! - 低价购买成功：`9x19mm RIP x 31购买成功，价格更新，已用更低价格成交`
//!
//! **关键版式事实（标定实测）**：toast 文本中只有开头的「子弹名 x 数量」是亮绿字，
//! 其后的消息文本是白字；文本整体在窗口内水平居中（绿字左缘 L 与文本右缘 R 满足
//! R ≈ 2×窗口中心 − L，标定帧误差 ≤2px）。失败 toast（如限流提示）整句白字、无绿字。
//!
//! 识别策略（不引入中文 OCR，纯图像判定——文字内容允许"读不准"，归类必须可靠）：
//! 1. **绿字门控**：监听带内亮绿像素计数越阈即判「有成功 toast」；
//!    失败类 toast 整句白字（G≈R≈B）天然不满足，从而被排除（按需求失败类不在处理范围）。
//! 2. **候选带推导**：绿字 bbox 给出文本左缘与文本行高度；由居中对称推出整句右缘
//!    `R = 2×中心 − L`（+ 余量），裁出「绿名 + 白消息」完整文本带。
//! 3. **判别模板滑动匹配**：仅「低价」文案含独有关键字形「更低价格」——用实机标定帧
//!    裁出的该白字字形模板（内嵌 jpg，编译期打进二进制，无外部文件依赖）与候选带做
//!    滑动归一化互相关（NCC），峰值 ≥ 阈值判「低价购买成功」，否则判「正常购买成功」。
//!    滑窗匹配天然容忍子弹名/数量导致的文案长度差异；模板与候选均按亮度 bbox 收紧
//!    到字形后再归一到统一高度，消除分辨率 / DPI 缩放差异。
//! 4. **监听窗取峰值**：toast 有淡入淡出（各 ≈0.1s），模糊帧 NCC 偏低；
//!    在监听窗内对每个绿字帧累计 NCC 峰值再判定，淡入淡出不影响归类；
//!    「低价」一旦确定性命中即提前返回，缩短购买循环耗时。
//!
//! 监听全程经 `stop` 分片轮询，F9 可即时中断（返回 `None`）。
//! 监听带坐标按客户区尺寸比例推导（实机标定：toast 顶带 y∈[173,218]、水平居中、
//! 最长 ≈545px @1080p），只需「包含」toast 即可，精确落点由绿字 bbox 二次定位。
//! 已知局限：多条 toast 垂直堆叠时绿字 bbox 跨两行，归一后字形被压扁、NCC 下降——
//! 本工具单次购买只触发一条 toast，不处理堆叠场景。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use windows::Win32::Foundation::HWND;

use crate::screenshot::{crop_region, ClientCapturer};

/// 内嵌「更低价格」白字字形模板：实机 1080p 标定帧（低价成交 toast）消息区裁剪，
/// 加载时按亮度 bbox 自动收紧到字形本体。
const MARKER_BYTES: &[u8] = include_bytes!("../../assets/toast/lower_price_marker.jpg");

/// 点击购买后的 toast 监听窗时长（ms）。
/// toast 出现延迟 ≈0.3s + 单条持续 ≈1.1s（堆叠时更长），3s 足够覆盖且不显著拖慢购买循环。
const MONITOR_MS: u64 = 3000;
/// 监听轮询间隔（ms）：toast 有效期 ~1s，90ms 采样 ≥10fps，足够取到清晰帧。
const POLL_MS: u64 = 90;
/// 中断睡眠分片（ms）：F9 在数十毫秒内生效。
const SLICE_MS: u64 = 20;
/// NCC 判定阈值：峰值 ≥ 此值判「低价购买成功」。
/// 标定帧自匹配与「正常文案」交叉匹配的实测值见单测（双边留 ≥0.1 余量），取中值。
const NCC_LOWER_THRESHOLD: f32 = 0.6;
/// 模板与候选统一归一到此像素高（字形高对齐，消除分辨率 / DPI 缩放差异）。
const NORM_H: usize = 32;
/// 亮字判定阈值（max 通道）：toast 文字（绿名/白消息）字形核心像素远高于此，
/// 深色半透明底与游戏背景远低于此。用于模板与候选带的字形 bbox 收紧。
const BRIGHT_THRESH: u8 = 110;
/// 候选带右缘相对「居中对称点」的额外余量（px，1080p 基准）：
/// 对称估计与实测的偏差吸收，宁多勿少——多出的背景窗口 NCC 得分低，无害。
const RIGHT_MARGIN: i32 = 48;
/// 候选带垂直方向在绿字行基础上的外扩（px）：容纳抗锯齿边缘，亮度 bbox 会再收紧。
const CAND_Y_PAD: i32 = 5;
/// 绿字 bbox 外扩像素：保留抗锯齿边缘，避免字形削边。
const BBOX_PAD: i32 = 3;

/// 购买成功 toast 的两类结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuyToastKind {
    /// `…购买成功，已存入仓库`（按显示价正常成交）。
    NormalSuccess,
    /// `…购买成功，价格更新，已用更低价格成交`（行情变动，以更低实际价成交）。
    LowerPriceSuccess,
}

impl BuyToastKind {
    /// 展示用文案（毛玻璃面板 / 日志共用）。
    pub fn label(self) -> &'static str {
        match self {
            BuyToastKind::NormalSuccess => "正常购买成功（已存入仓库）",
            BuyToastKind::LowerPriceSuccess => "低价购买成功（已用更低价格成交）",
        }
    }
}

/// 可被 `stop` 中断的分片睡眠：命中中断返回 `true`（调用方应尽快退出）。
fn sleep_interruptible(total_ms: u64, stop: &AtomicBool) -> bool {
    let mut elapsed = 0u64;
    while elapsed < total_ms {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        let d = (total_ms - elapsed).min(SLICE_MS);
        std::thread::sleep(Duration::from_millis(d));
        elapsed += d;
    }
    false
}

/// 亮绿字判定（成功 toast 的「子弹名 x 数量」文字色）：G 显著高于 R/B 且足够亮。
/// 白字（消息文本与失败 toast）G≈R≈B 不满足——这正是门控只认成功 toast 的依据。
fn is_marker_green(r: u8, g: u8, b: u8) -> bool {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    g >= 120 && g - r >= 40 && g - b >= 20
}

/// 统计 RGBA 缓冲内的亮绿像素数。
fn green_count(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4)
        .filter(|p| is_marker_green(p[0], p[1], p[2]))
        .count()
}

/// 绿字存在性阈值：随监听带面积缩放（1080p 带 ≈8.7 万像素 → ≈109），下限 60。
/// 标定帧绿名部分 ≈939px，余量充足。
fn green_threshold(rw: i32, rh: i32) -> usize {
    ((rw.max(0) as usize * rh.max(0) as usize) / 800).max(60)
}

/// 满足 `pred` 的像素包围盒（相对本缓冲左上角，外扩 `pad` 并夹紧到边界）。
/// 无命中像素返回 `None`。
fn bbox_where(rgba: &[u8], w: i32, h: i32, pad: i32, pred: impl Fn(u8, u8, u8) -> bool) -> Option<[i32; 4]> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, -1, -1);
    let w = w as usize;
    for y in 0..h as usize {
        let row = y * w * 4;
        for x in 0..w {
            let p = &rgba[row + x * 4..row + x * 4 + 4];
            if pred(p[0], p[1], p[2]) {
                if (x as i32) < x0 {
                    x0 = x as i32;
                }
                if (x as i32) > x1 {
                    x1 = x as i32;
                }
                if (y as i32) < y0 {
                    y0 = y as i32;
                }
                if (y as i32) > y1 {
                    y1 = y as i32;
                }
            }
        }
    }
    if x1 < 0 {
        return None;
    }
    let x0 = (x0 - pad).max(0);
    let y0 = (y0 - pad).max(0);
    let x1 = (x1 + pad).min(w as i32 - 1);
    let y1 = (y1 + pad).min(h as i32 - 1);
    Some([x0, y0, x1 - x0 + 1, y1 - y0 + 1])
}

/// 亮绿像素的包围盒（成功 toast 的「子弹名 x 数量」部分）。
fn green_bbox(rgba: &[u8], w: i32, h: i32) -> Option<[i32; 4]> {
    bbox_where(rgba, w, h, BBOX_PAD, is_marker_green)
}

/// 亮字像素（任一通道 ≥ [`BRIGHT_THRESH`]）的包围盒：toast 文字（绿名 + 白消息）整体。
fn bright_bbox(rgba: &[u8], w: i32, h: i32) -> Option<[i32; 4]> {
    bbox_where(rgba, w, h, 0, |r, g, b| {
        r.max(g).max(b) >= BRIGHT_THRESH
    })
}

/// RGBA → 灰度 f32（Rec.601 亮度）。
fn to_gray(rgba: &[u8]) -> Vec<f32> {
    rgba.chunks_exact(4)
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect()
}

/// 双线性缩放灰度图（行优先 f32）到 `tw × th`。
fn resize_gray(src: &[f32], sw: usize, sh: usize, tw: usize, th: usize) -> Vec<f32> {
    let mut out = vec![0f32; tw * th];
    if sw == 0 || sh == 0 || tw == 0 || th == 0 {
        return out;
    }
    let rx = sw as f32 / tw as f32;
    let ry = sh as f32 / th as f32;
    for dy in 0..th {
        let fy = ((dy as f32 + 0.5) * ry - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = fy - y0 as f32;
        for dx in 0..tw {
            let fx = ((dx as f32 + 0.5) * rx - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = fx - x0 as f32;
            let a = src[y0 * sw + x0];
            let b = src[y0 * sw + x1];
            let c = src[y1 * sw + x0];
            let d = src[y1 * sw + x1];
            out[dy * tw + dx] = a * (1.0 - wx) * (1.0 - wy)
                + b * wx * (1.0 - wy)
                + c * (1.0 - wx) * wy
                + d * wx * wy;
        }
    }
    out
}

/// 归一化互相关（减均值 / 除标准差）：两图同构 ≈1，无关 ≈0。
fn ncc(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let mut ma = 0f64;
    let mut mb = 0f64;
    for i in 0..n {
        ma += a[i] as f64;
        mb += b[i] as f64;
    }
    ma /= n as f64;
    mb /= n as f64;
    let mut cov = 0f64;
    let mut va = 0f64;
    let mut vb = 0f64;
    for i in 0..n {
        let da = a[i] as f64 - ma;
        let db = b[i] as f64 - mb;
        cov += da * db;
        va += da * da;
        vb += db * db;
    }
    if va <= 1e-9 || vb <= 1e-9 {
        return 0.0;
    }
    (cov / (va.sqrt() * vb.sqrt())) as f32
}

/// 「更低价格」白字判别模板（进程内只加载一次）。
struct Marker {
    data: Vec<f32>,
    w: usize,
    h: usize,
}

/// 从内嵌 jpg 加载模板：RGBA → 亮度 bbox 收紧到字形 → 灰度 → 高度归一 [`NORM_H`]。
/// 加载失败返回 `None`（此后所有打分恒为 0，监听退化为「只判绿字有无」，不崩溃）。
fn marker() -> &'static Option<Marker> {
    static M: OnceLock<Option<Marker>> = OnceLock::new();
    M.get_or_init(|| {
        let img = image::load_from_memory(MARKER_BYTES).ok()?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as i32, rgba.height() as i32);
        let bbox = bright_bbox(rgba.as_raw(), w, h)?;
        let (crop, cw, ch) = crop_region(rgba.as_raw(), w, h, bbox)?;
        let gray = to_gray(&crop);
        let tw = ((cw as f32 / ch.max(1) as f32) * NORM_H as f32).round() as usize;
        if tw < 8 {
            return None;
        }
        let data = resize_gray(&gray, cw as usize, ch as usize, tw, NORM_H);
        Some(Marker {
            data,
            w: tw,
            h: NORM_H,
        })
    })
}

/// 依据客户区尺寸推导 toast 监听带（相对客户区左上角）。
/// 按实机标定比例放宽（y∈[13%,24%]H、x∈[15%,85%]W），容忍标题栏 / 分辨率偏差。
fn toast_band(w: i32, h: i32) -> [i32; 4] {
    let y0 = (h as f32 * 0.13) as i32;
    let y1 = (h as f32 * 0.24) as i32;
    let x0 = (w as f32 * 0.15) as i32;
    let x1 = (w as f32 * 0.85) as i32;
    [x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)]
}

/// 对一段含绿字成功 toast 的 ROI 计算「更低价格」模板滑动 NCC 峰值。
///
/// 流程：绿字 bbox 定位文本左缘与文本行 → 居中对称推出整句右缘、裁出候选带 →
/// 亮度 bbox 收紧到字形 → 灰度、高度归一 → 逐 x 滑窗 NCC 取最大。
fn score_lower_price(roi_rgba: &[u8], rw: i32, rh: i32) -> f32 {
    let Some(m) = marker() else {
        return 0.0;
    };
    // 1) 绿字 bbox：成功 toast 的「子弹名 x 数量」→ 文本左缘 + 文本行高度。
    let Some(gb) = green_bbox(roi_rgba, rw, rh) else {
        return 0.0;
    };
    let left = gb[0];
    let cy0 = (gb[1] - CAND_Y_PAD).max(0);
    let cy1 = (gb[1] + gb[3] - 1 + CAND_Y_PAD).min(rh - 1);
    // 2) 文本整体水平居中：整句右缘 ≈ 2×带中心 − 绿字左缘（+余量）。
    let right = (2 * (rw / 2) - left + RIGHT_MARGIN).clamp(left + 1, rw - 1);
    let crect = [left, cy0, right - left + 1, cy1 - cy0 + 1];
    // 3) 裁候选带 → 亮度 bbox 收紧到字形 → 灰度 → 高度归一（与模板同流程）。
    let Some((crop, cw, ch)) = crop_region(roi_rgba, rw, rh, crect) else {
        return 0.0;
    };
    let Some(bb) = bright_bbox(&crop, cw, ch) else {
        return 0.0;
    };
    let Some((tight, tw_i, th_i)) = crop_region(&crop, cw, ch, bb) else {
        return 0.0;
    };
    if th_i < 1 {
        return 0.0;
    }
    let cand_w = (tw_i as f32 / th_i as f32 * NORM_H as f32).round() as usize;
    if cand_w < m.w {
        return 0.0; // 候选比模板还窄，不可能包含「更低价格」
    }
    let gray = to_gray(&tight);
    let cand = resize_gray(&gray, tw_i as usize, th_i as usize, cand_w, NORM_H);
    // 4) 逐 x 滑窗 NCC 取峰值。
    let mut best = 0f32;
    for x in 0..=(cand_w - m.w) {
        let mut win = Vec::with_capacity(m.w * m.h);
        for row in 0..m.h {
            let start = row * cand_w + x;
            win.extend_from_slice(&cand[start..start + m.w]);
        }
        let s = ncc(&win, &m.data);
        if s > best {
            best = s;
        }
    }
    best
}

/// 点击「购买」后监听结果 toast，归类为两种购买成功之一。
///
/// 在 [`MONITOR_MS`] 监听窗内反复截取客户区、推导监听带、做绿字门控与模板匹配：
/// - NCC 峰值过阈即提前返回「低价购买成功」；
/// - 绿字连续稳定 2 帧（≈180ms）且 NCC 明显低于低价阈值，即提前返回「正常购买成功」
///   （加速：免去余下监听窗等待，正常成交判定由 ~3s 缩至 ~0.5s）；
/// - 窗结束仍未见绿字稳定 2 帧（或全程无绿字）则按原逻辑兜底返回。
/// 全程未见绿字返回 `None`（点击可能静默失败，由调用方记日志，
/// 本模块不做失败处理——按需求失败类不在范围）。
pub fn monitor_after_buy(hwnd: HWND, stop: &AtomicBool) -> Option<BuyToastKind> {
    let cap = ClientCapturer::new(hwnd);
    let mut max_ncc = 0f32;
    let mut seen_green = false;
    // 连续绿字帧计数：用于「稳定 2 帧(~180ms)即判定正常成交」的提前返回（见循环内）。
    let mut green_streak = 0u32;
    let mut waited = 0u64;
    while waited < MONITOR_MS {
        if stop.load(Ordering::Relaxed) {
            crate::log::write("[TOAST] 监听期间收到停止信号，中止");
            return None;
        }
        if let Ok(frame) = cap.capture() {
            let band = toast_band(frame.width, frame.height);
            if let Some((roi, rw, rh)) = crop_region(&frame.rgba, frame.width, frame.height, band) {
                if green_count(&roi) >= green_threshold(rw, rh) {
                    if !seen_green {
                        seen_green = true;
                        crate::log::write("[TOAST] 检测到购买成功 toast（绿字）");
                    }
                    green_streak += 1;
                    let s = score_lower_price(&roi, rw, rh);
                    if s > max_ncc {
                        max_ncc = s;
                    }
                    if max_ncc >= NCC_LOWER_THRESHOLD {
                        crate::log::write(&format!(
                            "[TOAST] NCC={max_ncc:.3} ≥ {NCC_LOWER_THRESHOLD} → 低价购买成功（提前命中）"
                        ));
                        return Some(BuyToastKind::LowerPriceSuccess);
                    }
                    // 加速：绿字已连续稳定 2 帧（≈180ms）且 NCC 明显低于低价阈值，
                    // 即可确定本次为「正常购买成功」，无需再等满 3s 监听窗。
                    // 低价成交的 NCC 远高于阈值，2 帧内已必定命中上一分支提前返回，
                    // 故此处不会把低价为正常误判；稳定 2 帧也过滤了瞬态绿字抖动。
                    if green_streak >= 2 {
                        crate::log::write(&format!(
                            "[TOAST] 绿字稳定 {green_streak} 帧、NCC={max_ncc:.3} < {NCC_LOWER_THRESHOLD} → 正常购买成功（提前判定）"
                        ));
                        return Some(BuyToastKind::NormalSuccess);
                    }
                } else {
                    // 本帧无绿字（toast 尚未弹出 / 已淡出）→ 连续计数清零，避免抖动误判。
                    green_streak = 0;
                }
            }
        }
        if sleep_interruptible(POLL_MS, stop) {
            return None;
        }
        waited += POLL_MS;
    }
    crate::log::write(&format!(
        "[TOAST] 监听窗结束：绿字={seen_green} max_ncc={max_ncc:.3}（阈值 {NCC_LOWER_THRESHOLD}）"
    ));
    if seen_green {
        Some(BuyToastKind::NormalSuccess)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 加载 assets/toast 下的标定帧 fixture（实机 1080p 录屏裁剪）。
    fn load_rgba(name: &str) -> (Vec<u8>, i32, i32) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/toast/");
        let img = image::open(format!("{dir}{name}")).expect(name);
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as i32, rgba.height() as i32);
        (rgba.into_raw(), w, h)
    }

    #[test]
    fn marker_template_loads() {
        // 分步定位：解码 → 亮度 bbox → 归一，任一步失败即指明具体环节。
        let img = image::load_from_memory(MARKER_BYTES).expect("模板 jpg 解码失败");
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as i32, rgba.height() as i32);
        let bbox = bright_bbox(rgba.as_raw(), w, h).expect("模板内未检出亮字 bbox");
        let m = marker();
        let m = m.as_ref().expect("marker() 加载失败");
        assert_eq!(m.h, NORM_H);
        assert!(m.w >= 40, "模板宽度过小：{}（bbox={bbox:?}）", m.w);
    }

    #[test]
    fn fail_toast_has_no_green() {
        // 失败 toast（您的操作过快）整句白字，不应通过绿字门控。
        let (f, fw, fh) = load_rgba("crop_fail.jpg");
        let cnt = green_count(&f);
        assert!(
            cnt < green_threshold(fw, fh),
            "失败白字 toast 不应触发绿字门控：count={cnt}"
        );
    }

    #[test]
    fn success_toasts_pass_green_gate() {
        for name in ["crop_successA.jpg", "crop_successB.jpg"] {
            let (img, w, h) = load_rgba(name);
            assert!(
                green_count(&img) >= green_threshold(w, h),
                "{name} 成功 toast 应通过绿字门控"
            );
        }
    }

    #[test]
    fn lower_price_scores_high_normal_stays_low() {
        let (b, bw, bh) = load_rgba("crop_successB.jpg");
        let s_lower = score_lower_price(&b, bw, bh);
        let (a, aw, ah) = load_rgba("crop_successA.jpg");
        let s_normal = score_lower_price(&a, aw, ah);
        eprintln!("NCC 标定值：低价自匹配={s_lower:.3} 正常交叉匹配={s_normal:.3}（阈值 {NCC_LOWER_THRESHOLD}）");
        assert!(
            s_lower >= NCC_LOWER_THRESHOLD,
            "低价 toast 应过阈：lower={s_lower:.3} normal={s_normal:.3}"
        );
        assert!(
            s_normal < NCC_LOWER_THRESHOLD - 0.1,
            "正常 toast 不应误判为低价：lower={s_lower:.3} normal={s_normal:.3}"
        );
    }
}
