//! Windows 平台窗口操作封装：查找游戏窗口、激活并聚焦窗口、
//! 截图排除（SetWindowDisplayAffinity）、模拟人类键鼠输入（SendInput），
//! 以及内置轻量提示音合成。
//!
//! 毛玻璃（磨砂半透明）效果不再由本模块提供：原先的 `DwmEnableBlurBehindWindow`
//! 仅适用于 GDI 分层窗口，在本程序的 swapchain 透明窗口上会合成黑屏；现改为
//! `src/platform/dxgi_capture.rs` 用 DXGI Desktop Duplication 自采集桌面并模糊，
//! 由 `ui/` 下面板作为底图绘制。
//!
//! 截图相关能力（窗口/客户区/区域截图、裁剪、保存、标注）已迁移至独立模块
//! `src/screenshot.rs`，本文件不再包含截图实现。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::thread;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HMODULE, HWND, LPARAM, POINT, TRUE, FALSE};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen,
};
use windows::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MOUSEINPUT, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, SendInput, VK_A,
    VK_BACK, VK_CONTROL, VK_ESCAPE, VK_RETURN, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow,
    SetWindowDisplayAffinity, SetWindowPos, SetCursorPos, ShowWindow, HWND_TOP, SWP_NOMOVE,
    SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE, WINDOW_DISPLAY_AFFINITY,
};

use crate::config::Scale;

/// 诊断日志总开关：默认关闭（返回 `false`）。
///
/// 仅当环境变量 `DELTA_OBSERVER_DEBUG` 为 `1` / `true` / `yes` / `on` 时启用，
/// 启用后 [`click_client_rect`] / [`move_mouse_to_client`] 才会输出 `[CLICK-DIAG]` /
/// `[MOVE-DIAG]` 坐标细节，以及额外的 `ClientToScreen(hwnd,(0,0))` 探测。
/// 常态运行下不输出，避免把观察日志（observe.log）刷满坐标细节、也不额外调用一次坐标转换。
fn diag_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        match std::env::var("DELTA_OBSERVER_DEBUG") {
            Ok(v) => {
                let v = v.trim().to_ascii_lowercase();
                v == "1" || v == "true" || v == "yes" || v == "on"
            }
            Err(_) => false,
        }
    })
}

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

// ====================== 截图功能已迁移至 `src/screenshot.rs` ======================
// 本文件保留：窗口查找 / 聚焦 / 截图排除（WDA）/ 模拟键鼠 / 提示音。
// 毛玻璃（磨砂半透明）由 `src/platform/dxgi_capture.rs` + `ui/` 下面板实现，本文件不再提供。

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

