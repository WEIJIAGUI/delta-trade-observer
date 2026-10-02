//! 截图模块：客户区截图 / 区域裁剪 / 调试标注 / PNG 落盘，统一对外接口。
//!
//! # 核心能力
//! - **客户区截图** [`ClientCapturer`]：构造一次、反复 `capture()`。
//!   底层用 `PrintWindow(PW_CLIENTONLY | PW_RENDERFULLCONTENT)` 把游戏客户区
//!   **直接渲染进客户区尺寸的缓冲**——缓冲原点天然 = 客户区左上角，
//!   窗口边框 / 标题栏 / DWM 阴影从机制上不可能进入输出
//!   （旧实现「整窗渲染再按可见区裁剪」曾因两者原点差 7px 隐形边框，导致输出左侧黑边、内容整体错位）。
//!   失败回退 `BitBlt`（客户区 DC，原点同为客户区左上角）。
//! - **区域裁剪** [`crop_region`]：从缓冲按 `Rect = [x, y, w, h]` 裁剪，越界夹紧。
//! - **标注** [`draw_rect_outline`] / [`draw_digit`]：在缓冲上画矩形框 / 3×5 数字点阵（调试用）。
//! - **保存** [`save_rgba_png`]：用 `png` 原生编码器 `Compression::Fast` 落盘。
//!
//! # 坐标系契约（唯一真相）
//! 截图缓冲原点 = 客户区左上角，所有 `Rect` 与 `config` 区域坐标同帧，
//! 调用方**无需任何偏移换算**，可直接裁剪/标注。

use std::path::Path;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, SRCCOPY, ReleaseDC, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use png::{BitDepth, ColorType, Compression, Encoder};

/// 矩形区域 `[x, y, w, h]`：相对客户区（无标题栏截图）左上角。
/// 与 `config::Rect` 底层类型相同（均为 `[i32; 4]`），跨模块互操作无需转换。
pub type Rect = [i32; 4];

/// 数位框标注颜色，顺序与 `config::BulletRects::digit_rects()`（千 / 百 / 十 / 个）一致。
/// 集中定义为共享常量，供「开始观察」调试帧与「OCR 测试」标注预览复用，避免重复字面量。
pub const DIGIT_BOX_COLORS: [[u8; 4]; 4] = [
    [255, 70, 70, 255],  // 0 千位
    [60, 220, 90, 255],  // 1 百位
    [80, 150, 255, 255], // 2 十位
    [255, 215, 60, 255], // 3 个位
];

/// 名称框标注颜色（灰，仅作位置参照）。
pub const NAME_BOX_COLOR: [u8; 4] = [180, 180, 200, 255];

/// 数位框标注厚度（像素）。比名称框粗一档，便于在小图上分辨框与底图。
pub const DIGIT_BOX_THICKNESS: i32 = 2;

/// 位序数字标注相对框顶的上移量（像素）：`3×5` 点阵字高 5 + 2px 间隙，
/// 使数字落在框上方而不压住框内内容。
pub const DIGIT_LABEL_LIFT_PX: i32 = 7;

/// 一张 RGBA 截图（行优先，每像素 4 字节）。
///
/// 来源是客户区截图（原点 = 客户区左上角，与 `config` 坐标同帧）。
/// 提供 `draw_rect_outline` / `draw_digit` 便捷方法，
/// 避免裸缓冲与裸尺寸在多处传递（更易出错、难维护）。
pub struct CapturedImage {
    pub rgba: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

impl CapturedImage {
    /// 构造（调用方需保证 `rgba.len() == width * height * 4`）。
    pub fn new(rgba: Vec<u8>, width: i32, height: i32) -> Self {
        Self {
            rgba,
            width,
            height,
        }
    }

    /// 区域裁剪：相对本图左上角的 `rect`，越界夹紧；返回新图（含实际宽高）。
    pub fn dimensions(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// 从本地图片文件加载为 `CapturedImage`（RGBA，原点 = 图片左上角）。
    ///
    /// 供 OCR 测试 Tab 复用同一套坐标与识别逻辑：用户选择一张交易行截图，
    /// 转成与游戏客户区截图同构的 RGBA 缓冲，即可直接喂给识别器。
    /// 支持 `image` 默认特性覆盖的编码（PNG / JPG / BMP 等）。
    pub fn from_file(path: &str) -> Result<CapturedImage, String> {
        let img = image::open(path).map_err(|e| format!("读取图片失败：{e}"))?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as i32, rgba.height() as i32);
        Ok(CapturedImage::new(rgba.into_raw(), w, h))
    }
}

/// 取窗口**客户区**尺寸（物理像素，宽, 高）。失败返回 `None`。
fn client_size(hwnd: HWND) -> Option<(i32, i32)> {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        if GetClientRect(hwnd, &mut rc).is_err() {
            return None;
        }
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        (w > 0 && h > 0).then_some((w, h))
    }
}

