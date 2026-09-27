//! 基于 Windows 内置 OCR（Windows.Media.Ocr，WinRT）的文字识别。
//!
//! 设计要点（针对交易行价格识别难点定制，依据 2026-09-27 实测截图）：
//! - 零外部依赖（不依赖 Tesseract / Python），直接调用系统 OCR 引擎；
//! - 预处理管线：裁剪小图 → **近白像素二值化**（浅色文字 → 黑字白底）→
//!   **4× 最近邻放大**（12px 级小字号放大后识别率显著提升）；
//!   二值化同时天然排除深色背景、卡片描边与**金黄硬币图标**（低亮度/高饱和，不满足
//!   「近白且低饱和」判据），从源头减少价格区噪声；
//! - 语言引擎由调用方指定：中文名用 `zh-CN`，纯数字价格用 `en-US`（数字识别更稳），
//!   主引擎语言包缺失时自动回退另一种。

use windows::core::{ComInterface, HSTRING};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Win32::System::WinRT::IMemoryBufferByteAccess;

/// 小字号文本的放大倍率（名称等通用文字，12px 级 → 48px 级）。
const UPSCALE: i32 = 4;

/// 价格文本的放大倍率：价格字号更小、且需逐位精准，放大更激进（12px 级 → 72px 级）。
const PRICE_UPSCALE: i32 = 6;

/// 单个识别单词及其在图像中的像素矩形（原点 = 图像左上角）。
/// `w/h` 当前未被调用方使用，保留以备按包围盒过滤/调参之需。
#[derive(Clone)]
pub struct Word {
    pub text: String,
    pub x: i32,
    pub y: i32,
    #[allow(dead_code)]
    pub w: i32,
    #[allow(dead_code)]
    pub h: i32,
}

/// 尝试用指定语言标签创建 OCR 引擎；语言包缺失时返回 None。
fn create_engine(tag: &str) -> Option<OcrEngine> {
    let tag = HSTRING::from(tag);
    let lang = Language::CreateLanguage(&tag).ok()?;
    OcrEngine::TryCreateFromLanguage(&lang).ok()
}

/// 近白文字二值化：亮度足够高且低饱和的像素视为文字（黑），其余视为背景（白）。
/// 输入 RGBA，输出 BGRA（SoftwareBitmap::Bgra8 直接可用）。
///
/// 判据：`min(R,G,B) > 150`（足够亮）且 `max - min < 70`（低饱和，排除金黄硬币图标）。
fn binarize_light_text(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for chunk in rgba.chunks_exact(4) {
        let (r, g, b) = (chunk[0] as u32, chunk[1] as u32, chunk[2] as u32);
        let min = r.min(g).min(b);
        let max = r.max(g).max(b);
        if min > 150 && max - min < 70 {
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

/// 价格文字判断：比 `binarize_light_text` 更宽松，旨在保留**任意颜色**的价格数字。
///
/// 游戏价格文字常为亮黄 / 金 / 浅蓝等**高饱和彩色**（并不满足「近白」），
/// 原二值化（`mx-mn < 170` 上限）会把纯黄/金价格整片抹成背景 → OCR 看到空白 →
/// 数字识别「极度不准确」，表现就是价格区被整片抹成背景 → OCR 看到的几乎为空。这里改为：
/// - 像素必须足够明亮（`max(R,G,B) > 170`，过滤暗背景）；
/// - 且具备一定颜色（最大最小通道差 `> 25`，排除纯灰/白噪声）；
/// 二者满足即视为文字（黑），其余置白（背景）。
/// 特例排除**金黄硬币图标**：`R>180 && G>140 && B<120 && R-B>80`。
///
/// 注意：价格已按数位切分为 5 个独立小框（`config::split_price_5`），每框内几乎只有
/// 一个数字位，放宽饱和度上限不会引入数字外的干扰，反而是保留亮黄/金数字的关键。
fn is_price_text(r: u32, g: u32, b: u32) -> bool {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let bright_enough = mx > 170;
    let colored = (mx - mn) > 25; // 非纯灰/纯白，确为彩色文字
    let coin = r > 180 && g > 140 && b < 120 && (r - b) > 80;
    bright_enough && colored && !coin
}

/// 价格专用二值化：保留任意颜色的明亮价格文字（黑），其余置白（BGRA）。
fn binarize_price_text(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for chunk in rgba.chunks_exact(4) {
        let (r, g, b) = (chunk[0] as u32, chunk[1] as u32, chunk[2] as u32);
        if is_price_text(r, g, b) {
            out.extend_from_slice(&[0, 0, 0, 255]); // 文字 → 黑
        } else {
            out.extend_from_slice(&[255, 255, 255, 255]); // 背景 → 白
        }
    }
    out
}

/// 去斑点：去除孤立黑像素（4 邻域黑像素 < 2 的黑点置白），抑制二值化引入的盐噪声，
/// 避免 OCR 将噪点误识为短线 / 小数点。
fn despeckle(bgra: &mut [u8], w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let w = w as usize;
    let h = h as usize;
    let src: Vec<u8> = bgra.to_vec(); // 副本，避免原地修改读到已改值
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let is_black = src[i] == 0 && src[i + 1] == 0 && src[i + 2] == 0;
            if !is_black {
                continue;
            }
            let mut black_neighbors = 0u32;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let j = (ny as usize * w + nx as usize) * 4;
                if src[j] == 0 && src[j + 1] == 0 && src[j + 2] == 0 {
                    black_neighbors += 1;
                }
            }
            if black_neighbors < 2 {
                bgra[i] = 255;
                bgra[i + 1] = 255;
                bgra[i + 2] = 255;
                // alpha 保持 255
            }
        }
    }
}