/// 播放一段内存合成的轻量提示音（观察完成 / 结束时的确认反馈）。
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
fn move_mouse_human(tx: i32, ty: i32) {
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
fn mouse_click_screen(x: i32, y: i32) {
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

/// 把客户区坐标 `(x, y)` 转换为屏幕坐标（供鼠标点击 / 可视化标注）。失败返回 `None`。
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

/// 把客户区矩形 `[x, y, w, h]` 的两个角点都转换为屏幕坐标，返回屏幕矩形；
/// 任一角点转换失败返回 `None`。供神经网络可视化图层把检测框绘制到屏幕对应位置。
pub fn client_rect_to_screen(hwnd: HWND, rect: [i32; 4]) -> Option<[i32; 4]> {
    let (x0, y0) = client_to_screen(hwnd, rect[0], rect[1])?;
    let (x1, y1) = client_to_screen(hwnd, rect[0] + rect[2], rect[1] + rect[3])?;
    Some([x0, y0, x1 - x0, y1 - y0])
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

/// 打开购买界面：在 `buy_area` 的**中央 50×50 区域**内随机取一点点击，使游戏弹出其购买 / 确认界面。
///
/// **仅执行一次点击，不等待、不 ESC、不恢复焦点**——点击后停留在该界面，
/// 由购买阶段（`buy` 模块）轮询等待界面稳定并做重验证，结束后再统一 ESC 返回扫描。
///
/// 坐标契约：`buy_area` 为标定视口（`CALIB_VIEWPORT_W`）下的矩形，由调用方传入**未缩放**原值，
/// 本函数内部按 `scale` 映射到实际客户区，点击落点**严格限制在** `buy_area` 的中央 50×50 子区域内
/// （相对 `buy_area` 左上角：横向 `239..289`、纵向 `25..75`），杜绝越界点击到外部区域。
///
/// 返回是否成功执行（坐标转换失败返回 `false`）。
pub fn open_buy_interface(hwnd: HWND, buy_area: [i32; 4], scale: &Scale) -> bool {
    // 中央 50×50 子区域：相对 buy_area 左上角，标定视口坐标（与 buy_area 同一坐标契约）。
    // 横向 239..289（宽 50）、纵向 25..75（高 50），由总区域尺寸 478×100 推得。
    let central = [buy_area[0] + 239, buy_area[1] + 25, 50, 50];
    let area = scale.rect(central);
    // 在 central 区域内随机取一点（含边界，绝不越出该 50×50 范围）。
    let tx = area[0] + rnd_range(0, area[2].max(1) as i64) as i32;
    let ty = area[1] + rnd_range(0, area[3].max(1) as i64) as i32;
    let (sx, sy) = match client_to_screen(hwnd, tx, ty) {
        Some(p) => p,
        None => return false,
    };
    mouse_click_screen(sx, sy);
    true
}

/// 发送一次 ESC 键（按下 → 抬起），用于购买阶段结束 / 取消后返回扫描阶段。
///
/// 购买阶段无论「价格回升中止」还是「触发购买提示后」，均以 ESC 退出当前界面回到扫描，
/// 由调用方在调用前后负责窗口焦点恢复。
pub fn send_esc() {
    send_vk(VK_ESCAPE, true);
    rnd_sleep_ms(20, 50);
    send_vk(VK_ESCAPE, false);
}

/// 在客户区坐标矩形中心处模拟一次人类左键点击（先人类化移动，再按下/抬起）。
///
/// 坐标契约：`rect` 为相对客户区左上角的 `[x, y, w, h]`，与本程序其余坐标约定一致；
/// 内部经 `client_to_screen` 转换为屏幕坐标后派发 `SendInput`。
/// 购买阶段「减 / 加 / 购买」按钮即经此点击。
/// 返回是否成功（坐标转换失败返回 `false`）。
pub fn click_client_rect(hwnd: HWND, rect: [i32; 4]) -> bool {
    let cx = rect[0] + rect[2] / 2;
    let cy = rect[1] + rect[3] / 2;
    let (sx, sy) = match client_to_screen(hwnd, cx, cy) {
        Some(p) => p,
        None => {
            if diag_enabled() {
                crate::log::write(&format!(
                    "[CLICK-DIAG] 坐标转换失败 client_to_screen 返回 None（鼠标不会移动）：hwnd={:?} rect={:?} 客户区中心=({},{})",
                    hwnd.0, rect, cx, cy
                ));
            }
            return false;
        }
    };
    if diag_enabled() {
        // 仅在排查时额外做一次原点探测，常态运行不消耗该调用。
        let origin = client_to_screen(hwnd, 0, 0);
        crate::log::write(&format!(
            "[CLICK-DIAG] 已派发点击：hwnd={:?} rect={:?} 客户区中心=({},{}) → 屏幕坐标=({},{}) 窗口客户原点={:?}",
            hwnd.0, rect, cx, cy, sx, sy, origin
        ));
    }
    mouse_click_screen(sx, sy);
    true
}

/// 把鼠标移动（人类化轨迹，不点击）到客户区坐标矩形中心。
///
/// 购买阶段「数量已对齐目标」后，把光标移到「购买」按钮中心待命（暂不点击），
/// 由调用方随后统一 ESC 退出。坐标契约同 [`click_client_rect`]。
/// 返回是否成功（坐标转换失败返回 `false`）。
pub fn move_mouse_to_client(hwnd: HWND, rect: [i32; 4]) -> bool {
    let cx = rect[0] + rect[2] / 2;
    let cy = rect[1] + rect[3] / 2;
    let (sx, sy) = match client_to_screen(hwnd, cx, cy) {
        Some(p) => p,
        None => {
            if diag_enabled() {
                crate::log::write(&format!(
                    "[MOVE-DIAG] 坐标转换失败 client_to_screen 返回 None（鼠标不会移动）：hwnd={:?} rect={:?} 客户区中心=({},{})",
                    hwnd.0, rect, cx, cy
                ));
            }
            return false;
        }
    };
    if diag_enabled() {
        let origin = client_to_screen(hwnd, 0, 0);
        crate::log::write(&format!(
            "[MOVE-DIAG] 已派发移动：hwnd={:?} rect={:?} 客户区中心=({},{}) → 屏幕坐标=({},{}) 窗口客户原点={:?}",
            hwnd.0, rect, cx, cy, sx, sy, origin
        ));
    }
    move_mouse_human(sx, sy);
    true
}
