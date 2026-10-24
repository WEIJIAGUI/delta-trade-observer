//! 低级键盘钩子（WH_KEYBOARD_LL）：即使游戏独占键盘焦点，也能捕获 F9（结束观察）、
//! F2（神经网络可视化按住标志）与导游模式的 Enter（确认计划）。
//!
//! 为什么需要它：`global-hotkey`（基于 `RegisterHotKey`）在「本窗口有焦点」时可靠，
//! 但进入激活态时 `start()` 会把键盘焦点交给游戏窗口（`win::focus_window`），
//! 游戏以无边框/全屏原始输入方式持有焦点时，`RegisterHotKey` 的消息**不会**投递到本进程，
//! 导致 F9 失效、无法退出激活态，导游模式的 Enter 确认也无法经 egui 捕获。
//!
//! 低级键盘钩子由系统在按键分发给任何窗口**之前**于本线程消息循环回调，
//! 与焦点归属无关（射击游戏常态下依然生效），作为全局快捷键的可靠兜底。
//! - F9：仅置位原子标志（`take_f9` 取走），并**透传**按键（不影响游戏输入）。
//! - F2：维护「电平」标志（`f2_held` 读取）——按下置位、松开清除，供主循环判断
//!   是否实时渲染神经网络可视化；始终**透传**，不影响游戏输入（游戏仍可正常接收 F2）。
//! - Enter（导游模式）：仅当有待确认计划（`Tour::pending_text` 为 `Some`）时才拦截——
//!   调用 `Tour::confirm` 放行后台线程，并以 `LRESULT(1)` 吞掉该次 Enter，避免其继续下发到游戏
//!   （否则确认计划的回车会同时触发游戏内的回车动作）。无待确认计划时透传，游戏正常接收回车。
//!
//! 注意：绝不能通过 `ViewportCommand::Focus` 抢焦点来让 egui 捕获 Enter——
//! 激活态窗口已被 `MousePassthrough` 标记为 `WS_EX_TRANSPARENT | WS_EX_LAYERED`，
//! 抢为前台窗口会让 DWM 把整块全屏穿透窗口合成为不透明黑，造成「整体黑屏」。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HINSTANCE, LRESULT, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, KBDLLHOOKSTRUCT, SetWindowsHookExW, WH_KEYBOARD_LL, HHOOK,
};

/// F9（结束观察）按键标志：钩子置位，主循环取走（消费式）。
static F9_DOWN: AtomicBool = AtomicBool::new(false);

/// F2（神经网络可视化）按住状态：按下置位、松开清除，供主循环每帧读取。
/// 与 `F9_DOWN` 不同，这是「电平」而非「边沿」标志——可视化需在按住期间持续显示。
static F2_HELD: AtomicBool = AtomicBool::new(false);

/// 当前导游模式协调器引用：由主循环每帧刷新为 `app.tour` 的当前克隆，
/// 以便钩子在 Enter 时判断是否有待确认计划并放行。用 `Mutex` 包裹以支持每帧更新
/// （Re-observe 会重建 `Tour`，需让钩子始终指向最新实例）。
static TOUR_REF: OnceLock<Mutex<Option<crate::Tour>>> = OnceLock::new();

/// 钩子句柄（进程存活期保持，不卸载）。
static HOOK: OnceLock<HHOOK> = OnceLock::new();

/// 安装低级键盘钩子。须在拥有消息循环的线程（主线程）调用一次。
///
/// 失败（极少见）仅影响「焦点被游戏独占时 F9 / 导游确认」的兜底，不影响其它功能。
pub fn install() {
    // 低级钩子必须将 hmod 置 NULL（HINSTANCE(0)）、dwThreadId 置 0（钩取本桌面全部线程）。
    let r = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), HINSTANCE(0), 0) };
    if let Ok(h) = r {
        let _ = HOOK.set(h);
    }
}

/// 主循环每帧调用：把当前 `Tour` 引用刷入钩子（处理 Re-observe 重建 `Tour` 的情况）。
pub fn set_tour(tour: crate::Tour) {
    if let Some(mut cell) = TOUR_REF.get_or_init(|| Mutex::new(None)).lock().ok() {
        *cell = Some(tour);
    }
}

/// 消费式读取 F9 标志（读到即清零）。
pub fn take_f9() -> bool {
    F9_DOWN.swap(false, Ordering::Relaxed)
}

/// 读取 F2 是否正被按住（电平标志，非消费式）。
/// 主循环每帧调用：为真时渲染神经网络可视化，为假时隐藏并跳过推理。
pub fn f2_held() -> bool {
    F2_HELD.load(Ordering::Relaxed)
}

/// 低级键盘钩子回调（unsafe extern "system"，由本线程消息循环调用）。
extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // code < 0 时必须透传。
    if code >= 0 {
        let msg = wparam.0 as u32;
        // lparam 为 *const KBDLLHOOKSTRUCT；vkCode 即虚拟键码。
        let ks = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        match ks.vkCode {
            0x71 => {
                // VK_F2（神经网络可视化）：按下置位、松开清除，均为「电平」维护。
                // 始终透传，不影响游戏输入。
                match msg {
                    0x0100 | 0x0104 => F2_HELD.store(true, Ordering::Relaxed), // WM_KEYDOWN / WM_SYSKEYDOWN
                    0x0101 | 0x0105 => F2_HELD.store(false, Ordering::Relaxed), // WM_KEYUP / WM_SYSKEYUP
                    _ => {}
                }
            }
            0x78 => {
                // VK_F9（结束观察）：仅按下置位，松开忽略；始终透传。
                if msg == 0x0100 || msg == 0x0104 {
                    F9_DOWN.store(true, Ordering::Relaxed);
                }
            }
            0x0D => {
                // VK_RETURN / Enter：仅导游模式有待确认计划时拦截确认。
                if msg == 0x0100 || msg == 0x0104 {
                    if let Some(cell) = TOUR_REF.get() {
                        if let Ok(g) = cell.lock() {
                            if let Some(tour) = g.as_ref() {
                                if tour.pending_text().is_some() {
                                    tour.confirm();
                                    // 吞掉本次 Enter，不继续下发给游戏（避免回车被游戏消费）。
                                    return LRESULT(1);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // 关键：其余按键一律透传，绝不拦截，保证游戏正常接收输入。
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