/// 游戏窗口客户区截图器：构造一次，整个观察周期反复 `capture()` 复用。
///
/// # 即开即用
/// 句柄在构造时绑定，之后每次 `capture()` 都是独立完整的截图，无隐藏状态、
/// 无需调用方再传 `hwnd` 或处理窗口几何——所见即客户区。
///
/// # 实现要点（黑边根因的彻底修复）
/// 旧实现「整窗渲染 + 按可见区裁剪」依赖两个不同原点的矩形换算：
/// 渲染缓冲原点 = 窗口矩形左上角（含 ~7px 不可见改尺寸边框，渲染为纯黑），
/// 裁剪基准却取 `DWMWA_EXTENDED_FRAME_BOUNDS`（可见区，不含该边框）——
/// 两者原点恰好相差 7px，导致输出左侧黑边、内容整体偏移、右缘截断。
/// 新实现用 `PW_CLIENTONLY` 让系统**只渲染客户区**进客户区尺寸的缓冲：
/// 缓冲原点 = 客户区原点，不经过任何偏移换算，黑边从机制上不可能出现。
pub struct ClientCapturer {
    hwnd: HWND,
}

impl ClientCapturer {
    /// 绑定目标窗口句柄（不校验有效性，失败在 `capture` 时报错）。
    pub fn new(hwnd: HWND) -> Self {
        Self { hwnd }
    }

    /// 当前客户区宽度（像素）；获取失败返回 0（调用方可据此退化到标定视口）。
    pub fn client_width(&self) -> i32 {
        client_size(self.hwnd).map(|(w, _)| w).unwrap_or(0)
    }

    /// 截取客户区，返回原点 = 客户区左上角的 RGBA 图（与 `config` 坐标同帧）。
    ///
    /// 主路径 `PrintWindow(PW_CLIENTONLY | PW_RENDERFULLCONTENT)`（对 DX 游戏可靠）；
    /// 失败回退 `BitBlt`（客户区 DC `GetDC`，原点同为客户区左上角）。
    /// **不隐式激活窗口**——聚焦由调用方显式完成。
    pub fn capture(&self) -> Result<CapturedImage, String> {
        let (w, h) = client_size(self.hwnd).ok_or("获取客户区尺寸失败")?;

        unsafe {
            let hdc = GetDC(self.hwnd);
            if hdc.is_invalid() {
                return Err("获取客户区 DC 失败".into());
            }
            let mem_dc = CreateCompatibleDC(hdc);
            let bitmap = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem_dc, bitmap);

            // PW_CLIENTONLY(0x1) | PW_RENDERFULLCONTENT(0x2)：只渲染客户区、
            // 且强制 DWM 渲染完整内容（对 DX 游戏可靠）。
            if !PrintWindow(self.hwnd, mem_dc, PRINT_WINDOW_FLAGS(0x1 | 0x2)).as_bool() {
                // 回退：BitBlt 从客户区 DC 拷到内存 DC（原点一致，无黑边）。
                let _ = BitBlt(mem_dc, 0, 0, w, h, hdc, 0, 0, SRCCOPY);
            }

            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // 负高度 => 行序自上而下（便于直接编码）
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = 0; // BI_RGB

            let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
            GetDIBits(
                mem_dc,
                bitmap,
                0,
                h as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            );

            ReleaseDC(self.hwnd, hdc);
            SelectObject(mem_dc, old);
            DeleteObject(bitmap);
            DeleteDC(mem_dc);

            // GetDIBits(BI_RGB) 返回 BGRA；交换 R/B 通道得到 RGBA（alpha 通道已正确）。
            for chunk in buf.chunks_exact_mut(4) {
                chunk.swap(0, 2);
            }

            Ok(CapturedImage::new(buf, w, h))
        }
    }
}

/// 从 RGBA 缓冲中裁剪矩形区域 `rect`（相对本图左上角），越界部分自动夹紧。
///
/// 返回裁剪后的缓冲及**实际**宽高（夹紧后）；区域无效（尺寸非正或完全越界）返回 `None`。
pub fn crop_region(
    rgba: &[u8],
    full_w: i32,
    full_h: i32,
    rect: Rect,
) -> Option<(Vec<u8>, i32, i32)> {
    let (rx, ry, rw, rh) = (rect[0], rect[1], rect[2], rect[3]);
    if rw <= 0 || rh <= 0 || full_w <= 0 || full_h <= 0 {
        return None;
    }
    // 对称夹紧：起点夹到 [0, 尺寸]，终点夹到 [0, 尺寸]；起点必 ≤ 终点（rw>0 时）。
    let x0 = rx.max(0).min(full_w) as usize;
    let y0 = ry.max(0).min(full_h) as usize;
    let x1 = (rx + rw).max(0).min(full_w) as usize;
    let y1 = (ry + rh).max(0).min(full_h) as usize;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let w = (x1 - x0) as i32;
    let h = (y1 - y0) as i32;
    let stride = full_w as usize * 4;
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        let start = y * stride + x0 * 4;
        out.extend_from_slice(&rgba[start..start + (x1 - x0) * 4]);
    }
    Some((out, w, h))
}