/// 价格专用预处理（落盘调试用）：颜色感知二值化 → 去斑点 → 6× 放大。
/// 输出 BGRA，与 `recognize_bgra` 输入约定一致。
pub fn preprocessed_price(rgba: &[u8], w: i32, h: i32) -> (Vec<u8>, i32, i32) {
    let mut bgra = binarize_price_text(rgba);
    despeckle(&mut bgra, w, h);
    upscale_nearest(&bgra, w, h, PRICE_UPSCALE)
}

/// 识别一整块裁剪图（RGBA）中的**价格数字**：颜色感知二值化 + 去噪 + 6× 放大，
/// 用 en-US 引擎（缺失回退 zh-CN）识别后仅保留数字（千分位分隔符由 `extract_price` 剔除）。
///
/// 专用于价格格；名称格仍走 `recognize_words`（近白二值化，对白底名称更稳定）。
pub fn recognize_price(rgba: &[u8], w: i32, h: i32) -> String {
    let engine = match create_engine("en-US").or_else(|| create_engine("zh-CN")) {
        Some(e) => e,
        None => return String::new(),
    };
    let (b, bw, bh) = preprocessed_price(rgba, w, h);
    if bw <= 0 {
        return String::new();
    }
    let words = recognize_bgra(&b, bw, bh, &engine);
    let s: String = words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    crate::observe::extract_price(&s)
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
                        w: r.Width as i32,
                        h: r.Height as i32,
                    });
                }
            }
        }
    }
    words
}

/// 识别一小块裁剪图（RGBA，与 `win::crop_region` 输出同构）。
///
/// 流程：二值化 → 4× 放大 → 用 `primary` 语言引擎识别（缺失时回退另一语言）。
/// 名称格传 `"zh-CN"`，价格格传 `"en-US"`。
pub fn recognize_words(rgba: &[u8], w: i32, h: i32, primary: &str) -> Vec<Word> {
    let fallback = if primary == "zh-CN" { "en-US" } else { "zh-CN" };
    let Some(engine) = create_engine(primary).or_else(|| create_engine(fallback)) else {
        return Vec::new();
    };
    let (ubgra, uw, uh) = preprocessed(rgba, w, h);
    if uw <= 0 {
        return Vec::new();
    }
    recognize_bgra(&ubgra, uw, uh, &engine)
}
