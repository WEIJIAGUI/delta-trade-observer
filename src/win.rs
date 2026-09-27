//! Windows 平台窗口操作封装：查找游戏窗口、激活并聚焦窗口、
//! 截图排除（SetWindowDisplayAffinity）、毛玻璃（DwmEnableBlurBehindWindow），
//! 窗口截图（PrintWindow + GetDIBits → PNG），模拟人类键鼠输入（SendInput），
//! 以及内置轻量提示音合成。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HMODULE, HWND, LPARAM, POINT, RECT, TRUE, FALSE};
use windows::Win32::Graphics::Dwm::{DwmEnableBlurBehindWindow, DWM_BLURBEHIND};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, HRGN, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, SRCCOPY, SelectObject,
    ReleaseDC,
};
use windows::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MOUSEINPUT, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, SendInput, VK_A,
    VK_BACK, VK_CONTROL, VK_RETURN, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetForegroundWindow, SetWindowDisplayAffinity, SetWindowPos, SetCursorPos, ShowWindow,
    HWND_TOP, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE, WINDOW_DISPLAY_AFFINITY,
};

/// 枚举窗口时的上下文：要匹配的关键字 + 命中结果。
struct SearchCtx {
    keyword: String,
    found: Option<HWND>,
}

/// `EnumWindows` 的回调：逐个检查可见窗口标题是否包含目标关键字。
/// 命中后立即返回 `BOOL(0)` 终止枚举。
unsafe extern "system" fn enum_find(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut SearchCtx);

    if IsWindowVisible(hwnd).as_bool() {
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; (len + 1) as usize];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n > 0 {
                let title = String::from_utf16_lossy(&buf[..n as usize]);
                if title.contains(&ctx.keyword) {
                    ctx.found = Some(hwnd);
                    return BOOL(0); // 命中，停止枚举
                }
            }
        }
    }
    BOOL(1) // 继续枚举
}

