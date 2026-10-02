//! 价格（数字）识别专用模块。
//!
//! 价格由 4 个数位（**千 / 百 / 十 / 个**）各自独立识别一位数字后拼接而成。
//! 每个数位对应 `config::BulletRects` 中的 `thousands / hundreds / tens / ones` 坐标区域；
//! 单字符识别由神经网络分类器 [`nn_classifier`] 完成（基于 `model.onnx`，输入 32×32 灰度）。
//!
//! 本模块在「逐位裁剪 + 神经网络分类」之上叠加一层可靠性保障：
//! **合法性校验**：四个数位必须从个位起连续向上、不得出现中间空缺
//! （例如「个、十、千」识别到而「百」缺失即为非法——真实价格不存在这种断层）；
//! 校验未过则判定为异常，交由调用方标记（写入 anomaly 日志、价格置空）。
//!
//! 识别严格遵循神经网络置信度阈值 0.90 的**硬规则**（见 [`nn_classifier::CONF_THRESHOLD`]）：
//! 任一数位置信度低于 0.90 一律判「未识别」，**不存在「放宽重试」**——
//! 用户明确要求低于 90% 视为无数字，故不提供任何下调阈值的重试路径。

use crate::config::Rect;
use crate::screenshot;
use crate::nn_classifier;
use std::sync::OnceLock;

/// 单个数位的识别可视化详情（供 OCR 测试 Tab 直观展示每个数字位的识别效果）。
#[derive(Clone)]
pub struct DigitDetail {
    /// 参与匹配的实际数字裁剪图（已内缩，RGBA，与 `screenshot::crop_region` 同构）。
    pub crop: Vec<u8>,
    pub crop_w: i32,
    pub crop_h: i32,
    /// 神经网络对 0..9 类的 softmax 概率（区间 `[0.0, 1.0]`）。
    pub scores: [f32; 10],
    /// 最高概率对应的数字。
    pub best_digit: Option<u8>,
    /// 最高分（= `scores[best_digit]`）。
    pub best_score: f32,
    /// 经阈值判定后的识别结果（低于阈值判为 `None`，即「这一位认不出」）。
    pub recognized: Option<char>,
}

/// 调试开关：环境变量 `DELTA_OBSERVER_DEBUG=1` 时，将每个数位的原始裁剪落盘到
/// `debug_test_digits/d0..d3_raw.png`（0=千位 … 3=个位），便于校准坐标与识别阈值。
static DEBUG: OnceLock<bool> = OnceLock::new();
fn debug_enabled() -> bool {
    *DEBUG.get_or_init(|| std::env::var("DELTA_OBSERVER_DEBUG").is_ok())
}

/// 数位框左右各内缩的像素数（识别前去除邻位笔画干扰）。
///
/// 背景：相邻数位框彼此紧邻（间距仅数像素，见 `config.rs`）。
/// 字体存在抗锯齿与轻微字距浮动，邻位笔画可能溢进本位的匹配特征；
/// 各边内缩 1px 即可隔离，代价可忽略。
const DIGIT_INSET_PX: i32 = 1;

/// 对单个数位矩形做左右内缩（见 [`DIGIT_INSET_PX`]）。
///
/// 宽度过小（≤ 2 × 内缩量）时不做内缩，避免把识别区域缩没。
fn inset_digit_rect(mut r: Rect) -> Rect {
    if r[2] > 2 * DIGIT_INSET_PX {
        r[0] += DIGIT_INSET_PX;
        r[2] -= 2 * DIGIT_INSET_PX;
    }
    r
}

/// 单个价格的识别结果。
#[derive(Debug)]
pub enum PriceRec {
    /// 完整且合法的价格字符串，如 `"1920"`。
    Price(String),
    /// 四个数位全部未识别：视为「当前帧无数据」，非错误（通常不计入有效记录）。
    Empty,
    /// 识别存在「中间缺失 / 断层」等非法结构，重试后仍无法修正 → 标记为异常。
    /// `reason` 说明缺失/错位的数位，`partial` 为已识别片段（用于日志与排查）。
    Invalid { reason: &'static str, partial: String },
}

/// 价格识别器：构造一次，整个观察周期复用。
///
/// # 即开即用（封装要点）
/// 把「逐位裁剪 + 神经网络分类 + 合法性校验」的完整策略收拢为一个类型，
/// 调用方（`observe`）在观察开始时构造一次，之后对每枚子弹直接 `recognize()`，
/// 不感知内部阈值、内缩、重试等细节。自身无可变状态，可安全复用。
pub struct PriceRecognizer {
    /// 调试落盘开关（进程启动时由环境变量决定，见 [`debug_enabled`]）。
    debug: bool,
}

impl Default for PriceRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl PriceRecognizer {
    pub fn new() -> Self {
        Self {
            debug: debug_enabled(),
        }
    }

