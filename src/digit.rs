//! 数字（价格）识别专用模块。
//!
//! 设计目标：解决「现有 OCR 对数字识别极度不准确」的问题，收敛为一条**专用价格链路**。
//!
//! 核心思路（对齐用户方案）：把每个子弹的价格框按数位切分为 **5 个独立截图范围**
//! （万 / 千 / 百 / 十 / 个，见 `config::split_price_5`），对每一位**独立识别一个数字**，
//! 再按序拼接得到最终价格（如 `1,9,2,0` → `1920`）。逐位识别从根本上规避了整框 OCR
//! 因游戏字体/配色（彩色价格文字）被整片抹成背景、或多位数字粘连导致的错识。
//!
//! 识别后端（可插拔）：
//! - 当前默认后端：`ocr::recognize_price`（颜色感知二值化 + 去噪 + 6× 放大 + en-US + 纯数字抽取），
//!   针对游戏字体/配色做了专门预处理，显著优于原 near-white 二值化直接喂 WinRT 的通用路径。
//! - 深度学习后端（预留）：`recognize_single_digit` 内部即为单一数字位的识别入口，
//!   接入 YOLO/CRNN 等模型时只需替换该函数的实现（如调用 ONNX Runtime 推理），
//!   上层的 `recognize_price_digits` 拼接逻辑与调用方 `observe` 完全不变。

use crate::config::Rect;
use crate::ocr;
use crate::win;
use std::sync::OnceLock;

/// 调试开关：环境变量 `DELTA_OBSERVER_DEBUG=1` 时，将每个数位的原始裁剪与二值化放大图
/// 落盘到 `debug_test_digits/` 目录，便于校准坐标与识别阈值（默认关闭，零开销）。
static DEBUG: OnceLock<bool> = OnceLock::new();
fn debug_enabled() -> bool {
    *DEBUG.get_or_init(|| std::env::var("DELTA_OBSERVER_DEBUG").is_ok())
}

/// 逐位识别价格：给定 5 个数位截图框（顺序 **万 / 千 / 百 / 十 / 个**），
/// 对每位独立识别一个数字并拼接为最终价格字符串。
///
/// 拼接规则：从左到右扫描 5 位，**前导空位（未识别到数字）直接跳过**；一旦遇到首个数字，
/// 后续空位保守补 `0`（价格不会出现中间空缺，补 0 防止错位）；全空则返回空串。
pub fn recognize_price_digits(rgba: &[u8], w: i32, h: i32, digits: [Rect; 5]) -> String {
    let debug = debug_enabled();
    let mut s = String::new();
    let mut started = false;
    for (i, &d) in digits.iter().enumerate() {
        if debug {
            if let Some((img, iw, ih)) = win::crop_region(rgba, w, h, d) {
                let _ = win::save_rgba_png(
                    "debug_test_digits",
                    &format!("d{i}_raw.png"),
                    &img,
                    iw as u32,
                    ih as u32,
                );
                let (b, bw, bh) = ocr::preprocessed_price(&img, iw, ih);
                let _ = win::save_rgba_png(
                    "debug_test_digits",
                    &format!("d{i}_bin.png"),
                    &b,
                    bw as u32,
                    bh as u32,
                );
            }
        }
        match recognize_single_digit(rgba, w, h, d) {
            Some(c) => {
                started = true;
                s.push(c);
            }
            None if started => s.push('0'),
            None => {}
        }
    }
    s
}

/// 识别**单个数字位**：裁剪小图 → 预处理 → 后端识别 → 取首个数字字符。
///
/// 当前后端 = OCR（`ocr::recognize_price` 的单数字位版，返回一串纯数字，取首字符）。
/// **深度学习接入点**：将本函数体替换为模型推理（输入裁剪 RGBA，输出 `0-9` 或 `None`），
/// 上层的逐位拼接与所有调用方均无需改动。
fn recognize_single_digit(rgba: &[u8], w: i32, h: i32, rect: Rect) -> Option<char> {
    // 内缩 1px 去除相邻数字边缘干扰（分割点可能贴着左右数字）。
    let mut r = rect;
    r[0] += 1;
    r[2] -= 2;
    if r[2] < 1 {
        r[2] = 1;
    }
    let (img, iw, ih) = win::crop_region(rgba, w, h, r)?;
    let s = ocr::recognize_price(&img, iw, ih);
    s.chars().find(|c| c.is_ascii_digit())
}