/// 按标题包含 `keyword` 查找第一个可见窗口，返回其句柄。
pub fn find_window_by_keyword(keyword: &str) -> Option<HWND> {
    let mut ctx = SearchCtx {
        keyword: keyword.to_string(),
        found: None,
    };
    unsafe {
        let _ = EnumWindows(Some(enum_find), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.found
}

/// 激活并把目标窗口拉到前台（带失败重试，力争 100% 成功）。
///
/// 为什么原实现会有「小概率失败」：
/// Windows 有“前台锁超时”机制——若调用方进程不是当前前台进程，
/// `SetForegroundWindow` 会被系统拒绝（仅让目标窗口在任务栏闪一下）。
/// 标准解法是用 `AttachThreadInput` 把本线程的输入队列与“当前前台线程”
/// 临时绑定，使系统认为二者同属一个输入队列，从而允许抢占前台。
/// 此外再用 `SetWindowPos(HWND_TOP)` 提 Z 序 + 二次校验 + 重试兜底。
pub fn focus_window(hwnd: HWND) -> bool {
    unsafe {
        // 1. 若被最小化，先还原。
        if IsIconic(hwnd).as_bool() {
            ShowWindow(hwnd, SW_RESTORE);
        }
        // 2. 提到 Z 序最前（并确保在任务栏/桌面上可见）。
        let _ = SetWindowPos(
            hwnd,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );

        // 3. 绕过前台锁：把本线程与前台线程绑定后再 SetForegroundWindow。
        let fg = GetForegroundWindow();
        let mut ok = if fg == hwnd {
            SetForegroundWindow(hwnd).as_bool()
        } else {
            let fg_thread = GetWindowThreadProcessId(fg, None);
            let cur_thread = GetCurrentThreadId();
            if fg_thread != 0 && fg_thread != cur_thread {
                let _ = AttachThreadInput(cur_thread, fg_thread, TRUE);
                let r = SetForegroundWindow(hwnd).as_bool();
                let _ = AttachThreadInput(cur_thread, fg_thread, FALSE);
                r
            } else {
                SetForegroundWindow(hwnd).as_bool()
            }
        };

        // 4. 二次校验 + 重试（应对极小概率的竞态/调度延迟）。
        if !ok || GetForegroundWindow() != hwnd {
            for _ in 0..4 {
                thread::sleep(std::time::Duration::from_millis(40));
                let _ = SetForegroundWindow(hwnd);
                if GetForegroundWindow() == hwnd {
                    ok = true;
                    break;
                }
            }
        }
        ok
    }
}

/// 让指定窗口从屏幕截图中消失 / 恢复。
///
/// `exclude = true`：设置 `WDA_EXCLUDEFROMCAPTURE`（值 0x11），
///   使 Windows 的 DWM 合成、BitBlt、PrintWindow、游戏栏、OBS 等
///   所有基于窗口的截图/录屏都不会捕获到该窗口的像素。
/// `exclude = false`：恢复为 `WDA_NONE`（值 0），正常参与截图。
///
/// 前置条件：窗口必须是分层窗口（WS_EX_LAYERED）。本程序在
/// `NativeOptions` 里已 `with_transparent(true)`，创建即为分层窗口，
/// 因此可随时安全调用。
pub fn set_exclude_from_capture(hwnd: HWND, exclude: bool) {
    unsafe {
        let affinity = if exclude {
            WINDOW_DISPLAY_AFFINITY(0x11u32) // WDA_EXCLUDEFROMCAPTURE
        } else {
            WINDOW_DISPLAY_AFFINITY(0u32) // WDA_NONE
        };
        let _ = SetWindowDisplayAffinity(hwnd, affinity);
    }
}

/// 为窗口启用 / 禁用 DWM 毛玻璃（背景模糊），实现真正的 frosted glass 观感。
///
/// 仅对分层（透明）窗口生效。`hRgnBlur = NULL` 时整窗模糊；
/// 失败（如旧系统不支持）不影响程序运行 —— egui 的半透明面板自身也已具备毛玻璃观感。
pub fn set_window_blur(hwnd: HWND, enable: bool) {
    unsafe {
        let bb = DWM_BLURBEHIND {
            dwFlags: 0x1 | 0x2, // DWM_BB_ENABLE | DWM_BB_BLURREGION
            fEnable: BOOL(if enable { 1 } else { 0 }),
            hRgnBlur: HRGN::default(),
            fTransitionOnMaximized: BOOL(0),
        };
        let _ = DwmEnableBlurBehindWindow(hwnd, &bb);
    }
}

/// 截取整个窗口（以 `GetWindowRect` 为范围，**含标题栏等非客户区**），返回整窗 RGBA 缓冲与尺寸。
///
/// 这是最底层的抓屏源：用 `PrintWindow(PW_RENDERFULLCONTENT)` 渲染整窗（对 DX 游戏最可靠），
/// 坐标基准 = 窗口左上角（原点），与 `代码配置（src/config.rs）` 记录的 `[x, y, w, h]` 区域坐标一致。
///
/// **注意**：此函数返回的是「含标题栏」的整窗缓冲。任何对外输出（默认全屏、口径区域、整窗标注图）
/// 都应经过 `capture_client_rgba` 裁掉非客户区，且把 `代码配置（src/config.rs）` 坐标减去其返回的 `(ox, oy)` 偏移，
/// 才能保证标题栏绝不会进入截图范围。
pub fn capture_window_rgba(hwnd: HWND) -> Result<(Vec<u8>, i32, i32), String> {
    unsafe {
        // 截图前确保目标窗口已激活并聚焦（提升对 DX 游戏的兼容性）。
        focus_window(hwnd);

        let mut wrect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut wrect).is_err() {
            return Err("获取窗口矩形失败".into());
        }
        let full_w = (wrect.right - wrect.left) as i32;
        let full_h = (wrect.bottom - wrect.top) as i32;
        if full_w <= 0 || full_h <= 0 {
            return Err("窗口尺寸无效".into());
        }

        let hwnd_dc = GetDC(hwnd);
        if hwnd_dc.is_invalid() {
            return Err("获取窗口 DC 失败".into());
        }
        let mem_dc = CreateCompatibleDC(hwnd_dc);
        let bitmap = CreateCompatibleBitmap(hwnd_dc, full_w, full_h);
        let old = SelectObject(mem_dc, bitmap);

        // PW_RENDERFULLCONTENT(0x2)：强制 DWM 渲染完整内容（含非客户区，对 DX 游戏更可靠）。
        if !PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(2)).as_bool() {
            let _ = BitBlt(mem_dc, 0, 0, full_w, full_h, hwnd_dc, 0, 0, SRCCOPY);
        }

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = full_w;
        bmi.bmiHeader.biHeight = -full_h; // 负数 => 行序自上而下（便于直接编码）
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = 0; // BI_RGB

        let mut buf = vec![0u8; (full_w as usize) * (full_h as usize) * 4];
        GetDIBits(
            mem_dc,
            bitmap,
            0,
            full_h as u32,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        ReleaseDC(hwnd, hwnd_dc);
        SelectObject(mem_dc, old);
        DeleteObject(bitmap);
        DeleteDC(mem_dc);

        // BGRA -> RGBA（交换 R/B 通道）。
        for chunk in buf.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }

        Ok((buf, full_w, full_h))
    }
}