    /// 对单个数位矩形做一次完整识别：裁剪 → 神经网络分类 → 阈值判定，
    /// 返回「最终识别字符」与「可视化详情（裁剪图 + 各类概率）」。
    ///
    /// 这是 [`recognize_detail`] 的唯一识别入口，阈值 / 内缩逻辑单一来源。
    ///
    /// ## 识别引擎
    /// 使用神经网络分类器（`nn_classifier::classify`）：若 `model.onnx` 已加载则识别该位；
    /// 模型缺失时 `classify` 返回 `None`，该位判为「未识别」（不再有模板匹配兜底）。
    fn match_one(
        &self,
        rgba: &[u8],
        w: i32,
        h: i32,
        rect: Rect,
    ) -> (Option<char>, Option<DigitDetail>) {
        // 内缩去除相邻数位边缘干扰（数位框在标定中紧贴 / 重叠 1px）。
        let r = inset_digit_rect(rect);
        let Some((img, iw, ih)) = screenshot::crop_region(rgba, w, h, r) else {
            return (None, None);
        };
        if self.debug {
            let _ = screenshot::save_rgba_png(
                "debug_test_digits",
                &format!("d_{}_{}.png", r[0], r[1]),
                &img,
                iw as u32,
                ih as u32,
            );
        }

        // 神经网络分类（模型缺失时 classify 返回 None，该位认不出）。
        let detail = nn_classifier::classify(rgba, w, h, r)
            .map(|out| Self::build_nn_detail(&img, iw, ih, &out.probs, out.recognized));

        let recognized = detail.as_ref().and_then(|d| d.recognized);
        (recognized, detail)
    }

    /// 由神经网络 11 类概率 + 已判定识别结果，构造与 NN 输出同构的 `DigitDetail`
    /// （供 OCR 测试 Tab 复用同一套可视化）。`scores` 取 0-9 类概率，`best_digit`/`best_score`
    /// 为其中最高者；`recognized` 已在 `classify` 内按 90% 阈值与 `x` 非数字规则判定。
    fn build_nn_detail(
        crop: &[u8],
        crop_w: i32,
        crop_h: i32,
        probs: &[f32],
        recognized: Option<char>,
    ) -> DigitDetail {
        let mut best = 0usize;
        let mut best_score = *probs.get(0).unwrap_or(&0.0);
        for i in 1..10 {
            if probs[i] > best_score {
                best_score = probs[i];
                best = i;
            }
        }
        let mut scores = [0f32; 10];
        for i in 0..10 {
            scores[i] = *probs.get(i).unwrap_or(&0.0);
        }
        DigitDetail {
            crop: crop.to_vec(),
            crop_w,
            crop_h,
            scores,
            best_digit: Some(best as u8),
            best_score,
            recognized,
        }
    }

    /// 识别 4 个数位，返回 `[千, 百, 十, 个]` 各自识别到的数字字符（未识别为 `None`）。
    fn recognize_digits(
        &self,
        rgba: &[u8],
        w: i32,
        h: i32,
        digit_rects: &[Rect; 4],
    ) -> [Option<char>; 4] {
        let mut out = [None; 4];
        for (i, &r) in digit_rects.iter().enumerate() {
            out[i] = self.match_one(rgba, w, h, r).0;
        }
        out
    }

    /// 同 [`recognize`](Self::recognize)，但额外返回每个数位的识别可视化详情
    /// （裁剪图 + 各类概率），供 OCR 测试 Tab 直观展示每个数字位的识别效果。
    ///
    /// 返回 `(最终价格结果, [千, 百, 十, 个] 各自的匹配详情)`；裁剪失败的位对应 `None`。
    ///
    /// 与 [`recognize`](Self::recognize) 共用同一 0.90 硬阈值，不做放宽重试（见模块文档）。
    pub fn recognize_detail(
        &self,
        rgba: &[u8],
        w: i32,
        h: i32,
        digit_rects: [Rect; 4],
    ) -> (PriceRec, Vec<Option<DigitDetail>>) {
        let chars = self.recognize_digits(rgba, w, h, &digit_rects);
        let details: Vec<Option<DigitDetail>> = digit_rects
            .iter()
            .map(|&r| self.match_one(rgba, w, h, r).1)
            .collect();
        (validate(chars), details)
    }
}

/// 合法性校验：四个数位必须从「个位」起连续向上、不得出现中间空缺。
///
/// - 全 `None` → `Empty`（无数据，非错误）；
/// - 存在「高位存在但紧邻低位缺失」的断层 → `Invalid`；
/// - 其余 → 合法，按 **千→百→十→个** 顺序拼成价格字符串。
///
/// 规则覆盖用户示例：个位、十位、千位均识别到但百位缺失 →
/// `千位存在 ∧ 百位缺失` 命中，判定为非法。
fn validate(chars: [Option<char>; 4]) -> PriceRec {
    // 顺序与 `config::BulletRects::digit_rects()` 一致：千 / 百 / 十 / 个。
    let [th, h, t, o] = chars;
    let present = |x: Option<char>| x.is_some();

    if !present(th) && !present(h) && !present(t) && !present(o) {
        return PriceRec::Empty;
    }
    // 由低位向高位检查：一旦某位存在，其紧邻的低位必须也存在（不允许断层）。
    if present(t) && !present(o) {
        return PriceRec::Invalid {
            reason: "十位识别到但个位缺失",
            partial: describe(&chars),
        };
    }
    if present(h) && !present(t) {
        return PriceRec::Invalid {
            reason: "百位识别到但十位缺失",
            partial: describe(&chars),
        };
    }
    if present(th) && !present(h) {
        return PriceRec::Invalid {
            reason: "千位识别到但百位缺失",
            partial: describe(&chars),
        };
    }
    let s: String = chars.iter().filter_map(|c| *c).collect();
    PriceRec::Price(s)
}

/// 把已识别片段格式化为可读文本（如 `千=1 百=- 十=2 个=0`），用于异常日志。
fn describe(chars: &[Option<char>; 4]) -> String {
    let labels = ["千", "百", "十", "个"];
    chars
        .iter()
        .zip(labels)
        .map(|(c, lab)| {
            format!(
                "{lab}={}",
                c.map(|x| x.to_string())
                    .unwrap_or_else(|| "-".to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}
