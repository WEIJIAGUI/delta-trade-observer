//! 基于 Windows 内置 OCR（Windows.Media.Ocr，WinRT）的文字识别。
//!
//! 设计要点（依据 2026-09-27 实测截图）：
//! - 零外部依赖（不依赖 Tesseract / Python），直接调用系统 OCR 引擎；
//! - 预处理管线：裁剪小图 → **近白像素二值化**（浅色文字 → 黑字白底）→
//!   **4× 最近邻放大**（12px 级小字号放大后识别率显著提升）；
//!   二值化同时天然排除深色背景、卡片描边与**金黄硬币图标**（低亮度/高饱和，不满足
//!   「近白且低饱和」判据），从源头减少噪声；
//! - **用途已收窄**：本模块现**仅用于子弹名称识别**（`zh-CN`）。价格已改由
//!   [`crate::digit`] 做逐位神经网络识别，不再经过 OCR
//!   （规避整框 OCR 把彩色价格整片吞掉或粘连错识）；
//! - 主引擎语言包缺失时自动回退 `en-US`。

use std::collections::HashMap;

use windows::core::{ComInterface, HSTRING};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Win32::System::WinRT::IMemoryBufferByteAccess;

use crate::config::Rect;
use crate::screenshot;

/// 小字号文本的放大倍率（名称等通用文字，12px 级 → 48px 级）。
const UPSCALE: i32 = 4;

/// 单个识别单词及其在图像中的位置（原点 = 图像左上角）。
///
/// 仅保留 `x / y`：`observe::ocr_crop` 需按「行（y）、同行按 x」排序后拼接文本。
/// 包围盒宽高 `w / h` 无任何调用方使用（原以 `#[allow(dead_code)]` 保留），已移除。
#[derive(Clone)]
pub struct Word {
    pub text: String,
    pub x: i32,
    pub y: i32,
}

/// 尝试用指定语言标签创建 OCR 引擎；语言包缺失时返回 None。
fn create_engine(tag: &str) -> Option<OcrEngine> {
    let tag = HSTRING::from(tag);
    let lang = Language::CreateLanguage(&tag).ok()?;
    OcrEngine::TryCreateFromLanguage(&lang).ok()
}

/// 二值化判据 1：像素三通道**最小值**的下限，低于此值视为背景（不够亮）。
///
/// 用 `min` 而非均值：只要有一个通道偏暗就不是近白文字，可排除深色卡面与阴影。
const LIGHT_TEXT_MIN_LUMA: u32 = 150;

/// 二值化判据 2：像素**最大最小通道差**的上限，超过此值视为彩色（非文字）。
///
/// 交易行里的金币图标是金黄色的（高饱和），会被这条判据排除，从源头减少价格区噪声。
const LIGHT_TEXT_MAX_SPREAD: u32 = 70;

/// 近白文字二值化：亮度足够高且低饱和的像素视为文字（黑），其余视为背景（白）。
/// 输入 RGBA，输出 BGRA（SoftwareBitmap::Bgra8 直接可用）。
///
/// 判据：`min(R,G,B) > LIGHT_TEXT_MIN_LUMA`（足够亮）
/// 且 `max - min < LIGHT_TEXT_MAX_SPREAD`（低饱和，排除金黄硬币图标）。
fn binarize_light_text(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for chunk in rgba.chunks_exact(4) {
        let (r, g, b) = (chunk[0] as u32, chunk[1] as u32, chunk[2] as u32);
        let min = r.min(g).min(b);
        let max = r.max(g).max(b);
        if min > LIGHT_TEXT_MIN_LUMA && max - min < LIGHT_TEXT_MAX_SPREAD {
            out.extend_from_slice(&[0, 0, 0, 255]); // 文字 → 黑
        } else {
            out.extend_from_slice(&[255, 255, 255, 255]); // 背景 → 白
        }
    }
    out
}

/// 最近邻放大 BGRA 缓冲 `f` 倍，返回（缓冲, 宽, 高）。
fn upscale_nearest(bgra: &[u8], w: i32, h: i32, f: i32) -> (Vec<u8>, i32, i32) {
    let (uw, uh) = (w * f, h * f);
    if w <= 0 || h <= 0 || uw <= 0 || uh <= 0 {
        return (Vec::new(), 0, 0);
    }
    let mut out = vec![0u8; (uw as usize) * (uh as usize) * 4];
    for y in 0..uh {
        let sy = (y / f) as usize;
        for x in 0..uw {
            let sx = (x / f) as usize;
            let src = (sy * w as usize + sx) * 4;
            let dst = (y as usize * uw as usize + x as usize) * 4;
            out[dst..dst + 4].copy_from_slice(&bgra[src..src + 4]);
        }
    }
    (out, uw, uh)
}

/// 与 `recognize_words` 完全一致的预处理管线（二值化 + 放大），
/// 供调试落盘二值化图，验证二值化阈值与裁剪框是否合适。
pub fn preprocessed(rgba: &[u8], w: i32, h: i32) -> (Vec<u8>, i32, i32) {
    let bgra = binarize_light_text(rgba);
    upscale_nearest(&bgra, w, h, UPSCALE)
}