/// 从整窗 RGBA 缓冲中裁剪出矩形区域 `rect = [x, y, w, h]`（相对窗口左上角）。
/// 越界部分自动夹紧；区域无效返回 `None`。返回裁剪后的缓冲及**实际**宽高（夹紧后）。
pub fn crop_region(
    rgba: &[u8],
    full_w: i32,
    full_h: i32,
    rect: [i32; 4],
) -> Option<(Vec<u8>, i32, i32)> {
    let (rx, ry, rw, rh) = (rect[0], rect[1], rect[2], rect[3]);
    if rw <= 0 || rh <= 0 || full_w <= 0 || full_h <= 0 {
        return None;
    }
    let x0 = rx.max(0).min(full_w - 1) as usize;
    let y0 = ry.max(0).min(full_h - 1) as usize;
    let x1 = (rx + rw).max(0).min(full_w) as usize;
    let y1 = (ry + rh).max(0).min(full_h) as usize;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let w = x1 - x0;
    let h = y1 - y0;
    let stride = full_w as usize * 4;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in y0..y1 {
        let start = y * stride + x0 * 4;
        out.extend_from_slice(&rgba[start..start + w * 4]);
    }
    Some((out, w as i32, h as i32))
}

/// 将 RGBA 缓冲保存为 PNG 到 `dir/fname`，自动创建目录。
///
/// 采用 `png` 原生编码器，压缩等级固定为 `BestSpeed`（zlib level 1）：批量截图场景下，
/// 编码耗时相比 `image` 默认的 level 6 通常可下降数倍，文件体积仅温和增大——
/// 对本地截图归档完全可接受，是「速度优先」与「体积可接受」之间的最优平衡点。
pub fn save_rgba_png(dir: &str, fname: &str, rgba: &[u8], w: u32, h: u32) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
    let path = Path::new(dir).join(fname);
    let file = std::fs::File::create(&path).map_err(|e| format!("创建文件失败：{e}"))?;
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG 头写入失败：{e}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    Ok(fname.to_string())
}

/// 截取游戏窗口并**剔除标题栏等非客户区**，返回「客户区缓冲 + 尺寸」。
///
/// 这是所有对外截图入口的统一底层：无论窗口处于最大化 / 最小化（已还原）/ 全屏 / 多显示器 /
/// 不同 Windows 版本，均先以 `PrintWindow` 抓取整窗（对 DX 游戏最可靠），再用 `client_crop_rect`
/// 裁掉标题栏与边框，从而保证**标题栏绝不会进入任何输出**。无边框 / 全屏下偏移为 0，等同整窗。
///
/// **坐标系契约（唯一真相）**：返回的缓冲原点 = 客户区（无标题栏）左上角，与默认 `shot_*.png`
/// 及 `代码配置（src/config.rs）` 的区域坐标 `[x,y,w,h]` 完全一致（均为客户区左上角基准）。调用方**无需任何偏移换算**，
/// 可直接用 `代码配置（src/config.rs）` 坐标裁剪。
pub fn capture_client_rgba(hwnd: HWND) -> Result<(Vec<u8>, i32, i32), String> {
    let (rgba, w, h) = capture_window_rgba(hwnd)?;
    match client_crop_rect(hwnd) {
        Some(rect) => match crop_region(&rgba, w, h, rect) {
            Some((c, cw, ch)) => Ok((c, cw, ch)),
            // 极端兜底：客户区裁剪失败仍返回整窗（标题栏可能残留，但保证有图可用）。
            None => Ok((rgba, w, h)),
        },
        None => Ok((rgba, w, h)),
    }
}