/// 将 RGBA 缓冲保存为 PNG 到 `dir/fname`，自动创建目录。
///
/// 采用 `png` 原生编码器，压缩等级固定为 `Compression::Fast`（≈ zlib level 1~3）：
/// 批量截图编码耗时相比 `image` 默认的 level 6 通常下降数倍，文件体积仅温和增大——
/// 对本地截图归档完全可接受（速度优先、体积可接受的最优平衡）。
///
/// **注意**：`png` 0.17 的 `Compression` 没有 `BestSpeed` 变体（误用会编译失败），最快档即 `Fast`。
pub fn save_rgba_png(dir: &str, fname: &str, rgba: &[u8], w: u32, h: u32) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let path = Path::new(dir).join(fname);
    let file = std::fs::File::create(&path).map_err(|e| format!("创建文件失败：{e}"))?;
    let mut encoder = Encoder::new(file, w, h);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    encoder.set_compression(Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG 头写入失败：{e}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    Ok(fname.to_string())
}

/// 在 RGBA 缓冲上以 `color`（RGBA）画实心矩形边框（厚度 `thick` 像素），用于标注裁剪区域。
///
/// 坐标 `rect` 相对本图左上角，越界部分自动夹紧。作用于已剔除标题栏的客户区缓冲。
pub fn draw_rect_outline(
    rgba: &mut [u8],
    w: i32,
    h: i32,
    rect: Rect,
    color: [u8; 4],
    thick: i32,
) {
    let (rx, ry, rw, rh) = (rect[0], rect[1], rect[2], rect[3]);
    if rw <= 0 || rh <= 0 || w <= 0 || h <= 0 {
        return;
    }
    let x0 = rx.max(0).min(w) as usize;
    let y0 = ry.max(0).min(h) as usize;
    let x1 = (rx + rw).max(0).min(w) as usize;
    let y1 = (ry + rh).max(0).min(h) as usize;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let t = thick.max(1) as usize;
    let set = |buf: &mut [u8], x: usize, y: usize| {
        let idx = (y * w as usize + x) * 4;
        if idx + 3 < buf.len() {
            buf[idx] = color[0];
            buf[idx + 1] = color[1];
            buf[idx + 2] = color[2];
            buf[idx + 3] = color[3];
        }
    };
    for y in y0..y1 {
        if y < y0 + t || y >= y1.saturating_sub(t) {
            for x in x0..x1 {
                set(rgba, x, y);
            }
        }
    }
    for x in x0..x1 {
        if x < x0 + t || x >= x1.saturating_sub(t) {
            for y in y0..y1 {
                set(rgba, x, y);
            }
        }
    }
}

/// 点阵字形尺寸（列 × 行，像素）：仅够画出可辨的 0–9，越小越不遮挡被标注内容。
const GLYPH_COLS: usize = 3;
/// 见 [`GLYPH_COLS`]。
const GLYPH_ROWS: usize = 5;

/// `GLYPH_COLS × GLYPH_ROWS` 像素点阵字体（仅数字 0–9），
/// 用于调试框序号标注（千/百/十/个 → 0/1/2/3）。
const DIGIT_FONT_3X5: [[u8; GLYPH_COLS * GLYPH_ROWS]; 10] = [
    [1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1], // 0
    [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1], // 1
    [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1], // 2
    [1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 3
    [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1], // 4
    [1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 5
    [1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1], // 6
    [1, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0], // 7
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1], // 8
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 9
];

/// 在 RGBA 缓冲上以 `GLYPH_COLS × GLYPH_ROWS` 点阵绘制单个数字（左上角 `x, y`，颜色 `color`）。
pub fn draw_digit(rgba: &mut [u8], w: i32, h: i32, x: i32, y: i32, digit: u8, color: [u8; 4]) {
    if digit as usize >= DIGIT_FONT_3X5.len() {
        return;
    }
    let g = DIGIT_FONT_3X5[digit as usize];
    for row in 0..GLYPH_ROWS {
        for col in 0..GLYPH_COLS {
            if g[row * GLYPH_COLS + col] == 1 {
                let px = x + col as i32;
                let py = y + row as i32;
                if px >= 0 && py >= 0 && px < w && py < h {
                    let idx = (py as usize * w as usize + px as usize) * 4;
                    if idx + 3 < rgba.len() {
                        rgba[idx] = color[0];
                        rgba[idx + 1] = color[1];
                        rgba[idx + 2] = color[2];
                        rgba[idx + 3] = color[3];
                    }
                }
            }
        }
    }
}