/// 识别 BGRA 缓冲（w*h*4，Bgra8 布局）中的全部单词。
fn recognize_bgra(bgra: &[u8], w: i32, h: i32, engine: &OcrEngine) -> Vec<Word> {
    let bmp = match SoftwareBitmap::Create(BitmapPixelFormat::Bgra8, w, h) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    // 把像素写入 SoftwareBitmap 的后备缓冲。
    unsafe {
        let buffer = match bmp.LockBuffer(BitmapBufferAccessMode::ReadWrite) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        let reference = match buffer.CreateReference() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let access: IMemoryBufferByteAccess = match reference.cast() {
            Ok(a) => a,
            Err(_) => return Vec::new(),
        };
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: u32 = 0;
        if access.GetBuffer(&mut ptr, &mut len).is_err() {
            return Vec::new();
        }
        if len as usize >= bgra.len() {
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), ptr, bgra.len());
        } else {
            return Vec::new();
        }
    }

    let op = match engine.RecognizeAsync(&bmp) {
        Ok(op) => op,
        Err(_) => return Vec::new(),
    };
    let result = match op.get() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut words = Vec::new();
    if let Ok(lines) = result.Lines() {
        let n = match lines.Size() {
            Ok(n) => n,
            Err(_) => 0,
        };
        for i in 0..n {
            let line = match lines.GetAt(i) {
                Ok(l) => l,
                Err(_) => continue,
            };
            let ws = match line.Words() {
                Ok(ws) => ws,
                Err(_) => continue,
            };
            let m = match ws.Size() {
                Ok(m) => m,
                Err(_) => 0,
            };
            for j in 0..m {
                let word = match ws.GetAt(j) {
                    Ok(wd) => wd,
                    Err(_) => continue,
                };
                let text = match word.Text() {
                    Ok(t) => t.to_string(),
                    Err(_) => continue,
                };
                if text.trim().is_empty() {
                    continue;
                }
                if let Ok(r) = word.BoundingRect() {
                    words.push(Word {
                        text,
                        x: r.X as i32,
                        y: r.Y as i32,
                    });
                }
            }
        }
    }
    words
}

/// 子弹名称识别器：构造一次，整个观察周期复用。
///
/// # 即开即用（封装要点）
/// WinRT OCR 引擎（`OcrEngine`）是昂贵的 COM 对象。旧实现**每次识别都新建引擎、
/// 用完即弃**——不仅白白开销，创建偶发失败还会静默切换回退语言，导致
/// 「同一功能两次调用表现不一致」。本结构体在首次用到某语言时创建一次引擎并
/// **连同失败结果一起缓存**，之后整个观察周期稳定复用，行为可预期。
///
/// # 职责
/// 封装名称识别的完整链路：区域裁剪 → 近白二值化 → 4× 放大 → 引擎识别 →
/// 按阅读顺序（行按 y、同行按 x）拼接文本。调用方只需传整帧截图与目标矩形。
pub struct NameOcr {
    /// 引擎缓存：语言标签 → 引擎（含 `None` = 该语言创建失败，避免反复重试）。
    engines: HashMap<&'static str, Option<OcrEngine>>,
}

impl NameOcr {
    pub fn new() -> Self {
        Self {
            engines: HashMap::new(),
        }
    }

    /// 取（或首次创建并缓存）指定语言的 OCR 引擎。
    fn engine(&mut self, tag: &'static str) -> Option<&OcrEngine> {
        if !self.engines.contains_key(tag) {
            let created = create_engine(tag);
            self.engines.insert(tag, created);
        }
        self.engines.get(tag).and_then(|e| e.as_ref())
    }

    /// 识别整帧截图中 `rect` 区域的文字，返回按阅读顺序以空格拼接的文本。
    ///
    /// 主语言 [`NAME_LANG_PRIMARY`] 引擎缺失时自动回退 [`NAME_LANG_FALLBACK`]。
    pub fn read(&mut self, rgba: &[u8], w: i32, h: i32, rect: Rect) -> String {
        let Some((img, iw, ih)) = screenshot::crop_region(rgba, w, h, rect) else {
            return String::new();
        };
        let tag = if self.engine(NAME_LANG_PRIMARY).is_some() {
            NAME_LANG_PRIMARY
        } else {
            NAME_LANG_FALLBACK
        };
        let Some(engine) = self.engine(tag) else {
            return String::new();
        };
        let (ubgra, uw, uh) = preprocessed(&img, iw, ih);
        if uw <= 0 {
            return String::new();
        }
        let mut words = recognize_bgra(&ubgra, uw, uh, engine);
        // 按阅读顺序排序：行按 y 分桶（OCR_LINE_BUCKET_PX），桶内按 x。
        words.sort_by(|a, b| {
            (a.y / OCR_LINE_BUCKET_PX)
                .cmp(&(b.y / OCR_LINE_BUCKET_PX))
                .then(a.x.cmp(&b.x))
        });
        words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// 名称识别的主语言（中文界面文字）。
const NAME_LANG_PRIMARY: &'static str = "zh-CN";
/// 主语言引擎不可用时的回退语言。
const NAME_LANG_FALLBACK: &'static str = "en-US";

/// OCR 单词按行归并时的纵坐标容差（像素）。
///
/// 同一行文字各单词的 `y` 会有 1~2px 抖动，直接按 `y` 排序会把同行拆散；
/// 先按 `y / 容差` 分桶（相当于每 8px 一档），桶内再按 `x` 排序，即得阅读顺序。
const OCR_LINE_BUCKET_PX: i32 = 8;