/// 在 RGBA 缓冲上以 `color`（RGBA）画实心矩形边框（厚度 `thick` 像素），用于标注裁剪区域。
///
/// 坐标 `rect = [x, y, w, h]` 相对「客户区（无标题栏）左上角」，与 `代码配置（src/config.rs）` 及 `capture_client_rgba`
/// 返回的缓冲原点一致；越界部分自动夹紧。作用于已剔除标题栏的客户区缓冲。
pub fn draw_rect_outline(
    rgba: &mut [u8],
    w: i32,
    h: i32,
    rect: [i32; 4],
    color: [u8; 4],
    thick: i32,
) {
    let (rx, ry, rw, rh) = (rect[0], rect[1], rect[2], rect[3]);
    if rw <= 0 || rh <= 0 || w <= 0 || h <= 0 {
        return;
    }
    let x0 = rx.max(0).min(w - 1) as usize;
    let y0 = ry.max(0).min(h - 1) as usize;
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
    // 上、下边（含整行范围，便于与左右边形成闭合框）
    for y in y0..y1 {
        if y < y0 + t || y >= y1.saturating_sub(t) {
            for x in x0..x1 {
                set(rgba, x, y);
            }
        }
    }
    // 左、右边
    for x in x0..x1 {
        if x < x0 + t || x >= x1.saturating_sub(t) {
            for y in y0..y1 {
                set(rgba, x, y);
            }
        }
    }
}

/// 计算客户区（剔除标题栏等非客户区）相对「窗口左上角」的裁剪矩形 `[x, y, w, h]`。
///
/// 实现：用 `GetClientRect` 取客户区尺寸，`ClientToScreen(0,0)` 求客户区左上角屏幕坐标，
/// 再减去 `GetWindowRect` 的窗口左上角屏幕坐标，得到客户区在整窗缓冲中的偏移。
/// 无边框 / 全屏窗口偏移为 0，等同于整窗（标题栏本就不存在）。
pub fn client_crop_rect(hwnd: HWND) -> Option<[i32; 4]> {
    unsafe {
        let mut crect: RECT = std::mem::zeroed();
        if GetClientRect(hwnd, &mut crect).is_err() {
            return None;
        }
        let cw = crect.right - crect.left;
        let ch = crect.bottom - crect.top;
        if cw <= 0 || ch <= 0 {
            return None;
        }
        // 客户区左上角在客户坐标系中即 (0,0)，转成屏幕坐标后减窗口左上角屏幕坐标。
        let mut pt = POINT { x: 0, y: 0 };
        if !ClientToScreen(hwnd, &mut pt).as_bool() {
            return None;
        }
        let mut wrect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut wrect).is_err() {
            return None;
        }
        let ox = pt.x - wrect.left;
        let oy = pt.y - wrect.top;
        Some([ox, oy, cw, ch])
    }
}

/// 截取整个窗口并保存为 PNG 到 `dir`（默认全屏截图入口）。
///
/// 通过 `capture_client_rgba` 统一剔除标题栏等非客户区，保证默认全屏截图只保留内容区、
/// 永远不会包含标题栏（见 `capture_client_rgba` 的窗口状态兼容性说明）。
/// 文件名按本地时间生成：`shot_YYYYMMDD_HHMMSS.png`。
pub fn capture_window_to_png(hwnd: HWND, dir: &str) -> Result<String, String> {
    let (rgba, w, h) = capture_client_rgba(hwnd)?;
    let st = unsafe { GetLocalTime() };
    let fname = format!(
        "shot_{:04}{:02}{:02}_{:02}{:02}{:02}.png",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    );
    save_rgba_png(dir, &fname, &rgba, w as u32, h as u32)?;
    Ok(fname)
}

/// 合成一段简短的提示音 WAV（PCM 16bit 单声道），双音上行，asset-free。
/// 返回完整 WAV 字节（含 44 字节文件头）。
fn build_chime_wav() -> Vec<u8> {
    const SR: u32 = 44100;
    let d1 = 0.12f32;
    let d2 = 0.14f32;
    let n1 = (SR as f32 * d1) as usize;
    let n2 = (SR as f32 * d2) as usize;
    let total = n1 + n2;
    let mut samples: Vec<i16> = vec![0; total];

    let fill = |freq: f32, dur: f32, start: usize, data: &mut Vec<i16>| {
        let n = (SR as f32 * dur) as usize;
        let fade_len = (SR as f32 * 0.006) as usize; // 6ms 淡入淡出，避免爆音
        for i in 0..n {
            let t = i as f32 / SR as f32;
            let fade = (i.min(n - i) as f32 / fade_len as f32).min(1.0);
            let s = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.32 * fade;
            data[start + i] = (s * i16::MAX as f32) as i16;
        }
    };
    fill(880.0, d1, 0, &mut samples); // A5
    fill(1318.5, d2, n1, &mut samples); // E6（上行）

    let data_len = (total * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    b.extend_from_slice(&SR.to_le_bytes());
    b.extend_from_slice(&(SR * 2).to_le_bytes()); // 字节率
    b.extend_from_slice(&2u16.to_le_bytes()); // 块对齐
    b.extend_from_slice(&16u16.to_le_bytes()); // 位深
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in &samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

/// 播放一段内存合成的轻量确认提示音（截图成功反馈）。
///
/// 使用 `SND_MEMORY`（内存 WAV）且**同步**播放（不含 `SND_ASYNC`）：
/// `PlaySoundW` 会在播放完成后才返回，因此函数返回后即可安全释放缓冲区，
/// 不会因异步播放导致悬垂指针或内存泄漏。
pub fn play_capture_chime() {
    let wav = build_chime_wav();
    unsafe {
        let _ = PlaySoundW(
            PCWSTR::from_raw(wav.as_ptr() as *const u16),
            HMODULE::default(),
            SND_MEMORY | SND_NODEFAULT,
        );
    }
    // wav 在此处随函数返回被释放；同步播放已结束，安全。
}

// ====================== 模拟人类键鼠输入（SendInput） ======================

/// xorshift64* 伪随机数发生器（进程级共享状态；仅用于行为随机化，无密码学需求）。
static RNG: AtomicU64 = AtomicU64::new(0);

fn rnd_u64() -> u64 {
    let mut x = RNG.load(Ordering::Relaxed);
    if x == 0 {
        x = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64 | 1)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    RNG.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// `[a, b]` 闭区间随机整数。
fn rnd_range(a: i64, b: i64) -> i64 {
    if b <= a {
        return a;
    }
    a + (rnd_u64() % (b - a + 1) as u64) as i64
}

/// 随机毫秒延时。
fn rnd_sleep_ms(lo: i64, hi: i64) {
    thread::sleep(std::time::Duration::from_millis(rnd_range(lo, hi) as u64));
}

/// 简单伪随机 ±2 像素抖动，模拟人类点击的微小落点偏移。
fn jitter() -> i32 {
    rnd_range(-2, 2) as i32
}

/// 获取当前鼠标屏幕坐标（失败返回 (0, 0)）。
fn get_cursor_pos() -> (i32, i32) {
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_ok() {
            (pt.x, pt.y)
        } else {
            (0, 0)
        }
    }
}

/// 向系统派发一组 INPUT 事件（鼠标或键盘）。
fn send_input_event(input: INPUT) {
    unsafe {
        let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

/// 构造一个键盘事件（VK 码模式）。
fn keybd(vk: VIRTUAL_KEY, down: bool) -> KEYBDINPUT {
    KEYBDINPUT {
        wVk: vk,
        wScan: 0,
        dwFlags: if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP },
        time: 0,
        dwExtraInfo: 0,
    }
}

/// 发送一个虚拟键的按下 / 抬起。
fn send_vk(vk: VIRTUAL_KEY, down: bool) {
    send_input_event(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: keybd(vk, down) },
    });
}

/// 以 Unicode 方式输入单个字符（布局无关，支持任意口径名，含拉丁/数字/符号）。
/// 注意：仅覆盖 BMP 字符（口径均为 ASCII/Latin，足够）。
fn send_unicode_char(ch: char) {
    let scan = ch as u16;
    let down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: KEYEVENTF_UNICODE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_input_event(down);
    rnd_sleep_ms(6, 18); // 按下 → 抬起间隔随机化
    send_input_event(up);
}

/// 人类化鼠标移动：从当前位置分段滑向目标。
///
/// 轨迹以 smoothstep 缓动为骨架，每步叠加「平滑随机游走」的横向/纵向偏移
/// （相邻步偏移相关，既非独立白噪声也非确定性贝塞尔曲线），偏移幅度随进度收敛，
/// 末端再叠加 ±2px 落点抖动；步间延时随机化（4~14ms）。
pub fn move_mouse_human(tx: i32, ty: i32) {
    let (sx, sy) = get_cursor_pos();
    let dx = (tx - sx) as f32;
    let dy = (ty - sy) as f32;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 6.0 {
        unsafe {
            let _ = SetCursorPos(tx + jitter(), ty + jitter());
        }
        return;
    }
    let steps = ((dist / rnd_range(9, 16) as f32) as i64 + rnd_range(1, 3)).clamp(3, 48);
    let steps_f = steps as f32;
    let mut jx = 0.0f32;
    let mut jy = 0.0f32;
    for i in 1..=steps {
        let t = i as f32 / steps_f;
        let ease = t * t * (3.0 - 2.0 * t); // smoothstep 缓动骨架
        let decay = 1.0 - ease; // 偏移幅度随进度收敛，末端精准落点
        jx = jx * 0.55 + (rnd_range(-100, 100) as f32 / 100.0) * 7.0 * decay;
        jy = jy * 0.55 + (rnd_range(-100, 100) as f32 / 100.0) * 7.0 * decay;
        let px = sx as f32 + dx * ease + jx;
        let py = sy as f32 + dy * ease + jy;
        unsafe {
            let _ = SetCursorPos(px.round() as i32, py.round() as i32);
        }
        rnd_sleep_ms(4, 14);
    }
    unsafe {
        let _ = SetCursorPos(tx + jitter(), ty + jitter());
    }
    rnd_sleep_ms(20, 60);
}

/// 在屏幕坐标 `(x, y)` 处模拟一次人类左键点击：
/// 先人类化移动到目标（分段随机轨迹），随机延时后按下 → 随机保持 → 抬起。
pub fn mouse_click_screen(x: i32, y: i32) {
    // 先人类化移动到目标点，再随机延时后点击。
    move_mouse_human(x, y);
    rnd_sleep_ms(40, 120);
    send_input_event(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    rnd_sleep_ms(25, 70); // 按下保持时长随机化
    send_input_event(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_LEFTUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
}

/// 把客户区坐标 `(x, y)` 转换为屏幕坐标（供鼠标点击）。失败返回 `None`。
pub fn client_to_screen(hwnd: HWND, x: i32, y: i32) -> Option<(i32, i32)> {
    unsafe {
        let mut pt = POINT { x, y };
        if ClientToScreen(hwnd, &mut pt).as_bool() {
            Some((pt.x, pt.y))
        } else {
            None
        }
    }
}

/// 模拟人类在商品检索栏输入指定文本：
/// 1) 点击检索栏中心（带抖动）以聚焦；
/// 2) Ctrl+A 全选 → Backspace 清空；
/// 3) 逐字符 Unicode 输入（布局无关）；
/// 4) Enter 触发检索。
///
/// `rect` 为检索栏相对「客户区（无标题栏）」左上角的矩形 `[x, y, w, h]`
/// （与 `src/config.rs`、`capture_client_rgba` 同一坐标基准）。
pub fn type_into_search(hwnd: HWND, text: &str, rect: [i32; 4]) {
    let cx = rect[0] + rect[2] / 2 + jitter() / 2;
    let cy = rect[1] + rect[3] / 2 + jitter() / 2;
    let (sx, sy) = match client_to_screen(hwnd, cx, cy) {
        Some(p) => p,
        None => return,
    };
    mouse_click_screen(sx, sy);
    rnd_sleep_ms(90, 200);

    // 全选
    send_vk(VK_CONTROL, true);
    send_vk(VK_A, true);
    rnd_sleep_ms(10, 30);
    send_vk(VK_A, false);
    send_vk(VK_CONTROL, false);
    rnd_sleep_ms(20, 60);

    // 清空
    send_vk(VK_BACK, true);
    rnd_sleep_ms(10, 25);
    send_vk(VK_BACK, false);
    rnd_sleep_ms(40, 90);

    // 输入口径名（逐字符 Unicode，键入节奏随机化：普通间隔 + 小概率「犹豫停顿」）
    for ch in text.chars() {
        send_unicode_char(ch);
        if rnd_u64() % 100 < 8 {
            rnd_sleep_ms(150, 320);
        } else {
            rnd_sleep_ms(45, 150);
        }
    }
    rnd_sleep_ms(40, 90);

    // 回车触发检索
    send_vk(VK_RETURN, true);
    send_vk(VK_RETURN, false);
}
